//! `vector-space themes | backgrounds | unlocks`: pick by picture. A grid of
//! preview cards with a filter; the current one is marked; Enter or a click
//! applies it through Omarchy's own commands. Replaces walker's preview menus
//! (elephant's omarchythemes, omarchyBackgroundSelector, omarchyunlocks).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::util::{self, Fuzzy};

const COLS: u32 = 3;
const THUMB_W: i32 = 272;
const THUMB_H: i32 = 168;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Themes,
    Backgrounds,
    Unlocks,
}

impl Kind {
    pub fn parse(w: &str) -> Option<Kind> {
        Some(match w {
            "themes" | "theme" => Kind::Themes,
            "backgrounds" | "background" | "wallpapers" | "wallpaper" => Kind::Backgrounds,
            "unlocks" | "unlock" => Kind::Unlocks,
            _ => return None,
        })
    }

    fn title(self) -> &'static str {
        match self {
            Kind::Themes => "THEME",
            Kind::Backgrounds => "BACKGROUND",
            Kind::Unlocks => "UNLOCK",
        }
    }
}

struct Entry {
    name: String,
    preview: Option<PathBuf>,
    cmd: Vec<String>,
    current: bool,
}

struct Gallery {
    win: gtk::ApplicationWindow,
    mode: gtk::Label,
    filter: gtk::Entry,
    grid: gtk::FlowBox,
    count: gtk::Label,
    entries: RefCell<Vec<Entry>>,
    kind: Cell<Kind>,
}

thread_local! {
    static GALLERY: RefCell<Option<Rc<Gallery>>> = const { RefCell::new(None) };
}

pub fn open(app: &gtk::Application, kind: Kind) {
    let g = GALLERY.with(|g| g.borrow().clone()).unwrap_or_else(|| {
        let g = build(app);
        GALLERY.with(|x| *x.borrow_mut() = Some(g.clone()));
        g
    });
    if g.win.is_visible() && g.kind.get() == kind {
        g.win.set_visible(false);
        return;
    }
    g.show(kind);
}

