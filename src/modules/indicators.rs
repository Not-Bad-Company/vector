//! Small state marks in the centre, each hidden unless active:
//! voxtype, screen recording, idle lock off, sleep lock, Windows VM,
//! do-not-disturb, and privacy: microphone and camera in use.
//! Also `Profile`: the platform power profile (asusctl: quiet/balanced/performance).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::mako;
use crate::mode;
use crate::panel::{Card, Panel};
use crate::slot::Slot;
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const WIN_IDLE_FILE: &str = "/media/daanh/Shared/VMs/win11-setup/win11-idle.txt";
const WIN_LIMIT: u64 = 1800;

struct Inner {
    vox: Slot,
    rec: Slot,
    idle: Slot,
    sleep: Slot,
    win: Slot,
    dnd: Slot,
    mic: Slot,
    cam: Slot,
    theme: Rc<Theme>,
    vox_tip: RefCell<String>,
    mic_apps: RefCell<Vec<String>>,
    cam_apps: RefCell<Vec<String>>,
    win_state: RefCell<WinState>,
    streams: RefCell<Vec<util::Stream>>,
    mic_busy: Cell<bool>,
    vm_pid: Cell<Option<u32>>,
    order: Vec<String>,
    /// the mode block is on the bar: these states show there instead of as icons
    modes: bool,
}

const M_REC: mode::State = mode::State { key: "rec", label: "REC", rank: 0 };
const M_DICTATE: mode::State = mode::State { key: "dictate", label: "DICTATE", rank: 1 };
const M_VM: mode::State = mode::State { key: "vm", label: "VM", rank: 3 };
const M_SILENT: mode::State = mode::State { key: "silent", label: "SILENT", rank: 4 };
const M_AWAKE: mode::State = mode::State { key: "awake", label: "AWAKE", rank: 5 };

#[derive(Default, Clone)]
struct WinState {
    cpu_ticks: u64,
    at: f64,
    low_since: f64,
    since: String,
    input_idle: u64,
    cpu_quiet: u64,
    idle: bool,
}

pub struct Indicators(Rc<Inner>);

fn simple_tip(title: &str, detail: &str, hint: &str) -> Option<gtk::Widget> {
    let s = ui::sheet();
    s.append(&ui::head(title, detail).0);
    if !hint.is_empty() {
        s.append(&ui::thint(hint));
    }
    Some(s.upcast())
}

