//! sp-server: HTTP front end over the Rust core. Used by the hub binary (main.rs) and,
//! through sp-ffi, embedded in the mobile app on 127.0.0.1.
//!
//! Content is a library of collections (DESIGN §5): one pack each (Wikipedia Essentials, wikiHow, survival
//! guides, books, …), any subset installed, possibly none (games or maps only). Search ranks across all.
//!
//! HTML (no JavaScript needed):
//!   GET /                          home: search, collections
//!   GET /search?q=&mode=&c=        results (`c`: one collection id)
//!   GET /doc/{c}/{n}               article n of collection c, deep-linkable via #pN
//!   GET /link/{c}/{src}/{*path}    resolves an internal article link, redirects to /doc/…
//!   GET /img/{key}?v=320|640       image bytes (falls back to the variant that exists)
//! Maps (optional, `--maps DIR`): see maps.rs (/map, /tiles/…, /maps/assets/…, /api/maps)
//! Games (optional, `--games DIR`): see games.rs (/games, /game/{id}/…)
//! JSON:
//!   GET /api/search?q=&mode=&c=&k=
//!   GET /api/doc/{c}/{n}
//!   GET /api/complete?q=&k=
//!   GET /api/info
//! Answers (optional, `--llm GGUF`): see ask.rs (/ask, /api/ask)
//! Chat (optional, `--chat DIR`): see chat.rs (/chat, /api/chat/…)
//! Books to read in the browser (`--books DIR`, or the app's `installed`): see reading.rs (/books, /read/)

pub mod ask;
pub mod books;
pub mod browse;
pub mod catalog;
pub mod chat;
pub mod emergency;
pub mod games;
pub mod maps;
pub mod offer;
pub mod reading;
pub mod tls;
mod page;

use anyhow::Context;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use sp_search::text::snippet;
use sp_search::Mode;
use sp_search::library::Library;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Server configuration (CLI flags on the hub, JSON from the app via sp-ffi).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Library directory: every subdirectory with a manifest.json is a collection; `models/` may hold
    /// the embedding GGUF. Missing or empty is fine (games / maps only).
    #[serde(default)]
    pub library: Option<PathBuf>,
    /// Single pack directories, each added as a collection (hub, experiments)
    #[serde(default)]
    pub packs: Vec<PathBuf>,
    /// Embedding GGUF (default: <library>/models/*.gguf, then each pack's models/*.gguf). Without one,
    /// search is BM25 only.
    #[serde(default)]
    pub model: Option<PathBuf>,
    /// CPU threads for query embedding (default: SP_THREADS or ~80% of cores)
    #[serde(default)]
    pub threads: Option<usize>,
    /// Offline maps dir: region *.pmtiles (+ .places/) and assets/
    #[serde(default)]
    pub maps: Option<PathBuf>,
    /// Games dir: one subdirectory per game with index.html and game.json
    #[serde(default)]
    pub games: Option<PathBuf>,
    /// Hub only: publish /api/catalog and /download/… so apps can fetch content
    #[serde(default)]
    pub downloads: bool,
    /// Chat model for /ask: a GGUF file or a directory holding one. Loaded on the first question.
    #[serde(default)]
    pub llm: Option<PathBuf>,
    /// Hub only: the ebook shelf (ingest/books/epub_shelf.py): shelf.json(.gz), epub/, epub-images/, covers/
    #[serde(default)]
    pub books: Option<PathBuf>,
    /// Chat for people on this Wi-Fi: the directory for its messages (chat.rs). Without it, no chat.
    #[serde(default)]
    pub chat: Option<PathBuf>,
    /// App only: its installed.json. The books on the phone are offered to browsers at /books (reading.rs),
    /// and everything installed to other phones running the app (offer.rs).
    #[serde(default)]
    pub installed: Option<PathBuf>,
    /// The Android app's APK, offered at /app.apk to phones nearby (a hub: default <maps>/survivalpedia.apk)
    #[serde(default)]
    pub apk: Option<PathBuf>,
}

pub struct App {
    pub lib: Library,
    embedder: Option<Mutex<sp_embed::Embedder>>,
    query_prefix: String,
    pub maps: Option<maps::Maps>,
    pub games: Option<games::Games>,
    pub catalog: Option<catalog::Catalog>,
    pub llm: ask::LlmSlot,
    /// Browse sections per collection (same order as lib.cols)
    pub browse: Vec<browse::Browse>,
    /// The ebook shelf's title/description search (hub with --books)
    pub books: Option<books::BookIndex>,
    /// Shared, so a hub reloading its content keeps the same chat
    pub chat: Option<Arc<chat::Chat>>,
    pub installed: Option<PathBuf>,
    pub apk: Option<PathBuf>,
}

