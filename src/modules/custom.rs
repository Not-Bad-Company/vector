//! Script modules, compatible with Waybar's `custom/*`: the command prints
//! `{"text", "tooltip", "class", "alt", "percentage"}` JSON (or plain lines:
//! text, tooltip, class). `interval = 0` keeps the command running and reads
//! one update per line. Empty text hides the module.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::config;
use crate::slot::Slot;
use crate::ui;

trait TapMargin {
    fn tap_margin(self) -> Self;
}
impl TapMargin for gtk::Label {
    fn tap_margin(self) -> Self {
        self.set_margin_top(10);
        self
    }
}
use crate::util;

#[derive(Default, Clone)]
struct Out {
    text: String,
    tooltip: String,
    /// the whole JSON object: a sheet tooltip / panel lives in it
    raw: serde_json::Value,
    classes: Vec<String>,
    alt: String,
    percentage: Option<f64>,
}

fn parse(line: &str, json: bool) -> Out {
    if json {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
            let s = |k: &str| match &v[k] {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            let classes = match &v["class"] {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                _ => vec![],
            };
            return Out { text: s("text"), tooltip: s("tooltip"), classes, alt: s("alt"), percentage: v["percentage"].as_f64(), raw: v };
        }
    }
    let mut it = line.lines();
    Out {
        text: it.next().unwrap_or("").to_string(),
        tooltip: it.next().unwrap_or("").to_string(),
        classes: it.next().map(|c| vec![c.to_string()]).unwrap_or_default(),
        ..Default::default()
    }
}

struct Inner {
    slot: Slot,
    spec: config::Custom,
    out: RefCell<Out>,
    busy: Cell<bool>,
    child: Cell<Option<u32>>,
    me: RefCell<std::rc::Weak<Inner>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(pid) = self.child.get() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }
}

pub struct Custom(Rc<Inner>);

impl Custom {
    pub fn new(spec: &config::Custom, ctx: &Ctx) -> Custom {
        let name = format!("custom-{}", spec.name);
        let slot = ctx.slot(&format!("custom:{}", spec.name), &[]);
        let _ = name;
        let inner = Rc::new(Inner { slot, spec: spec.clone(), out: RefCell::default(), busy: Cell::new(false), child: Cell::new(None), me: RefCell::default() });
        *inner.me.borrow_mut() = Rc::downgrade(&inner);
        let s = &inner.spec;
        for (cmd, which) in [(&s.on_click, 1), (&s.on_click_middle, 2), (&s.on_click_right, 3)] {
            if let Some(c) = cmd.clone() {
                let w = Rc::downgrade(&inner);
                let f = move || {
                    util::spawn_sh(&c);
                    // Waybar re-runs the module after a click
                    if let Some(i) = w.upgrade() {
                        util::after(300, &i, |i| i.run());
                    }
                };
                match which {
                    1 => inner.slot.on_left(f),
                    2 => inner.slot.on_middle(f),
                    _ => inner.slot.on_right(f),
                }
            }
        }
        if s.on_scroll_up.is_some() || s.on_scroll_down.is_some() {
            let (u, d) = (s.on_scroll_up.clone().unwrap_or_default(), s.on_scroll_down.clone().unwrap_or_default());
            inner.slot.on_scroll(move || util::spawn_sh(&u), move || util::spawn_sh(&d));
        }
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || {
            let i = w.upgrade()?;
            let o = i.out.borrow();
            let call: crate::sheet::Call = Rc::new(|_: &str| {});
            if o.raw["tooltip"].is_object() {
                return crate::sheet::tooltip(&o.raw["tooltip"], &call);
            }
            if o.tooltip.is_empty() {
                return None;
            }
            let sh = ui::sheet();
            sh.append(&ui::label(&o.tooltip, &[]).tap_margin());
            Some(sh.upcast())
        });

        inner.slot.set_visible(false);
        if inner.spec.interval <= 0.0 {
            inner.stream();
        } else {
            inner.run();
            util::every((inner.spec.interval * 1000.0) as u64, &inner, |i| i.run());
        }
        Custom(inner)
    }
}

