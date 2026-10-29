//! Lua plugins: `~/.config/vector-bar/init.lua`, then `plugins/*.lua` in name order
//! (same layout as Vector Find). Scripts get a `bar` table:
//!
//! ```lua
//! bar.module("profile", {
//!   interval = 2,                         -- seconds between update() calls
//!   update = function()                   -- returns what the item shows
//!     local p = bar.read("/sys/firmware/acpi/platform_profile") or "?"
//!     return {
//!       text = "󰾅 " .. p,                  -- Pango markup
//!       class = p == "performance" and "warn" or nil,   -- dim / faint / warn / crit
//!       visible = true,
//!       tooltip = { title = "profile", detail = p, items = { {kv = {"mode", p}} } },
//!       panel = { title = "profile", items = {
//!         {buttons = { {"quiet", {call = "set_quiet"}}, {"next", "asusctl profile next"} }} } },
//!     }
//!   end,
//!   on_click = "asusctl profile next",    -- a shell command or a function
//!   on_right = function() bar.notify("profile", "right click") end,
//!   actions = { set_quiet = function() bar.spawn("asusctl profile set Quiet") end },
//! })
//!
//! bar.sh(cmd)            -> stdout, exit code   (waits up to 3 s: keep it quick)
//! bar.async(cmd, fn)     runs cmd in the background, then fn(stdout, code)
//! bar.spawn(cmd)         fire and forget
//! bar.read(path)         file contents or nil
//! bar.json(text)         decoded table (nil when invalid)
//! bar.notify(title, body)
//! bar.refresh(name)      run a module's update now
//! bar.home               $HOME
//! ```
//! With a `panel`, a left click opens it (unless `on_click` is set; then the
//! right click does). Sheets use the format in `sheet.rs`.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use mlua::{Function, Lua, LuaSerdeExt, RegistryKey, Table, Value};

use crate::modules::{BarInfo, Ctx, Module};
use crate::panel::Panel;
use crate::sheet;
use crate::slot::{Notify, Slot};
use crate::ui;
use crate::util;

pub struct Runtime {
    lua: Lua,
    modules: RefCell<Vec<(String, RegistryKey)>>,
    live: RefCell<Vec<(String, Weak<Inner>)>>,
    pub errors: RefCell<Vec<String>>,
}

thread_local! {
    static RT: RefCell<Weak<Runtime>> = RefCell::new(Weak::new());
}

fn rt() -> Option<Rc<Runtime>> {
    RT.with(|r| r.borrow().upgrade())
}

fn err(e: mlua::Error) -> String {
    match e {
        mlua::Error::CallbackError { cause, .. } => cause.to_string(),
        e => e.to_string(),
    }
}

