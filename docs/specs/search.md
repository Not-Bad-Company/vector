# search.rs — `:find` (fuzzy, recursive, by name) and `:grep` (content)

File: `crates/vector-core/src/search.rs`. Crates already available: `nucleo` 0.5 (re-exports
`nucleo_matcher` as `nucleo::Matcher`, `nucleo::pattern`, `nucleo::Utf32Str`), `ignore`,
`grep-searcher`, `grep-regex`, `grep-matcher`. Called from a worker thread by the app, so plain
blocking functions are right; they must stop promptly when `cancel` is set.

```rust
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    pub hidden: bool,          // include dot files/dirs
    pub follow_gitignore: bool,// honour .gitignore/.ignore (default false)
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindHit {
    pub path: PathBuf,         // relative to root
    pub is_dir: bool,
    pub score: u32,
    pub indices: Vec<u32>,     // char indices in path.to_string_lossy() that matched, sorted
}

/// Fuzzy-match every file and folder name under `root` (not root itself) against `query`.
/// Matching is done against the relative path with nucleo's path config; smart case.
/// Best `limit` hits, score descending, ties: shorter path, then path order.
/// Does not follow symlinks. Unreadable dirs are skipped silently.
pub fn find(root: &Path, query: &str, opts: &SearchOptions, cancel: &AtomicBool, limit: usize) -> Vec<FindHit>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    pub path: PathBuf,   // relative to root
    pub line: u64,       // 1-based
    pub text: String,    // the line, trimmed, cut to 200 chars (char boundary safe)
}

#[derive(Debug)]
pub enum SearchError { BadPattern(String) }
impl std::fmt::Display for SearchError

/// Regex search of file contents under `root`. Smart case (case-insensitive unless the pattern
/// has an uppercase char). Binary files skipped. Files over 50 MB skipped. Hits sorted by path
/// then line, at most `limit`.
pub fn grep(root: &Path, pattern: &str, opts: &SearchOptions, cancel: &AtomicBool, limit: usize) -> Result<Vec<GrepHit>, SearchError>;
```

Use `ignore::WalkBuilder` (parallel walker is fine) with `hidden(!opts.hidden)`,
`git_ignore/ignore/git_exclude(opts.follow_gitignore)`, `parents(false)`, `follow_links(false)`.

## Tests (required)

Build a tree in `tempfile::tempdir()`: nested dirs, hidden file, a `.gitignore`d file, a binary
file (contains NUL bytes), unicode names. find: fuzzy match across path segments, ranking (exact
name beats scattered), hidden toggle, gitignore toggle, limit, cancel pre-set returns empty,
indices are correct. grep: hits with line numbers, smart case, bad regex error, binary skipped,
hidden toggle, limit, long line cut at 200 chars.
