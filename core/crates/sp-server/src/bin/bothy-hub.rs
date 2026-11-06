//! bothy-hub: a Bothy hub for people who aren't developers.
//!
//!   bothy-hub get [--preset small|standard|everything] [--add ID,..] [--skip ID,..] [--from URL]
//!       download content (or update it) from the download server into the data folder; resumable,
//!       every file checked against its BLAKE3 hash
//!   bothy-hub check [--full]      is everything there and ready? (--full re-hashes every file)
//!   bothy-hub start [--port 8080] serve the hub; with nothing downloaded yet, it runs `get` first
//!   bothy-hub                     the same as `start` (so double-clicking it works)
//!   bothy-hub maps [search|add|remove]   map regions
//!   bothy-hub reset-password      forget the Manage page's admin password
//!
//! A running hub has a Manage content page (`/manage`, behind an admin password set on the first visit): pick what to keep,
//! as in the app's Content tab; it downloads in the background, then reloads the content in place.
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    /// Forget the Manage page's admin password, so the next visit to /manage sets a new one
    ResetPassword,
    /// List the map regions, or add and remove them: maps; maps france; maps add fr us-ca; maps remove fr
    Maps {
        /// list [SEARCH], add NAME…, or remove NAME… (a name is an id like fr or us-ca, a country, or a place name)
        #[arg(default_value = "list")]
        action: String,
        names: Vec<String>,
        /// Download server
        #[arg(long, env = "BOTHY_FROM", default_value = DEFAULT_FROM)]
        from: String,
        /// Don't ask before downloading
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(clap::Args, Clone, Default)]
struct GetArgs {
    /// small (~3 GB), standard (~9 GB) or everything (~30 GB); asked when not given and there's a terminal. Maps come separately (bothy-hub maps)
    #[arg(long, env = "BOTHY_PRESET")]
    preset: Option<String>,
    /// Extra components, by id or id prefix (e.g. col:ifixit:images), or maps by id, country or name (map:fr, map:us, map:california)
    #[arg(long, value_delimiter = ',', env = "BOTHY_ADD")]
    add: Vec<String>,
    /// Components to leave out, by id or id prefix (e.g. llm, col:ifixit:images), or maps as in --add
    #[arg(long, value_delimiter = ',', env = "BOTHY_SKIP")]
    skip: Vec<String>,
    /// Download server (anything serving /api/catalog and /download/…, e.g. another hub)
    #[arg(long, env = "BOTHY_FROM", default_value = DEFAULT_FROM)]
    from: String,
    /// Don't ask before downloading
    #[arg(long, short)]
    yes: bool,
    /// Exactly these components (the Manage page's choice), instead of a preset or the current choice
    #[arg(skip)]
    exact: Option<BTreeSet<String>>,
    /// Report progress here (the Manage page) and stop when it's cancelled
    #[arg(skip)]
    job: Option<Arc<Job>>,
}

/// A download started from the Manage page.
#[derive(Default)]
struct Job {
    running: AtomicBool,
    cancel: AtomicBool,
    total: AtomicU64,
    done: AtomicU64,
    /// what happened last: "", "done", "cancelled" or an error
    outcome: Mutex<String>,
    step: Mutex<String>,
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

/// The components chosen by a `get` that hasn't finished yet (pending.json), if any.
fn pending(dir: &Path) -> Option<BTreeSet<String>> {
    std::fs::read(dir.join("pending.json")).ok().and_then(|b| serde_json::from_slice(&b).ok())
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

/// Component ids for a preset. Images bigger than 1 GB (wikiHow, iFixit) only come with `everything`;
/// compact (128-dim) indexes too, since a hub has room for full ones and they only matter to phones
/// copying from the hub. Maps: only the world overview; detailed regions are chosen one by one (`maps add`).
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
            id == "map:world"
        } else if id == "llm" {
            level >= 2
        } else if id.starts_with("shelf:") {
            // books: the 1,000 most read in every preset, 10,000 in standard, all of them in everything
            id == "shelf:top1000" || (level >= 2 && id == "shelf:top10000") || level == 3
        } else {
            true // articles, the search model, map assets, games
        };
        if take {
            ids.insert(id.to_string());
        }
    }
    Ok(ids)
}

