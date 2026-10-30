//! AI: the agents working on this laptop (from `crew who --json`), with what
//! each is doing and a message box; the AI workspace (workspace 0) and the AI
//! browser; and whether the safety pieces (guard hook, aisudo) are in place.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk::prelude::*;
use serde_json::Value;

use crate::page::{Ctx, Frame, View};
use crate::ui::{self, Keyed, Row};

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

fn ago(ts: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let s = (now - ts).max(0.0) as i64;
    match s {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86400),
    }
}

fn short(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s
    }
}

#[derive(Default, Clone, PartialEq)]
struct Safety {
    guard_accounts: Vec<String>,
    guard_current: Option<bool>,
    aisudo: bool,
}

fn safety() -> Safety {
    let h = home();
    let mut s = Safety::default();
    if let Ok(rd) = std::fs::read_dir(&h) {
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n == ".claude" || n.starts_with(".claude-account"))
            .collect();
        names.sort();
        for n in names {
            let t = std::fs::read_to_string(format!("{h}/{n}/settings.json")).unwrap_or_default();
            if t.contains("aiws-guard") {
                s.guard_accounts.push(n.trim_start_matches('.').to_string());
            }
        }
    }
    let live = std::fs::read(format!("{h}/.local/libexec/aiws-guard")).ok();
    let repo = std::fs::read(format!("{h}/.local/src/vector/agents/libexec/aiws-guard")).ok();
    s.guard_current = match (live, repo) {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    };
    s.aisudo = std::path::Path::new("/usr/local/libexec/aisudo").exists();
    s
}

fn browser_up() -> bool {
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:9333".parse().expect("static addr"),
        std::time::Duration::from_millis(150),
    )
    .is_ok()
}

struct Ai {
    frame: Frame,
    agents: Keyed,
    ws: gtk::Box,
    safe: gtk::Box,
    open: RefCell<HashSet<String>>,
    who: RefCell<Value>,
    windows: RefCell<Vec<String>>,
    browser: RefCell<bool>,
    safety: RefCell<Safety>,
    sent: RefCell<Option<(String, String)>>,
}

pub fn page(_: &Ctx) -> View {
    let frame = Frame::new("ai");
    frame.body.append(&ui::section("agents"));
    let agents = Keyed::new(&["group"]);
    frame.body.append(&agents.root);
    frame.body.append(&ui::section("ai workspace"));
    let ws = ui::group();
    frame.body.append(&ws);
    frame.body.append(&ui::section("safety"));
    let safe = ui::group();
    frame.body.append(&safe);
    frame.foot_add(&ui::text("crew · aiws", &["faint"]));

    let a = Rc::new(Ai {
        frame,
        agents,
        ws,
        safe,
        open: RefCell::default(),
        who: RefCell::new(Value::Null),
        windows: RefCell::default(),
        browser: RefCell::new(false),
        safety: RefCell::default(),
        sent: RefCell::default(),
    });
    let root = a.frame.root.clone();
    let (x, y) = (a.clone(), a.clone());
    View::new(&root).on_show(move || x.refresh()).every(4000, move || y.refresh())
}

impl Ai {
    fn refresh(self: &Rc<Self>) {
        let w = self.clone();
        glib::spawn_future_local(async move {
            let (_, out, _) = ui::run(vec!["crew".into(), "who".into(), "--json".into()]).await;
            let (_, wins, _) = ui::run(vec!["aiws".into(), "windows".into()]).await;
            let who: Value = serde_json::from_str(&out).unwrap_or(Value::Null);
            let windows: Vec<String> = wins.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
            let browser = browser_up();
            let safety = safety();
            let ws_changed = *w.windows.borrow() != windows || *w.browser.borrow() != browser;
            let safe_changed = *w.safety.borrow() != safety;
            *w.who.borrow_mut() = who;
            *w.windows.borrow_mut() = windows;
            *w.browser.borrow_mut() = browser;
            *w.safety.borrow_mut() = safety;
            w.draw_agents();
            if ws_changed || w.ws.first_child().is_none() {
                w.draw_ws();
            }
            if safe_changed || w.safe.first_child().is_none() {
                w.draw_safety();
            }
        });
    }

