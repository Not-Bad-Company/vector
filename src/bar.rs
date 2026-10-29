//! One layer-shell bar window per monitor, laid out like nvim's lualine:
//! solid blocks in the mode colour, tinted segments next to them, plain
//! items in between, joined by slanted edges.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::gdk;
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::config::Config;
use crate::modules::{BarInfo, Module};
use crate::theme::Theme;
use crate::{mode, slot, ui};

pub type Section = Vec<(String, Rc<dyn Module>)>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Plain,
    Seg,
    Block,
}

fn rgba(hex: &str, alpha: f32) -> gdk::RGBA {
    let c = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK);
    gdk::RGBA::new(c.red(), c.green(), c.blue(), c.alpha() * alpha)
}

pub fn kind_color(t: &Theme, k: Kind) -> Option<gdk::RGBA> {
    match k {
        Kind::Plain => None,
        Kind::Seg => Some(rgba(&t.foreground, 0.13)),
        Kind::Block => Some(rgba(t.mode_color(mode::current()), 1.0)),
    }
}

/// A slanted edge between two zones. `right`: the edge of a right-side
/// zone (`/`), else of a left-side one (`\`). Both halves are painted here,
/// so translucent segments don't stack up.
pub fn slant(right: bool, before: Kind, after: Kind, t: &Rc<Theme>) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.add_css_class("slant");
    d.set_content_width(10);
    d.set_vexpand(true);
    let t = t.clone();
    d.set_draw_func(move |area, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // the triangle belongs to `before` on the left side, to `after` on the right
        let (tri, rest) = if right { (after, before) } else { (before, after) };
        let accent = area.has_css_class("accent-slant");
        let color = |k: Kind| if accent && k == Kind::Block { Some(rgba(&t.accent, 1.0)) } else { kind_color(&t, k) };
        let paint = |cr: &gtk::cairo::Context, k: Kind| match color(k) {
            Some(c) => {
                cr.set_source_color(&c);
                let _ = cr.fill();
            }
            None => cr.new_path(),
        };
        // the "rest" half
        if right {
            cr.move_to(0.0, 0.0);
            cr.line_to(w, 0.0);
            cr.line_to(0.0, h);
        } else {
            cr.move_to(w, 0.0);
            cr.line_to(w, h);
            cr.line_to(0.0, h);
        }
        cr.close_path();
        paint(cr, rest);
        if right {
            cr.move_to(w, 0.0);
            cr.line_to(w, h);
            cr.line_to(0.0, h);
        } else {
            cr.move_to(0.0, 0.0);
            cr.line_to(w, 0.0);
            cr.line_to(0.0, h);
        }
        cr.close_path();
        paint(cr, tri);
    });
    d
}

