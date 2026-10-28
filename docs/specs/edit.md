# edit.rs — `:edit`, oil.nvim style: a folder as a text buffer, diffed into file operations

File: `crates/vector-core/src/edit.rs`. Pure logic, no I/O. The app writes the buffer to a temp
file, opens it in nvim, reads it back after nvim exits, calls `plan`, shows `describe` in a
confirmation dialog, and only then executes the changes.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed { pub id: usize, pub name: String, pub is_dir: bool }

/// Buffer text. First line is a comment header; then one line per entry: `/{id:0W} {name}`
/// (W = digits of the largest id, at least 3), directories with a trailing `/`.
/// Entries whose name contains '\n' or '\r' are left out (they can't be edited as a line).
pub fn render(dir_display: &str, entries: &[Listed]) -> String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Move to the trash (the app never deletes permanently from :edit).
    Delete { name: String },
    /// Create, `path` relative to the folder, may contain '/'. Parents are created as needed.
    Create { path: String, dir: bool },
    /// Rename or move. `to` is relative to the folder and may contain '/' (move into subfolder).
    Rename { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan { pub changes: Vec<Change> }   // in safe execution order
impl Plan { pub fn is_empty(&self) -> bool; }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    UnknownId { line: usize, id: usize },       // 1-based line numbers
    DuplicateId { id: usize, lines: Vec<usize> },
    InvalidName { line: usize, name: String, reason: &'static str },
    TypeChanged { line: usize, name: String },  // trailing '/' added to a file or removed from a dir
    TargetTaken { line: usize, name: String },  // target exists and is not freed by this plan
    SameTarget { name: String, lines: Vec<usize> },
    Malformed { line: usize, text: String },    // starts with '/' but no valid "/<digits> <name>"
}
impl std::fmt::Display for EditError  // short lowercase line, mentions line numbers

pub fn plan(original: &[Listed], edited: &str) -> Result<Plan, EditError>;

/// One line per change for the confirmation list, e.g. "rename  a.txt → b.txt",
/// "move    a.txt → sub/a.txt", "trash   old.log", "create  notes/", "create  x/y.txt".
pub fn describe(plan: &Plan) -> Vec<String>;
```

## Rules

- Lines that are empty/whitespace-only or start with `#` are ignored.
- `/<id> <name>`: exactly one space after the digits; the name is the rest of the line verbatim
  (only a trailing `\r` is stripped). Id not in `original` (or left out by render) → UnknownId.
- A line without the `/<digits> ` prefix is a creation. Trailing `/` → directory. A name may
  contain `/` to create nested paths (`a/b/c.txt` creates `a/` and `a/b/` if missing).
- Original ids that no line mentions → Delete (only ids that `render` would have written).
- Same id on two lines → DuplicateId (copying is ambiguous; abort rather than guess).
- Name unchanged → nothing. Changed → Rename. If the new name has a `/` (other than the dir's own
  trailing `/`) it is a move; the target's parent must be an existing directory in `original`
  (kept, possibly renamed — use its new name) or one created in this plan.
- Invalid names: empty, `.`, `..`, any `..` or empty path component, starts with `/`, contains NUL.
- Targets: a target that equals a name in `original` is fine only if that original entry is
  renamed away or deleted in this plan. Two changes producing the same target → SameTarget.
  Case-sensitive comparison.
- Order: all Deletes first, then directory Creates (parents before children), then Renames, then
  file Creates. Renames whose target is the source of another pending rename are ordered after
  it; cycles (swap a↔b, rotations) are broken by first renaming one member to a temporary name
  `.vector-tmp-N` (N the smallest number not in `original` and not used as a target), then the
  rest, then temp → final.
- Moving into a directory that is itself renamed in the same plan: execute the dir rename
  first and use its new name in the move target (target paths in the plan are final names).
- A save that changes nothing → `Ok(Plan { changes: vec![] })`.

## Tests (required)

render format (id width, dir slash, header, newline-name skipped); no-op; rename; delete;
create file, dir, nested; move into existing subfolder; move into new subfolder created in the
same edit; move into a renamed folder; swap of two names; 3-cycle rotation; rename onto a deleted
name (allowed); rename onto existing kept name (TargetTaken); two lines to one target
(SameTarget); duplicate id; unknown id; malformed line; invalid names; type change; comments and
blank lines ignored; describe output. For swaps, simulate the plan on an in-memory set of names
and assert the final set is right.
