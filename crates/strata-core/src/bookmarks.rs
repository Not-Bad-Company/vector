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
}
