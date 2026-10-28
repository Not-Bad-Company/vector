//! nvim-style modes and key sequences. The app turns GDK key events into [`Key`]s, feeds them to
//! a [`Keymap`] and runs the [`Action`]s that come out. Text entry in the command line and the
//! filter is left to the entry widget; only the keys that leave those modes are mapped here.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Visual,
    /// `:` command line.
    Command,
    /// `/` live filter.
    Filter,
}

impl Mode {
    /// Text of the statusline badge.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::Command => "COMMAND",
            Mode::Filter => "FILTER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    /// Ctrl plus a lowercase letter.
    Ctrl(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    ShiftDelete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Char(c) => write!(f, "{c}"),
            Key::Ctrl(c) => write!(f, "<C-{c}>"),
            Key::F(n) => write!(f, "<F{n}>"),
            other => write!(f, "<{other:?}>"),
        }
    }
}

/// Parses vim notation like `gg`, `:mkdir x<CR>`, `<C-t>`, `<Esc>` into keys (for `--keys` and
/// tests). Unknown `<…>` names are kept as literal characters.
pub fn parse_keys(s: &str) -> Vec<Key> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(k) = named_key(&rest[1..end])
        {
            out.push(k);
            rest = &rest[end + 1..];
            continue;
        }
        out.push(Key::Char(c));
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn named_key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    if let Some(c) = lower.strip_prefix("c-")
        && let [ch] = c.chars().collect::<Vec<_>>()[..]
    {
        return Some(Key::Ctrl(ch));
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse().ok()) {
        return Some(Key::F(n));
    }
    Some(match lower.as_str() {
        "cr" | "enter" | "return" => Key::Enter,
        "esc" => Key::Esc,
        "tab" => Key::Tab,
        "s-tab" => Key::BackTab,
        "bs" => Key::Backspace,
        "del" => Key::Delete,
        "s-del" => Key::ShiftDelete,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "space" => Key::Char(' '),
        "lt" => Key::Char('<'),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    /// `G`: last row, or row N with a count.
    Bottom,
    /// Open the entry under the cursor (folder: go in; file: default app).
    Open,
    Parent,
    Back,
    Forward,
    Yank,
    Cut,
    Paste,
    Trash,
    DeletePermanently,
    Rename,
    Undo,
    Repeat,
    VisualStart,
    /// Leave visual/filter, clear the selection.
    Escape,
    CommandStart,
    FilterStart,
    ToggleHidden,
    ToggleView,
    TogglePreview,
    ToggleSplit,
    OtherPane,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    ToggleSelect,
    SelectAll,
    FolderSize,
    Edit,
    Terminal,
    Reload,
    PathEdit,
    /// The context menu, for keyboard users.
    Menu,
    /// The key cheat sheet.
    Help,
    Quit,
}

impl Action {
    /// Whether `.` may repeat it (changes to files, not movement or view toggles).
    pub fn repeatable(self) -> bool {
        matches!(self, Action::Paste | Action::Trash)
    }
}

/// Bindings in normal and visual mode. Sequences of more than one key wait for the rest.
pub const BINDINGS: &[(&str, Action)] = &[
    ("j", Action::Down),
    ("<Down>", Action::Down),
    ("k", Action::Up),
    ("<Up>", Action::Up),
    ("<C-d>", Action::PageDown),
    ("<PageDown>", Action::PageDown),
    ("<C-u>", Action::PageUp),
    ("<PageUp>", Action::PageUp),
    ("gg", Action::Top),
    ("<Home>", Action::Top),
    ("G", Action::Bottom),
    ("<End>", Action::Bottom),
    ("l", Action::Open),
    ("<Right>", Action::Open),
    ("<CR>", Action::Open),
    ("h", Action::Parent),
    ("-", Action::Parent),
    ("<Left>", Action::Parent),
    ("<BS>", Action::Parent),
    ("H", Action::Back),
    ("L", Action::Forward),
    // single keys: no doubled presses (yy/dd) needed
    ("y", Action::Yank),
    ("x", Action::Cut),
    ("p", Action::Paste),
    ("d", Action::Trash),
    ("D", Action::DeletePermanently),
    ("<Del>", Action::Trash),
    ("<S-Del>", Action::DeletePermanently),
    ("r", Action::Rename),
    ("<F2>", Action::Rename),
    ("u", Action::Undo),
    ("<C-z>", Action::Undo),
    (".", Action::Repeat),
    ("v", Action::VisualStart),
    ("V", Action::VisualStart),
    ("<Esc>", Action::Escape),
    (":", Action::CommandStart),
    ("/", Action::FilterStart),
    ("zh", Action::ToggleHidden),
    ("<C-h>", Action::ToggleHidden),
    ("w", Action::ToggleView),
    ("i", Action::TogglePreview),
    ("s", Action::ToggleSplit),
    ("<F3>", Action::ToggleSplit),
    ("<Tab>", Action::OtherPane),
    ("<C-t>", Action::NewTab),
    ("<C-w>", Action::CloseTab),
    ("gt", Action::NextTab),
    ("gT", Action::PrevTab),
    (" ", Action::ToggleSelect),
    ("<C-a>", Action::SelectAll),
    ("gs", Action::FolderSize),
    ("ge", Action::Edit),
    ("<C-l>", Action::PathEdit),
    ("gx", Action::Terminal),
    ("gm", Action::Menu),
    ("?", Action::Help),
    ("<F1>", Action::Help),
    ("<C-r>", Action::Reload),
    ("<F5>", Action::Reload),
    ("ZZ", Action::Quit),
];

/// Result of feeding one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// Part of a longer sequence or a count; wait for more.
    Pending,
    /// Run `action` with `count` (1 when no count was typed, except [`Action::Bottom`] where
    /// `count` is the typed row number or 0 for "last").
    Run { action: Action, count: usize },
    /// Not bound; the sequence was dropped.
    Unbound,
}

