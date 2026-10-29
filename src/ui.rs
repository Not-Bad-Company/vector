//! The "sheet": shared building blocks for tooltips and panels.
//!
//! Layout rules (same as the old tooltip-lib.sh / bar-panel):
//!   accent bold name · dim detail
//!   section            dim lowercase group heading
//!   label   value      dim 8-char label column
//!   dim hints
//! Colour only where something needs attention.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, pango};

pub const PCT_WARN: f64 = 75.0;
pub const PCT_CRIT: f64 = 90.0;

pub fn dim(s: &str) -> String {
    format!("<span alpha=\"50%\">{s}</span>")
}

pub fn colored(s: &str, color: &str) -> String {
    format!("<span color=\"{color}\">{s}</span>")
}

pub fn label(markup: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_xalign(0.0);
    for c in classes.iter().filter(|c| !c.is_empty()) {
        l.add_css_class(c);
    }
    l
}

pub fn text(t: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    for c in classes.iter().filter(|c| !c.is_empty()) {
        l.add_css_class(c);
    }
    l
}

pub fn ellipsize(l: &gtk::Label, chars: i32) {
    l.set_ellipsize(pango::EllipsizeMode::End);
    l.set_max_width_chars(chars);
}

pub fn hbox(spacing: i32, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

pub fn vbox(spacing: i32, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

pub fn spacer() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.set_hexpand(true);
    b
}

/// A tooltip sheet: vertical box with tooltip padding.
pub fn sheet() -> gtk::Box {
    vbox(3, &["sheet"])
}

/// Tooltip header: a little statusline, the title in a block with a
/// slanted edge, then the detail. Returns (box, detail label).
pub fn head(title: &str, detail: &str) -> (gtk::Box, gtk::Label) {
    let b = hbox(0, &["thead"]);
    let t = text(&title.to_uppercase(), &["htitle"]);
    t.set_valign(gtk::Align::Fill);
    b.append(&t);
    let theme = crate::panel::current_theme();
    b.append(&crate::bar::slant(false, crate::bar::Kind::Block, crate::bar::Kind::Plain, &theme).tap(|s| s.set_mode_accent()));
    let d = label(detail, &["hdetail"]);
    ellipsize(&d, 44);
    d.set_hexpand(true);
    b.append(&d);
    (b, d)
}

/// Slants in tooltips always use the accent (tooltips don't follow the mode).
pub trait ModeAccent {
    fn set_mode_accent(&self);
}
impl ModeAccent for gtk::DrawingArea {
    fn set_mode_accent(&self) {
        self.add_css_class("accent-slant");
    }
}

pub fn set_detail(l: &gtk::Label, detail: &str) {
    if detail.is_empty() {
        l.set_markup("");
    } else {
        l.set_markup(&format!(" · {detail}"));
    }
}

pub fn section(t: &str) -> gtk::Label {
    text(t, &["section"])
}

/// Section heading for tooltips.
pub fn tsection(t: &str) -> gtk::Label {
    text(t, &["tsec"])
}

/// Key/value row. Returns (row, value label).
pub fn kv(k: &str, v: &str) -> (gtk::Box, gtk::Label) {
    let b = hbox(0, &["kv"]);
    let kl = text(k, &["k"]);
    let vl = label(v, &["v"]);
    vl.set_hexpand(true);
    vl.set_wrap(true);
    vl.set_wrap_mode(pango::WrapMode::WordChar);
    vl.set_max_width_chars(52);
    b.append(&kl);
    b.append(&vl);
    (b, vl)
}

/// Key/value for tooltips: same columns, no panel padding.
pub fn tkv(k: &str, v: &str) -> gtk::Box {
    let (b, _) = kv(k, v);
    b.remove_css_class("kv");
    b.add_css_class("tkv");
    b
}

pub fn hint(t: &str) -> gtk::Label {
    text(t, &["hint"])
}

/// Tooltip footer hint.
pub fn thint(t: &str) -> gtk::Label {
    let l = text(t, &["dim"]);
    l.set_margin_top(8);
    l
}

/// Colour class for a percentage.
pub fn pct_class(pct: f64) -> &'static str {
    if pct >= PCT_CRIT {
        "crit"
    } else if pct >= PCT_WARN {
        "warn"
    } else {
        ""
    }
}

pub fn pct_markup(pct: f64, t: &crate::theme::Theme) -> String {
    let s = format!("{pct:.0}%");
    if pct >= PCT_CRIT {
        colored(&s, &t.color1)
    } else if pct >= PCT_WARN {
        colored(&s, &t.color3)
    } else {
        s
    }
}

