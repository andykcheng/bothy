//! Server-rendered HTML pages. Plain HTML + CSS so it works on any phone browser on the hub's Wi-Fi.

use crate::{App, SearchOut, SearchParams};
use sp_pack::reader::Doc;

pub const CSS: &str = r#"
@font-face { font-family:Inter; font-weight:400; font-display:swap; src:url(/fonts/Inter-Regular.woff2) format("woff2"); }
@font-face { font-family:Inter; font-weight:500 600; font-display:swap; src:url(/fonts/Inter-SemiBold.woff2) format("woff2"); }
@font-face { font-family:Inter; font-weight:700 900; font-display:swap; src:url(/fonts/Inter-Bold.woff2) format("woff2"); }
:root { --bg:#F6F4EF; --surface:#FFFFFF; --fg:#1E2421; --muted:#5F6B64; --accent:#2F6B4F; --accent-soft:#E3EEE7;
  --border:#E2DED5; --hl:#FBEFC4; --shadow:0 1px 2px rgba(30,36,33,.06), 0 2px 8px rgba(30,36,33,.05); --radius:14px; }
@media (prefers-color-scheme: dark) {
  :root { --bg:#151A1E; --surface:#1E252A; --fg:#E6EAE7; --muted:#97A39C; --accent:#7CC4A0; --accent-soft:#22342C;
    --border:#2C353B; --hl:#4A4220; --shadow:none; }
}
* { box-sizing: border-box; }
body { margin:0; background:var(--bg); color:var(--fg);
  font: 17px/1.6 Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; -webkit-font-smoothing:antialiased; }
a { color:var(--accent); text-decoration:none; } a:hover { text-decoration:underline; }
header { position:sticky; top:0; z-index:5; padding:10px 16px; background:color-mix(in srgb, var(--bg) 88%, transparent);
  backdrop-filter:blur(8px); border-bottom:1px solid var(--border); display:flex; gap:14px; align-items:center; flex-wrap:wrap; }
header .brand { font-weight:700; color:var(--fg); letter-spacing:-.01em; display:flex; align-items:center; gap:8px; }
header .brand::before { content:""; width:22px; height:22px; border-radius:7px; background:var(--accent);
  -webkit-mask: radial-gradient(circle at 50% 38%, transparent 0 4px, #000 4.5px); }
header nav { display:flex; gap:4px; max-width:100%; overflow-x:auto; scrollbar-width:none; }
header nav a { white-space:nowrap; }
header nav a { color:var(--muted); padding:6px 10px; border-radius:999px; font-size:15px; }
header nav a.fa { color:#C62828; font-weight:600; }
header nav a:hover { background:var(--accent-soft); color:var(--accent); text-decoration:none; }
form.search { flex:1; min-width:260px; }
form.search .row { display:flex; gap:6px; }
form.search .opts { display:flex; gap:6px; margin-top:6px; flex-wrap:wrap; }
form.search .opts:empty { display:none; }
form.search input[type=search] { flex:1; min-width:0; font:inherit; padding:10px 16px; border:1px solid var(--border);
  border-radius:999px; background:var(--surface); color:var(--fg); box-shadow:var(--shadow); outline:none; }
form.search input[type=search]:focus { border-color:var(--accent); box-shadow:0 0 0 3px var(--accent-soft); }
form.search select { font:inherit; font-size:13px; padding:4px 10px; max-width:60%; border-radius:999px; border:1px solid var(--border);
  background:var(--surface); color:var(--fg); }
form.search button, .btn { font:inherit; font-size:15px; font-weight:600; padding:9px 18px; border-radius:999px; border:0;
  background:var(--accent); color:#fff; cursor:pointer; }
@media (prefers-color-scheme: dark) { form.search button, .btn { color:#0E1A14; } }
main { max-width:880px; margin:0 auto; padding:18px 16px 40px; }
.hero { text-align:center; padding:9vh 0 20px; }
.hero h1 { font-size:34px; letter-spacing:-.02em; margin:0 0 4px; }
.hero p { color:var(--muted); margin:0; }
.hero form.search { max-width:640px; margin:22px auto 10px; }
.section-title { font-size:13px; font-weight:600; text-transform:uppercase; letter-spacing:.06em; color:var(--muted); margin:28px 0 10px; }
.cards { display:grid; grid-template-columns:repeat(auto-fill, minmax(240px, 1fr)); gap:12px; }
.card { display:block; background:var(--surface); border:1px solid var(--border); border-radius:var(--radius); padding:14px 16px;
  box-shadow:var(--shadow); color:var(--fg); }
a.card:hover { border-color:var(--accent); text-decoration:none; }
.card .t { font-weight:600; } .card .d { color:var(--muted); font-size:14px; margin-top:2px; }
.hit { background:var(--surface); border:1px solid var(--border); border-radius:var(--radius); padding:12px 16px; margin:10px 0;
  box-shadow:var(--shadow); }
.hit .t { font-size:18px; font-weight:600; }
.meta { color:var(--muted); font-size:13px; margin-top:2px; }
.badge { display:inline-block; max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; vertical-align:bottom; font-size:12px; font-weight:500; padding:1px 8px; border-radius:999px; background:var(--accent-soft);
  color:var(--accent); margin-right:4px; }
.snip { margin-top:4px; color:var(--fg); font-size:15px; }
article { background:var(--surface); border:1px solid var(--border); border-radius:var(--radius); padding:8px 22px 18px;
  box-shadow:var(--shadow); }
article h1 { letter-spacing:-.015em; line-height:1.25; }
article img { max-width:100%; height:auto; border-radius:8px; }
article img.math { vertical-align:middle; background:#fff; border-radius:0; }
article table { border-collapse:collapse; display:block; overflow-x:auto; max-width:100%; font-size:15px; }
article td, article th { border:1px solid var(--border); padding:4px 8px; vertical-align:top; text-align:left; }
article figure { margin:14px 0; } article figcaption { color:var(--muted); font-size:14px; }
article :target { background:var(--hl); border-radius:6px; }
nav.book { display:flex; justify-content:space-between; align-items:center; gap:8px; margin:12px 0; font-size:15px; }
nav.book a { padding:6px 14px; border-radius:999px; background:var(--accent-soft); font-weight:500; }
nav.book a:hover { text-decoration:none; filter:brightness(.97); }
nav.book span { width:90px; }
nav.crumbs { font-size:14px; color:var(--muted); margin:4px 0 8px; } nav.crumbs a { color:var(--muted); }
.lede { color:var(--muted); margin-top:-4px; }
.cards.sections { margin-bottom:18px; }
.az, .pages { display:flex; flex-wrap:wrap; gap:6px; margin:10px 0 14px; }
.az a, .pages a { min-width:34px; text-align:center; padding:6px 8px; border:1px solid var(--border); border-radius:10px; text-decoration:none; background:var(--surface); }
.az a.on, .pages a.on { background:var(--accent); color:#fff; border-color:var(--accent); }
ul.browse { list-style:none; padding:0; margin:0; columns:2 280px; column-gap:28px; }
ul.browse li { padding:6px 0; border-bottom:1px solid var(--border); break-inside:avoid; }
ul.toc { padding-left:0; list-style:none; } ul.toc li { padding:7px 0; border-bottom:1px solid var(--border); }
footer.attr { margin-top:18px; color:var(--muted); font-size:13px; }
.timing { color:var(--muted); font-size:12px; margin-top:16px; }
.grid3 { display:grid; grid-template-columns:repeat(3, minmax(0, 1fr)); gap:14px; }
@media (max-width:640px) { .grid3 { gap:8px; } .tile .body { padding:6px 8px; } .tile .t { font-size:14px; line-height:1.3; } .tile .d { font-size:12px; } }
.tile { display:block; background:var(--surface); border:1px solid var(--border); border-radius:var(--radius); overflow:hidden;
  box-shadow:var(--shadow); color:var(--fg); }
a.tile:hover { border-color:var(--accent); text-decoration:none; }
.tile .cover { aspect-ratio:4/3; background:var(--accent-soft) center/cover no-repeat; }
.tile .body { padding:10px 12px; } .tile .t { font-weight:600; } .tile .d { color:var(--muted); font-size:13px; }
.chips { display:flex; flex-wrap:wrap; gap:8px; }
.chips a { padding:6px 12px; border-radius:999px; border:1px solid var(--border); background:var(--surface); color:var(--fg); font-size:14px; }
.chips a:hover { border-color:var(--accent); text-decoration:none; } .chips a span { color:var(--muted); font-size:12px; }
.bgrid { display:grid; grid-template-columns:repeat(auto-fill, minmax(104px, 1fr)); gap:16px 12px; margin:10px 0 18px; }
.btile { min-width:0; } .btile > a:first-child { display:block; color:var(--fg); } .btile > a:first-child:hover { text-decoration:none; }
.btile a.dl { display:inline-block; font-size:12px; font-weight:600; margin-top:2px; }
.btile .bcover { aspect-ratio:2/3; border-radius:6px; background:var(--accent-soft) center/cover no-repeat; box-shadow:var(--shadow);
  border:1px solid var(--border); display:flex; align-items:flex-end; padding:8px; overflow:hidden; }
.btile .bcover span { color:var(--accent); font-weight:600; font-size:13px; line-height:1.25; }
.btile .bcover.img span { display:none; }
.btile .t { font-size:13px; font-weight:600; line-height:1.3; margin-top:6px; display:-webkit-box; -webkit-line-clamp:2; -webkit-box-orient:vertical; overflow:hidden; }
.btile .d { font-size:12px; color:var(--muted); white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
"#;

/// The stylesheet's address, with a hash of its content: browsers cache /style.css, and a new server
/// version must not be shown with the old styles.
pub fn css_href() -> &'static str {
    static HREF: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HREF.get_or_init(|| format!("/style.css?v={}", &blake3::hash(CSS.as_bytes()).to_hex()[..10]))
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub(crate) fn shell(title: &str, header: &str, body: &str, lang: &str) -> String {
    format!(
        "<!DOCTYPE html><html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{}</title><link rel=\"stylesheet\" href=\"{}\"></head><body>{header}<main>{body}</main></body></html>",
        esc(title),
        css_href()
    )
}

/// A page with just the site header, for pages outside the library (e.g. games).
pub fn plain(title: &str, body: &str) -> String {
    shell(title, "<header><a class=\"brand\" href=\"/\">Bothy</a></header>", body, "en")
}

fn search_form(app: &App, q: &str, mode: &str, col: &str) -> String {
    let opt = |v: &str, label: &str, cur: &str| {
        format!("<option value=\"{v}\"{}>{}</option>", if v == cur { " selected" } else { "" }, esc(label))
    };
    let modes = if app.embedder.is_some() && app.lib.has_dense() {
        format!("<select name=\"mode\">{}{}{}</select>",
            opt("hybrid", "Best match", mode), opt("bm25", "Keywords", mode), opt("dense", "Meaning", mode))
    } else {
        String::new()
    };
    let cols = if app.lib.cols.len() > 1 {
        let mut o = opt("", "Everything", col);
        for c in &app.lib.cols {
            o.push_str(&opt(&c.id, &c.title, col));
        }
        format!("<select name=\"c\">{o}</select>")
    } else {
        String::new()
    };
    format!(
        "<form class=\"search\" action=\"/search\"><div class=\"row\"><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Search the library…\" autofocus>\
<button>Search</button></div><div class=\"opts\">{modes}{cols}</div></form>",
        esc(q)
    )
}

fn nav(app: &App) -> String {
    let mut n = String::from("<nav><a class=\"fa\" href=\"/emergency\">First aid</a>");
    if app.llm.path.is_some() { n.push_str("<a href=\"/ask\">Ask</a>"); }
    if app.books.is_some() || app.installed.is_some() { n.push_str("<a href=\"/books\">Books</a>"); }
    if app.maps.is_some() { n.push_str("<a href=\"/map\">Map</a>"); }
    if app.games.is_some() { n.push_str("<a href=\"/games\">Games</a>"); }
    if app.chat.is_some() { n.push_str("<a href=\"/chat\">Chat</a>"); }
    if app.manage { n.push_str("<a href=\"/manage\">Manage</a>"); }
    n.push_str("</nav>");
    n
}

pub(crate) fn header(app: &App, q: &str, mode: &str, col: &str) -> String {
    format!("<header><a class=\"brand\" href=\"/\">Bothy</a>{}{}</header>", search_form(app, q, mode, col), nav(app))
}

pub fn home(app: &App) -> String {
    let mut body = String::from("<div class=\"hero\"><h1>Bothy</h1>");
    if app.lib.cols.is_empty() {
        body.push_str("<p>No library collections are installed here.</p></div>");
    } else {
        body.push_str(&format!("<p>{} articles · works offline</p>{}</div>", fmt_n(app.lib.doc_count()), search_form(app, "", "hybrid", "")));
        body.push_str("<div class=\"section-title\">Library</div><div class=\"cards\">");
        for c in &app.lib.cols {
            body.push_str(&format!(
                "<a class=\"card\" href=\"/browse/{}\"><div class=\"t\">{}</div><div class=\"d\">{}{}{} {}</div></a>",
                c.id,
                esc(&c.title),
                esc(&c.description),
                if c.description.is_empty() { "" } else { " · " },
                fmt_n(c.searcher.pack.doc_count()),
                if c.kind == "books" { "chapters" } else { "articles" }
            ));
        }
        body.push_str("</div>");
    }
    let mut more = String::new();
    let mut card = |href: &str, t: &str, d: &str| more.push_str(&format!("<a class=\"card\" href=\"{href}\"><div class=\"t\">{t}</div><div class=\"d\">{d}</div></a>"));
    card("/emergency", "<span style=\"color:#C62828\">First aid</span>", "CPR, choking, bleeding, burns, poisoning and more: what to do, step by step");
    if app.llm.path.is_some() { card("/ask", "Ask a question", "Answers written from the library by an offline AI"); }
    if app.books.is_some() || app.installed.is_some() { card("/books", "Books", "Read classics and old manuals in the browser, or download them"); }
    if app.maps.is_some() { card("/map", "Offline map", "Streets, places and search"); }
    if app.games.is_some() { card("/games", "Games", "Play in the browser, no internet needed"); }
    if app.chat.is_some() { card("/chat", "Chat", "Talk with everyone on this Wi-Fi, or privately with one person"); }
    if app.manage { card("/manage", "Manage", "Add maps, books and more to this hub (needs the admin password)"); }
    if !more.is_empty() {
        body.push_str(&format!("<div class=\"section-title\">More</div><div class=\"cards\">{more}</div>"));
    }
    if let Some(size) = app.apk.as_ref().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()) {
        // Android only (there is no iPhone app; everything above works in Safari)
        body.push_str(&format!(
            "<div class=\"cards\" id=\"getapp\" hidden><a class=\"card\" href=\"/app.apk\"><div class=\"t\">Get the app</div>\
<div class=\"d\">Bothy for Android · {} MB · keep all this on your phone, and download content from here</div></a></div>\
<script>if(/Android/i.test(navigator.userAgent))document.getElementById('getapp').hidden=false</script>",
            size.div_ceil(1 << 20)
        ));
    }
    shell("Bothy", "", &body, "en")
}

pub(crate) fn fmt_n(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn results(app: &App, p: &SearchParams, out: &SearchOut) -> String {
    let col = p.c.as_deref().unwrap_or("");
    let mut body = crate::emergency::banner(&out.query);
    if !body.is_empty() {
        body.push_str(&format!("<style>{}</style>", crate::emergency::CSS));
    }
    if app.llm.path.is_some() && !out.query.is_empty() {
        body.push_str(&format!(
            "<p><a href=\"/ask?q={}\">Ask the AI: “{}” →</a></p>",
            url_encode(&out.query),
            esc(&out.query)
        ));
    }
    if out.hits.is_empty() && !out.query.is_empty() {
        body.push_str("<p>No results.</p>");
    }
    for h in &out.hits {
        body.push_str(&format!(
            "<div class=\"hit\"><a class=\"t\" href=\"/doc/{}/{}#{}\">{}</a><div class=\"meta\"><span class=\"badge\">{}</span></div><div class=\"snip\">{}</div></div>",
            h.collection, h.doc, h.block, esc(&h.title), esc(&app.lib.cols[h.col].title), esc(&h.snippet)
        ));
    }
    body.push_str(&format!(
        "<div class=\"timing\">{} · embed {:.0} ms · search {:.1} ms</div>",
        out.mode, out.embed_ms, out.search_ms
    ));
    shell(&format!("{} – Bothy", out.query), &header(app, &out.query, out.mode, col), &body, "en")
}

pub(crate) fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Rewrites stored `img:` / `wiki:` references to server URLs. A `wiki:` link that no installed collection can open
/// (a person, a place, anything left out of the packs) becomes plain text; installing a bigger pack brings it back.
fn render_html(app: &App, ci: usize, html: &str, col: &str, source: u8) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<a href=\"wiki:") {
        out.push_str(&rest[..i]);
        const OPEN: &str = "<a href=\"wiki:";
        let tail = &rest[i + OPEN.len()..];
        let (Some(q), Some(close)) = (tail.find('"'), tail.find("</a>")) else {
            out.push_str(OPEN);
            rest = tail;
            continue;
        };
        let target = &tail[..q];
        let path = target.split('#').next().unwrap_or("");
        let open_end = tail[q..].find('>').map(|e| q + e + 1).unwrap_or(q);
        if path.is_empty() || open_end > close || app.lib.lookup_title(&link_title(path), Some(ci)).is_some() {
            out.push_str(&format!("<a href=\"/link/{col}/{source}/{target}"));
            rest = &tail[q..];
        } else {
            out.push_str(&tail[open_end..close]);
            rest = &tail[close + 4..];
        }
    }
    out.push_str(rest);
    out.replace("src=\"img:", "src=\"/img/")
}

/// The title a stored link path names: percent-decoded, `&amp;` unescaped (lookup normalises case and `_`).
fn link_title(path: &str) -> String {
    let b = path.as_bytes();
    let mut v = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(x) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                v.push(x);
                i += 3;
                continue;
            }
        }
        v.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&v).replace("&amp;", "&")
}

/// " Written by asker (question), a and b (answers)." for docs that list their authors (Stack Exchange; the
/// licence asks for names and profile links).
fn authors(r: &serde_json::Value) -> String {
    let Some(list) = r["authors"].as_array().filter(|l| !l.is_empty()) else { return String::new() };
    let name = |a: &serde_json::Value| {
        let n = esc(a["name"].as_str().unwrap_or("?"));
        match a["url"].as_str() {
            Some(u) => format!("<a href=\"{}\">{n}</a>", esc(u)),
            None => n,
        }
    };
    let join = |v: Vec<String>| match v.len() {
        0 => String::new(),
        1 => v[0].clone(),
        n => format!("{} and {}", v[..n - 1].join(", "), v[n - 1]),
    };
    let mut parts = Vec::new();
    for (role, one, many) in [("question", "question", "question"), ("answer", "answer", "answers")] {
        let v: Vec<String> = list.iter().filter(|a| a["role"] == role).map(name).collect();
        if !v.is_empty() {
            parts.push(format!("{} ({})", join(v.clone()), if v.len() == 1 { one } else { many }));
        }
    }
    if parts.is_empty() { String::new() } else { format!(" Written by {}.", parts.join("; ")) }
}

pub fn doc(app: &App, ci: usize, d: &Doc) -> String {
    let r = &d.record;
    let col = &app.lib.cols[ci];
    let pack = &col.searcher.pack;
    let src_idx = pack.entry(d.num).source;
    let src_meta = &pack.manifest["sources"][src_idx as usize];
    let s = |k: &str| r[k].as_str().unwrap_or("");
    let url = s("url");
    let attribution = format!(
        "From <i>{}</i>{}. Revision of {}. Text licensed under {}. Images may carry their own licences.",
        esc(src_meta["title"].as_str().unwrap_or(&d.source)),
        if url.is_empty() { String::new() } else { format!(": <a href=\"{}\">{}</a>", esc(url), esc(url)) },
        esc(if s("rev_date").is_empty() { "unknown date" } else { s("rev_date") }),
        esc(s("license")),
    ) + &authors(r);
    let body = format!(
        "<article><h1>{}</h1>{}</article><footer class=\"attr\">{}</footer>",
        esc(&d.title),
        render_html(app, ci, s("html"), &col.id, src_idx),
        attribution
    );
    shell(&d.title, &header(app, "", "hybrid", ""), &body, d.lang)
}

pub fn not_found(path: &str) -> String {
    shell("Not found", "", &format!("<p>No article for <b>{}</b> in the library. <a href=\"/\">Search</a></p>", esc(path)), "en")
}

#[cfg(test)]
mod tests {
    #[test]
    fn authors_line() {
        let r = serde_json::json!({"authors": [
            {"name": "noobaker", "role": "question", "url": "https://x/users/1/noobaker"},
            {"name": "rumtscho", "role": "answer"},
            {"name": "a<b", "role": "answer"},
            {"name": "c", "role": "answer"},
        ]});
        assert_eq!(
            super::authors(&r),
            " Written by <a href=\"https://x/users/1/noobaker\">noobaker</a> (question); rumtscho, a&lt;b and c (answers)."
        );
        assert_eq!(super::authors(&serde_json::json!({})), "");
    }
}