impl Indicators {
    pub fn new(ctx: &Ctx) -> Indicators {
        let o = ctx.opts("indicators");
        let small = ["small"];
        let inner = Rc::new(Inner {
            vox: ctx.slot("voxtype", &[]),
            rec: ctx.slot("recording", &["small", "crit"]),
            idle: ctx.slot("idle", &["small", "crit"]),
            sleep: ctx.slot("sleeplock", &small),
            win: ctx.slot("win11", &small),
            dnd: ctx.slot("dnd", &["small", "crit"]),
            mic: ctx.slot("mic", &["small", "warn"]),
            cam: ctx.slot("camera", &["small", "crit"]),
            theme: ctx.theme.clone(),
            vox_tip: RefCell::default(),
            mic_apps: RefCell::default(),
            cam_apps: RefCell::default(),
            win_state: RefCell::default(),
            streams: RefCell::default(),
            mic_busy: Cell::new(false),
            vm_pid: Cell::new(None),
            order: o
                .list("show")
                .unwrap_or_else(|| ["voxtype", "recording", "idle", "sleeplock", "win11", "dnd", "mic", "camera"].iter().map(|s| s.to_string()).collect()),
            modes: ctx.cfg.left.iter().chain(&ctx.cfg.center).chain(&ctx.cfg.right).any(|n| n == "mode" || n == "menu"),
        });
        for s in inner.all() {
            s.set_visible(false);
        }
        inner.rec.set_markup("󰻂");
        inner.rec.on_left(|| util::spawn_sh("omarchy-capture-screenrecording --stop-recording"));
        inner.rec.tooltip(|| simple_tip("recording", "screen", "click stop"));
        inner.idle.set_markup("󱫖");
        inner.idle.on_left(|| util::spawn_sh("omarchy-toggle-idle"));
        inner.idle.tooltip(|| simple_tip("idle lock", "off", "click turn back on"));
        inner.sleep.set_markup("󰒳");
        inner.sleep.on_left(|| util::spawn(&[&util::home().join(".local/libexec/toggle_sleeplock").to_string_lossy()]));
        inner.dnd.set_markup("󰂛");
        inner.dnd.on_left(|| mako::get().set_dnd(false));
        inner.dnd.tooltip(|| simple_tip("notifications", "silenced", "click unsilence"));
        inner.win.set_markup("\u{f17a}");
        inner.win.on_left(|| util::spawn_sh("~/.local/bin/win11"));
        inner.win.on_right(|| util::spawn_sh("~/.local/bin/win11-off"));
        inner.mic.set_markup("󰍬");
        inner.cam.set_markup("󰄀");
        inner.vox.on_left(|| util::spawn_sh("omarchy-voxtype-model"));
        inner.vox.on_right(|| util::spawn_sh("omarchy-voxtype-config"));

        let w = Rc::downgrade(&inner);
        inner.sleep.tooltip(move || {
            let since = std::fs::metadata(sleeplock_unit()).ok().and_then(|m| m.modified().ok()).map(|t| {
                let s = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                util::local(s, "%H:%M")
            });
            let _ = w.upgrade()?;
            let s = ui::sheet();
            s.append(&ui::head("sleep lock", &format!("on{}", since.map(|x| format!(" since {x}")).unwrap_or_default())).0);
            s.append(&ui::tkv("blocks", "sleep and idle").tap(|b| b.set_margin_top(6)));
            s.append(&ui::thint("click or m4 to allow sleep"));
            Some(s.upcast())
        });
        let w = Rc::downgrade(&inner);
        inner.win.tooltip(move || w.upgrade().map(|i| i.win_tip()));
        let w = Rc::downgrade(&inner);
        inner.vox.tooltip(move || {
            let i = w.upgrade()?;
            let t = i.vox_tip.borrow().clone();
            simple_tip("voxtype", &util::esc(&t.to_lowercase()), "click model  ·  right config")
        });
        let w = Rc::downgrade(&inner);
        inner.mic.tooltip(move || {
            let i = w.upgrade()?;
            let apps = i.mic_apps.borrow().join(", ");
            simple_tip("microphone", &format!("in use by {}", util::esc(&apps)), "")
        });
        let w = Rc::downgrade(&inner);
        inner.cam.tooltip(move || {
            let i = w.upgrade()?;
            let apps = i.cam_apps.borrow().join(", ");
            simple_tip("camera", &format!("in use by {}", util::esc(&apps)), "")
        });

        // do-not-disturb: mako signals
        let m = mako::get();
        let w = Rc::downgrade(&inner);
        m.changed.connect(move || {
            if let Some(i) = w.upgrade() {
                i.show(&i.dnd, M_SILENT, mako::get().dnd());
            }
        });
        inner.show(&inner.dnd, M_SILENT, m.dnd());

        // voxtype streams its own state
        if util::which("voxtype") {
            let w = Rc::downgrade(&inner);
            let s = util::stream("voxtype status --follow --extended --format json", Duration::from_secs(10), move |line| {
                let Some(i) = w.upgrade() else { return };
                let Some(line) = line else {
                    i.show(&i.vox, M_DICTATE, false);
                    return;
                };
                let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let class = v["class"].as_str().or(v["alt"].as_str()).unwrap_or("idle").to_string();
                *i.vox_tip.borrow_mut() = v["tooltip"].as_str().unwrap_or("").to_string();
                let (icon, cls) = match class.as_str() {
                    "recording" => ("󰍬", "crit"),
                    "transcribing" => ("󰔟", "warn"),
                    _ => ("", ""),
                };
                i.vox.set_markup(icon);
                i.vox.set_classes(&[cls, "small"]);
                if class == "recording" {
                    i.show(&i.vox, M_DICTATE, true);
                } else {
                    // transcribing stays an icon (it's brief); idle hides
                    mode::set(M_DICTATE, false);
                    i.vox.set_visible(!icon.is_empty());
                }
            });
            inner.streams.borrow_mut().push(s);
        }

        // microphone: pactl subscribe tells us when recording streams come and go
        if util::which("pactl") {
            let w = Rc::downgrade(&inner);
            let s = util::stream("pactl subscribe", Duration::from_secs(5), move |line| {
                let Some(i) = w.upgrade() else { return };
                if line.as_deref().is_none_or(|l| l.contains("source-output")) {
                    i.poll_mic();
                }
            });
            inner.streams.borrow_mut().push(s);
            inner.poll_mic();
        }

        inner.poll();
        inner.poll_cam();
        util::every(2000, &inner, |i| i.poll());
        util::every(10_000, &inner, |i| i.poll_cam());
        util::every(5000, &inner, |i| i.poll_win());
        Indicators(inner)
    }
}

