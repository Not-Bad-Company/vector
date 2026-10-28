# git.rs — git status badges for the entries of one folder

File: `crates/vector-core/src/git.rs`. Crate available: `gix` 0.90 with features `status`,
`sha1` (default features off). Called from a worker thread.

```rust
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Badge {        // ordered by priority, highest last
    Untracked,
    Added,              // new file staged in the index
    Modified,           // changed in worktree or index (incl. deleted/renamed/type change)
    Conflicted,
}

/// Badges for the direct children of `dir`, keyed by child file name.
/// A child folder gets the highest-priority badge of anything changed below it.
/// Unchanged and ignored entries are absent. `None` when `dir` is not inside a git work tree
/// (or the repo can't be opened). `dir` may be the repo root or any folder inside it.
/// Inside `.git` itself → None.
pub fn badges(dir: &Path) -> Option<HashMap<String, Badge>>;
```

Use `gix::discover`, then the repo's status API (`repo.status(progress)?.into_iter(...)`, both
index-worktree and tree-index changes). Untracked dirs: report the dir as Untracked (git's
default "normal" collapse is fine). If gix 0.90 cannot do something here, say exactly what in
your report rather than shelling out to `git`.

## Tests (required)

Make repos in `tempfile::tempdir()` with the `git` CLI (`git init`, `-c user.name=t -c
user.email=t@e.invalid commit`): clean file (absent), modified, untracked, staged new (Added),
deleted file (Modified, keyed by its name even though it no longer exists), nested change makes
its parent folder Modified, priority aggregation, `badges` from a subfolder, non-repo → None.
