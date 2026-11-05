//! /browse/{c}: a collection's sections (from `<pack>/browse.json`, written by `sp browse`), their
//! subsections and articles. Long lists get an A–Z bar and pages. Collections without browse.json
//! list every article A–Z.

use crate::page::{esc, shell};
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::response::Html;
use serde::Deserialize;
use std::collections::BTreeMap;

const LETTERS_FROM: usize = 300; // more articles than this in one list: A–Z bar
const PAGE: usize = 400;

pub struct Browse {
    pub pack_order: bool,
    pub sections: Vec<(Vec<String>, Vec<u32>)>,
}

impl Browse {
    pub fn load(pack: &sp_pack::reader::Pack) -> Browse {
        let p = pack.dir.join("browse.json");
        if let Some(v) = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()) {
            let sections = v["sections"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| {
                    let path = s["path"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
                    let docs = s["docs"].as_array().into_iter().flatten().filter_map(|x| x.as_u64().map(|n| n as u32)).collect();
                    (path, docs)
                })
                .collect();
            return Browse { pack_order: v["order"] == "pack", sections };
        }
        // no sections: everything A–Z at the top
        let mut docs: Vec<u32> = (0..pack.doc_count() as u32).collect();
        docs.sort_by_cached_key(|&d| pack.title(d).to_lowercase());
        Browse { pack_order: false, sections: vec![(Vec::new(), docs)] }
    }
}

#[derive(Deserialize)]
pub struct Params {
    /// section path, names joined with " › "
    p: Option<String>,
    /// first letter filter
    l: Option<String>,
    page: Option<usize>,
}

fn letter(t: &str) -> char {
    t.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_ascii_uppercase()).filter(|c| c.is_ascii_uppercase()).unwrap_or('#')
}

fn href(c: &str, path: &[String], l: Option<char>, page: usize) -> String {
    let mut h = format!("/browse/{}", c);
    let mut q = Vec::new();
    if !path.is_empty() {
        q.push(format!("p={}", crate::page::url_encode(&path.join(" › "))));
    }
    if let Some(l) = l {
        q.push(format!("l={}", crate::page::url_encode(&l.to_string())));
    }
    if page > 0 {
        q.push(format!("page={page}"));
    }
    if !q.is_empty() {
        h.push('?');
        h.push_str(&q.join("&"));
    }
    h
}

pub async fn page(State(app): State<AppState>, Path(c): Path<String>, Query(q): Query<Params>) -> Html<String> {
    let Some(ci) = app.lib.find(&c) else { return Html(crate::page::not_found(&c)) };
    let col = &app.lib.cols[ci];
    let pack = &col.searcher.pack;
    let b = &app.browse[ci];
    let path: Vec<String> = q.p.as_deref().filter(|s| !s.is_empty()).map(|s| s.split(" › ").map(String::from).collect()).unwrap_or_default();

    // children of this path with their article counts, and the articles right here
    let mut kids: BTreeMap<&str, usize> = BTreeMap::new();
    let mut here: Vec<u32> = Vec::new();
    for (p, docs) in &b.sections {
        if p.len() < path.len() || p[..path.len()] != path[..] {
            continue;
        }
        if p.len() == path.len() {
            here.extend(docs);
        } else {
            *kids.entry(p[path.len()].as_str()).or_default() += docs.len();
        }
    }
    if !b.pack_order {
        here.sort_by_cached_key(|&d| pack.title(d).to_lowercase());
    }

    let mut body = String::new();
    // breadcrumb
    body.push_str("<nav class=\"crumbs\">");
    body.push_str(&format!("<a href=\"{}\">{}</a>", href(&c, &[], None, 0), esc(&col.title)));
    for i in 0..path.len() {
        body.push_str(&format!(" › <a href=\"{}\">{}</a>", href(&c, &path[..=i], None, 0), esc(&path[i])));
    }
    body.push_str("</nav>");
    body.push_str(&format!("<h1>{}</h1>", esc(path.last().map(String::as_str).unwrap_or(&col.title))));
    if path.is_empty() && !col.description.is_empty() {
        body.push_str(&format!("<p class=\"lede\">{}</p>", esc(&col.description)));
    }

    if !kids.is_empty() {
        body.push_str("<div class=\"cards sections\">");
        for (name, n) in &kids {
            let mut p = path.clone();
            p.push(name.to_string());
            body.push_str(&format!(
                "<a class=\"card\" href=\"{}\"><div class=\"t\">{}</div><div class=\"d\">{} articles</div></a>",
                href(&c, &p, None, 0),
                esc(name),
                crate::page::fmt_n(*n)
            ));
        }
        body.push_str("</div>");
    }

    if !here.is_empty() {
        if !kids.is_empty() {
            body.push_str("<div class=\"section-title\">Articles</div>");
        }
        // A–Z bar for long lists (not for read-in-order guides)
        let mut list = here.clone();
        if list.len() > LETTERS_FROM && !b.pack_order {
            let mut letters: BTreeMap<char, usize> = BTreeMap::new();
            for &d in &list {
                *letters.entry(letter(pack.title(d))).or_default() += 1;
            }
            let cur = q.l.as_deref().and_then(|s| s.chars().next()).filter(|l| letters.contains_key(l))
                .unwrap_or_else(|| *letters.keys().next().unwrap());
            body.push_str("<div class=\"az\">");
            for l in letters.keys() {
                body.push_str(&format!(
                    "<a{} href=\"{}\">{}</a>",
                    if *l == cur { " class=\"on\"" } else { "" },
                    href(&c, &path, Some(*l), 0),
                    l
                ));
            }
            body.push_str("</div>");
            list.retain(|&d| letter(pack.title(d)) == cur);
            let pages = list.len().div_ceil(PAGE);
            let pg = q.page.unwrap_or(0).min(pages.saturating_sub(1));
            let shown = &list[pg * PAGE..((pg + 1) * PAGE).min(list.len())];
            body.push_str(&list_html(&c, pack, shown));
            if pages > 1 {
                body.push_str("<div class=\"pages\">");
                for i in 0..pages {
                    body.push_str(&format!(
                        "<a{} href=\"{}\">{}</a>",
                        if i == pg { " class=\"on\"" } else { "" },
                        href(&c, &path, Some(cur), i),
                        i + 1
                    ));
                }
                body.push_str("</div>");
            }
        } else {
            body.push_str(&list_html(&c, pack, &list));
        }
    }
    let title = format!("{} – Bothy", path.last().map(String::as_str).unwrap_or(&col.title));
    Html(shell(&title, &crate::page::header(&app, "", "hybrid", &c), &body, "en"))
}

fn list_html(c: &str, pack: &sp_pack::reader::Pack, docs: &[u32]) -> String {
    let mut s = String::from("<ul class=\"browse\">");
    for &d in docs {
        s.push_str(&format!("<li><a href=\"/doc/{}/{}\">{}</a></li>", c, d, esc(pack.title(d))));
    }
    s.push_str("</ul>");
    s
}
