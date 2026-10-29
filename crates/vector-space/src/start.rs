//! The Start menu (SUPER tap, or a click on the bar's mode block), in the
//! Vista layout. Left: pinned and most used apps, with all apps and recent
//! files as pages of the same pane, and the search along the bottom (typing
//! hands the query to Vector Space, which has every mode). Right: you, your
//! places, settings and system tiles, and power along the bottom.
//! Keyboard: type to search, Down/Up to walk the rows, Enter opens, Escape closes.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::apps::{self, App};
use crate::util::{self, Usage};

const PINS: &str = "start-pins.json";
const LEFT_W: i32 = 400;
const RIGHT_W: i32 = 336;
const MOST_USED: usize = 10;
const TILE_COLS: u32 = 4;

pub struct Start {
    win: gtk::ApplicationWindow,
    entry: gtk::Entry,
    pages: gtk::Stack,
    pinned: gtk::Box,
    frequent: gtk::Box,
    all: gtk::Box,
    recent: gtk::Box,
    nav: gtk::Button,
    nav_label: gtk::Label,
    nav_glyph: gtk::Label,
    time: gtk::Label,
    date: gtk::Label,
    apps: RefCell<Vec<App>>,
    armed: RefCell<Option<String>>,
    /// rows rebuilt while hidden (after a close, and once at start), so opening only draws
    prepared: Cell<bool>,
    /// the all-apps page is built when first shown after a prepare
    all_built: Cell<bool>,
}

thread_local! {
    static START: RefCell<Option<Rc<Start>>> = const { RefCell::new(None) };
}

/// `vector-space start`: open, or close when open.
pub fn toggle(app: &gtk::Application) {
    let s = START.with(|s| s.borrow().clone()).unwrap_or_else(|| {
        let s = build(app);
        START.with(|x| *x.borrow_mut() = Some(s.clone()));
        s
    });
    if s.win.is_visible() {
        s.hide();
    } else {
        s.show();
    }
}

pub fn hide() {
    if let Some(s) = START.with(|s| s.borrow().clone())
        && s.win.is_visible()
    {
        s.hide();
    }
}

/// Built with the daemon, so even the first open only draws.
pub fn init(app: &gtk::Application) {
    START.with(|s| *s.borrow_mut() = Some(build(app)));
}

