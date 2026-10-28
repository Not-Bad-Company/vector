//! Reading GTK file manager bookmarks.

use std::path::PathBuf;

/// A bookmarked local path and its display label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub path: PathBuf,
    pub label: String,
}

/// Parses local file URIs from a GTK bookmarks file.
pub fn parse(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter_map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line).trim();
            if line.is_empty() {
                return None;
            }
            let (uri, label) = line
                .split_once(' ')
                .map_or((line, None), |(u, l)| (u, Some(l)));
            let encoded = uri.strip_prefix("file://")?;
            let mut decoded = percent_decode(encoded.as_bytes())?;
            if decoded.first() != Some(&b'/') {
                return None;
            }
            while decoded.len() > 1 && decoded.last() == Some(&b'/') {
                decoded.pop();
            }
            let path = PathBuf::from(std::ffi::OsString::from_vec(decoded));
            let label = label
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "/".to_owned())
                });
            Some(Bookmark { path, label })
        })
        .collect()
}

/// The path a bookmarks line points at (`file://` lines only).
fn line_path(line: &str) -> Option<PathBuf> {
    parse(line).into_iter().next().map(|b| b.path)
}

/// `file://` URI for a path, percent-encoding everything but unreserved bytes and `/`.
pub fn uri(path: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Adds a bookmark at the end (no-op if the path is already bookmarked). Other lines,
/// including ones vector doesn't show (sftp://, smb://), stay as they are.
pub fn add(text: &str, path: &std::path::Path, label: Option<&str>) -> String {
    if text.lines().any(|l| line_path(l).as_deref() == Some(path)) {
        return text.to_string();
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&uri(path));
    if let Some(l) = label.map(str::trim).filter(|l| !l.is_empty()) {
        out.push(' ');
        out.push_str(l);
    }
    out.push('\n');
    out
}

/// Removes the bookmark of `path`.
pub fn remove(text: &str, path: &std::path::Path) -> String {
    text.lines()
        .filter(|l| line_path(l).as_deref() != Some(path))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// Gives the bookmark of `path` a new label (empty: back to the folder name).
pub fn rename(text: &str, path: &std::path::Path, label: &str) -> String {
    text.lines()
        .map(|l| {
            if line_path(l).as_deref() == Some(path) {
                let uri = l.split(' ').next().unwrap_or(l);
                match label.trim() {
                    "" => format!("{uri}\n"),
                    lbl => format!("{uri} {lbl}\n"),
                }
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

/// Moves the bookmark of `path` one place up or down among the bookmarks vector shows; other
/// lines stay where they are.
pub fn shift(text: &str, path: &std::path::Path, up: bool) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let shown = |l: &str| line_path(l).is_some();
    let Some(i) = lines
        .iter()
        .position(|l| line_path(l).as_deref() == Some(path))
    else {
        return text.to_string();
    };
    let other = if up {
        (0..i).rev().find(|&j| shown(lines[j]))
    } else {
        (i + 1..lines.len()).find(|&j| shown(lines[j]))
    };
    let Some(j) = other else {
        return text.to_string();
    };
    lines.swap(i, j);
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Moves the bookmark of `path` to where the one of `target` is (dragging it onto that row):
/// moving down it lands after it, moving up before it.
pub fn move_to(text: &str, path: &std::path::Path, target: &std::path::Path) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    let find = |p: &std::path::Path, lines: &[&str]| {
        lines
            .iter()
            .position(|l| line_path(l).as_deref() == Some(p))
    };
    let (Some(i), Some(j)) = (find(path, &lines), find(target, &lines)) else {
        return text.to_string();
    };
    if i == j {
        return text.to_string();
    }
    let line = lines.remove(i);
    lines.insert(j, line);
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Returns the GTK bookmarks file under the user's XDG config directory.
pub fn default_path() -> PathBuf {
    glib::user_config_dir().join("gtk-3.0").join("bookmarks")
}

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

fn percent_decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            decoded.push((hi << 4) | lo);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    Some(decoded)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_labels_and_decodes_bytes() {
        let entries =
            parse("file:///tmp/plain\nfile:///tmp/my%20dir Some label\nfile:///tmp/%C3%A9\n");
        assert_eq!(entries[0].label, "plain");
        assert_eq!(entries[1].path, PathBuf::from("/tmp/my dir"));
        assert_eq!(entries[1].label, "Some label");
        assert_eq!(entries[2].path, PathBuf::from("/tmp/é"));
    }

    #[test]
    fn skips_bad_and_nonlocal_lines() {
        assert!(
            parse("\nsftp://host/path\nsmb://host/share\nfile:///bad%Q0\nfile:relative\n")
                .is_empty()
        );
    }

    #[test]
    fn root_trailing_slash_and_crlf() {
        let entries = parse("file:///\r\nfile:///tmp/folder/\r\n");
        assert_eq!(entries[0].label, "/");
        assert_eq!(entries[1].path, PathBuf::from("/tmp/folder"));
        assert_eq!(entries[1].label, "folder");
    }

    #[test]
    fn edits_keep_other_lines() {
        let text = "sftp://host/x Server\nfile:///tmp/a%20b A\n";
        let added = add(text, std::path::Path::new("/tmp/new é"), Some("New"));
        assert_eq!(added, format!("{text}file:///tmp/new%20%C3%A9 New\n"));
        assert_eq!(
            parse(&added).last().map(|b| b.label.clone()).as_deref(),
            Some("New")
        );
        assert_eq!(add(&added, std::path::Path::new("/tmp/a b"), None), added);
        let renamed = rename(&added, std::path::Path::new("/tmp/a b"), "Renamed");
        assert!(
            renamed.contains("file:///tmp/a%20b Renamed\n")
                && renamed.starts_with("sftp://host/x Server\n")
        );
        let down = shift(&renamed, std::path::Path::new("/tmp/a b"), false);
        assert!(down.ends_with("file:///tmp/a%20b Renamed\n"));
        assert_eq!(shift(&down, std::path::Path::new("/tmp/a b"), false), down);
        assert_eq!(
            shift(&down, std::path::Path::new("/tmp/a b"), true),
            renamed
        );
        // the sftp line isn't shown, so the first shown bookmark can't move above it
        assert_eq!(
            shift(&renamed, std::path::Path::new("/tmp/a b"), true),
            renamed
        );
        let three = add(&renamed, std::path::Path::new("/c"), None);
        let p = |s: &str| std::path::PathBuf::from(s);
        let order = |t: &str| parse(t).into_iter().map(|b| b.path).collect::<Vec<_>>();
        // down: lands after the target; up: before it
        assert_eq!(
            order(&move_to(&three, &p("/tmp/a b"), &p("/c"))),
            [p("/tmp/new é"), p("/c"), p("/tmp/a b")]
        );
        assert_eq!(
            order(&move_to(&three, &p("/c"), &p("/tmp/a b"))),
            [p("/c"), p("/tmp/a b"), p("/tmp/new é")]
        );
        assert_eq!(move_to(&three, &p("/c"), &p("/nope")), three);
        let removed = remove(&renamed, std::path::Path::new("/tmp/a b"));
        assert!(!removed.contains("a%20b") && removed.contains("sftp://"));
    }
}
