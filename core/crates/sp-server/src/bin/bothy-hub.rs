//! bothy-hub: a Bothy hub for people who aren't developers.
//!
//!   bothy-hub get [--preset small|standard|everything] [--add ID,..] [--skip ID,..] [--from URL]
//!       download content (or update it) from the download server into the data folder; resumable,
//!       every file checked against its BLAKE3 hash
//!   bothy-hub check [--full]      is everything there and ready? (--full re-hashes every file)
//!   bothy-hub start [--port 8080] serve the hub; with nothing downloaded yet, it runs `get` first
//!   bothy-hub                     the same as `start` (so double-clicking it works)
//!
//! The data folder is `--data DIR`, else $BOTHY_DATA, else `bothy-data` next to the program. It uses
//! the app's layout (collections/, models/, maps/, games/, llm/), and `installed.json` records what
//! is complete, keyed by catalog component id (DATA-FORMATS §4).

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_FROM: &str = "https://data.pocketbothy.com/v1";

#[derive(Parser)]
#[command(name = "bothy-hub", version, about = "Bothy hub: offline knowledge, maps, games and chat for everyone on your Wi-Fi")]
struct Cli {
    /// Data folder (default: $BOTHY_DATA, else bothy-data next to this program)
    #[arg(long, global = true)]
    data: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download or update content
    Get(GetArgs),
    /// Check that everything downloaded is complete, and show what the hub will offer
    Check {
        /// Re-hash every file (slow, but catches damaged files)
        #[arg(long)]
        full: bool,
    },
    /// Start the hub
    Start(StartArgs),
}

#[derive(clap::Args, Clone, Default)]
struct GetArgs {
    /// small (~3 GB), standard (~8 GB) or everything (~13 GB); asked when not given and there's a terminal
    #[arg(long, env = "BOTHY_PRESET")]
    preset: Option<String>,
    /// Extra components, by id or id prefix (e.g. map:uk, col:ifixit:images)
    #[arg(long, value_delimiter = ',', env = "BOTHY_ADD")]
    add: Vec<String>,
    /// Components to leave out, by id or id prefix (e.g. llm, map:uk)
    #[arg(long, value_delimiter = ',', env = "BOTHY_SKIP")]
    skip: Vec<String>,
    /// Download server (anything serving /api/catalog and /download/…, e.g. another hub)
    #[arg(long, env = "BOTHY_FROM", default_value = DEFAULT_FROM)]
    from: String,
    /// Don't ask before downloading
    #[arg(long, short)]
    yes: bool,
}

#[derive(clap::Args, Clone)]
struct StartArgs {
    #[arg(long, env = "BOTHY_PORT", default_value_t = 8080)]
    port: u16,
    /// HTTPS port, which chat needs for voice messages and GPS (0 = off)
    #[arg(long, env = "BOTHY_HTTPS_PORT", default_value_t = 8443)]
    https_port: u16,
    /// Don't let phones copy content from this hub
    #[arg(long)]
    no_downloads: bool,
    /// No chat
    #[arg(long)]
    no_chat: bool,
    /// How to fetch content on first start, when nothing is downloaded yet
    #[command(flatten)]
    get: GetArgs,
}

fn data_dir(cli: &Cli) -> PathBuf {
    if let Some(d) = &cli.data {
        return d.clone();
    }
    if let Some(d) = std::env::var_os("BOTHY_DATA") {
        return d.into();
    }
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    exe.unwrap_or_else(|| PathBuf::from(".")).join("bothy-data")
}

fn gb(b: u64) -> String {
    if b >= 1_000_000_000 { format!("{:.1} GB", b as f64 / 1e9) } else { format!("{} MB", (b as f64 / 1e6).ceil() as u64) }
}

fn ask(q: &str) -> String {
    print!("{q}");
    std::io::stdout().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s).ok();
    s.trim().to_string()
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

// ---------- installed.json ----------

fn installed_path(dir: &Path) -> PathBuf {
    dir.join("installed.json")
}