fn build(app: &gtk::Application) -> Rc<Start> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("vspace");
    win.add_css_class("vstart");
    win.set_title(Some("Vector Start"));
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some("vector-start"));
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
    backdrop.add_css_class("start-backdrop");
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    card.add_css_class("card");
    card.add_css_class("start");
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.set_margin_top(34);
    card.set_margin_start(8);
    backdrop.append(&card);
    win.set_child(Some(&backdrop));

    // ── left: apps ───────────────────────────────────────────────
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.add_css_class("start-left");
    left.set_size_request(LEFT_W, -1);
    let pinned = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let frequent = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let home = gtk::Box::new(gtk::Orientation::Vertical, 0);
    home.append(&section("pinned"));
    home.append(&pinned);
    home.append(&section("most used"));
    home.append(&frequent);
    let all = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let recent = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let recent_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    recent_page.append(&section("recent files"));
    recent_page.append(&recent);
    let pages = gtk::Stack::new();
    pages.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    pages.set_transition_duration(160);
    pages.add_named(&scrolled(&home), Some("home"));
    pages.add_named(&scrolled(&all), Some("all"));
    pages.add_named(&scrolled(&recent_page), Some("recent"));
    pages.set_vexpand(true);
    left.append(&pages);

    // all apps / back
    let nav = gtk::Button::new();
    nav.add_css_class("start-row");
    nav.add_css_class("start-nav");
    let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let nav_glyph = glyph("\u{f003b}");
    let nav_label = gtk::Label::new(Some("all apps"));
    nav_label.set_xalign(0.0);
    nav_label.set_hexpand(true);
    let nav_arrow = gtk::Label::new(Some("\u{f0142}"));
    nav_arrow.add_css_class("start-meta");
    nav_box.append(&nav_glyph);
    nav_box.append(&nav_label);
    nav_box.append(&nav_arrow);
    nav.set_child(Some(&nav_box));
    left.append(&nav);

    // the search, along the bottom
    let search = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    search.add_css_class("start-searchbar");
    let sglyph = gtk::Label::new(Some("\u{f0349}"));
    sglyph.add_css_class("start-sglyph");
    let entry = gtk::Entry::new();
    entry.add_css_class("start-search");
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("type to search"));
    let hint = gtk::Label::new(Some("= calc  > convert  ; clip"));
    hint.add_css_class("start-hint");
    search.append(&sglyph);
    search.append(&entry);
    search.append(&hint);
    left.append(&search);
    card.append(&left);

    // ── right: you, places, tiles, power ─────────────────────────
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.add_css_class("start-right");
    right.set_size_request(RIGHT_W, -1);
    let user = user_block();
    let when = gtk::Box::new(gtk::Orientation::Vertical, 2);
    when.set_hexpand(true);
    when.set_halign(gtk::Align::End);
    when.set_valign(gtk::Align::Center);
    let time = gtk::Label::new(None);
    time.add_css_class("start-time");
    time.set_xalign(1.0);
    let date = gtk::Label::new(None);
    date.add_css_class("start-date");
    date.set_xalign(1.0);
    when.append(&time);
    when.append(&date);
    user.append(&when);
    right.append(&user);
    card.append(&right);

    let s = Rc::new(Start {
        win,
        entry,
        pages,
        pinned,
        frequent,
        all,
        recent,
        nav,
        nav_label,
        nav_glyph,
        time,
        date,
        apps: RefCell::default(),
        armed: RefCell::default(),
        prepared: Cell::new(false),
        all_built: Cell::new(false),
    });

    right.append(&section("places"));
    for (g, name, path) in places() {
        let p = path.clone();
        right.append(&s.button(&glyph(g), &name, "", move || util::launch(&["vector-find", &p.to_string_lossy()])));
    }
    let w = Rc::downgrade(&s);
    let recent_row = row(&glyph("\u{f02da}"), "recent files", "\u{f0142}");
    recent_row.connect_clicked(move |_| {
        if let Some(s) = w.upgrade() {
            s.page("recent");
        }
    });
    right.append(&recent_row);

    right.append(&section("settings"));
    right.append(&s.tiles(SETTINGS));
    right.append(&section("system"));
    right.append(&s.tiles(SYSTEM));

    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    right.append(&spacer);

    // power, level with the search
    let power = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    power.add_css_class("start-power");
    power.set_homogeneous(true);
    for &(g, tip, cmd, confirm) in POWER {
        let b = gtk::Button::new();
        b.add_css_class("start-pow");
        b.set_child(Some(&glyph(g)));
        b.set_tooltip_text(Some(tip));
        let w = Rc::downgrade(&s);
        let key = tip.to_string();
        b.connect_clicked(move |b| {
            let Some(s) = w.upgrade() else { return };
            if confirm && s.armed.borrow().as_deref() != Some(&key) {
                // the risky ones need a second click
                *s.armed.borrow_mut() = Some(key.clone());
                b.add_css_class("armed");
                b.set_tooltip_text(Some(&format!("click again to {key}")));
                return;
            }
            s.hide();
            util::launch(&["sh", "-c", cmd]);
        });
        power.append(&b);
    }
    right.append(&power);

    let w = Rc::downgrade(&s);
    s.nav.connect_clicked(move |_| {
        if let Some(s) = w.upgrade() {
            let home = s.pages.visible_child_name().as_deref() == Some("home");
            s.page(if home { "all" } else { "home" });
        }
    });

    // typing searches: the query goes to Vector Space
    let w = Rc::downgrade(&s);
    s.entry.connect_changed(move |e| {
        let text = e.text().to_string();
        if text.is_empty() {
            return;
        }
        let Some(s) = w.upgrade() else { return };
        s.hide();
        e.set_text("");
        if let Some(ui) = crate::ui::get() {
            ui.search(&text);
        }
    });

    // keys: Escape closes (or goes back a page), Down leaves the search for the rows
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = Rc::downgrade(&s);
    keys.connect_key_pressed(move |_, k, _, _| {
        let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
        match k {
            gdk::Key::Escape => {
                if s.pages.visible_child_name().as_deref() != Some("home") {
                    s.page("home");
                } else {
                    s.hide();
                }
                glib::Propagation::Stop
            }
            gdk::Key::Down if s.entry.has_focus() => {
                s.focus_first();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    s.win.add_controller(keys);

    // a click outside closes
    let click = gtk::GestureClick::new();
    let w = Rc::downgrade(&s);
    let c2 = card.clone();
    click.connect_pressed(move |g, _, x, y| {
        let (Some(s), Some(bd)) = (w.upgrade(), g.widget()) else { return };
        if let Some(b) = c2.compute_bounds(&bd)
            && !b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
        {
            s.hide();
        }
    });
    backdrop.add_controller(click);
    s.prepare_soon(1500);
    s
}

impl Start {
    fn show(self: &Rc<Self>) {
        if let Some(ui) = crate::ui::get() {
            ui.hide();
        }
        let t = std::time::Instant::now();
        if !self.prepared.get() {
            self.prepare();
        }
        self.prepared.set(false);
        self.page("home");
        *self.armed.borrow_mut() = None;
        self.tick();
        // keep the clock right while open
        let w = Rc::downgrade(self);
        glib::timeout_add_seconds_local(5, move || match w.upgrade() {
            Some(s) if s.win.is_visible() => {
                s.tick();
                glib::ControlFlow::Continue
            }
            _ => glib::ControlFlow::Break,
        });
        self.place();
        self.win.present();
        self.entry.grab_focus();
        if std::env::var_os("VECTOR_SPACE_TIMING").is_some() {
            eprintln!("start shown in {:?}", t.elapsed());
        }
        if std::env::var_os("VECTOR_SPACE_TEST").is_some() {
            eprintln!("start shown on {:?}", self.win.monitor().and_then(|m| m.connector()));
        }
    }

    fn hide(self: &Rc<Self>) {
        self.win.set_visible(false);
        self.prepare_soon(200);
    }

    /// Reload apps, usage and recent files while nobody looks.
    fn prepare_soon(self: &Rc<Self>, ms: u64) {
        self.prepared.set(false);
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
            if let Some(s) = w.upgrade()
                && !s.win.is_visible()
            {
                s.prepare();
            }
        });
    }

    fn prepare(self: &Rc<Self>) {
        *self.apps.borrow_mut() = apps::load();
        self.fill_apps();
        self.fill_recent();
        self.all_built.set(false);
        self.prepared.set(true);
    }

    fn tick(&self) {
        if let Ok(now) = glib::DateTime::now_local() {
            self.time.set_text(&now.format("%H:%M").map(|s| s.to_string()).unwrap_or_default());
            self.date.set_text(&now.format("%a %-d %b").map(|s| s.to_lowercase()).unwrap_or_default());
        }
    }

    fn page(self: &Rc<Self>, name: &str) {
        if name == "all" && !self.all_built.get() {
            self.fill_all();
        }
        self.pages.set_visible_child_name(name);
        let home = name == "home";
        self.nav_label.set_text(if home { "all apps" } else { "back" });
        self.nav_glyph.set_text(if home { "\u{f003b}" } else { "\u{f0141}" });
    }

    fn focus_first(&self) {
        let page = self.pages.visible_child().and_then(|c| first_button(&c));
        if let Some(b) = page {
            b.grab_focus();
        }
    }

    /// The focused monitor (or VECTOR_OUTPUT for test instances).
    fn place(&self) {
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
            self.win.set_monitor(Some(&m));
        }
    }

    fn fill_apps(self: &Rc<Self>) {
        clear(&self.pinned);
        clear(&self.frequent);
        clear(&self.all);
        let apps = self.apps.borrow();
        let pins = pins(&apps);
        for id in &pins {
            if let Some(a) = apps.iter().find(|a| &a.id == id) {
                let r = self.app_row(a, true);
                r.add_css_class("start-pinned");
                self.pinned.append(&r);
            }
        }
        if self.pinned.first_child().is_none() {
            self.pinned.append(&empty("right click an app to pin it here"));
        }
        // most used: launch counts, fading with time, pins left out
        let usage = Usage::load("launches.json");
        let mut used: Vec<&App> =
            apps.iter().filter(|a| !pins.contains(&a.id) && usage.bonus(&a.id) > 0).collect();
        used.sort_by(|a, b| usage.bonus(&b.id).cmp(&usage.bonus(&a.id)).then(a.name.cmp(&b.name)));
        for a in used.into_iter().take(MOST_USED) {
            self.frequent.append(&self.app_row(a, false));
        }
        if self.frequent.first_child().is_none() {
            self.frequent.append(&empty("the apps you open most show up here"));
        }
    }

    fn fill_all(self: &Rc<Self>) {
        clear(&self.all);
        let apps = self.apps.borrow();
        let pins = pins(&apps);
        let mut sorted: Vec<&App> = apps.iter().collect();
        sorted.sort_by_key(|a| a.name.to_lowercase());
        let mut letter = None;
        for a in sorted {
            let first = a.name.chars().next().map(|c| c.to_ascii_uppercase());
            if first != letter {
                letter = first;
                if let Some(c) = first {
                    let l = section(&c.to_string());
                    l.add_css_class("start-letter");
                    self.all.append(&l);
                }
            }
            self.all.append(&self.app_row(a, pins.contains(&a.id)));
        }
        self.all_built.set(true);
    }

    fn fill_recent(self: &Rc<Self>) {
        clear(&self.recent);
        let files = recent_files(12);
        if files.is_empty() {
            self.recent.append(&empty("files you open show up here"));
        }
        for (p, when) in files {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dir = p
                .parent()
                .map(|d| d.to_string_lossy().replace(&*util::home().to_string_lossy(), "~"))
                .unwrap_or_default();
            let path = p.clone();
            let r = self.button(&glyph(file_glyph(&p)), &name, &when, move || {
                util::launch(&["xdg-open", &path.to_string_lossy()])
            });
            r.set_tooltip_text(Some(&dir));
            self.recent.append(&r);
        }
    }

    /// A row that closes the menu and runs `f`.
    fn button(self: &Rc<Self>, icon: &impl IsA<gtk::Widget>, name: &str, meta: &str, f: impl Fn() + 'static) -> gtk::Button {
        let r = row(icon, name, meta);
        let w = Rc::downgrade(self);
        r.connect_clicked(move |_| {
            if let Some(s) = w.upgrade() {
                s.hide();
            }
            f();
        });
        r
    }

    fn tiles(self: &Rc<Self>, items: &'static [(&'static str, &'static str, &'static str)]) -> gtk::Grid {
        let g = gtk::Grid::new();
        g.add_css_class("start-tiles");
        g.set_column_homogeneous(true);
        for (i, &(gl, name, cmd)) in items.iter().enumerate() {
            let b = gtk::Button::new();
            b.add_css_class("start-tile");
            let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let ic = gtk::Label::new(Some(gl));
            ic.add_css_class("start-tile-glyph");
            let l = gtk::Label::new(Some(name));
            l.add_css_class("start-tile-label");
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            col.append(&ic);
            col.append(&l);
            b.set_child(Some(&col));
            b.set_tooltip_text(Some(name));
            let w = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(s) = w.upgrade() {
                    s.hide();
                }
                util::launch(&["sh", "-c", cmd]);
            });
            g.attach(&b, (i as u32 % TILE_COLS) as i32, (i as u32 / TILE_COLS) as i32, 1, 1);
        }
        g
    }

    fn app_row(self: &Rc<Self>, a: &App, pinned: bool) -> gtk::Button {
        let icon: gtk::Widget = match &a.icon {
            Some(i) => {
                let img = gtk::Image::from_gicon(i);
                img.set_pixel_size(24);
                img.add_css_class("start-appicon");
                img.upcast()
            }
            None => glyph("\u{f08c6}").upcast(),
        };
        let id = a.id.clone();
        let r = self.button(&icon, &a.name, "", move || apps::launch(&id, None));
        r.set_tooltip_text(Some(if pinned { "right click: unpin" } else { "right click: pin" }));
        // right click pins or unpins
        let id = a.id.clone();
        let w = Rc::downgrade(self);
        let rc = gtk::GestureClick::new();
        rc.set_button(3);
        rc.connect_released(move |_, _, _, _| {
            let Some(s) = w.upgrade() else { return };
            let mut p = pins(&s.apps.borrow());
            if let Some(i) = p.iter().position(|x| x == &id) {
                p.remove(i);
            } else {
                p.push(id.clone());
            }
            util::write_json(&util::state_dir().join(PINS), &p);
            s.fill_apps();
            if s.all_built.get() {
                s.fill_all();
            }
        });
        r.add_controller(rc);
        r
    }
}

