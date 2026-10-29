//! Network: Wi-Fi over iwd (D-Bus, event driven) or wired. Bar: signal bars /
//! ethernet / disconnected. Click: Wi-Fi panel (power, saved + nearby networks,
//! connect with password, disconnect, forget, scan). Right: impala.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::dbus::{self, Fail, Objects};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const IWD: &str = "net.connman.iwd";
const BARS: [&str; 5] = ["󰤯", "󰤟", "󰤢", "󰤥", "󰤨"];
const LOCK: &str = "󰌾";
const AGENT_PATH: &str = "/org/vectoros/bar/iwdagent";
const AGENT_XML: &str = r#"<node><interface name="net.connman.iwd.Agent">
<method name="Release"/>
<method name="RequestPassphrase"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="RequestPrivateKeyPassphrase"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="RequestUserNameAndPassword"><arg type="o" direction="in"/><arg type="s" direction="out"/><arg type="s" direction="out"/></method>
<method name="RequestUserPassword"><arg type="o" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="out"/></method>
<method name="Cancel"><arg type="s" direction="in"/></method>
</interface></node>"#;

#[derive(Clone, Default, PartialEq, Debug)]
pub struct WifiNet {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub connected: bool,
    pub connecting: bool,
    pub known: Option<String>,
    pub level: usize,
    pub dbm: i64,
}

#[derive(Clone, Default, PartialEq, Debug)]
pub struct State {
    pub adapter: Option<String>,
    pub device: Option<String>,
    pub iface: String,
    pub powered: bool,
    pub station: bool,
    pub st_state: String,
    pub scanning: bool,
    pub nets: Vec<WifiNet>,
    pub rssi: Option<i64>,
    pub freq: Option<u32>,
    pub bitrate: Option<i64>,
    pub ip: Option<String>,
    pub wired: Option<(String, Option<String>)>,
}

impl State {
    pub fn current(&self) -> Option<&WifiNet> {
        self.nets.iter().find(|n| n.connected)
    }
    fn signal_pct(&self) -> i64 {
        self.rssi.map(|d| (2 * (d + 100)).clamp(0, 100)).unwrap_or(0)
    }
}

/// IPv4 of an interface (getifaddrs, no subprocess).
pub fn ipv4(iface: &str) -> Option<String> {
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
                    let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                    out = Some(ip.to_string());
                    break;
                }
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(ifa);
    }
    out
}

/// A physical, plugged-in ethernet interface (name, ip).
fn wired() -> Option<(String, Option<String>)> {
    for e in std::fs::read_dir("/sys/class/net").ok()?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name == "lo" || p.join("wireless").exists() || !p.join("device").exists() {
            continue;
        }
        if util::read_num(p.join("type")) == Some(1) && util::read_num(p.join("carrier")) == Some(1) {
            let ip = ipv4(&name);
            return Some((name, ip));
        }
    }
    None
}

