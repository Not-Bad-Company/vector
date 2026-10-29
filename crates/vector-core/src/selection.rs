//! Cursor and selection over a [`DirModel`], keyed by name so they survive re-sorting and
//! entries appearing or disappearing.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::entry::Entry;
use crate::model::DirModel;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    cursor: Option<String>,
    /// Index the cursor was last at, used when its entry disappears.
    cursor_hint: usize,
    selected: BTreeSet<String>,
    /// Start of a visual-mode range.
    anchor: Option<String>,
}

impl Selection {
    /// Index of the cursor in the shown entries, clamped into range; `None` for an empty model.
    pub fn cursor(&self, m: &DirModel) -> Option<usize> {
        if m.is_empty() {
            return None;
        }
        let found = self.cursor.as_deref().and_then(|n| m.index_of(n));
        Some(found.unwrap_or(self.cursor_hint.min(m.len() - 1)))
    }

    pub fn cursor_entry<'a>(&self, m: &'a DirModel) -> Option<&'a Entry> {
        self.cursor(m).and_then(|i| m.get(i))
    }

    /// Puts the cursor on index `i` (clamped). Extends the visual range when one is active.
    pub fn set_cursor(&mut self, m: &DirModel, i: usize) {
        if m.is_empty() {
            self.cursor = None;
            self.cursor_hint = 0;
            return;
        }
        let i = i.min(m.len() - 1);
        self.cursor = m.get(i).map(|e| e.name.clone());
        self.cursor_hint = i;
        if self.anchor.is_some() {
            self.update_visual(m);
        }
    }

    pub fn set_cursor_name(&mut self, m: &DirModel, name: &str) {
        if let Some(i) = m.index_of(name) {
            self.set_cursor(m, i);
        }
    }

    /// Moves the cursor by `delta` rows, stopping at the ends.
    pub fn move_by(&mut self, m: &DirModel, delta: isize) {
        let cur = self.cursor(m).unwrap_or(0);
        let next = cur.saturating_add_signed(delta);
        self.set_cursor(m, next);
    }

    pub fn is_selected(&self, name: &str) -> bool {
        self.selected.contains(name)
    }

    pub fn selected_names(&self) -> impl Iterator<Item = &str> {
        self.selected.iter().map(String::as_str)
    }

    pub fn has_selection(&self) -> bool {
        !self.selected.is_empty()
    }

    /// Replaces the selection, e.g. from a mouse selection in the view.
    pub fn set_selected<I: IntoIterator<Item = String>>(&mut self, names: I) {
        self.selected = names.into_iter().collect();
    }

    pub fn toggle(&mut self, name: &str) {
        if !self.selected.remove(name) {
            self.selected.insert(name.to_string());
        }
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Starts visual mode at the cursor.
    pub fn start_visual(&mut self, m: &DirModel) {
        self.anchor = self.cursor_entry(m).map(|e| e.name.clone());
        self.update_visual(m);
    }

    /// Leaves visual mode, keeping the selected range.
    pub fn end_visual(&mut self) {
        self.anchor = None;
    }

    pub fn in_visual(&self) -> bool {
        self.anchor.is_some()
    }

    fn update_visual(&mut self, m: &DirModel) {
        let (Some(a), Some(c)) = (
            self.anchor.as_deref().and_then(|n| m.index_of(n)),
            self.cursor(m),
        ) else {
            return;
        };
        let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
        self.selected = (lo..=hi)
            .filter_map(|i| m.get(i))
            .map(|e| e.name.clone())
            .collect();
    }

    /// Drops selected names that are no longer shown (after a reload, filter or delete).
    pub fn prune(&mut self, m: &DirModel) {
        self.selected.retain(|n| m.index_of(n).is_some());
        if self
            .anchor
            .as_deref()
            .is_some_and(|n| m.index_of(n).is_none())
        {
            self.anchor = None;
        }
    }

    /// What an action applies to: the selection if any, else the entry under the cursor.
    /// In display order.
    pub fn targets<'a>(&self, m: &'a DirModel) -> Vec<&'a Entry> {
        if self.selected.is_empty() {
            self.cursor_entry(m).into_iter().collect()
        } else {
            m.iter()
                .filter(|e| self.selected.contains(&e.name))
                .collect()
        }
    }
}