const SETTINGS: &[(&str, &str, &str)] = &[
    ("\u{f05a9}", "wi-fi", "omarchy-launch-wifi"),
    ("\u{f00af}", "bluetooth", "omarchy-launch-bluetooth"),
    ("\u{f057e}", "sound", "omarchy-launch-audio"),
    ("\u{f030c}", "keys", "omarchy-menu-keybindings"),
    ("\u{f03d8}", "theme", "omarchy-menu theme"),
    ("\u{f0e09}", "wallpaper", "omarchy-theme-bg-next"),
    ("\u{f0b34}", "font", "omarchy-menu style"),
    ("\u{f0493}", "setup", "omarchy-menu setup"),
];

const SYSTEM: &[(&str, &str, &str)] = &[
    ("\u{f06b0}", "update", "omarchy-menu update"),
    ("\u{f0ab7}", "install", "omarchy-menu install"),
    ("\u{f05e8}", "remove", "omarchy-menu remove"),
    ("\u{f02fd}", "about", "omarchy-launch-about"),
];

const POWER: &[(&str, &str, &str, bool)] = &[
    ("\u{f033e}", "lock", "omarchy-system-lock", false),
    ("\u{f04b2}", "sleep", "systemctl suspend", false),
    ("\u{f0343}", "log out", "omarchy-system-logout", true),
    ("\u{f0709}", "restart", "omarchy-system-reboot", true),
    ("\u{f0425}", "shut down", "omarchy-system-shutdown", true),
];

