//! Books for people in a browser: the hub's shelf (`--books DIR`) or, on a phone sharing nearby, the
//! books on that phone (`installed`: the app's installed.json, read on each request so new downloads
//! show at once). They read in the app's own reader (app/assets/reader, epub.js) with `web=1`, which
//! keeps settings and places in the browser, or download the EPUB.
//!
//!   GET /books?q=               shelf: groups and categories, most read; or search. Phone: its books
//!   GET /books/c/{cat}?page=    one category, most read first
//!   GET /books/cover/{id}       cover image
//!   GET /read/?book={id}&title= the reader; its files under /read/
//!   GET /read/book/{id}.epub    the book: shelf `1342` (text) or `1342i` (illustrated); phone ids as in the app

use crate::page::{esc, plain, url_encode};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::path::PathBuf;

const PER_PAGE: usize = 60;

macro_rules! asset {
    ($f:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../app/assets/reader/", $f))
    };
}

/// A book on the sharing phone.
struct PhoneBook {
    id: String,
    title: String,
    author: String,
    epub: PathBuf,
    cover: Option<PathBuf>,
}

fn phone_books(installed: &std::path::Path) -> Vec<PhoneBook> {
    let root = installed.parent().unwrap_or(std::path::Path::new("."));
    let Ok(raw) = std::fs::read(installed) else { return Vec::new() };
    let Ok(j) = serde_json::from_slice::<serde_json::Value>(&raw) else { return Vec::new() };
    let safe = |p: &str| !p.is_empty() && !p.starts_with('/') && !p.split('/').any(|s| s == "..");
    let mut out: Vec<PhoneBook> = j
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, c)| c["group"] == "books")
        .filter_map(|(k, c)| {
            let id = k.strip_prefix("book:")?.to_string();
            if !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return None;
            }
            let epub = c["files"].as_array()?.iter().filter_map(|f| f["path"].as_str()).find(|p| p.ends_with(".epub") && safe(p))?;
            Some(PhoneBook {
                id,
                title: c["title"].as_str().unwrap_or("").to_string(),
                author: c["description"].as_str().unwrap_or("").to_string(),
                epub: root.join(epub),
                cover: c["cover"].as_str().filter(|p| safe(p)).map(|p| root.join(p)),
            })
        })
        .filter(|b| b.epub.is_file())
        .collect();
    out.sort_by_key(|b| b.title.to_lowercase());
    out
}

/// A book in a grid: the cover and title open the reader, the link under it downloads the EPUB. `read` is
/// the id the reader opens (`1342i` for an illustrated edition), `id` the cover's.
fn tile(id: &str, read: &str, title: &str, author: &str, cover: bool) -> String {
    let style = if cover { format!(" style=\"background-image:url('/books/cover/{id}')\"") } else { String::new() };
    format!(
        "<div class=\"btile\"><a href=\"/read/?web=1&amp;book={read}&amp;title={}\"><div class=\"bcover{}\"{style}><span>{}</span></div>\
<div class=\"t\">{}</div><div class=\"d\">{}</div></a><a class=\"dl\" href=\"/read/book/{read}.epub\" download=\"{}.epub\">Download</a></div>",
        esc(&url_encode(title)),
        if cover { " img" } else { "" },
        esc(title),
        esc(title),
        esc(author),
        esc(&title.replace(['/', '\\', ':', '"'], " ")),
    )
}

/// Shelf books open in their illustrated edition when there is one.
fn shelf_tile(b: &crate::books::Book) -> String {
    let id = b.id.to_string();
    tile(&id, &if b.ill { format!("{id}i") } else { id.clone() }, &b.title, &b.author, b.cover)
}

fn search_box(q: &str) -> String {
    format!(
        "<form class=\"search\" action=\"/books\"><div class=\"row\"><input type=\"search\" name=\"q\" value=\"{}\" placeholder=\"Title or author…\">\
<button>Search</button></div></form>",
        esc(q)
    )
}

const NOTE: &str = "<p class=\"meta\">Tap a book to read it here, or Download it for Books on iPhone or any ebook app.</p>";

#[derive(Deserialize)]
pub struct BooksQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    page: usize,
}

