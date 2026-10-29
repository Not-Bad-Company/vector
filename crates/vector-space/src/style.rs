//! The stylesheet, in the Vector language: square corners, JetBrains Mono,
//! an nvim lualine mode block, dim lowercase secondary text, rows with a left
//! accent border. Colours from the Omarchy theme; reloaded when it changes.

use std::cell::RefCell;

use vector_core::theme::Theme;

use crate::item::Mode;

thread_local! {
    static ACCENT: RefCell<String> = RefCell::new("#81a1c1".into());
}

/// The accent, for Pango markup (search match highlights).
pub fn accent() -> String {
    ACCENT.with(|a| a.borrow().clone())
}

/// Every mode wears the theme's accent, like the bar's NORMAL block.
fn mode_color(t: &Theme, _: Mode) -> &str {
    &t.accent
}

pub fn css(t: &Theme) -> String {
    ACCENT.with(|a| *a.borrow_mut() = t.accent.clone());
    let (bg, fg, red) = (&t.background, &t.foreground, &t.color1);
    let acc = mode_color(t, Mode::Apps);
    format!(
        r#"
window.vspace {{
  background: transparent;
  color: {fg};
  font-family: 'JetBrainsMonoNL Nerd Font', 'JetBrainsMono Nerd Font', 'Symbols Nerd Font', monospace;
  font-size: 13px;
}}
window.vspace * {{ border-radius: 0; outline: none; -gtk-icon-shadow: none; }}
.backdrop {{ background: alpha(black, 0.18); animation: fade-in 110ms ease-out; }}
@keyframes fade-in {{ from {{ opacity: 0; }} to {{ opacity: 1; }} }}
.card {{
  background: {bg};
  border: 1px solid alpha({fg}, 0.13);
  box-shadow: 0 24px 64px alpha(black, 0.5), 0 2px 8px alpha(black, 0.25);
  animation: card-in 150ms cubic-bezier(0.2, 0.9, 0.3, 1);
}}
@keyframes card-in {{
  from {{ opacity: 0; transform: translateY(-6px) scale(0.985); }}
  to {{ opacity: 1; transform: none; }}
}}

/* header: the mode block, then the search */
.head {{ min-height: 52px; border-bottom: 1px solid alpha({fg}, 0.08); background: alpha({fg}, 0.03); }}
.head .mode {{
  background: {acc}; color: {bg}; font-weight: bold; font-size: 12px;
  padding: 0 16px 0 18px; letter-spacing: 1px;
}}
.head entry, .head text {{
  background: transparent; border: none; box-shadow: none; outline: none;
  font-size: 16px; min-height: 50px; padding: 0 10px; color: {fg};
  caret-color: {acc};
}}
.head entry placeholder, .head text placeholder {{ color: alpha({fg}, 0.32); }}
.head entry > image {{ opacity: 0; min-width: 0; }}
.head .count {{ color: alpha({fg}, 0.32); margin-right: 18px; font-size: 11px; }}

/* list */
list, gridview, listview {{ background: transparent; }}
listview {{ padding: 6px; }}
gridview {{ padding: 8px; }}
listview > row, gridview > child {{ padding: 0; background: transparent; outline: none; }}
listview > row:focus, gridview > child:focus {{ outline: none; }}
.section {{
  color: alpha({fg}, 0.42); font-weight: bold; font-size: 11.5px;
  padding: 12px 12px 4px 12px;
}}
.row {{
  min-height: 30px; padding: 7px 14px 7px 10px;
  border-left: 2px solid transparent;
  transition: background 80ms ease;
}}
listview > row:hover .row {{ background: alpha({fg}, 0.04); }}
listview > row:selected {{ background: transparent; color: inherit; }}
listview > row:selected .row {{ background: alpha({acc}, 0.13); border-left-color: {acc}; }}
.row .icon {{ min-width: 24px; min-height: 24px; color: alpha({fg}, 0.62); }}
listview > row:selected .row .icon.glyph {{ color: {acc}; }}
.row .icon.glyph {{ font-size: 16px; }}
.row .icon.emo {{ font-size: 20px; }}
.row .thumb {{ border: 1px solid alpha({fg}, 0.10); }}
.row .title {{ font-size: 13.5px; }}
.row .sub {{ color: alpha({fg}, 0.5); font-size: 12px; }}
.row .meta {{ color: alpha({fg}, 0.4); margin-left: 12px; font-size: 12px; }}
listview > row:selected .row .meta {{ color: alpha({fg}, 0.55); }}
.row.recent .meta, .row.unseen .title {{ color: {acc}; }}
.row.critical .icon, .row.critical .title {{ color: {red}; }}
.row.pinned .icon {{ color: {acc}; }}
.empty {{ color: alpha({fg}, 0.45); padding: 28px 24px; font-size: 13px; }}

/* emoji grid */
.emo-cell {{ min-width: 50px; min-height: 48px; transition: background 80ms ease; }}
.emo-cell label {{ font-size: 26px; font-family: 'Noto Color Emoji', sans-serif; }}
gridview > child:hover .emo-cell {{ background: alpha({fg}, 0.05); }}
gridview > child:selected {{ background: transparent; }}
gridview > child:selected .emo-cell {{ background: alpha({acc}, 0.2); box-shadow: inset 0 -2px {acc}; }}

/* preview */
.preview {{ border-left: 1px solid alpha({fg}, 0.07); background: alpha({fg}, 0.018); }}
.preview .pbox {{ padding: 20px 22px; }}
.preview .ptitle {{ font-weight: bold; font-size: 14px; }}
.preview .pkv {{ color: alpha({fg}, 0.5); font-size: 12px; }}
.preview .ptext {{ background: transparent; color: alpha({fg}, 0.9); font-size: 13px; }}
.preview .ptext text {{ background: transparent; }}
.preview .pbig {{ font-size: 76px; font-family: 'Noto Color Emoji', sans-serif; }}
.preview picture {{ border: 1px solid alpha({fg}, 0.10); }}
.preview .pbody {{ color: alpha({fg}, 0.82); }}
.preview .pact {{ color: alpha({fg}, 0.75); }}

/* footer: the modes, then the keys */
.foot {{ min-height: 34px; border-top: 1px solid alpha({fg}, 0.08); background: alpha({fg}, 0.03); padding: 0 6px; }}
.foot .tab {{ padding: 0 9px; color: alpha({fg}, 0.45); font-size: 12px; transition: color 80ms ease, box-shadow 80ms ease; }}
.foot .tab:hover {{ color: alpha({fg}, 0.85); }}
.foot .tab .k {{ color: alpha({fg}, 0.6); font-weight: bold; margin-right: 4px; }}
.foot .tab.on {{ color: {fg}; box-shadow: inset 0 -2px {acc}; }}
.foot .tab.on .k {{ color: {acc}; }}
.foot .keys {{ color: alpha({fg}, 0.42); margin-right: 10px; font-size: 12px; }}
.foot .keys.warn {{ color: {red}; }}
.foot .dnd {{ color: {acc}; margin-right: 12px; font-size: 12px; }}
scrollbar {{ background: transparent; }}
scrollbar slider {{ background: alpha({fg}, 0.18); min-width: 3px; }}
"#
    )
}

