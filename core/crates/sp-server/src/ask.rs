//! Answers from the library with a small local model (DESIGN §8). Optional: `--llm GGUF|DIR`.
//!
//!   GET /ask?q=         page; streams the answer from /api/ask with a little JavaScript
//!   GET /api/ask?q=&k=&think=  NDJSON stream, one event per line:
//!   POST /api/ask {"q", "k", "history": [{"q", "a"}, …]}  the same, as a follow-up in a conversation
//!       {"type":"card","id","title","first"}       a first-aid card that fits the question (emergency.rs)
//!       {"type":"status","text":…}                 e.g. loading the model
//!       {"type":"sources","sources":[{n,doc,title,block,snippet}]}
//!       {"type":"text","text":…}                   pieces of the answer
//!       {"type":"reset"}                           discard the text so far
//!       {"type":"done","prompt_tokens":…,"prompt_ms":…,"gen_tokens":…,"gen_ms":…}
//!       {"type":"error","error":…}
//!
//! Fixed pipeline: hybrid search → top k passages (one chunk per article) → answer only from them,
//! citing [n]. The sources are always shown with the answer. One answer at a time: the model has
//! a single context, so further questions wait.

use crate::{AppState, SearchParams, page};
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;
use sp_rag::{Llm, Passage, Piece, Turn};
use sp_search::text::snippet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Passages per answer; each is at most PASSAGE_CHARS (~250 tokens). Eight give the model more to work from
/// (4 was often too little); prompt processing roughly doubles, to ~10-20 s on a phone.
const DEFAULT_K: usize = 8;
const PASSAGE_CHARS: usize = 1300;
const MAX_ANSWER_TOKENS: usize = 600;

/// The configured model, loaded on first use (it is ~1.6 GB; most sessions never ask).
pub struct LlmSlot {
    pub path: Option<PathBuf>,
    loaded: Mutex<Option<Arc<Llm>>>,
}

impl LlmSlot {
    pub fn new(cfg: Option<&Path>) -> Self {
        LlmSlot { path: cfg.and_then(sp_rag::find_gguf), loaded: Mutex::new(None) }
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.lock().unwrap().is_some()
    }

    fn get(&self) -> anyhow::Result<Arc<Llm>> {
        let mut slot = self.loaded.lock().unwrap();
        if let Some(l) = &*slot {
            return Ok(l.clone());
        }
        let Some(p) = &self.path else { anyhow::bail!("no AI model installed") };
        let l = Arc::new(Llm::load(p, sp_rag::default_threads())?);
        *slot = Some(l.clone());
        Ok(l)
    }
}

/// The directory the hub serves for download: the model's own directory (GGUF + licence).
pub fn llm_dir(p: &Path) -> PathBuf {
    if p.is_file() { p.parent().unwrap_or(Path::new(".")).to_path_buf() } else { p.to_path_buf() }
}

#[derive(Deserialize)]
pub struct AskParams {
    #[serde(default)]
    q: String,
    k: Option<usize>,
    /// Let a reasoning model think first (slower; experimental)
    #[serde(default)]
    think: bool,
    /// Earlier turns of the conversation, oldest first (POST only)
    #[serde(default)]
    history: Vec<TurnIn>,
}

#[derive(Deserialize)]
pub struct TurnIn {
    q: String,
    a: String,
}

fn line(v: serde_json::Value) -> Vec<u8> {
    let mut b = v.to_string().into_bytes();
    b.push(b'\n');
    b
}

pub async fn api(State(app): State<AppState>, Query(p): Query<AskParams>) -> Response {
    stream(app, p)
}

/// Same as GET, with the conversation so far: `{"q": …, "history": [{"q": …, "a": …}, …]}`.
pub async fn api_post(State(app): State<AppState>, axum::Json(p): axum::Json<AskParams>) -> Response {
    stream(app, p)
}

fn stream(app: AppState, p: AskParams) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    tokio::task::spawn_blocking(move || {
        let send = |v| tx.blocking_send(line(v)).is_ok();
        if let Err(e) = run(&app, &p, &send) {
            send(json!({"type": "error", "error": format!("{e:#}")}));
        }
    });
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, std::convert::Infallible>(b), rx))
    });
    ([(header::CONTENT_TYPE, "application/x-ndjson"), (header::CACHE_CONTROL, "no-store")], Body::from_stream(stream))
        .into_response()
}

