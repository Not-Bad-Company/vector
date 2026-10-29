//! Vector Bar: the Vector OS top bar.
//!
//!   vector-bar                    run the bar (single instance)
//!   vector-bar --test             run beside another bar (no exclusive zone)
//!   vector-bar toggle <panel>     open/close a panel (`clipboard@center` centres it)
//!   vector-bar refresh [module]   re-read a module now (replaces `pkill -RTMIN+N waybar`)
//!   vector-bar reload             re-read config, style and theme
//!   vector-bar close | modules | quit

mod bar;
mod config;
mod dbus;
mod hypr;
mod mako;
mod lua;
mod mode;
mod modules;
mod panel;
mod sheet;
mod slot;
mod theme;
mod ui;
mod util;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio};

use crate::config::Config;
use crate::modules::{Ctx, Module};
use crate::theme::Theme;

const APP_ID: &str = "org.vectoros.Bar";
const TEST_ID: &str = "org.vectoros.BarTest";
const USAGE: &str = "usage: vector-bar [--test] | toggle <panel>[@center] | refresh [module] | reload | close | modules | quit";

#[derive(Default)]
struct State {
    modules: HashMap<String, Rc<dyn Module>>,
    bars: Vec<gtk::ApplicationWindow>,
    css: Option<gtk::CssProvider>,
    user_css: Option<gtk::CssProvider>,
    error: Option<String>,
    reload_pending: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static TEST: Cell<bool> = const { Cell::new(false) };
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return glib::ExitCode::SUCCESS;
    }
    let test = args.iter().any(|a| a == "--test");
    TEST.with(|t| t.set(test));
    let app = gtk::Application::new(
        Some(if test { TEST_ID } else { APP_ID }),
        gio::ApplicationFlags::HANDLES_COMMAND_LINE,
    );

    // D-Bus actions, so a plain `gdbus call ... org.gtk.Actions.Activate` works too
    for name in ["toggle", "refresh"] {
        let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        a.connect_activate(move |_, p| {
            if let Some(s) = p.and_then(|v| v.str()) {
                command(&[name, s]);
            }
        });
        app.add_action(&a);
    }

    app.connect_startup(|app| {
        // the bar is the app: keep running with no windows (e.g. no monitor yet)
        std::mem::forget(app.hold());
        panel::init(app, set_open);
        reload(app);
        watch_files(app);
        watch_monitors(app);
    });

    app.connect_command_line(|app, cl| {
        let args: Vec<String> = cl.arguments().iter().map(|a| a.to_string_lossy().to_string()).collect();
        let words: Vec<&str> = args.iter().skip(1).map(String::as_str).filter(|a| *a != "--test").collect();
        if words.first() == Some(&"quit") {
            app.quit();
            return glib::ExitCode::SUCCESS;
        }
        if !words.is_empty() {
            let out = command(&words);
            if !out.is_empty() {
                cl.print_literal(&format!("{out}\n"));
            }
        }
        glib::ExitCode::SUCCESS
    });

    // a command (toggle, refresh, ...) must talk to a running bar, never become one
    let words: Vec<&String> = args.iter().skip(1).filter(|a| *a != "--test").collect();
    if !words.is_empty() && !running(if test { TEST_ID } else { APP_ID }) {
        eprintln!("vector-bar is not running{}", if test { " (test instance)" } else { "" });
        std::process::exit(1);
    }
    app.run()
}

