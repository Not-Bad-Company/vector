//! The cards: a layer-shell column in a corner of the focused screen, newest
//! on top. Each card is a little statusline (app block, slanted edge, age,
//! close), then the text, an optional picture, progress, actions, and a thin
//! line along the bottom that drains as its time runs out.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gtk::glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, graphene, gsk};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::Note;
use crate::ui;
use crate::util;

// ── a bin that can slide its child sideways (the swipe) ─────────────────

/// Fixed card height matching the 2-line clipboard copy notification.
const CARD_HEIGHT: i32 = 88;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Swipe {
        pub offset: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Swipe {
        const NAME: &'static str = "VbSwipe";
        type Type = super::Swipe;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Swipe {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for Swipe {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }
        fn measure(&self, o: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let w = super::super::get().cfg().width.max(200);
            match o {
                gtk::Orientation::Horizontal => (w, w, -1, -1),
                gtk::Orientation::Vertical => (CARD_HEIGHT, CARD_HEIGHT, -1, -1),
                _ => (0, 0, -1, -1),
            }
        }
        fn size_allocate(&self, w: i32, h: i32, baseline: i32) {
            if let Some(c) = self.obj().first_child() {
                let t = gsk::Transform::new()
                    .translate(&graphene::Point::new(self.offset.get() as f32, 0.0));
                c.allocate(w, h, baseline, Some(t));
            }
        }
    }

    #[derive(Default)]
    pub struct Thumb {
        pub w: Cell<i32>,
        pub h: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Thumb {
        const NAME: &'static str = "VbThumb";
        type Type = super::Thumb;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Thumb {
        fn dispose(&self) {
            while let Some(c) = self.obj().first_child() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for Thumb {
        fn measure(&self, o: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let s = match o {
                gtk::Orientation::Horizontal => self.w.get().max(0),
                gtk::Orientation::Vertical => self.h.get().max(0),
                _ => 0,
            };
            (s, s, -1, -1)
        }
        fn size_allocate(&self, w: i32, h: i32, baseline: i32) {
            if let Some(c) = self.obj().first_child() {
                c.allocate(w, h, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct Swipe(ObjectSubclass<imp::Swipe>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

glib::wrapper! {
    pub struct Thumb(ObjectSubclass<imp::Thumb>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// A cover-cropped picture whose natural size is `(w, h)`, not the texture's resolution.
pub fn thumb(tex: &gdk::Texture, w: i32, h: i32, class: &str) -> gtk::Widget {
    let t: Thumb = glib::Object::new();
    t.imp().w.set(w);
    t.imp().h.set(h);
    t.set_overflow(gtk::Overflow::Hidden);
    if w < 0 {
        t.set_hexpand(true);
    } else {
        t.set_valign(gtk::Align::Start);
    }
    if !class.is_empty() {
        t.add_css_class(class);
    }
    let p = gtk::Picture::for_paintable(tex);
    p.set_content_fit(gtk::ContentFit::Cover);
    p.set_can_shrink(true);
    p.set_parent(&t);
    t.upcast()
}

impl Swipe {
    fn new(child: &impl IsA<gtk::Widget>) -> Swipe {
        let s: Swipe = glib::Object::new();
        child.set_parent(&s);
        s
    }
    fn offset(&self) -> f64 {
        self.imp().offset.get()
    }
    fn set_offset(&self, x: f64) {
        self.imp().offset.set(x);
        let w = self.width().max(1) as f64;
        self.set_opacity((1.0 - (x.abs() / w) * 1.1).clamp(0.0, 1.0));
        self.queue_allocate();
    }
}

// ── the column ──────────────────────────────────────────────────────────

struct CardW {
    id: u32,
    note: Note,
    swipe: Swipe,
    age: gtk::Label,
    leaving: Rc<Cell<bool>>,
}

struct Popup {
    app: gtk::Application,
    win: RefCell<Option<gtk::ApplicationWindow>>,
    column: RefCell<Option<gtk::Box>>,
    more: RefCell<Option<gtk::Button>>,
    cards: RefCell<Vec<CardW>>,
    hover: Cell<bool>,
}

thread_local! {
    static POP: RefCell<Option<Rc<Popup>>> = const { RefCell::new(None) };
}

fn pop() -> Option<Rc<Popup>> {
    POP.with(|p| p.borrow().clone())
}

pub fn init(app: &gtk::Application) {
    let p = Rc::new(Popup {
        app: app.clone(),
        win: RefCell::default(),
        column: RefCell::default(),
        more: RefCell::default(),
        cards: RefCell::default(),
        hover: Cell::new(false),
    });
    POP.with(|x| *x.borrow_mut() = Some(p.clone()));
    let w = Rc::downgrade(&p);
    super::get().changed.connect(move || {
        if let Some(p) = w.upgrade() {
            p.sync();
        }
    });
    // ages ("now", "4m") stay true
    let w = Rc::downgrade(&p);
    glib::timeout_add_seconds_local(30, move || match w.upgrade() {
        Some(p) => {
            for c in p.cards.borrow().iter() {
                c.age.set_text(&age(&c.note));
            }
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// After a theme or config change: rebuild the cards.
pub fn restyle() {
    if let Some(p) = pop() {
        let old: Vec<CardW> = p.cards.borrow_mut().drain(..).collect();
        if let Some(col) = p.column.borrow().as_ref() {
            for c in old {
                col.remove(&c.swipe);
            }
        }
        if let Some(w) = p.win.borrow_mut().take() {
            w.destroy();
        }
        *p.column.borrow_mut() = None;
        *p.more.borrow_mut() = None;
        p.sync();
    }
}

fn focused_monitor() -> Option<gdk::Monitor> {
    let name = crate::hypr::json("monitors").and_then(|v| {
        v.as_array()?
            .iter()
            .find(|m| m["focused"].as_bool() == Some(true))
            .and_then(|m| m["name"].as_str().map(str::to_string))
    });
    let d = gdk::Display::default()?;
    let ms = d.monitors();
    let all: Vec<gdk::Monitor> = (0..ms.n_items())
        .filter_map(|i| ms.item(i).and_downcast::<gdk::Monitor>())
        .collect();
    name.and_then(|n| {
        all.iter()
            .find(|m| m.connector().is_some_and(|c| c == n))
            .cloned()
    })
    .or_else(|| all.first().cloned())
}

fn age(n: &Note) -> String {
    let s = (util::now() - n.time).max(0.0) as i64;
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn hypr_gaps_out() -> (i32, i32, i32, i32) {
    if let Some(v) = crate::hypr::json("getoption general:gaps_out") {
        if let Some(s) = v["custom"].as_str() {
            let nums: Vec<i32> = s
                .split_whitespace()
                .filter_map(|p| p.parse::<i32>().ok())
                .collect();
            match nums.as_slice() {
                [t, r, b, l, ..] => return (*t, *r, *b, *l),
                [v] => return (*v, *v, *v, *v),
                _ => {}
            }
        }
        if let Some(n) = v["int"].as_i64() {
            let n = n as i32;
            return (n, n, n, n);
        }
    }
    (5, 5, 5, 5)
}

impl Popup {
    fn ensure_window(self: &Rc<Self>) -> gtk::Box {
        if let Some(c) = self.column.borrow().as_ref() {
            return c.clone();
        }
        let cfg = super::get().cfg();
        let win = gtk::ApplicationWindow::new(&self.app);
        win.add_css_class("vnotify");
        win.init_layer_shell();
        win.set_layer(Layer::Overlay);
        win.set_namespace(Some("vector-notifications"));
        win.set_keyboard_mode(KeyboardMode::None);
        win.set_exclusive_zone(0);
        let pos = cfg.position.to_lowercase();
        let top = !pos.starts_with("bottom");
        win.set_anchor(if top { Edge::Top } else { Edge::Bottom }, true);
        if pos.ends_with("left") {
            win.set_anchor(Edge::Left, true);
        } else if !pos.ends_with("center") {
            win.set_anchor(Edge::Right, true);
        }
        // clear of the window gaps, so the cards float instead of lining up
        // with the tiled windows' edges
        let (gt, gr, gb, gl) = hypr_gaps_out();
        let off = |g: i32| (g + 12).max(16);
        win.set_margin(Edge::Top, off(gt));
        win.set_margin(Edge::Right, off(gr));
        win.set_margin(Edge::Bottom, off(gb));
        win.set_margin(Edge::Left, off(gl));
        if let Some(m) = focused_monitor() {
            win.set_monitor(Some(&m));
        }
        let col = ui::vbox(6, &["ncolumn"]);
        col.set_size_request(cfg.width, -1);
        col.set_valign(if top {
            gtk::Align::Start
        } else {
            gtk::Align::End
        });
        let more = gtk::Button::with_label("");
        more.add_css_class("nmore");
        more.set_visible(false);
        ui::pointer(&more);
        more.connect_clicked(|_| open_panel());
        if top {
            col.append(&more);
        } else {
            col.prepend(&more);
        }
        win.set_child(Some(&col));
        // hover: clocks stop, every card shows its whole text
        let motion = gtk::EventControllerMotion::new();
        let w = Rc::downgrade(self);
        motion.connect_enter(move |_, _, _| {
            if let Some(p) = w.upgrade() {
                p.set_hover(true);
            }
        });
        let w = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let Some(p) = w.upgrade() {
                p.set_hover(false);
            }
        });
        win.add_controller(motion);
        *self.win.borrow_mut() = Some(win);
        *self.column.borrow_mut() = Some(col.clone());
        *self.more.borrow_mut() = Some(more);
        col
    }

    fn set_hover(&self, on: bool) {
        if self.hover.replace(on) == on {
            return;
        }
        super::get().pause(on);
    }

    fn sync(self: &Rc<Self>) {
        let store = super::get();
        let cfg = store.cfg();
        let notes = store.visible();
        let shown: Vec<&Note> = notes.iter().take(cfg.max_visible.max(1)).collect();
        if shown.is_empty() && self.column.borrow().is_none() {
            return;
        }
        let col = self.ensure_window();
        let top = !cfg.position.to_lowercase().starts_with("bottom");

        // gone: slide out, then remove
        let gone: Vec<u32> = self
            .cards
            .borrow()
            .iter()
            .filter(|c| !c.leaving.get() && !shown.iter().any(|n| n.id == c.id))
            .map(|c| c.id)
            .collect();
        for id in gone {
            self.leave(id);
        }
        // new or changed
        let mut prev: Option<gtk::Widget> = None;
        for n in &shown {
            let existing = self
                .cards
                .borrow()
                .iter()
                .position(|c| c.id == n.id && !c.leaving.get());
            let widget = match existing {
                Some(i) if self.cards.borrow()[i].note == **n => {
                    self.cards.borrow()[i].swipe.clone()
                }
                Some(i) => {
                    // updated in place (progress, replaced text, one more of the same)
                    let old = self.cards.borrow()[i].swipe.clone();
                    let c = self.card(n);
                    col.insert_child_after(&c.swipe, Some(&old));
                    col.remove(&old);
                    let w = c.swipe.clone();
                    self.cards.borrow_mut()[i] = c;
                    w
                }
                None => {
                    let c = self.card(n);
                    let w = c.swipe.clone();
                    col.append(&w);
                    self.cards.borrow_mut().push(c);
                    w
                }
            }
            .upcast::<gtk::Widget>();
            // newest nearest the screen edge
            if top {
                col.reorder_child_after(&widget, prev.as_ref());
            } else {
                match &prev {
                    Some(p) => col.reorder_child_after(&widget, p.prev_sibling().as_ref()),
                    None => col.reorder_child_after(
                        &widget,
                        col.last_child().filter(|l| l != &widget).as_ref(),
                    ),
                }
            }
            prev = Some(widget);
        }
        if let Some(more) = self.more.borrow().as_ref() {
            let rest = notes.len().saturating_sub(shown.len());
            more.set_visible(rest > 0);
            more.set_label(&format!("+{rest} more  ·  open notifications"));
            if top {
                col.reorder_child_after(
                    more,
                    col.last_child()
                        .filter(|l| l != more.upcast_ref::<gtk::Widget>())
                        .as_ref(),
                );
            } else {
                col.reorder_child_after(more, None::<&gtk::Widget>);
            }
        }
        let win = self.win.borrow().clone();
        if let Some(win) = win {
            let any = !self.cards.borrow().is_empty();
            if any && !win.is_visible() {
                if let Some(m) = focused_monitor() {
                    win.set_monitor(Some(&m));
                }
                win.present();
            } else if !any && win.is_visible() {
                self.set_hover(false);
                win.set_visible(false);
            }
        }
    }

    /// Slide out to the right (or keep going the way it was swiped), then drop.
    fn leave(self: &Rc<Self>, id: u32) {
        let Some((swipe, leaving)) = self
            .cards
            .borrow()
            .iter()
            .find(|c| c.id == id)
            .map(|c| (c.swipe.clone(), c.leaving.clone()))
        else {
            return;
        };
        leaving.set(true);
        let from = swipe.offset();
        let dir = if from < 0.0 { -1.0 } else { 1.0 };
        let dist = swipe.width().max(300) as f64 + 20.0;
        let start = Instant::now();
        let w = Rc::downgrade(self);
        swipe.add_tick_callback(move |sw, _| {
            let t = (start.elapsed().as_secs_f64() / 0.16).min(1.0);
            let ease = 1.0 - (1.0 - t).powi(3);
            sw.set_offset(from + (dir * dist - from) * ease);
            if t < 1.0 {
                return glib::ControlFlow::Continue;
            }
            if let Some(p) = w.upgrade() {
                p.cards
                    .borrow_mut()
                    .retain(|c| c.id != id || !c.leaving.get());
                if let Some(col) = p.column.borrow().as_ref() {
                    col.remove(sw);
                }
                if p.cards.borrow().is_empty() {
                    p.sync();
                }
            }
            glib::ControlFlow::Break
        });
    }

    fn card(self: &Rc<Self>, n: &Note) -> CardW {
        let theme = crate::panel::current_theme();
        let id = n.id;
        let card = ui::vbox(0, &["card", "ncard"]);
        card.set_overflow(gtk::Overflow::Hidden);
        card.set_size_request(super::get().cfg().width.max(200), CARD_HEIGHT);
        match n.urgency {
            0 => card.add_css_class("low"),
            2 => card.add_css_class("critical"),
            _ => {}
        }

        // statusline: APP ◤ age ×n ............ 󰅖
        let head = ui::hbox(0, &["head"]);
        let block = ui::text(&n.label().to_uppercase(), &["htitle"]);
        block.set_valign(gtk::Align::Fill);
        ui::ellipsize(&block, 22);
        head.append(&block);
        let block_color = match n.urgency {
            2 => theme.color1.clone(),
            0 => theme.foreground.clone(),
            _ => theme.accent.clone(),
        };
        head.append(&slant(
            &block_color,
            if n.urgency == 0 { 0.28 } else { 1.0 },
        ));
        let icon = app_icon(n);
        if let Some(i) = &icon {
            i.set_margin_start(6);
            head.append(i);
        }
        let age_l = ui::text(&age(n), &["hdetail"]);
        age_l.set_margin_start(if icon.is_some() { 6 } else { 4 });
        head.append(&age_l);
        if n.count > 1 {
            let c = ui::text(&format!("×{}", n.count), &["ncount"]);
            c.set_margin_start(8);
            head.append(&c);
        }
        if n.timeout == 0 && n.urgency < 2 {
            let pin = ui::text("󰐃", &["hdetail", "npin"]);
            pin.set_tooltip_text(Some("stays until dismissed"));
            pin.set_margin_start(8);
            head.append(&pin);
        }
        head.append(&ui::spacer());
        let x = ui::icon_button("󰅖", "", move || super::get().dismiss(id));
        x.add_css_class("nclose");
        head.append(&x);
        card.append(&head);

        // fixed 2-line content area (same dimensions as the clipboard copy notification)
        let pic = texture(n);
        let content = ui::hbox(12, &["ncontent"]);
        content.set_vexpand(true);
        if let Some((tex, _, _)) = &pic {
            content.append(&thumb(tex, 36, 36, "nthumb"));
        }
        let texts = ui::vbox(3, &[]);
        texts.set_hexpand(true);
        texts.set_valign(gtk::Align::Center);

        let summary = super::plain(&n.summary);
        let s = ui::text(&summary, &["nsum"]);
        s.set_single_line_mode(true);
        s.set_lines(1);
        s.set_ellipsize(gtk::pango::EllipsizeMode::End);
        s.set_width_chars(1);
        s.set_max_width_chars(1);
        s.set_hexpand(true);
        s.set_xalign(0.0);
        texts.append(&s);

        let body_plain = super::plain(&n.body);
        if let (Some(v), true) = (n.value, body_plain.is_empty()) {
            let row = ui::hbox(10, &["nprogress"]);
            row.append(&ui::meter(v as f64 / 100.0, ""));
            row.append(&ui::text(&format!("{v}%"), &["dim"]));
            texts.append(&row);
        } else {
            let line2 = match (n.value, body_plain.is_empty()) {
                (Some(v), false) => format!("{v}% · {}", super::markup(&body_plain)),
                _ => super::markup(&body_plain),
            };
            let b = ui::label(&line2, &["body-text", "nbody"]);
            b.set_single_line_mode(true);
            b.set_lines(1);
            b.set_ellipsize(gtk::pango::EllipsizeMode::End);
            b.set_width_chars(1);
            b.set_max_width_chars(1);
            b.set_hexpand(true);
            b.set_xalign(0.0);
            texts.append(&b);
        }
        content.append(&texts);
        card.append(&content);

        card.set_tooltip_text(None);
        card.append(&drain(id, &block_color));

        let swipe = Swipe::new(&card);
        gestures(&swipe, id);
        ui::pointer(&swipe);
        CardW {
            id,
            note: n.clone(),
            swipe,
            age: age_l,
            leaving: Rc::new(Cell::new(false)),
        }
    }
}

fn open_panel() {
    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.to_string_lossy().to_string();
        util::spawn(&[&exe, "toggle", "notifications"]);
    }
}

/// The edge after the block, in the block's colour.
fn slant(hex: &str, alpha: f32) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(10);
    d.set_vexpand(true);
    let c = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::WHITE);
    let c = gdk::RGBA::new(c.red(), c.green(), c.blue(), alpha);
    d.set_draw_func(move |_, cr, w, h| {
        cr.move_to(0.0, 0.0);
        cr.line_to(w as f64, 0.0);
        cr.line_to(0.0, h as f64);
        cr.close_path();
        cr.set_source_color(&c);
        let _ = cr.fill();
    });
    d
}

fn desktop_icon(de: &str) -> Option<String> {
    let de = de.trim_end_matches(".desktop");
    if de.is_empty() {
        return None;
    }
    let file = format!("{de}.desktop");
    let dirs = [
        util::home().join(".local/share/applications"),
        std::path::PathBuf::from("/usr/local/share/applications"),
        std::path::PathBuf::from("/usr/share/applications"),
    ];
    for d in dirs {
        if let Some(text) = util::read(d.join(&file)) {
            for line in text.lines() {
                if let Some(icon) = line.strip_prefix("Icon=") {
                    let icon = icon.trim();
                    if !icon.is_empty() {
                        return Some(icon.to_string());
                    }
                }
            }
        }
    }
    None
}

/// The app's icon (theme name, or the desktop entry's), small in the header.
fn app_icon(n: &Note) -> Option<gtk::Image> {
    let display = gdk::Display::default()?;
    let theme = gtk::IconTheme::for_display(&display);
    let named = |name: &str| {
        (!name.is_empty() && !name.starts_with('/') && theme.has_icon(name))
            .then(|| gtk::Image::from_icon_name(name))
    };
    let img = named(&n.icon)
        .or_else(|| desktop_icon(&n.desktop_entry).and_then(|ic| named(&ic)))
        .or_else(|| named(&n.app.to_lowercase()))?;
    img.set_pixel_size(14);
    img.add_css_class("nicon");
    Some(img)
}

/// The note's texture and its dimensions: image data, or a picture file.
fn texture(n: &Note) -> Option<(gdk::Texture, i32, i32)> {
    let tex = n.pixels.clone().or_else(|| {
        (!n.image.is_empty())
            .then(|| gdk::Texture::from_filename(&n.image).ok())
            .flatten()
    })?;
    let (w, h) = (tex.width(), tex.height());
    Some((tex, w, h))
}

/// The bottom line: drains while the card's time runs out.
fn drain(id: u32, hex: &str) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.add_css_class("ndrain");
    d.set_content_height(2);
    d.set_hexpand(true);
    let c = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::WHITE);
    let c = gdk::RGBA::new(c.red(), c.green(), c.blue(), 0.75);
    d.set_draw_func(move |_, cr, w, h| {
        let Some(f) = super::get().remaining(id) else {
            return;
        };
        cr.rectangle(0.0, 0.0, w as f64 * f, h as f64);
        cr.set_source_color(&c);
        let _ = cr.fill();
    });
    d.add_tick_callback(move |d, _| {
        let s = super::get();
        match s.remaining(id) {
            Some(_) => {
                if !s.paused() {
                    d.queue_draw();
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        }
    });
    d
}

/// Click accepts, sideways swipe (touchpad or drag) dismisses, right click
/// dismisses, middle click dismisses all.
fn gestures(sw: &Swipe, id: u32) {
    // two-finger touchpad swipe
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::HORIZONTAL);
    let last = Rc::new(Cell::new((0.0f64, Instant::now())));
    let active = Rc::new(Cell::new(false));
    {
        let (last, active) = (last.clone(), active.clone());
        scroll.connect_scroll_begin(move |_| {
            active.set(true);
            last.set((0.0, Instant::now()));
        });
    }
    {
        let (last, active, w) = (last.clone(), active.clone(), sw.downgrade());
        scroll.connect_scroll(move |c, dx, dy| {
            let Some(sw) = w.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if dx.abs() < dy.abs() {
                return glib::Propagation::Proceed;
            }
            // wheels (no begin/end) tilt in steps
            let px = if c.unit() == gdk::ScrollUnit::Wheel {
                dx * 40.0
            } else {
                dx
            };
            sw.set_offset(sw.offset() - px);
            last.set((-px, Instant::now()));
            if !active.get() {
                settle(&sw, id, 0.0);
            }
            glib::Propagation::Stop
        });
    }
    {
        let (last, active, w) = (last.clone(), active.clone(), sw.downgrade());
        scroll.connect_scroll_end(move |_| {
            active.set(false);
            let Some(sw) = w.upgrade() else { return };
            let (d, at) = last.get();
            let v = if at.elapsed().as_millis() < 80 {
                d
            } else {
                0.0
            };
            settle(&sw, id, v);
        });
    }
    sw.add_controller(scroll);

    // mouse/touch: press and drag sideways, or just click
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    let start_off = Rc::new(Cell::new(0.0));
    let moved = Rc::new(Cell::new(false));
    {
        let (start_off, moved, w) = (start_off.clone(), moved.clone(), sw.downgrade());
        drag.connect_drag_begin(move |_, _, _| {
            moved.set(false);
            if let Some(sw) = w.upgrade() {
                start_off.set(sw.offset());
            }
        });
    }
    {
        let (start_off, moved, w) = (start_off.clone(), moved.clone(), sw.downgrade());
        drag.connect_drag_update(move |_, dx, dy| {
            let Some(sw) = w.upgrade() else { return };
            if !moved.get() && dx.abs() > 8.0 && dx.abs() > dy.abs() {
                moved.set(true);
            }
            if moved.get() {
                sw.set_offset(start_off.get() + dx);
            }
        });
    }
    {
        let (moved, w) = (moved.clone(), sw.downgrade());
        drag.connect_drag_end(move |g, dx, dy| {
            let Some(sw) = w.upgrade() else { return };
            if moved.get() {
                settle(&sw, id, 0.0);
            } else if dx.abs() < 8.0 && dy.abs() < 8.0 && g.current_button() == gdk::BUTTON_PRIMARY
            {
                super::get().activate(id);
            }
        });
    }
    sw.add_controller(drag);

    let other = gtk::GestureClick::new();
    other.set_button(0);
    other.connect_released(move |g, _, _, _| match g.current_button() {
        gdk::BUTTON_SECONDARY => super::get().dismiss(id),
        gdk::BUTTON_MIDDLE => super::get().dismiss_all(),
        _ => {}
    });
    sw.add_controller(other);
}

/// After a swipe: far (or fast) enough dismisses, else it springs back.
fn settle(sw: &Swipe, id: u32, velocity: f64) {
    let x = sw.offset();
    let w = sw.width().max(1) as f64;
    // lenient: a short swipe or a light flick is enough
    if x.abs() > (w * 0.15).min(56.0)
        || (velocity.abs() > 6.0 && velocity.signum() == x.signum() && x.abs() > 14.0)
    {
        super::get().dismiss(id);
        return;
    }
    let from = x;
    let start = Instant::now();
    sw.add_tick_callback(move |sw, _| {
        let t = (start.elapsed().as_secs_f64() / 0.18).min(1.0);
        let ease = 1.0 - (1.0 - t).powi(3);
        sw.set_offset(from * (1.0 - ease));
        if t < 1.0 {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    });
}
