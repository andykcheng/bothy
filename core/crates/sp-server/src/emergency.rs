//! First-aid cards (DESIGN §19): a handful of hand-written cards (CPR, choking, bleeding, poisoning…) shared
//! with the phone app, which shows them natively even with nothing downloaded
//! (`app/assets/emergency/cards.json`). The server shows them at `/emergency`, and a search or Ask question
//! that sounds like an emergency gets the matching card above its results ("my child drank bleach" →
//! Poisoning), because a search engine ranks encyclopaedia articles, not what to do right now.

use crate::{AppState, page};
use axum::extract::{Query, State};
use axum::response::Html;
use serde::Deserialize;
use std::sync::OnceLock;

const JSON: &str = include_str!("../../../../app/assets/emergency/cards.json");

#[derive(Deserialize)]
pub struct Cards {
    pub disclaimer: String,
    pub call: String,
    pub reviewed: bool,
    pub cards: Vec<Card>,
}

#[derive(Deserialize)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub when: String,
    pub steps: Vec<String>,
    #[serde(default)]
    pub children: Vec<String>,
    #[serde(default)]
    pub dont: Vec<String>,
    pub call_if: String,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    weak: Vec<String>,
    #[serde(default)]
    with: Vec<String>,
    #[serde(default)]
    not: Vec<String>,
    #[serde(default)]
    pub more: Vec<String>,
}

pub fn cards() -> &'static Cards {
    static C: OnceLock<Cards> = OnceLock::new();
    C.get_or_init(|| serde_json::from_str(JSON).expect("app/assets/emergency/cards.json"))
}