fn run(app: &crate::App, p: &AskParams, send: &dyn Fn(serde_json::Value) -> bool) -> anyhow::Result<()> {
    let q = p.q.trim();
    anyhow::ensure!(!q.is_empty(), "empty question");
    if let Some(card) = crate::emergency::best(q) {
        if !send(card) {
            return Ok(());
        }
    }
    anyhow::ensure!(app.llm.path.is_some(), "no AI model installed");
    let t0 = Instant::now();
    let k = p.k.unwrap_or(DEFAULT_K).clamp(1, 12);
    let history = &p.history[p.history.len().saturating_sub(sp_rag::MAX_HISTORY)..];
    let hits = retrieve(app, q, history, k)?;
    let mut sources = Vec::new();
    let mut passages = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let pack = &app.lib.cols[h.col].searcher.pack;
        let c = pack.chunk(h.chunk);
        let d = pack.get(h.doc)?;
        let html = d.record["html"].as_str().unwrap_or("");
        passages.push(Passage { title: d.title.clone(), text: snippet(html, c.block_first, c.block_last, PASSAGE_CHARS) });
        sources.push(json!({"n": i + 1, "collection": h.collection, "doc": h.doc, "title": d.title, "block": h.block, "snippet": h.snippet}));
    }
    if !send(json!({"type": "sources", "sources": sources})) {
        return Ok(());
    }
    if passages.is_empty() {
        send(json!({"type": "text", "text": "The library passages don't cover this."}));
        send(json!({"type": "done", "total_ms": t0.elapsed().as_secs_f64() * 1e3}));
        return Ok(());
    }
    if !app.llm.is_loaded() && !send(json!({"type": "status", "text": "Loading the AI model…"})) {
        return Ok(());
    }
    let llm = app.llm.get()?;
    send(json!({"type": "status", "text": format!("Reading {} passages…", passages.len())}));
    let turns: Vec<Turn> = history.iter().map(|t| Turn { question: t.q.clone(), answer: t.a.clone() }).collect();
    let stats = llm.answer(q, &passages, &turns, MAX_ANSWER_TOKENS, p.think, |piece| match piece {
        Piece::Text(t) => send(json!({"type": "text", "text": t})),
        Piece::Reset => send(json!({"type": "reset"})),
        Piece::Thinking => send(json!({"type": "status", "text": "Thinking…"})),
    })?;
    send(json!({
        "type": "done", "model": llm.name,
        "prompt_tokens": stats.prompt_tokens, "prompt_ms": stats.prompt_ms.round(),
        "gen_tokens": stats.gen_tokens, "gen_ms": stats.gen_ms.round(),
        "total_ms": (t0.elapsed().as_secs_f64() * 1e3).round(),
    }));
    Ok(())
}

/// Passages for a question. A follow-up ("and if it is a child?") often means little alone, so it
/// is searched together with the last two questions; its own search adds one passage in case the
/// topic changed. One passage per article.
fn retrieve(app: &crate::App, q: &str, history: &[TurnIn], k: usize) -> anyhow::Result<Vec<crate::HitOut>> {
    // English passages only (DESIGN §1): older packs still hold zh/ja articles
    let search = |text: &str| app.search(&SearchParams { q: text.to_string(), mode: None, lang: Some("en".into()), c: None, k: Some(k) });
    let own = search(q)?.hits;
    if history.is_empty() {
        return Ok(own);
    }
    let earlier: Vec<&str> = history[history.len().saturating_sub(2)..].iter().map(|t| t.q.trim()).collect();
    let joint = search(&format!("{} {q}", earlier.join(" ")))?.hits;
    // the joint search fills all but the last slot; the follow-up's own best hit takes that one
    let mut out: Vec<crate::HitOut> = Vec::new();
    let add = |out: &mut Vec<crate::HitOut>, h: crate::HitOut| {
        if !out.iter().any(|o| o.doc == h.doc && o.col == h.col) {
            out.push(h);
        }
    };
    let mut joint = joint.into_iter();
    while out.len() + 1 < k {
        match joint.next() {
            Some(h) => add(&mut out, h),
            None => break,
        }
    }
    for h in own.into_iter().chain(joint) {
        if out.len() >= k {
            break;
        }
        add(&mut out, h);
    }
    Ok(out)
}

