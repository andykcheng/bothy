//! Hub content catalog (DESIGN §5, §16): what the mobile app can download from this server.
//!
//!   GET /api/catalog                     components with files, sizes and BLAKE3 hashes
//!   GET /download/collections/{id}/{rel} collection pack files (HTTP Range supported, so downloads resume)
//!   GET /download/models/{rel}           the query embedding model
//!   GET /download/maps/{rel}             map regions, place indexes and map assets
//!   GET /download/games/{rel}            games
//!   GET /download/llm/{rel}              the chat model
//!
//! Components (nothing is required: a phone may hold only games, or only maps):
//!   col:<id>             one collection's articles + titles + keyword (BM25) index
//!   col:<id>:images      its images
//!   col:<id>:vec:<name>  one of its semantic (vector) indexes; needs the collection and `model`
//!   model                query embedding model, shared by every semantic index
//!   map-assets           map renderer, style, fonts, sprites (needed by any map region)
//!   map:<id>             one map region with its place-search index
//!   game:<id>            one game (see games.rs), with its cover image if it has one
//!   llm                  chat model for answers (see ask.rs)
//!
//! Each component carries `group`; a collection's components share `group: "col:<id>"` and the group's
//! title/description/kind sit in `groups`. On the device, files land under the same relative paths
//! (`collections/<id>/…`, `models/…`, `maps/…`, `games/…`, `llm/…`), so a collection with only some
//! components is still a valid pack.

use crate::AppState;
use anyhow::Result;
use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Catalog {
    pub json: Value,
}

fn hash(p: &Path) -> Result<String> {
    Ok(blake3::Hasher::new().update_mmap_rayon(p)?.finalize().to_hex().to_string())
}

