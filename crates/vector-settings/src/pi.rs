//! Pi: the homelab server. Health, services, scheduled jobs (logs, run now),
//! and the site it hosts: scrapers per store, visits (Umami), app and
//! accounts. Data comes from one read-only JSON command on the server
//! (`ssh HOST STATUS`), asked once a minute while the page is in view.
//!
//! Config, `~/.config/vectoros/pi.json` (nothing here is specific to one setup):
//! { "ssh": "daanhpi", "status": "bin/kw-status", "name": "krentenweger",
//!   "admin": "https://…/admin", "site": "https://…",
//!   "actions": [ { "unit": "x.service", "label": "run report", "confirm": "sends a mail" } ] }

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::page::{Ctx, Frame, View};
use crate::ui::{self, Row};

#[derive(Deserialize, Default, Clone)]
struct Action {
    unit: String,
    label: String,
    #[serde(default)]
    confirm: String,
}

#[derive(Deserialize, Default, Clone)]
struct Cfg {
    #[serde(default)]
    ssh: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    admin: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    actions: Vec<Action>,
}

fn cfg() -> Option<Cfg> {
    let home = std::env::var("HOME").unwrap_or_default();
    let t = std::fs::read_to_string(format!("{home}/.config/vectoros/pi.json")).ok()?;
    serde_json::from_str(&t).ok()
}

fn ssh_argv(c: &Cfg, cmd: &str) -> Vec<String> {
    ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6", &c.ssh, cmd]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn gb(b: f64) -> String {
    format!("{:.0} GB", b / 1e9)
}

fn dur(s: f64) -> String {
    let s = s as i64;
    match s {
        s if s < 90 => format!("{s} s"),
        s if s < 5400 => format!("{} min", s / 60),
        s if s < 172800 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86400),
    }
}

/// "Thu 2026-10-29 10:30:01 CET" -> "10:30" today, "thu 10:30" otherwise.
fn when_systemd(s: &str) -> String {
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() < 3 {
        return if s.is_empty() || s == "n/a" { "never".into() } else { s.into() };
    }
    let today = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok()).map(|s| s.to_string());
    let hm = &p[2][..p[2].len().min(5)];
    if today.as_deref() == Some(p[1]) {
        hm.to_string()
    } else {
        format!("{} {hm}", p[0].to_lowercase())
    }
}

/// ISO UTC -> local "10:30" / "thu 10:30".
fn when_iso(s: &str) -> String {
    let Ok(t) = glib::DateTime::from_iso8601(s, Some(&glib::TimeZone::utc())) else { return s.into() };
    let Ok(l) = t.to_local() else { return s.into() };
    let today = glib::DateTime::now_local().ok().and_then(|d| d.format("%Y-%m-%d").ok());
    let day = l.format("%Y-%m-%d").ok();
    let f = if day == today { "%H:%M" } else { "%a %H:%M" };
    l.format(f).map(|s| s.to_lowercase()).unwrap_or_default()
}

struct Pi {
    cfg: Cfg,
    frame: Frame,
    body: gtk::Box,
    data: RefCell<Value>,
    error: RefCell<Option<String>>,
    loading: Cell<bool>,
    open: RefCell<HashSet<String>>,
    logs: RefCell<std::collections::HashMap<String, String>>,
    armed: RefCell<Option<String>>,
    running: RefCell<HashSet<String>>,
    note: RefCell<Option<String>>,
}

