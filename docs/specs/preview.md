# preview.rs — the preview pane

File: `crates/vector/src/preview.rs` (GTK app crate). Crates available: `gtk4` (as `gtk`, re-exports
`gdk`, `gdk_pixbuf`, `glib`, `gio`), `sourceview5`, `vector-core` (`entry::Entry`, `theme::Theme`,
`icons::icon_for`). CSS classes already exist in `crates/vector/src/style.rs`: use `.preview` on
the root, `.title` for the name, `.dim` for meta text, `.empty` for "no preview"; pictures get the
`.preview picture` border automatically; text views get `.preview textview`.

```rust
pub struct Preview { /* private */ }
impl Preview {
    pub fn new(theme: &vector_core::theme::Theme) -> std::rc::Rc<Preview>;
    pub fn widget(&self) -> &gtk::Widget;            // a vertical gtk::Box, min width ~320
    /// Shows `entry` (None = clear). Cancels/ignores any load still running for an older entry.
    pub fn show(self: &std::rc::Rc<Self>, entry: Option<&vector_core::entry::Entry>);
}
```

Layout: header line with `icon_for(..)` and the name (`.title`, ellipsized middle), a dim lowercase
meta line `"12.3 kb · text/plain · 28 oct 2026 14:02"` (folders: `"N items"` once known), then the
body filling the rest:

- **Images** (`content_type` starts with `image/`): first a freedesktop thumbnail if one exists:
  `$XDG_CACHE_HOME/thumbnails/{x-large,large,normal}/<md5 of the file:// URI>.png`
  (`glib::user_cache_dir()`, `glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)`),
  used only if its mtime ≥ the file's mtime. Otherwise load the image itself scaled to fit
  512×512 with `gdk_pixbuf::Pixbuf::from_file_at_scale` inside `gio::spawn_blocking`, convert to
  `gdk::Texture::for_pixbuf` on the main thread. `gtk::Picture` with `ContentFit::Contain`.
  SVG and huge images: same path (pixbuf scales on load).
- **Video** (`video/*`): `ffmpegthumbnailer -i <path> -o <out.png> -s 512 -t 10%` via
  `gio::Subprocess` (async, `wait_check_future`), out in `glib::user_cache_dir()/vector/thumbs/
  <md5 uri>-<mtime>.png` (reuse if present). Then load like an image.
- **PDF** (`application/pdf`): `pdftoppm -png -singlefile -f 1 -l 1 -scale-to 768 <path> <base>`
  (writes `<base>.png`), same cache dir, then like an image.
- **Text**: content type is-a `text/plain` (`gio::content_type_is_a`) or one of json, xml,
  javascript, x-shellscript, toml, yaml, x-desktop… or unknown type with no NUL byte in the
  first 8 KiB. Read at most 64 KiB in `gio::spawn_blocking`, decode lossily; NUL found → treat
  as "no preview". Show in a read-only `sourceview5::View` (no cursor, monospace, wrap off,
  line numbers on, horizontal + vertical scroll), language from
  `sourceview5::LanguageManager::default().guess_language(Some(file_name), Some(content_type))`,
  highlighting on. Style scheme: generate once an XML style scheme named `vector` from the theme
  (text = foreground on transparent/no background, `def:comment` foreground at ~50% alpha
  (`#rrggbb80`), `def:keyword`/`def:preprocessor` accent, `def:string` color2, `def:number`,
  `def:constant`, `def:special-char` color3, `def:type` accent, `def:function` foreground bold,
  `def:error` color1, line numbers at 40% alpha, current line no highlight). Write it to
  `glib::user_cache_dir()/vector/styles/vector.xml`, `StyleSchemeManager::default()
  .append_search_path(dir)`, then `buffer.set_style_scheme(scheme("vector"))`.
- **Folders**: up to 200 child names (icon + name, dim for hidden) using
  `vector_core::listing::list_blocking` in `gio::spawn_blocking`, sorted with
  `vector_core::sort::compare` and `SortSpec::default()`, hidden ones excluded; meta shows count.
- **Everything else**, errors, broken symlinks: big icon + `.empty` label "no preview".

Rules:
- Never block the main thread: every file read and subprocess is async / `spawn_blocking`.
- A generation counter (`Cell<u64>`) makes stale results be dropped when the user moves on fast.
- Missing `ffmpegthumbnailer`/`pdftoppm` → "no preview", no panic, no unwrap on I/O.
- Keep it simple and quiet: no buttons, no borders besides the CSS ones, no extra colours.