pub async fn index(State(app): State<AppState>, Query(p): Query<BooksQuery>) -> Response {
    let q = p.q.trim();
    if let Some(ix) = &app.books {
        let mut body = format!("<h1>Books</h1>{}{NOTE}", search_box(q));
        if !q.is_empty() {
            let hits = ix.search(q, PER_PAGE);
            body.push_str(&format!("<div class=\"section-title\">{} for “{}”</div>", if hits.is_empty() { "No books" } else { "Books" }, esc(q)));
            body.push_str("<div class=\"bgrid\">");
            for (id, _) in hits {
                if let Some(b) = ix.by_id.get(&id).map(|&i| &ix.books[i]) {
                    body.push_str(&shelf_tile(b));
                }
            }
            body.push_str("</div>");
        } else {
            let mut top: Vec<&crate::books::Book> = ix.books.iter().collect();
            top.sort_by_key(|b| b.rank);
            body.push_str("<div class=\"section-title\">Most read</div><div class=\"bgrid\">");
            for b in top.into_iter().take(18) {
                body.push_str(&shelf_tile(b));
            }
            body.push_str("</div>");
            for (g, cats) in &ix.groups {
                body.push_str(&format!("<div class=\"section-title\">{}</div><div class=\"chips\">", esc(g)));
                for &c in cats {
                    if let Some((name, list)) = ix.categories.get(c).filter(|(_, l)| !l.is_empty()) {
                        body.push_str(&format!(
                            "<a href=\"/books/c/{c}\">{} <span>{}</span></a>",
                            esc(name),
                            crate::page::fmt_n(list.len())
                        ));
                    }
                }
                body.push_str("</div>");
            }
        }
        return Html(plain("Books", &body)).into_response();
    }
    if let Some(inst) = &app.installed {
        let all = phone_books(inst);
        let ql = q.to_lowercase();
        let shown: Vec<&PhoneBook> =
            all.iter().filter(|b| ql.is_empty() || b.title.to_lowercase().contains(&ql) || b.author.to_lowercase().contains(&ql)).collect();
        let mut body = format!("<h1>Books</h1><p class=\"meta\">{} on this phone</p>", all.len());
        if all.len() > 12 {
            body.push_str(&search_box(q));
        }
        body.push_str(NOTE);
        if all.is_empty() {
            body.push_str("<p>No books on this phone yet.</p>");
        }
        body.push_str("<div class=\"bgrid\">");
        for b in shown {
            body.push_str(&tile(&b.id, &b.id, &b.title, &b.author, b.cover.as_ref().is_some_and(|c| c.is_file())));
        }
        body.push_str("</div>");
        return Html(plain("Books", &body)).into_response();
    }
    (StatusCode::NOT_FOUND, Html(plain("Books", "<p>No books here.</p>"))).into_response()
}

pub async fn category(State(app): State<AppState>, Path(c): Path<usize>, Query(p): Query<BooksQuery>) -> Response {
    let Some(ix) = &app.books else { return StatusCode::NOT_FOUND.into_response() };
    let Some((name, list)) = ix.categories.get(c) else { return StatusCode::NOT_FOUND.into_response() };
    let pages = list.len().div_ceil(PER_PAGE).max(1);
    let page = p.page.min(pages - 1);
    let mut body = format!(
        "<nav class=\"crumbs\"><a href=\"/books\">Books</a></nav><h1>{}</h1><p class=\"meta\">{} books, most read first</p>{NOTE}<div class=\"bgrid\">",
        esc(name),
        crate::page::fmt_n(list.len())
    );
    for &i in list.iter().skip(page * PER_PAGE).take(PER_PAGE) {
        let b = &ix.books[i as usize];
        body.push_str(&shelf_tile(b));
    }
    body.push_str("</div>");
    if pages > 1 {
        body.push_str("<div class=\"pages\">");
        for n in 0..pages {
            body.push_str(&format!("<a href=\"/books/c/{c}?page={n}\"{}>{}</a>", if n == page { " class=\"on\"" } else { "" }, n + 1));
        }
        body.push_str("</div>");
    }
    Html(plain(name, &body)).into_response()
}