pub fn page(_: &Ctx) -> View {
    let frame = Frame::new("pi");
    let Some(cfg) = cfg().filter(|c| !c.ssh.is_empty() && !c.status.is_empty()) else {
        frame.detail.set_text("not set up");
        frame.body.append(&ui::section("set up"));
        let g = ui::group();
        g.append(&ui::text(
            "Write ~/.config/vectoros/pi.json with the ssh host and the server's status command, e.g. {\"ssh\": \"myserver\", \"status\": \"bin/status\"}. The command prints JSON (host, services, timers).",
            &["note"],
        ));
        frame.body.append(&g);
        return View::new(&frame.root);
    };
    let body = ui::vbox(0, &[]);
    frame.body.append(&body);
    let refresh = ui::link("refresh", &[], || {});
    frame.controls.append(&refresh);
    if !cfg.admin.is_empty() {
        let u = cfg.admin.clone();
        frame.foot_add(&ui::link("admin panel ›", &["primary"], move || open_uri(&u)));
    }
    if !cfg.site.is_empty() {
        let u = cfg.site.clone();
        frame.foot_add(&ui::link("site ›", &[], move || open_uri(&u)));
    }
    let host = cfg.ssh.clone();
    frame.foot_add(&ui::link("ssh ›", &[], move || {
        ui::spawn(&["uwsm-app", "--", "xdg-terminal-exec", "ssh", &host]);
    }));
    frame.foot_add(&ui::spacer());
    frame.foot_add(&ui::text(&format!("ssh {} {}", cfg.ssh, cfg.status), &["faint"]));

    let p = Rc::new(Pi {
        cfg,
        frame,
        body,
        data: RefCell::new(Value::Null),
        error: RefCell::default(),
        loading: Cell::new(false),
        open: RefCell::default(),
        logs: RefCell::default(),
        armed: RefCell::default(),
        running: RefCell::default(),
        note: RefCell::default(),
    });
    let w = Rc::downgrade(&p);
    refresh.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.fetch();
        }
    });
    let root = p.frame.root.clone();
    let (a, b) = (p.clone(), p.clone());
    View::new(&root).on_show(move || a.fetch()).every(60_000, move || b.fetch())
}

fn open_uri(u: &str) {
    let _ = gio::AppInfo::launch_default_for_uri(u, gio::AppLaunchContext::NONE);
}

impl Pi {
    fn fetch(self: &Rc<Self>) {
        if self.loading.replace(true) {
            return;
        }
        if self.data.borrow().is_null() {
            self.frame.detail.set_text("asking the server…");
        }
        let p = self.clone();
        glib::spawn_future_local(async move {
            let (ok, out, err) = ui::run(ssh_argv(&p.cfg, &p.cfg.status)).await;
            p.loading.set(false);
            match serde_json::from_str::<Value>(&out) {
                Ok(v) if ok => {
                    *p.data.borrow_mut() = v;
                    *p.error.borrow_mut() = None;
                }
                _ => {
                    let e = err.lines().last().unwrap_or("no answer").to_string();
                    *p.error.borrow_mut() = Some(e);
                }
            }
            p.draw();
        });
    }

    fn draw(self: &Rc<Self>) {
        let d = self.data.borrow().clone();
        let b = &self.body;
        ui::clear(b);
        if let Some(e) = self.error.borrow().as_ref() {
            b.append(&ui::text(&format!("can't reach {}: {e}", self.cfg.ssh), &["msg", "crit"]));
        }
        if d.is_null() {
            if self.error.borrow().is_some() {
                self.frame.detail.set_text("offline");
            }
            return;
        }
        if let Some(n) = self.note.borrow().as_ref() {
            b.append(&ui::text(n, &["msg", "dim"]));
        }
        let h = &d["host"];
        let mut detail = vec![if self.error.borrow().is_some() { "unreachable · last known".to_string() } else { "online".into() }];
        if let Some(t) = h["temp"].as_f64() {
            detail.push(format!("{t:.0}°C"));
        }
        if let Some(u) = h["uptime"].as_f64() {
            detail.push(format!("up {}", dur(u)));
        }
        let down: Vec<&str> = d["services"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| !(s["state"] == "active" || s["state"].as_str().is_some_and(|x| x.starts_with("Up"))))
            .filter(|s| !s["state"].as_str().is_some_and(|x| x.starts_with("Exited (0)")))
            .filter_map(|s| s["name"].as_str())
            .collect();
        if !down.is_empty() {
            detail.push(format!("<span foreground=\"#d77\">{} down</span>", down.len()));
        }
        self.frame.detail.set_markup(&detail.join(" · "));

        self.draw_site(&d);
        self.draw_stores(&d);
        self.draw_jobs(&d);
        self.draw_server(&d);
    }

