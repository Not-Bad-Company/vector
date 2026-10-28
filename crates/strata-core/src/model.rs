//! The listing of one folder as the view shows it: sorted, hidden files and the live filter
//! applied. Entries arrive in batches from [`crate::listing`] and change through the monitor.

use std::path::{Path, PathBuf};

use crate::entry::Entry;
use crate::sort::{SortSpec, compare};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewOptions {
    pub sort: SortSpec,
    pub show_hidden: bool,
    /// Live filter from `/`: substring of the name, smart case. Empty = no filter.
    pub filter: String,
}

impl ViewOptions {
    /// Whether `e` is shown under these options.
    pub fn shows(&self, e: &Entry) -> bool {
        if e.hidden && !self.show_hidden {
            return false;
        }
        matches_filter(&e.name, &self.filter)
    }
}

/// Smart-case substring match: case-insensitive unless the pattern has an uppercase letter.
pub fn matches_filter(name: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    if filter.chars().any(char::is_uppercase) {
        name.contains(filter)
    } else {
        name.to_lowercase().contains(filter)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DirModel {
    dir: PathBuf,
    /// Every entry, hidden ones included, kept sorted by `opts.sort`.
    all: Vec<Entry>,
    /// Indices into `all` of the shown entries, in order.
    shown: Vec<usize>,
    opts: ViewOptions,
}

impl DirModel {
    pub fn new(dir: impl Into<PathBuf>, opts: ViewOptions) -> DirModel {
        DirModel {
            dir: dir.into(),
            all: Vec::new(),
            shown: Vec::new(),
            opts,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn options(&self) -> &ViewOptions {
        &self.opts
    }

    /// Number of shown entries.
    pub fn len(&self) -> usize {
        self.shown.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// Number of entries in the folder, hidden and filtered ones included.
    pub fn total(&self) -> usize {
        self.all.len()
    }

    /// The `i`th shown entry.
    pub fn get(&self, i: usize) -> Option<&Entry> {
        self.shown.get(i).map(|&j| &self.all[j])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.shown.iter().map(|&j| &self.all[j])
    }

    /// Every entry including hidden and filtered ones, in sort order.
    pub fn all(&self) -> &[Entry] {
        &self.all
    }

    /// Position of the shown entry called `name`.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.shown.iter().position(|&j| self.all[j].name == name)
    }

    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.all.iter().find(|e| e.name == name)
    }

    /// Adds a batch from the listing. Entries already present (by name) are replaced.
    pub fn extend(&mut self, batch: Vec<Entry>) {
        if self.all.is_empty() {
            self.all = batch;
        } else {
            let incoming: std::collections::HashSet<&str> =
                batch.iter().map(|e| e.name.as_str()).collect();
            if self.all.iter().any(|x| incoming.contains(x.name.as_str())) {
                self.all.retain(|x| !incoming.contains(x.name.as_str()));
            }
            self.all.extend(batch);
        }
        self.resort();
    }

    /// Inserts or replaces one entry (monitor: created / changed).
    pub fn upsert(&mut self, e: Entry) {
        if let Some(i) = self.all.iter().position(|x| x.name == e.name) {
            self.all.remove(i);
        }
        let at = self
            .all
            .binary_search_by(|x| compare(x, &e, &self.opts.sort))
            .unwrap_or_else(|i| i);
        self.all.insert(at, e);
        self.refilter();
    }

    /// Removes the entry called `name`; returns whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        match self.all.iter().position(|x| x.name == name) {
            Some(i) => {
                self.all.remove(i);
                self.refilter();
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.all.clear();
        self.shown.clear();
    }

    pub fn set_sort(&mut self, sort: SortSpec) {
        if self.opts.sort != sort {
            self.opts.sort = sort;
            self.resort();
        }
    }

    pub fn set_show_hidden(&mut self, show: bool) {
        if self.opts.show_hidden != show {
            self.opts.show_hidden = show;
            self.refilter();
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        if self.opts.filter != filter {
            self.opts.filter = filter.to_string();
            self.refilter();
        }
    }

    fn resort(&mut self) {
        let spec = self.opts.sort;
        self.all.sort_by(|a, b| compare(a, b, &spec));
        self.refilter();
    }

    fn refilter(&mut self) {
        let opts = &self.opts;
        self.shown = (0..self.all.len())
            .filter(|&i| opts.shows(&self.all[i]))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;
    use crate::sort::SortKey;
    use crate::test_support::entry;

    fn names(m: &DirModel) -> Vec<&str> {
        m.iter().map(|e| e.name.as_str()).collect()
    }

    fn sample() -> DirModel {
        let mut m = DirModel::new("/x", ViewOptions::default());
        m.extend(vec![
            entry("b.txt", Kind::File, 3, 1),
            entry(".hidden", Kind::File, 1, 2),
            entry("Docs", Kind::Dir, 0, 3),
        ]);
        m.extend(vec![
            entry("a10.txt", Kind::File, 9, 4),
            entry("a2.txt", Kind::File, 2, 5),
        ]);
        m
    }

    #[test]
    fn batches_merge_sorted_and_hide_dotfiles() {
        let m = sample();
        assert_eq!(names(&m), ["Docs", "a2.txt", "a10.txt", "b.txt"]);
        assert_eq!(m.total(), 5);
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn hidden_toggle() {
        let mut m = sample();
        m.set_show_hidden(true);
        assert_eq!(names(&m), ["Docs", ".hidden", "a2.txt", "a10.txt", "b.txt"]);
        m.set_show_hidden(false);
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn sort_changes() {
        let mut m = sample();
        m.set_sort(SortSpec {
            key: SortKey::Size,
            ..SortSpec::default()
        });
        assert_eq!(names(&m), ["Docs", "a2.txt", "b.txt", "a10.txt"]);
        m.set_sort(SortSpec {
            key: SortKey::Modified,
            reverse: true,
            dirs_first: true,
        });
        assert_eq!(names(&m), ["Docs", "a2.txt", "a10.txt", "b.txt"]);
    }

    #[test]
    fn filter_smart_case() {
        let mut m = sample();
        m.set_filter("a");
        assert_eq!(names(&m), ["a2.txt", "a10.txt"]);
        m.set_filter("D");
        assert_eq!(names(&m), ["Docs"]);
        m.set_filter("d");
        assert_eq!(names(&m), ["Docs"]);
        m.set_filter("zzz");
        assert!(m.is_empty());
        m.set_filter("");
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn upsert_and_remove_keep_order() {
        let mut m = sample();
        m.upsert(entry("a5.txt", Kind::File, 1, 1));
        assert_eq!(names(&m), ["Docs", "a2.txt", "a5.txt", "a10.txt", "b.txt"]);
        m.upsert(entry("a5.txt", Kind::Dir, 0, 1));
        assert_eq!(names(&m), ["a5.txt", "Docs", "a2.txt", "a10.txt", "b.txt"]);
        assert!(m.remove("a5.txt"));
        assert!(!m.remove("a5.txt"));
        assert_eq!(m.index_of("b.txt"), Some(3));
        assert_eq!(m.get(0).map(|e| e.name.as_str()), Some("Docs"));
    }

    #[test]
    fn extend_replaces_duplicates() {
        let mut m = sample();
        m.extend(vec![entry("b.txt", Kind::File, 100, 1)]);
        assert_eq!(m.total(), 5);
        assert_eq!(m.find("b.txt").map(|e| e.size), Some(100));
    }
}