/// Pinned apps: what was pinned, else the browser, terminal and files.
fn pins(apps: &[App]) -> Vec<String> {
    if let Some(p) = util::read_json::<Vec<String>>(&util::state_dir().join(PINS)) {
        return p;
    }
    let browser = std::process::Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let want = [browser.as_str(), "Alacritty.desktop", "io.github.not_bad_company.Vector.desktop"];
    want.iter()
        .filter(|w| !w.is_empty() && apps.iter().any(|a| &a.id == *w))
        .map(|w| w.to_string())
        .collect()
}

fn places() -> Vec<(&'static str, String, PathBuf)> {
    let home = util::home();
    let mut out = vec![("\u{f02dc}", "home".to_string(), home.clone())];
    for (dir, g, name) in [
        (glib::UserDirectory::Documents, "\u{f0219}", "documents"),
        (glib::UserDirectory::Downloads, "\u{f01da}", "downloads"),
        (glib::UserDirectory::Pictures, "\u{f02e9}", "pictures"),
        (glib::UserDirectory::Videos, "\u{f0567}", "videos"),
        (glib::UserDirectory::Music, "\u{f075a}", "music"),
    ] {
        if let Some(p) = glib::user_special_dir(dir).filter(|p| p.is_dir() && *p != home) {
            out.push((g, name.to_string(), p));
        }
    }
    let projects = home.join("Projects");
    if projects.is_dir() {
        out.push(("\u{f0b65}", "projects".to_string(), projects));
    }
    out
}

