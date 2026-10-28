//! Logic for turning an edited directory listing into file operations.

use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: usize,
    pub name: String,
    pub is_dir: bool,
}

/// Renders the editable buffer for a directory listing.
pub fn render(dir_display: &str, entries: &[Listed]) -> String {
    let width = entries
        .iter()
        .map(|entry| entry.id.to_string().len())
        .max()
        .unwrap_or(3)
        .max(3);
    let mut output = format!("# {dir_display}\n");
    for entry in entries {
        if entry.name.contains(['\n', '\r']) {
            continue;
        }
        let suffix = if entry.is_dir { "/" } else { "" };
        output.push_str(&format!("/{:0width$} {}{}\n", entry.id, entry.name, suffix));
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Moves an entry to the trash.
    Delete { name: String },
    /// Creates an entry relative to the edited directory.
    Create { path: String, dir: bool },
    /// Renames or moves an entry relative to the edited directory.
    Rename { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
}

impl Plan {
    /// Returns whether the plan contains no operations.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownId {
        line: usize,
        id: usize,
    },
    DuplicateId {
        id: usize,
        lines: Vec<usize>,
    },
    InvalidName {
        line: usize,
        name: String,
        reason: &'static str,
    },
    TypeChanged {
        line: usize,
        name: String,
    },
    TargetTaken {
        line: usize,
        name: String,
    },
    SameTarget {
        name: String,
        lines: Vec<usize>,
    },
    Malformed {
        line: usize,
        text: String,
    },
    /// A move into a folder that does not exist and is not created by this edit.
    NoParent {
        line: usize,
        name: String,
    },
    /// A move into a folder name that is both an old and a new name in this edit.
    AmbiguousParent {
        line: usize,
        name: String,
    },
    /// Renames that depend on each other in a way that can't be ordered safely.
    Unschedulable {
        line: usize,
    },
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownId { line, id } => write!(f, "line {line}: unknown id {id}"),
            Self::DuplicateId { id, lines } => {
                write!(f, "lines {}: duplicate id {id}", format_lines(lines))
            }
            Self::InvalidName { line, name, reason } => {
                write!(f, "line {line}: invalid name {name:?}: {reason}")
            }
            Self::TypeChanged { line, name } => {
                write!(f, "line {line}: type changed for {name:?}")
            }
            Self::TargetTaken { line, name } => {
                write!(f, "line {line}: target is taken: {name:?}")
            }
            Self::SameTarget { name, lines } => {
                write!(f, "lines {}: same target {name:?}", format_lines(lines))
            }
            Self::Malformed { line, text } => write!(f, "line {line}: malformed entry {text:?}"),
            Self::NoParent { line, name } => write!(f, "line {line}: no folder {name:?}"),
            Self::AmbiguousParent { line, name } => {
                write!(
                    f,
                    "line {line}: {name:?} is both an old and a new folder name"
                )
            }
            Self::Unschedulable { line } => {
                write!(f, "line {line}: these renames can't be ordered safely")
            }
        }
    }
}

impl std::error::Error for EditError {}

fn format_lines(lines: &[usize]) -> String {
    lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone)]
struct EditedEntry {
    id: Option<usize>,
    line: usize,
    name: String,
    dir: bool,
}

#[derive(Clone)]
struct RenameEntry {
    from: String,
    to: String,
    dir: bool,
    line: usize,
}

fn validate_name(line: usize, name: &str) -> Result<(), EditError> {
    let reason = if name.is_empty() {
        Some("empty")
    } else if name.starts_with('/') {
        Some("absolute path")
    } else if name.contains('\0') {
        Some("contains NUL")
    } else if name.split('/').any(|part| part.is_empty()) {
        Some("empty path component")
    } else if name.split('/').any(|part| part == "." || part == "..") {
        Some("dot path component")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(EditError::InvalidName {
            line,
            name: name.to_owned(),
            reason,
        });
    }
    Ok(())
}

fn renderable(entry: &Listed) -> bool {
    !entry.name.contains(['\n', '\r'])
}

