//! Search over a pack: BM25 per language + IVF dense vectors, fused with RRF (DESIGN §7).

pub mod bm25;
pub mod library;
pub mod text;
pub mod vec;

use anyhow::Result;
use sp_pack::Pack;
use std::collections::HashMap;
use std::path::Path;

pub const RRF_K: f32 = 60.0;

pub struct Searcher {
    pub pack: Pack,
    pub bm25: Option<bm25::Bm25Index>,
    /// Installed vector components (DESIGN §5); empty means BM25 only.
    pub vecs: Vec<VecComponent>,
    /// By doc: has at least one chunk in an installed vector component. With partial tiers
    /// (e.g. core only), hybrid fusion must not penalise docs dense search cannot see.
    dense_docs: Vec<bool>,
    /// nprobe is for a ~1024-list index and scales with √(each component's list count).
    pub vec_params: vec::VecParams,
}

/// One optional vector index: `index/vec/{source}.{tier}.d{dims}/`, or the legacy `index/vec/`.
pub struct VecComponent {
    pub name: String,
    pub index: vec::VecIndex,
}

/// `{source}.{tier}.d{dims}` -> (`{source}.{tier}`, dims)
fn component_key(name: &str) -> Option<(&str, usize)> {
    let (key, d) = name.rsplit_once(".d")?;
    Some((key, d.parse().ok()?))
}

fn open_vecs(dir: &Path) -> Result<Vec<VecComponent>> {
    let vp = dir.join("index/vec");
    if vp.join("vec.json").exists() {
        return Ok(vec![VecComponent { name: "all".into(), index: vec::VecIndex::open(&vp)? }]);
    }
    // per (source, tier), use the longest vectors installed
    let mut best: HashMap<String, (usize, String)> = HashMap::new();
    for e in std::fs::read_dir(&vp).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.path().join("vec.json").exists() {
            continue;
        }
        let Some((key, d)) = component_key(&name) else { continue };
        if best.get(key).is_none_or(|(bd, _)| d > *bd) {
            best.insert(key.to_string(), (d, name));
        }
    }
    let mut names: Vec<String> = best.into_values().map(|(_, n)| n).collect();
    names.sort();
    names.into_iter().map(|name| Ok(VecComponent { index: vec::VecIndex::open(&vp.join(&name))?, name })).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Bm25,
    Dense,
    Hybrid,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub doc: u32,
    pub chunk: u32,
    pub score: f32,
    pub bm25_rank: Option<usize>,
    pub dense_rank: Option<usize>,
}

impl Searcher {
    pub fn open(dir: &Path) -> Result<Self> {
        let pack = Pack::open(dir)?;
        // English only (DESIGN §1); bm25-zh/ja in older prototype packs are ignored
        let p = dir.join("index").join("bm25-en");
        let bm25 = if p.exists() { Some(bm25::Bm25Index::open(&p, "en")?) } else { None };
        let vecs = open_vecs(dir)?;
        let mut dense_docs = vec![false; pack.doc_count()];
        for c in &vecs {
            for id in c.index.chunk_ids() {
                dense_docs[pack.chunk(id).doc as usize] = true;
            }
        }
        Ok(Searcher { pack, bm25, vecs, dense_docs, vec_params: Default::default() })
    }

    /// BM25 over the English index (empty if the pack has none).
    pub fn bm25(&self, query: &str, k: usize) -> Result<Vec<(u32, f32)>> {
        match &self.bm25 {
            Some(idx) => idx.search(query, k),
            None => Ok(Vec::new()),
        }
    }

    pub fn has_dense(&self) -> bool {
        !self.vecs.is_empty()
    }

    /// Whether `doc` has a chunk in an installed vector component.
    pub fn has_dense_doc(&self, doc: u32) -> bool {
        self.dense_docs.get(doc as usize).copied().unwrap_or(false)
    }

    /// Top-k (chunk, cosine) over every installed component. `qvec` is the full query embedding;
    /// it is truncated and renormalised to each component's dims (Matryoshka).
    pub fn dense(&self, qvec: &[f32], k: usize) -> Vec<(u32, f32)> {
        let mut out: Vec<(u32, f32)> = Vec::new();
        for c in &self.vecs {
            let v = &c.index;
            let mut q: Vec<f32> = qvec[..v.d.min(qvec.len())].to_vec();
            let n = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            q.iter_mut().for_each(|x| *x /= n);
            // scale with √lists: small components need a larger probed share for the same recall
            let np = (self.vec_params.nprobe as f64 * (v.lists as f64 / 1024.0).sqrt()).ceil() as usize;
            let p = vec::VecParams {
                nprobe: np.clamp(8.min(v.lists), v.lists.max(1)),
                rerank: self.vec_params.rerank,
            };
            out.extend(v.search(&q, k, &p));
        }
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(k);
        out
    }

    /// Ranked docs; each doc is represented by its best chunk. `qvec` must be the full query
    /// embedding (it is truncated and renormalised to the index dims here).
    pub fn search(&self, query: &str, qvec: Option<&[f32]>, mode: Mode, k: usize) -> Result<Vec<Hit>> {
        let depth = (k * 10).max(100);
        let b = if mode != Mode::Dense { self.bm25(query, depth)? } else { Vec::new() };
        let d = match (mode, qvec) {
            (Mode::Bm25, _) | (_, None) => Vec::new(),
            (_, Some(q)) => self.dense(q, depth),
        };
        // RRF over chunk ranks, then keep each doc's best chunk.
        let mut acc: HashMap<u32, Hit> = HashMap::new();
        let mut add = |list: &[(u32, f32)], is_bm25: bool| {
            for (r, &(chunk, _)) in list.iter().enumerate() {
                let doc = self.pack.chunk(chunk).doc;
                let s = 1.0 / (RRF_K + r as f32 + 1.0);
                let h = acc.entry(doc).or_insert(Hit { doc, chunk, score: 0.0, bm25_rank: None, dense_rank: None });
                let rank = if is_bm25 { &mut h.bm25_rank } else { &mut h.dense_rank };
                if rank.is_none() {
                    // only the doc's best chunk per ranker counts
                    *rank = Some(r + 1);
                    h.score += s;
                }
            }
        };
        add(&b, true);
        add(&d, false);
        if !d.is_empty() {
            // docs outside every installed vector component: impute the dense rank from BM25
            for h in acc.values_mut() {
                if h.dense_rank.is_none() && !self.dense_docs[h.doc as usize] {
                    if let Some(r) = h.bm25_rank {
                        h.score += 1.0 / (RRF_K + r as f32);
                    }
                }
            }
        }
        let mut hits: Vec<Hit> = acc.into_values().collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);
        Ok(hits)
    }
}