/// The newest existing files from the shared recently-used list, with when.
fn recent_files(n: usize) -> Vec<(PathBuf, String)> {
    let xbel = glib::user_data_dir().join("recently-used.xbel");
    let Ok(text) = std::fs::read_to_string(xbel) else { return vec![] };
    let mut seen: Vec<(String, PathBuf)> = Vec::new();
    for chunk in text.split("<bookmark ").skip(1) {
        let attr = |k: &str| {
            chunk.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::to_string)
        };
        let (Some(href), Some(when)) = (attr("href"), attr("visited").or_else(|| attr("modified"))) else {
            continue;
        };
        let Some(p) = href.strip_prefix("file://").map(|p| PathBuf::from(unescape(p))) else { continue };
        if p.is_file() && !seen.iter().any(|(_, q)| q == &p) {
            seen.push((when, p));
        }
    }
    seen.sort_by(|a, b| b.0.cmp(&a.0));
    seen.into_iter()
        .take(n)
        .map(|(when, p)| {
            let ago = glib::DateTime::from_iso8601(&when, None)
                .map(|d| util::age(d.to_unix() as f64))
                .unwrap_or_default();
            (p, ago)
        })
        .collect()
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn file_glyph(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "svg" | "avif" | "heic") => "\u{f021f}",
        Some("mp4" | "mkv" | "webm" | "mov") => "\u{f022b}",
        Some("mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a") => "\u{f0223}",
        Some("pdf") => "\u{f0226}",
        Some("xlsx" | "ods" | "csv") => "\u{f021b}",
        Some("docx" | "odt" | "md" | "txt") => "\u{f0219}",
        Some("zip" | "gz" | "xz" | "7z" | "tar" | "zst") => "\u{f05c4}",
        _ => "\u{f0214}",
    }
}

