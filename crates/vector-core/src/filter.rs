//! The `/` filter language. Space-separated terms, all of which must match:
//!
//! - `text`: name contains it (smart case)
//! - `*.exe`, `IMG_??.jpg`: glob on the name (case-insensitive)
//! - `ext:exe` or `ext:png,jpg`: extension is one of these
//! - `type:image` (`is:` works too): `dir`, `file`, `link`, `image`, `video`, `audio`, `doc`,
//!   `archive`, `code`, `text`, `exec`, `hidden`; several with `|` or `,`
//! - `size>10m`, `size<1k`, `size=0`: units b, k, m, g, t (powers of 1024)
//! - `mod<7d` (changed in the last 7 days), `mod>1y`: units min, h, d, w, mo, y
//! - `!term`: not
//!
//! Example: `type:image !*.gif size>1m` = images over 1 MB that aren't GIFs.

use crate::entry::{Entry, Kind, extension_of};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Cmp {
    Less,
    More,
    Equal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Term {
    Not(Box<Term>),
    Text(String),
    Glob(Vec<char>),
    Ext(Vec<String>),
    Type(Vec<Class>),
    Size(Cmp, u64),
    /// Age in seconds.
    Age(Cmp, i64),
}

/// File classes for `type:`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Dir,
    File,
    Link,
    Image,
    Video,
    Audio,
    Doc,
    Archive,
    Code,
    Text,
    Exec,
    Hidden,
}

impl Class {
    pub const ALL: [(&'static str, Class); 12] = [
        ("dir", Class::Dir),
        ("file", Class::File),
        ("link", Class::Link),
        ("image", Class::Image),
        ("video", Class::Video),
        ("audio", Class::Audio),
        ("doc", Class::Doc),
        ("archive", Class::Archive),
        ("code", Class::Code),
        ("text", Class::Text),
        ("exec", Class::Exec),
        ("hidden", Class::Hidden),
    ];

    fn parse(s: &str) -> Option<Class> {
        let s = s.to_ascii_lowercase();
        let s = match s.as_str() {
            "folder" | "directory" => "dir",
            "img" | "picture" | "photo" => "image",
            "music" | "sound" => "audio",
            "document" | "docs" => "doc",
            "zip" => "archive",
            "src" | "source" => "code",
            "executable" | "x" => "exec",
            "symlink" => "link",
            other => other,
        };
        Class::ALL.iter().find(|(n, _)| *n == s).map(|(_, c)| *c)
    }

    /// The class of an entry, for "show only this kind" in menus.
    pub fn of(e: &Entry) -> Option<Class> {
        [
            Class::Dir,
            Class::Image,
            Class::Video,
            Class::Audio,
            Class::Doc,
            Class::Archive,
            Class::Code,
        ]
        .into_iter()
        .find(|c| c.matches(e))
    }

    pub fn name(self) -> &'static str {
        Class::ALL
            .iter()
            .find(|(_, c)| *c == self)
            .map_or("", |(n, _)| n)
    }

