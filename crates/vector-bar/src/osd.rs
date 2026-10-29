//! The on-screen display: volume, microphone, display and keyboard
//! brightness, media keys, output switching and short messages, as a small
//! statusline near the bottom of the focused screen. Replaces swayosd:
//! `~/.local/bin/swayosd-client` hands its arguments here, so Omarchy's keys
//! and scripts drive it unchanged.
//!
//!   vector-bar osd --output-volume raise|lower|mute-toggle|+N|-N
//!   vector-bar osd --input-volume  raise|lower|mute-toggle|+N|-N
//!   vector-bar osd --brightness    raise|lower|+N|-N|N
//!   vector-bar osd --playerctl     play-pause|next|previous|play|pause|stop
//!   vector-bar osd --custom-message TEXT [--custom-icon NAME]
//!   vector-bar osd --custom-progress 0.62 [--custom-progress-text 62%] [--custom-icon NAME]
//!
//! Known icon names (display-brightness, keyboard-brightness, microphone,
//! sink-volume, touchpad...) turn a plain swayosd message into the full view
//! for that thing, read from the system.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use serde::{Deserialize, Serialize};

use crate::ui;
use crate::util;

/// `[osd]` in config.toml.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// ms on screen after the last change
    pub timeout: u64,
    /// volume ceiling in percent (above 100 amplifies)
    pub max_volume: u32,
    /// volume / brightness step for raise / lower, percent
    pub step: u32,
    /// px from the bottom of the screen
    pub margin: i32,
    pub width: i32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            timeout: 1600,
            max_volume: 100,
            step: 5,
            margin: 72,
            width: 400,
        }
    }
}

// ── what is shown ───────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
struct View {
    block: String,
    crit: bool,
    glyph: String,
    title: String,
    detail: String,
    /// 0..1 (more when amplified)
    value: Option<f64>,
    value_text: String,
    /// meter as this many cells (keyboard levels); else 20 cells of 5%
    cells: Option<u32>,
    /// a plain bar instead of cells (track position)
    smooth: bool,
    /// outputs to pick from, the current one marked
    list: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq)]
enum Req {
    Volume {
        input: bool,
        arg: String,
    },
    Brightness(String),
    Player(String),
    Custom {
        msg: String,
        icon: String,
        progress: Option<f64>,
        progress_text: String,
    },
}

fn parse(args: &[&str]) -> Option<Req> {
    let mut it = args.iter().peekable();
    let (mut msg, mut icon, mut progress, mut ptext) =
        (String::new(), String::new(), None, String::new());
    let mut req = None;
    while let Some(a) = it.next() {
        let (k, inline) = match a.split_once('=') {
            Some((k, v)) => (k, Some(v.to_string())),
            None => (*a, None),
        };
        let mut val = || {
            inline
                .clone()
                .or_else(|| it.next().map(|s| s.to_string()))
                .unwrap_or_default()
        };
        match k {
            "--output-volume" | "-o" => {
                req = Some(Req::Volume {
                    input: false,
                    arg: val(),
                })
            }
            "--input-volume" | "-i" => {
                req = Some(Req::Volume {
                    input: true,
                    arg: val(),
                })
            }
            "--brightness" | "-b" => req = Some(Req::Brightness(val())),
            "--playerctl" | "-p" => req = Some(Req::Player(val())),
            "--custom-message" | "-m" => msg = val(),
            "--custom-icon" => icon = val(),
            "--custom-progress" => progress = val().parse::<f64>().ok(),
            "--custom-progress-text" => ptext = val(),
            "--custom-segmented-progress" => {
                // "3:5" style: value:of
                let v = val();
                if let Some((a, b)) = v.split_once(':')
                    && let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>())
                {
                    progress = Some(a / b.max(1.0));
                }
            }
            "--monitor"
            | "--device"
            | "--max-volume"
            | "--player"
            | "--top-margin"
            | "--custom-icon-color" => {
                val();
            }
            _ => {}
        }
    }
    req.or_else(|| {
        (!msg.is_empty() || progress.is_some() || !icon.is_empty()).then_some(Req::Custom {
            msg,
            icon,
            progress,
            progress_text: ptext,
        })
    })
}

// ── reading and changing things (worker thread) ─────────────────────────

fn sh(argv: &[&str]) -> Option<String> {
    util::run(argv, Duration::from_secs(2))
        .filter(|o| o.ok)
        .map(|o| o.stdout)
}

fn pct(v: f64) -> String {
    format!("{}%", (v * 100.0).round() as i64)
}

