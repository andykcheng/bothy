//! Chat for people on the same Wi-Fi (optional, `--chat DIR`): one room for everyone plus private
//! messages, from any browser (the hub's Wi-Fi, or a phone sharing its hotspot). Messages go through
//! this server, which keeps them on disk, so people who join late or drop out catch up.
//!
//!   GET  /chat                                  the chat page (needs JavaScript)
//!   POST /api/chat/join  {id, token, name}      first visit or rename -> {"people": […]}
//!   POST /api/chat/send  {id, token, to?, text, lat?, lon?} -> {"seq": N}; `to` = a person's id for a
//!                        private message; lat/lon make it a location (the text is then optional)
//!   POST /api/chat/upload?id=&token=&to=&kind=image|audio&text=   body: the file -> {"seq": N}
//!        a photo (JPEG/PNG/WebP/GIF, ≤ 4 MB; the page shrinks photos first) or a voice clip (WebM/Ogg/MP4/
//!        MP3/WAV, ≤ 3 MB); the type is checked from the bytes. Files live in DIR/files/, oldest removed
//!        past 256 MB in all.
//!   GET  /api/chat/file/{name}?id=&token=        a file, for people who can see its message
//!   GET  /api/chat/poll?id=&token=&since=&wait=1
//!        -> {"seq", "messages": [{seq, ts, from, name, to, text, kind, file, lat, lon}…],
//!            "people": [{id, name, online}…]}
//!        messages after `since` that this person may see; with wait=1 it waits up to 25 s for one.
//!
//! People are a random id the browser makes, proven by a secret token (only its hash is stored): the
//! first join claims the id. Private messages are shown only to the two people, but they are stored on
//! this server like everything else, so whoever runs it can read them (the page says so).

use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const KEEP: usize = 3000; // messages kept in memory (the file keeps everything)
const MAX_TEXT: usize = 2000;
const MAX_NAME: usize = 32;
const ONLINE: Duration = Duration::from_secs(60);
const WAIT: Duration = Duration::from_secs(25);
const MAX_IMAGE: usize = 4 << 20;
const MAX_AUDIO: usize = 3 << 20;
const FILES_CAP: u64 = 256 << 20;

#[derive(Clone, Serialize, Deserialize)]
pub struct Msg {
    pub seq: u64,
    pub ts: u64,
    /// sender's id; empty for notices from the server ("Ann joined")
    pub from: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    pub text: String,
    /// "image", "audio" or "location"; none for plain text
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// the attachment's name under files/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
}

impl Msg {
    fn visible_to(&self, id: &str) -> bool {
        self.to.is_none() || self.from == id || self.to.as_deref() == Some(id)
    }
}

/// The file type from its first bytes: (extension, MIME type, kind).
pub fn sniff(b: &[u8]) -> Option<(&'static str, &'static str, &'static str)> {
    let at = |i: usize, m: &[u8]| b.len() >= i + m.len() && &b[i..i + m.len()] == m;
    Some(if at(0, &[0xFF, 0xD8, 0xFF]) {
        ("jpg", "image/jpeg", "image")
    } else if at(0, b"\x89PNG") {
        ("png", "image/png", "image")
    } else if at(0, b"RIFF") && at(8, b"WEBP") {
        ("webp", "image/webp", "image")
    } else if at(0, b"GIF8") {
        ("gif", "image/gif", "image")
    } else if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        ("webm", "audio/webm", "audio")
    } else if at(0, b"OggS") {
        ("ogg", "audio/ogg", "audio")
    } else if at(4, b"ftyp") {
        ("m4a", "audio/mp4", "audio")
    } else if at(0, b"ID3") || at(0, &[0xFF, 0xFB]) || at(0, &[0xFF, 0xF3]) {
        ("mp3", "audio/mpeg", "audio")
    } else if at(0, b"RIFF") && at(8, b"WAVE") {
        ("wav", "audio/wav", "audio")
    } else {
        return None;
    })
}

