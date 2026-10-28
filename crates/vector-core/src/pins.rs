//! Pinned items. A pin in a folder keeps an item at the top of that folder; a global pin lists
//! it in the sidebar everywhere. Stored as lines `folder<TAB>/path` and `global<TAB>/path`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pins {
    folder: BTreeSet<PathBuf>,
    global: Vec<PathBuf>,
}

impl Pins {
    pub fn parse(text: &str) -> Pins {
        let mut p = Pins::default();
        for line in text.lines() {
            let Some((kind, path)) = line.split_once('\t') else {
                continue;
            };
            if !path.starts_with('/') {
                continue;
            }
            match kind {
                "folder" => {
                    p.folder.insert(PathBuf::from(path));
                }
                "global" if !p.global.iter().any(|g| g == Path::new(path)) => {
                    p.global.push(PathBuf::from(path))
                }
                _ => {}
            }
        }
        p
    }

    pub fn render(&self) -> String {
        let line = |kind: &str, p: &Path| {
            p.to_str()
                .filter(|s| !s.contains(['\n', '\t']))
                .map(|s| format!("{kind}\t{s}\n"))
        };
        self.folder
            .iter()
            .filter_map(|p| line("folder", p))
            .chain(self.global.iter().filter_map(|p| line("global", p)))
            .collect()
    }

    /// Pins or unpins `item` in its folder; returns whether it is pinned now.
    pub fn toggle_folder(&mut self, item: &Path) -> bool {
        if self.folder.remove(item) {
            false
        } else {
            self.folder.insert(item.to_path_buf());
            true
        }
    }

    /// Pins or unpins `item` everywhere; returns whether it is pinned now.
    pub fn toggle_global(&mut self, item: &Path) -> bool {
        match self.global.iter().position(|g| g == item) {
            Some(i) => {
                self.global.remove(i);
                false
            }
            None => {
                self.global.push(item.to_path_buf());
                true
            }
        }
    }

    /// Names pinned inside `dir`.
    pub fn in_folder(&self, dir: &Path) -> std::collections::HashSet<String> {
        self.folder
            .iter()
            .filter(|p| p.parent() == Some(dir))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }

    pub fn is_global(&self, item: &Path) -> bool {
        self.global.iter().any(|g| g == item)
    }

    /// Global pins, in the order they were pinned.
    pub fn global(&self) -> &[PathBuf] {
        &self.global
    }

    /// Follows a rename or move so the pin stays on the item.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        if self.folder.remove(from) {
            self.folder.insert(to.to_path_buf());
        }
        for g in &mut self.global {
            if g == from {
                *g = to.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggles_and_round_trip() {
        let mut p = Pins::default();
        assert!(p.toggle_folder(Path::new("/a/b.txt")));
        assert!(p.toggle_global(Path::new("/a/b.txt")));
        assert!(p.toggle_global(Path::new("/c")));
        assert_eq!(p.in_folder(Path::new("/a")), ["b.txt".to_string()].into());
        assert!(p.in_folder(Path::new("/")).is_empty());
        let back = Pins::parse(&format!("{}junk\nfolder\trelative\n", p.render()));
        assert_eq!(back, p);
        assert!(!p.toggle_folder(Path::new("/a/b.txt")));
        assert!(!p.toggle_global(Path::new("/c")));
        assert_eq!(p.global(), [PathBuf::from("/a/b.txt")]);
        p.moved(Path::new("/a/b.txt"), Path::new("/a/c.txt"));
        assert!(p.is_global(Path::new("/a/c.txt")));
    }
}
