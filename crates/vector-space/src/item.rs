//! One result row, whatever the mode.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Apps,
    Clipboard,
    Notes,
    Emoji,
    Web,
    Calc,
    Convert,
}

impl Mode {
    pub const ALL: [Mode; 7] = [Mode::Apps, Mode::Clipboard, Mode::Notes, Mode::Emoji, Mode::Web, Mode::Calc, Mode::Convert];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Apps => "apps",
            Mode::Clipboard => "clipboard",
            Mode::Notes => "notifications",
            Mode::Emoji => "emoji",
            Mode::Web => "web",
            Mode::Calc => "calc",
            Mode::Convert => "convert",
        }
    }

    /// The footer tab: short, the footer is shared with the key hints.
    pub fn tab(self) -> &'static str {
        match self {
            Mode::Clipboard => "clip",
            Mode::Notes => "notes",
            m => m.name(),
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "apps" | "app" | "launch" => Mode::Apps,
            "clipboard" | "clip" | "clips" => Mode::Clipboard,
            "notifications" | "notes" | "notification" => Mode::Notes,
            "emoji" | "emojis" | "symbols" => Mode::Emoji,
            "web" | "search" | "websearch" => Mode::Web,
            "calc" | "calculator" => Mode::Calc,
            "convert" | "conv" | "color" | "colour" | "units" | "currency" => Mode::Convert,
            _ => return None,
        })
    }

    /// Typed into an empty search, these switch mode. `$` and `@` are
    /// Walker's old prefixes, kept for the fingers that learned them.
    pub fn from_prefix(c: char) -> Option<Mode> {
        Some(match c {
            ';' | '$' => Mode::Clipboard,
            '!' => Mode::Notes,
            ':' => Mode::Emoji,
            '?' | '@' => Mode::Web,
            '=' => Mode::Calc,
            '>' | '#' => Mode::Convert,
            _ => return None,
        })
    }

    pub fn prefix(self) -> &'static str {
        match self {
            Mode::Apps => "",
            Mode::Clipboard => ";",
            Mode::Notes => "!",
            Mode::Emoji => ":",
            Mode::Web => "?",
            Mode::Calc => "=",
            Mode::Convert => ">",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Mode::Apps => "launch an app",
            Mode::Clipboard => "search the clipboard",
            Mode::Notes => "search notifications",
            Mode::Emoji => "search emoji",
            Mode::Web => "search google",
            Mode::Calc => "calculate",
            Mode::Convert => "10 km to mi · 50 usd · #c9a467 · or copy a file first",
        }
    }

    /// Right pane with details of the selected row.
    pub fn preview(self) -> bool {
        matches!(self, Mode::Clipboard | Mode::Notes | Mode::Emoji)
    }
}

#[derive(Clone, Debug)]
pub enum Icon {
    /// a Nerd Font glyph
    Glyph(&'static str),
    /// an icon name or file from a desktop entry
    Gicon(gio::Icon),
    /// a picture shown as a thumbnail
    File(PathBuf),
    /// an emoji drawn as text
    Text(String),
    /// a colour swatch
    Color(crate::color::Rgba),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Launch { id: String, action: Option<String> },
    Clip(u64),
    Note(u32),
    Emoji(String),
    Open(String),
    Search(String),
    Copy(String),
    /// convert files (or the clipboard image with this id) to a format
    Convert { src: Vec<PathBuf>, image: Option<u64>, to: String },
    /// focus a window (Hyprland address)
    Focus(String),
    /// run a command
    Run(&'static [&'static str]),
    Nothing,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub title: String,
    /// matched char indices of `title`
    pub hl: Vec<u32>,
    pub sub: String,
    pub meta: String,
    pub icon: Icon,
    pub action: Action,
    /// a heading drawn above this row
    pub section: Option<String>,
    /// extra CSS class for the row (urgency, pinned...)
    pub class: Option<&'static str>,
}

impl Item {
    pub fn new(title: impl Into<String>, icon: Icon, action: Action) -> Item {
        Item {
            title: title.into(),
            hl: Vec::new(),
            sub: String::new(),
            meta: String::new(),
            icon,
            action,
            section: None,
            class: None,
        }
    }

    pub fn sub(mut self, s: impl Into<String>) -> Item {
        self.sub = s.into();
        self
    }

    pub fn meta(mut self, s: impl Into<String>) -> Item {
        self.meta = s.into();
        self
    }

    pub fn hl(mut self, hl: Vec<u32>) -> Item {
        self.hl = hl;
        self
    }

    pub fn class(mut self, c: &'static str) -> Item {
        self.class = Some(c);
        self
    }
}
