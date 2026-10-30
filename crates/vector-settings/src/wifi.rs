//! Wi-Fi over iwd (D-Bus): power, the connection and its details, networks in
//! range (connect, password, enterprise sign-in), every saved network
//! (auto-connect, forget), and joining a hidden network.
//!
//! Passwords reach iwd through an agent registered while the page is in view
//! (iwd allows one; the bar's Wi-Fi panel registers its own only while open).
//! Enterprise networks (eduroam) need a provisioning file in /var/lib/iwd,
//! written with pkexec.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;

use crate::dbus::{self, Fail, Objects};
use crate::page::{Ctx, Frame, View};
use crate::ui::{self, Keyed, Row};

const IWD: &str = "net.connman.iwd";
const BARS: [&str; 5] = ["󰤯", "󰤟", "󰤢", "󰤥", "󰤨"];
const AGENT_PATH: &str = "/org/vectoros/settings/iwdagent";
const AGENT_XML: &str = r#"<node><interface name="net.connman.iwd.Agent">
<method name="Release"/>
<method name="RequestPassphrase"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="RequestPrivateKeyPassphrase"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="RequestUserNameAndPassword"><arg type="o" direction="in"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
<method name="RequestUserPassword"><arg type="o" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="out"/></method>
<method name="Cancel"><arg type="s" direction="in"/></method>
</interface></node>"#;

#[derive(Clone, Default, PartialEq, Debug)]
struct Net {
    path: String,
    name: String,
    kind: String,
    connected: bool,
    connecting: bool,
    known: Option<String>,
    level: usize,
    dbm: i64,
}

#[derive(Clone, Default, PartialEq, Debug)]
struct Known {
    path: String,
    name: String,
    kind: String,
    hidden: bool,
    auto: bool,
    last: String,
}

#[derive(Clone, Default, PartialEq, Debug)]
struct State {
    adapter: Option<String>,
    device: Option<String>,
    iface: String,
    mac: String,
    powered: bool,
    station: bool,
    st_state: String,
    scanning: bool,
    nets: Vec<Net>,
    known: Vec<Known>,
    /// GetDiagnostics of the connection
    diag: HashMap<String, String>,
    ip: Option<String>,
    gateway: Option<String>,
}

impl State {
    fn current(&self) -> Option<&Net> {
        self.nets.iter().find(|n| n.connected)
    }
}

fn ipv4(iface: &str) -> Option<String> {
    let mut out = None;
    unsafe {
        let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifa) != 0 {
            return None;
        }
        let mut p = ifa;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null() && (*a.ifa_addr).sa_family as i32 == libc::AF_INET {
                let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy();
                if name == iface {
                    let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                    out = Some(std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)).to_string());
                    break;
                }
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(ifa);
    }
    out
}

/// Default gateway of an interface, from /proc/net/route.
fn gateway(iface: &str) -> Option<String> {
    let t = std::fs::read_to_string("/proc/net/route").ok()?;
    t.lines().skip(1).find_map(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        (f.len() > 2 && f[0] == iface && f[1] == "00000000").then(|| {
            let g = u32::from_str_radix(f[2], 16).unwrap_or(0);
            std::net::Ipv4Addr::from(g.to_le_bytes()).to_string()
        })
    })
}

fn variant_text(v: &glib::Variant) -> String {
    v.str()
        .map(str::to_string)
        .or_else(|| v.get::<i16>().map(|x| x.to_string()))
        .or_else(|| v.get::<i32>().map(|x| x.to_string()))
        .or_else(|| v.get::<u32>().map(|x| x.to_string()))
        .or_else(|| v.get::<u16>().map(|x| x.to_string()))
        .or_else(|| v.get::<u8>().map(|x| x.to_string()))
        .unwrap_or_default()
}

async fn collect(c: &gio::DBusConnection) -> State {
    let objs: Objects = dbus::managed(c, IWD, "/").await;
    let mut s = State {
        adapter: objs
            .iter()
            .find(|(_, i)| i.contains_key("net.connman.iwd.Adapter"))
            .map(|(p, _)| p.clone()),
        ..Default::default()
    };
    for (p, ifs) in &objs {
        if let Some(k) = ifs.get("net.connman.iwd.KnownNetwork") {
            s.known.push(Known {
                path: p.clone(),
                name: dbus::get_str(k, "Name"),
                kind: dbus::get_str(k, "Type"),
                hidden: dbus::get_bool(k, "Hidden"),
                auto: dbus::get_bool(k, "AutoConnect"),
                last: dbus::get_str(k, "LastConnectedTime"),
            });
        }
    }
    // most recently used first
    s.known.sort_by(|a, b| b.last.cmp(&a.last).then(a.name.cmp(&b.name)));
    let dev = objs.iter().find(|(_, i)| {
        i.get("net.connman.iwd.Device")
            .is_some_and(|d| dbus::get_str(d, "Mode") == "station")
    });
    let Some((dev, ifs)) = dev else { return s };
    s.device = Some(dev.clone());
    let d = &ifs["net.connman.iwd.Device"];
    s.powered = dbus::get_bool(d, "Powered");
    s.iface = dbus::get_str(d, "Name");
    s.mac = dbus::get_str(d, "Address");
    let Some(st) = ifs.get("net.connman.iwd.Station") else { return s };
    s.station = true;
    s.st_state = dbus::get_str(st, "State");
    s.scanning = dbus::get_bool(st, "Scanning");
    let target = dbus::get_str(st, "ConnectedNetwork");
    if let Ok(v) = dbus::call(c, IWD, dev, "net.connman.iwd.Station", "GetOrderedNetworks", None, 2000).await {
        let arr = v.child_value(0);
        for i in 0..arr.n_children() {
            let e = arr.child_value(i);
            let path = e.child_value(0).str().unwrap_or("").to_string();
            let strength = e.child_value(1).get::<i16>().map(i64::from).unwrap_or(-10000);
            let Some(n) = objs.get(&path).and_then(|x| x.get("net.connman.iwd.Network")) else {
                continue;
            };
            let dbm = strength / 100;
            let known = dbus::get_str(n, "KnownNetwork");
            s.nets.push(Net {
                path: path.clone(),
                name: dbus::get_str(n, "Name"),
                kind: dbus::get_str(n, "Type"),
                connected: dbus::get_bool(n, "Connected") && s.st_state != "connecting",
                connecting: path == target && matches!(s.st_state.as_str(), "connecting" | "roaming"),
                known: (!known.is_empty()).then_some(known),
                level: ((dbm + 90) / 10).clamp(0, 4) as usize,
                dbm,
            });
        }
    }
    if s.current().is_some() {
        if let Ok(v) = dbus::call(c, IWD, dev, "net.connman.iwd.StationDiagnostic", "GetDiagnostics", None, 1500).await {
            for (k, v) in dbus::props(&v.child_value(0)) {
                s.diag.insert(k, variant_text(&v));
            }
        }
        s.ip = ipv4(&s.iface);
        s.gateway = gateway(&s.iface);
    }
    s
}