pub type AppState = Arc<App>;

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    /// Restrict to one collection (id)
    #[serde(default)]
    pub c: Option<String>,
    #[serde(default)]
    pub k: Option<usize>,
}

#[derive(serde::Serialize)]
pub struct HitOut {
    pub rank: usize,
    /// Collection id
    pub collection: String,
    #[serde(skip)]
    pub col: usize,
    pub doc: u32,
    #[serde(skip)]
    pub chunk: u32,
    pub title: String,
    pub source: String,
    pub lang: &'static str,
    pub block: String,
    pub snippet: String,
    pub bm25_rank: Option<usize>,
    pub dense_rank: Option<usize>,
}

#[derive(serde::Serialize)]
pub struct SearchOut {
    pub query: String,
    pub mode: &'static str,
    pub hits: Vec<HitOut>,
    pub embed_ms: f64,
    pub search_ms: f64,
}

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("error: {:#}", self.0)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

impl App {
    /// Books a browser can read here: the hub's shelf, or the sharing phone's own books.
    pub fn has_books(&self) -> bool {
        self.books.is_some() || self.installed.is_some()
    }

    fn mode(&self, m: Option<&str>) -> Mode {
        let want = match m {
            Some("bm25") => Mode::Bm25,
            Some("dense") => Mode::Dense,
            _ => Mode::Hybrid,
        };
        if self.embedder.is_none() || !self.lib.has_dense() { Mode::Bm25 } else { want }
    }

