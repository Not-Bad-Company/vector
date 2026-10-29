# Rules for AI agents (paste into ~/.claude/CLAUDE.md, ~/.codex/AGENTS.md, ...)

## AI workspace: never work on the owner's screen

Load the `ai-workspace` skill before any GUI, screenshot or test-file work and
the `ai-browser` skill before any web browsing. Workspace 0 (Hyprland id 10)
belongs to AI agents. It lives on an invisible
headless output named `AI`; the owner can watch it (SUPER+0) but works on the
other workspaces. Every GUI thing an agent opens, tests or screenshots goes
there, never on the screen the owner is using. Tool: `aiws` (`aiws help`).

- Start apps: `aiws run CMD ARGS...` (opens on workspace 0 without taking
  focus; prints pid and log), then `aiws wait [TEXT]` until its window is
  there. Don't launch GUI apps any other way, and never with
  `hyprctl dispatch exec`, `xdg-open`, `notify-send` to the real session, or
  by talking to an app the owner already has open (their Zen, Outlook...).
- Isolate test instances: `eval "$(aiws env)"` first gives scratch XDG dirs, a
  private D-Bus and GTK animations off (they freeze on the headless output).
  Real clipboard, notifications, files and browser profiles stay untouched.
- Test files: `aiws make SPEC...` makes synthetic ones, never use the owner's
  own photos, videos or documents. `aiws make jpg:12 clip.mp4 song.mp3
  report.pdf data.csv bundle.zip tree` (images, video, audio, text, office
  docs, archives; `tree` = a folder for file-manager tests). Options
  `--size WxH`, `--secs N`, `--dir D`; default dir is the sandbox's `files/`.
- Browser: `aiws browser [URL]` = the AI browser, Chromium on workspace 0 with
  its own persistent profile (the owner logs in there once, logins stay) and
  CDP on http://127.0.0.1:9333. The `ai-browser` MCP tools drive it (start it
  first); scripts use `chromium.connectOverCDP("http://127.0.0.1:9333")`.
  It is a normal browser to websites (not flagged as automated), so sites
  that block bots work. It replaces Claude in Chrome: same abilities, one
  shared browser for every Claude account and for Codex/Antigravity (CDP).
  Plain page checks without logins: a headless
  Playwright script. Never use or copy the owner's own browser profiles or
  cookies; only visit what the task needs.
- See: `aiws shot [FILE]` (png of workspace 0). List: `aiws windows`.
- Input without focus: `aiws key ADDR "CTRL, L"`, `aiws type ADDR "text"`.
  Never click on the owner's own pointer (ydotool and the like move it).
- Real mouse, keyboard, focus and clipboard: `aiws nest`, a nested Hyprland
  in a window on workspace 0 with its own seat. `aiws nest run CMD`, then
  `nest windows` (positions), `nest click X Y [right]`, `nest drag`,
  `nest scroll`, `nest type TEXT`, `nest key ctrl+l`, `nest shot`.
  Clipboard tests: `eval "$(aiws nest env)"` then wl-copy/wl-paste go to the
  nested clipboard. Use it for anything needing clicks, drags, focus or
  copy/paste; `aiws nest stop` when done (it costs nothing idle, 150 MB RAM).
- A guard hook (aiws-guard) blocks reading or writing the owner's clipboard,
  notification history, notify-send / xdg-open / hyprctl exec or workspace
  switches on their screen, and private browser data. Its message says what
  to use instead. Only if the owner explicitly asked for exactly that action,
  add `# owner-asked` to the command.
- Layer-shell test instances of Vector apps follow `VECTOR_OUTPUT=AI` (set by
  `aiws run`/`aiws env`) and then never grab the keyboard.
- Clean up before you stop, always: close the windows you opened (`aiws close all`
  also closes the AI browser; logins stay), `aiws nest stop`, kill the
  sandbox's dbus-daemon, then `aiws clean` (made files, logs, dead sandboxes).
- Agent shells have `AI_WORKSPACE=1`; the `aiws` daemon moves any window from
  such a process to workspace 0. Only when the owner explicitly asks to see
  something on their own screen, start it with `AI_WORKSPACE=0`.

## Other agents (crew)

Other AI agents often run at the same time (other windows and accounts,
sub-agents, Codex, Antigravity). `crew` () shows what each is really doing and its hooks stop clashing edits and
git commands by themselves. Load the `crew` skill when a crew warning appears,
when you need something from another agent, or before saying what another
agent is doing. Don't message other agents to announce, report or acknowledge.
External sub-agents join as `<this session>/<label>` and can be messaged with
`crew send <session>/<label> "..."`; `crew watch me --subagents-only` follows
your sub-agents live.

## Root commands and protected files

- Root: never `sudo` (it would wait for a password). Use `aisudo CMD`:
  services (`aisudo systemctl restart X`), logs (`aisudo journalctl -u X`),
  installing/upgrading official packages (`aisudo pacman -S --needed PKG`,
  `-Syu`), reading system files (`aisudo cat /etc/...`) and some diagnostics
  (`aisudo --help` lists everything). Anything else: give the owner the exact
  command to run. If aisudo says it isn't installed: the owner runs
  `sudo ~/.local/bin/aisudo-install` once.
- The guard stops deleting, moving away or overwriting the owner's personal
  files (Pictures, Videos, Music, Documents, Desktop, Downloads), keys and
  dotfile history, and recursive deletes of whole top-level home folders.
  Reading them is allowed but avoid it unless the task needs it. New files
  there are fine. To get rid of something you're unsure about, use
  `gio trash PATH` (recoverable) instead of `rm`.
