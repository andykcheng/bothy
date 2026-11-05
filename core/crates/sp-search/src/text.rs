//! Text helpers over stored doc HTML.

/// Plain text of blocks p{first}..=p{last} from stored HTML, cut to `max` chars.
pub fn snippet(html: &str, first: u32, last: u32, max: usize) -> String {
    let start = html.find(&format!(" id=\"p{first}\"")).and_then(|i| html[..i].rfind('<')).unwrap_or(0);
    let end = html
        .find(&format!(" id=\"p{}\"", last + 1))
        .and_then(|i| html[..i].rfind('<'))
        .unwrap_or(html.len())
        .max(start);
    let text = strip_tags(&html[start..end]);
    let mut s: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        s.push('…');
    }
    s
}

pub fn strip_tags(h: &str) -> String {
    let mut out = String::with_capacity(h.len());
    let mut in_tag = false;
    for c in h.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