fn build(app: &gtk::Application) -> Rc<Gallery> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("vspace");
    win.set_title(Some("Vector Gallery"));
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_namespace(Some("vector-gallery"));
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
    card.add_css_class("gallery");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Start);
    card.set_margin_top(120);
    backdrop.append(&card);
    win.set_child(Some(&backdrop));

    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    head.add_css_class("head");
    let mode = gtk::Label::new(None);
    mode.add_css_class("mode");
    mode.set_valign(gtk::Align::Fill);
    let filter = gtk::Entry::new();
    filter.set_hexpand(true);
    filter.set_placeholder_text(Some("filter"));
    let count = gtk::Label::new(None);
    count.add_css_class("count");
    head.append(&mode);
    head.append(&crate::ui::slant());
    head.append(&filter);
    head.append(&count);
    card.append(&head);

    let grid = gtk::FlowBox::new();
    grid.add_css_class("gallery-grid");
    grid.set_selection_mode(gtk::SelectionMode::Single);
    grid.set_homogeneous(true);
    grid.set_min_children_per_line(COLS);
    grid.set_max_children_per_line(COLS);
    grid.set_activate_on_single_click(true);
    grid.set_valign(gtk::Align::Start);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_child(Some(&grid));
    scroll.set_size_request(COLS as i32 * (THUMB_W + 20) + 24, 3 * (THUMB_H + 52));
    card.append(&scroll);

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.add_css_class("foot");
    let keys = gtk::Label::new(Some("⏎ apply   ←↑→↓ move   esc close"));
    keys.add_css_class("keys");
    keys.set_hexpand(true);
    keys.set_xalign(1.0);
    foot.append(&keys);
    card.append(&foot);

    let g = Rc::new(Gallery {
        win,
        mode,
        filter,
        grid,
        count,
        entries: RefCell::default(),
        kind: Cell::new(Kind::Themes),
    });

    let w = Rc::downgrade(&g);
    g.filter.connect_changed(move |_| {
        if let Some(g) = w.upgrade() {
            g.grid.invalidate_filter();
            g.select_first_visible();
        }
    });
    let w = Rc::downgrade(&g);
    g.grid.set_filter_func(move |child| {
        let Some(g) = w.upgrade() else { return true };
        let q = g.filter.text();
        if q.is_empty() {
            return true;
        }
        let entries = g.entries.borrow();
        let Some(e) = entries.get(child.index() as usize) else { return true };
        Fuzzy::new(&q).score(&e.name).is_some()
    });
    let w = Rc::downgrade(&g);
    g.grid.connect_child_activated(move |_, child| {
        if let Some(g) = w.upgrade() {
            g.apply(child.index() as usize);
        }
    });

    let keyc = gtk::EventControllerKey::new();
    keyc.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = Rc::downgrade(&g);
    keyc.connect_key_pressed(move |_, k, _, _| {
        let Some(g) = w.upgrade() else { return glib::Propagation::Proceed };
        match k {
            gdk::Key::Escape => g.win.set_visible(false),
            gdk::Key::Return | gdk::Key::KP_Enter => {
                if let Some(c) = g.grid.selected_children().first() {
                    g.apply(c.index() as usize);
                }
            }
            gdk::Key::Right => g.step(1),
            gdk::Key::Left => g.step(-1),
            gdk::Key::Down => g.step(COLS as i32),
            gdk::Key::Up => g.step(-(COLS as i32)),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    g.win.add_controller(keyc);

    let click = gtk::GestureClick::new();
    let w = Rc::downgrade(&g);
    let c2 = card.clone();
    click.connect_pressed(move |gest, _, x, y| {
        let (Some(g), Some(bd)) = (w.upgrade(), gest.widget()) else { return };
        if let Some(b) = c2.compute_bounds(&bd)
            && !b.contains_point(&gtk::graphene::Point::new(x as f32, y as f32))
        {
            g.win.set_visible(false);
        }
    });
    backdrop.add_controller(click);
    g
}

impl Gallery {
    fn show(self: &Rc<Self>, kind: Kind) {
        crate::start::hide();
        if let Some(ui) = crate::ui::get() {
            ui.hide();
        }
        self.kind.set(kind);
        self.mode.set_text(kind.title());
        let entries = match kind {
            Kind::Themes => themes(),
            Kind::Backgrounds => backgrounds(),
            Kind::Unlocks => unlocks(),
        };
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        for e in &entries {
            self.grid.insert(&card(e), -1);
        }
        self.count.set_text(&format!("{}", entries.len()));
        let current = entries.iter().position(|e| e.current);
        *self.entries.borrow_mut() = entries;
        self.filter.set_text("");
        self.place();
        self.win.present();
        self.filter.grab_focus();
        match current.and_then(|i| self.grid.child_at_index(i as i32)) {
            Some(c) => self.grid.select_child(&c),
            None => self.select_first_visible(),
        }
        // the cards fill in as thumbnails load
        if std::env::var_os("VECTOR_SPACE_TEST").is_some() {
            eprintln!("gallery {} items", self.entries.borrow().len());
        }
    }

    fn visible(&self) -> Vec<gtk::FlowBoxChild> {
        let mut out = Vec::new();
        let mut c = self.grid.first_child();
        while let Some(ch) = c {
            c = ch.next_sibling();
            if let Ok(fc) = ch.downcast::<gtk::FlowBoxChild>()
                && fc.is_child_visible()
            {
                out.push(fc);
            }
        }
        out
    }

    fn select_first_visible(&self) {
        if let Some(c) = self.visible().first() {
            self.grid.select_child(c);
        }
    }

    fn step(&self, by: i32) {
        let vis = self.visible();
        if vis.is_empty() {
            return;
        }
        let cur = self.grid.selected_children().first().and_then(|s| vis.iter().position(|c| c == s));
        let next = match cur {
            Some(i) => (i as i32 + by).clamp(0, vis.len() as i32 - 1) as usize,
            None => 0,
        };
        self.grid.select_child(&vis[next]);
        // keep it in view
        vis[next].grab_focus();
        self.filter.grab_focus_without_selecting();
    }

    fn apply(&self, i: usize) {
        let Some(cmd) = self.entries.borrow().get(i).map(|e| e.cmd.clone()) else { return };
        self.win.set_visible(false);
        let argv: Vec<&str> = cmd.iter().map(String::as_str).collect();
        util::launch(&argv);
    }

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
}

fn card(e: &Entry) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    b.add_css_class("gallery-card");
    if e.current {
        b.add_css_class("current");
    }
    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_size_request(THUMB_W, THUMB_H);
    pic.add_css_class("gallery-thumb");
    if let Some(p) = &e.preview {
        let pw = pic.downgrade();
        crate::ui::thumb(p, THUMB_W * 2, move |t| {
            if let (Some(pic), Some(t)) = (pw.upgrade(), t) {
                pic.set_paintable(Some(&t));
            }
        });
    }
    b.append(&pic);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("gallery-name");
    let l = gtk::Label::new(Some(&e.name));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&l);
    if e.current {
        let m = gtk::Label::new(Some("current"));
        m.add_css_class("gallery-current");
        row.append(&m);
    }
    b.append(&row);
    b
}