pub async fn page(State(app): State<AppState>, Query(p): Query<AskParams>) -> Html<String> {
    let q = p.q.trim();
    // Without JavaScript the form still works: each question loads /ask?q=… (no follow-ups).
    let form = format!(
        "<form id=\"askform\" class=\"search\" action=\"/ask\"><div class=\"row\"><input id=\"qin\" type=\"search\" name=\"q\" \
placeholder=\"Ask a question…\" autocomplete=\"off\"{}><button id=\"askbtn\">Ask</button></div></form>",
        if q.is_empty() { " autofocus" } else { "" }
    );
    let body = if app.llm.path.is_none() {
        format!("<h1>Ask</h1>{}{form}<p>No AI model is installed. Search still works fully. \
On the phone, download <b>AI answers</b> from the Content tab.</p><style>{}</style>", crate::emergency::banner(q), crate::emergency::CSS)
    } else {
        format!(
            "<h1>Ask</h1><div id=\"chat\" data-q=\"{}\"></div>{form}\
<p id=\"newtopic\" hidden><button type=\"button\">New topic</button></p>\
<p class=\"warn\">Answers are written by a small AI on this device, only from the library's articles, \
and it remembers the conversation for follow-up questions. It can be wrong: check the sources, \
especially doses and safety steps.</p><style>{ASK_CSS}{}</style><script>{ASK_JS}</script>",
            page::esc(q),
            crate::emergency::CSS
        )
    };
    Html(page::plain(if q.is_empty() { "Ask" } else { q }, &body))
}

const ASK_CSS: &str = ".turn{margin:16px 0;padding-bottom:12px;border-bottom:1px solid var(--border)}\
.turn .q{display:inline-block;font-weight:600;background:var(--card);border:1px solid var(--border);\
border-radius:12px;padding:6px 12px} .turn .a{white-space:pre-wrap;font-size:18px;margin:10px 0 6px;min-height:1.2em}\
.turn .a a{font-size:14px;vertical-align:super} .turn .st{color:var(--muted);font-size:14px}\
.turn details summary{color:var(--muted);font-size:14px;cursor:pointer} .turn ol{margin:6px 0}\
.turn li{margin-bottom:6px} .turn .snip{font-size:14px;color:var(--muted)}\
.warn{font-size:13px;color:var(--muted);border-left:3px solid #d9a400;padding-left:8px;margin-top:18px}\
#newtopic button{font-size:15px;padding:6px 12px;border-radius:8px;border:1px solid var(--border);\
background:var(--card);color:var(--fg)}";

