//! The launcher window: a full-screen layer surface (click outside the card
//! closes it) with the card near the top of the focused monitor. Built once,
//! hidden and shown again, so it opens instantly.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::apps::{self, App, Window};
use crate::clip::{self, Clip, Kind};
use crate::emoji::{self, Emoji};
use crate::item::{Action, Icon, Item, Mode};
use crate::notes::{self, Snapshot};
use crate::{calc, convert, util, web};

const WIDTH: i32 = 860;
const HEIGHT: i32 = 540;
const LIST_W: i32 = 420;
const GRID_COLS: u32 = 7;

enum Row {
    Header(String),
    Item(Box<Item>),
}

#[derive(Default)]
struct State {
    mode: Option<Mode>,
    items: Vec<Item>,
    /// store position -> index in `items` (None for headers)
    rows: Vec<Option<usize>>,
    sel: Option<u32>,
    apps: Vec<App>,
    wins: Vec<Window>,
    emoji: Vec<Emoji>,
    clips: Vec<Clip>,
    notes: Snapshot,
    suggestions: Vec<String>,
    suggest_gen: u64,
    /// ctrl+shift+delete was pressed once: the next press clears
    confirm_clear: bool,
    /// a preview built for this action already
    previewed: Option<Action>,
}

pub struct Ui {
    win: gtk::ApplicationWindow,
    card: gtk::Box,
    mode_lbl: gtk::Label,
    entry: gtk::Entry,
    count: gtk::Label,
    store: gio::ListStore,
    list_sel: gtk::SingleSelection,
    grid_sel: gtk::SingleSelection,
    list: gtk::ListView,
    grid: gtk::GridView,
    stack: gtk::Stack,
    empty: gtk::Label,
    preview: gtk::ScrolledWindow,
    tabs: Vec<(Mode, gtk::Box)>,
    keys: gtk::Label,
    dnd: gtk::Label,
    st: RefCell<State>,
    /// set_text from code: do not treat as typing
    quiet: Cell<bool>,
    /// the empty app search is built and waiting (see `prepare_soon`)
    prepared: Cell<bool>,
    _watch: RefCell<Vec<Box<dyn std::any::Any>>>,
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

pub fn get() -> Option<Rc<Ui>> {
    UI.with(|u| u.borrow().clone())
}

pub fn build(app: &gtk::Application) -> Rc<Ui> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("vspace");
    win.set_title(Some("Vector Space"));
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some("vector-space"));
    win.set_keyboard_mode(KeyboardMode::Exclusive);
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        win.set_anchor(e, true);
    }
    win.set_exclusive_zone(-1);

    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.add_css_class("backdrop");
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.set_size_request(WIDTH, HEIGHT);
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    backdrop.append(&card);
    win.set_child(Some(&backdrop));

    // header
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    head.add_css_class("head");
    let mode_lbl = gtk::Label::new(Some("APPS"));
    mode_lbl.add_css_class("mode");
    mode_lbl.set_valign(gtk::Align::Fill);
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    let count = gtk::Label::new(None);
    count.add_css_class("count");
    head.append(&mode_lbl);
    head.append(&slant());
    head.append(&entry);
    card.append(&head);

    // body: list or grid, and the preview
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let list_sel = gtk::SingleSelection::new(Some(store.clone()));
    list_sel.set_autoselect(false);
    list_sel.set_can_unselect(true);
    let grid_sel = gtk::SingleSelection::new(Some(store.clone()));
    grid_sel.set_autoselect(false);
    grid_sel.set_can_unselect(true);
    let list = gtk::ListView::new(Some(list_sel.clone()), Some(row_factory()));
    // a click opens; hovering only highlights (it must not move the keyboard selection)
    list.set_single_click_activate(false);
    let grid = gtk::GridView::new(Some(grid_sel.clone()), Some(emoji_factory()));
    grid.set_single_click_activate(false);
    grid.set_min_columns(GRID_COLS);
    grid.set_max_columns(GRID_COLS);
    let scroll = |w: &gtk::Widget| {
        let s = gtk::ScrolledWindow::new();
        s.set_hscrollbar_policy(gtk::PolicyType::Never);
        s.set_child(Some(w));
        s.set_vexpand(true);
        s
    };
    let empty = gtk::Label::new(None);
    empty.add_css_class("empty");
    empty.set_valign(gtk::Align::Start);
    empty.set_xalign(0.0);
    empty.set_wrap(true);
    empty.set_max_width_chars(30);
    let stack = gtk::Stack::new();
    stack.add_named(&scroll(list.upcast_ref()), Some("list"));
    stack.add_named(&scroll(grid.upcast_ref()), Some("grid"));
    stack.add_named(&empty, Some("empty"));
    stack.set_hexpand(true);
    stack.set_hhomogeneous(false);
    stack.set_size_request(LIST_W, -1);
    let preview = gtk::ScrolledWindow::new();
    preview.add_css_class("preview");
    preview.set_hscrollbar_policy(gtk::PolicyType::Never);
    preview.set_size_request(WIDTH - LIST_W, -1);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    body.append(&stack);
    body.append(&preview);
    card.append(&body);

    // footer
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.add_css_class("foot");
    let mut tabs = Vec::new();
    for m in Mode::ALL {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("tab");
        if !m.prefix().is_empty() {
            let k = gtk::Label::new(Some(m.prefix()));
            k.add_css_class("k");
            b.append(&k);
        }
        b.append(&gtk::Label::new(Some(m.tab())));
        b.set_cursor_from_name(Some("pointer"));
        foot.append(&b);
        tabs.push((m, b));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    foot.append(&spacer);
    let dnd = gtk::Label::new(Some("\u{f009b} silenced"));
    dnd.add_css_class("dnd");
    dnd.set_visible(false);
    foot.append(&dnd);
    let keys = gtk::Label::new(None);
    keys.add_css_class("keys");
    keys.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    keys.set_max_width_chars(44);
    foot.append(&keys);
    card.append(&foot);

    let ui = Rc::new(Ui {
        win,
        card,
        mode_lbl,
        entry,
        count,
        store,
        list_sel,
        grid_sel,
        list,
        grid,
        stack,
        empty,
        preview,
        tabs,
        keys,
        dnd,
        st: RefCell::default(),
        quiet: Cell::new(false),
        prepared: Cell::new(false),
        _watch: RefCell::default(),
    });
    UI.with(|u| *u.borrow_mut() = Some(ui.clone()));
    ui.wire(&backdrop);
    ui.st.borrow_mut().apps = apps::load();
    ui.st.borrow_mut().emoji = emoji::all();
    ui.prepare_soon();
    ui
}

