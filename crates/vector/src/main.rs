//! vector: a GTK4 file manager with nvim-style modes on top of normal mouse browsing.
//!
//! `vector [FOLDER]`
//! Debug flags for tests: `--snapshot OUT.png` renders the window to a PNG and exits, after
//! `--after MS` (default 800) and after feeding `--keys "<vim notation>"` once the folder has
//! loaded. `--size WxH` sets the window size.

mod dbus;
mod details;
mod fmt;
mod iconstore;
mod pane;
mod pinstore;
mod portal;
mod preview;
mod settings;
mod sidebar;
mod style;
mod terminal;
#[cfg(feature = "terminal")]
mod termpanel;
mod thumbs;
mod window;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "io.github.not_bad_company.Vector";

#[derive(Default, Clone)]
struct Args {
    path: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    after: u64,
    keys: String,
    size: Option<(i32, i32)>,
    /// `--service`: no window; serve org.freedesktop.FileManager1 (D-Bus activation).
    service: bool,
    /// `--portal`: no window; serve the xdg-desktop-portal FileChooser backend.
    portal: bool,
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
            "--service" => a.service = true,
            "--portal" => a.portal = true,
            "-h" | "--help" => {
                println!(
                    "usage: vector-fm [FOLDER]\n\n  --service    serve org.freedesktop.FileManager1 (D-Bus activation)\n  --portal     serve the xdg-desktop-portal file chooser\n\ndevelopment:\n  --snapshot OUT.png [--after MS] [--keys KEYS] [--size WxH]"
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
            eprintln!("{}: {e}", vector_core::APP_NAME);
            return glib::ExitCode::FAILURE;
        }
    };
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::NON_UNIQUE);
    let args = Rc::new(args);
    let keep: Rc<RefCell<Vec<Rc<window::Win>>>> = Rc::default();
    app.connect_activate(move |app| {
        gtk::Window::set_default_icon_name(APP_ID);
        let theme = vector_core::theme::Theme::load();
        if let Some(display) = gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(&style::css(&theme));
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        // opens a window; windows are dropped from `keep` when they close
        let open: dbus::Open = {
            let (app, keep, theme, size) = (app.clone(), keep.clone(), theme.clone(), args.size);
            Rc::new(move |dir: PathBuf, select: Option<PathBuf>| {
                let win = window::Win::new(&app, &dir, theme.clone());
                if let Some((w, h)) = size {
                    win.window.set_default_size(w, h);
                }
                if let Some(item) = select {
                    win.pane().select_paths(std::slice::from_ref(&item));
                }
                win.window.present();
                let k = keep.clone();
                let ptr = Rc::downgrade(&win);
                win.window.connect_destroy(move |_| {
                    k.borrow_mut()
                        .retain(|w| !std::ptr::eq(Rc::as_ptr(w), ptr.as_ptr()));
                });
                keep.borrow_mut().push(win);
            })
        };
        if args.snapshot.is_none() {
            // test runs leave the bus name to the real file manager
            // the name stays owned for the life of the process
            let _ = dbus::own(open.clone());
        }
        if args.portal {
            let (app2, keep2, theme2) = (app.clone(), keep.clone(), theme.clone());
            let show: portal::Show =
                Rc::new(move |req: portal::Request, respond: portal::Respond| {
                    let start = req
                        .folder
                        .clone()
                        .filter(|p| p.is_dir())
                        .unwrap_or_else(glib::home_dir);
                    let win = window::Win::new(&app2, &start, theme2.clone());
                    win.window.set_default_size(1100, 700);
                    win.set_picker(req, respond);
                    win.window.present();
                    // tests: render the picker once it has settled
                    if let Some(out) = std::env::var_os("VECTOR_PICK_SNAPSHOT") {
                        let w = win.window.clone();
                        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                            if let Err(e) = snapshot(&w, std::path::Path::new(&out)) {
                                eprintln!("vector: snapshot failed: {e}");
                            }
                        });
                    }
                    let k = keep2.clone();
                    let ptr = Rc::downgrade(&win);
                    win.window.connect_destroy(move |_| {
                        k.borrow_mut()
                            .retain(|w| !std::ptr::eq(Rc::as_ptr(w), ptr.as_ptr()));
                    });
                    keep2.borrow_mut().push(win);
                });
            let _ = portal::own(show);
            std::mem::forget(app.hold());
            return;
        }
        if args.service {
            // started by D-Bus activation: stay up for ShowItems/ShowFolders calls
            std::mem::forget(app.hold());
            return;
        }
        let start = args
            .path
            .clone()
            .map(|p| std::path::absolute(&p).unwrap_or(p))
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(glib::home_dir);
        open(start, None);
        if std::env::var_os("VECTOR_TIMING").is_some() {
            watch_stalls();
        }
        let first = keep.borrow().last().cloned();
        if let Some(win) = first
            && (args.snapshot.is_some() || !args.keys.is_empty())
        {
            schedule_script(win, args.clone(), app.clone());
        }
    });
    // GTK must not see our own options
    let code = app.run_with_args::<&str>(&[]);
    window::forget_cwd();
    pane::save_index();
    code
}

