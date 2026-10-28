//! Videos (and GIFs): mpv decodes and draws into a GLArea through the render API, so playback
//! is mpv's (hardware decoding, its scalers) inside a GTK window.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::mpv::{Event, Mpv, Render, Value};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct State {
    pub time: f64,
    pub duration: f64,
    pub paused: bool,
    pub volume: f64,
    pub width: f64,
    pub height: f64,
    pub zoom: f64,
}

/// Called with `true` when only the playback time moved.
pub type OnChange = Box<dyn Fn(bool)>;

pub struct Video {
    pub area: gtk::GLArea,
    mpv: Option<Mpv>,
    render: RefCell<Option<Render>>,
    pub state: Cell<State>,
    /// A file asked for before the GL area existed: mpv needs the render context first.
    pending: RefCell<Option<std::path::PathBuf>>,
    /// The file playing, to load again if the GL area is ever realized anew.
    loaded: RefCell<Option<std::path::PathBuf>>,
    pub error: RefCell<Option<String>>,
    /// Called whenever `state` changes; `true` when only the playback time moved.
    pub on_change: RefCell<Option<OnChange>>,
}

impl Video {
    pub fn new(background: &str) -> Rc<Video> {
        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        area.set_vexpand(true);
        area.set_auto_render(false);
        let (events_tx, events_rx) = async_channel::bounded::<()>(1);
        // mpv's own config folder: $XDG_CONFIG_HOME/vector/mpv (mpv.conf, input is ours)
        let config = glib::user_config_dir().join("vector").join("mpv");
        let config = config.to_string_lossy().into_owned();
        let mpv = Mpv::new(
            &[
                ("config-dir", &config),
                ("config", "yes"),
                ("vo", "libmpv"),
                ("hwdec", "auto-safe"),
                ("keep-open", "yes"),
                ("idle", "yes"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("load-scripts", "no"),
                ("osc", "no"),
                ("load-stats-overlay", "no"),
                ("load-console", "no"),
                ("load-auto-profiles", "no"),
                ("load-select", "no"),
                ("load-positioning", "no"),
                ("load-commands", "no"),
                ("load-context-menu", "no"),
                ("osd-level", "0"),
                ("terminal", "no"),
                ("ytdl", "no"),
                ("audio-client-name", "vector-view"),
                ("background-color", background),
                ("loop-file", "inf"),
                ("image-display-duration", "inf"),
            ],
            events_tx,
        );
        let error = RefCell::new(mpv.is_none().then(|| "libmpv could not start".to_string()));
        if let Some(m) = &mpv {
            for p in [
                "time-pos",
                "duration",
                "volume",
                "width",
                "height",
                "video-zoom",
            ] {
                m.observe_f64(p);
            }
            m.observe_flag("pause");
        }
        let video = Rc::new(Video {
            area,
            mpv,
            render: RefCell::default(),
            pending: RefCell::default(),
            loaded: RefCell::default(),
            state: Cell::new(State {
                volume: 100.0,
                ..State::default()
            }),
            error,
            on_change: RefCell::default(),
        });

        let weak = Rc::downgrade(&video);
        video.area.connect_realize(move |area| {
            let Some(v) = weak.upgrade() else { return };
            area.make_current();
            if area.error().is_some() {
                v.fail("no OpenGL for video");
                return;
            }
            let (tx, rx) = async_channel::bounded::<()>(1);
            match v
                .mpv
                .as_ref()
                .and_then(|m| m.render_context(tx, wl_display(area)))
            {
                Some(r) => *v.render.borrow_mut() = Some(r),
                None => {
                    v.fail("mpv can't draw into this window (no OpenGL video output)");
                    return;
                }
            }
            let again = v
                .pending
                .borrow_mut()
                .take()
                .or_else(|| v.loaded.borrow().clone());
            if let Some(p) = again {
                v.open(&p);
            }
            let a = area.downgrade();
            glib::spawn_future_local(async move {
                while rx.recv().await.is_ok() {
                    match a.upgrade() {
                        Some(a) => a.queue_render(),
                        None => break,
                    }
                }
            });
        });
        let weak = Rc::downgrade(&video);
        video.area.connect_unrealize(move |area| {
            if let Some(v) = weak.upgrade() {
                area.make_current();
                v.render.borrow_mut().take();
            }
        });
        let weak = Rc::downgrade(&video);
        video.area.connect_render(move |area, _| {
            if let Some(v) = weak.upgrade()
                && let Some(r) = v.render.borrow().as_ref()
            {
                let sf = area.scale_factor();
                r.draw(area.width() * sf, area.height() * sf);
            }
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(&video);
        glib::spawn_future_local(async move {
            while events_rx.recv().await.is_ok() {
                let Some(v) = weak.upgrade() else { break };
                v.drain();
            }
        });
        video
    }

    fn drain(&self) {
        let Some(m) = &self.mpv else { return };
        let mut s = self.state.get();
        let before = s;
        while let Some(e) = m.next_event() {
            match e {
                Event::Property(name, value) => {
                    let n = match value {
                        Value::Double(n) => n,
                        Value::Flag(b) => {
                            if name == "pause" {
                                s.paused = b;
                            }
                            continue;
                        }
                        _ => 0.0,
                    };
                    match name.as_str() {
                        "time-pos" => s.time = n,
                        "duration" => s.duration = n,
                        "volume" => s.volume = n,
                        "width" => s.width = n,
                        "height" => s.height = n,
                        "video-zoom" => s.zoom = n,
                        _ => {}
                    }
                }
                Event::FileLoaded => *self.error.borrow_mut() = None,
                Event::EndFile(Some(e)) => {
                    *self.error.borrow_mut() = Some(format!("can't play this file: {e}"));
                    self.changed(false);
                }
                Event::EndFile(None) | Event::Shutdown => {}
            }
        }
        if s != before {
            self.state.set(s);
            let only_time = State {
                time: before.time,
                ..s
            } == before;
            self.changed(only_time);
        }
    }

    fn changed(&self, only_time: bool) {
        if let Some(f) = self.on_change.borrow().as_ref() {
            f(only_time);
        }
    }

    fn fail(&self, msg: &str) {
        *self.error.borrow_mut() = Some(msg.to_string());
        self.changed(false);
    }

    pub fn open(&self, path: &Path) {
        if self.mpv.is_none() {
            self.fail("libmpv could not start");
            return;
        }
        if self.render.borrow().is_none() {
            *self.pending.borrow_mut() = Some(path.to_path_buf());
            return;
        }
        *self.loaded.borrow_mut() = Some(path.to_path_buf());
        *self.error.borrow_mut() = None;
        if let Some(m) = &self.mpv {
            m.set_flag("pause", false);
            m.command(&["loadfile", &path.to_string_lossy()]);
        }
    }

    pub fn stop(&self) {
        self.pending.borrow_mut().take();
        self.loaded.borrow_mut().take();
        if let Some(m) = &self.mpv {
            m.command(&["stop"]);
        }
        self.state.set(State {
            volume: self.state.get().volume,
            ..State::default()
        });
    }

    pub fn toggle_pause(&self) {
        if let Some(m) = &self.mpv {
            m.command(&["cycle", "pause"]);
        }
    }

    pub fn set_pause(&self, paused: bool) {
        if let Some(m) = &self.mpv {
            m.set_flag("pause", paused);
        }
    }

    pub fn seek(&self, secs: f64) {
        if let Some(m) = &self.mpv {
            m.command(&["seek", &secs.to_string(), "relative+exact"]);
        }
    }

    pub fn seek_to(&self, secs: f64) {
        if let Some(m) = &self.mpv {
            m.command(&["seek", &secs.max(0.0).to_string(), "absolute+exact"]);
        }
    }

    pub fn frame_step(&self, back: bool) {
        if let Some(m) = &self.mpv {
            m.command(&[if back {
                "frame-back-step"
            } else {
                "frame-step"
            }]);
        }
    }

    pub fn add_volume(&self, d: f64) {
        if let Some(m) = &self.mpv {
            let v = (m.f64("volume").unwrap_or(100.0) + d).clamp(0.0, 130.0);
            m.set_f64("volume", v);
        }
    }

    /// Zoom in mpv's terms: log2 of the scale on top of fitting.
    pub fn add_zoom(&self, d: f64) {
        if let Some(m) = &self.mpv {
            let z = (m.f64("video-zoom").unwrap_or(0.0) + d).clamp(-3.0, 5.0);
            m.set_f64("video-zoom", z);
        }
    }

    /// Moves the picture by a fraction of its size.
    pub fn pan(&self, dx: f64, dy: f64) {
        if let Some(m) = &self.mpv {
            for (p, d) in [("video-pan-x", dx), ("video-pan-y", dy)] {
                let v = (m.f64(p).unwrap_or(0.0) + d).clamp(-2.0, 2.0);
                m.set_f64(p, v);
            }
        }
    }

    pub fn reset_view(&self) {
        if let Some(m) = &self.mpv {
            for p in ["video-zoom", "video-pan-x", "video-pan-y"] {
                m.set_f64(p, 0.0);
            }
        }
    }

    pub fn is_zoomed(&self) -> bool {
        self.state.get().zoom != 0.0
    }

    /// Starts writing the current frame (source resolution, no zoom) to `out` as PNG; mpv does
    /// it on its own thread, so the file appears a little later.
    pub fn screenshot(&self, out: &Path) -> bool {
        self.mpv.as_ref().is_some_and(|m| {
            m.command(&[
                "async",
                "screenshot-to-file",
                &out.to_string_lossy(),
                "video",
            ])
        })
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        // the render context must go before the player
        if self.render.borrow().is_some() {
            self.area.make_current();
            self.render.borrow_mut().take();
        }
    }
}

/// The Wayland connection of the window, for mpv's hardware decoders; null elsewhere.
fn wl_display(w: &impl IsA<gtk::Widget>) -> *mut std::ffi::c_void {
    unsafe extern "C" {
        fn gdk_wayland_display_get_wl_display(
            display: *mut gtk::gdk::ffi::GdkDisplay,
        ) -> *mut std::ffi::c_void;
    }
    let display = w.as_ref().display();
    if display.type_().name() != "GdkWaylandDisplay" {
        return std::ptr::null_mut();
    }
    use glib::translate::ToGlibPtr;
    // SAFETY: a GdkWaylandDisplay, checked above; the connection lives as long as the display.
    unsafe { gdk_wayland_display_get_wl_display(display.to_glib_none().0) }
}
