//! A sharing phone offers what it has to other phones running the app (DESIGN §19): the app's
//! Content and Books tabs download from it exactly as from a hub. Everything comes from the app's
//! `installed.json`, whose BLAKE3 hashes were checked when the files arrived, so nothing is hashed
//! again here, and the phone receiving them checks them once more. Only files listed there are
//! served. The person's own imported EPUBs are not offered.
//!
//!   GET /api/catalog       the installed components (no books), in the hub catalog's format
//!   GET /api/books         the installed Project Gutenberg books, in the hub shelf's format
//!   GET /download/{*path}  one installed file (byte ranges, so downloads resume)
//!   GET /app.apk           the app itself, to install on a phone that doesn't have it yet

use crate::AppState;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::path::PathBuf;
use tower::ServiceExt;

/// The content root and the installed components, read fresh (the phone may have added some since).
fn installed(app: &crate::App) -> Option<(PathBuf, Map<String, Value>)> {
    let p = app.installed.as_ref()?;
    let root = p.parent()?.to_path_buf();
    let j: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    Some((root, j.as_object()?.clone()))
}

fn safe(p: &str) -> bool {
    !p.is_empty() && !p.starts_with('/') && !p.contains('\\') && !p.split('/').any(|s| s.is_empty() || s == "." || s == "..")
}

/// Every file of a component is in place with its recorded size.
fn complete(root: &std::path::Path, c: &Value) -> bool {
    let Some(files) = c["files"].as_array() else { return false };
    files.iter().all(|f| {
        let (Some(path), Some(size)) = (f["path"].as_str(), f["size"].as_u64()) else { return false };
        safe(path) && std::fs::metadata(root.join(path)).is_ok_and(|m| m.is_file() && m.len() == size)
    })
}

/// A Project Gutenberg book (numeric id); imported books have other ids and stay private.
fn gutenberg(id: &str, c: &Value) -> Option<u64> {
    if c["group"] != "books" {
        return None;
    }
    id.strip_prefix("book:")?.parse().ok()
}

pub async fn catalog(State(app): State<AppState>) -> Response {
    let Some((root, comps)) = installed(&app) else { return StatusCode::NOT_FOUND.into_response() };
    let list: Vec<&Value> = comps
        .iter()
        .filter(|(id, c)| !id.starts_with("book:") && c["group"] != "books" && complete(&root, c))
        .map(|(_, c)| c)
        .collect();
    let have: HashSet<&str> = list.iter().filter_map(|c| c["id"].as_str()).collect();
    let groups: Vec<Value> = app.lib.cols.iter().filter(|c| have.contains(format!("col:{}", c.id).as_str())).map(crate::catalog::group).collect();
    let books = comps.iter().any(|(id, c)| gutenberg(id, c).is_some());
    let mut out = json!({
        "format": "survivalpedia-catalog", "version": 2, "source": "phone",
        "groups": groups, "components": list,
    });
    if books {
        out["books"] = json!({"index": "/api/books", "download": "/download/books"});
    }
    axum::Json(out).into_response()
}

pub async fn books(State(app): State<AppState>) -> Response {
    let Some((root, comps)) = installed(&app) else { return StatusCode::NOT_FOUND.into_response() };
    let mut books = Vec::new();
    for (id, c) in &comps {
        let Some(n) = gutenberg(id, c) else { continue };
        let Some(epub) = c["files"].as_array().into_iter().flatten().filter_map(|f| f["path"].as_str()).find(|p| p.ends_with(".epub") && safe(p))
        else {
            continue;
        };
        let Ok(m) = std::fs::metadata(root.join(epub)) else { continue };
        let cover = std::fs::metadata(root.join(format!("books/covers/{n}.jpg"))).ok().map(|m| m.len());
        // one edition is on the phone: offered under both names (see `download`)
        let ill = epub.contains("epub-images/");
        books.push((
            c["title"].as_str().unwrap_or("").to_lowercase(),
            json!({"id": n, "t": c["title"], "a": c["description"], "c": [], "s": m.len(), "si": if ill { Some(m.len()) } else { None }, "cs": cover}),
        ));
    }
    books.sort_by(|a, b| a.0.cmp(&b.0));
    let books: Vec<Value> = books.into_iter().enumerate().map(|(i, (_, mut b))| { b["r"] = json!(i); b }).collect();
    if books.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(json!({"categories": [], "groups": [], "books": books})).into_response()
}

pub async fn download(State(app): State<AppState>, Path(path): Path<String>, req: Request) -> Response {
    let Some((root, comps)) = installed(&app) else { return StatusCode::NOT_FOUND.into_response() };
    let offered = |p: &str| {
        comps.iter().filter(|(id, c)| !id.starts_with("book:") || gutenberg(id, c).is_some()).any(|(_, c)| {
            c["files"].as_array().is_some_and(|fs| fs.iter().any(|f| f["path"] == p))
        })
    };
    let mut path = path;
    if !offered(&path) {
        // a book is asked for as the edition the asker chose; the phone has one of the two
        let other = path
            .strip_prefix("books/epub/")
            .map(|n| format!("books/epub-images/{n}"))
            .or_else(|| path.strip_prefix("books/epub-images/").map(|n| format!("books/epub/{n}")));
        match other.filter(|o| offered(o)) {
            Some(o) => path = o,
            None => return StatusCode::NOT_FOUND.into_response(),
        }
    }
    if !safe(&path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    tower_http::services::ServeFile::new(root.join(&path)).oneshot(req).await.into_response()
}

pub async fn apk(State(app): State<AppState>, req: Request) -> Response {
    let Some(p) = app.apk.clone().filter(|p| p.is_file()) else { return StatusCode::NOT_FOUND.into_response() };
    let mut r = tower_http::services::ServeFile::new_with_mime(p, &"application/vnd.android.package-archive".parse().unwrap())
        .oneshot(req)
        .await
        .into_response();
    r.headers_mut().insert(header::CONTENT_DISPOSITION, "attachment; filename=\"survivalpedia.apk\"".parse().unwrap());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_clean_relative_paths() {
        assert!(safe("maps/uk.pmtiles"));
        assert!(!safe("../etc/passwd"));
        assert!(!safe("/etc/passwd"));
        assert!(!safe("maps//x"));
        assert!(!safe("maps/./x"));
        assert!(!safe(""));
    }

    #[test]
    fn only_gutenberg_books() {
        let b = json!({"group": "books"});
        assert_eq!(gutenberg("book:1342", &b), Some(1342));
        assert_eq!(gutenberg("book:u-ab12", &b), None);
        assert_eq!(gutenberg("book:1342", &json!({"group": "maps"})), None);
    }
}
