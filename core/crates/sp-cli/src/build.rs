//! `sp build`: ingest output dirs (zim_extract.py + embed.py) -> pack.

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sp_pack::npy::Npy;
use sp_pack::writer::checksum_files;
use sp_pack::{ChunkEntry, PackWriter, VARIANTS, parse_image_key};
use sp_search::bm25::Bm25Writer;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct Opts {
    pub inputs: Vec<PathBuf>,
    pub out: PathBuf,
    pub no_vec: bool,
    /// Embedding subdir in each input dir (default "emb")
    pub emb_name: String,
    pub lists: Option<usize>,
    /// Also embed-index chunks made only of tables (infoboxes); off by default because their
    /// identifier-heavy vectors become hubs that match unrelated queries. BM25 still has them.
    pub vec_tables: bool,
    /// Vector lengths to build (Matryoshka truncation, each ≤ the embedding dims), e.g. [256, 128].
    pub dims: Vec<usize>,
    /// Legacy layout: one index over all sources at `index/vec/` with `dims[0]` (experiments).
    pub single_vec: bool,
    /// Directory with `{wiki}.tsv` (page_id, views) from ingest/pageviews.py, for the core/rest split.
    pub pageviews: Option<PathBuf>,
    /// Share of each source's chunks that goes into the `core` tier (most-viewed docs first).
    pub core_frac: f64,
    /// Collection metadata for the manifest: {id, title?, description?, kind}
    pub collection: Value,
}

/// page_id -> views for a source, from `{lang}.wikipedia.tsv` when the source is a Wikipedia.
fn load_views(dir: &Path, s: &Value) -> Result<Option<HashMap<u64, u64>>> {
    let Some(origin) = s["zim_metadata"]["Source"].as_str() else { return Ok(None) };
    let p = dir.join(format!("{}.tsv", origin.trim_end_matches(".org")));
    if !p.exists() {
        return Ok(None);
    }
    let mut m = HashMap::new();
    for l in lines(&p)? {
        let l = l?;
        let mut f = l.split('\t');
        if let (Some(id), Some(n)) = (f.next(), f.next()) {
            m.insert(id.parse()?, n.parse()?);
        }
    }
    Ok(Some(m))
}

fn l2_rows(v: &[f32], full: usize, dims: usize, out: &mut Vec<f32>) {
    for row in v.chunks_exact(full) {
        let r = &row[..dims];
        let n = r.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        out.extend(r.iter().map(|x| x / n));
    }
}

fn lines(p: &Path) -> Result<impl Iterator<Item = Result<String>>> {
    let f = std::fs::File::open(p).with_context(|| format!("opening {}", p.display()))?;
    Ok(BufReader::with_capacity(1 << 20, f).lines().map(|l| l.map_err(Into::into)))
}

fn block_num(id: &str) -> u32 {
    id.trim_start_matches('p').parse().unwrap_or(0)
}

/// Block numbers of top-level `<table id="pN">` blocks in a cleaned doc.
fn table_blocks(html: &str) -> HashSet<u32> {
    html.match_indices("<table id=\"p")
        .filter_map(|(i, m)| {
            let rest = &html[i + m.len()..];
            rest[..rest.find('"')?].parse().ok()
        })
        .collect()
}

/// Doc record stored in the pack: drop fields the reader rebuilds or doesn't need.
fn record(mut d: Value) -> Value {
    if let Some(o) = d.as_object_mut() {
        for k in ["blocks", "images", "path"] {
            o.remove(k);
        }
    }
    d
}

