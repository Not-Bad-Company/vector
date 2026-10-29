//! Headphones with a local control API: Nothing earbuds (earctl, `earbuds`)
//! and Sony headphones (sonyctl, `sony`). Hidden unless the device is
//! connected (found by its vendor service UUID over BlueZ, not its name).
//! Bar: noise-control icon + battery. Click: next noise mode. Right: panel
//! (batteries, noise control, ambient, EQ, volume, settings, find, reconnect).
//! Middle: earbuds low latency / Sony ambient level.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::prelude::*;
use serde_json::{Value, json};

use super::menu::confirm_button;
use super::{BarInfo, Ctx, Module};
use crate::dbus;
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::ui::{self, Tap};
use crate::util;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Earbuds,
    Sony,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Earbuds => "earbuds",
            Kind::Sony => "sony",
        }
    }
    fn uuid(self) -> &'static str {
        match self {
            Kind::Earbuds => "aeac4a03-dff5-498f-843a-34487cf133eb",
            Kind::Sony => "956c7b26-d49a-4ba8-b03f-b17d393cb6e2",
        }
    }
    fn endpoint(self) -> &'static str {
        match self {
            Kind::Earbuds => "http://127.0.0.1:8787",
            Kind::Sony => "http://127.0.0.1:8788",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Kind::Earbuds => "nothing earbuds",
            Kind::Sony => "sony headphones",
        }
    }
}

fn get(ep: &str, path: &str) -> Option<Value> {
    let mut r = ureq::get(&format!("{ep}{path}"))
        .config()
        .timeout_global(Some(Duration::from_secs(4)))
        .http_status_as_error(false)
        .build()
        .call()
        .ok()?;
    if r.status() != 200 {
        return None;
    }
    r.body_mut().read_json().ok()
}

fn post(ep: &str, path: &str, body: &Value) -> bool {
    ureq::post(&format!("{ep}{path}"))
        .config()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .send_json(body)
        .is_ok_and(|r| r.status() == 200)
}

fn delete(ep: &str, path: &str) {
    let _ = ureq::delete(&format!("{ep}{path}"))
        .config()
        .timeout_global(Some(Duration::from_secs(4)))
        .build()
        .call();
}

#[derive(Clone, Default, PartialEq)]
struct State {
    address: Option<String>,
    server: bool,
    status: Option<Value>,
}

struct Inner {
    kind: Kind,
    slot: Slot,
    conn: Option<gio::DBusConnection>,
    state: RefCell<State>,
    changed: Notify,
    busy: Cell<bool>,
    last_connect: Cell<f64>,
    last_nc: RefCell<String>,
    low_sent: RefCell<Vec<String>>,
    _sub: RefCell<Option<dbus::Sub>>,
}

pub struct Headphones(Rc<Inner>);

impl Headphones {
    pub fn earbuds(ctx: &Ctx) -> Headphones {
        Self::new(ctx, Kind::Earbuds)
    }
    pub fn sony(ctx: &Ctx) -> Headphones {
        Self::new(ctx, Kind::Sony)
    }