/// Turns keys into actions in normal and visual mode, with counts (`5j`) and multi-key
/// sequences (`gg`, `gs`).
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    pending: Vec<Key>,
    count: Option<usize>,
}

impl Keymap {
    /// What has been typed so far (count and keys), for the statusline.
    pub fn pending_text(&self) -> String {
        let mut s = self.count.map(|c| c.to_string()).unwrap_or_default();
        for k in &self.pending {
            s.push_str(&k.to_string());
        }
        s
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.count = None;
    }

    pub fn feed(&mut self, key: Key) -> Feed {
        if key == Key::Esc {
            self.reset();
            return Feed::Run {
                action: Action::Escape,
                count: 1,
            };
        }
        if self.pending.is_empty()
            && let Key::Char(c @ '0'..='9') = key
            && (c != '0' || self.count.is_some())
        {
            let d = c as usize - '0' as usize;
            self.count = Some(
                self.count
                    .unwrap_or(0)
                    .saturating_mul(10)
                    .saturating_add(d)
                    .min(1_000_000),
            );
            return Feed::Pending;
        }
        self.pending.push(key);
        let mut prefix = false;
        for (seq, action) in BINDINGS {
            let keys = parse_keys(seq);
            if keys == self.pending {
                let count = match (action, self.count) {
                    (Action::Bottom, c) => c.unwrap_or(0),
                    (_, c) => c.unwrap_or(1),
                };
                self.reset();
                return Feed::Run {
                    action: *action,
                    count,
                };
            }
            if keys.len() > self.pending.len() && keys.starts_with(&self.pending) {
                prefix = true;
            }
        }
        if prefix {
            Feed::Pending
        } else {
            self.reset();
            Feed::Unbound
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(keys: &str) -> Vec<Feed> {
        let mut m = Keymap::default();
        parse_keys(keys).into_iter().map(|k| m.feed(k)).collect()
    }

    fn act(a: Action, count: usize) -> Feed {
        Feed::Run { action: a, count }
    }

    #[test]
    fn parse_vim_notation() {
        assert_eq!(
            parse_keys("gg<CR><C-t><Esc><F2><s-tab><lt>x<nope>"),
            vec![
                Key::Char('g'),
                Key::Char('g'),
                Key::Enter,
                Key::Ctrl('t'),
                Key::Esc,
                Key::F(2),
                Key::BackTab,
                Key::Char('<'),
                Key::Char('x'),
                Key::Char('<'),
                Key::Char('n'),
                Key::Char('o'),
                Key::Char('p'),
                Key::Char('e'),
                Key::Char('>'),
            ]
        );
        assert_eq!(
            parse_keys(":mkdir ä"),
            ":mkdir ä".chars().map(Key::Char).collect::<Vec<_>>()
        );
    }

    #[test]
    fn single_keys_and_sequences() {
        assert_eq!(run("j"), [act(Action::Down, 1)]);
        assert_eq!(run("gg"), [Feed::Pending, act(Action::Top, 1)]);
        assert_eq!(run("y"), [act(Action::Yank, 1)]);
        assert_eq!(run("x"), [act(Action::Cut, 1)]);
        assert_eq!(run("d"), [act(Action::Trash, 1)]);
        assert_eq!(run("D"), [act(Action::DeletePermanently, 1)]);
        assert_eq!(run("p"), [act(Action::Paste, 1)]);
        assert_eq!(run("-"), [act(Action::Parent, 1)]);
        assert_eq!(run("<CR>"), [act(Action::Open, 1)]);
        assert_eq!(run("u."), [act(Action::Undo, 1), act(Action::Repeat, 1)]);
        assert_eq!(run(":"), [act(Action::CommandStart, 1)]);
        assert_eq!(run("/"), [act(Action::FilterStart, 1)]);
        assert_eq!(run("v"), [act(Action::VisualStart, 1)]);
    }

    #[test]
    fn counts() {
        assert_eq!(run("5j"), [Feed::Pending, act(Action::Down, 5)]);
        assert_eq!(
            run("12k"),
            [Feed::Pending, Feed::Pending, act(Action::Up, 12)]
        );
        assert_eq!(run("G"), [act(Action::Bottom, 0)]);
        assert_eq!(run("3G"), [Feed::Pending, act(Action::Bottom, 3)]);
        // 0 alone is not a count
        assert_eq!(run("0"), [Feed::Unbound]);
        assert_eq!(run("10j")[2], act(Action::Down, 10));
        let mut m = Keymap::default();
        for _ in 0..30 {
            m.feed(Key::Char('9'));
        }
        assert!(matches!(
            m.feed(Key::Char('j')),
            Feed::Run {
                count: 1_000_000,
                ..
            }
        ));
    }

    #[test]
    fn unbound_and_broken_sequences_reset() {
        assert_eq!(run("q"), [Feed::Unbound]);
        assert_eq!(run("gq"), [Feed::Pending, Feed::Unbound]);
        // after a broken sequence the next key starts fresh
        assert_eq!(
            run("gqj"),
            [Feed::Pending, Feed::Unbound, act(Action::Down, 1)]
        );
        assert_eq!(
            run("5<Esc>j"),
            [Feed::Pending, act(Action::Escape, 1), act(Action::Down, 1)]
        );
        assert_eq!(run("g<Esc>"), [Feed::Pending, act(Action::Escape, 1)]);
    }

    #[test]
    fn pending_text_and_repeatable() {
        let mut m = Keymap::default();
        m.feed(Key::Char('3'));
        m.feed(Key::Char('g'));
        assert_eq!(m.pending_text(), "3g");
        assert!(Action::Paste.repeatable() && Action::Trash.repeatable());
        assert!(!Action::Down.repeatable() && !Action::Undo.repeatable());
    }

    #[test]
    fn no_binding_is_a_prefix_of_another_with_different_meaning() {
        // a binding that is a strict prefix of another would make the longer one unreachable
        for (a, _) in BINDINGS {
            for (b, _) in BINDINGS {
                let (ka, kb) = (parse_keys(a), parse_keys(b));
                assert!(
                    !(ka.len() < kb.len() && kb.starts_with(&ka)),
                    "{a} shadows {b}"
                );
            }
        }
    }
}