/// Map components matching a name: an id (fr, map:fr, us-ca), a country id or name (us, france: all its
/// parts), or a region name (california). Case doesn't matter.
fn map_ids(cat: &Value, name: &str) -> Vec<String> {
    let n = name.trim().to_lowercase();
    let n = n.strip_prefix("map:").unwrap_or(&n);
    let maps = cat["components"].as_array().into_iter().flatten().filter(|c| c["id"].as_str().is_some_and(|i| i.starts_with("map:")));
    let low = |c: &Value, k: &str| c[k].as_str().unwrap_or_default().to_lowercase();
    maps.filter(|c| {
        let id = c["id"].as_str().unwrap_or_default();
        id[4..] == *n || low(c, "country") == n || low(c, "name") == n || low(c, "country_title") == n
    })
    .filter_map(|c| c["id"].as_str().map(String::from))
    .collect()
}

/// Components that `replaces` older ones (per-site indexes merged into one): chosen when any of those is.
fn with_replacements(cat: &Value, mut ids: BTreeSet<String>) -> BTreeSet<String> {
    for c in cat["components"].as_array().into_iter().flatten() {
        if c["replaces"].as_array().into_iter().flatten().any(|o| o.as_str().is_some_and(|o| ids.contains(o))) {
            ids.extend(c["id"].as_str().map(String::from));
        }
    }
    ids
}

/// A component that replaces older ones counts as installed when all of them are (same files), so
/// nothing is downloaded again or removed. Returns whether `installed` changed.
fn migrate(cat: &Value, installed: &mut BTreeMap<String, Value>) -> bool {
    let mut changed = false;
    for c in cat["components"].as_array().into_iter().flatten() {
        let old: Vec<&str> = c["replaces"].as_array().into_iter().flatten().filter_map(|o| o.as_str()).collect();
        if !old.is_empty() && old.iter().all(|o| installed.contains_key(*o)) {
            for o in &old {
                installed.remove(*o);
            }
            installed.insert(c["id"].as_str().unwrap_or_default().to_string(), c.clone());
            changed = true;
        }
    }
    changed
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
    // maps by country or name too: map:us (every state), map:france, map:california
    let by_name = |pats: &[String]| -> BTreeSet<String> {
        pats.iter().filter(|p| p.trim().starts_with("map:")).flat_map(|p| map_ids(cat, p)).collect()
    };
    ids.extend(by_name(&a.add));
    let skip_maps = by_name(&a.skip);
    ids.retain(|id| !matches(id, &a.skip) && !skip_maps.contains(id));
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
        } else if id.starts_with("shelf:") {
            c["title"].as_str().unwrap_or(id).trim_start_matches("The ").to_string()
        } else if id.starts_with("map:") {
            c["name"].as_str().or(id.strip_prefix("map:")).unwrap_or_default().to_string()
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
            "maps" => {
                let mut p: Vec<String> = parts.iter().filter(|p| *p != "pictures").cloned().collect();
                if p.len() > 8 {
                    let more = p.len() - 6;
                    p.truncate(6);
                    p.push(format!("and {more} more"));
                }
                format!("Maps ({})", p.join(", "))
            }
            "games" => "Games".to_string(),
            "shelf" => format!("Books ({})", parts.join(", ")),
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
    job: Option<Arc<Job>>,
}