async fn collect(c: &gio::DBusConnection) -> State {
    let objs: Objects = dbus::managed(c, IWD, "/").await;
    let mut s = State { adapter: objs.iter().find(|(_, i)| i.contains_key("net.connman.iwd.Adapter")).map(|(p, _)| p.clone()), ..Default::default() };
    s.wired = wired();
    let dev = objs.iter().find(|(_, i)| i.get("net.connman.iwd.Device").is_some_and(|d| dbus::get_str(d, "Mode") == "station"));
    let Some((dev, ifs)) = dev else { return s };
    s.device = Some(dev.clone());
    let d = &ifs["net.connman.iwd.Device"];
    s.powered = dbus::get_bool(d, "Powered");
    s.iface = dbus::get_str(d, "Name");
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
            let Some(n) = objs.get(&path).and_then(|x| x.get("net.connman.iwd.Network")) else { continue };
            let dbm = strength / 100;
            let known = dbus::get_str(n, "KnownNetwork");
            s.nets.push(WifiNet {
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
    s.nets.sort_by_key(|n| (!n.connected, n.known.is_none()));
    if s.current().is_some() {
        if let Ok(v) = dbus::call(c, IWD, dev, "net.connman.iwd.StationDiagnostic", "GetDiagnostics", None, 1500).await {
            let p = dbus::props(&v.child_value(0));
            s.rssi = dbus::get_i64(&p, "RSSI");
            s.freq = dbus::get_i64(&p, "Frequency").map(|f| f as u32);
            s.bitrate = dbus::get_i64(&p, "RxBitrate").or_else(|| dbus::get_i64(&p, "TxBitrate"));
        }
        if s.rssi.is_none() {
            s.rssi = s.current().map(|n| n.dbm);
        }
        s.ip = ipv4(&s.iface);
    }
    s
}

struct Inner {
    slot: Slot,
    conn: Option<gio::DBusConnection>,
    state: RefCell<State>,
    changed: Notify,
    theme: Rc<Theme>,
    pending: Cell<bool>,
    _sub: RefCell<Option<dbus::Sub>>,
}

pub struct Network(Rc<Inner>);

impl Network {
    pub fn new(ctx: &Ctx) -> Network {
        let o = ctx.opts("network");
        let conn = dbus::system();
        let inner = Rc::new(Inner {
            slot: ctx.slot("network", &[]),
            conn,
            state: RefCell::default(),
            changed: Notify::default(),
            theme: ctx.theme.clone(),
            pending: Cell::new(false),
            _sub: RefCell::default(),
        });
        let right = o.str("on_right", "omarchy-launch-wifi");
        inner.slot.on_right(move || util::spawn_sh(&right));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => wifi_panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        if let Some(c) = &inner.conn {
            let w = Rc::downgrade(&inner);
            *inner._sub.borrow_mut() = Some(dbus::subscribe(c, IWD, move |_, _, _| {
                if let Some(i) = w.upgrade() {
                    i.schedule();
                }
            }));
        }
        inner.query();
        util::every(o.num("interval", 5.0) as u64 * 1000, &inner, |i| i.query());
        Network(inner)
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
        let Some(c) = self.conn.clone() else {
            self.slot.set_markup("󰤮");
            return;
        };
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
        let (icon, cls) = if s.wired.is_some() && s.current().is_none() {
            ("󰀂", "")
        } else if let Some(n) = s.current() {
            let lvl = s.rssi.map(|d| ((d + 90) / 10).clamp(0, 4) as usize).unwrap_or(n.level);
            (BARS[lvl], "")
        } else if s.nets.iter().any(|n| n.connecting) {
            (BARS[0], "dim")
        } else {
            ("󰤮", "dim")
        };
        self.slot.set_markup(icon);
        self.slot.set_classes(&[cls]);
        self.slot.bump();
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = self.state.borrow();
        let sh = ui::sheet();
        if let (Some((ifc, ip)), None) = (&s.wired, s.current()) {
            sh.append(&ui::head("ethernet", "connected").0);
            sh.append(&ui::tkv("device", ifc).tap(|b| b.set_margin_top(6)));
            sh.append(&ui::tkv("ip", ip.as_deref().unwrap_or("…")));
        } else if let Some(n) = s.current() {
            sh.append(&ui::head("wi-fi", &util::esc(&n.name)).0);
            let pct = s.signal_pct();
            sh.append(&ui::tmeter("signal", pct as f64 / 100.0, &format!("{pct}%"), &format!("{} dBm", s.rssi.unwrap_or(n.dbm)), if pct < 30 { "warn" } else { "" }));
            if let Some(b) = s.bitrate {
                sh.append(&ui::tkv("speed", &format!("{} Mbit/s", b / 10)));
            }
            if let Some(f) = s.freq {
                sh.append(&ui::tkv("band", &format!("{:.1} GHz", f as f64 / 1000.0)));
            }
            sh.append(&ui::tkv("ip", s.ip.as_deref().unwrap_or("…")));
        } else {
            sh.append(&ui::head("wi-fi", if !s.powered { "off" } else { "disconnected" }).0);
        }
        sh.append(&ui::thint("click networks  ·  right impala"));
        sh.upcast()
    }
}

impl Module for Network {
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
    expanded: Option<String>,
    passwords: HashMap<String, String>,
    agent: Option<gio::RegistrationId>,
    agent_ok: Option<bool>,
    scan_hold: Option<std::time::Instant>,
    switch_hold: Option<(bool, std::time::Instant)>,
    entry: Option<gtk::PasswordEntry>,
}

fn err_text(f: &Fail, with_pw: bool) -> String {
    match f.name.as_str() {
        "Failed" => if with_pw { "wrong password?" } else { "couldn't connect" }.into(),
        "InvalidFormat" => "wrong password?".into(),
        "Aborted" => "cancelled".into(),
        "InProgress" => "already connecting".into(),
        "NotSupported" => "this network type isn't supported here".into(),
        "NoAgent" => "no password prompt available: use impala".into(),
        "Timeout" => "timed out".into(),
        _ => f.message.clone(),
    }
}

fn wifi_panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(350);
    let card = Card::new("wi-fi", "");
    let ps: Rc<RefCell<PState>> = Rc::default();
    let Some(conn) = i.conn.clone() else {
        card.body.append(&ui::text("no system bus", &["empty-note"]));
        return card.root.upcast();
    };

    // power switch in the header
    let sw = gtk::Switch::new();
    sw.set_valign(gtk::Align::Center);
    card.controls.append(&sw);
    let sw_guard = Rc::new(Cell::new(false));

    let info = ui::vbox(0, &[]);
    card.body.append(&info);
    let msgs = ui::vbox(0, &[]);
    card.body.append(&msgs);
    let list = ui::KeyedList::new();
    card.body.append(&list.root);

    // footer: scan + impala
    let spin = gtk::Spinner::new();
    spin.set_visible(false);
    let scan_l = ui::text("scan", &[]);
    let scan_box = ui::hbox(6, &[]);
    scan_box.append(&spin);
    scan_box.append(&scan_l);
    scan_box.append(&ui::text("s", &["faint"]));
    let scan_btn = gtk::Button::new();
    scan_btn.set_child(Some(&scan_box));
    scan_btn.add_css_class("link");
    ui::pointer(&scan_btn);
    card.foot_add(&scan_btn);
    card.foot_add(&ui::spacer());
    card.foot_add(&ui::link("impala", &[], {
        let p = p.clone();
        move || {
            p.close();
            util::spawn_sh("omarchy-launch-wifi");
        }
    }));

    // the agent hands iwd the password typed into a row
    if let Ok(node) = gio::DBusNodeInfo::for_xml(AGENT_XML) {
        if let Some(iface) = node.lookup_interface("net.connman.iwd.Agent") {
            let ps2 = ps.clone();
            let reg = conn
                .register_object(AGENT_PATH, &iface)
                .method_call(move |_, _, _, _, method, params, inv| match method {
                    "RequestPassphrase" => {
                        let net = params.child_value(0).str().unwrap_or("").to_string();
                        let pw = ps2.borrow().passwords.get(&net).cloned();
                        match pw {
                            Some(pw) => inv.return_value(Some(&(pw,).to_variant())),
                            None => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "no passphrase"),
                        }
                    }
                    "Release" | "Cancel" => inv.return_value(None),
                    _ => inv.return_dbus_error("net.connman.iwd.Agent.Error.Canceled", "unsupported"),
                })
                .build();
            if let Ok(id) = reg {
                ps.borrow_mut().agent = Some(id);
                let (c2, ps3) = (conn.clone(), ps.clone());
                glib::spawn_future_local(async move {
                    let r = dbus::call(&c2, IWD, "/net/connman/iwd", "net.connman.iwd.AgentManager", "RegisterAgent", Some((glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path"),).to_variant()), 3000).await;
                    ps3.borrow_mut().agent_ok = Some(r.is_ok() || r.as_ref().err().is_some_and(|f| f.name == "AlreadyExists"));
                });
            }
        }
    }
    {
        let (c2, ps2) = (conn.clone(), ps.clone());
        p.on_close(move || {
            let mut s = ps2.borrow_mut();
            // tell the daemon first (async: closing must not wait on it), then drop our object
            let registered = s.agent_ok == Some(true);
            let id = s.agent.take();
            let path = glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path");
            let c3 = c2.clone();
            let done = move |_: Result<glib::Variant, glib::Error>| {
                if let Some(id) = id {
                    let _ = c3.unregister_object(id);
                }
            };
            if registered {
                c2.call(Some(IWD), "/net/connman/iwd", "net.connman.iwd.AgentManager", "UnregisterAgent", Some(&(path,).to_variant()), None, gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE, done);
            } else {
                done(Ok(().to_variant()));
            }
        });
    }

    let render: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
    let rerender = {
        let r = render.clone();
        move || {
            let f = r.borrow().clone();
            if let Some(f) = f {
                f();
            }
        }
    };
    let rerender: Rc<dyn Fn()> = Rc::new(rerender);

    let do_scan = {
        let (c, i, ps, rr) = (conn.clone(), Rc::downgrade(i), ps.clone(), rerender.clone());
        move || {
            let Some(dev) = i.upgrade().and_then(|i| i.state.borrow().device.clone()) else { return };
            ps.borrow_mut().scan_hold = Some(std::time::Instant::now());
            ps.borrow_mut().errors.remove("scan");
            rr();
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                if let Err(f) = dbus::call(&c, IWD, &dev, "net.connman.iwd.Station", "Scan", None, 5000).await {
                    if f.name != "Busy" && f.name != "InProgress" {
                        let mut s = ps.borrow_mut();
                        s.scan_hold = None;
                        s.errors.insert("scan".into(), if f.name == "NotAvailable" { "can't scan while connecting".into() } else { format!("scan failed: {}", f.message) });
                    }
                }
                rr();
            });
        }
    };
    let do_scan: Rc<dyn Fn()> = Rc::new(do_scan);
    {
        let s = do_scan.clone();
        scan_btn.connect_clicked(move |_| s());
    }
    {
        let s = do_scan.clone();
        let ps = ps.clone();
        p.on_key(move |k, m| {
            let typing = ps.borrow().entry.as_ref().is_some_and(|e| e.has_focus());
            if (k == gtk::gdk::Key::s || k == gtk::gdk::Key::S) && !typing && !m.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK) {
                s();
                return true;
            }
            false
        });
    }

    // power
    {
        let (c, i, ps, guard, rr) = (conn.clone(), Rc::downgrade(i), ps.clone(), sw_guard.clone(), rerender.clone());
        sw.connect_state_set(move |_, on| {
            if guard.get() {
                return glib::Propagation::Proceed;
            }
            ps.borrow_mut().switch_hold = Some((on, std::time::Instant::now()));
            ps.borrow_mut().errors.remove("power");
            let Some(i) = i.upgrade() else { return glib::Propagation::Proceed };
            let st = i.state.borrow().clone();
            if on {
                util::spawn(&["rfkill", "unblock", "wlan"]);
            }
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                let r = if let Some(dev) = &st.device {
                    dbus::set_prop(&c, IWD, dev, "net.connman.iwd.Device", "Powered", on.to_variant()).await
                } else if let Some(a) = &st.adapter {
                    dbus::set_prop(&c, IWD, a, "net.connman.iwd.Adapter", "Powered", on.to_variant()).await
                } else {
                    Ok(().to_variant())
                };
                if let Err(f) = r {
                    let mut s = ps.borrow_mut();
                    s.errors.insert("power".into(), format!("couldn't turn wi-fi {}: {}", if on { "on" } else { "off" }, f.message));
                    s.switch_hold = None;
                }
                rr();
            });
            glib::Propagation::Proceed
        });
    }

    // connect / disconnect / forget
    let connect: Rc<dyn Fn(String, Option<String>)> = {
        let (c, i, ps, rr) = (conn.clone(), Rc::downgrade(i), ps.clone(), rerender.clone());
        Rc::new(move |key: String, pw: Option<String>| {
            let Some(i) = i.upgrade() else { return };
            let Some(n) = i.state.borrow().nets.iter().find(|n| n.path == key).cloned() else { return };
            if ps.borrow().busy.contains_key(&key) {
                return;
            }
            if let Some(pw) = &pw {
                if n.kind == "psk" && !(8..=63).contains(&pw.chars().count()) {
                    ps.borrow_mut().errors.insert(key.clone(), "wi-fi passwords are 8 to 63 characters".into());
                    rr();
                    return;
                }
                ps.borrow_mut().passwords.insert(key.clone(), pw.clone());
            }
            {
                let mut s = ps.borrow_mut();
                s.busy.insert(key.clone(), "connecting".into());
                s.errors.remove(&key);
                s.expanded = None;
                s.entry = None;
            }
            rr();
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                if pw.is_some() {
                    if let Some(k) = &n.known {
                        // iwd never asks again for a saved network: forget, then connect fresh
                        let _ = dbus::call(&c, IWD, k, "net.connman.iwd.KnownNetwork", "Forget", None, 5000).await;
                    }
                }
                let r = dbus::call(&c, IWD, &key, "net.connman.iwd.Network", "Connect", None, 90_000).await;
                let mut s = ps.borrow_mut();
                s.busy.remove(&key);
                s.passwords.remove(&key);
                if let Err(f) = r {
                    s.errors.insert(key.clone(), err_text(&f, pw.is_some()));
                    if n.kind == "psk" && f.name != "InProgress" {
                        s.expanded = Some(key.clone());
                    }
                }
                drop(s);
                rr();
            });
        })
    };
    let disconnect: Rc<dyn Fn(String)> = {
        let (c, i, ps, rr) = (conn.clone(), Rc::downgrade(i), ps.clone(), rerender.clone());
        Rc::new(move |key: String| {
            let Some(dev) = i.upgrade().and_then(|i| i.state.borrow().device.clone()) else { return };
            ps.borrow_mut().busy.insert(key.clone(), "disconnecting".into());
            ps.borrow_mut().expanded = None;
            rr();
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                let r = dbus::call(&c, IWD, &dev, "net.connman.iwd.Station", "Disconnect", None, 10_000).await;
                let mut s = ps.borrow_mut();
                s.busy.remove(&key);
                if let Err(f) = r {
                    s.errors.insert(key, f.message);
                }
                drop(s);
                rr();
            });
        })
    };
    let forget: Rc<dyn Fn(String, String)> = {
        let (c, ps, rr) = (conn.clone(), ps.clone(), rerender.clone());
        Rc::new(move |key: String, known: String| {
            ps.borrow_mut().expanded = None;
            let (c, ps, rr) = (c.clone(), ps.clone(), rr.clone());
            glib::spawn_future_local(async move {
                if let Err(f) = dbus::call(&c, IWD, &known, "net.connman.iwd.KnownNetwork", "Forget", None, 5000).await {
                    ps.borrow_mut().errors.insert(key, f.message);
                }
                rr();
            });
        })
    };

    let draw = {
        let i = Rc::downgrade(i);
        let (ps, sw, guard, info, msgs, list) = (ps.clone(), sw.clone(), sw_guard.clone(), info.clone(), msgs.clone(), list.clone());
        let (detail, spin, scan_l, scan_btn) = (card.detail.clone(), spin.clone(), scan_l.clone(), scan_btn.clone());
        let (connect, disconnect, forget, rr) = (connect.clone(), disconnect.clone(), forget.clone(), rerender.clone());
        let pc = p.clone();
        move || {
            let Some(i) = i.upgrade() else { return };
            let s = i.state.borrow().clone();
            let mut st = ps.borrow_mut();
            // switch: show our request while iwd catches up (up to 4 s)
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

            let cur = s.current().cloned();
            let going = s.nets.iter().find(|n| n.connecting).cloned();
            let d = if s.device.is_none() {
                if s.adapter.is_some() { "off".to_string() } else { "no wireless device".into() }
            } else if let Some(c) = &cur {
                util::esc(&c.name)
            } else if let Some(g) = &going {
                format!("connecting to {}", util::esc(&g.name))
            } else if !s.powered {
                "off".into()
            } else if s.st_state.is_empty() {
                "disconnected".into()
            } else {
                s.st_state.clone()
            };
            detail.set_markup(&d);

            let scanning = s.scanning || st.scan_hold.is_some_and(|t| t.elapsed() < Duration::from_secs(4) && !s.scanning && s.nets.is_empty());
            if s.scanning {
                st.scan_hold = None;
            }
            spin.set_visible(scanning);
            if scanning {
                spin.start();
            } else {
                spin.stop();
            }
            scan_l.set_text(if scanning { "scanning…" } else { "scan" });
            scan_btn.set_visible(s.powered && s.station);

            // connection details
            ui::clear(&info);
            if let Some(c) = &cur {
                let pct = s.rssi.map(|d| (2 * (d + 100)).clamp(0, 100)).unwrap_or(0);
                let mut parts = vec![format!("{pct}%")];
                if let Some(f) = s.freq {
                    parts.push(format!("{:.1} GHz", f as f64 / 1000.0));
                }
                if let Some(b) = s.bitrate {
                    parts.push(format!("{} Mbit/s", b / 10));
                }
                info.append(&ui::kv("signal", &parts.join(&format!(" {} ", ui::dim("·")))).0.tap(|b| b.set_margin_top(2)));
                info.append(&ui::kv("ip", s.ip.as_deref().unwrap_or("…")).0);
                let _ = c;
            }
            if let Some((ifc, ip)) = &s.wired {
                info.append(&ui::kv("wired", &format!("{ifc}  {}", ui::dim(ip.as_deref().unwrap_or("")))).0);
            }

            ui::clear(&msgs);
            for k in ["power", "scan"] {
                if let Some(e) = st.errors.get(k) {
                    msgs.append(&ui::text(e, &["msg", "crit"]));
                }
            }

            // network rows
            let mut items: Vec<ui::Item> = Vec::new();
            if s.device.is_none() || !s.powered {
                let t = if s.adapter.is_none() { "iwd sees no wi-fi device" } else { "wi-fi is off" };
                items.push(("empty".into(), t.into(), Box::new(move || ui::text(t, &["empty-note"]).upcast())));
            } else if s.nets.is_empty() {
                let t = if scanning { "looking for networks…" } else { "no networks found · s to scan again" };
                items.push(("empty".into(), t.into(), Box::new(move || ui::text(t, &["empty-note"]).upcast())));
            }
            let groups = [("saved", s.nets.iter().filter(|n| n.connected || n.known.is_some()).cloned().collect::<Vec<_>>()), ("nearby", s.nets.iter().filter(|n| !(n.connected || n.known.is_some())).cloned().collect())];
            for (title, nets) in groups {
                if nets.is_empty() || !s.powered {
                    continue;
                }
                items.push((format!("sec-{title}"), String::new(), Box::new(move || ui::section(title).upcast())));
                for n in nets.into_iter().take(14) {
                    let busy = st.busy.get(&n.path).cloned().or_else(|| n.connecting.then(|| "connecting".to_string()));
                    let err = st.errors.get(&n.path).cloned();
                    let expanded = st.expanded.as_deref() == Some(&n.path);
                    // not the live dBm: a row must not be rebuilt under a click or while typing
                    let level = if expanded { 0 } else { n.level };
                    let sig = format!("{}|{}|{:?}|{}|{}|{level}|{busy:?}|{err:?}|{expanded}|{:?}", n.name, n.kind, n.known, n.connected, n.connecting, st.agent_ok);
                    let (ps, connect, disconnect, forget, rr, pc) = (ps.clone(), connect.clone(), disconnect.clone(), forget.clone(), rr.clone(), pc.clone());
                    let agent_ok = st.agent_ok;
                    items.push((n.path.clone(), sig, Box::new(move || net_row(&n, busy.as_deref(), err.as_deref(), expanded, agent_ok, &ps, &connect, &disconnect, &forget, &rr, &pc))));
                }
            }
            drop(st);
            list.sync(items);
        }
    };
    *render.borrow_mut() = Some(Rc::new(draw));
    rerender();
    let rr = rerender.clone();
    p.watch(&i.changed, move || rr());
    // fresh results every time the panel opens
    do_scan();
    let w = Rc::downgrade(i);
    p.every(2500, move || {
        if let Some(i) = w.upgrade() {
            i.query();
        }
    });
    card.root.upcast()
}

