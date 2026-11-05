//! `sp browse`: sections for browsing a collection, written to `<pack>/browse.json` (read by sp-server's
//! /browse pages). Runs on a finished pack, so adding or changing sections needs no rebuild.
//!
//! A doc's section path comes from, in order: `--sections` (TSV `title<TAB>A › B`), the record's
//! `browse` field (array of names), or the first `--depth` of its `categories`. Docs without one go
//! under "Other". With `--order pack` each section keeps the pack's order (guides read in sequence),
//! otherwise titles A–Z.

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub struct Opts {
    pub pack: PathBuf,
    pub depth: usize,
    pub sections: Option<PathBuf>,
    pub pack_order: bool,
}

#[derive(Deserialize)]
struct Rec {
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    browse: Vec<String>,
}

pub fn run(o: Opts) -> Result<()> {
    let pack = sp_pack::reader::Pack::open(&o.pack)?;
    let by_title: HashMap<String, Vec<String>> = match &o.sections {
        Some(p) => std::fs::read_to_string(p)
            .with_context(|| format!("reading {}", p.display()))?
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(t, s)| (t.to_string(), s.split(" › ").map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()))
            .collect(),
        None => HashMap::new(),
    };
    let mut sections: BTreeMap<Vec<String>, Vec<u32>> = BTreeMap::new();
    let n = pack.doc_count() as u32;
    for d in 0..n {
        let title = pack.title(d);
        let path = if let Some(p) = by_title.get(title) {
            p.clone()
        } else {
            let r: Rec = serde_json::from_slice(&pack.record_bytes(d)?).with_context(|| format!("doc {d}"))?;
            if !r.browse.is_empty() {
                r.browse
            } else {
                r.categories.into_iter().take(o.depth).collect()
            }
        };
        let path = if path.is_empty() { vec!["Other".to_string()] } else { path };
        sections.entry(path).or_default().push(d);
    }
    if !o.pack_order {
        for docs in sections.values_mut() {
            docs.sort_by_cached_key(|&d| pack.title(d).to_lowercase());
        }
    }
    let out = json!({
        "order": if o.pack_order { "pack" } else { "title" },
        "sections": sections.iter().map(|(p, d)| json!({"path": p, "docs": d})).collect::<Vec<_>>(),
    });
    let tmp = o.pack.join("browse.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&out)?)?;
    std::fs::rename(&tmp, o.pack.join("browse.json"))?;
    let top: BTreeMap<&str, usize> = sections.iter().fold(BTreeMap::new(), |mut m, (p, d)| {
        *m.entry(p[0].as_str()).or_default() += d.len();
        m
    });
    println!("{} docs in {} sections; top level: {:?}", n, sections.len(), top);
    Ok(())
}
