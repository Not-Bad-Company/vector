//! The frozen-screen overlay: a layer-shell surface on one output showing the
//! grab taken the instant the key was pressed. The user drags a region, clicks a
//! window or an output, or points at a pixel (colour picker, with a magnifier).
//! The callback gets one outcome; the overlay then goes away.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::capture::{Monitor, WinRect};
use crate::color;

/// What the overlay lets the user pick.
#[derive(Clone)]
pub enum Kind {
    /// Drag a rectangle.
    Region,
    /// Click (or Enter on) the window under the pointer, or drag a rectangle.
    Window(Vec<WinRect>),
    /// Click anywhere, or Enter, for the whole output.
    Output,
    /// Click (or Enter) a pixel. h, r, l pick the format that is copied (hex, rgb, hsl).
    Picker,
}

/// The result, in the output's local logical coordinates.
#[derive(Clone, Copy, Debug)]
pub enum Outcome {
    Cancel,
    Rect { x: f64, y: f64, w: f64, h: f64 },
    Point { x: f64, y: f64, format: color::Format },
}

/// The frozen grab and the output it was taken from.
#[derive(Clone)]
pub struct Frozen {
    pub pixbuf: Pixbuf,
    pub mon: Monitor,
}

impl Frozen {
    /// The RGB value of the physical pixel under logical point (x, y).
    pub fn pixel(&self, x: f64, y: f64) -> Option<(u8, u8, u8)> {
        let sx = (x * self.pixbuf.width() as f64 / self.mon.w as f64).floor() as i32;
        let sy = (y * self.pixbuf.height() as f64 / self.mon.h as f64).floor() as i32;
        if sx < 0 || sy < 0 || sx >= self.pixbuf.width() || sy >= self.pixbuf.height() {
            return None;
        }
        let stride = self.pixbuf.rowstride() as usize;
        let ch = self.pixbuf.n_channels() as usize;
        let i = sy as usize * stride + sx as usize * ch;
        let bytes = self.pixbuf.read_pixel_bytes();
        let p = bytes.get(i..i + 3)?;
        Some((p[0], p[1], p[2]))
    }
}

type Done = Box<dyn FnOnce(Outcome, &Frozen)>;

struct State {
    frozen: Frozen,
    kind: Kind,
    accent: (f64, f64, f64),
    drag: Option<((f64, f64), (f64, f64))>,
    pointer: Option<(f64, f64)>,
    hover: Option<usize>,
    format: color::Format,
    done: Option<Done>,
    /// Keeps the layer window alive while the overlay is up.
    window: Option<gtk::Window>,
}

/// Shows the overlay on the output of `frozen`. `done` runs once with the outcome.
pub fn run(app: &gtk::Application, frozen: Frozen, kind: Kind, accent: &str, done: impl FnOnce(Outcome, &Frozen) + 'static) {
    let Some(gmon) = find_gdk_monitor(&frozen.mon.name) else {
        eprintln!("vector-capture: no output named {}", frozen.mon.name);
        done(Outcome::Cancel, &frozen);
        return;
    };
    let picker = matches!(kind, Kind::Picker);
    let state = Rc::new(RefCell::new(State {
        frozen,
        kind,
        accent: rgb(accent),
        drag: None,
        pointer: None,
        hover: None,
        format: color::Format::Hex,
        done: Some(Box::new(done)),
        window: None,
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

    // pointer: hover highlights, the magnifier and the drag rectangle
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
            let outcome = if moved >= 4.0 && !picker {
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
                gdk::Key::h | gdk::Key::r | gdk::Key::l if picker => {
                    st.borrow_mut().format = match key {
                        gdk::Key::r => color::Format::Rgb,
                        gdk::Key::l => color::Format::Hsl,
                        _ => color::Format::Hex,
                    };
                    let w = st.borrow().window.clone();
                    if let Some(w) = w {
                        w.queue_draw();
                    }
                }
                _ => {}
            }
            glib::Propagation::Stop
        });
    }
    win.add_controller(keys);

    win.present();
}

