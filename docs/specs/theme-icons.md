# theme.rs and icons.rs

Two small pure modules. Files: `crates/vector-core/src/theme.rs`, `crates/vector-core/src/icons.rs`.

## theme.rs

Colours come from `~/.config/omarchy/current/theme/colors.toml`, read at startup, so a theme change
carries over. Lines look like `accent = "#81a1c1"`. Only `key = "#rrggbb"` lines matter;
everything else is ignored.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub background: String, // "#rrggbb"
    pub foreground: String,
    pub accent: String,
    pub color1: String,     // red / critical
    pub color2: String,     // green
    pub color3: String,     // yellow / warning
}
impl Default for Theme  // background "#1e1e2e", foreground "#cdd6f4", accent "#81a1c1",
                        // color1 "#bf616a", color2 "#a3be8c", color3 "#ebcb8b"
impl Theme {
    pub fn parse(text: &str) -> Theme;      // defaults for missing keys; ignores bad values
    pub fn load() -> Theme;                 // reads $HOME/.config/omarchy/current/theme/colors.toml,
                                            // Default on any error
}
```
Accept `#rrggbb` case-insensitively, store lowercase. Ignore `#rgb`, `0x…`, unquoted, etc.
Tests: full file, missing keys, garbage lines, comments, whitespace variations, uppercase.
Do not read the real colors.toml in tests.

## icons.rs

Nerd Font (Material Design `nf-md-*`, codepoints U+F0001..U+F1AF0) icons, one per entry.

```rust
pub fn icon_for(name: &str, is_dir: bool, is_symlink: bool) -> &'static str;
```

- Folders: generic folder `\u{F024B}`; special names (case-insensitive): Documents `\u{F0C82}`,
  Downloads `\u{F024D}`, Pictures `\u{F024F}`, Music `\u{F1359}`, Videos `\u{F1A1A}`?  — check each
  codepoint is a real nf-md glyph of the right meaning (look the names up in the Nerd Fonts
  cheat sheet data if you know it; otherwise use generic folder for that one), `.git` (git icon),
  `node_modules`, `.config`, Desktop, Projects/src. Symlinked folder: `\u{F0257}` (folder-link)
  or similar.
- Files by extension (case-insensitive): images, video, audio, pdf, archives (zip, tar, gz, xz,
  zst, 7z, rar), text/markdown, code (rs, py, js, ts, c, h, cpp, go, java, lua, sh/bash/zsh/fish,
  html, css, json, toml, yaml/yml, xml, sql), office docs (odt, docx, ods, xlsx, odp, pptx),
  iso/img, exe/appimage, lock files, and special names (Makefile, Dockerfile, Cargo.toml,
  LICENSE, README*, .gitignore). Fallback file `\u{F0214}`. Symlinked file: a link icon.
- A `const` table or `match`, no allocation per call.

Tests: each category, case-insensitivity, multi-dot names (`a.tar.gz` → archive), dotfiles with
no extension (`.bashrc`), name without extension, folder special names, symlink variants.
