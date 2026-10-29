//! `record region|output [--audio=desktop,mic]`: screen recording with
//! gpu-screen-recorder, the same options as the Omarchy flow (kms backend, 60 fps
//! constant frame rate, AAC audio). The same command stops a recording: it sends
//! SIGINT, which gpu-screen-recorder needs to write the file properly.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use crate::capture::{self, Monitor};
use crate::overlay::{self, Frozen, Kind, Outcome};
use crate::util;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Audio {
    pub desktop: bool,
    pub mic: bool,
}

impl Audio {
    /// `desktop`, `mic` or `desktop,mic`; anything else is no audio.
    pub fn parse(s: &str) -> Audio {
        Audio { desktop: s.split(',').any(|p| p == "desktop"), mic: s.split(',').any(|p| p == "mic") }
    }
}

struct Recording {
    proc: gio::Subprocess,
}

thread_local! {
    static REC: RefCell<Option<Rc<Recording>>> = const { RefCell::new(None) };
}

pub fn active() -> bool {
    REC.with(|r| r.borrow().is_some())
}

/// Starts a recording of a region (`output` = false) or a whole output (`output` = true).
/// Stops the running one, if there is one, instead.
pub fn toggle(app: &gtk::Application, output: bool, audio: Audio) -> bool {
    if active() {
        return stop();
    }
    let Some(mon) = capture::target_monitor() else { return false };
    let pixbuf = match capture::grab(&mon) {
        Ok(pb) => pb,
        Err(e) => {
            notify(app, "Screen recording failed", &e);
            return false;
        }
    };
    let kind = if output { Kind::Output } else { Kind::Region };
    let app2 = app.clone();
    let accent = util::accent();
    let frozen = Frozen { pixbuf, mon: mon.clone() };
    overlay::run(app, frozen, kind, &accent, move |outcome, frozen| {
        let Outcome::Rect { x, y, w, h } = outcome else { return };
        start(&app2, &frozen.mon, x, y, w, h, audio);
    });
    true
}

fn start(app: &gtk::Application, mon: &Monitor, x: f64, y: f64, w: f64, h: f64, audio: Audio) {
    let whole = x == 0.0 && y == 0.0 && w as i32 == mon.w && h as i32 == mon.h;
    let target = if whole {
        mon.name.clone()
    } else {
        // gpu-screen-recorder takes WxH+X+Y in global logical coordinates, as slurp prints them
        format!("{}x{}+{}+{}", w.round() as i32, h.round() as i32, mon.x + x.round() as i32, mon.y + y.round() as i32)
    };
    let path = capture::timestamped(&videos_dir(), "screenrecording", "mp4");
    // VECTOR_CAPTURE_RECORDER replaces the recorder in tests (the owner's screen is never recorded from a test)
    let recorder = std::env::var("VECTOR_CAPTURE_RECORDER").ok().filter(|r| !r.is_empty()).unwrap_or_else(|| "gpu-screen-recorder".into());
    let mut argv: Vec<String> = vec![recorder, "-w".into(), target];
    argv.extend(["-k", "auto", "-f", "60", "-fm", "cfr", "-fallback-cpu-encoding", "yes", "-o"].map(String::from));
    argv.push(path.to_string_lossy().into_owned());
    let sources: Vec<&str> = [(audio.desktop, "default_output"), (audio.mic, "default_input")].iter().filter(|(on, _)| *on).map(|(_, d)| *d).collect();
    if !sources.is_empty() {
        argv.extend(["-a".to_owned(), sources.join("|"), "-ac".to_owned(), "aac".to_owned()]);
    }
    let proc = match gio::Subprocess::newv(&argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>(), gio::SubprocessFlags::NONE) {
        Ok(p) => p,
        Err(e) => {
            notify(app, "Screen recording failed", &e.to_string());
            return;
        }
    };
    let rec = Rc::new(Recording { proc });
    REC.with(|r| *r.borrow_mut() = Some(rec.clone()));
    notify(app, "Screen recording started", "Press the same key again to stop");
    let app2 = app.clone();
    rec.proc.clone().wait_async(gio::Cancellable::NONE, move |res| {
        REC.with(|r| *r.borrow_mut() = None);
        if res.is_ok() {
            notify(&app2, "Screen recording saved", &path.to_string_lossy());
        } else {
            notify(&app2, "Screen recording error", "The recorder stopped unexpectedly");
        }
    });
}

/// Sends SIGINT to the running recording. True when there was one.
fn stop() -> bool {
    REC.with(|r| match r.borrow().as_ref() {
        Some(rec) => {
            rec.proc.send_signal(libc::SIGINT);
            true
        }
        None => false,
    })
}

/// `$XDG_VIDEOS_DIR`, else ~/Videos; created if missing.
fn videos_dir() -> PathBuf {
    let dir = std::env::var_os("OMARCHY_SCREENRECORD_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(xdg_videos)
        .unwrap_or_else(|| util::home().join("Videos"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn xdg_videos() -> Option<PathBuf> {
    let text = std::fs::read_to_string(util::config_dir().join("user-dirs.dirs")).ok()?;
    let line = text.lines().find(|l| l.starts_with("XDG_VIDEOS_DIR="))?;
    let raw = line.trim_start_matches("XDG_VIDEOS_DIR=").trim().trim_matches('"');
    let path = raw.replace("$HOME", &util::home().to_string_lossy());
    (!path.is_empty()).then(|| PathBuf::from(path))
}

fn notify(app: &gtk::Application, title: &str, body: &str) {
    let n = gio::Notification::new(title);
    n.set_body(Some(body));
    app.send_notification(Some("vector-capture"), &n);
}