/// The outcome of a click (no drag) at `pos`: the hovered window for window mode,
/// the whole output for output mode, the pixel for the picker, nothing for region mode.
fn click_outcome(s: &State, pos: (f64, f64)) -> Option<Outcome> {
    match &s.kind {
        Kind::Region => None,
        Kind::Output => Some(full(&s.frozen.mon)),
        Kind::Picker => Some(Outcome::Point { x: pos.0, y: pos.1, format: s.format }),
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
    if let Kind::Picker = s.kind {
        if let Some(p) = s.pointer {
            draw_magnifier(&s, cr, p, (ar, ag, ab));
        }
        return;
    }

    let rect = match (s.drag, &s.kind) {
        (Some((a, b)), _) => Some(rect_of(a, b)),
        (None, Kind::Window(rects)) => s.hover.map(|i| rects[i].clone()).map(|r| Outcome::Rect { x: r.x, y: r.y, w: r.w, h: r.h }),
        (None, Kind::Output) => Some(full(&s.frozen.mon)),
        _ => None,
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
        tag(cr, &label, x + w, y + h, (ar, ag, ab));
    }
}

/// The colour picker's magnifier: a grid of the pixels around the pointer, the centre one
/// outlined, and the colour in every format below it (the chosen one first).
fn draw_magnifier(s: &State, cr: &gtk::cairo::Context, p: (f64, f64), accent: (f64, f64, f64)) {
    const RADIUS: i32 = 7; // 15 x 15 pixels
    const CELL: f64 = 10.0;
    let size = (2 * RADIUS + 1) as f64 * CELL;
    let (mut x0, mut y0) = (p.0 + 24.0, p.1 + 24.0);
    if x0 + size > s.frozen.mon.w as f64 {
        x0 = p.0 - 24.0 - size;
    }
    if y0 + size + 56.0 > s.frozen.mon.h as f64 {
        y0 = p.1 - 24.0 - size - 56.0;
    }
    cr.set_source_rgb(0.0, 0.0, 0.0);
    cr.rectangle(x0 - 2.0, y0 - 2.0, size + 4.0, size + 4.0);
    let _ = cr.fill();
    for dy in -RADIUS..=RADIUS {
        for dx in -RADIUS..=RADIUS {
            if let Some((r, g, b)) = s.frozen.pixel(p.0 + dx as f64, p.1 + dy as f64) {
                cr.set_source_rgb(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
                cr.rectangle(x0 + (dx + RADIUS) as f64 * CELL, y0 + (dy + RADIUS) as f64 * CELL, CELL, CELL);
                let _ = cr.fill();
            }
        }
    }
    cr.set_source_rgb(accent.0, accent.1, accent.2);
    cr.set_line_width(2.0);
    cr.rectangle(x0 + RADIUS as f64 * CELL, y0 + RADIUS as f64 * CELL, CELL, CELL);
    let _ = cr.stroke();

    let Some((r, g, b)) = s.frozen.pixel(p.0, p.1) else { return };
    let chosen = match s.format {
        color::Format::Hex => color::hex(r, g, b),
        color::Format::Rgb => color::rgb(r, g, b),
        color::Format::Hsl => color::hsl(r, g, b),
    };
    tag(cr, &chosen, x0 + size, y0 + size + 22.0, accent);
    cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(12.0);
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.move_to(x0, y0 + size + 52.0);
    let line = format!("h {}   r {}   l {}", color::hex(r, g, b), color::rgb(r, g, b), color::hsl(r, g, b));
    let _ = cr.show_text(&line);
}

/// A small tag ending at (x, y), in the accent colour with dark text.
fn tag(cr: &gtk::cairo::Context, text: &str, x: f64, y: f64, accent: (f64, f64, f64)) {
    cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Bold);
    cr.set_font_size(13.0);
    let tw = cr.text_extents(text).map(|e| e.x_advance()).unwrap_or(80.0);
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