/// Whether a yank or cut acts on the set it last took (`pending`) rather than the cursor row
/// alone. A yank or cut clears the selection, so with nothing selected and the cursor on one of
/// those files the set is what the user means: cut then ctrl+c copies the whole cut set.
pub fn on_pending_set(has_selection: bool, cursor: Option<&Path>, pending: &[PathBuf]) -> bool {
    !has_selection && cursor.is_some_and(|c| pending.iter().any(|p| p == c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;
    use crate::model::ViewOptions;
    use crate::test_support::entry;

    fn model(n: usize) -> DirModel {
        let mut m = DirModel::new("/x", ViewOptions::default());
        m.extend(
            (0..n)
                .map(|i| entry(&format!("f{i}"), Kind::File, 0, 0))
                .collect(),
        );
        m
    }

    fn names(v: Vec<&Entry>) -> Vec<String> {
        v.into_iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn cursor_moves_and_clamps() {
        let m = model(5);
        let mut s = Selection::default();
        assert_eq!(s.cursor(&m), Some(0));
        s.move_by(&m, 3);
        assert_eq!(s.cursor(&m), Some(3));
        s.move_by(&m, 10);
        assert_eq!(s.cursor(&m), Some(4));
        s.move_by(&m, -100);
        assert_eq!(s.cursor(&m), Some(0));
        assert_eq!(s.cursor(&model(0)), None);
    }

    #[test]
    fn cursor_follows_name_across_resort_and_survives_removal() {
        let mut m = model(5);
        let mut s = Selection::default();
        s.set_cursor(&m, 2);
        m.upsert(entry("a-first", Kind::File, 0, 0));
        assert_eq!(s.cursor_entry(&m).map(|e| e.name.as_str()), Some("f2"));
        m.remove("f2");
        assert_eq!(s.cursor(&m), Some(2));
        let mut small = model(1);
        s.set_cursor(&m, 4);
        small.remove("f0");
        assert_eq!(s.cursor(&small), None);
    }

    #[test]
    fn visual_range_both_directions() {
        let m = model(6);
        let mut s = Selection::default();
        s.set_cursor(&m, 3);
        s.start_visual(&m);
        s.move_by(&m, 2);
        assert_eq!(names(s.targets(&m)), ["f3", "f4", "f5"]);
        s.move_by(&m, -4);
        assert_eq!(names(s.targets(&m)), ["f1", "f2", "f3"]);
        s.end_visual();
        s.move_by(&m, 1);
        assert_eq!(names(s.targets(&m)), ["f1", "f2", "f3"]);
        s.clear();
        assert_eq!(names(s.targets(&m)), ["f2"]);
    }

    #[test]
    fn toggle_prune_and_targets_order() {
        let mut m = model(4);
        let mut s = Selection::default();
        s.toggle("f3");
        s.toggle("f1");
        assert_eq!(names(s.targets(&m)), ["f1", "f3"]);
        s.toggle("f3");
        assert_eq!(names(s.targets(&m)), ["f1"]);
        m.remove("f1");
        s.prune(&m);
        assert!(!s.has_selection());
    }

    #[test]
    fn pending_set_applies_only_with_nothing_selected_and_cursor_on_it() {
        let set = vec![PathBuf::from("/d/a"), PathBuf::from("/d/b")];
        assert!(on_pending_set(false, Some(Path::new("/d/b")), &set));
        // a selection of its own wins
        assert!(!on_pending_set(true, Some(Path::new("/d/b")), &set));
        // the cursor moved to a file that was not part of it
        assert!(!on_pending_set(false, Some(Path::new("/d/c")), &set));
        assert!(!on_pending_set(false, None, &set));
        assert!(!on_pending_set(false, Some(Path::new("/d/a")), &[]));
    }
}
