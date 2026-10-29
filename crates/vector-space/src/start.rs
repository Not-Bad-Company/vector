//! The Start menu (SUPER tap, or a click on the bar's mode block), in the
//! Vista layout: apps on the left (pinned, most used, all apps, and a search
//! field that hands the query to Vector Space), places, recent files,
//! settings and the system on the right, power along the bottom.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::apps::{self, App};
use crate::util::{self, Usage};

const PINS: &str = "start-pins.json";
const LEFT_W: i32 = 360;
const RIGHT_W: i32 = 300;
const MOST_USED: usize = 7;

pub struct Start {
    win: gtk::ApplicationWindow,
    entry: gtk::Entry,
    pages: gtk::Stack,
    pinned: gtk::Box,
    frequent: gtk::Box,
    all: gtk::Box,
    recent: gtk::Box,
    apps: RefCell<Vec<App>>,
    armed: RefCell<Option<String>>,
    built: Cell<bool>,
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
    if let Some(s) = START.with(|s| s.borrow().clone()) {
        s.hide();
    }
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
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.add_css_class("start");
    card.set_halign(gtk::Align::Start);
    card.set_valign(gtk::Align::Start);
    card.set_margin_top(34);
    card.set_margin_start(8);
    backdrop.append(&card);
    win.set_child(Some(&backdrop));

    let cols = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    card.append(&cols);

    // left: apps
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.add_css_class("start-left");
    left.set_size_request(LEFT_W, -1);
    let pinned = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let frequent = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let home = gtk::Box::new(gtk::Orientation::Vertical, 0);
    home.append(&pinned);
    home.append(&section("most used"));
    home.append(&frequent);
    let all = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let all_scroll = gtk::ScrolledWindow::new();
    all_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    all_scroll.set_child(Some(&all));
    all_scroll.set_vexpand(true);
    let pages = gtk::Stack::new();
    pages.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    pages.set_transition_duration(140);
    pages.add_named(&home, Some("home"));
    pages.add_named(&all_scroll, Some("all"));
    pages.set_vexpand(true);
    left.append(&pages);
    let more = row(&glyph("\u{f003b}"), "all apps", "›");
    more.add_css_class("start-more");
    left.append(&more);
    let entry = gtk::Entry::new();
    entry.add_css_class("start-search");
    entry.set_placeholder_text(Some("search apps, files, = calc, > convert…"));
    left.append(&entry);
    cols.append(&left);

    // right: you, places, recent, settings, system
    let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
    right.add_css_class("start-right");
    right.set_size_request(RIGHT_W, -1);
    right.append(&user_block());
    let recent = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let right_rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let rscroll = gtk::ScrolledWindow::new();
    rscroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    rscroll.set_propagate_natural_height(true);
    rscroll.set_child(Some(&right_rows));
    rscroll.set_vexpand(true);
    right.append(&rscroll);
    cols.append(&right);

    // power, along the bottom right
    let power = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    power.add_css_class("start-power");
    power.set_halign(gtk::Align::End);
    card.append(&power);

    let s = Rc::new(Start {
        win,
        entry,
        pages,
        pinned,
        frequent,
        all,
        recent,
        apps: RefCell::default(),
        armed: RefCell::default(),
        built: Cell::new(false),
    });

    // places, recent, settings and system rows (static, built once)
    right_rows.append(&section("places"));
    for (glyph_, name, path) in places() {
        let p = path.clone();
        right_rows.append(&s.button(&glyph(glyph_), &name, "", move || {
            util::launch(&["vector-find", &p.to_string_lossy()])
        }));
    }
    right_rows.append(&section("recent"));
    right_rows.append(&s.recent);
    right_rows.append(&section("settings"));
    for (g, name, cmd) in SETTINGS {
        right_rows.append(&s.button(&glyph(g), name, "", move || util::launch(&["sh", "-c", cmd])));
    }
    right_rows.append(&section("system"));
    for (g, name, cmd) in SYSTEM {
        right_rows.append(&s.button(&glyph(g), name, "", move || util::launch(&["sh", "-c", cmd])));
    }
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

