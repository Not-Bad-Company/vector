//! Proton VPN over wg-quick (~/.local/bin/vpn does the root parts).
//! State is read natively: the pv-* interface in /sys/class/net, the kill
//! switch unit over systemd's D-Bus, the script's own setting files.
//! Bar: shield off / connecting / on / blocked. Click: panel (on/off,
//! exit IP and place, kill switch, auto-connect, trust this Wi-Fi,
//! servers). Right: connect/disconnect the last server.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::dbus;
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const ON: &str = "\u{f099d}";
const OFF: &str = "\u{f099c}";
const SERVER: &str = "\u{f059f}";

fn vpn_bin() -> String {
    util::home()
        .join(".local/bin/vpn")
        .to_string_lossy()
        .to_string()
}
fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| util::home().join(".local/state"))
        .join("vpn")
}
fn conf_dir() -> PathBuf {
    util::config_dir().join("vpn")
}

/// pv-nl-free-12 -> NL-FREE#12 (same as the script's pretty()).
pub fn pretty(iface: &str) -> String {
    let n = iface.trim_start_matches("pv-").to_uppercase();
    match n.rsplit_once('-') {
        Some((a, b)) if b.chars().all(|c| c.is_ascii_digit()) => format!("{a}#{b}"),
        _ => n,
    }
}

fn country(name: &str) -> &'static str {
    match name.split(['-', '#']).next().unwrap_or("") {
        "NL" => "netherlands",
        "JP" => "japan",
        "US" => "united states",
        "PL" => "poland",
        "RO" => "romania",
        "NO" => "norway",
        "CA" => "canada",
        "MX" => "mexico",
        "SG" => "singapore",
        "CH" => "switzerland",
        "DE" => "germany",
        "UK" | "GB" => "united kingdom",
        "FR" => "france",
        "SE" => "sweden",
        _ => "",
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub servers: Vec<String>,
    pub active: Option<String>,
    pub up: bool,
    pub blocked: bool,
    pub killswitch: bool,
    pub auto: bool,
    pub ssid: Option<String>,
    pub trusted: bool,
    pub last: Option<String>,
    pub loc: HashMap<String, String>,
}

fn setting(name: &str, default: &str) -> String {
    util::read_trim(conf_dir().join(name))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default.into())
}

async fn unit_active(c: &gio::DBusConnection, unit: &str) -> bool {
    let Ok(v) = dbus::call(
        c,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "GetUnit",
        Some((unit,).to_variant()),
        1000,
    )
    .await
    else {
        return false;
    };
    let Some(path) = v.child_value(0).get::<glib::variant::ObjectPath>() else {
        return false;
    };
    let Ok(st) = dbus::call(
        c,
        "org.freedesktop.systemd1",
        path.as_str(),
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(("org.freedesktop.systemd1.Unit", "ActiveState").to_variant()),
        1000,
    )
    .await
    else {
        return false;
    };
    st.child_value(0)
        .as_variant()
        .and_then(|x| x.str().map(|s| s == "active"))
        .unwrap_or(false)
}

async fn ssid(c: &gio::DBusConnection) -> Option<String> {
    let objs = dbus::managed(c, "net.connman.iwd", "/").await;
    let net = objs
        .values()
        .filter_map(|i| i.get("net.connman.iwd.Station"))
        .map(|st| dbus::get_str(st, "ConnectedNetwork"))
        .find(|s| !s.is_empty())?;
    objs.get(&net)
        .and_then(|i| i.get("net.connman.iwd.Network"))
        .map(|n| dbus::get_str(n, "Name"))
}

