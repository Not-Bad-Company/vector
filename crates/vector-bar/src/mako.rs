//! The notification client every module shares (bell, mode block, silence
//! mark). With the built-in server (notify/) it reads the store directly;
//! with `[notifications] enable = false` it talks to mako over D-Bus
//! (`fr.emersion.Mako`), with change signals instead of `makoctl` polling.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

pub use crate::notify::Note;
use crate::notify::{self, Store};
use crate::slot::Notify;

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/fr/emersion/Mako";
const IFACE: &str = "fr.emersion.Mako";
pub const DND: &str = notify::DND;

#[derive(Default)]
struct State {
    on_screen: Vec<Note>,
    modes: Vec<String>,
}

pub struct Mako {
    conn: Option<gio::DBusConnection>,
    local: Option<Rc<Store>>,
    state: RefCell<State>,
    pub changed: Notify,
}

thread_local! {
    static MAKO: RefCell<Option<Rc<Mako>>> = const { RefCell::new(None) };
}

/// The shared client (one subscription for every module).
#[allow(deprecated)]
pub fn get() -> Rc<Mako> {
    if let Some(m) = MAKO.with(|m| m.borrow().clone()) {
        return m;
    }
    let local = notify::enabled().then(notify::get);
    let conn = if local.is_some() {
        None
    } else {
        gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()
    };
    let m = Rc::new(Mako {
        conn,
        local,
        state: RefCell::default(),
        changed: Notify::default(),
    });
    if let Some(s) = &m.local {
        let w = Rc::downgrade(&m);
        s.changed.connect(move || {
            if let Some(m) = w.upgrade() {
                m.refresh();
            }
        });
    }
    if let Some(c) = &m.conn {
        let w = Rc::downgrade(&m);
        c.signal_subscribe(
            Some(BUS),
            Some("org.freedesktop.DBus.Properties"),
            Some("PropertiesChanged"),
            Some(PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, _| {
                if let Some(m) = w.upgrade() {
                    m.refresh();
                }
            },
        );
        // mako restarted (theme switch, desktop-glass): re-read
        let w = Rc::downgrade(&m);
        c.signal_subscribe(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            None,
            Some(BUS),
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, _| {
                if let Some(m) = w.upgrade() {
                    m.refresh();
                }
            },
        );
    }
    m.refresh();
    MAKO.with(|x| *x.borrow_mut() = Some(m.clone()));
    m
}

fn dict_str(d: &glib::VariantDict, k: &str) -> String {
    d.lookup_value(k, None)
        .and_then(|v| {
            v.str()
                .map(str::to_string)
                .or_else(|| v.as_variant().and_then(|x| x.str().map(str::to_string)))
        })
        .unwrap_or_default()
}

pub fn parse_notes(v: &glib::Variant) -> Vec<Note> {
    // (aa{sv})
    let arr = v.child_value(0);
    let mut out = Vec::new();
    for i in 0..arr.n_children() {
        let d = glib::VariantDict::new(Some(&arr.child_value(i)));
        let id = d
            .lookup_value("id", None)
            .and_then(|v| v.get::<u32>())
            .unwrap_or(0);
        let urgency = d
            .lookup_value("urgency", None)
            .and_then(|v| v.get::<u8>())
            .unwrap_or(1);
        let actions = d
            .lookup_value("actions", None)
            .map(|a| {
                let mut v = Vec::new();
                for j in 0..a.n_children() {
                    let e = a.child_value(j);
                    if let (Some(k), Some(l)) = (e.child_value(0).str(), e.child_value(1).str()) {
                        v.push((k.to_string(), l.to_string()));
                    }
                }
                v
            })
            .unwrap_or_default();
        out.push(Note {
            id,
            app: dict_str(&d, "app-name"),
            summary: dict_str(&d, "summary"),
            body: dict_str(&d, "body"),
            urgency,
            icon: dict_str(&d, "app-icon"),
            category: dict_str(&d, "category"),
            desktop_entry: dict_str(&d, "desktop-entry"),
            actions,
            ..Default::default()
        });
    }
    out
}

impl Mako {
    /// The built-in store, when it serves.
    pub fn store(&self) -> Option<&Rc<Store>> {
        self.local.as_ref()
    }

    pub fn refresh(self: &Rc<Self>) {
        if let Some(s) = &self.local {
            let (notes, modes) = (s.visible(), s.modes());
            let changed = {
                let st = self.state.borrow();
                st.on_screen != notes || st.modes != modes
            };
            *self.state.borrow_mut() = State {
                on_screen: notes,
                modes,
            };
            // the store also changes for history-only reasons (seen, cleared)
            let _ = changed;
            self.changed.emit();
            return;
        }
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let notes = c
                .call_future(
                    Some(BUS),
                    PATH,
                    IFACE,
                    "ListNotifications",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                )
                .await;
            let modes = c
                .call_future(
                    Some(BUS),
                    PATH,
                    IFACE,
                    "ListModes",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                )
                .await;
            let Some(m) = w.upgrade() else { return };
            let notes = notes.map(|v| parse_notes(&v)).unwrap_or_default();
            let modes = modes
                .ok()
                .and_then(|v| v.child_value(0).get::<Vec<String>>())
                .unwrap_or_default();
            let changed = {
                let s = m.state.borrow();
                s.on_screen != notes || s.modes != modes
            };
            {
                let mut s = m.state.borrow_mut();
                s.on_screen = notes;
                s.modes = modes;
            }
            if changed {
                m.changed.emit();
            }
        });
    }

    pub fn on_screen(&self) -> Vec<Note> {
        self.state.borrow().on_screen.clone()
    }

    pub fn dnd(&self) -> bool {
        self.state.borrow().modes.iter().any(|m| m == DND)
    }

    /// History (most recent first), fetched on demand.
    pub fn history(&self, done: impl FnOnce(Vec<Note>) + 'static) {
        if let Some(s) = &self.local {
            return done(s.history());
        }
        let Some(c) = self.conn.clone() else {
            return done(Vec::new());
        };
        glib::spawn_future_local(async move {
            let v = c
                .call_future(
                    Some(BUS),
                    PATH,
                    IFACE,
                    "ListHistory",
                    None,
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                )
                .await;
            done(v.map(|v| parse_notes(&v)).unwrap_or_default());
        });
    }

    fn call(self: &Rc<Self>, method: &str, args: Option<glib::Variant>) {
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        c.call(
            Some(BUS),
            PATH,
            IFACE,
            method,
            args.as_ref(),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
            move |_| {
                if let Some(m) = w.upgrade() {
                    m.refresh();
                }
            },
        );
    }

    pub fn set_dnd(self: &Rc<Self>, on: bool) {
        if let Some(s) = &self.local {
            return s.set_dnd(on);
        }
        let mut modes = self.state.borrow().modes.clone();
        modes.retain(|m| m != DND);
        if on {
            modes.push(DND.into());
        }
        if modes.is_empty() {
            modes.push("default".into());
        }
        self.call("SetModes", Some((modes,).to_variant()));
    }

    pub fn dismiss(self: &Rc<Self>, id: u32) {
        if let Some(s) = &self.local {
            return s.dismiss(id);
        }
        let d = glib::VariantDict::new(None);
        d.insert("id", id);
        self.call(
            "DismissNotifications",
            Some(glib::Variant::tuple_from_iter([d.end()])),
        );
    }

    pub fn dismiss_all(self: &Rc<Self>) {
        if let Some(s) = &self.local {
            return s.dismiss_all();
        }
        let d = glib::VariantDict::new(None);
        d.insert("all", true);
        self.call(
            "DismissNotifications",
            Some(glib::Variant::tuple_from_iter([d.end()])),
        );
    }

    pub fn invoke(self: &Rc<Self>, id: u32, action: &str) {
        if let Some(s) = &self.local {
            return if action == "default" {
                s.activate(id)
            } else {
                s.invoke(id, action)
            };
        }
        self.call("InvokeAction", Some((id, action).to_variant()));
    }

    pub fn restore(self: &Rc<Self>) {
        if let Some(s) = &self.local {
            return s.restore();
        }
        self.call("RestoreNotification", None);
    }
}
