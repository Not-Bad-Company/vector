//! Bluetooth over BlueZ (D-Bus, event driven). Bar: on / connected / off.
//! Click: panel (power, devices with battery, scan, pair with its own agent,
//! connect, disconnect, forget). Right: bluetui.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::dbus::{self, Fail};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::ui::{self, Tap};
use crate::util;

const BLUEZ: &str = "org.bluez";
const AGENT_PATH: &str = "/org/vectoros/bar/btagent";
const AGENT_XML: &str = r#"<node><interface name="org.bluez.Agent1">
<method name="Release"/>
<method name="RequestPinCode"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="DisplayPinCode"><arg type="o" direction="in"/><arg type="s" direction="in"/></method>
<method name="RequestPasskey"><arg type="o" direction="in"/><arg type="u" direction="out"/></method>
<method name="DisplayPasskey"><arg type="o" direction="in"/><arg type="u" direction="in"/><arg type="q" direction="in"/></method>
<method name="RequestConfirmation"><arg type="o" direction="in"/><arg type="u" direction="in"/></method>
<method name="RequestAuthorization"><arg type="o" direction="in"/></method>
<method name="AuthorizeService"><arg type="o" direction="in"/><arg type="s" direction="in"/></method>
<method name="Cancel"/>
</interface></node>"#;