#[derive(Serialize, Deserialize)]
struct Person {
    name: String,
    token: String, // blake3 of the token
    #[serde(skip)]
    seen: Option<Instant>,
    #[serde(skip)]
    sent: VecDeque<Instant>,
}

struct Inner {
    msgs: VecDeque<Msg>,
    next: u64,
    people: HashMap<String, Person>,
    /// bytes in files/
    files: u64,
}

pub struct Chat {
    dir: PathBuf,
    inner: Mutex<Inner>,
    new: tokio::sync::Notify,
    /// the HTTPS port, once it listens (tls.rs): the page sends people there for the mic and GPS
    https: std::sync::atomic::AtomicU16,
}

#[derive(Debug, PartialEq)]
pub enum ChatError {
    BadRequest(&'static str),
    Forbidden,
    TooFast,
    /// a file removed to save space
    Gone,
}

pub enum Attach {
    None,
    Location(f64, f64),
    /// kind wanted ("image" or "audio") and the bytes
    File(&'static str, Vec<u8>),
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn valid_id(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn clean(s: &str, max: usize) -> String {
    // no control characters except newlines; at most `max` characters
    s.trim().chars().filter(|c| *c == '\n' || !c.is_control()).take(max).collect::<String>().trim().to_string()
}

impl Chat {
    pub fn open(dir: &Path) -> anyhow::Result<Chat> {
        std::fs::create_dir_all(dir)?;
        let mut msgs = VecDeque::new();
        let mut next = 1;
        if let Ok(s) = std::fs::read_to_string(dir.join("messages.jsonl")) {
            for l in s.lines() {
                // a torn last line (power cut while writing) is skipped
                if let Ok(m) = serde_json::from_str::<Msg>(l) {
                    next = next.max(m.seq + 1);
                    msgs.push_back(m);
                    if msgs.len() > KEEP {
                        msgs.pop_front();
                    }
                }
            }
        }
        let people = std::fs::read(dir.join("people.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        std::fs::create_dir_all(dir.join("files"))?;
        let files = std::fs::read_dir(dir.join("files"))?.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum();
        Ok(Chat {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner { msgs, next, people, files }),
            new: tokio::sync::Notify::new(),
            https: Default::default(),
        })
    }

    /// Where chat keeps its files; the HTTPS certificate lives here too.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The HTTPS port (0: none). Set by whoever opens the HTTPS listener.
    pub fn set_https_port(&self, port: u16) {
        self.https.store(port, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn https_port(&self) -> u16 {
        self.https.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn save_people(&self, people: &HashMap<String, Person>) {
        let tmp = self.dir.join("people.json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec(people).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, self.dir.join("people.json"));
        }
    }

    fn push(&self, inner: &mut Inner, from: &str, name: &str, to: Option<String>, text: String) -> u64 {
        self.push_msg(inner, Msg { seq: 0, ts: 0, from: from.to_string(), name: name.to_string(), to, text, kind: None, file: None, lat: None, lon: None })
    }

    fn push_msg(&self, inner: &mut Inner, mut m: Msg) -> u64 {
        m.seq = inner.next;
        m.ts = now_ms();
        inner.next += 1;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("messages.jsonl")) {
            let _ = writeln!(f, "{}", serde_json::to_string(&m).unwrap_or_default());
        }
        let seq = m.seq;
        inner.msgs.push_back(m);
        if inner.msgs.len() > KEEP {
            inner.msgs.pop_front();
        }
        self.new.notify_waiters();
        seq
    }

    fn check(inner: &Inner, id: &str, token: &str) -> Result<(), ChatError> {
        match inner.people.get(id) {
            Some(p) if p.token == hash(token) => Ok(()),
            _ => Err(ChatError::Forbidden),
        }
    }

    /// Claims `id` with `token` on first use; afterwards renames.
    pub fn join(&self, id: &str, token: &str, name: &str) -> Result<(), ChatError> {
        let name = clean(&name.replace('\n', " "), MAX_NAME);
        if !valid_id(id) || token.len() < 16 || token.len() > 128 {
            return Err(ChatError::BadRequest("bad id or token"));
        }
        if name.is_empty() {
            return Err(ChatError::BadRequest("a name is needed"));
        }
        let mut inner = self.inner.lock().unwrap();
        let notice = match inner.people.get_mut(id) {
            Some(p) => {
                if p.token != hash(token) {
                    return Err(ChatError::Forbidden);
                }
                if p.name == name {
                    None
                } else {
                    let old = std::mem::replace(&mut p.name, name.clone());
                    Some(format!("{old} is now called {name}"))
                }
            }
            None => {
                inner.people.insert(id.to_string(), Person { name: name.clone(), token: hash(token), seen: None, sent: VecDeque::new() });
                Some(format!("{name} joined"))
            }
        };
        if let Some(p) = inner.people.get_mut(id) {
            p.seen = Some(Instant::now());
        }
        if let Some(text) = notice {
            self.save_people(&inner.people);
            self.push(&mut inner, "", "", None, text);
        }
        Ok(())
    }

    pub fn send(&self, id: &str, token: &str, to: Option<&str>, text: &str) -> Result<u64, ChatError> {
        self.post(id, token, to, text, Attach::None)
    }

    /// A message with an attachment: a file's bytes (checked here) or a location.
    pub fn post(&self, id: &str, token: &str, to: Option<&str>, text: &str, att: Attach) -> Result<u64, ChatError> {
        let text = clean(text, MAX_TEXT);
        if text.is_empty() && matches!(att, Attach::None) {
            return Err(ChatError::BadRequest("empty message"));
        }
        let (kind, ext, lat, lon, bytes) = match att {
            Attach::None => (None, None, None, None, None),
            Attach::Location(lat, lon) => {
                if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) || !lat.is_finite() || !lon.is_finite() {
                    return Err(ChatError::BadRequest("bad location"));
                }
                (Some("location"), None, Some(lat), Some(lon), None)
            }
            Attach::File(want, b) => {
                let Some((ext, _, kind)) = sniff(&b) else { return Err(ChatError::BadRequest("unknown file type")) };
                if kind != want {
                    return Err(ChatError::BadRequest("not that kind of file"));
                }
                if b.len() > if kind == "image" { MAX_IMAGE } else { MAX_AUDIO } {
                    return Err(ChatError::BadRequest("file too big"));
                }
                (Some(kind), Some(ext), None, None, Some(b))
            }
        };
        let mut inner = self.inner.lock().unwrap();
        Self::check(&inner, id, token)?;
        let to = to.filter(|t| !t.is_empty()).map(String::from);
        if let Some(t) = &to
            && !inner.people.contains_key(t)
        {
            return Err(ChatError::BadRequest("no such person"));
        }
        let p = inner.people.get_mut(id).unwrap();
        // at most 10 messages in 10 seconds
        let now = Instant::now();
        while p.sent.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(10)) {
            p.sent.pop_front();
        }
        if p.sent.len() >= 10 {
            return Err(ChatError::TooFast);
        }
        p.sent.push_back(now);
        p.seen = Some(now);
        let name = p.name.clone();
        let file = match (ext, bytes) {
            (Some(ext), Some(b)) => {
                let f = format!("{}-{}.{ext}", inner.next, &hash(&format!("{id}{}", now_ms()))[..8]);
                let dir = self.dir.join("files");
                let tmp = dir.join(format!("{f}.tmp"));
                if std::fs::write(&tmp, &b).and_then(|_| std::fs::rename(&tmp, dir.join(&f))).is_err() {
                    return Err(ChatError::BadRequest("could not store the file"));
                }
                inner.files += b.len() as u64;
                self.trim_files(&mut inner);
                Some(f)
            }
            _ => None,
        };
        let m = Msg { seq: 0, ts: 0, from: id.to_string(), name, to, text, kind: kind.map(String::from), file, lat, lon };
        Ok(self.push_msg(&mut inner, m))
    }

    /// Keeps files/ under its cap by removing the oldest files (their messages stay, marked as gone).
    fn trim_files(&self, inner: &mut Inner) {
        if inner.files <= FILES_CAP {
            return;
        }
        let dir = self.dir.join("files");
        let mut all: Vec<(u64, PathBuf, u64)> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let e = e.ok()?;
                let seq = e.file_name().to_string_lossy().split('-').next()?.parse().ok()?;
                Some((seq, e.path(), e.metadata().ok()?.len()))
            })
            .collect();
        all.sort();
        for (_, path, len) in all {
            if inner.files <= FILES_CAP * 9 / 10 {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                inner.files = inner.files.saturating_sub(len);
            }
        }
    }

    /// A stored file, if `id` may see the message it belongs to.
    pub fn file(&self, id: &str, token: &str, name: &str) -> Result<(Vec<u8>, &'static str), ChatError> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') || name.starts_with('.') {
            return Err(ChatError::BadRequest("bad name"));
        }
        {
            let inner = self.inner.lock().unwrap();
            Self::check(&inner, id, token)?;
            if !inner.msgs.iter().any(|m| m.file.as_deref() == Some(name) && m.visible_to(id)) {
                return Err(ChatError::Forbidden);
            }
        }
        let b = std::fs::read(self.dir.join("files").join(name)).map_err(|_| ChatError::Gone)?;
        let mime = sniff(&b).map(|t| t.1).unwrap_or("application/octet-stream");
        Ok((b, mime))
    }

    /// What `id` may see after `since` (the last 100 when `since` is 0), and who is here.
    pub fn read(&self, id: &str, token: &str, since: u64) -> Result<(u64, Vec<Msg>, Vec<Value>), ChatError> {
        let mut inner = self.inner.lock().unwrap();
        Self::check(&inner, id, token)?;
        inner.people.get_mut(id).unwrap().seen = Some(Instant::now());
        let visible = |m: &&Msg| m.visible_to(id);
        let mut msgs: Vec<Msg> = inner.msgs.iter().rev().filter(|m| m.seq > since).filter(visible).take(if since == 0 { 100 } else { 500 }).cloned().collect();
        msgs.reverse();
        let mut people: Vec<Value> = inner
            .people
            .iter()
            .map(|(pid, p)| json!({"id": pid, "name": p.name, "online": p.seen.is_some_and(|t| t.elapsed() < ONLINE)}))
            .collect();
        people.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
        Ok((inner.next - 1, msgs, people))
    }

    /// Like `read`, but waits (up to 25 s) until there is something new.
    pub async fn wait(&self, id: &str, token: &str, since: u64) -> Result<(u64, Vec<Msg>, Vec<Value>), ChatError> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let notified = self.new.notified();
            tokio::pin!(notified);
            notified.as_mut().enable(); // registered before reading, so no message slips between
            let r = self.read(id, token, since)?;
            if !r.1.is_empty() || tokio::time::Instant::now() >= deadline {
                return Ok(r);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.read(id, token, since);
            }
        }
    }
}

