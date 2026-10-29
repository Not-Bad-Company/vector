//! `shot region|window|output [--view]`: freeze the output, let the user pick, then
//! crop the frozen image. The overlay is never in the saved or copied picture,
//! because the crop comes from the grab taken before it appeared.

use std::process::Command;
use std::time::Instant;

use gtk::prelude::*;

use crate::capture::{self, Monitor};
use crate::overlay::{self, Frozen, Kind, Outcome};
use crate::util;

pub fn start(app: &gtk::Application, mode: &str, view: bool) -> bool {
    let t0 = Instant::now();
    let Some(mon) = capture::target_monitor() else {
        notify(app, "Screenshot failed", "No output to capture");
        return false;
    };
    let pixbuf = match capture::grab(&mon) {
        Ok(pb) => pb,
        Err(e) => {
            notify(app, "Screenshot failed", &e);
            return false;
        }
    };
    let kind = match mode {
        "window" => Kind::Window(capture::windows_on(&mon)),
        "output" | "fullscreen" => Kind::Output,
        _ => Kind::Region,
    };
    let frozen = Frozen { pixbuf, mon: mon.clone() };
    let app2 = app.clone();
    let accent = util::accent();
    overlay::run(app, frozen, kind, &accent, move |outcome, frozen| finish(&app2, outcome, frozen, &mon, view));
    if std::env::var_os("VECTOR_CAPTURE_TIMING").is_some() {
        eprintln!("vector-capture: frozen overlay requested after {} ms", t0.elapsed().as_millis());
    }
    true
}

fn finish(app: &gtk::Application, outcome: Outcome, frozen: &Frozen, mon: &Monitor, view: bool) {
    let Outcome::Rect { x, y, w, h } = outcome else { return };
    let Some(cropped) = capture::crop(&frozen.pixbuf, mon, x, y, w, h) else { return };
    capture::copy_to_clipboard(&cropped);
    let path = capture::timestamped(&capture::pictures_dir(), "screenshot", "png");
    if let Err(e) = capture::save_png(&cropped, &path) {
        notify(app, "Screenshot failed", &e);
        return;
    }
    notify(app, "Screenshot saved to clipboard and file", &path.to_string_lossy());
    if view {
        // Vector View opens the file in its own window
        let _ = Command::new("vector-view").arg(&path).spawn();
    }
}

fn notify(app: &gtk::Application, title: &str, body: &str) {
    let n = gtk::gio::Notification::new(title);
    n.set_body(Some(body));
    app.send_notification(Some("vector-capture"), &n);
}
