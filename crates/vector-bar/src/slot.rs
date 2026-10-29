//! A `Slot` is one clickable item on the bar. Modules set its text, classes,
//! tooltip and panel once; the slot mirrors that onto every bar (one per
//! monitor) and handles clicks, scrolling and click overrides from the config.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::config::Bind;
use crate::panel;
use crate::util;

pub type Action = Rc<dyn Fn()>;
pub type TooltipFn = Rc<dyn Fn() -> Option<gtk::Widget>>;
pub type PanelFn = Rc<dyn Fn(&panel::Panel) -> gtk::Widget>;

/// Change notification with connect/disconnect (for open panels).
/// Listeners with their ids.
type Listeners = Rc<RefCell<Vec<(u64, Rc<dyn Fn()>)>>>;

#[derive(Clone, Default)]
pub struct Notify {
    list: Listeners,
    next: Rc<Cell<u64>>,
}

impl Notify {
    pub fn connect(&self, f: impl Fn() + 'static) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.list.borrow_mut().push((id, Rc::new(f)));
        id
    }
    pub fn disconnect(&self, id: u64) {
        self.list.borrow_mut().retain(|(i, _)| *i != id);
    }
    pub fn emit(&self) {
        let fs: Vec<_> = self.list.borrow().iter().map(|(_, f)| f.clone()).collect();
        for f in fs {
            f();
        }
    }
}

type Clicker = Rc<dyn Fn(u32, &gtk::Widget)>;

thread_local! {
    static LAYOUT: Notify = Notify::default();
    /// every clickable bar widget, so a click on the bar row that the panel
    /// overlay caught can be handed to it (see panel.rs)
    static CLICKERS: RefCell<Vec<(glib::WeakRef<gtk::Widget>, Clicker)>> = RefCell::default();
}

/// Registers a click handler for `w` (button, widget).
pub fn register_click(w: &impl IsA<gtk::Widget>, f: Clicker) {
    let _ = CLICKERS.try_with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|(w, _)| w.upgrade().is_some());
        c.push((w.upcast_ref::<gtk::Widget>().downgrade(), f));
    });
}

/// Runs the click handler of `w` or its nearest registered ancestor.
pub fn click_at(w: &gtk::Widget, button: u32) -> bool {
    let mut cur = Some(w.clone());
    while let Some(x) = cur {
        let f = CLICKERS.with(|c| {
            c.borrow()
                .iter()
                .find(|(r, _)| r.upgrade().as_ref() == Some(&x))
                .map(|(_, f)| f.clone())
        });
        if let Some(f) = f {
            f(button, &x);
            return true;
        }
        cur = x.parent();
    }
    false
}

/// Emitted when any slot is shown or hidden (the bar re-checks its dividers).
pub fn layout() -> Notify {
    LAYOUT.try_with(|l| l.clone()).unwrap_or_default()
}

#[derive(Default)]
struct State {
    markup: String,
    classes: Vec<String>,
    visible: bool,
}

#[derive(Default)]
pub struct Actions {
    pub left: Option<Action>,
    pub middle: Option<Action>,
    pub right: Option<Action>,
    pub scroll_up: Option<Action>,
    pub scroll_down: Option<Action>,
}

struct Inner {
    name: String,
    state: RefCell<State>,
    views: RefCell<Vec<(glib::WeakRef<gtk::Box>, gtk::Label)>>,
    actions: RefCell<Actions>,
    tooltip: RefCell<Option<TooltipFn>>,
    panel: RefCell<Option<PanelFn>>,
    /// Bumped on every change; tooltips rebuild when it moved.
    version: Cell<u64>,
    bind: Bind,
    extra_class: Vec<String>,
}

#[derive(Clone)]
pub struct Slot(Rc<Inner>);

