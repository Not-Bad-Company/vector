# command.rs — the `:` command line: parser and tab completion

File: `crates/strata-core/src/command.rs`. Pure logic, no I/O (directory listing is injected).

## Parser

```rust
use crate::sort::SortKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toggle { Toggle, On, Off }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    New(String),                          // :new <name>     create empty file (name may contain '/')
    Mkdir(String),                        // :mkdir <name>   create folder (may contain '/')
    Find(String),                         // :find <query>   fuzzy recursive name search
    Grep(String),                         // :grep <pattern> content search
    Cd(String),                           // :cd [path]      no arg = home ("~")
    Sort { key: SortKey, reverse: bool }, // :sort name|size|mtime|type [rev]   (also "sort! key" = reverse)
    Hidden(Toggle),                       // :hidden [on|off]
    Open(Option<String>),                 // :open [name]    no arg = current selection
    Term,                                 // :term
    Edit,                                 // :edit
    Quit,                                 // :q / :quit
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,                                // blank line
    Unknown(String),                      // no command matches
    Ambiguous(String, Vec<&'static str>), // prefix matches several
    MissingArg(&'static str),             // command name, e.g. "mkdir"
    BadArg { command: &'static str, arg: String },
}

pub fn parse(line: &str) -> Result<Command, ParseError>;
impl std::fmt::Display for ParseError  // one short lowercase line, e.g. "unknown command: foo"
```

- `line` is what follows the `:`; a leading `:` is also accepted and skipped. Leading/trailing
  whitespace trimmed.
- Command names: `new mkdir find grep cd sort hidden open term edit quit`, plus the alias `q`.
  Any unambiguous prefix is accepted (`:mk` = mkdir, `:e` = edit). Exact name wins over prefix.
- A `!` directly after the name (`:sort! size`) sets `reverse` for sort; for other commands it is
  `BadArg`.
- Argument = rest of the line after the name and whitespace, kept verbatim (inner spaces
  preserved: `:new my file.txt` → `New("my file.txt")`). If the whole argument is wrapped in
  matching `"…"` or `'…'`, strip them.
- `sort` keys: `name`, `size`, `mtime` (alias `modified`, `date`), `type` (alias `ext`), optional
  second word `rev`/`reverse`. No key → `MissingArg`. Bad key → `BadArg`.
- `hidden`: no arg → Toggle, `on`/`true`/`show` → On, `off`/`false`/`hide` → Off, else BadArg.
- `new`, `mkdir`, `find`, `grep` need an argument (MissingArg). `term`, `edit`, `quit` take none
  (BadArg if given).

## Completion

```rust
pub struct CompletionCtx<'a> {
    pub cwd: &'a std::path::Path,
    pub home: &'a std::path::Path,
    /// Lists a directory: (name, is_dir). Returns empty on error.
    pub list_dir: &'a dyn Fn(&std::path::Path) -> Vec<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// Byte offset in the line where the replaced word starts.
    pub start: usize,
    /// Full replacement texts for line[start..], sorted, deduplicated.
    pub candidates: Vec<String>,
}

pub fn complete(line: &str, ctx: &CompletionCtx) -> Completion;
```

- Cursor is always at the end of `line` (no leading `:`).
- While typing the command name (no space yet): candidates = command names with that prefix
  (not the `q` alias). `start` = offset of the name.
- `cd`: argument completes to directories only. `open`, `new`, `mkdir`: files and dirs.
  `sort`: the four keys. `hidden`: `on`, `off`. Others: no candidates.
- Path arguments: split at the last `/`. The part before is resolved against cwd (absolute if it
  starts with `/`, home if it starts with `~/` or is `~`). Candidates keep the typed directory
  part verbatim and append the matching name; directories get a trailing `/`. Names starting
  with `.` are offered only when the typed name part starts with `.`.
- Matching: case-sensitive prefix; if that gives nothing, case-insensitive prefix.
- Sort candidates case-insensitively.

## Tests (required)

Every command parses; prefixes; ambiguity; quotes; spaces kept; sort keys/rev/`!`; hidden
variants; missing/extra args; Display text. Completion: command names, `cd` dirs only with
trailing `/`, nested `cd Documents/no` → `Documents/notes/`, `~/` expansion (use a fake home in
the injected lister), hidden only with `.` prefix, case-insensitive fallback, unknown command → no
candidates.
