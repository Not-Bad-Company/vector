//! Colours from the active Omarchy theme.

use std::fs;
use std::path::{Path, PathBuf};

/// Theme file, relative to `$HOME`.
const THEME_PATH: &str = ".config/omarchy/current/theme/colors.toml";

/// The user's own colours, same format; wins over the Omarchy theme.
const USER_THEME: &str = "vector/theme.toml";

/// The colours vector draws with. Every value is a lowercase `#rrggbb` string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Window background.
    pub background: String,
    /// Main text colour.
    pub foreground: String,
    /// Selection and highlight colour.
    pub accent: String,
    /// Red, used for critical messages.
    pub color1: String,
    /// Green.
    pub color2: String,
    /// Yellow, used for warnings.
    pub color3: String,
    /// Blue.
    pub color4: String,
    /// Magenta.
    pub color5: String,
    /// Cyan.
    pub color6: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: "#1e1e2e".into(),
            foreground: "#cdd6f4".into(),
            accent: "#81a1c1".into(),
            color1: "#bf616a".into(),
            color2: "#a3be8c".into(),
            color3: "#ebcb8b".into(),
            color4: "#81a1c1".into(),
            color5: "#b48ead".into(),
            color6: "#88c0d0".into(),
        }
    }
}

impl Theme {
    /// Reads `key = "#rrggbb"` lines. Missing or invalid keys keep their default.
    pub fn parse(text: &str) -> Theme {
        let mut theme = Theme::default();
        for line in text.lines() {
            let Some((key, value)) = parse_line(line) else {
                continue;
            };
            let slot = match key {
                "background" => &mut theme.background,
                "foreground" => &mut theme.foreground,
                "accent" => &mut theme.accent,
                "color1" => &mut theme.color1,
                "color2" => &mut theme.color2,
                "color3" => &mut theme.color3,
                "color4" => &mut theme.color4,
                "color5" => &mut theme.color5,
                "color6" => &mut theme.color6,
                _ => continue,
            };
            *slot = value;
        }
        theme
    }

    /// `$XDG_CONFIG_HOME/vector/theme.toml` if it exists, else the active Omarchy theme, else
    /// the defaults. Both files use `key = "#rrggbb"` lines; missing keys keep their default.
    pub fn load() -> Theme {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        if let Some(text) = config.and_then(|c| fs::read_to_string(c.join(USER_THEME)).ok()) {
            return Theme::parse(&text);
        }
        load_from(std::env::var_os("HOME").map(PathBuf::from).as_deref())
    }
}

/// Loads the theme from under `home`. Returns the default if `home` is unset or the file is unreadable.
fn load_from(home: Option<&Path>) -> Theme {
    let Some(home) = home else {
        return Theme::default();
    };
    fs::read_to_string(home.join(THEME_PATH))
        .map(|text| Theme::parse(&text))
        .unwrap_or_default()
}

/// Splits one line into a key and a lowercase colour. Returns `None` for anything else.
fn parse_line(line: &str) -> Option<(&str, String)> {
    let (key, rest) = line.split_once('=')?;
    let rest = rest.trim_start().strip_prefix('"')?;
    let (value, tail) = rest.split_once('"')?;
    let tail = tail.trim();
    if !(tail.is_empty() || tail.starts_with('#')) {
        return None;
    }
    if !is_hex_colour(value) {
        return None;
    }
    let key = key.trim().trim_matches('"');
    Some((key, value.to_ascii_lowercase()))
}

/// True for exactly `#` followed by six hex digits.
fn is_hex_colour(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value.bytes().skip(1).all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r##"
background = "#101010"
foreground = "#f0f0f0"
accent = "#abcdef"
color1 = "#ff0000"
color2 = "#00ff00"
color3 = "#ffff00"
"##;

    #[test]
    fn full_file() {
        let t = Theme::parse(FULL);
        assert_eq!(t.background, "#101010");
        assert_eq!(t.foreground, "#f0f0f0");
        assert_eq!(t.accent, "#abcdef");
        assert_eq!(t.color1, "#ff0000");
        assert_eq!(t.color2, "#00ff00");
        assert_eq!(t.color3, "#ffff00");
    }

    #[test]
    fn empty_text_gives_default() {
        assert_eq!(Theme::parse(""), Theme::default());
    }

    #[test]
    fn missing_keys_keep_defaults() {
        let t = Theme::parse("accent = \"#112233\"\n");
        let d = Theme::default();
        assert_eq!(t.accent, "#112233");
        assert_eq!(t.background, d.background);
        assert_eq!(t.foreground, d.foreground);
        assert_eq!(t.color1, d.color1);
        assert_eq!(t.color2, d.color2);
        assert_eq!(t.color3, d.color3);
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let text = "this is not toml\n=\n\"#ffffff\"\nbackground\nbackground = \n\
                    accent = [\"#ffffff\"]\n[colors]\n";
        assert_eq!(Theme::parse(text), Theme::default());
    }

    #[test]
    fn comments_are_ignored() {
        let text = "# accent = \"#000000\"\n# comment\nforeground = \"#222222\" # trailing\n";
        let t = Theme::parse(text);
        assert_eq!(t.foreground, "#222222");
        assert_eq!(t.accent, Theme::default().accent);
    }

    #[test]
    fn whitespace_variations() {
        let text = "  background=\"#aaaaaa\"  \n\tforeground   =   \"#bbbbbb\"\naccent=\"#cccccc\"";
        let t = Theme::parse(text);
        assert_eq!(t.background, "#aaaaaa");
        assert_eq!(t.foreground, "#bbbbbb");
        assert_eq!(t.accent, "#cccccc");
    }

    #[test]
    fn uppercase_hex_is_stored_lowercase() {
        let t = Theme::parse("accent = \"#ABCDEF\"\ncolor1 = \"#AbC123\"\n");
        assert_eq!(t.accent, "#abcdef");
        assert_eq!(t.color1, "#abc123");
    }

    #[test]
    fn invalid_colour_formats_are_rejected() {
        let text = "background = \"#fff\"\n\
                    foreground = \"0xffffff\"\n\
                    accent = #81a1c1\n\
                    color1 = \"#gggggg\"\n\
                    color2 = \"81a1c1\"\n\
                    color3 = \"#ffffff00\"\n";
        assert_eq!(Theme::parse(text), Theme::default());
    }

    #[test]
    fn last_valid_value_wins() {
        let t = Theme::parse("accent = \"#111111\"\naccent = \"#222222\"\n");
        assert_eq!(t.accent, "#222222");
    }

    #[test]
    fn invalid_value_does_not_clobber_earlier_valid_one() {
        let t = Theme::parse("accent = \"#111111\"\naccent = \"nope\"\n");
        assert_eq!(t.accent, "#111111");
    }

    #[test]
    fn load_reads_theme_file_under_home() {
        let dir = tempfile::tempdir().unwrap();
        let theme_dir = dir.path().join(".config/omarchy/current/theme");
        fs::create_dir_all(&theme_dir).unwrap();
        fs::write(theme_dir.join("colors.toml"), FULL).unwrap();

        let t = load_from(Some(dir.path()));
        assert_eq!(t.background, "#101010");
        assert_eq!(t.color3, "#ffff00");
    }

    #[test]
    fn load_without_file_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(Some(dir.path())), Theme::default());
    }

    #[test]
    fn load_without_home_gives_default() {
        assert_eq!(load_from(None), Theme::default());
    }
}
