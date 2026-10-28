//! Sorting of directory entries.

use std::cmp::Ordering;

use crate::entry::Entry;

/// What a directory listing is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Modified,
    Type,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Modified => "mtime",
            SortKey::Type => "type",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortSpec {
    pub key: SortKey,
    pub reverse: bool,
    /// Folders before files, regardless of key and direction.
    pub dirs_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec {
            key: SortKey::Name,
            reverse: false,
            dirs_first: true,
        }
    }
}

/// Orders two entries by `spec`. Ties on the key fall back to natural name order, so the order is
/// total and stable across reloads.
pub fn compare(a: &Entry, b: &Entry, spec: &SortSpec) -> Ordering {
    // by size or date, folders take their place among the files (biggest/newest first is
    // what those sorts are for); by name or type they come first
    let groups = matches!(spec.key, SortKey::Name | SortKey::Type);
    if spec.dirs_first && groups && a.is_dir() != b.is_dir() {
        return if a.is_dir() {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let by_key = match spec.key {
        SortKey::Name => Ordering::Equal,
        SortKey::Size => b.size.cmp(&a.size),
        SortKey::Modified => b.modified.cmp(&a.modified),
        SortKey::Type => cmp_ignore_case(a.extension(), b.extension()),
    };
    let ord = by_key.then_with(|| natural_cmp(&a.name, &b.name));
    if spec.reverse { ord.reverse() } else { ord }
}

fn cmp_ignore_case(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// Natural order: digit runs compare by value ("note2" < "note10"), text runs case-insensitively.
/// Equal-looking names ("a" / "A", "01" / "1") are ordered by their exact text so the result is
/// a total order.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a, b);
    loop {
        match (x.is_empty(), y.is_empty()) {
            (true, true) => break,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        let (rx, tx) = split_run(x);
        let (ry, ty) = split_run(y);
        let dx = rx.as_bytes()[0].is_ascii_digit();
        let dy = ry.as_bytes()[0].is_ascii_digit();
        let ord = match (dx, dy) {
            (true, true) => {
                let nx = rx.trim_start_matches('0');
                let ny = ry.trim_start_matches('0');
                nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny))
            }
            // digits sort before letters, like most file managers
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => cmp_ignore_case(rx, ry),
        };
        if ord != Ordering::Equal {
            return ord;
        }
        x = tx;
        y = ty;
    }
    a.cmp(b)
}

/// Splits off the leading run of ASCII digits or of non-digits.
fn split_run(s: &str) -> (&str, &str) {
    let digit = s.as_bytes()[0].is_ascii_digit();
    let end = s
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit() != digit)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s.split_at(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;
    use crate::test_support::entry;

    fn sorted(mut v: Vec<Entry>, spec: SortSpec) -> Vec<String> {
        v.sort_by(|a, b| compare(a, b, &spec));
        v.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn natural_numbers() {
        let mut v = vec!["note10", "note2", "note1", "note100", "Note3", "note02"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            v,
            ["note1", "note02", "note2", "Note3", "note10", "note100"]
        );
    }

    #[test]
    fn natural_is_total_and_case_insensitive() {
        assert_eq!(natural_cmp("a", "B"), Ordering::Less);
        assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
        assert_ne!(natural_cmp("a", "A"), Ordering::Equal);
        assert_ne!(natural_cmp("01", "1"), Ordering::Equal);
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
        assert_eq!(natural_cmp("", "a"), Ordering::Less);
        assert_eq!(natural_cmp("2", "a"), Ordering::Less);
    }

    #[test]
    fn natural_unicode_and_huge_numbers() {
        assert_eq!(natural_cmp("ünï", "über"), Ordering::Greater);
        let big = "file99999999999999999999999999";
        let bigger = "file100000000000000000000000000";
        assert_eq!(natural_cmp(big, bigger), Ordering::Less);
        assert_eq!(natural_cmp("日本1", "日本10"), Ordering::Less);
    }

    #[test]
    fn dirs_first_by_name_and_type_biggest_and_newest_first() {
        let v = vec![
            entry("b.txt", Kind::File, 5, 30),
            entry("zdir", Kind::Dir, 0, 10),
            entry("a.rs", Kind::File, 50, 20),
            entry("adir", Kind::Dir, 0, 40),
        ];
        let name = SortSpec::default();
        assert_eq!(sorted(v.clone(), name), ["adir", "zdir", "a.rs", "b.txt"]);
        let size = SortSpec {
            key: SortKey::Size,
            ..name
        };
        assert_eq!(sorted(v.clone(), size), ["a.rs", "b.txt", "adir", "zdir"]);
        let mtime = SortSpec {
            key: SortKey::Modified,
            ..name
        };
        assert_eq!(sorted(v.clone(), mtime), ["adir", "b.txt", "a.rs", "zdir"]);
        let ty = SortSpec {
            key: SortKey::Type,
            ..name
        };
        assert_eq!(sorted(v.clone(), ty), ["adir", "zdir", "a.rs", "b.txt"]);
        let rev = SortSpec {
            reverse: true,
            ..name
        };
        assert_eq!(sorted(v.clone(), rev), ["zdir", "adir", "b.txt", "a.rs"]);
        let mixed = SortSpec {
            dirs_first: false,
            ..name
        };
        assert_eq!(sorted(v, mixed), ["a.rs", "adir", "b.txt", "zdir"]);
    }

    #[test]
    fn ties_fall_back_to_name() {
        let v = vec![
            entry("c", Kind::File, 1, 0),
            entry("a", Kind::File, 1, 0),
            entry("b", Kind::File, 1, 0),
        ];
        let size = SortSpec {
            key: SortKey::Size,
            ..SortSpec::default()
        };
        assert_eq!(sorted(v, size), ["a", "b", "c"]);
    }

    #[test]
    fn extension_rules() {
        assert_eq!(crate::entry::extension_of(".bashrc"), "");
        assert_eq!(crate::entry::extension_of("a.tar.gz"), "gz");
        assert_eq!(crate::entry::extension_of("Makefile"), "");
        assert_eq!(crate::entry::extension_of("x."), "");
    }
}
