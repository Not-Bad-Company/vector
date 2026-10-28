//! Folders ranked by how often and how recently they were visited (zoxide's "frecency"), for
//! the sidebar's recent list and `:z`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Total score above which every score shrinks, so old habits fade out.
const MAX_TOTAL: f64 = 2000.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frecency {
    /// Visit score and last visit (Unix seconds) per folder.
    dirs: HashMap<PathBuf, (f64, i64)>,
}

impl Frecency {
    /// Reads `score\tlast visit\tpath` lines; bad lines are skipped.
    pub fn parse(text: &str) -> Frecency {
        let mut dirs = HashMap::new();
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (Some(score), Some(last), Some(path)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if let (Ok(score), Ok(last)) = (score.parse::<f64>(), last.parse::<i64>())
                && score.is_finite()
                && score > 0.0
                && path.starts_with('/')
            {
                dirs.insert(PathBuf::from(path), (score, last));
            }
        }
        Frecency { dirs }
    }

    pub fn to_text(&self) -> String {
        let mut lines: Vec<_> = self
            .dirs
            .iter()
            .map(|(p, (s, t))| format!("{s:.3}\t{t}\t{}\n", p.display()))
            .collect();
        lines.sort();
        lines.concat()
    }

    /// One more visit to `dir` at `now`.
    pub fn visit(&mut self, dir: &Path, now: i64) {
        let e = self.dirs.entry(dir.to_path_buf()).or_insert((0.0, now));
        e.0 += 1.0;
        e.1 = now;
        let total: f64 = self.dirs.values().map(|(s, _)| s).sum();
        if total > MAX_TOTAL {
            for v in self.dirs.values_mut() {
                v.0 *= 0.9;
            }
            self.dirs.retain(|_, (s, _)| *s >= 1.0);
        }
    }

    /// Forgets `dir` (it was deleted, or the user doesn't want it listed).
    pub fn forget(&mut self, dir: &Path) {
        self.dirs.remove(dir);
    }

    /// The score at `now`: visits, weighted by how long ago the last one was.
    fn score(&self, dir: &Path, now: i64) -> f64 {
        let Some((s, last)) = self.dirs.get(dir) else {
            return 0.0;
        };
        let age = now - last;
        let weight = if age < 3600 {
            4.0
        } else if age < 86_400 {
            2.0
        } else if age < 7 * 86_400 {
            0.5
        } else {
            0.25
        };
        s * weight
    }

    /// The `n` best folders, best first, leaving out those `skip` says no to.
    pub fn top(&self, n: usize, now: i64, skip: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
        let mut all: Vec<_> = self
            .dirs
            .keys()
            .filter(|p| !skip(p))
            .map(|p| (self.score(p, now), p))
            .collect();
        all.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        all.into_iter().take(n).map(|(_, p)| p.clone()).collect()
    }

    /// The best folder for `:z words`: every word appears in the path, in order (ignoring
    /// case), and the last word in the folder's own name.
    pub fn find(&self, query: &str, now: i64, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return None;
        }
        self.top(usize::MAX, now, |p| !matches(p, &words))
            .into_iter()
            .find(|p| exists(p))
    }
}

fn matches(path: &Path, words: &[String]) -> bool {
    let full = path.to_string_lossy().to_lowercase();
    let mut from = 0;
    for w in words {
        match full[from..].find(w.as_str()) {
            Some(i) => from += i + w.len(),
            None => return false,
        }
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    words.last().is_some_and(|w| name.contains(w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;

    #[test]
    fn ranks_by_visits_and_recency() {
        let mut f = Frecency::default();
        let now = 1_000_000;
        for _ in 0..3 {
            f.visit(Path::new("/a/old"), now - 30 * 24 * H);
        }
        f.visit(Path::new("/a/new"), now - 60);
        // 3 visits a month ago (×0.25) lose to 1 visit a minute ago (×4)
        assert_eq!(
            f.top(2, now, |_| false),
            vec![PathBuf::from("/a/new"), PathBuf::from("/a/old")]
        );
        assert_eq!(
            f.top(5, now, |p| p.ends_with("new")),
            vec![PathBuf::from("/a/old")]
        );
    }

    #[test]
    fn finds_words_in_order_with_the_last_in_the_name() {
        let mut f = Frecency::default();
        for p in ["/home/u/Projects/vector", "/home/u/vector/docs", "/srv/www"] {
            f.visit(Path::new(p), 100);
        }
        let any = |_: &Path| true;
        assert_eq!(
            f.find("vec", 100, any),
            Some(PathBuf::from("/home/u/Projects/vector"))
        );
        assert_eq!(
            f.find("vector docs", 100, any),
            Some(PathBuf::from("/home/u/vector/docs"))
        );
        assert_eq!(f.find("docs vector", 100, any), None);
        assert_eq!(f.find("WWW", 100, any), Some(PathBuf::from("/srv/www")));
        // a folder that no longer exists is skipped
        assert_eq!(f.find("www", 100, |_| false), None);
        assert_eq!(f.find("  ", 100, any), None);
    }

    #[test]
    fn round_trips_and_skips_junk() {
        let mut f = Frecency::default();
        f.visit(Path::new("/x y/z"), 5);
        f.visit(Path::new("/x y/z"), 7);
        let text = f.to_text();
        assert_eq!(text, "2.000\t7\t/x y/z\n");
        assert_eq!(Frecency::parse(&text), f);
        let junk = Frecency::parse("nope\n1\t2\trelative\nNaN\t1\t/a\n-1\t1\t/b\n3\tx\t/c\n");
        assert_eq!(junk, Frecency::default());
    }

    #[test]
    fn old_scores_fade_when_the_total_grows() {
        let mut f = Frecency::default();
        f.visit(Path::new("/once"), 0);
        for _ in 0..(MAX_TOTAL as usize + 5) {
            f.visit(Path::new("/often"), 0);
        }
        assert!(!f.dirs.contains_key(Path::new("/once")));
        assert!(f.dirs[Path::new("/often")].0 < MAX_TOTAL);
        f.forget(Path::new("/often"));
        assert!(f.top(1, 0, |_| false).is_empty());
    }
}