/// Loads every plugin file; `None` when there is no init.lua or plugins dir.
pub fn load() -> Option<Rc<Runtime>> {
    let dir = util::config_dir().join("vector-bar");
    let mut files = Vec::new();
    if dir.join("init.lua").exists() {
        files.push(dir.join("init.lua"));
    }
    if let Ok(rd) = std::fs::read_dir(dir.join("plugins")) {
        let mut p: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "lua"))
            .collect();
        p.sort();
        files.extend(p);
    }
    if files.is_empty() {
        return None;
    }
    let lua = Lua::new();
    let mut errors = Vec::new();
    if let Err(e) = install(&lua) {
        errors.push(format!("lua: {}", err(e)));
    }
    let rt = Rc::new(Runtime {
        lua,
        modules: RefCell::default(),
        live: RefCell::default(),
        errors: RefCell::default(),
    });
    RT.with(|r| *r.borrow_mut() = Rc::downgrade(&rt));
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        match std::fs::read_to_string(&f) {
            Ok(code) => {
                if let Err(e) = rt.lua.load(&code).set_name(&name).exec() {
                    errors.push(format!("{name}: {}", err(e)));
                }
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    // registrations made while the files ran
    let regs: Vec<(String, RegistryKey)> = PENDING.with(|p| p.borrow_mut().drain(..).collect());
    rt.modules.borrow_mut().extend(regs);
    *rt.errors.borrow_mut() = errors;
    Some(rt)
}

thread_local! {
    static PENDING: RefCell<Vec<(String, RegistryKey)>> = const { RefCell::new(Vec::new()) };
}

fn install(lua: &Lua) -> mlua::Result<()> {
    let bar = lua.create_table()?;
    bar.set("home", util::home().to_string_lossy().to_string())?;
    bar.set(
        "module",
        lua.create_function(|lua, (name, spec): (String, Table)| {
            let key = lua.create_registry_value(spec)?;
            PENDING.with(|p| p.borrow_mut().push((name, key)));
            Ok(())
        })?,
    )?;
    bar.set(
        "sh",
        lua.create_function(|_, cmd: String| {
            let o = util::run(&["sh", "-c", &cmd], Duration::from_secs(3));
            Ok(match o {
                Some(o) => (o.stdout, if o.ok { 0 } else { 1 }),
                None => (String::new(), -1),
            })
        })?,
    )?;
    bar.set(
        "async",
        lua.create_function(|lua, (cmd, f): (String, Function)| {
            let key = lua.create_registry_value(f)?;
            util::bg(
                move || util::run(&["sh", "-c", &cmd], Duration::from_secs(60)),
                move |o| {
                    let Some(rt) = rt() else { return };
                    if let Ok(f) = rt.lua.registry_value::<Function>(&key) {
                        let (out, code) = match o {
                            Some(o) => (o.stdout, if o.ok { 0 } else { 1 }),
                            None => (String::new(), -1),
                        };
                        if let Err(e) = f.call::<()>((out, code)) {
                            eprintln!("vector-bar: lua async: {}", err(e));
                        }
                    }
                    let _ = rt.lua.remove_registry_value(key);
                },
            );
            Ok(())
        })?,
    )?;
    bar.set(
        "spawn",
        lua.create_function(|_, cmd: String| Ok(util::spawn_sh(&cmd)))?,
    )?;
    bar.set(
        "read",
        lua.create_function(|_, p: String| Ok(util::read(util::expand(&p))))?,
    )?;
    bar.set(
        "json",
        lua.create_function(|lua, t: String| {
            match serde_json::from_str::<serde_json::Value>(&t) {
                Ok(v) => lua.to_value(&v),
                Err(_) => Ok(Value::Nil),
            }
        })?,
    )?;
    bar.set(
        "notify",
        lua.create_function(|_, (t, b): (String, Option<String>)| {
            util::spawn(&[
                "notify-send",
                "-a",
                "vector-bar",
                &t,
                &b.unwrap_or_default(),
            ]);
            Ok(())
        })?,
    )?;
    bar.set(
        "refresh",
        lua.create_function(|_, name: String| {
            if let Some(rt) = rt() {
                let live: Vec<_> = rt
                    .live
                    .borrow()
                    .iter()
                    .filter(|(n, _)| *n == name)
                    .filter_map(|(_, w)| w.upgrade())
                    .collect();
                glib::idle_add_local_once(move || {
                    for i in live {
                        i.update();
                    }
                });
            }
            Ok(())
        })?,
    )?;
    lua.globals().set("bar", bar)?;
    Ok(())
}

impl Runtime {
    pub fn has(&self, name: &str) -> bool {
        self.modules.borrow().iter().any(|(n, _)| n == name)
    }

    pub fn build(self: &Rc<Self>, name: &str, ctx: &Ctx) -> Option<Rc<dyn Module>> {
        let spec: Table = {
            let m = self.modules.borrow();
            let (_, key) = m.iter().find(|(n, _)| n == name)?;
            self.lua.registry_value(key).ok()?
        };
        let inner = Rc::new(Inner {
            name: name.to_string(),
            slot: ctx.slot(name, &[]),
            spec,
            last: RefCell::new(serde_json::Value::Null),
            changed: Notify::default(),
            rt: self.clone(),
        });
        self.live
            .borrow_mut()
            .push((name.to_string(), Rc::downgrade(&inner)));
        // first result first: it may carry a panel that wire() hooks up
        inner.update();
        inner.wire();
        let secs: f64 = inner.spec.get("interval").unwrap_or(5.0);
        if secs > 0.0 {
            util::every((secs * 1000.0) as u64, &inner, |i| i.update());
        }
        Some(Rc::new(LuaModule(inner)))
    }
}

struct Inner {
    name: String,
    slot: Slot,
    spec: Table,
    last: RefCell<serde_json::Value>,
    changed: Notify,
    /// keeps the Lua state alive as long as the module lives
    rt: Rc<Runtime>,
}

impl Inner {
    fn click(&self, field: &str) -> Option<Box<dyn Fn()>> {
        match self.spec.get::<Value>(field).ok()? {
            Value::String(s) => {
                let cmd = s.to_str().ok()?.to_string();
                Some(Box::new(move || util::spawn_sh(&cmd)))
            }
            Value::Function(f) => {
                let name = self.name.clone();
                Some(Box::new(move || {
                    if let Err(e) = f.call::<()>(()) {
                        eprintln!("vector-bar: {name}: {}", err(e));
                    }
                }))
            }
            _ => None,
        }
    }

    fn call_action(&self, action: &str) {
        let f = self
            .spec
            .get::<Table>("actions")
            .ok()
            .and_then(|t| t.get::<Function>(action).ok());
        match f {
            Some(f) => {
                if let Err(e) = f.call::<()>(()) {
                    eprintln!("vector-bar: {}: action {action}: {}", self.name, err(e));
                }
            }
            None => eprintln!("vector-bar: {}: no action {action}", self.name),
        }
    }

    fn wire(self: &Rc<Self>) {
        let has_panel = self.spec.contains_key("panel").unwrap_or(false);
        let left = self.click("on_click");
        let left_set = left.is_some();
        if let Some(f) = left {
            self.slot.on_left(f);
        }
        if let Some(f) = self.click("on_middle") {
            self.slot.on_middle(f);
        }
        match self.click("on_right") {
            Some(f) => self.slot.on_right(f),
            None if has_panel && left_set => {
                let w = Rc::downgrade(self);
                self.slot.on_right(move || {
                    if let Some(i) = w.upgrade() {
                        i.slot.toggle_panel(None, false);
                    }
                });
            }
            None => {}
        }
        if let (Some(u), Some(d)) = (self.click("on_scroll_up"), self.click("on_scroll_down")) {
            self.slot.on_scroll(u, d);
        }
        let w = Rc::downgrade(self);
        self.slot.tooltip(move || {
            let i = w.upgrade()?;
            let v = i.last.borrow()["tooltip"].clone();
            sheet::tooltip(&v, &i.caller())
        });
        // a panel: from update()'s result, or a `panel` function called on open
        if self.last.borrow()["panel"].is_object() || has_panel {
            let w = Rc::downgrade(self);
            self.slot.panel(move |p| match w.upgrade() {
                Some(i) => i.panel(p),
                None => ui::vbox(0, &[]).upcast(),
            });
        }
    }

    fn caller(self: &Rc<Self>) -> sheet::Call {
        let w = Rc::downgrade(self);
        Rc::new(move |a: &str| {
            if let Some(i) = w.upgrade() {
                i.call_action(a);
                i.update();
            }
        })
    }

    fn panel_value(&self) -> serde_json::Value {
        match self.spec.get::<Value>("panel") {
            Ok(Value::Function(f)) => match f.call::<Value>(()) {
                Ok(v) => self.rt.lua.from_value(v).unwrap_or_default(),
                Err(e) => {
                    serde_json::json!({"title": self.name, "items": [{"text": err(e), "class": "crit"}]})
                }
            },
            _ => self.last.borrow()["panel"].clone(),
        }
    }

    fn panel(self: &Rc<Self>, p: &Panel) -> gtk::Widget {
        let v = self.panel_value();
        if let Some(w) = v["width"].as_i64() {
            p.set_width(w as i32);
        }
        let card = Rc::new(sheet::card(&v, &self.caller()));
        let w = Rc::downgrade(self);
        let c2 = card.clone();
        p.watch(&self.changed, move || {
            if let Some(i) = w.upgrade() {
                sheet::refill(&c2, &i.panel_value(), &i.caller());
            }
        });
        card.root.clone().upcast()
    }

    fn update(self: &Rc<Self>) {
        let Ok(f) = self.spec.get::<Function>("update") else {
            return;
        };
        match f
            .call::<Value>(())
            .and_then(|v| self.rt.lua.from_value::<serde_json::Value>(v))
        {
            Ok(v) => {
                let text = match &v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v["text"].as_str().unwrap_or("").to_string(),
                };
                self.slot.set_markup(&text);
                let cls = v["class"].as_str().unwrap_or("");
                self.slot.set_classes(&[cls]);
                self.slot
                    .set_visible(v["visible"].as_bool().unwrap_or(!text.is_empty()));
                *self.last.borrow_mut() = v;
                self.slot.bump();
                self.changed.emit();
            }
            Err(e) => {
                let msg = err(e);
                self.slot.set_markup("");
                self.slot.set_classes(&["crit"]);
                self.slot.set_visible(true);
                *self.last.borrow_mut() = serde_json::json!({"tooltip": {"title": self.name, "detail": "plugin error", "items": [{"text": msg, "class": "crit"}]}});
                self.slot.bump();
            }
        }
    }
}

struct LuaModule(Rc<Inner>);

impl Module for LuaModule {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.update();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}