    fn matches(self, e: &Entry) -> bool {
        let ct = e.content_type.as_str();
        let ext = extension_of(&e.name).to_ascii_lowercase();
        let ext = ext.as_str();
        match self {
            Class::Dir => e.kind == Kind::Dir,
            Class::File => e.kind == Kind::File,
            Class::Link => e.symlink,
            Class::Image => ct.starts_with("image/"),
            Class::Video => ct.starts_with("video/"),
            Class::Audio => ct.starts_with("audio/"),
            Class::Doc => {
                ct == "application/pdf"
                    || ct.starts_with("application/vnd.oasis.opendocument")
                    || ct.starts_with("application/vnd.openxmlformats")
                    || ct.starts_with("application/msword")
                    || ct.starts_with("application/vnd.ms-")
                    || matches!(
                        ext,
                        "pdf"
                            | "doc"
                            | "docx"
                            | "odt"
                            | "ods"
                            | "odp"
                            | "xls"
                            | "xlsx"
                            | "ppt"
                            | "pptx"
                            | "rtf"
                            | "epub"
                            | "md"
                            | "txt"
                    )
            }
            Class::Archive => {
                matches!(
                    ct,
                    "application/zip"
                        | "application/x-tar"
                        | "application/gzip"
                        | "application/x-compressed-tar"
                        | "application/x-xz"
                        | "application/x-xz-compressed-tar"
                        | "application/zstd"
                        | "application/x-zstd-compressed-tar"
                        | "application/x-7z-compressed"
                        | "application/vnd.rar"
                        | "application/x-rar"
                        | "application/x-bzip2"
                ) || matches!(
                    ext,
                    "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "7z" | "rar" | "bz2"
                )
            }
            Class::Code => matches!(
                ext,
                "rs" | "py"
                    | "js"
                    | "ts"
                    | "tsx"
                    | "jsx"
                    | "c"
                    | "h"
                    | "cc"
                    | "cpp"
                    | "hpp"
                    | "go"
                    | "java"
                    | "kt"
                    | "lua"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "fish"
                    | "rb"
                    | "php"
                    | "cs"
                    | "swift"
                    | "html"
                    | "css"
                    | "scss"
                    | "json"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "xml"
                    | "sql"
                    | "dart"
                    | "zig"
                    | "nix"
            ),
            Class::Text => {
                ct.starts_with("text/")
                    || matches!(
                        ct,
                        "application/json" | "application/xml" | "application/toml"
                    )
            }
            Class::Exec => {
                e.kind == Kind::File
                    && (e.executable
                        || matches!(ext, "exe" | "msi" | "appimage" | "bat" | "cmd" | "com")
                        || ct == "application/x-executable"
                        || ct == "application/x-ms-dos-executable")
            }
            Class::Hidden => e.hidden,
        }
    }
}

/// A parsed filter. The empty filter matches everything.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    terms: Vec<Term>,
}

impl Filter {
    pub fn parse(s: &str) -> Result<Filter, String> {
        let terms = s
            .split_whitespace()
            .map(parse_term)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Filter { terms })
    }

    /// Like [`Filter::parse`], but a term that doesn't parse (`size>`, half typed) is read as
    /// plain text, so the live filter never errors while typing.
    pub fn parse_lenient(s: &str) -> Filter {
        let terms = s
            .split_whitespace()
            .map(|t| parse_term(t).unwrap_or_else(|_| Term::Text(t.to_string())))
            .collect();
        Filter { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether a bare name passes the text and glob terms (other terms are ignored).
    pub fn matches_name(&self, name: &str) -> bool {
        let lower: Vec<char> = name.to_lowercase().chars().collect();
        self.terms.iter().all(|t| match t {
            Term::Text(s) => crate::model::matches_filter(name, s),
            Term::Glob(p) => glob(p, &lower),
            _ => true,
        })
    }

    /// Whether `e` passes; `now` is the current Unix time.
    pub fn matches(&self, e: &Entry, now: i64) -> bool {
        self.terms.iter().all(|t| term_matches(t, e, now))
    }
}

fn parse_term(t: &str) -> Result<Term, String> {
    if let Some(rest) = t.strip_prefix('!') {
        if rest.is_empty() {
            return Err("nothing after !".into());
        }
        return Ok(Term::Not(Box::new(parse_term(rest)?)));
    }
    let lower = t.to_ascii_lowercase();
    if let Some(v) = lower.strip_prefix("ext:") {
        let exts: Vec<String> = split_values(v)
            .map(|e| e.trim_start_matches('.').to_string())
            .collect();
        if exts.is_empty() {
            return Err("ext: needs an extension".into());
        }
        return Ok(Term::Ext(exts));
    }
    if let Some(v) = lower
        .strip_prefix("type:")
        .or_else(|| lower.strip_prefix("is:"))
    {
        let classes = split_values(v)
            .map(|c| Class::parse(c).ok_or_else(|| format!("unknown type {c}")))
            .collect::<Result<Vec<_>, _>>()?;
        if classes.is_empty() {
            return Err("type: needs a type".into());
        }
        return Ok(Term::Type(classes));
    }
    if let Some(v) = lower.strip_prefix("size") {
        let (cmp, n) = split_cmp(v).ok_or("size needs <, > or = and a number")?;
        return Ok(Term::Size(
            cmp,
            parse_size(n).ok_or_else(|| format!("bad size {n}"))?,
        ));
    }
    if let Some(v) = lower
        .strip_prefix("mod")
        .or_else(|| lower.strip_prefix("age"))
    {
        let (cmp, n) = split_cmp(v).ok_or("mod needs <, > or = and an age like 7d")?;
        return Ok(Term::Age(
            cmp,
            parse_age(n).ok_or_else(|| format!("bad age {n}"))?,
        ));
    }
    if t.contains(['*', '?']) {
        return Ok(Term::Glob(lower.chars().collect()));
    }
    Ok(Term::Text(t.to_string()))
}

fn split_values(v: &str) -> impl Iterator<Item = &str> {
    v.split([',', '|']).filter(|s| !s.is_empty())
}

fn split_cmp(v: &str) -> Option<(Cmp, &str)> {
    let v = v.strip_prefix(':').unwrap_or(v);
    let (cmp, rest) = match v.chars().next()? {
        '<' => (Cmp::Less, &v[1..]),
        '>' => (Cmp::More, &v[1..]),
        '=' => (Cmp::Equal, &v[1..]),
        _ => return None,
    };
    (!rest.is_empty()).then_some((cmp, rest))
}

fn number_and_unit(s: &str) -> Option<(f64, &str)> {
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let n: f64 = s[..split].parse().ok()?;
    (n.is_finite() && n >= 0.0).then_some((n, &s[split..]))
}

fn parse_size(s: &str) -> Option<u64> {
    let (n, unit) = number_and_unit(s)?;
    let mul: f64 = match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mul).min(u64::MAX as f64) as u64)
}

