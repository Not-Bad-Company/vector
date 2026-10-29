//! `~/.config/vector-bar/config.toml`. Every key is optional; see
//! `data/config.toml` for the documented defaults.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::util;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub height: i32,
    pub font: String,
    pub font_size: f64,
    /// Bar background opacity in glass mode (desktop-glass): 1 = solid.
    pub opacity: f64,
    /// Bar colour for warm/low states (theme yellow is often too loud on a bar).
    pub warn: Option<String>,
    pub crit: Option<String>,
    pub left: Vec<String>,
    pub center: Vec<String>,
    pub right: Vec<String>,
    /// Modules drawn as solid blocks in the mode colour (nvim lualine's mode/position).
    pub blocks: Vec<String>,
    /// Modules on the tinted segment next to a block.
    pub segments: Vec<String>,
    /// Per-module options: `[modules.clock] format = "..."`.
    pub modules: HashMap<String, toml::Table>,
    /// Click overrides: `[bind.network] right = "impala"`.
    pub bind: HashMap<String, Bind>,
    /// Waybar-style script modules.
    pub custom: Vec<Custom>,
    /// Built-in notification server and rules.
    pub notifications: crate::notify::Config,
    /// On-screen display (volume, brightness, media keys).
    pub osd: crate::osd::Config,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bind {
    pub left: Option<String>,
    pub middle: Option<String>,
    pub right: Option<String>,
    pub scroll_up: Option<String>,
    pub scroll_down: Option<String>,
}

/// A module driven by an external command, compatible with Waybar's
/// `custom/*` (`return-type: json` = `{text, tooltip, class, alt, percentage}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Custom {
    pub name: String,
    pub exec: String,
    /// Seconds between runs; 0 = run once and keep reading lines (streaming).
    pub interval: f64,
    pub json: bool,
    /// Pango markup allowed in text (Waybar `escape: false`).
    pub markup: bool,
    pub format: Option<String>,
    pub max_length: Option<usize>,
    pub on_click: Option<String>,
    pub on_click_middle: Option<String>,
    pub on_click_right: Option<String>,
    pub on_scroll_up: Option<String>,
    pub on_scroll_down: Option<String>,
}

impl Default for Custom {
    fn default() -> Self {
        Self {
            name: String::new(),
            exec: String::new(),
            interval: 5.0,
            json: true,
            markup: true,
            format: None,
            max_length: None,
            on_click: None,
            on_click_middle: None,
            on_click_right: None,
            on_scroll_up: None,
            on_scroll_down: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
        Self {
            height: 26,
            font: "JetBrainsMonoNL Nerd Font".into(),
            font_size: 12.0,
            // a light blur leaves bright wallpaper details as specks through a thin bar
            opacity: 0.8,
            warn: None,
            crit: None,
            left: v(&["mode", "workspaces", "agents"]),
            center: v(&["media", "indicators"]),
            right: v(&[
                "tray",
                "phone",
                "ai",
                "bluetooth",
                "vpn",
                "network",
                "profile",
                "sysmon",
                "battery",
                "clock",
            ]),
            blocks: v(&["mode", "clock"]),
            segments: v(&["workspaces", "profile", "sysmon", "battery"]),
            modules: HashMap::new(),
            bind: HashMap::new(),
            custom: Vec::new(),
            notifications: crate::notify::Config::default(),
            osd: crate::osd::Config::default(),
        }
    }
}

pub fn path() -> std::path::PathBuf {
    util::config_dir().join("vector-bar/config.toml")
}

pub fn user_css() -> std::path::PathBuf {
    util::config_dir().join("vector-bar/style.css")
}

impl Config {
    /// Loads the config. A broken file falls back to defaults and returns the
    /// parse error so the bar can show it instead of dying.
    pub fn load() -> (Config, Option<String>) {
        match util::read(path()) {
            None => (Config::default(), None),
            Some(text) => match toml::from_str::<Config>(&text) {
                Ok(c) => (c, None),
                Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
            },
        }
    }

    pub fn module(&self, name: &str) -> Opts<'_> {
        Opts(self.modules.get(name))
    }
}

/// Typed access to one module's option table.
#[derive(Clone, Copy)]
pub struct Opts<'a>(Option<&'a toml::Table>);

impl Opts<'_> {
    pub fn str(&self, key: &str, default: &str) -> String {
        self.0
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    }
    pub fn num(&self, key: &str, default: f64) -> f64 {
        self.0
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(default)
    }
    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.0
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }
    pub fn list(&self, key: &str) -> Option<Vec<String>> {
        let a = self.0?.get(key)?.as_array()?;
        Some(
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        )
    }
}
