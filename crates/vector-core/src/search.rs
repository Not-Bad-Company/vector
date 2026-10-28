//! Fuzzy file-name search and regular-expression content search.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::WalkBuilder;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

/// Options shared by name and content searches.
#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// Include dot files and directories.
    pub hidden: bool,
    /// Honor `.gitignore` and `.ignore` files.
    pub follow_gitignore: bool,
    /// Maximum traversal depth.
    pub max_depth: Option<usize>,
}

/// A fuzzy match against a relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindHit {
    /// Path relative to the search root.
    pub path: PathBuf,
    /// Whether the match is a directory.
    pub is_dir: bool,
    /// Fuzzy match score.
    pub score: u32,
    /// Character indices in the displayed relative path that matched.
    pub indices: Vec<u32>,
}

/// Fuzzy-match paths below `root`, ranked by score and then path length.
pub fn find(
    root: &Path,
    query: &str,
    opts: &SearchOptions,
    cancel: &AtomicBool,
    limit: usize,
) -> Vec<FindHit> {
    if cancel.load(Ordering::Relaxed) || limit == 0 {
        return Vec::new();
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut hits = Vec::new();
    for entry in walker(root, opts) {
        if cancel.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path == root {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok() else {
            continue;
        };
        let display = relative.to_string_lossy();
        let mut utf32 = Vec::new();
        let haystack = Utf32Str::new(&display, &mut utf32);
        let mut indices = Vec::new();
        let Some(score) = pattern.indices(haystack, &mut matcher, &mut indices) else {
            continue;
        };
        indices.sort_unstable();
        indices.dedup();
        hits.push(FindHit {
            path: relative.to_path_buf(),
            is_dir: entry.file_type().is_some_and(|kind| kind.is_dir()),
            score,
            indices,
        });
    }
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(limit);
    hits
}

/// A matching content line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    /// Path relative to the search root.
    pub path: PathBuf,
    /// One-based line number.
    pub line: u64,
    /// Trimmed matching line, limited to 200 characters.
    pub text: String,
}

/// A search pattern could not be compiled.
#[derive(Debug)]
pub enum SearchError {
    /// Invalid regular expression.
    BadPattern(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPattern(message) => write!(f, "invalid search pattern: {message}"),
        }
    }
}