    fn draw_agents(self: &Rc<Self>) {
        let who = self.who.borrow().clone();
        let claude: Vec<Value> = who["claude"].as_array().cloned().unwrap_or_default();
        let ext: Vec<Value> = who["external"].as_array().cloned().unwrap_or_default();
        let busy = claude.iter().filter(|c| c["status"] == "busy").count();
        self.frame.detail.set_markup(&ui::esc(&match (claude.len(), busy) {
            (0, _) => "no agents running".to_string(),
            (n, b) => format!("{n} agent{} · {b} working", if n == 1 { "" } else { "s" }),
        }));
        let mut items: Vec<ui::Item> = Vec::new();
        for c in claude {
            let name = c["name"].as_str().unwrap_or("?").to_string();
            let open = self.open.borrow().contains(&name);
            let sent = self.sent.borrow().clone().filter(|(n, _)| *n == name).map(|(_, m)| m);
            // while open, only rebuild when the agent did something new
            let sig = if open {
                format!("open|{}|{}|{:?}", c["status"], c["calls"].as_array().map(|a| a.len()).unwrap_or(0), sent)
            } else {
                format!("{}|{}|{}|{}", c["status"], c["title"], c["last"], c["subs"])
            };
            let w = self.clone();
            items.push((name.clone(), sig, Box::new(move || w.agent_row(&c, open, sent))));
        }
        for e in ext {
            let key = format!("ext-{}", e["pid"]);
            let sig = e.to_string();
            items.push((
                key,
                sig,
                Box::new(move || {
                    let meta = format!(
                        "{} · for {} · {}",
                        e["mode"].as_str().unwrap_or(""),
                        e["owner"].as_str().unwrap_or("?"),
                        ago(e["started"].as_f64().unwrap_or(0.0))
                    );
                    let r = Row::new("\u{f0bc8}", e["tool"].as_str().unwrap_or("?"), &ui::dim(&meta));
                    r.root.upcast()
                }),
            ));
        }
        if items.is_empty() {
            items.push(("empty".into(), String::new(), Box::new(|| ui::text("no agents running", &["note"]).upcast())));
        }
        self.agents.sync(items);
    }

    fn agent_row(self: &Rc<Self>, c: &Value, open: bool, sent: Option<String>) -> gtk::Widget {
        let name = c["name"].as_str().unwrap_or("?").to_string();
        let busy = c["status"] == "busy";
        let mut meta = vec![c["account"].as_str().unwrap_or("").to_string()];
        meta.push(if busy { "working".into() } else { format!("idle · {}", ago(c["last"].as_f64().unwrap_or(0.0))) });
        let running_subs = c["subs"].as_array().map(|s| s.iter().filter(|x| x["running"] == true).count()).unwrap_or(0);
        if running_subs > 0 {
            meta.push(format!("{running_subs} sub-agent{}", if running_subs == 1 { "" } else { "s" }));
        }
        let row = Row::new(if busy { "\u{f06a9}" } else { "\u{f0b79}" }, &name, &ui::dim(&meta.join(" · ")));
        row.active(busy);
        if busy {
            row.trail.append(&gtk::Spinner::builder().spinning(true).build());
        }
        let task = c["title"].as_str().filter(|t| !t.is_empty()).or(c["topic"].as_str()).unwrap_or("");
        let ex = row.expand();
        if !task.is_empty() {
            ex.append(&ui::text(&short(task, 120), &["dim"]));
        }
        {
            let (w, n) = (Rc::downgrade(self), name.clone());
            row.on_click(move || {
                if let Some(w) = w.upgrade() {
                    let mut o = w.open.borrow_mut();
                    if !o.remove(&n) {
                        o.insert(n.clone());
                    }
                    drop(o);
                    w.draw_agents();
                }
            });
        }
        if open {
            if let Some(p) = c["prompt"].as_str() {
                ex.append(&ui::kv("asked", &ui::esc(&short(p, 300))));
            }
            if let Some(cwd) = c["cwd"].as_str() {
                ex.append(&ui::kv("in", &ui::esc(&cwd.replace(&home(), "~"))));
            }
            let calls = c["calls"].as_array().cloned().unwrap_or_default();
            if !calls.is_empty() {
                ex.append(&ui::text("recent actions", &["k"]));
                for call in calls.iter().rev().take(6) {
                    let t = call[0].as_f64().unwrap_or(0.0);
                    let s = call[1].as_str().unwrap_or("");
                    let l = ui::markup(&format!("{} {}", ui::dim(&format!("{:>9}", ago(t))), ui::esc(&short(s, 110))), &[]);
                    l.set_wrap(false);
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    ex.append(&l);
                }
            }
            if let Some(edits) = c["edits"].as_object() {
                let mut e: Vec<(&String, f64)> = edits.iter().map(|(k, v)| (k, v.as_f64().unwrap_or(0.0))).collect();
                e.sort_by(|a, b| b.1.total_cmp(&a.1));
                if !e.is_empty() {
                    let files: Vec<String> = e.iter().take(6).map(|(k, _)| k.replace(&home(), "~")).collect();
                    ex.append(&ui::kv("files", &ui::esc(&files.join("\n"))));
                }
            }
            for s in c["subs"].as_array().into_iter().flatten().filter(|s| s["running"] == true) {
                ex.append(&ui::kv(
                    "sub-agent",
                    &format!("{} {}", ui::esc(s["desc"].as_str().unwrap_or("")), ui::dim(s["type"].as_str().unwrap_or(""))),
                ));
            }
            // message, as the owner: reaches the agent at its next step
            let line = ui::hbox(8, &[]);
            let e = gtk::Entry::builder().placeholder_text("message to this agent").hexpand(true).build();
            let go = {
                let (w, n, e) = (Rc::downgrade(self), name.clone(), e.clone());
                Rc::new(move || {
                    let t = e.text().trim().to_string();
                    let Some(w) = w.upgrade() else { return };
                    if t.is_empty() {
                        return;
                    }
                    e.set_text("");
                    let (w2, n2) = (w.clone(), n.clone());
                    glib::spawn_future_local(async move {
                        let (ok, _, err) = ui::run(vec!["crew".into(), "send".into(), n2.clone(), t]).await;
                        *w2.sent.borrow_mut() =
                            Some((n2, if ok { "sent: it reads it at its next step".into() } else { short(&err, 100) }));
                        w2.draw_agents();
                    });
                })
            };
            let g2 = go.clone();
            e.connect_activate(move |_| g2());
            line.append(&e);
            line.append(&ui::link("send", &["primary"], move || go()));
            ex.append(&line);
            if let Some(m) = sent {
                ex.append(&ui::text(&m, &["dim"]));
            }
        }
        row.root.upcast()
    }