async fn collect(c: &gio::DBusConnection) -> State {
    let mut servers: Vec<String> = std::fs::read_dir("/etc/wireguard")
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    e.file_name().to_str().and_then(|n| {
                        n.strip_prefix("pv-")
                            .map(|_| n.trim_end_matches(".conf").to_string())
                    })
                })
                .filter(|n| n.starts_with("pv-"))
                .collect()
        })
        .unwrap_or_default();
    servers.sort();
    let active = std::fs::read_dir("/sys/class/net").ok().and_then(|rd| {
        rd.flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .find(|n| n.starts_with("pv-"))
    });
    let up = active.as_ref().is_some_and(|a| {
        util::read_num(format!("/sys/class/net/{a}/statistics/rx_bytes")).unwrap_or(0) > 0
    });
    let ks_active = unit_active(c, "vpn-killswitch.service").await;
    let ssid = ssid(c).await;
    let trusted_list = util::read(conf_dir().join("trusted")).unwrap_or_default();
    let trusted = ssid
        .as_ref()
        .is_some_and(|s| trusted_list.lines().any(|l| l == s));
    let mut loc = HashMap::new();
    if let Some(v) = util::read_json(state_dir().join("location.json")) {
        if v["key"].as_str() == active.as_deref() {
            for k in ["ip", "city", "country", "org"] {
                if let Some(x) = v[k].as_str() {
                    loc.insert(k.to_string(), x.to_string());
                }
            }
        }
    }
    State {
        servers,
        blocked: ks_active && active.is_none(),
        active,
        up,
        killswitch: setting("killswitch", "on") == "on",
        auto: setting("auto", "on") == "on",
        ssid,
        trusted,
        last: util::read_trim(state_dir().join("last")),
        loc,
    }
}

struct Inner {
    slot: Slot,
    conn: Option<gio::DBusConnection>,
    state: RefCell<State>,
    changed: Notify,
    theme: Rc<Theme>,
    locating: Cell<bool>,
    busy: RefCell<HashMap<String, String>>,
    errors: RefCell<HashMap<String, String>>,
}

pub struct Vpn(Rc<Inner>);

