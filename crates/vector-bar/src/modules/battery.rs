//! Battery + power. Bar: `23% 󰁻`. Click: power panel (battery details, power
//! mode, charge limit, lock/suspend/restart/shut down). Right: battery notification.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const ICONS: [&str; 10] = ["󰁺", "󰁻", "󰁼", "󰁽", "󰁾", "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"];
const CHARGING: [&str; 10] = ["󰢜", "󰂆", "󰂇", "󰂈", "󰢝", "󰂉", "󰢞", "󰂊", "󰂋", "󰂅"];

#[derive(Clone, Default, PartialEq)]
pub struct Bat {
    pub present: bool,
    pub capacity: i64,
    pub status: String,
    pub watts: f64,
    pub energy_now: f64,
    pub energy_full: f64,
    pub energy_design: f64,
    pub cycles: i64,
    pub limit: Option<i64>,
    pub ac: bool,
}

impl Bat {
    pub fn read(dev: &Path) -> Bat {
        let n = |f: &str| util::read_num(dev.join(f));
        let ac = std::fs::read_dir("/sys/class/power_supply")
            .map(|rd| {
                rd.flatten().any(|e| {
                    util::read_trim(e.path().join("type")).as_deref() == Some("Mains")
                        && util::read_num(e.path().join("online")) == Some(1)
                })
            })
            .unwrap_or(false);
        Bat {
            present: dev.join("capacity").exists(),
            capacity: n("capacity").unwrap_or(0),
            status: util::read_trim(dev.join("status"))
                .unwrap_or_default()
                .to_lowercase(),
            watts: n("power_now").unwrap_or(0) as f64 / 1e6,
            energy_now: n("energy_now").unwrap_or(0) as f64 / 1e6,
            energy_full: n("energy_full").unwrap_or(0) as f64 / 1e6,
            energy_design: n("energy_full_design").unwrap_or(0) as f64 / 1e6,
            cycles: n("cycle_count").unwrap_or(0),
            limit: n("charge_control_end_threshold"),
            ac,
        }
    }

    /// Hours to empty (discharging) or to full/limit (charging).
    pub fn hours(&self) -> Option<f64> {
        if self.watts < 0.3 {
            return None;
        }
        match self.status.as_str() {
            "discharging" => Some(self.energy_now / self.watts),
            "charging" => {
                let target = self.energy_full * self.limit.unwrap_or(100) as f64 / 100.0;
                Some(((target - self.energy_now) / self.watts).max(0.0))
            }
            _ => None,
        }
    }
}

fn hm(h: f64) -> String {
    let m = (h * 60.0).round() as i64;
    if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {:02}m", m / 60, m % 60)
    }
}

struct Inner {
    slot: Slot,
    dev: PathBuf,
    bat: RefCell<Bat>,
    changed: Notify,
    warn: i64,
    crit: i64,
    theme: Rc<Theme>,
    /// the low-battery card on screen (0: none)
    card: Cell<u32>,
    /// highest level shown this discharge: 0 none, 1 warning, 2 critical
    stage: Cell<u8>,
}

pub struct Battery(Rc<Inner>);

impl Battery {
    pub fn new(ctx: &Ctx) -> Battery {
        let o = ctx.opts("battery");
        let dev = PathBuf::from("/sys/class/power_supply").join(o.str("device", "BAT0"));
        let inner = Rc::new(Inner {
            slot: ctx.slot("battery", &[]),
            dev,
            bat: RefCell::default(),
            changed: Notify::default(),
            warn: o.num("warning", 20.0) as i64,
            crit: o.num("critical", 10.0) as i64,
            theme: ctx.theme.clone(),
            card: Cell::new(0),
            stage: Cell::new(0),
        });
        let right = o.str(
            "on_right",
            "notify-send -u low \"$(omarchy-battery-status)\"",
        );
        inner.slot.on_right(move || util::spawn_sh(&right));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => power_panel(&i, p),
            None => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        });
        inner.update();
        util::every(o.num("interval", 5.0) as u64 * 1000, &inner, |i| i.update());
        Battery(inner)
    }
}

impl Inner {
    fn update(&self) {
        let b = Bat::read(&self.dev);
        if !b.present {
            self.slot.set_visible(false);
            return;
        }
        self.slot.set_visible(true);
        let idx = ((b.capacity.clamp(0, 100) as usize) / 10).min(9);
        let charging = b.status == "charging";
        let full =
            b.status == "full" || (b.ac && b.capacity >= b.limit.unwrap_or(100) - 1 && !charging);
        let text = if full {
            "󰂅".to_string()
        } else if charging {
            format!("{}% {}", b.capacity, CHARGING[idx])
        } else if b.ac {
            format!("{}% 󰚥", b.capacity)
        } else {
            format!("{}% {}", b.capacity, ICONS[idx])
        };
        self.slot.set_markup(&text);
        let discharging = b.status == "discharging";
        let cls = if discharging && b.capacity <= self.crit {
            "crit"
        } else if discharging && b.capacity <= self.warn {
            "warn"
        } else {
            ""
        };
        self.slot.set_classes(&[cls]);
        self.low_card(&b, discharging);
        if *self.bat.borrow() != b {
            *self.bat.borrow_mut() = b;
            self.changed.emit();
        }
    }