fn file(path: PathBuf, mime: &'static str) -> Response {
    match std::fs::read(&path) {
        Ok(b) => ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "max-age=86400")], b).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn cover(State(app): State<AppState>, Path(id): Path<String>) -> Response {
    if let Some(ix) = &app.books {
        return match id.parse::<u64>() {
            Ok(n) => file(ix.dir.join("covers").join(format!("{n}.jpg")), "image/jpeg"),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if let Some(inst) = &app.installed
        && let Some(c) = phone_books(inst).into_iter().find(|b| b.id == id).and_then(|b| b.cover)
    {
        let mime = if c.extension().is_some_and(|e| e == "png") { "image/png" } else { "image/jpeg" };
        return file(c, mime);
    }
    StatusCode::NOT_FOUND.into_response()
}

pub async fn epub(State(app): State<AppState>, Path(name): Path<String>) -> Response {
    let Some(id) = name.strip_suffix(".epub") else { return StatusCode::NOT_FOUND.into_response() };
    if let Some(ix) = &app.books {
        let (n, ill) = match id.strip_suffix('i') {
            Some(n) => (n, true),
            None => (id, false),
        };
        let Ok(n) = n.parse::<u64>() else { return StatusCode::NOT_FOUND.into_response() };
        return file(ix.dir.join(if ill { "epub-images" } else { "epub" }).join(format!("{n}.epub")), "application/epub+zip");
    }
    if let Some(inst) = &app.installed
        && let Some(b) = phone_books(inst).into_iter().find(|b| b.id == id)
    {
        return file(b.epub, "application/epub+zip");
    }
    StatusCode::NOT_FOUND.into_response()
}

pub async fn reader_redirect() -> Redirect {
    Redirect::permanent("/read/")
}

pub async fn reader_file(Path(f): Path<String>) -> Response {
    let (body, mime): (&'static [u8], &str) = match f.as_str() {
        "" | "index.html" => (asset!("reader.html"), "text/html; charset=utf-8"),
        "epub.min.js" => (asset!("epub.min.js"), "text/javascript"),
        "jszip.min.js" => (asset!("jszip.min.js"), "text/javascript"),
        "LICENSES.txt" => (asset!("LICENSES.txt"), "text/plain; charset=utf-8"),
        // the reader asks for Inter as a .ttf next to it; browsers take the WOFF2 we have
        "Inter-Regular.ttf" => (crate::INTER_REGULAR, "font/woff2"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "max-age=86400")], body).into_response()
}

pub async fn reader_index() -> Response {
    reader_file(Path(String::new())).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_books_from_installed_json() {
        let d = std::env::temp_dir().join(format!("sp-reading-{}", std::process::id()));
        std::fs::create_dir_all(d.join("books/epub")).unwrap();
        std::fs::create_dir_all(d.join("books/imported")).unwrap();
        std::fs::write(d.join("books/epub/1342.epub"), b"x").unwrap();
        std::fs::write(d.join("books/imported/ab12.epub"), b"x").unwrap();
        std::fs::write(d.join("secret.epub"), b"x").unwrap();
        let installed = serde_json::json!({
            "book:1342": {"id": "book:1342", "title": "Pride and Prejudice", "group": "books", "description": "Jane Austen",
                          "cover": "books/covers/1342.jpg", "files": [{"path": "books/epub/1342.epub"}, {"path": "books/covers/1342.jpg"}]},
            "book:imp-ab12": {"title": "紅樓夢", "group": "books", "description": "曹雪芹", "files": [{"path": "books/imported/ab12.epub"}]},
            "book:evil": {"title": "x", "group": "books", "files": [{"path": "books/../secret.epub"}]},
            "book:gone": {"title": "x", "group": "books", "files": [{"path": "books/epub/9.epub"}]},
            "col:guides": {"title": "Guides", "group": "col:guides", "files": [{"path": "collections/guides/x.epub"}]},
        });
        std::fs::write(d.join("installed.json"), installed.to_string()).unwrap();
        let b = phone_books(&d.join("installed.json"));
        let mut ids: Vec<&str> = b.iter().map(|b| b.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["1342", "imp-ab12"]);
        assert_eq!(b.len(), 2, "path escapes, missing files and other groups are left out");
        assert!(b.iter().any(|x| x.id == "1342" && x.author == "Jane Austen"));
        std::fs::remove_dir_all(d).ok();
    }
}