impl Vpn {
    pub fn new(ctx: &Ctx) -> Vpn {
        let inner = Rc::new(Inner {
            slot: ctx.slot("vpn", &[]),
            conn: dbus::system(),
            state: RefCell::default(),
            changed: Notify::default(),
            theme: ctx.theme.clone(),
            locating: Cell::new(false),
            busy: RefCell::default(),
            errors: RefCell::default(),
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_right(move || {
            if let Some(i) = w.upgrade() {
                let on = i.state.borrow().active.is_some();
                i.act(
                    "head",
                    &[if on { "down" } else { "up" }],
                    if on {
                        "disconnecting…"
                    } else {
                        "connecting…"
                    },
                );
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        inner.query();
        util::every(3000, &inner, |i| i.query());
        Vpn(inner)
    }
}

impl Inner {
    fn query(self: &Rc<Self>) {
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let s = collect(&c).await;
            let Some(i) = w.upgrade() else { return };
            // exit IP/place: looked up (and cached) by `vpn location` once per connection
            if s.up && s.loc.is_empty() && !i.locating.replace(true) {
                let w2 = Rc::downgrade(&i);
                let bin = vpn_bin();
                util::bg(
                    move || util::run(&[&bin, "location", ""], Duration::from_secs(8)),
                    move |_| {
                        if let Some(i) = w2.upgrade() {
                            i.locating.set(false);
                            i.query();
                        }
                    },
                );
            }
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
        let (icon, cls) = if s.servers.is_empty() {
            ("󰦜", "dim")
        } else if s.blocked {
            ("󰦞", "crit")
        } else if s.active.is_none() {
            ("󰦜", "dim")
        } else if s.up {
            ("󰦝", "")
        } else {
            ("󰦝", "dim")
        };
        self.slot.set_markup(icon);
        self.slot.set_classes(&[cls]);
        self.slot.bump();
    }

    fn act(self: &Rc<Self>, key: &str, args: &[&str], busy: &str) {
        self.busy.borrow_mut().insert(key.into(), busy.into());
        self.errors.borrow_mut().remove(key);
        self.changed.emit();
        let argv: Vec<String> = std::iter::once(vpn_bin())
            .chain(args.iter().map(|s| s.to_string()))
            .collect();
        let (w, key) = (Rc::downgrade(self), key.to_string());
        util::bg(
            move || {
                let a: Vec<&str> = argv.iter().map(String::as_str).collect();
                match util::run(&a, Duration::from_secs(40)) {
                    Some(o) if o.ok => None,
                    Some(o) => Some(
                        o.stderr
                            .lines()
                            .last()
                            .unwrap_or("failed")
                            .trim_start_matches("vpn: ")
                            .to_string(),
                    ),
                    None => Some("timed out".into()),
                }
            },
            move |err| {
                let Some(i) = w.upgrade() else { return };
                i.busy.borrow_mut().remove(&key);
                if let Some(e) = err {
                    i.errors.borrow_mut().insert(key, e);
                }
                i.query();
                i.changed.emit();
            },
        );
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = self.state.borrow();
        let sh = ui::sheet();
        if s.servers.is_empty() {
            sh.append(&ui::head("vpn", "not set up").0);
            sh.append(&ui::text("run: vpn import ~/Downloads/*.conf", &["dim"]));
            return sh.upcast();
        }
        let detail = if s.blocked {
            ui::colored("blocked", &self.theme.color1)
        } else if let Some(a) = &s.active {
            format!(
                "{} · {}",
                if s.up { "connected" } else { "connecting…" },
                pretty(a)
            )
        } else {
            "off".into()
        };
        sh.append(&ui::head("vpn", &detail).0);
        if s.blocked {
            sh.append(
                &ui::text("tunnel dropped, kill switch is holding traffic", &["dim"])
                    .tap(|l| l.set_margin_top(6)),
            );
        } else if s.active.is_some() && s.up {
            sh.append(
                &ui::tkv("ip", s.loc.get("ip").map(String::as_str).unwrap_or("…"))
                    .tap(|b| b.set_margin_top(6)),
            );
            let place = [s.loc.get("city"), s.loc.get("country")]
                .into_iter()
                .flatten()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            sh.append(&ui::tkv(
                "exit",
                if place.is_empty() { "…" } else { &place },
            ));
        } else if let Some(l) = s.last.as_ref().or(s.servers.first()) {
            sh.append(&ui::tkv("next", &pretty(l)).tap(|b| b.set_margin_top(6)));
        }
        sh.append(&ui::tkv("kill sw", if s.killswitch { "on" } else { "off" }));
        let auto = if !s.auto {
            ui::dim("off")
        } else if s.trusted {
            format!("on {}", ui::dim("· this wi-fi is trusted"))
        } else {
            "on".into()
        };
        sh.append(&ui::tkv("auto", &auto));
        sh.append(&ui::thint(if s.active.is_some() {
            "click servers  ·  right disconnect"
        } else {
            "click servers  ·  right connect"
        }));
        sh.upcast()
    }
}

impl Module for Vpn {
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

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(340);
    let card = Card::new("vpn", "");
    let guard = Rc::new(Cell::new(false));
    let sw = gtk::Switch::new();
    sw.set_valign(gtk::Align::Center);
    card.controls.append(&sw);
    {
        let (i, guard) = (Rc::downgrade(i), guard.clone());
        sw.connect_state_set(move |_, on| {
            if !guard.get() {
                if let Some(i) = i.upgrade() {
                    i.act(
                        "head",
                        &[if on { "up" } else { "down" }],
                        if on {
                            "connecting…"
                        } else {
                            "disconnecting…"
                        },
                    );
                }
            }
            glib::Propagation::Proceed
        });
    }
    let body = ui::vbox(0, &[]);
    card.body.append(&body);
    let hovered = Rc::new(Cell::new(false));
    {
        let m = gtk::EventControllerMotion::new();
        let h = hovered.clone();
        m.connect_enter(move |_, _, _| h.set(true));
        let h = hovered.clone();
        m.connect_leave(move |_| h.set(false));
        body.add_controller(m);
    }
    let fill = {
        let (i, body, detail, sw, guard) = (
            Rc::downgrade(i),
            body.clone(),
            card.detail.clone(),
            sw.clone(),
            guard.clone(),
        );
        move || {
            let Some(i) = i.upgrade() else { return };
            let s = i.state.borrow().clone();
            let busy = i.busy.borrow().clone();
            let errors = i.errors.borrow().clone();
            detail.set_markup(&if let Some(a) = &s.active {
                format!(
                    "{} · {}",
                    if s.up { "connected" } else { "connecting…" },
                    pretty(a)
                )
            } else if s.blocked {
                ui::colored("blocked · tunnel dropped", &i.theme.color1)
            } else {
                "off".into()
            });
            guard.set(true);
            sw.set_active(s.active.is_some());
            sw.set_state(s.active.is_some());
            guard.set(false);
            sw.set_visible(!s.servers.is_empty());
            ui::clear(&body);
            if s.blocked {
                body.append(&ui::text("kill switch is holding all traffic until you reconnect or switch the vpn off", &["msg", "crit"]).tap(|l| l.set_wrap(true)));
            }
            if let Some(b) = busy.get("head") {
                body.append(&ui::hbox(8, &["msg"]).tap(|x| {
                    x.append(&ui::spinner());
                    x.append(&ui::text(b, &["dim"]));
                }));
            }
            if let Some(e) = errors.get("head") {
                body.append(&ui::text(e, &["msg", "crit"]).tap(|l| l.set_wrap(true)));
            }
            if s.servers.is_empty() {
                body.append(&ui::text(
                    "no servers yet · run: vpn import",
                    &["empty-note"],
                ));
                return;
            }
            if s.active.is_some() && s.up {
                body.append(&ui::section("exit").tap(|l| l.add_css_class("first")));
                body.append(
                    &ui::kv(
                        "ip",
                        &format!(
                            "{}  {}",
                            s.loc.get("ip").map(String::as_str).unwrap_or("…"),
                            ui::dim(&util::esc(
                                &s.loc
                                    .get("org")
                                    .map(|o| o
                                        .split_once(' ')
                                        .map(|x| x.1)
                                        .unwrap_or(o)
                                        .to_lowercase())
                                    .unwrap_or_default()
                            ))
                        ),
                    )
                    .0,
                );
                let place = [s.loc.get("city"), s.loc.get("country")]
                    .into_iter()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
                    .to_lowercase();
                if !place.is_empty() {
                    body.append(&ui::kv("place", &util::esc(&place)).0);
                }
            }
            body.append(&ui::section("settings"));
            let mut setting =
                |icon: &str, name: &str, meta: &str, on: bool, args: Vec<&'static str>| {
                    let r = ui::Row::new(icon, name, meta);
                    let i2 = Rc::downgrade(&i);
                    let key = name.to_string();
                    let s2 = ui::switch(on, move |_| {
                        if let Some(i) = i2.upgrade() {
                            i.act(&key, &args, "…");
                        }
                    });
                    r.trail.append(&s2);
                    body.append(&r.root);
                    if let Some(e) = errors.get(name) {
                        body.append(&ui::text(e, &["msg", "crit"]));
                    }
                };
            setting(
                if s.killswitch { "\u{f0565}" } else { OFF },
                "kill switch",
                "if it drops",
                s.killswitch,
                vec!["set", "killswitch", if s.killswitch { "off" } else { "on" }],
            );
            setting(
                "\u{f05a9}",
                "auto-connect",
                "away from home",
                s.auto,
                vec!["set", "auto", if s.auto { "off" } else { "on" }],
            );
            if let Some(ss) = &s.ssid {
                setting(
                    if s.trusted { "\u{f16bd}" } else { "\u{f05a9}" },
                    &format!("trust {}", ss.to_lowercase()),
                    "",
                    s.trusted,
                    vec![if s.trusted { "untrust" } else { "trust" }],
                );
            }
            body.append(&ui::section("servers"));
            for sv in &s.servers {
                let on = s.active.as_deref() == Some(sv);
                let name = pretty(sv);
                let mut meta = country(&name).to_string();
                if !on && s.active.is_none() && s.last.as_deref() == Some(sv) {
                    meta = "last used".into();
                }
                let r = ui::Row::new(if on { ON } else { OFF }, &name, &meta);
                r.set_active(on);
                if let Some(b) = busy.get(sv) {
                    r.meta.set_text("");
                    r.trail.append(&ui::spinner());
                    r.trail.append(&ui::text(b, &["meta"]));
                }
                let (i2, sv2) = (Rc::downgrade(&i), sv.clone());
                ui::on_click(&r.root, move || {
                    if let Some(i) = i2.upgrade() {
                        if on {
                            i.act(&sv2, &["down"], "disconnecting…");
                        } else {
                            i.act(&sv2, &["up", &sv2], "connecting…");
                        }
                    }
                });
                body.append(&r.root);
                if let Some(e) = errors.get(sv) {
                    body.append(&ui::text(e, &["msg", "crit"]));
                }
            }
            let _ = SERVER;
        }
    };
    let fill = Rc::new(fill);
    fill();
    let (f2, h2) = (fill.clone(), hovered.clone());
    p.watch(&i.changed, move || {
        if !h2.get() {
            f2()
        }
    });
    // apply what changed while the pointer was on the list
    let (f3, h3) = (fill.clone(), hovered.clone());
    let m = gtk::EventControllerMotion::new();
    m.connect_leave(move |_| {
        if !h3.get() {
            f3()
        }
    });
    card.root.add_controller(m);
    let pc = p.clone();
    card.foot_add(&ui::link("more servers", &[], move || {
        pc.close();
        util::spawn(&["xdg-open", "https://account.protonvpn.com/downloads"]);
    }));
    card.root.upcast()
}