fn dev_icon(kind: &str) -> &'static str {
    match kind {
        "audio-headset" | "audio-headphones" => "\u{f02cb}",
        "audio-card" => "\u{f04c3}",
        "input-mouse" => "\u{f037d}",
        "input-keyboard" => "\u{f030c}",
        "input-gaming" => "\u{f0296}",
        "phone" => "\u{f03f2}",
        "computer" | "video-display" => "\u{f0379}",
        "input-tablet" => "\u{f04f7}",
        _ => "\u{f00af}",
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dev {
    pub path: String,
    pub name: String,
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<i64>,
    pub rssi: Option<i64>,
    pub address: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub adapter: Option<String>,
    pub powered: bool,
    pub discovering: bool,
    pub devs: Vec<Dev>,
}

async fn collect(c: &gio::DBusConnection) -> State {
    let objs = dbus::managed(c, BLUEZ, "/").await;
    let Some((ap, ai)) = objs.iter().find(|(_, i)| i.contains_key("org.bluez.Adapter1")) else { return State::default() };
    let a = &ai["org.bluez.Adapter1"];
    let mut devs = Vec::new();
    for (p, i) in &objs {
        let Some(d) = i.get("org.bluez.Device1") else { continue };
        if !p.starts_with(&format!("{ap}/")) {
            continue;
        }
        let name = {
            let a = dbus::get_str(d, "Alias");
            if a.is_empty() { dbus::get_str(d, "Name") } else { a }
        };
        // anonymous beacons: no name of their own and never paired
        if name.is_empty() || (!dbus::get_bool(d, "Paired") && dbus::get_str(d, "Name").is_empty()) {
            continue;
        }
        devs.push(Dev {
            path: p.clone(),
            name,
            icon: dbus::get_str(d, "Icon"),
            paired: dbus::get_bool(d, "Paired"),
            connected: dbus::get_bool(d, "Connected"),
            battery: i.get("org.bluez.Battery1").and_then(|b| dbus::get_i64(b, "Percentage")),
            rssi: dbus::get_i64(d, "RSSI"),
            address: dbus::get_str(d, "Address"),
        });
    }
    devs.sort_by_key(|d| (!d.connected, d.name.to_lowercase()));
    State { adapter: Some(ap.clone()), powered: dbus::get_bool(a, "Powered"), discovering: dbus::get_bool(a, "Discovering"), devs }
}

fn bt_error(f: &Fail) -> String {
    let m = f.message.to_lowercase();
    if f.name == "Timeout" {
        return "timed out: is it on and nearby?".into();
    }
    if m.contains("page-timeout") || m.contains("page timeout") || m.contains("host is down") {
        return "no answer: is it on and nearby?".into();
    }
    if m.contains("profile-unavailable") {
        return "nothing on it this laptop can use (audio not running?)".into();
    }
    match f.name.as_str() {
        "AuthenticationFailed" | "AuthenticationRejected" | "Rejected" => "pairing refused: put it in pairing mode".into(),
        "AuthenticationCanceled" => "pairing cancelled".into(),
        "AuthenticationTimeout" => "pairing timed out".into(),
        "ConnectionAttemptFailed" => "couldn't reach it".into(),
        "InProgress" => "busy with it already, try again in a moment".into(),
        "NotReady" => "adapter not ready".into(),
        "DoesNotExist" => "it went away: scan again".into(),
        _ => m,
    }
}

struct Inner {
    slot: Slot,
    conn: Option<gio::DBusConnection>,
    state: RefCell<State>,
    changed: Notify,
    pending: Cell<bool>,
    _sub: RefCell<Option<dbus::Sub>>,
}

pub struct Bluetooth(Rc<Inner>);

impl Bluetooth {
    pub fn new(ctx: &Ctx) -> Bluetooth {
        let o = ctx.opts("bluetooth");
        let inner = Rc::new(Inner { slot: ctx.slot("bluetooth", &[]), conn: dbus::system(), state: RefCell::default(), changed: Notify::default(), pending: Cell::new(false), _sub: RefCell::default() });
        let right = o.str("on_right", "omarchy-launch-bluetooth");
        inner.slot.on_right(move || util::spawn_sh(&right));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        if let Some(c) = &inner.conn {
            let w = Rc::downgrade(&inner);
            *inner._sub.borrow_mut() = Some(dbus::subscribe(c, BLUEZ, move |_, _, _| {
                if let Some(i) = w.upgrade() {
                    i.schedule();
                }
            }));
        }
        inner.query();
        util::every(10_000, &inner, |i| i.query());
        Bluetooth(inner)
    }
}

impl Inner {
    fn schedule(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        util::after(80, self, |i| {
            i.pending.set(false);
            i.query();
        });
    }

    fn query(self: &Rc<Self>) {
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let s = collect(&c).await;
            let Some(i) = w.upgrade() else { return };
            let changed = *i.state.borrow() != s;
            *i.state.borrow_mut() = s;
            i.render();
            if changed {
                i.changed.emit();
            }
        });
    }

    fn render(&self) {
        let s = self.state.borrow();
        if s.adapter.is_none() {
            self.slot.set_visible(false);
            return;
        }
        self.slot.set_visible(true);
        let conn = s.devs.iter().filter(|d| d.connected).count();
        let (icon, cls) = if !s.powered { ("󰂲", "dim") } else if conn > 0 { ("󰂱", "") } else { ("", "") };
        self.slot.set_markup(icon);
        self.slot.set_classes(&[cls]);
        self.slot.bump();
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = self.state.borrow();
        let sh = ui::sheet();
        let conn: Vec<&Dev> = s.devs.iter().filter(|d| d.connected).collect();
        let detail = if !s.powered { "off".to_string() } else if conn.is_empty() { "on".into() } else { format!("{} connected", conn.len()) };
        sh.append(&ui::head("bluetooth", &detail).0);
        if !conn.is_empty() {
            sh.append(&ui::tsection("devices"));
            for d in conn {
                let row = ui::hbox(8, &[]);
                row.append(&ui::text(dev_icon(&d.icon), &["ok"]));
                let n = ui::text(&d.name, &[]);
                n.set_hexpand(true);
                row.append(&n);
                if let Some(b) = d.battery {
                    let cls = if b <= 10 { "crit" } else if b <= 20 { "warn" } else { "" };
                    let m = ui::meter(b as f64 / 100.0, cls);
                    m.set_size_request(48, -1);
                    m.set_hexpand(false);
                    row.append(&m);
                    row.append(&ui::text(&format!("{b}%"), &[cls]));
                }
                sh.append(&row);
            }
        }
        sh.append(&ui::thint("click devices  ·  right bluetui"));
        sh.upcast()
    }
}

