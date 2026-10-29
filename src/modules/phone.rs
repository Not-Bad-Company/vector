//! Phone via the phonectl daemon (its Unix socket; `subscribe` streams every
//! change, nothing polls). Hidden until paired. Bar: link icon + battery.
//! Click: panel (battery, network, media, find my phone, ringer, clipboard).
//! Right: send clipboard. Middle: reconnect.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::{Value, json};

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const I_PHONE: &str = "\u{f011c}";
const I_BT: &str = "\u{f0815}";
const I_OFF: &str = "\u{f0950}";
const I_CALL: &str = "\u{f0952}";
const I_BOLT: &str = "\u{f140b}";

fn sock() -> PathBuf {
    util::runtime_dir().join("phonectl/daemon.sock")
}

/// One request on the daemon socket (blocking: call from a worker thread).
pub fn request(req: &Value, timeout: Duration) -> Option<Value> {
    let mut s = UnixStream::connect(sock()).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.write_all(format!("{req}\n").as_bytes()).ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn bars(level: Option<i64>) -> String {
    let Some(l) = level else { return String::new() };
    let g = ["▂", "▄", "▆", "█"];
    let n = l.clamp(0, 4) as usize;
    format!("{}{}", g[..n].concat(), if n < 4 { ui::dim(&g[n..].concat()) } else { String::new() })
}

fn since(ts: i64) -> String {
    let d = (util::now() as i64 - ts).max(0);
    match d {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", d / 60),
        3600..86400 => format!("{} h ago", d / 3600),
        _ => format!("{} days ago", d / 86400),
    }
}

struct Inner {
    slot: Slot,
    data: RefCell<Option<Value>>,
    changed: Notify,
    theme: Rc<Theme>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub struct Phone(Rc<Inner>);

impl Phone {
    pub fn new(ctx: &Ctx) -> Phone {
        let inner = Rc::new(Inner {
            slot: ctx.slot("phone", &[]),
            data: RefCell::default(),
            changed: Notify::default(),
            theme: ctx.theme.clone(),
            stop: Default::default(),
        });
        inner.slot.set_visible(false);
        inner.slot.on_right(|| util::spawn(&["phonectl", "clip"]));
        inner.slot.on_middle(|| util::spawn(&["phonectl", "connect"]));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().and_then(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });

        // subscribe on a thread; None = daemon gone
        let (tx, rx) = async_channel::unbounded::<Option<Value>>();
        let stop = inner.stop.clone();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            let mut delay = Duration::from_secs(2);
            while !stop.load(Ordering::SeqCst) {
                if let Ok(mut s) = UnixStream::connect(sock()) {
                    delay = Duration::from_secs(2);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    if s.write_all(b"{\"method\":\"subscribe\"}\n").is_ok() {
                        let mut r = BufReader::new(s);
                        let mut line = String::new();
                        loop {
                            if stop.load(Ordering::SeqCst) {
                                return;
                            }
                            line.clear();
                            match r.read_line(&mut line) {
                                Ok(0) => break,
                                Ok(_) => {
                                    if let Ok(v) = serde_json::from_str::<Value>(&line) {
                                        if v["type"] == "status" && tx.send_blocking(Some(v["data"].clone())).is_err() {
                                            return;
                                        }
                                    }
                                }
                                // read timeout: just check `stop` again
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                                Err(_) => break,
                            }
                        }
                    }
                }
                if tx.send_blocking(None).is_err() {
                    return;
                }
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        });
        let w = Rc::downgrade(&inner);
        glib::spawn_future_local(async move {
            while let Ok(v) = rx.recv().await {
                let Some(i) = w.upgrade() else { return };
                *i.data.borrow_mut() = v;
                i.render();
                i.changed.emit();
            }
        });
        Phone(inner)
    }
}

impl Inner {
    fn render(&self) {
        let d = self.data.borrow();
        let Some(s) = d.as_ref() else {
            // daemon not running: show the off icon only if it was ever paired (socket exists)
            self.slot.set_markup(I_OFF);
            self.slot.set_classes(&["dim"]);
            self.slot.set_visible(sock().exists());
            return;
        };
        let state = s["state"].as_str().unwrap_or("disconnected");
        if state == "unpaired" {
            self.slot.set_visible(false);
            return;
        }
        let st = &s["status"];
        let level = st["battery"]["level"].as_i64();
        let charging = st["battery"]["charging"].as_bool().unwrap_or(false);
        let connected = state == "connected";
        let bt = s["transport"].as_str() == Some("bluetooth");
        let icon = if !connected {
            I_OFF
        } else if st["call"].as_str().unwrap_or("idle") != "idle" {
            I_CALL
        } else if bt {
            I_BT
        } else {
            I_PHONE
        };
        let mut text = icon.to_string();
        if let (true, Some(l)) = (connected, level) {
            text += &format!(" {l}%");
            if charging {
                text += &format!(" {I_BOLT}");
            }
        }
        self.slot.set_markup(&text);
        let cls = if !connected {
            "dim"
        } else if level.is_some_and(|l| l <= 10) && !charging {
            "crit"
        } else if level.is_some_and(|l| l <= 20) && !charging {
            "warn"
        } else {
            ""
        };
        self.slot.set_classes(&[cls]);
        self.slot.set_visible(true);
        self.slot.bump();
    }