fn strip_dir_marker(name: &str) -> (&str, bool) {
    if let Some(path) = name.strip_suffix('/') {
        (path, true)
    } else {
        (name, false)
    }
}

fn rewrite_renamed_parent(target: &str, dirs: &[RenameEntry]) -> String {
    let mut result = target.to_owned();
    let mut ordered = dirs.to_vec();
    ordered.sort_by_key(|entry| std::cmp::Reverse(entry.from.len()));
    for entry in ordered {
        if result == entry.from {
            result = entry.to.clone();
        } else if let Some(rest) = result.strip_prefix(&format!("{}/", entry.from)) {
            result = format!("{}/{rest}", entry.to);
        }
    }
    result
}

/// Parses an edited buffer and builds a safe, ordered operation plan.
pub fn plan(original: &[Listed], edited: &str) -> Result<Plan, EditError> {
    let mut originals = HashMap::new();
    let mut original_names = HashSet::new();
    for entry in original.iter().filter(|entry| renderable(entry)) {
        originals.insert(entry.id, entry);
        original_names.insert(entry.name.clone());
    }

    let mut parsed = Vec::new();
    let mut mentioned: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, raw) in edited.lines().enumerate() {
        let line = index + 1;
        let text = raw.strip_suffix('\r').unwrap_or(raw);
        if text.trim().is_empty() || text.starts_with('#') {
            continue;
        }
        if text.starts_with('/') {
            let Some(space) = text.find(' ') else {
                return Err(EditError::Malformed {
                    line,
                    text: text.to_owned(),
                });
            };
            let digits = &text[1..space];
            if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(EditError::Malformed {
                    line,
                    text: text.to_owned(),
                });
            }
            let Ok(id) = digits.parse::<usize>() else {
                return Err(EditError::Malformed {
                    line,
                    text: text.to_owned(),
                });
            };
            let name = &text[space + 1..];
            let Some(original_entry) = originals.get(&id) else {
                return Err(EditError::UnknownId { line, id });
            };
            mentioned.entry(id).or_default().push(line);
            let (path, dir) = strip_dir_marker(name);
            if dir != original_entry.is_dir {
                return Err(EditError::TypeChanged {
                    line,
                    name: name.to_owned(),
                });
            }
            validate_name(line, path)?;
            parsed.push(EditedEntry {
                id: Some(id),
                line,
                name: path.to_owned(),
                dir,
            });
        } else {
            let (path, dir) = strip_dir_marker(text);
            validate_name(line, path)?;
            parsed.push(EditedEntry {
                id: None,
                line,
                name: path.to_owned(),
                dir,
            });
        }
    }

    if let Some((id, lines)) = mentioned.iter().find(|(_, lines)| lines.len() > 1) {
        return Err(EditError::DuplicateId {
            id: *id,
            lines: lines.clone(),
        });
    }

    let mut deletes = Vec::new();
    let mut renames = Vec::new();
    let mut creates = Vec::new();
    let mut destinations: HashMap<String, Vec<usize>> = HashMap::new();
    let dir_renames: Vec<RenameEntry> = parsed
        .iter()
        .filter_map(|entry| {
            let id = entry.id?;
            let old = originals.get(&id)?;
            (entry.dir && entry.name != old.name).then(|| RenameEntry {
                from: old.name.clone(),
                to: entry.name.clone(),
                dir: true,
                line: entry.line,
            })
        })
        .collect();

    for entry in &parsed {
        match entry.id {
            Some(id) => {
                let old = originals[&id];
                if entry.name != old.name {
                    let to = if entry.name.contains('/') {
                        let top = entry.name.split('/').next().unwrap_or("");
                        let was = dir_renames.iter().any(|d| d.from == top);
                        let top_slash = format!("{top}/");
                        let becomes = dir_renames.iter().any(|d| d.to == top)
                            || parsed.iter().any(|e| {
                                e.id.is_none() && (e.name == top || e.name.starts_with(&top_slash))
                            });
                        if was && becomes {
                            return Err(EditError::AmbiguousParent {
                                line: entry.line,
                                name: top.to_owned(),
                            });
                        }
                        rewrite_renamed_parent(&entry.name, &dir_renames)
                    } else {
                        entry.name.clone()
                    };
                    destinations.entry(to.clone()).or_default().push(entry.line);
                    renames.push(RenameEntry {
                        from: old.name.clone(),
                        to,
                        dir: entry.dir,
                        line: entry.line,
                    });
                }
            }
            None => {
                destinations
                    .entry(entry.name.clone())
                    .or_default()
                    .push(entry.line);
                creates.push(entry.clone());
            }
        }
    }
    for entry in original.iter().filter(|entry| renderable(entry)) {
        if !mentioned.contains_key(&entry.id) {
            deletes.push(entry.name.clone());
        }
    }

    for (name, lines) in &destinations {
        if lines.len() > 1 {
            return Err(EditError::SameTarget {
                name: name.clone(),
                lines: lines.clone(),
            });
        }
    }

    let freed: HashSet<String> = deletes
        .iter()
        .cloned()
        .chain(renames.iter().map(|rename| rename.from.clone()))
        .collect();

    // Directory creates include missing parents. Existing final directories satisfy parents.
    let mut final_dirs = HashSet::new();
    let mut dir_creates: BTreeMap<String, usize> = BTreeMap::new();
    let mut file_creates = Vec::new();
    for entry in &creates {
        if entry.dir {
            let components: Vec<&str> = entry.name.split('/').collect();
            for end in 1..=components.len() {
                let path = components[..end].join("/");
                if !original_names.contains(&path) || freed.contains(&path) {
                    dir_creates.entry(path.clone()).or_insert(entry.line);
                }
            }
        } else {
            let components: Vec<&str> = entry.name.split('/').collect();
            for end in 1..components.len() {
                let path = components[..end].join("/");
                if !original_names.contains(&path) || freed.contains(&path) {
                    dir_creates.entry(path).or_insert(entry.line);
                }
            }
            file_creates.push(entry.clone());
        }
    }

    for (path, line) in &dir_creates {
        let explicit_create = creates.iter().any(|entry| entry.dir && entry.name == *path);
        if !explicit_create {
            destinations.entry(path.clone()).or_default().push(*line);
        }
    }
    for (name, lines) in &destinations {
        if lines.len() > 1 {
            return Err(EditError::SameTarget {
                name: name.clone(),
                lines: lines.clone(),
            });
        }
    }

    for entry in original
        .iter()
        .filter(|entry| entry.is_dir && renderable(entry))
    {
        if !freed.contains(&entry.name) {
            final_dirs.insert(entry.name.clone());
        }
    }
    for rename in renames.iter().filter(|rename| rename.dir) {
        final_dirs.insert(rename.to.clone());
    }
    for path in dir_creates.keys() {
        final_dirs.insert(path.clone());
    }

    // Rewrites are applied before parent checks; every target path must stay relative.
    for rename in &renames {
        if let Some(parent) = rename.to.rsplit_once('/').map(|(parent, _)| parent)
            && !final_dirs.contains(parent)
        {
            return Err(EditError::NoParent {
                line: rename.line,
                name: parent.to_owned(),
            });
        }
    }
    for entry in &creates {
        if let Some(parent) = entry.name.rsplit_once('/').map(|(parent, _)| parent)
            && !final_dirs.contains(parent)
        {
            return Err(EditError::NoParent {
                line: entry.line,
                name: parent.to_owned(),
            });
        }
    }

    // Check target occupancy against entries that survive at their original path.
    for rename in &renames {
        if original_names.contains(&rename.to) && !freed.contains(&rename.to) {
            return Err(EditError::TargetTaken {
                line: rename.line,
                name: rename.to.clone(),
            });
        }
    }
    for entry in &creates {
        if original_names.contains(&entry.name) && !freed.contains(&entry.name) {
            return Err(EditError::TargetTaken {
                line: entry.line,
                name: entry.name.clone(),
            });
        }
    }
    // A newly created directory may serve as a parent, but cannot replace a kept file.
    for (path, line) in &dir_creates {
        if original_names.contains(path) && !freed.contains(path) {
            let is_existing_dir = original
                .iter()
                .any(|entry| entry.name == *path && entry.is_dir);
            if !is_existing_dir {
                return Err(EditError::TargetTaken {
                    line: *line,
                    name: path.clone(),
                });
            }
        }
    }
    let mut changes = deletes
        .into_iter()
        .map(|name| Change::Delete { name })
        .collect::<Vec<_>>();
    let mut sorted_dirs: Vec<String> = dir_creates.into_keys().collect();
    sorted_dirs.sort_by(|a, b| (a.matches('/').count(), a).cmp(&(b.matches('/').count(), b)));
    changes.extend(schedule(renames, sorted_dirs, &original_names)?);
    file_creates.sort_by_key(|entry| entry.line);
    changes.extend(file_creates.into_iter().map(|entry| Change::Create {
        path: entry.name,
        dir: false,
    }));

    // Reject a destination that is occupied by an unfreed original, including creates.
    for (target, lines) in destinations {
        if original_names.contains(&target) && !freed.contains(&target) {
            let is_existing_parent = final_dirs.contains(&target) && !target.is_empty();
            if !is_existing_parent {
                return Err(EditError::TargetTaken {
                    line: lines[0],
                    name: target,
                });
            }
        }
    }
    Ok(Plan { changes })
}