/// The Start menu (start.rs): two columns, rows and tiles as flat buttons
/// with the accent edge on hover and focus, search and power level along the bottom.
pub fn start_css(t: &Theme) -> String {
    let (bg, fg, acc, red) = (&t.background, &t.foreground, &t.accent, &t.color1);
    format!(
        r#"
.start-backdrop {{ background: transparent; }}
.card.start {{ animation: start-in 140ms cubic-bezier(0.2, 0.9, 0.3, 1); }}
@keyframes start-in {{
  from {{ opacity: 0; transform: translate(-8px, -8px) scale(0.98); }}
  to {{ opacity: 1; transform: none; }}
}}
.start-left {{ padding-top: 6px; }}
.start-right {{ background: alpha({fg}, 0.035); border-left: 1px solid alpha({fg}, 0.08); }}

/* section labels */
.start-section {{
  font-size: 11px; color: alpha({fg}, 0.4); letter-spacing: 1px;
  padding: 14px 20px 6px 20px;
}}
.start-letter {{ color: {acc}; font-weight: bold; padding-top: 12px; }}
.start-empty {{ color: alpha({fg}, 0.32); font-size: 12px; padding: 6px 20px 10px 20px; }}

/* rows: flat buttons, accent edge on hover and keyboard focus */
button.start-row {{
  background: transparent; border: none; box-shadow: none; border-radius: 0;
  border-left: 2px solid transparent; padding: 6px 18px; min-height: 26px;
  color: {fg}; font-weight: normal; transition: background 90ms ease;
}}
button.start-row:hover {{ background: alpha({fg}, 0.06); border-left-color: alpha({acc}, 0.6); }}
button.start-row:focus-visible {{ background: alpha({acc}, 0.14); border-left-color: {acc}; }}
button.start-row:active {{ background: alpha({acc}, 0.22); }}
button.start-row label {{ font-size: 13px; }}
.start-glyph {{ font-size: 16px; min-width: 24px; color: alpha({fg}, 0.72); }}
button.start-row:hover .start-glyph, button.start-row:focus-visible .start-glyph {{ color: {acc}; }}
.start-meta {{ color: alpha({fg}, 0.38); font-size: 11px; }}
.start-right button.start-row {{ padding: 5px 20px; min-height: 22px; }}

/* all apps / back, and the search */
button.start-nav {{ border-top: 1px solid alpha({fg}, 0.07); padding-top: 9px; padding-bottom: 9px; }}
.start-searchbar {{
  min-height: 50px; background: alpha({fg}, 0.045);
  border-top: 1px solid alpha({fg}, 0.09);
}}
.start-searchbar:focus-within {{ border-top-color: {acc}; background: alpha({fg}, 0.06); }}
.start-sglyph {{ font-size: 15px; color: alpha({fg}, 0.5); padding: 0 4px 0 20px; }}
.start-searchbar:focus-within .start-sglyph {{ color: {acc}; }}
.start-search, .start-search text {{
  background: transparent; border: none; box-shadow: none; outline: none;
  min-height: 50px; padding: 0 8px; font-size: 14px; color: {fg}; caret-color: {acc};
}}
.start-search text placeholder {{ color: alpha({fg}, 0.35); }}
.start-hint {{ font-size: 11px; color: alpha({fg}, 0.28); padding-right: 18px; }}

/* you */
.start-user {{ padding: 18px 20px 14px 20px; border-bottom: 1px solid alpha({fg}, 0.07); }}
.start-avatar {{
  background: {acc}; color: {bg}; font-weight: bold; font-size: 20px;
  min-width: 44px; min-height: 44px;
}}
.start-name {{ font-size: 15px; font-weight: bold; }}
.start-host {{ font-size: 11px; color: alpha({fg}, 0.42); }}

/* settings and system tiles */
.start-tiles {{ padding: 0 12px; }}
button.start-tile {{
  background: transparent; border: none; box-shadow: none; border-radius: 0;
  padding: 9px 2px 8px 2px; color: alpha({fg}, 0.85);
  border-bottom: 2px solid transparent;
}}
button.start-tile:hover {{ background: alpha({fg}, 0.06); border-bottom-color: alpha({acc}, 0.6); }}
button.start-tile:focus-visible {{ background: alpha({acc}, 0.14); border-bottom-color: {acc}; }}
.start-tile-glyph {{ font-size: 19px; color: alpha({fg}, 0.75); }}
button.start-tile:hover .start-tile-glyph, button.start-tile:focus-visible .start-tile-glyph {{ color: {acc}; }}
.start-tile-label {{ font-size: 11px; }}

/* power, level with the search */
.start-power {{ min-height: 50px; border-top: 1px solid alpha({fg}, 0.09); background: alpha({fg}, 0.045); }}
button.start-pow {{
  background: transparent; border: none; box-shadow: none; border-radius: 0;
  color: alpha({fg}, 0.65); padding: 0;
}}
button.start-pow .start-glyph {{ font-size: 17px; }}
button.start-pow:hover {{ background: alpha({fg}, 0.07); }}
button.start-pow:hover .start-glyph {{ color: {fg}; }}
button.start-pow:focus-visible {{ background: alpha({acc}, 0.14); }}
button.start-pow.armed {{ background: {red}; }}
button.start-pow.armed .start-glyph {{ color: {bg}; }}
"#
    )
}
