//! Tooltips and panels described as data (from Lua plugins or script JSON),
//! drawn with the same widgets as the built-in modules.
//!
//! ```json
//! { "title": "printer", "detail": "idle",
//!   "items": [
//!     {"section": "ink"},
//!     {"meter": ["black", 0.42, "42%", "about 300 pages"]},
//!     {"kv": ["queue", "empty"]},
//!     {"row": {"icon": "", "name": "print test page", "meta": "", "run": "lp test.pdf", "active": false}},
//!     {"text": "anything", "class": "dim"},
//!     {"buttons": [["pause", "lpadmin -p x -r"], ["resume", {"call": "resume"}]]},
//!     {"hint": "click opens cups"} ] }
//! ```
//! `run` is a shell command; `{"call": "name"}` calls the plugin's action `name`.

use std::rc::Rc;

use gtk::prelude::*;
use serde_json::Value;

use crate::panel::Card;
use crate::ui::{self, Tap};
use crate::util;

/// What a clickable item does.
pub type Call = Rc<dyn Fn(&str)>;

fn action(v: &Value, call: &Call) -> Option<Box<dyn Fn()>> {
    match v {
        Value::String(cmd) => {
            let cmd = cmd.clone();
            Some(Box::new(move || util::spawn_sh(&cmd)))
        }
        Value::Object(o) => {
            let name = o.get("call")?.as_str()?.to_string();
            let call = call.clone();
            Some(Box::new(move || call(&name)))
        }
        _ => None,
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        x => x.to_string(),
    }
}

/// Appends the items to `b`. `tooltip`: tooltip variants (no panel padding).
pub fn items(b: &gtk::Box, items: &Value, call: &Call, tooltip: bool) {
    let Some(arr) = items.as_array() else { return };
    for it in arr {
        if let Some(t) = it.get("section") {
            b.append(&if tooltip {
                ui::tsection(&s(t))
            } else {
                ui::section(&s(t))
            });
        } else if let Some(kv) = it.get("kv").and_then(Value::as_array) {
            let (k, v) = (
                kv.first().map(s).unwrap_or_default(),
                kv.get(1).map(s).unwrap_or_default(),
            );
            b.append(&if tooltip {
                ui::tkv(&k, &v)
            } else {
                ui::kv(&k, &v).0
            });
        } else if let Some(m) = it.get("meter").and_then(Value::as_array) {
            let k = m.first().map(s).unwrap_or_default();
            let f = m.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            let v = m
                .get(2)
                .map(s)
                .unwrap_or_else(|| format!("{:.0}%", f * 100.0));
            let d = m.get(3).map(s).unwrap_or_default();
            let cls = it
                .get("class")
                .map(s)
                .unwrap_or_else(|| ui::pct_class(f * 100.0).to_string());
            b.append(&if tooltip {
                ui::tmeter(&k, f, &v, &d, &cls)
            } else {
                ui::meter_row(&k, f, &v, &d, &cls)
            });
        } else if let Some(r) = it.get("row") {
            let row = ui::Row::new(&s(&r["icon"]), &s(&r["name"]), &s(&r["meta"]));
            row.set_active(r["active"] == true);
            if let Some(f) = action(&r["run"], call) {
                ui::on_click(&row.root, f);
            }
            b.append(&row.root);
        } else if let Some(t) = it.get("text") {
            let cls = it.get("class").map(s).unwrap_or_default();
            let l = ui::label(&s(t), &[if tooltip { "" } else { "msg" }, &cls]);
            l.set_wrap(true);
            l.set_max_width_chars(56);
            b.append(&l);
        } else if let Some(bs) = it.get("buttons").and_then(Value::as_array) {
            let row = ui::hbox(4, &["chips"]);
            for bt in bs {
                let Some(pair) = bt.as_array() else { continue };
                let label = pair.first().map(s).unwrap_or_default();
                let on = pair.get(2).is_some_and(|v| v == true);
                if let Some(f) = pair.get(1).and_then(|v| action(v, call)) {
                    row.append(&ui::chip(&label, on, f).tap(|c| c.set_margin_end(4)));
                }
            }
            b.append(&row);
        } else if let Some(h) = it.get("hint") {
            b.append(&if tooltip {
                ui::thint(&s(h))
            } else {
                ui::hint(&s(h))
            });
        }
    }
}

/// A tooltip from a sheet value.
pub fn tooltip(v: &Value, call: &Call) -> Option<gtk::Widget> {
    if let Value::String(t) = v {
        if t.is_empty() {
            return None;
        }
        // plain Waybar-style markup tooltip
        let sh = ui::sheet();
        sh.append(&ui::label(t, &[]).tap(|l| l.set_margin_top(10)));
        return Some(sh.upcast());
    }
    if !v.is_object() {
        return None;
    }
    let sh = ui::sheet();
    sh.append(&ui::head(&s(&v["title"]), &s(&v["detail"])).0);
    items(&sh, &v["items"], call, true);
    Some(sh.upcast())
}

/// A panel card from a sheet value; returns the card so it can be refilled.
pub fn card(v: &Value, call: &Call) -> Card {
    let c = Card::new(&s(&v["title"]), &s(&v["detail"]));
    items(&c.body, &v["items"], call, false);
    c
}

/// Refills an existing card in place (live panels).
pub fn refill(c: &Card, v: &Value, call: &Call) {
    c.set_detail(&s(&v["detail"]));
    ui::clear(&c.body);
    items(&c.body, &v["items"], call, false);
}