/// A thin level meter, 0..=1.
pub fn meter(frac: f64, class: &str) -> gtk::LevelBar {
    let m = gtk::LevelBar::new();
    m.set_min_value(0.0);
    m.set_max_value(1.0);
    m.set_value(frac.clamp(0.0, 1.0));
    m.add_css_class("meter");
    // drop GTK's default low/high/full offsets, colour comes from our classes
    m.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
    m.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
    m.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
    if !class.is_empty() {
        m.add_css_class(class);
    }
    m.set_valign(gtk::Align::Center);
    m.set_hexpand(true);
    m
}

/// `label  [=====    ]  42%  detail`.
pub fn meter_row(k: &str, frac: f64, value: &str, detail: &str, class: &str) -> gtk::Box {
    let b = hbox(10, &["meterrow"]);
    b.append(&text(k, &["k"]));
    let m = meter(frac, class);
    m.set_size_request(110, -1);
    m.set_hexpand(false);
    b.append(&m);
    let v = label(value, &["v"]);
    if !class.is_empty() {
        v.add_css_class(class);
    }
    b.append(&v);
    if !detail.is_empty() {
        let d = label(detail, &["dim"]);
        d.set_hexpand(true);
        ellipsize(&d, 34);
        b.append(&d);
    }
    b
}

pub fn pointer(w: &impl IsA<gtk::Widget>) {
    w.set_cursor(gdk::Cursor::from_name("pointer", None).as_ref());
}

/// Makes any widget clickable (primary button release).
pub fn on_click(w: &impl IsA<gtk::Widget>, f: impl Fn() + 'static) {
    let g = gtk::GestureClick::new();
    g.set_button(gdk::BUTTON_PRIMARY);
    g.connect_released(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        f()
    });
    w.add_controller(g);
    w.add_css_class("clickable");
    pointer(w);
}

pub fn link(t: &str, classes: &[&str], f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(t);
    b.add_css_class("link");
    for c in classes {
        b.add_css_class(c);
    }
    b.connect_clicked(move |_| f());
    pointer(&b);
    b
}

pub fn icon_button(glyph: &str, tip: &str, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(glyph);
    b.add_css_class("icon");
    if !tip.is_empty() {
        b.set_tooltip_text(Some(tip));
    }
    b.connect_clicked(move |_| f());
    pointer(&b);
    b
}

/// A chip: small toggle-looking button. `.on` when selected.
pub fn chip(t: &str, on: bool, f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(t);
    b.add_css_class("chip");
    if on {
        b.add_css_class("on");
    }
    b.connect_clicked(move |_| f());
    pointer(&b);
    b
}

/// A row of mutually exclusive chips. `pick` gets the chosen key; the row
/// marks the clicked chip at once (optimistic), the next state refresh corrects it.
pub fn segmented(options: &[(&str, &str)], current: &str, pick: impl Fn(&str) + 'static) -> gtk::Box {
    let b = hbox(0, &["seg"]);
    let pick = Rc::new(pick);
    let buttons: Rc<RefCell<Vec<(String, gtk::Button)>>> = Rc::default();
    for (key, name) in options {
        let key = key.to_string();
        let p = pick.clone();
        let all = buttons.clone();
        let k2 = key.clone();
        let btn = chip(name, key == current, move || {
            for (k, b) in all.borrow().iter() {
                if *k == k2 {
                    b.add_css_class("on");
                } else {
                    b.remove_css_class("on");
                }
            }
            p(&k2)
        });
        buttons.borrow_mut().push((key, btn.clone()));
        b.append(&btn);
    }
    b
}

/// Switch that reports user toggles only (not programmatic `set_active`).
pub fn switch(active: bool, f: impl Fn(bool) + 'static) -> gtk::Switch {
    let s = gtk::Switch::new();
    s.set_active(active);
    s.set_valign(gtk::Align::Center);
    let guard = Rc::new(Cell::new(false));
    s.connect_state_set(move |_, v| {
        if !guard.get() {
            f(v);
        }
        glib::Propagation::Proceed
    });
    s
}

/// Panel list row: icon, name (ellipsized), meta on the right.
pub struct Row {
    pub root: gtk::Box,
    pub icon: gtk::Label,
    pub name: gtk::Label,
    pub meta: gtk::Label,
    pub trail: gtk::Box,
}

impl Row {
    pub fn new(icon: &str, name: &str, meta: &str) -> Row {
        let root = hbox(10, &["row"]);
        let ic = label(icon, &["icon", "dim"]);
        let nm = text(name, &["name"]);
        nm.set_hexpand(true);
        ellipsize(&nm, 30);
        let mt = label(meta, &["meta"]);
        let trail = hbox(2, &[]);
        root.append(&ic);
        root.append(&nm);
        root.append(&mt);
        root.append(&trail);
        Row { root, icon: ic, name: nm, meta: mt, trail }
    }