fn load_installed(dir: &Path) -> BTreeMap<String, Value> {
    std::fs::read(installed_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_installed(dir: &Path, m: &BTreeMap<String, Value>) -> Result<()> {
    let tmp = dir.join("installed.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    std::fs::rename(tmp, installed_path(dir))?;
    Ok(())
}

// ---------- catalog and selection ----------

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .user_agent(concat!("bothy-hub/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn fetch_catalog(agent: &ureq::Agent, from: &str) -> Result<Value> {
    let url = format!("{}/api/catalog", from.trim_end_matches('/'));
    let mut r = agent.get(&url).call().with_context(|| format!("couldn't reach the download server ({url}). Is this computer online?"))?;
    let b = r.body_mut().with_config().limit(64 << 20).read_to_vec()?;
    let c: Value = serde_json::from_slice(&b).context("the download server sent something that isn't a catalog")?;
    if c["components"].as_array().is_none_or(|a| a.is_empty()) {
        bail!("the download server's catalog is empty");
    }
    Ok(c)
}

fn size(c: &Value) -> u64 {
    c["size"].as_u64().unwrap_or(0)
}

/// Component ids for a preset. Images bigger than 1 GB (wikiHow, iFixit) and the detailed map regions
/// only come with `everything`; compact (128-dim) indexes too, since a hub has room for full ones and
/// they only matter to phones copying from the hub.
fn preset(cat: &Value, name: &str) -> Result<BTreeSet<String>> {
    let level = match name {
        "small" | "s" | "1" => 1,
        "standard" | "std" | "2" | "" => 2,
        "everything" | "all" | "full" | "3" => 3,
        _ => bail!("unknown preset {name:?}: use small, standard or everything"),
    };
    let mut ids = BTreeSet::new();
    for c in cat["components"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or_default();
        let take = if id.contains(":vec:") {
            level == 3 || !id.ends_with(".d128")
        } else if id.ends_with(":images") {
            level == 3 || (level == 2 && size(c) < 1_000_000_000)
        } else if id.starts_with("map:") {
            level == 3 || id == "map:world" || (level == 2 && size(c) < 4_000_000_000)
        } else if id == "llm" {
            level >= 2
        } else {
            true // articles, the search model, map assets, games
        };
        if take {
            ids.insert(id.to_string());
        }
    }
    Ok(ids)
}

fn matches(id: &str, pats: &[String]) -> bool {
    pats.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).any(|p| id == p || id.starts_with(&format!("{p}:")) || (p.ends_with(':') && id.starts_with(p)))
}

/// The chosen ids plus everything they require; components whose requirements were skipped are dropped.
fn resolve(cat: &Value, mut ids: BTreeSet<String>, a: &GetArgs) -> BTreeSet<String> {
    let comps: BTreeMap<&str, &Value> =
        cat["components"].as_array().into_iter().flatten().filter_map(|c| Some((c["id"].as_str()?, c))).collect();
    for id in comps.keys() {
        if matches(id, &a.add) {
            ids.insert(id.to_string());
        }
    }
    ids.retain(|id| !matches(id, &a.skip));
    loop {
        let mut more = Vec::new();
        for id in &ids {
            for r in comps.get(id.as_str()).and_then(|c| c["requires"].as_array()).into_iter().flatten() {
                if let Some(r) = r.as_str().filter(|r| !ids.contains(*r) && comps.contains_key(r) && !matches(r, &a.skip)) {
                    more.push(r.to_string());
                }
            }
        }
        if more.is_empty() {
            break;
        }
        ids.extend(more);
    }
    let ok = |id: &String| {
        comps.get(id.as_str()).is_some_and(|c| c["requires"].as_array().into_iter().flatten().all(|r| r.as_str().is_some_and(|r| ids.contains(r))))
    };
    let keep: BTreeSet<String> = ids.iter().filter(|i| ok(i)).cloned().collect();
    keep
}

fn total(cat: &Value, ids: &BTreeSet<String>) -> u64 {
    cat["components"].as_array().into_iter().flatten().filter(|c| c["id"].as_str().is_some_and(|i| ids.contains(i))).map(size).sum()
}

fn describe_choice(cat: &Value, ids: &BTreeSet<String>) -> Vec<String> {
    let groups: BTreeMap<String, String> = cat["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|g| Some((g["id"].as_str()?.to_string(), g["title"].as_str()?.to_string())))
        .collect();
    let mut lines = Vec::new();
    let mut by_group: BTreeMap<String, (u64, Vec<String>)> = BTreeMap::new();
    for c in cat["components"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or_default();
        if !ids.contains(id) {
            continue;
        }
        let g = c["group"].as_str().unwrap_or("other").to_string();
        let e = by_group.entry(g).or_default();
        e.0 += size(c);
        let part = if id.ends_with(":images") {
            "pictures".to_string()
        } else if id.contains(":vec:") {
            "search by meaning".to_string()
        } else if id.starts_with("col:") {
            "articles".to_string()
        } else if let Some(m) = id.strip_prefix("map:") {
            m.to_string()
        } else {
            String::new()
        };
        if !part.is_empty() && !e.1.contains(&part) {
            e.1.push(part);
        }
    }
    for (g, (bytes, parts)) in by_group {
        let title = match g.as_str() {
            "models" => "Search-by-meaning model".to_string(),
            "maps" => format!("Maps ({})", parts.iter().filter(|p| *p != "pictures").cloned().collect::<Vec<_>>().join(", ")),
            "games" => "Games".to_string(),
            "ai" => "AI answers (needs about 3 GB of free memory)".to_string(),
            _ => format!("{} ({})", groups.get(&g).cloned().unwrap_or(g.clone()), parts.join(", ")),
        };
        lines.push(format!("  {:>8}  {title}", gb(bytes)));
    }
    lines
}

// ---------- downloading ----------

fn hash_file(p: &Path) -> Result<String> {
    Ok(blake3::Hasher::new().update_mmap_rayon(p)?.finalize().to_hex().to_string())
}

fn safe_rel(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.contains('\\') && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// Percent-encodes a relative path for a URL, keeping the slashes (font names have spaces).
fn encode_path(p: &str) -> String {
    let mut s = String::with_capacity(p.len());
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

struct Progress {
    total: u64,
    done: u64,
    t0: Instant,
    last: Instant,
}

impl Progress {
    fn show(&mut self, extra: u64, force: bool) {
        let tty = std::io::stdout().is_terminal();
        if !force && self.last.elapsed() < Duration::from_millis(if tty { 700 } else { 15_000 }) {
            return;
        }
        self.last = Instant::now();
        let now = self.done + extra;
        let secs = self.t0.elapsed().as_secs_f64().max(0.1);
        let rate = now as f64 / secs;
        let eta = if rate > 1.0 { ((self.total.saturating_sub(now)) as f64 / rate) as u64 } else { 0 };
        let pct = if self.total > 0 { now * 100 / self.total } else { 100 };
        print!(
            "{}  {} of {}  ({pct}%)  {:.1} MB/s  about {} left     {}",
            if tty { "\r" } else { "" },
            gb(now),
            gb(self.total),
            rate / 1e6,
            if eta >= 3600 { format!("{}h {:02}m", eta / 3600, eta / 60 % 60) } else { format!("{}m {:02}s", eta / 60, eta % 60) },
            if tty { "" } else { "\n" }
        );
        std::io::stdout().flush().ok();
    }
}

fn download(agent: &ureq::Agent, from: &str, dir: &Path, f: &Value, prog: &mut Progress) -> Result<()> {
    let rel = f["path"].as_str().unwrap_or_default();
    anyhow::ensure!(safe_rel(rel), "unsafe path in catalog: {rel:?}");
    let want = f["size"].as_u64().unwrap_or(0);
    let hash = f["blake3"].as_str().unwrap_or_default();
    let dst = dir.join(rel);
    let part = dir.join(format!("{rel}.part"));
    std::fs::create_dir_all(dst.parent().unwrap())?;
    let url = format!("{}/download/{}", from.trim_end_matches('/'), encode_path(rel));
    let mut tries = 0;
    loop {
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let res = (|| -> Result<()> {
            if have < want {
                let mut req = agent.get(&url);
                if have > 0 {
                    req = req.header("Range", &format!("bytes={have}-"));
                }
                let mut r = req.call()?;
                let append = have > 0 && r.status() == 206;
                let mut out = std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&part)?;
                let mut got = if append { have } else { 0 };
                let mut rd = r.body_mut().as_reader();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = rd.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])?;
                    got += n as u64;
                    prog.show(got, false);
                }
                out.sync_all()?;
            }
            if want == 0 {
                std::fs::File::create(&part)?;
            }
            let len = std::fs::metadata(&part)?.len();
            anyhow::ensure!(len == want, "got {len} bytes, expected {want}");
            if !hash.is_empty() && hash_file(&part)? != hash {
                std::fs::remove_file(&part)?;
                bail!("damaged in transit (hash mismatch)");
            }
            std::fs::rename(&part, &dst)?;
            Ok(())
        })();
        match res {
            Ok(()) => return Ok(()),
            Err(e) if tries < 8 => {
                tries += 1;
                println!("\n  {rel}: {e:#}; retrying ({tries}/8)…");
                std::thread::sleep(Duration::from_secs(2u64.pow(tries.min(5))));
            }
            Err(e) => return Err(e).with_context(|| format!("downloading {rel}")),
        }
    }
}

fn file_ok(dir: &Path, f: &Value) -> bool {
    let rel = f["path"].as_str().unwrap_or_default();
    safe_rel(rel) && std::fs::metadata(dir.join(rel)).is_ok_and(|m| m.len() == f["size"].as_u64().unwrap_or(u64::MAX))
}

fn get(dir: &Path, a: &GetArgs, first_run: bool) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("can't create the data folder {}", dir.display()))?;
    let agent = agent();
    println!("Fetching the catalog from {} …", a.from);
    let cat = fetch_catalog(&agent, &a.from)?;
    let mut installed = load_installed(dir);
    let free = fs4::available_space(dir).unwrap_or(0);

    let name = match &a.preset {
        Some(p) => p.clone(),
        None if !installed.is_empty() => "__update".into(),
        None if interactive() && !a.yes => {
            let sizes: Vec<u64> = ["small", "standard", "everything"].iter().map(|p| total(&cat, &resolve(&cat, preset(&cat, p).unwrap(), a))).collect();
            println!();
            println!("What should this hub hold? Free space here: {}", gb(free));
            println!("  1) Small       {:>8}  all articles and guides, search by meaning, first aid, games, world map", gb(sizes[0]));
            println!("  2) Standard    {:>8}  + pictures for Wikipedia and the survival guides, AI answers, UK map", gb(sizes[1]));
            println!("  3) Everything  {:>8}  + all pictures (wikiHow, iFixit), compact indexes for phones", gb(sizes[2]));
            ask("Choose 1, 2 or 3 [2]: ")
        }
        None => "standard".into(),
    };
    let ids = if name == "__update" {
        // keep what's installed, plus anything new in groups already chosen (e.g. a new index)
        installed.keys().cloned().collect::<BTreeSet<_>>()
    } else {
        preset(&cat, &name)?
    };
    let ids = resolve(&cat, ids, a);
    let comps: Vec<&Value> = cat["components"].as_array().into_iter().flatten().filter(|c| c["id"].as_str().is_some_and(|i| ids.contains(i))).collect();

    // what's missing or changed (a component is current when its files match the catalog's)
    let todo: Vec<&Value> = comps.iter().copied().filter(|c| installed.get(c["id"].as_str().unwrap_or_default()) != Some(*c) || !c["files"].as_array().into_iter().flatten().all(|f| file_ok(dir, f))).collect();
    let need: u64 = todo.iter().flat_map(|c| c["files"].as_array().into_iter().flatten()).filter(|f| !file_ok(dir, f) || !installed.values().any(|i| i["files"].as_array().is_some_and(|fs| fs.contains(f)))).filter_map(|f| f["size"].as_u64()).sum();

    println!();
    println!("This hub will hold {}:", gb(total(&cat, &ids)));
    for l in describe_choice(&cat, &ids) {
        println!("{l}");
    }
    if todo.is_empty() {
        println!("\nEverything is already downloaded and up to date.");
        return remove_unchosen(dir, &mut installed, &ids);
    }
    println!("\nTo download: {} (free space: {})", gb(need), gb(free));
    if need + 200_000_000 > free {
        bail!("not enough free space in {}: need {}, have {}. Free some space, choose a smaller preset, or use --data to pick another disk", dir.display(), gb(need), gb(free));
    }
    if interactive() && !a.yes && !first_run {
        let y = ask("Start downloading? [Y/n]: ");
        if y.eq_ignore_ascii_case("n") || y.eq_ignore_ascii_case("no") {
            return Ok(());
        }
    }
    println!("Downloading into {} (you can stop with Ctrl+C and run this again to resume)", dir.display());
    let mut prog = Progress { total: need, done: 0, t0: Instant::now(), last: Instant::now() };
    for c in &todo {
        let id = c["id"].as_str().unwrap_or_default();
        for f in c["files"].as_array().into_iter().flatten() {
            let current = file_ok(dir, f) && installed.values().any(|i| i["files"].as_array().is_some_and(|fs| fs.contains(f)));
            if current {
                continue;
            }
            download(&agent, &a.from, dir, f, &mut prog)?;
            prog.done += f["size"].as_u64().unwrap_or(0);
            prog.show(0, false);
        }
        installed.insert(id.to_string(), (*c).clone());
        save_installed(dir, &installed)?;
    }
    prog.show(0, true);
    println!("\nDone. Every file was checked against its fingerprint.");
    remove_unchosen(dir, &mut installed, &ids)?;
    if !first_run {
        println!("If the hub is running, restart it to use the new content (with Docker: docker restart bothy).");
    }
    Ok(())
}

