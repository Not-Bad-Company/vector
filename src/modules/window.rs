//! The focused window's title (Hyprland events), dim and shortened.
//! Click: nothing; middle: close window; hover: class and full title.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::hypr;
use crate::slot::Slot;
use crate::ui;
use crate::util;

struct Inner {
    slot: Slot,
    max: usize,
    current: RefCell<(String, String)>,
}

pub struct Window(Rc<Inner>);

impl Window {
    pub fn new(ctx: &Ctx) -> Window {
        let o = ctx.opts("window");
        let inner = Rc::new(Inner { slot: ctx.slot("window", &["window-title"]), max: o.num("max_length", 60.0) as usize, current: RefCell::default() });
        inner.slot.set_visible(false);
        inner.slot.on_middle(|| hypr::dispatch("killactive"));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || {
            let i = w.upgrade()?;
            let (class, title) = i.current.borrow().clone();
            if title.is_empty() {
                return None;
            }
            let s = ui::sheet();
            s.append(&ui::head(&class.to_lowercase(), "window").0);
            s.append(&ui::text(&title, &[]).tap_wrap());
            s.append(&ui::thint("middle close"));
            Some(s.upcast())
        });
        let w = Rc::downgrade(&inner);
        hypr::events(move |name, data| {
            let Some(i) = w.upgrade() else { return false };
            if name == "activewindow" {
                let (class, title) = data.split_once(',').unwrap_or((data, ""));
                i.set(class, title);
            } else if name == "closewindow" || name == "workspace" {
                i.query();
            }
            true
        });
        inner.query();
        Window(inner)
    }
}

trait TapWrap {
    fn tap_wrap(self) -> Self;
}
impl TapWrap for gtk::Label {
    fn tap_wrap(self) -> Self {
        self.set_wrap(true);
        self.set_max_width_chars(60);
        self.set_margin_top(6);
        self
    }
}

impl Inner {
    fn set(&self, class: &str, title: &str) {
        *self.current.borrow_mut() = (class.to_string(), title.to_string());
        let t = if title.chars().count() > self.max { title.chars().take(self.max).collect::<String>() + "…" } else { title.to_string() };
        self.slot.set_markup(&util::esc(&t));
        self.slot.set_visible(!t.is_empty());
        self.slot.bump();
    }

    fn query(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        util::bg(
            || hypr::json("activewindow").map(|v| (v["class"].as_str().unwrap_or("").to_string(), v["title"].as_str().unwrap_or("").to_string())),
            move |r| {
                if let Some(i) = w.upgrade() {
                    let (c, t) = r.unwrap_or_default();
                    i.set(&c, &t);
                }
            },
        );
    }
}

impl Module for Window {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.query();
    }
}
