//! `vector-space dmenu`: pick one line from a list, or type a line, in Vector's
//! look. The client (main.rs) writes the request to a file and waits on a Unix
//! socket; the daemon shows this window and writes the answer there:
//! "1\n<chosen text>" when picked, "0\n" when cancelled. Replaces
//! `walker --dmenu` for Omarchy's menus.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::util::{self, Fuzzy};

const WIDTH: i32 = 440;
const ROW_H: i32 = 38;
const MAX_ROWS: usize = 14;

#[derive(serde::Deserialize, Default)]
pub struct Request {
    pub prompt: String,
    pub items: Vec<String>,
    /// 1-based row to start on
    pub index: Option<usize>,
    pub input_only: bool,
    pub placeholder: Option<String>,
    pub reply: PathBuf,
}

struct Pick {
    win: gtk::ApplicationWindow,
    mode: gtk::Label,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroll: gtk::ScrolledWindow,
    req: RefCell<Request>,
    shown: RefCell<Vec<usize>>,
    done: Cell<bool>,
}

thread_local! {
    static PICK: RefCell<Option<Rc<Pick>>> = const { RefCell::new(None) };
}

/// `pick <request.json>` from the client.
pub fn open(app: &gtk::Application, path: &str) {
    let Some(req) = util::read_json::<Request>(std::path::Path::new(path)) else { return };
    let _ = std::fs::remove_file(path);
    let p = PICK.with(|p| p.borrow().clone()).unwrap_or_else(|| {
        let p = build(app);
        PICK.with(|x| *x.borrow_mut() = Some(p.clone()));
        p
    });
    // a pick still open is cancelled by the new one
    if p.win.is_visible() {
        p.finish(None);
    }
    p.show(req);
}

