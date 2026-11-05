mod browse;
mod build;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use sp_search::text::{snippet, strip_tags};
use sp_search::{Mode, Searcher};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "sp", about = "Survivalpedia pack tool")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Bm25,
    Dense,
    Hybrid,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a pack from ingest output dirs (each with source.json, docs.jsonl, chunks.jsonl, emb/)
    /// Write <pack>/browse.json: sections for the hub's /browse pages (from categories, a record's
    /// `browse` field, or a title<TAB>section TSV)
    Browse {
        pack: PathBuf,
        /// How many levels of a doc's categories make its section path
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// TSV of title<TAB>section path ("A › B"), taking precedence over the records
        #[arg(long)]
        sections: Option<PathBuf>,
        /// Keep the pack's order within a section (guides), instead of titles A–Z
        #[arg(long)]
        pack_order: bool,
    },
    Build {
        #[arg(long)]
        out: PathBuf,
        /// Embedding GGUF to copy into the pack's models/ dir
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long)]
        no_vec: bool,
        /// Embedding subdir in each input dir
        #[arg(long, default_value = "emb")]
        emb_name: String,
        /// IVF list count (default √N)
        #[arg(long)]
        lists: Option<usize>,
        /// Keep table-only chunks (infoboxes) in the vector index
        #[arg(long)]
        vec_tables: bool,
        /// Vector lengths to build (Matryoshka truncation)
        #[arg(long, value_delimiter = ',', default_value = "256,128")]
        dims: Vec<usize>,
        /// One vector index over all sources at index/vec/ with the first --dims (experiments)
        #[arg(long)]
        single_vec: bool,
        /// Pageview totals dir (ingest/pageviews.py) for the core/rest tier split
        #[arg(long)]
        pageviews: Option<PathBuf>,
        /// Share of each source's chunks in the core tier
        #[arg(long, default_value_t = 0.3)]
        core_frac: f64,
        /// Collection id (stable, used in URLs and download paths; default: the --out dir name)
        #[arg(long)]
        id: Option<String>,
        /// Collection title shown in the app, e.g. "Wikipedia Essentials"
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        description: Option<String>,
        /// articles | guides | books | …
        #[arg(long, default_value = "articles")]
        kind: String,
        inputs: Vec<PathBuf>,
    },
    /// Cut a map region out of a remote PMTiles build (HTTP range requests)
    MapExtract {
        /// Source archive URL, e.g. https://build.protomaps.com/20261030.pmtiles
        #[arg(long)]
        url: String,
        /// min_lon,min_lat,max_lon,max_lat
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        bbox: Vec<f64>,
        #[arg(long, default_value_t = 0)]
        min_zoom: u8,
        #[arg(long, default_value_t = 15)]
        max_zoom: u8,
        #[arg(long, default_value_t = 16)]
        threads: usize,
        /// Region id recorded in the metadata
        #[arg(long)]
        region: String,
        /// Human-readable region name
        #[arg(long)]
        title: Option<String>,
        out: PathBuf,
    },
    /// Show a PMTiles archive's header and metadata summary
    MapInfo { file: PathBuf },
    /// Build the place-search index for a region archive (writes <file stem>.places/)
    MapPlaces { file: PathBuf },
    /// Search a region's place index
    MapFind {
        file: PathBuf,
        query: String,
        /// lon,lat to favour nearby results
        #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
        near: Vec<f64>,
        #[arg(short, default_value_t = 10)]
        k: usize,
    },
    /// Print named features of one tile (debugging the gazetteer)
    MapDump {
        file: PathBuf,
        z: u8,
        x: u32,
        y: u32,
        #[arg(long, value_delimiter = ',', default_value = "places,pois,roads")]
        layers: Vec<String>,
        #[arg(long, default_value_t = 5)]
        per_layer: usize,
    },
    /// Search a pack
    Search {
        pack: PathBuf,
        query: String,
        #[arg(long, value_enum, default_value = "hybrid")]
        mode: ModeArg,
        #[arg(short, default_value_t = 10)]
        k: usize,
        #[arg(long)]
        nprobe: Option<usize>,
        /// Embedding GGUF (default: the pack's models/*.gguf)
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Print a doc by number or title
    Get {
        pack: PathBuf,
        doc: String,
        /// Print the stored HTML instead of a text rendering
        #[arg(long)]
        html: bool,
    },
    /// Title prefix autocomplete
    Title {
        pack: PathBuf,
        prefix: String,
        #[arg(short, default_value_t = 20)]
        k: usize,
    },
    /// Dense-index recall vs exact search over the original f16 vectors
    VecCheck {
        pack: PathBuf,
        /// JSONL with a "q" field per line (golden or synthetic queries)
        #[arg(long)]
        queries: Vec<PathBuf>,
        /// Ingest dirs in the same order as given to `sp build` (their emb/emb_f16.npy)
        #[arg(long, num_args = 1..)]
        emb: Vec<PathBuf>,
        #[arg(long)]
        model: Option<PathBuf>,
        #[arg(short, default_value_t = 10)]
        k: usize,
    },
    /// Embed texts with the GGUF model and print JSON vectors (agreement checks)
    Embed {
        #[arg(long)]
        model: PathBuf,
        /// Prepend this pack's query instruction
        #[arg(long)]
        query_pack: Option<PathBuf>,
        texts: Vec<String>,
    },
}

