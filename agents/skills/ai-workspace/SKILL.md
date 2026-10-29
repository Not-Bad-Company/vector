---
name: ai-workspace
description: Use before ANY GUI or desktop work on this laptop - launching an app or window, testing a GTK/Qt/Electron/Vector app, taking a screenshot, clicking or typing into a window, testing clipboard or notifications, a test instance of vector-bar/vector-space/vector-find, or needing test images/videos/documents/files. Everything runs on the AI workspace (workspace 0, a headless output) via `aiws`, never on the owner's screen and never with their own files.
---

# AI workspace (`aiws`)

The owner works on workspaces 1-9 of their laptop screen. Workspace 0 (Hyprland
id 10) belongs to AI: an invisible headless output named `AI`. The owner can
watch it with SUPER+0. Rule: nothing an agent does may appear on, focus, or read
from the owner's screen, clipboard, notifications or personal files.

## Pick the tool

| Need | Use |
|---|---|
| Open an app, look at it | `aiws run CMD...`, `aiws wait TEXT`, `aiws shot` |
| Keys/text into a window, no mouse | `aiws windows`, `aiws key ADDR "CTRL, L"`, `aiws type ADDR "text"` |
| Real mouse clicks, drags, scrolling, focus, clipboard | `aiws nest ...` (nested compositor) |
| Test instance of a Vector app or anything using D-Bus/XDG dirs | `eval "$(aiws env)"` first, same command |
| Test files (images, video, audio, docs, archives, folder trees) | `aiws make ...` |
| Websites (logged in or bot-protected), browser automation | the `ai-browser` skill: `aiws browser` |

## Recipes

Open and inspect an app:
```sh
aiws run myapp --flag          # opens on workspace 0, never takes focus; prints pid + log
aiws wait myapp                # waits until its window is there, prints its address
aiws shot /path/shot.png       # then Read the png
```

Isolated test instance (own XDG dirs, private D-Bus, GTK animations off):
```sh
eval "$(aiws env)"; VECTOR_SPACE_TEST=1 aiws run vector-space-daemon
# when done: kill $AIWS_DBUS_PID; aiws clean
```
Shell state does not carry over between tool calls: put `eval "$(aiws env)"` in
every command that needs the sandbox, or save the printed exports to a file
and `source` it.

Real input (clicks, drags, copy/paste) in the nested compositor:
```sh
aiws nest run myapp            # starts the nested Hyprland on first use
aiws nest windows              # positions: "at X,Y size WxH"
aiws nest shot /path/n.png     # its own screen; coordinates match clicks
aiws nest click 400 120        # [right|middle]; also move, drag X1 Y1 X2 Y2, scroll X Y N
aiws nest type "hello"; aiws nest key ctrl+a; aiws nest key Return
eval "$(aiws nest env)"; wl-paste   # the nested clipboard, never the owner's
aiws nest stop                 # when done
```

Test files (never the owner's photos, videos or documents):
```sh
aiws make jpg:12 clip.mp4 song.mp3 report.pdf data.csv bundle.zip tree
# --size WxH, --secs N, --dir D. Default dir: $AIWS_SANDBOX/files or $XDG_RUNTIME_DIR/aiws/files
```

Always clean up before you stop: `aiws close all`, `aiws nest stop`, kill your sandbox bus, `aiws clean`.

## Gotchas

- Layer-shell surfaces open on the focused output. Vector apps follow
  `VECTOR_OUTPUT=AI` (set by `aiws run` and `aiws env`) and then never grab the keyboard.
- GTK fade-in animations freeze on the headless output. `aiws env` turns them off.
- Something starting from a D-Bus-activated or already-running app (`xdg-open`,
  an open Zen) lands in the owner's app. Start your own instance with `aiws run`.
- Never use `hyprctl dispatch exec`, `xdg-open`, `notify-send`, `wl-copy` or
  `wl-paste` on the real session, or `ydotool`. The aiws-guard hook blocks most
  of these and says what to use instead. Only when the owner explicitly asked
  for exactly that action, add `# owner-asked` to the command.
- `pkill -f PATTERN` can match your own shell. Kill by pid or check `/proc/PID/cmdline`.
- Every agent shell has `AI_WORKSPACE=1`. The aiws daemon moves any window from
  such a process to workspace 0, so a forgotten `aiws run` still won't land
  on the owner's screen. It may flash briefly, though, so use `aiws run`.