/// The default sink or source as pactl describes it.
fn pa_default(input: bool) -> Option<serde_json::Value> {
    let name = sh(&[
        "pactl",
        if input {
            "get-default-source"
        } else {
            "get-default-sink"
        },
    ])?
    .trim()
    .to_string();
    let list: serde_json::Value = serde_json::from_str(&sh(&[
        "pactl",
        "-f",
        "json",
        "list",
        if input { "sources" } else { "sinks" },
    ])?)
    .ok()?;
    list.as_array()?
        .iter()
        .find(|s| s["name"].as_str() == Some(name.as_str()))
        .cloned()
}

fn pa_volume(s: &serde_json::Value) -> f64 {
    let Some(ch) = s["volume"].as_object() else {
        return 0.0;
    };
    let vals: Vec<f64> = ch
        .values()
        .filter_map(|c| c["value_percent"].as_str())
        .filter_map(|p| p.trim_end_matches('%').parse::<f64>().ok())
        .collect();
    if vals.is_empty() {
        0.0
    } else {
        vals.iter().sum::<f64>() / vals.len() as f64 / 100.0
    }
}

fn prop<'a>(s: &'a serde_json::Value, k: &str) -> &'a str {
    s["properties"][k].as_str().unwrap_or("")
}

/// "Speakers" / "WH-1000XM5", "built-in · ALC285" / "bluetooth · LDAC", glyph.
fn describe(s: &serde_json::Value, input: bool) -> (String, String, String) {
    let bus = prop(s, "device.bus");
    let api = prop(s, "device.api");
    let form = prop(s, "device.form_factor");
    let desc = s["description"].as_str().unwrap_or("");
    let port = s["active_port"]
        .as_str()
        .and_then(|ap| {
            s["ports"]
                .as_array()?
                .iter()
                .find(|p| p["name"].as_str() == Some(ap))?["description"]
                .as_str()
        })
        .unwrap_or("");
    let bt = bus == "bluetooth" || api == "bluez5";
    let usb = bus == "usb";
    let hdmi = port.to_lowercase().contains("hdmi")
        || desc.to_lowercase().contains("hdmi")
        || port.to_lowercase().contains("displayport");
    let glyph = if input {
        if bt || form == "headset" {
            "󰋎"
        } else {
            "󰍬"
        }
    } else if form == "headset" {
        "󰋎"
    } else if bt || form == "headphone" || port.to_lowercase().contains("headphone") {
        "󰋋"
    } else if hdmi {
        "󰍹"
    } else {
        "󰓃"
    };
    let (title, detail) = if bt {
        let codec = prop(s, "api.bluez5.codec").to_uppercase();
        let d = if codec.is_empty() {
            "bluetooth".to_string()
        } else {
            format!("bluetooth · {codec}")
        };
        (desc.to_string(), d)
    } else if !port.is_empty() && !hdmi {
        let nick = prop(s, "node.nick");
        let mut d = if usb {
            "usb".to_string()
        } else {
            "built-in".to_string()
        };
        if !nick.is_empty() {
            d = format!("{d} · {nick}");
        }
        (port.to_string(), d)
    } else {
        (
            desc.to_string(),
            if usb {
                "usb".into()
            } else if hdmi {
                "display audio".into()
            } else {
                String::new()
            },
        )
    };
    (title, detail, glyph.to_string())
}

#[derive(Default)]
struct WorkerState {
    /// (pct, muted, when) for [sink, source]
    vol: [Option<(i32, bool, Instant)>; 2],
    /// cached (title, detail, glyph, when) for [sink, source]
    sink_meta: [Option<(String, String, String, Instant)>; 2],
    /// cached (title, detail, when) for display brightness
    display_meta: Option<(String, String, Instant)>,
}

static STATE: std::sync::LazyLock<Mutex<WorkerState>> =
    std::sync::LazyLock::new(|| Mutex::new(WorkerState::default()));

/// Fast volume + mute read via `wpctl get-volume` ("Volume: 0.50 [MUTED]").
fn wp_volume(input: bool) -> Option<(f64, bool)> {
    let dev = if input {
        "@DEFAULT_AUDIO_SOURCE@"
    } else {
        "@DEFAULT_AUDIO_SINK@"
    };
    let out = sh(&["wpctl", "get-volume", dev])?;
    let muted = out.contains("MUTED");
    let v = out
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<f64>().ok())?;
    Some((v, muted))
}

fn sink_describe(input: bool, force: bool) -> Option<(String, String, String)> {
    let idx = usize::from(input);
    if !force
        && let Ok(st) = STATE.lock()
        && let Some((t, d, g, at)) = &st.sink_meta[idx]
        && at.elapsed() < Duration::from_millis(2500)
    {
        return Some((t.clone(), d.clone(), g.clone()));
    }
    let s = pa_default(input)?;
    let (t, d, g) = describe(&s, input);
    if let Ok(mut st) = STATE.lock() {
        st.sink_meta[idx] = Some((t.clone(), d.clone(), g.clone(), Instant::now()));
    }
    Some((t, d, g))
}