    pub fn search(&self, p: &SearchParams) -> anyhow::Result<SearchOut> {
        let mode = self.mode(p.mode.as_deref());
        let k = p.k.unwrap_or(20).clamp(1, 100);
        let lang = p.lang.as_deref().filter(|l| !l.is_empty() && *l != "all");
        let q = p.q.trim();
        let t = Instant::now();
        let qvec = match (&self.embedder, mode) {
            (Some(e), Mode::Dense | Mode::Hybrid) if !q.is_empty() => {
                Some(e.lock().unwrap().embed(&format!("{}{q}", self.query_prefix))?)
            }
            _ => None,
        };
        let embed_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let only = p.c.as_deref().filter(|c| !c.is_empty()).and_then(|c| self.lib.find(c));
        let raw = if q.is_empty() || self.lib.cols.is_empty() {
            Vec::new()
        } else {
            self.lib.search(q, qvec.as_deref(), mode, if lang.is_some() { k * 5 } else { k }, only)?
        };
        let mut hits = Vec::new();
        for lh in raw {
            let (col, h) = (lh.col, lh.hit);
            let pack = &self.lib.cols[col].searcher.pack;
            if lang.is_some_and(|l| pack.lang(h.doc) != l) {
                continue;
            }
            let c = pack.chunk(h.chunk);
            let d = pack.get(h.doc)?;
            hits.push(HitOut {
                rank: hits.len() + 1,
                collection: self.lib.cols[col].id.clone(),
                col,
                doc: h.doc,
                chunk: h.chunk,
                title: d.title,
                source: d.source,
                lang: d.lang,
                block: format!("p{}", c.block_first),
                snippet: snippet(d.record["html"].as_str().unwrap_or(""), c.block_first, c.block_last, 240),
                bm25_rank: h.bm25_rank,
                dense_rank: h.dense_rank,
            });
            if hits.len() >= k {
                break;
            }
        }
        let mode = match mode {
            Mode::Bm25 => "bm25",
            Mode::Dense => "dense",
            Mode::Hybrid => "hybrid",
        };
        Ok(SearchOut { query: q.to_string(), mode, hits, embed_ms, search_ms: t.elapsed().as_secs_f64() * 1e3 })
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, AppError> {
    Ok(tokio::task::spawn_blocking(f).await.context("worker panicked")??)
}

async fn index(State(app): State<AppState>) -> Html<String> {
    Html(page::home(&app))
}

async fn search_html(State(app): State<AppState>, Query(p): Query<SearchParams>) -> Result<Html<String>, AppError> {
    let app2 = app.clone();
    let (p, out) = blocking(move || {
        let out = app2.search(&p)?;
        Ok((p, out))
    })
    .await?;
    Ok(Html(page::results(&app, &p, &out)))
}

async fn search_json(State(app): State<AppState>, Query(p): Query<SearchParams>) -> Result<Json<SearchOut>, AppError> {
    Ok(Json(blocking(move || app.search(&p)).await?))
}

#[derive(Deserialize)]
struct EmbedParams {
    texts: Vec<String>,
    /// Queries get the pinned query instruction (as library search does); documents get none.
    #[serde(default)]
    query: bool,
}

/// Vectors for short texts with the library's embedding model (the app's Notes use it for search by meaning):
/// the first [`EMBED_DIMS`] dimensions, L2-normalised, as the packs store them. 503 when no model is loaded.
async fn embed_json(State(app): State<AppState>, Json(p): Json<EmbedParams>) -> Result<Response, AppError> {
    if app.embedder.is_none() {
        return Ok((StatusCode::SERVICE_UNAVAILABLE, "no embedding model").into_response());
    }
    if p.texts.len() > 32 {
        return Ok((StatusCode::BAD_REQUEST, "at most 32 texts per call").into_response());
    }
    let app2 = app.clone();
    let vectors = blocking(move || {
        let e = app2.embedder.as_ref().unwrap().lock().unwrap();
        p.texts
            .iter()
            .map(|t| {
                let v = e.embed(&if p.query { format!("{}{t}", app2.query_prefix) } else { t.clone() })?;
                Ok(truncate_unit(&v, EMBED_DIMS))
            })
            .collect::<anyhow::Result<Vec<_>>>()
    })
    .await?;
    Ok(Json(serde_json::json!({"dims": EMBED_DIMS, "vectors": vectors})).into_response())
}

pub const EMBED_DIMS: usize = 256;

/// The first `n` dimensions of a vector, rescaled to unit length (Matryoshka truncation).
fn truncate_unit(v: &[f32], n: usize) -> Vec<f32> {
    let v = &v[..n.min(v.len())];
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter().map(|x| ((x / norm) * 1e5).round() / 1e5).collect()
}

fn resolve(app: &App, c: &str, n: u32) -> Option<usize> {
    app.lib.find(c).filter(|&i| (n as usize) < app.lib.cols[i].searcher.pack.doc_count())
}

async fn doc_html(State(app): State<AppState>, UrlPath((c, n)): UrlPath<(String, u32)>) -> Result<Response, AppError> {
    let Some(i) = resolve(&app, &c, n) else {
        return Ok((StatusCode::NOT_FOUND, Html(page::not_found(&format!("{c}/{n}")))).into_response());
    };
    let app2 = app.clone();
    let doc = blocking(move || app2.lib.cols[i].searcher.pack.get(n)).await?;
    Ok(Html(page::doc(&app, i, &doc)).into_response())
}

/// Old single-pack links (/doc/{n}): the first collection.
async fn doc_legacy(State(app): State<AppState>, UrlPath(n): UrlPath<u32>) -> Response {
    match app.lib.cols.first() {
        Some(c) => Redirect::to(&format!("/doc/{}/{n}", c.id)).into_response(),
        None => (StatusCode::NOT_FOUND, "no collections installed").into_response(),
    }
}

async fn doc_json(State(app): State<AppState>, UrlPath((c, n)): UrlPath<(String, u32)>) -> Result<Response, AppError> {
    let Some(i) = resolve(&app, &c, n) else {
        return Ok((StatusCode::NOT_FOUND, "no such doc").into_response());
    };
    let d = blocking(move || app.lib.cols[i].searcher.pack.get(n)).await?;
    Ok(Json(serde_json::json!({
        "collection": c, "doc": d.num, "title": d.title, "lang": d.lang, "source": d.source, "record": d.record,
    }))
    .into_response())
}

/// Internal links are stored as `wiki:<path>` relative to the doc's source; they are rendered as
/// /link/{collection}/{source index}/{path}. The same collection and source win, then any collection.
async fn wiki_link(State(app): State<AppState>, UrlPath((c, src, path)): UrlPath<(String, u8, String)>) -> Response {
    let (path, frag) = match path.split_once('#') {
        Some((p, f)) => (p.to_string(), format!("#{f}")),
        None => (path, String::new()),
    };
    let prefer = app.lib.find(&c);
    let same_source = prefer.and_then(|i| {
        let pack = &app.lib.cols[i].searcher.pack;
        pack.lookup_title(&path).into_iter().find(|&d| pack.entry(d).source == src).map(|d| (i, d))
    });
    match same_source.or_else(|| app.lib.lookup_title(&path, prefer)) {
        Some((i, d)) => Redirect::to(&format!("/doc/{}/{d}{frag}", app.lib.cols[i].id)).into_response(),
        None => (StatusCode::NOT_FOUND, Html(page::not_found(&path))).into_response(),
    }
}

#[derive(Deserialize)]
struct ImgParams {
    v: Option<String>,
}

async fn image(State(app): State<AppState>, UrlPath(key): UrlPath<String>, Query(p): Query<ImgParams>) -> Response {
    let Some(k) = sp_pack::parse_image_key(&key) else {
        return (StatusCode::BAD_REQUEST, "bad key").into_response();
    };
    let order: &[&str] = if p.v.as_deref() == Some("640") { &["640", "320", "svg"] } else { &["320", "640", "svg"] };
    for v in order {
        if let Ok(Some(data)) = app.lib.image(&k, v) {
            let ct = if *v == "svg" { "image/svg+xml" } else { "image/webp" };
            return ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "public, max-age=31536000, immutable")], data)
                .into_response();
        }
    }
    (StatusCode::NOT_FOUND, "no such image").into_response()
}

