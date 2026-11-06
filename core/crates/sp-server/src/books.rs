//! The ebook shelf's search (hub with `--books`): keywords over each book's title, author, subjects and
//! Gutenberg description (shelf.json + shelf-text.jsonl from ingest/books/epub_shelf.py). Book texts
//! are not indexed: books are for reading, documents for searching.
//!
//!   GET /api/books/search?q=…&limit=50  -> {"results": [{"id", "score"}…]}  best first
//!   GET /api/books/{id}                 -> {"id", "summary", "subjects"}

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path as FsPath;

const STOP: &[&str] = &[
    "a", "an", "and", "the", "of", "in", "on", "to", "by", "for", "with", "is", "it", "its", "as", "at", "or", "from",
    "this", "that", "be", "are", "was", "his", "her", "their", "who", "which", "into", "s",
];
// field weights: a word in the title counts most
const W_TITLE: f32 = 3.0;
const W_AUTHOR: f32 = 2.0;
const W_SUBJECT: f32 = 1.5;
const W_SUMMARY: f32 = 1.0;

pub(crate) struct Book {
    pub id: u64,
    pub rank: u32,
    pub title: String,
    pub author: String,
    /// has an illustrated edition (epub-images/), has a cover (covers/)
    pub ill: bool,
    pub cover: bool,
    summary: String,
    subjects: Vec<String>,
}

pub struct BookIndex {
    pub(crate) dir: std::path::PathBuf,
    pub(crate) books: Vec<Book>,
    pub(crate) by_id: HashMap<u64, usize>,
    /// Gutenberg's shelves: name and the books in each (indices into `books`, most read first)
    pub(crate) categories: Vec<(String, Vec<u32>)>,
    /// groups of categories (Fiction, Practical…): name, category indices
    pub(crate) groups: Vec<(String, Vec<usize>)>,
    /// shelf.json cut down to the books that are here, when some aren't (a hub holding only the
    /// most-read books); served at /api/books instead of the file
    pub filtered: Option<Value>,
    terms: BTreeMap<String, Vec<(u32, f32)>>, // term -> (book index, weighted tf)
    len: Vec<f32>,
    avg_len: f32,
}

pub fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase();
            // crude stemming, the same for books and queries: "horses" -> "horse" (not "glass"),
            // "growing" -> "grow", "building" -> "build"
            if w.len() > 5 && w.ends_with("ing") {
                w[..w.len() - 3].to_string()
            } else if w.len() > 4 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") {
                w[..w.len() - 1].to_string()
            } else {
                w
            }
        })
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

