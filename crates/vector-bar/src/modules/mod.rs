//! Bar modules. Each module is created once and shows itself on every bar.

use std::rc::Rc;

use crate::config::{Config, Opts};
use crate::slot::Slot;
use crate::theme::Theme;

pub mod ai;
pub mod battery;
pub mod bluetooth;
pub mod clipboard;
pub mod clock;
pub mod custom;
pub mod headphones;
pub mod homelab;
pub mod indicators;
pub mod media;
pub mod menu;
pub mod network;
pub mod notifications;
pub mod phone;
pub mod sysmon;
pub mod tray;
pub mod vpn;
pub mod window;
pub mod workspaces;

/// Shared, read-only context for building modules.
pub struct Ctx {
    pub cfg: Rc<Config>,
    pub theme: Rc<Theme>,
    pub lua: Option<Rc<crate::lua::Runtime>>,
}

impl Ctx {
    pub fn opts(&self, name: &str) -> Opts<'_> {
        self.cfg.module(name)
    }
    pub fn slot(&self, name: &str, classes: &[&str]) -> Slot {
        Slot::new(name, self.cfg.bind.get(name), classes)
    }
}

/// Which bar a view is for.
pub struct BarInfo {
    pub connector: String,
}

pub trait Module {
    /// The widget(s) for one bar.
    fn view(&self, bar: &BarInfo) -> gtk::Widget;
    /// `vector-bar refresh <name>`: re-read state now.
    fn refresh(&self) {}
    /// Slots with panels, for `vector-bar toggle <name>`.
    fn slots(&self) -> Vec<Slot> {
        Vec::new()
    }
}

pub fn build(name: &str, ctx: &Ctx) -> Option<Rc<dyn Module>> {
    if let Some(custom) = name.strip_prefix("custom:") {
        return ctx
            .cfg
            .custom
            .iter()
            .find(|c| c.name == custom)
            .map(|c| Rc::new(custom::Custom::new(c, ctx)) as Rc<dyn Module>);
    }
    Some(match name {
        "mode" | "menu" => Rc::new(menu::Menu::new(ctx)),
        "window" => Rc::new(window::Window::new(ctx)),
        "workspaces" => Rc::new(workspaces::Workspaces::new(ctx)),
        "clock" => Rc::new(clock::Clock::new(ctx)),
        "media" => Rc::new(media::Media::new(ctx)),
        "indicators" => Rc::new(indicators::Indicators::new(ctx)),
        "profile" => Rc::new(indicators::Profile::new(ctx)),
        "homelab" => Rc::new(homelab::Homelab::new(ctx)),
        "phone" => Rc::new(phone::Phone::new(ctx)),
        "earbuds" => Rc::new(headphones::Headphones::earbuds(ctx)),
        "sony" => Rc::new(headphones::Headphones::sony(ctx)),
        "ai" => Rc::new(ai::Ai::shared(ctx)),
        "agents" => Rc::new(ai::AgentsLine::shared(ctx)),
        "clipboard" => Rc::new(clipboard::Clipboard::new(ctx)),
        "notifications" => Rc::new(notifications::Notifications::new(ctx)),
        "bluetooth" => Rc::new(bluetooth::Bluetooth::new(ctx)),
        "vpn" => Rc::new(vpn::Vpn::new(ctx)),
        "network" => Rc::new(network::Network::new(ctx)),
        "sysmon" => Rc::new(sysmon::Sysmon::new(ctx)),
        "battery" => Rc::new(battery::Battery::new(ctx)),
        "tray" => Rc::new(tray::Tray::new(ctx)),
        _ => {
            // a Lua plugin of that name (plugins may also replace nothing built in)
            if let Some(rt) = ctx.lua.as_ref().filter(|rt| rt.has(name)) {
                return rt.build(name, ctx);
            }
            eprintln!("vector-bar: unknown module {name:?}");
            return None;
        }
    })
}