    fn draw_ws(self: &Rc<Self>) {
        ui::clear(&self.ws);
        let wins = self.windows.borrow().clone();
        let n = wins.len();
        let r = Row::new(
            "\u{f0379}",
            "workspace 0",
            &ui::dim(&if n == 0 { "empty".to_string() } else { format!("{n} window{}", if n == 1 { "" } else { "s" }) }),
        );
        r.trail.append(&ui::link("watch", &[], || ui::spawn(&["aiws", "show"])));
        // clean up asks once
        let armed = Rc::new(std::cell::Cell::new(false));
        let cl = ui::link("clean up", &["danger"], || {});
        let (cl2, w) = (cl.clone(), Rc::downgrade(self));
        cl.connect_clicked(move |_| {
            if !armed.replace(true) {
                cl2.set_label("close everything?");
                return;
            }
            let w = w.clone();
            glib::spawn_future_local(async move {
                for argv in [vec!["aiws", "close", "all"], vec!["aiws", "nest", "stop"], vec!["aiws", "clean"]] {
                    ui::run(argv.into_iter().map(String::from).collect()).await;
                }
                if let Some(w) = w.upgrade() {
                    w.refresh();
                }
            });
        });
        r.trail.append(&cl);
        self.ws.append(&r.root);
        let names: Vec<String> = wins
            .iter()
            .map(|l| l.split_whitespace().skip(3).collect::<Vec<_>>().join(" "))
            .filter(|s| !s.is_empty())
            .take(8)
            .collect();
        if !names.is_empty() {
            self.ws.append(&ui::kv("open", &ui::esc(&names.join("\n"))));
        }
        let up = *self.browser.borrow();
        let b = Row::new("\u{f059f}", "ai browser", &ui::dim(if up { "running · logins kept" } else { "closed · logins kept" }));
        b.active(up);
        if !up {
            b.trail.append(&ui::link("open", &[], || ui::spawn(&["aiws", "browser"])));
        }
        self.ws.append(&b.root);
    }

    fn draw_safety(self: &Rc<Self>) {
        ui::clear(&self.safe);
        let s = self.safety.borrow().clone();
        let g = Row::new(
            "\u{f0483}",
            "guard hook",
            &ui::dim(&if s.guard_accounts.is_empty() {
                "not installed".to_string()
            } else {
                format!("on for {}", s.guard_accounts.join(", "))
            }),
        );
        g.active(!s.guard_accounts.is_empty());
        self.safe.append(&g.root);
        if s.guard_current == Some(false) {
            let ex = g.expand();
            ex.append(&ui::text("a newer guard is in the repo (fewer false alarms). Install it with:", &["warn"]));
            let cmd = "cp ~/.local/src/vector/agents/libexec/aiws-guard ~/.local/libexec/aiws-guard";
            let l = ui::text(cmd, &[]);
            l.set_selectable(true);
            ex.append(&l);
            ex.append(&ui::link("copy command", &[], move || {
                if let Some(d) = gtk::gdk::Display::default() {
                    d.clipboard().set_text(cmd);
                }
            }));
        }
        let a = Row::new("\u{f0306}", "aisudo", &ui::dim(if s.aisudo { "installed: agents can run allowlisted root commands" } else { "not installed: sudo ~/.local/bin/aisudo-install" }));
        a.active(s.aisudo);
        self.safe.append(&a.root);
    }
}