// ── rows ─────────────────────────────────────────────────────────────────

/// The bar's slanted block edge (◤), in the accent.
fn slant() -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(14);
    d.set_valign(gtk::Align::Fill);
    d.set_draw_func(|_, cr, w, h| {
        let c = gdk::RGBA::parse(crate::style::accent()).unwrap_or(gdk::RGBA::WHITE);
        cr.move_to(0.0, 0.0);
        cr.line_to(w as f64, 0.0);
        cr.line_to(0.0, h as f64);
        cr.close_path();
        cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, 1.0);
        let _ = cr.fill();
    });
    d
}

fn icon_widget(icon: &Icon, size: i32) -> gtk::Widget {
    match icon {
        Icon::Glyph(g) => {
            let l = gtk::Label::new(Some(g));
            l.add_css_class("icon");
            l.add_css_class("glyph");
            l.upcast()
        }
        Icon::Text(t) => {
            let l = gtk::Label::new(Some(t));
            l.add_css_class("icon");
            l.add_css_class("emo");
            l.upcast()
        }
        Icon::Color(c) => {
            let c = *c;
            let d = gtk::DrawingArea::new();
            d.set_content_width(size - 6);
            d.set_content_height(size - 6);
            d.set_valign(gtk::Align::Center);
            d.set_halign(gtk::Align::Center);
            d.add_css_class("icon");
            d.set_draw_func(move |_, cr, w, h| {
                cr.set_source_rgba(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0, c.a);
                cr.rectangle(0.5, 0.5, w as f64 - 1.0, h as f64 - 1.0);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.5, 0.5, 0.5, 0.5);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            });
            d.upcast()
        }
        Icon::Gicon(g) => {
            let i = gtk::Image::from_gicon(g);
            i.set_pixel_size(size);
            i.add_css_class("icon");
            i.upcast()
        }
        Icon::File(p) => {
            let img = gtk::Image::new();
            img.set_pixel_size(size);
            img.add_css_class("icon");
            img.add_css_class("thumb");
            let w = img.downgrade();
            thumb(p, size * 2, move |t| {
                if let (Some(img), Some(t)) = (w.upgrade(), t) {
                    img.set_paintable(Some(&t));
                }
            });
            img.upcast()
        }
    }
}

fn row_widget(it: &Item) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("row");
    if let Some(c) = it.class {
        row.add_css_class(c);
    }
    // a fixed icon column, so glyphs, app icons and thumbnails line up
    let ic = icon_widget(&it.icon, 26);
    ic.set_valign(gtk::Align::Center);
    ic.set_halign(gtk::Align::Center);
    let col = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    col.set_size_request(30, -1);
    col.set_valign(gtk::Align::Center);
    ic.set_hexpand(true);
    // explicit, or the icon's hexpand would spread to the row
    col.set_hexpand(false);
    col.append(&ic);
    row.append(&col);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(None);
    title.set_markup(&util::highlight(&util::one_line(&it.title, 160), &it.hl));
    title.add_css_class("title-l");
    title.add_css_class("title");
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    text.append(&title);
    if !it.sub.is_empty() {
        let sub = gtk::Label::new(Some(&it.sub));
        sub.add_css_class("sub");
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&sub);
    }
    row.append(&text);
    if !it.meta.is_empty() {
        let m = gtk::Label::new(Some(&it.meta));
        m.add_css_class("meta");
        m.set_valign(gtk::Align::Center);
        row.append(&m);
    }
    row.upcast()
}

fn row_factory() -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_bind(|_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let Some(b) = li.item().and_downcast::<glib::BoxedAnyObject>() else { return };
        let row = b.borrow::<Row>();
        match &*row {
            Row::Header(t) => {
                let l = gtk::Label::new(Some(t));
                l.add_css_class("section");
                l.set_xalign(0.0);
                li.set_child(Some(&l));
                li.set_selectable(false);
                li.set_activatable(false);
            }
            Row::Item(it) => {
                let w = row_widget(it);
                click_opens(&w, li.position());
                li.set_child(Some(&w));
                li.set_selectable(true);
                li.set_activatable(true);
            }
        }
    });
    f.connect_unbind(|_, obj| {
        if let Some(li) = obj.downcast_ref::<gtk::ListItem>() {
            li.set_child(None::<&gtk::Widget>);
        }
    });
    f
}

fn emoji_factory() -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(|_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("emo-cell");
        let l = gtk::Label::new(None);
        l.set_hexpand(true);
        b.append(&l);
        li.set_child(Some(&b));
    });
    f.connect_bind(|_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else { return };
        let Some(b) = li.item().and_downcast::<glib::BoxedAnyObject>() else { return };
        let Some(cell) = li.child() else { return };
        let Some(l) = cell.first_child().and_downcast::<gtk::Label>() else { return };
        if let Row::Item(it) = &*b.borrow::<Row>() {
            if let Icon::Text(e) = &it.icon {
                l.set_text(e);
            }
            cell.set_tooltip_text(Some(&it.title));
        }
        // the cell is reused: its click handler follows the position
        unsafe { cell.set_data("pos", li.position()) };
        if unsafe { cell.data::<bool>("clicks") }.is_none() {
            unsafe { cell.set_data("clicks", true) };
            let c = gtk::GestureClick::new();
            let cw = cell.downgrade();
            c.connect_released(move |_, _, _, _| {
                let pos = cw.upgrade().and_then(|c| unsafe { c.data::<u32>("pos").map(|p| *p.as_ptr()) });
                if let (Some(ui), Some(p)) = (get(), pos) {
                    ui.st.borrow_mut().sel = Some(p);
                    ui.activate(false);
                }
            });
            cell.add_controller(c);
        }
    });
    f
}

/// A click on a row opens it (the row at `pos` in the list).
fn click_opens(w: &gtk::Widget, pos: u32) {
    let c = gtk::GestureClick::new();
    c.connect_released(move |_, _, _, _| {
        if let Some(ui) = get() {
            ui.st.borrow_mut().sel = Some(pos);
            ui.activate(false);
        }
    });
    w.add_controller(c);
}

// ── thumbnails ───────────────────────────────────────────────────────────

type ThumbCb = Box<dyn FnOnce(Option<gdk::Texture>)>;

thread_local! {
    static THUMBS: RefCell<HashMap<(PathBuf, i32), Option<gdk::Texture>>> = RefCell::default();
    static PENDING: RefCell<HashMap<(PathBuf, i32), Vec<ThumbCb>>> = RefCell::default();
}