fn parse_age(s: &str) -> Option<i64> {
    let (n, unit) = number_and_unit(s)?;
    let secs: f64 = match unit {
        "s" => 1.0,
        "min" => 60.0,
        "h" => 3600.0,
        "" | "d" => 86_400.0,
        "w" => 7.0 * 86_400.0,
        "mo" => 30.0 * 86_400.0,
        "y" => 365.0 * 86_400.0,
        _ => return None,
    };
    Some((n * secs).min(i64::MAX as f64) as i64)
}

fn compare<T: Ord>(cmp: &Cmp, value: T, limit: T) -> bool {
    match cmp {
        Cmp::Less => value < limit,
        Cmp::More => value > limit,
        Cmp::Equal => value == limit,
    }
}

fn term_matches(t: &Term, e: &Entry, now: i64) -> bool {
    match t {
        Term::Not(inner) => !term_matches(inner, e, now),
        Term::Text(s) => crate::model::matches_filter(&e.name, s),
        Term::Glob(p) => glob(p, &e.name.to_lowercase().chars().collect::<Vec<_>>()),
        Term::Ext(exts) => {
            let ext = extension_of(&e.name).to_ascii_lowercase();
            e.kind != Kind::Dir && exts.contains(&ext)
        }
        Term::Type(classes) => classes.iter().any(|c| c.matches(e)),
        Term::Size(cmp, n) => e.kind != Kind::Dir && compare(cmp, e.size, *n),
        Term::Age(cmp, secs) => {
            e.modified > 0 && compare(cmp, now.saturating_sub(e.modified), *secs)
        }
    }
}

