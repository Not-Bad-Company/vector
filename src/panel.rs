//! Drop-down panels. One open at a time, on a transparent overlay surface that
//! covers the screen *below* the bar (so the bar stays clickable): the card is
//! placed right under the widget that opened it; a click outside, Esc, or the
//! icon again closes it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::gdk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::slot::{Notify, PanelFn};
use crate::ui;

const GAP: i32 = 6;
const MARGIN: i32 = 8;
/// Body height before a panel scrolls; set per open from the monitor height.
fn max_body() -> i32 {
    MAXB.with(|m| m.get())
}

/// Extra top offset (test mode: the test bar does not reserve space).
fn top_offset() -> i32 {
    TOP.with(|t| t.get())
}

pub fn set_top_offset(px: i32) {
    TOP.with(|t| t.set(px));
}

pub fn set_theme(t: Rc<crate::theme::Theme>) {
    THEME.with(|x| *x.borrow_mut() = Some(t));
}

pub fn current_theme() -> Rc<crate::theme::Theme> {
    theme()
}

fn theme() -> Rc<crate::theme::Theme> {
    THEME.with(|x| x.borrow().clone()).unwrap_or_default()
}

thread_local! {
    static TOP: Cell<i32> = const { Cell::new(0) };
    static MAXB: Cell<i32> = const { Cell::new(560) };
    static THEME: RefCell<Option<Rc<crate::theme::Theme>>> = const { RefCell::new(None) };
    static APP: RefCell<Option<gtk::Application>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static ON_OPEN: RefCell<Option<Rc<dyn Fn(&str, bool)>>> = const { RefCell::new(None) };
}

struct Open {
    name: String,
    win: gtk::ApplicationWindow,
    panel: Panel,
}

pub fn init(app: &gtk::Application, on_open: impl Fn(&str, bool) + 'static) {
    APP.with(|a| *a.borrow_mut() = Some(app.clone()));
    ON_OPEN.with(|o| *o.borrow_mut() = Some(Rc::new(on_open)));
}

pub fn is_open(name: &str) -> bool {
    OPEN.with(|o| o.borrow().as_ref().is_some_and(|o| o.name == name))
}

pub fn any_open() -> bool {
    OPEN.with(|o| o.borrow().is_some())
}

fn notify_open(name: &str, open: bool) {
    let f = ON_OPEN.with(|o| o.borrow().clone());
    if let Some(f) = f {
        f(name, open);
    }
}

pub fn close() {
    let Some(open) = OPEN.with(|o| o.borrow_mut().take()) else { return };
    open.panel.shutdown();
    open.win.close();
    notify_open(&open.name, false);
}

pub fn toggle(name: &str, build: PanelFn, anchor: Option<gtk::Widget>, center: bool) {
    if is_open(name) {
        close();
        return;
    }
    close();
    open(name, build, anchor, center);
}

