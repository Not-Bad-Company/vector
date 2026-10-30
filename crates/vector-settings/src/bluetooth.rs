//! Bluetooth over BlueZ. Discovery and the pairing agent live only while this
//! page is visible; all device state comes from ObjectManager and live signals.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gio;
use gtk::prelude::*;

use crate::dbus::{self, Fail, Objects};
use crate::page::{Ctx, Frame, View};
use crate::ui::{self, Keyed, Row};

const BLUEZ: &str = "org.bluez";
const AGENT_PATH: &str = "/org/vectoros/settings/btagent";
const AGENT_XML: &str = r#"<node><interface name="org.bluez.Agent1">
<method name="Release"/><method name="RequestPinCode"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
<method name="DisplayPinCode"><arg type="o" direction="in"/><arg type="s" direction="in"/></method>
<method name="RequestPasskey"><arg type="o" direction="in"/><arg type="u" direction="out"/></method>
<method name="DisplayPasskey"><arg type="o" direction="in"/><arg type="u" direction="in"/><arg type="q" direction="in"/></method>
<method name="RequestConfirmation"><arg type="o" direction="in"/><arg type="u" direction="in"/></method>
<method name="RequestAuthorization"><arg type="o" direction="in"/></method>
<method name="AuthorizeService"><arg type="o" direction="in"/><arg type="s" direction="in"/></method><method name="Cancel"/>
</interface></node>"#;

#[derive(Clone, Debug, Default, PartialEq)]
struct Device {
    path: String,
    name: String,
    icon: String,
    class: u32,
    paired: bool,
    connected: bool,
    trusted: bool,
    blocked: bool,
    battery: Option<i64>,
    rssi: Option<i64>,
    address: String,
    kind: String,
    uuids: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    adapter: Option<String>,
    name: String,
    address: String,
    powered: bool,
    discoverable: bool,
    pairable: bool,
    discoverable_timeout: u32,
    discovering: bool,
    devices: Vec<Device>,
}

#[derive(Default)]
struct Prompt {
    message: String,
    entry: String,
    invocation: Option<gio::DBusMethodInvocation>,
}

fn dev_icon(kind: &str, class: u32) -> &'static str {
    match kind {
        "audio-headset" | "audio-headphones" => "\u{f02cb}",
        "audio-card" => "\u{f04c3}",
        "input-mouse" => "\u{f037d}",
        "input-keyboard" => "\u{f030c}",
        "input-gaming" => "\u{f0296}",
        "phone" => "\u{f03f2}",
        "computer" | "video-display" => "\u{f0379}",
        "input-tablet" => "\u{f04f7}",
        _ => match (class >> 8) & 0x1f {
            1 => "\u{f03f2}",
            2 => "\u{f0379}",
            3 => "\u{f030c}",
            5 => "\u{f02cb}",
            6 => "\u{f0296}",
            _ => "\u{f00af}",
        },
    }
}

fn type_name(icon: &str) -> &'static str {
    match icon {
        "audio-headset" => "headset",
        "audio-headphones" => "headphones",
        "audio-card" => "audio",
        "input-mouse" => "mouse",
        "input-keyboard" => "keyboard",
        "input-gaming" => "game controller",
        "phone" => "phone",
        "computer" => "computer",
        "video-display" => "display",
        "input-tablet" => "tablet",
        _ => "device",
    }
}

fn service_name(uuid: &str) -> &'static str {
    let short = uuid
        .strip_prefix("0000")
        .and_then(|s| s.get(..4))
        .unwrap_or(uuid);
    match short.to_ascii_lowercase().as_str() {
        "110b" => "audio sink",
        "110c" => "remote control",
        "110e" => "remote control target",
        "1108" => "headset",
        "111e" => "handsfree",
        "111f" => "handsfree audio gateway",
        "110a" => "audio source",
        "1124" => "HID",
        "112f" => "phonebook access",
        "1112" => "LAN access",
        "180f" => "battery service",
        "1812" => "HID service",
        _ => "service",
    }
}

