//! The D-Bus side: `org.freedesktop.Notifications` (spec 1.2) for apps, and
//! mako's `fr.emersion.Mako` so makoctl and everything built on it still work.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use super::{Note, Store};

const BUS: &str = "org.freedesktop.Notifications";
const FDO_PATH: &str = "/org/freedesktop/Notifications";
const FDO: &str = "org.freedesktop.Notifications";
const MAKO_PATH: &str = "/fr/emersion/Mako";
const MAKO: &str = "fr.emersion.Mako";
const VEC_PATH: &str = "/org/vectoros/Notifications";
const VEC: &str = "org.vectoros.Notifications";

const FDO_XML: &str = r#"
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="GetCapabilities"><arg direction="out" type="as"/></method>
    <method name="Notify">
      <arg direction="in" type="s" name="app_name"/>
      <arg direction="in" type="u" name="replaces_id"/>
      <arg direction="in" type="s" name="app_icon"/>
      <arg direction="in" type="s" name="summary"/>
      <arg direction="in" type="s" name="body"/>
      <arg direction="in" type="as" name="actions"/>
      <arg direction="in" type="a{sv}" name="hints"/>
      <arg direction="in" type="i" name="expire_timeout"/>
      <arg direction="out" type="u" name="id"/>
    </method>
    <method name="CloseNotification"><arg direction="in" type="u" name="id"/></method>
    <method name="GetServerInformation">
      <arg direction="out" type="s" name="name"/>
      <arg direction="out" type="s" name="vendor"/>
      <arg direction="out" type="s" name="version"/>
      <arg direction="out" type="s" name="spec_version"/>
    </method>
    <signal name="NotificationClosed"><arg type="u" name="id"/><arg type="u" name="reason"/></signal>
    <signal name="ActionInvoked"><arg type="u" name="id"/><arg type="s" name="action_key"/></signal>
    <signal name="ActivationToken"><arg type="u" name="id"/><arg type="s" name="activation_token"/></signal>
  </interface>
</node>"#;

const MAKO_XML: &str = r#"
<node>
  <interface name="fr.emersion.Mako">
    <method name="DismissNotifications"><arg direction="in" type="a{sv}"/></method>
    <method name="InvokeAction"><arg direction="in" type="u"/><arg direction="in" type="s"/></method>
    <method name="RestoreNotification"/>
    <method name="ListNotifications"><arg direction="out" type="aa{sv}"/></method>
    <method name="ListHistory"><arg direction="out" type="aa{sv}"/></method>
    <method name="Reload"/>
    <method name="SetMode"><arg direction="in" type="s"/></method>
    <method name="ListModes"><arg direction="out" type="as"/></method>
    <method name="SetModes"><arg direction="in" type="as"/></method>
    <property name="Modes" type="as" access="read"/>
  </interface>
</node>"#;

/// For Vector Space's notification list: everything mako's interface has
/// not (history with pictures and times, forgetting one, silence switch).
const VEC_XML: &str = r#"
<node>
  <interface name="org.vectoros.Notifications">
    <method name="List"><arg direction="out" type="aa{sv}"/></method>
    <method name="Activate"><arg direction="in" type="u"/></method>
    <method name="Invoke"><arg direction="in" type="u"/><arg direction="in" type="s"/></method>
    <method name="Dismiss"><arg direction="in" type="u"/></method>
    <method name="ClearHistory"/>
    <method name="MarkSeen"/>
    <method name="GetDnd"><arg direction="out" type="b"/></method>
    <method name="SetDnd"><arg direction="in" type="b"/></method>
    <signal name="Changed"/>
  </interface>
</node>"#;

fn iface(xml: &str, name: &str) -> Option<gio::DBusInterfaceInfo> {
    gio::DBusNodeInfo::for_xml(xml).ok()?.lookup_interface(name)
}