pub fn run(o: Opts) -> Result<()> {
    let t0 = Instant::now();
    ensure!(!o.out.exists() || std::fs::read_dir(&o.out)?.next().is_none(), "{} is not empty", o.out.display());
    let mut w = PackWriter::create(&o.out)?;

    // Source descriptors
    let mut sources = Vec::new();
    for dir in &o.inputs {
        let s: Value = serde_json::from_slice(&std::fs::read(dir.join("source.json"))?)?;
        let id = format!("{}-{}", s["source"].as_str().unwrap_or("?"), s["lang"].as_str().unwrap_or("?"));
        sources.push((dir.clone(), id, s));
    }
    ensure!(sources.len() < 256, "too many sources");

    // 1. zstd dictionaries per language from sampled records
    let mut samples: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
    for (dir, _, s) in &sources {
        let lang = s["lang"].as_str().unwrap().to_string();
        let total = s["docs"].as_u64().unwrap_or(1).max(1);
        let step = (total / 3000).max(1) as usize;
        let v = samples.entry(lang).or_default();
        for (i, l) in lines(&dir.join("docs.jsonl"))?.enumerate() {
            if i % step == 0 {
                let rec = serde_json::to_vec(&record(serde_json::from_str(&l?)?))?;
                v.push(rec);
            }
        }
    }
    for (lang, s) in &samples {
        w.train_dict(lang, s)?;
        eprintln!("dict {lang}: {} samples", s.len());
    }

    // 2. docs, titles, redirects
    let mut doc_nums: Vec<HashMap<String, u32>> = Vec::new(); // per source: path -> doc
    let mut docid_num: HashMap<String, u32> = HashMap::new();
    let mut src_meta = Vec::new();
    let mut titles: Vec<String> = Vec::new(); // by doc number
    let mut tables: HashMap<u32, HashSet<u32>> = HashMap::new(); // doc -> table block numbers
    let mut page_ids: Vec<Option<u64>> = Vec::new(); // by doc number
    for (si, (dir, id, s)) in sources.iter().enumerate() {
        let lang = s["lang"].as_str().unwrap();
        let mut paths = HashMap::new();
        let mut licenses: HashMap<String, u64> = HashMap::new();
        for l in lines(&dir.join("docs.jsonl"))? {
            let d: Value = serde_json::from_str(&l?)?;
            let title = d["title"].as_str().unwrap_or("").to_string();
            let path = d["path"].as_str().unwrap_or("").to_string();
            let doc_id = d["doc_id"].as_str().unwrap_or("").to_string();
            *licenses.entry(d["license"].as_str().unwrap_or("?").to_string()).or_default() += 1;
            let tb = table_blocks(d["html"].as_str().unwrap_or(""));
            page_ids.push(d["page_id"].as_u64());
            let rec = serde_json::to_vec(&record(d))?;
            let n = w.add_doc(lang, si as u8, &title, &rec)?;
            w.add_title(&title, si as u8, n);
            titles.push(title);
            if !tb.is_empty() {
                tables.insert(n, tb);
            }
            paths.insert(path, n);
            docid_num.insert(doc_id, n);
        }
        w.flush_block()?;
        let mut redirects = 0u64;
        let rp = dir.join("redirects.jsonl");
        if rp.exists() {
            for l in lines(&rp)? {
                let r: Value = serde_json::from_str(&l?)?;
                if let Some(&n) = r["to_path"].as_str().and_then(|p| paths.get(p)) {
                    w.add_title(r["from_title"].as_str().unwrap_or(""), si as u8, n);
                    redirects += 1;
                }
            }
        }
        // a ZIM's own metadata, else source.json's top-level fields (docs_extract.py, guides)
        let zm = &s["zim_metadata"];
        let meta = |zim: &str, own: &str| if zm[zim].is_null() { s[own].clone() } else { zm[zim].clone() };
        src_meta.push(json!({
            "id": id, "source": s["source"], "lang": lang, "zim": s["zim"],
            "title": meta("Title", "title"), "date": meta("Date", "date"), "origin": meta("Source", "url"),
            "creator": meta("Creator", "author"), "publisher": meta("Publisher", "publisher"),
            "docs": paths.len(), "redirects": redirects, "licenses": licenses,
        }));
        eprintln!("docs {id}: {} docs, {redirects} redirects ({:.0}s)", paths.len(), t0.elapsed().as_secs_f32());
        doc_nums.push(paths);
    }

    // 3. images (dedup by content key across sources)
    let mut seen: HashSet<[u8; 12]> = HashSet::new();
    for (dir, id, _) in &sources {
        let ip = dir.join("images.jsonl");
        if !ip.exists() {
            continue;
        }
        let mut added = 0;
        for l in lines(&ip)? {
            let r: Value = serde_json::from_str(&l?)?;
            let hex = r["key"].as_str().unwrap_or("");
            let Some(key) = parse_image_key(hex) else { continue };
            if !seen.insert(key) {
                continue;
            }
            for v in r["variants"].as_array().into_iter().flatten() {
                let v = v.as_str().unwrap_or("");
                let name = if v == "svg" { format!("{hex}.svg") } else { format!("{hex}.{v}.webp") };
                let data = std::fs::read(dir.join("images").join(&hex[..2]).join(name))?;
                let vi = VARIANTS.iter().position(|x| *x == v).context("variant")? as u8;
                w.add_image(key, vi, &data)?;
            }
            added += 1;
        }
        eprintln!("images {id}: {added} new ({:.0}s)", t0.elapsed().as_secs_f32());
    }
    w.finish()?;

    // 4. chunks + BM25 (one tantivy index per language, built in parallel)
    let mut chunks: Vec<ChunkEntry> = Vec::new();
    let mut per_lang: HashMap<String, Vec<(u32, u32, String)>> = HashMap::new(); // lang -> (chunk, doc, body)
    let mut chunk_counts = Vec::new();
    let mut chunk_ord: HashMap<String, u32> = HashMap::new();
    let mut table_only: Vec<bool> = Vec::new(); // by chunk ordinal
    for (dir, id, s) in &sources {
        let lang = s["lang"].as_str().unwrap().to_string();
        let start = chunks.len();
        for l in lines(&dir.join("chunks.jsonl"))? {
            let c: Value = serde_json::from_str(&l?)?;
            let doc = *docid_num.get(c["doc_id"].as_str().unwrap_or("")).context("chunk for unknown doc")?;
            let ids = c["block_ids"].as_array().context("block_ids")?;
            let first = ids.first().and_then(|v| v.as_str()).map(block_num).unwrap_or(0);
            let last = ids.last().and_then(|v| v.as_str()).map(block_num).unwrap_or(first);
            let ord = chunks.len() as u32;
            chunk_ord.insert(c["chunk_id"].as_str().unwrap_or("").to_string(), ord);
            chunks.push(ChunkEntry { doc, block_first: first, block_last: last });
            table_only.push(tables.get(&doc).is_some_and(|tb| {
                ids.iter().all(|b| b.as_str().is_some_and(|b| tb.contains(&block_num(b))))
            }));
            per_lang.entry(lang.clone()).or_default().push((ord, doc, c["text"].as_str().unwrap_or("").to_string()));
        }
        chunk_counts.push((id.clone(), start, chunks.len()));
    }
    w.write_chunks(&chunks)?;
    eprintln!("chunks: {} ({:.0}s)", chunks.len(), t0.elapsed().as_secs_f32());

    let pack_dir = o.out.clone();
    std::thread::scope(|sc| -> Result<()> {
        let mut hs = Vec::new();
        for (lang, items) in &per_lang {
            let dir = pack_dir.join("index").join(format!("bm25-{lang}"));
            let titles = &titles;
            hs.push(sc.spawn(move || -> Result<()> {
                let mut bw = Bm25Writer::create(&dir, lang)?;
                for (ord, doc, body) in items {
                    bw.add(*ord, &titles[*doc as usize], body)?;
                }
                bw.finish()?;
                eprintln!("bm25 {lang}: {} chunks", items.len());
                Ok(())
            }));
        }
        for h in hs {
            h.join().map_err(|_| anyhow::anyhow!("bm25 thread panicked"))??;
        }
        Ok(())
    })?;
    eprintln!("bm25 done ({:.0}s)", t0.elapsed().as_secs_f32());

    // 5. vectors: per source, `core` (most-viewed docs) and `rest` tiers at each length (DESIGN §5)
    let mut emb_manifest = Value::Null;
    let mut vec_meta = Value::Null;
    if !o.no_vec {
        ensure!(!o.dims.is_empty(), "no vector dims given");
        let mut comps = Vec::new();
        let mut single: (Vec<f32>, Vec<u32>) = (Vec::new(), Vec::new());
        let mut skipped = 0usize;
        for (dir, id, s) in &sources {
            let ed = dir.join(&o.emb_name);
            let m: Value = serde_json::from_slice(&std::fs::read(ed.join("manifest.json")).with_context(|| {
                format!("{id}: no embeddings (run ingest/embed.py, or pass --no-vec)")
            })?)?;
            if emb_manifest.is_null() {
                emb_manifest = m.clone();
            }
            for k in ["model", "dims", "query_instruction", "document_prefix", "pooling"] {
                ensure!(m[k] == emb_manifest[k], "{id}: embedding setting {k} differs from the other sources");
            }
            let ids = std::fs::read_to_string(ed.join("chunk_ids.txt"))?;
            let ords = ids
                .lines()
                .map(|cid| chunk_ord.get(cid).copied().with_context(|| format!("{id}: unknown chunk_id {cid}")))
                .collect::<Result<Vec<u32>>>()?;
            let a = Npy::load(&ed.join("emb_f16.npy"))?;
            ensure!(a.shape.len() == 2 && a.shape[0] == ords.len(), "{id}: emb_f16.npy shape {:?}", a.shape);
            let full = a.shape[1];
            for &d in &o.dims {
                ensure!(d <= full && d % 8 == 0, "dims {d}: must be a multiple of 8 and ≤ {full}");
            }
            let raw = a.to_f32()?;
            let keep: Vec<usize> = (0..ords.len()).filter(|&i| o.vec_tables || !table_only[ords[i] as usize]).collect();
            skipped += ords.len() - keep.len();

            if o.single_vec {
                for &i in &keep {
                    single.1.push(ords[i]);
                    l2_rows(&raw[i * full..(i + 1) * full], full, o.dims[0], &mut single.0);
                }
                continue;
            }

            // tier: rank docs by views, fill `core` up to core_frac of this source's chunks
            let views = match &o.pageviews {
                Some(pv) => load_views(pv, s)?,
                None => None,
            };
            let mut core_docs: HashSet<u32> = HashSet::new();
            if let Some(views) = &views {
                let mut per_doc: HashMap<u32, usize> = HashMap::new();
                for &i in &keep {
                    *per_doc.entry(chunks[ords[i] as usize].doc).or_default() += 1;
                }
                let mut docs: Vec<(u64, u32, usize)> = per_doc
                    .iter()
                    .map(|(&doc, &n)| (page_ids[doc as usize].and_then(|p| views.get(&p).copied()).unwrap_or(0), doc, n))
                    .collect();
                docs.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                let budget = (keep.len() as f64 * o.core_frac).round() as usize;
                let mut used = 0;
                for (_, doc, n) in docs {
                    if used >= budget {
                        break;
                    }
                    core_docs.insert(doc);
                    used += n;
                }
            } else {
                eprintln!("vec {id}: no pageviews, every doc goes to core");
            }
            for tier in ["core", "rest"] {
                let rows: Vec<usize> = keep
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let core = views.is_none() || core_docs.contains(&chunks[ords[i] as usize].doc);
                        core == (tier == "core")
                    })
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                let row_ids: Vec<u32> = rows.iter().map(|&i| ords[i]).collect();
                for &d in &o.dims {
                    let mut data = Vec::with_capacity(rows.len() * d);
                    for &i in &rows {
                        l2_rows(&raw[i * full..(i + 1) * full], full, d, &mut data);
                    }
                    let lists = o.lists.unwrap_or(((rows.len() as f64).sqrt().round() as usize).max(1));
                    let name = format!("{id}.{tier}.d{d}");
                    let mut meta = sp_search::vec::build(&pack_dir.join("index/vec").join(&name), &data, &row_ids, d, lists)?;
                    meta["name"] = json!(name);
                    meta["source"] = json!(id);
                    meta["tier"] = json!(tier);
                    meta["docs"] = json!(rows.iter().map(|&i| chunks[ords[i] as usize].doc).collect::<HashSet<_>>().len());
                    eprintln!("vec {name}: {} × {d}, {lists} lists ({:.0}s)", rows.len(), t0.elapsed().as_secs_f32());
                    comps.push(meta);
                }
            }
        }
        eprintln!("vec: skipped {skipped} table-only chunks");
        if o.single_vec {
            let (data, row_ids) = single;
            let d = o.dims[0];
            let n = row_ids.len();
            let lists = o.lists.unwrap_or(((n as f64).sqrt().round() as usize).max(1));
            vec_meta = sp_search::vec::build(&pack_dir.join("index/vec"), &data, &row_ids, d, lists)?;
            vec_meta["skipped_table_chunks"] = json!(skipped);
            eprintln!("vec: {n} × {d}, {lists} lists ({:.0}s)", t0.elapsed().as_secs_f32());
        } else {
            vec_meta = json!({"components": comps, "skipped_table_chunks": skipped, "core_frac": o.core_frac});
        }
    }

    // 6. manifest
    let mut files = Vec::new();
    let mut sizes: HashMap<String, u64> = HashMap::new();
    for (rel, size, hash) in checksum_files(&pack_dir)? {
        let group = if rel.starts_with("index/vec/") && rel.matches('/').count() >= 3 {
            rel.split('/').take(3).collect::<Vec<_>>().join("/")
        } else if rel.starts_with("index/") {
            rel.split('/').take(2).collect::<Vec<_>>().join("/")
        } else {
            rel.split(['-', '.']).next().unwrap_or("").to_string()
        };
        *sizes.entry(group).or_default() += size;
        files.push(json!({"path": rel, "size": size, "blake3": hash}));
    }
    let manifest = json!({
        "format": "survivalpedia-pack",
        "format_version": sp_pack::FORMAT_VERSION,
        "collection": o.collection,
        "created_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),
        "languages": per_lang.keys().collect::<Vec<_>>(),
        "sources": src_meta,
        "counts": {"docs": w.doc_count(), "chunks": chunks.len(), "images": seen.len()},
        "embedding": emb_manifest,
        "vec_index": vec_meta,
        "stats": w.stats,
        "sizes": sizes,
        "files": files,
        "license_note": "Text keeps each source's licence (see sources[].licenses); index and embedding files are CC BY-SA.",
    });
    std::fs::write(pack_dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    eprintln!("done in {:.0}s", t0.elapsed().as_secs_f32());
    if sizes.is_empty() {
        bail!("empty pack");
    }
    println!("{}", serde_json::to_string_pretty(&json!({"counts": manifest["counts"], "sizes": manifest["sizes"]}))?);
    Ok(())
}
