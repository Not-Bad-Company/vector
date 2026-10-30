//! PipeWire sound controls through pipewire-pulse's pactl interface.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::Value;

use crate::page::{Ctx, Frame, View};
use crate::ui::{self, Keyed, Row};

#[derive(Clone, Default)]
struct Port {
    name: String,
    description: String,
}

#[derive(Clone, Default)]
struct AudioNode {
    id: String,
    name: String,
    description: String,
    volume: f64,
    muted: bool,
    ports: Vec<Port>,
    active_port: String,
    app: String,
    device: String,
}

#[derive(Clone, Default)]
struct Card {
    id: String,
    name: String,
    active: String,
    profiles: Vec<(String, String)>,
}

#[derive(Clone, Default)]
struct State {
    sinks: Vec<AudioNode>,
    sources: Vec<AudioNode>,
    apps: Vec<AudioNode>,
    cards: Vec<Card>,
    source_users: HashMap<String, String>,
    default_sink: String,
    default_source: String,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
fn value_text(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(|x| x.as_str().map(str::to_string).or_else(|| x.as_i64().map(|n| n.to_string())))
        .unwrap_or_default()
}
/// Average of the channels, in percent (pactl JSON: {"front-left": {"value": 28382, ...}, ...}).
fn volume(v: &Value) -> f64 {
    let Some(ch) = v.get("volume").and_then(Value::as_object) else { return 0.0 };
    let vals: Vec<f64> = ch.values().filter_map(|c| c.get("value").and_then(Value::as_f64)).collect();
    if vals.is_empty() {
        return 0.0;
    }
    (vals.iter().sum::<f64>() / vals.len() as f64 * 100.0 / 65536.0).round()
}
fn ports(v: &Value) -> (Vec<Port>, String) {
    let xs = v
        .get("ports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let active = s(v, "active_port");
    let mut out = Vec::new();
    for p in xs {
        if p.get("available")
            .and_then(Value::as_str)
            .is_some_and(|a| a == "no")
        {
            continue;
        }
        out.push(Port {
            name: s(&p, "name"),
            description: {
                let d = s(&p, "description");
                if d.is_empty() { s(&p, "name") } else { d }
            },
        });
    }
    (out, active)
}
fn nodes(raw: &str, kind: &str) -> Vec<AudioNode> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let (ps, active) = ports(v);
            let props = v.get("properties").unwrap_or(&Value::Null);
            let app = [
                s(props, "application.name"),
                s(props, "media.name"),
                s(props, "application.process.binary"),
            ]
            .into_iter()
            .find(|x| !x.is_empty())
            .unwrap_or_else(|| "unknown app".into());
            AudioNode {
                id: v.get("index").map(Value::to_string).unwrap_or_default(),
                name: s(v, "name"),
                description: {
                    let d = s(v, "description");
                    if d.is_empty() { s(v, "name") } else { d }
                },
                volume: volume(v),
                muted: v.get("mute").and_then(Value::as_bool).unwrap_or(false),
                ports: ps,
                active_port: active,
                app,
                device: value_text(v, "sink"),
            }
        })
        .filter(|n| !n.id.is_empty() && (kind != "source" || !n.name.ends_with(".monitor")))
        .collect()
}