/// Prints the longest gap between 5 ms main-loop ticks during the first 4 s: how long the
/// window could not react to input.
fn watch_stalls() {
    let start = std::time::Instant::now();
    let last = Rc::new(std::cell::Cell::new(start));
    let worst = Rc::new(std::cell::Cell::new(Duration::ZERO));
    glib::timeout_add_local(Duration::from_millis(5), move || {
        let now = std::time::Instant::now();
        worst.set(worst.get().max(now - last.get()));
        last.set(now);
        if now - start > Duration::from_secs(4) {
            eprintln!(
                "vector timing: longest main-loop stall {:.1} ms",
                worst.get().as_secs_f64() * 1000.0
            );
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
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
        // "<wait>" in --keys pauses half a second (for an editor or an operation to finish)
        for (i, part) in args.keys.split("<wait>").enumerate() {
            if i > 0 {
                glib::timeout_future(Duration::from_millis(500)).await;
            }
            for key in vector_core::keymap::parse_keys(part) {
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
            eprintln!("{}: snapshot failed: {e}", vector_core::APP_NAME);
        }
        app.quit();
    });
}

fn snapshot(window: &gtk::ApplicationWindow, out: &std::path::Path) -> Result<(), String> {
    let (w, h) = (window.width(), window.height());
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snap = gtk::Snapshot::new();
    paintable.snapshot(&snap, w as f64, h as f64);
    let node = snap
        .to_node()
        .ok_or_else(|| format!("nothing drawn (mapped {}, {w}x{h})", window.is_mapped()))?;
    let renderer = window
        .native()
        .and_then(|n| n.renderer())
        .ok_or("no renderer")?;
    let texture = renderer.render_texture(
        &node,
        Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
    );
    texture.save_to_png(out).map_err(|e| e.to_string())?;
    // popovers (the context menu) are their own surfaces: write each open one next to it
    let mut child = window.child().and_then(|c| c.first_child());
    let mut n = 0;
    while let Some(c) = child {
        if let Some(pop) = c.downcast_ref::<gtk::Popover>()
            && pop.is_visible()
        {
            n += 1;
            let (pw, ph) = (pop.width(), pop.height());
            let paintable = gtk::WidgetPaintable::new(Some(pop));
            let snap = gtk::Snapshot::new();
            paintable.snapshot(&snap, pw as f64, ph as f64);
            if let (Some(node), Some(r)) = (snap.to_node(), pop.native().and_then(|n| n.renderer()))
            {
                let tex = r.render_texture(
                    &node,
                    Some(&gtk::graphene::Rect::new(0.0, 0.0, pw as f32, ph as f32)),
                );
                let path = out.with_extension(format!("popover{n}.png"));
                tex.save_to_png(&path).map_err(|e| e.to_string())?;
            }
        }
        child = c.next_sibling();
    }
    Ok(())
}
