//! Offline place search (DESIGN §16): a gazetteer built from a region's basemap tiles.
//!
//! Sources: the `places` layer at every zoom (countries … neighbourhoods), plus `pois` and named
//! `roads` at the archive's max zoom. Each street/POI gets the nearest town as context
//! ("High Street, Oxford"). Names in all scripts are indexed (name, name:en, name:zh-Hans,
//! name:zh-Hant, name:ja).
//!
//! Files in `<region>.places/`:
//!   records.bin   20-byte records: lon f32, lat f32, importance f32, kind u16, class u8, pad u8, str u32
//!   strings.bin   per record: names joined by 0x1F, then 0x1E + context
//!   names.fst     set of `normalised name suffix` 0x00 `record id (u32 BE)`; suffixes start at
//!                 word starts (and at every CJK character), so prefix search finds inner words
//!   meta.json     kind labels, counts

use crate::mvt;
use crate::pmt::{self, Archive};
use anyhow::{Context, Result, ensure};
use fst::{IntoStreamer, Set, Streamer};
use memmap2::Mmap;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const NAME_KEYS: [&str; 5] = ["name", "name:en", "name:zh-Hans", "name:zh-Hant", "name:ja"];
const RECORD: usize = 20;
/// Records gathered per query before ranking (common words like "high street" have many).
const MAX_CANDIDATES: usize = 500_000;
/// Candidates kept after the cheap first-stage score for full name scoring.
const REFINE: usize = 1000;
/// Score lost per decade of distance (km) from the map centre.
const DIST_WEIGHT: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Place,
    Poi,
    Street,
}

impl Class {
    fn from_u8(v: u8) -> Class {
        match v {
            0 => Class::Place,
            1 => Class::Poi,
            _ => Class::Street,
        }
    }
}

struct Item {
    lon: f64,
    lat: f64,
    class: Class,
    kind: String,
    importance: f32,
    names: Vec<String>,
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0xac00..=0xd7af)
}

/// NFKC, lowercase, punctuation dropped, whitespace collapsed: "St. James's Park" -> "st jamess park".
pub fn normalize(s: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.nfkc().flat_map(|c| c.to_lowercase()) {
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else if c.is_whitespace() || c == '-' || c == '/' || c == ',' {
            space = true;
        }
    }
    out
}

/// Index keys for a normalised name: the whole name, each later word, each CJK character start.
fn suffixes(n: &str) -> Vec<&str> {
    let mut v = vec![n];
    let mut prev: Option<char> = None;
    for (i, c) in n.char_indices() {
        if i > 0 && (prev == Some(' ') || is_cjk(c)) && c != ' ' {
            v.push(&n[i..]);
        }
        prev = Some(c);
    }
    v.dedup();
    v
}

fn place_importance(kind: &str, population: f64) -> f32 {
    let base = match kind {
        "country" => 100.0,
        "region" => 90.0,
        "city" => 80.0,
        "town" => 70.0,
        "village" => 55.0,
        "suburb" => 50.0,
        "neighbourhood" | "quarter" => 45.0,
        "hamlet" => 40.0,
        _ => 38.0,
    };
    base + 2.0 * (1.0 + population.max(0.0)).log10() as f32
}

fn poi_importance(f: &mvt::Feature, kind: &str) -> f32 {
    let min_zoom = f.get("min_zoom").and_then(|v| v.as_f64()).unwrap_or(15.0);
    let mut s = 30.0 + (16.0 - min_zoom as f32).max(0.0);
    if f.get("wikidata").is_some() {
        s += 8.0;
    }
    if matches!(kind, "hospital" | "station" | "aerodrome" | "university" | "national_park") {
        s += 5.0;
    }
    // usually named after the street or place they sit on, so they shadow the real thing
    if matches!(kind, "bus_stop" | "parking" | "information" | "crossing" | "pedestrian" | "platform" | "bicycle_parking") {
        s -= 12.0;
    }
    s
}

fn street_importance(f: &mvt::Feature) -> f32 {
    let min_zoom = f.get("min_zoom").and_then(|v| v.as_f64()).unwrap_or(15.0);
    28.0 + (16.0 - min_zoom as f32).max(0.0) / 2.0
}

fn names_of(f: &mvt::Feature) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for k in NAME_KEYS {
        if let Some(n) = f.get(k).and_then(|v| v.as_str()) {
            let n = n.trim();
            if !n.is_empty() && !v.iter().any(|x| x == n) {
                v.push(n.to_string());
            }
        }
    }
    v
}

