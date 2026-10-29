//! Colours from the active Omarchy theme and the built-in stylesheet.

use crate::util;

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub background: String,
    pub foreground: String,
    pub accent: String,
    /// red
    pub color1: String,
    /// green
    pub color2: String,
    /// yellow
    pub color3: String,
    /// magenta
    pub color5: String,
    /// cyan
    pub color6: String,
    /// Bar-icon colours for "needs attention" (warm/low) and "bad" (hot/critical).
    pub warn: String,
    pub crit: String,
    /// Glass mode (desktop-glass): see-through blurred bar.
    pub glass: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: "#2e3440".into(),
            foreground: "#d8dee9".into(),
            accent: "#81a1c1".into(),
            color1: "#bf616a".into(),
            color2: "#a3be8c".into(),
            color3: "#ebcb8b".into(),
            color5: "#b48ead".into(),
            color6: "#88c0d0".into(),
            warn: "#d08770".into(),
            crit: "#a55555".into(),
            glass: false,
        }
    }
}

pub fn theme_file() -> std::path::PathBuf {
    util::config_dir().join("omarchy/current/theme/colors.toml")
}

pub fn glass_file() -> std::path::PathBuf {
    util::config_dir().join("hypr/desktop-glass/glass.conf")
}

impl Theme {
    /// Colour of a mode key (see `mode::COLORS`).
    pub fn mode_color(&self, key: &str) -> &str {
        let name = crate::mode::COLORS
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, c)| *c)
            .unwrap_or("accent");
        match name {
            "red" => &self.color1,
            "green" => &self.color2,
            "yellow" => &self.color3,
            "magenta" => &self.color5,
            "cyan" => &self.color6,
            _ => &self.accent,
        }
    }

    pub fn load(warn: Option<&str>, crit: Option<&str>) -> Theme {
        let mut t = Theme::default();
        // the Omarchy theme, then ~/.config/vector-bar/theme.toml on top (same format)
        let user = util::read(util::config_dir().join("vector-bar/theme.toml")).unwrap_or_default();
        let text = util::read(theme_file()).map(|base| format!("{base}\n{user}"));
        let mut bar_warn = None;
        let mut bar_crit = None;
        for line in text.as_deref().unwrap_or(&user).lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').to_lowercase();
                match k.trim() {
                    "bar_warn" => bar_warn = Some(v),
                    "bar_crit" => bar_crit = Some(v),
                    _ => {}
                }
            }
        }
        if let Some(text) = text {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else {
                    continue;
                };
                let v = v.trim().trim_matches('"').to_lowercase();
                if !(v.len() == 7
                    && v.starts_with('#')
                    && v[1..].chars().all(|c| c.is_ascii_hexdigit()))
                {
                    continue;
                }
                match k.trim() {
                    "background" => t.background = v,
                    "foreground" => t.foreground = v,
                    "accent" => t.accent = v,
                    "color1" => t.color1 = v,
                    "color2" => t.color2 = v,
                    "color3" => t.color3 = v,
                    "color5" => t.color5 = v,
                    "color6" => t.color6 = v,
                    _ => {}
                }
            }
        }
        if let Some(w) = warn.map(str::to_string).or(bar_warn) {
            t.warn = w;
        }
        if let Some(c) = crit.map(str::to_string).or(bar_crit) {
            t.crit = c;
        }
        t.glass =
            util::read(glass_file()).is_some_and(|s| s.contains("layerrule") && s.contains("blur"));
        t
    }

    /// The whole built-in stylesheet. User CSS is loaded on top of it.
    pub fn css(&self, font: &str, font_size: f64, height: i32) -> String {
        let Theme {
            background: bg,
            foreground: fg,
            accent: acc,
            color1: red,
            color2: green,
            color3: yellow,
            warn,
            crit,
            ..
        } = self;
        let bar_bg = if self.glass {
            // a light blur leaves small bright wallpaper details as specks through a thin
            // bar: mostly opaque, the blur only tints it
            format!("alpha({bg}, 0.8)")
        } else {
            bg.clone()
        };
        let bar_edge = if self.glass {
            format!("border-bottom: 1px solid alpha({fg}, 0.08);")
        } else {
            String::new()
        };
        let card_bg = if self.glass {
            format!("alpha({bg}, 0.96)")
        } else {
            format!("alpha({bg}, 0.97)")
        };
        format!(
            r#"
@define-color vb_bg {bg};
@define-color vb_fg {fg};
@define-color vb_accent {acc};
@define-color vb_red {red};
@define-color vb_green {green};
@define-color vb_yellow {yellow};
@define-color vb_warn {warn};
@define-color vb_crit {crit};

window.vbar, window.vpanel, window.vnotify, window.vosd, tooltip, popover {{
  font-family: '{font}';
  font-size: {font_size}px;
  color: {fg};
}}
window.vbar * {{ font-family: '{font}'; }}

/* ── bar: nvim lualine across the top ──────────────── */
window.vbar {{ background: {bar_bg}; {bar_edge} }}
window.vbar .bar {{ min-height: {inner}px; }}
window.vbar .right {{ margin-right: 0; }}
.zone.seg {{ background: alpha({fg}, 0.13); }}
.zone.block .item, .zone.block .item label {{ color: {bg}; font-weight: bold; }}
.zone.block .item {{ padding: 0 10px; transition: none; }}
.zone.block .item.clickable:hover, .zone.block .item.clickable.slant-hover {{ background: alpha({bg}, 0.14); }}
.zone.block .item.open {{ background: alpha({bg}, 0.20); box-shadow: none; }}
.zone.seg .item {{ padding: 0 8px; }}
.zone.plain {{ padding: 0 4px; }}
.zone.seg .workspaces {{ padding: 0 3px; }}
.item {{ padding: 0 7px; min-width: 12px; transition: color 160ms ease, background 160ms ease; }}
.item.clickable:hover {{ background: alpha({fg}, 0.07); }}
.item label {{ color: inherit; }}
.item.dim, .item .dim {{ color: alpha({fg}, 0.55); }}
.item.faint {{ color: alpha({fg}, 0.42); }}
.item.warn {{ color: {warn}; }}
.item.crit {{ color: {crit}; }}
.item.small label {{ font-size: {small}px; }}
.item .mark {{ margin: 0 1px; }}
.item .mark + label {{ margin-left: 7px; }}
.divider {{ color: alpha({fg}, 0.35); padding: 0 2px; }}
.window-title {{ color: alpha({fg}, 0.55); padding: 0 10px; }}

.ws {{ padding: 0 6px; min-width: 9px; transition: background 140ms ease; }}
.ws label {{ transition: color 140ms ease, opacity 140ms ease; }}
.ws.empty label {{ opacity: 0.42; }}
.ws.active label {{ font-weight: bold; }}
.ws.urgent label {{ color: {crit}; }}
.ws:hover {{ background: alpha({fg}, 0.08); }}
.indicators .item {{ padding: 0 5px; }}

/* ── tooltips ────────────────────────────────────────── */
tooltip {{
  background: {card_bg};
  border: 1px solid alpha({fg}, 0.18);
  border-radius: 0;
  padding: 0;
  box-shadow: none;
}}
tooltip > box {{ padding: 0; }}
tooltip .sheet {{ padding: 0 0 12px 0; min-width: 260px; }}
tooltip .sheet.bare {{ padding: 0; }}
tooltip .sheet.bare > .thead {{ margin: 0; border-bottom: none; }}
.tkv .k {{ min-width: {kcol}px; margin-right: 12px; color: alpha({fg}, 0.5); }}
tooltip .sheet > * {{ margin-left: 16px; margin-right: 16px; }}
tooltip .sheet > .thead {{ margin: 0 0 8px 0; background: alpha({fg}, 0.035); border-bottom: 1px solid alpha({fg}, 0.08); min-height: 24px; }}
tooltip .thead .htitle {{ background: {acc}; color: {bg}; font-weight: bold; padding: 0 10px; }}
tooltip .thead .hdetail {{ color: alpha({fg}, 0.6); margin: 0 12px 0 6px; }}
tooltip .tsec {{ font-weight: bold; margin-top: 12px; margin-bottom: 2px; }}

/* ── panels ──────────────────────────────────────────── */
window.vpanel {{ background: transparent; }}
.card {{
  background: {card_bg};
  border: 1px solid alpha({fg}, 0.18);
  box-shadow: 0 12px 32px alpha(black, 0.38);
  padding: 0;
  animation: card-in 130ms cubic-bezier(0.2, 0.9, 0.3, 1);
}}
@keyframes card-in {{
  from {{ opacity: 0; transform: translateY(-6px); }}
  to {{ opacity: 1; transform: none; }}
}}
.card .head {{ background: alpha({fg}, 0.035); border-bottom: 1px solid alpha({fg}, 0.08); min-height: 30px; padding: 0 12px 0 0; margin-bottom: 10px; }}
.card .head .htitle {{ color: {bg}; font-weight: bold; padding: 0 14px; }}
.card .head .hdetail {{ color: alpha({fg}, 0.6); }}
.card .head .hmark {{ margin-left: 10px; }}
.card .foot {{ padding: 5px 12px; margin-top: 10px; background: alpha({fg}, 0.035); border-top: 1px solid alpha({fg}, 0.08); min-height: 28px; }}
.card .foot .key {{ color: alpha({fg}, 0.4); }}
.card .body {{ padding-bottom: 10px; }}

.title {{ color: {acc}; font-weight: bold; }}
.dim {{ color: alpha({fg}, 0.5); }}
.faint {{ color: alpha({fg}, 0.32); }}
.ok {{ color: {acc}; }}
.good {{ color: {green}; }}
.warn {{ color: {yellow}; }}
.crit {{ color: {red}; }}
.mono-big {{ font-size: {big}px; font-weight: bold; }}
.section {{ padding: 18px 20px 6px 20px; font-weight: bold; }}
.section.first {{ padding-top: 6px; }}
.sep {{ background: alpha({fg}, 0.08); min-height: 1px; margin: 6px 0; }}
.kv {{ padding: 4px 20px; }}
.kv .k {{ color: alpha({fg}, 0.5); min-width: {kcol}px; margin-right: 12px; }}
.note {{ padding: 2px 16px; }}
.hint {{ padding: 10px 20px 0 20px; color: alpha({fg}, 0.5); }}
.empty-note {{ padding: 18px 20px; color: alpha({fg}, 0.45); }}
.msg {{ padding: 4px 20px 8px 20px; }}

.row {{
  padding: 9px 20px;
  border-left: 2px solid transparent;
  transition: background 120ms ease;
}}
.row.clickable:hover {{ background: alpha({fg}, 0.06); }}
.row.active {{ border-left-color: {acc}; background: alpha({acc}, 0.08); }}
.row.active.clickable:hover {{ background: alpha({acc}, 0.13); }}
.row.sel {{ background: alpha({fg}, 0.09); border-left-color: alpha({fg}, 0.5); }}
.row .icon {{ min-width: 22px; }}
.row .meta {{ color: alpha({fg}, 0.5); }}
.expand {{ padding: 4px 20px 12px 46px; }}

/* meters: thin flat bars */
levelbar.meter {{ min-height: 4px; }}
levelbar.meter trough {{ background: alpha({fg}, 0.10); border: none; border-radius: 0; min-height: 4px; padding: 0; }}
levelbar.meter block {{ border: none; border-radius: 0; min-height: 4px; }}
levelbar.meter block.filled {{ background: {acc}; }}
levelbar.meter block.empty {{ background: transparent; }}
levelbar.meter.warn block.filled {{ background: {yellow}; }}
levelbar.meter.crit block.filled {{ background: {red}; }}
levelbar.meter.pace block.filled {{ background: alpha({fg}, 0.35); }}
.meterrow {{ padding: 6px 20px; }}
.meterrow .k {{ color: alpha({fg}, 0.5); min-width: {kcol}px; margin-right: 4px; }}
.meterrow .v {{ min-width: 44px; margin-right: 6px; }}

/* buttons */
button.link, button.chip, button.icon {{
  background: none; border: none; box-shadow: none; border-radius: 0; outline: none;
  padding: 3px 8px; min-height: 0; min-width: 0;
  color: {fg};
  transition: background 120ms ease, color 120ms ease, opacity 120ms ease;
}}
button.link {{ opacity: 0.65; }}
button.link:hover {{ opacity: 1; background: alpha({fg}, 0.07); }}
button.link.primary {{ color: {acc}; opacity: 0.95; }}
button.link.danger:hover {{ color: {red}; }}
button.icon {{ padding: 2px 6px; opacity: 0.6; }}
button.icon:hover {{ opacity: 1; background: alpha({fg}, 0.08); }}
button.chip {{ border: 1px solid alpha({fg}, 0.16); padding: 4px 10px; }}
button.chip:hover {{ background: alpha({fg}, 0.07); }}
button.chip.on {{ border-color: {acc}; background: alpha({acc}, 0.16); color: {fg}; }}
.chips {{ padding: 6px 20px 8px 20px; }}
.seg button.chip {{ margin-right: -1px; }}
button.x {{ opacity: 0; padding: 0 5px; }}
.row:hover button.x {{ opacity: 0.55; }}
.row:hover button.x:hover {{ opacity: 1; }}

entry {{
  background: alpha({fg}, 0.06);
  border: 1px solid alpha({fg}, 0.15);
  border-radius: 0; color: {fg}; min-height: 26px; padding: 0 8px;
  box-shadow: none; outline: none;
}}
entry:focus-within {{ border-color: {acc}; }}
.search {{ padding: 2px 20px 10px 20px; }}

switch {{
  background: alpha({fg}, 0.10);
  border: 1px solid alpha({fg}, 0.22);
  border-radius: 0; min-height: 12px; min-width: 26px; padding: 0;
  transition: background 150ms ease, border-color 150ms ease;
}}
switch:checked {{ background: alpha({acc}, 0.25); border-color: {acc}; }}
switch slider {{
  background: alpha({fg}, 0.55); border-radius: 0; border: none; box-shadow: none;
  min-width: 10px; min-height: 10px; margin: 1px; transition: background 150ms ease;
}}
switch:checked slider {{ background: {acc}; }}
switch image {{ opacity: 0; }}

scale {{ padding: 6px 0; }}
scale trough {{ background: alpha({fg}, 0.12); border-radius: 0; min-height: 4px; border: none; }}
scale highlight {{ background: {acc}; border-radius: 0; }}
scale slider {{ background: {fg}; border-radius: 0; min-width: 10px; min-height: 10px; margin: -4px; border: none; box-shadow: none; }}

scrollbar {{ background: transparent; }}
scrollbar slider {{ background: alpha({fg}, 0.2); border-radius: 0; min-width: 3px; }}
spinner {{ color: {acc}; }}
.thumb {{ padding: 0 20px 10px 52px; }}
.thumb picture {{ border: 1px solid alpha({fg}, 0.12); }}
.app {{ color: {acc}; }}
.body-text {{ color: alpha({fg}, 0.65); }}

/* calendar */
.cal {{ padding: 4px 16px 8px 16px; }}
.cal label {{ min-width: 30px; min-height: 22px; }}
.cal .wd {{ color: alpha({fg}, 0.45); }}
.cal .wk {{ color: alpha({fg}, 0.3); }}
.cal .out {{ color: alpha({fg}, 0.25); }}
.cal .today {{ color: {bg}; background: {acc}; font-weight: bold; }}
.cal .weekend {{ color: alpha({fg}, 0.7); }}

/* spark graphs */
.spark {{ color: {acc}; padding: 0 20px; }}

/* ── notifications ───────────────────────────────────── */
window.vnotify, window.vosd {{ background: transparent; }}
.item.unseen {{ color: {acc}; }}
.ncard {{
  box-shadow: none;
  animation: ncard-in 170ms cubic-bezier(0.2, 0.9, 0.3, 1);
  transition: border-color 140ms ease;
}}
@keyframes ncard-in {{
  from {{ opacity: 0; transform: translateX(28px); }}
  to {{ opacity: 1; transform: none; }}
}}
.ncard:hover {{ border-color: alpha({acc}, 0.45); }}
.ncard .head {{ min-height: 26px; margin-bottom: 9px; padding: 0 2px 0 0; }}
.ncard .htitle {{ background: {acc}; color: {bg}; padding: 0 10px; }}
.ncard.low .htitle {{ background: alpha({fg}, 0.28); color: {fg}; }}
.ncard.critical {{ border-color: alpha({red}, 0.75); }}
.ncard.critical .htitle {{ background: {red}; color: {bg}; }}
.ncard .ncount {{ color: {acc}; font-weight: bold; }}
.ncard .npin {{ color: alpha({fg}, 0.4); }}
.ncard button.nclose {{ opacity: 0; padding: 0 6px; }}
.ncard:hover button.nclose {{ opacity: 0.55; }}
.ncard:hover button.nclose:hover {{ opacity: 1; }}
.ncontent {{ padding: 0 14px 12px 14px; }}
.nsum {{ font-weight: bold; }}
.nprogress {{ margin-top: 2px; }}
.nthumb {{ border: 1px solid alpha({fg}, 0.12); }}
.nbanner {{ margin: 0 14px 12px 14px; border: 1px solid alpha({fg}, 0.12); }}
.nactions {{ padding: 0 8px 8px 8px; margin-top: -4px; }}
.ndrain {{ background: alpha({fg}, 0.05); }}
button.nmore {{
  background: {card_bg}; border: 1px solid alpha({fg}, 0.18); border-radius: 0;
  box-shadow: none; color: alpha({fg}, 0.6); padding: 4px 12px; min-height: 0;
}}
button.nmore:hover {{ color: {fg}; border-color: alpha({acc}, 0.45); }}
.nrow.past {{ opacity: 0.82; }}

/* ── on-screen display ───────────────────────────────── */
.osd {{
  box-shadow: 0 10px 28px alpha(black, 0.35);
  animation: osd-in 90ms ease-out;
}}
@keyframes osd-in {{
  from {{ opacity: 0; }}
  to {{ opacity: 1; }}
}}
.osd .head {{
  margin-bottom: 0;
  min-height: 30px;
  padding: 0 14px 0 0;
  background: alpha({fg}, 0.03);
  border-bottom: 1px solid alpha({fg}, 0.07);
}}
.osd .htitle {{ background: {acc}; color: {bg}; padding: 0 12px; }}
.osd.crit-block .htitle {{ background: {red}; }}
.osd .oglyph {{ color: {acc}; font-size: {osd_glyph}px; }}
.osd.crit-block .oglyph {{ color: {red}; }}
.osd .otitle {{ font-weight: bold; }}
.osd .hdetail {{ color: alpha({fg}, 0.52); }}
.osd .ovalue {{ font-weight: bold; font-size: {osd_val}px; }}
.ometer {{ margin: 10px 14px 12px 14px; }}
.olist {{ padding: 0 14px 12px 14px; }}
.olist label {{ padding: 2px 0; }}
.olist .ocur {{ color: {acc}; font-weight: bold; }}
"#,
            small = (font_size - 2.0).max(8.0),
            big = font_size + 6.0,
            osd_glyph = font_size + 2.0,
            osd_val = font_size + 2.0,
            kcol = (font_size * 6.2) as i32,
            // the glass edge is 1px of the bar's height, not extra
            inner = if self.glass { height - 1 } else { height },
        ) + &self.mode_css()
    }

    /// The mode colour on blocks, panel headers, slanted edges' neighbours,
    /// the open item and the active workspace.
    fn mode_css(&self) -> String {
        let mut css = String::new();
        for (k, _) in crate::mode::COLORS {
            let c = self.mode_color(k);
            css += &format!(
                "window.mode-{k} .zone.block, window.mode-{k} .htitle {{ background: {c}; }}\n\
                 window.mode-{k} .zone.plain .item.open, window.mode-{k} .zone.seg .item.open {{ box-shadow: inset 0 -2px {c}; background: alpha({c}, 0.12); }}\n\
                 window.mode-{k} .ws.active label {{ color: {c}; }}\n\
                 window.mode-{k} .title {{ color: {c}; }}\n"
            );
        }
        css
    }
}