fn err_text(f: &Fail, with_pw: bool) -> String {
    match f.name.as_str() {
        "Failed" if with_pw => "wrong password?".into(),
        "Failed" => "couldn't connect".into(),
        "InvalidFormat" => "wrong password?".into(),
        "Aborted" => "cancelled".into(),
        "InProgress" => "already connecting".into(),
        "NotSupported" => "this network type isn't supported".into(),
        "NoAgent" => "no password prompt: the bar's wi-fi panel may hold it, close it and retry".into(),
        "NotFound" => "network not found".into(),
        "NotConfigured" => "this network needs sign-in details first".into(),
        "Timeout" => "timed out".into(),
        _ => f.message.clone(),
    }
}

/// "3 h ago" from iwd's ISO time.
fn ago(iso: &str) -> String {
    let Ok(t) = glib::DateTime::from_iso8601(iso, Some(&glib::TimeZone::utc())) else {
        return String::new();
    };
    let Ok(now) = glib::DateTime::now_utc() else { return String::new() };
    let s = now.difference(&t).as_seconds();
    match s {
        s if s < 90 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s if s < 86400 * 60 => format!("{} days ago", s / 86400),
        s => format!("{} months ago", s / (86400 * 30)),
    }
}

fn kind_name(k: &str) -> &'static str {
    match k {
        "psk" => "password",
        "open" => "open",
        "8021x" => "enterprise",
        "wep" => "wep",
        _ => "other",
    }
}

/// iwd's file name for a network: plain when the name is simple, else `=hex`.
fn iwd_file(ssid: &str, ext: &str) -> String {
    if !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_alphanumeric() || " _-".contains(c)) {
        format!("{ssid}.{ext}")
    } else {
        let hex: String = ssid.bytes().map(|b| format!("{b:02x}")).collect();
        format!("={hex}.{ext}")
    }
}

/// Sign-in details for an enterprise network.
#[derive(Clone, Default)]
struct Eap {
    method: String,
    identity: String,
    password: String,
    anonymous: String,
}

fn eap_file(e: &Eap) -> String {
    let mut s = String::from("[Security]\n");
    let (m, phase2) = match e.method.as_str() {
        "ttls-pap" => ("TTLS", "Tunneled-PAP"),
        "ttls-mschapv2" => ("TTLS", "Tunneled-MSCHAPv2"),
        _ => ("PEAP", "MSCHAPV2"),
    };
    s += &format!("EAP-Method={m}\n");
    let outer = if e.anonymous.is_empty() { &e.identity } else { &e.anonymous };
    s += &format!("EAP-Identity={outer}\n");
    s += &format!("EAP-{m}-Phase2-Method={phase2}\n");
    s += &format!("EAP-{m}-Phase2-Identity={}\n", e.identity);
    s += &format!("EAP-{m}-Phase2-Password={}\n", e.password);
    s += "\n[Settings]\nAutoConnect=true\n";
    s
}

struct Wifi {
    conn: gio::DBusConnection,
    state: RefCell<State>,
    frame: Frame,
    power: ui::Switch,
    msgs: gtk::Box,
    current: gtk::Box,
    nets: Keyed,
    nets_sec: gtk::Box,
    saved: Keyed,
    saved_sec: gtk::Box,
    other_sec: gtk::Box,
    other: gtk::Box,
    scan_l: gtk::Label,
    spin: gtk::Spinner,
    scan_btn: gtk::Button,
    // what the user is doing
    busy: RefCell<HashMap<String, String>>,
    errors: RefCell<HashMap<String, String>>,
    open_row: RefCell<Option<String>>,
    passwords: RefCell<HashMap<String, String>>,
    hidden_pw: RefCell<Option<String>>,
    eap: RefCell<HashMap<String, Eap>>,
    agent: RefCell<Option<gio::RegistrationId>>,
    agent_ok: Cell<Option<bool>>,
    scan_hold: Cell<Option<Instant>>,
    power_hold: Cell<Option<(bool, Instant)>>,
    pending: Cell<bool>,
    sub: RefCell<Option<dbus::Sub>>,
    typing: RefCell<Option<gtk::Widget>>,
}

