//! strata: a GTK4 file manager with nvim-style modes on top of normal mouse browsing.
//!
//! `strata [FOLDER]`
//! Debug flags for tests: `--snapshot OUT.png` renders the window to a PNG and exits, after
//! `--after MS` (default 800) and after feeding `--keys "<vim notation>"` once the folder has
//! loaded. `--size WxH` sets the window size.

mod fmt;
mod pane;
mod preview;
mod sidebar;
mod style;
mod thumbs;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "dev.strata.Strata";

#[derive(Default, Clone)]
struct Args {
    path: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    after: u64,
    keys: String,
    size: Option<(i32, i32)>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        after: 800,
        ..Args::default()
    };
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        let s = arg.to_string_lossy().into_owned();
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match s.as_str() {
            "--snapshot" => a.snapshot = Some(PathBuf::from(value("--snapshot")?)),
            "--after" => {
                a.after = value("--after")?
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "--after takes milliseconds")?
            }
            "--keys" => a.keys = value("--keys")?.to_string_lossy().into_owned(),
            "--size" => {
                let v = value("--size")?.to_string_lossy().into_owned();
                let (w, h) = v.split_once('x').ok_or("--size takes WxH")?;
                a.size = Some((
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                ));
            }
            "-h" | "--help" => {
                println!(
                    "usage: {} [FOLDER] [--snapshot OUT.png [--after MS] [--keys KEYS] [--size WxH]]",
                    strata_core::APP_NAME
                );
                std::process::exit(0);
            }
            // a folder given as a file:// URI (desktop files pass %U)
            _ if s.starts_with("file://") => a.path = gio::File::for_uri(&s).path(),
            _ if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => a.path = Some(PathBuf::from(arg)),
        }
    }
    Ok(a)
}

fn main() -> glib::ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}: {e}", strata_core::APP_NAME);
            return glib::ExitCode::FAILURE;
        }
    };
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::NON_UNIQUE);
    let args = Rc::new(args);
    let keep: Rc<RefCell<Vec<Rc<window::Win>>>> = Rc::default();
    app.connect_activate(move |app| {
        let theme = strata_core::theme::Theme::load();
        if let Some(display) = gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(&style::css(&theme));
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        let start = args
            .path
            .clone()
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(glib::home_dir);
        let win = window::Win::new(app, &start, theme);
        if let Some((w, h)) = args.size {
            win.window.set_default_size(w, h);
        }
        win.window.present();
        if args.snapshot.is_some() || !args.keys.is_empty() {
            schedule_script(win.clone(), args.clone(), app.clone());
        }
        keep.borrow_mut().push(win);
    });
    // GTK must not see our own options
    app.run_with_args::<&str>(&[])
}

/// Waits for the first folder to load, feeds `--keys`, waits `--after`, writes the snapshot.
fn schedule_script(win: Rc<window::Win>, args: Rc<Args>, app: gtk::Application) {
    glib::spawn_future_local(async move {
        for _ in 0..200 {
            if !win.pane().loading.get() {
                break;
            }
            glib::timeout_future(Duration::from_millis(25)).await;
        }
        glib::timeout_future(Duration::from_millis(150)).await;
        for key in strata_core::keymap::parse_keys(&args.keys) {
            win.handle_key(key, true);
            // let listings, completions and dialogs settle between keys
            glib::timeout_future(Duration::from_millis(60)).await;
            for _ in 0..80 {
                if !win.pane().loading.get() {
                    break;
                }
                glib::timeout_future(Duration::from_millis(25)).await;
            }
        }
        let Some(out) = args.snapshot.clone() else {
            return;
        };
        glib::timeout_future(Duration::from_millis(args.after)).await;
        // a window on a hidden workspace may not be laid out yet; give it a moment
        let mut result = Err(String::new());
        for _ in 0..40 {
            result = snapshot(&win.window, &out);
            if result.is_ok() {
                break;
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
        if let Err(e) = result {
            eprintln!("{}: snapshot failed: {e}", strata_core::APP_NAME);
        }
        app.quit();
    });
}

fn snapshot(window: &gtk::ApplicationWindow, out: &std::path::Path) -> Result<(), String> {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, w as f64, h as f64);
    let node = snap.to_node().ok_or("nothing drawn")?;
    let renderer = window
        .native()
        .and_then(|n| n.renderer())
        .ok_or("no renderer")?;
    let texture = renderer.render_texture(
        &node,
        Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
    );
    texture.save_to_png(out).map_err(|e| e.to_string())
}