pub fn start(store: &Rc<Store>) {
    let Ok(conn) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        eprintln!("vector-bar: notifications: no session bus");
        return;
    };
    *store.conn.borrow_mut() = Some(conn.clone());
    let (Some(fdo), Some(mako)) = (iface(FDO_XML, FDO), iface(MAKO_XML, MAKO)) else {
        return;
    };
    let w = Rc::downgrade(store);
    let _ = conn
        .register_object(FDO_PATH, &fdo)
        .method_call(move |_, sender, _, _, method, params, inv| {
            let Some(s) = w.upgrade() else { return };
            fdo_call(&s, sender.unwrap_or(""), method, &params, inv);
        })
        .build();
    if let Some(vec) = iface(VEC_XML, VEC) {
        let w = Rc::downgrade(store);
        let _ = conn
            .register_object(VEC_PATH, &vec)
            .method_call(move |_, _, _, _, method, params, inv| {
                let Some(s) = w.upgrade() else { return };
                vec_call(&s, method, &params, inv);
            })
            .build();
        let w = Rc::downgrade(store);
        store.changed.connect(move || {
            if let Some(s) = w.upgrade()
                && let Some(c) = self::conn(&s)
            {
                let _ = c.emit_signal(None, VEC_PATH, VEC, "Changed", None);
            }
        });
    }
    let w = Rc::downgrade(store);
    let w2 = Rc::downgrade(store);
    let _ = conn
        .register_object(MAKO_PATH, &mako)
        .method_call(move |_, _, _, _, method, params, inv| {
            let Some(s) = w.upgrade() else { return };
            mako_call(&s, method, &params, inv);
        })
        .property(move |_, _, _, _, prop| match (prop, w2.upgrade()) {
            ("Modes", Some(s)) => s.modes().to_variant(),
            _ => Vec::<String>::new().to_variant(),
        })
        .build();
    // mako (started by Omarchy's autostart, or still running from before)
    // does not allow replacement: we queue, and ask it to leave
    let w = Rc::downgrade(store);
    let w2 = Rc::downgrade(store);
    let _ = gio::bus_own_name_on_connection(
        &conn,
        BUS,
        gio::BusNameOwnerFlags::REPLACE,
        move |_, _| {
            if let Some(s) = w.upgrade() {
                s.serving.set(true);
                s.changed.emit();
            }
        },
        move |c, _| {
            if let Some(s) = w2.upgrade() {
                s.serving.set(false);
            }
            evict_mako(&c);
        },
    );
}

