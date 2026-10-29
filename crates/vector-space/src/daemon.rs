//! Vector Space's resident window (GTK). `vector-space` (main.rs) is what
//! people and keybinds run; it hands commands to this over D-Bus and starts it
//! when it is not running.
//!
//!   vector-space [mode]          open (or close) in a mode: apps, clipboard,
//!                                notifications, emoji, web, calc
//!   vector-space --service       run hidden, ready to open (vector-space.service)
//!   vector-space close | quit
//!   vector-space clip-store      record the selection (run by wl-paste --watch)
//!   vector-space clip-add MIME   add stdin to the clipboard history
//!   vector-space to FMT FILE...  convert files (as the `>` mode does)
//!
//! One process stays resident: it owns the window (so it opens instantly) and
//! runs the clipboard recorder. `VECTOR_SPACE_TEST=1` runs a separate test
//! instance that records nothing.

mod apps;
mod calc;
mod clip;
mod color;
mod convert;
mod emoji;
mod item;
mod notes;
mod style;
mod ui;
mod util;
mod web;

use std::cell::RefCell;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::item::Mode;

const APP_ID: &str = "org.vectoros.Space";
const TEST_ID: &str = "org.vectoros.SpaceTest";
const USAGE: &str = "usage: vector-space [apps|clipboard|notifications|emoji|web|calc] | --service | close | quit | clip-store | clip-add MIME | to FORMAT FILE...";

thread_local! {
    static CSS: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static RECORDER: RefCell<Option<gio::Subprocess>> = const { RefCell::new(None) };
}

fn test() -> bool {
    std::env::var("VECTOR_SPACE_TEST").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return glib::ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("vector-space {}", env!("CARGO_PKG_VERSION"));
            return glib::ExitCode::SUCCESS;
        }
        // no GTK for these: they run once per copy
        Some("clip-store") => {
            clip::record();
            return glib::ExitCode::SUCCESS;
        }
        Some("to") => {
            let (Some(fmt), true) = (args.get(2), args.len() > 3) else {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            };
            return if convert::cli(fmt, &args[3..]) { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE };
        }
        Some("clip-add") => {
            let Some(mime) = args.get(2) else {
                eprintln!("{USAGE}");
                return glib::ExitCode::FAILURE;
            };
            return if clip::add_stdin(mime) { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE };
        }
        Some(w) if w != "--service" && w != "close" && w != "quit" && Mode::parse(w).is_none() => {
            eprintln!("{USAGE}");
            return glib::ExitCode::FAILURE;
        }
        _ => {}
    }

    // the fast path: the resident instance gets the word over D-Bus, without
    // this process loading GTK at all (that alone took ~150 ms)
    if let Some(word) = match args.get(1).map(String::as_str) {
        None => Some("apps"),
        Some("--service") => None,
        Some(w) => Some(w),
    } && forward(word)
    {
        return glib::ExitCode::SUCCESS;
    }

    let app = gtk::Application::new(Some(if test() { TEST_ID } else { APP_ID }), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    let toggle = gio::SimpleAction::new("command", Some(glib::VariantTy::STRING));
    toggle.connect_activate(|_, p| {
        if let Some(w) = p.and_then(|v| v.str()) {
            command(w);
        }
    });
    app.add_action(&toggle);
    app.connect_startup(|app| {
        std::mem::forget(app.hold());
        load_css();
        watch_theme();
        ui::build(app);
        convert::refresh_rates();
        if !test() {
            clip::import_clipse();
            start_recorder();
        }
    });
    app.connect_shutdown(|_| {
        RECORDER.with(|r| {
            if let Some(p) = r.borrow_mut().take() {
                p.force_exit();
            }
        });
    });
    app.connect_command_line(|app, cl| {
        let args: Vec<String> = cl.arguments().iter().map(|a| a.to_string_lossy().to_string()).collect();
        let _ = app;
        match args.get(1).map(String::as_str) {
            Some("--service") => glib::ExitCode::SUCCESS,
            Some(w) => command(w),
            None => command("apps"),
        }
    });
    // quit/close must never start an instance just to stop it
    if matches!(args.get(1).map(String::as_str), Some("quit" | "close")) && !running() {
        return glib::ExitCode::SUCCESS;
    }
    app.run()
}

fn command(word: &str) -> glib::ExitCode {
    let Some(ui) = ui::get() else { return glib::ExitCode::FAILURE };
    match word {
        "quit" => {
            if let Some(app) = gio::Application::default() {
                app.quit();
            }
        }
        "close" => ui.hide(),
        w => match Mode::parse(w) {
            Some(m) => ui.toggle(m),
            None => return glib::ExitCode::FAILURE,
        },
    }
    glib::ExitCode::SUCCESS
}

/// Hands `word` to the running instance (org.gtk.Actions). False when none runs.
fn forward(word: &str) -> bool {
    let id = if test() { TEST_ID } else { APP_ID };
    let Ok(c) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    let path = format!("/{}", id.replace('.', "/"));
    let args = (
        "command",
        vec![word.to_variant()],
        std::collections::HashMap::<String, glib::Variant>::new(),
    );
    c.call_sync(
        Some(id),
        &path,
        "org.gtk.Actions",
        "Activate",
        Some(&args.to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        1000,
        gio::Cancellable::NONE,
    )
    .is_ok()
}

fn running() -> bool {
    let Ok(c) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    c.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&(if test() { TEST_ID } else { APP_ID },).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|v| v.child_value(0).get::<bool>())
    .unwrap_or(false)
}

fn load_css() {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let theme = vector_core::theme::Theme::load();
    let p = gtk::CssProvider::new();
    p.load_from_string(&style::css(&theme));
    CSS.with(|c| {
        if let Some(old) = c.borrow_mut().replace(p.clone()) {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
    });
    gtk::style_context_add_provider_for_display(&display, &p, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
}

/// Omarchy switches themes by re-pointing `~/.config/omarchy/current/theme`.
fn watch_theme() {
    let dir = util::config_dir().join("omarchy/current");
    let Ok(m) = gio::File::for_path(&dir).monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE) else {
        return;
    };
    m.connect_changed(|_, _, _, _| {
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), load_css);
    });
    std::mem::forget(m);
}

/// `wl-paste --watch vector-space clip-store`, restarted if it ever stops.
fn start_recorder() {
    // the light client records: no GTK to load on every copy
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("vector-space"))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "vector-space".into());
    let argv = [std::ffi::OsStr::new("wl-paste"), "--watch".as_ref(), exe.as_ref(), "clip-store".as_ref()];
    let p = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDIN_PIPE) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("vector-space: clipboard recorder: {e}");
            return;
        }
    };
    p.wait_async(gio::Cancellable::NONE, |_| {
        let ours = RECORDER.with(|r| r.borrow().is_some());
        if ours {
            glib::timeout_add_local_once(std::time::Duration::from_secs(2), start_recorder);
        }
    });
    RECORDER.with(|r| *r.borrow_mut() = Some(p));
}