/// `*` any run, `?` one char. Iterative with backtracking to the last `*`.
fn glob(p: &[char], s: &[char]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::entry;

    const NOW: i64 = 1_000_000_000;

    fn e(name: &str, kind: Kind, size: u64, age_days: i64, ct: &str) -> Entry {
        let mut x = entry(name, kind, size, NOW - age_days * 86_400);
        x.content_type = ct.into();
        x
    }

    fn sample() -> Vec<Entry> {
        let mut setup = e(
            "setup.EXE",
            Kind::File,
            5 << 20,
            2,
            "application/x-ms-dos-executable",
        );
        setup.executable = false;
        let mut script = e("run.sh", Kind::File, 100, 400, "application/x-shellscript");
        script.executable = true;
        vec![
            setup,
            script,
            e("photo.jpg", Kind::File, 3 << 20, 1, "image/jpeg"),
            e("anim.gif", Kind::File, 2 << 20, 30, "image/gif"),
            e("clip.mp4", Kind::File, 50 << 20, 10, "video/mp4"),
            e("notes.md", Kind::File, 10, 0, "text/markdown"),
            e(
                "backup.tar.gz",
                Kind::File,
                1 << 30,
                800,
                "application/x-compressed-tar",
            ),
            e("Games", Kind::Dir, 0, 5, "inode/directory"),
            e(".config", Kind::Dir, 0, 5, "inode/directory"),
        ]
    }

    fn names(f: &str) -> Vec<String> {
        let filter = Filter::parse(f).unwrap();
        sample()
            .into_iter()
            .filter(|x| filter.matches(x, NOW))
            .map(|x| x.name)
            .collect()
    }

    #[test]
    fn text_and_globs() {
        assert_eq!(names("o"), ["photo.jpg", "notes.md", ".config"]);
        assert_eq!(names("*.exe"), ["setup.EXE"]);
        assert_eq!(names("*.?if"), ["anim.gif"]);
        assert_eq!(names("*a*.*"), ["anim.gif", "backup.tar.gz"]);
        assert!(glob(
            &"a*b*c".chars().collect::<Vec<_>>(),
            &"aXbYbZc".chars().collect::<Vec<_>>()
        ));
        assert!(!glob(
            &"a*b".chars().collect::<Vec<_>>(),
            &"ab c".chars().collect::<Vec<_>>()
        ));
        assert!(glob(&"*".chars().collect::<Vec<_>>(), &[]));
    }

    #[test]
    fn extensions_and_types() {
        assert_eq!(names("ext:exe"), ["setup.EXE"]);
        assert_eq!(names("ext:.jpg,gif"), ["photo.jpg", "anim.gif"]);
        assert_eq!(names("type:image"), ["photo.jpg", "anim.gif"]);
        assert_eq!(
            names("is:image|video"),
            ["photo.jpg", "anim.gif", "clip.mp4"]
        );
        assert_eq!(names("type:exec"), ["setup.EXE", "run.sh"]);
        assert_eq!(names("type:dir"), ["Games", ".config"]);
        assert_eq!(names("type:archive"), ["backup.tar.gz"]);
        assert_eq!(names("type:doc"), ["notes.md"]);
        assert_eq!(names("type:hidden"), [".config"]);
        assert_eq!(names("type:folder !type:hidden"), ["Games"]);
    }

    #[test]
    fn sizes_ages_and_negation() {
        assert_eq!(names("size>10m"), ["clip.mp4", "backup.tar.gz"]);
        assert_eq!(names("size<1k"), ["run.sh", "notes.md"]);
        assert_eq!(names("size:=10"), ["notes.md"]);
        assert_eq!(names("mod<3d"), ["setup.EXE", "photo.jpg", "notes.md"]);
        assert_eq!(names("mod>1y"), ["run.sh", "backup.tar.gz"]);
        assert_eq!(names("type:image !*.gif size>1m"), ["photo.jpg"]);
        assert_eq!(names("!type:file"), ["Games", ".config"]);
        assert_eq!(names("").len(), sample().len());
    }

    #[test]
    fn errors_and_lenient() {
        for bad in [
            "type:nope",
            "size>",
            "size>10q",
            "mod<3x",
            "ext:",
            "!",
            "size10m",
        ] {
            assert!(Filter::parse(bad).is_err(), "{bad}");
        }
        let f = Filter::parse_lenient("size>");
        assert_eq!(
            f,
            Filter {
                terms: vec![Term::Text("size>".into())]
            }
        );
        assert!(Filter::parse_lenient("  ").is_empty());
    }

    #[test]
    fn class_of() {
        let v = sample();
        assert_eq!(Class::of(&v[2]), Some(Class::Image));
        assert_eq!(Class::of(&v[7]), Some(Class::Dir));
        assert_eq!(Class::of(&v[0]), None);
        assert_eq!(Class::Image.name(), "image");
    }
}
