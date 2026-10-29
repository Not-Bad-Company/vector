//! A few folders kept after leaving them, so going back shows them at once.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Modification time of a folder (changes when entries are added, removed or renamed in it).
pub fn mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

/// The most recently left folders, each tagged with its modification time at that point.
/// A folder changed since (or gone) is dropped rather than shown stale.
pub struct DirCache<T> {
    cap: usize,
    /// Oldest first.
    items: Vec<(PathBuf, SystemTime, T)>,
}

impl<T> DirCache<T> {
    pub fn new(cap: usize) -> DirCache<T> {
        DirCache {
            cap,
            items: Vec::new(),
        }
    }

    /// Keeps `value` for `dir`, replacing an earlier one and evicting the oldest past `cap`.
    pub fn put(&mut self, dir: &Path, mtime: SystemTime, value: T) {
        self.items.retain(|(p, _, _)| p != dir);
        self.items.push((dir.to_path_buf(), mtime, value));
        while self.items.len() > self.cap {
            self.items.remove(0);
        }
    }

    /// Takes the value for `dir` if its folder has not changed since it was put. Entries of
    /// other folders that changed are dropped too.
    pub fn take(&mut self, dir: &Path) -> Option<T> {
        self.items.retain(|(p, t, _)| mtime(p) == Some(*t));
        let i = self.items.iter().position(|(p, _, _)| p == dir)?;
        Some(self.items.remove(i).2)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::scratch;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn take_returns_value_once_while_folder_unchanged() {
        let d = scratch();
        let dir = d.path().join("big");
        std::fs::create_dir(&dir).unwrap();
        let mut c = DirCache::new(4);
        c.put(&dir, mtime(&dir).unwrap(), "rows");
        assert_eq!(c.take(&dir), Some("rows"));
        assert_eq!(c.take(&dir), None);
    }

    #[test]
    fn changed_folder_is_dropped() {
        let d = scratch();
        let dir = d.path().join("changed");
        std::fs::create_dir(&dir).unwrap();
        let mut c = DirCache::new(4);
        c.put(&dir, UNIX_EPOCH + Duration::from_secs(5), "stale");
        assert_eq!(c.take(&dir), None);
        assert!(c.is_empty());
    }

    #[test]
    fn missing_folder_has_no_mtime_and_is_not_kept() {
        let d = scratch();
        let dir = d.path().join("gone");
        assert!(mtime(&dir).is_none());
        let mut c = DirCache::new(4);
        c.put(&dir, UNIX_EPOCH, 1);
        assert_eq!(c.take(&dir), None);
    }

    #[test]
    fn evicts_oldest_past_cap_and_replaces_same_folder() {
        let d = scratch();
        let mut c = DirCache::new(2);
        let dirs: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = d.path().join(format!("f{i}"));
                std::fs::create_dir(&p).unwrap();
                p
            })
            .collect();
        for (i, p) in dirs.iter().enumerate() {
            c.put(p, mtime(p).unwrap(), i);
        }
        assert_eq!(c.len(), 2);
        assert_eq!(c.take(&dirs[0]), None);
        c.put(&dirs[1], mtime(&dirs[1]).unwrap(), 10);
        assert_eq!(c.len(), 2);
        assert_eq!(c.take(&dirs[1]), Some(10));
        assert_eq!(c.take(&dirs[2]), Some(2));
    }

    #[test]
    fn taking_one_folder_drops_other_changed_ones() {
        let d = scratch();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let mut c = DirCache::new(4);
        c.put(&a, UNIX_EPOCH + Duration::from_secs(5), "old a");
        c.put(&b, mtime(&b).unwrap(), "b");
        assert_eq!(c.take(&b), Some("b"));
        assert!(c.is_empty());
    }
}
