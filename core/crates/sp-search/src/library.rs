//! A library of collections (DESIGN §5): one pack per collection (e.g. Wikipedia Essentials, wikiHow,
//! survival guides, books), any subset installed. Search ranks across all of them at once: BM25 hits
//! are merged by score and dense hits by cosine (every collection uses the same embedding model),
//! then the two global lists are fused with RRF, as in `Searcher::search`.

use crate::{Hit, Mode, RRF_K, Searcher};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Collection {
    /// Stable id (manifest `collection.id`, else the directory name); used in URLs.
    pub id: String,
    pub title: String,
    pub description: String,
    /// "articles", "guides", "books", …
    pub kind: String,
    pub searcher: Searcher,
}

impl Collection {
    pub fn open(dir: &Path) -> Result<Collection> {
        let searcher = Searcher::open(dir).with_context(|| format!("opening collection {}", dir.display()))?;
        let m = &searcher.pack.manifest;
        let c = &m["collection"];
        let dir_name = dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let id = c["id"].as_str().map(String::from).unwrap_or(dir_name);
        let title = c["title"].as_str().map(String::from).unwrap_or_else(|| {
            m["sources"].as_array().into_iter().flatten().filter_map(|s| s["title"].as_str()).collect::<Vec<_>>().join(", ")
        });
        Ok(Collection {
            id,
            title,
            description: c["description"].as_str().unwrap_or("").to_string(),
            kind: c["kind"].as_str().unwrap_or("articles").to_string(),
            searcher,
        })
    }
}

/// A hit in one collection.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LibHit {
    pub col: usize,
    #[serde(flatten)]
    pub hit: Hit,
}

#[derive(Default)]
pub struct Library {
    pub cols: Vec<Collection>,
}

/// Pack directories under `dir` (each subdirectory holding a manifest.json), sorted by name.
pub fn collection_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("manifest.json").is_file())
        .collect();
    v.sort();
    v
}

impl Library {
    /// Opens each pack directory as a collection. Ids must be unique.
    pub fn open(dirs: &[PathBuf]) -> Result<Library> {
        let mut cols = Vec::new();
        for d in dirs {
            let c = Collection::open(d)?;
            anyhow::ensure!(!cols.iter().any(|o: &Collection| o.id == c.id), "two collections with id {}", c.id);
            cols.push(c);
        }
        Ok(Library { cols })
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.cols.iter().position(|c| c.id == id)
    }

    pub fn has_dense(&self) -> bool {
        self.cols.iter().any(|c| c.searcher.has_dense())
    }

    pub fn doc_count(&self) -> usize {
        self.cols.iter().map(|c| c.searcher.pack.doc_count()).sum()
    }

    pub fn chunk_count(&self) -> usize {
        self.cols.iter().map(|c| c.searcher.pack.chunk_count()).sum()
    }

    /// Ranked docs across every collection; each doc is represented by its best chunk.
    /// `only`: restrict to one collection.
    pub fn search(&self, query: &str, qvec: Option<&[f32]>, mode: Mode, k: usize, only: Option<usize>) -> Result<Vec<LibHit>> {
        let depth = (k * 10).max(100);
        let cols: Vec<usize> = match only {
            Some(i) => vec![i],
            None => (0..self.cols.len()).collect(),
        };
        let mut b: Vec<(usize, u32, f32)> = Vec::new();
        let mut d: Vec<(usize, u32, f32)> = Vec::new();
        for &i in &cols {
            let s = &self.cols[i].searcher;
            if mode != Mode::Dense {
                b.extend(s.bm25(query, depth)?.into_iter().map(|(c, x)| (i, c, x)));
            }
            if let (Mode::Dense | Mode::Hybrid, Some(q)) = (mode, qvec) {
                d.extend(s.dense(q, depth).into_iter().map(|(c, x)| (i, c, x)));
            }
        }
        for l in [&mut b, &mut d] {
            l.sort_by(|x, y| y.2.total_cmp(&x.2));
            l.truncate(depth);
        }
        let mut acc: HashMap<(usize, u32), LibHit> = HashMap::new();
        let mut add = |list: &[(usize, u32, f32)], is_bm25: bool| {
            for (r, &(col, chunk, _)) in list.iter().enumerate() {
                let doc = self.cols[col].searcher.pack.chunk(chunk).doc;
                let h = acc.entry((col, doc)).or_insert(LibHit {
                    col,
                    hit: Hit { doc, chunk, score: 0.0, bm25_rank: None, dense_rank: None },
                });
                let rank = if is_bm25 { &mut h.hit.bm25_rank } else { &mut h.hit.dense_rank };
                if rank.is_none() {
                    *rank = Some(r + 1);
                    h.hit.score += 1.0 / (RRF_K + r as f32 + 1.0);
                }
            }
        };
        add(&b, true);
        add(&d, false);
        if !d.is_empty() {
            // docs no installed vector component covers (whole collections without one, or partial
            // tiers): impute the dense rank from BM25 so hybrid fusion doesn't bury them
            for h in acc.values_mut() {
                if h.hit.dense_rank.is_none() && !self.cols[h.col].searcher.has_dense_doc(h.hit.doc) {
                    if let Some(r) = h.hit.bm25_rank {
                        h.hit.score += 1.0 / (RRF_K + r as f32);
                    }
                }
            }
        }
        let mut hits: Vec<LibHit> = acc.into_values().collect();
        hits.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
        hits.truncate(k);
        Ok(hits)
    }

    /// Exact title lookup: `prefer` first, then the other collections.
    pub fn lookup_title(&self, title: &str, prefer: Option<usize>) -> Option<(usize, u32)> {
        let order = prefer.into_iter().chain((0..self.cols.len()).filter(|&i| Some(i) != prefer));
        for i in order {
            if let Some(&d) = self.cols[i].searcher.pack.lookup_title(title).first() {
                return Some((i, d));
            }
        }
        None
    }

    /// Prefix autocomplete across collections: (key, collection, doc), shortest keys first.
    pub fn complete(&self, prefix: &str, limit: usize) -> Vec<(String, usize, u32)> {
        let mut out: Vec<(String, usize, u32)> = Vec::new();
        for (i, c) in self.cols.iter().enumerate() {
            out.extend(c.searcher.pack.complete(prefix, limit).into_iter().map(|(k, d)| (k, i, d)));
        }
        out.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
        out.truncate(limit);
        out
    }

    /// Image bytes from whichever collection has the key.
    pub fn image(&self, key: &[u8; 12], variant: &str) -> Result<Option<Vec<u8>>> {
        for c in &self.cols {
            if let Some(b) = c.searcher.pack.image(key, variant)? {
                return Ok(Some(b));
            }
        }
        Ok(None)
    }
}