impl BookIndex {
    pub fn load(dir: &FsPath) -> anyhow::Result<BookIndex> {
        let shelf: Value = serde_json::from_slice(&std::fs::read(dir.join("shelf.json"))?)?;
        let mut text: HashMap<u64, (String, Vec<String>)> = HashMap::new();
        if let Ok(s) = std::fs::read_to_string(dir.join("shelf-text.jsonl")) {
            for l in s.lines() {
                let v: Value = serde_json::from_str(l)?;
                let subs = v["sub"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
                text.insert(v["id"].as_u64().unwrap_or(0), (v["d"].as_str().unwrap_or("").to_string(), subs));
            }
        }
        let mut books = Vec::new();
        let mut terms: BTreeMap<String, Vec<(u32, f32)>> = BTreeMap::new();
        let mut len = Vec::new();
        let mut cat_books: HashMap<usize, Vec<u32>> = HashMap::new();
        let mut kept: Vec<Value> = Vec::new();
        let mut cut = false;
        for b in shelf["books"].as_array().into_iter().flatten() {
            let id = b["id"].as_u64().unwrap_or(0);
            // only books whose EPUB is here; drop editions and covers that aren't
            if !dir.join(format!("epub/{id}.epub")).is_file() {
                cut = true;
                continue;
            }
            let mut b = b.clone();
            if b.get("si").is_some() && !dir.join(format!("epub-images/{id}.epub")).is_file() {
                b.as_object_mut().map(|o| o.remove("si"));
                cut = true;
            }
            if b.get("cs").is_some() && !dir.join(format!("covers/{id}.jpg")).is_file() {
                b.as_object_mut().map(|o| o.remove("cs"));
                cut = true;
            }
            let b = &b;
            let (summary, subjects) = text.remove(&id).unwrap_or_default();
            let mut tf: HashMap<String, f32> = HashMap::new();
            let mut n = 0.0;
            let fields: [(f32, String); 4] = [
                (W_TITLE, b["t"].as_str().unwrap_or("").to_string()),
                (W_AUTHOR, b["a"].as_str().unwrap_or("").to_string()),
                (W_SUBJECT, subjects.join(" ")),
                (W_SUMMARY, summary.clone()),
            ];
            for (w, f) in &fields {
                for t in tokens(f) {
                    *tf.entry(t).or_default() += w;
                    n += 1.0;
                }
            }
            let i = books.len() as u32;
            for (t, f) in tf {
                terms.entry(t).or_default().push((i, f));
            }
            len.push(n);
            for c in b["c"].as_array().into_iter().flatten().filter_map(|c| c.as_u64()) {
                cat_books.entry(c as usize).or_default().push(i);
            }
            books.push(Book {
                id,
                rank: b["r"].as_u64().unwrap_or(u64::MAX >> 33) as u32,
                title: b["t"].as_str().unwrap_or("").to_string(),
                author: b["a"].as_str().unwrap_or("").to_string(),
                ill: b.get("si").is_some(),
                cover: b.get("cs").is_some(),
                summary,
                subjects,
            });
            kept.push(b.clone());
        }
        let avg_len = len.iter().sum::<f32>() / len.len().max(1) as f32;
        let by_id = books.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
        let categories: Vec<(String, Vec<u32>)> = shelf["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ci, c)| {
                let mut list = cat_books.remove(&ci).unwrap_or_default();
                list.sort_by_key(|&i| books[i as usize].rank);
                (c["name"].as_str().unwrap_or("").to_string(), list)
            })
            .collect();
        let groups = shelf["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|g| {
                let cats = g["categories"].as_array().into_iter().flatten().filter_map(|c| c.as_u64().map(|c| c as usize)).collect();
                (g["name"].as_str().unwrap_or("").to_string(), cats)
            })
            .collect();
        let filtered = cut.then(|| {
            let mut f = shelf.clone();
            if let Some(cs) = f["categories"].as_array_mut() {
                for (ci, c) in cs.iter_mut().enumerate() {
                    c["count"] = json!(categories.get(ci).map_or(0, |c| c.1.len()));
                }
            }
            f["books"] = Value::Array(kept);
            f
        });
        Ok(BookIndex { dir: dir.to_path_buf(), books, by_id, categories, groups, filtered, terms, len, avg_len })
    }

    pub fn len(&self) -> usize {
        self.books.len()
    }

    /// Best matches: BM25 over the weighted fields, the last word also as a prefix (search as you type),
    /// books matching more of the words first, popularity as a gentle tie-breaker.
    pub fn search(&self, q: &str, limit: usize) -> Vec<(u64, f32)> {
        let qt = tokens(q);
        if qt.is_empty() {
            return Vec::new();
        }
        let n = self.books.len() as f32;
        let (k1, b) = (1.2f32, 0.5f32);
        let mut score: HashMap<u32, (f32, u32)> = HashMap::new();
        for (qi, t) in qt.iter().enumerate() {
            let last = qi + 1 == qt.len();
            // the term itself, and for the last word every term it starts (best of them per book)
            let mut per_book: HashMap<u32, f32> = HashMap::new();
            let lists: Vec<(&String, &Vec<(u32, f32)>)> = if last && t.len() >= 2 {
                self.terms.range(t.clone()..).take_while(|(k, _)| k.starts_with(t.as_str())).take(64).collect()
            } else {
                self.terms.get_key_value(t).into_iter().collect()
            };
            for (term, list) in lists {
                let idf = ((n - list.len() as f32 + 0.5) / (list.len() as f32 + 0.5) + 1.0).ln();
                let exact = if term == t { 1.0 } else { 0.7 };
                for &(d, tf) in list {
                    let norm = tf * (k1 + 1.0) / (tf + k1 * (1.0 - b + b * self.len[d as usize] / self.avg_len));
                    let s = idf * norm * exact;
                    let e = per_book.entry(d).or_default();
                    if s > *e {
                        *e = s;
                    }
                }
            }
            for (d, s) in per_book {
                let e = score.entry(d).or_default();
                e.0 += s;
                e.1 += 1;
            }
        }
        let ln_n = (n + 1.0).ln();
        let mut out: Vec<(u64, f32)> = score
            .into_iter()
            .map(|(d, (s, hit))| {
                let bk = &self.books[d as usize];
                let cover = hit as f32 / qt.len() as f32;
                let pop = 1.0 + 0.25 * (1.0 - (bk.rank as f32 + 1.0).ln() / ln_n).max(0.0);
                (bk.id, s * cover * cover * pop)
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out.truncate(limit);
        out
    }
}

#[derive(Deserialize)]
pub struct SearchParams {
    q: String,
    limit: Option<usize>,
}

pub async fn search(State(app): State<AppState>, Query(p): Query<SearchParams>) -> Result<Json<Value>, StatusCode> {
    let ix = app.books.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let hits = ix.search(&p.q, p.limit.unwrap_or(50).min(500));
    Ok(Json(json!({"results": hits.iter().map(|(id, s)| json!({"id": id, "score": s})).collect::<Vec<_>>()})))
}

pub async fn details(State(app): State<AppState>, Path(id): Path<u64>) -> Result<Json<Value>, StatusCode> {
    let ix = app.books.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let b = ix.by_id.get(&id).map(|&i| &ix.books[i]).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(json!({"id": b.id, "summary": b.summary, "subjects": b.subjects})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_drop_stopwords_and_plurals() {
        assert_eq!(tokens("The Horses of Dr. Jekyll's"), vec!["horse", "dr", "jekyll"]);
        assert_eq!(tokens("glass"), vec!["glass"]);
        assert_eq!(tokens("Growing"), vec!["grow"]);
    }
}

/// GET /api/books when the hub holds only some of the shelf: the shelf cut down to them.
pub async fn shelf(State(app): State<AppState>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match app.books.as_ref().and_then(|b| b.filtered.as_ref()) {
        Some(f) => Json(f.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