    /// One low-battery card for the whole discharge: shown at the warning
    /// level, updated in place as the percentage drops, raised to critical at
    /// the critical level, never timing out, closed when the charger is in.
    /// Swiped away, it stays away until the next level is crossed.
    fn low_card(&self, b: &Bat, discharging: bool) {
        let store = crate::notify::get();
        let serving = crate::notify::enabled();
        let live = |id: u32| id != 0 && store.live().iter().any(|n| n.id == id);
        if !discharging || b.capacity > self.warn {
            let id = self.card.replace(0);
            if live(id) {
                store.close(id, 3);
            }
            if !discharging {
                self.stage.set(0);
            }
            return;
        }
        let want = if b.capacity <= self.crit { 2 } else { 1 };
        let id = self.card.get();
        let showing = live(id);
        if !showing && want <= self.stage.get() {
            return; // swiped away at this level
        }
        let left = b
            .hours()
            .map(|h| format!(" · about {} left", hm(h)))
            .unwrap_or_default();
        let body = format!("{}%{left}", b.capacity);
        if !serving {
            // another notification server: one plain notification per level
            if want > self.stage.get() {
                self.stage.set(want);
                util::spawn(&[
                    "notify-send",
                    "-u",
                    if want == 2 { "critical" } else { "normal" },
                    "-a",
                    "Battery",
                    "-i",
                    "battery-caution",
                    "Battery low",
                    &body,
                ]);
            }
            return;
        }
        if showing
            && store
                .live()
                .iter()
                .any(|n| n.id == id && n.body == body && (n.urgency == 2) == (want == 2))
        {
            return;
        }
        let first = want > self.stage.get();
        self.stage.set(self.stage.get().max(want));
        let note = crate::notify::Note {
            app: "Battery".into(),
            summary: if want == 2 { "Battery almost empty".into() } else { "Battery low".into() },
            body,
            urgency: if want == 2 { 2 } else { 1 },
            icon: "battery-caution".into(),
            category: "device.warning".into(),
            ..Default::default()
        };
        let new = store.notify(note, if showing { id } else { 0 }, 0);
        self.card.set(new);
        if first && !showing {
            util::spawn(&["omarchy-hook", "battery-low", &b.capacity.to_string()]);
        }
    }

    fn tooltip(&self) -> gtk::Widget {
        let b = self.bat.borrow();
        let s = ui::sheet();
        let state = if b.ac && b.status != "charging" {
            "plugged in".to_string()
        } else {
            b.status.clone()
        };
        s.append(&ui::head("battery", &state).0);
        let cls = if b.status == "discharging" && b.capacity <= self.crit {
            "crit"
        } else if b.status == "discharging" && b.capacity <= self.warn {
            "warn"
        } else {
            ""
        };
        let when = match b.hours() {
            Some(h) if b.status == "charging" => format!("full in {}", hm(h)),
            Some(h) => format!("{} left", hm(h)),
            None => String::new(),
        };
        s.append(&ui::tmeter(
            "level",
            b.capacity as f64 / 100.0,
            &format!("{}%", b.capacity),
            &when,
            cls,
        ));
        s.append(&ui::tkv(
            if b.status == "charging" {
                "input"
            } else {
                "draw"
            },
            &format!("{:.1} W", b.watts),
        ));
        if b.energy_design > 0.0 {
            s.append(&ui::tkv(
                "health",
                &format!(
                    "{:.0}%  {}",
                    b.energy_full / b.energy_design * 100.0,
                    ui::dim(&format!("· {} cycles", b.cycles))
                ),
            ));
        }
        if let Some(l) = b.limit.filter(|l| *l < 100) {
            s.append(&ui::tkv("limit", &format!("{l}%")));
        }
        if let Some(p) = util::read_trim("/sys/firmware/acpi/platform_profile") {
            s.append(&ui::tkv("profile", &p));
        }
        s.append(&ui::thint("click power  ·  right details"));
        s.upcast()
    }
}

impl Module for Battery {
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

fn power_panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(340);
    let card = Card::new("power", "");
    let body = &card.body;

    // battery
    body.append(&ui::section("battery").tap(|l| l.add_css_class("first")));
    let level = ui::hbox(0, &[]);
    body.append(&level);
    let details = ui::vbox(0, &[]);
    body.append(&details);

