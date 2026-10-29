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
const USAGE: &str = "usage: vector-space [start|dmenu [-p PROMPT] [-c N] [--inputonly]|apps|clipboard|notifications|emoji|web|calc|convert] | close | quit | to FORMAT FILE... | clip-store | clip-add MIME";

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
        Some("dmenu") => dmenu(&args[2..]),
        Some(w @ ("close" | "quit")) => {
            forward(w);
            ok(true)
        }
        w => {
            let w = w.unwrap_or("apps");
            if Mode::parse(w).is_none() && w != "start" {
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

/// `vector-space dmenu`: lines from stdin, the chosen one on stdout (exit 1 when
/// cancelled). Takes walker's dmenu flags, so `walker --dmenu` callers (Omarchy's
/// menus) can use it unchanged: -p prompt, -c 1-based start row, --inputonly,
/// --placeholder; size flags are ignored.
fn dmenu(args: &[String]) -> std::process::ExitCode {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    let (mut prompt, mut index, mut input_only, mut placeholder) = (String::new(), None, false, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-p" | "--prompt" => prompt = it.next().cloned().unwrap_or_default(),
            "-c" | "--current" => index = it.next().and_then(|v| v.parse::<usize>().ok()),
            "--inputonly" | "-I" => input_only = true,
            "--placeholder" => placeholder = it.next().cloned(),
            "--dmenu" | "-d" | "--exit" | "-n" | "--nohints" | "--nosearch" => {}
            // flags with a value we don't use
            _ if a.starts_with("--") && !a.contains('=') => {
                it.next();
            }
            _ => {}
        }
    }
    let mut items = Vec::new();
    if !input_only {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        items = s.lines().map(str::to_string).filter(|l| !l.is_empty()).collect();
    }
    let dir = std::path::PathBuf::from(std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()))
        .join("vector-space");
    let _ = std::fs::create_dir_all(&dir);
    let id = std::process::id();
    let sock = dir.join(format!("pick-{id}.sock"));
    let req = dir.join(format!("pick-{id}.json"));
    let _ = std::fs::remove_file(&sock);
    let Ok(listener) = UnixListener::bind(&sock) else { return std::process::ExitCode::FAILURE };
    let body = serde_json::json!({
        "prompt": prompt, "items": items, "index": index, "input_only": input_only,
        "placeholder": placeholder, "reply": sock,
    });
    if std::fs::write(&req, body.to_string()).is_err() {
        return std::process::ExitCode::FAILURE;
    }
    let word = format!("pick {}", req.display());
    if !forward(&word) {
        // no daemon yet: start it hidden, then ask again
        let exe = std::env::current_exe()
            .ok()
            .map(|p| p.with_file_name("vector-space-daemon"))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| "vector-space-daemon".into());
        let _ = std::process::Command::new(&exe).arg("--service").spawn();
        let sent = (0..60).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(50));
            forward(&word)
        });
        if !sent {
            let _ = std::fs::remove_file(&sock);
            return std::process::ExitCode::FAILURE;
        }
    }
    let answer = listener.accept().ok().map(|(mut s, _)| {
        let mut a = String::new();
        let _ = s.read_to_string(&mut a);
        a
    });
    let _ = std::fs::remove_file(&sock);
    match answer.as_deref().and_then(|a| a.strip_prefix("1\n")) {
        Some(text) => {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{text}");
            std::process::ExitCode::SUCCESS
        }
        None => std::process::ExitCode::from(1),
    }
}