    fn new(ctx: &Ctx, kind: Kind) -> Headphones {
        let inner = Rc::new(Inner {
            kind,
            slot: ctx.slot(kind.name(), &[]),
            conn: dbus::system(),
            state: RefCell::default(),
            changed: Notify::default(),
            busy: Cell::new(false),
            last_connect: Cell::new(0.0),
            last_nc: RefCell::new(if kind == Kind::Earbuds {
                "noise_cancellation_adaptive".into()
            } else {
                "noise_cancelling".into()
            }),
            low_sent: RefCell::default(),
            _sub: RefCell::default(),
        });
        inner.slot.set_visible(false);
        let w = Rc::downgrade(&inner);
        inner.slot.on_left(move || {
            if let Some(i) = w.upgrade() {
                i.cycle_noise();
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.middle();
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_right(move || {
            if let Some(i) = w.upgrade() {
                i.slot.toggle_panel(None, false);
            }
        });
        let w = Rc::downgrade(&inner);
        inner
            .slot
            .tooltip(move || w.upgrade().and_then(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        // a device connecting shows up at once
        if let Some(c) = &inner.conn {
            let w = Rc::downgrade(&inner);
            *inner._sub.borrow_mut() =
                Some(dbus::subscribe(c, "org.bluez", move |_, member, _| {
                    if (member == "PropertiesChanged" || member == "InterfacesAdded")
                        && let Some(i) = w.upgrade()
                    {
                        util::after(1200, &i, |i| i.poll());
                    }
                }));
        }
        inner.poll();
        util::every(15_000, &inner, |i| i.poll());
        Headphones(inner)
    }
}

impl Inner {
    fn poll(self: &Rc<Self>) {
        if self.busy.replace(true) {
            return;
        }
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        let uuid = self.kind.uuid();
        glib::spawn_future_local(async move {
            let objs = dbus::managed(&c, "org.bluez", "/").await;
            let addr = objs
                .values()
                .filter_map(|i| i.get("org.bluez.Device1"))
                .find(|d| {
                    dbus::get_bool(d, "Connected")
                        && d.get("UUIDs")
                            .and_then(|u| u.get::<Vec<String>>())
                            .is_some_and(|u| u.iter().any(|x| x.eq_ignore_ascii_case(uuid)))
                });
            let addr = addr.map(|d| dbus::get_str(d, "Address"));
            let Some(i) = w.upgrade() else { return };
            let Some(addr) = addr else {
                i.busy.set(false);
                *i.state.borrow_mut() = State::default();
                i.render();
                i.changed.emit();
                return;
            };
            let ep = i.kind.endpoint();
            let try_connect = util::now() - i.last_connect.get() > 20.0;
            if try_connect {
                i.last_connect.set(util::now());
            }
            let w2 = Rc::downgrade(&i);
            let a2 = addr.clone();
            util::bg(
                move || {
                    let server = ureq::get(&format!("{ep}/api/session"))
                        .config()
                        .timeout_global(Some(Duration::from_secs(2)))
                        .http_status_as_error(false)
                        .build()
                        .call()
                        .is_ok();
                    if !server {
                        return (false, None);
                    }
                    let mut st = get(ep, "/api/status");
                    if st.is_none() && try_connect {
                        post(ep, "/api/session/auto-connect", &json!({"address": a2}));
                        st = get(ep, "/api/status");
                    }
                    (true, st)
                },
                move |(server, status)| {
                    let Some(i) = w2.upgrade() else { return };
                    i.busy.set(false);
                    let s = State {
                        address: Some(addr),
                        server,
                        status,
                    };
                    i.check_low(&s);
                    let changed = *i.state.borrow() != s;
                    *i.state.borrow_mut() = s;
                    i.render();
                    if changed {
                        i.changed.emit();
                    }
                },
            );
        });
    }

    fn noise(&self, st: &Value) -> String {
        match self.kind {
            Kind::Earbuds => st["anc"].as_str().unwrap_or("").to_string(),
            Kind::Sony => st["noise_control"]["mode"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        }
    }

    /// (lowest battery, charging)
    fn battery(&self, st: &Value) -> (Option<i64>, bool) {
        match self.kind {
            Kind::Earbuds => {
                let lv: Vec<&Value> = ["left", "right"]
                    .iter()
                    .map(|s| &st["battery"][s]["Level"])
                    .filter(|v| v.is_object())
                    .collect();
                (
                    lv.iter().filter_map(|v| v["percent"].as_i64()).min(),
                    lv.iter().any(|v| v["charging"] == true),
                )
            }
            Kind::Sony => (
                st["battery"]["percent"].as_i64(),
                st["battery"]["charging"] == true,
            ),
        }
    }

    fn render(&self) {
        let s = self.state.borrow();
        if s.address.is_none() {
            self.slot.set_visible(false);
            return;
        }
        self.slot.set_visible(true);
        let Some(st) = s.status.as_ref().filter(|_| s.server) else {
            self.slot.set_markup(if self.kind == Kind::Earbuds {
                "󱡑"
            } else {
                "󰋍"
            });
            self.slot.set_classes(&["dim"]);
            self.slot.bump();
            return;
        };
        let n = self.noise(st);
        if n.starts_with("noise_cancel") {
            *self.last_nc.borrow_mut() = n.clone();
        }
        let icon = match (self.kind, n.as_str()) {
            (Kind::Earbuds, "off") => "󱡒",
            (Kind::Earbuds, "transparency") => "󰟅",
            (Kind::Earbuds, _) => "󱡏",
            (Kind::Sony, "off") => "󰟎",
            (Kind::Sony, "ambient") => "󰋎",
            (Kind::Sony, _) => "󰋋",
        };
        let (lvl, chg) = self.battery(st);
        let mut t = icon.to_string();
        if let Some(l) = lvl {
            t += &format!(" {l}%");
        }
        if chg {
            t += " \u{f140b}";
        }
        self.slot.set_markup(&t);
        let cls = if n == "off" {
            "dim"
        } else if lvl.is_some_and(|l| l <= 10) && !chg {
            "crit"
        } else if lvl.is_some_and(|l| l <= 20) && !chg {
            "warn"
        } else {
            ""
        };
        self.slot.set_classes(&[cls]);
        self.slot.bump();
    }

    fn check_low(&self, s: &State) {
        let Some(st) = &s.status else { return };
        let sides: Vec<(String, Option<i64>, bool)> = match self.kind {
            Kind::Earbuds => ["left", "right"]
                .iter()
                .map(|side| {
                    (
                        side.to_string(),
                        st["battery"][side]["Level"]["percent"].as_i64(),
                        st["battery"][side]["Level"]["charging"] == true,
                    )
                })
                .collect(),
            Kind::Sony => vec![(
                "battery".into(),
                st["battery"]["percent"].as_i64(),
                st["battery"]["charging"] == true,
            )],
        };
        let mut sent = self.low_sent.borrow_mut();
        for (side, pct, chg) in sides {
            let Some(p) = pct else { continue };
            if chg || p > 20 {
                sent.retain(|x| x != &side);
            } else if !sent.contains(&side) {
                sent.push(side.clone());
                let body = if self.kind == Kind::Earbuds {
                    format!("{} bud at {p}%", side)
                } else {
                    format!("{p}% left")
                };
                util::spawn(&[
                    "notify-send",
                    "-u",
                    "normal",
                    "-a",
                    self.kind.name(),
                    "-i",
                    "battery-low",
                    &format!("{} battery low", self.kind.title()),
                    &body,
                ]);
            }
        }
    }

    fn send(self: &Rc<Self>, path: &'static str, body: Value) {
        let ep = self.kind.endpoint();
        let w = Rc::downgrade(self);
        util::bg(
            move || post(ep, path, &body),
            move |_| {
                if let Some(i) = w.upgrade() {
                    i.busy.set(false);
                    i.poll();
                }
            },
        );
    }

    fn cycle_noise(self: &Rc<Self>) {
        let Some(st) = self.state.borrow().status.clone() else {
            return;
        };
        let cur = self.noise(&st);
        match self.kind {
            Kind::Earbuds => {
                let next = if cur.starts_with("noise_cancel") {
                    "transparency".to_string()
                } else if cur == "transparency" {
                    "off".into()
                } else {
                    self.last_nc.borrow().clone()
                };
                self.send("/api/anc", json!({"level": next}));
            }
            Kind::Sony => {
                let next = match cur.as_str() {
                    "noise_cancelling" => "ambient",
                    "ambient" => "off",
                    _ => "noise_cancelling",
                };
                self.send("/api/noise-control", json!({"mode": next}));
            }
        }
    }

    fn middle(self: &Rc<Self>) {
        let Some(st) = self.state.borrow().status.clone() else {
            return;
        };
        match self.kind {
            Kind::Earbuds => self.send(
                "/api/latency",
                json!({"low_latency_enabled": st["low_latency"] != true}),
            ),
            Kind::Sony => {
                let cur = st["noise_control"]["ambient_level"].as_i64().unwrap_or(0);
                let next = [1, 5, 10, 15, 20]
                    .into_iter()
                    .find(|l| *l > cur)
                    .unwrap_or(1);
                self.send(
                    "/api/noise-control",
                    json!({"mode": "ambient", "ambient_level": next}),
                );
            }
        }
    }

    fn tooltip(&self) -> Option<gtk::Widget> {
        let s = self.state.borrow();
        s.address.as_ref()?;
        let sh = ui::sheet();
        let Some(st) = s.status.as_ref().filter(|_| s.server) else {
            sh.append(
                &ui::head(
                    self.kind.title(),
                    if s.server {
                        "waiting for the session…"
                    } else {
                        "control server is not running"
                    },
                )
                .0,
            );
            return Some(sh.upcast());
        };
        let name = match self.kind {
            Kind::Earbuds => st["session"]["model"]["name"].as_str(),
            Kind::Sony => st["session"]["name"].as_str(),
        }
        .unwrap_or(self.kind.title())
        .to_lowercase();
        sh.append(&ui::head(&name, &noise_label(&self.noise(st))).0);
        sh.append(&ui::tsection("battery"));
        for (k, v, c) in batteries(self.kind, st) {
            let row = ui::hbox(8, &[]);
            row.append(&ui::text(&k, &["dim"]).tap(|l| l.set_width_chars(7)));
            let cls = if v <= 10 {
                "crit"
            } else if v <= 20 {
                "warn"
            } else {
                ""
            };
            let m = ui::meter(v as f64 / 100.0, cls);
            m.set_size_request(70, -1);
            m.set_hexpand(false);
            row.append(&m);
            row.append(&ui::text(
                &format!("{v}%{}", if c { "  charging" } else { "" }),
                &[cls],
            ));
            sh.append(&row);
        }
        sh.append(&ui::thint(match self.kind {
            Kind::Earbuds => "click noise mode  ·  middle low latency  ·  right controls",
            Kind::Sony => "click noise mode  ·  middle ambient level  ·  right controls",
        }));
        Some(sh.upcast())
    }
}

fn noise_label(n: &str) -> String {
    match n {
        "off" => "noise control off".into(),
        "transparency" => "transparency".into(),
        "ambient" => "ambient sound".into(),
        "noise_cancelling" => "noise cancelling".into(),
        x if x.starts_with("noise_cancellation_") => format!(
            "noise cancelling · {}",
            x.trim_start_matches("noise_cancellation_")
        ),
        x => x.replace('_', " "),
    }
}

/// (label, percent, charging) per battery the device reports.
fn batteries(k: Kind, st: &Value) -> Vec<(String, i64, bool)> {
    match k {
        Kind::Earbuds => ["left", "right", "case"]
            .iter()
            .filter_map(|s| {
                let l = &st["battery"][s]["Level"];
                Some((s.to_string(), l["percent"].as_i64()?, l["charging"] == true))
            })
            .collect(),
        Kind::Sony => st["battery"]["percent"]
            .as_i64()
            .map(|p| vec![("battery".to_string(), p, st["battery"]["charging"] == true)])
            .unwrap_or_default(),
    }
}

impl Module for Headphones {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.poll();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(380);
    let card = Card::new(i.kind.name(), "");
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
        let (w, body, detail, pc) = (
            Rc::downgrade(i),
            body.clone(),
            card.detail.clone(),
            p.clone(),
        );
        move || {
            let Some(i) = w.upgrade() else { return };
            ui::clear(&body);
            let s = i.state.borrow().clone();
            let Some(st) = s.status.clone().filter(|_| s.server) else {
                detail.set_markup(if s.server {
                    "waiting for the session…"
                } else {
                    "control server is not running"
                });
                if !s.server {
                    body.append(&ui::text(
                        &format!(
                            "systemctl --user enable --now {}",
                            if i.kind == Kind::Earbuds {
                                "earctl"
                            } else {
                                "sonyctl"
                            }
                        ),
                        &["empty-note"],
                    ));
                }
                return;
            };
            let name = match i.kind {
                Kind::Earbuds => st["session"]["model"]["name"].as_str(),
                Kind::Sony => st["session"]["name"].as_str(),
            }
            .unwrap_or(i.kind.title())
            .to_lowercase();
            detail.set_markup(&util::esc(&name));
            body.append(&ui::section("battery").tap(|l| l.add_css_class("first")));
            for (k, v, c) in batteries(i.kind, &st) {
                let cls = if v <= 10 {
                    "crit"
                } else if v <= 20 {
                    "warn"
                } else {
                    ""
                };
                body.append(&ui::meter_row(
                    &k,
                    v as f64 / 100.0,
                    &format!("{v}%"),
                    if c { "charging" } else { "" },
                    cls,
                ));
            }

            body.append(&ui::section("noise control"));
            let cur = i.noise(&st);
            let opts: Vec<(&str, &str)> = match i.kind {
                Kind::Earbuds => vec![
                    ("noise_cancellation_adaptive", "adaptive"),
                    ("noise_cancellation_high", "high"),
                    ("noise_cancellation_mid", "mid"),
                    ("noise_cancellation_low", "low"),
                    ("transparency", "transp"),
                    ("off", "off"),
                ],
                Kind::Sony => vec![
                    ("noise_cancelling", "cancel"),
                    ("ambient", "ambient"),
                    ("off", "off"),
                ],
            };
            let w2 = Rc::downgrade(&i);
            let kind = i.kind;
            body.append(
                &ui::segmented(&opts, &cur, move |v| {
                    if let Some(i) = w2.upgrade() {
                        match kind {
                            Kind::Earbuds => i.send("/api/anc", json!({"level": v})),
                            Kind::Sony => i.send("/api/noise-control", json!({"mode": v})),
                        }
                    }
                })
                .tap(|b| b.add_css_class("chips")),
            );
            if i.kind == Kind::Sony && cur == "ambient" {
                let lvl = st["noise_control"]["ambient_level"]
                    .as_f64()
                    .unwrap_or(10.0);
                let row = ui::hbox(8, &["kv"]);
                row.append(&ui::text("level", &["k"]));
                let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 20.0, 1.0);
                sc.set_value(lvl);
                sc.set_hexpand(true);
                sc.set_draw_value(true);
                let w3 = Rc::downgrade(&i);
                sc.connect_change_value(move |_, _, v| {
                    if let Some(i) = w3.upgrade() {
                        i.send(
                            "/api/noise-control",
                            json!({"mode": "ambient", "ambient_level": v.round() as i64}),
                        );
                    }
                    glib::Propagation::Proceed
                });
                row.append(&sc);
                body.append(&row);
                let voice = st["noise_control"]["focus_on_voice"] == true;
                let r = ui::Row::new("\u{f0362}", "focus on voice", "");
                let w3 = Rc::downgrade(&i);
                r.trail.append(&ui::switch(voice, move |on| {
                    if let Some(i) = w3.upgrade() {
                        i.send(
                            "/api/noise-control",
                            json!({"mode": "ambient", "focus_on_voice": on}),
                        );
                    }
                }));
                body.append(&r.root);
            }

            body.append(&ui::section("sound"));
            match i.kind {
                Kind::Earbuds => {
                    if let Some(eq) = st["eq"].as_i64() {
                        let w3 = Rc::downgrade(&i);
                        let seg = ui::segmented(
                            &[
                                ("0", "balanced"),
                                ("1", "voice"),
                                ("2", "treble"),
                                ("3", "bass"),
                                ("5", "custom"),
                            ],
                            &eq.to_string(),
                            move |v| {
                                if let (Some(i), Ok(m)) = (w3.upgrade(), v.parse::<i64>()) {
                                    i.send("/api/eq", json!({"mode": m}));
                                }
                            },
                        );
                        body.append(&seg.tap(|b| b.add_css_class("chips")));
                    }
                    if st["enhanced_bass"].is_object() {
                        let lvl = if st["enhanced_bass"]["enabled"] == true {
                            st["enhanced_bass"]["level"].as_i64().unwrap_or(0)
                        } else {
                            0
                        };
                        let row = ui::hbox(0, &["kv"]);
                        row.append(&ui::text("bass", &["k"]));
                        let w3 = Rc::downgrade(&i);
                        row.append(&ui::segmented(
                            &[
                                ("0", "off"),
                                ("1", "1"),
                                ("2", "2"),
                                ("3", "3"),
                                ("4", "4"),
                                ("5", "5"),
                            ],
                            &lvl.to_string(),
                            move |v| {
                                if let (Some(i), Ok(l)) = (w3.upgrade(), v.parse::<i64>()) {
                                    i.send(
                                        "/api/enhanced-bass",
                                        json!({"enabled": l > 0, "level": l}),
                                    );
                                }
                            },
                        ));
                        body.append(&row);
                    }
                }
                Kind::Sony => {
                    if let Some(eq) = st["equalizer"]["preset"].as_str() {
                        let flow = gtk::FlowBox::new();
                        flow.set_selection_mode(gtk::SelectionMode::None);
                        flow.set_max_children_per_line(6);
                        flow.set_margin_start(16);
                        flow.set_margin_end(16);
                        for (k, l) in [
                            ("off", "off"),
                            ("bright", "bright"),
                            ("excited", "excited"),
                            ("mellow", "mellow"),
                            ("relaxed", "relaxed"),
                            ("vocal", "vocal"),
                            ("treble", "treble"),
                            ("bass", "bass"),
                            ("speech", "speech"),
                            ("custom", "custom"),
                        ] {
                            let w3 = Rc::downgrade(&i);
                            flow.insert(
                                &ui::chip(l, k == eq, move || {
                                    if let Some(i) = w3.upgrade() {
                                        i.send("/api/eq", json!({"preset": k}));
                                    }
                                }),
                                -1,
                            );
                        }
                        body.append(&flow);
                    }
                    if let Some(v) = st["volume"]["level"].as_f64() {
                        let row = ui::hbox(8, &["kv"]);
                        row.append(&ui::text("volume", &["k"]));
                        let sc =
                            gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 30.0, 1.0);
                        sc.set_value(v);
                        sc.set_hexpand(true);
                        sc.set_draw_value(true);
                        let w3 = Rc::downgrade(&i);
                        sc.connect_change_value(move |_, _, v| {
                            if let Some(i) = w3.upgrade() {
                                i.send("/api/volume", json!({"level": v.round() as i64}));
                            }
                            glib::Propagation::Proceed
                        });
                        row.append(&sc);
                        body.append(&row);
                    }
                }
            }

            body.append(&ui::section("settings"));
            let toggles: Vec<(&str, &str, &'static str, &'static str, bool)> = match i.kind {
                Kind::Earbuds => vec![
                    (
                        "\u{f04c5}",
                        "low latency",
                        "/api/latency",
                        "low_latency_enabled",
                        st["low_latency"] == true,
                    ),
                    (
                        "\u{f07c5}",
                        "in-ear detection",
                        "/api/in-ear",
                        "detection_enabled",
                        st["in_ear_detection"] == true,
                    ),
                ],
                Kind::Sony => vec![
                    (
                        "\u{f0d4b}",
                        "dsee extreme",
                        "/api/dsee",
                        "enabled",
                        st["dsee"]["enabled"] == true,
                    ),
                    (
                        "\u{f0b79}",
                        "voice prompts",
                        "/api/voice-guidance",
                        "enabled",
                        st["voice_guidance"]["enabled"] == true,
                    ),
                ],
            };
            for (icon, name, path, field, on) in toggles {
                let r = ui::Row::new(icon, name, "");
                let w3 = Rc::downgrade(&i);
                r.trail.append(&ui::switch(on, move |v| {
                    if let Some(i) = w3.upgrade() {
                        let mut m = serde_json::Map::new();
                        m.insert(field.to_string(), Value::Bool(v));
                        i.send(path, Value::Object(m));
                    }
                }));
                body.append(&r.root);
            }
            if i.kind == Kind::Earbuds {
                for (k, l) in [("personalized_anc", "anc fit"), ("super_mic", "super mic")] {
                    if let Some(v) = st[k].as_bool() {
                        body.append(&ui::kv(l, if v { "on" } else { "off" }).0);
                    }
                }
                let row = ui::hbox(4, &["chips"]);
                row.append(&ui::text("find", &["dim"]).tap(|l| l.set_margin_end(8)));
                for (side, glyph) in [("left", "ring left"), ("right", "ring right")] {
                    let w3 = Rc::downgrade(&i);
                    let b = confirm_button(
                        glyph,
                        "plays a loud tone: take the buds out first",
                        move || {
                            if let Some(i) = w3.upgrade() {
                                i.send("/api/ring", json!({"enable": true, "side": side}));
                            }
                        },
                    );
                    b.add_css_class("chip");
                    row.append(&b);
                }
                let w3 = Rc::downgrade(&i);
                row.append(&ui::chip("stop", false, move || {
                    if let Some(i) = w3.upgrade() {
                        i.send("/api/ring", json!({"enable": false, "side": "left"}));
                        i.send("/api/ring", json!({"enable": false, "side": "right"}));
                    }
                }));
                body.append(&row);
            }
            let _ = &pc;
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
    let w = Rc::downgrade(i);
    card.foot_add(&ui::link("reconnect", &[], move || {
        if let Some(i) = w.upgrade() {
            let ep = i.kind.endpoint();
            i.last_connect.set(0.0);
            let w2 = Rc::downgrade(&i);
            util::bg(
                move || delete(ep, "/api/session"),
                move |_| {
                    if let Some(i) = w2.upgrade() {
                        i.poll();
                    }
                },
            );
        }
    }));
    card.root.upcast()
}