    // all apps / back
    let w = Rc::downgrade(&s);
    let lbl = more.last_child().and_downcast::<gtk::Label>();
    let title = more.first_child().and_then(|c| c.next_sibling()).and_downcast::<gtk::Label>();
    let click = gtk::GestureClick::new();
    click.connect_released(move |_, _, _, _| {
        let Some(s) = w.upgrade() else { return };
        let to_all = s.pages.visible_child_name().as_deref() != Some("all");
        s.pages.set_visible_child_name(if to_all { "all" } else { "home" });
        if let (Some(t), Some(l)) = (&title, &lbl) {
            t.set_text(if to_all { "back" } else { "all apps" });
            l.set_text(if to_all { "‹" } else { "›" });
        }
    });
    more.add_controller(click);

    // typing searches: the query goes to Vector Space, which has every mode
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

    // Escape or a click outside closes
    let keys = gtk::EventControllerKey::new();
    let w = Rc::downgrade(&s);
    keys.connect_key_pressed(move |_, k, _, _| {
        if k == gdk::Key::Escape {
            if let Some(s) = w.upgrade() {
                s.hide();
            }
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    s.win.add_controller(keys);
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
    s
}

impl Start {
    fn show(self: &Rc<Self>) {
        crate::ui::get().map(|ui| ui.hide());
        if !self.built.get() {
            *self.apps.borrow_mut() = apps::load();
            self.built.set(true);
        }
        self.fill_apps();
        self.fill_recent();
        self.pages.set_visible_child_name("home");
        *self.armed.borrow_mut() = None;
        self.place();
        self.win.present();
        if std::env::var_os("VECTOR_SPACE_TEST").is_some() {
            eprintln!("start shown on {:?}", self.win.monitor().and_then(|m| m.connector()));
        }
        self.entry.grab_focus();
    }

    fn hide(&self) {
        if std::env::var_os("VECTOR_SPACE_TEST").is_some() && self.win.is_visible() {
            eprintln!("start hidden");
        }
        self.win.set_visible(false);
        // pick up apps installed meanwhile on the next open
        self.built.set(false);
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
                self.pinned.append(&self.app_row(a, true));
            }
        }
        // most used: launch counts, fading with time, pins left out
        let usage = Usage::load("launches.json");
        let mut used: Vec<&App> = apps.iter().filter(|a| !pins.contains(&a.id) && usage.bonus(&a.id) > 0).collect();
        used.sort_by(|a, b| usage.bonus(&b.id).cmp(&usage.bonus(&a.id)).then(a.name.cmp(&b.name)));
        for a in used.into_iter().take(MOST_USED) {
            self.frequent.append(&self.app_row(a, false));
        }
        if self.frequent.first_child().is_none() {
            let l = gtk::Label::new(Some("apps you open show up here"));
            l.add_css_class("start-empty");
            l.set_xalign(0.0);
            self.frequent.append(&l);
        }
        let mut sorted: Vec<&App> = apps.iter().collect();
        sorted.sort_by_key(|a| a.name.to_lowercase());
        let mut letter = None;
        for a in sorted {
            let first = a.name.chars().next().map(|c| c.to_ascii_uppercase());
            if first != letter {
                letter = first;
                if let Some(c) = first {
                    self.all.append(&section(&c.to_string()));
                }
            }
            self.all.append(&self.app_row(a, pins.contains(&a.id)));
        }
    }

    fn fill_recent(self: &Rc<Self>) {
        clear(&self.recent);
        let files = recent_files(5);
        if files.is_empty() {
            let l = gtk::Label::new(Some("nothing yet"));
            l.add_css_class("start-empty");
            l.set_xalign(0.0);
            self.recent.append(&l);
        }
        for p in files {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let path = p.clone();
            self.recent.append(&self.button(&glyph(file_glyph(&p)), &name, "", move || {
                util::launch(&["xdg-open", &path.to_string_lossy()])
            }));
        }
    }

    /// A clickable row that closes the menu and runs `f`.
    fn button(self: &Rc<Self>, icon: &impl IsA<gtk::Widget>, name: &str, meta: &str, f: impl Fn() + 'static) -> gtk::Box {
        let r = row(icon, name, meta);
        let w = Rc::downgrade(self);
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            if let Some(s) = w.upgrade() {
                s.hide();
            }
            f();
        });
        r.add_controller(click);
        r
    }

    fn app_row(self: &Rc<Self>, a: &App, pinned: bool) -> gtk::Box {
        let icon: gtk::Widget = match &a.icon {
            Some(i) => {
                let img = gtk::Image::from_gicon(i);
                img.set_pixel_size(22);
                img.upcast()
            }
            None => glyph("\u{f08c6}").upcast(),
        };
        let id = a.id.clone();
        let r = self.button(&icon, &a.name, if pinned { "\u{f0403}" } else { "" }, move || apps::launch(&id, None));
        r.set_tooltip_text(Some(if pinned { "right click: unpin" } else { "right click: pin to start" }));
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
        });
        r.add_controller(rc);
        r
    }
}