// A conversation: each question is POSTed to /api/ask with the earlier turns, and the answer
// streams in. [n] in an answer links to that turn's cited passage. The conversation is kept in
// sessionStorage, so going to a source and back restores it instead of asking again.
const ASK_JS: &str = r#"
(() => {
  const chat = document.getElementById('chat'); if (!chat) return;
  const form = document.getElementById('askform'), qin = document.getElementById('qin');
  const btn = document.getElementById('askbtn'), newp = document.getElementById('newtopic');
  const KEY = 'sp-ask';
  const esc = s => s.replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
  let turns = [], busy = false;

  const cite = (text, src) => esc(text).replace(/\*\*(.+?)\*\*/g, '<b>$1</b>').replace(/\[(\d+)\]/g, (m, n) => {
    const s = src[n - 1]; return s ? `<a href="/doc/${s.collection}/${s.doc}#${s.block}">[${n}]</a>` : m;
  });
  const html = (t, last) => `<div class="q">${esc(t.q)}</div>` +
    (t.card ? `<div class="fa-banner"><div class="fa-k">First aid</div><a href="/emergency#${esc(t.card.id)}"><b>${esc(t.card.title)}</b> · ${esc(t.card.first)}</a></div>` : '') +
    `<div class="a">${cite(t.a, t.sources)}</div>` +
    (t.status ? `<div class="st">${esc(t.status)}</div>` : '') +
    (t.sources.length ? `<details${last ? ' open' : ''}><summary>Sources (${t.sources.length})</summary><ol>` +
      t.sources.map(s => `<li><a href="/doc/${s.collection}/${s.doc}#${s.block}">${esc(s.title)}</a><div class="snip">${esc(s.snippet)}</div></li>`).join('') +
      '</ol></details>' : '') +
    (t.timing ? `<div class="timing">${esc(t.timing)}</div>` : '');
  const els = [];
  const draw = i => { els[i].innerHTML = html(turns[i], i === turns.length - 1); };
  const drawAll = () => {
    chat.innerHTML = ''; els.length = 0;
    turns.forEach((t, i) => { const d = document.createElement('div'); d.className = 'turn'; chat.append(d); els.push(d); draw(i); });
    newp.hidden = !turns.length;
    qin.placeholder = turns.length ? 'Ask a follow-up…' : 'Ask a question…';
  };
  const save = () => sessionStorage.setItem(KEY, JSON.stringify(turns));

  async function ask(q) {
    const history = turns.filter(t => t.a && !t.failed).map(t => ({q: t.q, a: t.a}));
    turns.push({q, a: '', sources: [], status: 'Searching…', timing: ''});
    const i = turns.length - 1, t = turns[i];
    drawAll(); els[i].scrollIntoView({block: 'start'});
    busy = true; qin.value = ''; qin.disabled = btn.disabled = true;
    const on = e => {
      if (e.type === 'card') t.card = e;
      else if (e.type === 'status') t.status = e.text;
      else if (e.type === 'sources') { t.sources = e.sources; t.status = e.sources.length ? 'Thinking…' : ''; }
      else if (e.type === 'text') { t.status = ''; t.a += e.text; }
      else if (e.type === 'reset') t.a = '';
      else if (e.type === 'done') {
        t.status = '';
        if (e.gen_tokens !== undefined) t.timing =
          `${e.model} · ${(e.total_ms / 1000).toFixed(1)} s · read ${e.prompt_tokens} tokens in ${(e.prompt_ms / 1000).toFixed(1)} s · ` +
          `wrote ${e.gen_tokens} tokens at ${(e.gen_tokens / Math.max(e.gen_ms, 1) * 1000).toFixed(1)}/s`;
      } else if (e.type === 'error') { t.status = 'Error: ' + e.error; t.failed = true; }
      draw(i);
    };
    try {
      const r = await fetch('/api/ask', {method: 'POST', headers: {'Content-Type': 'application/json'},
                                         body: JSON.stringify({q, history})});
      if (!r.ok) throw new Error(await r.text());
      const rd = r.body.getReader(), dec = new TextDecoder();
      let buf = '';
      for (;;) {
        const {done, value} = await rd.read(); if (done) break;
        buf += dec.decode(value, {stream: true});
        let j; while ((j = buf.indexOf('\n')) >= 0) { const l = buf.slice(0, j); buf = buf.slice(j + 1); if (l) on(JSON.parse(l)); }
      }
    } catch (err) { t.status = 'Error: ' + err.message; t.failed = true; draw(i); }
    busy = false; qin.disabled = btn.disabled = false; qin.focus();
    save();
  }

  form.addEventListener('submit', e => {
    e.preventDefault();
    const q = qin.value.trim(); if (!q || busy) return;
    if (!turns.length) history.replaceState(null, '', '/ask?q=' + encodeURIComponent(q));
    ask(q);
  });
  newp.querySelector('button').addEventListener('click', () => {
    if (busy) return;
    turns = []; save(); history.replaceState(null, '', '/ask'); drawAll(); qin.focus();
  });

  const q0 = chat.dataset.q;
  let saved = [];
  try { saved = JSON.parse(sessionStorage.getItem(KEY) || '[]'); } catch (_) {}
  if (saved.length && saved[0].q === q0) { turns = saved; drawAll(); }
  else if (q0) ask(q0);
  else drawAll();
})();
"#;