fn build(app: &gtk::Application) -> Rc<Pick> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("vspace");
    win.set_title(Some("Vector Pick"));
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some("vector-pick"));
    win.set_keyboard_mode(if std::env::var_os("VECTOR_OUTPUT").is_some() {
        KeyboardMode::None
    } else {
        KeyboardMode::Exclusive
    });
    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        win.set_anchor(e, true);
    }
    win.set_exclusive_zone(-1);

    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.add_css_class("backdrop");
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.add_css_class("pick");
    card.set_size_request(WIDTH, -1);
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Start);
    card.set_margin_top(180);
    backdrop.append(&card);
    win.set_child(Some(&backdrop));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    head.add_css_class("head");
    let mode = gtk::Label::new(None);
    mode.add_css_class("mode");
    mode.set_valign(gtk::Align::Fill);
    let entry = gtk::Entry::new();
    entry.set_hexpand(true);
    head.append(&mode);
    head.append(&crate::ui::slant());
    head.append(&entry);
    card.append(&head);

    let list = gtk::ListBox::new();
    list.add_css_class("pick-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_child(Some(&list));
    card.append(&scroll);

    let p = Rc::new(Pick {
        win,
        mode,
        entry,
        list,
        scroll,
        req: RefCell::default(),
        shown: RefCell::default(),
        done: Cell::new(true),
    });

    let w = Rc::downgrade(&p);
    p.entry.connect_changed(move |_| {
        if let Some(p) = w.upgrade() {
            p.filter();
        }
    });
    let w = Rc::downgrade(&p);
    p.entry.connect_activate(move |_| {
        if let Some(p) = w.upgrade() {
            p.accept();
        }
    });
    let w = Rc::downgrade(&p);
    p.list.connect_row_activated(move |_, row| {
        if let Some(p) = w.upgrade() {
            p.list.select_row(Some(row));
            p.accept();
        }
    });

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = Rc::downgrade(&p);
    keys.connect_key_pressed(move |_, k, _, m| {
        let Some(p) = w.upgrade() else { return glib::Propagation::Proceed };
        let ctrl = m.contains(gdk::ModifierType::CONTROL_MASK);
        match k {
            gdk::Key::Escape => p.finish(None),
            gdk::Key::Down | gdk::Key::Tab => p.step(1),
            gdk::Key::Up | gdk::Key::ISO_Left_Tab => p.step(-1),
            gdk::Key::n | gdk::Key::j if ctrl => p.step(1),
            gdk::Key::p | gdk::Key::k if ctrl => p.step(-1),
            gdk::Key::Return | gdk::Key::KP_Enter => p.accept(),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    p.win.add_controller(keys);

    let click = gtk::GestureClick::new();
    let w = Rc::downgrade(&p);
    let c2 = card.clone();
    click.connect_pressed(move |g, _, x, y| {
        let (Some(p), Some(bd)) = (w.upgrade(), g.widget()) else { return };
        if let Some(b) = c2.compute_bounds(&bd)
            && !b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
        {
            p.finish(None);
        }
    });
    backdrop.add_controller(click);
    p
}

impl Pick {
    fn show(self: &Rc<Self>, req: Request) {
        crate::start::hide();
        if let Some(ui) = crate::ui::get() {
            ui.hide();
        }
        let prompt = req.prompt.trim().trim_end_matches('…').trim_end_matches("...").trim().to_string();
        self.mode.set_text(&if prompt.is_empty() { "PICK".to_string() } else { prompt.to_uppercase() });
        self.entry.set_placeholder_text(req.placeholder.as_deref().or(if req.input_only {
            Some("type and press enter")
        } else {
            Some("filter")
        }));
        let input_only = req.input_only;
        let index = req.index;
        *self.req.borrow_mut() = req;
        self.done.set(false);
        self.entry.set_text("");
        self.filter();
        self.scroll.set_visible(!input_only);
        if let Some(i) = index.and_then(|i| i.checked_sub(1))
            && let Some(row) = self.list.row_at_index(i as i32)
        {
            self.list.select_row(Some(&row));
        }
        place(&self.win);
        self.win.present();
        self.entry.grab_focus();
        // tests pick a row without input: VECTOR_SPACE_PICK_AUTO=N (1-based), with an
        // optional filter typed first: VECTOR_SPACE_PICK_TYPE=text
        if let Ok(n) = std::env::var("VECTOR_SPACE_PICK_AUTO").map(|v| v.parse::<i32>().unwrap_or(1)) {
            if let Ok(t) = std::env::var("VECTOR_SPACE_PICK_TYPE") {
                self.entry.set_text(&t);
            }
            let w = Rc::downgrade(self);
            glib::timeout_add_local_once(std::time::Duration::from_millis(1200), move || {
                if let Some(p) = w.upgrade() {
                    if let Some(r) = p.list.row_at_index(n - 1) {
                        p.list.select_row(Some(&r));
                    }
                    p.accept();
                }
            });
        }
    }

    /// Rebuild the rows for the current filter, best match first.
    fn filter(&self) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let q = self.entry.text().to_string();
        let req = self.req.borrow();
        let mut shown: Vec<(u32, usize, Vec<u32>)> = Vec::new();
        let mut fz = Fuzzy::new(&q);
        for (i, item) in req.items.iter().enumerate() {
            if fz.empty {
                shown.push((0, i, vec![]));
            } else if let Some((s, hl)) = fz.indices(item) {
                shown.push((s, i, hl));
            }
        }
        if !fz.empty {
            shown.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        for (_, i, hl) in &shown {
            let l = gtk::Label::new(None);
            l.set_markup(&util::highlight(&req.items[*i], hl));
            l.set_xalign(0.0);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let row = gtk::ListBoxRow::new();
            row.add_css_class("pick-row");
            row.set_child(Some(&l));
            self.list.append(&row);
        }
        let n = shown.len().min(MAX_ROWS) as i32;
        self.scroll.set_min_content_height(n * ROW_H);
        self.scroll.set_max_content_height(MAX_ROWS as i32 * ROW_H);
        self.scroll.set_visible(!req.input_only && !shown.is_empty());
        if let Some(r) = self.list.row_at_index(0) {
            self.list.select_row(Some(&r));
        }
        *self.shown.borrow_mut() = shown.into_iter().map(|(_, i, _)| i).collect();
    }

    fn step(&self, by: i32) {
        let n = self.shown.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.list.selected_row().map(|r| r.index()).unwrap_or(-1);
        let next = (cur + by).rem_euclid(n);
        if let Some(r) = self.list.row_at_index(next) {
            self.list.select_row(Some(&r));
            r.grab_focus();
            self.entry.grab_focus_without_selecting();
        }
    }

    fn accept(&self) {
        let req = self.req.borrow();
        let text = if req.input_only {
            Some(self.entry.text().to_string())
        } else {
            self.list
                .selected_row()
                .and_then(|r| self.shown.borrow().get(r.index() as usize).copied())
                .map(|i| req.items[i].clone())
                // nothing matches: hand back what was typed, as dmenu does
                .or_else(|| Some(self.entry.text().to_string()).filter(|t| !t.is_empty()))
        };
        drop(req);
        self.finish(text);
    }

    fn finish(&self, text: Option<String>) {
        if self.done.replace(true) {
            return;
        }
        self.win.set_visible(false);
        let reply = self.req.borrow().reply.clone();
        if let Ok(mut s) = UnixStream::connect(&reply) {
            let _ = match text {
                Some(t) => write!(s, "1\n{t}"),
                None => write!(s, "0\n"),
            };
        }
    }
}

fn place(win: &gtk::ApplicationWindow) {
    let name = std::env::var("VECTOR_OUTPUT").ok().filter(|o| !o.is_empty()).or_else(|| {
        util::hypr_json("monitors").and_then(|v| {
            v.as_array()?
                .iter()
                .find(|m| m["focused"].as_bool() == Some(true))
                .and_then(|m| m["name"].as_str().map(str::to_string))
        })
    });
    let Some(d) = gdk::Display::default() else { return };
    let ms = d.monitors();
    if let Some(m) = (0..ms.n_items())
        .filter_map(|i| ms.item(i).and_downcast::<gdk::Monitor>())
        .find(|m| name.is_some() && m.connector().map(|c| c.to_string()) == name)
    {
        win.set_monitor(Some(&m));
    }
}
