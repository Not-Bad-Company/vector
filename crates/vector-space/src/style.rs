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

/// The Start menu (start.rs): the card's two columns, rows, power buttons.
pub fn start_css(t: &Theme) -> String {
    let (bg, fg, acc) = (&t.background, &t.foreground, &t.accent);
    format!(
        r#"
.start-backdrop {{ background: transparent; }}
.card.start {{ animation: start-in 130ms cubic-bezier(0.2, 0.9, 0.3, 1); }}
@keyframes start-in {{
  from {{ opacity: 0; transform: translate(-6px, -6px) scale(0.985); }}
  to {{ opacity: 1; transform: none; }}
}}
.start-left {{ padding: 10px 0 0 0; }}
.start-right {{ background: alpha({fg}, 0.035); border-left: 1px solid alpha({fg}, 0.08); padding: 0 0 6px 0; }}
.start-section {{
  font-size: 11px; color: alpha({fg}, 0.42); letter-spacing: 1px;
  padding: 12px 18px 4px 18px; text-transform: lowercase;
}}
.start-row {{ padding: 7px 18px; min-height: 26px; border-left: 2px solid transparent; }}
.start-row:hover {{ background: alpha({fg}, 0.06); border-left-color: {acc}; }}
.start-row:active {{ background: alpha({acc}, 0.18); }}
.start-row label {{ font-size: 13px; }}
.start-glyph {{ font-size: 16px; min-width: 22px; color: alpha({fg}, 0.75); }}
.start-meta {{ color: alpha({fg}, 0.4); font-size: 12px; }}
.start-empty {{ color: alpha({fg}, 0.35); font-size: 12px; padding: 4px 18px; }}
.start-more {{ border-top: 1px solid alpha({fg}, 0.08); margin-top: 6px; }}
.start-search, .start-search text {{
  background: alpha({fg}, 0.04); border: none; border-top: 1px solid alpha({fg}, 0.08);
  box-shadow: none; min-height: 46px; padding: 0 18px; font-size: 14px; color: {fg}; caret-color: {acc};
}}
.start-search:focus-within {{ border-top-color: {acc}; }}
.start-user {{ padding: 16px 18px 12px 18px; border-bottom: 1px solid alpha({fg}, 0.08); }}
.start-avatar {{
  background: {acc}; color: {bg}; font-weight: bold; font-size: 18px;
  min-width: 40px; min-height: 40px;
}}
.start-name {{ font-size: 15px; font-weight: bold; }}
.start-host {{ font-size: 11px; color: alpha({fg}, 0.45); }}
.start-power {{ border-top: 1px solid alpha({fg}, 0.08); background: alpha({fg}, 0.035); }}
.start-pow {{
  background: transparent; border: none; box-shadow: none; padding: 8px 14px; min-width: 20px;
  color: alpha({fg}, 0.7);
}}
.start-pow:hover {{ background: alpha({fg}, 0.08); color: {fg}; }}
.start-pow.armed {{ background: {red}; color: {bg}; }}
"#,
        red = t.color1
    )
}
