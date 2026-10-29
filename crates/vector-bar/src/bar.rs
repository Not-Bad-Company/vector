//! One layer-shell bar window per monitor, laid out like nvim's lualine:
//! solid blocks in the mode colour, tinted segments next to them, plain
//! items in between, joined by slanted edges.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, Layer, LayerShell};

use crate::config::Config;
use crate::modules::{BarInfo, Module};
use crate::theme::Theme;
use crate::{mode, slot, ui};

/// One zone of a section: its kind and the modules and names in it.
type Zone<'a> = (Kind, Vec<&'a Rc<dyn Module>>, Vec<&'a str>);

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

/// The block item beside a slant, for its hover and open tint. Filled in by `attach`.
pub type Neighbour = Rc<RefCell<Option<gtk::glib::WeakRef<gtk::Widget>>>>;

/// Outline of one half of a slant: the triangle (`tri`) or the rest, in a `w` x `h` box.
fn half(cr: &gtk::cairo::Context, w: f64, h: f64, right: bool, tri: bool) {
    let pts = match (right, tri) {
        (true, true) => [(w, 0.0), (w, h), (0.0, h)],
        (true, false) => [(0.0, 0.0), (w, 0.0), (0.0, h)],
        (false, true) => [(0.0, 0.0), (w, 0.0), (0.0, h)],
        (false, false) => [(w, 0.0), (w, h), (0.0, h)],
    };
    cr.move_to(pts[0].0, pts[0].1);
    cr.line_to(pts[1].0, pts[1].1);
    cr.line_to(pts[2].0, pts[2].1);
    cr.close_path();
}

/// A slanted edge between two zones. `right`: the edge of a right-side
/// zone (`/`), else of a left-side one (`\`). Both halves are painted here,
/// so translucent segments don't stack up. `nb` is the block item beside it, if any.
pub fn slant(
    right: bool,
    before: Kind,
    after: Kind,
    t: &Rc<Theme>,
    nb: Neighbour,
) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.add_css_class("slant");
    d.set_content_width(10);
    d.set_vexpand(true);
    let t = t.clone();
    d.set_draw_func(move |area, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        // the triangle belongs to `before` on the left side, to `after` on the right
        let (tri, rest) = if right {
            (after, before)
        } else {
            (before, after)
        };
        let accent = area.has_css_class("accent-slant");
        let color = |k: Kind| {
            if accent && k == Kind::Block {
                Some(rgba(&t.accent, 1.0))
            } else {
                kind_color(&t, k)
            }
        };
        let paint = |cr: &gtk::cairo::Context, k: Kind, is_tri: bool| {
            half(cr, w, h, right, is_tri);
            match color(k) {
                Some(c) => {
                    cr.set_source_color(&c);
                    let _ = cr.fill();
                }
                None => cr.new_path(),
            }
        };
        paint(cr, rest, false);
        paint(cr, tri, true);
        // a hovered or open block item tints its half of the slant too, as the CSS tints the item
        let item = nb.borrow().as_ref().and_then(|r| r.upgrade());
        if let Some(item) = item {
            let open = item.has_css_class("open");
            let hover = (item.has_css_class("slant-hover")
                || item.state_flags().contains(gtk::StateFlags::PRELIGHT))
                && item.has_css_class("clickable");
            let alpha = if open {
                0.20
            } else if hover {
                0.14
            } else {
                0.0
            };
            if alpha > 0.0 {
                half(cr, w, h, right, tri == Kind::Block);
                cr.set_source_color(&rgba(&t.background, alpha));
                let _ = cr.fill();
            }
        }
    });
    d
}

