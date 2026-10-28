//! The listing of one folder as the view shows it: sorted, hidden files and the live filter
//! applied. Entries arrive in batches from [`crate::listing`] and change through the monitor.

use std::path::{Path, PathBuf};

use crate::entry::Entry;
use crate::sort::{SortSpec, compare};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ViewOptions {
    pub sort: SortSpec,
    pub show_hidden: bool,
    /// Live filter from `/`, in the [`crate::filter`] language. Empty = no filter.
    pub filter: String,
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
    /// Names listed in the folder's `.hidden` file.
    hidden_names: std::collections::HashSet<String>,
    /// File picker: only files matching one of these show (folders always do).
    allow: Option<Vec<Allow>>,
    /// Names pinned in this folder: they sort first.
    pinned: std::collections::HashSet<String>,
}

/// One pattern of a file picker filter (xdg-desktop-portal: 0 = glob, 1 = MIME type).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allow {
    /// Case-insensitive glob on the name, like `*.png`.
    Glob(String),
    /// A MIME type or a supertype of the entry's, like `image/png` or `image/*`.
    Mime(String),
}

impl Allow {
    fn matches(&self, e: &Entry) -> bool {
        match self {
            Allow::Glob(g) => crate::filter::Filter::parse(g)
                .map(|f| f.matches_name(&e.name))
                .unwrap_or(false),
            Allow::Mime(m) => match m.strip_suffix("/*") {
                Some(top) => e.content_type.split('/').next() == Some(top),
                None => e.content_type == *m || gio::content_type_is_a(&e.content_type, m),
            },
        }
    }
}

impl DirModel {
    pub fn new(dir: impl Into<PathBuf>, opts: ViewOptions) -> DirModel {
        DirModel {
            dir: dir.into(),
            all: Vec::new(),
            shown: Vec::new(),
            opts,
            hidden_names: Default::default(),
            allow: None,
            pinned: Default::default(),
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
            .binary_search_by(|x| self.order(x, &e))
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

    /// Pinned entries first, then the sort.
    fn order(&self, a: &Entry, b: &Entry) -> std::cmp::Ordering {
        let pa = self.pinned.contains(&a.name);
        let pb = self.pinned.contains(&b.name);
        pb.cmp(&pa).then_with(|| compare(a, b, &self.opts.sort))
    }

    fn resort(&mut self) {
        let mut all = std::mem::take(&mut self.all);
        all.sort_by(|a, b| self.order(a, b));
        self.all = all;
        self.refilter();
    }

    /// Sets the names pinned in this folder (they sort first).
    pub fn set_pinned(&mut self, names: std::collections::HashSet<String>) {
        if self.pinned != names {
            self.pinned = names;
            self.resort();
        }
    }

    pub fn is_pinned(&self, name: &str) -> bool {
        self.pinned.contains(name)
    }

    /// Sets a folder's measured size (it lists as 0). Re-sorts when sorted by size.
    pub fn set_size(&mut self, name: &str, size: u64) {
        let Some(e) = self.all.iter_mut().find(|e| e.name == name) else {
            return;
        };
        if e.size == size {
            return;
        }
        e.size = size;
        if self.opts.sort.key == crate::sort::SortKey::Size {
            self.resort();
        }
    }

    /// Sets the names from the folder's `.hidden` file; they count as hidden files.
    pub fn set_hidden_names(&mut self, names: std::collections::HashSet<String>) {
        if self.hidden_names != names {
            self.hidden_names = names;
            self.refilter();
        }
    }

    /// Restricts the files shown (file picker filters); `None` shows all.
    pub fn set_allow(&mut self, allow: Option<Vec<Allow>>) {
        self.allow = allow;
        self.refilter();
    }

    /// Hidden: a dotfile, a backup file, or listed in `.hidden`.
    pub fn is_hidden(&self, e: &Entry) -> bool {
        e.hidden || self.hidden_names.contains(&e.name)
    }

    fn refilter(&mut self) {
        let filter = crate::filter::Filter::parse_lenient(&self.opts.filter);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let show_hidden = self.opts.show_hidden;
        self.shown = (0..self.all.len())
            .filter(|&i| {
                let e = &self.all[i];
                (show_hidden || !self.is_hidden(e))
                    && filter.matches(e, now)
                    && (e.is_dir()
                        || self
                            .allow
                            .as_ref()
                            .is_none_or(|a| a.iter().any(|x| x.matches(e))))
            })
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
        // biggest first, folders among the files
        assert_eq!(names(&m), ["a10.txt", "b.txt", "a2.txt", "Docs"]);
        m.set_sort(SortSpec {
            key: SortKey::Modified,
            reverse: true,
            dirs_first: true,
        });
        // reversed: oldest first
        assert_eq!(names(&m), ["b.txt", "Docs", "a10.txt", "a2.txt"]);
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

    #[test]
    fn dot_hidden_names_hide_and_unhide() {
        let mut m = sample();
        m.set_hidden_names(["b.txt".to_string(), "Docs".to_string()].into());
        assert_eq!(names(&m), ["a2.txt", "a10.txt"]);
        m.set_show_hidden(true);
        assert_eq!(m.len(), 5);
        m.set_show_hidden(false);
        m.set_hidden_names(Default::default());
        assert_eq!(names(&m), ["Docs", "a2.txt", "a10.txt", "b.txt"]);
    }

    #[test]
    fn filter_language_applies() {
        let mut m = sample();
        m.set_filter("*.txt !a2*");
        assert_eq!(names(&m), ["a10.txt", "b.txt"]);
        m.set_filter("type:dir");
        assert_eq!(names(&m), ["Docs"]);
        // half-typed terms are text, never an error
        m.set_filter("size>");
        assert!(m.is_empty());
    }

    #[test]
    fn measured_folder_sizes_sort_by_size() {
        let mut m = sample();
        m.set_sort(SortSpec {
            key: crate::sort::SortKey::Size,
            ..SortSpec::default()
        });
        m.set_size("Docs", 1_000);
        assert_eq!(names(&m)[0], "Docs");
        assert_eq!(m.find("Docs").map(|e| e.size), Some(1_000));
        m.set_size("nope", 5);
    }

    #[test]
    fn picker_allow_list() {
        let mut m = sample();
        m.set_allow(Some(vec![
            Allow::Glob("*.TXT".into()),
            Allow::Glob("b*".into()),
        ]));
        assert_eq!(names(&m), ["Docs", "a2.txt", "a10.txt", "b.txt"]);
        m.set_allow(Some(vec![Allow::Glob("a1*".into())]));
        assert_eq!(names(&m), ["Docs", "a10.txt"]);
        let mut img = entry("p.png", Kind::File, 1, 1);
        img.content_type = "image/png".into();
        assert!(Allow::Mime("image/*".into()).matches(&img));
        assert!(Allow::Mime("image/png".into()).matches(&img));
        assert!(!Allow::Mime("video/*".into()).matches(&img));
        m.set_allow(None);
        assert_eq!(m.len(), 4);
    }

    #[test]
    fn pinned_entries_sort_first_and_stay_first() {
        let mut m = sample();
        m.set_pinned(["b.txt".to_string()].into());
        assert_eq!(names(&m), ["b.txt", "Docs", "a2.txt", "a10.txt"]);
        m.upsert(entry("a1.txt", Kind::File, 1, 1));
        assert_eq!(names(&m)[0], "b.txt");
        assert_eq!(names(&m)[2], "a1.txt");
        m.set_sort(SortSpec {
            reverse: true,
            ..SortSpec::default()
        });
        assert_eq!(names(&m)[0], "b.txt");
        assert!(m.is_pinned("b.txt"));
    }
}
