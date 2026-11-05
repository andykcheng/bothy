//! Region extraction from a remote PMTiles build (e.g. the daily Protomaps planet) with HTTP
//! range requests: only the leaf directories and tile bytes inside the region are downloaded.

use crate::pmt::{self, Entry, Header};
use anyhow::{Context, Result, bail};
use pmtiles::{Compression, PmTilesWriter, TileType};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub struct Opts {
    pub url: String,
    /// min lon, min lat, max lon, max lat
    pub bbox: [f64; 4],
    pub min_zoom: u8,
    pub max_zoom: u8,
    /// Cut along these shapes instead of the whole bbox (bbox should then be their bounds)
    pub polygons: Option<Vec<crate::region::Polygon>>,
    /// Only work out the size: read directories, fetch no tiles, write nothing
    pub estimate: bool,
    /// Local copy of the build's leaf directories (downloaded once, reused by every region)
    pub dir_cache: Option<std::path::PathBuf>,
    /// Parallel HTTP requests
    pub threads: usize,
    /// Extra metadata merged into the archive's metadata JSON (region name, source build, …)
    pub meta: serde_json::Value,
}

/// Byte gaps up to this size between wanted ranges are fetched rather than split into requests.
const MERGE_GAP: u64 = 256 << 10;
const MAX_RANGE: u64 = 32 << 20;

struct Remote {
    agent: ureq::Agent,
    url: String,
    bytes: AtomicU64,
    requests: AtomicU64,
}

impl Remote {
    fn get(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let range = format!("bytes={}-{}", offset, offset + len - 1);
        let mut last = None;
        for attempt in 0..5 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(1 << attempt));
            }
            let r = self.agent.get(&self.url).header("Range", &range).call().map_err(anyhow::Error::from).and_then(|mut r| {
                Ok(r.body_mut().with_config().limit(len + 1024).read_to_vec()?)
            });
            match r {
                Ok(b) if b.len() as u64 == len => {
                    self.bytes.fetch_add(len, Ordering::Relaxed);
                    self.requests.fetch_add(1, Ordering::Relaxed);
                    return Ok(b);
                }
                Ok(b) => last = Some(anyhow::anyhow!("range {range}: got {} bytes", b.len())),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap()).with_context(|| format!("GET {} {range}", self.url))
    }
}

/// Groups sorted (offset, len) spans into fetch ranges; returns (range_offset, range_len, span indexes).
fn merge_spans(spans: &[(u64, u64)]) -> Vec<(u64, u64, Vec<usize>)> {
    let mut out: Vec<(u64, u64, Vec<usize>)> = Vec::new();
    for (i, &(o, l)) in spans.iter().enumerate() {
        if let Some(last) = out.last_mut() {
            let end = last.0 + last.1;
            if o <= end + MERGE_GAP && o + l - last.0 <= MAX_RANGE {
                last.1 = last.1.max(o + l - last.0);
                last.2.push(i);
                continue;
            }
        }
        out.push((o, l, vec![i]));
    }
    out
}

/// The whole leaf-directory section of the build, from `path` (downloaded there first if needed).
fn leaf_section(remote: &Remote, h: &Header, path: &Path) -> Result<Vec<u8>> {
    if std::fs::metadata(path).map(|m| m.len()).ok() != Some(h.leaf_length) {
        eprintln!("downloading the build's directories ({:.0} MB) to {}", h.leaf_length as f64 / 1e6, path.display());
        let chunks: Vec<(u64, u64)> = (0..h.leaf_length)
            .step_by(MAX_RANGE as usize)
            .map(|o| (h.leaf_offset + o, MAX_RANGE.min(h.leaf_length - o)))
            .collect();
        let parts: Vec<Vec<u8>> = chunks.par_iter().map(|&(o, l)| remote.get(o, l)).collect::<Result<_>>()?;
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, parts.concat())?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(std::fs::read(path)?)
}