impl Progress {
    fn show(&mut self, extra: u64, force: bool) {
        if let Some(j) = &self.job {
            j.total.store(self.total, Ordering::Relaxed);
            j.done.store(self.done + extra, Ordering::Relaxed);
        }
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
                    if prog.job.as_ref().is_some_and(|j| j.cancel.load(Ordering::Relaxed)) {
                        bail!("cancelled");
                    }
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
        let cancelled = prog.job.as_ref().is_some_and(|j| j.cancel.load(Ordering::Relaxed));
        match res {
            Ok(()) => return Ok(()),
            Err(e) if tries < 8 && !cancelled => {
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
    if migrate(&cat, &mut installed) {
        save_installed(dir, &installed)?;
    }
    let free = fs4::available_space(dir).unwrap_or(0);

    let name = match &a.preset {
        _ if a.exact.is_some() => "__exact".into(),
        Some(p) => p.clone(),
        None if pending(dir).is_some() => "__pending".into(),
        None if !installed.is_empty() => "__update".into(),
        None if interactive() && !a.yes => {
            let sizes: Vec<u64> = ["small", "standard", "everything"].iter().map(|p| total(&cat, &resolve(&cat, preset(&cat, p).unwrap(), a))).collect();
            println!();
            println!("What should this hub hold? Free space here: {}", gb(free));
            println!("  1) Small       {:>8}  all articles and guides, search by meaning, first aid, games, world map, 1,000 books", gb(sizes[0]));
            println!("  2) Standard    {:>8}  + pictures for Wikipedia and the survival guides, AI answers, 10,000 books", gb(sizes[1]));
            println!("  3) Everything  {:>8}  + all pictures (wikiHow, iFixit), all 54,000 books, compact indexes for phones", gb(sizes[2]));
            ask("Choose 1, 2 or 3 [2]: ")
        }
        None => "standard".into(),
    };
    let ids = if let Some(e) = &a.exact {
        e.clone()
    } else if name == "__pending" {
        // an earlier download didn't finish: carry on with what was chosen then
        pending(dir).unwrap_or_default()
    } else if name == "__update" {
        // keep what's installed, plus anything new in groups already chosen (e.g. a new index)
        installed.keys().cloned().collect::<BTreeSet<_>>()
    } else {
        // a preset (also given on every run by BOTHY_PRESET in Docker) keeps the map regions chosen before
        let mut ids = preset(&cat, &name)?;
        ids.extend(installed.keys().filter(|k| k.starts_with("map:")).cloned());
        ids
    };
    let ids = resolve(&cat, with_replacements(&cat, ids), a);
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
    if interactive() && !a.yes && !first_run && a.job.is_none() {
        let y = ask("Start downloading? [Y/n]: ");
        if y.eq_ignore_ascii_case("n") || y.eq_ignore_ascii_case("no") {
            return Ok(());
        }
    }
    std::fs::write(dir.join("pending.json"), serde_json::to_vec(&ids)?)?;
    println!("Downloading into {} (you can stop with Ctrl+C and run this again to resume)", dir.display());
    let mut prog = Progress { total: need, done: 0, t0: Instant::now(), last: Instant::now(), job: a.job.clone() };
    for c in &todo {
        let id = c["id"].as_str().unwrap_or_default();
        if let Some(j) = &a.job {
            *j.step.lock().unwrap() = c["title"].as_str().unwrap_or(id).to_string();
        }
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
    let _ = std::fs::remove_file(dir.join("pending.json"));
    if !first_run && a.job.is_none() {
        println!("If the hub is running, restart it to use the new content (with Docker: docker restart bothy), or next time use Manage on the hub's home page, which needs no restart.");
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
        let mut names: Vec<&str> = m.regions.iter().map(|r| r.title.as_str()).collect();
        if names.len() > 8 {
            let more = names.len() - 6;
            names.truncate(6);
            v.push(format!("Maps: {} and {more} more", names.join(", ")));
        } else {
            v.push(format!("Maps: {}", names.join(", ")));
        }
    }
    if let Some(b) = &app.books {
        v.push(format!("{} books to read or copy to phones", b.len()));
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
                // and folders left empty (a map's .places/)
                let mut d = dir.join(rel);
                while d.pop() && d != dir && std::fs::remove_dir(&d).is_ok() {}
            }
        }
        installed.remove(k);
    }
    save_installed(dir, installed)?;
    println!("Removed {} that you no longer chose ({} freed).", gone.join(", "), gb(freed));
    Ok(())
}

// ---------- maps ----------

fn maps(dir: &Path, action: &str, names: &[String], from: &str, yes: bool) -> Result<()> {
    let installed = load_installed(dir);
    let mut names: Vec<String> = names.iter().map(|n| n.trim_start_matches("map:").to_string()).collect();
    let action = match action {
        "list" | "ls" | "search" | "add" | "remove" | "rm" | "delete" => action,
        word => {
            names.insert(0, word.to_string()); // `bothy-hub maps france` searches
            "list"
        }
    };
    match action {
        "list" | "ls" | "search" => {
            let cat = fetch_catalog(&agent(), from)?;
            let q = names.join(" ").to_lowercase();
            let mut by: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
            for c in cat["components"].as_array().into_iter().flatten() {
                let id = c["id"].as_str().unwrap_or_default();
                let Some(short) = id.strip_prefix("map:") else { continue };
                let name = c["name"].as_str().unwrap_or(short);
                let country = c["country_title"].as_str().unwrap_or_default();
                if !q.is_empty() && ![short, name, country].iter().any(|s| s.to_lowercase().contains(&q)) {
                    continue;
                }
                by.entry(c["continent"].as_str().unwrap_or("Other").to_string()).or_default().push(c);
            }
            if by.is_empty() {
                println!("No map region matches {q:?}. Try a country name, e.g.: bothy-hub maps germany");
                return Ok(());
            }
            for (cont, mut cs) in by {
                println!("\n{cont}");
                cs.sort_by_key(|c| (c["country_title"].as_str().unwrap_or_default().to_string(), c["name"].as_str().unwrap_or_default().to_string()));
                for c in cs {
                    let id = c["id"].as_str().unwrap_or_default();
                    let mark = if installed.contains_key(id) { "✓" } else { " " };
                    let name = c["name"].as_str().unwrap_or(&id[4..]);
                    let name = match c["country_title"].as_str() {
                        Some(ct) => format!("{ct} · {name}"),
                        None => name.to_string(),
                    };
                    println!("  {mark} {:<10} {:>8}  {name}", &id[4..], gb(size(c)));
                }
            }
            println!("\n✓ = on this hub. Add one with: bothy-hub maps add <id or country>   (e.g. maps add fr, maps add us-ca, maps add germany)");
            Ok(())
        }
        "add" | "remove" | "rm" | "delete" => {
            if names.is_empty() {
                bail!("say which maps, e.g.: bothy-hub maps {action} fr");
            }
            let cat = fetch_catalog(&agent(), from)?;
            let mut ids = Vec::new();
            for n in &names {
                let found = map_ids(&cat, n);
                if found.is_empty() {
                    bail!("no map region called {n:?}; see the list with: bothy-hub maps {n}");
                }
                ids.extend(found);
            }
            if installed.is_empty() && pending(dir).is_none() {
                bail!("this hub has nothing downloaded yet: start it once first (or run bothy-hub get), then add maps");
            }
            let mut a = GetArgs { from: from.to_string(), yes, ..Default::default() };
            if action == "add" {
                a.add = ids;
            } else {
                let missing: Vec<&String> = ids.iter().filter(|i| !installed.contains_key(*i)).collect();
                if missing.len() == ids.len() {
                    println!("None of those maps is on this hub.");
                    return Ok(());
                }
                a.skip = ids;
            }
            // keep the current choice (not BOTHY_PRESET), plus or minus these maps
            get(dir, &a, false)
        }
        _ => unreachable!(),
    }
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
        books: some(dir.join("books")).filter(|b| b.join("shelf.json").is_file()),
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

// ---------- the Manage content page ----------

const PW_ITER: u32 = 200_000;

fn random_hex(n: usize) -> Result<String> {
    let mut b = vec![0u8; n];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).map_err(|_| anyhow::anyhow!("no system randomness"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok())).collect()
}

/// The Manage page's password, set on the first visit: PBKDF2-SHA256 with a random salt, in
/// `<data>/admin-password`.
fn password_path(dir: &Path) -> PathBuf {
    dir.join("admin-password")
}

fn set_password(dir: &Path, pw: &str) -> Result<()> {
    let salt = random_hex(16)?;
    let mut hash = [0u8; 32];
    ring::pbkdf2::derive(ring::pbkdf2::PBKDF2_HMAC_SHA256, std::num::NonZeroU32::new(PW_ITER).unwrap(), salt.as_bytes(), pw.as_bytes(), &mut hash);
    let rec = serde_json::json!({"kdf": "pbkdf2-sha256", "iter": PW_ITER, "salt": salt, "hash": hash.iter().map(|x| format!("{x:02x}")).collect::<String>()});
    let tmp = dir.join("admin-password.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&rec)?)?;
    std::fs::rename(tmp, password_path(dir))?;
    Ok(())
}

/// None when no password is set yet.
fn check_password(dir: &Path, pw: &str) -> Option<bool> {
    let rec: Value = serde_json::from_slice(&std::fs::read(password_path(dir)).ok()?).ok()?;
    let iter = std::num::NonZeroU32::new(rec["iter"].as_u64()? as u32)?;
    let hash = unhex(rec["hash"].as_str()?)?;
    let salt = rec["salt"].as_str()?;
    Some(ring::pbkdf2::verify(ring::pbkdf2::PBKDF2_HMAC_SHA256, iter, salt.as_bytes(), pw.as_bytes(), &hash).is_ok())
}

fn manage_links(port: u16) -> Vec<String> {
    if in_docker() {
        return vec![format!("http://<this computer's address>:{port}/manage")];
    }
    let mut v: Vec<String> = lan_addresses().iter().map(|ip| format!("http://{ip}:{port}/manage")).collect();
    v.push(format!("http://localhost:{port}/manage"));
    v
}

/// The hub's content: the library app and its router, keeping `chat` if given (so a reload keeps it).
fn build(dir: &Path, a: &StartArgs, chat: Option<Arc<sp_server::chat::Chat>>) -> Result<(Arc<sp_server::App>, axum::Router, sp_server::Config)> {
    let mut cfg = layout(dir);
    cfg.downloads = !a.no_downloads;
    cfg.manage = true;
    if !a.no_chat && chat.is_none() {
        cfg.chat = Some(dir.join("chat"));
    }
    let mut app = sp_server::load(&cfg)?;
    if chat.is_some() {
        Arc::get_mut(&mut app).context("app is shared")?.chat = chat;
    }
    let router = sp_server::router(app.clone(), &cfg);
    Ok((app, router, cfg))
}

struct Manage {
    dir: PathBuf,
    start: StartArgs,
    /// signed-in browsers (random tokens, forgotten on restart)
    sessions: Mutex<BTreeSet<String>>,
    /// wrong passwords in a row, and when the last one was
    fails: Mutex<(u32, Option<Instant>)>,
    job: Arc<Job>,
    catalog: Mutex<Option<(Instant, Value)>>,
    live: Arc<std::sync::RwLock<axum::Router>>,
    chat: Option<Arc<sp_server::chat::Chat>>,
}

impl Manage {
    fn authed(&self, h: &axum::http::HeaderMap) -> bool {
        h.get("x-bothy-session").and_then(|v| v.to_str().ok()).is_some_and(|k| self.sessions.lock().unwrap().contains(k))
    }

    fn catalog(&self, fresh: bool) -> Result<Value> {
        if let Some((t, c)) = self.catalog.lock().unwrap().as_ref()
            && !fresh
            && t.elapsed() < Duration::from_secs(600)
        {
            return Ok(c.clone());
        }
        let c = fetch_catalog(&agent(), &self.start.get.from)?;
        *self.catalog.lock().unwrap() = Some((Instant::now(), c.clone()));
        Ok(c)
    }

    /// Re-open the content after a download and serve it from now on.
    fn reload(&self) -> Result<()> {
        let (app, router, cfg) = build(&self.dir, &self.start, self.chat.clone())?;
        *self.live.write().unwrap() = router;
        println!("Content reloaded:");
        for l in summary(&app, &cfg) {
            println!("  - {l}");
        }
        Ok(())
    }
}

mod web {
    use super::*;
    use axum::Json;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{Html, IntoResponse, Response};

    pub async fn page() -> Html<&'static str> {
        Html(include_str!("manage.html"))
    }

    fn job_json(j: &Job) -> Value {
        serde_json::json!({
            "running": j.running.load(Ordering::Relaxed),
            "total": j.total.load(Ordering::Relaxed),
            "done": j.done.load(Ordering::Relaxed),
            "step": *j.step.lock().unwrap(),
            "outcome": *j.outcome.lock().unwrap(),
        })
    }

    pub async fn state(State(m): State<Arc<Manage>>, h: HeaderMap, q: axum::extract::Query<BTreeMap<String, String>>) -> Response {
        if !m.authed(&h) {
            return (StatusCode::FORBIDDEN, "Sign in with the admin password").into_response();
        }
        let fresh = q.contains_key("fresh");
        let m2 = m.clone();
        let cat = tokio::task::spawn_blocking(move || m2.catalog(fresh)).await.map_err(anyhow::Error::from).and_then(|r| r);
        let mut inst = load_installed(&m.dir);
        if let Ok(c) = &cat
            && !m.job.running.load(Ordering::SeqCst)
            && migrate(c, &mut inst)
        {
            let _ = save_installed(&m.dir, &inst);
        }
        let installed: Vec<String> = inst.into_keys().collect();
        let free = fs4::available_space(&m.dir).unwrap_or(0);
        Json(serde_json::json!({
            "from": m.start.get.from,
            "catalog": cat.as_ref().ok(),
            "error": cat.as_ref().err().map(|e| format!("Couldn't reach the download server {}: {e:#}", m.start.get.from)),
            "installed": installed,
            "free": free,
            "job": job_json(&m.job),
        }))
        .into_response()
    }

    pub async fn status(State(m): State<Arc<Manage>>, h: HeaderMap) -> Json<Value> {
        Json(serde_json::json!({"password_set": password_path(&m.dir).is_file(), "signed_in": m.authed(&h)}))
    }

    fn session(m: &Manage) -> Response {
        match random_hex(24) {
            Ok(t) => {
                m.sessions.lock().unwrap().insert(t.clone());
                Json(serde_json::json!({"token": t})).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    /// First visit: choose the admin password (only while none is set).
    pub async fn setup(State(m): State<Arc<Manage>>, Json(body): Json<Value>) -> Response {
        let pw = body["password"].as_str().unwrap_or_default().to_string();
        if pw.chars().count() < 6 {
            return (StatusCode::BAD_REQUEST, "Use at least 6 characters").into_response();
        }
        let m2 = m.clone();
        let r = tokio::task::spawn_blocking(move || {
            let _guard = m2.fails.lock().unwrap(); // one setup at a time
            if password_path(&m2.dir).is_file() {
                return Ok(false);
            }
            set_password(&m2.dir, &pw).map(|_| true)
        })
        .await;
        match r {
            Ok(Ok(true)) => {
                println!("Admin password set on the Manage page.");
                session(&m)
            }
            Ok(Ok(false)) => (StatusCode::CONFLICT, "A password is already set: sign in instead").into_response(),
            Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Couldn't save the password: {e:#}")).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    pub async fn login(State(m): State<Arc<Manage>>, Json(body): Json<Value>) -> Response {
        {
            let f = m.fails.lock().unwrap();
            if f.0 >= 5 && f.1.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
                return (StatusCode::TOO_MANY_REQUESTS, "Too many wrong passwords: wait a minute and try again").into_response();
            }
        }
        let pw = body["password"].as_str().unwrap_or_default().to_string();
        let dir = m.dir.clone();
        let ok = tokio::task::spawn_blocking(move || check_password(&dir, &pw)).await.ok().flatten();
        match ok {
            Some(true) => {
                *m.fails.lock().unwrap() = (0, None);
                session(&m)
            }
            Some(false) => {
                {
                    let mut f = m.fails.lock().unwrap();
                    *f = (f.0 + 1, Some(Instant::now()));
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                (StatusCode::FORBIDDEN, "Wrong password").into_response()
            }
            None => (StatusCode::CONFLICT, "No password is set yet").into_response(),
        }
    }

    pub async fn logout(State(m): State<Arc<Manage>>, h: HeaderMap) -> StatusCode {
        if let Some(t) = h.get("x-bothy-session").and_then(|v| v.to_str().ok()) {
            m.sessions.lock().unwrap().remove(t);
        }
        StatusCode::NO_CONTENT
    }

    pub async fn job(State(m): State<Arc<Manage>>, h: HeaderMap) -> Response {
        if !m.authed(&h) {
            return StatusCode::FORBIDDEN.into_response();
        }
        Json(job_json(&m.job)).into_response()
    }

    pub async fn apply(State(m): State<Arc<Manage>>, h: HeaderMap, Json(body): Json<Value>) -> Response {
        if !m.authed(&h) {
            return StatusCode::FORBIDDEN.into_response();
        }
        let ids: BTreeSet<String> = body["ids"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
        if m.job.running.swap(true, Ordering::SeqCst) {
            return (StatusCode::CONFLICT, "A download is already running").into_response();
        }
        let j = &m.job;
        j.cancel.store(false, Ordering::SeqCst);
        j.total.store(0, Ordering::Relaxed);
        j.done.store(0, Ordering::Relaxed);
        *j.outcome.lock().unwrap() = String::new();
        *j.step.lock().unwrap() = "Starting".into();
        let m2 = m.clone();
        tokio::task::spawn_blocking(move || {
            let a = GetArgs { from: m2.start.get.from.clone(), yes: true, exact: Some(ids), job: Some(m2.job.clone()), ..Default::default() };
            let r = get(&m2.dir, &a, false).and_then(|_| {
                *m2.job.step.lock().unwrap() = "Loading the new content".into();
                m2.reload()
            });
            let cancelled = m2.job.cancel.load(Ordering::SeqCst);
            let outcome = match r {
                Ok(()) => "done".to_string(),
                Err(_) if cancelled => {
                    let _ = std::fs::remove_file(m2.dir.join("pending.json")); // don't resume it on the next start
                    "cancelled".to_string()
                }
                Err(e) => {
                    eprintln!("Download from the Manage page failed: {e:#}");
                    format!("{e:#}")
                }
            };
            *m2.job.outcome.lock().unwrap() = outcome;
            m2.job.running.store(false, Ordering::SeqCst);
        });
        StatusCode::ACCEPTED.into_response()
    }

    pub async fn cancel(State(m): State<Arc<Manage>>, h: HeaderMap) -> Response {
        if !m.authed(&h) {
            return StatusCode::FORBIDDEN.into_response();
        }
        m.job.cancel.store(true, Ordering::SeqCst);
        StatusCode::ACCEPTED.into_response()
    }
}

async fn start(dir: &Path, a: &StartArgs) -> Result<()> {
    if pending(dir).is_some() {
        println!("The last download didn't finish, so let's finish it first.\n");
        get(dir, &a.get, true)?;
    } else if load_installed(dir).is_empty() {
        println!("Welcome to Bothy. Nothing is downloaded yet, so let's fetch the library first.\n");
        get(dir, &a.get, true)?;
    }
    let (app, router, cfg) = build(dir, a, None)?;
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", a.port))
        .await
        .with_context(|| format!("port {} is in use: is a hub already running? Try --port 8081", a.port))?;
    let live = Arc::new(std::sync::RwLock::new(router));
    let m = Arc::new(Manage {
        dir: dir.to_path_buf(),
        start: a.clone(),
        sessions: Mutex::new(BTreeSet::new()),
        fails: Mutex::new((0, None)),
        job: Arc::new(Job::default()),
        catalog: Mutex::new(None),
        live: live.clone(),
        chat: app.chat.clone(),
    });
    // the Manage page stays put; everything else goes to the current content, swapped on reload
    let content = tower::service_fn(move |req: axum::extract::Request| {
        let r = live.read().unwrap().clone();
        async move { tower::ServiceExt::oneshot(r, req).await }
    });
    let router = axum::Router::new()
        .route("/manage", axum::routing::get(web::page))
        .route("/api/manage", axum::routing::get(web::state))
        .route("/api/manage/job", axum::routing::get(web::job))
        .route("/api/manage/status", axum::routing::get(web::status))
        .route("/api/manage/setup", axum::routing::post(web::setup))
        .route("/api/manage/login", axum::routing::post(web::login))
        .route("/api/manage/logout", axum::routing::post(web::logout))
        .route("/api/manage/apply", axum::routing::post(web::apply))
        .route("/api/manage/cancel", axum::routing::post(web::cancel))
        .with_state(m)
        .fallback_service(content);
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
    drop(app);
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
    let first = !password_path(dir).is_file();
    println!(
        "\nTo add maps, books and more, open Manage on the hub's home page{}:",
        if first { " and choose an admin password (the first visit sets it, so do it now)" } else { " (it asks for the admin password)" }
    );
    for l in manage_links(a.port) {
        println!("  {l}");
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
        Some(Cmd::Maps { action, names, from, yes }) => maps(&dir, action, names, from, *yes),
        Some(Cmd::ResetPassword) => match std::fs::remove_file(password_path(&dir)) {
            Ok(()) => {
                println!("The admin password is forgotten. Open the hub's Manage page and choose a new one.");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("No admin password is set: the first visit to the Manage page sets one.");
                Ok(())
            }
            Err(e) => Err(e.into()),
        },
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
            {"id": "map:world", "size": 1, "requires": ["map-assets"]}, {"id": "map:uk", "size": 3, "requires": ["map-assets"], "name": "United Kingdom"},
            {"id": "map:us-ca", "size": 3, "requires": ["map-assets"], "name": "California", "country": "us", "country_title": "United States of America"},
            {"id": "map:us-tx", "size": 3, "requires": ["map-assets"], "name": "Texas", "country": "us", "country_title": "United States of America"},
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
        assert!(m.contains("col:g:images") && m.contains("llm") && !m.contains("map:uk") && !m.contains("col:w:images"));
        let e = resolve(&c, preset(&c, "everything").unwrap(), &GetArgs::default());
        assert_eq!(e.len(), 11, "everything but the regional maps");
    }

    #[test]
    fn map_names() {
        let c = cat();
        assert_eq!(map_ids(&c, "uk"), ["map:uk"]);
        assert_eq!(map_ids(&c, "map:UK"), ["map:uk"]);
        assert_eq!(map_ids(&c, "united kingdom"), ["map:uk"]);
        assert_eq!(map_ids(&c, "us"), ["map:us-ca", "map:us-tx"]);
        assert_eq!(map_ids(&c, "United States of America").len(), 2);
        assert_eq!(map_ids(&c, "california"), ["map:us-ca"]);
        assert!(map_ids(&c, "atlantis").is_empty());
        let a = GetArgs { add: vec!["map:us".into()], ..Default::default() };
        let s = resolve(&c, preset(&c, "small").unwrap(), &a);
        assert!(s.contains("map:us-ca") && s.contains("map:us-tx") && s.contains("map-assets"));
        let a = GetArgs { add: vec!["map:us".into()], skip: vec!["map:texas".into()], ..Default::default() };
        let s = resolve(&c, preset(&c, "small").unwrap(), &a);
        assert!(s.contains("map:us-ca") && !s.contains("map:us-tx"));
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
    fn merged_indexes_replace_old_ones() {
        let cat = json!({"components": [
            {"id": "col:q", "size": 1},
            {"id": "col:q:vec:core.d256", "size": 2, "requires": ["col:q"], "replaces": ["col:q:vec:a.core.d256", "col:q:vec:b.core.d256"],
             "files": [{"path": "a"}, {"path": "b"}]},
        ]});
        let mut inst: BTreeMap<String, Value> =
            [("col:q", json!({})), ("col:q:vec:a.core.d256", json!({})), ("col:q:vec:b.core.d256", json!({}))].into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        assert!(migrate(&cat, &mut inst));
        assert_eq!(inst.keys().cloned().collect::<Vec<_>>(), ["col:q", "col:q:vec:core.d256"]);
        let ids = with_replacements(&cat, ["col:q".to_string(), "col:q:vec:a.core.d256".to_string()].into());
        assert!(ids.contains("col:q:vec:core.d256"), "one old index chosen: the merged one is chosen");
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