impl Module for Bluetooth {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.query();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

// ── panel ────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct PState {
    busy: HashMap<String, String>,
    errors: HashMap<String, String>,
    codes: HashMap<String, String>,
    pairing: HashSet<String>,
    expanded: Option<String>,
    agent: Option<gio::RegistrationId>,
    agent_ok: bool,
    by_us: bool,
    show_nearby: bool,
    switch_hold: Option<(bool, std::time::Instant)>,
    scan_hold: Option<(bool, std::time::Instant)>,
}

type Act = Rc<dyn Fn(String)>;

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(350);
    let card = Card::new("bluetooth", "");
    let Some(conn) = i.conn.clone() else {
        card.body.append(&ui::text("no system bus", &["empty-note"]));
        return card.root.upcast();
    };
    let ps: Rc<RefCell<PState>> = Rc::default();
    let sw = gtk::Switch::new();
    sw.set_valign(gtk::Align::Center);
    card.controls.append(&sw);
    let guard = Rc::new(Cell::new(false));
    let msgs = ui::vbox(0, &[]);
    card.body.append(&msgs);
    let list = ui::KeyedList::new();
    card.body.append(&list.root);

    let spin = gtk::Spinner::new();
    let scan_l = ui::text("scan for devices", &[]);
    let sb = ui::hbox(6, &[]);
    sb.append(&spin);
    sb.append(&scan_l);
    sb.append(&ui::text("s", &["faint"]));
    let scan_btn = gtk::Button::new();
    scan_btn.set_child(Some(&sb));
    scan_btn.add_css_class("link");
    ui::pointer(&scan_btn);
    card.foot_add(&scan_btn);
    card.foot_add(&ui::spacer());
    card.foot_add(&ui::link("bluetui", &[], {
        let p = p.clone();
        move || {
            p.close();
            util::spawn_sh("omarchy-launch-bluetooth");
        }
    }));

    let draw: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
    let rr: Rc<dyn Fn()> = {
        let d = draw.clone();
        Rc::new(move || {
            let f = d.borrow().clone();
            if let Some(f) = f {
                f();
            }
        })
    };

    // pairing agent: only devices the user started pairing from here are accepted
    if let Ok(node) = gio::DBusNodeInfo::for_xml(AGENT_XML) {
        if let Some(iface) = node.lookup_interface("org.bluez.Agent1") {
            let (ps2, rr2) = (ps.clone(), rr.clone());
            let reg = conn
                .register_object(AGENT_PATH, &iface)
                .method_call(move |_, _, _, _, method, params, inv| {
                    let dev = (params.n_children() > 0).then(|| params.child_value(0).str().unwrap_or("").to_string()).unwrap_or_default();
                    let ours = ps2.borrow().pairing.contains(&dev);
                    {
                        let mut s = ps2.borrow_mut();
                        match method {
                            "DisplayPasskey" => {
                                let k = params.child_value(1).get::<u32>().unwrap_or(0);
                                s.codes.insert(dev.clone(), format!("type {k:06} on the device, then enter"));
                            }
                            "DisplayPinCode" => {
                                let k = params.child_value(1).str().unwrap_or("").to_string();
                                s.codes.insert(dev.clone(), format!("type {k} on the device, then enter"));
                            }
                            "RequestConfirmation" if ours => {
                                let k = params.child_value(1).get::<u32>().unwrap_or(0);
                                s.codes.insert(dev.clone(), format!("code {k:06}: confirm on the device if it asks"));
                            }
                            _ => {}
                        }
                    }
                    match method {
                        "Release" | "Cancel" | "DisplayPinCode" | "DisplayPasskey" => inv.return_value(None),
                        "RequestConfirmation" | "RequestAuthorization" | "AuthorizeService" if ours => inv.return_value(None),
                        "RequestPinCode" if ours => inv.return_value(Some(&("0000",).to_variant())),
                        _ => inv.return_dbus_error("org.bluez.Error.Rejected", "not started from the panel"),
                    }
                    rr2();
                })
                .build();
            if let Ok(id) = reg {
                ps.borrow_mut().agent = Some(id);
                let (c2, ps3) = (conn.clone(), ps.clone());
                glib::spawn_future_local(async move {
                    let path = glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path");
                    let r = dbus::call(&c2, BLUEZ, "/org/bluez", "org.bluez.AgentManager1", "RegisterAgent", Some((path, "DisplayYesNo").to_variant()), 3000).await;
                    ps3.borrow_mut().agent_ok = r.is_ok() || r.err().is_some_and(|f| f.name == "AlreadyExists");
                });
            }
        }
    }
    {
        let (c2, ps2, i2) = (conn.clone(), ps.clone(), Rc::downgrade(i));
        p.on_close(move || {
            let mut s = ps2.borrow_mut();
            if s.by_us {
                if let Some(a) = i2.upgrade().and_then(|i| i.state.borrow().adapter.clone()) {
                    c2.call(Some(BLUEZ), &a, "org.bluez.Adapter1", "StopDiscovery", None, None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE, |_| {});
                }
            }
            if s.agent_ok {
                let path = glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path");
                let _ = c2.call_sync(Some(BLUEZ), "/org/bluez", "org.bluez.AgentManager1", "UnregisterAgent", Some(&(path,).to_variant()), None, gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE);
            }
            if let Some(id) = s.agent.take() {
                let _ = c2.unregister_object(id);
            }
        });
    }

    let adapter = {
        let i = Rc::downgrade(i);
        move || i.upgrade().and_then(|i| i.state.borrow().adapter.clone())
    };
    let scan: Rc<dyn Fn(bool)> = {
        let (c, ps, rr, adapter) = (conn.clone(), ps.clone(), rr.clone(), adapter.clone());
        Rc::new(move |on: bool| {
            let Some(a) = adapter() else { return };
            {
                let mut s = ps.borrow_mut();
                s.scan_hold = Some((on, std::time::Instant::now()));
                s.errors.remove("scan");
                s.by_us = on;
                if on {
                    s.show_nearby = true;
                }
            }
            rr();
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                if let Err(f) = dbus::call(&c, BLUEZ, &a, "org.bluez.Adapter1", if on { "StartDiscovery" } else { "StopDiscovery" }, None, 5000).await {
                    if f.name != "InProgress" {
                        let mut s = ps.borrow_mut();
                        s.scan_hold = None;
                        s.by_us = false;
                        s.errors.insert("scan".into(), if on { format!("couldn't start scanning: {}", bt_error(&f)) } else { "another app started this scan: stop it there".into() });
                    }
                }
                rr();
            });
        })
    };
    // discovery slows pairing/connecting down: stop our scan first (results stay listed)
    let quiet = {
        let (c, ps, adapter) = (conn.clone(), ps.clone(), adapter.clone());
        move || {
            let mut s = ps.borrow_mut();
            if s.by_us {
                s.by_us = false;
                s.scan_hold = None;
                if let Some(a) = adapter() {
                    c.call(Some(BLUEZ), &a, "org.bluez.Adapter1", "StopDiscovery", None, None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE, |_| {});
                }
            }
        }
    };
    let connect: Rc<dyn Fn(String, bool)> = {
        let (c, ps, rr) = (conn.clone(), ps.clone(), rr.clone());
        let me: Rc<RefCell<Option<Rc<dyn Fn(String, bool)>>>> = Rc::default();
        let me2 = me.clone();
        let f: Rc<dyn Fn(String, bool)> = Rc::new(move |key: String, retry: bool| {
            ps.borrow_mut().busy.insert(key.clone(), "connecting".into());
            rr();
            let (c, ps, rr, me) = (c.clone(), ps.clone(), rr.clone(), me2.clone());
            glib::spawn_future_local(async move {
                let r = dbus::call(&c, BLUEZ, &key, "org.bluez.Device1", "Connect", None, 45_000).await;
                let err = r.err().filter(|f| f.name != "AlreadyConnected");
                if let Some(f) = &err {
                    let m = f.message.to_lowercase();
                    if retry && (m.contains("profile-unavailable") || m.contains("abort-by-local") || f.name == "InProgress") {
                        // common right after pairing or while audio profiles come up
                        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                            let f = me.borrow().clone();
                            if let Some(f) = f {
                                f(key, false);
                            }
                        });
                        return;
                    }
                }
                let mut s = ps.borrow_mut();
                s.busy.remove(&key);
                if let Some(f) = err {
                    s.errors.insert(key, bt_error(&f));
                }
                drop(s);
                rr();
            });
        });
        *me.borrow_mut() = Some(f.clone());
        f
    };
    let act: Act = {
        let (c, ps, rr, i, connect) = (conn.clone(), ps.clone(), rr.clone(), Rc::downgrade(i), connect.clone());
        let quiet = Rc::new(quiet);
        Rc::new(move |key: String| {
            let Some(i) = i.upgrade() else { return };
            let Some(d) = i.state.borrow().devs.iter().find(|d| d.path == key).cloned() else { return };
            if ps.borrow().busy.contains_key(&key) {
                return;
            }
            {
                let mut s = ps.borrow_mut();
                s.errors.remove(&key);
                s.expanded = None;
            }
            if d.connected {
                ps.borrow_mut().busy.insert(key.clone(), "disconnecting".into());
                rr();
                let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
                glib::spawn_future_local(async move {
                    let r = dbus::call(&c, BLUEZ, &key, "org.bluez.Device1", "Disconnect", None, 15_000).await;
                    let mut s = ps.borrow_mut();
                    s.busy.remove(&key);
                    if let Err(f) = r {
                        s.errors.insert(key, bt_error(&f));
                    }
                    drop(s);
                    rr();
                });
            } else if d.paired {
                quiet();
                connect(key, true);
            } else {
                quiet();
                {
                    let mut s = ps.borrow_mut();
                    s.busy.insert(key.clone(), "pairing".into());
                    s.pairing.insert(key.clone());
                }
                rr();
                let (c, ps, rr, connect) = (c.clone(), ps.clone(), rr.clone(), connect.clone());
                glib::spawn_future_local(async move {
                    let r = dbus::call(&c, BLUEZ, &key, "org.bluez.Device1", "Pair", None, 90_000).await;
                    {
                        let mut s = ps.borrow_mut();
                        s.codes.remove(&key);
                        s.pairing.remove(&key);
                    }
                    match r {
                        Err(f) if f.name != "AlreadyExists" => {
                            let mut s = ps.borrow_mut();
                            s.busy.remove(&key);
                            s.errors.insert(key, bt_error(&f));
                            drop(s);
                            rr();
                        }
                        _ => {
                            let _ = dbus::set_prop(&c, BLUEZ, &key, "org.bluez.Device1", "Trusted", true.to_variant()).await;
                            connect(key, true);
                        }
                    }
                });
            }
        })
    };
    let forget: Act = {
        let (c, ps, rr, adapter) = (conn.clone(), ps.clone(), rr.clone(), adapter.clone());
        Rc::new(move |key: String| {
            let Some(a) = adapter() else { return };
            ps.borrow_mut().expanded = None;
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                let path = glib::variant::ObjectPath::try_from(key.clone()).ok();
                if let Some(path) = path {
                    if let Err(f) = dbus::call(&c, BLUEZ, &a, "org.bluez.Adapter1", "RemoveDevice", Some((path,).to_variant()), 5000).await {
                        ps.borrow_mut().errors.insert(key, bt_error(&f));
                    }
                }
                rr();
            });
        })
    };

    // power
    {
        let (c, i, ps, guard, rr) = (conn.clone(), Rc::downgrade(i), ps.clone(), guard.clone(), rr.clone());
        sw.connect_state_set(move |_, on| {
            if guard.get() {
                return glib::Propagation::Proceed;
            }
            let Some(a) = i.upgrade().and_then(|i| i.state.borrow().adapter.clone()) else { return glib::Propagation::Proceed };
            ps.borrow_mut().switch_hold = Some((on, std::time::Instant::now()));
            ps.borrow_mut().errors.remove("power");
            if on {
                util::spawn(&["rfkill", "unblock", "bluetooth"]);
            }
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                // the adapter can take a moment after an rfkill unblock
                let mut last = None;
                for _ in 0..3 {
                    match dbus::set_prop(&c, BLUEZ, &a, "org.bluez.Adapter1", "Powered", on.to_variant()).await {
                        Ok(_) => {
                            last = None;
                            break;
                        }
                        Err(f) => {
                            last = Some(f);
                            glib::timeout_future(Duration::from_millis(400)).await;
                        }
                    }
                }
                if let Some(f) = last {
                    let mut s = ps.borrow_mut();
                    s.errors.insert("power".into(), format!("couldn't turn bluetooth {}: {}", if on { "on" } else { "off" }, f.message));
                    s.switch_hold = None;
                }
                rr();
            });
            glib::Propagation::Proceed
        });
    }
    {
        let (scan, ps, i) = (scan.clone(), ps.clone(), Rc::downgrade(i));
        let toggle = Rc::new(move || {
            let discovering = i.upgrade().map(|i| i.state.borrow().discovering).unwrap_or(false);
            let held = ps.borrow().scan_hold.filter(|(_, t)| t.elapsed() < Duration::from_secs(4)).map(|(w, _)| w);
            scan(!held.unwrap_or(discovering));
        });
        let t2 = toggle.clone();
        scan_btn.connect_clicked(move |_| t2());
        p.on_key(move |k, m| {
            if (k == gtk::gdk::Key::s || k == gtk::gdk::Key::S) && !m.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK) {
                toggle();
                return true;
            }
            false
        });
    }

    let render = {
        let i = Rc::downgrade(i);
        let (ps, sw, guard, msgs, list, detail, spin, scan_l, scan_btn) = (ps.clone(), sw.clone(), guard.clone(), msgs.clone(), list.clone(), card.detail.clone(), spin.clone(), scan_l.clone(), scan_btn.clone());
        let (act, forget, rr) = (act.clone(), forget.clone(), rr.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            let s = i.state.borrow().clone();
            let mut st = ps.borrow_mut();
            let mut shown = s.powered;
            if let Some((want, at)) = st.switch_hold {
                if want != s.powered && at.elapsed() < Duration::from_secs(4) {
                    shown = want;
                } else {
                    st.switch_hold = None;
                }
            }
            guard.set(true);
            sw.set_active(shown);
            sw.set_state(shown);
            guard.set(false);
            sw.set_visible(s.adapter.is_some());
            let conn: Vec<&Dev> = s.devs.iter().filter(|d| d.connected).collect();
            detail.set_markup(&util::esc(&if s.adapter.is_none() {
                "no adapter".to_string()
            } else if !s.powered {
                "off".into()
            } else if conn.len() == 1 {
                conn[0].name.clone()
            } else if !conn.is_empty() {
                format!("{} connected", conn.len())
            } else {
                "on".into()
            }));
            let mut scanning = s.discovering && s.powered;
            if let Some((want, at)) = st.scan_hold {
                if want != scanning && at.elapsed() < Duration::from_secs(4) {
                    scanning = want;
                } else {
                    st.scan_hold = None;
                }
            }
            spin.set_visible(scanning);
            if scanning {
                spin.start();
            } else {
                spin.stop();
            }
            scan_l.set_text(if scanning { "scanning… · stop" } else { "scan for devices" });
            scan_btn.set_visible(s.powered);
            ui::clear(&msgs);
            for k in ["power", "scan"] {
                if let Some(e) = st.errors.get(k) {
                    msgs.append(&ui::text(e, &["msg", "crit"]));
                }
            }
            let mut items: Vec<ui::Item> = Vec::new();
            let empty = |key: &str, t: &'static str| -> ui::Item { (key.to_string(), t.to_string(), Box::new(move || ui::text(t, &["empty-note"]).upcast())) };
            if s.adapter.is_none() {
                items.push(empty("e", "no bluetooth adapter found"));
            } else if !s.powered {
                items.push(empty("e", "bluetooth is off"));
            } else {
                let paired: Vec<Dev> = s.devs.iter().filter(|d| d.paired).cloned().collect();
                let mut nearby: Vec<Dev> = s.devs.iter().filter(|d| !d.paired).cloned().collect();
                nearby.sort_by_key(|d| -d.rssi.unwrap_or(-999));
                let mut group = |title: &'static str, devs: Vec<Dev>, items: &mut Vec<ui::Item>| {
                    items.push((format!("sec-{title}"), String::new(), Box::new(move || ui::section(title).upcast())));
                    for d in devs {
                        let busy = st.busy.get(&d.path).cloned();
                        let note = st.errors.get(&d.path).map(|e| (e.clone(), "crit")).or_else(|| st.codes.get(&d.path).map(|c| (c.clone(), "warn")));
                        let exp = st.expanded.as_deref() == Some(&d.path);
                        let sig = format!("{d:?}{busy:?}{note:?}{exp}");
                        let (act, forget, ps, rr) = (act.clone(), forget.clone(), ps.clone(), rr.clone());
                        items.push((d.path.clone(), sig, Box::new(move || dev_row(&d, busy.as_deref(), note.as_ref(), exp, &act, &forget, &ps, &rr))));
                    }
                };
                if !paired.is_empty() {
                    group("devices", paired.clone(), &mut items);
                }
                let any_busy = nearby.iter().any(|d| st.busy.contains_key(&d.path));
                if scanning || st.show_nearby || any_busy {
                    let none = nearby.is_empty();
                    group("nearby", nearby.into_iter().take(12).collect(), &mut items);
                    if none {
                        items.push(empty("none", if scanning { "looking for devices…" } else { "nothing found · s to scan again" }));
                    }
                } else if paired.is_empty() {
                    items.push(empty("e", "no devices yet · s to scan"));
                }
            }
            drop(st);
            list.sync(items);
        }
    };
    *draw.borrow_mut() = Some(Rc::new(render));
    rr();
    let rr2 = rr.clone();
    p.watch(&i.changed, move || rr2());
    card.root.upcast()
}