/// A picture scaled to fit `size` px, decoded off the GTK thread and cached.
pub fn thumb(path: &Path, size: i32, cb: impl FnOnce(Option<gdk::Texture>) + 'static) {
    let key = (path.to_path_buf(), size);
    if let Some(t) = THUMBS.with(|c| c.borrow().get(&key).cloned()) {
        return cb(t);
    }
    let first = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let v = p.entry(key.clone()).or_default();
        v.push(Box::new(cb));
        v.len() == 1
    });
    if !first {
        return;
    }
    let (tx, rx) = async_channel::bounded(1);
    let p = path.to_path_buf();
    std::thread::spawn(move || {
        let r = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&p, size, size, true).ok().map(|pb| {
            let pb = pb.apply_embedded_orientation().unwrap_or(pb);
            (pb.width(), pb.height(), pb.rowstride(), pb.has_alpha(), pb.read_pixel_bytes())
        });
        let _ = tx.send_blocking(r);
    });
    glib::spawn_future_local(async move {
        let tex = rx.recv().await.ok().flatten().map(|(w, h, stride, alpha, bytes)| {
            let fmt = if alpha { gdk::MemoryFormat::R8g8b8a8 } else { gdk::MemoryFormat::R8g8b8 };
            gdk::MemoryTexture::new(w, h, fmt, &bytes, stride as usize).upcast::<gdk::Texture>()
        });
        THUMBS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() > 400 {
                c.clear();
            }
            c.insert(key.clone(), tex.clone());
        });
        let cbs = PENDING.with(|p| p.borrow_mut().remove(&key)).unwrap_or_default();
        for cb in cbs {
            cb(tex.clone());
        }
    });
}

// ── behaviour ────────────────────────────────────────────────────────────

fn mods(state: gdk::ModifierType) -> (bool, bool, bool) {
    (
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
        state.contains(gdk::ModifierType::ALT_MASK),
    )
}

