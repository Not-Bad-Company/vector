//! The frozen-screen overlay: a layer-shell surface on one output showing the
//! grab taken the instant the key was pressed. The user drags a region, clicks a
//! window or an output, or (colour picker) points at a pixel. The callback gets
//! one outcome; the overlay then goes away.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::capture::{Monitor, WinRect};

/// What the overlay lets the user pick.
#[derive(Clone)]
pub enum Kind {
    /// Drag a rectangle.
    Region,
    /// Click (or Enter on) the window under the pointer, or drag a rectangle.
    Window(Vec<WinRect>),
    /// Click anywhere, or Enter, for the whole output.
    Output,
}

/// The result, in the output's local logical coordinates.
#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Cancel,
    Rect { x: f64, y: f64, w: f64, h: f64 },
}

/// The frozen grab and the output it was taken from.
#[derive(Clone)]
pub struct Frozen {
    pub pixbuf: Pixbuf,
    pub mon: Monitor,
}

type Done = Box<dyn FnOnce(Outcome, &Frozen)>;

struct State {
    frozen: Frozen,
    kind: Kind,
    accent: (f64, f64, f64),
    drag: Option<((f64, f64), (f64, f64))>,
    pointer: Option<(f64, f64)>,
    hover: Option<usize>,
    done: Option<Done>,
    /// Keeps the layer window alive while the overlay is up.
    window: Option<gtk::Window>,
    shown_at: Option<std::time::Instant>,
}

/// Shows the overlay on the output of `frozen`. `done` runs once with the outcome.
pub fn run(app: &gtk::Application, frozen: Frozen, kind: Kind, accent: &str, done: impl FnOnce(Outcome, &Frozen) + 'static) {
    let Some(gmon) = find_gdk_monitor(&frozen.mon.name) else {
        eprintln!("vector-capture: no output named {}", frozen.mon.name);
        done(Outcome::Cancel, &frozen);
        return;
    };
    let state = Rc::new(RefCell::new(State {
        frozen,
        kind,
        accent: rgb(accent),
        drag: None,
        pointer: None,
        hover: None,
        done: Some(Box::new(done)),
        window: None,
        shown_at: None,
    }));

    let win = gtk::Window::builder().application(app).decorated(false).build();
    win.init_layer_shell();
    win.set_layer(Layer::Overlay);
    win.set_keyboard_mode(KeyboardMode::Exclusive);
    win.set_exclusive_zone(-1);
    win.set_monitor(Some(&gmon));
    for edge in [gtk4_layer_shell::Edge::Top, gtk4_layer_shell::Edge::Bottom, gtk4_layer_shell::Edge::Left, gtk4_layer_shell::Edge::Right] {
        win.set_anchor(edge, true);
    }
    win.add_css_class("vcapture-overlay");

    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_cursor_from_name(Some("crosshair"));
    {
        let st = state.clone();
        area.set_draw_func(move |_, cr, w, h| draw(&st, cr, w, h));
    }
    win.set_child(Some(&area));
    state.borrow_mut().window = Some(win.clone());

    // pointer: hover highlights and the drag rectangle
    let motion = gtk::EventControllerMotion::new();
    {
        let (st, area) = (state.clone(), area.clone());
        motion.connect_motion(move |_, x, y| {
            let mut s = st.borrow_mut();
            s.pointer = Some((x, y));
            if let Kind::Window(rects) = &s.kind {
                s.hover = rects.iter().position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
            }
            drop(s);
            area.queue_draw();
        });
    }
    area.add_controller(motion);

    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    {
        let (st, area) = (state.clone(), area.clone());
        drag.connect_drag_begin(move |_, x, y| {
            st.borrow_mut().drag = Some(((x, y), (x, y)));
            area.queue_draw();
        });
    }
    {
        let (st, area) = (state.clone(), area.clone());
        drag.connect_drag_update(move |g, dx, dy| {
            if let Some((sx, sy)) = g.start_point() {
                if let Some(d) = st.borrow_mut().drag.as_mut() {
                    d.1 = (sx + dx, sy + dy);
                }
            }
            area.queue_draw();
        });
    }
    {
        let st = state.clone();
        drag.connect_drag_end(move |g, dx, dy| {
            let start = g.start_point().unwrap_or((0.0, 0.0));
            let end = (start.0 + dx, start.1 + dy);
            let moved = (end.0 - start.0).abs() + (end.1 - start.1).abs();
            let outcome = if moved >= 4.0 {
                Some(rect_of(start, end))
            } else {
                click_outcome(&st.borrow(), start)
            };
            finish(&st, outcome);
        });
    }
    area.add_controller(drag);

    let cancel = gtk::GestureClick::new();
    cancel.set_button(3);
    {
        let st = state.clone();
        cancel.connect_pressed(move |_, _, _, _| finish(&st, Some(Outcome::Cancel)));
    }
    area.add_controller(cancel);

    let keys = gtk::EventControllerKey::new();
    {
        let st = state.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => finish(&st, Some(Outcome::Cancel)),
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    let outcome = {
                        let s = st.borrow();
                        let pos = s.pointer.unwrap_or((0.0, 0.0));
                        click_outcome(&s, pos).or(Some(full(&s.frozen.mon)))
                    };
                    finish(&st, outcome);
                }
                _ => {}
            }
            glib::Propagation::Stop
        });
    }
    win.add_controller(keys);

    win.present();
    state.borrow_mut().shown_at = Some(std::time::Instant::now());
}

