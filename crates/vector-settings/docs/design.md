# vectorOS Settings

One window for the system: a sidebar of pages, each page one area. It replaces
the TUIs Omarchy opens (impala, bluetui, wiremix) and, page by page, hand-edited
config files: a setting you can change here is written to the same file you
would edit, so editing by hand keeps working.

```
vector-settings [PAGE]     open or raise the window on PAGE (single instance)
vector-settings --list     page names
```

Keys: Esc / Ctrl+W close, Ctrl+PgUp / Ctrl+PgDn previous / next page,
Ctrl+R (or F5) rescans on the Wi-Fi page.

## Look

The Vector look (`vector_core::style`) plus `css()` in `main.rs`: a sidebar
with groups, page title in the accent colour with a dim detail line under it,
sections as dim lowercase headings over bordered groups of rows. Rows: icon,
name, dim meta on the right, then controls; a row expands under itself for
passwords, sign-in forms and details. Destructive actions ("forget") take two
clicks. Nothing rebuilds a row the user is typing in or dragging (`ui::Keyed`
compares a signature per row).

## Pages

Each page is `fn page(&Ctx) -> View` in its own file, listed in `pages()`.
`View` hooks: `on_show` (refresh, register D-Bus agents, start scans),
`on_hide` (stop them), `on_stop` (window closes), `every(ms)` (timers that
run only while the page is in view). Nothing polls while a page is hidden or
the window is closed.

| page | backend | status |
|---|---|---|
| wi-fi | iwd over D-Bus; agent while in view; enterprise files via pkexec | done |
| bluetooth | BlueZ over D-Bus; pairing agent and discovery while in view | done |
| sound | PipeWire through `pactl` (JSON), `pactl subscribe` while in view | done |
| about | fastfetch JSON, `vector-update/install/remove` | done |
| display | `hyprctl monitors`, writes `~/.config/hypr/monitors.conf` | planned |
| power | power-profiles-daemon / asusctl, hypridle timeouts | planned |
| keyboard & touchpad | `~/.config/hypr/input.conf` in place (`conf.rs`): layouts, compose, repeat, touchpad, pointer | done |
| appearance | themes, backgrounds, fonts (the Vector Space galleries) | planned |
| bar | `~/.config/vector-bar/config.toml` (modules, order) | planned |
| default apps | `mimeapps.list`, `xdg-terminals.list` | planned |
| pi | `ssh HOST STATUS` (one read-only JSON command, once a minute while in view), `~/.config/vectoros/pi.json` | done |
| ai | `crew who --json`, `aiws`, guard and aisudo files | done |

## Wi-Fi notes

- iwd allows one password agent. The page registers its agent only while in
  view; the bar's Wi-Fi panel does the same while open. If the other one holds
  it, the page says so.
- Enterprise (eduroam): iwd reads sign-in details from
  `/var/lib/iwd/<name>.8021x`. The page writes it once with
  `pkexec install -m600` (PEAP/MSCHAPv2, TTLS/PAP, TTLS/MSCHAPv2), then
  connects. Names with characters other than letters, digits, space, `-` and
  `_` use iwd's `=hex` file name.
- Hidden networks: `Station.ConnectHiddenNetwork`; the password goes through
  the same agent.

## Pi notes

The page knows nothing about one particular server: `~/.config/vectoros/pi.json`
names the ssh host, the status command, the admin and site URLs and which
systemd units may be started from the page (`actions`, two clicks, with a
warning text). The status command prints JSON with `host`, `services`,
`timers` and optionally `kw` (the site's own database numbers), `umami`
(visits) and `backup`; sections without data are left out. The owner's
command (`kw-status`) is private, in their dotfiles, copied to the server's
`~/bin`; it caches its answer 30 s and only reads.
