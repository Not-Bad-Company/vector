//! The Raspberry Pi + NAS. Two bar items: `pi` (󰐿 temp, ⚡ when throttled)
//! and `nas` (󰋊 drive temp, only while the drive spins; orange after 30 min).
//! Click: panel (Pi health, storage, drive, local mount and who holds it,
//! services). Pi right: ssh; middle: refresh. NAS click: panel; right: open /mnt/nas.
//!
//! Metrics come from the Pi's forced-command ssh key (`ssh -n nas-metrics`),
//! reachability from `tailscale status`, the mount from /proc/mounts. The
//! drive's mount point is never stat-ed while unmounted (that would wake it
//! through autofs).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::Value;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

#[derive(Clone, Default, PartialEq)]
struct State {
    online: bool,
    last_seen: String,
    m: Value,
    cached: bool,
    mounted: bool,
    holders: Vec<String>,
    spin_since: f64,
    fetched: f64,
}

struct Inner {
    pi: Slot,
    nas: Slot,
    state: RefCell<State>,
    changed: Notify,
    theme: Rc<Theme>,
    busy: Cell<bool>,
    host: String,
    peer: String,
    mount: String,
    ssh_host: String,
}

pub struct Homelab(Rc<Inner>);

fn kb(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)).unwrap_or(0)
}

fn fmt_kb(k: u64) -> String {
    util::size_kb(k)
}

fn spin_file() -> std::path::PathBuf {
    util::cache_dir().join("vector-bar/hdd-spin-start")
}

fn metrics_cache() -> std::path::PathBuf {
    util::cache_dir().join("vector-bar/pi-metrics.json")
}

impl Homelab {
    pub fn new(ctx: &Ctx) -> Homelab {
        let o = ctx.opts("homelab");
        let inner = Rc::new(Inner {
            pi: ctx.slot("pi", &[]),
            nas: ctx.slot("nas", &[]),
            state: RefCell::default(),
            changed: Notify::default(),
            theme: ctx.theme.clone(),
            busy: Cell::new(false),
            host: o.str("host", "daanhpi"),
            peer: o.str("tailscale_name", "daanhpi"),
            mount: o.str("mount", "/mnt/nas"),
            ssh_host: o.str("metrics_ssh", "nas-metrics"),
        });
        // first paint from the last known metrics
        let seed = util::read_json(metrics_cache()).or_else(|| util::read_json(util::cache_dir().join("waybar-pi-metrics.json")));
        if let Some(m) = seed {
            inner.state.borrow_mut().m = m;
            inner.state.borrow_mut().cached = true;
        }
        let host = inner.host.clone();
        let right = o.str("on_right", &format!("uwsm app -- xdg-terminal-exec ssh {host}"));
        inner.pi.on_right(move || util::spawn_sh(&right));
        let w = Rc::downgrade(&inner);
        inner.pi.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.poll();
            }
        });
        let open = o.str("open", &format!("vector-find {}", inner.mount));
        inner.nas.on_right(move || util::spawn_sh(&open));
        let w = Rc::downgrade(&inner);
        inner.nas.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.poll();
            }
        });
        for s in [&inner.pi, &inner.nas] {
            let w = Rc::downgrade(&inner);
            s.tooltip(move || w.upgrade().map(|i| i.tooltip()));
            let w = Rc::downgrade(&inner);
            s.panel(move |p| match w.upgrade() {
                Some(i) => panel(&i, p),
                None => ui::vbox(0, &[]).upcast(),
            });
        }
        inner.nas.set_visible(false);
        inner.render();
        inner.poll();
        util::every(o.num("interval", 30.0) as u64 * 1000, &inner, |i| i.poll());
        Homelab(inner)
    }
}