impl Ui {
    fn wire(self: &Rc<Self>, backdrop: &gtk::Box) {
        // click outside the card closes
        let click = gtk::GestureClick::new();
        let w = Rc::downgrade(self);
        click.connect_pressed(move |g, _, x, y| {
            let Some(ui) = w.upgrade() else { return };
            let Some(bd) = g.widget() else { return };
            if let Some(b) = ui.card.compute_bounds(&bd)
                && !b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
            {
                ui.hide();
            }
        });
        backdrop.add_controller(click);

        let w = Rc::downgrade(self);
        self.entry.connect_changed(move |_| {
            if let Some(ui) = w.upgrade()
                && !ui.quiet.get()
            {
                ui.st.borrow_mut().confirm_clear = false;
                ui.typed();
            }
        });
        let w = Rc::downgrade(self);
        self.entry.connect_activate(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.activate(false);
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, k, _, state| {
            let Some(ui) = w.upgrade() else { return glib::Propagation::Proceed };
            if ui.key(k, state) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        self.win.add_controller(keys);

        for (sel, view) in [(&self.list_sel, "list"), (&self.grid_sel, "grid")] {
            let w = Rc::downgrade(self);
            sel.connect_selected_notify(move |s| {
                let Some(ui) = w.upgrade() else { return };
                let cur = ui.stack.visible_child_name();
                if cur.as_deref() != Some(view) {
                    return;
                }
                let p = s.selected();
                if p != gtk::INVALID_LIST_POSITION && ui.st.borrow().sel != Some(p) {
                    ui.st.borrow_mut().sel = Some(p);
                    ui.selection_changed();
                }
            });
        }
        let w = Rc::downgrade(self);
        self.list.connect_activate(move |_, pos| {
            if let Some(ui) = w.upgrade() {
                ui.st.borrow_mut().sel = Some(pos);
                ui.activate(false);
            }
        });
        let w = Rc::downgrade(self);
        self.grid.connect_activate(move |_, pos| {
            if let Some(ui) = w.upgrade() {
                ui.st.borrow_mut().sel = Some(pos);
                ui.activate(false);
            }
        });

        for (m, b) in &self.tabs {
            let (w, m) = (Rc::downgrade(self), *m);
            let c = gtk::GestureClick::new();
            c.connect_released(move |_, _, _, _| {
                if let Some(ui) = w.upgrade() {
                    ui.set_mode(m, "");
                }
            });
            b.add_controller(c);
        }

        // live updates
        let w = Rc::downgrade(self);
        let mon = gio::AppInfoMonitor::get();
        mon.connect_changed(move |_| {
            if let Some(ui) = w.upgrade() {
                ui.st.borrow_mut().apps = apps::load();
                if ui.mode() == Mode::Apps && ui.win.is_visible() {
                    ui.requery();
                } else if !ui.win.is_visible() {
                    ui.prepare_soon();
                }
            }
        });
        self._watch.borrow_mut().push(Box::new(mon));
        let _ = std::fs::create_dir_all(clip::dir());
        if let Ok(fm) = gio::File::for_path(clip::dir()).monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            let w = Rc::downgrade(self);
            fm.connect_changed(move |_, f, _, ev| {
                if f.basename().is_some_and(|b| b.as_os_str() == "history.json")
                    && matches!(ev, gio::FileMonitorEvent::ChangesDoneHint | gio::FileMonitorEvent::Created | gio::FileMonitorEvent::Renamed | gio::FileMonitorEvent::MovedIn)
                    && let Some(ui) = w.upgrade()
                    && ui.mode() == Mode::Clipboard
                    && ui.win.is_visible()
                {
                    ui.st.borrow_mut().clips = clip::load().items;
                    ui.requery();
                }
            });
            self._watch.borrow_mut().push(Box::new(fm));
        }
        let w = Rc::downgrade(self);
        if let Some(sub) = notes::watch(move || {
            if let Some(ui) = w.upgrade()
                && ui.mode() == Mode::Notes
                && ui.win.is_visible()
            {
                ui.refresh_notes(false);
            }
        }) {
            self._watch.borrow_mut().push(Box::new(sub));
        }
    }

    pub fn mode(&self) -> Mode {
        self.st.borrow().mode.unwrap_or(Mode::Apps)
    }

    /// `vector-space [mode]`: open in that mode; again closes.
    pub fn toggle(self: &Rc<Self>, mode: Mode) {
        if self.win.is_visible() {
            if self.mode() == mode {
                self.hide();
            } else {
                self.set_mode(mode, "");
            }
            return;
        }
        self.show(mode);
    }

    pub fn show(self: &Rc<Self>, mode: Mode) {
        let t = std::time::Instant::now();
        self.place();
        self.st.borrow_mut().wins = apps::windows();
        // the app list was built right after the last close (see `hide`)
        if !(mode == Mode::Apps && self.prepared.get()) {
            self.set_mode(mode, "");
        }
        self.prepared.set(false);
        let t1 = t.elapsed();
        self.win.present();
        self.entry.grab_focus();
        if std::env::var_os("VECTOR_SPACE_TIMING").is_some() {
            eprintln!("rows {t1:?} shown {:?}", t.elapsed());
        }
        if std::env::var_os("VECTOR_SPACE_TEST").is_some() {
            let c = self.card.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                if let Some(r) = c.parent().and_then(|p| c.compute_bounds(&p)) {
                    eprintln!("card {} {} {}x{}", r.x(), r.y(), r.width(), r.height());
                }
            });
        }
    }

    pub fn hide(self: &Rc<Self>) {
        self.win.set_visible(false);
        {
            let mut s = self.st.borrow_mut();
            s.confirm_clear = false;
            s.suggest_gen += 1;
            s.suggestions.clear();
        }
        self.prepare_soon();
    }

    /// Builds the empty app search while nobody looks, so the next open only
    /// has to draw.
    pub fn prepare_soon(self: &Rc<Self>) {
        self.prepared.set(false);
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
            if let Some(ui) = w.upgrade()
                && !ui.win.is_visible()
            {
                ui.set_mode(Mode::Apps, "");
                ui.prepared.set(true);
            }
        });
    }

    /// The focused monitor, and the card a sixth of the way down it.
    fn place(&self) {
        let name = util::hypr_json("monitors").and_then(|v| {
            v.as_array()?
                .iter()
                .find(|m| m["focused"].as_bool() == Some(true))
                .and_then(|m| m["name"].as_str().map(str::to_string))
        });
        let Some(display) = gdk::Display::default() else { return };
        let mons = display.monitors();
        let mon = (0..mons.n_items())
            .filter_map(|i| mons.item(i).and_downcast::<gdk::Monitor>())
            .find(|m| name.is_some() && m.connector().map(|c| c.to_string()) == name);
        if let Some(m) = &mon {
            self.win.set_monitor(Some(m));

        }
    }

    fn set_mode(self: &Rc<Self>, mode: Mode, text: &str) {
        let prev = self.st.borrow().mode;
        {
            let mut s = self.st.borrow_mut();
            s.mode = Some(mode);
            s.sel = None;
            s.confirm_clear = false;
            s.previewed = None;
            s.suggestions.clear();
            s.suggest_gen += 1;
        }
        for m in Mode::ALL {
            self.card.remove_css_class(&format!("m-{}", m.name()));
        }
        self.card.add_css_class(&format!("m-{}", mode.name()));
        self.mode_lbl.set_text(&mode.name().to_uppercase());
        for (m, b) in &self.tabs {
            if *m == mode {
                b.add_css_class("on");
            } else {
                b.remove_css_class("on");
            }
        }
        self.preview.set_visible(mode.preview());
        self.keys.set_markup(&self.hints());
        self.quiet.set(true);
        self.entry.set_placeholder_text(Some(mode.placeholder()));
        self.entry.set_text(text);
        self.entry.set_position(-1);
        self.quiet.set(false);
        self.dnd.set_visible(false);
        match mode {
            Mode::Clipboard => self.st.borrow_mut().clips = clip::load().items,
            Mode::Convert => {
                self.st.borrow_mut().clips = clip::load().items;
                convert::refresh_rates();
            }
            Mode::Notes => {
                self.refresh_notes(prev != Some(Mode::Notes) || !self.win.is_visible());
            }
            _ => {}
        }
        self.requery();
    }

    /// Footer key hints: the keys bright, what they do dim.
    fn hints(&self) -> String {
        let pairs: &[(&str, &str)] = match self.mode() {
            Mode::Apps => &[("⏎", "open"), ("tab", "modes")],
            Mode::Clipboard => &[("⏎", "copy"), ("^⏎", "paste"), ("^p", "pin")],
            Mode::Notes => &[("⏎", "open"), ("⇧del", "dismiss")],
            Mode::Emoji => &[("⏎", "copy"), ("^⏎", "paste")],
            Mode::Web => &[("⏎", "search in zen")],
            Mode::Calc => &[("⏎", "copy")],
            Mode::Convert => &[("⏎", "copy or convert")],
        };
        pairs
            .iter()
            .map(|(k, v)| format!("<span alpha=\"85%\" weight=\"bold\">{}</span> {}", util::escape(k), util::escape(v)))
            .collect::<Vec<_>>()
            .join("    ")
    }

    fn refresh_notes(self: &Rc<Self>, mark_seen: bool) {
        let w = Rc::downgrade(self);
        notes::fetch(move |snap| {
            let Some(ui) = w.upgrade() else { return };
            if mark_seen && snap.notes.iter().any(|n| n.unseen) {
                notes::mark_seen();
            }
            ui.dnd.set_visible(snap.dnd && ui.mode() == Mode::Notes);
            ui.st.borrow_mut().notes = snap;
            if ui.mode() == Mode::Notes {
                ui.requery();
            }
        });
    }

    fn typed(self: &Rc<Self>) {
        if self.mode() == Mode::Web {
            self.ask_suggestions();
        }
        self.st.borrow_mut().sel = None;
        self.requery();
    }

    fn ask_suggestions(self: &Rc<Self>) {
        let q = self.entry.text().trim().to_string();
        let generation = {
            let mut s = self.st.borrow_mut();
            s.suggest_gen += 1;
            if q.is_empty() {
                s.suggestions.clear();
            }
            s.suggest_gen
        };
        if q.is_empty() {
            return;
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(130), move || {
            let Some(ui) = w.upgrade() else { return };
            if ui.st.borrow().suggest_gen != generation {
                return;
            }
            let (tx, rx) = async_channel::bounded(1);
            let q2 = q.clone();
            std::thread::spawn(move || {
                let _ = tx.send_blocking(web::suggest(&q2));
            });
            let w = Rc::downgrade(&ui);
            glib::spawn_future_local(async move {
                let Ok(r) = rx.recv().await else { return };
                let Some(ui) = w.upgrade() else { return };
                if ui.st.borrow().suggest_gen != generation || ui.mode() != Mode::Web {
                    return;
                }
                ui.st.borrow_mut().suggestions = r;
                ui.requery();
            });
        });
    }

    /// Recomputes the rows for the current mode and text, keeping the
    /// selected entry selected when it is still there.
    fn requery(self: &Rc<Self>) {
        let q = self.entry.text().to_string();
        let mode = self.mode();
        let keep = {
            let s = self.st.borrow();
            s.sel.and_then(|p| s.rows.get(p as usize).copied().flatten()).and_then(|i| s.items.get(i)).map(|i| i.action.clone())
        };
        let items = {
            let s = self.st.borrow();
            match mode {
                Mode::Apps => apps::query(&s.apps, &s.wins, &q),
                Mode::Clipboard => clip_items(&s.clips, &q),
                Mode::Notes => notes::query(&s.notes, &q),
                Mode::Emoji => emoji::query(&s.emoji, &q),
                Mode::Web => web::query(&q, &s.suggestions),
                Mode::Calc => calc_items(&q),
                Mode::Convert => convert::query(&q, &s.clips),
            }
        };
        let grid = mode == Mode::Emoji;
        let mut rows = Vec::new();
        let mut objs = Vec::new();
        for (i, it) in items.iter().enumerate() {
            if !grid && let Some(h) = &it.section {
                rows.push(None);
                objs.push(glib::BoxedAnyObject::new(Row::Header(h.clone())));
            }
            rows.push(Some(i));
            objs.push(glib::BoxedAnyObject::new(Row::Item(Box::new(it.clone()))));
        }
        let first = rows.iter().position(|r| r.is_some()).map(|p| p as u32);
        let sel = keep
            .filter(|_| self.st.borrow().sel.is_some())
            .and_then(|a| rows.iter().position(|r| r.and_then(|i| items.get(i)).is_some_and(|it| it.action == a)))
            .map(|p| p as u32)
            .or(first);
        let n = items.len();
        {
            let mut s = self.st.borrow_mut();
            s.items = items;
            s.rows = rows;
            s.sel = sel;
            s.previewed = None;
        }
        self.store.splice(0, self.store.n_items(), &objs);
        self.count.set_text(&match (mode, n) {
            (Mode::Calc | Mode::Web, _) | (_, 0) => String::new(),
            (_, 1) => "1 result".into(),
            _ => format!("{n} results"),
        });
        if n == 0 {
            self.empty.set_text(&self.empty_text(&q));
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack.set_visible_child_name(if grid { "grid" } else { "list" });
        }
        self.show_selection();
        self.selection_changed();
    }

    fn empty_text(&self, q: &str) -> String {
        let s = self.st.borrow();
        let q = q.trim();
        match self.mode() {
            Mode::Clipboard if q.is_empty() => "nothing copied yet".into(),
            Mode::Notes if !s.notes.ok => "no notification server: vector-bar is not running".into(),
            Mode::Notes if q.is_empty() => "no notifications".into(),
            Mode::Web => "type to search google in zen, or a web address to open it".into(),
            Mode::Calc => "type a sum:   2^10   sqrt(2)*3   0xff + 1   (4+5)%3".into(),
            Mode::Convert if q.is_empty() => "type a value:  10 km to mi   100 gulden   50 usd in inr   #c9a467   1793286669\n\nor copy a file (an image, video, song or document) and come back here to change its format".into(),
            _ => format!("nothing matches “{q}”"),
        }
    }

    fn show_selection(&self) {
        let sel = self.st.borrow().sel;
        let grid = self.mode() == Mode::Emoji;
        let (model, other) = if grid { (&self.grid_sel, &self.list_sel) } else { (&self.list_sel, &self.grid_sel) };
        other.set_selected(gtk::INVALID_LIST_POSITION);
        match sel {
            Some(p) => {
                model.set_selected(p);
                if grid {
                    self.grid.scroll_to(p, gtk::ListScrollFlags::NONE, None);
                } else {
                    // keep a section heading right above the first row visible
                    let at = if p > 0 && self.st.borrow().rows.get(p as usize - 1) == Some(&None) { p - 1 } else { p };
                    self.list.scroll_to(at, gtk::ListScrollFlags::NONE, None);
                }
            }
            None => model.set_selected(gtk::INVALID_LIST_POSITION),
        }
    }

    fn selected(&self) -> Option<Item> {
        let s = self.st.borrow();
        let i = s.rows.get(s.sel? as usize).copied().flatten()?;
        s.items.get(i).cloned()
    }

    /// Moves the selection by `d` rows, over headings, without wrapping past
    /// the ends unless already there.
    fn step(&self, d: i32) {
        let (rows, cur) = {
            let s = self.st.borrow();
            (s.rows.clone(), s.sel)
        };
        let n = rows.len() as i32;
        if n == 0 {
            return;
        }
        let Some(cur) = cur else {
            self.st.borrow_mut().sel = rows.iter().position(|r| r.is_some()).map(|p| p as u32);
            self.show_selection();
            self.selection_changed();
            return;
        };
        let cur = cur as i32;
        let last = rows.iter().rposition(|r| r.is_some()).unwrap_or(0) as i32;
        let first = rows.iter().position(|r| r.is_some()).unwrap_or(0) as i32;
        let mut p = if d.abs() == 1 && cur == last && d > 0 {
            first
        } else if d.abs() == 1 && cur == first && d < 0 {
            last
        } else {
            (cur + d).clamp(first, last)
        };
        while p >= 0 && p < n && rows[p as usize].is_none() {
            p += d.signum();
        }
        if !(0..n).contains(&p) {
            return;
        }
        self.st.borrow_mut().sel = Some(p as u32);
        self.show_selection();
        self.selection_changed();
    }

    fn selection_changed(&self) {
        if !self.mode().preview() {
            return;
        }
        let it = self.selected();
        let a = it.as_ref().map(|i| i.action.clone());
        if self.st.borrow().previewed == a && a.is_some() {
            return;
        }
        self.st.borrow_mut().previewed = a;
        let w: gtk::Widget = match it {
            Some(it) => self.preview_for(&it),
            None => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        };
        self.preview.set_child(Some(&w));
        self.preview.vadjustment().set_value(0.0);
    }

    fn preview_for(&self, it: &Item) -> gtk::Widget {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        b.add_css_class("pbox");
        let kv = |k: &str, v: &str| {
            let l = gtk::Label::new(None);
            l.set_markup(&format!("<span alpha=\"55%\">{}</span>   {}", util::escape(k), util::escape(v)));
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            l.add_css_class("pkv");
            l
        };
        match &it.action {
            Action::Clip(id) => {
                let c = self.st.borrow().clips.iter().find(|c| c.id == *id).cloned();
                if let Some(c) = c {
                    preview_clip(&b, &c, &kv);
                }
            }
            Action::Note(id) => {
                let n = self.st.borrow().notes.notes.iter().find(|n| n.id == *id).cloned();
                if let Some(n) = n {
                    let t = gtk::Label::new(Some(&n.summary));
                    t.add_css_class("ptitle");
                    t.set_xalign(0.0);
                    t.set_wrap(true);
                    t.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    b.append(&t);
                    let when = glib::DateTime::from_unix_local(n.time as i64)
                        .ok()
                        .and_then(|d| d.format("%a %d %b %H:%M").ok())
                        .map(|s| s.to_lowercase())
                        .unwrap_or_default();
                    b.append(&kv(
                        &notes::label(&n).to_lowercase(),
                        &format!(
                            "{when}{}{}",
                            if n.phone { " · 󰄜 from your phone" } else { "" },
                            if n.count > 1 { format!(" · ×{}", n.count) } else { String::new() }
                        ),
                    ));
                    if !n.body.is_empty() {
                        let body = gtk::Label::new(Some(&n.body));
                        body.add_css_class("pbody");
                        body.set_xalign(0.0);
                        body.set_wrap(true);
                        body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        body.set_selectable(false);
                        b.append(&body);
                    }
                    if !n.image.is_empty() && Path::new(&n.image).is_file() {
                        b.append(&picture(Path::new(&n.image), 400, 260));
                    }
                    if n.live && !n.actions.is_empty() {
                        for (i, (_, label)) in n.actions.iter().filter(|a| a.0 != "default").enumerate().take(9) {
                            let l = gtk::Label::new(None);
                            l.set_markup(&format!("<b> alt+{} </b>  {}", i + 1, util::escape(label)));
                            l.add_css_class("pact");
                            l.set_xalign(0.0);
                            b.append(&l);
                        }
                    } else if !n.live {
                        b.append(&kv("⏎", "focuses the app's window"));
                    }
                }
            }
            Action::Emoji(e) => {
                let big = gtk::Label::new(Some(e));
                big.add_css_class("pbig");
                big.set_margin_top(18);
                b.append(&big);
                let t = gtk::Label::new(Some(&it.title));
                t.add_css_class("ptitle");
                b.append(&t);
                let (codes, words): (Vec<&str>, Vec<&str>) = it.sub.split(" | ").filter(|s| !s.is_empty()).partition(|s| s.starts_with(':'));
                if !codes.is_empty() {
                    b.append(&kv("code", &codes.join(" ")));
                }
                if !words.is_empty() {
                    b.append(&kv("words", &words.join(", ")));
                }
                b.append(&kv("group", &it.meta));
                let cps: Vec<String> = e.chars().filter(|c| *c != '\u{fe0f}').map(|c| format!("U+{:04X}", c as u32)).collect();
                b.append(&kv("code point", &cps.join(" ")));
            }
            _ => {}
        }
        b.upcast()
    }

    // ── keys ─────────────────────────────────────────────────────────────

    fn key(self: &Rc<Self>, k: gdk::Key, state: gdk::ModifierType) -> bool {
        let (ctrl, shift, alt) = mods(state);
        let empty = self.entry.text().is_empty();
        let mode = self.mode();
        let grid = mode == Mode::Emoji;
        match k {
            gdk::Key::Escape => {
                if !empty {
                    self.entry.set_text("");
                } else {
                    self.hide();
                }
                true
            }
            gdk::Key::Down if grid => {
                self.step(GRID_COLS as i32);
                true
            }
            gdk::Key::Up if grid => {
                self.step(-(GRID_COLS as i32));
                true
            }
            gdk::Key::Right if grid && !ctrl => {
                self.step(1);
                true
            }
            gdk::Key::Left if grid && !ctrl => {
                self.step(-1);
                true
            }
            gdk::Key::Down => {
                self.step(1);
                true
            }
            gdk::Key::Up => {
                self.step(-1);
                true
            }
            gdk::Key::j | gdk::Key::n if ctrl => {
                self.step(1);
                true
            }
            gdk::Key::k if ctrl => {
                self.step(-1);
                true
            }
            gdk::Key::Page_Down => {
                self.step(if grid { GRID_COLS as i32 * 5 } else { 8 });
                true
            }
            gdk::Key::Page_Up => {
                self.step(if grid { -(GRID_COLS as i32) * 5 } else { -8 });
                true
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                let i = Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0) as i32;
                let d = if k == gdk::Key::ISO_Left_Tab || shift { -1 } else { 1 };
                let next = Mode::ALL[(i + d).rem_euclid(Mode::ALL.len() as i32) as usize];
                self.set_mode(next, "");
                true
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                self.activate(ctrl);
                true
            }
            gdk::Key::BackSpace if empty && mode != Mode::Apps && !ctrl => {
                self.set_mode(Mode::Apps, "");
                true
            }
            gdk::Key::Delete | gdk::Key::KP_Delete if shift && ctrl => {
                self.clear_all();
                true
            }
            gdk::Key::Delete | gdk::Key::KP_Delete if shift => {
                self.delete_selected();
                true
            }
            gdk::Key::p if ctrl && mode == Mode::Clipboard => {
                if let Some(Action::Clip(id)) = self.selected().map(|i| i.action) {
                    clip::toggle_pin(id);
                    self.st.borrow_mut().clips = clip::load().items;
                    self.requery();
                }
                true
            }
            gdk::Key::d if ctrl && mode == Mode::Notes => {
                let on = !self.st.borrow().notes.dnd;
                notes::set_dnd(on);
                self.st.borrow_mut().notes.dnd = on;
                self.dnd.set_visible(on);
                true
            }
            _ if alt && mode == Mode::Notes => {
                let Some(d) = k.to_unicode().and_then(|c| c.to_digit(10)).filter(|d| (1..=9).contains(d)) else {
                    return false;
                };
                if let Some(Action::Note(id)) = self.selected().map(|i| i.action) {
                    let n = self.st.borrow().notes.notes.iter().find(|n| n.id == id).cloned();
                    if let Some(n) = n.filter(|n| n.live)
                        && let Some((key, _)) = n.actions.iter().filter(|a| a.0 != "default").nth(d as usize - 1)
                    {
                        notes::invoke(id, key);
                        self.hide();
                    }
                }
                true
            }
            _ if ctrl && !alt => {
                let d = k.to_unicode().and_then(|c| c.to_digit(10)).filter(|d| (1..=Mode::ALL.len() as u32).contains(d));
                match d {
                    Some(d) => {
                        self.set_mode(Mode::ALL[d as usize - 1], "");
                        true
                    }
                    None => false,
                }
            }
            _ if empty && !ctrl && !alt => {
                let Some(m) = k.to_unicode().and_then(Mode::from_prefix) else { return false };
                if m == mode {
                    return false;
                }
                self.set_mode(m, "");
                true
            }
            _ => false,
        }
    }

    fn activate(self: &Rc<Self>, alt: bool) {
        let Some(it) = self.selected() else { return };
        match it.action {
            Action::Launch { id, action } => {
                self.hide();
                apps::launch(&id, action.as_deref());
            }
            Action::Clip(id) => {
                let c = self.st.borrow().clips.iter().find(|c| c.id == id).cloned();
                self.hide();
                if let Some((data, mime)) = c.and_then(|c| c.payload()) {
                    util::wl_copy(data, Some(&mime));
                    if alt {
                        util::paste_later();
                    }
                }
            }
            Action::Note(id) => {
                self.hide();
                notes::activate(id);
            }
            Action::Emoji(e) => {
                self.hide();
                emoji::picked(&e);
                util::wl_copy(e.into_bytes(), Some("text/plain;charset=utf-8"));
                if alt {
                    util::paste_later();
                }
            }
            Action::Open(u) => {
                self.hide();
                web::open(&u);
            }
            Action::Search(q) => {
                self.hide();
                web::search(&q);
            }
            Action::Copy(s) => {
                self.hide();
                util::wl_copy(s.into_bytes(), Some("text/plain;charset=utf-8"));
                if alt {
                    util::paste_later();
                }
            }
            Action::Convert { src, image, to } => {
                self.hide();
                convert::start(src, image, to);
            }
            Action::Focus(addr) => {
                self.hide();
                let _ = util::hypr(&format!("dispatch focuswindow address:{addr}"));
            }
            Action::Run(cmd) => {
                self.hide();
                util::launch(cmd);
            }
            Action::Nothing => {}
        }
    }

    fn delete_selected(self: &Rc<Self>) {
        match self.selected().map(|i| i.action) {
            Some(Action::Clip(id)) => {
                clip::delete(id);
                self.st.borrow_mut().clips = clip::load().items;
                self.requery_after_delete();
            }
            Some(Action::Note(id)) => {
                notes::dismiss(id);
                // drop it here now; the server's Changed signal brings the truth
                self.st.borrow_mut().notes.notes.retain(|n| n.id != id);
                self.requery_after_delete();
            }
            _ => {}
        }
    }

    /// After a delete the next row takes the selection (not the first one).
    fn requery_after_delete(self: &Rc<Self>) {
        let pos = self.st.borrow().sel;
        self.st.borrow_mut().sel = None;
        self.requery();
        let n = self.st.borrow().rows.len() as u32;
        if let Some(p) = pos
            && n > 0
        {
            let mut p = p.min(n - 1);
            let rows = self.st.borrow().rows.clone();
            while p > 0 && rows[p as usize].is_none() {
                p -= 1;
            }
            if rows[p as usize].is_none() {
                p = rows.iter().position(|r| r.is_some()).unwrap_or(0) as u32;
            }
            self.st.borrow_mut().sel = Some(p);
            self.show_selection();
            self.selection_changed();
        }
    }

    fn clear_all(self: &Rc<Self>) {
        let mode = self.mode();
        if !matches!(mode, Mode::Clipboard | Mode::Notes) {
            return;
        }
        if !self.st.borrow().confirm_clear {
            self.st.borrow_mut().confirm_clear = true;
            self.keys.set_text(if mode == Mode::Clipboard {
                "press ^⇧del again to clear everything not pinned"
            } else {
                "press ^⇧del again to clear the history"
            });
            self.keys.add_css_class("warn");
            let w = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
                if let Some(ui) = w.upgrade() {
                    ui.st.borrow_mut().confirm_clear = false;
                    ui.keys.remove_css_class("warn");
                    ui.keys.set_markup(&ui.hints());
                }
            });
            return;
        }
        self.st.borrow_mut().confirm_clear = false;
        self.keys.remove_css_class("warn");
        self.keys.set_markup(&self.hints());
        if mode == Mode::Clipboard {
            clip::clear_unpinned();
            self.st.borrow_mut().clips = clip::load().items;
        } else {
            notes::clear_history();
            self.st.borrow_mut().notes.notes.retain(|n| n.live);
        }
        self.requery();
    }
}

