//! The stylesheet: the visual language of the bar's drop-down panels (~/.local/libexec/bar-panel):
//! square corners, JetBrains Mono, accent only for titles and the active state, dim lowercase
//! secondary text, rows with a left accent border, a 97% background with a quiet 1px border.
//! Colours come from the Omarchy theme at startup.

use vector_core::theme::Theme;

pub const FONT: &str = "JetBrainsMonoNL Nerd Font";

pub fn css(t: &Theme) -> String {
    let (bg, fg, acc) = (&t.background, &t.foreground, &t.accent);
    let (red, green, yellow) = (&t.color1, &t.color2, &t.color3);
    format!(
        r#"
window.vector {{
  background: transparent;
  color: {fg};
  font-family: '{FONT}';
  font-size: 12px;
}}
.root {{ background: alpha({bg}, 0.97); }}
window.vector * {{ border-radius: 0; outline: none; -gtk-icon-shadow: none; }}
.title {{ color: {acc}; font-weight: bold; }}
.dim {{ opacity: 0.6; }}
.faint {{ opacity: 0.32; }}
.crit {{ color: {red}; }}
.warn {{ color: {yellow}; }}
.good {{ color: {green}; }}
.ok {{ color: {acc}; }}
.section {{ padding: 8px 16px 4px 16px; opacity: 0.5; }}
.sep {{ background: alpha({fg}, 0.08); min-height: 1px; min-width: 1px; }}
.icon {{ min-width: 22px; }}

/* ── sidebar ── */
.places {{ padding: 8px 0; }}
.sidebar-scroll {{ border-right: 1px solid alpha({fg}, 0.08); background: transparent; }}
scrolledwindow, viewport {{ background: transparent; }}
.row {{
  padding: 7px 16px;
  border-left: 2px solid transparent;
  transition: background 120ms ease;
}}
.row:hover {{ background: alpha({fg}, 0.06); }}
.row.active {{ border-left-color: {acc}; background: alpha({acc}, 0.08); }}
.row.active:hover {{ background: alpha({acc}, 0.13); }}
.row .meta {{ opacity: 0.5; }}

/* ── header: tabs and path bar ── */
.header {{ border-bottom: 1px solid alpha({fg}, 0.08); padding: 0 8px; min-height: 34px; }}
.tabs {{ border-bottom: 1px solid alpha({fg}, 0.08); padding: 0 8px; }}
button.link {{
  background: none; border: none; box-shadow: none; border-radius: 0; outline: none;
  padding: 3px 8px; min-height: 0; min-width: 0;
  color: {fg}; opacity: 0.6;
  transition: opacity 120ms ease, background 120ms ease;
}}
button.link:hover {{ opacity: 1; background: alpha({fg}, 0.07); }}
button.link.primary {{ color: {acc}; opacity: 0.95; }}
button.link label {{ text-decoration: none; }}
button.x {{ opacity: 0; padding: 0 5px; }}
.row:hover button.x {{ opacity: 0.55; }}
.row:hover button.x:hover {{ opacity: 1; }}
button.link.danger:hover {{ color: {red}; }}
.tab {{ border-bottom: 2px solid transparent; }}
.tab > button.link {{ padding: 6px 4px 6px 12px; }}
.tab > button.x {{ padding: 6px 8px 6px 4px; }}
.tab.active {{ border-bottom-color: {acc}; }}
.tab:hover button.x {{ opacity: 0.55; }}
.tab:hover button.x:hover {{ opacity: 1; }}
.crumb-sep {{ opacity: 0.32; }}

entry {{
  background: alpha({fg}, 0.06);
  border: 1px solid alpha({fg}, 0.15);
  border-radius: 0;
  color: {fg};
  min-height: 26px;
  padding: 0 8px;
  box-shadow: none;
  outline: none;
  caret-color: {acc};
}}
entry:focus-within {{ border-color: {acc}; }}
entry selection {{ background: alpha({acc}, 0.35); color: {fg}; }}

/* ── file views ── */
.pane {{ background: transparent; }}
.pane.focused-pane {{ }}
.pane-split {{ border-left: 1px solid alpha({fg}, 0.08); }}
listview, gridview {{ background: transparent; color: {fg}; }}
listview > row {{
  padding: 0;
  border-left: 2px solid transparent;
  background: transparent;
  transition: background 120ms ease;
}}
listview > row:hover {{ background: alpha({fg}, 0.06); }}
/* selected rows are grey (like the panels' .row.sel); the cursor row has the accent */
/* the marks sit on the row's content: selected rows grey (like the panels' .row.sel), the
   cursor row in the accent; an unfocused split pane shows its cursor grey */
listview > row:selected {{ background: transparent; border-left-color: transparent; color: {fg}; }}
listview > row:selected > .file {{ box-shadow: inset 2px 0 alpha({fg}, 0.45); background: alpha({fg}, 0.08); }}
listview > row > .file.cursor {{ box-shadow: inset 2px 0 {acc}; background: alpha({acc}, 0.10); }}
.pane:not(.focused-pane) listview > row > .file.cursor {{ box-shadow: inset 2px 0 alpha({fg}, 0.45); background: alpha({fg}, 0.06); }}
.colheader {{ padding: 6px 16px 5px 16px; border-bottom: 1px solid alpha({fg}, 0.08); }}
.colheader label {{ opacity: 0.6; }}
.colheader label.ok {{ opacity: 1; }}
.file {{ padding: 7px 16px 7px 14px; }}
.file .meta {{ opacity: 0.6; }}
.file.hidden-file .name {{ opacity: 0.6; }}
.file .badge {{ min-width: 14px; }}
.file.marked .icon {{ color: {acc}; }}
.file.cut .name, .file.cut .meta {{ opacity: 0.45; }}
.notice {{
  background: {bg};
  border: 1px solid alpha({fg}, 0.18);
  border-left: 2px solid {acc};
  box-shadow: 0 10px 28px alpha(black, 0.35);
  padding: 8px 16px;
}}
.notice.crit {{ border-left-color: {red}; color: {red}; }}
gridview > child {{
  padding: 10px 6px 8px 6px;
  border-left: 2px solid transparent;
  background: transparent;
}}
gridview > child:hover {{ background: alpha({fg}, 0.06); }}
gridview > child:selected {{ background: alpha({fg}, 0.08); border-left-color: alpha({fg}, 0.45); color: {fg}; }}
gridview > child > .tile.cursor {{ box-shadow: inset 0 -2px {acc}; }}
.tile .big-icon {{ font-size: 34px; min-height: 64px; }}
.tile picture {{ border: 1px solid alpha({fg}, 0.12); }}
.drop-target {{ background: alpha({acc}, 0.10); }}
.empty {{ padding: 14px 16px; opacity: 0.45; }}

/* ── preview ── */
.preview {{ border-left: 1px solid alpha({fg}, 0.08); padding: 14px 16px; }}
.details {{ margin-top: 14px; }}
.preview picture {{ border: 1px solid alpha({fg}, 0.12); }}
.preview textview, .preview textview text {{ background: transparent; color: {fg}; font-family: '{FONT}'; font-size: 11px; }}
.preview textview border, .preview gutter, .preview gutter > * {{ background: transparent; }}

/* ── statusline and command line ── */
.statusline {{ background: alpha({fg}, 0.035); border-top: 1px solid alpha({fg}, 0.08); min-height: 26px; }}
.statusline > label {{ padding: 0 10px; }}
.mode {{ padding: 4px 10px; opacity: 0.6; font-weight: bold; }}
.mode.active {{ background: {acc}; color: {bg}; opacity: 1; }}
.progress {{ min-height: 2px; }}
progressbar trough {{ background: alpha({fg}, 0.08); min-height: 2px; border-radius: 0; }}
progressbar progress {{ background: {acc}; min-height: 2px; border-radius: 0; }}
.pickbar {{ border-top: 1px solid alpha({fg}, 0.08); padding: 6px 10px; }}
.pickbar > .title {{ padding: 0 8px; }}
.pickbar entry {{ margin: 0 8px; }}
.cmdline {{ border-top: 1px solid alpha({fg}, 0.08); padding: 6px 10px; }}
.cmdline entry {{ background: transparent; border: none; padding: 0 4px; min-height: 22px; }}
.cmdline .prompt {{ color: {acc}; font-weight: bold; }}
.completions {{ border-top: 1px solid alpha({fg}, 0.08); padding: 4px 0; }}
.completions .row {{ padding: 4px 16px; }}
.message.crit {{ color: {red}; }}

/* ── dialogs and menus ── */
.card {{
  /* opaque: it sits over file names, not over a wallpaper like the bar panels */
  background: {bg};
  border: 1px solid alpha({fg}, 0.18);
  padding: 14px 0 10px 0;
  box-shadow: 0 10px 28px alpha(black, 0.35);
  color: {fg};
}}
.head {{ padding: 0 16px 10px 16px; }}
.body {{ padding: 0 16px; }}
.foot {{ padding: 10px 10px 0 10px; }}
window.dialog {{ background: transparent; }}
popover > contents, popover.menu > contents {{
  background: alpha({bg}, 0.97);
  border: 1px solid alpha({fg}, 0.18);
  border-radius: 0;
  box-shadow: 0 10px 28px alpha(black, 0.35);
  color: {fg};
  padding: 6px 0;
}}
popover > arrow {{ background: none; border: none; }}
popover.menu modelbutton {{ padding: 6px 16px; border-radius: 0; border-left: 2px solid transparent; }}
popover.menu modelbutton:hover {{ background: alpha({acc}, 0.08); border-left-color: {acc}; }}
popover.menu separator {{ background: alpha({fg}, 0.08); margin: 4px 0; }}

scrollbar {{ background: transparent; border: none; }}
scrollbar slider {{ background: alpha({fg}, 0.22); border-radius: 0; min-width: 4px; min-height: 4px; border: none; }}
scrollbar slider:hover {{ background: alpha({fg}, 0.4); }}
paned > separator {{ background: alpha({fg}, 0.08); min-width: 1px; min-height: 1px; }}
"#
    )
}