/// Links a slant to the block item beside it, so the block and its diagonal
/// edge hover and click as one shape without any frame lag.
pub fn attach(s: &gtk::DrawingArea, nb: &Neighbour, item: &gtk::Widget) {
    *nb.borrow_mut() = Some(item.downgrade());
    if item.has_css_class("clickable") {
        ui::pointer(s);
    }
    let in_item = Rc::new(std::cell::Cell::new(false));
    let in_slant = Rc::new(std::cell::Cell::new(false));
    let sync = {
        let s = s.downgrade();
        let item = item.downgrade();
        let in_item = in_item.clone();
        let in_slant = in_slant.clone();
        Rc::new(move || {
            let hov = in_item.get() || in_slant.get();
            if let Some(it) = item.upgrade() {
                if hov {
                    it.add_css_class("slant-hover");
                } else {
                    it.remove_css_class("slant-hover");
                }
                it.queue_draw();
            }
            if let Some(s) = s.upgrade() {
                s.queue_draw();
            }
        })
    };
    let m_item = gtk::EventControllerMotion::new();
    {
        let (in_item, sync) = (in_item.clone(), sync.clone());
        m_item.connect_enter(move |_, _, _| {
            in_item.set(true);
            sync();
        });
    }
    {
        let (in_item, sync) = (in_item.clone(), sync.clone());
        m_item.connect_leave(move |_| {
            in_item.set(false);
            sync();
        });
    }
    item.add_controller(m_item);

    let m_slant = gtk::EventControllerMotion::new();
    {
        let (in_slant, sync) = (in_slant.clone(), sync.clone());
        m_slant.connect_enter(move |_, _, _| {
            in_slant.set(true);
            sync();
        });
    }
    {
        let (in_slant, sync) = (in_slant.clone(), sync.clone());
        m_slant.connect_leave(move |_| {
            in_slant.set(false);
            sync();
        });
    }
    s.add_controller(m_slant);

    let click = gtk::GestureClick::new();
    click.set_button(0);
    let item_w = item.downgrade();
    click.connect_released(move |g, _, _, _| {
        if let Some(it) = item_w.upgrade() {
            slot::click_at(&it, g.current_button());
        }
    });
    s.add_controller(click);

    let weak = s.downgrade();
    let w2 = weak.clone();
    item.connect_state_flags_changed(move |_, _| {
        if let Some(s) = weak.upgrade() {
            s.queue_draw();
        }
    });
    item.connect_notify_local(Some("css-classes"), move |_, _| {
        if let Some(s) = w2.upgrade() {
            s.queue_draw();
        }
    });
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
    win.set_namespace(Some(if test {
        "vector-bar-test"
    } else {
        "vector-bar"
    }));
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

    let info = BarInfo {
        connector: monitor
            .connector()
            .map(|s| s.to_string())
            .unwrap_or_default(),
    };
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
        let mut zones: Vec<Zone<'_>> = Vec::new();
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
        // last item of the previous zone, for a slant that follows a block
        let mut prev_last: Option<gtk::Widget> = None;
        let n = zones.len();
        for (zi, (k, ms, ns)) in zones.into_iter().enumerate() {
            // edge into this zone (not at the screen edge)
            let at_edge = (i == 0 && zi == 0) || (i == 2 && zi == n);
            let mut lead = None;
            if prev != k && !at_edge {
                // left side leans `\`, right side `/`; the centre opens with `/` and closes with `\`
                let nb = Neighbour::default();
                let s = slant(
                    right || (i == 1 && prev == Kind::Plain),
                    prev,
                    k,
                    theme,
                    nb.clone(),
                );
                if let (Kind::Block, Some(w)) = (prev, &prev_last) {
                    attach(&s, &nb, w);
                }
                slants.push(s.clone());
                b.append(&s);
                lead = Some((s, nb));
            }
            let z = ui::hbox(0, &["zone"]);
            z.add_css_class(match k {
                Kind::Plain => "plain",
                Kind::Seg => "seg",
                Kind::Block => "block",
            });
            let dividers = i == 1 && k == Kind::Plain;
            let mut first: Option<gtk::Widget> = None;
            let mut last: Option<gtk::Widget> = None;
            for (j, m) in ms.iter().enumerate() {
                if dividers && j > 0 {
                    z.append(&ui::text("·", &["divider"]));
                }
                let w = m.view(&info);
                w.set_valign(gtk::Align::Fill);
                w.add_css_class(&format!("m-{}", ns[j].replace(':', "-")));
                z.append(&w);
                first.get_or_insert_with(|| w.clone());
                last = Some(w);
            }
            // a block zone's edge item is the neighbour of the slant leading into it
            if let (Kind::Block, Some((s, nb)), Some(w)) = (k, &lead, &first) {
                attach(s, nb, w);
            }
            b.append(&z);
            prev = k;
            prev_last = last;
        }
        // trailing edge out of the last zone on the left/centre side
        if i < 2 && prev != Kind::Plain {
            let nb = Neighbour::default();
            let s = slant(false, prev, Kind::Plain, theme, nb.clone());
            if let (Kind::Block, Some(w)) = (prev, &prev_last) {
                attach(&s, &nb, w);
            }
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
        let Some(z) = zone.downcast_ref::<gtk::Box>() else {
            continue;
        };
        let mut seen = false;
        let mut pending: Option<gtk::Widget> = None;
        let mut c = z.first_child();
        while let Some(ch) = c {
            let next = ch.next_sibling();
            if ch.has_css_class("divider") {
                ch.set_visible(false);
                pending = Some(ch);
            } else if shown(&ch) {
                if seen && let Some(d) = pending.take() {
                    d.set_visible(true);
                }
                seen = true;
                pending = None;
            }
            c = next;
        }
    }
}
