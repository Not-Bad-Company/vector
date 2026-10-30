//! Small widget helpers shared by the pages: the same look as the bar's panels
//! (square, mono, dim lowercase secondary text, accent only where it matters).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;

pub fn vbox(spacing: i32, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, spacing);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

pub fn hbox(spacing: i32, classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, spacing);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

pub fn text(s: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(s));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

pub fn markup(s: &str, classes: &[&str]) -> gtk::Label {
    let l = text("", classes);
    l.set_markup(s);
    l
}

pub fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

pub fn spacer() -> gtk::Box {
    let b = hbox(0, &[]);
    b.set_hexpand(true);
    b
}

/// A flat text button.
pub fn link(label: &str, classes: &[&str], f: impl Fn() + 'static) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("link");
    for c in classes {
        b.add_css_class(c);
    }
    b.set_valign(gtk::Align::Center);
    b.set_cursor_from_name(Some("pointer"));
    b.connect_clicked(move |_| f());
    b
}

/// A section heading above a group.
pub fn section(title: &str) -> gtk::Box {
    let b = hbox(8, &["sec"]);
    b.append(&text(title, &["sec-title"]));
    b
}

/// A bordered block of rows.
pub fn group() -> gtk::Box {
    vbox(0, &["group"])
}

/// key ··· value, for details.
pub fn kv(k: &str, v: &str) -> gtk::Box {
    let b = hbox(12, &["kv"]);
    let kl = text(k, &["k"]);
    kl.set_width_chars(14);
    kl.set_wrap(false);
    b.append(&kl);
    let vl = markup(v, &["v"]);
    vl.set_selectable(true);
    vl.set_hexpand(true);
    b.append(&vl);
    b
}

/// A list row: icon, name, meta on the right, then `trail` for controls.
pub struct Row {
    pub root: gtk::Box,
    pub line: gtk::Box,
    pub meta: gtk::Label,
    pub trail: gtk::Box,
}

impl Row {
    pub fn new(icon: &str, name: &str, meta: &str) -> Row {
        let root = vbox(0, &["srow"]);
        let line = hbox(10, &["line"]);
        let i = text(icon, &["icon"]);
        i.set_wrap(false);
        let n = text(name, &["name"]);
        n.set_wrap(false);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        n.set_hexpand(true);
        let m = markup(meta, &["meta"]);
        m.set_wrap(false);
        let trail = hbox(6, &[]);
        line.append(&i);
        line.append(&n);
        line.append(&m);
        line.append(&trail);
        root.append(&line);
        Row { root, line, meta: m, trail }
    }

    pub fn active(&self, on: bool) {
        if on {
            self.root.add_css_class("active");
        } else {
            self.root.remove_css_class("active");
        }
    }

    /// Click anywhere on the line (not on its buttons).
    pub fn on_click(&self, f: impl Fn() + 'static) {
        self.root.add_css_class("clickable");
        self.line.set_cursor_from_name(Some("pointer"));
        let g = gtk::GestureClick::new();
        g.connect_released(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            f();
        });
        self.line.add_controller(g);
    }

    /// A part under the line that shows extra controls (password, details).
    pub fn expand(&self) -> gtk::Box {
        let b = vbox(6, &["expand"]);
        self.root.append(&b);
        b
    }
}

pub fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// A switch that doesn't fire its handler when the code sets it.
#[derive(Clone)]
pub struct Switch {
    pub sw: gtk::Switch,
    quiet: Rc<std::cell::Cell<bool>>,
}

impl Switch {
    pub fn new(f: impl Fn(bool) + 'static) -> Switch {
        let sw = gtk::Switch::new();
        sw.set_valign(gtk::Align::Center);
        let quiet = Rc::new(std::cell::Cell::new(false));
        let q = quiet.clone();
        sw.connect_state_set(move |_, on| {
            if !q.get() {
                f(on);
            }
            glib::Propagation::Proceed
        });
        Switch { sw, quiet }
    }

    pub fn set(&self, on: bool) {
        self.quiet.set(true);
        self.sw.set_active(on);
        self.sw.set_state(on);
        self.quiet.set(false);
    }
}

/// A list that only rebuilds the rows whose signature changed, so a row being
/// typed into or clicked survives refreshes.
#[derive(Clone)]
pub struct Keyed {
    pub root: gtk::Box,
    rows: Rc<RefCell<Vec<(String, String, gtk::Widget)>>>,
}

pub type Item = (String, String, Box<dyn FnOnce() -> gtk::Widget>);

impl Keyed {
    pub fn new(classes: &[&str]) -> Keyed {
        Keyed {
            root: vbox(0, classes),
            rows: Rc::default(),
        }
    }

    pub fn sync(&self, items: Vec<Item>) {
        let mut old: HashMap<String, (String, gtk::Widget)> = self
            .rows
            .borrow_mut()
            .drain(..)
            .map(|(k, s, w)| (k, (s, w)))
            .collect();
        let mut new = Vec::new();
        for (k, sig, build) in items {
            let w = match old.remove(&k) {
                Some((s, w)) if s == sig => w,
                Some((_, w)) => {
                    self.root.remove(&w);
                    build()
                }
                None => build(),
            };
            new.push((k, sig, w));
        }
        for (_, (_, w)) in old {
            self.root.remove(&w);
        }
        // put every row in order (moves only the ones out of place)
        let mut prev: Option<gtk::Widget> = None;
        for (_, _, w) in &new {
            if w.parent().is_none() {
                self.root.insert_child_after(w, prev.as_ref());
            } else if w.prev_sibling() != prev {
                self.root.reorder_child_after(w, prev.as_ref());
            }
            prev = Some(w.clone());
        }
        self.root.set_visible(!new.is_empty());
        *self.rows.borrow_mut() = new;
    }
}

/// Text with a dimmed part.
pub fn dim(s: &str) -> String {
    format!("<span alpha=\"55%\">{}</span>", esc(s))
}

/// Runs a program without waiting (output dropped).
pub fn spawn(argv: &[&str]) {
    let _ = std::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Runs a program off the main thread and hands back (success, stdout, stderr).
pub async fn run(argv: Vec<String>) -> (bool, String, String) {
    let r = gio::spawn_blocking(move || {
        std::process::Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(std::process::Stdio::null())
            .output()
    })
    .await;
    match r {
        Ok(Ok(o)) => (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).to_string(),
            String::from_utf8_lossy(&o.stderr).to_string(),
        ),
        _ => (false, String::new(), "couldn't start".into()),
    }
}