    fn tooltip(&self) -> Option<gtk::Widget> {
        let d = self.data.borrow();
        let s = d.as_ref()?;
        let sh = ui::sheet();
        let state = s["state"].as_str().unwrap_or("");
        let detail = match state {
            "connected" => s["transport"].as_str().unwrap_or("connected").to_string(),
            "connecting" => "connecting…".into(),
            "suspended" => "laptop suspended".into(),
            _ => "offline".into(),
        };
        sh.append(&ui::head(&s["phone"]["name"].as_str().unwrap_or("phone").to_lowercase(), &detail).0);
        let st = &s["status"];
        if !st.is_null() {
            sh.append(&ui::tsection(if state == "connected" { "status" } else { "last known" }));
            if let Some(l) = st["battery"]["level"].as_i64() {
                sh.append(&ui::tkv("battery", &self.batt(l, st)));
            }
            let net = &st["network"];
            let n = match net["type"].as_str() {
                Some("wifi") => format!("wi-fi {}", bars(net["wifi_level"].as_i64())),
                Some("cellular") => format!("{} {}", net["cellular"].as_str().unwrap_or("mobile"), bars(net["cellular_level"].as_i64())),
                _ => ui::dim("no network"),
            };
            sh.append(&ui::tkv("network", &n));
            if let Some(n) = s["notifications"].as_i64() {
                sh.append(&ui::tkv("notifs", &format!("{n} on the phone")));
            }
        }
        sh.append(&ui::thint("click phone  ·  right send clipboard  ·  middle reconnect"));
        Some(sh.upcast())
    }

    fn batt(&self, l: i64, st: &Value) -> String {
        let pct = format!("{l}%");
        let pct = if l <= 10 { ui::colored(&pct, &self.theme.color1) } else if l <= 20 { ui::colored(&pct, &self.theme.color3) } else { pct };
        let how = if st["battery"]["full"].as_bool() == Some(true) {
            ui::dim(" · full")
        } else if st["battery"]["charging"].as_bool() == Some(true) {
            ui::dim(&format!(" · charging{}", st["battery"]["plugged"].as_str().map(|p| format!(" ({p})")).unwrap_or_default()))
        } else {
            String::new()
        };
        pct + &how
    }
}

impl Module for Phone {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(340);
    let card = Card::new("phone", "");
    let body = ui::vbox(0, &[]);
    card.body.append(&body);
    let busy: Rc<RefCell<HashMap<String, String>>> = Rc::default();
    let errors: Rc<RefCell<HashMap<String, String>>> = Rc::default();

    let draw: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::default();
    let redraw: Rc<dyn Fn()> = {
        let d = draw.clone();
        Rc::new(move || {
            let f = d.borrow().clone();
            if let Some(f) = f {
                f();
            }
        })
    };
    // phone calls run on a thread: the phone may take a moment to answer
    let call: Rc<dyn Fn(&str, &str, Value)> = {
        let (busy, errors, rr) = (busy.clone(), errors.clone(), redraw.clone());
        Rc::new(move |key: &str, method: &str, params: Value| {
            busy.borrow_mut().insert(key.into(), "…".into());
            errors.borrow_mut().remove(key);
            rr();
            let req = json!({"method": "call", "params": {"method": method, "params": params}});
            let (busy, errors, rr, key) = (busy.clone(), errors.clone(), rr.clone(), key.to_string());
            util::bg(
                move || request(&req, Duration::from_secs(12)),
                move |r| {
                    busy.borrow_mut().remove(&key);
                    let ok = r.as_ref().is_some_and(|v| v["ok"] == true);
                    if !ok {
                        let e = r.as_ref().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_else(|| "phonectl daemon is not running".into());
                        errors.borrow_mut().insert(key, e);
                    }
                    rr();
                },
            );
        })
    };

