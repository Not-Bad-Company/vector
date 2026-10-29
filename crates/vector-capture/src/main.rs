//! Vector Capture: screenshots, colour picker, screen recording and text from a
//! screenshot. This is the light client: no GTK, so a keybind reaches the
//! resident overlay in milliseconds. `vector-capture-daemon` (run by
//! vector-capture.service) owns the overlay; the client forwards to it over D-Bus
//! and starts it when it is not running.
//!
//!   vector-capture shot region|window|output [--view]

use std::os::unix::process::CommandExt;

use gio::prelude::*;

const USAGE: &str = "usage: vector-capture shot region|window|output [--view]";

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        Some("--version") => {
            println!("vector-capture {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        _ => {}
    }
    let words = if args.is_empty() { "shot region".to_owned() } else { args.join(" ") };
    if forward(&words) {
        return std::process::ExitCode::SUCCESS;
    }
    // no daemon running: become it, with the same arguments
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("vector-capture-daemon"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| "vector-capture-daemon".into());
    let err = std::process::Command::new(&exe).args(&args).exec();
    eprintln!("vector-capture: {}: {err}", exe.display());
    std::process::ExitCode::FAILURE
}

fn id() -> &'static str {
    if std::env::var("VECTOR_CAPTURE_TEST").is_ok_and(|v| !v.is_empty() && v != "0") {
        "org.vectoros.CaptureTest"
    } else {
        "org.vectoros.Capture"
    }
}

/// Hands `words` to the running daemon (its `command` action). False when none runs.
fn forward(words: &str) -> bool {
    let id = id();
    let Ok(c) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else { return false };
    let path = format!("/{}", id.replace('.', "/"));
    let args = ("command", vec![words.to_variant()], std::collections::HashMap::<String, glib::Variant>::new());
    c.call_sync(
        Some(id),
        &path,
        "org.gtk.Actions",
        "Activate",
        Some(&args.to_variant()),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        2000,
        gio::Cancellable::NONE,
    )
    .is_ok()
}