fn open(name: &str, build: PanelFn, anchor: Option<gtk::Widget>, center: bool) {
    let Some(app) = APP.with(|a| a.borrow().clone()) else { return };
    let win = gtk::ApplicationWindow::new(&app);
    win.add_css_class("vpanel");
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some("vector-bar-panel"));
    win.set_keyboard_mode(KeyboardMode::Exclusive);
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        win.set_anchor(e, true);
    }
    // covers the bar too: while a panel holds the keyboard Hyprland sends the
    // pointer only here, so clicks on the bar row are passed on to the bar
    win.set_exclusive_zone(-1);
    // follow the mode colour like the bar
    let apply = {
        let w = win.downgrade();
        move || {
            if let Some(w) = w.upgrade() {
                for (k, _) in crate::mode::COLORS {
                    w.remove_css_class(&format!("mode-{k}"));
                }
                w.add_css_class(&format!("mode-{}", crate::mode::current()));
            }
        }
    };
    apply();
    let mid = crate::mode::changed().connect(apply);
    win.connect_destroy(move |_| crate::mode::changed().disconnect(mid));

    let display = gdk::Display::default();
    let monitor = anchor
        .as_ref()
        .and_then(|a| a.native())
        .and_then(|n| n.surface())
        .and_then(|s| display.as_ref().and_then(|d| d.monitor_at_surface(&s)))
        .or_else(|| {
            display.as_ref().and_then(|d| d.monitors().item(0)).and_then(|m| m.downcast::<gdk::Monitor>().ok())
        });
    if let Some(m) = &monitor {
        win.set_monitor(Some(m));
    }
    let mon_w = monitor.as_ref().map(|m| m.geometry().width()).unwrap_or(1920);
    let mon_h = monitor.as_ref().map(|m| m.geometry().height()).unwrap_or(1080);
    // bar, gaps, header and footer: the rest is for the body
    MAXB.with(|m| m.set((mon_h - 26 - top_offset() - 2 * GAP - 110).max(300)));

    let panel = Panel(Rc::new(PInner {
        width: Cell::new(360),
        closers: RefCell::default(),
        sources: RefCell::default(),
        key: RefCell::default(),
        closed: Cell::new(false),
    }));
    let card = ui::vbox(0, &["card"]);
    let content = build(&panel);
    card.append(&content);
    // a little roomier than each panel asks for: nothing should feel squeezed
    let w = panel.0.width.get() + 40;
    card.set_size_request(w, -1);
    let nat = card.measure(gtk::Orientation::Horizontal, -1).1.max(w);
    if std::env::var_os("VB_DEBUG").is_some() {
        let (mn, nh, _, _) = card.measure(gtk::Orientation::Vertical, nat);
        eprintln!("panel {name}: width {nat} height min {mn} nat {nh}");
    }

    let anchor_x = anchor.as_ref().and_then(|a| {
        let root = a.root()?;
        let p = a.compute_point(&root, &gtk::graphene::Point::new(a.width() as f32 / 2.0, 0.0))?;
        Some(p.x() as i32)
    });
    let x = match (center, anchor_x) {
        (false, Some(cx)) => cx - nat / 2,
        _ => (mon_w - nat) / 2,
    };
    let x = x.clamp(MARGIN, (mon_w - nat - MARGIN).max(MARGIN));

    // a plain box (not GtkFixed, which allocates the minimum height) so the
    // card always gets its natural height, also when its lists grow later
    let fixed = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.set_margin_start(x);
    let bar_h = anchor.as_ref().and_then(|a| a.root()).map(|r| r.height()).filter(|h| *h > 0).unwrap_or(26);
    card.set_margin_top(bar_h + GAP + top_offset());
    fixed.append(&card);
    let outside = gtk::GestureClick::new();
    outside.set_button(0);
    let card_w = card.downgrade();
    let fixed_w = fixed.downgrade();
    let bar_root = anchor.as_ref().and_then(|a| a.root()).map(|r| r.upcast::<gtk::Widget>().downgrade());
    outside.connect_released(move |g, _, px, py| {
        if let (Some(c), Some(f)) = (card_w.upgrade(), fixed_w.upgrade()) {
            if let Some(b) = c.compute_bounds(&f) {
                if b.contains_point(&gtk::graphene::Point::new(px as f32, py as f32)) {
                    return;
                }
            }
        }
        let button = g.current_button();
        // on the bar row: let the bar item there handle it (it closes/switches the panel)
        let target_y = py - top_offset() as f64;
        if target_y >= 0.0 && (target_y as i32) < bar_h {
            if let Some(bar) = bar_root.as_ref().and_then(|w| w.upgrade()) {
                if let Some(hit) = bar.pick(px, target_y, gtk::PickFlags::DEFAULT) {
                    if crate::slot::click_at(&hit, button) {
                        return;
                    }
                }
            }
        }
        close();
    });
    fixed.add_controller(outside);
    win.set_child(Some(&fixed));

    let keys = gtk::EventControllerKey::new();
    let p2 = panel.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        let h = p2.0.key.borrow().clone();
        if let Some(h) = h {
            if h(key, mods) {
                return glib::Propagation::Stop;
            }
        }
        if key == gdk::Key::Escape {
            close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    win.add_controller(keys);

    OPEN.with(|o| *o.borrow_mut() = Some(Open { name: name.to_string(), win: win.clone(), panel }));
    notify_open(name, true);
    win.present();
}