fn sleeplock_unit() -> PathBuf {
    util::runtime_dir().join("systemd/transient/sleeplock.service")
}

impl Inner {
    fn all(&self) -> [&Slot; 8] {
        [&self.vox, &self.rec, &self.idle, &self.sleep, &self.win, &self.dnd, &self.mic, &self.cam]
    }

    fn by_name(&self, n: &str) -> Option<&Slot> {
        self.all().into_iter().find(|s| s.name() == n)
    }

    /// Shows a state as a mode (when the mode block is on the bar) or as its icon.
    fn show(&self, slot: &Slot, st: mode::State, on: bool) {
        if self.modes {
            mode::set(st, on);
            slot.set_visible(false);
        } else {
            slot.set_visible(on);
        }
    }

    fn poll(&self) {
        let found = util::find_comms(&["gpu-screen-recorder", "hypridle", "qemu-system-x86_64"]);
        self.show(&self.rec, M_REC, found[0].is_some());
        self.idle.set_visible(found[1].is_none());
        self.show(&self.sleep, M_AWAKE, sleeplock_unit().exists());
        // the VM: only its own qemu (checked by command line) counts
        let vm = found[2].filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| String::from_utf8_lossy(&c).contains("guest=win11"))
        });
        self.vm_pid.set(vm);
    }

    fn poll_mic(self: &Rc<Self>) {
        if self.mic_busy.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        util::bg(
            || {
                let o = util::run(&["pactl", "-f", "json", "list", "source-outputs"], Duration::from_secs(2))?;
                let v: serde_json::Value = serde_json::from_str(&o.stdout).ok()?;
                let mut apps: Vec<String> = v
                    .as_array()?
                    .iter()
                    .filter(|s| s["corked"].as_bool() != Some(true))
                    // monitors of an output (visualisers, recorders of desktop audio) are not the mic
                    .filter(|s| !s["source"].as_str().is_some_and(|x| x.ends_with(".monitor")))
                    .filter_map(|s| {
                        let p = &s["properties"];
                        p["application.name"].as_str().or(p["application.process.binary"].as_str()).map(str::to_string)
                    })
                    .collect();
                apps.sort();
                apps.dedup();
                Some(apps)
            },
            move |apps| {
                let Some(i) = w.upgrade() else { return };
                i.mic_busy.set(false);
                let apps = apps.unwrap_or_default();
                i.mic.set_visible(!apps.is_empty());
                *i.mic_apps.borrow_mut() = apps;
                i.mic.bump();
            },
        );
    }

    fn poll_cam(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        util::bg(
            || {
                let mut apps = Vec::new();
                let Ok(rd) = std::fs::read_dir("/proc") else { return apps };
                for e in rd.flatten() {
                    if !e.file_name().to_str().is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit())) {
                        continue;
                    }
                    let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else { continue };
                    let uses = fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.to_string_lossy().starts_with("/dev/video")));
                    if uses {
                        if let Some(c) = util::read_trim(e.path().join("comm")) {
                            apps.push(c);
                        }
                    }
                }
                apps.sort();
                apps.dedup();
                // pipewire/wireplumber hold the devices open all the time
                apps.retain(|a| !matches!(a.as_str(), "pipewire" | "wireplumber" | ".pipewire-wrap"));
                apps
            },
            move |apps| {
                let Some(i) = w.upgrade() else { return };
                i.cam.set_visible(!apps.is_empty());
                *i.cam_apps.borrow_mut() = apps;
                i.cam.bump();
            },
        );
    }

    fn poll_win(&self) {
        let pid = self.vm_pid.get();
        let Some(pid) = pid else {
            *self.win_state.borrow_mut() = WinState::default();
            self.show(&self.win, M_VM, false);
            return;
        };
        let stat = util::read(format!("/proc/{pid}/stat")).unwrap_or_default();
        let f: Vec<&str> = stat.rsplit_once(')').map(|(_, r)| r.split_whitespace().collect()).unwrap_or_default();
        let ticks = f.get(11).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
        let hz = 100.0;
        let now = util::now();
        let mut st = self.win_state.borrow_mut();
        if st.at > 0.0 && now > st.at {
            let pct = (ticks.saturating_sub(st.cpu_ticks)) as f64 / hz / (now - st.at) * 100.0;
            if pct < 60.0 {
                if st.low_since == 0.0 {
                    st.low_since = now;
                }
            } else {
                st.low_since = 0.0;
            }
        }
        st.cpu_ticks = ticks;
        st.at = now;
        if st.since.is_empty() {
            st.since = std::fs::metadata(format!("/proc/{pid}"))
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| util::local(t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0), "%H:%M"))
                .unwrap_or_default();
        }
        st.input_idle = std::fs::metadata(WIN_IDLE_FILE)
            .ok()
            .filter(|m| m.modified().ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e.as_secs() < 90))
            .and_then(|_| util::read(WIN_IDLE_FILE))
            .map(|s| s.chars().filter(|c| c.is_ascii_digit()).collect::<String>())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        st.cpu_quiet = if st.low_since > 0.0 { (now - st.low_since) as u64 } else { 0 };
        st.idle = st.input_idle >= WIN_LIMIT && st.cpu_quiet >= WIN_LIMIT;
        let idle = st.idle;
        drop(st);
        self.win.set_classes(&[if idle { "crit" } else { "" }]);
        // an idle VM holding the GPU is worth an icon even with the mode block
        if self.modes {
            mode::set(M_VM, true);
            self.win.set_visible(idle);
        } else {
            self.win.set_visible(true);
        }
        self.win.bump();
    }

    fn win_tip(&self) -> gtk::Widget {
        let st = self.win_state.borrow();
        let s = ui::sheet();
        let detail = if st.idle {
            format!("{}{}", ui::colored("idle", &self.theme.color1), if st.since.is_empty() { String::new() } else { format!(", on since {}", st.since) })
        } else {
            format!("running{}", if st.since.is_empty() { String::new() } else { format!(" since {}", st.since) })
        };
        s.append(&ui::head("windows 11", &detail).0);
        s.append(&ui::tsection("idle"));
        s.append(&ui::tkv("input", &format!("{} min {}", st.input_idle / 60, ui::dim("without input"))));
        s.append(&ui::tkv("cpu", &format!("{} min {}", st.cpu_quiet / 60, ui::dim("under 60% of a core"))));
        s.append(&ui::text("turns red at 30 min for both", &["dim"]));
        s.append(&ui::thint("click open  ·  right shut down"));
        s.upcast()
    }
}

