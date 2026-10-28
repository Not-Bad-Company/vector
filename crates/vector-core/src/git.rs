//! Git status badges for directory entries.

use std::collections::HashMap;
use std::path::Path;

use gix::bstr::ByteSlice;

/// Git status badge for an entry, ordered by priority (lowest to highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Badge {
    Untracked,
    Added,
    Modified,
    Conflicted,
}

/// The branch `dir` is on (or the short commit when detached); `None` outside a repo.
pub fn branch(dir: &Path) -> Option<String> {
    if dir.components().any(|c| c.as_os_str() == ".git") {
        return None;
    }
    let repo = gix::discover(dir).ok()?;
    match repo.head_name().ok()? {
        Some(name) => Some(name.shorten().to_string()),
        None => Some(repo.head_id().ok()?.shorten().ok()?.to_string()),
    }
}

/// Badges for the direct children of `dir`, keyed by child file name.
/// A child folder gets the highest-priority badge of anything changed below it.
/// Unchanged and ignored entries are absent. `None` when `dir` is not inside a git work tree
/// (or the repo can't be opened). `dir` may be the repo root or any folder inside it.
/// Inside `.git` itself → None.
pub fn badges(dir: &Path) -> Option<HashMap<String, Badge>> {
    let dir_canon = dir.canonicalize().ok()?;
    if !dir_canon.is_dir() {
        return None;
    }
    if dir.components().any(|c| c.as_os_str() == ".git")
        || dir_canon.components().any(|c| c.as_os_str() == ".git")
    {
        return None;
    }

    let repo = gix::discover(&dir_canon).ok()?;
    let work_dir = repo.workdir()?;
    let work_dir_canon = work_dir.canonicalize().ok()?;

    if !dir_canon.starts_with(&work_dir_canon) {
        return None;
    }

    if repo
        .git_dir()
        .canonicalize()
        .is_ok_and(|git_dir_canon| dir_canon.starts_with(&git_dir_canon))
    {
        return None;
    }

    let rel_dir = dir_canon.strip_prefix(&work_dir_canon).ok()?;

    let status_platform = repo.status(gix::progress::Discard).ok()?;
    let iter = status_platform.into_iter(None::<gix::bstr::BString>).ok()?;

    let mut result: HashMap<String, Badge> = HashMap::new();

    let mut record = |rela_str: &gix::bstr::BStr, badge: Badge| {
        let clean = rela_str.trim_end_with(|c| c == '/');
        let Ok(item_path) = gix::path::from_bstr(clean.as_bstr()) else {
            return;
        };
        let Ok(rel_to_dir) = item_path.strip_prefix(rel_dir) else {
            return;
        };
        if rel_to_dir.as_os_str().is_empty() {
            return;
        }
        let Some(first) = rel_to_dir.components().next() else {
            return;
        };
        let child_name = first.as_os_str().to_string_lossy().into_owned();
        result
            .entry(child_name)
            .and_modify(|b| *b = (*b).max(badge))
            .or_insert(badge);
    };

    for item in iter {
        let Ok(item) = item else {
            continue;
        };
        match item {
            gix::status::Item::IndexWorktree(worktree_item) => match worktree_item {
                gix::status::index_worktree::Item::Modification {
                    rela_path, status, ..
                } => match status {
                    gix::status::plumbing::index_as_worktree::EntryStatus::Conflict { .. } => {
                        record(rela_path.as_bstr(), Badge::Conflicted);
                    }
                    gix::status::plumbing::index_as_worktree::EntryStatus::Change(change) => {
                        match change {
                            gix::status::plumbing::index_as_worktree::Change::Removed
                            | gix::status::plumbing::index_as_worktree::Change::Type { .. }
                            | gix::status::plumbing::index_as_worktree::Change::Modification {
                                ..
                            }
                            | gix::status::plumbing::index_as_worktree::Change::SubmoduleModification(..) => {
                                record(rela_path.as_bstr(), Badge::Modified);
                            }
                        }
                    }
                    gix::status::plumbing::index_as_worktree::EntryStatus::IntentToAdd => {
                        record(rela_path.as_bstr(), Badge::Modified);
                    }
                    gix::status::plumbing::index_as_worktree::EntryStatus::NeedsUpdate(_) => {}
                },
                gix::status::index_worktree::Item::DirectoryContents { entry, .. } => {
                    if let gix::dir::entry::Status::Untracked = entry.status {
                        record(entry.rela_path.as_bstr(), Badge::Untracked);
                    }
                }
                gix::status::index_worktree::Item::Rewrite {
                    source,
                    dirwalk_entry,
                    ..
                } => {
                    match source {
                        gix::status::index_worktree::RewriteSource::RewriteFromIndex {
                            source_rela_path,
                            ..
                        } => {
                            record(source_rela_path.as_bstr(), Badge::Modified);
                        }
                        gix::status::index_worktree::RewriteSource::CopyFromDirectoryEntry {
                            ..
                        } => {}
                    }
                    record(dirwalk_entry.rela_path.as_bstr(), Badge::Modified);
                }
            },
            gix::status::Item::TreeIndex(change) => match change {
                gix::diff::index::Change::Addition { location, .. } => {
                    record(location.as_bstr(), Badge::Added);
                }
                gix::diff::index::Change::Deletion { location, .. } => {
                    record(location.as_bstr(), Badge::Modified);
                }
                gix::diff::index::Change::Modification { location, .. } => {
                    record(location.as_bstr(), Badge::Modified);
                }
                gix::diff::index::Change::Rewrite {
                    source_location,
                    location,
                    copy,
                    ..
                } => {
                    if copy {
                        record(location.as_bstr(), Badge::Added);
                    } else {
                        record(source_location.as_bstr(), Badge::Modified);
                        record(location.as_bstr(), Badge::Modified);
                    }
                }
            },
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn run_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("failed to execute git command");
        assert!(
            output.status.success(),
            "git command {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        run_git(dir.path(), &["init"]);
        run_git(dir.path(), &["config", "user.name", "t"]);
        run_git(dir.path(), &["config", "user.email", "t@e.invalid"]);
        dir
    }

    #[test]
    fn test_non_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(badges(dir.path()), None);
    }

    #[test]
    fn test_inside_git_dir() {
        let dir = init_repo();
        assert_eq!(badges(&dir.path().join(".git")), None);
    }

    #[test]
    fn test_clean_file_absent() {
        let dir = init_repo();
        fs::write(dir.path().join("clean.txt"), "hello").expect("write");
        run_git(dir.path(), &["add", "clean.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        let res = badges(dir.path()).expect("badges");
        assert!(!res.contains_key("clean.txt"));
    }

    #[test]
    fn test_modified() {
        let dir = init_repo();
        fs::write(dir.path().join("file.txt"), "hello").expect("write");
        run_git(dir.path(), &["add", "file.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        fs::write(dir.path().join("file.txt"), "world").expect("write");
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("file.txt"), Some(&Badge::Modified));
    }

    #[test]
    fn test_untracked() {
        let dir = init_repo();
        fs::write(dir.path().join("untracked.txt"), "hello").expect("write");
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("untracked.txt"), Some(&Badge::Untracked));
    }

    #[test]
    fn test_untracked_dir() {
        let dir = init_repo();
        let untracked_sub = dir.path().join("untracked_dir");
        fs::create_dir_all(&untracked_sub).expect("create_dir");
        fs::write(untracked_sub.join("file.txt"), "hello").expect("write");

        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("untracked_dir"), Some(&Badge::Untracked));
    }

    #[test]
    fn test_staged_new_added() {
        let dir = init_repo();
        fs::write(dir.path().join("new.txt"), "hello").expect("write");
        run_git(dir.path(), &["add", "new.txt"]);
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("new.txt"), Some(&Badge::Added));
    }

    #[test]
    fn test_deleted_file_modified() {
        let dir = init_repo();
        fs::write(dir.path().join("deleted.txt"), "hello").expect("write");
        run_git(dir.path(), &["add", "deleted.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        fs::remove_file(dir.path().join("deleted.txt")).expect("remove");
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("deleted.txt"), Some(&Badge::Modified));
    }

    #[test]
    fn test_nested_change_makes_parent_folder_modified() {
        let dir = init_repo();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).expect("create_dir");
        fs::write(sub.join("nested.txt"), "hello").expect("write");
        run_git(dir.path(), &["add", "sub/nested.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        fs::write(sub.join("nested.txt"), "world").expect("write");
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("sub"), Some(&Badge::Modified));
    }

    #[test]
    fn test_priority_aggregation() {
        let dir = init_repo();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).expect("create_dir");

        fs::write(sub.join("f1.txt"), "1").expect("write");
        run_git(dir.path(), &["add", "sub/f1.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        // f1 is modified
        fs::write(sub.join("f1.txt"), "1-mod").expect("write");
        // f2 is staged new (Added)
        fs::write(sub.join("f2.txt"), "2").expect("write");
        run_git(dir.path(), &["add", "sub/f2.txt"]);
        // f3 is untracked
        fs::write(sub.join("f3.txt"), "3").expect("write");

        // sub has Modified, Added, Untracked. Priority highest: Modified
        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("sub"), Some(&Badge::Modified));

        // Another folder with only Untracked and Added -> Added
        let sub2 = dir.path().join("sub2");
        fs::create_dir_all(&sub2).expect("create_dir");
        fs::write(sub2.join("u.txt"), "u").expect("write");
        fs::write(sub2.join("a.txt"), "a").expect("write");
        run_git(dir.path(), &["add", "sub2/a.txt"]);

        let res2 = badges(dir.path()).expect("badges");
        assert_eq!(res2.get("sub2"), Some(&Badge::Added));
    }

    #[test]
    fn test_badges_from_subfolder() {
        let dir = init_repo();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).expect("create_dir");

        fs::write(sub.join("m.txt"), "m").expect("write");
        run_git(dir.path(), &["add", "sub/m.txt"]);
        run_git(dir.path(), &["commit", "-m", "init"]);

        fs::write(sub.join("m.txt"), "m2").expect("write");
        fs::write(sub.join("u.txt"), "u").expect("write");

        let res = badges(&sub).expect("badges");
        assert_eq!(res.get("m.txt"), Some(&Badge::Modified));
        assert_eq!(res.get("u.txt"), Some(&Badge::Untracked));
    }

    #[test]
    fn test_conflicted() {
        let dir = init_repo();
        let f = dir.path().join("conf.txt");
        fs::write(&f, "base\n").expect("write");
        run_git(dir.path(), &["add", "conf.txt"]);
        run_git(dir.path(), &["commit", "-m", "base"]);

        run_git(dir.path(), &["checkout", "-b", "feature"]);
        fs::write(&f, "feature\n").expect("write");
        run_git(dir.path(), &["commit", "-am", "feature commit"]);

        run_git(dir.path(), &["checkout", "HEAD~1"]);
        fs::write(&f, "main change\n").expect("write");
        run_git(dir.path(), &["commit", "-am", "other commit"]);

        // Merge feature branch to cause conflict
        let _ = Command::new("git")
            .args(["merge", "feature"])
            .current_dir(dir.path())
            .output();

        let res = badges(dir.path()).expect("badges");
        assert_eq!(res.get("conf.txt"), Some(&Badge::Conflicted));
    }
}