pub fn page(ctx: &Ctx) -> View {
    let Some(conn) = ctx.system.clone() else {
        let f = Frame::new("wi-fi");
        f.body.append(&ui::text("no system bus", &["note"]));
        return View::new(&f.root);
    };
    let frame = Frame::new("wi-fi");
    let w: Rc<Wifi> = Rc::new_cyclic(|weak: &std::rc::Weak<Wifi>| {
        let wk = weak.clone();
        let power = ui::Switch::new(move |on| {
            if let Some(w) = wk.upgrade() {
                w.set_power(on);
            }
        });
        frame.controls.append(&power.sw);
        let msgs = ui::vbox(0, &[]);
        frame.body.append(&msgs);
        let current = ui::vbox(0, &[]);
        frame.body.append(&current);
        let nets_sec = ui::section("networks");
        frame.body.append(&nets_sec);
        let nets = Keyed::new(&["group"]);
        frame.body.append(&nets.root);
        let saved_sec = ui::section("saved networks");
        frame.body.append(&saved_sec);
        let saved = Keyed::new(&["group"]);
        frame.body.append(&saved.root);
        let other_sec = ui::section("other network");
        frame.body.append(&other_sec);
        let other = ui::group();
        frame.body.append(&other);

        let spin = gtk::Spinner::new();
        spin.set_visible(false);
        let scan_l = ui::text("scan", &[]);
        let sb = ui::hbox(6, &[]);
        sb.append(&spin);
        sb.append(&scan_l);
        sb.append(&ui::text("ctrl+r", &["faint"]));
        let scan_btn = gtk::Button::new();
        scan_btn.set_child(Some(&sb));
        scan_btn.add_css_class("link");
        let wk = weak.clone();
        scan_btn.connect_clicked(move |_| {
            if let Some(w) = wk.upgrade() {
                w.scan();
            }
        });
        frame.foot_add(&scan_btn);
        frame.foot_add(&ui::spacer());
        frame.foot_add(&ui::text("iwd", &["faint"]));

        Wifi {
            conn,
            state: RefCell::default(),
            frame,
            power,
            msgs,
            current,
            nets,
            nets_sec,
            saved,
            saved_sec,
            other_sec,
            other,
            scan_l,
            spin,
            scan_btn,
            busy: RefCell::default(),
            errors: RefCell::default(),
            open_row: RefCell::default(),
            passwords: RefCell::default(),
            hidden_pw: RefCell::default(),
            eap: RefCell::default(),
            agent: RefCell::default(),
            agent_ok: Cell::new(None),
            scan_hold: Cell::new(None),
            power_hold: Cell::new(None),
            pending: Cell::new(false),
            sub: RefCell::default(),
            typing: RefCell::default(),
        }
    });
    w.build_other();

    let keys = gtk::EventControllerKey::new();
    let wk = Rc::downgrade(&w);
    keys.connect_key_pressed(move |_, k, _, m| {
        if (k == gtk::gdk::Key::r || k == gtk::gdk::Key::F5)
            && (k == gtk::gdk::Key::F5 || m.contains(gtk::gdk::ModifierType::CONTROL_MASK))
            && let Some(w) = wk.upgrade()
        {
            w.scan();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    w.frame.root.add_controller(keys);

    let root = w.frame.root.clone();
    let (a, b, c, d) = (w.clone(), w.clone(), w.clone(), w.clone());
    View::new(&root)
        .on_show(move || {
            a.start();
        })
        .on_hide(move || b.pause())
        .on_stop(move || c.pause())
        .every(3000, move || d.query())
}

impl Wifi {
    fn start(self: &Rc<Self>) {
        if self.sub.borrow().is_none() {
            let wk = Rc::downgrade(self);
            *self.sub.borrow_mut() = Some(dbus::subscribe(&self.conn, IWD, move |_, _, _| {
                if let Some(w) = wk.upgrade() {
                    w.schedule();
                }
            }));
        }
        self.register_agent();
        self.query();
        self.scan();
    }

    fn pause(&self) {
        self.sub.borrow_mut().take();
        self.unregister_agent();
    }

    fn schedule(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let wk = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            if let Some(w) = wk.upgrade() {
                w.pending.set(false);
                w.query();
            }
        });
    }

    fn query(self: &Rc<Self>) {
        let wk = Rc::downgrade(self);
        let c = self.conn.clone();
        glib::spawn_future_local(async move {
            let s = collect(&c).await;
            let Some(w) = wk.upgrade() else { return };
            *w.state.borrow_mut() = s;
            w.draw();
        });
    }

    // ── agent ──

    fn register_agent(self: &Rc<Self>) {
        if self.agent.borrow().is_some() {
            return;
        }
        let Ok(node) = gio::DBusNodeInfo::for_xml(AGENT_XML) else { return };
        let Some(iface) = node.lookup_interface("net.connman.iwd.Agent") else { return };
        let wk = Rc::downgrade(self);
        let reg = self
            .conn
            .register_object(AGENT_PATH, &iface)
            .method_call(move |_, _, _, _, method, params, inv| {
                let Some(w) = wk.upgrade() else {
                    inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "gone");
                    return;
                };
                let net = params.child_value(0).str().unwrap_or("").to_string();
                match method {
                    "RequestPassphrase" => {
                        let pw = w
                            .passwords
                            .borrow()
                            .get(&net)
                            .cloned()
                            .or_else(|| w.hidden_pw.borrow().clone());
                        match pw {
                            Some(pw) => inv.return_value(Some(&(pw,).to_variant())),
                            None => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "no passphrase"),
                        }
                    }
                    "RequestUserNameAndPassword" => {
                        let name = w.state.borrow().nets.iter().find(|n| n.path == net).map(|n| n.name.clone());
                        match name.and_then(|n| w.eap.borrow().get(&n).cloned()) {
                            Some(e) => inv.return_value(Some(&(e.identity, e.password).to_variant())),
                            None => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "no details"),
                        }
                    }
                    "RequestUserPassword" => {
                        let name = w.state.borrow().nets.iter().find(|n| n.path == net).map(|n| n.name.clone());
                        match name.and_then(|n| w.eap.borrow().get(&n).cloned()) {
                            Some(e) => inv.return_value(Some(&(e.password,).to_variant())),
                            None => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "no details"),
                        }
                    }
                    "Release" | "Cancel" => inv.return_value(None),
                    _ => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "unsupported"),
                }
            })
            .build();
        let Ok(id) = reg else { return };
        *self.agent.borrow_mut() = Some(id);
        let wk = Rc::downgrade(self);
        let c = self.conn.clone();
        glib::spawn_future_local(async move {
            let r = dbus::call(
                &c,
                IWD,
                "/net/connman/iwd",
                "net.connman.iwd.AgentManager",
                "RegisterAgent",
                Some((glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path"),).to_variant()),
                3000,
            )
            .await;
            if let Some(w) = wk.upgrade() {
                w.agent_ok.set(Some(r.is_ok()));
                w.draw();
            }
        });
    }

    fn unregister_agent(&self) {
        let Some(id) = self.agent.borrow_mut().take() else { return };
        let registered = self.agent_ok.replace(None) == Some(true);
        let c = self.conn.clone();
        let done = move |_: Result<glib::Variant, glib::Error>| {
            let _ = c.unregister_object(id);
        };
        if registered {
            self.conn.call(
                Some(IWD),
                "/net/connman/iwd",
                "net.connman.iwd.AgentManager",
                "UnregisterAgent",
                Some(&(glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path"),).to_variant()),
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
                done,
            );
        } else {
            done(Ok(().to_variant()));
        }
    }

    // ── actions ──

    fn fail(self: &Rc<Self>, key: &str, msg: String) {
        self.errors.borrow_mut().insert(key.to_string(), msg);
        self.draw();
    }

    fn scan(self: &Rc<Self>) {
        let Some(dev) = self.state.borrow().device.clone() else { return };
        self.scan_hold.set(Some(Instant::now()));
        self.errors.borrow_mut().remove("scan");
        self.draw();
        let (w, c) = (self.clone(), self.conn.clone());
        glib::spawn_future_local(async move {
            if let Err(f) = dbus::call(&c, IWD, &dev, "net.connman.iwd.Station", "Scan", None, 5000).await
                && f.name != "Busy"
                && f.name != "InProgress"
            {
                w.scan_hold.set(None);
                w.fail(
                    "scan",
                    if f.name == "NotAvailable" {
                        "can't scan while connecting".into()
                    } else {
                        format!("scan failed: {}", f.message)
                    },
                );
            }
        });
    }

    fn set_power(self: &Rc<Self>, on: bool) {
        self.power_hold.set(Some((on, Instant::now())));
        self.errors.borrow_mut().remove("power");
        if on {
            ui::spawn(&["rfkill", "unblock", "wlan"]);
        }
        let st = self.state.borrow().clone();
        let (w, c) = (self.clone(), self.conn.clone());
        glib::spawn_future_local(async move {
            let r = if let Some(dev) = &st.device {
                dbus::set_prop(&c, IWD, dev, "net.connman.iwd.Device", "Powered", on.to_variant()).await
            } else if let Some(a) = &st.adapter {
                dbus::set_prop(&c, IWD, a, "net.connman.iwd.Adapter", "Powered", on.to_variant()).await
            } else {
                Ok(().to_variant())
            };
            if let Err(f) = r {
                w.power_hold.set(None);
                w.fail("power", format!("couldn't turn wi-fi {}: {}", if on { "on" } else { "off" }, f.message));
            }
        });
    }

    fn connect(self: &Rc<Self>, key: &str, pw: Option<String>) {
        let Some(n) = self.state.borrow().nets.iter().find(|n| n.path == key).cloned() else { return };
        if self.busy.borrow().contains_key(key) {
            return;
        }
        if let Some(pw) = &pw {
            if n.kind == "psk" && !(8..=63).contains(&pw.chars().count()) {
                self.fail(key, "wi-fi passwords are 8 to 63 characters".into());
                return;
            }
            self.passwords.borrow_mut().insert(key.to_string(), pw.clone());
        }
        self.busy.borrow_mut().insert(key.to_string(), "connecting".into());
        self.errors.borrow_mut().remove(key);
        *self.open_row.borrow_mut() = None;
        self.draw();
        let (w, c, key) = (self.clone(), self.conn.clone(), key.to_string());
        glib::spawn_future_local(async move {
            if pw.is_some()
                && let Some(k) = &n.known
            {
                // iwd never asks again for a saved network: forget, then connect fresh
                let _ = dbus::call(&c, IWD, k, "net.connman.iwd.KnownNetwork", "Forget", None, 5000).await;
            }
            let r = dbus::call(&c, IWD, &key, "net.connman.iwd.Network", "Connect", None, 90_000).await;
            w.busy.borrow_mut().remove(&key);
            w.passwords.borrow_mut().remove(&key);
            if let Err(f) = r {
                if n.kind == "psk" && f.name != "InProgress" {
                    *w.open_row.borrow_mut() = Some(key.clone());
                }
                w.fail(&key, err_text(&f, pw.is_some()));
            } else {
                w.query();
            }
        });
    }

    fn disconnect(self: &Rc<Self>, key: &str) {
        let Some(dev) = self.state.borrow().device.clone() else { return };
        self.busy.borrow_mut().insert(key.to_string(), "disconnecting".into());
        self.draw();
        let (w, c, key) = (self.clone(), self.conn.clone(), key.to_string());
        glib::spawn_future_local(async move {
            let r = dbus::call(&c, IWD, &dev, "net.connman.iwd.Station", "Disconnect", None, 10_000).await;
            w.busy.borrow_mut().remove(&key);
            match r {
                Err(f) => w.fail(&key, f.message),
                Ok(_) => w.query(),
            }
        });
    }

    fn forget(self: &Rc<Self>, known: &str) {
        let (w, c, known) = (self.clone(), self.conn.clone(), known.to_string());
        glib::spawn_future_local(async move {
            match dbus::call(&c, IWD, &known, "net.connman.iwd.KnownNetwork", "Forget", None, 5000).await {
                Err(f) => w.fail(&known, f.message),
                Ok(_) => w.query(),
            }
        });
    }

    fn set_auto(self: &Rc<Self>, known: &str, on: bool) {
        let (w, c, known) = (self.clone(), self.conn.clone(), known.to_string());
        glib::spawn_future_local(async move {
            if let Err(f) =
                dbus::set_prop(&c, IWD, &known, "net.connman.iwd.KnownNetwork", "AutoConnect", on.to_variant()).await
            {
                w.fail(&known, f.message);
            }
            w.query();
        });
    }

    fn join_hidden(self: &Rc<Self>, ssid: String, pw: Option<String>) {
        let Some(dev) = self.state.borrow().device.clone() else { return };
        if let Some(p) = &pw
            && !(8..=63).contains(&p.chars().count())
        {
            self.fail("other", "wi-fi passwords are 8 to 63 characters".into());
            return;
        }
        *self.hidden_pw.borrow_mut() = pw;
        self.busy.borrow_mut().insert("other".into(), format!("joining {ssid}"));
        self.errors.borrow_mut().remove("other");
        self.draw();
        let (w, c) = (self.clone(), self.conn.clone());
        glib::spawn_future_local(async move {
            let r = dbus::call(
                &c,
                IWD,
                &dev,
                "net.connman.iwd.Station",
                "ConnectHiddenNetwork",
                Some((ssid.as_str(),).to_variant()),
                90_000,
            )
            .await;
            w.busy.borrow_mut().remove("other");
            *w.hidden_pw.borrow_mut() = None;
            match r {
                Err(f) => w.fail(
                    "other",
                    match f.name.as_str() {
                        "NotHidden" => "that network isn't hidden: pick it in the list above".into(),
                        "NotFound" => "no hidden network with that name in range".into(),
                        _ => err_text(&f, true),
                    },
                ),
                Ok(_) => {
                    w.reset_other();
                    w.query();
                }
            }
        });
    }

    /// Writes the enterprise provisioning file (pkexec), then connects.
    fn enterprise(self: &Rc<Self>, key: String, ssid: String, e: Eap) {
        if e.identity.is_empty() || e.password.is_empty() {
            self.fail(&key, "fill in the user name and password".into());
            return;
        }
        self.busy.borrow_mut().insert(key.clone(), "saving sign-in".into());
        self.errors.borrow_mut().remove(&key);
        self.eap.borrow_mut().insert(ssid.clone(), e.clone());
        self.draw();
        let (w, c) = (self.clone(), self.conn.clone());
        glib::spawn_future_local(async move {
            let dir = glib::user_runtime_dir();
            let tmp = dir.join(format!("vector-settings-{}.8021x", std::process::id()));
            let wrote = {
                use std::io::Write;
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&tmp)
                    .and_then(|mut f| f.write_all(eap_file(&e).as_bytes()))
            };
            if wrote.is_err() {
                w.busy.borrow_mut().remove(&key);
                w.fail(&key, "couldn't write the sign-in file".into());
                return;
            }
            let target = format!("/var/lib/iwd/{}", iwd_file(&ssid, "8021x"));
            let (ok, _, err) = ui::run(vec![
                "pkexec".into(),
                "install".into(),
                "-m600".into(),
                "-o".into(),
                "root".into(),
                "-g".into(),
                "root".into(),
                tmp.to_string_lossy().to_string(),
                target,
            ])
            .await;
            let _ = std::fs::remove_file(&tmp);
            if !ok {
                w.busy.borrow_mut().remove(&key);
                let msg = if err.contains("dismissed") || err.contains("Not authorized") || err.is_empty() {
                    "cancelled: the sign-in needs your password once".to_string()
                } else {
                    err.trim().to_string()
                };
                w.fail(&key, msg);
                return;
            }
            // iwd picks the file up by itself
            glib::timeout_future(Duration::from_millis(800)).await;
            let s = collect(&c).await;
            *w.state.borrow_mut() = s.clone();
            w.busy.borrow_mut().remove(&key);
            if let Some(n) = s.nets.iter().find(|n| n.name == ssid && n.kind == "8021x") {
                let p = n.path.clone();
                w.connect(&p, None);
            } else if key == "other" {
                w.reset_other();
                w.join_hidden(ssid, None);
            } else {
                w.draw();
            }
        });
    }

    // ── drawing ──

    fn draw(self: &Rc<Self>) {
        let s = self.state.borrow().clone();

        // power switch: show the request while iwd catches up (up to 4 s)
        let mut shown = s.powered;
        if let Some((want, at)) = self.power_hold.get() {
            if want != s.powered && at.elapsed() < Duration::from_secs(4) {
                shown = want;
            } else {
                self.power_hold.set(None);
            }
        }
        self.power.set(shown);
        self.power.sw.set_visible(s.adapter.is_some());

        let cur = s.current().cloned();
        let going = s.nets.iter().find(|n| n.connecting).cloned();
        let detail = if s.adapter.is_none() {
            "no wireless device".to_string()
        } else if s.device.is_none() || !s.powered {
            "off".into()
        } else if let Some(c) = &cur {
            format!("connected to <b>{}</b>", ui::esc(&c.name))
        } else if let Some(g) = &going {
            format!("connecting to {}…", ui::esc(&g.name))
        } else {
            "not connected".into()
        };
        self.frame.detail.set_markup(&detail);

        let scanning = s.scanning
            || self
                .scan_hold
                .get()
                .is_some_and(|t| t.elapsed() < Duration::from_secs(4) && !s.scanning && s.nets.is_empty());
        if s.scanning {
            self.scan_hold.set(None);
        }
        self.spin.set_visible(scanning);
        self.spin.set_spinning(scanning);
        self.scan_l.set_text(if scanning { "scanning…" } else { "scan" });
        self.scan_btn.set_visible(s.powered && s.station);

        ui::clear(&self.msgs);
        for k in ["power", "scan"] {
            if let Some(e) = self.errors.borrow().get(k) {
                self.msgs.append(&ui::text(e, &["msg", "crit"]));
            }
        }
        if self.agent_ok.get() == Some(false) && s.powered {
            self.msgs.append(&ui::text(
                "another program holds the wi-fi password prompt (the bar's wi-fi panel?): new passwords may not reach iwd",
                &["msg", "warn"],
            ));
        }

        let on = s.powered && s.device.is_some();
        self.draw_current(&s, cur.as_ref());
        self.draw_nets(&s, on, scanning);
        self.draw_saved(&s);
        self.other_sec.set_visible(on);
        self.other.set_visible(on);
        self.draw_other_status();
    }

    fn draw_current(self: &Rc<Self>, s: &State, cur: Option<&Net>) {
        // rebuilt only when it changes (the details are selectable)
        let sig = format!("{cur:?}{:?}{:?}{:?}{:?}{:?}", s.diag, s.ip, s.gateway, self.busy.borrow().get(cur.map(|c| c.path.as_str()).unwrap_or("")), self.errors.borrow().get(cur.map(|c| c.path.as_str()).unwrap_or("")));
        if unsafe { self.current.data::<String>("sig") }.is_some_and(|p| unsafe { p.as_ref() } == &sig) {
            return;
        }
        unsafe { self.current.set_data("sig", sig) };
        ui::clear(&self.current);
        let Some(c) = cur else { return };
        self.current.append(&ui::section("connection"));
        let g = ui::group();
        let rssi: i64 = s.diag.get("RSSI").and_then(|v| v.parse().ok()).unwrap_or(c.dbm);
        let pct = (2 * (rssi + 100)).clamp(0, 100);
        let lvl = ((rssi + 90) / 10).clamp(0, 4) as usize;
        let row = Row::new(BARS[lvl], &c.name, "connected");
        row.active(true);
        if let Some(b) = self.busy.borrow().get(&c.path) {
            row.trail.append(&gtk::Spinner::builder().spinning(true).build());
            row.meta.set_text(b);
        }
        let (k1, k2) = (c.path.clone(), c.known.clone());
        let wk = Rc::downgrade(self);
        row.trail.append(&ui::link("disconnect", &[], move || {
            if let Some(w) = wk.upgrade() {
                w.disconnect(&k1);
            }
        }));
        if let Some(k) = k2 {
            let wk = Rc::downgrade(self);
            row.trail.append(&ui::link("forget", &["danger"], move || {
                if let Some(w) = wk.upgrade() {
                    w.forget(&k);
                }
            }));
        }
        g.append(&row.root);
        if let Some(e) = self.errors.borrow().get(&c.path) {
            g.append(&ui::text(e, &["msg", "crit"]));
        }
        let freq: Option<f64> = s.diag.get("Frequency").and_then(|v| v.parse().ok());
        g.append(&ui::kv("signal", &format!("{pct}% {}", ui::dim(&format!("{rssi} dBm")))));
        if let Some(f) = freq {
            let band = if f < 3000.0 { "2.4" } else if f < 5925.0 { "5" } else { "6" };
            let ch = s.diag.get("Channel").cloned().unwrap_or_default();
            g.append(&ui::kv(
                "band",
                &format!("{band} GHz {}", ui::dim(&format!("channel {ch} · {} MHz", f as i64))),
            ));
        }
        if let Some(m) = s.diag.get("RxMode") {
            g.append(&ui::kv("standard", &ui::esc(m)));
        }
        let rate = |k: &str| s.diag.get(k).and_then(|v| v.parse::<i64>().ok()).map(|b| format!("{} Mbit/s", b / 10));
        if let (Some(rx), tx) = (rate("RxBitrate"), rate("TxBitrate")) {
            g.append(&ui::kv(
                "speed",
                &format!("{} {}", ui::esc(&rx), ui::dim(&format!("down · {} up", tx.unwrap_or_else(|| "?".into())))),
            ));
        }
        if let Some(sec) = s.diag.get("Security") {
            g.append(&ui::kv("security", &ui::esc(sec)));
        }
        g.append(&ui::kv("ip address", &ui::esc(s.ip.as_deref().unwrap_or("waiting…"))));
        if let Some(gw) = &s.gateway {
            g.append(&ui::kv("router", &ui::esc(gw)));
        }
        let dns = ui::kv("dns", "…");
        g.append(&dns);
        if let Some(b) = s.diag.get("ConnectedBss") {
            g.append(&ui::kv("access point", &ui::esc(b)));
        }
        g.append(&ui::kv("this device", &format!("{} {}", ui::esc(&s.iface), ui::dim(&s.mac))));
        self.current.append(&g);
        // dns from systemd-resolved
        let iface = s.iface.clone();
        glib::spawn_future_local(async move {
            let (ok, out, _) = ui::run(vec!["resolvectl".into(), "dns".into(), iface]).await;
            let v = out.split_once(':').map(|(_, r)| r.trim().to_string()).unwrap_or_default();
            if let Some(l) = dns.last_child().and_downcast::<gtk::Label>() {
                l.set_markup(&if ok && !v.is_empty() { ui::esc(&v) } else { ui::dim("automatic") });
            }
        });
    }

    fn draw_nets(self: &Rc<Self>, s: &State, on: bool, scanning: bool) {
        let mut items: Vec<ui::Item> = Vec::new();
        if !on {
            let t = if s.adapter.is_none() { "iwd sees no wi-fi device" } else { "wi-fi is off" };
            items.push(("empty".into(), t.into(), Box::new(move || ui::text(t, &["note"]).upcast())));
        } else {
            let list: Vec<Net> = s.nets.iter().filter(|n| !n.connected).cloned().collect();
            if list.is_empty() {
                let t = if scanning { "looking for networks…" } else { "no other networks in range · ctrl+r to scan" };
                items.push(("empty".into(), t.into(), Box::new(move || ui::text(t, &["note"]).upcast())));
            }
            let open_row = self.open_row.borrow().clone();
            for n in list.into_iter().take(30) {
                let busy = self.busy.borrow().get(&n.path).cloned().or_else(|| n.connecting.then(|| "connecting".to_string()));
                let err = self.errors.borrow().get(&n.path).cloned();
                let open = open_row.as_deref() == Some(&n.path);
                // not the live dBm: a row must not be rebuilt while typing in it
                let level = if open { 0 } else { n.level };
                let sig = format!(
                    "{}|{}|{:?}|{}|{level}|{busy:?}|{err:?}|{open}|{:?}",
                    n.name, n.kind, n.known, n.connecting, self.agent_ok.get()
                );
                let w = self.clone();
                items.push((n.path.clone(), sig, Box::new(move || w.net_row(&n, busy.as_deref(), err.as_deref(), open))));
            }
        }
        self.nets_sec.set_visible(true);
        self.nets.sync(items);
    }

    fn net_row(self: &Rc<Self>, n: &Net, busy: Option<&str>, err: Option<&str>, open: bool) -> gtk::Widget {
        let mut meta = Vec::new();
        if n.known.is_some() {
            meta.push("saved".to_string());
        }
        if n.kind != "open" {
            meta.push(format!("󰌾 {}", kind_name(&n.kind)));
        } else {
            meta.push("open".into());
        }
        let row = Row::new(BARS[n.level], &n.name, &ui::dim(&meta.join(" · ")));
        if let Some(b) = busy {
            row.meta.set_text(b);
            row.trail.append(&gtk::Spinner::builder().spinning(true).build());
        }
        {
            let (w, n) = (Rc::downgrade(self), n.clone());
            let busy = busy.is_some();
            row.on_click(move || {
                let Some(w) = w.upgrade() else { return };
                if busy || n.connecting {
                    return;
                }
                let needs_details = (n.known.is_none() && n.kind != "open") || n.kind == "8021x" && n.known.is_none();
                if needs_details || w.open_row.borrow().as_deref() == Some(&n.path) {
                    let mut o = w.open_row.borrow_mut();
                    *o = if o.as_deref() == Some(&n.path) { None } else { Some(n.path.clone()) };
                    drop(o);
                    w.errors.borrow_mut().remove(&n.path);
                    w.draw();
                } else {
                    w.connect(&n.path, None);
                }
            });
        }
        if let Some(e) = err {
            row.expand().append(&ui::text(e, &["crit"]));
        }
        if open {
            let ex = row.expand();
            if n.kind == "8021x" {
                self.eap_form(&ex, n.path.clone(), Some(n.name.clone()));
            } else {
                let line = ui::hbox(8, &[]);
                let e = gtk::PasswordEntry::new();
                e.set_show_peek_icon(true);
                e.set_hexpand(true);
                e.set_property("placeholder-text", if n.known.is_some() { "new password" } else { "password" });
                let go = {
                    let (w, key, e) = (Rc::downgrade(self), n.path.clone(), e.clone());
                    move || {
                        let t = e.text().to_string();
                        if let (Some(w), false) = (w.upgrade(), t.is_empty()) {
                            w.connect(&key, Some(t));
                        }
                    }
                };
                let go = Rc::new(go);
                let g2 = go.clone();
                e.connect_activate(move |_| g2());
                line.append(&e);
                line.append(&ui::link("connect", &["primary"], move || go()));
                if n.known.is_some() {
                    let (w, key) = (Rc::downgrade(self), n.path.clone());
                    line.append(&ui::link("use saved", &[], move || {
                        if let Some(w) = w.upgrade() {
                            w.connect(&key, None);
                        }
                    }));
                }
                ex.append(&line);
                let e3 = e.clone();
                glib::idle_add_local_once(move || {
                    e3.grab_focus();
                });
                *self.typing.borrow_mut() = Some(e.upcast());
            }
        }
        row.root.upcast()
    }

    /// The enterprise sign-in form. `ssid` None: the hidden-network form's.
    fn eap_form(self: &Rc<Self>, into: &gtk::Box, key: String, ssid: Option<String>) -> Rc<dyn Fn() -> Option<(String, Eap)>> {
        let methods = ["peap", "ttls-pap", "ttls-mschapv2"];
        let method = gtk::DropDown::from_strings(&["PEAP (most, eduroam)", "TTLS + PAP", "TTLS + MSCHAPv2"]);
        let user = gtk::Entry::builder().placeholder_text("user name (e.g. you@school.nl)").hexpand(true).build();
        let pw = gtk::PasswordEntry::builder().placeholder_text("password").show_peek_icon(true).hexpand(true).build();
        let anon = gtk::Entry::builder().placeholder_text("anonymous identity (optional)").hexpand(true).build();
        let l1 = ui::hbox(8, &[]);
        l1.append(&user);
        l1.append(&method);
        let l2 = ui::hbox(8, &[]);
        l2.append(&pw);
        l2.append(&anon);
        into.append(&l1);
        into.append(&l2);
        let read: Rc<dyn Fn() -> Option<(String, Eap)>> = {
            let (user, pw, anon, method) = (user.clone(), pw.clone(), anon.clone(), method.clone());
            Rc::new(move || {
                Some((
                    String::new(),
                    Eap {
                        method: methods[method.selected() as usize].to_string(),
                        identity: user.text().trim().to_string(),
                        password: pw.text().to_string(),
                        anonymous: anon.text().trim().to_string(),
                    },
                ))
            })
        };
        if let Some(ssid) = ssid {
            let line = ui::hbox(8, &[]);
            line.append(&ui::text("saved for this laptop only (root-only file); asks your password once", &["dim"]));
            line.append(&ui::spacer());
            let (w, r) = (Rc::downgrade(self), read.clone());
            let go = Rc::new(move || {
                if let (Some(w), Some((_, e))) = (w.upgrade(), r()) {
                    w.enterprise(key.clone(), ssid.clone(), e);
                }
            });
            let g2 = go.clone();
            pw.connect_activate(move |_| g2());
            line.append(&ui::link("sign in", &["primary"], move || go()));
            into.append(&line);
            let u = user.clone();
            glib::idle_add_local_once(move || {
                u.grab_focus();
            });
        }
        read
    }

    fn draw_saved(self: &Rc<Self>, s: &State) {
        let mut items: Vec<ui::Item> = Vec::new();
        let in_range: HashMap<&str, &Net> = s.nets.iter().filter_map(|n| n.known.as_deref().map(|k| (k, n))).collect();
        for k in &s.known {
            let near = in_range.get(k.path.as_str()).map(|n| n.connected);
            let err = self.errors.borrow().get(&k.path).cloned();
            let sig = format!("{k:?}{near:?}{err:?}");
            let (w, k) = (self.clone(), k.clone());
            items.push((
                k.path.clone(),
                sig,
                Box::new(move || {
                    let mut meta = vec![kind_name(&k.kind).to_string()];
                    if k.hidden {
                        meta.push("hidden".into());
                    }
                    match near {
                        Some(true) => meta.push("connected".into()),
                        Some(false) => meta.push("in range".into()),
                        None => {
                            let a = ago(&k.last);
                            if !a.is_empty() {
                                meta.push(format!("used {a}"));
                            }
                        }
                    }
                    let row = Row::new(if near.is_some() { "󰤨" } else { "󰤮" }, &k.name, &ui::dim(&meta.join(" · ")));
                    row.active(near == Some(true));
                    let auto_l = ui::text("auto", &["dim"]);
                    auto_l.set_wrap(false);
                    row.trail.append(&auto_l);
                    let wk = Rc::downgrade(&w);
                    let path = k.path.clone();
                    let sw = ui::Switch::new(move |on| {
                        if let Some(w) = wk.upgrade() {
                            w.set_auto(&path, on);
                        }
                    });
                    sw.set(k.auto);
                    sw.sw.set_tooltip_text(Some("connect by itself when in range"));
                    row.trail.append(&sw.sw);
                    // forget asks once: the first click arms it
                    let armed = Rc::new(Cell::new(false));
                    let (wk, path) = (Rc::downgrade(&w), k.path.clone());
                    let fl = ui::link("forget", &["danger"], || {});
                    let fl2 = fl.clone();
                    fl.connect_clicked(move |_| {
                        if armed.replace(true) {
                            if let Some(w) = wk.upgrade() {
                                w.forget(&path);
                            }
                        } else {
                            fl2.set_label("forget?");
                        }
                    });
                    row.trail.append(&fl);
                    if let Some(e) = err {
                        row.expand().append(&ui::text(&e, &["crit"]));
                    }
                    row.root.upcast()
                }),
            ));
        }
        if items.is_empty() {
            items.push((
                "empty".into(),
                String::new(),
                Box::new(|| ui::text("no saved networks yet", &["note"]).upcast()),
            ));
        }
        self.saved_sec.set_visible(true);
        self.saved.sync(items);
    }

    // ── hidden network ──

    fn build_other(self: &Rc<Self>) {
        let g = &self.other;
        let pad = ui::vbox(8, &["kv"]);
        pad.set_margin_top(6);
        pad.set_margin_bottom(8);
        let l1 = ui::hbox(8, &[]);
        let ssid = gtk::Entry::builder().placeholder_text("network name").hexpand(true).build();
        let sec = gtk::DropDown::from_strings(&["password", "open", "enterprise"]);
        l1.append(&ssid);
        l1.append(&sec);
        pad.append(&l1);
        let pw = gtk::PasswordEntry::builder().placeholder_text("password").show_peek_icon(true).hexpand(true).build();
        pad.append(&pw);
        let eap_box = ui::vbox(8, &[]);
        let read_eap = self.eap_form(&eap_box, "other".into(), None);
        eap_box.set_visible(false);
        pad.append(&eap_box);
        let foot = ui::hbox(8, &[]);
        let status = ui::text("", &["dim"]);
        status.set_hexpand(true);
        foot.append(&status);
        let join = ui::link("join", &["primary"], || {});
        foot.append(&join);
        pad.append(&foot);
        g.append(&pad);
        unsafe {
            g.set_data("status", status.clone());
            g.set_data("ssid", ssid.clone().upcast::<gtk::Editable>());
            g.set_data("pw", pw.clone().upcast::<gtk::Editable>());
        }
        {
            let (pw, eap_box) = (pw.clone(), eap_box.clone());
            sec.connect_selected_notify(move |d| {
                pw.set_visible(d.selected() == 0);
                eap_box.set_visible(d.selected() == 2);
            });
        }
        let go = {
            let wk = Rc::downgrade(self);
            let (ssid, sec, pw) = (ssid.clone(), sec.clone(), pw.clone());
            Rc::new(move || {
                let Some(w) = wk.upgrade() else { return };
                let name = ssid.text().trim().to_string();
                if name.is_empty() {
                    w.fail("other", "type the network's name".into());
                    return;
                }
                match sec.selected() {
                    0 => w.join_hidden(name, Some(pw.text().to_string())),
                    1 => w.join_hidden(name, None),
                    _ => {
                        if let Some((_, e)) = read_eap() {
                            w.enterprise("other".into(), name, e);
                        }
                    }
                }
            })
        };
        let g2 = go.clone();
        join.connect_clicked(move |_| g2());
        let g3 = go.clone();
        pw.connect_activate(move |_| g3());
        ssid.connect_activate(move |_| go());
    }

    fn draw_other_status(&self) {
        let Some(status) = (unsafe { self.other.data::<gtk::Label>("status") }) else { return };
        let status = unsafe { status.as_ref() };
        if let Some(b) = self.busy.borrow().get("other") {
            status.set_text(&format!("{b}…"));
            status.remove_css_class("crit");
        } else if let Some(e) = self.errors.borrow().get("other") {
            status.set_text(e);
            status.add_css_class("crit");
        } else {
            status.set_text("for networks that don't show their name");
            status.remove_css_class("crit");
        }
    }

    fn reset_other(&self) {
        for k in ["ssid", "pw"] {
            if let Some(e) = unsafe { self.other.data::<gtk::Editable>(k) } {
                unsafe { e.as_ref() }.set_text("");
            }
        }
        self.errors.borrow_mut().remove("other");
    }
}