enum Step {
    Rename(RenameEntry),
    Mkdir(String),
}

fn parent_of(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

/// Orders renames and folder creations so that none lands on a name that is still taken,
/// nothing goes into a folder before that folder exists under its final name, and a new folder
/// waits for the rename that frees its name. Cycles (swaps, rotations) go through a temporary
/// name.
fn schedule(
    renames: Vec<RenameEntry>,
    mkdirs: Vec<String>,
    original_names: &HashSet<String>,
) -> Result<Vec<Change>, EditError> {
    let mut used: HashSet<String> = renames.iter().map(|e| e.to.clone()).collect();
    used.extend(mkdirs.iter().cloned());
    let mut pending: Vec<Step> = mkdirs.into_iter().map(Step::Mkdir).collect();
    pending.extend(renames.into_iter().map(Step::Rename));
    let mut output = Vec::new();
    let mut temp_counter = 1usize;
    while !pending.is_empty() {
        let freed_later = |name: &str| {
            pending
                .iter()
                .any(|o| matches!(o, Step::Rename(r) if r.from == name))
        };
        let parent_pending = |parent: &str, me: Option<&str>| {
            pending.iter().any(|o| match o {
                Step::Mkdir(p) => p == parent,
                Step::Rename(r) => r.to == parent && Some(r.from.as_str()) != me,
            })
        };
        let ready = pending.iter().position(|step| match step {
            Step::Rename(e) => {
                !freed_later(&e.to)
                    && !parent_of(&e.to).is_some_and(|p| parent_pending(p, Some(&e.from)))
            }
            Step::Mkdir(path) => {
                !freed_later(path) && !parent_of(path).is_some_and(|p| parent_pending(p, None))
            }
        });
        if let Some(index) = ready {
            output.push(match pending.remove(index) {
                Step::Rename(e) => Change::Rename {
                    from: e.from,
                    to: e.to,
                },
                Step::Mkdir(path) => Change::Create { path, dir: true },
            });
            continue;
        }
        // stuck: move a rename's source out of the way of whoever waits for its name
        let waiting_for = |name: &str| {
            pending.iter().any(|o| match o {
                Step::Mkdir(p) => p == name,
                Step::Rename(r) => r.to == name,
            })
        };
        let Some(index) = pending
            .iter()
            .position(|step| matches!(step, Step::Rename(e) if waiting_for(&e.from)))
        else {
            let line = pending.iter().find_map(|step| match step {
                Step::Rename(e) => Some(e.line),
                Step::Mkdir(_) => None,
            });
            return Err(EditError::Unschedulable {
                line: line.unwrap_or(0),
            });
        };
        let temp = loop {
            let candidate = format!(".strata-tmp-{temp_counter}");
            temp_counter += 1;
            if !original_names.contains(&candidate) && !used.contains(&candidate) {
                break candidate;
            }
        };
        used.insert(temp.clone());
        if let Step::Rename(entry) = &mut pending[index] {
            output.push(Change::Rename {
                from: entry.from.clone(),
                to: temp.clone(),
            });
            entry.from = temp;
        }
    }
    Ok(output)
}

/// Formats plan operations for a confirmation list.
pub fn describe(plan: &Plan) -> Vec<String> {
    plan.changes
        .iter()
        .map(|change| match change {
            Change::Delete { name } => format!("trash   {name}"),
            Change::Create { path, dir } => {
                format!("create  {}{}", path, if *dir { "/" } else { "" })
            }
            Change::Rename { from, to } => format!(
                "{}  {from} → {to}",
                if from.contains('/') || to.contains('/') {
                    "move  "
                } else {
                    "rename"
                }
            ),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn file(id: usize, name: &str) -> Listed {
        Listed {
            id,
            name: name.to_owned(),
            is_dir: false,
        }
    }

    fn dir(id: usize, name: &str) -> Listed {
        Listed {
            id,
            name: name.to_owned(),
            is_dir: true,
        }
    }

    fn set(changes: &[Change], initial: &[&str]) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = initial.iter().map(|name| (*name).to_owned()).collect();
        for change in changes {
            match change {
                Change::Delete { name } => {
                    names.remove(name);
                }
                Change::Create { path, .. } => {
                    assert!(!names.contains(path), "create over {path:?}");
                    if let Some((parent, _)) = path.rsplit_once('/') {
                        assert!(names.contains(parent), "no parent for {path:?}");
                    }
                    names.insert(path.clone());
                }
                Change::Rename { from, to } => {
                    // the executor never overwrites, so neither may the plan
                    assert!(
                        !names.contains(to),
                        "rename onto existing {to:?} in {names:?}"
                    );
                    if let Some((parent, _)) = to.rsplit_once('/') {
                        assert!(names.contains(parent), "no parent for {to:?} in {names:?}");
                    }
                    assert!(names.remove(from), "missing source {from:?} in {names:?}");
                    let prefix = format!("{from}/");
                    let children: Vec<String> = names
                        .iter()
                        .filter(|n| n.starts_with(&prefix))
                        .cloned()
                        .collect();
                    for child in children {
                        names.remove(&child);
                        names.insert(format!("{to}/{}", &child[prefix.len()..]));
                    }
                    names.insert(to.clone());
                }
            }
        }
        names
    }

    #[test]
    fn render_format_and_skip_newline_names() {
        let entries = [
            file(2, "a"),
            dir(1000, "folder"),
            file(4, "line\nbreak"),
            file(5, "cr\rname"),
        ];
        assert_eq!(
            render("/work", &entries),
            "# /work\n/0002 a\n/1000 folder/\n"
        );
    }

    #[test]
    fn no_op_and_comments_blanks_are_ignored() {
        let entries = [file(1, "a.txt")];
        let result = plan(&entries, "# header\n\n   \n/001 a.txt\n").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn rename_and_delete() {
        let entries = [file(1, "a"), file(2, "gone")];
        assert_eq!(
            plan(&entries, "/001 b\n").unwrap().changes,
            vec![
                Change::Delete {
                    name: "gone".into()
                },
                Change::Rename {
                    from: "a".into(),
                    to: "b".into()
                }
            ]
        );
    }

    #[test]
    fn creates_files_directories_and_nested_parents() {
        let result = plan(&[], "notes/\nx/y/z.txt\n").unwrap();
        assert_eq!(
            result.changes,
            vec![
                Change::Create {
                    path: "notes".into(),
                    dir: true
                },
                Change::Create {
                    path: "x".into(),
                    dir: true
                },
                Change::Create {
                    path: "x/y".into(),
                    dir: true
                },
                Change::Create {
                    path: "x/y/z.txt".into(),
                    dir: false
                },
            ]
        );
    }

    #[test]
    fn move_into_existing_folder() {
        let entries = [file(1, "a"), dir(2, "sub")];
        assert_eq!(
            plan(&entries, "/001 sub/a\n/002 sub/\n").unwrap().changes,
            vec![Change::Rename {
                from: "a".into(),
                to: "sub/a".into()
            }]
        );
    }

    #[test]
    fn move_into_folder_created_in_same_edit() {
        let entries = [file(1, "a")];
        assert_eq!(
            plan(&entries, "/001 new/a\nnew/\n").unwrap().changes,
            vec![
                Change::Create {
                    path: "new".into(),
                    dir: true
                },
                Change::Rename {
                    from: "a".into(),
                    to: "new/a".into()
                },
            ]
        );
    }

    #[test]
    fn moving_into_renamed_folder_uses_final_path() {
        let entries = [dir(1, "old"), file(2, "loose")];
        assert_eq!(
            plan(&entries, "/002 old/loose\n/001 new/\n")
                .unwrap()
                .changes,
            vec![
                Change::Rename {
                    from: "old".into(),
                    to: "new".into()
                },
                Change::Rename {
                    from: "loose".into(),
                    to: "new/loose".into()
                },
            ]
        );
    }

    #[test]
    fn swap_and_rotation_use_temporary_names() {
        let entries = [file(1, "a"), file(2, "b")];
        let swap = plan(&entries, "/001 b\n/002 a\n").unwrap();
        assert_eq!(
            set(&swap.changes, &["a", "b"]),
            BTreeSet::from(["a".into(), "b".into()])
        );
        assert_eq!(swap.changes.len(), 3);

        let entries = [file(1, "a"), file(2, "b"), file(3, "c")];
        let rotation = plan(&entries, "/001 b\n/002 c\n/003 a\n").unwrap();
        assert_eq!(
            set(&rotation.changes, &["a", "b", "c"]),
            BTreeSet::from(["a".into(), "b".into(), "c".into()])
        );
        assert_eq!(rotation.changes.len(), 4);
    }

    #[test]
    fn file_and_folder_trading_names_are_ordered() {
        let entries = [file(1, "a"), dir(2, "b")];
        let p = plan(&entries, "/001 x\n/002 a/\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a", "b"]),
            BTreeSet::from(["a".into(), "x".into()])
        );
        let p = plan(&entries, "/001 b\n/002 a/\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a", "b"]),
            BTreeSet::from(["a".into(), "b".into()])
        );
    }

    #[test]
    fn move_into_swapped_folder_waits_for_the_swap() {
        let entries = [dir(1, "a"), dir(2, "b"), file(3, "f"), file(4, "g")];
        // "a/f" could mean the old or the new "a": abort instead of guessing
        assert!(matches!(
            plan(&entries, "/001 b/\n/002 a/\n/003 a/f\n/004 b/g\n"),
            Err(EditError::AmbiguousParent { line: 3, .. })
        ));
        // into a folder by its final name, while the swap needs a temporary name
        let entries = [dir(1, "a"), dir(2, "b"), file(3, "f"), dir(4, "c")];
        let p = plan(&entries, "/001 b/\n/002 a/\n/003 c/f\n/004 c/\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a", "b", "c", "f"]),
            BTreeSet::from(["a".into(), "b".into(), "c".into(), "c/f".into()])
        );
        // into a renamed folder by its final name
        let entries = [dir(1, "a"), file(2, "f")];
        let p = plan(&entries, "/001 z/\n/002 z/f\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a", "f"]),
            BTreeSet::from(["z".into(), "z/f".into()])
        );
    }

    #[test]
    fn move_into_missing_folder_is_no_parent() {
        let entries = [file(1, "f")];
        assert!(matches!(
            plan(&entries, "/001 nope/f\n"),
            Err(EditError::NoParent { line: 1, .. })
        ));
    }

    #[test]
    fn rename_onto_deleted_name_is_allowed() {
        let entries = [file(1, "a"), file(2, "b")];
        let result = plan(&entries, "/001 b\n").unwrap();
        assert_eq!(
            result.changes,
            vec![
                Change::Delete { name: "b".into() },
                Change::Rename {
                    from: "a".into(),
                    to: "b".into()
                }
            ]
        );
    }

    #[test]
    fn rejects_kept_target_duplicate_target_and_duplicate_id() {
        let entries = [file(1, "a"), file(2, "b"), file(3, "c")];
        assert!(matches!(
            plan(&entries, "/001 b\n/002 b\n/003 c\n"),
            Err(EditError::TargetTaken { .. })
        ));
        assert!(matches!(
            plan(&entries, "/001 x\n/002 x\n/003 c\n"),
            Err(EditError::SameTarget { .. })
        ));
        assert!(matches!(
            plan(&entries, "/001 x\nx/y\n/003 c\n"),
            Err(EditError::SameTarget { .. })
        ));
        assert!(matches!(
            plan(&entries, "/001 x\n/001 y\n"),
            Err(EditError::DuplicateId { .. })
        ));
    }

    #[test]
    fn rejects_unknown_malformed_invalid_and_type_change() {
        let entries = [file(1, "a"), dir(2, "folder")];
        assert!(matches!(
            plan(&entries, "/099 unknown\n"),
            Err(EditError::UnknownId { line: 1, id: 99 })
        ));
        assert!(matches!(
            plan(&entries, "/nope x\n"),
            Err(EditError::Malformed { line: 1, .. })
        ));
        assert!(matches!(
            plan(&entries, "../escape\n"),
            Err(EditError::InvalidName { .. })
        ));
        assert!(matches!(
            plan(&entries, "/001 a/\n"),
            Err(EditError::TypeChanged { .. })
        ));
        assert!(matches!(
            plan(&entries, "/002 folder\n"),
            Err(EditError::TypeChanged { .. })
        ));
        assert!(matches!(
            plan(&[file(1, "a")], "/001 \n"),
            Err(EditError::InvalidName { .. })
        ));
        for invalid in [".", "..", "a//b", "a/../b", "nul\0name"] {
            assert!(
                matches!(plan(&[], invalid), Err(EditError::InvalidName { .. })),
                "{invalid:?}"
            );
        }
        assert!(matches!(
            plan(&[file(1, "a")], "/001 /absolute\n"),
            Err(EditError::InvalidName { .. })
        ));
    }

    #[test]
    fn describe_uses_confirmation_labels() {
        let p = Plan {
            changes: vec![
                Change::Rename {
                    from: "a.txt".into(),
                    to: "b.txt".into(),
                },
                Change::Rename {
                    from: "a.txt".into(),
                    to: "sub/a.txt".into(),
                },
                Change::Delete {
                    name: "old.log".into(),
                },
                Change::Create {
                    path: "notes".into(),
                    dir: true,
                },
                Change::Create {
                    path: "x/y.txt".into(),
                    dir: false,
                },
            ],
        };
        assert_eq!(
            describe(&p),
            vec![
                "rename  a.txt → b.txt",
                "move    a.txt → sub/a.txt",
                "trash   old.log",
                "create  notes/",
                "create  x/y.txt"
            ]
        );
    }

    #[test]
    fn new_folder_waits_for_the_rename_that_frees_its_name() {
        let entries = [dir(1, "a"), file(2, "f")];
        let p = plan(&entries, "/001 b/\na/\na/x\n/002 f\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a", "f"]),
            BTreeSet::from(["a".into(), "a/x".into(), "b".into(), "f".into()])
        );
        // "a/f": the old a (now b) or the new a? abort
        assert!(matches!(
            plan(&entries, "/001 b/\na/\n/002 a/f\n"),
            Err(EditError::AmbiguousParent { line: 3, .. })
        ));
        let entries = [file(1, "a")];
        let p = plan(&entries, "/001 b\na/\n").unwrap();
        assert_eq!(
            set(&p.changes, &["a"]),
            BTreeSet::from(["a".into(), "b".into()])
        );
    }
}
