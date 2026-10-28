//! The stylesheet: the visual language of Omarchy's bar panels:
//! square corners, JetBrains Mono, accent only for titles and the active state, dim lowercase
//! secondary text, rows with a left accent border, a 97% background with a quiet 1px border.
//! Colours come from the Omarchy theme at startup.

use crate::theme::Theme;

/// A Nerd Font is needed for the icons; the first one installed wins.
pub const FONT: &str = "'JetBrainsMonoNL Nerd Font', 'JetBrainsMono Nerd Font', 'JetBrainsMono NF', 'Symbols Nerd Font Mono', 'Symbols Nerd Font', monospace";

pub fn css(t: &Theme) -> String {
    let (bg, fg, acc) = (&t.background, &t.foreground, &t.accent);
    let (red, green, yellow) = (&t.color1, &t.color2, &t.color3);
    let (blue, magenta, cyan) = (&t.color4, &t.color5, &t.color6);
    let mut css = format!(
        r#"
window.vector {{
  background: transparent;
  color: {fg};
  font-family: {FONT};
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
/* keyboard cursor in the sidebar */
.row.cursor {{ border-left-color: {acc}; background: alpha({acc}, 0.16); }}
.row .meta {{ opacity: 0.5; }}
/* g1…g9 */
.row .place-num {{ opacity: 0.28; font-size: 10px; }}
.row.active .place-num, .row.cursor .place-num {{ opacity: 0.7; }}

/* ── header: tabs and path bar ── */
.header {{ background: alpha({fg}, 0.035); border-bottom: 1px solid alpha({fg}, 0.08); padding: 0 8px; min-height: 34px; }}
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
button.link.danger {{ color: {red}; opacity: 0.85; }}
button.link.danger:hover {{ color: {red}; opacity: 1; }}
.tab {{ border-bottom: 2px solid transparent; transition: background 120ms ease; }}
/* one hover surface for the whole tab, not one per button */
.tab:hover {{ background: alpha({fg}, 0.06); }}
.tab > button.link:hover {{ background: none; }}
.tab > button.link {{ padding: 6px 4px 6px 12px; }}
.tab > button.x {{ padding: 6px 8px 6px 4px; }}
.tab.active {{ border-bottom-color: {acc}; }}
.tab:hover button.x {{ opacity: 0.55; }}
.tab:hover button.x:hover {{ opacity: 1; }}
.crumb-sep {{ opacity: 0.32; padding: 0 2px; }}
button.link.crumb {{ padding: 3px 6px; opacity: 0.75; }}
button.link.crumb.primary {{ font-weight: bold; opacity: 1; }}

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
.colheader {{ padding: 6px 16px 5px 10px; border-bottom: 1px solid alpha({fg}, 0.08); }}
.colheader label {{ opacity: 0.6; }}
.colheader label.ok {{ opacity: 1; }}
.file {{ padding: 7px 16px 7px 8px; }}
.file .meta {{ opacity: 0.6; }}
.file.hidden-file .name {{ opacity: 0.6; }}
.file .badge {{ min-width: 14px; }}
.file .sep-dot, .colheader .sep-dot {{ opacity: 0.32; }}
.file.marked .icon {{ color: {acc}; }}
/* what a file is, by colour, like nvim-web-devicons: folders bold in the accent */
.k-dir .icon, .k-dir .big-icon {{ color: {acc}; }}
.file.k-dir .name, .tile.k-dir .name {{ font-weight: bold; }}
.k-image .icon, .k-image .big-icon {{ color: {magenta}; }}
.k-video .icon, .k-video .big-icon {{ color: {red}; }}
.k-audio .icon, .k-audio .big-icon {{ color: {cyan}; }}
.k-doc .icon, .k-doc .big-icon {{ color: {blue}; }}
.k-archive .icon, .k-archive .big-icon {{ color: {yellow}; }}
.k-code .icon, .k-code .big-icon {{ color: {green}; }}
.k-link .name {{ font-style: italic; }}
/* relative line numbers: the cursor row's own number stands out, like nvim's CursorLineNr */
.lnum {{ opacity: 0.28; font-size: 11px; }}
.file.cursor .lnum {{ opacity: 1; color: {yellow}; font-weight: bold; }}
.file .meta.sz-small {{ opacity: 0.4; }}
.file .meta.sz-big {{ opacity: 1; color: {yellow}; }}
.file .meta.sz-huge {{ opacity: 1; color: {red}; font-weight: bold; }}
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
  background: transparent;
}}
gridview > child:hover {{ background: alpha({fg}, 0.06); }}
/* no edge lines in the grid: grey for selected tiles, the accent tint for the cursor */
gridview > child:selected {{ background: alpha({fg}, 0.08); color: {fg}; }}
gridview > child > .tile.cursor {{ background: alpha({acc}, 0.16); }}
.tile .big-icon {{ font-size: 34px; min-height: 64px; }}
.tile picture {{ border: 1px solid alpha({fg}, 0.12); }}
.drop-target {{ background: alpha({acc}, 0.10); }}
.empty {{ padding: 14px 16px; opacity: 0.45; }}

/* ── preview ── */
.preview {{ padding: 14px 16px; }}
.details {{ margin-top: 14px; }}
.termpanel {{ padding: 8px 0 0 14px; min-height: 220px; }}
.termpanel vte-terminal {{ background: transparent; }}
.playhead {{ background: {fg}; min-width: 2px; }}
.wave {{ border: 1px solid alpha({fg}, 0.12); background: alpha({fg}, 0.03); }}
.details-section {{ font-weight: bold; margin-bottom: 2px; }}
.preview picture {{ border: 1px solid alpha({fg}, 0.12); }}
.preview textview, .preview textview text {{ background: transparent; color: {fg}; font-family: {FONT}; font-size: 11px; }}
.preview textview border, .preview gutter, .preview gutter > * {{ background: transparent; }}

/* ── statusline and command line ── */
.statusline {{ background: alpha({fg}, 0.035); border-top: 1px solid alpha({fg}, 0.08); min-height: 26px; }}
.statusline > label {{ padding: 0 10px; }}
.mode {{ padding: 4px 10px; opacity: 0.6; font-weight: bold; }}
.mode.active {{ background: {acc}; color: {bg}; opacity: 1; }}
/* vector-find's statusline is nvim's lualine: coloured blocks with slanted edges */
.statusline.lualine {{ min-height: 24px; }}
.statusline.lualine > label.seg {{ background: alpha({fg}, 0.13); }}
.statusline.lualine > label.mode, .statusline.lualine > label.zone {{ padding: 0 12px; opacity: 1; font-weight: bold; color: {bg}; }}
.statusline.lualine > .slant {{ min-width: 10px; }}
.statusline.lualine > .slant.on-seg {{ background: alpha({fg}, 0.13); }}
.statusline.lualine > .slant.from-seg, .statusline.lualine > .slant.to-seg {{ color: alpha({fg}, 0.13); }}
.progress {{ min-height: 2px; }}
progressbar trough {{ background: alpha({fg}, 0.08); min-height: 2px; border-radius: 0; }}
progressbar progress {{ background: {acc}; min-height: 2px; border-radius: 0; }}
.pickbar {{ border-top: 1px solid alpha({fg}, 0.08); padding: 6px 10px; }}
.pickbar > .title {{ padding: 0 8px; }}
.pickbar entry {{ margin: 0 8px; }}
.cmdline {{ border-top: 1px solid alpha({fg}, 0.08); padding: 6px 12px; }}
.cmdline entry {{ background: transparent; border: none; padding: 0 4px; min-height: 22px; }}
.cmdline .prompt {{ color: {acc}; font-weight: bold; }}
.completions {{ border-top: 1px solid alpha({fg}, 0.08); padding: 4px 0; }}
.completions .row {{ padding: 4px 16px; }}
.message.crit {{ color: {red}; }}
.help-panel {{ padding: 16px 0 10px 0; }}
.help-panel .head {{ padding: 0 20px 12px 20px; }}
.help-panel .help-tabs {{ padding: 0 12px; border-bottom: 1px solid alpha({fg}, 0.08); }}
.help-panel .help-tabs button.link {{ padding: 6px 10px; border-bottom: 2px solid transparent; }}
.help-panel .help-tabs button.link.primary {{ border-bottom-color: {acc}; }}
.help-panel .body {{ padding: 14px 20px; }}
.help-panel .help-intro {{ margin-bottom: 14px; opacity: 0.8; }}
.help-panel .help-section {{ color: {acc}; font-weight: bold; margin: 12px 0 6px 0; }}
.help-panel .help-sub {{ margin-top: 8px; }}
.help-panel .foot {{ padding: 10px 20px 0 20px; }}
button.link.app-row {{ padding: 5px 8px; opacity: 0.85; border-left: 2px solid transparent; }}
button.link.app-row.target {{ opacity: 1; border-left-color: {acc}; background: alpha({acc}, 0.10); }}
checkbutton.always {{ margin-top: 10px; }}
checkbutton.always label {{ margin-left: 6px; }}
checkbutton check {{ border-radius: 0; border: 1px solid alpha({fg}, 0.3); background: transparent; min-width: 12px; min-height: 12px; }}
checkbutton check:checked {{ background: {acc}; border-color: {acc}; color: {bg}; }}
tooltip {{ background: alpha({bg}, 0.97); border: 1px solid alpha({fg}, 0.18); border-radius: 0; box-shadow: none; }}
tooltip label {{ color: {fg}; font-family: {FONT}; font-size: 12px; }}

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
popover {{ font-family: {FONT}; font-size: 12px; }}
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
paned > separator {{ background: alpha({fg}, 0.08); background-image: none; min-width: 1px; min-height: 1px; margin: 0; padding: 0; border: none; box-shadow: none; }}
"#
    );
    // the mode's colour on the mode block, the position block and their edges
    for (mode, colour) in [
        ("normal", acc),
        ("visual", magenta),
        ("command", yellow),
        ("filter", green),
        ("sidebar", cyan),
        ("rename", red),
    ] {
        css += &format!(
            ".statusline.m-{mode} > .mode, .statusline.m-{mode} > .zone {{ background: {colour}; }}\n\
             .statusline.m-{mode} > .slant.from-mode, .statusline.m-{mode} > .slant.to-mode {{ color: {colour}; }}\n"
        );
    }
    css
}