impl Inner {
    fn apply(&self, o: Out) {
        let mut text = o.text.clone();
        if let Some(f) = &self.spec.format {
            text = f
                .replace("{}", &o.text)
                .replace("{text}", &o.text)
                .replace("{alt}", &o.alt)
                .replace("{percentage}", &o.percentage.map(|p| format!("{p:.0}")).unwrap_or_default());
        }
        if let Some(max) = self.spec.max_length {
            if text.chars().count() > max {
                text = text.chars().take(max.saturating_sub(1)).collect::<String>() + "…";
            }
        }
        let markup = if self.spec.markup { text.clone() } else { util::esc(&text) };
        self.slot.set_visible(!o.text.trim().is_empty());
        self.slot.set_markup(&markup);
        let cls: Vec<&str> = o.classes.iter().map(String::as_str).collect();
        self.slot.set_classes(&cls);
        // a "panel" sheet in the output: opened by a left click (unless on_click is set)
        if o.raw["panel"].is_object() && !self.slot.has_panel() {
            let me = self.me.borrow().clone();
            self.slot.panel(move |_| {
                let call: crate::sheet::Call = Rc::new(|_: &str| {});
                let v = me.upgrade().map(|i| i.out.borrow().raw["panel"].clone()).unwrap_or_default();
                crate::sheet::card(&v, &call).root.upcast()
            });
        }
        *self.out.borrow_mut() = o;
    }

    fn run(self: &Rc<Self>) {
        if self.busy.replace(true) {
            return;
        }
        let cmd = util::expand(&self.spec.exec).to_string_lossy().to_string();
        let cmd = if self.spec.exec.starts_with("~/") { cmd } else { self.spec.exec.replace("~/", &format!("{}/", util::home().display())) };
        let json = self.spec.json;
        let w = Rc::downgrade(self);
        util::bg(
            move || util::run(&["sh", "-c", &cmd], Duration::from_secs(30)).map(|o| o.stdout),
            move |out| {
                let Some(i) = w.upgrade() else { return };
                i.busy.set(false);
                if let Some(o) = out {
                    i.apply(parse(&o, json));
                }
            },
        );
    }

    fn stream(self: &Rc<Self>) {
        let cmd = self.spec.exec.replace("~/", &format!("{}/", util::home().display()));
        let (tx, rx) = async_channel::unbounded::<Option<String>>();
        let (ptx, prx) = std::sync::mpsc::channel::<u32>();
        std::thread::spawn(move || {
            loop {
                let mut c = Command::new("sh");
                c.arg("-c").arg(&cmd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
                unsafe {
                    use std::os::unix::process::CommandExt;
                    c.pre_exec(|| {
                        libc::setsid();
                        Ok(())
                    });
                }
                if let Ok(mut child) = c.spawn() {
                    let _ = ptx.send(child.id());
                    if let Some(out) = child.stdout.take() {
                        for line in BufReader::new(out).lines() {
                            let Ok(line) = line else { break };
                            if tx.send_blocking(Some(line)).is_err() {
                                let _ = child.kill();
                                return;
                            }
                        }
                    }
                    let _ = child.wait();
                }
                if tx.send_blocking(None).is_err() {
                    return;
                }
                // restart like Waybar's restart-interval
                std::thread::sleep(Duration::from_secs(10));
            }
        });
        let w = Rc::downgrade(self);
        let json = self.spec.json;
        glib::spawn_future_local(async move {
            while let Ok(msg) = rx.recv().await {
                let Some(i) = w.upgrade() else { return };
                if let Ok(pid) = prx.try_recv() {
                    i.child.set(Some(pid));
                }
                match msg {
                    Some(line) => i.apply(parse(&line, json)),
                    None => i.slot.set_visible(false),
                }
            }
        });
    }
}

impl Module for Custom {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        if self.0.spec.interval > 0.0 {
            self.0.run();
        }
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

/// Red mark shown when config.toml or style.css has an error.
pub struct ErrorMark(Slot);

impl ErrorMark {
    pub fn new(err: &str, ctx: &Ctx) -> ErrorMark {
        let s = ctx.slot("error", &["crit"]);
        s.set_markup("");
        let e = err.to_string();
        s.tooltip(move || {
            let sh = ui::sheet();
            sh.append(&ui::head("vector bar", "config error").0);
            let l = ui::text(&e, &["crit"]);
            l.set_wrap(true);
            l.set_max_width_chars(60);
            l.set_margin_top(6);
            sh.append(&l);
            sh.append(&ui::thint("fix the file; the bar reloads by itself  ·  click opens it"));
            Some(sh.upcast())
        });
        s.on_left(|| util::spawn(&["xdg-open", &config::path().to_string_lossy()]));
        ErrorMark(s)
    }
}

impl Module for ErrorMark {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.view()
    }
}
