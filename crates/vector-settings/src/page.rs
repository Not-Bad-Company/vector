//! What a page is: its sidebar entry, its widgets, and what happens when it is
//! shown, hidden and closed. Plus the standard frame (title, detail, header
//! controls, scrolling body, footer) every page uses.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::ui;

pub struct PageDef {
    pub id: &'static str,
    pub icon: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub build: fn(&Ctx) -> View,
}

/// Shared for all pages.
pub struct Ctx {
    pub system: Option<gio::DBusConnection>,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx {
            system: crate::dbus::system(),
        }
    }
}

type Hook = Rc<dyn Fn()>;

pub struct View {
    pub root: gtk::Widget,
    on_show: RefCell<Vec<Hook>>,
    on_hide: RefCell<Vec<Hook>>,
    on_stop: RefCell<Vec<Hook>>,
    timers: RefCell<Vec<(u64, Hook, Option<glib::SourceId>)>>,
}

impl View {
    pub fn new(root: &impl IsA<gtk::Widget>) -> View {
        View {
            root: root.clone().upcast(),
            on_show: RefCell::default(),
            on_hide: RefCell::default(),
            on_stop: RefCell::default(),
            timers: RefCell::default(),
        }
    }

    /// Runs each time the page comes into view (refresh, scan).
    pub fn on_show(self, f: impl Fn() + 'static) -> View {
        self.on_show.borrow_mut().push(Rc::new(f));
        self
    }

    pub fn on_hide(self, f: impl Fn() + 'static) -> View {
        self.on_hide.borrow_mut().push(Rc::new(f));
        self
    }

    /// Runs when the window closes (unregister agents, ...).
    pub fn on_stop(self, f: impl Fn() + 'static) -> View {
        self.on_stop.borrow_mut().push(Rc::new(f));
        self
    }

    /// Calls `f` every `ms` while the page is in view.
    pub fn every(self, ms: u64, f: impl Fn() + 'static) -> View {
        self.timers.borrow_mut().push((ms, Rc::new(f), None));
        self
    }

    pub fn shown(&self) {
        for f in self.on_show.borrow().iter() {
            f();
        }
        for (ms, f, id) in self.timers.borrow_mut().iter_mut() {
            if id.is_none() {
                let f = f.clone();
                *id = Some(glib::timeout_add_local(
                    std::time::Duration::from_millis(*ms),
                    move || {
                        f();
                        glib::ControlFlow::Continue
                    },
                ));
            }
        }
    }

    pub fn hidden(&self) {
        for (_, _, id) in self.timers.borrow_mut().iter_mut() {
            if let Some(id) = id.take() {
                id.remove();
            }
        }
        for f in self.on_hide.borrow().iter() {
            f();
        }
    }

    pub fn stop(&self) {
        self.hidden();
        for f in self.on_stop.borrow().iter() {
            f();
        }
    }
}

/// The standard page layout.
pub struct Frame {
    pub root: gtk::Box,
    pub detail: gtk::Label,
    pub controls: gtk::Box,
    pub body: gtk::Box,
    pub foot: gtk::Box,
}

impl Frame {
    pub fn new(title: &str) -> Frame {
        let root = ui::vbox(0, &["page"]);
        let head = ui::hbox(12, &["page-head"]);
        let titles = ui::vbox(2, &[]);
        titles.append(&ui::text(title, &["ptitle"]));
        let detail = ui::markup("", &["pdetail"]);
        titles.append(&detail);
        titles.set_hexpand(true);
        head.append(&titles);
        let controls = ui::hbox(10, &[]);
        controls.set_valign(gtk::Align::Center);
        head.append(&controls);
        root.append(&head);
        let body = ui::vbox(0, &["page-body"]);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        let clamp = ui::vbox(0, &[]);
        clamp.append(&body);
        body.set_size_request(640, -1);
        body.set_halign(gtk::Align::Fill);
        scroll.set_child(Some(&clamp));
        root.append(&scroll);
        let foot = ui::hbox(6, &["page-foot"]);
        foot.set_visible(false);
        root.append(&foot);
        Frame {
            root,
            detail,
            controls,
            body,
            foot,
        }
    }

    pub fn foot_add(&self, w: &impl IsA<gtk::Widget>) {
        self.foot.set_visible(true);
        self.foot.append(w);
    }
}