    let fill = {
        let i = Rc::downgrade(i);
        let level = level.clone();
        let details = details.clone();
        let detail = card.detail.clone();
        move || {
            let Some(i) = i.upgrade() else { return };
            let b = i.bat.borrow().clone();
            detail.set_markup(&format!(
                "{}% · {}",
                b.capacity,
                if b.ac && b.status != "charging" {
                    "plugged in"
                } else {
                    &b.status
                }
            ));
            ui::clear(&level);
            let cls = if b.capacity <= i.crit {
                "crit"
            } else if b.capacity <= i.warn {
                "warn"
            } else {
                ""
            };
            level.append(&ui::meter_row(
                "level",
                b.capacity as f64 / 100.0,
                &format!("{}%", b.capacity),
                "",
                cls,
            ));
            ui::clear(&details);
            if let Some(h) = b.hours() {
                let (k, v) = if b.status == "charging" {
                    ("full in", hm(h))
                } else {
                    ("left", hm(h))
                };
                details.append(&ui::kv(k, &v).0);
            }
            let rate = if b.status == "charging" {
                "input"
            } else {
                "draw"
            };
            details.append(&ui::kv(rate, &format!("{:.1} W", b.watts)).0);
            if b.energy_design > 0.0 {
                let health = b.energy_full / b.energy_design * 100.0;
                details.append(
                    &ui::kv(
                        "health",
                        &format!(
                            "{health:.0}%  {}",
                            ui::dim(&format!(
                                "· {:.0} of {:.0} Wh · {} cycles",
                                b.energy_full, b.energy_design, b.cycles
                            ))
                        ),
                    )
                    .0,
                );
            }
            let _ = &i.theme;
        }
    };
    fill();
    p.watch(&i.changed, fill);

    // charge limit (ASUS & co: charge_control_end_threshold, needs write access)
    if let Some(limit) = i.bat.borrow().limit {
        let lim_path = i.dev.join("charge_control_end_threshold");
        let writable = std::fs::OpenOptions::new()
            .write(true)
            .open(&lim_path)
            .is_ok();
        let shown = if writable {
            String::new()
        } else {
            format!("{limit}%")
        };
        let (row, _) = ui::kv("limit", &shown);
        if writable {
            let opts = [("60", "60%"), ("80", "80%"), ("100", "100%")];
            let seg = ui::segmented(&opts, &limit.to_string(), move |v| {
                let _ = std::fs::write(&lim_path, v);
            });
            row.append(&seg);
        }
        body.append(&row);
    }

    // power mode
    let profile = util::sh_out("powerprofilesctl get", Duration::from_millis(800));
    if let Some(cur) = profile {
        body.append(&ui::section("power mode"));
        let opts = [
            ("power-saver", "saver"),
            ("balanced", "balanced"),
            ("performance", "performance"),
        ];
        let seg = ui::segmented(&opts, &cur, |v| {
            util::spawn(&["powerprofilesctl", "set", v])
        });
        seg.add_css_class("chips");
        body.append(&seg);
    }

    // session
    body.append(&ui::section("session"));
    let actions: Vec<(&str, &str, &str, bool)> = {
        let mut v = vec![
            ("󰌾", "lock", "omarchy-system-lock", false),
            (
                "󱄄",
                "screensaver",
                "omarchy-launch-screensaver force",
                false,
            ),
        ];
        if !std::path::Path::new(&util::home().join(".local/state/omarchy/toggles/suspend-off"))
            .exists()
        {
            v.push(("󰒲", "suspend", "systemctl suspend", false));
        }
        if util::sh_out(
            "omarchy-hibernation-available && echo y",
            Duration::from_millis(800),
        )
        .as_deref()
            == Some("y")
        {
            v.push(("󰤁", "hibernate", "systemctl hibernate", true));
        }
        v.push(("󰍃", "log out", "omarchy-system-logout", true));
        v.push(("󰜉", "restart", "omarchy-system-reboot", true));
        v.push(("󰐥", "shut down", "omarchy-system-shutdown", true));
        v
    };
    for (icon, name, cmd, confirm) in actions {
        let row = ui::Row::new(icon, name, "");
        let armed = Rc::new(Cell::new(false));
        let meta = row.meta.clone();
        let cmd = cmd.to_string();
        let p2 = p.clone();
        let root = row.root.clone();
        ui::on_click(&row.root, move || {
            if confirm && !armed.get() {
                armed.set(true);
                meta.set_markup("<span color='#bf616a'>click again</span>");
                root.add_css_class("sel");
                let (armed, meta, root) = (armed.clone(), meta.clone(), root.clone());
                glib::timeout_add_local_once(Duration::from_secs(3), move || {
                    armed.set(false);
                    meta.set_markup("");
                    root.remove_css_class("sel");
                });
                return;
            }
            p2.close();
            util::spawn_sh(&cmd);
        });
        body.append(&row.root);
    }
    card.root.upcast()
}
