//! Reading and changing Hyprland-style config files in place: `key = value`
//! lines inside `name { ... }` blocks. Comments, order and everything else in
//! the file stay as they were, so hand edits and Settings can share a file.

/// Where the Hyprland config lives (`VECTOR_SETTINGS_HYPR` overrides it for tests).
pub fn hypr_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("VECTOR_SETTINGS_HYPR") {
        return d.into();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(home).join(".config/hypr")
}

fn strip_comment(s: &str) -> &str {
    // Hyprland: `#` starts a comment, `##` is a literal #
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' {
            if i + 1 < b.len() && b[i + 1] == b'#' {
                i += 2;
                continue;
            }
            return &s[..i];
        }
        i += 1;
    }
    s
}

/// One parsed line.
enum L<'a> {
    Open(&'a str),
    Close,
    Set { key: &'a str, value: &'a str, commented: bool },
    Other,
}

fn parse(line: &str) -> L<'_> {
    let t = line.trim();
    let (commented, body) = match t.strip_prefix('#') {
        Some(r) => (true, r.trim()),
        None => (false, t),
    };
    let code = if commented { body } else { strip_comment(body).trim() };
    if !commented {
        if let Some(name) = code.strip_suffix('{') {
            return L::Open(name.trim());
        }
        if code == "}" {
            return L::Close;
        }
    }
    if let Some((k, v)) = code.split_once('=') {
        let k = k.trim();
        if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':' || c == '.') {
            let v = if commented { strip_comment(v) } else { v };
            return L::Set { key: k, value: v.trim(), commented };
        }
    }
    L::Other
}

/// A config file held as lines.
pub struct Conf {
    pub lines: Vec<String>,
}

impl Conf {
    pub fn parse(text: &str) -> Conf {
        Conf {
            lines: text.lines().map(str::to_string).collect(),
        }
    }

    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    /// (line index, depth path) for every line.
    fn walk(&self) -> Vec<Vec<String>> {
        let mut path: Vec<String> = Vec::new();
        let mut out = Vec::with_capacity(self.lines.len());
        for l in &self.lines {
            match parse(l) {
                L::Open(n) => {
                    out.push(path.clone());
                    path.push(n.to_string());
                }
                L::Close => {
                    path.pop();
                    out.push(path.clone());
                }
                _ => out.push(path.clone()),
            }
        }
        out
    }

    /// The value of `key` directly inside the block `path` (uncommented only).
    pub fn get(&self, path: &[&str], key: &str) -> Option<String> {
        let paths = self.walk();
        let mut found = None;
        for (i, l) in self.lines.iter().enumerate() {
            if let L::Set { key: k, value, commented: false } = parse(l)
                && k == key
                && paths[i] == path
            {
                found = Some(value.to_string());
            }
        }
        found
    }

    /// Sets `key = value` inside `path`: the live line is changed, else a
    /// commented-out one is switched on, else a line is added at the end of the
    /// block (the block is created when missing).
    pub fn set(&mut self, path: &[&str], key: &str, value: &str) {
        let paths = self.walk();
        let mut live = None;
        let mut commented = None;
        for (i, l) in self.lines.iter().enumerate() {
            if paths[i] != path {
                continue;
            }
            if let L::Set { key: k, commented: c, .. } = parse(l)
                && k == key
            {
                if c {
                    commented.get_or_insert(i);
                } else {
                    live = Some(i);
                }
            }
        }
        let indent = "  ".repeat(path.len());
        if let Some(i) = live.or(commented) {
            let old = &self.lines[i];
            let ws: String = old.chars().take_while(|c| c.is_whitespace()).collect();
            // keep a trailing comment on a live line
            let tail = if live.is_some() {
                let t = old.trim_start();
                let code = strip_comment(t);
                t[code.len()..].to_string()
            } else {
                String::new()
            };
            let tail = if tail.is_empty() { tail } else { format!(" {}", tail.trim_start()) };
            self.lines[i] = format!("{ws}{key} = {value}{tail}");
            return;
        }
        // the end of the block, or create it
        match self.block_end(path, &paths) {
            Some(end) => self.lines.insert(end, format!("{indent}{key} = {value}")),
            None => {
                let (parent, name) = path.split_at(path.len() - 1);
                let pi = "  ".repeat(parent.len());
                let block = vec![format!("{pi}{} {{", name[0]), format!("{indent}{key} = {value}"), format!("{pi}}}")];
                let at = if parent.is_empty() {
                    if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                        self.lines.push(String::new());
                    }
                    self.lines.len()
                } else {
                    // make sure the parent exists, then insert at its end
                    if self.block_end(parent, &paths).is_none() {
                        self.set(parent, "__vs_tmp", "0");
                        let p2 = self.walk();
                        let idx = self.lines.iter().enumerate().position(|(i, l)| {
                            p2[i] == parent && matches!(parse(l), L::Set { key: "__vs_tmp", .. })
                        });
                        if let Some(i) = idx {
                            self.lines.remove(i);
                        }
                    }
                    let p2 = self.walk();
                    self.block_end(parent, &p2).unwrap_or(self.lines.len())
                };
                for (j, l) in block.into_iter().enumerate() {
                    self.lines.insert(at + j, l);
                }
            }
        }
    }

    /// Index of the closing `}` line of the first block at `path`.
    fn block_end(&self, path: &[&str], paths: &[Vec<String>]) -> Option<usize> {
        if path.is_empty() {
            return Some(self.lines.len());
        }
        let mut inside = false;
        for (i, l) in self.lines.iter().enumerate() {
            let here: Vec<&str> = paths[i].iter().map(String::as_str).collect();
            match parse(l) {
                L::Open(n) if !inside && here == path[..path.len() - 1] && n == path[path.len() - 1] => inside = true,
                L::Close if inside && here == path[..path.len() - 1] => return Some(i),
                _ => {}
            }
        }
        None
    }
}

