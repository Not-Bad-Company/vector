//! The `:` command line: parser and tab completion. Pure logic; directory listing is injected.

use crate::sort::SortKey;
use std::path::{Path, PathBuf};

/// Argument of `:hidden`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toggle {
    Toggle,
    On,
    Off,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    New(String),
    Mkdir(String),
    Find(String),
    Grep(String),
    Cd(String),
    Sort {
        key: SortKey,
        reverse: bool,
    },
    Hidden(Toggle),
    Open(Option<String>),
    Term,
    Edit,
    /// `:filter [expr]`: set the `/` filter (no arg clears it).
    Filter(String),
    /// `:flat`: toggle listing the whole tree under this folder.
    Flat,
    Quit,
}

/// Why a command line did not parse; `Display` gives the message for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    Unknown(String),
    Ambiguous(String, Vec<&'static str>),
    MissingArg(&'static str),
    BadArg { command: &'static str, arg: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty command"),
            Self::Unknown(name) => write!(f, "unknown command: {name}"),
            Self::Ambiguous(name, _) => write!(f, "ambiguous command: {name}"),
            Self::MissingArg(name) => write!(f, "missing argument: {name}"),
            Self::BadArg { command, arg } => write!(f, "bad argument for {command}: {arg}"),
        }
    }
}

/// Command names, in the order completion offers them.
pub const COMMANDS: &[&str] = &[
    "new", "mkdir", "find", "grep", "cd", "sort", "hidden", "open", "term", "edit", "filter",
    "flat", "quit",
];

/// Parses what follows the `:` (a leading `:` is skipped too).
pub fn parse(line: &str) -> Result<Command, ParseError> {
    let line = line.trim().strip_prefix(':').unwrap_or(line.trim()).trim();
    if line.is_empty() {
        return Err(ParseError::Empty);
    }
    let split = line.find(char::is_whitespace).unwrap_or(line.len());
    let raw_name = &line[..split];
    let mut name = raw_name;
    let bang = raw_name.ends_with('!');
    if bang {
        name = &raw_name[..raw_name.len() - 1];
    }
    let rest = line[split..].trim();
    let command = resolve(name)?;
    if bang && command != "sort" {
        return Err(ParseError::BadArg {
            command,
            arg: "!".into(),
        });
    }
    let arg = unquote(rest);
    match command {
        "new" => required(arg, "new").map(Command::New),
        "mkdir" => required(arg, "mkdir").map(Command::Mkdir),
        "find" => required(arg, "find").map(Command::Find),
        "grep" => required(arg, "grep").map(Command::Grep),
        "cd" => Ok(Command::Cd(if arg.is_empty() {
            "~".into()
        } else {
            arg.into()
        })),
        "sort" => {
            let mut words = arg.split_whitespace();
            let key_word = words.next().ok_or(ParseError::MissingArg("sort"))?;
            let key = match key_word {
                "name" => SortKey::Name,
                "size" => SortKey::Size,
                "mtime" | "modified" | "date" => SortKey::Modified,
                "type" | "ext" => SortKey::Type,
                _ => {
                    return Err(ParseError::BadArg {
                        command: "sort",
                        arg: key_word.into(),
                    });
                }
            };
            let second = words.next();
            if let Some(word) = second
                && (!matches!(word, "rev" | "reverse") || words.next().is_some())
            {
                return Err(ParseError::BadArg {
                    command: "sort",
                    arg: arg.into(),
                });
            }
            Ok(Command::Sort {
                key,
                reverse: bang || second.is_some(),
            })
        }
        "hidden" => match arg.to_ascii_lowercase().as_str() {
            "" => Ok(Command::Hidden(Toggle::Toggle)),
            "on" | "true" | "show" => Ok(Command::Hidden(Toggle::On)),
            "off" | "false" | "hide" => Ok(Command::Hidden(Toggle::Off)),
            _ => Err(ParseError::BadArg {
                command: "hidden",
                arg: arg.into(),
            }),
        },
        "open" => Ok(Command::Open(if arg.is_empty() {
            None
        } else {
            Some(arg.into())
        })),
        "term" if arg.is_empty() => Ok(Command::Term),
        "edit" if arg.is_empty() => Ok(Command::Edit),
        "filter" => Ok(Command::Filter(arg.to_owned())),
        "flat" if arg.is_empty() => Ok(Command::Flat),
        "quit" if arg.is_empty() => Ok(Command::Quit),
        other => Err(ParseError::BadArg {
            command: other,
            arg: arg.into(),
        }),
    }
}