/// Lower case, apostrophes dropped ("won't" → "wont"), anything else that isn't a letter or digit a space;
/// padded with spaces so `contains(" phrase ")` matches whole words.
fn norm(s: &str) -> String {
    let mut out = String::from(" ");
    for ch in s.chars().flat_map(char::to_lowercase) {
        match ch {
            '\'' | '’' | '‘' => {}
            c if c.is_alphanumeric() => out.push(c),
            _ if out.ends_with(' ') => {}
            _ => out.push(' '),
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

fn has(q: &str, phrase: &str) -> bool {
    q.contains(&format!(" {phrase} "))
}

/// How strongly a query asks for a card: 0 none; a keyword scores 2 plus its length in words (a longer
/// phrase is more specific); a weak word with a different `with` word scores 1.
fn score(card: &Card, q: &str) -> usize {
    if card.not.iter().any(|w| has(q, w)) {
        return 0;
    }
    let strong = card.keywords.iter().filter(|k| has(q, k)).map(|k| 2 + k.split(' ').count()).max();
    if let Some(s) = strong {
        return s;
    }
    let weak: Vec<&String> = card.weak.iter().filter(|w| has(q, w)).collect();
    let with = card.with.iter().any(|w| has(q, w) && weak.iter().any(|v| *v != w));
    usize::from(!weak.is_empty() && with)
}

/// The cards a query asks for, best first: at most two, and the second only if it also matched a keyword.
pub fn matches(query: &str) -> Vec<&'static Card> {
    let q = norm(query);
    let mut scored: Vec<(usize, usize, &Card)> =
        cards().cards.iter().enumerate().map(|(i, c)| (score(c, &q), i, c)).filter(|s| s.0 > 0).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<&Card> = scored.iter().take(1).map(|s| s.2).collect();
    if let Some(s) = scored.get(1).filter(|s| s.0 >= 3) {
        out.push(s.2);
    }
    out
}

/// The red box above search results and Ask answers.
pub fn banner(query: &str) -> String {
    let m = matches(query);
    if m.is_empty() {
        return String::new();
    }
    let mut s = String::from("<div class=\"fa-banner\"><div class=\"fa-k\">First aid</div>");
    for c in m {
        s.push_str(&format!(
            "<a href=\"/emergency#{}\"><b>{}</b> · {}</a>",
            c.id,
            page::esc(&c.title),
            page::esc(c.steps.first().map(String::as_str).unwrap_or(""))
        ));
    }
    s.push_str(&format!("<div class=\"fa-call\">{}</div></div>", page::esc(&cards().call)));
    s
}

/// `{"id", "title"}` of the best card, for the Ask stream.
pub fn best(query: &str) -> Option<serde_json::Value> {
    matches(query).first().map(|c| serde_json::json!({"type": "card", "id": c.id, "title": c.title, "first": c.steps[0]}))
}

pub const CSS: &str = ".fa-banner{border:2px solid #C62828;border-radius:var(--radius);padding:10px 14px;margin:10px 0 14px;\
background:color-mix(in srgb,#C62828 7%,var(--surface))} .fa-banner a{display:block;color:var(--fg);margin:4px 0}\
.fa-banner b{color:#C62828} .fa-k{font-size:12px;font-weight:700;letter-spacing:.08em;text-transform:uppercase;color:#C62828}\
.fa-call{font-size:14px;color:var(--muted);margin-top:4px}\
.fa-list{display:grid;grid-template-columns:repeat(auto-fill,minmax(220px,1fr));gap:8px;margin:12px 0 20px}\
.fa-list a{background:var(--surface);border:1px solid var(--border);border-left:4px solid #C62828;border-radius:10px;\
padding:10px 12px;color:var(--fg);font-weight:600} .fa-card{background:var(--surface);border:1px solid var(--border);\
border-radius:var(--radius);padding:4px 18px 14px;margin:18px 0;scroll-margin-top:80px} .fa-card h2{color:#C62828}\
.fa-card .when{color:var(--muted)} .fa-card ol li{margin-bottom:8px;font-size:18px} .fa-card h3{font-size:16px;margin:14px 0 4px}\
.fa-tel{display:flex;gap:8px;flex-wrap:wrap} .fa-tel a{background:#C62828;color:#fff;font-weight:600;padding:8px 14px;border-radius:999px}\
.fa-card .callif{border-left:3px solid #C62828;padding-left:10px;font-weight:600}";

#[derive(Deserialize)]
pub struct Params {
    #[serde(default)]
    q: String,
}

/// GET /emergency: every card, an index at the top; `?q=` jumps to the matching card.
pub async fn page(State(app): State<AppState>, Query(p): Query<Params>) -> Html<String> {
    let all = cards();
    let mut b = String::from("<h1>First aid</h1>");
    b.push_str(&format!("<p class=\"fa-call\" style=\"font-size:17px;color:var(--fg)\"><b>{}</b></p>", page::esc(&all.call)));
    b.push_str("<p class=\"fa-tel\">");
    for n in ["112", "999", "911", "000"] {
        b.push_str(&format!("<a href=\"tel:{n}\">Call {n}</a>"));
    }
    b.push_str("</p><div class=\"fa-list\">");
    for c in &all.cards {
        b.push_str(&format!("<a href=\"#{}\">{}</a>", c.id, page::esc(&c.title)));
    }
    b.push_str("</div>");
    let list = |b: &mut String, head: &str, items: &[String], tag: &str| {
        if !items.is_empty() {
            b.push_str(&format!("<h3>{head}</h3><{tag}>"));
            for i in items {
                b.push_str(&format!("<li>{}</li>", page::esc(i)));
            }
            b.push_str(&format!("</{tag}>"));
        }
    };
    for c in &all.cards {
        b.push_str(&format!("<section class=\"fa-card\" id=\"{}\"><h2>{}</h2><p class=\"when\">{}</p>", c.id, page::esc(&c.title), page::esc(&c.when)));
        list(&mut b, "What to do", &c.steps, "ol");
        list(&mut b, "Children and babies", &c.children, "ul");
        list(&mut b, "Don't", &c.dont, "ul");
        b.push_str(&format!("<p class=\"callif\">Call the emergency number: {}</p>", page::esc(&c.call_if)));
        let more: Vec<String> = c
            .more
            .iter()
            .filter_map(|t| app.lib.lookup_title(t, None).map(|(ci, n)| format!("<a href=\"/doc/{}/{n}\">{}</a>", app.lib.cols[ci].id, page::esc(t))))
            .collect();
        if !more.is_empty() {
            b.push_str(&format!("<p>More in the library: {}</p>", more.join(" · ")));
        }
        b.push_str("</section>");
    }
    b.push_str(&format!("<p class=\"warn\">{}</p>", page::esc(&all.disclaimer)));
    if !p.q.is_empty() {
        if let Some(c) = matches(&p.q).first() {
            b.push_str(&format!("<script>location.hash='{}'</script>", c.id));
        }
    }
    b.push_str(&format!("<style>{CSS}</style>"));
    Html(page::plain("First aid – Bothy", &b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(q: &str) -> Option<&'static str> {
        matches(q).first().map(|c| c.id.as_str())
    }

    #[test]
    fn cards_parse() {
        let c = cards();
        assert!(c.cards.len() >= 15);
        for card in &c.cards {
            assert!(!card.steps.is_empty() && !card.keywords.is_empty(), "{}", card.id);
        }
    }

    #[test]
    fn emergencies_find_their_card() {
        for (q, want) in [
            ("my child drank bleach", "poisoning"),
            ("toddler swallowed a button battery", "poisoning"),
            ("what to do if someone took too many pills", "poisoning"),
            ("baby choking on food", "choking"),
            ("something stuck in his throat", "choking"),
            ("how to do CPR", "cpr"),
            ("my dad collapsed and is not breathing", "cpr"),
            ("cut my finger badly, bleeding won't stop", "bleeding"),
            ("how to use a tourniquet", "bleeding"),
            ("burned my hand on the kettle", "burns"),
            ("toddler scalded by boiling water", "burns"),
            ("face drooping and slurred speech", "stroke"),
            ("chest pain spreading to left arm", "heart-attack"),
            ("friend passed out and won't wake up", "recovery"),
            ("signs of hypothermia", "hypothermia"),
            ("heat stroke symptoms", "heat"),
            ("child having a seizure", "seizure"),
            ("bee sting allergic reaction swollen lips", "anaphylaxis"),
            ("asthma attack without inhaler", "asthma"),
            ("diabetic shaky and confused", "low-sugar"),
        ] {
            assert_eq!(top(q), Some(want), "{q}");
        }
    }

    #[test]
    fn everyday_searches_stay_clear() {
        for q in [
            "how to bleed brakes",
            "bleeding a radiator",
            "two stroke engine oil mix",
            "bleach for water purification",
            "how to make fire",
            "burn wood in a stove",
            "blood pressure",
            "blood sugar levels",
            "pipe fitting sizes",
            "food poisoning",
            "poison ivy rash",
            "what is a chemical reaction",
            "car engine overheating",
            "freezing vegetables",
            "the unconscious mind",
            "hand cut dovetails",
            "choke cable adjustment",
            "breast stroke technique",
        ] {
            assert_eq!(top(q), None, "{q}");
        }
    }

    #[test]
    fn apostrophes_and_case() {
        assert_eq!(norm("Won't  STOP—bleeding!"), " wont stop bleeding ");
    }
}

/// Prints the card each question gets, to review by eye:
/// `SP_QUERIES=../eval/essentials-questions.jsonl cargo test -p sp-server card_audit -- --ignored --nocapture`
#[cfg(test)]
#[test]
#[ignore]
fn card_audit() {
    let path = std::env::var("SP_QUERIES").expect("SP_QUERIES=file.jsonl");
    let (mut n, mut hit) = (0, 0);
    for l in std::fs::read_to_string(path).unwrap().lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let q = v["q"].as_str().unwrap_or("");
        n += 1;
        let m: Vec<&str> = matches(q).iter().map(|c| c.id.as_str()).collect();
        if !m.is_empty() {
            hit += 1;
            println!("{:<28} {q}", m.join(","));
        }
    }
    println!("{hit} of {n} questions get a card");
}