/// Reads a file, changes it, writes it back atomically (Hyprland reloads by itself).
pub fn edit(file: &std::path::Path, f: impl FnOnce(&mut Conf)) -> std::io::Result<()> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut c = Conf::parse(&text);
    f(&mut c);
    let new = c.text();
    if new == text {
        return Ok(());
    }
    let tmp = file.with_extension("vector-settings.tmp");
    std::fs::write(&tmp, new)?;
    std::fs::rename(&tmp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = "# top\ninput {\n  kb_layout = us\n  kb_options = compose:caps # ,grp\n  # sensitivity = 0.35\n\n  touchpad {\n    natural_scroll = true\n  }\n}\n\ngesture = 3, left, dispatcher, exec, x\n";

    #[test]
    fn reads_nested_values() {
        let c = Conf::parse(INPUT);
        assert_eq!(c.get(&["input"], "kb_layout").as_deref(), Some("us"));
        assert_eq!(c.get(&["input"], "kb_options").as_deref(), Some("compose:caps"));
        assert_eq!(c.get(&["input", "touchpad"], "natural_scroll").as_deref(), Some("true"));
        assert_eq!(c.get(&["input"], "sensitivity"), None);
        assert_eq!(c.get(&["input"], "natural_scroll"), None);
    }

    #[test]
    fn changes_keep_comments() {
        let mut c = Conf::parse(INPUT);
        c.set(&["input"], "kb_options", "compose:ralt");
        assert!(c.text().contains("  kb_options = compose:ralt # ,grp\n"));
        c.set(&["input"], "sensitivity", "0.2");
        assert!(c.text().contains("  sensitivity = 0.2\n"));
        assert!(!c.text().contains("# sensitivity"));
    }

    #[test]
    fn adds_missing_keys_and_blocks() {
        let mut c = Conf::parse(INPUT);
        c.set(&["input", "touchpad"], "scroll_factor", "0.5");
        assert_eq!(c.get(&["input", "touchpad"], "scroll_factor").as_deref(), Some("0.5"));
        assert!(c.text().contains("    natural_scroll = true\n    scroll_factor = 0.5\n  }\n"));
        c.set(&["input", "touchpad"], "tap-to-click", "false");
        c.set(&["cursor"], "inactive_timeout", "3");
        assert_eq!(c.get(&["cursor"], "inactive_timeout").as_deref(), Some("3"));
        let mut d = Conf::parse("gesture = 3, left\n");
        d.set(&["input", "touchpad"], "natural_scroll", "true");
        assert_eq!(d.get(&["input", "touchpad"], "natural_scroll").as_deref(), Some("true"));
        assert!(d.text().contains("input {\n  touchpad {\n    natural_scroll = true\n  }\n}\n"), "{}", d.text());
        // the rest is untouched
        assert!(c.text().starts_with("# top\ninput {\n  kb_layout = us\n"));
        assert!(c.text().contains("gesture = 3, left, dispatcher, exec, x"));
    }
}