async fn collect(c: &gio::DBusConnection) -> State {
    let objs: Objects = dbus::managed(c, BLUEZ, "/").await;
    let Some((ap, ai)) = objs
        .iter()
        .find(|(_, i)| i.contains_key("org.bluez.Adapter1"))
    else {
        return State::default();
    };
    let a = &ai["org.bluez.Adapter1"];
    let mut devices = Vec::new();
    for (path, ifs) in &objs {
        let Some(d) = ifs.get("org.bluez.Device1") else {
            continue;
        };
        if !path.starts_with(&format!("{ap}/")) {
            continue;
        }
        let name = {
            let alias = dbus::get_str(d, "Alias");
            if alias.is_empty() {
                dbus::get_str(d, "Name")
            } else {
                alias
            }
        };
        if name.is_empty() || (!dbus::get_bool(d, "Paired") && dbus::get_str(d, "Name").is_empty())
        {
            continue;
        }
        let uuids = d
            .get("UUIDs")
            .map(|v| {
                (0..v.n_children())
                    .filter_map(|i| v.child_value(i).str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        devices.push(Device {
            path: path.clone(),
            name,
            icon: dbus::get_str(d, "Icon"),
            class: dbus::get_i64(d, "Class").unwrap_or(0) as u32,
            paired: dbus::get_bool(d, "Paired"),
            connected: dbus::get_bool(d, "Connected"),
            trusted: dbus::get_bool(d, "Trusted"),
            blocked: dbus::get_bool(d, "Blocked"),
            battery: ifs
                .get("org.bluez.Battery1")
                .and_then(|b| dbus::get_i64(b, "Percentage")),
            rssi: dbus::get_i64(d, "RSSI"),
            address: dbus::get_str(d, "Address"),
            kind: dbus::get_str(d, "Icon"),
            uuids,
        });
    }
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| b.paired.cmp(&a.paired))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    State {
        adapter: Some(ap.clone()),
        name: dbus::get_str(a, "Alias"),
        address: dbus::get_str(a, "Address"),
        powered: dbus::get_bool(a, "Powered"),
        discoverable: dbus::get_bool(a, "Discoverable"),
        pairable: dbus::get_bool(a, "Pairable"),
        discoverable_timeout: dbus::get_i64(a, "DiscoverableTimeout").unwrap_or(0) as u32,
        discovering: dbus::get_bool(a, "Discovering"),
        devices,
    }
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
        "AuthenticationFailed" | "AuthenticationRejected" | "Rejected" => {
            "pairing refused: put it in pairing mode".into()
        }
        "AuthenticationCanceled" => "pairing cancelled".into(),
        "AuthenticationTimeout" => "pairing timed out".into(),
        "ConnectionAttemptFailed" => "couldn't reach it".into(),
        "InProgress" => "busy with it already, try again in a moment".into(),
        "NotReady" => "adapter not ready".into(),
        "DoesNotExist" => "it went away: scan again".into(),
        _ => m,
    }
}

struct Bluetooth {
    conn: gio::DBusConnection,
    state: RefCell<State>,
    frame: Frame,
    power: ui::Switch,
    msg: gtk::Box,
    adapter_sec: gtk::Box,
    adapter_group: gtk::Box,
    own: Keyed,
    nearby: Keyed,
    own_sec: gtk::Box,
    near_sec: gtk::Box,
    scan_label: gtk::Label,
    scan_spin: gtk::Spinner,
    scan_button: gtk::Button,
    adapter_entry: RefCell<Option<gtk::Entry>>,
    adapter_address: RefCell<Option<gtk::Label>>,
    adapter_discoverable: RefCell<Option<ui::Switch>>,
    adapter_pairable: RefCell<Option<ui::Switch>>,
    expanded: RefCell<Option<String>>,
    busy: RefCell<HashMap<String, String>>,
    errors: RefCell<HashMap<String, String>>,
    prompts: RefCell<HashMap<String, Prompt>>,
    agent: RefCell<Option<gio::RegistrationId>>,
    agent_ok: Cell<Option<bool>>,
    sub: RefCell<Option<dbus::Sub>>,
    pending: Cell<bool>,
    power_hold: Cell<Option<(bool, Instant)>>,
}

pub fn page(ctx: &Ctx) -> View {
    let Some(conn) = ctx.system.clone() else {
        let f = Frame::new("bluetooth");
        f.body.append(&ui::text("no system bus", &["note"]));
        return View::new(&f.root);
    };
    let frame = Frame::new("bluetooth");
    let w: Rc<Bluetooth> = Rc::new_cyclic(|weak: &std::rc::Weak<Bluetooth>| {
        let wk = weak.clone();
        let power = ui::Switch::new(move |on| {
            if let Some(w) = wk.upgrade() {
                w.set_power(on);
            }
        });
        frame.controls.append(&power.sw);
        let msg = ui::vbox(0, &[]);
        frame.body.append(&msg);
        let adapter_sec = ui::section("this laptop");
        frame.body.append(&adapter_sec);
        let adapter_group = ui::group();
        frame.body.append(&adapter_group);
        let own_sec = ui::section("my devices");
        frame.body.append(&own_sec);
        let own = Keyed::new(&["group"]);
        frame.body.append(&own.root);
        let near_sec = ui::section("nearby");
        frame.body.append(&near_sec);
        let nearby = Keyed::new(&["group"]);
        frame.body.append(&nearby.root);
        let scan_spin = gtk::Spinner::new();
        scan_spin.set_visible(false);
        let scan_label = ui::text("scan", &[]);
        let content = ui::hbox(6, &[]);
        content.append(&scan_spin);
        content.append(&scan_label);
        let scan_button = gtk::Button::new();
        scan_button.set_child(Some(&content));
        scan_button.add_css_class("link");
        let wk = weak.clone();
        scan_button.connect_clicked(move |_| {
            if let Some(w) = wk.upgrade() {
                w.scan(true);
            }
        });
        frame.foot_add(&scan_button);
        frame.foot_add(&ui::spacer());
        frame.foot_add(&ui::text("bluez", &["faint"]));
        Bluetooth {
            conn,
            state: RefCell::default(),
            frame,
            power,
            msg,
            adapter_sec,
            adapter_group,
            own,
            nearby,
            own_sec,
            near_sec,
            scan_label,
            scan_spin,
            scan_button,
            adapter_entry: RefCell::default(),
            adapter_address: RefCell::default(),
            adapter_discoverable: RefCell::default(),
            adapter_pairable: RefCell::default(),
            expanded: RefCell::default(),
            busy: RefCell::default(),
            errors: RefCell::default(),
            prompts: RefCell::default(),
            agent: RefCell::default(),
            agent_ok: Cell::new(None),
            sub: RefCell::default(),
            pending: Cell::new(false),
            power_hold: Cell::default(),
        }
    });
    let root = w.frame.root.clone();
    let (a, b, c, d) = (w.clone(), w.clone(), w.clone(), w.clone());
    View::new(&root)
        .on_show(move || a.start())
        .on_hide(move || b.pause())
        .on_stop(move || c.pause())
        .every(3000, move || d.query())
}

impl Bluetooth {
    fn start(self: &Rc<Self>) {
        if self.sub.borrow().is_none() {
            let wk = Rc::downgrade(self);
            *self.sub.borrow_mut() = Some(dbus::subscribe(&self.conn, BLUEZ, move |_, _, _| {
                if let Some(w) = wk.upgrade() {
                    w.schedule();
                }
            }));
        }
        self.register_agent();
        self.query();
        self.scan(true);
    }
    fn pause(self: &Rc<Self>) {
        self.sub.borrow_mut().take();
        if let Some(a) = self.state.borrow().adapter.clone() {
            self.call_adapter(&a, "StopDiscovery", None);
        }
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
        let (wk, c) = (Rc::downgrade(self), self.conn.clone());
        glib::spawn_future_local(async move {
            let s = collect(&c).await;
            if let Some(w) = wk.upgrade() {
                *w.state.borrow_mut() = s;
                w.draw();
            }
        });
    }
    fn register_agent(self: &Rc<Self>) {
        if self.agent.borrow().is_some() {
            return;
        }
        let Ok(node) = gio::DBusNodeInfo::for_xml(AGENT_XML) else {
            return;
        };
        let Some(iface) = node.lookup_interface("org.bluez.Agent1") else {
            return;
        };
        let wk = Rc::downgrade(self);
        let reg = self
            .conn
            .register_object(AGENT_PATH, &iface)
            .method_call(move |_, _, _, _, method, params, inv| {
                let Some(w) = wk.upgrade() else {
                    inv.return_dbus_error("org.bluez.Error.Canceled", "page closed");
                    return;
                };
                let path = params.child_value(0).str().unwrap_or("").to_string();
                let name = w
                    .state
                    .borrow()
                    .devices
                    .iter()
                    .find(|d| d.path == path)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "device".into());
                match method {
                    "RequestConfirmation" => {
                        let code = params.child_value(1).get::<u32>().unwrap_or(0);
                        w.prompt(
                            path,
                            format!("{name}: code {code:06} — does it match?"),
                            Some(inv.clone()),
                        );
                    }
                    "RequestPasskey" => {
                        w.prompt(path, format!("{name}: enter passkey"), Some(inv.clone()))
                    }
                    "RequestPinCode" => {
                        w.prompt(path, format!("{name}: enter PIN"), Some(inv.clone()))
                    }
                    "DisplayPasskey" => {
                        let code = params.child_value(1).get::<u32>().unwrap_or(0);
                        let entered = params.child_value(2).get::<u16>().unwrap_or(0);
                        w.prompt(
                            path,
                            format!("{name}: type {code:06} on device ({entered} entered"),
                            None,
                        );
                        inv.return_value(None);
                    }
                    "DisplayPinCode" => {
                        let val = params.child_value(1);
                        let code = val.str().unwrap_or("").to_string();
                        w.prompt(path, format!("{name}: type {code} on device"), None);
                        inv.return_value(None);
                    }
                    "AuthorizeService" | "RequestAuthorization" => {
                        let allowed = w
                            .state
                            .borrow()
                            .devices
                            .iter()
                            .any(|d| d.path == path && d.trusted);
                        if allowed {
                            inv.return_value(None);
                        } else {
                            inv.return_dbus_error(
                                "org.bluez.Error.Rejected",
                                "device is not trusted",
                            );
                        }
                    }
                    "Cancel" | "Release" => {
                        w.prompts.borrow_mut().clear();
                        inv.return_value(None);
                    }
                    _ => inv.return_dbus_error("org.bluez.Error.Rejected", "unsupported request"),
                }
                w.draw();
            })
            .build();
        if let Ok(id) = reg {
            *self.agent.borrow_mut() = Some(id);
            self.agent_ok.set(Some(false));
            let (wk, c) = (Rc::downgrade(self), self.conn.clone());
            glib::spawn_future_local(async move {
                let path = glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path");
                let r = dbus::call(
                    &c,
                    BLUEZ,
                    "/org/bluez",
                    "org.bluez.AgentManager1",
                    "RegisterAgent",
                    Some((path.clone(), "KeyboardDisplay").to_variant()),
                    3000,
                )
                .await;
                let ok = r.is_ok() || r.err().is_some_and(|f| f.name == "AlreadyExists");
                if ok {
                    let _ = dbus::call(
                        &c,
                        BLUEZ,
                        "/org/bluez",
                        "org.bluez.AgentManager1",
                        "RequestDefaultAgent",
                        Some((path,).to_variant()),
                        3000,
                    )
                    .await;
                }
                if let Some(w) = wk.upgrade() {
                    w.agent_ok.set(Some(ok));
                    w.draw();
                }
            });
        }
    }
    fn unregister_agent(&self) {
        let Some(id) = self.agent.borrow_mut().take() else {
            return;
        };
        let c = self.conn.clone();
        let registered = self.agent_ok.replace(None) == Some(true);
        let done = move |_: Result<glib::Variant, glib::Error>| {
            let _ = c.unregister_object(id);
        };
        if registered {
            self.conn.call(
                Some(BLUEZ),
                "/org/bluez",
                "org.bluez.AgentManager1",
                "UnregisterAgent",
                Some(
                    &(glib::variant::ObjectPath::try_from(AGENT_PATH).expect("static path"),)
                        .to_variant(),
                ),
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
    fn prompt(&self, path: String, message: String, invocation: Option<gio::DBusMethodInvocation>) {
        self.prompts.borrow_mut().insert(
            path,
            Prompt {
                message,
                entry: String::new(),
                invocation,
            },
        );
    }
    fn scan(self: &Rc<Self>, on: bool) {
        let Some(a) = self.state.borrow().adapter.clone() else {
            return;
        };
        if on && !self.state.borrow().powered {
            return;
        }
        let (w, c, a) = (self.clone(), self.conn.clone(), a);
        glib::spawn_future_local(async move {
            let r = dbus::call(
                &c,
                BLUEZ,
                &a,
                "org.bluez.Adapter1",
                if on {
                    "StartDiscovery"
                } else {
                    "StopDiscovery"
                },
                None,
                5000,
            )
            .await;
            if let Err(f) = r
                && f.name != "InProgress"
                && f.name != "NotReady"
            {
                w.errors
                    .borrow_mut()
                    .insert("scan".into(), format!("couldn't scan: {}", bt_error(&f)));
            }
            w.query();
        });
    }
    fn call_adapter(self: &Rc<Self>, a: &str, method: &str, args: Option<glib::Variant>) {
        let (w, c, a, m) = (
            self.clone(),
            self.conn.clone(),
            a.to_string(),
            method.to_string(),
        );
        glib::spawn_future_local(async move {
            if let Err(f) = dbus::call(&c, BLUEZ, &a, "org.bluez.Adapter1", &m, args, 3000).await
                && f.name != "InProgress"
                && f.name != "NotReady"
            {
                w.errors.borrow_mut().insert("scan".into(), bt_error(&f));
                w.draw();
            }
        });
    }
    fn set_power(self: &Rc<Self>, on: bool) {
        let Some(a) = self.state.borrow().adapter.clone() else {
            return;
        };
        self.power_hold.set(Some((on, Instant::now())));
        if on {
            ui::spawn(&["rfkill", "unblock", "bluetooth"]);
        }
        let (w, c) = (self.clone(), self.conn.clone());
        glib::spawn_future_local(async move {
            let mut err = None;
            for _ in 0..3 {
                match dbus::set_prop(
                    &c,
                    BLUEZ,
                    &a,
                    "org.bluez.Adapter1",
                    "Powered",
                    on.to_variant(),
                )
                .await
                {
                    Ok(_) => {
                        err = None;
                        break;
                    }
                    Err(f) => {
                        err = Some(f);
                        glib::timeout_future(Duration::from_millis(400)).await;
                    }
                }
            }
            if let Some(f) = err {
                w.errors.borrow_mut().insert(
                    "power".into(),
                    format!(
                        "couldn't turn bluetooth {}: {}",
                        if on { "on" } else { "off" },
                        f.message
                    ),
                );
                w.power_hold.set(None);
            }
            w.query();
        });
    }
    fn set_device(self: &Rc<Self>, path: &str, prop: &str, on: bool) {
        let (w, c, p, k) = (
            self.clone(),
            self.conn.clone(),
            path.to_string(),
            prop.to_string(),
        );
        glib::spawn_future_local(async move {
            if let Err(f) =
                dbus::set_prop(&c, BLUEZ, &p, "org.bluez.Device1", &k, on.to_variant()).await
            {
                w.errors.borrow_mut().insert(p, bt_error(&f));
            }
            w.query();
        });
    }
    fn rename(self: &Rc<Self>, path: &str, name: String, adapter: bool) {
        let (w, c, p) = (self.clone(), self.conn.clone(), path.to_string());
        let iface = if adapter {
            "org.bluez.Adapter1"
        } else {
            "org.bluez.Device1"
        };
        glib::spawn_future_local(async move {
            if let Err(f) = dbus::set_prop(&c, BLUEZ, &p, iface, "Alias", name.to_variant()).await {
                w.errors.borrow_mut().insert(p, bt_error(&f));
            }
            w.query();
        });
    }
    fn act(self: &Rc<Self>, path: &str) {
        let Some(d) = self
            .state
            .borrow()
            .devices
            .iter()
            .find(|d| d.path == path)
            .cloned()
        else {
            return;
        };
        if self.busy.borrow().contains_key(path) {
            return;
        }
        self.errors.borrow_mut().remove(path);
        if d.connected {
            self.run_device(path, "Disconnect", "disconnecting", 15000);
        } else if d.paired {
            self.run_device(path, "Connect", "connecting", 45000);
        } else {
            self.pair(path);
        }
    }
    fn run_device(self: &Rc<Self>, path: &str, method: &str, label: &str, timeout: i32) {
        let (w, c, p, m) = (
            self.clone(),
            self.conn.clone(),
            path.to_string(),
            method.to_string(),
        );
        self.busy.borrow_mut().insert(p.clone(), label.into());
        self.draw();
        glib::spawn_future_local(async move {
            let r = dbus::call(&c, BLUEZ, &p, "org.bluez.Device1", &m, None, timeout).await;
            w.busy.borrow_mut().remove(&p);
            if let Err(f) = r
                && !(m == "Connect" && f.name == "AlreadyConnected")
            {
                w.errors.borrow_mut().insert(p, bt_error(&f));
            }
            w.query();
        });
    }
    fn pair(self: &Rc<Self>, path: &str) {
        let (w, c, p) = (self.clone(), self.conn.clone(), path.to_string());
        self.busy.borrow_mut().insert(p.clone(), "pairing".into());
        self.draw();
        glib::spawn_future_local(async move {
            let r = dbus::call(&c, BLUEZ, &p, "org.bluez.Device1", "Pair", None, 90000).await;
            w.prompts.borrow_mut().remove(&p);
            if let Err(f) = r
                && f.name != "AlreadyExists"
            {
                w.busy.borrow_mut().remove(&p);
                w.errors.borrow_mut().insert(p, bt_error(&f));
                w.query();
                return;
            }
            let _ = dbus::set_prop(
                &c,
                BLUEZ,
                &p,
                "org.bluez.Device1",
                "Trusted",
                true.to_variant(),
            )
            .await;
            if let Err(f) =
                dbus::call(&c, BLUEZ, &p, "org.bluez.Device1", "Connect", None, 45000).await
                && f.name != "AlreadyConnected"
            {
                w.errors.borrow_mut().insert(p.clone(), bt_error(&f));
            }
            w.busy.borrow_mut().remove(&p);
            w.query();
        });
    }
    fn forget(self: &Rc<Self>, path: &str) {
        let Some(a) = self.state.borrow().adapter.clone() else {
            return;
        };
        let (w, c, p) = (self.clone(), self.conn.clone(), path.to_string());
        glib::spawn_future_local(async move {
            let op = glib::variant::ObjectPath::try_from(p.clone()).ok();
            if let Some(op) = op
                && let Err(f) = dbus::call(
                    &c,
                    BLUEZ,
                    &a,
                    "org.bluez.Adapter1",
                    "RemoveDevice",
                    Some((op,).to_variant()),
                    5000,
                )
                .await
            {
                w.errors.borrow_mut().insert(p.clone(), bt_error(&f));
            }
            w.expanded.borrow_mut().take();
            w.query();
        });
    }
    fn draw(self: &Rc<Self>) {
        let s = self.state.borrow().clone();
        let connected = s.devices.iter().filter(|d| d.connected).count();
        self.frame.detail.set_text(&if s.adapter.is_none() {
            "no adapter".to_string()
        } else if !s.powered {
            "off".to_string()
        } else if connected == 0 {
            "on".to_string()
        } else {
            format!("on · {connected} connected")
        });
        let mut shown = s.powered;
        if let Some((want, t)) = self.power_hold.get() {
            if want != s.powered && t.elapsed() < Duration::from_secs(4) {
                shown = want
            } else {
                self.power_hold.set(None)
            }
        }
        self.power.set(shown);
        self.power.sw.set_visible(s.adapter.is_some());
        ui::clear(&self.msg);
        for key in ["power", "scan"] {
            if let Some(e) = self.errors.borrow().get(key) {
                self.msg.append(&ui::text(e, &["msg", "crit"]));
            }
        }
        if self.agent_ok.get() == Some(false) && s.powered {
            self.msg.append(&ui::text(
                "another program holds the Bluetooth pairing agent",
                &["msg", "warn"],
            ));
        }
        self.adapter_sec.set_visible(s.adapter.is_some());
        self.adapter_group.set_visible(s.adapter.is_some());
        if s.adapter.is_some() {
            self.draw_adapter(&s);
        }
        self.own_sec.set_visible(s.adapter.is_some());
        self.near_sec.set_visible(s.adapter.is_some() && s.powered);
        let mut own: Vec<ui::Item> = Vec::new();
        let mut near: Vec<ui::Item> = Vec::new();
        for d in &s.devices {
            if d.paired {
                let w = self.clone();
                let d = d.clone();
                let busy = self.busy.borrow().get(&d.path).cloned();
                let err = self.errors.borrow().get(&d.path).cloned();
                let expanded = self.expanded.borrow().as_deref() == Some(&d.path);
                let prompt = self
                    .prompts
                    .borrow()
                    .get(&d.path)
                    .map(|p| p.message.clone());
                let sig = format!(
                    "{d:?}|{busy:?}|{err:?}|{expanded}|{prompt:?}|{}",
                    self.prompts
                        .borrow()
                        .get(&d.path)
                        .map(|p| p.entry.clone())
                        .unwrap_or_default()
                );
                own.push((
                    d.path.clone(),
                    sig,
                    Box::new(move || {
                        w.device_row(
                            &d,
                            busy.as_deref(),
                            err.as_deref(),
                            expanded,
                            prompt.as_deref(),
                            true,
                        )
                    }),
                ));
            } else if s.powered {
                let w = self.clone();
                let d = d.clone();
                let busy = self.busy.borrow().get(&d.path).cloned();
                let err = self.errors.borrow().get(&d.path).cloned();
                let prompt = self
                    .prompts
                    .borrow()
                    .get(&d.path)
                    .map(|p| p.message.clone());
                let sig = format!("{d:?}|{busy:?}|{err:?}|{prompt:?}");
                near.push((
                    d.path.clone(),
                    sig,
                    Box::new(move || {
                        w.device_row(
                            &d,
                            busy.as_deref(),
                            err.as_deref(),
                            false,
                            prompt.as_deref(),
                            false,
                        )
                    }),
                ));
            }
        }
        own.sort_by(|a, b| a.0.cmp(&b.0));
        near.sort_by(|a, b| {
            let da = s.devices.iter().find(|d| d.path == a.0);
            let db = s.devices.iter().find(|d| d.path == b.0);
            db.and_then(|d| d.rssi)
                .unwrap_or(-1000)
                .cmp(&da.and_then(|d| d.rssi).unwrap_or(-1000))
        });
        if own.is_empty() {
            own.push((
                "empty".into(),
                "none paired".into(),
                Box::new(|| ui::text("no paired devices", &["note"]).upcast()),
            ));
        }
        if near.is_empty() && s.powered {
            near.push((
                "empty".into(),
                "none".into(),
                Box::new(|| ui::text("looking for nearby devices…", &["note"]).upcast()),
            ));
        }
        self.own.sync(own);
        self.nearby.sync(near);
        let scanning = s.discovering;
        self.scan_spin.set_visible(scanning);
        self.scan_spin.set_spinning(scanning);
        self.scan_label
            .set_text(if scanning { "scanning…" } else { "scan" });
        self.scan_button.set_visible(s.powered);
    }
    fn draw_adapter(self: &Rc<Self>, s: &State) {
        // Keep entries and switches alive across property refreshes.
        if unsafe { self.adapter_group.data::<bool>("bt-adapter") }.is_some() {
            if let Some(entry) = self.adapter_entry.borrow().as_ref() {
                if !entry.has_focus() {
                    entry.set_text(&s.name);
                }
            }
            if let Some(label) = self.adapter_address.borrow().as_ref() {
                label.set_text(&s.address);
            }
            if let Some(sw) = self.adapter_discoverable.borrow().as_ref() {
                sw.set(s.discoverable);
            }
            if let Some(sw) = self.adapter_pairable.borrow().as_ref() {
                sw.set(s.pairable);
            }
            return;
        }
        unsafe {
            self.adapter_group.set_data("bt-adapter", true);
        }
        let name = ui::text("adapter name", &["k"]);
        name.set_width_chars(14);
        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_text(&s.name);
        let line = ui::hbox(8, &["kv"]);
        line.append(&name);
        line.append(&entry);
        *self.adapter_entry.borrow_mut() = Some(entry.clone());
        let c = self.clone();
        let p = s.adapter.clone().unwrap_or_default();
        let e = entry.clone();
        line.append(&ui::link("save", &[], move || {
            c.rename(&p, e.text().to_string(), true)
        }));
        self.adapter_group.append(&line);
        let address_row = ui::hbox(12, &["kv"]);
        address_row.append(&ui::text("address", &["k"]));
        let address = ui::text(&s.address, &["v"]);
        address.set_selectable(true);
        address_row.append(&address);
        self.adapter_group.append(&address_row);
        *self.adapter_address.borrow_mut() = Some(address);
        let c = self.clone();
        let p = s.adapter.clone().unwrap_or_default();
        let sw = ui::Switch::new(move |on| c.set_adapter_prop(&p, "Discoverable", on));
        *self.adapter_discoverable.borrow_mut() = Some(sw.clone());
        sw.set(s.discoverable);
        let row = ui::hbox(12, &["kv"]);
        row.append(&ui::text(
            &format!(
                "discoverable{}",
                if s.discoverable_timeout > 0 {
                    format!(" · {}s timeout", s.discoverable_timeout)
                } else {
                    String::new()
                }
            ),
            &["k"],
        ));
        row.append(&ui::spacer());
        row.append(&sw.sw);
        self.adapter_group.append(&row);
        let c = self.clone();
        let p = s.adapter.clone().unwrap_or_default();
        let sw = ui::Switch::new(move |on| c.set_adapter_prop(&p, "Pairable", on));
        *self.adapter_pairable.borrow_mut() = Some(sw.clone());
        sw.set(s.pairable);
        let row = ui::hbox(12, &["kv"]);
        row.append(&ui::text("pairable", &["k"]));
        row.append(&ui::spacer());
        row.append(&sw.sw);
        self.adapter_group.append(&row);
    }
    fn set_adapter_prop(self: &Rc<Self>, path: &str, prop: &str, on: bool) {
        let (w, c, p, k) = (
            self.clone(),
            self.conn.clone(),
            path.to_string(),
            prop.to_string(),
        );
        glib::spawn_future_local(async move {
            if let Err(f) =
                dbus::set_prop(&c, BLUEZ, &p, "org.bluez.Adapter1", &k, on.to_variant()).await
            {
                w.errors.borrow_mut().insert("power".into(), bt_error(&f));
            }
            w.query();
        });
    }
    fn device_row(
        self: &Rc<Self>,
        d: &Device,
        busy: Option<&str>,
        err: Option<&str>,
        expanded: bool,
        prompt: Option<&str>,
        paired: bool,
    ) -> gtk::Widget {
        let mut meta = Vec::new();
        if d.connected {
            meta.push("connected".to_string());
        }
        if let Some(b) = d.battery {
            meta.push(format!("{b}%"));
        }
        meta.push(type_name(&d.kind).into());
        let row = Row::new(
            dev_icon(&d.icon, d.class),
            &d.name,
            &ui::dim(&meta.join(" · ")),
        );
        if let Some(b) = busy {
            row.meta.set_text(b);
            row.trail
                .append(&gtk::Spinner::builder().spinning(true).build());
        }
        let key = d.path.clone();
        let w = self.clone();
        row.on_click(move || {
            if paired {
                if *w.expanded.borrow() == Some(key.clone()) {
                    w.expanded.borrow_mut().take();
                } else {
                    *w.expanded.borrow_mut() = Some(key.clone());
                }
            } else {
                w.act(&key);
            }
            w.draw();
        });
        let key = d.path.clone();
        let w = self.clone();
        row.trail.append(&ui::link(
            if d.connected {
                "disconnect"
            } else if paired {
                "connect"
            } else {
                "pair"
            },
            &[],
            move || w.act(&key),
        ));
        if let Some(p) = prompt {
            let ex = row.expand();
            ex.append(&ui::text(p, &["msg"]));
            if p.contains("matches?") {
                let key = d.path.clone();
                let w = self.clone();
                ex.append(&ui::link("matches", &[], move || w.answer(&key, true)));
                let key = d.path.clone();
                let w = self.clone();
                ex.append(&ui::link("doesn't match", &[], move || {
                    w.answer(&key, false)
                }));
            } else if p.contains("enter passkey") || p.contains("enter PIN") {
                let ent = gtk::Entry::new();
                ent.set_placeholder_text(Some("code"));
                ex.append(&ent);
                let key = d.path.clone();
                let w = self.clone();
                let ent2 = ent.clone();
                ex.append(&ui::link("send", &[], move || {
                    w.answer_text(&key, &ent2.text())
                }));
            }
        }
        if let Some(e) = err {
            let ex = row.expand();
            ex.append(&ui::text(e, &["msg", "crit"]));
        }
        if expanded && paired {
            let ex = row.expand();
            ex.append(&ui::kv("address", &ui::esc(&d.address)));
            ex.append(&ui::kv("type", type_name(&d.kind)));
            let c = self.clone();
            let p = d.path.clone();
            let sw = ui::Switch::new(move |on| c.set_device(&p, "Trusted", on));
            sw.set(d.trusted);
            ex.append(&switch_line("trusted", sw));
            let c = self.clone();
            let p = d.path.clone();
            let sw = ui::Switch::new(move |on| c.set_device(&p, "Blocked", on));
            sw.set(d.blocked);
            ex.append(&switch_line("blocked", sw));
            let ent = gtk::Entry::new();
            ent.set_text(&d.name);
            ent.set_hexpand(true);
            let rr = ui::hbox(8, &[]);
            rr.append(&ent);
            let c = self.clone();
            let p = d.path.clone();
            let ee = ent.clone();
            rr.append(&ui::link("rename", &[], move || {
                c.rename(&p, ee.text().to_string(), false)
            }));
            ex.append(&rr);
            let services = d
                .uuids
                .iter()
                .map(|u| service_name(u))
                .collect::<Vec<_>>()
                .join(", ");
            let services = if services.is_empty() {
                "none".to_string()
            } else {
                ui::esc(&services)
            };
            ex.append(&ui::kv("services", &services));
            let f = ui::link("forget", &["crit"], {
                let w = self.clone();
                let p = d.path.clone();
                move || {
                    let mut x = w.errors.borrow_mut();
                    if x.get(&p).map(String::as_str) == Some("forget?") {
                        drop(x);
                        w.forget(&p);
                    } else {
                        x.insert(p.clone(), "forget?".into());
                        drop(x);
                        w.draw();
                    }
                }
            });
            ex.append(&f);
        }
        row.root.upcast()
    }
    fn answer(self: &Rc<Self>, path: &str, yes: bool) {
        if let Some(p) = self.prompts.borrow_mut().remove(path) {
            if let Some(inv) = p.invocation {
                if yes {
                    inv.return_value(None)
                } else {
                    inv.return_dbus_error("org.bluez.Error.Rejected", "passkey did not match")
                }
            }
        }
        self.draw();
    }
    fn answer_text(self: &Rc<Self>, path: &str, text: &str) {
        if let Some(p) = self.prompts.borrow_mut().remove(path) {
            if let Some(inv) = p.invocation {
                if p.message.contains("passkey") {
                    if let Ok(n) = text.parse::<u32>() {
                        inv.return_value(Some(&(n,).to_variant()))
                    } else {
                        inv.return_dbus_error("org.bluez.Error.Rejected", "invalid passkey")
                    }
                } else {
                    inv.return_value(Some(&(text,).to_variant()))
                }
            }
        }
        self.draw();
    }
}

fn switch_line(label: &str, sw: ui::Switch) -> gtk::Box {
    let r = ui::hbox(12, &["kv"]);
    r.append(&ui::text(label, &["k"]));
    r.append(&ui::spacer());
    r.append(&sw.sw);
    r
}
