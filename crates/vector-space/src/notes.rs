//! Notifications, from Vector Bar's notification server over D-Bus
//! (`org.vectoros.Notifications` at `/org/vectoros/Notifications`).

use gio::prelude::*;

use crate::item::{Action, Icon, Item};
use crate::util::{self, Fuzzy};

const BUS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/vectoros/Notifications";
const IFACE: &str = "org.vectoros.Notifications";

#[derive(Clone, Debug, Default)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub urgency: u32,
    pub image: String,
    pub time: f64,
    pub count: u32,
    pub unseen: bool,
    /// still on screen
    pub live: bool,
    pub actions: Vec<(String, String)>,
}

/// What the server answered last; `None` while nobody serves notifications.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub notes: Vec<Note>,
    pub dnd: bool,
    pub ok: bool,
}

fn conn() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()
}

fn parse(v: &glib::Variant) -> Vec<Note> {
    let arr = v.child_value(0);
    (0..arr.n_children())
        .map(|i| {
            let d = glib::VariantDict::new(Some(&arr.child_value(i)));
            let s = |k: &str| d.lookup_value(k, None).and_then(|v| v.str().map(str::to_string)).unwrap_or_default();
            let u = |k: &str| d.lookup_value(k, None).and_then(|v| v.get::<u32>()).unwrap_or(0);
            let b = |k: &str| d.lookup_value(k, None).and_then(|v| v.get::<bool>()).unwrap_or(false);
            Note {
                id: u("id"),
                app: s("app"),
                summary: s("summary"),
                body: s("body"),
                urgency: u("urgency"),
                image: s("image"),
                time: d.lookup_value("time", None).and_then(|v| v.get::<f64>()).unwrap_or(0.0),
                count: u("count"),
                unseen: b("unseen"),
                live: b("live"),
                actions: d
                    .lookup_value("actions", None)
                    .and_then(|v| v.get::<Vec<(String, String)>>())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// Reads the list (async); `done` runs on the GTK thread.
pub fn fetch(done: impl FnOnce(Snapshot) + 'static) {
    let Some(c) = conn() else { return done(Snapshot::default()) };
    glib::spawn_future_local(async move {
        let list = c
            .call_future(Some(BUS), PATH, IFACE, "List", None, None, gio::DBusCallFlags::NONE, 2000)
            .await;
        let dnd = c
            .call_future(Some(BUS), PATH, IFACE, "GetDnd", None, None, gio::DBusCallFlags::NONE, 2000)
            .await
            .ok()
            .and_then(|v| v.child_value(0).get::<bool>())
            .unwrap_or(false);
        done(match list {
            Ok(v) => Snapshot { notes: parse(&v), dnd, ok: true },
            Err(_) => Snapshot::default(),
        })
    });
}

fn call(method: &str, args: Option<glib::Variant>) {
    let Some(c) = conn() else { return };
    c.call(Some(BUS), PATH, IFACE, method, args.as_ref(), None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE, |_| {});
}

pub fn activate(id: u32) {
    call("Activate", Some((id,).to_variant()));
}
pub fn invoke(id: u32, key: &str) {
    call("Invoke", Some((id, key).to_variant()));
}
pub fn dismiss(id: u32) {
    call("Dismiss", Some((id,).to_variant()));
}
pub fn clear_history() {
    call("ClearHistory", None);
}
pub fn mark_seen() {
    call("MarkSeen", None);
}
pub fn set_dnd(on: bool) {
    call("SetDnd", Some((on,).to_variant()));
}

/// Calls `f` whenever the server's list changes. Returns the subscription.
pub fn watch(f: impl Fn() + 'static) -> Option<(gio::DBusConnection, gio::SignalSubscription)> {
    let c = conn()?;
    let sub = c.subscribe_to_signal(Some(BUS), Some(IFACE), Some("Changed"), Some(PATH), None, gio::DBusSignalFlags::NONE, move |_| f());
    Some((c, sub))
}

/// The label a card shows: the app, or "VectorOS" for anonymous senders.
pub fn label(n: &Note) -> String {
    if n.app.is_empty() || n.app == "notify-send" { "VectorOS".into() } else { n.app.clone() }
}

fn glyph(n: &Note) -> &'static str {
    match n.urgency {
        2 => "\u{f0027}",
        0 => "\u{f009b}",
        _ => "\u{f009a}",
    }
}

pub fn query(s: &Snapshot, q: &str) -> Vec<Item> {
    let mut fz = Fuzzy::new(q);
    let mut out: Vec<(u32, bool, Item)> = Vec::new();
    for n in &s.notes {
        let title = if n.summary.is_empty() { label(n) } else { n.summary.clone() };
        let hay = format!("{} {} {}", title, n.body, n.app);
        let (score, hl) = if fz.empty {
            (0, vec![])
        } else {
            match (fz.indices(&title), fz.score(&hay)) {
                (Some((s, hl)), _) => (s + 20, hl),
                (None, Some(s)) => (s, vec![]),
                _ => continue,
            }
        };
        let icon = if !n.image.is_empty() && std::path::Path::new(&n.image).is_file() {
            Icon::File(n.image.clone().into())
        } else {
            Icon::Glyph(glyph(n))
        };
        let count = if n.count > 1 { format!(" ×{}", n.count) } else { String::new() };
        let mut it = Item::new(title, icon, Action::Note(n.id))
            .sub(util::one_line(&n.body, 110))
            .meta(format!("{} · {}{count}", label(n).to_lowercase(), util::age(n.time)))
            .hl(hl);
        if n.urgency >= 2 {
            it = it.class("critical");
        } else if n.unseen {
            it = it.class("unseen");
        }
        out.push((score, n.live, it));
    }
    if !fz.empty {
        out.sort_by_key(|x| std::cmp::Reverse(x.0));
    }
    // on screen first, then the history (newest first, as the server keeps it)
    out.sort_by_key(|x| !x.1);
    let mut items: Vec<Item> = Vec::new();
    let mut last = None;
    for (_, live, mut it) in out {
        if last != Some(live) {
            it.section = Some(if live { "on screen" } else { "history" }.into());
            last = Some(live);
        }
        items.push(it);
    }
    items
}