/// The area of the outcome for a click (no drag) at `pos`: the hovered window for window mode,
/// the whole output for output mode, nothing for region mode.
fn click_outcome(s: &State, pos: (f64, f64)) -> Option<Outcome> {
    match &s.kind {
        Kind::Region => None,
        Kind::Output => Some(full(&s.frozen.mon)),
        Kind::Window(rects) => {
            let i = rects.iter().position(|r| pos.0 >= r.x && pos.0 < r.x + r.w && pos.1 >= r.y && pos.1 < r.y + r.h).or(s.hover)?;
            let r = &rects[i];
            Some(Outcome::Rect { x: r.x.max(0.0), y: r.y.max(0.0), w: r.w.min(s.frozen.mon.w as f64 - r.x.max(0.0)), h: r.h.min(s.frozen.mon.h as f64 - r.y.max(0.0)) })
        }
    }
}

fn full(mon: &Monitor) -> Outcome {
    Outcome::Rect { x: 0.0, y: 0.0, w: mon.w as f64, h: mon.h as f64 }
}

fn rect_of(a: (f64, f64), b: (f64, f64)) -> Outcome {
    Outcome::Rect { x: a.0.min(b.0), y: a.1.min(b.1), w: (a.0 - b.0).abs(), h: (a.1 - b.1).abs() }
}

/// Ends the overlay once: hides the window, then calls the callback.
fn finish(st: &Rc<RefCell<State>>, outcome: Option<Outcome>) {
    let Some(outcome) = outcome else { return };
    let (done, win) = {
        let mut s = st.borrow_mut();
        (s.done.take(), s.window.take())
    };
    if let Some(win) = win {
        win.close();
    }
    if let Some(done) = done {
        let frozen = st.borrow().frozen.clone();
        done(outcome, &frozen);
    }
}

fn draw(st: &Rc<RefCell<State>>, cr: &gtk::cairo::Context, w: i32, _h: i32) {
    let s = st.borrow();
    let lw = w as f64;
    let pb = &s.frozen.pixbuf;
    let sc = pb.width() as f64 / lw.max(1.0);

    let paint_image = |cr: &gtk::cairo::Context| {
        let _ = cr.save();
        cr.scale(1.0 / sc, 1.0 / sc);
        cr.set_source_pixbuf(pb, 0.0, 0.0);
        let _ = cr.paint();
        let _ = cr.restore();
    };

    // the frozen screen, dimmed
    paint_image(cr);
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.32);
    let _ = cr.paint();

    let (ar, ag, ab) = s.accent;
    let rect = match (s.drag, &s.kind) {
        (Some((a, b)), _) => Some(rect_of(a, b)),
        (None, Kind::Window(rects)) => s.hover.map(|i| rects[i].clone()).map(|r| Outcome::Rect { x: r.x, y: r.y, w: r.w, h: r.h }),
        (None, Kind::Output) => Some(full(&s.frozen.mon)),
        (None, Kind::Region) => None,
    };
    if let Some(Outcome::Rect { x, y, w, h }) = rect {
        // the selection shows the screen as it is, undimmed
        let _ = cr.save();
        cr.rectangle(x, y, w, h);
        cr.clip();
        paint_image(cr);
        let _ = cr.restore();

        cr.set_source_rgb(ar, ag, ab);
        cr.set_line_width(2.0);
        cr.rectangle(x + 1.0, y + 1.0, (w - 2.0).max(0.0), (h - 2.0).max(0.0));
        let _ = cr.stroke();

        let label = format!("{} x {}", (w * sc).round() as i64, (h * sc).round() as i64);
        size_label(cr, &label, x + w, y + h, (ar, ag, ab));
    }
}

/// A small tag at the bottom-right corner of a selection.
fn size_label(cr: &gtk::cairo::Context, text: &str, x: f64, y: f64, accent: (f64, f64, f64)) {
    cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
    cr.set_font_size(13.0);
    let ext = cr.text_extents(text).ok();
    let tw = ext.map(|e| e.x_advance()).unwrap_or(80.0);
    let (bx, by) = (x - tw - 14.0, y + 6.0);
    cr.set_source_rgb(accent.0, accent.1, accent.2);
    cr.rectangle(bx, by, tw + 12.0, 20.0);
    let _ = cr.fill();
    cr.set_source_rgb(0.05, 0.05, 0.05);
    cr.move_to(bx + 6.0, by + 14.0);
    let _ = cr.show_text(text);
}

fn find_gdk_monitor(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_then(|o| o.downcast::<gdk::Monitor>().ok()))
        .find(|m| m.connector().is_some_and(|c| c == name))
}

/// `#rrggbb` to 0..1 floats; a bad value gives the Omarchy default blue.
pub fn rgb(hex: &str) -> (f64, f64, f64) {
    let h = hex.trim_start_matches('#');
    let p = |i: usize| u8::from_str_radix(h.get(i..i + 2).unwrap_or("81"), 16).unwrap_or(0x81) as f64 / 255.0;
    (p(0), p(2), p(4))
}
