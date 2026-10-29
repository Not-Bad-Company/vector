//! The mode block on the far left: the Vector mark and the system mode
//! (NORMAL, REC, DICTATE, SILENT, AWAKE, VM; several join with ·), in the
//! mode's colour. Click: the Vector menu (apps, capture, toggles, style,
//! system, session). Right: terminal.
//! The rarer Omarchy submenus (install, remove, setup...) stay reachable under "more".

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::Slot;
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

pub struct Menu {
    slot: Slot,
    theme: Rc<Theme>,
    /// detection lives in the indicators module; made here when it isn't on the bar
    _indicators: Option<super::indicators::Indicators>,
    listener: u64,
}

impl Drop for Menu {
    fn drop(&mut self) {
        crate::mode::changed().disconnect(self.listener);
    }
}

/// The active states, joined; empty when nothing is on (then only the mark shows).
fn label_text() -> String {
    crate::mode::active().iter().filter(|s| s.key != "normal").map(|s| s.label).collect::<Vec<_>>().join(" · ")
}

impl Menu {
    pub fn new(ctx: &Ctx) -> Menu {
        let slot = ctx.slot("mode", &["menu"]);
        slot.set_markup(&label_text());
        let s2 = slot.clone();
        let listener = crate::mode::changed().connect(move || {
            s2.set_markup(&label_text());
            s2.bump();
        });
        let on_bar = |n: &str| ctx.cfg.left.iter().chain(&ctx.cfg.center).chain(&ctx.cfg.right).any(|x| x == n);
        let _indicators = (!on_bar("indicators")).then(|| super::indicators::Indicators::new(ctx));
        let right = ctx.opts("menu").str("on_right", "xdg-terminal-exec");
        slot.on_right(move || util::spawn_sh(&right));
        slot.tooltip(|| {
            let s = ui::sheet();
            let states = crate::mode::active();
            s.append(&ui::head("vector", &states.iter().map(|x| x.label.to_lowercase()).collect::<Vec<_>>().join(" · ")).0);
            s.append(&ui::thint("click menu  ·  right terminal"));
            Some(s.upcast())
        });
        let theme = ctx.theme.clone();
        let t2 = theme.clone();
        slot.panel(move |p| menu_panel(p, &t2));
        Menu { slot, theme, _indicators, listener }
    }
}

impl Module for Menu {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        let w = self.slot.view();
        if let Some(b) = w.downcast_ref::<gtk::Box>() {
            b.prepend(&mark(&self.theme, 13).tap(|m| m.add_css_class("mark")));
        }
        w
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.slot.clone()]
    }
}

/// The Vector mark: two axes from an origin and the vector, drawn in the
/// widget's CSS colour (so it turns dark inside a mode block).
pub fn mark(_t: &Theme, size: i32) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(size);
    d.set_content_height(size);
    d.set_valign(gtk::Align::Center);
    d.set_draw_func(move |area, cr, w, h| {
        let c = area.color();
        let s = w.min(h) as f64;
        let u = s / 16.0;
        let (ox, oy) = (2.5 * u, 13.5 * u);
        cr.set_line_cap(gtk::cairo::LineCap::Square);
        cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * 0.7);
        cr.set_line_width(1.5 * u);
        cr.move_to(ox, 2.0 * u);
        cr.line_to(ox, oy);
        cr.line_to(14.5 * u, oy);
        let _ = cr.stroke();
        cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
        cr.set_line_width(2.2 * u);
        cr.move_to(ox, oy);
        cr.line_to(10.0 * u, 6.0 * u);
        let _ = cr.stroke();
        cr.move_to(14.0 * u, 2.0 * u);
        cr.line_to(7.6 * u, 3.8 * u);
        cr.line_to(12.2 * u, 8.4 * u);
        cr.close_path();
        let _ = cr.fill();
        cr.arc(ox, oy, 1.5 * u, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
    });
    d
}

pub fn action_row(p: &Panel, icon: &str, name: &str, meta: &str, cmd: &str) -> gtk::Box {
    let r = ui::Row::new(icon, name, meta);
    let cmd = cmd.to_string();
    let p = p.clone();
    ui::on_click(&r.root, move || {
        p.close();
        util::spawn_sh(&cmd);
    });
    r.root
}

/// A row that unfolds `content` under itself.
pub fn fold(icon: &str, name: &str, meta: &str, content: &gtk::Widget) -> gtk::Box {
    let b = ui::vbox(0, &[]);
    let r = ui::Row::new(icon, name, meta);
    let arrow = ui::text("", &["dim"]);
    r.trail.append(&arrow);
    let rev = gtk::Revealer::new();
    rev.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    rev.set_transition_duration(140);
    rev.set_child(Some(content));
    let rev2 = rev.clone();
    ui::on_click(&r.root, move || {
        let open = !rev2.reveals_child();
        rev2.set_reveal_child(open);
        arrow.set_text(if open { "" } else { "" });
    });
    b.append(&r.root);
    b.append(&rev);
    b
}

/// An icon button that needs a second click within 3 s (shut down etc).
pub fn confirm_button(icon: &str, tip: &str, f: impl Fn() + 'static) -> gtk::Button {
    let armed = Rc::new(Cell::new(false));
    let me: Rc<RefCell<Option<gtk::Button>>> = Rc::default();
    let m2 = me.clone();
    let tip_s = tip.to_string();
    let bt = ui::icon_button(icon, tip, move || {
        if !armed.get() {
            armed.set(true);
            if let Some(b) = m2.borrow().as_ref() {
                b.add_css_class("crit");
                b.set_tooltip_text(Some(&format!("{tip_s}: click again")));
            }
            let (armed, m3, tip_s) = (armed.clone(), m2.clone(), tip_s.clone());
            glib::timeout_add_local_once(Duration::from_secs(3), move || {
                armed.set(false);
                if let Some(b) = m3.borrow().as_ref() {
                    b.remove_css_class("crit");
                    b.set_tooltip_text(Some(&tip_s));
                }
            });
            return;
        }
        f();
    });
    *me.borrow_mut() = Some(bt.clone());
    bt
}