/// Items from one tile. Places come from every zoom; POIs and streets only at `detail_zoom`.
fn tile_items(a: &Archive, id: u64, e: &pmt::Entry, detail_zoom: u8) -> Result<Vec<Item>> {
    let c = pmt::tile_coord(id)?;
    let (z, x, y) = (c.z(), c.x(), c.y());
    let want: &[&str] = if z == detail_zoom { &["places", "pois", "roads"] } else { &["places"] };
    let data = pmt::decompress(a.data(e), a.header.tile_compression)?;
    let mut out = Vec::new();
    for l in mvt::decode(&data, want)? {
        for f in &l.features {
            let names = names_of(f);
            if names.is_empty() {
                continue;
            }
            let (lon, lat) = mvt::to_lonlat(z, x, y, l.extent, f.point);
            let kind = f.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let detail = f.get("kind_detail").and_then(|v| v.as_str()).unwrap_or("");
            let (class, kind, importance) = match l.name.as_str() {
                "places" => {
                    let k = if kind == "locality" && !detail.is_empty() { detail } else { kind };
                    let pop = f.get("population").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    (Class::Place, k, place_importance(k, pop))
                }
                "pois" => (Class::Poi, kind, poi_importance(f, kind)),
                _ => {
                    if f.geom_type != 2 {
                        continue;
                    }
                    (Class::Street, if detail.is_empty() { kind } else { detail }, street_importance(f))
                }
            };
            out.push(Item { lon, lat, class, kind: kind.to_string(), importance, names });
        }
    }
    Ok(out)
}

/// Grid cell for de-duplication, sized per class.
fn cell(it: &Item) -> (i32, i32) {
    let s = match it.class {
        Class::Place => 0.05,
        Class::Poi => 0.003,
        Class::Street => 0.01,
    };
    ((it.lon / s).floor() as i32, (it.lat / s).floor() as i32)
}

fn dist_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.1.to_radians(), b.1.to_radians());
    let dla = la2 - la1;
    let dlo = (b.0 - a.0).to_radians();
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    12742.0 * h.sqrt().asin()
}

