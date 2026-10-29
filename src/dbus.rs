//! Small D-Bus helpers over GDBus (system bus services: iwd, BlueZ).

use std::collections::HashMap;

use gtk::gio;
use gtk::prelude::*;

pub type Props = HashMap<String, glib::Variant>;
/// path -> interface -> properties
pub type Objects = HashMap<String, HashMap<String, Props>>;

pub fn system() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).ok()
}

pub fn session() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()
}

/// a{sv} -> map (values unboxed).
pub fn props(v: &glib::Variant) -> Props {
    let mut m = HashMap::new();
    for i in 0..v.n_children() {
        let e = v.child_value(i);
        if let (Some(k), Some(val)) = (e.child_value(0).str().map(str::to_string), e.child_value(1).as_variant()) {
            m.insert(k, val);
        }
    }
    m
}

pub fn objects(v: &glib::Variant) -> Objects {
    // (a{oa{sa{sv}}})
    let mut out = HashMap::new();
    let top = v.child_value(0);
    for i in 0..top.n_children() {
        let e = top.child_value(i);
        let path = e.child_value(0).str().unwrap_or("").to_string();
        let ifs = e.child_value(1);
        let mut m = HashMap::new();
        for j in 0..ifs.n_children() {
            let ie = ifs.child_value(j);
            m.insert(ie.child_value(0).str().unwrap_or("").to_string(), props(&ie.child_value(1)));
        }
        out.insert(path, m);
    }
    out
}

pub async fn managed(c: &gio::DBusConnection, service: &str, root: &str) -> Objects {
    match c
        .call_future(Some(service), root, "org.freedesktop.DBus.ObjectManager", "GetManagedObjects", None, None, gio::DBusCallFlags::NONE, 3000)
        .await
    {
        Ok(v) => objects(&v),
        Err(_) => HashMap::new(),
    }
}

/// A failed call: the last part of the D-Bus error name ("Busy", "Failed", ...) and the message.
#[derive(Debug, Clone)]
pub struct Fail {
    pub name: String,
    pub message: String,
}

impl Fail {
    pub fn from(e: &glib::Error) -> Fail {
        let remote = gio::DBusError::remote_error(e).map(|s| s.to_string()).unwrap_or_default();
        let mut msg = e.message().to_string();
        if let Some((_, rest)) = msg.rsplit_once(": ") {
            msg = rest.to_string();
        }
        let name = if remote.is_empty() {
            if e.matches(gio::IOErrorEnum::TimedOut) { "Timeout".into() } else { String::new() }
        } else {
            remote.rsplit('.').next().unwrap_or("").to_string()
        };
        Fail { name, message: msg.to_lowercase() }
    }
}

pub async fn call(c: &gio::DBusConnection, service: &str, path: &str, iface: &str, method: &str, args: Option<glib::Variant>, timeout_ms: i32) -> Result<glib::Variant, Fail> {
    c.call_future(Some(service), path, iface, method, args.as_ref(), None, gio::DBusCallFlags::NONE, timeout_ms)
        .await
        .map_err(|e| Fail::from(&e))
}

pub async fn set_prop(c: &gio::DBusConnection, service: &str, path: &str, iface: &str, prop: &str, value: glib::Variant) -> Result<glib::Variant, Fail> {
    call(c, service, path, "org.freedesktop.DBus.Properties", "Set", Some((iface, prop, value).to_variant()), 5000).await
}

pub fn get_str(p: &Props, k: &str) -> String {
    p.get(k).and_then(|v| v.str().map(str::to_string).or_else(|| v.get::<glib::variant::ObjectPath>().map(|o| o.as_str().to_string()))).unwrap_or_default()
}

pub fn get_bool(p: &Props, k: &str) -> bool {
    p.get(k).and_then(|v| v.get::<bool>()).unwrap_or(false)
}

pub fn get_i64(p: &Props, k: &str) -> Option<i64> {
    let v = p.get(k)?;
    v.get::<i16>()
        .map(i64::from)
        .or_else(|| v.get::<i32>().map(i64::from))
        .or_else(|| v.get::<u32>().map(i64::from))
        .or_else(|| v.get::<u8>().map(i64::from))
        .or_else(|| v.get::<i64>())
        .or_else(|| v.get::<u16>().map(i64::from))
}

/// Subscribes to every signal from `service`; unsubscribes when the guard drops.
pub struct Sub(gio::DBusConnection, Option<gio::SignalSubscriptionId>);

impl Drop for Sub {
    fn drop(&mut self) {
        if let Some(id) = self.1.take() {
            self.0.signal_unsubscribe(id);
        }
    }
}

pub fn subscribe(c: &gio::DBusConnection, service: &str, f: impl Fn(&str, &str, &glib::Variant) + 'static) -> Sub {
    let id = c.signal_subscribe(Some(service), None, None, None, None, gio::DBusSignalFlags::NONE, move |_, _, path, _iface, member, args| {
        f(path, member, args)
    });
    Sub(c.clone(), Some(id))
}