    pub fn set_active(&self, on: bool) {
        if on {
            self.root.add_css_class("active");
            self.icon.remove_css_class("dim");
            self.icon.add_css_class("ok");
        } else {
            self.root.remove_css_class("active");
            self.icon.remove_css_class("ok");
            self.icon.add_css_class("dim");
        }
    }
}

/// Removes all children of a box.
pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// A small spinner for "working…" states.
pub fn spinner() -> gtk::Spinner {
    let s = gtk::Spinner::new();
    s.start();
    s
}

/// `.tap(|x| ...)` for building widgets inline.
pub trait Tap: Sized {
    fn tap(self, f: impl FnOnce(&Self)) -> Self {
        f(&self);
        self
    }
}
impl<T> Tap for T {}

/// A list whose rows are kept by key: a row is rebuilt only when its
/// signature changed, and while the pointer is over the list the order is
/// frozen (new rows go last) so nothing moves under a click.
pub struct KeyedList {
    pub root: gtk::Box,
    rows: RefCell<Vec<(String, String, gtk::Widget)>>,
    hovered: Rc<Cell<bool>>,
    pending: RefCell<Option<Vec<Item>>>,
}

/// (key, signature, builder)
pub type Item = (String, String, Box<dyn Fn() -> gtk::Widget>);

impl KeyedList {
    pub fn new() -> Rc<KeyedList> {
        let root = vbox(0, &[]);
        let l = Rc::new(KeyedList { root, rows: RefCell::default(), hovered: Rc::default(), pending: RefCell::default() });
        let m = gtk::EventControllerMotion::new();
        let h = l.hovered.clone();
        m.connect_enter(move |_, _, _| h.set(true));
        let h = l.hovered.clone();
        let weak = Rc::downgrade(&l);
        m.connect_leave(move |_| {
            h.set(false);
            // apply the order we held back
            if let Some(l) = weak.upgrade() {
                if let Some(items) = l.pending.borrow_mut().take() {
                    l.sync_now(items, false);
                }
            }
        });
        l.root.add_controller(m);
        l
    }

    pub fn hovered(&self) -> bool {
        self.hovered.get()
    }

    pub fn sync(&self, items: Vec<Item>) {
        self.sync_now(items, self.hovered.get());
    }

    fn sync_now(&self, mut items: Vec<Item>, frozen: bool) {
        if frozen {
            let pos: std::collections::HashMap<String, usize> =
                self.rows.borrow().iter().enumerate().map(|(i, (k, _, _))| (k.clone(), i)).collect();
            items.sort_by_key(|(k, _, _)| pos.get(k).copied().unwrap_or(usize::MAX));
        }
        let mut old: std::collections::HashMap<String, (String, gtk::Widget)> =
            self.rows.borrow_mut().drain(..).map(|(k, s, w)| (k, (s, w))).collect();
        let mut new_rows = Vec::new();
        let mut prev: Option<gtk::Widget> = None;
        for (key, sig, build) in &items {
            let w = match old.remove(key) {
                Some((s, w)) if &s == sig => w,
                Some((_, w)) => {
                    let nw = build();
                    self.root.insert_child_after(&nw, Some(&w));
                    self.root.remove(&w);
                    nw
                }
                None => build(),
            };
            if w.parent().is_none() {
                self.root.insert_child_after(&w, prev.as_ref());
            } else if w.prev_sibling() != prev {
                self.root.reorder_child_after(&w, prev.as_ref());
            }
            prev = Some(w.clone());
            new_rows.push((key.clone(), sig.clone(), w));
        }
        for (_, (_, w)) in old {
            self.root.remove(&w);
        }
        *self.rows.borrow_mut() = new_rows;
        if frozen {
            *self.pending.borrow_mut() = Some(items);
        }
    }
}

/// Tooltip meter row: `label [====    ] value  detail`.
pub fn tmeter(k: &str, frac: f64, value: &str, detail: &str, class: &str) -> gtk::Box {
    let b = hbox(8, &[]);
    b.append(&text(k, &["dim"]).tap(|l| l.set_width_chars(7)));
    let m = meter(frac, class);
    m.set_size_request(80, -1);
    m.set_hexpand(false);
    b.append(&m);
    b.append(&label(value, &[class]).tap(|l| l.set_width_chars(5)));
    if !detail.is_empty() {
        b.append(&label(detail, &["dim"]));
    }
    b
}