#[derive(Deserialize)]
struct CompleteParams {
    q: String,
    k: Option<usize>,
}

async fn complete(State(app): State<AppState>, Query(p): Query<CompleteParams>) -> Json<serde_json::Value> {
    let rows: Vec<_> = app
        .lib
        .complete(&p.q, p.k.unwrap_or(10).min(50))
        .into_iter()
        .map(|(key, i, d)| {
            let pack = &app.lib.cols[i].searcher.pack;
            serde_json::json!({"key": key, "collection": app.lib.cols[i].id, "doc": d, "title": pack.title(d), "source": pack.source(d)})
        })
        .collect();
    Json(serde_json::json!(rows))
}

async fn info(State(app): State<AppState>) -> Json<serde_json::Value> {
    let cols: Vec<_> = app
        .lib
        .cols
        .iter()
        .map(|c| {
            let m = &c.searcher.pack.manifest;
            serde_json::json!({
                "id": c.id, "title": c.title, "description": c.description, "kind": c.kind,
                "counts": m["counts"], "sources": m["sources"],
                "vectors": c.searcher.vecs.iter().map(|v| serde_json::json!({"name": v.name, "n": v.index.n, "dims": v.index.d})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Json(serde_json::json!({
        "collections": cols,
        "docs": app.lib.doc_count(),
        "dense": app.embedder.is_some() && app.lib.has_dense(),
        "llm": app.llm.path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy()),
        "maps": app.maps.is_some(), "games": app.games.is_some(),
    }))
}

async fn style() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css"), (header::CACHE_CONTROL, "public, max-age=3600")], page::CSS)
}

/// Inter (SIL OFL 1.1, assets/fonts/Inter-OFL.txt), Latin subset, so pages look the same on every device.
pub(crate) const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.woff2");

async fn font(UrlPath(name): UrlPath<String>) -> Response {
    let data: &'static [u8] = match name.as_str() {
        "Inter-Regular.woff2" => INTER_REGULAR,
        "Inter-SemiBold.woff2" => include_bytes!("../assets/fonts/Inter-SemiBold.woff2"),
        "Inter-Bold.woff2" => include_bytes!("../assets/fonts/Inter-Bold.woff2"),
        "Inter-OFL.txt" => include_bytes!("../assets/fonts/Inter-OFL.txt"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let ct = if name.ends_with(".txt") { "text/plain; charset=utf-8" } else { "font/woff2" };
    ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "public, max-age=604800")], data).into_response()
}

fn first_gguf(dir: &std::path::Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "gguf"))
        .collect();
    v.sort();
    v.into_iter().next()
}

/// Every collection directory the config names: the library's subdirectories, then `packs`.
pub fn collection_dirs(cfg: &Config) -> Vec<PathBuf> {
    let mut dirs = cfg.library.as_deref().map(sp_search::library::collection_dirs).unwrap_or_default();
    dirs.extend(cfg.packs.iter().cloned());
    dirs
}