    let render = {
        let (i, body, detail, busy, errors, call, pc) = (Rc::downgrade(i), body.clone(), card.detail.clone(), busy.clone(), errors.clone(), call.clone(), p.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            ui::clear(&body);
            let d = i.data.borrow().clone();
            let Some(s) = d else {
                detail.set_markup("phonectl is not running");
                body.append(&ui::text("systemctl --user start phonectl", &["empty-note"]));
                return;
            };
            let state = s["state"].as_str().unwrap_or("disconnected");
            let st = &s["status"];
            let connected = state == "connected";
            let name = s["phone"]["name"].as_str().unwrap_or("phone").to_lowercase();
            detail.set_markup(&format!(
                "{} · {}",
                util::esc(&name),
                if connected { s["transport"].as_str().unwrap_or("connected").to_string() } else { match state { "connecting" => "connecting…".into(), "suspended" => "laptop suspended".into(), "unpaired" => "not paired".into(), _ => "offline".into() } }
            ));
            if state == "unpaired" {
                body.append(&ui::text("run: phonectl setup", &["empty-note"]));
                return;
            }
            let title = if connected {
                "status".to_string()
            } else if let Some(ls) = s["last_seen"].as_i64() {
                format!("last known · {}", since(ls))
            } else {
                "status".into()
            };
            body.append(&ui::section(&title).tap(|l| l.add_css_class("first")));
            if let Some(l) = st["battery"]["level"].as_i64() {
                let cls = if l <= 10 { "crit" } else if l <= 20 { "warn" } else { "" };
                let charging = st["battery"]["charging"].as_bool() == Some(true);
                let note = if st["battery"]["full"].as_bool() == Some(true) { "full".to_string() } else if charging { "charging".into() } else { String::new() };
                body.append(&ui::meter_row("battery", l as f64 / 100.0, &format!("{l}%"), &note, cls));
            }
            let net = &st["network"];
            if net["type"] == "wifi" {
                body.append(&ui::kv("wi-fi", &bars(net["wifi_level"].as_i64())).0);
            }
            if net["cellular"].is_string() || net["cellular_level"].is_i64() {
                body.append(&ui::kv("mobile", &format!("{} {}  {}", net["cellular"].as_str().unwrap_or(""), bars(net["cellular_level"].as_i64()), ui::dim(&util::esc(net["operator"].as_str().unwrap_or(""))))).0);
            }
            if let Some(n) = s["notifications"].as_i64() {
                body.append(&ui::kv("notifs", &format!("{n} {}", ui::dim("on the phone"))).0);
            }
            if st["call"].as_str().unwrap_or("idle") != "idle" {
                body.append(&ui::kv("call", if st["call"] == "ringing" { "ringing" } else { "in a call · laptop media paused" }).0);
            }

            if connected && st["media"].is_object() {
                let m = &st["media"];
                body.append(&ui::section(&format!("playing on the phone · {}", m["app"].as_str().unwrap_or("").to_lowercase())));
                let mut t = m["title"].as_str().unwrap_or("").to_string();
                if let Some(a) = m["artist"].as_str() {
                    t += &format!(" · {a}");
                }
                let row = ui::Row::new("\u{f075a}", &t, "");
                row.set_active(m["playing"] == true);
                for (g, act) in [("\u{f04ae}", "previous"), (if m["playing"] == true { "\u{f03e4}" } else { "\u{f040a}" }, "play_pause"), ("\u{f04ad}", "next")] {
                    let c = call.clone();
                    row.trail.append(&ui::icon_button(g, act, move || c("media", "media_action", json!({"action": act}))));
                }
                body.append(&row.root);
            }

            if connected {
                body.append(&ui::section("phone"));
                let ringing = st["ringing"] == true;
                let r = ui::Row::new("\u{f009e}", if ringing { "stop ringing" } else { "find my phone" }, if ringing { "" } else { "rings even on silent" });
                r.set_active(ringing);
                let c = call.clone();
                ui::on_click(&r.root, move || c("ring", "ring", json!({"on": !ringing})));
                body.append(&r.root);
                let ringer = st["ringer"].as_str().unwrap_or("normal").to_string();
                let rr = ui::hbox(0, &["kv"]);
                rr.append(&ui::text("ringer", &["k"]));
                let c = call.clone();
                rr.append(&ui::segmented(&[("normal", "ring"), ("vibrate", "vibrate"), ("silent", "silent")], &ringer, move |m| c("ringer", "ringer", json!({"mode": m}))));
                body.append(&rr);
                let r = ui::Row::new("\u{f0147}", "send clipboard to phone", "");
                let pc2 = pc.clone();
                ui::on_click(&r.root, move || {
                    pc2.close();
                    util::bg(|| request(&json!({"method": "clip"}), Duration::from_secs(5)), |_| {});
                });
                body.append(&r.root);
            } else if let Some(e) = s["error"].as_str() {
                body.append(&ui::text(e, &["msg", "dim"]).tap(|l| l.set_wrap(true)));
            }
            for (k, v) in busy.borrow().iter() {
                body.append(&ui::text(&format!("{k} {v}"), &["msg", "dim"]));
            }
            for (_, v) in errors.borrow().iter() {
                body.append(&ui::text(v, &["msg", "crit"]).tap(|l| l.set_wrap(true)));
            }
            let perms = &st["permissions"];
            let missing: Vec<&str> = [("notifications", "notification access"), ("phone_state", "phone state"), ("clipboard_auto", "automatic clipboard (tap its notification)")]
                .iter()
                .filter(|(k, _)| perms[*k] == false)
                .map(|(_, l)| *l)
                .collect();
            if connected && !missing.is_empty() {
                body.append(&ui::section("needs a tap on the phone"));
                for m in missing {
                    body.append(&ui::text(m, &["msg", "warn"]));
                }
            }
            if s["laptop"]["listening"] == false {
                body.append(&ui::text("tailscale is down", &["msg", "warn"]));
            }
        }
    };
    *draw.borrow_mut() = Some(Rc::new(render));
    redraw();
    let rr = redraw.clone();
    p.watch(&i.changed, move || rr());
    card.foot_add(&ui::link("reconnect", &[], || util::bg(|| request(&json!({"method": "poke"}), Duration::from_secs(5)), |_| {})));
    card.root.upcast()
}
