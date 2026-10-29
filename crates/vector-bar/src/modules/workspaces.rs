//! Hyprland workspaces, event driven. Persistent 1..N, plus any other
//! existing workspace; active shows the dot glyph, empty ones are dimmed,
//! urgent ones tinted, the special (scratchpad) workspace gets a mark while open.
//! Workspace 10 ("0") is the AI workspace (`aiws`): it lives on a headless
//! output, so a click pulls it onto this screen instead of switching to it.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::hypr;
use crate::ui;
use crate::util;

const AI_WS: i64 = 10;

#[derive(Default, Clone, PartialEq)]
struct Snapshot {
    /// id -> (name, monitor, windows)
    spaces: BTreeMap<i64, (String, String, i64)>,
    /// monitor -> active workspace id
    active: BTreeMap<String, i64>,
    /// monitor -> open special workspace name
    special: BTreeMap<String, String>,
    focused: String,
}

struct Inner {
    persistent: i64,
    active_glyph: String,
    per_monitor: bool,
    labels: BTreeMap<i64, String>,
    snap: RefCell<Snapshot>,
    urgent: RefCell<HashSet<i64>>,
    views: RefCell<Vec<(String, glib::WeakRef<gtk::Box>)>>,
    pending: std::cell::Cell<bool>,
}

