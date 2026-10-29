//! Emoji: Unicode's emoji list with CLDR keywords and GitHub shortcodes
//! (data/emoji.tsv: emoji, name, keywords, group). Skin tone variants are left
//! out; recent picks come first.

use crate::item::{Action, Icon, Item};
use crate::util::{Fuzzy, Usage};

const DATA: &str = include_str!("../data/emoji.tsv");
const USAGE: &str = "emoji.json";
const RECENT: usize = 24;

pub struct Emoji {
    pub emoji: &'static str,
    pub name: &'static str,
    pub keywords: &'static str,
    pub group: &'static str,
}

pub fn all() -> Vec<Emoji> {
    DATA.lines()
        .filter_map(|l| {
            let mut p = l.split('\t');
            Some(Emoji {
                emoji: p.next()?,
                name: p.next()?,
                keywords: p.next().unwrap_or(""),
                group: p.next().unwrap_or(""),
            })
        })
        .collect()
}

fn item(e: &Emoji, hl: Vec<u32>) -> Item {
    Item::new(e.name, Icon::Text(e.emoji.to_string()), Action::Emoji(e.emoji.to_string()))
        .sub(e.keywords)
        .meta(e.group)
        .hl(hl)
}

pub fn query(all: &[Emoji], q: &str) -> Vec<Item> {
    let usage = Usage::load(USAGE);
    let q = q.trim().trim_matches(':');
    if q.is_empty() {
        let mut recent: Vec<&Emoji> = all.iter().filter(|e| usage.0.contains_key(e.emoji)).collect();
        recent.sort_by(|a, b| usage.last(b.emoji).total_cmp(&usage.last(a.emoji)));
        recent.truncate(RECENT);
        let mut out: Vec<Item> = recent.iter().map(|e| item(e, vec![]).class("recent")).collect();
        out.extend(all.iter().map(|e| item(e, vec![])));
        return out;
    }
    let mut fz = Fuzzy::new(q);
    let ql = q.to_lowercase();
    let mut v: Vec<(u32, Item)> = all
        .iter()
        .filter_map(|e| {
            let name = fz.indices(e.name);
            let kw = e.keywords.split(" | ").filter_map(|k| {
                let k = k.trim_matches(':');
                if k == ql {
                    Some(200)
                } else if k.starts_with(&ql) {
                    Some(120)
                } else {
                    None
                }
            }).max();
            let fuzzy_kw = if kw.is_none() { fz.score(e.keywords).map(|s| s / 2) } else { None };
            let n = name.as_ref().map(|n| n.0 + if e.name.starts_with(&ql) { 80 } else if e.name.split(' ').any(|w| w.starts_with(&ql)) { 50 } else { 0 });
            let best = [n, kw, fuzzy_kw].into_iter().flatten().max()?;
            Some((best + usage.bonus(e.emoji), item(e, name.map(|n| n.1).unwrap_or_default())))
        })
        .collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.0));
    v.into_iter().map(|x| x.1).collect()
}

pub fn picked(e: &str) {
    Usage::bump(USAGE, e);
}
