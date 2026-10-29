//! `ocr`: select a region of the frozen screen and put its text on the clipboard.

use gtk::prelude::*;

use crate::capture;
use crate::ocr;
use crate::overlay::{self, Frozen, Kind, Outcome};
use crate::util;

pub fn start(app: &gtk::Application) -> bool {
    let Some(mon) = capture::target_monitor() else { return false };
    let pixbuf = match capture::grab(&mon) {
        Ok(pb) => pb,
        Err(e) => {
            notify(app, "Text extraction failed", &e);
            return false;
        }
    };
    let app2 = app.clone();
    let accent = util::accent();
    let frozen = Frozen { pixbuf, mon: mon.clone() };
    overlay::run(app, frozen, Kind::Region, &accent, move |outcome, frozen| {
        let Outcome::Rect { x, y, w, h } = outcome else { return };
        let Some(cropped) = capture::crop(&frozen.pixbuf, &frozen.mon, x, y, w, h) else { return };
        let t0 = std::time::Instant::now();
        let result = ocr::read(&cropped);
        if std::env::var_os("VECTOR_CAPTURE_TIMING").is_some() {
            eprintln!("vector-capture: text read in {} ms", t0.elapsed().as_millis());
        }
        match result {
            Ok(text) if !text.trim().is_empty() => {
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&text);
                }
                let lines = text.lines().count();
                notify(&app2, "Copied text from selection", &format!("{lines} line(s)"));
            }
            Ok(_) => notify(&app2, "No text found in the selection", ""),
            Err(e) => notify(&app2, "Text extraction failed", &e),
        }
    });
    true
}

fn notify(app: &gtk::Application, title: &str, body: &str) {
    let n = gtk::gio::Notification::new(title);
    n.set_body(Some(body));
    app.send_notification(Some("vector-capture"), &n);
}