const SETTINGS: &[(&str, &str, &str)] = &[
    ("\u{f05a9}", "wi-fi", "omarchy-launch-wifi"),
    ("\u{f00af}", "bluetooth", "omarchy-launch-bluetooth"),
    ("\u{f057e}", "sound", "omarchy-launch-audio"),
    ("\u{f03d8}", "theme", "omarchy-menu theme"),
    ("\u{f0e09}", "background", "omarchy-theme-bg-next"),
    ("\u{f0b34}", "font", "omarchy-menu font"),
    ("\u{f030c}", "keybindings", "omarchy-menu-keybindings"),
];

const SYSTEM: &[(&str, &str, &str)] = &[
    ("\u{f0d6e}", "update", "omarchy-menu update"),
    ("\u{f0ab7}", "install", "omarchy-menu install"),
    ("\u{f0a7a}", "remove", "omarchy-menu remove"),
    ("\u{f0493}", "setup", "omarchy-menu setup"),
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

/// The newest existing files from the shared recently-used list.
fn recent_files(n: usize) -> Vec<PathBuf> {
    let xbel = glib::user_data_dir().join("recently-used.xbel");
    let Ok(text) = std::fs::read_to_string(xbel) else { return vec![] };
    let mut seen: Vec<(String, PathBuf)> = Vec::new();
    for chunk in text.split("<bookmark ").skip(1) {
        let attr = |k: &str| {
            chunk.split(&format!("{k}=\"")).nth(1).and_then(|r| r.split('"').next()).map(str::to_string)
        };
        let (Some(href), Some(when)) = (attr("href"), attr("modified").or_else(|| attr("visited"))) else {
            continue;
        };
        let Some(p) = href.strip_prefix("file://").map(|p| PathBuf::from(unescape(p))) else { continue };
        if p.is_file() {
            seen.push((when, p));
        }
    }
    seen.sort_by(|a, b| b.0.cmp(&a.0));
    seen.into_iter().map(|(_, p)| p).take(n).collect()
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
        Some("zip" | "gz" | "xz" | "7z" | "tar" | "zst") => "\u{f05c4}",
        _ => "\u{f0214}",
    }
}

fn user_block() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.add_css_class("start-user");
    let user = glib::user_name().to_string_lossy().to_string();
    let real = glib::real_name().to_string_lossy().to_string();
    let name = if real.is_empty() || real == "Unknown" { user.clone() } else { real };
    let face = util::home().join(".face");
    let avatar: gtk::Widget = if face.is_file() {
        let p = gtk::Picture::for_filename(&face);
        p.set_size_request(40, 40);
        p.set_content_fit(gtk::ContentFit::Cover);
        p.upcast()
    } else {
        let l = gtk::Label::new(Some(&name.chars().next().unwrap_or('?').to_uppercase().to_string()));
        l.add_css_class("start-avatar");
        l.upcast()
    };
    b.append(&avatar);
    let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
    col.set_valign(gtk::Align::Center);
    let n = gtk::Label::new(Some(&name.to_lowercase()));
    n.add_css_class("start-name");
    n.set_xalign(0.0);
    let h = gtk::Label::new(Some("vectorOS"));
    h.add_css_class("start-host");
    h.set_xalign(0.0);
    col.append(&n);
    col.append(&h);
    b.append(&col);
    b
}

fn row(icon: &impl IsA<gtk::Widget>, name: &str, meta: &str) -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    r.add_css_class("start-row");
    r.append(icon);
    let l = gtk::Label::new(Some(name));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    r.append(&l);
    let m = gtk::Label::new(Some(meta));
    m.add_css_class("start-meta");
    r.append(&m);
    r
}

fn glyph(g: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(g));
    l.add_css_class("start-glyph");
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