fn menu_panel(p: &Panel, t: &Rc<Theme>) -> gtk::Widget {
    p.set_width(330);
    let host = util::read_trim("/proc/sys/kernel/hostname").unwrap_or_default();
    let up = util::read("/proc/uptime")
        .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()))
        .map(|s| util::dur(s as u64))
        .unwrap_or_default();
    let card = Card::new("vector os", &format!("{host} · up {up}"));
    let _ = t;
    let b = &card.body;

    b.append(&action_row(p, "󰀻", "apps", "super space", "omarchy-launch-walker"));
    b.append(&action_row(p, "", "files", "", "vector-find"));
    b.append(&action_row(p, "", "terminal", "", "xdg-terminal-exec"));
    b.append(&action_row(p, "", "keybindings", "", "omarchy-menu-keybindings"));

    b.append(&ui::section("capture"));
    let cap = ui::hbox(0, &["chips"]);
    for (label, cmd) in [
        ("󰹑 shot", "omarchy-capture-screenshot"),
        ("󰻃 record", "omarchy-capture-screenrecording --with-desktop-audio"),
        ("󰴑 text", "omarchy-capture-text-extraction"),
        ("󰃉 color", "pkill hyprpicker || hyprpicker -a"),
    ] {
        let p = p.clone();
        cap.append(&ui::chip(label, false, move || {
            p.close();
            // let the panel go away before anything is captured
            glib::timeout_add_local_once(Duration::from_millis(180), move || util::spawn_sh(cmd));
        })
        .tap(|c| c.set_margin_end(4)));
    }
    b.append(&cap);

    b.append(&ui::section("toggles"));
    for (icon, name, cmd) in [
        ("󰔎", "nightlight", "omarchy-toggle-nightlight"),
        ("󱫖", "idle lock", "omarchy-toggle-idle"),
        ("󰂛", "do not disturb", "omarchy-toggle-notification-silencing"),
        ("󱄄", "screensaver", "omarchy-toggle-screensaver"),
        ("", "window gaps", "omarchy-hyprland-window-gaps-toggle"),
    ] {
        let r = ui::Row::new(icon, name, "");
        ui::on_click(&r.root, move || util::spawn_sh(cmd));
        b.append(&r.root);
    }

    b.append(&ui::section("style"));
    let themes = ui::vbox(0, &[]);
    let current = util::sh_out("omarchy-theme-current", Duration::from_millis(600)).unwrap_or_default();
    for name in util::sh_out("omarchy-theme-list", Duration::from_secs(1)).unwrap_or_default().lines() {
        let r = ui::Row::new(if name == current { "󰄬" } else { "" }, name, "");
        r.root.set_margin_start(22);
        r.set_active(name == current);
        let n = name.to_string();
        let pc = p.clone();
        ui::on_click(&r.root, move || {
            pc.close();
            util::spawn(&["omarchy-theme-set", &n]);
        });
        themes.append(&r.root);
    }
    b.append(&fold("󰸌", "theme", &current.to_lowercase(), themes.upcast_ref()));
    b.append(&action_row(p, "", "next background", "", "omarchy-theme-bg-next"));
    let glass_on = t.glass;
    let glass = ui::Row::new("󰂵", "glass", if glass_on { "on" } else { "off" });
    glass.set_active(glass_on);
    {
        let pc = p.clone();
        ui::on_click(&glass.root, move || {
            pc.close();
            util::spawn(&["desktop-glass", if glass_on { "off" } else { "on" }, "glass"]);
        });
    }
    b.append(&glass.root);

    b.append(&ui::section("system"));
    b.append(&action_row(p, "󰚰", "update", "", "omarchy-launch-floating-terminal-with-presentation omarchy-update"));
    b.append(&action_row(p, "", "setup", "", "omarchy-menu setup"));
    let more = ui::vbox(0, &[]);
    for (name, arg) in [("install", "install"), ("remove", "remove"), ("trigger", "trigger"), ("learn", "learn"), ("omarchy menu", "")] {
        let r = action_row(p, "", name, "", &format!("omarchy-menu {arg}"));
        r.set_margin_start(22);
        more.append(&r);
    }
    b.append(&fold("󰇘", "more", "", more.upcast_ref()));

    // session as icons in the footer; the destructive ones ask twice
    let session = ui::hbox(2, &[]);
    for (icon, tip, cmd, confirm) in [
        ("󰌾", "lock", "omarchy-system-lock", false),
        ("󰒲", "suspend", "systemctl suspend", false),
        ("󰍃", "log out", "omarchy-system-logout", true),
        ("󰜉", "restart", "omarchy-system-reboot", true),
        ("󰐥", "shut down", "omarchy-system-shutdown", true),
    ] {
        let pc = p.clone();
        let run = move || {
            pc.close();
            util::spawn_sh(cmd);
        };
        session.append(&if confirm { confirm_button(icon, tip, run) } else { ui::icon_button(icon, tip, run) });
    }
    card.foot_add(&ui::text("session", &["dim"]).tap(|l| {
        l.set_hexpand(true);
        l.set_margin_start(6);
    }));
    card.foot_add(&session);
    card.root.upcast()
}