/// Stops a mako that holds the name; our queued request then gets it.
fn evict_mako(c: &gio::DBusConnection) {
    let c = c.clone();
    glib::spawn_future_local(async move {
        let r = c
            .call_future(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "GetConnectionUnixProcessID",
                Some(&(BUS,).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                1000,
            )
            .await;
        let Some(pid) = r.ok().and_then(|v| v.child_value(0).get::<u32>()) else {
            return;
        };
        let comm = crate::util::read_trim(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm == "mako" {
            eprintln!("vector-bar: notifications: asking mako ({pid}) to step aside");
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        } else {
            eprintln!(
                "vector-bar: notifications: {comm} ({pid}) serves notifications; waiting for it to quit"
            );
        }
    });
}

fn hint(h: &glib::VariantDict, keys: &[&str]) -> Option<glib::Variant> {
    keys.iter().find_map(|k| h.lookup_value(k, None))
}

fn hint_str(h: &glib::VariantDict, keys: &[&str]) -> String {
    hint(h, keys)
        .and_then(|v| v.str().map(str::to_string))
        .unwrap_or_default()
}

fn hint_bool(h: &glib::VariantDict, key: &str) -> bool {
    hint(h, &[key])
        .and_then(|v| {
            v.get::<bool>()
                .or_else(|| v.get::<u8>().map(|b| b != 0))
                .or_else(|| v.get::<i32>().map(|i| i != 0))
        })
        .unwrap_or(false)
}

/// image-data (iiibiiay) as a texture.
fn image_data(v: &glib::Variant) -> Option<gtk::gdk::Texture> {
    if v.n_children() != 7 {
        return None;
    }
    let w = v.child_value(0).get::<i32>()?;
    let h = v.child_value(1).get::<i32>()?;
    let stride = v.child_value(2).get::<i32>()?;
    let alpha = v.child_value(3).get::<bool>()?;
    let bits = v.child_value(4).get::<i32>()?;
    let channels = v.child_value(5).get::<i32>()?;
    let data = v.child_value(6);
    if w <= 0
        || h <= 0
        || bits != 8
        || !(channels == 3 || channels == 4)
        || (channels == 4) != alpha
    {
        return None;
    }
    let bytes = data.data_as_bytes();
    // a stride shorter than a row (or negative) would read past the data
    let (h_, row) = (h as usize, w as usize * channels as usize);
    let stride = usize::try_from(stride).ok().filter(|&s| s >= row)?;
    let need = stride
        .checked_mul(h_ - 1)
        .and_then(|n| n.checked_add(row))?;
    if bytes.len() < need {
        return None;
    }
    let fmt = if alpha {
        gtk::gdk::MemoryFormat::R8g8b8a8
    } else {
        gtk::gdk::MemoryFormat::R8g8b8
    };
    Some(gtk::gdk::MemoryTexture::new(w, h, fmt, &bytes, stride).upcast())
}

/// A path, a file:// URI, or nothing (an icon name).
fn as_file(s: &str) -> Option<String> {
    let p = s.strip_prefix("file://").unwrap_or(s);
    (p.starts_with('/') && std::path::Path::new(p).is_file()).then(|| p.to_string())
}

fn fdo_call(
    s: &Rc<Store>,
    _sender: &str,
    method: &str,
    p: &glib::Variant,
    inv: gio::DBusMethodInvocation,
) {
    match method {
        "GetCapabilities" => {
            let caps: Vec<&str> = vec![
                "actions",
                "body",
                "body-markup",
                "body-hyperlinks",
                "icon-static",
                "persistence",
                "x-canonical-private-synchronous",
            ];
            inv.return_value(Some(&(caps,).to_variant()));
        }
        "GetServerInformation" => {
            inv.return_value(Some(
                &(
                    "Vector Notifications",
                    "VectorOS",
                    env!("CARGO_PKG_VERSION"),
                    "1.2",
                )
                    .to_variant(),
            ));
        }
        "CloseNotification" => {
            let id = p.child_value(0).get::<u32>().unwrap_or(0);
            s.close(id, 3);
            inv.return_value(None);
        }
        "Notify" => {
            let str_at = |i: usize| p.child_value(i).str().unwrap_or("").to_string();
            let replaces = p.child_value(1).get::<u32>().unwrap_or(0);
            let actions_flat: Vec<String> =
                p.child_value(5).get::<Vec<String>>().unwrap_or_default();
            let hints = glib::VariantDict::new(Some(&p.child_value(6)));
            let timeout = p.child_value(7).get::<i32>().unwrap_or(-1);
            let icon = str_at(2);
            let mut n = Note {
                app: str_at(0),
                icon: icon.clone(),
                summary: str_at(3),
                body: str_at(4),
                actions: actions_flat
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| (c[0].clone(), c[1].clone()))
                    .collect(),
                urgency: hint(&hints, &["urgency"])
                    .and_then(|v| v.get::<u8>())
                    .unwrap_or(1)
                    .min(2),
                category: hint_str(&hints, &["category"]),
                desktop_entry: hint_str(&hints, &["desktop-entry"]),
                value: hint(&hints, &["value"])
                    .and_then(|v| v.get::<i32>().or_else(|| v.get::<u32>().map(|u| u as i32)))
                    .map(|v| v.clamp(0, 100)),
                transient: hint_bool(&hints, "transient"),
                phone: hint_bool(&hints, "x-phonectl"),
                resident: hint_bool(&hints, "resident"),
                tag: hint_str(
                    &hints,
                    &[
                        "x-canonical-private-synchronous",
                        "synchronous",
                        "x-dunst-stack-tag",
                    ],
                ),
                ..Default::default()
            };
            n.pixels = hint(&hints, &["image-data", "image_data", "icon_data"])
                .and_then(|v| image_data(&v));
            let path = hint_str(&hints, &["image-path", "image_path"]);
            if n.pixels.is_none() {
                if let Some(f) = as_file(&path).or_else(|| as_file(&icon)) {
                    n.image = f;
                } else if !path.is_empty() && n.icon.is_empty() {
                    n.icon = path; // an icon name passed as image-path
                }
            }
            let id = s.notify(n, replaces, timeout);
            inv.return_value(Some(&(id,).to_variant()));
        }
        _ => inv.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
    }
}

fn note_dict(n: &Note) -> glib::Variant {
    let d = glib::VariantDict::new(None);
    d.insert("app-name", &n.app);
    d.insert("app-icon", &n.icon);
    d.insert("category", &n.category);
    d.insert("desktop-entry", &n.desktop_entry);
    d.insert("summary", &n.summary);
    d.insert("body", &n.body);
    d.insert("id", n.id);
    d.insert("urgency", n.urgency);
    let acts: std::collections::HashMap<String, String> = n.actions.iter().cloned().collect();
    d.insert_value("actions", &acts.to_variant());
    d.end()
}

fn notes(v: &[Note]) -> glib::Variant {
    let items: Vec<glib::Variant> = v.iter().map(note_dict).collect();
    let arr = glib::Variant::array_from_iter_with_type(glib::VariantTy::VARDICT, items);
    glib::Variant::tuple_from_iter([arr])
}

fn mako_call(s: &Rc<Store>, method: &str, p: &glib::Variant, inv: gio::DBusMethodInvocation) {
    match method {
        "DismissNotifications" => {
            let d = glib::VariantDict::new(Some(&p.child_value(0)));
            let id = d
                .lookup_value("id", None)
                .and_then(|v| v.get::<u32>())
                .unwrap_or(0);
            let b = |k: &str| d.lookup_value(k, None).and_then(|v| v.get::<bool>());
            let (group, all, history) = (
                b("group").unwrap_or(false),
                b("all").unwrap_or(false),
                b("history").unwrap_or(true),
            );
            if (all && group) || ((all || group) && id != 0) {
                return inv.return_dbus_error(
                    "org.freedesktop.DBus.Error.InvalidArgs",
                    "id, group and all don't go together",
                );
            }
            s.dismiss_by(id, group, all, history);
            inv.return_value(None);
        }
        "InvokeAction" => {
            let id = p.child_value(0).get::<u32>().unwrap_or(0);
            let key = p.child_value(1).str().unwrap_or("default").to_string();
            s.invoke(id, &key);
            inv.return_value(None);
        }
        "RestoreNotification" => {
            s.restore();
            inv.return_value(None);
        }
        "ListNotifications" => inv.return_value(Some(&notes(&s.live()))),
        "ListHistory" => inv.return_value(Some(&notes(&s.history()))),
        "Reload" => {
            // the bar reloads its config by itself when the file changes
            s.changed.emit();
            inv.return_value(None);
        }
        "SetMode" => {
            let m = p.child_value(0).str().unwrap_or("default").to_string();
            s.set_modes(vec![m]);
            inv.return_value(None);
        }
        "SetModes" => {
            let m = p.child_value(0).get::<Vec<String>>().unwrap_or_default();
            s.set_modes(m);
            inv.return_value(None);
        }
        "ListModes" => inv.return_value(Some(&(s.modes(),).to_variant())),
        _ => inv.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
    }
}

/// One entry for Vector Space: plain text, the saved picture, the time.
fn vec_dict(n: &Note, live: bool) -> glib::Variant {
    let d = glib::VariantDict::new(None);
    d.insert("id", n.id);
    d.insert("app", n.label());
    d.insert("summary", super::plain(&n.summary));
    d.insert("body", super::plain(&n.body));
    d.insert("urgency", n.urgency as u32);
    d.insert("image", &n.image);
    d.insert("time", n.time);
    d.insert("count", n.count.max(1));
    d.insert("unseen", n.unseen);
    d.insert("live", live);
    d.insert("phone", n.phone);
    let acts: Vec<(String, String)> = n.actions.clone();
    d.insert_value("actions", &acts.to_variant());
    d.end()
}

fn vec_call(s: &Rc<Store>, method: &str, p: &glib::Variant, inv: gio::DBusMethodInvocation) {
    let id = || p.child_value(0).get::<u32>().unwrap_or(0);
    let live = |id: u32| s.live().iter().any(|n| n.id == id);
    match method {
        "List" => {
            let items: Vec<glib::Variant> = s
                .live()
                .iter()
                .filter(|n| !n.hidden)
                .map(|n| vec_dict(n, true))
                .chain(s.history().iter().map(|n| vec_dict(n, false)))
                .collect();
            let arr = glib::Variant::array_from_iter_with_type(glib::VariantTy::VARDICT, items);
            return inv.return_value(Some(&glib::Variant::tuple_from_iter([arr])));
        }
        "Activate" => {
            let id = id();
            if live(id) {
                s.activate(id);
            } else if let Some(n) = s.history().into_iter().find(|n| n.id == id) {
                super::focus_app(&n);
            }
        }
        "Invoke" => {
            let key = p.child_value(1).str().unwrap_or("default").to_string();
            s.invoke(id(), &key);
        }
        "Dismiss" => {
            let id = id();
            if live(id) {
                s.dismiss(id);
            } else {
                s.forget(id);
            }
        }
        "ClearHistory" => s.clear_history(),
        "MarkSeen" => s.mark_seen(),
        "GetDnd" => return inv.return_value(Some(&(s.dnd(),).to_variant())),
        "SetDnd" => s.set_dnd(p.child_value(0).get::<bool>().unwrap_or(false)),
        _ => return inv.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", method),
    }
    inv.return_value(None);
}

fn conn(s: &Store) -> Option<gio::DBusConnection> {
    s.serving.get().then(|| s.conn.borrow().clone()).flatten()
}

pub fn closed(s: &Store, id: u32, reason: u32) {
    if let Some(c) = conn(s) {
        let _ = c.emit_signal(
            None,
            FDO_PATH,
            FDO,
            "NotificationClosed",
            Some(&(id, reason).to_variant()),
        );
    }
}

/// ActionInvoked, preceded by an xdg-activation token so the app may raise
/// its window (the click on our card is the user input that allows it).
pub fn action(s: &Store, id: u32, key: &str) {
    let Some(c) = conn(s) else { return };
    if let Some(d) = gtk::gdk::Display::default()
        && let Some(tok) = d
            .app_launch_context()
            .startup_notify_id(None::<&gio::AppInfo>, &[])
    {
        let _ = c.emit_signal(
            None,
            FDO_PATH,
            FDO,
            "ActivationToken",
            Some(&(id, tok.as_str()).to_variant()),
        );
    }
    let _ = c.emit_signal(
        None,
        FDO_PATH,
        FDO,
        "ActionInvoked",
        Some(&(id, key).to_variant()),
    );
}

fn props_changed(s: &Store, prop: &str, value: glib::Variant) {
    if let Some(c) = conn(s) {
        let changed = glib::VariantDict::new(None);
        changed.insert_value(prop, &value);
        let body = glib::Variant::tuple_from_iter([
            MAKO.to_variant(),
            changed.end(),
            Vec::<String>::new().to_variant(),
        ]);
        let _ = c.emit_signal(
            None,
            MAKO_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&body),
        );
    }
}

pub fn modes_changed(s: &Store) {
    props_changed(s, "Modes", s.modes().to_variant());
}

/// mako announces its list changing the same way; clients re-read on it.
pub fn notifications_changed(s: &Store) {
    if let Some(c) = conn(s) {
        let body = glib::Variant::tuple_from_iter([
            MAKO.to_variant(),
            glib::VariantDict::new(None).end(),
            vec!["Notifications".to_string()].to_variant(),
        ]);
        let _ = c.emit_signal(
            None,
            MAKO_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&body),
        );
    }
}