/// The embedding model: `model`, else <library>/models/*.gguf, else the first pack holding one.
/// Where each catalog path prefix lives on disk (the same mapping as the /download routes).
pub fn download_roots(app: &App, cfg: &Config) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = app.lib.cols.iter().map(|c| (format!("collections/{}", c.id), c.searcher.pack.dir.clone())).collect();
    if let Some(d) = model_path(cfg).and_then(|m| m.parent().map(PathBuf::from)) {
        v.push(("models".into(), d));
    }
    if let Some(m) = &cfg.maps {
        v.push(("maps".into(), m.clone()));
    }
    if let Some(g) = &cfg.games {
        v.push(("games".into(), g.clone()));
    }
    if let Some(l) = &cfg.llm {
        v.push(("llm".into(), ask::llm_dir(l)));
    }
    if let Some(b) = cfg.books.as_ref().filter(|b| b.join("shelf.json").is_file()) {
        v.push(("books".into(), b.clone()));
    }
    v
}

pub fn model_path(cfg: &Config) -> Option<PathBuf> {
    cfg.model
        .clone()
        .or_else(|| cfg.library.as_ref().and_then(|l| first_gguf(&l.join("models"))))
        .or_else(|| collection_dirs(cfg).iter().find_map(|d| first_gguf(&d.join("models"))))
}

/// Opens the collections, model, maps, games and chat model.
pub fn load(cfg: &Config) -> anyhow::Result<Arc<App>> {
    let lib = Library::open(&collection_dirs(cfg))?;
    eprintln!("collections: {}", lib.cols.iter().map(|c| format!("{} ({} docs)", c.id, c.searcher.pack.doc_count())).collect::<Vec<_>>().join(", "));
    let model = model_path(cfg).filter(|_| lib.has_dense());
    let embedder = match &model {
        Some(m) => Some(Mutex::new(sp_embed::Embedder::load(m, cfg.threads.unwrap_or_else(sp_embed::default_threads))?)),
        None => {
            if lib.has_dense() {
                eprintln!("no embedding model: BM25 only");
            }
            None
        }
    };
    let query_prefix = lib
        .cols
        .iter()
        .find_map(|c| c.searcher.pack.manifest["embedding"]["query_instruction"].as_str())
        .unwrap_or("")
        .to_string();
    let maps = cfg.maps.as_deref().filter(|d| d.is_dir()).map(maps::Maps::open).transpose()?;
    if let Some(m) = &maps {
        eprintln!("maps: {}", m.regions.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", "));
    }
    let games = cfg.games.as_deref().filter(|d| d.is_dir()).map(games::Games::open).transpose()?;
    if let Some(g) = &games {
        eprintln!("games: {}", g.list.iter().map(|g| g.id.as_str()).collect::<Vec<_>>().join(", "));
    }
    let llm = ask::LlmSlot::new(cfg.llm.as_deref());
    if let Some(p) = &llm.path {
        eprintln!("llm: {} (loaded on the first question)", p.display());
    }
    let catalog = if cfg.downloads {
        let shelf = cfg.books.as_deref().filter(|b| b.join("shelf.json").is_file());
        let mut c = catalog::Catalog::build(&lib, model_path(cfg).as_deref(), cfg.maps.as_deref(), games.as_ref(), llm.path.as_deref(), shelf)?;
        if cfg.books.as_ref().is_some_and(|b| b.join("shelf.json").is_file()) {
            // the shelf is its own index (one entry per book), not catalog components
            c.json["books"] = serde_json::json!({"index": "/api/books", "download": "/download/books"});
        }
        Some(c)
    } else {
        None
    };
    let browse = lib.cols.iter().map(|c| browse::Browse::load(&c.searcher.pack)).collect();
    let books = match cfg.books.as_deref().filter(|b| b.join("shelf.json").is_file()) {
        Some(b) => {
            let ix = books::BookIndex::load(b)?;
            eprintln!("books: {} on the shelf", ix.len());
            Some(ix)
        }
        None => None,
    };
    let chat = cfg.chat.as_deref().map(chat::Chat::open).transpose()?.map(Arc::new);
    Ok(Arc::new(App { lib, embedder, query_prefix, maps, games, catalog, llm, browse, books, chat, installed: cfg.installed.clone(), apk: cfg.apk.clone() }))
}