fn user_block() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    b.add_css_class("start-user");
    let user = glib::user_name().to_string_lossy().to_string();
    let real = glib::real_name().to_string_lossy().to_string();
    let name = if real.is_empty() || real == "Unknown" { user.clone() } else { real };
    let face = util::home().join(".face");
    let avatar: gtk::Widget = if face.is_file() {
        let p = gtk::Picture::for_filename(&face);
        p.set_size_request(44, 44);
        p.set_content_fit(gtk::ContentFit::Cover);
        p.upcast()
    } else {
        let l = gtk::Label::new(Some(&name.chars().next().unwrap_or('?').to_uppercase().to_string()));
        l.add_css_class("start-avatar");
        l.upcast()
    };
    b.append(&avatar);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.set_valign(gtk::Align::Center);
    let n = gtk::Label::new(Some(&name.to_lowercase()));
    n.add_css_class("start-name");
    n.set_xalign(0.0);
    let up = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .map(|u| {
            let m = (u / 60.0) as u64;
            if m < 60 { format!("up {m}m") } else { format!("up {}h {}m", m / 60, m % 60) }
        })
        .unwrap_or_default();
    let h = gtk::Label::new(Some(&format!("vectorOS · {up}")));
    h.add_css_class("start-host");
    h.set_xalign(0.0);
    col.append(&n);
    col.append(&h);
    b.append(&col);
    b
}

fn row(icon: &impl IsA<gtk::Widget>, name: &str, meta: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.add_css_class("start-row");
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    r.append(icon);
    let l = gtk::Label::new(Some(name));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    r.append(&l);
    if !meta.is_empty() {
        let m = gtk::Label::new(Some(meta));
        m.add_css_class("start-meta");
        r.append(&m);
    }
    b.set_child(Some(&r));
    b
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Never);
    s.set_child(Some(child));
    s.set_vexpand(true);
    s
}

fn first_button(w: &gtk::Widget) -> Option<gtk::Button> {
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        return Some(b.clone());
    }
    let mut c = w.first_child();
    while let Some(ch) = c {
        if let Some(b) = first_button(&ch) {
            return Some(b);
        }
        c = ch.next_sibling();
    }
    None
}

fn glyph(g: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(g));
    l.add_css_class("start-glyph");
    l
}

fn empty(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("start-empty");
    l.set_xalign(0.0);
    l
}

fn section(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("start-section");
    l.set_xalign(0.0);
    l
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}