pub fn build(
    app: &gtk::Application,
    monitor: &gdk::Monitor,
    cfg: &Config,
    theme: &Rc<Theme>,
    sections: &[Section; 3],
    test: bool,
) -> gtk::ApplicationWindow {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("vbar");
    win.init_layer_shell();
    win.set_layer(Layer::Top);
    win.set_namespace(Some(if test { "vector-bar-test" } else { "vector-bar" }));
    win.set_monitor(Some(monitor));
    for e in [Edge::Top, Edge::Left, Edge::Right] {
        win.set_anchor(e, true);
    }
    if test {
        win.set_exclusive_zone(0);
    } else {
        win.auto_exclusive_zone_enable();
    }
    win.set_default_size(-1, cfg.height);

    let info = BarInfo { connector: monitor.connector().map(|s| s.to_string()).unwrap_or_default() };
    let kind = |name: &str| {
        if cfg.blocks.iter().any(|b| b == name) {
            Kind::Block
        } else if cfg.segments.iter().any(|b| b == name) {
            Kind::Seg
        } else {
            Kind::Plain
        }
    };
    let cb = gtk::CenterBox::new();
    cb.add_css_class("bar");
    let names = ["left", "center", "right"];
    let mut boxes = Vec::new();
    let mut slants = Vec::new();
    for (i, sec) in sections.iter().enumerate() {
        let b = ui::hbox(0, &[names[i]]);
        b.set_valign(gtk::Align::Fill);
        let right = i == 2;
        // group consecutive modules of the same kind into one zone
        let mut zones: Vec<(Kind, Vec<&Rc<dyn Module>>, Vec<&str>)> = Vec::new();
        for (name, m) in sec {
            let k = kind(name);
            match zones.last_mut() {
                Some((zk, ms, ns)) if *zk == k => {
                    ms.push(m);
                    ns.push(name);
                }
                _ => zones.push((k, vec![m], vec![name])),
            }
        }
        let mut prev = Kind::Plain;
        let n = zones.len();
        for (zi, (k, ms, ns)) in zones.into_iter().enumerate() {
            // edge into this zone (not at the screen edge)
            let at_edge = (i == 0 && zi == 0) || (i == 2 && zi == n);
            if prev != k && !at_edge {
                // left side leans `\`, right side `/`; the centre opens with `/` and closes with `\`
                let s = slant(right || (i == 1 && prev == Kind::Plain), prev, k, theme);
                slants.push(s.clone());
                b.append(&s);
            }
            let z = ui::hbox(0, &["zone"]);
            z.add_css_class(match k {
                Kind::Plain => "plain",
                Kind::Seg => "seg",
                Kind::Block => "block",
            });
            let dividers = i == 1 && k == Kind::Plain;
            for (j, m) in ms.iter().enumerate() {
                if dividers && j > 0 {
                    z.append(&ui::text("·", &["divider"]));
                }
                let w = m.view(&info);
                w.set_valign(gtk::Align::Fill);
                w.add_css_class(&format!("m-{}", ns[j].replace(':', "-")));
                z.append(&w);
            }
            b.append(&z);
            prev = k;
        }
        // trailing edge out of the last zone on the left/centre side
        if i < 2 && prev != Kind::Plain {
            let s = slant(false, prev, Kind::Plain, theme);
            slants.push(s.clone());
            b.append(&s);
        }
        boxes.push(b);
    }
    cb.set_start_widget(Some(&boxes[0]));
    cb.set_center_widget(Some(&boxes[1]));
    cb.set_end_widget(Some(&boxes[2]));
    win.set_child(Some(&cb));

    // the mode colours the window (blocks) and the slants
    let apply = {
        let win = win.downgrade();
        move || {
            let Some(win) = win.upgrade() else { return };
            for (k, _) in mode::COLORS {
                win.remove_css_class(&format!("mode-{k}"));
            }
            win.add_css_class(&format!("mode-{}", mode::current()));
            for s in &slants {
                s.queue_draw();
            }
        }
    };
    apply();
    let id = mode::changed().connect(apply);
    win.connect_destroy(move |_| mode::changed().disconnect(id));

    // centre dividers: only between modules that are actually showing
    let center = boxes[1].clone();
    fix_dividers(&center);
    let weak = center.downgrade();
    let id = slot::layout().connect(move || {
        if let Some(c) = weak.upgrade() {
            fix_dividers(&c);
        }
    });
    win.connect_destroy(move |_| slot::layout().disconnect(id));
    win
}

fn shown(w: &gtk::Widget) -> bool {
    if !w.is_visible() {
        return false;
    }
    if let Some(b) = w.downcast_ref::<gtk::Box>() {
        if b.has_css_class("item") {
            return true;
        }
        let mut c = b.first_child();
        while let Some(ch) = c {
            if shown(&ch) {
                return true;
            }
            c = ch.next_sibling();
        }
        return false;
    }
    true
}

fn fix_dividers(center: &gtk::Box) {
    let mut zc = center.first_child();
    while let Some(zone) = zc {
        zc = zone.next_sibling();
        let Some(z) = zone.downcast_ref::<gtk::Box>() else { continue };
        let mut seen = false;
        let mut pending: Option<gtk::Widget> = None;
        let mut c = z.first_child();
        while let Some(ch) = c {
            let next = ch.next_sibling();
            if ch.has_css_class("divider") {
                ch.set_visible(false);
                pending = Some(ch);
            } else if shown(&ch) {
                if seen {
                    if let Some(d) = pending.take() {
                        d.set_visible(true);
                    }
                }
                seen = true;
                pending = None;
            }
            c = next;
        }
    }
}