impl Inner {
    fn poll(self: &Rc<Self>) {
        if self.busy.replace(true) {
            return;
        }
        let (peer, mount, ssh) = (self.peer.clone(), self.mount.clone(), self.ssh_host.clone());
        let prev = self.state.borrow().clone();
        let w = Rc::downgrade(self);
        util::bg(
            move || {
                let mut s = State { m: prev.m.clone(), cached: true, spin_since: prev.spin_since, ..Default::default() };
                if let Some(o) = util::run(&["tailscale", "status", "--json"], Duration::from_millis(1500)) {
                    if let Ok(v) = serde_json::from_str::<Value>(&o.stdout) {
                        if let Some(p) = v["Peer"].as_object().and_then(|ps| ps.values().find(|p| p["HostName"] == peer.as_str())) {
                            s.online = p["Online"] == true;
                            s.last_seen = p["LastSeen"].as_str().and_then(util::parse_iso).map(|t| util::local(t, "%Y-%m-%d %H:%M")).unwrap_or_default();
                        }
                    }
                }
                // /proc/mounts, not the path itself: stat-ing it would wake the drive via autofs
                s.mounted = util::read("/proc/mounts").unwrap_or_default().lines().any(|l| {
                    let f: Vec<&str> = l.split_whitespace().collect();
                    f.len() > 2 && f[1] == mount && (f[2] == "cifs" || f[2] == "smb3")
                });
                if s.mounted {
                    if let Some(o) = util::run(&["fuser", "-m", &mount], Duration::from_secs(3)) {
                        let mut names: Vec<String> = o.stdout.split_whitespace().filter_map(|p| p.trim_end_matches(|c: char| c.is_alphabetic()).parse::<u32>().ok()).filter_map(|pid| util::read_trim(format!("/proc/{pid}/comm"))).collect();
                        names.sort();
                        names.dedup();
                        s.holders = names;
                    }
                }
                if s.online {
                    if let Some(o) = util::run(&["ssh", "-n", "-o", "ConnectTimeout=3", "-o", "BatchMode=yes", &ssh], Duration::from_millis(4500)) {
                        let raw = o.stdout.trim();
                        let json = raw.lines().find_map(|l| l.split_once("METRICS_JSON=").map(|x| x.1.to_string())).unwrap_or_else(|| raw.to_string());
                        if let Ok(v) = serde_json::from_str::<Value>(&json) {
                            if v.is_object() {
                                s.m = v;
                                s.cached = false;
                                s.fetched = util::now();
                                if let Ok(t) = serde_json::to_string(&s.m) {
                                    let _ = util::write_atomic(metrics_cache(), &t);
                                }
                            }
                        }
                    }
                }
                // how long the drive has been spinning: the longer of the Pi's and ours
                if s.m["hdd_status"] == "active" {
                    let mine = util::read_trim(spin_file()).and_then(|x| x.parse::<f64>().ok());
                    let start = match mine {
                        Some(t) => t,
                        None => {
                            let _ = util::write_atomic(spin_file(), &util::now().to_string());
                            util::now()
                        }
                    };
                    let pi_secs = s.m["hdd_spin_secs"].as_f64().unwrap_or(0.0);
                    s.spin_since = start.min(util::now() - pi_secs);
                } else {
                    let _ = std::fs::remove_file(spin_file());
                    s.spin_since = 0.0;
                }
                s
            },
            move |s| {
                let Some(i) = w.upgrade() else { return };
                i.busy.set(false);
                let changed = *i.state.borrow() != s;
                *i.state.borrow_mut() = s;
                i.render();
                if changed {
                    i.changed.emit();
                }
            },
        );
    }

    fn spin_secs(&self) -> u64 {
        let s = self.state.borrow();
        if s.spin_since > 0.0 { (util::now() - s.spin_since).max(0.0) as u64 } else { 0 }
    }