/// Does a bar own its D-Bus name? (Asked without registering ourselves.)
fn running(id: &str) -> bool {
    let Ok(c) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    c.call_sync(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", Some(&(id,).to_variant()), None, gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE)
        .ok()
        .and_then(|v| v.child_value(0).get::<bool>())
        .unwrap_or(false)
}

fn command(words: &[&str]) -> String {
    match words {
        ["toggle", what] => {
            let (name, center) = match what.split_once('@') {
                Some((n, w)) => (n, w == "center"),
                None => (*what, false),
            };
            match find_slot(name) {
                Some(s) if s.has_panel() => {
                    s.toggle_panel(None, center);
                    String::new()
                }
                Some(_) => format!("{name} has no panel"),
                None => format!("no panel {name:?}"),
            }
        }
        ["refresh", name] => {
            let m = STATE.with(|s| s.borrow().modules.get(*name).cloned());
            match m {
                Some(m) => {
                    m.refresh();
                    String::new()
                }
                None => format!("no module {name:?}"),
            }
        }
        ["refresh"] => {
            let ms: Vec<_> = STATE.with(|s| s.borrow().modules.values().cloned().collect());
            for m in ms {
                m.refresh();
            }
            String::new()
        }
        ["reload"] => {
            if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>() {
                reload(&app);
            }
            STATE.with(|s| s.borrow().error.clone()).unwrap_or_default()
        }
        ["close"] => {
            panel::close();
            String::new()
        }
        ["modules"] => STATE.with(|s| {
            let mut v: Vec<_> = s.borrow().modules.keys().cloned().collect();
            v.sort();
            v.join("\n")
        }),
        _ => USAGE.into(),
    }
}

/// A slot by name: the module of that name, or any module's slot with that name.
fn find_slot(name: &str) -> Option<slot::Slot> {
    let (direct, all) = STATE.with(|s| {
        let s = s.borrow();
        (s.modules.get(name).cloned(), s.modules.values().cloned().collect::<Vec<_>>())
    });
    if let Some(m) = direct {
        if let Some(s) = m.slots().into_iter().find(|s| s.has_panel()) {
            return Some(s);
        }
    }
    all.iter().flat_map(|m| m.slots()).find(|s| s.name() == name)
}

fn set_open(name: &str, open: bool) {
    let all: Vec<_> = STATE.with(|s| s.borrow().modules.values().cloned().collect());
    for m in all {
        for s in m.slots() {
            if s.name() == name {
                s.set_open(open);
            }
        }
    }
}

/// (Re)builds everything from config, theme and CSS.
fn reload(app: &gtk::Application) {
    panel::close();
    let (cfg, err) = Config::load();
    let theme = Theme::load(cfg.warn.as_deref(), cfg.crit.as_deref());
    let Some(display) = gdk::Display::default() else { return };

    // styles: built-in sheet, then the user's on top
    let css = gtk::CssProvider::new();
    css.connect_parsing_error(|_, sec, err| eprintln!("vector-bar: built-in css {}: {err}", sec.to_str()));
    css.load_from_string(&theme.css(&cfg.font, cfg.font_size, cfg.height));
    let user = gtk::CssProvider::new();
    let mut error = err;
    if let Some(text) = util::read(config::user_css()) {
        let e = Rc::new(RefCell::new(None::<String>));
        let e2 = e.clone();
        user.connect_parsing_error(move |_, _, err| {
            e2.borrow_mut().get_or_insert_with(|| format!("style.css: {err}"));
        });
        user.load_from_string(&text);
        if error.is_none() {
            error = e.borrow().clone();
        }
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(old) = s.css.take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        if let Some(old) = s.user_css.take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        gtk::style_context_add_provider_for_display(&display, &user, gtk::STYLE_PROVIDER_PRIORITY_USER);
        s.css = Some(css);
        s.user_css = Some(user);
        s.error = error.clone();
    });
    if let Some(e) = &error {
        eprintln!("vector-bar: {e}");
    }

    // modules (dropping the old ones stops their timers)
    let theme_rc = Rc::new(theme);
    panel::set_theme(theme_rc.clone());
    let lua = lua::load();
    if let Some(rt) = &lua {
        let errs = rt.errors.borrow().join("\n");
        if !errs.is_empty() {
            error = Some(match error {
                Some(e) => format!("{e}\n{errs}"),
                None => errs,
            });
        }
    }
    let ctx = Ctx { cfg: Rc::new(cfg.clone()), theme: theme_rc.clone(), lua };
    let old = STATE.with(|s| std::mem::take(&mut s.borrow_mut().modules));
    drop(old);
    mode::reset();
    let mut modules: HashMap<String, Rc<dyn Module>> = HashMap::new();
    let mut sections: [bar::Section; 3] = Default::default();
    for (i, list) in [&cfg.left, &cfg.center, &cfg.right].into_iter().enumerate() {
        for name in list {
            let m = match modules.get(name) {
                Some(m) => m.clone(),
                None => match modules::build(name, &ctx) {
                    Some(m) => m,
                    None => continue,
                },
            };
            modules.insert(name.clone(), m.clone());
            sections[i].push((name.clone(), m));
        }
    }
    // a config/style error shows as a red mark on the left; hover for the message
    if let Some(e) = &error {
        let m: Rc<dyn Module> = Rc::new(modules::custom::ErrorMark::new(e, &ctx));
        sections[0].push(("error".into(), m.clone()));
        modules.insert("error".into(), m);
    }

    let old_bars = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.modules = modules;
        std::mem::take(&mut s.bars)
    });
    for b in old_bars {
        b.destroy();
    }
    let test = TEST.with(|t| t.get());
    panel::set_top_offset(if test { cfg.height } else { 0 });
    let monitors = display.monitors();
    let mut bars = Vec::new();
    for i in 0..monitors.n_items() {
        let Some(m) = monitors.item(i).and_downcast::<gdk::Monitor>() else { continue };
        let w = bar::build(app, &m, &cfg, &theme_rc, &sections, test);
        w.present();
        bars.push(w);
    }
    STATE.with(|s| s.borrow_mut().bars = bars);
}

fn schedule_reload() {
    let pending = STATE.with(|s| std::mem::replace(&mut s.borrow_mut().reload_pending, true));
    if pending {
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(400), move || {
        STATE.with(|s| s.borrow_mut().reload_pending = false);
        if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>() {
            reload(&app);
        }
    });
}

fn watch_files(app: &gtk::Application) {
    let _ = app;
    let watch = |path: std::path::PathBuf, dir: bool| {
        let f = gio::File::for_path(&path);
        let m = if dir {
            f.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        } else {
            f.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        };
        m.ok()
    };
    let mut monitors = Vec::new();
    // config.toml + style.css, the Omarchy theme link (theme switch), glass mode
    let cfg_dir = util::config_dir().join("vector-bar");
    let _ = std::fs::create_dir_all(&cfg_dir);
    monitors.extend(watch(cfg_dir, true));
    monitors.extend(watch(util::config_dir().join("omarchy/current"), true));
    monitors.extend(watch(theme::glass_file(), false));
    for m in &monitors {
        m.connect_changed(move |_, _, _, ev| {
            if matches!(
                ev,
                gio::FileMonitorEvent::ChangesDoneHint
                    | gio::FileMonitorEvent::Created
                    | gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::MovedIn
                    | gio::FileMonitorEvent::Renamed
            ) {
                schedule_reload();
            }
        });
    }
    // live for the whole program
    std::mem::forget(monitors);
}

fn watch_monitors(app: &gtk::Application) {
    let _ = app;
    let Some(display) = gdk::Display::default() else { return };
    let list = display.monitors();
    list.connect_items_changed(|_, _, _, _| schedule_reload());
}