fn omarchy_path() -> PathBuf {
    std::env::var_os("OMARCHY_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home().join(".local/share/omarchy"))
}

fn current_theme() -> String {
    std::fs::read_to_string(util::home().join(".config/omarchy/current/theme.name"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Theme dirs: the user's first, then Omarchy's (names seen once).
fn theme_dirs() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for root in [util::home().join(".config/omarchy/themes"), omarchy_path().join("themes")] {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !out.iter().any(|(n, _)| n == &name) {
                out.push((name, d));
            }
        }
    }
    out
}

fn pretty(name: &str) -> String {
    name.replace(['-', '_'], " ")
}

fn images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(),
                Some("png" | "jpg" | "jpeg" | "webp" | "avif")
            )
        })
        .collect();
    v.sort();
    v
}

fn themes() -> Vec<Entry> {
    let cur = current_theme();
    let default = omarchy_path().join("themes");
    theme_dirs()
        .into_iter()
        .map(|(name, dir)| {
            let find = |d: &Path| {
                ["preview.png", "preview.jpg"]
                    .iter()
                    .map(|f| d.join(f))
                    .find(|p| p.is_file())
                    .or_else(|| images(&d.join("backgrounds")).into_iter().next())
            };
            Entry {
                preview: find(&dir).or_else(|| find(&default.join(&name))),
                cmd: vec!["omarchy-theme-set".into(), name.clone()],
                current: name == cur,
                name: pretty(&name),
            }
        })
        .collect()
}

fn backgrounds() -> Vec<Entry> {
    let cur = std::fs::canonicalize(util::home().join(".config/omarchy/current/background")).ok();
    let mut dirs = vec![util::home().join(".config/omarchy/current/theme/backgrounds")];
    let theme = current_theme();
    if !theme.is_empty() {
        dirs.push(util::home().join(".config/omarchy/backgrounds").join(&theme));
    }
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for d in dirs {
        for p in images(&d) {
            let real = std::fs::canonicalize(&p).unwrap_or(p.clone());
            if seen.contains(&real) {
                continue;
            }
            seen.push(real.clone());
            let name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            out.push(Entry {
                name: pretty(name.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-')),
                preview: Some(p.clone()),
                cmd: vec!["omarchy-theme-bg-set".into(), p.to_string_lossy().to_string()],
                current: cur.as_ref() == Some(&real),
            });
        }
    }
    out
}

fn unlocks() -> Vec<Entry> {
    let mut out: Vec<Entry> = theme_dirs()
        .into_iter()
        .filter_map(|(name, dir)| {
            let p = dir.join("preview-unlock.png");
            p.is_file().then(|| Entry {
                name: pretty(&name),
                preview: Some(p),
                cmd: vec![
                    "omarchy-launch-floating-terminal-with-presentation".into(),
                    format!("omarchy-plymouth-set-by-theme '{name}'"),
                ],
                current: false,
            })
        })
        .collect();
    let def = omarchy_path().join("default/plymouth/preview-unlock.png");
    out.push(Entry {
        name: "default".into(),
        preview: def.is_file().then_some(def),
        cmd: vec!["omarchy-launch-floating-terminal-with-presentation".into(), "omarchy-plymouth-reset".into()],
        current: false,
    });
    out
}