fn picture(path: &Path, w: i32, h: i32) -> gtk::Widget {
    let pic = gtk::Picture::new();
    pic.set_can_shrink(true);
    pic.set_content_fit(gtk::ContentFit::ScaleDown);
    pic.set_halign(gtk::Align::Start);
    pic.set_size_request(-1, 40);
    let wk = pic.downgrade();
    thumb(path, w.max(h) * 2, move |t| {
        if let (Some(p), Some(t)) = (wk.upgrade(), t) {
            let (tw, th) = (t.width() as f64 / 2.0, t.height() as f64 / 2.0);
            let s = (w as f64 / tw).min(h as f64 / th).min(1.0);
            p.set_size_request((tw * s) as i32, (th * s) as i32);
            p.set_paintable(Some(&t));
        }
    });
    pic.upcast()
}

/// A path with the home folder as `~`.
fn tilde(p: &Path) -> String {
    match p.strip_prefix(util::home()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn human(n: u64) -> String {
    let f = n as f64;
    match n {
        0..1024 => format!("{n} b"),
        1024..1_048_576 => format!("{:.1} kb", f / 1024.0),
        _ => format!("{:.1} mb", f / 1_048_576.0),
    }
}

const VIDEO: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "wmv", "flv", "mpg", "mpeg", "ts"];
const IMAGE: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "svg", "avif", "tiff", "tif", "heic", "ico"];

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn preview_clip(b: &gtk::Box, c: &Clip, kv: &dyn Fn(&str, &str) -> gtk::Label) {
    let when = glib::DateTime::from_unix_local(c.time as i64)
        .ok()
        .and_then(|d| d.format("%a %d %b %H:%M").ok())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    let pin = if c.pinned { " · pinned" } else { "" };
    match c.kind {
        Kind::Text => {
            let lines = c.text.lines().count().max(1);
            b.append(&kv("text", &format!("{} chars · {lines} line{} · {when}{pin}", c.size, if lines == 1 { "" } else { "s" })));
            let tv = gtk::TextView::new();
            tv.set_editable(false);
            tv.set_cursor_visible(false);
            tv.set_monospace(true);
            tv.set_wrap_mode(gtk::WrapMode::WordChar);
            tv.add_css_class("ptext");
            tv.set_can_focus(false);
            let shown: String = c.text.chars().take(20_000).collect();
            tv.buffer().set_text(&shown);
            b.append(&tv);
            if !c.blob.is_empty() {
                b.append(&kv("…", &format!("{} in all", human(c.size))));
            }
        }
        Kind::Image => {
            b.append(&kv("image", &format!("{}×{} · {} · {} · {when}{pin}", c.w, c.h, c.mime.trim_start_matches("image/"), human(c.size))));
            if let Some(p) = c.blob_path() {
                b.append(&picture(&p, 404, 380));
            }
        }
        Kind::Files => {
            let files = c.files();
            b.append(&kv("files", &format!("{} · {when}{pin}", if files.len() == 1 { "1 file".to_string() } else { format!("{} files", files.len()) })));
            for f in files.iter().take(12) {
                let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
                let name = gtk::Label::new(None);
                let exists = f.exists();
                let size = std::fs::metadata(f).map(|m| if m.is_dir() { "folder".into() } else { human(m.len()) }).unwrap_or_else(|_| "gone".into());
                name.set_markup(&format!("<b>{}</b>  <span alpha=\"50%\">{}</span>", util::escape(&f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), util::escape(&size)));
                name.set_xalign(0.0);
                name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                row.append(&name);
                let dir = gtk::Label::new(Some(&f.parent().map(tilde).unwrap_or_default()));
                dir.add_css_class("pkv");
                dir.set_xalign(0.0);
                dir.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                row.append(&dir);
                let e = ext_of(f);
                if exists && IMAGE.contains(&e.as_str()) {
                    row.append(&picture(f, 404, if files.len() == 1 { 340 } else { 160 }));
                } else if exists && VIDEO.contains(&e.as_str()) {
                    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    row.append(&holder);
                    let big = files.len() == 1;
                    match clip::thumb(f) {
                        Some(t) => holder.append(&picture(&t, 404, if big { 340 } else { 160 })),
                        None => {
                            let w = holder.downgrade();
                            clip::make_thumb(f, move |t| {
                                if let (Some(h), Some(t)) = (w.upgrade(), t) {
                                    h.append(&picture(&t, 404, if big { 340 } else { 160 }));
                                }
                            });
                        }
                    }
                }
                b.append(&row);
            }
            if files.len() > 12 {
                b.append(&kv("…", &format!("{} more", files.len() - 12)));
            }
        }
        Kind::Data => {
            b.append(&kv("data", &format!("{} · {} · {when}{pin}", c.mime, human(c.size))));
        }
    }
}

