//! Keyboard & touchpad: the `input { }` block of ~/.config/hypr/input.conf,
//! changed in place (comments and everything else stay). Hyprland reloads
//! the file by itself; sliders write once they settle.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use crate::conf::{self, Conf};
use crate::page::{Ctx, Frame, View};
use crate::ui;

fn file() -> std::path::PathBuf {
    conf::hypr_dir().join("input.conf")
}

struct Input {
    frame: Frame,
    body: gtk::Box,
    status: gtk::Label,
    /// pending writes: (path, key, value), flushed after the user stops moving
    pending: RefCell<Vec<(Vec<&'static str>, &'static str, String)>>,
    timer: Cell<Option<glib::SourceId>>,
}

pub fn page(_: &Ctx) -> View {
    let frame = Frame::new("keyboard & touchpad");
    frame.detail.set_markup(&ui::dim("~/.config/hypr/input.conf"));
    let body = ui::vbox(0, &[]);
    frame.body.append(&body);
    let status = ui::text("", &["faint"]);
    frame.foot_add(&status);
    frame.foot_add(&ui::spacer());
    frame.foot_add(&ui::link("open the file", &[], || {
        let f = file();
        ui::spawn(&["uwsm-app", "--", "omarchy-launch-editor", &f.to_string_lossy()]);
    }));
    let p = Rc::new(Input {
        frame,
        body,
        status,
        pending: RefCell::default(),
        timer: Cell::new(None),
    });
    let root = p.frame.root.clone();
    let a = p.clone();
    View::new(&root).on_show(move || a.draw())
}

