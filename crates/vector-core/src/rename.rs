//! Batch rename patterns for `:rename`. Two forms:
//!
//! - a template: `photo-{n}` → `photo-1.jpg`, `photo-2.jpg`, …; tokens `{n}` (counter from 1),
//!   `{n:3}` (zero-padded to 3), `{name}` (the old name without extension), `{ext}`, and
//!   `{date}` (modified, YYYY-MM-DD). The old extension is kept unless the template has a dot
//!   or `{ext}` in it.
//! - a replacement: `s/old/new/` replaces every `old` in the names with `new` (plain text).
//!
//! Pure: the window turns the new names into an `:edit` plan, which checks and applies them.

/// One item to rename: its name, whether it is a folder, and its modified time (Unix).
pub struct Item<'a> {
    pub name: &'a str,
    pub dir: bool,
    pub modified: i64,
}

/// The new names, in the same order. `Err` explains a bad pattern.
pub fn apply(pattern: &str, items: &[Item]) -> Result<Vec<String>, String> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("give a pattern, like photo-{n} or s/old/new/".into());
    }
    if let Some(rest) = pattern.strip_prefix("s/") {
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (from, to) = rest.split_once('/').ok_or("s/old/new/ needs both parts")?;
        if from.is_empty() {
            return Err("s/old/new/: old can't be empty".into());
        }
        return check(items.iter().map(|i| i.name.replace(from, to)).collect());
    }
    let keep_ext = !pattern.contains('.') && !pattern.contains("{ext}");
    let width = items.len().to_string().len();
    let out: Vec<String> = items
        .iter()
        .enumerate()
        .map(|(k, item)| {
            let (stem, ext) = split_ext(item.name, item.dir);
            let mut out = String::new();
            let mut rest = pattern;
            while let Some(open) = rest.find('{') {
                out.push_str(&rest[..open]);
                let close = rest[open..]
                    .find('}')
                    .map(|c| open + c)
                    .ok_or("unclosed { in the pattern")?;
                let token = &rest[open + 1..close];
                out.push_str(&match token {
                    "n" => (k + 1).to_string(),
                    "name" => stem.to_string(),
                    "ext" => ext.to_string(),
                    "date" => date(item.modified),
                    t if t.starts_with("n:") => {
                        let w: usize = t[2..]
                            .parse()
                            .map_err(|_| format!("{{{t}}}: the width must be a number"))?;
                        format!("{:0w$}", k + 1, w = w.clamp(1, 20))
                    }
                    "N" => format!("{:0width$}", k + 1),
                    t => {
                        return Err(format!(
                            "unknown token {{{t}}}: use n, n:3, name, ext, date"
                        ));
                    }
                });
                rest = &rest[close + 1..];
            }
            out.push_str(rest);
            if keep_ext && !ext.is_empty() {
                out.push('.');
                out.push_str(ext);
            }
            Ok(out)
        })
        .collect::<Result<_, String>>()?;
    check(out)
}

/// A rename only renames: no `/` (that would move things into folders) and no empty names.
fn check(names: Vec<String>) -> Result<Vec<String>, String> {
    if names.iter().any(|n| n.contains('/')) {
        return Err("a name can't contain /".into());
    }
    if names.iter().any(|n| n.trim().is_empty()) {
        return Err("that would leave a name empty".into());
    }
    Ok(names)
}

/// `photo.tar.gz` → (`photo.tar`, `gz`); folders and dotfiles have no extension.
fn split_ext(name: &str, dir: bool) -> (&str, &str) {
    if dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i + 1..]),
    }
}

fn date(t: i64) -> String {
    glib::DateTime::from_unix_local(t)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items<'a>(names: &[&'a str]) -> Vec<Item<'a>> {
        names
            .iter()
            .map(|n| Item {
                name: n,
                dir: !n.contains('.'),
                modified: 0,
            })
            .collect()
    }

    #[test]
    fn templates() {
        let v = items(&["IMG_001.JPG", "IMG_002.JPG", "notes"]);
        assert_eq!(
            apply("photo-{n}", &v).unwrap(),
            ["photo-1.JPG", "photo-2.JPG", "photo-3"]
        );
        assert_eq!(
            apply("{n:3} {name}", &v).unwrap(),
            ["001 IMG_001.JPG", "002 IMG_002.JPG", "003 notes"]
        );
        // a dot (or {ext}) means the pattern decides the extension
        assert_eq!(apply("p{n}.jpg", &v[..2]).unwrap(), ["p1.jpg", "p2.jpg"]);
        assert_eq!(
            apply("{name}-small.{ext}", &v[..1]).unwrap(),
            ["IMG_001-small.JPG"]
        );
        assert!(apply("{nope}", &v).is_err());
        assert!(apply("x{n", &v).is_err());
        assert!(apply("{n:x}", &v).is_err());
        assert!(apply("  ", &v).is_err());
        assert!(apply("sub/{n}", &v).is_err());
        // a silly width is capped, not allocated
        assert_eq!(apply("{n:99999999999}", &v[..1]).unwrap()[0].len(), 20 + 4);
    }

    #[test]
    fn padding_follows_the_count() {
        let names: Vec<String> = (0..12).map(|i| format!("f{i}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let out = apply("{N}", &items(&refs)).unwrap();
        assert_eq!(out[0], "01.txt");
        assert_eq!(out[11], "12.txt");
    }

    #[test]
    fn replacement() {
        let v = items(&["holiday 2024 a.png", "holiday 2024 b.png"]);
        assert_eq!(
            apply("s/2024/2025/", &v).unwrap(),
            ["holiday 2025 a.png", "holiday 2025 b.png"]
        );
        assert_eq!(apply("s/ /_", &v).unwrap()[0], "holiday_2024_a.png");
        assert!(apply("s//x/", &v).is_err());
        assert!(apply("s/onlyone", &v).is_err());
    }
}