/// Full command name for a typed name or unambiguous prefix (`q` is quit).
pub fn resolve(name: &str) -> Result<&'static str, ParseError> {
    if name == "q" {
        return Ok("quit");
    }
    if let Some(exact) = COMMANDS.iter().copied().find(|c| *c == name) {
        return Ok(exact);
    }
    let matches: Vec<_> = COMMANDS
        .iter()
        .copied()
        .filter(|c| c.starts_with(name))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => Err(ParseError::Unknown(name.to_owned())),
        many => Err(ParseError::Ambiguous(name.to_owned(), many.to_vec())),
    }
}

fn required(arg: &str, command: &'static str) -> Result<String, ParseError> {
    if arg.is_empty() {
        Err(ParseError::MissingArg(command))
    } else {
        Ok(arg.to_owned())
    }
}

fn unquote(arg: &str) -> &str {
    if arg.len() >= 2 {
        let bytes = arg.as_bytes();
        if (bytes[0] == b'"' && bytes[arg.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[arg.len() - 1] == b'\'')
        {
            return &arg[1..arg.len() - 1];
        }
    }
    arg
}

/// What completion may look at.
pub struct CompletionCtx<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// Lists a directory: (name, is_dir). Empty on error.
    pub list_dir: &'a dyn Fn(&Path) -> Vec<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Candidates replace `line[start..]`.
pub struct Completion {
    pub start: usize,
    pub candidates: Vec<String>,
}

/// Completes the word at the end of `line`.
pub fn complete(line: &str, ctx: &CompletionCtx<'_>) -> Completion {
    let (colon, input) = if let Some(s) = line.strip_prefix(':') {
        (1, s)
    } else {
        (0, line)
    };
    let Some(space) = input.find(char::is_whitespace) else {
        let candidates = COMMANDS
            .iter()
            .filter(|name| name.starts_with(input))
            .map(|s| (*s).to_owned())
            .collect();
        return Completion {
            start: colon,
            candidates,
        };
    };
    let name = resolve(input[..space].trim_end_matches('!')).unwrap_or("");
    let arg_start = colon + space + input[space..].len() - input[space..].trim_start().len();
    let arg = &line[arg_start..];
    if name == "filter" {
        // complete the last word only
        let word_at = arg.rfind(' ').map_or(0, |i| i + 1);
        let word = &arg[word_at..];
        let (neg, bare) = word.strip_prefix('!').map_or(("", word), |w| ("!", w));
        let candidates: Vec<String> = match bare.split_once(':') {
            Some(("type" | "is", v)) => {
                let done = v.rfind(['|', ',']).map_or(0, |i| i + 1);
                crate::filter::Class::ALL
                    .iter()
                    .filter(|(n, _)| n.starts_with(&v[done..]))
                    .map(|(n, _)| {
                        format!(
                            "{neg}{}:{}{n}",
                            &bare[..bare.find(':').unwrap_or(0)],
                            &v[..done]
                        )
                    })
                    .collect()
            }
            Some(_) => Vec::new(),
            None => ["type:", "ext:", "size>", "size<", "mod<", "mod>"]
                .iter()
                .filter(|k| k.starts_with(bare))
                .map(|k| format!("{neg}{k}"))
                .collect(),
        };
        return Completion {
            start: arg_start + word_at,
            candidates,
        };
    }
    let candidates = match name {
        "sort" => prefixed(&["name", "size", "mtime", "type"], arg),
        "hidden" => prefixed(&["on", "off"], arg),
        "cd" => path_candidates(arg, ctx, true),
        "open" | "new" | "mkdir" => path_candidates(arg, ctx, false),
        _ => Vec::new(),
    };
    Completion {
        start: arg_start,
        candidates,
    }
}

fn prefixed(values: &[&str], prefix: &str) -> Vec<String> {
    values
        .iter()
        .filter(|v| v.starts_with(prefix))
        .map(|v| (*v).to_owned())
        .collect()
}

fn path_candidates(arg: &str, ctx: &CompletionCtx<'_>, dirs_only: bool) -> Vec<String> {
    let (dir_part, name_part) = match arg.rfind('/') {
        Some(i) => (&arg[..=i], &arg[i + 1..]),
        None => ("", arg),
    };
    let base = resolve_dir(dir_part, ctx);
    let entries = (ctx.list_dir)(&base);
    let visible = |name: &str| !name.starts_with('.') || name_part.starts_with('.');
    let mut matching: Vec<_> = entries
        .iter()
        .filter(|(n, is_dir)| (!dirs_only || *is_dir) && visible(n) && n.starts_with(name_part))
        .collect();
    if matching.is_empty() {
        matching = entries
            .iter()
            .filter(|(n, is_dir)| {
                (!dirs_only || *is_dir)
                    && visible(n)
                    && n.to_ascii_lowercase()
                        .starts_with(&name_part.to_ascii_lowercase())
            })
            .collect();
    }
    let mut out: Vec<String> = matching
        .into_iter()
        .map(|(name, is_dir)| format!("{dir_part}{name}{}", if *is_dir { "/" } else { "" }))
        .collect();
    out.sort_by_key(|s| s.to_ascii_lowercase());
    out.dedup();
    out
}

fn resolve_dir(part: &str, ctx: &CompletionCtx<'_>) -> PathBuf {
    if part == "~" || part == "~/" {
        return ctx.home.to_path_buf();
    }
    if let Some(rest) = part.strip_prefix("~/") {
        return ctx.home.join(rest);
    }
    let path = Path::new(part);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn parser_commands_prefixes_and_errors() {
        for (s, expected) in [
            ("new a", Command::New("a".into())),
            ("mkdir a", Command::Mkdir("a".into())),
            ("find a", Command::Find("a".into())),
            ("grep a", Command::Grep("a".into())),
            ("cd", Command::Cd("~".into())),
            ("term", Command::Term),
            ("edit", Command::Edit),
            ("q", Command::Quit),
            ("op", Command::Open(None)),
        ] {
            assert_eq!(parse(s), Ok(expected));
        }
        assert_eq!(parse(":"), Err(ParseError::Empty));
        assert_eq!(parse("m"), Err(ParseError::MissingArg("mkdir")));
        assert_eq!(parse("zz"), Err(ParseError::Unknown("zz".into())));
        assert_eq!(parse("new"), Err(ParseError::MissingArg("new")));
        assert!(matches!(
            parse("term extra"),
            Err(ParseError::BadArg { .. })
        ));
        assert_eq!(
            ParseError::Unknown("foo".into()).to_string(),
            "unknown command: foo"
        );
    }

    #[test]
    fn parser_arguments_sort_and_hidden() {
        assert_eq!(
            parse("new 'my file.txt'"),
            Ok(Command::New("my file.txt".into()))
        );
        assert_eq!(
            parse("new my file.txt"),
            Ok(Command::New("my file.txt".into()))
        );
        assert_eq!(
            parse("sort date reverse"),
            Ok(Command::Sort {
                key: SortKey::Modified,
                reverse: true
            })
        );
        assert_eq!(
            parse("sort! ext"),
            Ok(Command::Sort {
                key: SortKey::Type,
                reverse: true
            })
        );
        assert_eq!(parse("sort"), Err(ParseError::MissingArg("sort")));
        assert!(matches!(parse("sort bad"), Err(ParseError::BadArg { .. })));
        for (word, toggle) in [
            ("", Toggle::Toggle),
            ("show", Toggle::On),
            ("true", Toggle::On),
            ("hide", Toggle::Off),
            ("false", Toggle::Off),
        ] {
            assert_eq!(
                parse(&format!("hidden {word}")),
                Ok(Command::Hidden(toggle))
            );
        }
        assert!(matches!(
            parse("hidden maybe"),
            Err(ParseError::BadArg { .. })
        ));
    }

    #[test]
    fn completion_commands_and_words() {
        let lister = |_p: &Path| Vec::new();
        let ctx = CompletionCtx {
            cwd: Path::new("/cwd"),
            home: Path::new("/home/fake"),
            list_dir: &lister,
        };
        assert_eq!(complete("mk", &ctx).candidates, vec!["mkdir"]);
        assert_eq!(complete("sort m", &ctx).candidates, vec!["mtime"]);
        assert!(complete("unknown ", &ctx).candidates.is_empty());
    }

    #[test]
    fn completion_paths_and_fallback() {
        let mut map: HashMap<PathBuf, Vec<(String, bool)>> = HashMap::new();
        map.insert(
            PathBuf::from("/cwd"),
            vec![
                ("Documents".into(), true),
                (".secret".into(), false),
                ("Alpha".into(), false),
            ],
        );
        map.insert(
            PathBuf::from("/cwd/Documents"),
            vec![("notes".into(), true), ("notes.txt".into(), false)],
        );
        map.insert(
            PathBuf::from("/home/fake"),
            vec![("Downloads".into(), true)],
        );
        let lister = |p: &Path| map.get(p).cloned().unwrap_or_default();
        let ctx = CompletionCtx {
            cwd: Path::new("/cwd"),
            home: Path::new("/home/fake"),
            list_dir: &lister,
        };
        assert_eq!(complete("cd Doc", &ctx).candidates, vec!["Documents/"]);
        assert_eq!(complete("c Doc", &ctx).candidates, vec!["Documents/"]);
        assert_eq!(complete("cd Doc", &ctx).start, 3);
        assert_eq!(
            complete("cd Documents/no", &ctx).candidates,
            vec!["Documents/notes/"]
        );
        assert_eq!(complete("cd ~/", &ctx).candidates, vec!["~/Downloads/"]);
        assert!(
            complete("open ", &ctx)
                .candidates
                .iter()
                .all(|s| !s.contains(".secret"))
        );
        assert_eq!(complete("open .", &ctx).candidates, vec![".secret"]);
        assert_eq!(complete("open a", &ctx).candidates, vec!["Alpha"]);
    }

    #[test]
    fn filter_and_flat() {
        assert_eq!(
            parse("filter ext:exe size>1m"),
            Ok(Command::Filter("ext:exe size>1m".into()))
        );
        assert_eq!(parse("filter"), Ok(Command::Filter(String::new())));
        assert_eq!(parse("flat"), Ok(Command::Flat));
        assert_eq!(parse("fl"), Ok(Command::Flat));
        assert!(matches!(parse("f"), Err(ParseError::Ambiguous(..))));
        assert!(matches!(parse("fi"), Err(ParseError::Ambiguous(..))));
    }

    #[test]
    fn filter_completion() {
        let lister = |_p: &Path| Vec::new();
        let ctx = CompletionCtx {
            cwd: Path::new("/c"),
            home: Path::new("/h"),
            list_dir: &lister,
        };
        assert_eq!(complete("filter ty", &ctx).candidates, vec!["type:"]);
        let c = complete("filter *.exe type:im", &ctx);
        assert_eq!(
            (c.start, c.candidates),
            (13, vec!["type:image".to_string()])
        );
        assert_eq!(
            complete("filter is:image|vi", &ctx).candidates,
            vec!["is:image|video"]
        );
        assert_eq!(
            complete("filter !s", &ctx).candidates,
            vec!["!size>", "!size<"]
        );
        assert!(complete("filter ext:", &ctx).candidates.is_empty());
    }
}
