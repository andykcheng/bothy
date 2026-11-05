//! Games (optional, `--games DIR`): small self-contained HTML games, one directory each with an
//! `index.html` and a `game.json` (id, title, description, players, version, license, author).
//! They need no network: saves and scores live in the browser's localStorage.
//!
//!   GET /games              list of games
//!   GET /game/{id}/…        a game's files
//!
//! The app downloads games through the catalog (`game:<id>` components) and serves them itself,
//! so they play even with no library installed.

use crate::AppState;
use anyhow::Result;
use axum::extract::State;
use axum::response::{Html, IntoResponse, Response};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Game {
    pub id: String,
    pub dir: PathBuf,
    pub meta: Value,
}

pub struct Games {
    pub dir: PathBuf,
    pub list: Vec<Game>,
}

impl Games {
    pub fn open(dir: &Path) -> Result<Games> {
        let mut list = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let d = e?.path();
            let Ok(raw) = std::fs::read(d.join("game.json")) else { continue };
            if !d.join("index.html").is_file() {
                continue;
            }
            let meta: Value = serde_json::from_slice(&raw)?;
            let id = d.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            list.push(Game { id, dir: d, meta });
        }
        list.sort_by(|a, b| a.meta["title"].as_str().cmp(&b.meta["title"].as_str()));
        Ok(Games { dir: dir.to_path_buf(), list })
    }
}

pub async fn page(State(app): State<AppState>) -> Response {
    let Some(games) = &app.games else {
        return Html("<p>No games on this server. Start sp-server with --games DIR.</p>".to_string()).into_response();
    };
    let s = |g: &Game, k: &str| crate::page::esc(g.meta[k].as_str().unwrap_or(""));
    let items: String = games
        .list
        .iter()
        .map(|g| {
            let cover = match g.meta["cover"].as_str() {
                Some(c) => format!(" style=\"background-image:url('/game/{}/{}')\"", g.id, crate::page::esc(c)),
                None => String::new(),
            };
            format!(
                "<a class=\"tile\" href=\"/game/{}/\"><div class=\"cover\"{cover}></div><div class=\"body\"><div class=\"t\">{}</div>\
<div class=\"d\">{} player{}</div></div></a>",
                g.id,
                s(g, "title"),
                s(g, "players"),
                if g.meta["players"].as_str() == Some("1") { "" } else { "s" },
            )
        })
        .collect();
    Html(crate::page::plain(
        "Games",
        &format!("<h1>Games</h1><p class=\"meta\">All offline. Scores and saved games stay in this browser.</p><div class=\"grid3\">{items}</div>"),
    ))
        .into_response()
}