/// Search file contents using a smart-case regular expression. Walks in file-name order and
/// stops at `limit` hits.
pub fn grep(
    root: &Path,
    pattern: &str,
    opts: &SearchOptions,
    cancel: &AtomicBool,
    limit: usize,
) -> Result<Vec<GrepHit>, SearchError> {
    let case_insensitive = !pattern.chars().any(char::is_uppercase);
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive)
        .line_terminator(Some(b'\n'))
        .build(pattern)
        .map_err(|error| SearchError::BadPattern(error.to_string()))?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .build();
    let mut hits = Vec::new();
    for entry in walker(root, opts) {
        if cancel.load(Ordering::Relaxed) || hits.len() >= limit {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if entry.metadata().map_or(true, |m| m.len() > MAX_GREP_SIZE) {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        // unreadable files are skipped like binary ones
        let _ = searcher.search_path(
            &matcher,
            path,
            UTF8(|line, text| {
                if cancel.load(Ordering::Relaxed) || hits.len() >= limit {
                    return Ok(false);
                }
                let trimmed = text.trim();
                let end = trimmed
                    .char_indices()
                    .nth(200)
                    .map_or(trimmed.len(), |(i, _)| i);
                hits.push(GrepHit {
                    path: relative.to_path_buf(),
                    line,
                    text: trimmed[..end].to_owned(),
                });
                Ok(true)
            }),
        );
    }
    hits.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    Ok(hits)
}

/// Files larger than this are not searched.
const MAX_GREP_SIZE: u64 = 50 * 1024 * 1024;

fn walker(root: &Path, opts: &SearchOptions) -> ignore::Walk {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(!opts.hidden)
        .git_ignore(opts.follow_gitignore)
        .ignore(opts.follow_gitignore)
        .git_exclude(opts.follow_gitignore)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .max_depth(opts.max_depth)
        .sort_by_file_name(|a, b| a.cmp(b));
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("nested/space")).unwrap();
        fs::write(dir.path().join("nested/space/needle.txt"), "hello\nHELLO\n").unwrap();
        fs::write(dir.path().join("scatter_n_e_e_d_l_e"), "other\n").unwrap();
        fs::write(dir.path().join(".hidden"), "hello\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "hello\n").unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.path().join("binary"), b"hello\0world\n").unwrap();
        fs::write(dir.path().join("café.txt"), "hello\n").unwrap();
        fs::write(
            dir.path().join("long.txt"),
            format!("{}needle\n", "x".repeat(250)),
        )
        .unwrap();
        dir
    }

    #[test]
    fn find_matches_rank_options_limit_cancel_and_indices() {
        let dir = fixture();
        let cancel = AtomicBool::new(false);
        let defaults = SearchOptions::default();
        let hits = find(dir.path(), "needle", &defaults, &cancel, 20);
        assert!(
            hits.iter()
                .any(|hit| hit.path == Path::new("nested/space/needle.txt"))
        );
        assert_eq!(hits[0].path, Path::new("nested/space/needle.txt"));
        assert_eq!(hits[0].indices, (13..19).collect::<Vec<u32>>());
        assert!(
            find(dir.path(), "nsp", &defaults, &cancel, 20)
                .iter()
                .any(|hit| hit.path == Path::new("nested/space/needle.txt"))
        );
        assert!(!hits.iter().any(|hit| hit.path == Path::new(".hidden")));
        let all = SearchOptions {
            hidden: true,
            follow_gitignore: true,
            ..Default::default()
        };
        let all_hits = find(dir.path(), "", &all, &cancel, 100);
        assert!(all_hits.iter().any(|hit| hit.path == Path::new(".hidden")));
        let ignored = find(dir.path(), "ignored", &defaults, &cancel, 10);
        assert!(
            ignored
                .iter()
                .any(|hit| hit.path == Path::new("ignored.txt"))
        );
        assert!(
            !find(dir.path(), "ignored", &all, &cancel, 10)
                .iter()
                .any(|hit| hit.path == Path::new("ignored.txt"))
        );
        assert_eq!(find(dir.path(), "", &all, &cancel, 1).len(), 1);
        let stopped = AtomicBool::new(true);
        assert!(find(dir.path(), "needle", &all, &stopped, 10).is_empty());
    }

    #[test]
    fn grep_handles_case_binary_hidden_limits_and_truncation() {
        let dir = fixture();
        let cancel = AtomicBool::new(false);
        let defaults = SearchOptions::default();
        let hits = grep(dir.path(), "hello", &defaults, &cancel, 100).unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.path == Path::new("nested/space/needle.txt") && hit.line == 1)
        );
        assert!(!hits.iter().any(|hit| hit.path == Path::new("binary")));
        assert!(!hits.iter().any(|hit| hit.path == Path::new(".hidden")));
        let upper = grep(dir.path(), "HELLO", &defaults, &cancel, 100).unwrap();
        assert!(upper.iter().all(|hit| hit.text.contains("HELLO")));
        let hidden = SearchOptions {
            hidden: true,
            ..Default::default()
        };
        assert!(
            grep(dir.path(), "hello", &hidden, &cancel, 100)
                .unwrap()
                .iter()
                .any(|hit| hit.path == Path::new(".hidden"))
        );
        assert_eq!(
            grep(dir.path(), "hello", &defaults, &cancel, 1)
                .unwrap()
                .len(),
            1
        );
        let long = grep(dir.path(), "needle", &defaults, &cancel, 10).unwrap();
        assert!(
            long.iter()
                .any(|hit| hit.path == Path::new("long.txt") && hit.text.chars().count() == 200)
        );
        assert!(matches!(
            grep(dir.path(), "[", &defaults, &cancel, 10),
            Err(SearchError::BadPattern(_))
        ));
    }

    #[test]
    fn grep_line_numbers_anchors_binary_and_cancel() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "one\ntwo end\nthree\n").unwrap();
        std::fs::write(dir.path().join("bin.dat"), b"end\0\x01\x02 end\n").unwrap();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let opts = SearchOptions::default();
        let no = AtomicBool::new(false);
        let hits = grep(dir.path(), "end$", &opts, &no, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            (hits[0].path.as_path(), hits[0].line),
            (Path::new("a.txt"), 2)
        );
        assert!(grep(dir.path(), "^$", &opts, &no, 10).unwrap().is_empty());
        let yes = AtomicBool::new(true);
        assert!(grep(dir.path(), "o", &opts, &yes, 10).unwrap().is_empty());
    }
}