fn find_model(pack: &Path, explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(m) = explicit {
        return Ok(m);
    }
    let dir = pack.join("models");
    for e in std::fs::read_dir(&dir).with_context(|| format!("no --model and no {}", dir.display()))? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "gguf") {
            return Ok(p);
        }
    }
    bail!("no .gguf in {}", dir.display())
}

fn threads() -> usize {
    sp_embed::default_threads()
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Browse { pack, depth, sections, pack_order } => {
            browse::run(browse::Opts { pack, depth, sections, pack_order })?;
        }
        Cmd::Build { out, model, no_vec, emb_name, lists, vec_tables, dims, single_vec, pageviews, core_frac, id, title, description, kind, inputs } => {
            let dir_name = out.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let mut collection = serde_json::json!({"id": id.unwrap_or(dir_name), "kind": kind});
            if let Some(t) = title { collection["title"] = t.into(); }
            if let Some(d) = description { collection["description"] = d.into(); }
            build::run(build::Opts {
                inputs, out: out.clone(), no_vec, emb_name, lists, vec_tables, dims, single_vec, pageviews, core_frac, collection,
            })?;
            if let Some(m) = model {
                std::fs::create_dir_all(out.join("models"))?;
                std::fs::copy(&m, out.join("models").join(m.file_name().unwrap()))?;
            }
        }
        Cmd::MapExtract { url, bbox, min_zoom, max_zoom, threads, region, title, out } => {
            anyhow::ensure!(bbox.len() == 4, "--bbox needs min_lon,min_lat,max_lon,max_lat");
            let meta = serde_json::json!({
                "survivalpedia": {"region": region, "title": title.unwrap_or_else(|| region.clone()), "bbox": bbox, "source_url": url},
            });
            let o = sp_maps::extract::Opts {
                url, bbox: [bbox[0], bbox[1], bbox[2], bbox[3]], min_zoom, max_zoom, threads, meta,
            };
            let s = sp_maps::extract::extract(&o, &out)?;
            println!(
                "{}",
                serde_json::json!({
                    "tiles": s.tiles, "distinct": s.unique, "downloaded_bytes": s.downloaded,
                    "requests": s.requests, "out_bytes": s.out_bytes,
                })
            );
        }
        Cmd::MapInfo { file } => {
            let a = sp_maps::pmt::Archive::open(&file)?;
            let m = a.metadata()?;
            println!("{:#?}", a.header);
            let layers: Vec<_> = m["vector_layers"].as_array().into_iter().flatten().filter_map(|l| l["id"].as_str()).collect();
            println!("layers: {layers:?}");
            println!("survivalpedia: {}", m["survivalpedia"]);
            for k in ["name", "version", "attribution", "planetiler:osm:osmosisreplicationtime"] {
                if !m[k].is_null() {
                    println!("{k}: {}", m[k]);
                }
            }
        }
        Cmd::MapPlaces { file } => {
            let meta = sp_maps::places::build(&file, &file.with_extension("places"))?;
            println!("{}", meta["counts"]);
        }
        Cmd::MapFind { file, query, near, k } => {
            let g = sp_maps::places::Gazetteer::open(&file.with_extension("places"))?;
            let t = Instant::now();
            let near = (near.len() == 2).then(|| (near[0], near[1]));
            let hits = g.search(&query, near, k);
            for h in &hits {
                let ctx = if h.context.is_empty() { String::new() } else { format!(", {}", h.context) };
                let m = h.matched.as_deref().map(|m| format!(" ({m})")).unwrap_or_default();
                println!("{:6.1}  {}{m}{ctx}  [{:?}/{}] {:.5},{:.5}", h.score, h.name, h.class, h.kind, h.lat, h.lon);
            }
            println!("{} hits in {:.2} ms", hits.len(), t.elapsed().as_secs_f64() * 1e3);
        }
        Cmd::MapDump { file, z, x, y, layers, per_layer } => {
            let a = sp_maps::pmt::Archive::open(&file)?;
            let raw = a.tile(z, x, y)?.context("no such tile")?;
            let data = sp_maps::pmt::decompress(raw, a.header.tile_compression)?;
            let want: Vec<&str> = layers.iter().map(|s| s.as_str()).collect();
            for l in sp_maps::mvt::decode(&data, &want)? {
                let named = l.features.iter().filter(|f| f.get("name").is_some()).count();
                println!("== {} ({} features, {named} named, extent {})", l.name, l.features.len(), l.extent);
                for f in l.features.iter().filter(|f| f.get("name").is_some()).take(per_layer) {
                    let (lon, lat) = sp_maps::mvt::to_lonlat(z, x, y, l.extent, f.point);
                    let props: Vec<String> = f.props.iter().map(|(k, v)| format!("{k}={v:?}")).collect();
                    println!("  [{lat:.5},{lon:.5}] type {} {}", f.geom_type, props.join(" "));
                }
            }
        }
        Cmd::Search { pack, query, mode, k, nprobe, model, json } => {
            let t = Instant::now();
            let mut s = Searcher::open(&pack)?;
            if let Some(np) = nprobe {
                s.vec_params.nprobe = np;
            }
            let t_open = t.elapsed();
            let mode = match mode {
                ModeArg::Bm25 => Mode::Bm25,
                ModeArg::Dense => Mode::Dense,
                ModeArg::Hybrid => Mode::Hybrid,
            };
            let (qvec, t_embed) = if mode != Mode::Bm25 {
                let e = sp_embed::Embedder::load(&find_model(&pack, model)?, threads())?;
                let prefix = s.pack.manifest["embedding"]["query_instruction"].as_str().unwrap_or("");
                let t = Instant::now();
                let v = e.embed(&format!("{prefix}{query}"))?;
                (Some(v), t.elapsed())
            } else {
                (None, Default::default())
            };
            let t = Instant::now();
            let hits = s.search(&query, qvec.as_deref(), mode, k)?;
            let t_search = t.elapsed();
            let mut rows = Vec::new();
            for (i, h) in hits.iter().enumerate() {
                let doc = s.pack.get(h.doc)?;
                let c = s.pack.chunk(h.chunk);
                let html = doc.record["html"].as_str().unwrap_or("");
                let snip = snippet(html, c.block_first, c.block_last, 160);
                if json {
                    rows.push(serde_json::json!({
                        "rank": i + 1, "doc": h.doc, "title": doc.title, "source": doc.source, "lang": doc.lang,
                        "chunk": h.chunk, "block": format!("p{}", c.block_first), "score": h.score,
                        "bm25_rank": h.bm25_rank, "dense_rank": h.dense_rank, "snippet": snip,
                    }));
                } else {
                    let r = |x: Option<usize>| x.map(|v| v.to_string()).unwrap_or("-".into());
                    println!(
                        "{:>2}. {} [{}] #p{}  (bm25 {}, dense {})\n    {}",
                        i + 1, doc.title, doc.source, c.block_first, r(h.bm25_rank), r(h.dense_rank), snip
                    );
                }
            }
            let timing = serde_json::json!({
                "open_ms": t_open.as_secs_f64() * 1e3, "embed_ms": t_embed.as_secs_f64() * 1e3,
                "search_ms": t_search.as_secs_f64() * 1e3,
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({"query": query, "hits": rows, "timing": timing}))?);
            } else {
                eprintln!("timing: {timing}");
            }
        }
        Cmd::Get { pack, doc, html } => {
            let p = sp_pack::Pack::open(&pack)?;
            let n = match doc.parse::<u32>() {
                Ok(n) => n,
                Err(_) => *p.lookup_title(&doc).first().context("no such title")?,
            };
            let d = p.get(n)?;
            let body = d.record["html"].as_str().unwrap_or("");
            println!("# {}  [{} / {}]  doc {}", d.title, d.source, d.lang, n);
            println!(
                "{}  rev {}  {}",
                d.record["url"].as_str().unwrap_or(""),
                d.record["rev_date"].as_str().unwrap_or("?"),
                d.record["license"].as_str().unwrap_or("?")
            );
            println!();
            if html {
                println!("{body}");
            } else {
                for part in body.split(" id=\"p").skip(1) {
                    let (id, rest) = part.split_once('"').unwrap_or(("", part));
                    let text = strip_tags(&format!("<x {rest}"));
                    if !text.is_empty() {
                        println!("[p{id}] {text}");
                    }
                }
            }
        }
        Cmd::Title { pack, prefix, k } => {
            let p = sp_pack::Pack::open(&pack)?;
            for (key, doc) in p.complete(&prefix, k) {
                println!("{key}\t-> {} [{}] doc {doc}", p.title(doc), p.source(doc));
            }
        }
        Cmd::VecCheck { pack, queries, emb, model, k } => vec_check(&pack, &queries, &emb, model, k)?,
        Cmd::Embed { model, query_pack, texts } => {
            let prefix = match query_pack {
                Some(p) => {
                    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(p.join("manifest.json"))?)?;
                    m["embedding"]["query_instruction"].as_str().unwrap_or("").to_string()
                }
                None => String::new(),
            };
            let e = sp_embed::Embedder::load(&model, threads())?;
            let mut out = Vec::new();
            for t in texts {
                out.push(e.embed(&format!("{prefix}{t}"))?);
            }
            println!("{}", serde_json::to_string(&out)?);
        }
    }
    Ok(())
}