impl Input {
    fn set(self: &Rc<Self>, path: &[&'static str], key: &'static str, value: String, delay: u64) {
        {
            let mut pend = self.pending.borrow_mut();
            pend.retain(|(p, k, _)| !(p == path && *k == key));
            pend.push((path.to_vec(), key, value));
        }
        if let Some(id) = self.timer.take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        self.timer.set(Some(glib::timeout_add_local_once(Duration::from_millis(delay), move || {
            if let Some(w) = w.upgrade() {
                w.timer.set(None);
                w.flush();
            }
        })));
    }

    fn flush(&self) {
        let pend = std::mem::take(&mut *self.pending.borrow_mut());
        if pend.is_empty() {
            return;
        }
        let r = conf::edit(&file(), |c| {
            for (p, k, v) in &pend {
                c.set(p, k, v);
            }
        });
        match r {
            Ok(()) => {
                self.status.set_text("saved · Hyprland applied it");
                self.status.remove_css_class("crit");
            }
            Err(e) => {
                self.status.set_text(&format!("couldn't save: {e}"));
                self.status.add_css_class("crit");
            }
        }
    }

    fn draw(self: &Rc<Self>) {
        ui::clear(&self.body);
        let text = std::fs::read_to_string(file()).unwrap_or_default();
        let c = Conf::parse(&text);
        let get = |p: &[&str], k: &str, d: &str| c.get(p, k).unwrap_or_else(|| d.to_string());
        let on = |v: String| matches!(v.as_str(), "true" | "1" | "yes" | "on");
        const IN: &[&str] = &["input"];
        const TP: &[&str] = &["input", "touchpad"];

        // keyboard
        self.body.append(&ui::section("keyboard"));
        let g = ui::group();
        {
            let line = ui::hbox(8, &["kv"]);
            let k = ui::text("layouts", &["k"]);
            k.set_width_chars(16);
            line.append(&k);
            let e = gtk::Entry::builder().text(get(IN, "kb_layout", "us")).hexpand(true).build();
            e.set_tooltip_text(Some("comma separated, e.g. us,nl (switch with the key below)"));
            let w = Rc::downgrade(self);
            e.connect_changed(move |e| {
                let t: String = e.text().chars().filter(|c| !c.is_whitespace()).collect();
                if let (Some(w), false) = (w.upgrade(), t.is_empty()) {
                    w.set(IN, "kb_layout", t, 900);
                }
            });
            line.append(&e);
            g.append(&line);
        }
        let opts = get(IN, "kb_options", "");
        let mut parts: Vec<String> = opts.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
        // compose key
        {
            let choices = [("", "none"), ("compose:caps", "caps lock"), ("compose:ralt", "right alt"), ("compose:menu", "menu key"), ("compose:rctrl", "right ctrl")];
            let cur = parts.iter().find(|p| p.starts_with("compose:")).cloned().unwrap_or_default();
            let others: Vec<String> = parts.iter().filter(|p| !p.starts_with("compose:")).cloned().collect();
            g.append(&self.choice("compose key", &choices, &cur, {
                let w = Rc::downgrade(self);
                move |v| {
                    let mut all = others.clone();
                    if !v.is_empty() {
                        all.insert(0, v.to_string());
                    }
                    if let Some(w) = w.upgrade() {
                        w.set(IN, "kb_options", all.join(","), 50);
                    }
                }
            }));
        }
        // layout switch key
        {
            let choices = [("", "none"), ("grp:alts_toggle", "both alt keys"), ("grp:alt_shift_toggle", "alt + shift"), ("grp:win_space_toggle", "super + space"), ("grp:caps_toggle", "caps lock")];
            let cur = parts.iter().find(|p| p.starts_with("grp:")).cloned().unwrap_or_default();
            parts.retain(|p| !p.starts_with("grp:"));
            g.append(&self.choice("switch layout", &choices, &cur, {
                let w = Rc::downgrade(self);
                let base = parts.clone();
                move |v| {
                    let mut all = base.clone();
                    if !v.is_empty() {
                        all.push(v.to_string());
                    }
                    if let Some(w) = w.upgrade() {
                        w.set(IN, "kb_options", all.join(","), 50);
                    }
                }
            }));
        }
        g.append(&self.slider("repeat rate", IN, "repeat_rate", get(IN, "repeat_rate", "25").parse().unwrap_or(25.0), 10.0, 80.0, 1.0, "/s"));
        g.append(&self.slider("repeat delay", IN, "repeat_delay", get(IN, "repeat_delay", "600").parse().unwrap_or(600.0), 150.0, 1000.0, 10.0, " ms"));
        g.append(&self.toggle("num lock on at start", IN, "numlock_by_default", on(get(IN, "numlock_by_default", "false"))));
        let test = gtk::Entry::builder().placeholder_text("try the keyboard here").hexpand(true).build();
        let tl = ui::hbox(8, &["kv"]);
        let k = ui::text("", &["k"]);
        k.set_width_chars(16);
        tl.append(&k);
        tl.append(&test);
        g.append(&tl);
        self.body.append(&g);

        // touchpad
        self.body.append(&ui::section("touchpad"));
        let g = ui::group();
        g.append(&self.toggle("natural scrolling", TP, "natural_scroll", on(get(TP, "natural_scroll", "false"))));
        g.append(&self.toggle("tap to click", TP, "tap-to-click", on(get(TP, "tap-to-click", "true"))));
        g.append(&self.toggle("two-finger right click", TP, "clickfinger_behavior", on(get(TP, "clickfinger_behavior", "false"))));
        g.append(&self.toggle("off while typing", TP, "disable_while_typing", on(get(TP, "disable_while_typing", "true"))));
        g.append(&self.slider("scroll speed", TP, "scroll_factor", get(TP, "scroll_factor", "1.0").parse().unwrap_or(1.0), 0.1, 2.0, 0.05, "×"));
        self.body.append(&g);

        // pointer
        self.body.append(&ui::section("pointer"));
        let g = ui::group();
        g.append(&self.slider("speed", IN, "sensitivity", get(IN, "sensitivity", "0").parse().unwrap_or(0.0), -1.0, 1.0, 0.05, ""));
        let accel = get(IN, "accel_profile", "");
        g.append(&self.choice("acceleration", &[("", "default"), ("adaptive", "adaptive"), ("flat", "flat (none)")], &accel, {
            let w = Rc::downgrade(self);
            move |v| {
                if let Some(w) = w.upgrade() {
                    w.set(IN, "accel_profile", v.to_string(), 50);
                }
            }
        }));
        g.append(&self.toggle("natural scrolling (mouse)", IN, "natural_scroll", on(get(IN, "natural_scroll", "false"))));
        self.body.append(&g);
    }

    fn label_cell(name: &str) -> gtk::Label {
        let k = ui::text(name, &["k"]);
        k.set_width_chars(16);
        k.set_wrap(false);
        k
    }

    fn toggle(self: &Rc<Self>, name: &str, path: &'static [&'static str], key: &'static str, on: bool) -> gtk::Box {
        let line = ui::hbox(8, &["kv"]);
        let l = Self::label_cell(name);
        l.set_hexpand(true);
        line.append(&l);
        let w = Rc::downgrade(self);
        let sw = ui::Switch::new(move |v| {
            if let Some(w) = w.upgrade() {
                w.set(path, key, v.to_string(), 50);
            }
        });
        sw.set(on);
        line.append(&sw.sw);
        line
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(self: &Rc<Self>, name: &str, path: &'static [&'static str], key: &'static str, v: f64, min: f64, max: f64, step: f64, unit: &'static str) -> gtk::Box {
        let line = ui::hbox(12, &["kv"]);
        line.append(&Self::label_cell(name));
        let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
        s.set_value(v);
        s.set_hexpand(true);
        s.set_draw_value(false);
        let digits = if step < 1.0 { 2 } else { 0 };
        let fmt = move |x: f64| format!("{x:.digits$}{unit}");
        let val = ui::text(&fmt(v), &["dim"]);
        val.set_width_chars(8);
        val.set_xalign(1.0);
        val.set_wrap(false);
        let w = Rc::downgrade(self);
        let v2 = val.clone();
        s.connect_value_changed(move |s| {
            let x = (s.value() / step).round() * step;
            v2.set_text(&fmt(x));
            if let Some(w) = w.upgrade() {
                w.set(path, key, format!("{x:.digits$}"), 600);
            }
        });
        line.append(&s);
        line.append(&val);
        line
    }

    fn choice(self: &Rc<Self>, name: &str, choices: &[(&'static str, &'static str)], cur: &str, f: impl Fn(&str) + 'static) -> gtk::Box {
        let line = ui::hbox(8, &["kv"]);
        let l = Self::label_cell(name);
        l.set_hexpand(true);
        line.append(&l);
        let labels: Vec<&str> = choices.iter().map(|c| c.1).collect();
        let dd = gtk::DropDown::from_strings(&labels);
        let i = choices.iter().position(|c| c.0 == cur).unwrap_or(0);
        dd.set_selected(i as u32);
        let values: Vec<&'static str> = choices.iter().map(|c| c.0).collect();
        dd.connect_selected_notify(move |d| {
            if let Some(v) = values.get(d.selected() as usize) {
                f(v);
            }
        });
        line.append(&dd);
        line
    }
}
