# Vector Space — design

The launcher of Vector OS (super+space). Replaces Walker, Vector Bar's clipboard
panel and notification panel, and clipse. One window with modes, in the same
visual language as Vector Find, Vector View and Vector Bar: square corners,
JetBrains Mono, a lualine mode block in the theme accent, dim lowercase detail.

## Modes and keys

Type the prefix into an empty search to switch; backspace on an empty search
goes back to apps. tab / shift+tab cycle, ctrl+1…7 jump.

| prefix | mode | enter | more |
|---|---|---|---|
| | apps | launch | also open windows, system actions (lock, suspend, log out, restart, shut down), inline sums and conversions, web fallback |
| `;` (`$`) | clipboard | copy | ctrl+enter copy and paste, ctrl+p pin, shift+del delete, ctrl+shift+del twice: clear unpinned |
| `!` | notifications | open (default action, or focus the app) | alt+1…9 action buttons, shift+del dismiss/forget, ctrl+d silence, ctrl+shift+del twice: clear history |
| `:` | emoji | copy | ctrl+enter copy and paste; arrows move in the grid |
| `?` (`@`) | web | search google in zen | live google suggestions, recent searches, typed URLs open directly |
| `=` | calc | copy result | hex/binary of whole results |
| `>` (`#`) | convert | copy, or convert a file | colours, units, currencies (166 + the old euro currencies + coins), unix times; files copied in a file manager (or a clipboard image): image, video, audio, document formats |

Esc clears the search, then closes. A click outside the card closes.

Hyprland: `SUPER SPACE` apps, `SUPER ALT V` clipboard, `SUPER CTRL E` emoji
(`~/.config/hypr/bindings.conf`).

## Architecture

- `vector-space` (`src/main.rs`): the light client. No GTK, so a keybind costs
  ~12 ms. Hands the mode to the daemon over D-Bus (`org.gtk.Actions.Activate`
  `command` on `org.vectoros.Space`); starts the daemon when none runs. Also
  `clip-store` (run by `wl-paste --watch` on every copy), `clip-add MIME`, and
  `to FORMAT FILE...` (the converter without the window).
- `vector-space-daemon` (`src/daemon.rs`): the GTK window, resident
  (`vector-space.service`). One full-screen layer surface (namespace
  `vector-space`, keyboard exclusive) with the card centred. Hidden, not
  destroyed; the empty app search is rebuilt right after each close so opening
  only has to map the surface (~50 ms from keypress).
- Clipboard: own store, `$XDG_DATA_HOME/vector-space/clipboard/history.json`
  plus blobs; every write under `history.lock`. Text, images, file lists, any
  other type; password-manager copies (`x-kde-passwordManagerHint`,
  `CLIPBOARD_STATE=sensitive`) are never kept. 750 unpinned entries. clipse's
  history was imported on first start.
- Notifications: Vector Bar stays the server; it serves
  `org.vectoros.Notifications` at `/org/vectoros/Notifications` (List,
  Activate, Invoke, Dismiss, ClearHistory, MarkSeen, Get/SetDnd, Changed).
- Converter: ImageMagick, ffmpeg, LibreOffice, pandoc, pdftotext. Results go
  next to the original (`name.ext`, else `name (1).ext`; clipboard images to
  ~/Pictures), onto the clipboard, and a notification says where.
  Rates: open.er-api.com and CoinGecko, cached 12 h in `$XDG_CACHE_HOME/vector-space`.
- No configuration file by design. Colours from the Omarchy theme, live.

## Testing

`VECTOR_SPACE_TEST=1` runs a separate instance (`org.vectoros.SpaceTest`) that
records nothing. Point `XDG_DATA_HOME`, `XDG_STATE_HOME`, `XDG_CACHE_HOME` at a
scratch folder, fill it with `vector-space clip-add`, and run notifications on a
private bus (`dbus-daemon --session --fork`) with a silenced `vector-bar --test`
whose config has an empty layout. Never test on real history.