impl Module for Indicators {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        let b = ui::hbox(0, &["indicators"]);
        for n in &self.0.order {
            if let Some(s) = self.0.by_name(n) {
                b.append(&s.view());
            }
        }
        b.upcast()
    }
    fn refresh(&self) {
        self.0.poll();
        self.0.poll_cam();
        self.0.poll_mic();
        self.0.poll_win();
        mako::get().refresh();
    }
    fn slots(&self) -> Vec<Slot> {
        self.0.all().into_iter().cloned().collect()
    }
}

// ── power profile ────────────────────────────────────────────────────────────

const PROFILE: &str = "/sys/firmware/acpi/platform_profile";

struct PInner {
    slot: Slot,
    current: RefCell<String>,
    asus: bool,
    hide: Vec<String>,
}

pub struct Profile(Rc<PInner>);

fn profile_icon(p: &str) -> &'static str {
    match p {
        "quiet" | "low-power" | "power-saver" => "󰾆",
        "performance" => "󰓅",
        _ => "󰾅",
    }
}

impl Profile {
    pub fn new(ctx: &Ctx) -> Profile {
        let o = ctx.opts("profile");
        let inner = Rc::new(PInner {
            slot: ctx.slot("profile", &[]),
            current: RefCell::default(),
            asus: util::which("asusctl"),
            hide: o.list("hide_when").unwrap_or_default(),
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_right(move || {
            if let Some(i) = w.upgrade() {
                i.cycle();
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || {
            let i = w.upgrade()?;
            let s = ui::sheet();
            s.append(&ui::head("power profile", &i.current.borrow()).0);
            s.append(&ui::thint("click choose  ·  right next"));
            Some(s.upcast())
        });
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => i.panel(p),
            None => ui::vbox(0, &[]).upcast(),
        });
        inner.poll();
        util::every(2000, &inner, |i| i.poll());
        Profile(inner)
    }
}

impl PInner {
    fn poll(&self) {
        let cur = util::read_trim(PROFILE).unwrap_or_default();
        if cur.is_empty() {
            self.slot.set_visible(false);
            return;
        }
        let shown = format!("{}  {cur}", profile_icon(&cur));
        self.slot.set_markup(&shown);
        self.slot.set_classes(&[match cur.as_str() {
            "performance" => "warn",
            "quiet" | "low-power" => "dim",
            _ => "",
        }]);
        self.slot.set_visible(!self.hide.contains(&cur));
        *self.current.borrow_mut() = cur;
    }

    fn set(&self, p: &str) {
        if self.asus {
            // asusctl names: Quiet / Balanced / Performance
            let mut name = p.to_string();
            if let Some(f) = name.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            util::spawn(&["asusctl", "profile", "set", &name]);
        } else {
            let map = match p {
                "quiet" | "low-power" => "power-saver",
                other => other,
            };
            util::spawn(&["powerprofilesctl", "set", map]);
        }
    }

    fn cycle(self: &Rc<Self>) {
        if self.asus {
            util::spawn(&["asusctl", "profile", "next"]);
        } else {
            let next = match self.current.borrow().as_str() {
                "quiet" | "low-power" => "balanced",
                "balanced" => "performance",
                _ => "quiet",
            };
            self.set(next);
        }
        util::after(400, self, |i| i.poll());
    }

    fn panel(self: &Rc<Self>, p: &Panel) -> gtk::Widget {
        p.set_width(300);
        let card = Card::new("power profile", if self.asus { "asusctl" } else { "platform" });
        let choices = util::read_trim("/sys/firmware/acpi/platform_profile_choices").unwrap_or_else(|| "quiet balanced performance".into());
        let cur = self.current.borrow().clone();
        let rows = ui::vbox(0, &[]);
        for c in choices.split_whitespace() {
            let r = ui::Row::new(profile_icon(c), c, if c == cur { "active" } else { "" });
            r.set_active(c == cur);
            let me = Rc::downgrade(self);
            let c2 = c.to_string();
            let pc = p.clone();
            ui::on_click(&r.root, move || {
                if let Some(i) = me.upgrade() {
                    i.set(&c2);
                    util::after(400, &i, |i| i.poll());
                }
                pc.close();
            });
            rows.append(&r.root);
        }
        card.body.append(&rows);
        if self.asus {
            // what asusd really does on plug/unplug (its own config, not `asusctl profile get`)
            let ron = util::read("/etc/asusd/asusd.ron").unwrap_or_default();
            let field = |k: &str| {
                ron.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}:")).map(|v| v.trim().trim_end_matches(',').to_lowercase()))
            };
            card.body.append(&ui::section("when power changes"));
            for (label, flag, prof, cli) in [
                ("charger", "change_platform_profile_on_ac", "platform_profile_on_ac", "-a"),
                ("battery", "change_platform_profile_on_battery", "platform_profile_on_battery", "-b"),
            ] {
                let switches = field(flag).as_deref() == Some("true");
                let target = field(prof).unwrap_or_default();
                let row = ui::hbox(0, &["kv"]);
                row.append(&ui::text(label, &["k"]));
                if switches {
                    let opts: Vec<(&str, &str)> = choices.split_whitespace().map(|c| (c, c)).collect();
                    let cli = cli.to_string();
                    let seg = ui::segmented(&opts, &target, move |p| {
                        let mut name = p.to_string();
                        if let Some(f) = name.get_mut(0..1) {
                            f.make_ascii_uppercase();
                        }
                        util::spawn(&["asusctl", "profile", "set", &cli, &name]);
                    });
                    row.append(&seg);
                } else {
                    row.append(&ui::label(&ui::dim("keeps the current profile"), &[]));
                }
                card.body.append(&row);
            }
            card.body.append(&ui::text("switching on/off per source: change_platform_profile_on_* in /etc/asusd/asusd.ron", &["msg", "faint"]).tap(|l| l.set_wrap(true)));
        }
        let _ = Path::new(PROFILE);
        card.root.upcast()
    }
}

impl Module for Profile {
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