fn vec_check(pack: &Path, queries: &[PathBuf], emb: &[PathBuf], model: Option<PathBuf>, k: usize) -> Result<()> {
    use rayon::prelude::*;
    let s = Searcher::open(pack)?;
    anyhow::ensure!(s.vecs.len() == 1, "vec-check needs a pack with exactly one vector component");
    let v = &s.vecs[0].index;
    let d = v.d;
    let mut data: Vec<f32> = Vec::new();
    for dir in emb {
        let a = sp_pack::npy::Npy::load(&dir.join("emb/emb_f16.npy"))?;
        data.extend(a.to_f32()?);
    }
    anyhow::ensure!(data.len() / d == v.n, "emb rows {} != index n {}", data.len() / d, v.n);
    let mut qs = Vec::new();
    for f in queries {
        for l in std::fs::read_to_string(f)?.lines().filter(|l| !l.trim().is_empty()) {
            let j: serde_json::Value = serde_json::from_str(l)?;
            qs.push(j["q"].as_str().unwrap_or("").to_string());
        }
    }
    let e = sp_embed::Embedder::load(&find_model(pack, model)?, threads())?;
    let prefix = s.pack.manifest["embedding"]["query_instruction"].as_str().unwrap_or("").to_string();
    let qv: Vec<Vec<f32>> = qs
        .iter()
        .map(|q| {
            let full = e.embed(&format!("{prefix}{q}"))?;
            let mut x = full[..d].to_vec();
            let n = x.iter().map(|a| a * a).sum::<f32>().sqrt().max(1e-12);
            x.iter_mut().for_each(|a| *a /= n);
            Ok(x)
        })
        .collect::<Result<_>>()?;
    let exact: Vec<Vec<u32>> = qv
        .par_iter()
        .map(|q| {
            let mut sc: Vec<(f32, u32)> =
                data.chunks_exact(d).enumerate().map(|(i, r)| (r.iter().zip(q).map(|(a, b)| a * b).sum(), i as u32)).collect();
            sc.select_nth_unstable_by(k - 1, |a, b| b.0.total_cmp(&a.0));
            sc.truncate(k);
            sc.into_iter().map(|x| x.1).collect()
        })
        .collect();
    println!("queries {}  n {}  lists {}  k {k}", qs.len(), v.n, v.lists);
    for (nprobe, rerank) in [(8, 400), (16, 400), (32, 400), (64, 400), (32, 1000), (128, 2000), (100000, 100000)] {
        let p = sp_search::vec::VecParams { nprobe, rerank };
        let t = Instant::now();
        let mut hit = 0usize;
        for (q, ex) in qv.iter().zip(&exact) {
            let got: std::collections::HashSet<u32> = v.search(q, k, &p).into_iter().map(|x| x.0).collect();
            hit += ex.iter().filter(|c| got.contains(c)).count();
        }
        let ms = t.elapsed().as_secs_f64() * 1e3 / qs.len() as f64;
        let label = if nprobe >= 100000 { "all lists (quantisation loss only)".to_string() } else { format!("nprobe {nprobe:>3} rerank {rerank:>4}") };
        println!("{label:<36} recall@{k} {:.3}   {ms:.2} ms/query", hit as f64 / (qs.len() * k) as f64);
    }
    Ok(())
}