/// Builds `<out>/` (records, strings, names.fst, meta.json) from a region archive.
pub fn build(archive: &Path, out: &Path) -> Result<serde_json::Value> {
    let t0 = std::time::Instant::now();
    let a = Archive::open(archive)?;
    let detail = a.header.max_zoom;
    let entries = a.entries()?;
    eprintln!("places: {} tile entries", entries.len());
    let per_tile: Vec<Vec<Item>> =
        entries.par_iter().map(|(id, e)| tile_items(&a, *id, e, detail)).collect::<Result<_>>()?;
    let raw: usize = per_tile.iter().map(|v| v.len()).sum();

    // de-duplicate: same class, kind, first name, nearby (features repeat across tile edges and zooms)
    let mut seen: HashMap<(u8, String, String, (i32, i32)), usize> = HashMap::new();
    let mut items: Vec<Item> = Vec::new();
    for it in per_tile.into_iter().flatten() {
        let key = (it.class as u8, it.kind.clone(), normalize(&it.names[0]), cell(&it));
        match seen.get(&key) {
            Some(&i) => {
                if it.importance > items[i].importance {
                    items[i].importance = it.importance;
                }
            }
            None => {
                seen.insert(key, items.len());
                items.push(it);
            }
        }
    }
    drop(seen);
    eprintln!("places: {raw} named features -> {} entries ({:.0}s)", items.len(), t0.elapsed().as_secs_f32());

    // context: nearest settlement within 15 km (for settlements, the nearest bigger one)
    let settle = |k: &str| match k {
        "city" => 3,
        "town" => 2,
        "village" | "suburb" => 1,
        _ => 0,
    };
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        if it.class == Class::Place && settle(&it.kind) > 0 {
            grid.entry(((it.lon / 0.1).floor() as i32, (it.lat / 0.1).floor() as i32)).or_default().push(i);
        }
    }
    let contexts: Vec<String> = items
        .par_iter()
        .map(|it| {
            let own = if it.class == Class::Place { settle(&it.kind) } else { 0 };
            if it.class == Class::Place && (own == 3 || matches!(it.kind.as_str(), "country" | "region")) {
                return String::new();
            }
            let (cx, cy) = ((it.lon / 0.1).floor() as i32, (it.lat / 0.1).floor() as i32);
            let mut best: Option<(f64, usize)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for &j in grid.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                        let p = &items[j];
                        if settle(&p.kind) <= own {
                            continue;
                        }
                        // prefer bigger places a little: a village 1 km away beats a city 12 km away
                        let d = dist_km((it.lon, it.lat), (p.lon, p.lat)) / (1.0 + 0.3 * settle(&p.kind) as f64);
                        if d < 15.0 && best.is_none_or(|b| d < b.0) {
                            best = Some((d, j));
                        }
                    }
                }
            }
            best.map(|(_, j)| items[j].names[0].clone()).unwrap_or_default()
        })
        .collect();

    // write records + strings
    std::fs::create_dir_all(out)?;
    let mut kinds: Vec<String> = Vec::new();
    let mut kind_ix: HashMap<String, u16> = HashMap::new();
    let mut rec = Vec::with_capacity(items.len() * RECORD);
    let mut strings: Vec<u8> = Vec::new();
    for (it, ctx) in items.iter().zip(&contexts) {
        let k = *kind_ix.entry(it.kind.clone()).or_insert_with(|| {
            kinds.push(it.kind.clone());
            (kinds.len() - 1) as u16
        });
        rec.extend((it.lon as f32).to_le_bytes());
        rec.extend((it.lat as f32).to_le_bytes());
        rec.extend(it.importance.to_le_bytes());
        rec.extend(k.to_le_bytes());
        rec.push(it.class as u8);
        rec.push(0);
        rec.extend((strings.len() as u32).to_le_bytes());
        strings.extend(it.names.join("\u{1f}").as_bytes());
        strings.push(0x1e);
        strings.extend(ctx.as_bytes());
        strings.push(b'\n');
    }
    ensure!(strings.len() < u32::MAX as usize, "strings.bin over 4 GB");
    std::fs::write(out.join("records.bin"), &rec)?;
    std::fs::write(out.join("strings.bin"), &strings)?;

    // names.fst
    let mut keys: Vec<Vec<u8>> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let mut ns: Vec<String> = it.names.iter().map(|n| normalize(n)).filter(|n| !n.is_empty()).collect();
        ns.sort();
        ns.dedup();
        for n in &ns {
            for s in suffixes(n) {
                let mut k = s.as_bytes().to_vec();
                k.push(0);
                k.extend((i as u32).to_be_bytes());
                keys.push(k);
            }
        }
    }
    keys.par_sort_unstable();
    keys.dedup();
    let mut b = fst::SetBuilder::new(std::io::BufWriter::new(File::create(out.join("names.fst"))?))?;
    for k in &keys {
        b.insert(k)?;
    }
    b.finish()?;

    let counts = |c: Class| items.iter().filter(|i| i.class == c).count();
    let meta = serde_json::json!({
        "format": "survivalpedia-places", "version": 1,
        "archive": archive.file_name().map(|f| f.to_string_lossy().into_owned()),
        "kinds": kinds,
        "counts": {"places": counts(Class::Place), "pois": counts(Class::Poi), "streets": counts(Class::Street), "keys": keys.len()},
    });
    std::fs::write(out.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
    eprintln!("places: wrote {} ({} keys, {:.0}s)", out.display(), keys.len(), t0.elapsed().as_secs_f32());
    Ok(meta)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub name: String,
    /// The matched name when it differs from `name` (e.g. the Japanese name)
    pub matched: Option<String>,
    pub names: Vec<String>,
    pub context: String,
    pub class: Class,
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
    pub score: f32,
}

pub struct Gazetteer {
    names: Set<Mmap>,
    records: Mmap,
    strings: Mmap,
    kinds: Vec<String>,
}

fn map(p: &Path) -> Result<Mmap> {
    let f = File::open(p).with_context(|| format!("opening {}", p.display()))?;
    Ok(unsafe { Mmap::map(&f)? })
}

