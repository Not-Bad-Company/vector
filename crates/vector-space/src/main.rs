//! Vector Space: the Vector OS launcher. Apps, clipboard history,
//! notifications, emoji, web search, a calculator and a converter in one window.
//!
//!   vector-space [mode]          open (or close) in a mode: apps, clipboard,
//!                                notifications, emoji, web, calc, convert
//!   vector-space close | quit
//!   vector-space to FMT FILE...  convert files (as the `>` mode does)
//!   vector-space clip-store      record the selection (run by wl-paste --watch)
//!   vector-space clip-add MIME   add stdin to the clipboard history
//!
//! This is the light client: no GTK, so a keybind opens the window in a few
//! milliseconds. The window lives in `vector-space-daemon` (vector-space.service),
//! which it talks to over D-Bus and starts when it is not running.
//! `VECTOR_SPACE_TEST=1` talks to (and starts) a separate test instance.

// the modules are shared with the daemon; this binary uses a part of them
#![allow(dead_code)]

mod clip;
mod color;
mod convert;
mod item;
mod style;
mod util;

use std::os::unix::process::CommandExt;

use gio::prelude::*;

use crate::item::Mode;

const APP_ID: &str = "org.vectoros.Space";
const TEST_ID: &str = "org.vectoros.SpaceTest";
const USAGE: &str = "usage: vector-space [apps|clipboard|notifications|emoji|web|calc|convert] | close | quit | to FORMAT FILE... | clip-store | clip-add MIME";

fn test() -> bool {
    std::env::var("VECTOR_SPACE_TEST").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let ok = |b: bool| if b { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE };
    let word = args.get(1).map(String::as_str);
    match word {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            ok(true)
        }
        Some("--version") => {
            println!("vector-space {}", env!("CARGO_PKG_VERSION"));
            ok(true)
        }
        Some("clip-store") => {
            clip::record();
            ok(true)
        }
        Some("clip-add") => match args.get(2) {
            Some(mime) => ok(clip::add_stdin(mime)),
            None => {
                eprintln!("{USAGE}");
                ok(false)
            }
        },
        Some("to") if args.len() > 3 => ok(convert::cli(&args[2], &args[3..])),
        Some("--service") => daemon(&args),
        Some(w @ ("close" | "quit")) => {
            forward(w);
            ok(true)
        }
        w => {
            let w = w.unwrap_or("apps");
            if Mode::parse(w).is_none() {
                eprintln!("{USAGE}");
                return ok(false);
            }
            if forward(w) {
                return ok(true);
            }
            daemon(&args)
        }
    }
}

/// Becomes the daemon (same arguments): it opens the window, or with
/// `--service` waits hidden.
fn daemon(args: &[String]) -> std::process::ExitCode {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("vector-space-daemon"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "vector-space-daemon".into());
    let err = std::process::Command::new(&exe).args(&args[1..]).exec();
    eprintln!("vector-space: {}: {err}", exe.display());
    std::process::ExitCode::FAILURE
}

/// Hands `word` to the running daemon (its `command` action). False when none runs.
fn forward(word: &str) -> bool {
    let id = if test() { TEST_ID } else { APP_ID };
    let Ok(c) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    let path = format!("/{}", id.replace('.', "/"));
    let args = ("command", vec![word.to_variant()], std::collections::HashMap::<String, glib::Variant>::new());
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
