//! The viewer window: a header with the file name and facts, the picture in the middle, a
//! statusline with the mode, buttons and (for videos) the timeline. Same look as vector.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use vector_core::keymap::Key;
use vector_core::theme::Theme;

use crate::canvas::Canvas;
use crate::pdf::PdfView;
use crate::playlist::{self, Item, Kind, Order};
use crate::video::Video;

/// Material Design icons from the Nerd Font.
mod icon {
    pub const MENU: &str = "\u{F035C}";
    pub const GRID: &str = "\u{F11D9}";
    pub const ORDER: &str = "\u{F04BA}";
    pub const PREV: &str = "\u{F004D}";
    pub const NEXT: &str = "\u{F0054}";
    pub const PLAY: &str = "\u{F040A}";
    pub const PAUSE: &str = "\u{F03E4}";
    pub const BACK: &str = "\u{F11F9}";
    pub const FORWARD: &str = "\u{F11F8}";
    pub const COPY: &str = "\u{F018F}";
    pub const CAMERA: &str = "\u{F0D5D}";
    pub const OPEN: &str = "\u{F03CC}";
    pub const FIT: &str = "\u{F18F5}";
    pub const INFO: &str = "\u{F02FD}";
    pub const TRASH: &str = "\u{F0A7A}";
    pub const FULL: &str = "\u{F0293}";
    pub const UNFULL: &str = "\u{F0294}";
    pub const IMAGE: &str = "\u{F0976}";
    pub const VIDEO: &str = "\u{F0230}";
    pub const PDF: &str = "\u{F0226}";
    pub const WORD: &str = "\u{F022C}";
    pub const SHEET: &str = "\u{F021B}";
    pub const SLIDES: &str = "\u{F0227}";
}

/// Gallery thumbnails kept in memory (about 256 KB each).
const THUMB_CACHE: usize = 1500;

/// Our own desktop id: left out of "open with".
const SELF_ID: &str = "io.github.not_bad_company.VectorView.desktop";

fn kind_icon(item: &Item) -> &'static str {
    match item.kind {
        Kind::Image => icon::IMAGE,
        Kind::Video => icon::VIDEO,
        Kind::Pdf => icon::PDF,
        Kind::Office => match playlist::ext(&item.path).as_str() {
            "xls" | "xlsx" | "ods" | "ots" => icon::SHEET,
            "ppt" | "pptx" | "odp" | "otp" | "odg" => icon::SLIDES,
            _ => icon::WORD,
        },
    }
}

fn button(glyph: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::with_label(glyph);
    b.add_css_class("link");
    b.add_css_class("glyph");
    b.set_tooltip_text(Some(tip));
    b.set_focus_on_click(false);
    b.set_can_focus(false);
    b
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn order_file() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join("vector")
        .join("view-order")
}

fn content_type(p: &Path) -> String {
    gio::content_type_guess(Some(p), None).0.to_string()
}

/// The app that opens `p` by default, ourselves left out.
fn default_app(p: &Path) -> Option<gio::AppInfo> {
    let ct = content_type(p);
    gio::AppInfo::default_for_type(&ct, false)
        .filter(|a| a.id().as_deref() != Some(SELF_ID))
        .or_else(|| other_apps(p).into_iter().next())
}

fn other_apps(p: &Path) -> Vec<gio::AppInfo> {
    gio::AppInfo::all_for_type(&content_type(p))
        .into_iter()
        .filter(|a| a.id().as_deref() != Some(SELF_ID) && a.should_show())
        .collect()
}

pub struct Win {
    pub window: gtk::ApplicationWindow,
    weak: RefCell<Weak<Win>>,
    header: gtk::Box,
    kind: gtk::Label,
    name: gtk::Label,
    meta: gtk::Label,
    facts: gtk::Label,
    stack: gtk::Stack,
    canvas: Canvas,
    video: Rc<Video>,
    pdf: Rc<PdfView>,
    message: gtk::Label,
    grid: gtk::GridView,
    grid_model: gtk::StringList,
    grid_sel: gtk::SingleSelection,
    notice: gtk::Label,
    notice_timer: RefCell<Option<glib::SourceId>>,
    help: gtk::Box,
    info: gtk::Revealer,
    info_rows: gtk::Grid,
    statusline: gtk::CenterBox,
    mode: gtk::Label,
    media: Vec<gtk::Widget>,
    play: gtk::Button,
    time: gtk::Label,
    timeline: gtk::DrawingArea,
    open_btn: gtk::Button,
    grid_btn: gtk::Button,
    info_btn: gtk::Button,
    copy_btn: gtk::Button,
    fit_btn: gtk::Button,
    full_btn: gtk::Button,
    theme: Theme,

    items: RefCell<Vec<Item>>,
    index: Cell<usize>,
    order: Cell<Order>,
    zoom_lock: Cell<bool>,
    generation: Cell<u64>,
    /// While the first file or folder is still being read (tests wait for it).
    pub loading: Cell<bool>,
    delete_armed: Cell<Option<Instant>>,
    thumbs: RefCell<HashMap<PathBuf, gdk::Texture>>,
    /// The default app of the file on show (looked up once per file).
    open_app: RefCell<Option<gio::AppInfo>>,
    pointer: Cell<(f64, f64)>,
    drag_last: Cell<(f64, f64)>,
    seeking: Cell<bool>,
    timeline_hover: Cell<Option<f64>>,
}

impl Win {
    pub fn new(app: &gtk::Application, theme: Theme) -> Rc<Win> {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("vector");
        window.set_title(Some("Vector View"));
        window.set_default_size(1200, 800);
        window.set_decorated(false);

        // ── header ──
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("header");
        let kind = label(icon::IMAGE, &["title", "kind"]);
        let name = label("", &["name"]);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_xalign(0.0);
        let meta = label("", &["dim"]);
        meta.set_xalign(0.0);
        let facts = label("", &["dim"]);
        facts.set_hexpand(true);
        facts.set_xalign(1.0);
        facts.set_margin_end(8);
        header.append(&kind);
        header.append(&name);
        header.append(&meta);
        header.append(&facts);

        // ── stage ──
        let canvas = Canvas::default();
        let bg = theme.background.clone();
        let video = Video::new(&bg);
        let pdf = PdfView::new();
        let message = label("", &["empty"]);
        message.set_wrap(true);
        message.set_justify(gtk::Justification::Center);
        let grid_model = gtk::StringList::new(&[]);
        let grid_sel = gtk::SingleSelection::new(Some(grid_model.clone()));
        let grid = gtk::GridView::new(Some(grid_sel.clone()), None::<gtk::ListItemFactory>);
        grid.set_max_columns(24);
        grid.set_min_columns(2);
        grid.set_single_click_activate(true);
        let grid_scroll = gtk::ScrolledWindow::new();
        grid_scroll.set_child(Some(&grid));
        grid_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&canvas, Some("image"));
        stack.add_named(&video.area, Some("video"));
        stack.add_named(&pdf.widget, Some("pdf"));
        stack.add_named(&message, Some("message"));
        stack.add_named(&grid_scroll, Some("grid"));
        stack.set_visible_child_name("message");

        // ── info panel ──
        let info_rows = gtk::Grid::new();
        info_rows.set_column_spacing(14);
        info_rows.set_row_spacing(4);
        let info_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        info_box.add_css_class("info-panel");
        info_box.append(&info_rows);
        let info_scroll = gtk::ScrolledWindow::new();
        info_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        info_scroll.set_child(Some(&info_box));
        info_scroll.set_propagate_natural_width(true);
        let info = gtk::Revealer::new();
        info.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        info.set_transition_duration(120);
        info.set_child(Some(&info_scroll));

        let middle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        middle.append(&stack);
        middle.append(&info);

