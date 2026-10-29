//! Web search in Zen: Google, live suggestions, recent searches, and plain URLs.

use serde::{Deserialize, Serialize};

use crate::item::{Action, Icon, Item};
use crate::util::{self, Fuzzy};

const RECENT: &str = "searches.json";

#[derive(Default, Serialize, Deserialize)]
struct Recent(Vec<(String, f64)>);

fn recent() -> Recent {
    util::read_json(&util::state_dir().join(RECENT)).unwrap_or_default()
}

pub fn url_for(q: &str) -> String {
    let mut enc = String::new();
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => enc.push(b as char),
            b' ' => enc.push('+'),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://www.google.com/search?q={enc}")
}

/// "github.com/x", "https://...", "localhost:3000": open directly.
pub fn as_url(q: &str) -> Option<String> {
    let q = q.trim();
    if q.contains(char::is_whitespace) || q.is_empty() {
        return None;
    }
    if q.starts_with("http://") || q.starts_with("https://") || q.starts_with("file://") {
        return Some(q.to_string());
    }
    let host = q.split('/').next()?;
    let is_host = host.starts_with("localhost")
        || host.split(':').next().is_some_and(|h| {
            let parts: Vec<&str> = h.split('.').collect();
            parts.len() >= 2
                && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '-'))
                && parts.last().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) || parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())))
        });
    let local = host.starts_with("localhost") || host.starts_with(|c: char| c.is_ascii_digit());
    is_host.then(|| format!("{}://{q}", if local { "http" } else { "https" }))
}

pub fn query(q: &str, suggestions: &[String]) -> Vec<Item> {
    let q = q.trim();
    let mut out = Vec::new();
    let rec = recent();
    if q.is_empty() {
        for (s, t) in rec.0.iter().take(12) {
            out.push(Item::new(s, Icon::Glyph("\u{f02da}"), Action::Search(s.clone())).meta(util::age(*t)));
        }
        if !out.is_empty() {
            out[0].section = Some("recent".into());
        }
        return out;
    }
    if let Some(u) = as_url(q) {
        out.push(Item::new(format!("open {u}"), Icon::Glyph("\u{f0337}"), Action::Open(u)).meta("zen"));
    }
    out.push(Item::new(q, Icon::Glyph("\u{f0349}"), Action::Search(q.to_string())).sub("search google").meta("zen"));
    let mut fz = Fuzzy::new(q);
    for (s, t) in &rec.0 {
        if s != q && let Some((_, hl)) = fz.indices(s) {
            out.push(Item::new(s, Icon::Glyph("\u{f02da}"), Action::Search(s.clone())).meta(util::age(*t)).hl(hl));
            if out.len() >= 5 {
                break;
            }
        }
    }
    for s in suggestions {
        if s != q && !out.iter().any(|i| &i.title == s) {
            let hl = fz.indices(s).map(|x| x.1).unwrap_or_default();
            out.push(Item::new(s, Icon::Glyph("\u{f0349}"), Action::Search(s.clone())).hl(hl));
        }
    }
    out
}

pub fn search(q: &str) {
    let q = q.trim().to_string();
    if q.is_empty() {
        return;
    }
    let mut r = recent();
    r.0.retain(|(s, _)| *s != q);
    r.0.insert(0, (q.clone(), util::now()));
    r.0.truncate(200);
    util::write_json(&util::state_dir().join(RECENT), &r);
    open(&url_for(&q));
}

pub fn open(url: &str) {
    util::launch(&["zen-browser", url]);
}

/// Google's suggestions for a query (blocking; call from a worker thread).
pub fn suggest(q: &str) -> Vec<String> {
    let url = format!(
        "https://suggestqueries.google.com/complete/search?client=firefox&q={}",
        url_for(q).trim_start_matches("https://www.google.com/search?q=")
    );
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_millis(1500)))
        .build()
        .new_agent();
    let Ok(mut r) = agent.get(&url).call() else { return vec![] };
    let Ok(body) = r.body_mut().read_to_string() else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return vec![] };
    v[1].as_array()
        .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).take(8).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(as_url("github.com/x").as_deref(), Some("https://github.com/x"));
        assert_eq!(as_url("localhost:3000").as_deref(), Some("http://localhost:3000"));
        assert_eq!(as_url("https://a.b").as_deref(), Some("https://a.b"));
        assert_eq!(as_url("hello world"), None);
        assert_eq!(as_url("rust"), None);
        assert_eq!(as_url("v1.2"), None);
        assert_eq!(url_for("a b&c"), "https://www.google.com/search?q=a+b%26c");
    }
}

#[cfg(test)]
mod net {
    #[test]
    #[ignore]
    fn suggests() {
        let s = super::suggest("rust gtk4");
        assert!(!s.is_empty(), "{s:?}");
    }
}