/// What the hub offers, in plain words.
fn summary(app: &sp_server::App, cfg: &sp_server::Config) -> Vec<String> {
    let mut v = Vec::new();
    for c in &app.lib.cols {
        v.push(format!("{} ({} articles)", c.title, c.searcher.pack.doc_count()));
    }
    v.push(if sp_server::has_semantic(app) { "Search by meaning and by keyword".into() } else { "Search by keyword".into() });
    v.push("First aid cards".into());
    if cfg.llm.is_some() {
        v.push("AI answers".into());
    }
    if let Some(m) = &app.maps {
        v.push(format!("Maps: {}", m.regions.iter().map(|r| r.title.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if let Some(g) = &app.games {
        v.push(format!("{} games", g.list.len()));
    }
    if app.chat.is_some() {
        v.push("Chat".into());
    }
    if cfg.downloads {
        v.push("Downloads for phones with the Bothy app".into());
    }
    v
}

/// Deletes the files of installed components that are no longer chosen (a smaller preset, or --skip),
/// keeping any file another chosen component still lists.
fn remove_unchosen(dir: &Path, installed: &mut BTreeMap<String, Value>, ids: &BTreeSet<String>) -> Result<()> {
    let gone: Vec<String> = installed.keys().filter(|k| !ids.contains(*k)).cloned().collect();
    if gone.is_empty() {
        return Ok(());
    }
    let keep: BTreeSet<String> = installed
        .iter()
        .filter(|(k, _)| ids.contains(*k))
        .flat_map(|(_, c)| c["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str().map(String::from)))
        .collect();
    let mut freed = 0;
    for k in &gone {
        for f in installed[k]["files"].as_array().into_iter().flatten() {
            let rel = f["path"].as_str().unwrap_or_default();
            if safe_rel(rel) && !keep.contains(rel) && std::fs::remove_file(dir.join(rel)).is_ok() {
                freed += f["size"].as_u64().unwrap_or(0);
            }
        }
        installed.remove(k);
    }
    save_installed(dir, installed)?;
    println!("Removed {} that you no longer chose ({} freed).", gone.join(", "), gb(freed));
    Ok(())
}

// ---------- check ----------

fn layout(dir: &Path) -> sp_server::Config {
    let some = |p: PathBuf| p.exists().then_some(p);
    let model = std::fs::read_dir(dir.join("models"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "gguf"));
    let llm = std::fs::read_dir(dir.join("llm"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "gguf"));
    sp_server::Config {
        library: some(dir.join("collections")),
        model,
        maps: some(dir.join("maps")).filter(|m| m.join("assets").is_dir()),
        games: some(dir.join("games")),
        llm,
        ..Default::default()
    }
}

fn check(dir: &Path, full: bool) -> Result<bool> {
    let installed = load_installed(dir);
    if installed.is_empty() {
        println!("Nothing is downloaded yet in {}. Run: bothy-hub get", dir.display());
        return Ok(false);
    }
    let files: Vec<&Value> = installed.values().flat_map(|c| c["files"].as_array().into_iter().flatten()).collect();
    let bytes: u64 = files.iter().filter_map(|f| f["size"].as_u64()).sum();
    println!("Checking {} files ({}) in {} …", files.len(), gb(bytes), dir.display());
    let mut bad = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let rel = f["path"].as_str().unwrap_or_default();
        let ok = file_ok(dir, f) && (!full || hash_file(&dir.join(rel)).ok().as_deref() == f["blake3"].as_str());
        if !ok {
            bad.push(rel.to_string());
        }
        if full && i % 50 == 0 {
            print!("\r  {}/{}", i + 1, files.len());
            std::io::stdout().flush().ok();
        }
    }
    if full {
        println!();
    }
    if !bad.is_empty() {
        println!("{} files are missing or damaged, e.g. {}", bad.len(), bad[0]);
        println!("Run: bothy-hub get   (it downloads only what's missing)");
        return Ok(false);
    }
    let cfg = layout(dir);
    let app = sp_server::load(&cfg)?;
    println!("\nAll files are complete. The hub will offer:");
    for l in summary(&app, &cfg) {
        println!("  - {l}");
    }
    println!("Free space left: {}", gb(fs4::available_space(dir).unwrap_or(0)));
    if in_docker() {
        println!("\nReady.");
    } else {
        println!("\nReady. Start it with: bothy-hub start");
    }
    Ok(true)
}

// ---------- start ----------

fn in_docker() -> bool {
    Path::new("/.dockerenv").exists()
}

fn lan_addresses() -> Vec<String> {
    // the address this computer would use to reach the local network (no packet is sent)
    let mut v = Vec::new();
    for probe in ["192.168.0.1:9", "10.0.0.1:9", "172.16.0.1:9", "8.8.8.8:9"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0")
            && s.connect(probe).is_ok()
            && let Ok(a) = s.local_addr()
            && !a.ip().is_loopback()
            && !a.ip().is_unspecified()
        {
            let ip = a.ip().to_string();
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
    }
    v
}

async fn start(dir: &Path, a: &StartArgs) -> Result<()> {
    if load_installed(dir).is_empty() {
        println!("Welcome to Bothy. Nothing is downloaded yet, so let's fetch the library first.\n");
        get(dir, &a.get, true)?;
    }
    let mut cfg = layout(dir);
    cfg.downloads = !a.no_downloads;
    if !a.no_chat {
        cfg.chat = Some(dir.join("chat"));
    }
    let app = sp_server::load(&cfg)?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", a.port))
        .await
        .with_context(|| format!("port {} is in use: is a hub already running? Try --port 8081", a.port))?;
    let router = sp_server::router(app.clone(), &cfg);
    if a.https_port != 0
        && let Some(chat) = app.chat.as_ref()
    {
        let tls = sp_server::tls::config(chat.dir())?;
        match tokio::net::TcpListener::bind(("0.0.0.0", a.https_port)).await {
            Ok(l) => {
                chat.set_https_port(a.https_port);
                let l = sp_server::tls::Listener::new(l, tls)?;
                let r = router.clone();
                tokio::spawn(async move {
                    let _ = axum::serve(l, r).await;
                });
            }
            Err(e) => eprintln!("HTTPS port {} unavailable ({e}); chat voice messages and GPS won't work", a.https_port),
        }
    }
    println!("\nBothy hub is running with:");
    for l in summary(&app, &cfg) {
        println!("  - {l}");
    }
    println!();
    if in_docker() {
        println!("Open http://<this computer's address>:{} on any phone or computer on the same network.", a.port);
    } else {
        let ips = lan_addresses();
        if ips.is_empty() {
            println!("Open http://<this computer's address>:{} on any phone or computer on the same network.", a.port);
        }
        for ip in ips {
            println!("Open http://{ip}:{} on any phone or computer on the same network.", a.port);
        }
        println!("On this computer: http://localhost:{}", a.port);
    }
    println!("{}", if in_docker() { "Stop it with: docker stop bothy" } else { "Stop it with Ctrl+C." });
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    let dir = data_dir(&cli);
    let res = match &cli.cmd {
        Some(Cmd::Get(a)) => get(&dir, a, false),
        Some(Cmd::Check { full }) => check(&dir, *full).map(|_| ()),
        Some(Cmd::Start(a)) => tokio::runtime::Runtime::new().map_err(Into::into).and_then(|rt| rt.block_on(start(&dir, a))),
        None => {
            let Some(Cmd::Start(a)) = Cli::parse_from(["bothy-hub", "start"]).cmd else { unreachable!() };
            tokio::runtime::Runtime::new().map_err(Into::into).and_then(|rt| rt.block_on(start(&dir, &a)))
        }
    };
    if let Err(e) = res {
        eprintln!("\nError: {e:#}");
        if cli.cmd.is_none() && interactive() {
            ask("Press Enter to close.");
        }
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cat() -> Value {
        json!({"components": [
            {"id": "col:w", "size": 10}, {"id": "col:w:images", "size": 2_000_000_000, "requires": ["col:w"]},
            {"id": "col:g", "size": 5}, {"id": "col:g:images", "size": 7, "requires": ["col:g"]},
            {"id": "col:w:vec:x.core.d256", "size": 3, "requires": ["col:w", "model"]},
            {"id": "col:w:vec:x.core.d128", "size": 2, "requires": ["col:w", "model"]},
            {"id": "model", "size": 1}, {"id": "map-assets", "size": 1},
            {"id": "map:world", "size": 1, "requires": ["map-assets"]}, {"id": "map:uk", "size": 3, "requires": ["map-assets"]},
            {"id": "llm", "size": 1}, {"id": "game:chess", "size": 1}
        ]})
    }

    #[test]
    fn presets_grow() {
        let c = cat();
        let s = resolve(&c, preset(&c, "small").unwrap(), &GetArgs::default());
        assert!(s.contains("col:w") && s.contains("col:w:vec:x.core.d256") && s.contains("model") && s.contains("map:world"));
        assert!(!s.contains("col:g:images") && !s.contains("llm") && !s.contains("map:uk") && !s.contains("col:w:vec:x.core.d128"));
        let m = resolve(&c, preset(&c, "standard").unwrap(), &GetArgs::default());
        assert!(m.contains("col:g:images") && m.contains("llm") && m.contains("map:uk") && !m.contains("col:w:images"));
        let e = resolve(&c, preset(&c, "everything").unwrap(), &GetArgs::default());
        assert_eq!(e.len(), 12);
    }

    #[test]
    fn skip_and_add_follow_requirements() {
        let c = cat();
        let a = GetArgs { skip: vec!["model".into(), "llm".into()], add: vec!["col:w:images".into()], ..Default::default() };
        let s = resolve(&c, preset(&c, "small").unwrap(), &a);
        assert!(!s.iter().any(|i| i.contains(":vec:")), "vectors need the model");
        assert!(s.contains("col:w:images") && !s.contains("llm"));
        let a = GetArgs { add: vec!["map:uk".into()], skip: vec!["map-assets".into()], ..Default::default() };
        let s = resolve(&c, preset(&c, "small").unwrap(), &a);
        assert!(!s.contains("map:uk") && !s.contains("map:world"), "maps need their assets");
    }

    #[test]
    fn paths_are_encoded() {
        assert_eq!(encode_path("maps/assets/fonts/Noto Sans v1/0-255.pbf"), "maps/assets/fonts/Noto%20Sans%20v1/0-255.pbf");
    }

    #[test]
    fn paths_are_checked() {
        assert!(safe_rel("collections/w/docs.idx"));
        assert!(!safe_rel("../etc/passwd") && !safe_rel("/etc/passwd") && !safe_rel("a//b") && !safe_rel("a\\b"));
    }
}
