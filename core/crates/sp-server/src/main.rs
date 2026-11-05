//! sp-server binary: the hub / desktop server. See lib.rs for routes.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Bothy HTTP server")]
struct Args {
    /// Collection pack directories (each one becomes a collection)
    packs: Vec<PathBuf>,
    /// Library directory: every subdirectory holding a manifest.json is a collection, models/ the embedding GGUF
    #[arg(long)]
    library: Option<PathBuf>,
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen: String,
    /// Embedding GGUF (default: <library>/models/*.gguf, else a pack's models/*.gguf). Without one, search is BM25 only.
    #[arg(long)]
    model: Option<PathBuf>,
    /// CPU threads for query embedding (default: SP_THREADS or ~80% of cores)
    #[arg(long)]
    threads: Option<usize>,
    /// Offline maps dir: region *.pmtiles (+ .places/) and assets/ (DESIGN §16)
    #[arg(long)]
    maps: Option<PathBuf>,
    /// Games dir: one subdirectory per game with index.html and game.json
    #[arg(long)]
    games: Option<PathBuf>,
    /// Chat model for /ask (a GGUF, or a directory holding one); loaded on the first question
    #[arg(long)]
    llm: Option<PathBuf>,
    /// Publish /api/catalog and /download/… so the mobile app can fetch content from this hub
    #[arg(long)]
    downloads: bool,
    /// Ebook shelf dir (shelf.json, epub/, covers/), published with --downloads at /api/books, /download/books
    #[arg(long)]
    books: Option<PathBuf>,
    /// Chat for people on this Wi-Fi (/chat): directory for its messages
    #[arg(long)]
    chat: Option<PathBuf>,
    /// Also serve HTTPS here (e.g. 0.0.0.0:8443) with a self-signed certificate kept in the chat dir,
    /// so chat can use the microphone and GPS (browsers allow those only on secure pages). Needs --chat.
    #[arg(long)]
    https: Option<String>,
    /// The Android app's APK, offered at /app.apk (default: <maps>/survivalpedia.apk when present)
    #[arg(long)]
    apk: Option<PathBuf>,
    /// The app's installed.json: show its books at /books, as a sharing phone does (for testing)
    #[arg(long)]
    installed: Option<PathBuf>,
    /// Write a static copy of the download catalog to this directory (api/catalog plus download/… as
    /// symlinks) for the download server, then exit. Implies --downloads.
    #[arg(long)]
    export_catalog: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut a = Args::parse();
    if a.apk.is_none() {
        a.apk = a.maps.as_ref().map(|m| m.join("survivalpedia.apk")).filter(|p| p.is_file());
    }
    if a.export_catalog.is_some() {
        a.downloads = true;
    }
    let cfg = sp_server::Config { library: a.library, packs: a.packs, model: a.model, threads: a.threads, maps: a.maps, games: a.games, downloads: a.downloads, llm: a.llm, books: a.books, chat: a.chat, installed: a.installed, apk: a.apk };
    let app = sp_server::load(&cfg)?;
    if let Some(out) = &a.export_catalog {
        let cat = app.catalog.as_ref().ok_or_else(|| anyhow::anyhow!("no catalog"))?;
        let (n, bytes) = sp_server::catalog::export(cat, &sp_server::download_roots(&app, &cfg), out)?;
        eprintln!("exported {n} files, {:.2} GB, to {}", bytes as f64 / 1e9, out.display());
        return Ok(());
    }
    let listener = tokio::net::TcpListener::bind(&a.listen).await?;
    eprintln!("sp-server: {}; listening on http://{}", sp_server::describe(&app), a.listen);
    let router = sp_server::router(app.clone(), &cfg);
    if let Some(addr) = &a.https {
        let chat = app.chat.as_ref().ok_or_else(|| anyhow::anyhow!("--https is for chat: add --chat DIR"))?;
        let tls = sp_server::tls::config(chat.dir())?;
        let l = tokio::net::TcpListener::bind(addr).await?;
        chat.set_https_port(l.local_addr()?.port());
        eprintln!("sp-server: also https://{addr} (self-signed certificate in {})", chat.dir().display());
        let r = router.clone();
        let l = sp_server::tls::Listener::new(l, tls)?;
        tokio::spawn(async move {
            let _ = axum::serve(l, r).await;
        });
    }
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}