    fn render(&self) {
        let s = self.state.borrow();
        let temp = s.m["pi_temp"].as_i64();
        let throttled = s.m["throttled_hex"].as_str().is_some_and(|h| h != "0x0");
        let mut text = match temp {
            Some(t) => format!("󰐿  {t}°C"),
            None => "󰐿  --".into(),
        };
        if throttled && s.online {
            text += " ⚡";
        }
        self.pi.set_markup(&text);
        let cls = if !s.online || s.cached {
            "faint"
        } else if throttled {
            "crit"
        } else if temp.is_some_and(|t| t >= 80) {
            "crit"
        } else if temp.is_some_and(|t| t >= 70) {
            "warn"
        } else {
            ""
        };
        self.pi.set_classes(&[cls]);
        self.pi.bump();
        let spinning = s.m["hdd_status"] == "active" && s.m["hdd_temp"].as_i64().is_some();
        drop(s);
        if spinning {
            let s = self.state.borrow();
            let long = self.spin_secs() >= 1800;
            self.nas.set_markup(&format!("󰋊 {}°C", s.m["hdd_temp"].as_i64().unwrap_or(0)));
            self.nas.set_classes(&[if long { "warn" } else { "" }]);
        }
        self.nas.set_visible(spinning);
        self.nas.bump();
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = self.state.borrow();
        let m = &s.m;
        let sh = ui::sheet();
        let detail = if !s.online {
            format!("offline since {}", s.last_seen)
        } else if s.cached {
            "cached".into()
        } else {
            format!("up {}", m["uptime_str"].as_str().unwrap_or("?"))
        };
        sh.append(&ui::head(&self.host, &detail).0);
        if m["throttled_hex"].as_str().is_some_and(|h| h != "0x0") {
            sh.append(&ui::text(m["throttled_msg"].as_str().unwrap_or("throttled"), &["crit"]));
        }
        sh.append(&ui::tsection("usage"));
        let load = m["load_1m"].as_str().and_then(|x| x.parse::<f64>().ok()).or(m["load_1m"].as_f64()).unwrap_or(0.0);
        let cores = m["cores"].as_f64().unwrap_or(4.0);
        sh.append(&tmeter("cpu", load / cores, &format!("{load:.2} load"), &m["pi_temp"].as_i64().map(|t| format!("{t}°")).unwrap_or_default()));
        sh.append(&tmeter("mem", kb(&m["ram_used_kb"]) as f64 / kb(&m["ram_total_kb"]).max(1) as f64, &format!("{} of {}", fmt_kb(kb(&m["ram_used_kb"])), fmt_kb(kb(&m["ram_total_kb"]))), ""));
        sh.append(&tmeter("root", m["root_pct"].as_f64().unwrap_or(0.0) / 100.0, &format!("{}%", m["root_pct"]), ""));
        sh.append(&ui::tsection("storage"));
        if kb(&m["nas_total_kb"]) > 0 {
            sh.append(&tmeter("nas", m["nas_pct"].as_f64().unwrap_or(0.0) / 100.0, &format!("{} of {}", fmt_kb(kb(&m["nas_used_kb"])), fmt_kb(kb(&m["nas_total_kb"]))), ""));
        }
        let hdd = if m["hdd_status"] == "active" {
            format!("spinning {}  {}  {}°", util::dur(self.spin_secs()), ui::dim("·"), m["hdd_temp"])
        } else {
            ui::dim("parked")
        };
        sh.append(&ui::tkv("hdd", &hdd));
        sh.append(&ui::tkv("mount", &self.mount_text(&s)));
        sh.append(&ui::thint("click details  ·  pi: right ssh, middle refresh  ·  nas: right open"));
        sh.upcast()
    }

    fn mount_text(&self, s: &State) -> String {
        if s.mounted && !s.holders.is_empty() {
            ui::colored(&format!("held by {}", util::esc(&s.holders.join(" "))), &self.theme.color3)
        } else if s.mounted {
            format!("{} {}", self.mount, ui::dim("idle"))
        } else {
            ui::dim("not mounted")
        }
    }
}

fn tmeter(k: &str, frac: f64, v: &str, extra: &str) -> gtk::Box {
    let cls = if frac >= 0.92 { "crit" } else if frac >= 0.8 { "warn" } else { "" };
    ui::tmeter(k, frac, v, extra, cls)
}