impl Gazetteer {
    pub fn open(dir: &Path) -> Result<Gazetteer> {
        let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("meta.json"))?)?;
        Ok(Gazetteer {
            names: Set::new(map(&dir.join("names.fst"))?)?,
            records: map(&dir.join("records.bin"))?,
            strings: map(&dir.join("strings.bin"))?,
            kinds: meta["kinds"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(String::from)).collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.records.len() / RECORD
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn record(&self, i: usize) -> (f32, f32, f32, u16, u8, usize) {
        let r = &self.records[i * RECORD..(i + 1) * RECORD];
        let f = |a: usize| f32::from_le_bytes(r[a..a + 4].try_into().unwrap());
        (f(0), f(4), f(8), u16::from_le_bytes([r[12], r[13]]), r[14], u32::from_le_bytes(r[16..20].try_into().unwrap()) as usize)
    }

    fn strings_at(&self, off: usize) -> (Vec<String>, String) {
        let end = self.strings[off..].iter().position(|&b| b == b'\n').map_or(self.strings.len(), |p| off + p);
        let s = String::from_utf8_lossy(&self.strings[off..end]);
        let (names, ctx) = s.split_once('\u{1e}').unwrap_or((&s, ""));
        (names.split('\u{1f}').map(String::from).collect(), ctx.to_string())
    }

    /// Prefix search over all name variants. `near` (lon, lat) favours nearby results.
    pub fn search(&self, query: &str, near: Option<(f64, f64)>, k: usize) -> Vec<Hit> {
        let q = normalize(query);
        let min_len = if q.chars().any(is_cjk) { 1 } else { 2 };
        if q.chars().count() < min_len {
            return Vec::new();
        }
        // candidates: record -> some key's name part equals the query (a whole-word match)
        let mut cand: HashMap<u32, bool> = HashMap::new();
        let mut stream = self.names.range().ge(q.as_bytes()).into_stream();
        while let Some(key) = stream.next() {
            if !key.starts_with(q.as_bytes()) || cand.len() >= MAX_CANDIDATES {
                break;
            }
            // key = name suffix, 0x00, id (4 bytes, which may themselves contain 0x00)
            let Some(z) = key.len().checked_sub(5).filter(|&z| key[z] == 0) else { continue };
            let id = u32::from_be_bytes(key[z + 1..].try_into().unwrap());
            *cand.entry(id).or_insert(false) |= z == q.len();
        }
        // stage 1: cheap score from the record alone (importance, distance), keep the best
        let dist_pen = |lon: f32, lat: f32| match near {
            Some(c) => DIST_WEIGHT * (1.0 + dist_km(c, (lon as f64, lat as f64))).log10() as f32,
            None => 0.0,
        };
        let mut pre: Vec<(f32, u32)> = cand
            .iter()
            .map(|(&id, &whole)| {
                let (lon, lat, imp, ..) = self.record(id as usize);
                (imp - dist_pen(lon, lat) + if whole { 10.0 } else { 0.0 }, id)
            })
            .collect();
        let keep = REFINE.min(pre.len());
        if keep > 0 && keep < pre.len() {
            pre.select_nth_unstable_by(keep - 1, |a, b| b.0.total_cmp(&a.0));
            pre.truncate(keep);
        }
        // stage 2: name-aware score for the survivors
        let mut hits: Vec<Hit> = pre
            .into_iter()
            .map(|(_, id)| {
                let (lon, lat, imp, kind, class, off) = self.record(id as usize);
                let (names, context) = self.strings_at(off);
                let norms: Vec<String> = names.iter().map(|n| normalize(n)).collect();
                let mi = norms.iter().position(|n| n.starts_with(&q));
                let mut score = imp - dist_pen(lon, lat);
                if norms.iter().any(|n| *n == q) {
                    score += 15.0; // whole name equals the query
                } else if mi.is_some() {
                    score += 6.0; // name starts with the query
                }
                let matched = mi.or_else(|| norms.iter().position(|n| n.contains(&q))).filter(|&i| i > 0).map(|i| names[i].clone());
                Hit {
                    name: names[0].clone(),
                    matched,
                    names,
                    context,
                    class: Class::from_u8(class),
                    kind: self.kinds.get(kind as usize).cloned().unwrap_or_default(),
                    lon: lon as f64,
                    lat: lat as f64,
                    score,
                }
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        // collapse repeats of one long street / POI along the result list
        let mut out: Vec<Hit> = Vec::new();
        for h in hits {
            if out.iter().any(|o| o.name == h.name && o.class == h.class && dist_km((o.lon, o.lat), (h.lon, h.lat)) < 2.0) {
                continue;
            }
            out.push(h);
            if out.len() >= k {
                break;
            }
        }
        out
    }
}