fn volume_view_with(input: bool, v_override: Option<(f64, bool)>, force_meta: bool) -> View {
    let (v, muted) = match v_override.or_else(|| wp_volume(input)) {
        Some(x) => x,
        None => {
            let Some(s) = pa_default(input) else {
                return View {
                    block: if input { "MIC" } else { "VOLUME" }.into(),
                    title: "no device".into(),
                    crit: true,
                    ..Default::default()
                };
            };
            (pa_volume(&s), s["mute"].as_bool().unwrap_or(false))
        }
    };
    let Some((title, detail, glyph)) = sink_describe(input, force_meta) else {
        return View {
            block: if input { "MIC" } else { "VOLUME" }.into(),
            title: "no device".into(),
            crit: true,
            ..Default::default()
        };
    };
    let block = match (input, muted) {
        (false, false) => "VOLUME",
        (false, true) => "MUTED",
        (true, false) => "MIC",
        (true, true) => "MIC OFF",
    };
    View {
        block: block.into(),
        crit: muted,
        glyph: if muted && !input {
            "󰝟".into()
        } else if muted {
            "󰍭".into()
        } else {
            glyph
        },
        title,
        detail,
        value: Some(v),
        value_text: if muted { "muted".into() } else { pct(v) },
        ..Default::default()
    }
}

fn volume_view(input: bool) -> View {
    volume_view_with(input, None, true)
}

fn step_volume_pct(cur: i32, arg: &str, cfg: &Config) -> Option<i32> {
    let step = cfg.step.max(1) as i32;
    let max = cfg.max_volume as i32;
    let next = match arg {
        "raise" => ((cur / step) + 1) * step,
        "lower" => ((cur + step - 1) / step - 1) * step,
        "mute-toggle" => return None,
        a => {
            if let Some(n) = a.strip_prefix('+').and_then(|n| n.parse::<i32>().ok()) {
                cur + n
            } else if let Some(n) = a.strip_prefix('-').and_then(|n| n.parse::<i32>().ok()) {
                cur - n
            } else if let Ok(n) = a.parse::<i32>() {
                n
            } else {
                return None;
            }
        }
    };
    Some(next.clamp(0, max))
}

fn apply_volume_steps(input: bool, args: &[String], cfg: &Config) -> View {
    let dev = if input {
        "@DEFAULT_AUDIO_SOURCE@"
    } else {
        "@DEFAULT_AUDIO_SINK@"
    };
    let idx = usize::from(input);
    let (hw_vol, hw_muted) = wp_volume(input).unwrap_or((0.0, false));
    let hw_pct = (hw_vol * 100.0).round() as i32;
    let (mut cur, mut muted) = {
        let st = STATE.lock().ok();
        match st.as_ref().and_then(|s| s.vol[idx]).filter(|(p, _, at)| {
            at.elapsed() < Duration::from_millis(2500) && (*p - hw_pct).abs() <= 8
        }) {
            Some((p, m, _)) => (p, m),
            None => (hw_pct, hw_muted),
        }
    };
    let mut changed_vol = false;
    let mut toggled_mute = false;
    for arg in args {
        if arg == "mute-toggle" {
            muted = !muted;
            toggled_mute = true;
        } else if let Some(next) = step_volume_pct(cur, arg, cfg) {
            if arg == "raise" || arg.starts_with('+') || !arg.starts_with('-') {
                muted = false;
            }
            cur = next;
            changed_vol = true;
        }
    }
    if toggled_mute && !changed_vol {
        sh(&["wpctl", "set-mute", dev, if muted { "1" } else { "0" }]);
    } else if changed_vol {
        if hw_muted && !muted {
            sh(&["wpctl", "set-mute", dev, "0"]);
        }
        let limit = format!("{:.2}", cfg.max_volume as f64 / 100.0);
        sh(&["wpctl", "set-volume", "-l", &limit, dev, &format!("{cur}%")]);
    }
    if let Ok(mut st) = STATE.lock() {
        st.vol[idx] = Some((cur, muted, Instant::now()));
    }
    volume_view_with(input, Some((cur as f64 / 100.0, muted)), false)
}

fn backlight() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new("/sys/class/backlight");
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    // the same order Omarchy picks the device in
    let pick = names
        .iter()
        .find(|n| n.starts_with("amdgpu_bl"))
        .or_else(|| names.iter().find(|n| *n == "intel_backlight"))
        .or_else(|| names.iter().find(|n| n.starts_with("acpi_video")))
        .or_else(|| names.first())?;
    Some(dir.join(pick))
}