pub fn router(app: Arc<App>, cfg: &Config) -> Router {
    let downloads: Vec<(String, PathBuf)> = if cfg.downloads {
        app.lib.cols.iter().map(|c| (c.id.clone(), c.searcher.pack.dir.clone())).collect()
    } else {
        Vec::new()
    };
    let model_dir = if cfg.downloads { model_path(cfg).and_then(|m| m.parent().map(PathBuf::from)) } else { None };
    let mut r = Router::new()
        .route("/", get(index))
        .route("/search", get(search_html))
        .route("/browse/{c}", get(browse::page))
        .route("/doc/{c}/{n}", get(doc_html))
        .route("/doc/{n}", get(doc_legacy))
        .route("/link/{c}/{src}/{*path}", get(wiki_link))
        .route("/img/{key}", get(image))
        .route("/style.css", get(style))
        .route("/fonts/{name}", get(font))
        .route("/api/search", get(search_json))
        .route("/api/doc/{c}/{n}", get(doc_json))
        .route("/api/complete", get(complete))
        .route("/api/info", get(info))
        .route("/api/embed", axum::routing::post(embed_json))
        .route("/map", get(maps::page))
        .route("/tiles/{z}/{x}/{y}", get(maps::tile))
        .route("/maps/assets/{*path}", get(maps::asset))
        .route("/api/maps", get(maps::list))
        .route("/api/places", get(maps::places))
        .route("/games", get(games::page))
        .route("/emergency", get(emergency::page))
        .route("/ask", get(ask::page))
        .route("/api/ask", get(ask::api).post(ask::api_post))
        .route("/chat", get(chat::page))
        .route("/api/chat/join", axum::routing::post(chat::join))
        .route("/api/chat/send", axum::routing::post(chat::send))
        .route("/api/chat/poll", get(chat::poll))
        .route(
            "/api/chat/upload",
            axum::routing::post(chat::upload).layer(axum::extract::DefaultBodyLimit::max(5 << 20)),
        )
        .route("/api/chat/file/{name}", get(chat::file))
        .route("/books", get(reading::index))
        .route("/books/c/{c}", get(reading::category))
        .route("/books/cover/{id}", get(reading::cover))
        .route("/read", get(reading::reader_redirect))
        .route("/read/", get(reading::reader_index))
        .route("/read/book/{name}", get(reading::epub))
        .route("/read/{f}", get(reading::reader_file))
        .route("/app.apk", get(offer::apk));
    if let Some(g) = &cfg.games {
        r = r.nest_service("/game", tower_http::services::ServeDir::new(g));
    }
    if !cfg.downloads && cfg.installed.is_some() {
        // a phone: what it has, for other phones running the app
        r = r
            .route("/api/catalog", get(offer::catalog))
            .route("/api/books", get(offer::books))
            .route("/download/{*path}", get(offer::download));
    }
    if cfg.downloads {
        r = r.route("/api/catalog", get(catalog::handler));
        for (id, dir) in &downloads {
            r = r.nest_service(&format!("/download/collections/{id}"), tower_http::services::ServeDir::new(dir));
        }
        if let Some(d) = &model_dir {
            r = r.nest_service("/download/models", tower_http::services::ServeDir::new(d));
        }
        if let Some(m) = &cfg.maps {
            r = r.nest_service("/download/maps", tower_http::services::ServeDir::new(m));
        }
        if let Some(g) = &cfg.games {
            r = r.nest_service("/download/games", tower_http::services::ServeDir::new(g));
        }
        if let Some(dir) = cfg.llm.as_deref().map(ask::llm_dir) {
            r = r.nest_service("/download/llm", tower_http::services::ServeDir::new(dir));
        }
        if let Some(b) = &cfg.books {
            // ~10 MB of JSON for 50k books; shelf.json.gz is sent to clients that accept gzip
            if app.books.as_ref().is_some_and(|ix| ix.filtered.is_some()) {
                r = r.route("/api/books", get(books::shelf)); // only some of the shelf is here
            } else {
                r = r.route_service("/api/books", tower_http::services::ServeFile::new(b.join("shelf.json")).precompressed_gzip());
            }
            r = r.route("/api/books/search", get(books::search)).route("/api/books/{id}", get(books::details));
            r = r.nest_service("/download/books", tower_http::services::ServeDir::new(b));
        }
    }
    r.with_state(app)
}

/// Semantic search is on: a query model is loaded and some collection has vectors.
pub fn has_semantic(app: &App) -> bool {
    app.embedder.is_some() && app.lib.has_dense()
}

pub fn describe(app: &App) -> String {
    format!(
        "{} collections, {} docs, {} chunks, dense {}",
        app.lib.cols.len(),
        app.lib.doc_count(),
        app.lib.chunk_count(),
        app.embedder.is_some() && app.lib.has_dense()
    )
}
