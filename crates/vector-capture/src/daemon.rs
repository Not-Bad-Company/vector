//! Vector Capture's resident process (GTK). `vector-capture` (main.rs) is what
//! people and keybinds run; it hands the command to this over D-Bus and starts
//! it when it is not running. Commands are words:
//!
//!   shot region|window|output [--view]
//!   pick [hex|rgb|hsl]
//!   --service        run resident (vector-capture.service)
//!
//! `VECTOR_CAPTURE_TEST=1` runs a separate test instance on its own D-Bus name.

mod capture;
mod color;
mod overlay;
mod pick;
mod shot;
mod util;

use gtk::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "org.vectoros.Capture";
const TEST_ID: &str = "org.vectoros.CaptureTest";

fn test() -> bool {
    std::env::var("VECTOR_CAPTURE_TEST").is_ok_and(|v| !v.is_empty() && v != "0")
}

fn main() -> glib::ExitCode {
    // no accessibility bus is needed for an overlay; without this GTK logs AT-SPI errors
    // on every start
    // SAFETY: called before any thread is started
    unsafe { std::env::set_var("GTK_A11Y", "none") };
    let app = gtk::Application::new(Some(if test() { TEST_ID } else { APP_ID }), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    let action = gio::SimpleAction::new("command", Some(glib::VariantTy::STRING));
    {
        let app2 = app.clone();
        action.connect_activate(move |_, p| {
            if let Some(words) = p.and_then(|v| v.str()) {
                command(&app2, words);
            }
        });
    }
    app.add_action(&action);
    app.connect_startup(|app| {
        // resident: stays up with no window open
        std::mem::forget(app.hold());
    });
    app.connect_command_line(|app, cl| {
        let words: Vec<String> = cl.arguments().iter().skip(1).map(|a| a.to_string_lossy().to_string()).collect();
        if words.first().map(String::as_str) == Some("--service") || words.is_empty() {
            return glib::ExitCode::SUCCESS;
        }
        if command(app, &words.join(" ")) { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE }
    });
    app.run()
}

/// Runs one command word-string. False when it is not one we know or it failed to start.
fn command(app: &gtk::Application, words: &str) -> bool {
    let mut it = words.split_whitespace();
    match it.next() {
        Some("shot") => {
            let mode = it.next().unwrap_or("region");
            let rest: Vec<&str> = it.collect();
            let view = rest.contains(&"--view");
            if !matches!(mode, "region" | "window" | "output" | "fullscreen") {
                eprintln!("vector-capture: unknown shot mode {mode}");
                return false;
            }
            shot::start(app, mode, view)
        }
        Some("pick") => {
            let format = match it.next() {
                Some("rgb") => Some(color::Format::Rgb),
                Some("hsl") => Some(color::Format::Hsl),
                Some("hex") => Some(color::Format::Hex),
                _ => None,
            };
            pick::start(app, format)
        }
        Some("quit") => {
            app.quit();
            true
        }
        _ => {
            eprintln!("usage: vector-capture shot region|window|output [--view]");
            false
        }
    }
}