fn err(e: ChatError) -> Response {
    match e {
        ChatError::BadRequest(m) => (StatusCode::BAD_REQUEST, Json(json!({"error": m}))).into_response(),
        ChatError::Forbidden => (StatusCode::FORBIDDEN, Json(json!({"error": "not joined"}))).into_response(),
        ChatError::TooFast => (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "slow down"}))).into_response(),
        ChatError::Gone => (StatusCode::GONE, Json(json!({"error": "removed to save space"}))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct JoinBody {
    id: String,
    token: String,
    name: String,
}

#[derive(Deserialize)]
pub struct SendBody {
    id: String,
    token: String,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    text: String,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lon: Option<f64>,
}

#[derive(Deserialize)]
pub struct UploadParams {
    id: String,
    token: String,
    #[serde(default)]
    to: Option<String>,
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
pub struct Who {
    id: String,
    token: String,
}

#[derive(Deserialize)]
pub struct PollParams {
    id: String,
    token: String,
    #[serde(default)]
    since: u64,
    #[serde(default)]
    wait: Option<u8>,
}

pub async fn join(State(app): State<AppState>, Json(b): Json<JoinBody>) -> Response {
    let Some(c) = &app.chat else { return StatusCode::NOT_FOUND.into_response() };
    match c.join(&b.id, &b.token, &b.name) {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn send(State(app): State<AppState>, Json(b): Json<SendBody>) -> Response {
    let Some(c) = &app.chat else { return StatusCode::NOT_FOUND.into_response() };
    let att = match (b.lat, b.lon) {
        (Some(lat), Some(lon)) => Attach::Location(lat, lon),
        _ => Attach::None,
    };
    match c.post(&b.id, &b.token, b.to.as_deref(), &b.text, att) {
        Ok(seq) => Json(json!({"seq": seq})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn upload(State(app): State<AppState>, Query(p): Query<UploadParams>, body: axum::body::Bytes) -> Response {
    let Some(c) = &app.chat else { return StatusCode::NOT_FOUND.into_response() };
    let kind = match p.kind.as_str() {
        "image" => "image",
        "audio" => "audio",
        _ => return err(ChatError::BadRequest("kind is image or audio")),
    };
    match c.post(&p.id, &p.token, p.to.as_deref(), &p.text, Attach::File(kind, body.to_vec())) {
        Ok(seq) => Json(json!({"seq": seq})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn file(
    State(app): State<AppState>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Query(w): Query<Who>,
    headers: axum::http::HeaderMap,
) -> Response {
    use axum::http::header;
    let Some(c) = &app.chat else { return StatusCode::NOT_FOUND.into_response() };
    let (b, mime) = match c.file(&w.id, &w.token, &name) {
        Ok(f) => f,
        Err(e) => return err(e),
    };
    let common = [(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private, max-age=604800"), (header::ACCEPT_RANGES, "bytes")];
    // Safari only plays audio from a server that answers byte ranges
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|r| byte_range(r, b.len()));
    match range {
        Some((a, z)) => (
            StatusCode::PARTIAL_CONTENT,
            common,
            [(header::CONTENT_RANGE, format!("bytes {a}-{z}/{}", b.len()))],
            b[a..=z].to_vec(),
        )
            .into_response(),
        None => (common, b).into_response(),
    }
}

/// "bytes=A-B", "bytes=A-" or "bytes=-N" (one range only) as inclusive offsets within len.
fn byte_range(h: &str, len: usize) -> Option<(usize, usize)> {
    let (a, z) = h.strip_prefix("bytes=")?.split_once('-')?;
    if len == 0 || z.contains(',') {
        return None;
    }
    let (a, z) = match (a.trim(), z.trim()) {
        ("", n) => {
            let n: usize = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (a, "") => (a.parse().ok()?, len - 1),
        (a, z) => (a.parse().ok()?, z.parse::<usize>().ok()?.min(len - 1)),
    };
    (a <= z && z < len).then_some((a, z))
}

pub async fn poll(State(app): State<AppState>, Query(p): Query<PollParams>) -> Response {
    let Some(c) = &app.chat else { return StatusCode::NOT_FOUND.into_response() };
    let r = if p.wait == Some(1) { c.wait(&p.id, &p.token, p.since).await } else { c.read(&p.id, &p.token, p.since) };
    match r {
        Ok((seq, messages, people)) => Json(json!({"seq": seq, "messages": messages, "people": people})).into_response(),
        Err(e) => err(e),
    }
}

pub async fn page(State(app): State<AppState>) -> Response {
    if app.chat.is_none() {
        return (StatusCode::NOT_FOUND, Html(crate::page::plain("Chat", "<p>Chat is not switched on here.</p>"))).into_response();
    }
    let https = app.chat.as_ref().map_or(0, |c| c.https_port());
    Html(
        PAGE.replace("\"/style.css\"", &format!("\"{}\"", crate::page::css_href()))
            .replace("const HTTPS_PORT = 0;", &format!("const HTTPS_PORT = {https};")),
    )
    .into_response()
}

const PAGE: &str = include_str!("chat.html");

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("sp-chat-{}-{}-{n}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const TA: &str = "tokenAAAAAAAAAAAAAAAA";
    const TB: &str = "tokenBBBBBBBBBBBBBBBB";

    #[test]
    fn rooms_private_messages_and_persistence() {
        let d = tmp();
        let c = Chat::open(&d).unwrap();
        c.join("annann01", TA, "Ann").unwrap();
        c.join("bobbob01", TB, "Bob").unwrap();
        c.join("carcar01", "tokenCCCCCCCCCCCCCCCC", "Cara").unwrap();
        // someone else can't take Ann's id
        assert_eq!(c.join("annann01", TB, "Mallory"), Err(ChatError::Forbidden));
        assert_eq!(c.send("annann01", TB, None, "hi"), Err(ChatError::Forbidden));
        c.send("annann01", TA, None, "Water at the church at 8").unwrap();
        c.send("annann01", TA, Some("bobbob01"), "Bob, can you bring the radio?").unwrap();
        let texts = |id: &str, t: &str| c.read(id, t, 0).unwrap().1.iter().map(|m| m.text.clone()).collect::<Vec<_>>();
        assert!(texts("bobbob01", TB).contains(&"Bob, can you bring the radio?".to_string()));
        assert!(!texts("carcar01", "tokenCCCCCCCCCCCCCCCC").contains(&"Bob, can you bring the radio?".to_string()));
        assert!(texts("carcar01", "tokenCCCCCCCCCCCCCCCC").contains(&"Water at the church at 8".to_string()));
        assert!(texts("carcar01", "tokenCCCCCCCCCCCCCCCC").contains(&"Ann joined".to_string()));
        // since: only newer
        let (seq, _, people) = c.read("bobbob01", TB, 0).unwrap();
        assert_eq!(people.len(), 3);
        assert!(c.read("bobbob01", TB, seq).unwrap().1.is_empty());
        drop(c);
        let c = Chat::open(&d).unwrap();
        let (seq2, msgs, _) = c.read("bobbob01", TB, 0).unwrap();
        assert_eq!(seq2, seq);
        assert_eq!(msgs.last().unwrap().text, "Bob, can you bring the radio?");
        assert_eq!(c.send("bobbob01", TB, None, "  \n "), Err(ChatError::BadRequest("empty message")));
        c.join("bobbob01", TB, "Robert").unwrap();
        assert_eq!(c.read("annann01", TA, seq2).unwrap().1[0].text, "Bob is now called Robert");
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn ranges() {
        assert_eq!(byte_range("bytes=0-", 10), Some((0, 9)));
        assert_eq!(byte_range("bytes=0-1", 10), Some((0, 1)));
        assert_eq!(byte_range("bytes=5-99", 10), Some((5, 9)));
        assert_eq!(byte_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(byte_range("bytes=10-", 10), None);
        assert_eq!(byte_range("bytes=0-1,4-5", 10), None);
        assert_eq!(byte_range("items=0-1", 10), None);
    }

    #[test]
    fn photos_voice_and_places() {
        let d = tmp();
        let c = Chat::open(&d).unwrap();
        c.join("annann01", TA, "Ann").unwrap();
        c.join("bobbob01", TB, "Bob").unwrap();
        c.join("carcar01", "tokenCCCCCCCCCCCCCCCC", "Cara").unwrap();
        let jpeg = [&[0xFF, 0xD8, 0xFF, 0xE0][..], &[0u8; 100]].concat();
        let ogg = [&b"OggS"[..], &[0u8; 100]].concat();
        // a photo for everyone, with a caption
        c.post("annann01", TA, None, "The bridge is down", Attach::File("image", jpeg.clone())).unwrap();
        let m = c.read("carcar01", "tokenCCCCCCCCCCCCCCCC", 0).unwrap().1.pop().unwrap();
        assert_eq!((m.kind.as_deref(), m.text.as_str()), (Some("image"), "The bridge is down"));
        let f = m.file.unwrap();
        assert!(f.ends_with(".jpg"));
        assert_eq!(c.file("carcar01", "tokenCCCCCCCCCCCCCCCC", &f).unwrap(), (jpeg.clone(), "image/jpeg"));
        // a private voice clip: Cara can't fetch it
        c.post("annann01", TA, Some("bobbob01"), "", Attach::File("audio", ogg.clone())).unwrap();
        let v = c.read("bobbob01", TB, 0).unwrap().1.pop().unwrap();
        let vf = v.file.unwrap();
        assert_eq!(c.file("bobbob01", TB, &vf).unwrap().1, "audio/ogg");
        assert_eq!(c.file("carcar01", "tokenCCCCCCCCCCCCCCCC", &vf), Err(ChatError::Forbidden));
        assert_eq!(c.file("bobbob01", TB, "../people.json"), Err(ChatError::BadRequest("bad name")));
        // the type comes from the bytes, not from what the browser says
        assert_eq!(c.post("annann01", TA, None, "", Attach::File("image", ogg)), Err(ChatError::BadRequest("not that kind of file")));
        assert_eq!(c.post("annann01", TA, None, "", Attach::File("image", b"<html>".to_vec())), Err(ChatError::BadRequest("unknown file type")));
        // places
        c.post("bobbob01", TB, None, "", Attach::Location(51.5007, -0.1246)).unwrap();
        let l = c.read("annann01", TA, 0).unwrap().1.pop().unwrap();
        assert_eq!((l.kind.as_deref(), l.lat, l.lon), (Some("location"), Some(51.5007), Some(-0.1246)));
        assert_eq!(c.post("bobbob01", TB, None, "", Attach::Location(95.0, 0.0)), Err(ChatError::BadRequest("bad location")));
        // survives a restart
        drop(c);
        let c = Chat::open(&d).unwrap();
        assert_eq!(c.file("carcar01", "tokenCCCCCCCCCCCCCCCC", &f).unwrap().0, jpeg);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn rate_limit() {
        let d = tmp();
        let c = Chat::open(&d).unwrap();
        c.join("annann01", TA, "Ann").unwrap();
        for _ in 0..10 {
            c.send("annann01", TA, None, "x").unwrap();
        }
        assert_eq!(c.send("annann01", TA, None, "x"), Err(ChatError::TooFast));
        std::fs::remove_dir_all(d).ok();
    }

    #[tokio::test]
    async fn wait_wakes_on_a_new_message() {
        let d = tmp();
        let c = std::sync::Arc::new(Chat::open(&d).unwrap());
        c.join("annann01", TA, "Ann").unwrap();
        c.join("bobbob01", TB, "Bob").unwrap();
        let (seq, _, _) = c.read("bobbob01", TB, 0).unwrap();
        let c2 = c.clone();
        let t = std::time::Instant::now();
        let h = tokio::spawn(async move { c2.wait("bobbob01", TB, seq).await.unwrap() });
        tokio::time::sleep(Duration::from_millis(100)).await;
        c.send("annann01", TA, None, "hello").unwrap();
        let (_, msgs, _) = h.await.unwrap();
        assert_eq!(msgs[0].text, "hello");
        assert!(t.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(d).ok();
    }
}