fn clip_items(clips: &[Clip], q: &str) -> Vec<Item> {
    let mut fz = util::Fuzzy::new(q);
    let mut out: Vec<(u32, bool, Item)> = Vec::new();
    for c in clips {
        let (title, icon, sub) = match c.kind {
            Kind::Text => (util::one_line(&c.text, 200), Icon::Glyph("\u{f0219}"), String::new()),
            Kind::Image => (
                format!("image  {}×{}", c.w, c.h),
                c.blob_path().map(Icon::File).unwrap_or(Icon::Glyph("\u{f02e9}")),
                String::new(),
            ),
            Kind::Files => {
                let f = c.files();
                let names: Vec<String> = f.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).collect();
                let first = f.first().cloned();
                let icon = match &first {
                    Some(p) if f.len() == 1 && IMAGE.contains(&ext_of(p).as_str()) && p.exists() => Icon::File(p.clone()),
                    Some(p) if f.len() == 1 && VIDEO.contains(&ext_of(p).as_str()) => clip::thumb(p).map(Icon::File).unwrap_or(Icon::Glyph("\u{f0567}")),
                    _ => Icon::Glyph(if f.len() == 1 { "\u{f0214}" } else { "\u{f0222}" }),
                };
                let _ = first;
                (names.join(", "), icon, String::new())
            }
            Kind::Data => (format!("data  {}", c.mime), Icon::Glyph("\u{f0219}"), String::new()),
        };
        let (score, hl) = if fz.empty {
            (0, vec![])
        } else {
            match (fz.indices(&title), fz.score(&c.haystack())) {
                (Some((s, hl)), _) => (s + 10, hl),
                (None, Some(s)) => (s, vec![]),
                _ => continue,
            }
        };
        let mut it = Item::new(title, icon, Action::Clip(c.id)).sub(sub).meta(util::age(c.time)).hl(hl);
        if c.pinned {
            it = it.class("pinned");
        }
        out.push((score, c.pinned, it));
    }
    if !fz.empty {
        out.sort_by_key(|x| std::cmp::Reverse(x.0));
    }
    out.sort_by_key(|x| !x.1);
    let any_pinned = out.first().is_some_and(|x| x.1);
    let mut items = Vec::new();
    let mut last = None;
    for (_, pinned, mut it) in out {
        if any_pinned && last != Some(pinned) {
            it.section = Some(if pinned { "pinned" } else { "recent" }.into());
            last = Some(pinned);
        }
        items.push(it);
    }
    items
}

fn calc_items(q: &str) -> Vec<Item> {
    let q = q.trim();
    if q.is_empty() {
        return vec![];
    }
    match calc::eval(q) {
        Some(v) => {
            let r = calc::format(v);
            let mut items = vec![Item::new(format!("= {r}"), Icon::Glyph("\u{f00ec}"), Action::Copy(r.clone())).sub(q.to_string()).meta("⏎ copy")];
            if v == v.trunc() && v.abs() < 1e15 && v.abs() >= 10.0 {
                let i = v as i64;
                items.push(Item::new(format!("{i:#x}"), Icon::Glyph("\u{f00ec}"), Action::Copy(format!("{i:#x}"))).sub("hex"));
                items.push(Item::new(format!("{i:#b}"), Icon::Glyph("\u{f00ec}"), Action::Copy(format!("{i:#b}"))).sub("binary"));
            }
            items
        }
        None => vec![Item::new("…", Icon::Glyph("\u{f00ec}"), Action::Nothing).sub("not a sum yet")],
    }
}