struct PInner {
    width: Cell<i32>,
    closers: RefCell<Vec<Box<dyn FnOnce()>>>,
    sources: RefCell<Vec<glib::SourceId>>,
    key: RefCell<Option<Rc<dyn Fn(gdk::Key, gdk::ModifierType) -> bool>>>,
    closed: Cell<bool>,
}

/// Handle given to a panel's builder: timers and watches that end with the panel.
#[derive(Clone)]
pub struct Panel(Rc<PInner>);

impl Panel {
    pub fn set_width(&self, w: i32) {
        self.0.width.set(w);
    }

    pub fn closed(&self) -> bool {
        self.0.closed.get()
    }

    /// Calls `f` every `ms` while the panel is open.
    pub fn every(&self, ms: u64, f: impl Fn() + 'static) {
        let id = glib::timeout_add_local(std::time::Duration::from_millis(ms), move || {
            f();
            glib::ControlFlow::Continue
        });
        self.0.sources.borrow_mut().push(id);
    }

    /// Calls `f` on every `n.emit()` while the panel is open.
    pub fn watch(&self, n: &Notify, f: impl Fn() + 'static) {
        let id = n.connect(f);
        let n = n.clone();
        self.on_close(move || n.disconnect(id));
    }

    pub fn on_close(&self, f: impl FnOnce() + 'static) {
        self.0.closers.borrow_mut().push(Box::new(f));
    }

    /// Key handler; return true when the key was used.
    pub fn on_key(&self, f: impl Fn(gdk::Key, gdk::ModifierType) -> bool + 'static) {
        *self.0.key.borrow_mut() = Some(Rc::new(f));
    }

    pub fn close(&self) {
        close();
    }

    fn shutdown(&self) {
        self.0.closed.set(true);
        for id in self.0.sources.borrow_mut().drain(..) {
            id.remove();
        }
        let closers: Vec<_> = self.0.closers.borrow_mut().drain(..).collect();
        for f in closers {
            f();
        }
        *self.0.key.borrow_mut() = None;
    }
}

/// The standard card layout: header (title · detail, right-side controls),
/// scrolling body, optional footer.
pub struct Card {
    pub root: gtk::Box,
    pub head: gtk::Box,
    pub detail: gtk::Label,
    pub controls: gtk::Box,
    pub body: gtk::Box,
    pub foot: gtk::Box,
}

impl Card {
    /// Header as a little statusline: the title in a mode-coloured block,
    /// a slanted edge, the detail, controls on the right.
    pub fn new(title: &str, detail: &str) -> Card {
        let root = ui::vbox(0, &[]);
        let head = ui::hbox(0, &["head"]);
        let t = theme();
        // titles are uppercase like nvim's mode names, except proper names (VectorOS)
        let shown = if title.chars().any(|c| c.is_uppercase()) { title.to_string() } else { title.to_uppercase() };
        let block = ui::text(&shown, &["htitle"]);
        block.set_valign(gtk::Align::Fill);
        head.append(&block);
        head.append(&crate::bar::slant(false, crate::bar::Kind::Block, crate::bar::Kind::Plain, &t));
        let d = ui::label("", &["hdetail"]);
        d.set_margin_start(6);
        d.set_hexpand(true);
        ui::ellipsize(&d, 34);
        head.append(&d);
        let controls = ui::hbox(6, &[]);
        controls.set_valign(gtk::Align::Center);
        head.append(&controls);
        root.append(&head);
        let body = ui::vbox(0, &["body"]);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(max_body());
        scroll.set_child(Some(&body));
        root.append(&scroll);
        let foot = ui::hbox(2, &["foot"]);
        foot.set_visible(false);
        root.append(&foot);
        let card = Card { root, head, detail: d, controls, body, foot };
        card.set_detail(detail);
        card
    }

    pub fn set_detail(&self, s: &str) {
        self.detail.set_markup(s);
    }

    pub fn foot_add(&self, w: &impl IsA<gtk::Widget>) {
        self.foot.set_visible(true);
        self.foot.append(w);
    }
}