    fn draw_site(self: &Rc<Self>, d: &Value) {
        let u = &d["umami"];
        let kw = &d["kw"];
        let name = if self.cfg.name.is_empty() { "site" } else { &self.cfg.name };
        let days: Vec<(String, f64, f64)> = u["kw_days"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| {
                (
                    x["day"].as_str().unwrap_or("").to_string(),
                    x["visitors"].as_f64().unwrap_or(0.0),
                    x["views"].as_f64().unwrap_or(0.0),
                )
            })
            .collect();
        if days.is_empty() && kw.is_null() {
            return;
        }
        self.body.append(&ui::section(&format!("{name} · visits")));
        let g = ui::group();
        if let Some(site) = u["sites"].as_array().and_then(|a| a.iter().find(|s| s["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))).or(a.first())) {
            let today = days.last().map(|x| x.1).unwrap_or(0.0);
            g.append(&ui::kv(
                "last 7 days",
                &format!(
                    "<b>{}</b> visitors {} {}",
                    site["visitors_7d"],
                    ui::dim("·"),
                    ui::dim(&format!("{} page views · {today:.0} visitors today", site["views_7d"]))
                ),
            ));
        }
        if !days.is_empty() {
            g.append(&chart(&days));
        }
        let list = |k: &str, v: &str, arr: &Value, label: &str| -> Option<gtk::Box> {
            let items: Vec<String> = arr
                .as_array()?
                .iter()
                .map(|x| format!("{} {}", ui::esc(x[k].as_str().unwrap_or("?")), ui::dim(&x[v].to_string())))
                .collect();
            (!items.is_empty()).then(|| ui::kv(label, &items.join("\n")))
        };
        if let Some(r) = list("path", "n", &u["kw_pages"], "top pages") {
            g.append(&r);
        }
        if let Some(r) = list("host", "n", &u["kw_referrers"], "came from") {
            g.append(&r);
        }
        if let Some(r) = list("device", "n", &u["kw_devices"], "devices") {
            g.append(&r);
        }
        let others: Vec<String> = u["sites"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| !s["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .map(|s| format!("{} {}", ui::esc(s["domain"].as_str().unwrap_or("?")), ui::dim(&format!("{} visitors", s["visitors_7d"]))))
            .collect();
        if !others.is_empty() {
            g.append(&ui::kv("other sites", &others.join("\n")));
        }
        self.body.append(&g);

        // app and accounts
        if !kw.is_null() {
            self.body.append(&ui::section(&format!("{name} · app & accounts")));
            let g = ui::group();
            let us = &kw["users"];
            g.append(&ui::kv(
                "accounts",
                &format!("<b>{}</b> {}", us["total"], ui::dim(&format!("{} verified · {} new this week", us["verified"], us["new_7d"]))),
            ));
            if let Some(p) = kw["premium"].as_object() {
                let act = p.get("active").and_then(Value::as_i64).unwrap_or(0);
                let rest: i64 = p.iter().filter(|(k, _)| *k != "active").filter_map(|(_, v)| v.as_i64()).sum();
                g.append(&ui::kv("premium", &format!("<b>{act}</b> active {}", ui::dim(&format!("· {rest} ended")))));
            }
            let devs: Vec<String> = kw["app_devices"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| format!("{} {}", r[1], ui::dim(&format!("{} · {} active this week", r[0].as_str().unwrap_or("?"), r[2]))))
                .collect();
            g.append(&ui::kv("app devices", &if devs.is_empty() { ui::dim("none") } else { devs.join("\n") }));
            let inst: Vec<String> = kw["app_installs_30d"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| format!("{} {}", r[1], ui::dim(r[0].as_str().unwrap_or("?"))))
                .collect();
            g.append(&ui::kv("installs, 30 days", &if inst.is_empty() { ui::dim("none recorded") } else { inst.join("\n") }));
            g.append(&ui::kv(
                "to review",
                &format!("{} {}", kw["review_open"], ui::dim(&format!("proposals · {} suggestions today", kw["suggestions_today"]))),
            ));
            self.body.append(&g);
        }
    }

    fn draw_stores(self: &Rc<Self>, d: &Value) {
        let Some(runs) = d["kw"]["scrape"].as_array() else { return };
        let fails = runs.iter().filter(|r| r["status"] != "success").count();
        let name = if self.cfg.name.is_empty() { "site" } else { &self.cfg.name };
        let s24 = &d["kw"]["scrape_24h"];
        let head = ui::section(&format!("{name} · scrapers"));
        head.append(&ui::markup(
            &ui::dim(&format!(
                "{} ok · {} failed in 24 h",
                s24["success"].as_i64().unwrap_or(0),
                s24["failed"].as_i64().unwrap_or(0)
            )),
            &[],
        ));
        self.body.append(&head);
        let g = ui::group();
        let mut runs = runs.clone();
        // failures first
        runs.sort_by_key(|r| (r["status"] == "success", r["store"].as_str().unwrap_or("").to_string()));
        for r in runs {
            let ok = r["status"] == "success";
            let meta = if ok {
                format!("{} offers · {} new · {}", r["found"], r["new"], when_iso(r["started"].as_str().unwrap_or("")))
            } else {
                format!("failed · {}", when_iso(r["started"].as_str().unwrap_or("")))
            };
            let row = Row::new(if ok { "\u{f012c}" } else { "\u{f0159}" }, r["store"].as_str().unwrap_or("?"), &ui::dim(&meta));
            if !ok {
                row.root.add_css_class("bad");
                row.expand().append(&ui::text(r["error"].as_str().unwrap_or(""), &["crit"]));
            }
            g.append(&row.root);
        }
        let _ = fails;
        self.body.append(&g);
    }

    fn draw_jobs(self: &Rc<Self>, d: &Value) {
        let Some(ts) = d["timers"].as_array() else { return };
        self.body.append(&ui::section("scheduled jobs"));
        let g = ui::group();
        for t in ts {
            let svc = t["service"].as_str().unwrap_or("").to_string();
            let name = t["timer"].as_str().unwrap_or("?").trim_end_matches(".timer").to_string();
            let enabled = t["enabled"] == "enabled";
            let failed = t["result"] != "success" && t["result"] != "";
            let running = t["active"] == "activating" || self.running.borrow().contains(&svc);
            let meta = if !enabled {
                "off".to_string()
            } else if running {
                "running…".into()
            } else {
                format!(
                    "{} {} · next {}",
                    if failed { "failed" } else { "ok" },
                    when_systemd(t["last"].as_str().unwrap_or("")),
                    when_systemd(t["next"].as_str().unwrap_or(""))
                )
            };
            let row = Row::new(
                if failed { "\u{f0159}" } else if enabled { "\u{f0954}" } else { "\u{f0a37}" },
                &name,
                &ui::dim(&meta),
            );
            if running {
                row.trail.append(&gtk::Spinner::builder().spinning(true).build());
            }
            if let Some(a) = self.cfg.actions.iter().find(|a| a.unit == svc).cloned() {
                let armed = self.armed.borrow().as_deref() == Some(svc.as_str());
                let label = if armed {
                    if a.confirm.is_empty() { "run now?".to_string() } else { format!("{}? {}", a.label, a.confirm) }
                } else {
                    a.label.clone()
                };
                let (w, unit) = (Rc::downgrade(self), svc.clone());
                row.trail.append(&ui::link(&label, if armed { &["danger"] } else { &[] }, move || {
                    if let Some(p) = w.upgrade() {
                        p.run_unit(&unit);
                    }
                }));
            }
            let open = self.open.borrow().contains(&svc);
            let ex = row.expand();
            let said = t["said"].as_str().unwrap_or("");
            if !said.is_empty() && !open {
                let l = ui::text(said, &["dim"]);
                l.set_wrap(false);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                ex.append(&l);
            }
            if open {
                let log = self.logs.borrow().get(&svc).cloned().unwrap_or_else(|| "loading…".into());
                let l = ui::text(&log, &["log"]);
                l.set_selectable(true);
                ex.append(&l);
            }
            let (w, unit) = (Rc::downgrade(self), svc.clone());
            row.on_click(move || {
                if let Some(p) = w.upgrade() {
                    p.toggle_log(&unit);
                }
            });
            g.append(&row.root);
        }
        self.body.append(&g);
    }

    fn draw_server(self: &Rc<Self>, d: &Value) {
        let h = &d["host"];
        self.body.append(&ui::section("server"));
        let g = ui::group();
        if let Some(t) = h["temp"].as_f64() {
            let warm = if h["throttled"] == true { " · throttled" } else { "" };
            g.append(&ui::kv("temperature", &format!("{t:.1}°C{}", ui::esc(warm))));
        }
        if let Some(l) = h["load"].as_f64() {
            g.append(&ui::kv("load", &format!("{l:.2}")));
        }
        if let (Some(u), Some(t)) = (h["mem_used"].as_f64(), h["mem_total"].as_f64()) {
            g.append(&ui::kv("memory", &format!("{} {}", gb(u), ui::dim(&format!("of {}", gb(t))))));
        }
        if let (Some(u), Some(t)) = (h["disk_used"].as_f64(), h["disk_total"].as_f64()) {
            g.append(&ui::kv("disk", &format!("{:.0}% {}", u / t * 100.0, ui::dim(&format!("{} of {}", gb(u), gb(t))))));
        }
        let bk = &d["backup"];
        if let Some(age) = bk["age"].as_f64() {
            let s = format!("{} ago {}", dur(age), ui::dim(&format!("· {}", gb(bk["size"].as_f64().unwrap_or(0.0) * 1.0))));
            g.append(&ui::kv("last backup", &if age > 36.0 * 3600.0 { format!("<span foreground=\"#d77\">{s}</span>") } else { s }));
        }
        self.body.append(&g);
        self.body.append(&ui::section("services"));
        let g = ui::group();
        for s in d["services"].as_array().into_iter().flatten() {
            let st = s["state"].as_str().unwrap_or("?");
            let ok = st == "active" || st.starts_with("Up");
            let quiet = st.starts_with("Exited (0)");
            let row = Row::new(
                if s["docker"] == true { "\u{f0868}" } else { "\u{f048b}" },
                s["name"].as_str().unwrap_or("?"),
                &if ok || quiet { ui::dim(st) } else { format!("<span foreground=\"#d77\">{}</span>", ui::esc(st)) },
            );
            g.append(&row.root);
        }
        self.body.append(&g);
    }

    fn toggle_log(self: &Rc<Self>, unit: &str) {
        let now_open = {
            let mut o = self.open.borrow_mut();
            if o.remove(unit) {
                false
            } else {
                o.insert(unit.to_string());
                true
            }
        };
        self.draw();
        if !now_open {
            return;
        }
        let (p, unit) = (self.clone(), unit.to_string());
        glib::spawn_future_local(async move {
            let cmd = format!("journalctl -u {unit} -n 20 -o short-iso --no-pager");
            let (_, out, err) = ui::run(ssh_argv(&p.cfg, &cmd)).await;
            let text = if out.trim().is_empty() { err } else { out };
            p.logs.borrow_mut().insert(unit, text.trim_end().to_string());
            p.draw();
        });
    }

    /// Starts an allowed unit: the first click arms, the second runs.
    fn run_unit(self: &Rc<Self>, unit: &str) {
        if !self.cfg.actions.iter().any(|a| a.unit == unit) {
            return;
        }
        if self.armed.borrow().as_deref() != Some(unit) {
            *self.armed.borrow_mut() = Some(unit.to_string());
            self.draw();
            return;
        }
        *self.armed.borrow_mut() = None;
        self.running.borrow_mut().insert(unit.to_string());
        self.draw();
        let (p, unit) = (self.clone(), unit.to_string());
        glib::spawn_future_local(async move {
            let cmd = format!("sudo -n systemctl start --no-block {unit} && rm -f /run/user/$(id -u)/kw-status.json");
            let (ok, _, err) = ui::run(ssh_argv(&p.cfg, &cmd)).await;
            *p.note.borrow_mut() = Some(if ok {
                format!("started {unit}: it shows up as running in a moment")
            } else {
                format!("couldn't start {unit}: {}", err.trim())
            });
            glib::timeout_future(std::time::Duration::from_secs(3)).await;
            p.running.borrow_mut().remove(&unit);
            p.fetch();
        });
    }
}

/// A small bar chart: visitors per day (bars) with page views as a faint bar behind.
fn chart(days: &[(String, f64, f64)]) -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.set_content_height(84);
    area.set_hexpand(true);
    area.set_margin_start(14);
    area.set_margin_end(14);
    area.set_margin_top(6);
    area.set_margin_bottom(4);
    let days = days.to_vec();
    let max = days.iter().map(|d| d.2.max(d.1)).fold(1.0, f64::max);
    area.set_draw_func(move |a, cr, w, h| {
        let fg = a.color();
        let n = days.len().max(1) as f64;
        let gap = 4.0;
        let bw = ((w as f64 - gap * (n - 1.0)) / n).max(2.0);
        let base = h as f64 - 14.0;
        cr.select_font_face("monospace", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
        cr.set_font_size(9.0);
        for (i, (day, vis, views)) in days.iter().enumerate() {
            let x = i as f64 * (bw + gap);
            let hv = views / max * (base - 4.0);
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.12);
            cr.rectangle(x, base - hv, bw, hv);
            let _ = cr.fill();
            let hs = vis / max * (base - 4.0);
            let last = i + 1 == days.len();
            cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, if last { 0.9 } else { 0.55 });
            cr.rectangle(x, base - hs, bw, hs);
            let _ = cr.fill();
            if i % 2 == days.len() % 2 || last {
                let t = day.get(8..10).unwrap_or("");
                let tw = cr.text_extents(t).map(|e| e.width()).unwrap_or(0.0);
                cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.45);
                cr.move_to(x + bw / 2.0 - tw / 2.0, base + 11.0);
                let _ = cr.show_text(t);
            }
        }
    });
    area.set_tooltip_text(Some("visitors per day (bright), page views (faint), last 14 days"));
    area.upcast()
}
