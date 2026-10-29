//! The bar over a fullscreen window. While the monitor's active workspace has
//! a fullscreen window the bar moves to the overlay layer without an
//! exclusive zone, stays for a few seconds, then folds into a small arrow tab
//! at the top centre. Hovering the tab brings the bar back; it folds again a
//! moment after the pointer leaves. Leaving fullscreen restores the normal
//! bar. Before this, a bar re-created during fullscreen (a reload, a monitor
//! change) was drawn over the fullscreen window and stayed there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::{hypr, util};

/// How long the bar stays when fullscreen starts, and after the pointer leaves.
const SHOW: Duration = Duration::from_secs(5);
const LINGER: Duration = Duration::from_millis(900);

struct Inner {
    bar: glib::WeakRef<gtk::ApplicationWindow>,
    tab: gtk::ApplicationWindow,
    connector: String,
    test: bool,
    fullscreen: Cell<bool>,
    hovered: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    pending: Cell<bool>,
}

pub fn attach(app: &gtk::Application, bar: &gtk::ApplicationWindow, monitor: &gdk::Monitor, test: bool) {
    let tab = gtk::ApplicationWindow::new(app);
    tab.add_css_class("vbar-tab");
    tab.init_layer_shell();
    tab.set_layer(Layer::Overlay);
    tab.set_namespace(Some("vector-bar-tab"));
    tab.set_keyboard_mode(KeyboardMode::None);
    tab.set_anchor(Edge::Top, true);
    tab.set_exclusive_zone(-1);
    tab.set_monitor(Some(monitor));
    let arrow = gtk::Label::new(Some("\u{f078}"));
    arrow.add_css_class("arrow");
    tab.set_child(Some(&arrow));

    let inner = Rc::new(Inner {
        bar: bar.downgrade(),
        tab: tab.clone(),
        connector: monitor.connector().map(|c| c.to_string()).unwrap_or_default(),
        test,
        fullscreen: Cell::new(false),
        hovered: Cell::new(false),
        timer: RefCell::default(),
        pending: Cell::new(false),
    });

    // hovering the tab opens the bar
    let enter = gtk::EventControllerMotion::new();
    let w = Rc::downgrade(&inner);
    enter.connect_enter(move |_, _, _| {
        if let Some(i) = w.upgrade() {
            i.hovered.set(true);
            i.unfold(None);
        }
    });
    tab.add_controller(enter);

    // the bar folds a moment after the pointer leaves it
    let motion = gtk::EventControllerMotion::new();
    let w = Rc::downgrade(&inner);
    motion.connect_enter(move |_, _, _| {
        if let Some(i) = w.upgrade() {
            i.hovered.set(true);
            i.cancel();
        }
    });
    let w = Rc::downgrade(&inner);
    motion.connect_leave(move |_| {
        if let Some(i) = w.upgrade() {
            i.hovered.set(false);
            if i.fullscreen.get() {
                i.fold_after(LINGER);
            }
        }
    });
    bar.add_controller(motion);

    let w = Rc::downgrade(&inner);
    let events = hypr::events(move |name, _| {
        let Some(i) = w.upgrade() else { return false };
        if matches!(
            name,
            "fullscreen"
                | "workspacev2"
                | "activewindowv2"
                | "openwindow"
                | "closewindow"
                | "movewindowv2"
                | "moveworkspacev2"
                | "focusedmonv2"
                | "activespecialv2"
        ) {
            i.schedule();
        }
        true
    });
    // the tab and the listener live as long as the bar
    let keep = RefCell::new(Some((inner.clone(), events)));
    bar.connect_destroy(move |_| {
        if let Some((i, _ev)) = keep.borrow_mut().take() {
            i.cancel();
            i.tab.destroy();
        }
    });
    inner.query();
}

impl Inner {
    fn schedule(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let me = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(30), move || {
            me.pending.set(false);
            me.query();
        });
    }

    fn query(self: &Rc<Self>) {
        let mon = self.connector.clone();
        let me = Rc::downgrade(self);
        util::bg(
            move || {
                let ws = hypr::json("monitors")?
                    .as_array()?
                    .iter()
                    .find(|m| m["name"] == mon.as_str())?["activeWorkspace"]["id"]
                    .as_i64()?;
                let full = hypr::json("workspaces")?
                    .as_array()?
                    .iter()
                    .find(|w| w["id"].as_i64() == Some(ws))
                    .map(|w| w["hasfullscreen"].as_bool() == Some(true))
                    .unwrap_or(false);
                Some(full)
            },
            move |full| {
                if let (Some(me), Some(full)) = (me.upgrade(), full) {
                    me.set_fullscreen(full);
                }
            },
        );
    }

    fn set_fullscreen(self: &Rc<Self>, full: bool) {
        if self.fullscreen.replace(full) == full {
            return;
        }
        let Some(bar) = self.bar.upgrade() else { return };
        self.cancel();
        if full {
            bar.set_layer(Layer::Overlay);
            bar.set_exclusive_zone(0);
            self.unfold(Some(SHOW));
        } else {
            bar.set_layer(Layer::Top);
            if !self.test {
                bar.auto_exclusive_zone_enable();
            }
            self.tab.set_visible(false);
            bar.set_visible(true);
        }
    }

    /// Show the bar; with a duration, fold again after it unless hovered.
    fn unfold(self: &Rc<Self>, stay: Option<Duration>) {
        let Some(bar) = self.bar.upgrade() else { return };
        self.cancel();
        self.tab.set_visible(false);
        bar.set_visible(true);
        if let Some(d) = stay {
            self.fold_after(d);
        }
    }

    fn fold_after(self: &Rc<Self>, d: Duration) {
        self.cancel();
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(d, move || {
            let Some(i) = w.upgrade() else { return };
            i.timer.borrow_mut().take();
            if i.hovered.get() || !i.fullscreen.get() {
                return;
            }
            i.hovered.set(false);
            if let Some(bar) = i.bar.upgrade() {
                bar.set_visible(false);
            }
            i.tab.set_visible(true);
        });
        *self.timer.borrow_mut() = Some(id);
    }

    fn cancel(&self) {
        if let Some(id) = self.timer.borrow_mut().take() {
            id.remove();
        }
    }
}
