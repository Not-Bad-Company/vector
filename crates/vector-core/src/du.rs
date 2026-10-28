//! Disk usage: folder sizes and the biggest files under a folder. Blocking, multi-threaded
//! walks (a few threads, so the machine stays quiet); call from a worker thread.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Threads per walk. Enough to hide disk latency; few enough not to spin up the fans.
pub const THREADS: usize = 3;

fn walker(root: &Path, one_file_system: bool) -> ignore::WalkParallel {
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .follow_links(false)
        .same_file_system(one_file_system)
        .threads(THREADS)
        .build_parallel()
}

/// Apparent size in bytes of everything below `dir` (symlinks not followed, other file systems
/// included). `None` when cancelled.
pub fn dir_size(dir: &Path, cancel: &AtomicBool) -> Option<u64> {
    let total = AtomicU64::new(0);
    walker(dir, false).run(|| {
        Box::new(|item| {
            if cancel.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            if let Ok(e) = item
                && e.file_type().is_some_and(|t| t.is_file())
                && let Ok(m) = e.metadata()
            {
                total.fetch_add(m.len(), Ordering::Relaxed);
            }
            ignore::WalkState::Continue
        })
    });
    (!cancel.load(Ordering::Relaxed)).then(|| total.load(Ordering::Relaxed))
}

/// The `n` biggest files below `root`, biggest first. `one_file_system` keeps the walk on the
/// root's file system (so `/` skips /proc, other drives and network mounts). `seen` counts
/// files as they are visited, for progress. Partial results when cancelled.
pub fn biggest_files(
    root: &Path,
    n: usize,
    one_file_system: bool,
    cancel: &AtomicBool,
    seen: &AtomicU64,
) -> Vec<(PathBuf, u64)> {
    if n == 0 {
        return Vec::new();
    }
    // min-heap of the current top n; `floor` lets threads skip small files without locking
    let heap: Mutex<BinaryHeap<Reverse<(u64, PathBuf)>>> = Mutex::new(BinaryHeap::new());
    let floor = AtomicU64::new(0);
    walker(root, one_file_system).run(|| {
        Box::new(|item| {
            if cancel.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Ok(e) = item else {
                return ignore::WalkState::Continue;
            };
            if !e.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            seen.fetch_add(1, Ordering::Relaxed);
            let Ok(len) = e.metadata().map(|m| m.len()) else {
                return ignore::WalkState::Continue;
            };
            if len <= floor.load(Ordering::Relaxed) {
                return ignore::WalkState::Continue;
            }
            if let Ok(mut h) = heap.lock() {
                h.push(Reverse((len, e.into_path())));
                if h.len() > n {
                    h.pop();
                }
                if h.len() == n
                    && let Some(Reverse((min, _))) = h.peek()
                {
                    floor.store(*min, Ordering::Relaxed);
                }
            }
            ignore::WalkState::Continue
        })
    });
    let mut v: Vec<(PathBuf, u64)> = heap
        .into_inner()
        .map(|h| h.into_iter().map(|Reverse((len, p))| (p, len)).collect())
        .unwrap_or_default();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tree() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        fs::create_dir_all(d.join("a/b")).unwrap();
        fs::create_dir_all(d.join(".hid")).unwrap();
        fs::write(d.join("a/one"), vec![0u8; 100]).unwrap();
        fs::write(d.join("a/b/two"), vec![0u8; 3000]).unwrap();
        fs::write(d.join(".hid/three"), vec![0u8; 50]).unwrap();
        fs::write(d.join("four"), vec![0u8; 700]).unwrap();
        std::os::unix::fs::symlink(d.join("a"), d.join("loop")).unwrap();
        t
    }

    #[test]
    fn sizes_include_hidden_and_skip_links() {
        let t = tree();
        let no = AtomicBool::new(false);
        assert_eq!(dir_size(t.path(), &no), Some(3850));
        assert_eq!(dir_size(&t.path().join("a"), &no), Some(3100));
        assert_eq!(dir_size(&t.path().join("missing"), &no), Some(0));
        assert_eq!(dir_size(t.path(), &AtomicBool::new(true)), None);
    }

    #[test]
    fn biggest_first_and_bounded() {
        let t = tree();
        let (no, seen) = (AtomicBool::new(false), AtomicU64::new(0));
        let top = biggest_files(t.path(), 2, true, &no, &seen);
        let names: Vec<_> = top
            .iter()
            .map(|(p, n)| (p.file_name().unwrap().to_owned(), *n))
            .collect();
        assert_eq!(names, [("two".into(), 3000), ("four".into(), 700)]);
        assert_eq!(seen.load(Ordering::Relaxed), 4);
        assert_eq!(biggest_files(t.path(), 10, true, &no, &seen).len(), 4);
        assert!(biggest_files(t.path(), 0, true, &no, &seen).is_empty());
    }
}