#[allow(clippy::too_many_arguments)]
fn dev_row(d: &Dev, busy: Option<&str>, note: Option<&(String, &str)>, expanded: bool, act: &Act, forget: &Act, ps: &Rc<RefCell<PState>>, rr: &Rc<dyn Fn()>) -> gtk::Widget {
    let b = ui::vbox(0, &[]);
    let (meta, cls) = match d.battery {
        Some(x) => (format!("{x}%"), if x <= 10 { "crit" } else if x <= 20 { "warn" } else { "" }),
        None if d.connected => ("connected".into(), ""),
        None if !d.paired => ("pair".into(), ""),
        None => (String::new(), ""),
    };
    let row = ui::Row::new(dev_icon(&d.icon), &d.name, &meta);
    if !cls.is_empty() {
        row.meta.add_css_class(cls);
    }
    row.set_active(d.connected);
    if let Some(bz) = busy {
        row.meta.set_text("");
        row.trail.append(&ui::spinner());
        row.trail.append(&ui::text(bz, &["meta"]));
    }
    if d.paired {
        let (ps, rr, key) = (ps.clone(), rr.clone(), d.path.clone());
        let more = ui::icon_button("\u{f01d9}", "more", move || {
            let mut s = ps.borrow_mut();
            s.expanded = if s.expanded.as_deref() == Some(&key) { None } else { Some(key.clone()) };
            drop(s);
            rr();
        });
        more.add_css_class("x");
        row.trail.append(&more);
    }
    let (a, key) = (act.clone(), d.path.clone());
    ui::on_click(&row.root, move || a(key.clone()));
    b.append(&row.root);
    if let Some((t, c)) = note {
        b.append(&ui::hbox(0, &["expand"]).tap(|x| x.append(&ui::text(t, &[c]))));
    }
    if expanded {
        let ex = ui::hbox(4, &["expand"]);
        let (a, key) = (act.clone(), d.path.clone());
        ex.append(&ui::link(if d.connected { "disconnect" } else { "connect" }, &[], move || a(key.clone())));
        let (f, key) = (forget.clone(), d.path.clone());
        ex.append(&ui::link("forget", &["danger"], move || f(key.clone())));
        ex.append(&ui::spacer());
        ex.append(&ui::text(&d.address.to_lowercase(), &["faint"]));
        b.append(&ex);
    }
    b.upcast()
}