async fn command(args: &[&str]) -> (bool, String, String) {
    ui::run(args.iter().map(|s| s.to_string()).collect()).await
}
// All pactl commands run off GTK's main thread through ui::run.
async fn get_state() -> State {
    let (ok, out, _) = command(&["pactl", "-f", "json", "list", "sinks"]).await;
    let sinks = if ok { nodes(&out, "sink") } else { Vec::new() };
    let (_, out, _) = command(&["pactl", "-f", "json", "list", "sources"]).await;
    let sources = nodes(&out, "source");
    let (_, out, _) = command(&["pactl", "-f", "json", "list", "sink-inputs"]).await;
    let apps = nodes(&out, "app");
    let (_, out, _) = command(&["pactl", "-f", "json", "list", "source-outputs"]).await;
    let source_users = serde_json::from_str::<Value>(&out)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .fold(HashMap::new(), |mut m, v| {
            let source = v.get("source").map(Value::to_string).unwrap_or_default();
            let props = v.get("properties").unwrap_or(&Value::Null);
            let app = [s(props, "application.name"), s(props, "media.name")]
                .into_iter()
                .find(|x| !x.is_empty())
                .unwrap_or_else(|| "unknown app".into());
            m.entry(source)
                .and_modify(|x: &mut String| {
                    x.push_str(", ");
                    x.push_str(&app);
                })
                .or_insert(app);
            m
        });
    let (_, out, _) = command(&["pactl", "-f", "json", "list", "cards"]).await;
    let cards = serde_json::from_str::<Value>(&out)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|v| {
            let profiles = v
                .get("profiles")
                .and_then(Value::as_object)
                .map(|ps| {
                    ps.iter()
                        .filter_map(|(k, p)| {
                            let available = p
                                .get("available")
                                .and_then(Value::as_bool)
                                .or_else(|| {
                                    p.get("available")
                                        .and_then(Value::as_str)
                                        .map(|x| x == "yes")
                                })
                                .unwrap_or(true);
                            available.then(|| {
                                (k.clone(), {
                                    let d = s(p, "description");
                                    if d.is_empty() { k.clone() } else { d }
                                })
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Card {
                id: s(v, "name"),
                name: {
                    let d = s(v, "description");
                    if d.is_empty() { s(v, "name") } else { d }
                },
                active: s(v, "active_profile"),
                profiles,
            }
        })
        .collect();
    let (_, default_sink, _) = command(&["pactl", "get-default-sink"]).await;
    let (_, default_source, _) = command(&["pactl", "get-default-source"]).await;
    State {
        sinks,
        sources,
        apps,
        cards,
        source_users,
        default_sink: default_sink.trim().into(),
        default_source: default_source.trim().into(),
    }
}

struct Sound {
    frame: Frame,
    state: RefCell<State>,
    outputs: Keyed,
    inputs: Keyed,
    apps: Keyed,
    cards: Keyed,
    busy: RefCell<HashSet<String>>,
    errors: RefCell<HashMap<String, String>>,
    dragging: RefCell<HashSet<String>>,
    pending_volume: RefCell<HashMap<String, (String, f64)>>,
    volume_timer: Cell<bool>,
    refresh: Cell<bool>,
    sub: RefCell<Option<gio::Subprocess>>,
    sub_stream: RefCell<Option<gio::DataInputStream>>,
}

pub fn page(_: &Ctx) -> View {
    let frame = Frame::new("sound");
    let out = Keyed::new(&["group"]);
    let input = Keyed::new(&["group"]);
    let apps = Keyed::new(&["group"]);
    let cards = Keyed::new(&["group"]);
    frame.body.append(&ui::section("output"));
    frame.body.append(&out.root);
    frame.body.append(&ui::section("input"));
    frame.body.append(&input.root);
    frame.body.append(&ui::section("apps"));
    frame.body.append(&apps.root);
    frame.body.append(&ui::section("devices"));
    frame.body.append(&cards.root);
    let w = Rc::new(Sound {
        frame,
        state: RefCell::default(),
        outputs: out,
        inputs: input,
        apps,
        cards,
        busy: RefCell::default(),
        errors: RefCell::default(),
        dragging: RefCell::default(),
        pending_volume: RefCell::default(),
        volume_timer: Cell::new(false),
        refresh: Cell::new(false),
        sub: RefCell::default(),
        sub_stream: RefCell::default(),
    });
    let (root, a, b, c, d) = (
        w.frame.root.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
        w.clone(),
    );
    View::new(&root)
        .on_show(move || a.start())
        .on_hide(move || b.pause())
        .on_stop(move || c.pause())
        .every(5000, move || d.query())
}

impl Sound {
    fn query(self: &Rc<Self>) {
        let wk = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let state = get_state().await;
            if let Some(w) = wk.upgrade() {
                *w.state.borrow_mut() = state;
                w.draw();
            }
        });
    }
    fn start(self: &Rc<Self>) {
        self.query();
        self.subscribe();
    }
    fn pause(&self) {
        if let Some(p) = self.sub.borrow_mut().take() {
            p.force_exit();
        }
        self.sub_stream.borrow_mut().take();
    }
    fn subscribe(self: &Rc<Self>) {
        if self.sub.borrow().is_some() {
            return;
        }
        let argv: [&std::ffi::OsStr; 2] = [
            std::ffi::OsStr::new("pactl"),
            std::ffi::OsStr::new("subscribe"),
        ];
        let Ok(p) = gio::Subprocess::newv(
            &argv,
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
        ) else {
            return;
        };
        let Some(out) = p.stdout_pipe() else { return };
        let stream = gio::DataInputStream::new(&out);
        *self.sub.borrow_mut() = Some(p);
        *self.sub_stream.borrow_mut() = Some(stream.clone());
        let wk = Rc::downgrade(self);
        Self::read_event(stream, wk);
    }
    fn read_event(stream: gio::DataInputStream, wk: std::rc::Weak<Self>) {
        stream.read_line_async(glib::Priority::DEFAULT, gio::Cancellable::NONE, move |r| {
            let Ok(Some(line)) = r else { return };
            if !line.is_empty() {
                if let Some(w) = wk.upgrade() {
                    w.schedule();
                }
            }
            if let Some(w) = wk.upgrade() {
                if let Some(s) = w.sub_stream.borrow().clone() {
                    Self::read_event(s, wk);
                }
            }
        });
    }
    fn schedule(self: &Rc<Self>) {
        if self.refresh.replace(true) {
            return;
        }
        let wk = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(100), move || {
            if let Some(w) = wk.upgrade() {
                w.refresh.set(false);
                w.query();
            }
        });
    }
    fn run(self: &Rc<Self>, key: String, args: Vec<String>) {
        if !self.busy.borrow_mut().insert(key.clone()) {
            return;
        }
        self.draw();
        let wk = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (ok, _, err) = ui::run(args).await;
            if let Some(w) = wk.upgrade() {
                w.busy.borrow_mut().remove(&key);
                if !ok {
                    w.errors.borrow_mut().insert(key, err.trim().to_string());
                } else {
                    w.errors.borrow_mut().remove(&key);
                }
                w.query();
            }
        });
    }
    fn set_volume(self: &Rc<Self>, id: &str, v: f64, target: &str) {
        self.pending_volume
            .borrow_mut()
            .insert(id.into(), (target.to_string(), v));
        if self.volume_timer.replace(true) {
            return;
        }
        let wk = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(33), move || {
            if let Some(w) = wk.upgrade() {
                w.volume_timer.set(false);
                let pending = std::mem::take(&mut *w.pending_volume.borrow_mut());
                for (id, (target, v)) in pending {
                    let (cmd, object) = match target.as_str() {
                        "source" => (
                            "set-source-volume",
                            w.state
                                .borrow()
                                .sources
                                .iter()
                                .find(|x| x.id == id)
                                .map(|x| x.name.clone())
                                .unwrap_or_default(),
                        ),
                        "app" => ("set-sink-input-volume", id.clone()),
                        _ => (
                            "set-sink-volume",
                            w.state
                                .borrow()
                                .sinks
                                .iter()
                                .find(|x| x.id == id)
                                .map(|x| x.name.clone())
                                .unwrap_or_default(),
                        ),
                    };
                    if !object.is_empty() {
                        let key = format!("volume:{target}:{object}");
                        w.run(
                            key,
                            vec!["pactl".into(), cmd.into(), object, format!("{v:.0}%")],
                        );
                    }
                }
            }
        });
    }
    fn scale(self: &Rc<Self>, node: AudioNode, target: &'static str, key: String) -> gtk::Scale {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 150.0, 1.0);
        scale.set_value(node.volume);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.add_mark(100.0, gtk::PositionType::Bottom, None);
        if node.volume > 100.0 {
            scale.add_css_class("crit");
        }
        let wk = Rc::downgrade(self);
        let id = node.id.clone();
        let k = key.clone();
        scale.connect_value_changed(move |s| {
            if let Some(w) = wk.upgrade() {
                w.dragging.borrow_mut().insert(k.clone());
                w.set_volume(&id, s.value(), target);
            }
        });
        let gesture = gtk::GestureClick::new();
        let wk = Rc::downgrade(self);
        let k = key.clone();
        gesture.connect_released(move |_, _, _, _| {
            if let Some(w) = wk.upgrade() {
                w.dragging.borrow_mut().remove(&k);
                w.schedule();
            }
        });
        scale.add_controller(gesture);
        scale
    }
    /// Slider, percentage and a mute toggle on one line.
    fn vol_line(self: &Rc<Self>, node: AudioNode, target: &'static str, key: String, mute_key: String) -> gtk::Box {
        let line = ui::hbox(12, &[]);
        let scale = self.scale(node.clone(), target, key);
        let pct = ui::text(&format!("{:.0}%", node.volume), &["dim"]);
        pct.set_wrap(false);
        pct.set_width_chars(5);
        pct.set_xalign(1.0);
        let p2 = pct.clone();
        scale.connect_value_changed(move |s| {
            p2.set_text(&format!("{:.0}%", s.value()));
            if s.value() > 100.0 {
                p2.add_css_class("warn");
            } else {
                p2.remove_css_class("warn");
            }
        });
        if node.volume > 100.0 {
            pct.add_css_class("warn");
        }
        line.append(&scale);
        line.append(&pct);
        let object = if target == "app" { node.id.clone() } else { node.name.clone() };
        let cmd = match target {
            "source" => "set-source-mute",
            "app" => "set-sink-input-mute",
            _ => "set-sink-mute",
        };
        let wk = Rc::downgrade(self);
        let muted = node.muted;
        let m = ui::link(if muted { "muted" } else { "mute" }, if muted { &["danger"] } else { &[] }, move || {
            if let Some(w) = wk.upgrade() {
                w.run(mute_key.clone(), vec!["pactl".into(), cmd.into(), object.clone(), if muted { "0" } else { "1" }.into()]);
            }
        });
        line.append(&m);
        line
    }

    #[allow(dead_code)]
    fn switch(self: &Rc<Self>, node: AudioNode, target: &'static str, key: String) -> ui::Switch {
        let object = if target == "app" { node.id } else { node.name };
        let cmd = match target {
            "source" => "set-source-mute",
            "app" => "set-sink-input-mute",
            _ => "set-sink-mute",
        };
        let sw = ui::Switch::new({
            let wk = Rc::downgrade(self);
            let key = key.clone();
            move |on| {
                if let Some(w) = wk.upgrade() {
                    w.run(
                        key.clone(),
                        vec![
                            "pactl".into(),
                            cmd.into(),
                            object.clone(),
                            if on { "1" } else { "0" }.into(),
                        ],
                    );
                }
            }
        });
        sw.set(node.muted);
        sw
    }
    fn dropdown(
        opts: Vec<(String, String)>,
        active: &str,
        changed: impl Fn(String) + 'static,
    ) -> gtk::DropDown {
        let labels: Vec<String> = opts.iter().map(|x| x.1.clone()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let dd = gtk::DropDown::from_strings(&refs);
        let selected = opts.iter().position(|x| x.0 == active).map(|i| i as u32);
        if let Some(i) = selected {
            dd.set_selected(i);
        }
        let opts = Rc::new(opts);
        let cb = Rc::new(changed);
        let last = Rc::new(Cell::new(selected.unwrap_or(u32::MAX)));
        let optsc = opts.clone();
        let cbc = cb.clone();
        let lc = last.clone();
        dd.connect_selected_notify(move |d| {
            let i = d.selected();
            if i == u32::MAX || lc.get() == i {
                return;
            }
            lc.set(i);
            if let Some((v, _)) = optsc.get(i as usize) {
                cbc(v.clone());
            }
        });
        dd
    }
    fn draw(self: &Rc<Self>) {
        let st = self.state.borrow().clone();
        if let Some(n) = st.sinks.iter().find(|n| n.name == st.default_sink) {
            self.frame.detail.set_markup(&format!(
                "{} · {:.0}%",
                ui::esc(&n.description),
                n.volume
            ));
            let control = ui::Switch::new({
                let wk = Rc::downgrade(self);
                let name = n.name.clone();
                move |on| {
                    if let Some(w) = wk.upgrade() {
                        w.run(
                            "default-mute".into(),
                            vec![
                                "pactl".into(),
                                "set-sink-mute".into(),
                                name.clone(),
                                if on { "1" } else { "0" }.into(),
                            ],
                        );
                    }
                }
            });
            let _ = control;
            ui::clear(&self.frame.controls);
            if n.muted {
                self.frame.controls.append(&ui::text("muted", &["crit"]));
            }
        } else {
            self.frame.detail.set_text("no audio output");
            ui::clear(&self.frame.controls);
        }
        let w = self.clone();
        let default = st.default_sink.clone();
        self.outputs.sync(
            st.sinks
                .iter()
                .cloned()
                .map(|n| {
                    let is_default = n.name == default;
                    let key = n.id.clone();
                    let held = w.dragging.borrow().contains(&format!("sink:{}", n.id));
                    let sig = format!(
                        "{}:{}:{}:{}:{}",
                        n.description,
                        is_default,
                        n.muted,
                        n.ports.len(),
                        if held {
                            String::new()
                        } else {
                            format!("{:.1}", n.volume)
                        }
                    );
                    let w = w.clone();
                    (
                        key.clone(),
                        sig,
                        Box::new(move || {
                            let row = Row::new(
                                "󰓃",
                                &n.description,
                                if is_default { "default" } else { "" },
                            );
                            row.active(is_default);
                            let nm = n.name.clone();
                            let wc = Rc::downgrade(&w);
                            row.on_click(move || {
                                if let Some(w) = wc.upgrade() {
                                    w.run(
                                        "default-output".into(),
                                        vec!["pactl".into(), "set-default-sink".into(), nm.clone()],
                                    );
                                }
                            });
                            if is_default {
                                let ex = row.expand();
                                ex.append(&w.vol_line(n.clone(), "sink", format!("sink:{}", n.id), format!("mute:{}", n.id)));
                                if n.ports.len() > 1 {
                                    let opts = n
                                        .ports
                                        .iter()
                                        .map(|p| (p.name.clone(), p.description.clone()))
                                        .collect();
                                    let nm = n.name.clone();
                                    let wk = Rc::downgrade(&w);
                                    let dd = Self::dropdown(opts, &n.active_port, move |port| {
                                        if let Some(w) = wk.upgrade() {
                                            w.run(
                                                format!("port:{nm}"),
                                                vec![
                                                    "pactl".into(),
                                                    "set-sink-port".into(),
                                                    nm.clone(),
                                                    port,
                                                ],
                                            );
                                        }
                                    });
                                    ex.append(&dd);
                                }
                            }
                            row.root.upcast()
                        }) as Box<dyn FnOnce() -> gtk::Widget>,
                    )
                })
                .collect(),
        );
        let default_source = st.default_source.clone();
        self.inputs.sync(
            st.sources
                .iter()
                .cloned()
                .map(|n| {
                    let key = n.id.clone();
                    let held = self.dragging.borrow().contains(&format!("source:{}", n.id));
                    let sig = format!(
                        "{}:{}:{}:{}:{}",
                        n.description,
                        n.muted,
                        n.ports.len(),
                        st.source_users.get(&n.id).cloned().unwrap_or_default(),
                        if held {
                            String::new()
                        } else {
                            format!("{:.1}", n.volume)
                        }
                    );
                    let w = self.clone();
                    let users = st.source_users.get(&n.id).cloned();
                    let default_source = default_source.clone();
                    (
                        key.clone(),
                        sig,
                        Box::new(move || {
                            let meta = if n.name == default_source {
                                "default".to_string()
                            } else {
                                String::new()
                            };
                            let row = Row::new("󰍬", &n.description, &meta);
                            row.active(n.name == default_source);
                            let nm = n.name.clone();
                            let wk = Rc::downgrade(&w);
                            row.on_click(move || {
                                if let Some(w) = wk.upgrade() {
                                    w.run(
                                        "default-input".into(),
                                        vec![
                                            "pactl".into(),
                                            "set-default-source".into(),
                                            nm.clone(),
                                        ],
                                    );
                                }
                            });
                            let ex = row.expand();
                            if let Some(users) = users {
                                ex.append(&ui::text(&format!("in use by {users}"), &["meta"]));
                            }
                            ex.append(&w.vol_line(n.clone(), "source", format!("source:{}", n.id), format!("mute-source:{}", n.id)));
                            row.root.upcast()
                        }) as Box<dyn FnOnce() -> gtk::Widget>,
                    )
                })
                .collect(),
        );
        let app_items: Vec<ui::Item> = if st.apps.is_empty() {
            vec![(
                "empty".into(),
                "empty".into(),
                Box::new(|| ui::text("nothing is playing", &["note"]).upcast())
                    as Box<dyn FnOnce() -> gtk::Widget>,
            )]
        } else {
            st.apps
                .iter()
                .cloned()
                .map(|n| {
                    let key = n.id.clone();
                    let held = self.dragging.borrow().contains(&format!("app:{}", n.id));
                    let sig = format!(
                        "{}:{}:{}:{}",
                        n.app,
                        n.muted,
                        n.device,
                        if held {
                            String::new()
                        } else {
                            format!("{:.1}", n.volume)
                        }
                    );
                    let w = self.clone();
                    let sinks = st.sinks.clone();
                    (
                        key,
                        sig,
                        Box::new(move || {
                            let row = Row::new("󰓇", &n.app, "");
                            let ex = row.expand();
                            ex.append(&w.vol_line(n.clone(), "app", format!("app:{}", n.id), format!("appmute:{}", n.id)));
                            let opts: Vec<(String, String)> = sinks
                                .iter()
                                .map(|x| (x.name.clone(), x.description.clone()))
                                .collect();
                            let active = sinks
                                .iter()
                                .find(|x| x.id == n.device)
                                .map(|x| x.name.clone())
                                .unwrap_or_default();
                            let id = n.id.clone();
                            let wk = Rc::downgrade(&w);
                            ex.append(&Self::dropdown(opts, &active, move |sink| {
                                if let Some(w) = wk.upgrade() {
                                    w.run(
                                        format!("move:{id}"),
                                        vec![
                                            "pactl".into(),
                                            "move-sink-input".into(),
                                            id.clone(),
                                            sink,
                                        ],
                                    );
                                }
                            }));
                            row.root.upcast()
                        }) as Box<dyn FnOnce() -> gtk::Widget>,
                    )
                })
                .collect()
        };
        self.apps.sync(app_items);
        self.cards.sync(
            st.cards
                .iter()
                .cloned()
                .map(|c| {
                    let key = c.id.clone();
                    let sig = format!("{}:{}", c.name, c.active);
                    let w = self.clone();
                    (
                        key,
                        sig,
                        Box::new(move || {
                            let row = Row::new("󰂯", &c.name, "");
                            let opts = c.profiles.clone();
                            let active = c.active.clone();
                            let id = c.id.clone();
                            let wk = Rc::downgrade(&w);
                            row.line
                                .append(&Self::dropdown(opts, &active, move |profile| {
                                    if let Some(w) = wk.upgrade() {
                                        w.run(
                                            format!("profile:{id}"),
                                            vec![
                                                "pactl".into(),
                                                "set-card-profile".into(),
                                                id.clone(),
                                                profile,
                                            ],
                                        );
                                    }
                                }));
                            row.root.upcast()
                        }) as Box<dyn FnOnce() -> gtk::Widget>,
                    )
                })
                .collect(),
        );
    }
}