        // ── overlays: notice and key help ──
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&middle));
        let notice = label("", &["notice"]);
        notice.set_halign(gtk::Align::Center);
        notice.set_valign(gtk::Align::End);
        notice.set_margin_bottom(18);
        notice.set_visible(false);
        notice.set_can_target(false);
        overlay.add_overlay(&notice);
        let help = help_card();
        help.set_visible(false);
        overlay.add_overlay(&help);

        // ── timeline and statusline ──
        let timeline = gtk::DrawingArea::new();
        timeline.add_css_class("timeline");
        timeline.set_content_height(9);
        timeline.set_visible(false);

        let mode = label("NORMAL", &["mode"]);
        let start = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        start.append(&mode);
        let menu_btn = button(icon::MENU, "menu  ·  m, right click");
        let grid_btn = button(icon::GRID, "gallery  ·  g");
        let order_btn = button(icon::ORDER, "order  ·  o");
        start.append(&menu_btn);
        start.append(&grid_btn);
        start.append(&order_btn);

        let center = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let prev = button(icon::PREV, "previous  ·  ←");
        let back = button(icon::BACK, "back 5 s  ·  [");
        let play = button(icon::PAUSE, "pause  ·  space");
        let fwd = button(icon::FORWARD, "forward 5 s  ·  ]");
        let next = button(icon::NEXT, "next  ·  →");
        let time = label("", &["dim", "time"]);
        for w in [&prev, &back, &play, &fwd, &next] {
            center.append(w);
        }
        center.append(&time);

        let end = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let copy_btn = button(icon::COPY, "copy  ·  ctrl+c");
        let open_btn = button(icon::OPEN, "open in…  ·  e");
        let fit_btn = button(icon::FIT, "fit to window  ·  0");
        let info_btn = button(icon::INFO, "details  ·  i");
        let trash_btn = button(icon::TRASH, "move to trash  ·  del del");
        trash_btn.add_css_class("danger-hover");
        let full_btn = button(icon::FULL, "fullscreen  ·  f");
        for w in [
            &copy_btn, &open_btn, &fit_btn, &info_btn, &trash_btn, &full_btn,
        ] {
            end.append(w);
        }

        let statusline = gtk::CenterBox::new();
        statusline.add_css_class("statusline");
        statusline.add_css_class("bar");
        statusline.set_start_widget(Some(&start));
        statusline.set_center_widget(Some(&center));
        statusline.set_end_widget(Some(&end));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("root");
        root.append(&header);
        root.append(&overlay);
        root.append(&timeline);
        root.append(&statusline);
        window.set_child(Some(&root));

        let order = std::fs::read_to_string(order_file())
            .ok()
            .and_then(|s| Order::parse(&s))
            .unwrap_or(Order::Name);

        let win = Rc::new(Win {
            window,
            weak: RefCell::default(),
            header,
            kind,
            name,
            meta,
            facts,
            stack,
            canvas,
            video,
            pdf,
            message,
            grid,
            grid_model,
            grid_sel,
            notice,
            notice_timer: RefCell::default(),
            help,
            info,
            info_rows,
            statusline,
            mode,
            media: vec![
                back.clone().upcast(),
                play.clone().upcast(),
                fwd.clone().upcast(),
                time.clone().upcast(),
            ],
            play,
            time,
            timeline,
            open_btn,
            grid_btn,
            info_btn,
            copy_btn,
            fit_btn,
            full_btn,
            theme,
            items: RefCell::default(),
            index: Cell::new(0),
            order: Cell::new(order),
            zoom_lock: Cell::new(false),
            generation: Cell::new(0),
            loading: Cell::new(true),
            delete_armed: Cell::new(None),
            thumbs: RefCell::default(),
            open_app: RefCell::default(),
            pointer: Cell::new((0.0, 0.0)),
            drag_last: Cell::new((0.0, 0.0)),
            seeking: Cell::new(false),
            timeline_hover: Cell::new(None),
        });
        *win.weak.borrow_mut() = Rc::downgrade(&win);

        let on = |b: &gtk::Button, f: fn(&Win)| {
            let w = Rc::downgrade(&win);
            b.connect_clicked(move |_| {
                if let Some(w) = w.upgrade() {
                    f(&w);
                }
            });
        };
        on(&menu_btn, |w| w.popup_menu(None));
        on(&win.grid_btn, |w| w.toggle_grid());
        on(&order_btn, |w| w.cycle_order());
        on(&prev, |w| w.step(-1));
        on(&next, |w| w.step(1));
        on(&back, |w| w.video.seek(-5.0));
        on(&fwd, |w| w.video.seek(5.0));
        on(&win.play, |w| w.video.toggle_pause());
        on(&win.copy_btn, |w| w.copy());
        on(&win.open_btn, |w| w.open_default());
        on(&win.fit_btn, |w| w.fit());
        on(&win.info_btn, |w| w.toggle_info());
        on(&trash_btn, |w| w.trash(true));
        on(&win.full_btn, |w| w.toggle_fullscreen());

        win.connect_stage();
        win.connect_timeline();
        win.connect_grid();
        win.connect_actions();
        win.connect_keys();
        let w = Rc::downgrade(&win);
        *win.video.on_change.borrow_mut() = Some(Box::new(move |only_time| {
            if let Some(w) = w.upgrade() {
                if only_time {
                    w.update_time();
                    return;
                }
                if w.current().is_some_and(|c| c.kind == Kind::Video)
                    && let Some(e) = w.video.error.borrow().clone()
                {
                    w.show_message(&e);
                }
                w.update_chrome();
            }
        }));
        let w = Rc::downgrade(&win);
        *win.pdf.on_change.borrow_mut() = Some(Box::new(move || {
            if let Some(w) = w.upgrade() {
                if let Some(e) = w.pdf.error.borrow().clone() {
                    w.show_message(&e);
                }
                w.update_chrome();
            }
        }));
        let w = Rc::downgrade(&win);
        win.window.connect_fullscreened_notify(move |_| {
            if let Some(w) = w.upgrade() {
                w.update_chrome();
            }
        });
        let w = Rc::downgrade(&win);
        win.window.connect_close_request(move |_| {
            if let Some(w) = w.upgrade() {
                w.video.stop();
            }
            glib::Propagation::Proceed
        });
        win
    }

    fn me(&self) -> Option<Rc<Win>> {
        self.weak.borrow().upgrade()
    }

    fn current(&self) -> Option<Item> {
        self.items.borrow().get(self.index.get()).cloned()
    }

    // ── opening ─────────────────────────────────────────────────────────────

    /// Opens files and folders given on the command line. One file: its whole folder, starting
    /// at it. One folder: everything in it. Several: just those.
    pub fn open_paths(&self, paths: Vec<PathBuf>) {
        let mut paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .collect();
        if paths.is_empty() {
            paths.push(
                glib::user_special_dir(glib::UserDirectory::Pictures)
                    .unwrap_or_else(glib::home_dir),
            );
        }
        if paths.len() == 1 {
            let p = paths.remove(0);
            let (dir, start) = if p.is_dir() {
                (p, None)
            } else {
                (
                    p.parent().map(Path::to_path_buf).unwrap_or_default(),
                    Some(p),
                )
            };
            if let Some(item) = start.as_ref().and_then(|s| Item::new(s.clone())) {
                // show the file at once; the rest of the folder follows
                *self.items.borrow_mut() = vec![item];
                self.show(0);
            } else if let Some(s) = &start {
                self.show_message(&format!(
                    "can't show {}",
                    s.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
            let Some(me) = self.me() else { return };
            glib::spawn_future_local(async move {
                let scan_dir = dir.clone();
                let mut items = gio::spawn_blocking(move || playlist::scan(&scan_dir))
                    .await
                    .unwrap_or_default();
                me.sort_items(&mut items);
                let current = me.current();
                let at = current.as_ref().map(|c| c.path.clone());
                let pos = match (
                    &current,
                    at.as_deref().and_then(|p| playlist::position(&items, p)),
                ) {
                    (_, Some(i)) => i,
                    // a hidden file, or one gone meanwhile: keep it where it is, first
                    (Some(c), None) => {
                        items.insert(0, c.clone());
                        0
                    }
                    (None, None) => 0,
                };
                let empty = items.is_empty();
                *me.items.borrow_mut() = items;
                if empty {
                    me.show_message(&format!("nothing to show in {}", dir.display()));
                    me.loading.set(false);
                } else if at.is_some() {
                    // the file is already on show
                    me.index.set(pos);
                    me.update_chrome();
                } else {
                    me.show(pos);
                }
            });
        } else {
            let mut items: Vec<Item> = paths.into_iter().filter_map(Item::new).collect();
            self.sort_items(&mut items);
            *self.items.borrow_mut() = items;
            if self.items.borrow().is_empty() {
                self.show_message("nothing to show");
                self.loading.set(false);
            } else {
                self.show(0);
            }
        }
    }

    fn sort_items(&self, items: &mut [Item]) {
        let seed = glib::random_int() as u64 | ((glib::random_int() as u64) << 32);
        playlist::sort(items, self.order.get(), seed);
    }

    fn show_message(&self, text: &str) {
        self.message.set_text(text);
        self.reveal("message");
    }

    /// Switches the stage to `page`, unless the gallery is up (it stays until closed).
    fn reveal(&self, page: &str) {
        if self.stack.visible_child_name().as_deref() != Some("grid") {
            self.stack.set_visible_child_name(page);
        }
    }

    /// Shows item `i`.
    fn show(&self, i: usize) {
        let Some(item) = self.items.borrow().get(i).cloned() else {
            return;
        };
        self.index.set(i);
        let g = self.generation.get() + 1;
        self.generation.set(g);
        self.delete_armed.set(None);
        if item.kind != Kind::Video {
            self.video.stop();
        }
        if matches!(item.kind, Kind::Image | Kind::Video) {
            self.pdf.close();
        }
        if self.info.reveals_child() {
            self.load_info();
        }
        *self.open_app.borrow_mut() = default_app(&item.path);
        let Some(me) = self.me() else { return };
        match item.kind {
            Kind::Image => {
                glib::spawn_future_local(async move {
                    let r = crate::load::image(item.path.clone()).await;
                    if me.generation.get() != g {
                        return;
                    }
                    match r {
                        Ok(t) => {
                            me.canvas.set_texture(Some(t), me.zoom_lock.get());
                            me.reveal("image");
                        }
                        Err(e) => {
                            // nothing of the previous picture may stay around to be copied
                            me.canvas.set_texture(None, false);
                            me.show_message(&e);
                        }
                    }
                    me.loading.set(false);
                    me.update_chrome();
                });
            }
            Kind::Video => {
                if !self.zoom_lock.get() {
                    self.video.reset_view();
                }
                self.video.open(&item.path);
                self.reveal("video");
                self.loading.set(false);
            }
            Kind::Pdf => {
                self.pdf.open(&item.path);
                self.reveal("pdf");
                self.loading.set(false);
            }
            Kind::Office => {
                self.show_message("converting with LibreOffice…");
                glib::spawn_future_local(async move {
                    let wanted = {
                        let me = Rc::downgrade(&me);
                        move || me.upgrade().is_some_and(|m| m.generation.get() == g)
                    };
                    let r = crate::load::office(item.path.clone(), wanted).await;
                    if me.generation.get() != g {
                        return;
                    }
                    match r {
                        Ok(pdf) => {
                            me.pdf.open(&pdf);
                            me.reveal("pdf");
                        }
                        Err(e) => me.show_message(&e),
                    }
                    me.loading.set(false);
                    me.update_chrome();
                });
            }
        }
        self.update_chrome();
    }

    fn step(&self, n: isize) {
        let len = self.items.borrow().len();
        if len > 0 {
            self.show(playlist::step(self.index.get(), n, len));
        }
    }

    // ── chrome ──────────────────────────────────────────────────────────────

    fn update_chrome(&self) {
        let Some(item) = self.current() else {
            self.name.set_text("");
            self.meta.set_text("");
            self.facts.set_text("");
            for w in &self.media {
                w.set_visible(false);
            }
            self.timeline.set_visible(false);
            return;
        };
        let video = item.kind == Kind::Video;
        let paged = matches!(item.kind, Kind::Pdf | Kind::Office);
        self.kind.set_text(kind_icon(&item));
        self.name.set_text(&item.name());
        self.window
            .set_title(Some(&format!("{} — Vector View", item.name())));
        let count = self.items.borrow().len();
        self.meta.set_text(&format!(
            "  ·  {} / {}  ·  by {}",
            self.index.get() + 1,
            count,
            self.order.get().label()
        ));

        let mut facts: Vec<String> = Vec::new();
        let zoomed = match item.kind {
            Kind::Image => {
                if let Some(t) = self.canvas.texture() {
                    facts.push(format!("{}×{}", t.width(), t.height()));
                }
                !self.canvas.is_fitted()
            }
            Kind::Video => {
                let s = self.video.state.get();
                if s.width > 0.0 {
                    facts.push(format!("{}×{}", s.width, s.height));
                }
                self.video.is_zoomed()
            }
            Kind::Pdf | Kind::Office => {
                let (p, n) = self.pdf.position();
                if n > 0 {
                    facts.push(format!("page {p} / {n}"));
                }
                self.pdf.zoom.get() != 1.0
            }
        };
        facts.push(playlist::size(item.size));
        facts.push(playlist::ext(&item.path));
        match item.kind {
            Kind::Image if self.canvas.texture().is_some() => {
                facts.push(format!("{}%", self.canvas.percent()))
            }
            Kind::Video if zoomed => facts.push(format!(
                "{}%",
                (2f64.powf(self.video.state.get().zoom) * 100.0).round()
            )),
            Kind::Pdf | Kind::Office if self.pdf.position().1 > 0 => {
                facts.push(format!("{}%", self.pdf.percent()))
            }
            _ => {}
        }
        self.facts.set_text(&facts.join("  ·  "));

        let lock = self.zoom_lock.get();
        self.mode.set_text(if lock { "ZOOM" } else { "NORMAL" });
        if lock {
            self.mode.add_css_class("active");
        } else {
            self.mode.remove_css_class("active");
        }

        for w in &self.media {
            w.set_visible(video);
        }
        let bars = self.header.is_visible();
        self.timeline.set_visible(video && bars);
        if video {
            let s = self.video.state.get();
            self.play
                .set_label(if s.paused { icon::PLAY } else { icon::PAUSE });
            self.play.set_tooltip_text(Some(if s.paused {
                "play  ·  space"
            } else {
                "pause  ·  space"
            }));
            self.update_time();
        }
        self.copy_btn
            .set_label(if video { icon::CAMERA } else { icon::COPY });
        self.copy_btn.set_tooltip_text(Some(if video {
            "save + copy frame  ·  s"
        } else if paged {
            "copy page  ·  ctrl+c"
        } else {
            "copy image  ·  ctrl+c"
        }));
        let app = self.open_app.borrow().clone();
        self.open_btn.set_sensitive(app.is_some());
        self.open_btn.set_tooltip_text(Some(&match &app {
            Some(a) => format!("open in {}  ·  e", a.display_name()),
            None => "no other app for this file".into(),
        }));
        let grid = self.stack.visible_child_name().as_deref() == Some("grid");
        for (b, on) in [
            (&self.fit_btn, zoomed),
            (&self.grid_btn, grid),
            (&self.info_btn, self.info.reveals_child()),
        ] {
            if on {
                b.add_css_class("primary");
            } else {
                b.remove_css_class("primary");
            }
        }
        let full = self.window.is_fullscreen();
        self.full_btn
            .set_label(if full { icon::UNFULL } else { icon::FULL });
        if full {
            self.full_btn.add_css_class("primary");
        } else {
            self.full_btn.remove_css_class("primary");
        }
        self.update_cursor();
    }

    /// The clock and the timeline: every frame while a video plays, so nothing else here.
    fn update_time(&self) {
        let s = self.video.state.get();
        let text = format!(
            "{} / {}",
            playlist::time(s.time),
            playlist::time(s.duration)
        );
        if self.time.text() != text {
            self.time.set_text(&text);
        }
        self.timeline.queue_draw();
    }

    fn update_cursor(&self) {
        let name = if self.drag_last.get() != (0.0, 0.0) && self.canvas.can_pan() {
            "grabbing"
        } else if self.canvas.can_pan() {
            "grab"
        } else {
            "default"
        };
        self.canvas.set_cursor_from_name(Some(name));
    }

    fn notify(&self, text: &str) {
        self.notice.remove_css_class("crit");
        self.notice.set_text(text);
        self.notice.set_visible(true);
        if let Some(id) = self.notice_timer.borrow_mut().take() {
            id.remove();
        }
        let n = self.notice.downgrade();
        let me = self.weak.borrow().clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(2200), move || {
            if let Some(n) = n.upgrade() {
                n.set_visible(false);
            }
            if let Some(me) = me.upgrade() {
                me.notice_timer.borrow_mut().take();
            }
        });
        *self.notice_timer.borrow_mut() = Some(id);
    }

    fn notify_error(&self, text: &str) {
        self.notify(text);
        self.notice.add_css_class("crit");
    }

    fn toggle_bars(&self) {
        let show = !self.header.is_visible();
        self.header.set_visible(show);
        self.statusline.set_visible(show);
        self.update_chrome();
    }

    fn toggle_fullscreen(&self) {
        if self.window.is_fullscreen() {
            self.window.unfullscreen();
        } else {
            self.window.fullscreen();
        }
    }

    fn toggle_help(&self) {
        self.help.set_visible(!self.help.is_visible());
    }

    // ── zoom and pan ────────────────────────────────────────────────────────

    fn kind(&self) -> Option<Kind> {
        if self.stack.visible_child_name().as_deref() == Some("grid") {
            return None;
        }
        self.current().map(|i| i.kind)
    }

    fn zoom(&self, steps: f64) {
        match self.kind() {
            Some(Kind::Image) => self.canvas.zoom(1.25f64.powf(steps)),
            Some(Kind::Video) => self.video.add_zoom(steps * 0.25),
            Some(Kind::Pdf | Kind::Office) => {
                self.pdf.set_zoom(self.pdf.zoom.get() * 1.2f64.powf(steps))
            }
            None => {}
        }
        self.update_chrome();
    }

    fn fit(&self) {
        match self.kind() {
            Some(Kind::Image) => self.canvas.fit(),
            Some(Kind::Video) => self.video.reset_view(),
            Some(Kind::Pdf | Kind::Office) => self.pdf.set_zoom(1.0),
            None => {}
        }
        self.update_chrome();
    }

    /// Pans by a fraction of the view (`dx`, `dy` in -1..1).
    fn pan(&self, dx: f64, dy: f64) {
        match self.kind() {
            Some(Kind::Image) => self.canvas.pan(
                -dx * self.canvas.width() as f64,
                -dy * self.canvas.height() as f64,
            ),
            Some(Kind::Video) => self.video.pan(-dx, -dy),
            Some(Kind::Pdf | Kind::Office) => {
                if dy != 0.0 {
                    self.pdf.scroll_px(dy * 600.0);
                } else {
                    let a = self.pdf.widget.hadjustment();
                    a.set_value(a.value() + dx * 600.0);
                }
            }
            None => {}
        }
        self.update_chrome();
    }

    fn toggle_zoom_lock(&self) {
        let lock = !self.zoom_lock.get();
        self.zoom_lock.set(lock);
        self.notify(if lock {
            "-- ZOOM --  zoom stays when you change file"
        } else {
            "-- NORMAL --  each file opens fitted"
        });
        if !lock {
            self.fit();
        }
        self.update_chrome();
    }

    // ── order and grid ──────────────────────────────────────────────────────

    fn set_order(&self, order: Order) {
        self.order.set(order);
        gio::spawn_blocking(move || {
            let f = order_file();
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(f, order.label());
        });
        let at = self.current().map(|c| c.path);
        let mut items = std::mem::take(&mut *self.items.borrow_mut());
        self.sort_items(&mut items);
        if let Some(p) = at
            && let Some(i) = playlist::position(&items, &p)
        {
            self.index.set(i);
        }
        *self.items.borrow_mut() = items;
        if self.stack.visible_child_name().as_deref() == Some("grid") {
            self.fill_grid();
        }
        self.notify(&format!("order: {}", order.label()));
        self.update_chrome();
    }

    fn cycle_order(&self) {
        self.set_order(self.order.get().next());
    }

    fn fill_grid(&self) {
        let paths: Vec<String> = self
            .items
            .borrow()
            .iter()
            .map(|i| i.path.to_string_lossy().into_owned())
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        self.grid_model.splice(0, self.grid_model.n_items(), &refs);
        let i = self.index.get() as u32;
        self.grid_sel.set_selected(i);
        self.grid.scroll_to(
            i,
            gtk::ListScrollFlags::FOCUS | gtk::ListScrollFlags::SELECT,
            None,
        );
    }

    fn toggle_grid(&self) {
        if self.stack.visible_child_name().as_deref() == Some("grid") {
            self.close_grid(None);
        } else {
            if self.current().is_some_and(|c| c.kind == Kind::Video) {
                self.video.set_pause(true);
            }
            self.fill_grid();
            self.stack.set_visible_child_name("grid");
            self.grid.grab_focus();
            self.update_chrome();
        }
    }

    /// Leaves the grid, showing `pick` (or the file shown before).
    fn close_grid(&self, pick: Option<usize>) {
        let i = pick.unwrap_or(self.index.get());
        if pick.is_some_and(|p| p != self.index.get()) || self.current().is_none() {
            // off the gallery first, so the new file's page can come up when it is ready
            self.message.set_text("");
            self.stack.set_visible_child_name("message");
            self.show(i);
        } else {
            let page = match self.current().map(|c| c.kind) {
                Some(Kind::Image) if self.canvas.texture().is_none() => "message",
                Some(Kind::Image) => "image",
                Some(Kind::Video) => "video",
                Some(_) => "pdf",
                None => "message",
            };
            self.stack.set_visible_child_name(page);
        }
        self.window.grab_focus();
        self.update_chrome();
    }

    fn connect_grid(&self) {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, li| {
            let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
            let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
            tile.add_css_class("tile");
            tile.set_valign(gtk::Align::Start);
            let frame = gtk::Overlay::new();
            let size = gtk::Box::new(gtk::Orientation::Vertical, 0);
            size.set_size_request(176, 132);
            frame.set_child(Some(&size));
            let glyph = label("", &["big-icon", "faint"]);
            frame.add_overlay(&glyph);
            let pic = gtk::Picture::new();
            pic.set_content_fit(gtk::ContentFit::Contain);
            pic.set_can_shrink(true);
            frame.add_overlay(&pic);
            tile.append(&frame);
            let name = gtk::Label::new(None);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            name.set_max_width_chars(20);
            tile.append(&name);
            li.set_child(Some(&tile));
        });
        let me = self.weak.borrow().clone();
        factory.connect_bind(move |_, li| {
            let Some(me) = me.upgrade() else { return };
            let li = li.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(path) = li
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|s| PathBuf::from(s.string().as_str()))
            else {
                return;
            };
            let Some(tile) = li.child() else { return };
            let frame = tile.first_child().expect("frame");
            let name = frame
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .expect("name");
            let glyph = frame
                .first_child()
                .and_then(|c| c.next_sibling())
                .and_downcast::<gtk::Label>()
                .expect("glyph");
            let pic = frame
                .last_child()
                .and_downcast::<gtk::Picture>()
                .expect("picture");
            // no file system calls here: binding runs for every tile scrolled past
            let kind = playlist::kind(&path).unwrap_or(Kind::Image);
            let item = Item {
                path: path.clone(),
                kind,
                size: 0,
                modified: 0,
            };
            name.set_text(&item.name());
            glyph.set_text(kind_icon(&item));
            let key = path.to_string_lossy().into_owned();
            pic.set_widget_name(&key);
            let cached = me.thumbs.borrow().get(&path).cloned();
            pic.set_paintable(cached.as_ref());
            glyph.set_visible(cached.is_none());
            if cached.is_none() {
                let me = me.clone();
                glib::spawn_future_local(async move {
                    // the tile may have been recycled for another file meanwhile
                    if pic.widget_name() != key {
                        return;
                    }
                    let Some(t) = crate::load::thumbnail(path.clone(), kind, 256).await else {
                        return;
                    };
                    let mut thumbs = me.thumbs.borrow_mut();
                    if thumbs.len() >= THUMB_CACHE {
                        thumbs.clear();
                    }
                    thumbs.insert(path, t.clone());
                    drop(thumbs);
                    if pic.widget_name() == key {
                        pic.set_paintable(Some(&t));
                        glyph.set_visible(false);
                    }
                });
            }
        });
        self.grid.set_factory(Some(&factory));
        let me = self.weak.borrow().clone();
        self.grid.connect_activate(move |_, pos| {
            if let Some(me) = me.upgrade() {
                me.close_grid(Some(pos as usize));
            }
        });
    }

    // ── stage input ─────────────────────────────────────────────────────────

    fn connect_stage(&self) {
        let me = self.weak.borrow().clone();
        // pointer position, for zooming toward it
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[strong]
            me,
            move |_, x, y| {
                if let Some(me) = me.upgrade() {
                    me.pointer.set((x, y));
                }
            }
        ));
        self.canvas.add_controller(motion);

        // wheel: zoom images and videos toward the pointer; ctrl+wheel zooms PDFs
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        scroll.connect_scroll(glib::clone!(
            #[strong]
            me,
            move |c, _, dy| {
                let Some(me) = me.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let ctrl = c
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                match me.kind() {
                    Some(Kind::Image) => {
                        let (x, y) = me.pointer.get();
                        me.canvas.zoom_at(1.15f64.powf(-dy), x, y);
                    }
                    Some(Kind::Video) => me.video.add_zoom(-dy * 0.1),
                    Some(Kind::Pdf | Kind::Office) if ctrl => {
                        me.pdf.set_zoom(me.pdf.zoom.get() * 1.1f64.powf(-dy))
                    }
                    _ => return glib::Propagation::Proceed,
                }
                me.update_chrome();
                glib::Propagation::Stop
            }
        ));
        self.stack.add_controller(scroll);

        // pinch
        let pinch = gtk::GestureZoom::new();
        let last = Rc::new(Cell::new(1.0));
        pinch.connect_begin(glib::clone!(
            #[strong]
            last,
            move |_, _| last.set(1.0)
        ));
        pinch.connect_scale_changed(glib::clone!(
            #[strong]
            me,
            move |g, s| {
                let Some(me) = me.upgrade() else { return };
                let f = s / last.get();
                last.set(s);
                match me.kind() {
                    Some(Kind::Image) => {
                        let (x, y) = g.bounding_box_center().unwrap_or(me.pointer.get());
                        me.canvas.zoom_at(f, x, y);
                    }
                    Some(Kind::Video) => me.video.add_zoom(f.log2()),
                    Some(Kind::Pdf | Kind::Office) => me.pdf.set_zoom(me.pdf.zoom.get() * f),
                    None => {}
                }
                me.update_chrome();
            }
        ));
        self.stack.add_controller(pinch);

        // drag to pan (images and videos)
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        drag.connect_drag_begin(glib::clone!(
            #[strong]
            me,
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.drag_last.set((0.0001, 0.0001));
                    me.update_cursor();
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[strong]
            me,
            move |_, ox, oy| {
                let Some(me) = me.upgrade() else { return };
                let (lx, ly) = me.drag_last.get();
                let (dx, dy) = (ox - lx, oy - ly);
                me.drag_last.set((ox, oy));
                match me.kind() {
                    Some(Kind::Image) if me.canvas.can_pan() => me.canvas.pan(dx, dy),
                    Some(Kind::Video) if me.video.is_zoomed() => {
                        let (w, h) = (me.stack.width() as f64, me.stack.height() as f64);
                        me.video.pan(dx / w.max(1.0), dy / h.max(1.0));
                    }
                    _ => {}
                }
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[strong]
            me,
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.drag_last.set((0.0, 0.0));
                    me.update_chrome();
                }
            }
        ));
        self.canvas.add_controller(drag);
        let vdrag = gtk::GestureDrag::new();
        vdrag.set_button(1);
        vdrag.connect_drag_update(glib::clone!(
            #[strong]
            me,
            move |g, ox, oy| {
                let Some(me) = me.upgrade() else { return };
                if !me.video.is_zoomed() {
                    return;
                }
                let (lx, ly) = me.drag_last.get();
                me.drag_last.set((ox, oy));
                let (w, h) = (me.stack.width() as f64, me.stack.height() as f64);
                me.video.pan((ox - lx) / w.max(1.0), (oy - ly) / h.max(1.0));
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        ));
        vdrag.connect_drag_end(glib::clone!(
            #[strong]
            me,
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.drag_last.set((0.0, 0.0));
                }
            }
        ));
        self.video.area.add_controller(vdrag);

        // PDFs: drag the page around once it is zoomed in (the theme's hand cursors)
        let pdrag = gtk::GestureDrag::new();
        pdrag.set_button(1);
        let start = Rc::new(Cell::new((0.0, 0.0)));
        pdrag.connect_drag_begin(glib::clone!(
            #[strong]
            me,
            #[strong]
            start,
            move |g, _, _| {
                let Some(me) = me.upgrade() else { return };
                if !me.pdf.can_pan() {
                    g.set_state(gtk::EventSequenceState::Denied);
                    return;
                }
                let w = &me.pdf.widget;
                start.set((w.hadjustment().value(), w.vadjustment().value()));
                w.set_cursor_from_name(Some("grabbing"));
            }
        ));
        pdrag.connect_drag_update(glib::clone!(
            #[strong]
            me,
            #[strong]
            start,
            move |_, dx, dy| {
                let Some(me) = me.upgrade() else { return };
                let (x, y) = start.get();
                let w = &me.pdf.widget;
                w.hadjustment().set_value(x - dx);
                w.vadjustment().set_value(y - dy);
            }
        ));
        pdrag.connect_drag_end(glib::clone!(
            #[strong]
            me,
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.pdf.update_cursor();
                }
            }
        ));
        self.pdf.widget.add_controller(pdrag);

        // clicks: double = fullscreen, middle = fit, right = menu
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.connect_pressed(glib::clone!(
            #[strong]
            me,
            move |g, n, x, y| {
                let Some(me) = me.upgrade() else { return };
                match g.current_button() {
                    // (not in the gallery: there the first click already opened a file)
                    1 if n == 2 && me.kind().is_some() => me.toggle_fullscreen(),
                    2 => me.fit(),
                    3 => me.popup_menu(Some((x, y))),
                    _ => {}
                }
            }
        ));
        self.stack.add_controller(click);

        // the fitted zoom (shown in the header) follows the window size; updated once the
        // resize settles into a frame, not from inside the layout pass
        let queued = Rc::new(Cell::new(false));
        self.canvas.connect_resized(move || {
            if queued.replace(true) {
                return;
            }
            let (me, queued) = (me.clone(), queued.clone());
            glib::idle_add_local_once(move || {
                queued.set(false);
                if let Some(me) = me.upgrade() {
                    me.update_chrome();
                }
            });
        });
    }

    fn connect_timeline(&self) {
        let me = self.weak.borrow().clone();
        let (fg, acc) = (
            gdk::RGBA::parse(&self.theme.foreground).unwrap_or(gdk::RGBA::WHITE),
            gdk::RGBA::parse(&self.theme.accent).unwrap_or(gdk::RGBA::WHITE),
        );
        self.timeline.set_draw_func(glib::clone!(
            #[strong]
            me,
            move |_, cr, w, h| {
                let Some(me) = me.upgrade() else { return };
                let s = me.video.state.get();
                let thick = if me.timeline_hover.get().is_some() || me.seeking.get() {
                    4.0
                } else {
                    2.0
                };
                let y = (h as f64 - thick) / 2.0;
                cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.15);
                cr.rectangle(0.0, y, w as f64, thick);
                let _ = cr.fill();
                if s.duration > 0.0 {
                    cr.set_source_rgba(
                        acc.red() as f64,
                        acc.green() as f64,
                        acc.blue() as f64,
                        1.0,
                    );
                    cr.rectangle(
                        0.0,
                        y,
                        w as f64 * (s.time / s.duration).clamp(0.0, 1.0),
                        thick,
                    );
                    let _ = cr.fill();
                }
            }
        ));
        let seek = |me: &Win, x: f64| {
            let d = me.video.state.get().duration;
            let w = me.timeline.width().max(1) as f64;
            if d > 0.0 {
                me.video.seek_to(d * (x / w).clamp(0.0, 1.0));
            }
        };
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(
            #[strong]
            me,
            move |_, x, _| {
                if let Some(me) = me.upgrade() {
                    me.seeking.set(true);
                    seek(&me, x);
                }
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[strong]
            me,
            move |g, ox, _| {
                if let Some(me) = me.upgrade()
                    && let Some((x, _)) = g.start_point()
                {
                    seek(&me, x + ox);
                }
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[strong]
            me,
            move |_, _, _| {
                if let Some(me) = me.upgrade() {
                    me.seeking.set(false);
                    me.timeline.queue_draw();
                }
            }
        ));
        self.timeline.add_controller(drag);
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[strong]
            me,
            move |_, x, _| {
                let Some(me) = me.upgrade() else { return };
                me.timeline_hover.set(Some(x));
                let d = me.video.state.get().duration;
                let w = me.timeline.width().max(1) as f64;
                me.timeline
                    .set_tooltip_text(Some(&playlist::time(d * (x / w).clamp(0.0, 1.0))));
                me.timeline.queue_draw();
            }
        ));
        motion.connect_leave(glib::clone!(
            #[strong]
            me,
            move |_| {
                if let Some(me) = me.upgrade() {
                    me.timeline_hover.set(None);
                    me.timeline.queue_draw();
                }
            }
        ));
        self.timeline.add_controller(motion);
    }

    // ── actions: open with, copy, trash, info ───────────────────────────────

    fn launch(&self, app: &gio::AppInfo, path: &Path) {
        if vector_core::no_launch() {
            self.notify(&format!("test run: would open in {}", app.display_name()));
            return;
        }
        let ctx = gtk::prelude::WidgetExt::display(&self.window).app_launch_context();
        match app.launch(&[gio::File::for_path(path)], Some(&ctx)) {
            Ok(()) => self.notify(&format!("opened in {}", app.display_name())),
            Err(e) => self.notify_error(&format!("{}: {}", app.display_name(), e.message())),
        }
    }

    fn open_default(&self) {
        let Some(item) = self.current() else { return };
        match self.open_app.borrow().clone() {
            Some(app) => self.launch(&app, &item.path),
            None => self.notify_error("no other app opens this file"),
        }
    }

    fn open_with(&self, id: &str) {
        let Some(item) = self.current() else { return };
        if let Some(app) = gio::AppInfo::all()
            .into_iter()
            .find(|a| a.id().as_deref() == Some(id))
        {
            self.launch(&app, &item.path);
        }
    }

    /// Asks the file manager to show the file in its folder (org.freedesktop.FileManager1),
    /// or opens the folder.
    fn show_in_folder(&self) {
        let Some(item) = self.current() else { return };
        if vector_core::no_launch() {
            self.notify("test run: would show the file in its folder");
            return;
        }
        let uri = gio::File::for_path(&item.path).uri().to_string();
        let dir = item
            .path
            .parent()
            .map(|d| gio::File::for_path(d).uri().to_string());
        let me = self.weak.borrow().clone();
        glib::spawn_future_local(async move {
            let shown = async {
                let bus = gio::bus_get_future(gio::BusType::Session).await.ok()?;
                bus.call_future(
                    Some("org.freedesktop.FileManager1"),
                    "/org/freedesktop/FileManager1",
                    "org.freedesktop.FileManager1",
                    "ShowItems",
                    Some(&(vec![uri], "").to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    3000,
                )
                .await
                .ok()
            }
            .await
            .is_some();
            if !shown
                && let Some(d) = dir
                && gio::AppInfo::launch_default_for_uri(&d, gio::AppLaunchContext::NONE).is_err()
                && let Some(me) = me.upgrade()
            {
                me.notify_error("no file manager to show the folder");
            }
        });
    }

    fn set_clipboard(&self, texture: &gdk::Texture, what: &str) {
        if std::env::var_os("VECTOR_NO_CLIPBOARD").is_some() {
            self.notify(&format!("{what} copied (test run: clipboard untouched)"));
            return;
        }
        gtk::prelude::WidgetExt::display(&self.window)
            .clipboard()
            .set_texture(texture);
        self.notify(&format!("{what} copied"));
    }

    /// Has mpv write the current video frame to `out` (in the background) and waits for the
    /// finished file without blocking. `keep` false removes it after reading.
    async fn grab_frame(&self, out: PathBuf, keep: bool) -> Option<gdk::Texture> {
        if !self.video.screenshot(&out) {
            return None;
        }
        // mpv writes the file on its own thread: wait until it stops growing
        let mut last = 0;
        let mut texture = None;
        for _ in 0..100 {
            glib::timeout_future(Duration::from_millis(50)).await;
            let p = out.clone();
            let len = gio::spawn_blocking(move || std::fs::metadata(p).map_or(0, |m| m.len()))
                .await
                .unwrap_or(0);
            if len > 0 && len == last {
                texture = crate::load::image(out.clone()).await.ok();
                break;
            }
            last = len;
        }
        if !keep {
            gio::spawn_blocking(move || std::fs::remove_file(out));
        }
        texture
    }

    fn copy(&self) {
        match self.kind() {
            Some(Kind::Image) => {
                if let Some(t) = self.canvas.texture() {
                    self.set_clipboard(&t, "image");
                }
            }
            Some(Kind::Pdf | Kind::Office) => match self.pdf.current_texture() {
                Some(t) => self.set_clipboard(&t, "page"),
                None => self.notify_error("page not drawn yet"),
            },
            Some(Kind::Video) => {
                let tmp = glib::user_runtime_dir().join(format!(
                    "vector-view-{}-{}.png",
                    std::process::id(),
                    glib::monotonic_time()
                ));
                let Some(me) = self.me() else { return };
                glib::spawn_future_local(async move {
                    match me.grab_frame(tmp, false).await {
                        Some(t) => me.set_clipboard(&t, "frame"),
                        None => me.notify_error("can't take a frame"),
                    }
                });
            }
            None => {}
        }
    }

    /// `s`: videos save the frame to Pictures/Screenshots (and copy it); the rest copies.
    fn save(&self) {
        let Some(item) = self.current() else { return };
        if item.kind != Kind::Video {
            return self.copy();
        }
        let stem = item
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stamp = glib::DateTime::now_local()
            .and_then(|d| d.format("%Y%m%d-%H%M%S"))
            .map(|s| s.to_string())
            .unwrap_or_default();
        let Some(me) = self.me() else { return };
        glib::spawn_future_local(async move {
            let out = gio::spawn_blocking(move || {
                let dir = glib::user_special_dir(glib::UserDirectory::Pictures)
                    .unwrap_or_else(glib::home_dir)
                    .join("Screenshots");
                std::fs::create_dir_all(&dir).ok()?;
                // never overwrite: two frames in one second get -2, -3…
                (1..1000)
                    .map(|n| {
                        let suffix = if n == 1 {
                            String::new()
                        } else {
                            format!("-{n}")
                        };
                        dir.join(format!("{stem}-{stamp}{suffix}.png"))
                    })
                    .find(|p| !p.exists())
            })
            .await
            .ok()
            .flatten();
            let Some(out) = out else {
                me.notify_error("can't write to Pictures/Screenshots");
                return;
            };
            match me.grab_frame(out.clone(), true).await {
                Some(t) => {
                    me.set_clipboard(&t, "frame");
                    me.notify(&format!("frame saved: {}", out.display()));
                }
                None => me.notify_error("can't take a frame"),
            }
        });
    }

    /// Moves the file to the trash. From the keyboard it takes two presses within 2 s.
    fn trash(&self, confirmed: bool) {
        let Some(item) = self.current() else { return };
        let armed = self
            .delete_armed
            .get()
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        if !confirmed && !armed {
            self.delete_armed.set(Some(Instant::now()));
            self.notify("press Delete again to move this file to the trash");
            return;
        }
        if confirmed && !armed {
            // the button asks the same way: a second click confirms
            self.delete_armed.set(Some(Instant::now()));
            self.notify("click again to move this file to the trash");
            return;
        }
        self.delete_armed.set(None);
        let me = self.weak.borrow().clone();
        glib::spawn_future_local(async move {
            let r = gio::File::for_path(&item.path)
                .trash_future(glib::Priority::DEFAULT)
                .await;
            let Some(me) = me.upgrade() else { return };
            match r {
                Ok(()) => {
                    let mut items = me.items.borrow_mut();
                    let removed = playlist::position(&items, &item.path);
                    if let Some(i) = removed {
                        items.remove(i);
                    }
                    let len = items.len();
                    drop(items);
                    me.notify(&format!("moved to trash: {}", item.name()));
                    let Some(removed) = removed else { return };
                    if len == 0 {
                        me.video.stop();
                        me.canvas.set_texture(None, false);
                        me.show_message("nothing left to show");
                        me.update_chrome();
                        return;
                    }
                    // the user may have moved on while the trash call ran
                    let (index, reload) = playlist::after_remove(me.index.get(), removed, len);
                    if reload {
                        me.show(index);
                    } else {
                        me.index.set(index);
                        me.update_chrome();
                    }
                }
                Err(e) => me.notify_error(&format!("trash failed: {}", e.message())),
            }
        });
    }

    fn toggle_info(&self) {
        let show = !self.info.reveals_child();
        self.info.set_reveal_child(show);
        if show {
            self.load_info();
        }
        self.update_chrome();
    }

    fn load_info(&self) {
        let Some(item) = self.current() else { return };
        let g = self.generation.get();
        let me = self.weak.borrow().clone();
        glib::spawn_future_local(async move {
            let path = item.path.clone();
            let rows = gio::spawn_blocking(move || crate::info::rows(&item))
                .await
                .unwrap_or_default();
            let Some(me) = me.upgrade() else { return };
            if me.generation.get() != g || me.current().map(|c| c.path) != Some(path) {
                return;
            }
            while let Some(c) = me.info_rows.first_child() {
                me.info_rows.remove(&c);
            }
            let mut r = 0;
            for (section, rows) in &rows {
                let head = label(section, &["title", "details-section"]);
                head.set_xalign(0.0);
                head.set_margin_top(if r == 0 { 0 } else { 12 });
                me.info_rows.attach(&head, 0, r, 2, 1);
                r += 1;
                for (k, v) in rows {
                    let key = label(k, &["dim"]);
                    key.set_xalign(0.0);
                    key.set_valign(gtk::Align::Start);
                    let val = label(v, &[]);
                    val.set_xalign(0.0);
                    val.set_wrap(true);
                    val.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    val.set_max_width_chars(30);
                    val.set_selectable(true);
                    me.info_rows.attach(&key, 0, r, 1, 1);
                    me.info_rows.attach(&val, 1, r, 1, 1);
                    r += 1;
                }
            }
        });
    }

    // ── menu ────────────────────────────────────────────────────────────────

    fn connect_actions(&self) {
        let group = gio::SimpleActionGroup::new();
        let me = self.weak.borrow().clone();
        let add = |name: &str, f: fn(&Win)| {
            let a = gio::SimpleAction::new(name, None);
            let me = me.clone();
            a.connect_activate(move |_, _| {
                if let Some(me) = me.upgrade() {
                    f(&me);
                }
            });
            group.add_action(&a);
        };
        add("open-default", |w| w.open_default());
        add("show-folder", |w| w.show_in_folder());
        add("copy", |w| w.copy());
        add("save", |w| w.save());
        add("gallery", |w| w.toggle_grid());
        add("info", |w| w.toggle_info());
        add("fullscreen", |w| w.toggle_fullscreen());
        add("trash", |w| {
            w.delete_armed.set(Some(Instant::now()));
            w.trash(true)
        });
        add("keys", |w| w.toggle_help());
        let with = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let m = me.clone();
        with.connect_activate(move |_, v| {
            if let (Some(me), Some(id)) = (m.upgrade(), v.and_then(|v| v.get::<String>())) {
                me.open_with(&id);
            }
        });
        group.add_action(&with);
        let order = gio::SimpleAction::new_stateful(
            "order",
            Some(glib::VariantTy::STRING),
            &self.order.get().label().to_variant(),
        );
        let m = me.clone();
        order.connect_activate(move |a, v| {
            if let (Some(me), Some(o)) = (
                m.upgrade(),
                v.and_then(|v| v.get::<String>())
                    .and_then(|s| Order::parse(&s)),
            ) {
                a.set_state(&o.label().to_variant());
                me.set_order(o);
            }
        });
        group.add_action(&order);
        self.window.insert_action_group("view", Some(&group));
    }

    fn popup_menu(&self, at: Option<(f64, f64)>) {
        let Some(item) = self.current() else { return };
        let menu = gio::Menu::new();
        let open = gio::Menu::new();
        if let Some(app) = default_app(&item.path) {
            open.append(
                Some(&format!("open in {}", app.display_name())),
                Some("view.open-default"),
            );
        }
        let with = gio::Menu::new();
        for app in other_apps(&item.path) {
            if let Some(id) = app.id() {
                with.append(
                    Some(&app.display_name()),
                    Some(&format!("view.open-with('{}')", id.replace('\'', "\\'"))),
                );
            }
        }
        if with.n_items() > 0 {
            open.append_submenu(Some("open with"), &with);
        }
        open.append(Some("show in folder"), Some("view.show-folder"));
        menu.append_section(None, &open);

        let edit = gio::Menu::new();
        if item.kind == Kind::Video {
            edit.append(Some("save frame"), Some("view.save"));
            edit.append(Some("copy frame"), Some("view.copy"));
        } else if item.kind == Kind::Image {
            edit.append(Some("copy image"), Some("view.copy"));
        } else {
            edit.append(Some("copy page"), Some("view.copy"));
        }
        menu.append_section(None, &edit);

        let view = gio::Menu::new();
        let order = gio::Menu::new();
        for o in Order::ALL {
            order.append(
                Some(o.label()),
                Some(&format!("view.order('{}')", o.label())),
            );
        }
        view.append_submenu(Some("order"), &order);
        view.append(Some("gallery"), Some("view.gallery"));
        view.append(Some("details"), Some("view.info"));
        view.append(Some("fullscreen"), Some("view.fullscreen"));
        view.append(Some("keys"), Some("view.keys"));
        menu.append_section(None, &view);

        let danger = gio::Menu::new();
        danger.append(Some("move to trash"), Some("view.trash"));
        menu.append_section(None, &danger);

        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_has_arrow(false);
        let parent: gtk::Widget = if at.is_some() {
            self.stack.clone().upcast()
        } else {
            self.statusline.clone().upcast()
        };
        pop.set_parent(&parent);
        let (x, y) = at.unwrap_or((24.0, 0.0));
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        if at.is_none() {
            pop.set_position(gtk::PositionType::Top);
        }
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    // ── keys ────────────────────────────────────────────────────────────────

    fn connect_keys(&self) {
        let ctl = gtk::EventControllerKey::new();
        ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.weak.borrow().clone();
        ctl.connect_key_pressed(move |_, keyval, _, state| {
            let Some(me) = me.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match to_key(keyval, state) {
                Some(k) if me.handle_key(k) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(ctl);
    }

    /// Runs one key. Returns false when it isn't ours (the grid's arrows, for example).
    pub fn handle_key(&self, key: Key) -> bool {
        if self.help.is_visible() {
            self.help.set_visible(false);
            return true;
        }
        let grid = self.stack.visible_child_name().as_deref() == Some("grid");
        if grid {
            match key {
                Key::Esc | Key::Char('g') => self.close_grid(None),
                Key::Char('q') => self.window.close(),
                Key::Char('o') => self.cycle_order(),
                Key::Char('?') => self.toggle_help(),
                Key::Tab => self.toggle_bars(),
                _ => return false,
            }
            return true;
        }
        let kind = self.current().map(|c| c.kind);
        let paged = matches!(kind, Some(Kind::Pdf | Kind::Office));
        let video = kind == Some(Kind::Video);
        match key {
            Key::Left => self.step(-1),
            Key::Right => self.step(1),
            Key::PageUp if paged => self.pdf.scroll(-1.0),
            Key::PageDown if paged => self.pdf.scroll(1.0),
            Key::PageUp => self.step(-1),
            Key::PageDown => self.step(1),
            Key::Home if paged => self.pdf.to_start(false),
            Key::End if paged => self.pdf.to_start(true),
            Key::Home => self.show(0),
            Key::End => self.show(self.items.borrow().len().saturating_sub(1)),
            Key::Char(' ') if video => self.video.toggle_pause(),
            Key::Char(' ') if paged => self.pdf.scroll(1.0),
            Key::Up if video => self.video.add_volume(5.0),
            Key::Down if video => self.video.add_volume(-5.0),
            Key::Up | Key::Char('k') if paged => self.pdf.scroll_px(-80.0),
            Key::Down | Key::Char('j') if paged => self.pdf.scroll_px(80.0),
            Key::Char('h') => self.pan(-0.1, 0.0),
            Key::Char('l') => self.pan(0.1, 0.0),
            Key::Char('k') => self.pan(0.0, -0.1),
            Key::Char('j') => self.pan(0.0, 0.1),
            Key::Char('+') | Key::Char('=') => self.zoom(1.0),
            Key::Char('-') => self.zoom(-1.0),
            Key::Char('0') => self.fit(),
            Key::Char('1') if kind == Some(Kind::Image) => {
                self.canvas.actual_size();
                self.update_chrome();
            }
            Key::Char('[') if video => self.video.seek(-5.0),
            Key::Char(']') if video => self.video.seek(5.0),
            Key::Char(',') if video => self.video.frame_step(true),
            Key::Char('.') if video => self.video.frame_step(false),
            Key::Char('o') => self.cycle_order(),
            Key::Char('g') => self.toggle_grid(),
            Key::Char('i') => self.toggle_info(),
            Key::Char('z') => self.toggle_zoom_lock(),
            Key::Char('f') => self.toggle_fullscreen(),
            Key::Char('e') => self.open_default(),
            Key::Char('m') => self.popup_menu(None),
            Key::Char('s') => self.save(),
            Key::Char('?') => self.toggle_help(),
            Key::Char('q') => self.window.close(),
            Key::Ctrl('c') => self.copy(),
            Key::Ctrl('o') => self.show_in_folder(),
            Key::Delete => self.trash(false),
            Key::Tab => self.toggle_bars(),
            Key::Esc => {
                if self.window.is_fullscreen() {
                    self.window.unfullscreen();
                } else if self.info.reveals_child() {
                    self.info.set_reveal_child(false);
                } else {
                    self.window.close();
                }
            }
            _ => return false,
        }
        true
    }
}

fn to_key(keyval: gdk::Key, state: gdk::ModifierType) -> Option<Key> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    if state.contains(gdk::ModifierType::ALT_MASK) {
        return None;
    }
    use gdk::Key as K;
    let k = match keyval {
        K::Escape => Key::Esc,
        K::Tab => Key::Tab,
        K::Delete | K::KP_Delete => Key::Delete,
        // shift+arrows seek, ctrl+arrows pan
        K::Left if shift => Key::Char('['),
        K::Right if shift => Key::Char(']'),
        K::Left if ctrl => Key::Char('h'),
        K::Right if ctrl => Key::Char('l'),
        K::Up if ctrl => Key::Char('k'),
        K::Down if ctrl => Key::Char('j'),
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Home => Key::Home,
        K::End => Key::End,
        K::Page_Up => Key::PageUp,
        K::Page_Down => Key::PageDown,
        K::Return | K::KP_Enter => Key::Enter,
        K::KP_Add => Key::Char('+'),
        K::KP_Subtract => Key::Char('-'),
        _ => {
            let c = keyval.to_unicode()?;
            if ctrl {
                Key::Ctrl(c.to_ascii_lowercase())
            } else if c.is_control() {
                return None;
            } else {
                Key::Char(c)
            }
        }
    };
    Some(k)
}

/// The `?` card: every key, laid out like vector-find's key guide.
fn help_card() -> gtk::Box {
    const KEYS: &[(&str, &[(&str, &str)])] = &[
        (
            "move",
            &[
                ("← →", "previous, next file (wraps)"),
                ("home end", "first, last file"),
                ("g", "gallery"),
                ("o", "order: name, date, size, random, type"),
            ],
        ),
        (
            "zoom",
            &[
                ("wheel + -", "zoom toward the pointer"),
                ("0 middle click", "fit to the window"),
                ("1", "actual size"),
                ("drag h j k l", "pan"),
                ("z", "keep the zoom between files"),
            ],
        ),
        (
            "play and read",
            &[
                ("space", "pause; page down in a PDF"),
                ("[ ] shift+← →", "seek 5 s"),
                (", .", "frame back, forward"),
                ("↑ ↓", "volume; scroll a PDF"),
                ("ctrl+wheel", "zoom a PDF"),
            ],
        ),
        (
            "file",
            &[
                ("e", "open in the default app"),
                ("m right click", "menu, open with"),
                ("ctrl+o", "show in folder"),
                ("s ctrl+c", "save frame, copy"),
                ("i", "details"),
                ("del del", "move to trash"),
                ("f tab", "fullscreen, hide the bars"),
                ("esc q", "leave fullscreen, close"),
            ],
        ),
    ];
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.add_css_class("help");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    let head = label("keys", &["title"]);
    head.set_xalign(0.0);
    let hb = gtk::Box::new(gtk::Orientation::Vertical, 0);
    hb.add_css_class("head");
    hb.append(&head);
    card.append(&hb);
    let cols = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    cols.add_css_class("body");
    for pair in KEYS.chunks(2) {
        // one grid per column, so keys line up across its sections
        let grid = gtk::Grid::new();
        grid.set_column_spacing(16);
        grid.set_row_spacing(4);
        let mut row = 0;
        for (section, rows) in pair {
            let sl = label(section, &["dim"]);
            sl.set_xalign(0.0);
            sl.set_margin_top(if row == 0 { 0 } else { 10 });
            grid.attach(&sl, 0, row, 2, 1);
            row += 1;
            for (k, d) in rows.iter() {
                let kl = label(k, &["ok"]);
                kl.set_xalign(0.0);
                let dl = label(d, &[]);
                dl.set_xalign(0.0);
                grid.attach(&kl, 0, row, 1, 1);
                grid.attach(&dl, 1, row, 1, 1);
                row += 1;
            }
        }
        cols.append(&grid);
    }
    card.append(&cols);
    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.add_css_class("foot");
    foot.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("close");
    close.add_css_class("link");
    let c = card.downgrade();
    close.connect_clicked(move |_| {
        if let Some(c) = c.upgrade() {
            c.set_visible(false);
        }
    });
    foot.append(&close);
    card.append(&foot);
    card
}