impl Slot {
    /// `name` is the module name (used for the CSS id `#name`, binds and `vector-bar toggle`).
    pub fn new(name: &str, bind: Option<&Bind>, classes: &[&str]) -> Slot {
        Slot(Rc::new(Inner {
            name: name.to_string(),
            state: RefCell::new(State {
                visible: true,
                ..Default::default()
            }),
            views: RefCell::default(),
            actions: RefCell::default(),
            tooltip: RefCell::default(),
            panel: RefCell::default(),
            version: Cell::new(0),
            bind: bind.cloned().unwrap_or_default(),
            extra_class: classes.iter().map(|s| s.to_string()).collect(),
        }))
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    fn views(&self) -> Vec<(gtk::Box, gtk::Label)> {
        let mut v = self.0.views.borrow_mut();
        v.retain(|(w, _)| w.upgrade().is_some());
        v.iter()
            .filter_map(|(w, l)| w.upgrade().map(|b| (b, l.clone())))
            .collect()
    }

    /// Bar text (Pango markup).
    pub fn set_markup(&self, m: &str) {
        let changed = {
            let mut s = self.0.state.borrow_mut();
            if s.markup == m {
                false
            } else {
                s.markup = m.to_string();
                true
            }
        };
        if changed {
            for (_, l) in self.views() {
                l.set_markup(m);
                l.set_visible(!m.is_empty());
            }
        }
        self.bump();
    }

    /// State classes (replaces the previous set): "dim", "warn", "crit", ...
    pub fn set_classes(&self, classes: &[&str]) {
        let new: Vec<String> = classes
            .iter()
            .filter(|c| !c.is_empty())
            .map(|s| s.to_string())
            .collect();
        let old = std::mem::replace(&mut self.0.state.borrow_mut().classes, new.clone());
        if old == new {
            return;
        }
        for (b, _) in self.views() {
            for c in &old {
                b.remove_css_class(c);
            }
            for c in &new {
                b.add_css_class(c);
            }
        }
    }

    pub fn set_visible(&self, on: bool) {
        if self.0.state.borrow().visible == on {
            return;
        }
        self.0.state.borrow_mut().visible = on;
        for (b, _) in self.views() {
            b.set_visible(on);
        }
        layout().emit();
        if !on && panel::is_open(&self.0.name) {
            panel::close();
        }
    }

    /// Marks the content as changed so an open tooltip is rebuilt.
    pub fn bump(&self) {
        self.0.version.set(self.0.version.get() + 1);
    }

    pub fn on_left(&self, f: impl Fn() + 'static) {
        self.0.actions.borrow_mut().left = Some(Rc::new(f));
    }
    pub fn on_middle(&self, f: impl Fn() + 'static) {
        self.0.actions.borrow_mut().middle = Some(Rc::new(f));
    }
    pub fn on_right(&self, f: impl Fn() + 'static) {
        self.0.actions.borrow_mut().right = Some(Rc::new(f));
    }
    pub fn on_scroll(&self, up: impl Fn() + 'static, down: impl Fn() + 'static) {
        let mut a = self.0.actions.borrow_mut();
        a.scroll_up = Some(Rc::new(up));
        a.scroll_down = Some(Rc::new(down));
    }

    /// Tooltip contents, built on hover. Return `None` for no tooltip.
    pub fn tooltip(&self, f: impl Fn() -> Option<gtk::Widget> + 'static) {
        *self.0.tooltip.borrow_mut() = Some(Rc::new(f));
    }

    /// Drop-down panel; left click toggles it unless something else is bound.
    pub fn panel(&self, f: impl Fn(&panel::Panel) -> gtk::Widget + 'static) {
        *self.0.panel.borrow_mut() = Some(Rc::new(f));
    }

    pub fn has_panel(&self) -> bool {
        self.0.panel.borrow().is_some()
    }

    /// Opens/closes this slot's panel under its widget on the given (or first) bar.
    pub fn toggle_panel(&self, anchor: Option<&gtk::Widget>, center: bool) {
        let Some(build) = self.0.panel.borrow().clone() else {
            return;
        };
        let anchor = anchor.cloned().or_else(|| {
            self.views()
                .into_iter()
                .find(|(b, _)| b.is_mapped())
                .map(|(b, _)| b.upcast())
        });
        panel::toggle(&self.0.name, build, anchor, center);
    }

    pub fn set_open(&self, open: bool) {
        for (b, _) in self.views() {
            if open {
                b.add_css_class("open");
            } else {
                b.remove_css_class("open");
            }
        }
    }

    /// Builds the widget for one bar.
    pub fn view(&self) -> gtk::Widget {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        b.add_css_class("item");
        b.set_widget_name(&self.0.name);
        for c in &self.0.extra_class {
            b.add_css_class(c);
        }
        let l = gtk::Label::new(None);
        b.append(&l);
        {
            let s = self.0.state.borrow();
            l.set_markup(&s.markup);
            l.set_visible(!s.markup.is_empty());
            for c in &s.classes {
                b.add_css_class(c);
            }
            b.set_visible(s.visible);
        }
        if panel::is_open(&self.0.name) {
            b.add_css_class("open");
        }

        let interactive = {
            let a = self.0.actions.borrow();
            let bd = &self.0.bind;
            a.left.is_some()
                || a.right.is_some()
                || a.middle.is_some()
                || self.0.panel.borrow().is_some()
                || bd.left.is_some()
                || bd.right.is_some()
                || bd.middle.is_some()
        };
        if interactive {
            b.add_css_class("clickable");
            crate::ui::pointer(&b);
        }

        // clicks
        let g = gtk::GestureClick::new();
        g.set_button(0);
        let me = self.clone();
        let anchor = b.downgrade();
        g.connect_released(move |g, _, _, _| {
            let btn = g.current_button();
            let anchor = anchor.upgrade().map(|w| w.upcast::<gtk::Widget>());
            me.click(btn, anchor.as_ref());
        });
        b.add_controller(g);
        let me = self.clone();
        register_click(&b, Rc::new(move |btn, w| me.click(btn, Some(w))));

        // scrolling (touchpads send small deltas: accumulate to whole steps)
        let sc = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let me = self.clone();
        let acc = Rc::new(Cell::new(0.0f64));
        sc.connect_scroll(move |c, _dx, dy| {
            let discrete = c.unit() == gdk::ScrollUnit::Wheel;
            let step = if discrete { 1.0 } else { 12.0 };
            acc.set(acc.get() + dy);
            while acc.get().abs() >= step {
                let up = acc.get() < 0.0;
                acc.set(acc.get() + if up { step } else { -step });
                me.scroll(up);
            }
            glib::Propagation::Stop
        });
        b.add_controller(sc);

        // tooltip, rebuilt when the slot changed since it was last built
        b.set_has_tooltip(true);
        let me = self.clone();
        let cache: Rc<RefCell<Option<(u64, f64, gtk::Widget)>>> = Rc::default();
        b.connect_query_tooltip(move |_, _, _, _, tip| {
            if panel::is_open(&me.0.name) {
                return false;
            }
            let Some(f) = me.0.tooltip.borrow().clone() else {
                return false;
            };
            let v = me.0.version.get();
            let now = util::now();
            let mut c = cache.borrow_mut();
            let fresh = matches!(&*c, Some((ver, at, _)) if *ver == v && now - at < 1.0);
            if !fresh {
                match f() {
                    Some(w) => {
                        if w.has_css_class("sheet")
                            && w.first_child().is_some_and(|c| c.next_sibling().is_none())
                        {
                            w.add_css_class("bare");
                        }
                        *c = Some((v, now, w))
                    }
                    None => {
                        *c = None;
                        return false;
                    }
                }
            }
            match &*c {
                Some((_, _, w)) => {
                    tip.set_custom(Some(w));
                    true
                }
                None => false,
            }
        });

        self.0.views.borrow_mut().push((b.downgrade(), l));
        b.upcast()
    }

    fn click(&self, button: u32, anchor: Option<&gtk::Widget>) {
        let bind = match button {
            gdk::BUTTON_PRIMARY => &self.0.bind.left,
            gdk::BUTTON_MIDDLE => &self.0.bind.middle,
            gdk::BUTTON_SECONDARY => &self.0.bind.right,
            _ => &None,
        };
        if let Some(cmd) = bind {
            util::spawn_sh(cmd);
            return;
        }
        let action = {
            let a = self.0.actions.borrow();
            match button {
                gdk::BUTTON_PRIMARY => a.left.clone(),
                gdk::BUTTON_MIDDLE => a.middle.clone(),
                gdk::BUTTON_SECONDARY => a.right.clone(),
                _ => None,
            }
        };
        match action {
            Some(f) => f(),
            None if button == gdk::BUTTON_PRIMARY => self.toggle_panel(anchor, false),
            None => {}
        }
    }

    fn scroll(&self, up: bool) {
        let bind = if up {
            &self.0.bind.scroll_up
        } else {
            &self.0.bind.scroll_down
        };
        if let Some(cmd) = bind {
            util::spawn_sh(cmd);
            return;
        }
        let f = {
            let a = self.0.actions.borrow();
            if up {
                a.scroll_up.clone()
            } else {
                a.scroll_down.clone()
            }
        };
        if let Some(f) = f {
            f();
        }
    }
}