/// Fetches spans (absolute offsets) in merged ranges, in parallel. Returns bytes per span.
fn fetch_spans(remote: &Remote, spans: &[(u64, u64)]) -> Result<Vec<Vec<u8>>> {
    let ranges = merge_spans(spans);
    let parts: Vec<Vec<(usize, Vec<u8>)>> = ranges
        .par_iter()
        .map(|(ro, rl, idx)| {
            let buf = remote.get(*ro, *rl)?;
            Ok(idx.iter().map(|&i| {
                let (o, l) = spans[i];
                let a = (o - ro) as usize;
                (i, buf[a..a + l as usize].to_vec())
            }).collect())
        })
        .collect::<Result<_>>()?;
    let mut out = vec![Vec::new(); spans.len()];
    for (i, b) in parts.into_iter().flatten() {
        out[i] = b;
    }
    Ok(out)
}

pub struct Stats {
    pub tiles: usize,
    pub unique: usize,
    pub downloaded: u64,
    pub requests: u64,
    pub out_bytes: u64,
}

pub fn extract(o: &Opts, out: &Path) -> Result<Stats> {
    let t0 = Instant::now();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(o.threads).build()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .build()
        .into();
    let remote = Remote { agent, url: o.url.clone(), bytes: AtomicU64::new(0), requests: AtomicU64::new(0) };

    let head = remote.get(0, pmt::ROOT_FETCH as u64)?;
    let h = Header::parse(&head)?;
    let max_zoom = o.max_zoom.min(h.max_zoom);
    let root_bytes = &head[h.root_offset as usize..(h.root_offset + h.root_length) as usize];
    let root = pmt::parse_directory(&pmt::decompress(root_bytes, h.internal_compression)?)?;
    let leaves = match &o.dir_cache {
        Some(p) => Some(pool.install(|| leaf_section(&remote, &h, p))?),
        None => None,
    };

    // wanted tile ids, sorted
    let mut wanted: Vec<u64> = Vec::new();
    for z in o.min_zoom..=max_zoom {
        match &o.polygons {
            Some(p) => {
                for (y, spans) in crate::region::tile_rows(p, z) {
                    for (x0, x1) in spans {
                        for x in x0..=x1 {
                            wanted.push(pmt::tile_id(z, x, y)?);
                        }
                    }
                }
            }
            None => {
                let (x0, y0, x1, y1) = pmt::tile_range(o.bbox, z);
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        wanted.push(pmt::tile_id(z, x, y)?);
                    }
                }
            }
        }
    }
    wanted.sort_unstable();
    eprintln!("region: {} tiles wanted, z{}–{max_zoom}", wanted.len(), o.min_zoom);

    // walk directories, fetching only leaves whose id range holds a wanted tile
    let mut found: Vec<(u64, Entry)> = Vec::new(); // (tile id, data entry)
    let mut level = vec![root];
    for depth in 0..4 {
        let mut leaf_spans: Vec<(u64, u64)> = Vec::new();
        for dir in &level {
            for (i, e) in dir.iter().enumerate() {
                let end = dir.get(i + 1).map_or(u64::MAX, |n| n.tile_id);
                let lo = wanted.partition_point(|&w| w < e.tile_id);
                if e.is_leaf() {
                    if lo < wanted.len() && wanted[lo] < end {
                        leaf_spans.push((h.leaf_offset + e.offset, e.length as u64));
                    }
                } else {
                    let run_end = e.tile_id + e.run_length as u64;
                    found.extend(wanted[lo..].iter().take_while(|&&w| w < run_end).map(|&w| (w, *e)));
                }
            }
        }
        if leaf_spans.is_empty() {
            break;
        }
        if depth == 3 {
            bail!("directory nesting too deep");
        }
        leaf_spans.sort_unstable();
        leaf_spans.dedup();
        let bytes = match &leaves {
            Some(l) => leaf_spans
                .iter()
                .map(|&(o, n)| {
                    let a = (o - h.leaf_offset) as usize;
                    l.get(a..a + n as usize).map(|b| b.to_vec()).context("leaf outside the cached directories")
                })
                .collect::<Result<Vec<_>>>()?,
            None => pool.install(|| fetch_spans(&remote, &leaf_spans))?,
        };
        level = bytes
            .par_iter()
            .map(|b| pmt::parse_directory(&pmt::decompress(b, h.internal_compression)?))
            .collect::<Result<_>>()?;
        eprintln!("directories depth {}: {} leaves ({:.0}s)", depth + 1, leaf_spans.len(), t0.elapsed().as_secs_f32());
    }
    found.sort_unstable_by_key(|x| x.0);

    // tile bytes: one fetch per distinct stored tile (duplicates share an offset)
    let mut spans: Vec<(u64, u64)> = found.iter().map(|(_, e)| (h.data_offset + e.offset, e.length as u64)).collect();
    spans.sort_unstable();
    spans.dedup();
    let total: u64 = spans.iter().map(|s| s.1).sum();
    eprintln!("tiles: {} present, {} distinct, {:.1} MB to fetch", found.len(), spans.len(), total as f64 / 1e6);
    if o.estimate {
        return Ok(Stats {
            tiles: found.len(),
            unique: spans.len(),
            downloaded: remote.bytes.load(Ordering::Relaxed),
            requests: remote.requests.load(Ordering::Relaxed),
            out_bytes: total,
        });
    }
    let data = pool.install(|| fetch_spans(&remote, &spans))?;
    let by_offset: HashMap<u64, usize> = spans.iter().enumerate().map(|(i, s)| (s.0 - h.data_offset, i)).collect();
    eprintln!("fetched in {:.0}s", t0.elapsed().as_secs_f32());

    // write
    let meta_bytes = remote.get(h.metadata_offset, h.metadata_length)?;
    let mut meta: serde_json::Value = serde_json::from_slice(&pmt::decompress(&meta_bytes, h.internal_compression)?)?;
    if let (Some(m), Some(extra)) = (meta.as_object_mut(), o.meta.as_object()) {
        for (k, v) in extra {
            m.insert(k.clone(), v.clone());
        }
    }
    let tile_type = match h.tile_type {
        1 => TileType::Mvt,
        2 => TileType::Png,
        3 => TileType::Jpeg,
        4 => TileType::Webp,
        _ => TileType::Unknown,
    };
    let compression = match h.tile_compression {
        pmt::COMPRESSION_NONE => Compression::None,
        pmt::COMPRESSION_GZIP => Compression::Gzip,
        c => bail!("unsupported tile compression {c}"),
    };
    let b = o.bbox;
    let mut w = PmTilesWriter::new(tile_type)
        .precompressed(compression)
        .min_zoom(o.min_zoom)
        .max_zoom(max_zoom)
        .bounds(b[0], b[1], b[2], b[3])
        .center((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)
        .center_zoom(o.min_zoom.max(6).min(max_zoom))
        .metadata(serde_json::to_string(&meta)?)
        .dedup_by_key::<u64>()
        .create(BufWriter::new(File::create(out)?))?;
    for (id, e) in &found {
        let i = by_offset[&e.offset];
        w.add_tile(pmt::tile_coord(*id)?, &data[i], Some(e.offset))?;
    }
    w.finalize()?;
    let out_bytes = std::fs::metadata(out)?.len();
    eprintln!("wrote {} ({:.1} MB) in {:.0}s", out.display(), out_bytes as f64 / 1e6, t0.elapsed().as_secs_f32());
    Ok(Stats {
        tiles: found.len(),
        unique: spans.len(),
        downloaded: remote.bytes.load(Ordering::Relaxed),
        requests: remote.requests.load(Ordering::Relaxed),
        out_bytes,
    })
}
