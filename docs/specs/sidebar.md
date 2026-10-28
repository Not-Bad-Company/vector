# Sidebar: bookmarks.rs (core) and sidebar.rs (app)

## crates/vector-core/src/bookmarks.rs (pure, unit-tested)

GTK bookmarks file (`~/.config/gtk-3.0/bookmarks`): one per line, `URI[ label]`, URI
percent-encoded, e.g. `file:///tmp/my%20dir Some label`.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark { pub path: std::path::PathBuf, pub label: String }
/// Only `file://` URIs; others skipped. Label = text after the first space, else the last path
/// component (or "/" for root). Percent-decoding per byte (names need not be UTF-8). Blank and
/// malformed lines skipped. Trailing slash removed except for "/".
pub fn parse(text: &str) -> Vec<Bookmark>;
/// `$XDG_CONFIG_HOME/gtk-3.0/bookmarks` (glib::user_config_dir()).
pub fn default_path() -> std::path::PathBuf;
```
Tests: plain, label, %20 and %C3%A9 decoding, invalid escape skipped, non-file (sftp://, smb://)
skipped, root, trailing slash, blank lines, CRLF.

## crates/vector/src/sidebar.rs (GTK)

```rust
pub struct Sidebar { /* private */ }
impl Sidebar {
    pub fn new(on_open: impl Fn(std::path::PathBuf) + 'static) -> std::rc::Rc<Sidebar>;
    pub fn widget(&self) -> &gtk::Widget;   // vertical box in a ScrolledWindow, class .sidebar, width ~200
    /// Marks the row whose path equals `path` with `.active`, others not.
    pub fn set_current(&self, path: &std::path::Path);
}
```

Sections, each with a lowercase `.section` label: `places`, `devices`, `bookmarks` (omit a section
when empty). Rows: horizontal box `.row` with an icon label (`.icon`, Nerd Font glyph from
`vector_core::icons::icon_for(name, true, false)`; home `\u{F02DC}` nf-md-home, root `\u{F02CA}`
nf-md-harddisk, drives `\u{F02CA}`, usb `\u{F11F0}` nf-md-usb_flash_drive if sure, network
`\u{F0318}` nf-md-lan) and a name label (ellipsize end). Click (GestureClick released) → `on_open(path)`.
Pointer cursor on rows.

- places: home (`glib::home_dir()`), then XDG dirs that exist (`glib::user_special_dir`:
  Desktop, Documents, Downloads, Music, Pictures, Videos) labelled lowercase-free as their folder
  name, then `/` labelled "root".
- devices: `gio::VolumeMonitor::get()`: every mount with a local `root().path()` (name =
  `mount.name()`), plus volumes without a mount (`volume.get_mount().is_none()`, `can_mount()`)
  — clicking those mounts them (`volume.mount(MountMountFlags::NONE, None::<&gio::MountOperation>,
  None::<&gio::Cancellable>, cb)`) and then opens the new mount's root; errors are ignored
  quietly (log with `glib::g_warning!` is fine). Mounts that `can_unmount()`/`can_eject()` get a
  small hover-only `button.link.x` with `\u{F01EA}` (nf-md-eject) that ejects/unmounts
  (`eject_with_operation` / `unmount_with_operation`, async). Rebuild the section on
  `mount-added`, `mount-removed`, `mount-changed`, `volume-added`, `volume-removed`.
- bookmarks: `vector_core::bookmarks::parse` of `default_path()` read at startup (read the file
  with `gio::File::load_contents_future` — async — not on the main thread synchronously), and
  re-read when a `gio::FileMonitor` on that file fires. Do NOT check whether bookmark targets
  exist (don't stat them).
- Never block the main thread. No unwrap on I/O.

## Privacy

Do not read the real `~/.config/gtk-3.0/bookmarks`, don't run the app, don't list any real
folders. Unit tests use inline strings only.