impl Module for Homelab {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        let b = ui::hbox(0, &["homelab"]);
        b.append(&self.0.pi.view());
        b.append(&self.0.nas.view());
        b.upcast()
    }
    fn refresh(&self) {
        self.0.poll();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.pi.clone(), self.0.nas.clone()]
    }
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(380);
    let card = Card::new("homelab", "");
    let body = ui::vbox(0, &[]);
    card.body.append(&body);
    let fill = {
        let (i, body, detail) = (Rc::downgrade(i), body.clone(), card.detail.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            let s = i.state.borrow().clone();
            let m = &s.m;
            ui::clear(&body);
            let model = m["model"].as_str().unwrap_or("raspberry pi").to_lowercase();
            detail.set_markup(&if !s.online {
                ui::colored(&format!("{} · offline since {}", i.host, s.last_seen), &i.theme.color1)
            } else {
                format!("{} · {} · up {}{}", i.host, model, m["uptime_str"].as_str().unwrap_or("?"), if s.cached { " · cached" } else { "" })
            });
            if m["throttled_hex"].as_str().is_some_and(|h| h != "0x0") {
                body.append(&ui::text(&format!("⚡ {}", m["throttled_msg"].as_str().unwrap_or("throttled")), &["msg", "crit"]));
            }
            body.append(&ui::section("pi").tap(|l| l.add_css_class("first")));
            let load = m["load_1m"].as_str().and_then(|x| x.parse::<f64>().ok()).or(m["load_1m"].as_f64()).unwrap_or(0.0);
            let cores = m["cores"].as_f64().unwrap_or(4.0);
            let loads = format!("{} {} {}", m["load_1m"].as_str().unwrap_or("-"), m["load_5m"].as_str().unwrap_or("-"), m["load_15m"].as_str().unwrap_or("-"));
            body.append(&ui::meter_row("cpu", load / cores, &format!("{load:.2}"), &format!("load {loads} · {:.1} GHz", m["cpu_freq_mhz"].as_f64().unwrap_or(0.0) / 1000.0), ""));
            if let Some(t) = m["pi_temp"].as_i64() {
                body.append(&ui::meter_row("temp", t as f64 / 85.0, &format!("{t}°"), "", if t >= 80 { "crit" } else if t >= 70 { "warn" } else { "" }));
            }
            let rp = m["ram_pct"].as_f64().unwrap_or(0.0);
            body.append(&ui::meter_row("mem", rp / 100.0, &format!("{rp:.0}%"), &format!("{} of {}", fmt_kb(kb(&m["ram_used_kb"])), fmt_kb(kb(&m["ram_total_kb"]))), if rp >= 95.0 { "crit" } else if rp >= 85.0 { "warn" } else { "" }));
            if kb(&m["swap_used_kb"]) > 0 {
                let sp = m["swap_pct"].as_f64().unwrap_or(0.0);
                body.append(&ui::meter_row("swap", sp / 100.0, &format!("{sp:.0}%"), &format!("{} of {}", fmt_kb(kb(&m["swap_used_kb"])), fmt_kb(kb(&m["swap_total_kb"]))), ""));
            }
            let rp = m["root_pct"].as_f64().unwrap_or(0.0);
            body.append(&ui::meter_row("root", rp / 100.0, &format!("{rp:.0}%"), &format!("{} of {}", fmt_kb(kb(&m["root_used_kb"])), fmt_kb(kb(&m["root_total_kb"]))), if rp >= 92.0 { "crit" } else if rp >= 80.0 { "warn" } else { "" }));

            body.append(&ui::section("nas"));
            if kb(&m["nas_total_kb"]) > 0 {
                let np = m["nas_pct"].as_f64().unwrap_or(0.0);
                body.append(&ui::meter_row("disk", np / 100.0, &format!("{np:.0}%"), &format!("{} of {}", fmt_kb(kb(&m["nas_used_kb"])), fmt_kb(kb(&m["nas_total_kb"]))), if np >= 92.0 { "crit" } else if np >= 80.0 { "warn" } else { "" }));
            }
            let secs = i.spin_secs();
            let hdd = if m["hdd_status"] == "active" {
                let t = format!("spinning {}  {}  {}°", util::dur(secs), ui::dim("·"), m["hdd_temp"]);
                if secs >= 1800 { ui::colored(&t, &i.theme.color3) } else { t }
            } else {
                ui::dim("parked")
            };
            body.append(&ui::kv("drive", &hdd).0);
            body.append(&ui::kv("mount", &i.mount_text(&s)).0);
            if s.mounted && !s.holders.is_empty() {
                body.append(&ui::text("the drive can't spin down while something holds the mount", &["msg", "dim"]).tap(|l| l.set_wrap(true)));
            }

            body.append(&ui::section("services"));
            let svc = |name: &str, active: bool, extra: String| ui::kv(name, &if active { extra } else { ui::dim("off") }).0;
            body.append(&svc("docker", m["docker_status"] == "active", format!("{} {}", m["docker_running"], ui::dim("running"))));
            body.append(&svc("smb", m["smb_status"] == "active", format!("{} {}", m["smb_conns"], ui::dim("client(s)"))));
            body.append(&svc("files", m["filebrowser_status"] == "active", "filebrowser".into()));
            body.append(&ui::kv("ip", &format!("{}  {}", m["ip_ts"].as_str().unwrap_or(""), ui::dim(m["ip_lan"].as_str().unwrap_or("")))).0);
            if s.fetched > 0.0 {
                body.append(&ui::kv("data", &ui::dim(&format!("live, {}", util::ago(s.fetched)))).0);
            }
        }
    };
    let fill = Rc::new(fill);
    fill();
    let f2 = fill.clone();
    p.watch(&i.changed, move || f2());
    let host = i.host.clone();
    let pc = p.clone();
    card.foot_add(&ui::link("ssh", &[], move || {
        pc.close();
        util::spawn_sh(&format!("uwsm app -- xdg-terminal-exec ssh {host}"));
    }));
    let (pc, mount) = (p.clone(), i.mount.clone());
    card.foot_add(&ui::link("open nas", &[], move || {
        pc.close();
        util::spawn(&["vector-find", &mount]);
    }));
    card.foot_add(&ui::spacer());
    let w = Rc::downgrade(i);
    card.foot_add(&ui::link("refresh", &[], move || {
        if let Some(i) = w.upgrade() {
            i.poll();
        }
    }));
    card.root.upcast()
}
