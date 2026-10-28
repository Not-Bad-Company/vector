//! The user's trash (freedesktop layout): where it is, and where a trashed item came from.

use std::path::{Path, PathBuf};

/// `~/.local/share/Trash/files`: what the sidebar's Trash shows.
pub fn files_dir() -> PathBuf {
    glib::user_data_dir().join("Trash/files")
}

/// Whether `dir` is the trash folder.
pub fn is_trash(dir: &Path) -> bool {
    dir == files_dir()
}

/// When `item` (in the trash) was trashed, as Unix seconds.
pub fn trashed_at(item: &Path) -> Option<i64> {
    let (_, when) = origin(item)?;
    let (date, time) = when.split_once(' ')?;
    let d: Vec<i32> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    let t: Vec<f64> = time.split(':').filter_map(|p| p.parse().ok()).collect();
    let (&[y, mo, da], &[h, mi, s]) = (d.as_slice(), t.as_slice()) else {
        return None;
    };
    glib::DateTime::from_local(y, mo, da, h as i32, mi as i32, s)
        .ok()
        .map(|dt| dt.to_unix())
}

/// Where a trashed item came from and when it was trashed, from its `.trashinfo`.
pub fn origin(item: &Path) -> Option<(PathBuf, String)> {
    let name = item.file_name()?;
    let info = item
        .parent()?
        .parent()?
        .join("info")
        .join(format!("{}.trashinfo", name.to_string_lossy()));
    parse_info(&std::fs::read_to_string(info).ok()?)
}

/// (original path, deletion date) from the text of a `.trashinfo` file.
pub fn parse_info(text: &str) -> Option<(PathBuf, String)> {
    let mut path = None;
    let mut date = String::new();
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("Path=") {
            path = Some(PathBuf::from(unescape(p)));
        } else if let Some(d) = line.strip_prefix("DeletionDate=") {
            date = d.replace('T', " ");
        }
    }
    Some((path?, date))
}

/// Undoes the %XX escapes of a `.trashinfo` path.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_trashinfo() {
        let t = "[Trash Info]\nPath=/home/u/My%20Docs/r%C3%A9sum%C3%A9.pdf\nDeletionDate=2026-10-28T21:05:00\n";
        assert_eq!(
            parse_info(t),
            Some((
                PathBuf::from("/home/u/My Docs/résumé.pdf"),
                "2026-10-28 21:05:00".into()
            ))
        );
        assert_eq!(parse_info("[Trash Info]\n"), None);
    }

    #[test]
    fn finds_the_info_next_to_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("files")).unwrap();
        std::fs::create_dir_all(d.path().join("info")).unwrap();
        std::fs::write(d.path().join("files/a.txt"), "x").unwrap();
        std::fs::write(
            d.path().join("info/a.txt.trashinfo"),
            "[Trash Info]\nPath=/tmp/a.txt\nDeletionDate=2026-01-01T00:00:00\n",
        )
        .unwrap();
        assert_eq!(
            origin(&d.path().join("files/a.txt")).map(|o| o.0),
            Some(PathBuf::from("/tmp/a.txt"))
        );
    }
}