fn focused_monitor_json() -> Option<serde_json::Value> {
    crate::hypr::json("monitors")?
        .as_array()?
        .iter()
        .find(|m| m["focused"].as_bool() == Some(true))
        .cloned()
}

fn display_meta() -> (String, String) {
    if let Ok(st) = STATE.lock()
        && let Some((t, d, at)) = &st.display_meta
        && at.elapsed() < Duration::from_millis(2500)
    {
        return (t.clone(), d.clone());
    }
    let mon = focused_monitor_json();
    let connector = mon
        .as_ref()
        .and_then(|m| m["name"].as_str())
        .unwrap_or("display")
        .to_string();
    let model = mon
        .as_ref()
        .and_then(|m| m["model"].as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let builtin = connector.starts_with("eDP");
    let title = if builtin {
        "built-in display".to_string()
    } else if !model.is_empty() {
        model
    } else {
        connector.clone()
    };
    let mut extra = Vec::new();
    if !builtin && title != connector {
        extra.push(connector);
    }
    // night light: hyprsunset's current temperature, when it tints
    if util::proc_running("hyprsunset")
        && let Some(t) =
            sh(&["hyprctl", "hyprsunset", "temperature"]).and_then(|t| t.trim().parse::<u32>().ok())
        && t > 0
        && t < 6400
    {
        extra.push(format!("night light {t}K"));
    }
    if util::runtime_dir().join("lumend.sock").exists() {
        extra.push("adaptive".into());
    }
    let detail = extra.join(" · ");
    if let Ok(mut st) = STATE.lock() {
        st.display_meta = Some((title.clone(), detail.clone(), Instant::now()));
    }
    (title, detail)
}

fn display_view(known: Option<f64>) -> View {
    let value = known.or_else(|| {
        let d = backlight()?;
        let cur = util::read_num(d.join("brightness"))? as f64;
        let max = util::read_num(d.join("max_brightness"))? as f64;
        Some(cur / max.max(1.0))
    });
    let (title, detail) = display_meta();
    View {
        block: "DISPLAY".into(),
        glyph: match value {
            Some(v) if v < 0.34 => "󰃞".into(),
            Some(v) if v < 0.67 => "󰃟".into(),
            _ => "󰃠".into(),
        },
        title,
        detail,
        value,
        value_text: value.map(pct).unwrap_or_default(),
        ..Default::default()
    }
}

fn change_brightness(arg: &str, cfg: &Config) {
    let step = cfg.step;
    let target = match arg {
        "raise" => format!("{step}%+"),
        "lower" => format!("{step}%-"),
        a if a.starts_with('+') => format!("{}%+", a.trim_start_matches('+').trim_end_matches('%')),
        a if a.starts_with('-') => format!("{}%-", a.trim_start_matches('-').trim_end_matches('%')),
        a => format!("{}%", a.trim_end_matches('%')),
    };
    if let Some(d) =
        backlight().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    {
        sh(&["brightnessctl", "-d", &d, "set", &target]);
    }
}

fn keyboard_view() -> View {
    let led = std::fs::read_dir("/sys/class/leds").ok().and_then(|rd| {
        rd.flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().contains("kbd_backlight"))
    });
    let (cur, max) = led
        .as_ref()
        .and_then(|d| {
            Some((
                util::read_num(d.join("brightness"))?,
                util::read_num(d.join("max_brightness"))?,
            ))
        })
        .unwrap_or((0, 0));
    let off = cur == 0;
    View {
        block: "KEYBOARD".into(),
        glyph: if off { "󰌐".into() } else { "󰌌".into() },
        title: if off {
            "backlight off".into()
        } else {
            "backlight".into()
        },
        detail: if max > 0 {
            format!("level {cur} of {max}")
        } else {
            String::new()
        },
        value: (max > 0).then(|| cur as f64 / max as f64),
        value_text: if off {
            "off".into()
        } else {
            format!("{cur}/{max}")
        },
        cells: (max > 0 && max <= 10).then_some(max as u32),
        ..Default::default()
    }
}