pub struct Workspaces(Rc<Inner>, #[allow(dead_code)] hypr::Events);

impl Workspaces {
    pub fn new(ctx: &Ctx) -> Workspaces {
        let o = ctx.opts("workspaces");
        let mut labels = BTreeMap::new();
        labels.insert(10, "0".to_string());
        if let Some(t) = ctx
            .cfg
            .modules
            .get("workspaces")
            .and_then(|t| t.get("names"))
            .and_then(|v| v.as_table())
        {
            for (k, v) in t {
                if let (Ok(id), Some(s)) = (k.parse::<i64>(), v.as_str()) {
                    labels.insert(id, s.to_string());
                }
            }
        }
        let inner = Rc::new(Inner {
            persistent: o.num("persistent", 9.0) as i64,
            active_glyph: o.str("active", ""),
            per_monitor: o.bool("per_monitor", false),
            labels,
            snap: RefCell::default(),
            urgent: RefCell::default(),
            views: RefCell::default(),
            pending: std::cell::Cell::new(false),
        });
        inner.query();
        let w = Rc::downgrade(&inner);
        // the event socket is shared by every module instance through this closure
        let ev = hypr::events(move |name, data| {
            let Some(i) = w.upgrade() else { return false };
            match name {
                "urgent" => {
                    // data = window address; find its workspace
                    let addr = format!("0x{data}");
                    let i2 = i.clone();
                    util::bg(
                        move || {
                            hypr::json("clients").and_then(|c| {
                                c.as_array()?
                                    .iter()
                                    .find(|x| x["address"] == addr.as_str())?["workspace"]["id"]
                                    .as_i64()
                            })
                        },
                        move |id| {
                            if let Some(id) = id {
                                i2.urgent.borrow_mut().insert(id);
                                i2.render();
                            }
                        },
                    );
                }
                "workspace" | "workspacev2" | "focusedmon" | "focusedmonv2" | "createworkspace"
                | "createworkspacev2" | "destroyworkspace" | "destroyworkspacev2"
                | "moveworkspace" | "moveworkspacev2" | "renameworkspace" | "openwindow"
                | "closewindow" | "movewindow" | "movewindowv2" | "activespecial"
                | "activespecialv2" | "monitoradded" | "monitorremoved" => {
                    i.schedule();
                }
                _ => {}
            }
            true
        });
        // safety net in case an event was missed
        util::every(5000, &inner, |i| i.query());
        Workspaces(inner, ev)
    }
}

impl Inner {
    fn schedule(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let me = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(15), move || {
            me.pending.set(false);
            me.query();
        });
    }

    fn query(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        util::bg(
            || {
                let mut s = Snapshot::default();
                for w in hypr::json("workspaces")?.as_array()? {
                    let id = w["id"].as_i64()?;
                    s.spaces.insert(
                        id,
                        (
                            w["name"].as_str().unwrap_or("").to_string(),
                            w["monitor"].as_str().unwrap_or("").to_string(),
                            w["windows"].as_i64().unwrap_or(0),
                        ),
                    );
                }
                for m in hypr::json("monitors")?.as_array()? {
                    let name = m["name"].as_str().unwrap_or("").to_string();
                    if m["focused"].as_bool() == Some(true) {
                        s.focused = name.clone();
                    }
                    if let Some(id) = m["activeWorkspace"]["id"].as_i64() {
                        s.active.insert(name.clone(), id);
                    }
                    let sp = m["specialWorkspace"]["name"].as_str().unwrap_or("");
                    if !sp.is_empty() {
                        s.special
                            .insert(name, sp.trim_start_matches("special:").to_string());
                    }
                }
                Some(s)
            },
            move |s| {
                let (Some(me), Some(s)) = (me.upgrade(), s) else {
                    return;
                };
                // visiting a workspace clears its urgency
                {
                    let mut u = me.urgent.borrow_mut();
                    for id in s.active.values() {
                        u.remove(id);
                    }
                }
                if *me.snap.borrow() != s {
                    *me.snap.borrow_mut() = s;
                }
                me.render();
            },
        );
    }

    fn render(&self) {
        let views: Vec<_> = {
            let mut v = self.views.borrow_mut();
            v.retain(|(_, w)| w.upgrade().is_some());
            v.iter()
                .filter_map(|(m, w)| w.upgrade().map(|b| (m.clone(), b)))
                .collect()
        };
        for (mon, b) in views {
            self.render_into(&mon, &b);
        }
    }

    fn render_into(&self, mon: &str, b: &gtk::Box) {
        let s = self.snap.borrow();
        let urgent = self.urgent.borrow();
        let mut ids: Vec<i64> = (1..=self.persistent).collect();
        for (id, (_, m, _)) in &s.spaces {
            if *id > 0 && !ids.contains(id) && (!self.per_monitor || m == mon) {
                ids.push(*id);
            }
        }
        ids.sort();
        let active = s
            .active
            .get(mon)
            .copied()
            .or_else(|| s.active.get(&s.focused).copied());
        let special = s.special.get(mon).cloned();

        // reuse buttons when the id list is unchanged (no flicker, hover survives)
        let mut existing: Vec<gtk::Widget> = Vec::new();
        let mut c = b.first_child();
        while let Some(ch) = c {
            c = ch.next_sibling();
            existing.push(ch);
        }
        // rebuild when the id list changes, not only its length: each button's
        // click is bound to its id
        let want: Vec<String> = ids
            .iter()
            .map(|i| i.to_string())
            .chain(special.iter().map(|_| "special".to_string()))
            .collect();
        let have: Vec<String> = existing.iter().map(|w| w.widget_name().to_string()).collect();
        if have != want {
            ui::clear(b);
            existing.clear();
            for id in &ids {
                let btn = ui::hbox(0, &["ws"]);
                btn.set_widget_name(&id.to_string());
                let l = gtk::Label::new(None);
                btn.append(&l);
                let id = *id;
                if id == AI_WS {
                    btn.add_css_class("ai");
                    ui::on_click(&btn, || util::spawn(&["aiws", "show"]));
                } else {
                    ui::on_click(&btn, move || hypr::dispatch(&format!("workspace {id}")));
                }
                b.append(&btn);
                existing.push(btn.upcast());
            }
            if special.is_some() {
                let btn = ui::hbox(0, &["ws", "special"]);
                btn.set_widget_name("special");
                btn.append(&gtk::Label::new(None));
                ui::on_click(&btn, move || hypr::dispatch("togglespecialworkspace"));
                b.append(&btn);
                existing.push(btn.upcast());
            }
        }
        for (w, id) in existing.iter().zip(ids.iter()) {
            let Some(l) = w.first_child().and_downcast::<gtk::Label>() else {
                continue;
            };
            let is_active = Some(*id) == active;
            let windows = s.spaces.get(id).map(|x| x.2).unwrap_or(0);
            let name = self
                .labels
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string());
            l.set_text(if is_active && !self.active_glyph.is_empty() {
                &self.active_glyph
            } else {
                &name
            });
            set(w, "active", is_active);
            set(w, "empty", windows == 0 && !is_active);
            set(w, "urgent", urgent.contains(id));
            let what = if *id == AI_WS { "AI workspace" } else { "workspace" };
            let tip = match s.spaces.get(id) {
                Some((_, _, n)) if *n > 0 => format!(
                    "{what} {name} · {n} window{}",
                    if *n == 1 { "" } else { "s" }
                ),
                _ => format!("{what} {name} · empty"),
            };
            w.set_tooltip_text(Some(&tip));
        }
        if let (Some(sp), Some(w)) = (&special, existing.get(ids.len())) {
            if let Some(l) = w.first_child().and_downcast::<gtk::Label>() {
                l.set_text("󰘔");
            }
            w.set_tooltip_text(Some(&format!("special workspace {sp} · click to hide")));
        }
    }
}

fn set(w: &gtk::Widget, class: &str, on: bool) {
    if on {
        w.add_css_class(class)
    } else {
        w.remove_css_class(class)
    }
}

impl Module for Workspaces {
    fn view(&self, bar: &BarInfo) -> gtk::Widget {
        let b = ui::hbox(0, &["workspaces"]);
        // scroll switches workspaces on this monitor
        let sc = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
        );
        sc.connect_scroll(|_, _, dy| {
            hypr::dispatch(if dy > 0.0 {
                "workspace m+1"
            } else {
                "workspace m-1"
            });
            glib::Propagation::Stop
        });
        b.add_controller(sc);
        self.0.render_into(&bar.connector, &b);
        self.0
            .views
            .borrow_mut()
            .push((bar.connector.clone(), b.downgrade()));
        b.upcast()
    }
    fn refresh(&self) {
        self.0.query();
    }
}