/// Files under `dir` (recursively) as (relative path, absolute path), sorted.
fn walk(dir: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn file(prefix: &str, rel: &str, p: &Path, blake3: Option<String>) -> Result<Value> {
    Ok(json!({
        "path": format!("{prefix}/{rel}"),
        "size": std::fs::metadata(p)?.len(),
        "blake3": match blake3 { Some(h) => h, None => hash(p)? },
    }))
}

fn component(id: &str, title: &str, group: &str, files: Vec<Value>, extra: Value) -> Value {
    let size: u64 = files.iter().filter_map(|f| f["size"].as_u64()).sum();
    let mut c = json!({"id": id, "title": title, "group": group, "size": size, "files": files});
    if let (Some(c), Some(e)) = (c.as_object_mut(), extra.as_object()) {
        c.extend(e.clone());
    }
    c
}

fn vec_title(name: &str) -> String {
    // {source}.{tier}.d{dims}
    let parts: Vec<&str> = name.rsplitn(3, '.').collect();
    if let [d, tier, source] = parts[..] {
        let _ = source;
        let tier = match tier { "core" => "most-read articles", "rest" | "tail" => "remaining articles", _ => "all articles" };
        let len = if d == "d128" { "compact" } else { "full" };
        format!("Semantic search · {tier} · {len}")
    } else {
        format!("Semantic index · {name}")
    }
}

/// A collection as a catalog group (its components share `group: "col:<id>"`).
pub fn group(col: &sp_search::library::Collection) -> Value {
    let pack = &col.searcher.pack;
    json!({
        "id": format!("col:{}", col.id), "title": col.title, "description": col.description, "kind": col.kind,
        "docs": pack.doc_count(), "created_unix": pack.manifest["created_unix"],
    })
}

impl Catalog {
    pub fn build(
        lib: &sp_search::library::Library,
        model: Option<&Path>,
        maps: Option<&Path>,
        games: Option<&crate::games::Games>,
        llm: Option<&Path>,
    ) -> Result<Catalog> {
        let t0 = std::time::Instant::now();
        let mut comps = Vec::new();
        let mut groups = Vec::new();
        for col in &lib.cols {
            let pack = &col.searcher.pack;
            let dir = &pack.dir;
            let manifest = &pack.manifest;
            let known: BTreeMap<String, String> = manifest["files"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| Some((f["path"].as_str()?.to_string(), f["blake3"].as_str()?.to_string())))
                .collect();
            let prefix = format!("collections/{}", col.id);
            let (mut text, mut images) = (Vec::new(), Vec::new());
            let mut vecs: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for (rel, p) in walk(dir)? {
                if rel.starts_with("models/") {
                    continue; // the shared `model` component
                }
                let f = file(&prefix, &rel, &p, known.get(&rel).cloned())?;
                if let Some(rest) = rel.strip_prefix("index/vec/") {
                    let name = rest.split('/').next().unwrap_or(rest).to_string();
                    vecs.entry(name).or_default().push(f);
                } else if rel.starts_with("img-") {
                    images.push(f);
                } else {
                    text.push(f);
                }
            }
            let g = format!("col:{}", col.id);
            groups.push(group(col));
            comps.push(component(&g, &format!("Articles + keyword search ({} articles)", pack.doc_count()), &g, text, json!({})));
            if !images.is_empty() {
                comps.push(component(&format!("{g}:images"), "Images", &g, images, json!({"requires": [g]})));
            }
            for (name, files) in vecs {
                comps.push(component(&format!("{g}:vec:{name}"), &vec_title(&name), &g, files, json!({"requires": [g, "model"]})));
            }
        }
        if let Some(m) = model.filter(|_| lib.has_dense()) {
            let files = vec![file("models", &m.file_name().unwrap_or_default().to_string_lossy(), m, None)?];
            comps.push(component("model", "Semantic search model (needed by every semantic index)", "models", files, json!({})));
        }

        if let Some(mdir) = maps.filter(|d| d.is_dir()) {
            let assets = mdir.join("assets");
            if assets.is_dir() {
                let files = walk(&assets)?
                    .iter()
                    .map(|(rel, p)| file("maps", &format!("assets/{rel}"), p, None))
                    .collect::<Result<Vec<_>>>()?;
                comps.push(component("map-assets", "Map renderer, style and fonts (needed by maps)", "maps", files, json!({})));
            }
            // hashing ~150 GB of maps on every start is too slow: remember hashes by size + mtime
            let cache_path = mdir.join(".hashes.json");
            let old: BTreeMap<String, Value> =
                std::fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let mut new: BTreeMap<String, Value> = BTreeMap::new();
            let mut cached = |rel: &str, p: &Path| -> Result<Value> {
                let md = std::fs::metadata(p)?;
                let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
                let key = json!([md.len(), mtime]);
                let h = match old.get(rel) {
                    Some(v) if v["k"] == key => v["h"].as_str().map(str::to_string),
                    _ => None,
                };
                let f = file("maps", rel, p, h)?;
                new.insert(rel.to_string(), json!({"k": key, "h": f["blake3"]}));
                Ok(f)
            };
            let mut regions = Vec::new();
            for (rel, p) in walk(mdir)? {
                if rel.contains('/') || !rel.ends_with(".pmtiles") || rel.contains(".tmp.") {
                    continue;
                }
                let a = sp_maps::pmt::Archive::open(&p)?;
                let m = a.metadata()?;
                let stem = rel.trim_end_matches(".pmtiles");
                let id = m["survivalpedia"]["region"].as_str().unwrap_or(stem).to_string();
                let title = m["survivalpedia"]["title"].as_str().unwrap_or(stem).to_string();
                let mut files = vec![cached(&rel, &p)?];
                let places = mdir.join(format!("{stem}.places"));
                if places.is_dir() {
                    for (prel, pp) in walk(&places)? {
                        files.push(cached(&format!("{stem}.places/{prel}"), &pp)?);
                    }
                }
                let h = &a.header;
                let sp = &m["survivalpedia"];
                let mut extra = json!({"requires": ["map-assets"], "bounds": h.bounds, "max_zoom": h.max_zoom, "name": title});
                for k in ["continent", "country", "country_title", "build"] {
                    if let Some(v) = sp[k].as_str() {
                        extra[k] = json!(v);
                    }
                }
                regions.push((sp["continent"].as_str().unwrap_or("").to_string(), title.clone(), component(&format!("map:{id}"), &format!("Map · {title}"), "maps", files, extra)));
            }
            // the world first, then by continent and name
            regions.sort_by(|a, b| ((a.0 != "World"), &a.0, &a.1).cmp(&((b.0 != "World"), &b.0, &b.1)));
            comps.extend(regions.into_iter().map(|r| r.2));
            if new != old {
                let _ = std::fs::write(&cache_path, serde_json::to_vec(&new)?); // read-only dirs just hash again
            }
        }
        for g in games.map(|g| g.list.as_slice()).unwrap_or_default() {
            let files = walk(&g.dir)?
                .iter()
                .map(|(rel, p)| file("games", &format!("{}/{rel}", g.id), p, None))
                .collect::<Result<Vec<_>>>()?;
            let m = &g.meta;
            comps.push(component(
                &format!("game:{}", g.id),
                m["title"].as_str().unwrap_or(&g.id),
                "games",
                files,
                json!({
                    "description": m["description"], "players": m["players"], "license": m["license"], "version": m["version"],
                    // served live for the store grid, and downloaded with the game
                    "cover": m["cover"].as_str().map(|c| format!("games/{}/{c}", g.id)),
                }),
            ));
        }
        if let Some(gguf) = llm {
            // the chosen GGUF plus the directory's other files (licence, model card)
            let dir = crate::ask::llm_dir(gguf);
            let files = walk(&dir)?
                .iter()
                .filter(|(rel, p)| !rel.contains('/') && (p == gguf || !rel.ends_with(".gguf")))
                .map(|(rel, p)| file("llm", rel, p, None))
                .collect::<Result<Vec<_>>>()?;
            let name = gguf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            comps.push(component(
                "llm",
                &format!("AI answers · {name} (written from the library's articles; needs ~3 GB RAM)"),
                "ai",
                files,
                json!({"model": name}),
            ));
        }
        eprintln!("catalog: {} components ({:.1}s)", comps.len(), t0.elapsed().as_secs_f32());
        Ok(Catalog {
            json: json!({
                "format": "survivalpedia-catalog", "version": 2, "source": "hub",
                "groups": groups,
                "components": comps,
            }),
        })
    }
}

pub async fn handler(State(app): State<AppState>) -> Json<Value> {
    Json(app.catalog.as_ref().map(|c| c.json.clone()).unwrap_or(json!({"components": []})))
}

/// Writes a static copy of the catalog for a plain file host (the download server, DESIGN §14):
/// `<out>/api/catalog` (the JSON) and `<out>/download/<path>` as symlinks to the files themselves.
/// Uploading `<out>` as it is gives clients the same URLs a hub serves, so the app and `bothy-hub get`
/// can use the file host as if it were a hub. `roots` maps a catalog path prefix to its directory.
pub fn export(cat: &Catalog, roots: &[(String, std::path::PathBuf)], out: &Path) -> Result<(usize, u64)> {
    std::fs::create_dir_all(out.join("api"))?;
    let (mut n, mut bytes) = (0, 0);
    for c in cat.json["components"].as_array().into_iter().flatten() {
        for f in c["files"].as_array().into_iter().flatten() {
            let path = f["path"].as_str().unwrap_or_default();
            let (prefix, dir) = roots
                .iter()
                .filter(|(p, _)| path.starts_with(&format!("{p}/")))
                .max_by_key(|(p, _)| p.len())
                .ok_or_else(|| anyhow::anyhow!("no source directory for {path}"))?;
            let src = dir.join(&path[prefix.len() + 1..]);
            anyhow::ensure!(std::fs::metadata(&src)?.len() == f["size"].as_u64().unwrap_or(u64::MAX), "size changed: {}", src.display());
            let dst = out.join("download").join(path);
            std::fs::create_dir_all(dst.parent().unwrap())?;
            let _ = std::fs::remove_file(&dst);
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::canonicalize(&src)?, &dst)?;
            #[cfg(not(unix))]
            std::fs::copy(&src, &dst)?;
            n += 1;
            bytes += f["size"].as_u64().unwrap_or(0);
        }
    }
    std::fs::write(out.join("api/catalog"), serde_json::to_vec(&cat.json)?)?;
    Ok((n, bytes))
}