fn clock(us: f64) -> String {
    let s = (us / 1_000_000.0).max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn player_view(action: &str) -> View {
    if !action.is_empty() {
        sh(&["playerctl", action]);
        // let the player catch up before asking it what it does now
        std::thread::sleep(Duration::from_millis(160));
    }
    let fmt = "{{status}}\t{{playerName}}\t{{title}}\t{{artist}}\t{{position}}\t{{mpris:length}}";
    let out = sh(&["playerctl", "metadata", "--format", fmt]).unwrap_or_default();
    let f: Vec<&str> = out.trim_end_matches('\n').split('\t').collect();
    if f.len() < 6 || f[0].is_empty() {
        return View {
            block: "MEDIA".into(),
            glyph: "󰝚".into(),
            title: "nothing playing".into(),
            ..Default::default()
        };
    }
    let playing = f[0] == "Playing";
    let (pos, len) = (
        f[4].parse::<f64>().unwrap_or(0.0),
        f[5].parse::<f64>().unwrap_or(0.0),
    );
    let block = match (action, playing) {
        ("next", _) => "NEXT",
        ("previous", _) => "PREVIOUS",
        (_, true) => "PLAYING",
        _ => "PAUSED",
    };
    let mut detail = vec![];
    if !f[3].is_empty() {
        detail.push(f[3].to_string());
    }
    detail.push(f[1].to_lowercase());
    View {
        block: block.into(),
        glyph: if playing {
            "󰐊".into()
        } else {
            "󰏤".into()
        },
        title: if f[2].is_empty() {
            "untitled".into()
        } else {
            f[2].to_string()
        },
        detail: detail.join(" · "),
        value: (len > 0.0).then(|| pos / len),
        value_text: if len > 0.0 {
            format!("{} / {}", clock(pos), clock(len))
        } else {
            String::new()
        },
        smooth: true,
        ..Default::default()
    }
}

/// Every output that can play right now, the default one marked.
fn outputs() -> Vec<(String, bool)> {
    let def = sh(&["pactl", "get-default-sink"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let Some(list) = sh(&["pactl", "-f", "json", "list", "sinks"])
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return Vec::new();
    };
    let Some(arr) = list.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|s| {
            s["ports"].as_array().is_none_or(|p| {
                p.is_empty()
                    || p.iter()
                        .any(|p| p["availability"].as_str() != Some("not available"))
            })
        })
        .map(|s| {
            let (t, _, g) = describe(s, false);
            (
                format!("{g}  {t}"),
                s["name"].as_str() == Some(def.as_str()),
            )
        })
        .collect()
}

fn custom_view(msg: &str, icon: &str, progress: Option<f64>, ptext: &str) -> View {
    let i = icon.to_lowercase();
    if i.starts_with("display-brightness") {
        return display_view(progress);
    }
    if i.starts_with("keyboard-brightness") {
        return keyboard_view();
    }
    if i.contains("microphone") || i.starts_with("audio-input") {
        return volume_view(true);
    }
    if i.starts_with("sink-volume") || i.starts_with("audio-volume") {
        let mut v = volume_view(false);
        v.block = "OUTPUT".into();
        v.list = outputs();
        if v.list.len() < 2 {
            v.list.clear();
        }
        return v;
    }
    let lower = msg.to_lowercase();
    let off = lower.contains("disabled") || lower.contains("off") || i.contains("disabled");
    let (block, glyph) = if i.contains("touchpad") {
        ("TOUCHPAD", if off { "󰤳" } else { "󰟸" })
    } else if i.contains("touch") {
        ("TOUCH", if off { "󰹆" } else { "󰆽" })
    } else {
        ("VectorOS", "")
    };
    View {
        block: block.into(),
        crit: false,
        glyph: glyph.into(),
        title: msg.to_string(),
        value: progress,
        value_text: if ptext.is_empty() {
            progress.map(pct).unwrap_or_default()
        } else {
            ptext.to_string()
        },
        ..Default::default()
    }
}

fn work_batch(batch: Vec<(u64, Req, Config)>) -> Option<(u64, View)> {
    let (last_seq, last_req, last_cfg) = batch.last()?.clone();
    // Coalesce consecutive volume steps on the same device so key-repeat never lags or races.
    if let Req::Volume { input, .. } = &last_req {
        let input = *input;
        let args: Vec<String> = batch
            .iter()
            .filter_map(|(_, r, _)| match r {
                Req::Volume { input: i, arg } if *i == input => Some(arg.clone()),
                _ => None,
            })
            .collect();
        return Some((last_seq, apply_volume_steps(input, &args, &last_cfg)));
    }
    let view = match &last_req {
        Req::Volume { input, arg } => {
            apply_volume_steps(*input, std::slice::from_ref(arg), &last_cfg)
        }
        Req::Brightness(_) => {
            for (_, r, c) in &batch {
                if let Req::Brightness(a) = r {
                    change_brightness(a, c);
                }
            }
            display_view(None)
        }
        Req::Player(a) => player_view(a),
        Req::Custom {
            msg,
            icon,
            progress,
            progress_text,
        } => custom_view(msg, icon, *progress, progress_text),
    };
    Some((last_seq, view))
}

// ── the window ──────────────────────────────────────────────────────────

type MeterState = Rc<RefCell<(f64, Option<u32>, bool, bool)>>;

struct Widgets {
    card: gtk::Box,
    block: gtk::Label,
    slant: gtk::DrawingArea,
    slant_crit: Rc<Cell<bool>>,
    glyph: gtk::Label,
    title: gtk::Label,
    detail: gtk::Label,
    value: gtk::Label,
    meter: gtk::DrawingArea,
    meter_state: MeterState,
    list: gtk::Box,
}

struct Osd {
    app: gtk::Application,
    win: RefCell<Option<gtk::ApplicationWindow>>,
    widgets: RefCell<Option<Widgets>>,
    hide: RefCell<Option<glib::SourceId>>,
    seq: Cell<u64>,
    shown_seq: Cell<u64>,
    cfg: RefCell<Config>,
    monitor: RefCell<Option<String>>,
}

type Queue = (Mutex<VecDeque<(u64, Req, Config)>>, Condvar);

static QUEUE: std::sync::LazyLock<Queue> =
    std::sync::LazyLock::new(|| (Mutex::new(VecDeque::new()), Condvar::new()));

thread_local! {
    static OSD: RefCell<Option<Rc<Osd>>> = const { RefCell::new(None) };
    static WORKER_STARTED: Cell<bool> = const { Cell::new(false) };
}

fn ensure_worker() {
    if WORKER_STARTED.with(|s| s.replace(true)) {
        return;
    }
    let (tx, rx) = async_channel::unbounded::<(u64, View)>();
    std::thread::spawn(move || {
        let (lock, cvar) = &*QUEUE;
        loop {
            let batch: Vec<(u64, Req, Config)> = {
                let mut q = lock.lock().unwrap();
                while q.is_empty() {
                    q = cvar.wait(q).unwrap();
                }
                q.drain(..).collect()
            };
            if let Some(res) = work_batch(batch)
                && tx.send_blocking(res).is_err()
            {
                break;
            }
        }
    });
    glib::spawn_future_local(async move {
        while let Ok((seq, view)) = rx.recv().await {
            let Some(o) = osd() else { continue };
            if seq < o.shown_seq.get() {
                continue;
            }
            o.shown_seq.set(seq);
            o.show(&view);
        }
    });
}

pub fn init(app: &gtk::Application, cfg: &Config) {
    ensure_worker();
    let existing = OSD.with(|o| o.borrow().clone());
    match existing {
        Some(o) => {
            *o.cfg.borrow_mut() = cfg.clone();
            *o.widgets.borrow_mut() = None;
        }
        None => {
            let o = Rc::new(Osd {
                app: app.clone(),
                win: RefCell::default(),
                widgets: RefCell::default(),
                hide: RefCell::default(),
                seq: Cell::new(0),
                shown_seq: Cell::new(0),
                cfg: RefCell::new(cfg.clone()),
                monitor: RefCell::default(),
            });
            OSD.with(|x| *x.borrow_mut() = Some(o));
        }
    }
}

fn osd() -> Option<Rc<Osd>> {
    OSD.with(|o| o.borrow().clone())
}

/// `vector-bar osd ...` (swayosd-client's arguments). Returns an error text.
pub fn request(args: &[&str]) -> String {
    let Some(req) = parse(args) else {
        return "osd: nothing to show (see vector-bar --help)".into();
    };
    let Some(o) = osd() else { return String::new() };
    ensure_worker();
    let seq = o.seq.get() + 1;
    o.seq.set(seq);
    let cfg = o.cfg.borrow().clone();
    let (lock, cvar) = &*QUEUE;
    if let Ok(mut q) = lock.lock() {
        q.push_back((seq, req, cfg));
        cvar.notify_one();
    }
    String::new()
}

/// A message from elsewhere in the bar (notification rules with `osd = true`).
pub fn message(block: &str, title: &str, detail: &str) {
    let Some(o) = osd() else { return };
    // "󰂛    Silenced notifications": a leading glyph becomes the icon
    let mut t = title.trim().to_string();
    let mut glyph = String::new();
    if let Some(c) = t
        .chars()
        .next()
        .filter(|c| ('\u{e000}'..='\u{f8ff}').contains(c) || *c >= '\u{f0000}')
    {
        glyph = c.to_string();
        t = t[c.len_utf8()..].trim().to_string();
    }
    let block = if block == "VectorOS" && t.to_lowercase().contains("silenced") {
        "SILENT".to_string()
    } else {
        block.to_string()
    };
    o.seq.set(o.seq.get() + 1);
    o.shown_seq.set(o.seq.get());
    o.show(&View {
        block,
        glyph,
        title: t.to_lowercase(),
        detail: detail.to_string(),
        ..Default::default()
    });
}

fn focused_monitor() -> Option<(gdk::Monitor, String)> {
    let name = focused_monitor_json().and_then(|m| m["name"].as_str().map(str::to_string));
    let d = gdk::Display::default()?;
    let ms = d.monitors();
    let all: Vec<gdk::Monitor> = (0..ms.n_items())
        .filter_map(|i| ms.item(i).and_downcast::<gdk::Monitor>())
        .collect();
    let m = name
        .as_ref()
        .and_then(|n| {
            all.iter()
                .find(|m| m.connector().is_some_and(|c| c == *n))
                .cloned()
        })
        .or_else(|| all.first().cloned())?;
    let c = m.connector().map(|c| c.to_string()).unwrap_or_default();
    Some((m, c))
}

impl Osd {
    fn show(self: &Rc<Self>, v: &View) {
        let cfg = self.cfg.borrow().clone();
        let already_visible = self.win.borrow().as_ref().is_some_and(|w| w.is_visible());
        if !already_visible {
            let mon = focused_monitor();
            let moved = mon.as_ref().map(|(_, c)| c.clone()) != *self.monitor.borrow();
            if moved && let Some(w) = self.win.borrow_mut().take() {
                w.destroy();
                *self.widgets.borrow_mut() = None;
            }
            if self.win.borrow().is_none() {
                let w = gtk::ApplicationWindow::new(&self.app);
                w.add_css_class("vosd");
                w.init_layer_shell();
                w.set_layer(Layer::Overlay);
                w.set_namespace(Some("vector-osd"));
                w.set_keyboard_mode(KeyboardMode::None);
                w.set_exclusive_zone(-1);
                w.set_anchor(Edge::Bottom, true);
                w.set_margin(Edge::Bottom, cfg.margin);
                if let Some((m, c)) = &mon {
                    w.set_monitor(Some(m));
                    *self.monitor.borrow_mut() = Some(c.clone());
                }
                // never in the way of the pointer
                w.connect_map(|w| {
                    if let Some(s) = w.surface() {
                        s.set_input_region(Some(&gtk::cairo::Region::create()));
                    }
                });
                *self.win.borrow_mut() = Some(w);
            }
        }
        let Some(win) = self.win.borrow().clone() else {
            return;
        };
        if self.widgets.borrow().is_none() {
            let ws = build_widgets(cfg.width);
            win.set_child(Some(&ws.card));
            *self.widgets.borrow_mut() = Some(ws);
        }
        if let Some(ws) = self.widgets.borrow().as_ref() {
            update_widgets(ws, v, cfg.width);
        }
        if !win.is_visible() {
            win.present();
        }
        if let Some(id) = self.hide.borrow_mut().take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id =
            glib::timeout_add_local_once(Duration::from_millis(cfg.timeout.max(300)), move || {
                if let Some(o) = w.upgrade() {
                    o.hide.borrow_mut().take();
                    if let Some(win) = o.win.borrow().as_ref() {
                        win.set_visible(false);
                    }
                }
            });
        *self.hide.borrow_mut() = Some(id);
    }
}

fn build_widgets(width: i32) -> Widgets {
    let t = crate::panel::current_theme();
    let card = ui::vbox(0, &["card", "osd"]);
    card.set_size_request(width, -1);
    let head = ui::hbox(0, &["head"]);
    let block = ui::text("", &["htitle"]);
    block.set_valign(gtk::Align::Fill);
    head.append(&block);
    let slant_crit = Rc::new(Cell::new(false));
    let slant_area = slant(&t.accent, &t.color1, slant_crit.clone());
    head.append(&slant_area);
    let glyph = ui::text("", &["oglyph"]);
    glyph.set_margin_start(10);
    head.append(&glyph);
    let title = ui::text("", &["otitle"]);
    title.set_margin_start(8);
    ui::ellipsize(&title, 24);
    head.append(&title);
    let detail = ui::text("", &["hdetail"]);
    detail.set_margin_start(8);
    ui::ellipsize(&detail, 22);
    head.append(&detail);
    head.append(&ui::spacer());
    let value = ui::text("", &["ovalue"]);
    value.set_margin_start(12);
    head.append(&value);
    card.append(&head);

    let meter_state = Rc::new(RefCell::new((0.0, None, false, false)));
    let meter_area = meter(meter_state.clone(), &t);
    card.append(&meter_area);

    let list = ui::vbox(0, &["olist"]);
    card.append(&list);

    Widgets {
        card,
        block,
        slant: slant_area,
        slant_crit,
        glyph,
        title,
        detail,
        value,
        meter: meter_area,
        meter_state,
        list,
    }
}

fn update_widgets(ws: &Widgets, v: &View, width: i32) {
    ws.card.set_size_request(width, -1);
    if v.crit {
        ws.card.add_css_class("crit-block");
    } else {
        ws.card.remove_css_class("crit-block");
    }
    ws.block.set_text(&v.block);
    if ws.slant_crit.replace(v.crit) != v.crit {
        ws.slant.queue_draw();
    }
    ws.glyph.set_text(&v.glyph);
    ws.glyph.set_visible(!v.glyph.is_empty());
    ws.title.set_text(&v.title);
    ws.detail.set_text(&v.detail);
    ws.detail.set_visible(!v.detail.is_empty());
    ws.value.set_text(&v.value_text);
    ws.value.set_visible(!v.value_text.is_empty());
    match v.value {
        Some(val) => {
            *ws.meter_state.borrow_mut() = (val, v.cells, v.smooth, v.crit);
            ws.meter.set_visible(true);
            ws.meter.queue_draw();
        }
        None => {
            ws.meter.set_visible(false);
        }
    }
    ui::clear(&ws.list);
    if !v.list.is_empty() {
        for (name, cur) in &v.list {
            let r = ui::text(name, &[if *cur { "ocur" } else { "dim" }]);
            r.set_xalign(0.0);
            ws.list.append(&r);
        }
        ws.list.set_visible(true);
    } else {
        ws.list.set_visible(false);
    }
}

fn slant(accent_hex: &str, crit_hex: &str, crit: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(10);
    d.set_vexpand(true);
    let acc = gdk::RGBA::parse(accent_hex).unwrap_or(gdk::RGBA::WHITE);
    let red = gdk::RGBA::parse(crit_hex).unwrap_or(gdk::RGBA::RED);
    d.set_draw_func(move |_, cr, w, h| {
        cr.move_to(0.0, 0.0);
        cr.line_to(w as f64, 0.0);
        cr.line_to(0.0, h as f64);
        cr.close_path();
        cr.set_source_color(if crit.get() { &red } else { &acc });
        let _ = cr.fill();
    });
    d
}

/// Cells of 5% (one per key press), or the device's own levels, or a plain
/// bar for a track. Past 100% the extra cells turn warm.
fn meter(state: MeterState, t: &crate::theme::Theme) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.add_css_class("ometer");
    d.set_content_height(8);
    d.set_hexpand(true);
    let rgba = |hex: &str, a: f32| {
        let c = gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::WHITE);
        gdk::RGBA::new(c.red(), c.green(), c.blue(), a)
    };
    let on_normal = rgba(&t.accent, 1.0);
    let on_muted = rgba(&t.foreground, 0.30);
    let over = rgba(&t.warn, 1.0);
    let off = rgba(&t.foreground, 0.10);
    d.set_draw_func(move |_, cr, w, h| {
        let (value, cells, smooth, muted) = *state.borrow();
        let on = if muted { &on_muted } else { &on_normal };
        let (w, h) = (w as f64, h as f64);
        if smooth {
            cr.set_source_color(&off);
            cr.rectangle(0.0, 0.0, w, h);
            let _ = cr.fill();
            cr.set_source_color(on);
            cr.rectangle(0.0, 0.0, w * value.clamp(0.0, 1.0), h);
            let _ = cr.fill();
            return;
        }
        // amplified: the scale stretches to 150%
        let top = if value > 1.0 { 1.5 } else { 1.0 };
        let n = cells.unwrap_or(if top > 1.0 { 30 } else { 20 }) as f64;
        let gap = 3.0;
        let cw = ((w - gap * (n - 1.0)) / n).max(1.0);
        let filled = (value / top * n).round();
        for i in 0..n as i32 {
            let x = i as f64 * (cw + gap);
            let i = i as f64;
            let c = if i < filled {
                if (i + 1.0) / n * top > 1.0 + 1e-9 {
                    &over
                } else {
                    on
                }
            } else {
                &off
            };
            cr.set_source_color(c);
            cr.rectangle(x, 0.0, cw, h);
            let _ = cr.fill();
        }
    });
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swayosd_args() {
        assert_eq!(
            parse(&["--monitor", "eDP-1", "--output-volume", "raise"]),
            Some(Req::Volume {
                input: false,
                arg: "raise".into()
            })
        );
        assert_eq!(
            parse(&["--output-volume=+1"]),
            Some(Req::Volume {
                input: false,
                arg: "+1".into()
            })
        );
        assert_eq!(
            parse(&["--playerctl", "next"]),
            Some(Req::Player("next".into()))
        );
        assert_eq!(
            parse(&[
                "--custom-icon",
                "display-brightness-symbolic",
                "--custom-progress",
                "0.62",
                "--custom-progress-text",
                "62%"
            ]),
            Some(Req::Custom {
                msg: String::new(),
                icon: "display-brightness-symbolic".into(),
                progress: Some(0.62),
                progress_text: "62%".into()
            })
        );
        assert_eq!(parse(&["--monitor", "x"]), None);
    }
}