#[allow(clippy::too_many_arguments)]
fn net_row(
    n: &WifiNet,
    busy: Option<&str>,
    err: Option<&str>,
    expanded: bool,
    agent_ok: Option<bool>,
    ps: &Rc<RefCell<PState>>,
    connect: &Rc<dyn Fn(String, Option<String>)>,
    disconnect: &Rc<dyn Fn(String)>,
    forget: &Rc<dyn Fn(String, String)>,
    rr: &Rc<dyn Fn()>,
    p: &Panel,
) -> gtk::Widget {
    let b = ui::vbox(0, &[]);
    let meta = if n.connected { "connected".to_string() } else if n.kind != "open" { LOCK.to_string() } else { String::new() };
    let row = ui::Row::new(BARS[n.level], &n.name, &meta);
    row.set_active(n.connected);
    if let Some(bz) = busy {
        row.meta.set_markup("");
        row.trail.append(&ui::spinner());
        row.trail.append(&ui::text(bz, &["meta"]));
    }
    b.append(&row.root);
    {
        let (n, ps, connect, rr) = (n.clone(), ps.clone(), connect.clone(), rr.clone());
        let busy = busy.is_some();
        ui::on_click(&row.root, move || {
            if busy || n.connecting {
                return;
            }
            if n.connected || (n.known.is_none() && n.kind != "open") {
                let mut s = ps.borrow_mut();
                s.expanded = if s.expanded.as_deref() == Some(&n.path) { None } else { Some(n.path.clone()) };
                s.errors.remove(&n.path);
                drop(s);
                rr();
            } else {
                connect(n.path.clone(), None);
            }
        });
    }
    if let Some(e) = err {
        b.append(&ui::hbox(0, &["expand"]).tap(|x| x.append(&ui::text(e, &["crit"]))));
    }
    if expanded {
        let ex = ui::hbox(4, &["expand"]);
        if n.connected {
            let (d, key) = (disconnect.clone(), n.path.clone());
            ex.append(&ui::link("disconnect", &[], move || d(key.clone())));
            if let Some(k) = n.known.clone() {
                let (f, key) = (forget.clone(), n.path.clone());
                ex.append(&ui::link("forget", &["danger"], move || f(key.clone(), k.clone())));
            }
        } else if n.kind == "8021x" {
            ex.append(&ui::text("enterprise network: set it up in impala", &["dim"]));
            let pc = p.clone();
            ex.append(&ui::link("impala", &[], move || {
                pc.close();
                util::spawn_sh("omarchy-launch-wifi");
            }));
        } else if agent_ok == Some(false) {
            ex.append(&ui::text("impala holds the password prompt", &["dim"]));
        } else {
            let e = gtk::PasswordEntry::new();
            e.set_show_peek_icon(true);
            e.set_hexpand(true);
            e.set_property("placeholder-text", if n.known.is_some() { "new password" } else { "password" });
            let (c, key) = (connect.clone(), n.path.clone());
            e.connect_activate(move |w| {
                let t = w.text().to_string();
                if !t.is_empty() {
                    c(key.clone(), Some(t));
                }
            });
            ex.append(&e);
            let (c, key, e2) = (connect.clone(), n.path.clone(), e.clone());
            ex.append(&ui::link("connect", &["primary"], move || {
                let t = e2.text().to_string();
                if !t.is_empty() {
                    c(key.clone(), Some(t));
                }
            }));
            let e3 = e.clone();
            glib::idle_add_local_once(move || {
                e3.grab_focus();
            });
            ps.borrow_mut().entry = Some(e);
        }
        b.append(&ex);
    }
    b.upcast()
}
