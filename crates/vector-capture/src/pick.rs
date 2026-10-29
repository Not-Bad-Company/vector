//! `pick [hex|rgb|hsl]`: the colour under the pointer on the frozen screen. Copies the
//! chosen format (hex by default); h, r, l in the overlay switch the format.

use gtk::prelude::*;

use crate::capture;
use crate::color;
use crate::overlay::{self, Frozen, Kind, Outcome};
use crate::util;

pub fn start(app: &gtk::Application, format: Option<color::Format>) -> bool {
    let Some(mon) = capture::target_monitor() else { return false };
    let pixbuf = match capture::grab(&mon) {
        Ok(pb) => pb,
        Err(e) => {
            notify(app, "Colour picker failed", &e);
            return false;
        }
    };
    let app2 = app.clone();
    let accent = util::accent();
    let frozen = Frozen { pixbuf, mon: mon.clone() };
    overlay::run(app, frozen, Kind::Picker, &accent, move |outcome, frozen| {
        let Outcome::Point { x, y, format: chosen } = outcome else { return };
        let Some((r, g, b)) = frozen.pixel(x, y) else { return };
        let text = color::format(format.unwrap_or(chosen), r, g, b);
        if let Some(display) = gtk::gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
        notify(&app2, &format!("Copied {text}"), &color::hex(r, g, b));
    });
    true
}

fn notify(app: &gtk::Application, title: &str, body: &str) {
    let n = gtk::gio::Notification::new(title);
    n.set_body(Some(body));
    app.send_notification(Some("vector-capture"), &n);
}
