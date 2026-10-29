# Vector Bar — design

The top bar of Vector OS. Replaces Waybar, the `bar-panel` Python daemon and the
~25 `~/.config/waybar/scripts/*` JSON printers with one native Rust + GTK4 program,
in the same visual language as Vector Find and Vector View.

## Why

- Waybar crashes now and then; every module is a shell/Python script printing JSON
  on an interval or a `SIGRTMIN+N` poke, so state is laggy, spawns a lot of
  processes and can't have real interaction.
- The drop-down panels live in a separate Python daemon that has to guess where
  the icon was (mouse x) and can't talk to the bar.
- The left button is still the Omarchy logo + `omarchy-menu`.

## Goals

1. Feature parity with today's Waybar setup (every module, every click,
   every tooltip, every panel). Nothing is lost when it replaces Waybar.
2. Better UI: one panel per module, opened right under its icon, with live data,
   real controls (toggles, sliders, lists) instead of walker menus and
   notify-send readouts. Hover tooltips stay for glanceable detail.
3. Event-driven where the system offers events (Hyprland IPC, BlueZ/iwd D-Bus,
   MPRIS, UPower, mako), cheap in-process polling elsewhere. No process per tick
   for things readable from /sys or /proc.
4. One **AI** entry: Claude (claude2, claude3; claude1 disabled), Codex (3 accounts)
   and Antigravity in a single overview. Spends no usage to read usage.
5. Customisable: `~/.config/vector-bar/config.toml` (layout, per-module options,
   click bindings, custom modules), `~/.config/vector-bar/style.css` (user CSS on
   top of the built-in sheet), live reload of both and of the Omarchy theme.
6. Robust: a module that fails shows its error state, never takes the bar down;
   runs as a supervised systemd user service.
7. Vector menu replaces the Omarchy logo/menu.

## Non-goals (for now)

- Porting all of `omarchy-menu` (install/remove packages, web apps…). The Vector
  menu covers daily use; "more" falls back to the relevant omarchy-menu submenu.
- Multi-monitor layouts beyond "same bar on every monitor".

## Anti-goals

- Anything that makes the bar slower, uglier or flakier than Waybar.
- Refreshing other tools' OAuth tokens (would log running Claude Code/Codex out).

## Architecture

- `vector-bar` binary. `vector-bar` (no args) runs the bar as a single-instance
  `GtkApplication` (`org.vectoros.Bar`). Any other invocation talks to the running
  instance over D-Bus actions:
  `vector-bar toggle <panel>[@center]`, `vector-bar refresh <module>`,
  `vector-bar reload`.
- One layer-shell window per monitor, top, exclusive zone, namespace `vector-bar`.
- Modules (`src/modules/*`): each owns its bar widget(s), tooltip, optional panel
  and its data source. Work that blocks (HTTP, subprocess, file scans) runs on a
  worker thread; results come back to the GTK thread over a channel.
- Panels: one layer-shell overlay surface per open panel with the card placed
  under the module's own widget (exact x from the widget, not the mouse);
  click outside / Esc / clicking the icon again closes. Keyboard mode
  on-demand so Wi-Fi passwords and clipboard search can be typed.
- Shared "sheet" widgets (`ui.rs`): header, section, key/value row, meter,
  toggle row, list row, hint line. Same layout rules as `tooltip-lib.sh`
  (accent bold title, dim lowercase labels, colour only for attention).

## Modules (parity checklist)

left: vector menu, workspaces 1–8 persistent (+ any extra), click to activate,
scroll to switch.
center: clock (format/alt format, right tz select), media (MPRIS; click
play-pause, right next, scroll skip), dividers that hide themselves, indicators:
voxtype, screen recording, idle inhibit, sleep lock, win11 VM, notification silencing.
right: pi + nas drive, phone (phonectl), Nothing buds (earctl), Sony (sonyctl),
AI, clipboard, notifications, bluetooth, vpn, network, sysmon, battery;
optional tray (StatusNotifierItem), custom exec modules (Waybar JSON format, so
any old script still works).

## Cut-over

1. Run side by side for testing (`vector-bar --test` = no exclusive zone).
2. Swap: `vector-bar.service` replaces `waybar.service`; `~/.local/bin/waybar`
   shim starts `vector-bar.service` (so `omarchy-restart-waybar`/toggle still
   work); signal senders (`pkill -RTMIN+N waybar`) switch to
   `vector-bar refresh <module>`; Hyprland glass layer rules get the
   `vector-bar` namespace; `bar-panel` keybind moves to `vector-bar toggle`.
3. Waybar config and bar-panel stay on disk (unused) for a rollback:
   `vector-bar-rollback` re-enables waybar.service.
