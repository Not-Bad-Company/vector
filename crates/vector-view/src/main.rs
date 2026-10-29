//! Vector View: images, videos, PDFs and office documents in the look of the vector file
//! manager. `vector-view [FILE|FOLDER]…`
//!
//! Debug flags for tests (as in vector): `--snapshot OUT.png` renders the window to a PNG and
//! exits, after `--after MS` (default 800) and after feeding `--keys "<vim notation>"` once the
//! first file has loaded. `--size WxH` sets the window size.

mod canvas;
// the `?` panel is vector-find's, shared
#[path = "../../vector/src/help.rs"]
mod help;
mod info;
mod load;
mod mpv;
mod pdf;
mod playlist;
#[path = "../../vector/src/slant.rs"]
mod slant;
mod video;
mod window;

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "io.github.not_bad_company.VectorView";

#[derive(Default)]
struct Args {
    paths: Vec<PathBuf>,
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
            "-V" | "--version" => {
                println!("vector-view {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!(
                    "usage: vector-view [FILE|FOLDER]…\n\nOne file opens its whole folder; a folder opens everything in it.\nPress ? in the window for the keys.\n\n  --version    print the version\n\ndevelopment:\n  --snapshot OUT.png [--after MS] [--keys KEYS] [--size WxH]"
                );
                std::process::exit(0);
            }
            _ if s.starts_with("file://") => {
                if let Some(p) = gio::File::for_uri(&s).path() {
                    a.paths.push(p);
                }
            }
            _ if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ => a.paths.push(PathBuf::from(arg)),
        }
    }
    Ok(a)
}

/// The viewer's additions to vector's stylesheet.
fn css(t: &vector_core::theme::Theme) -> String {
    let (bg, fg, acc, red) = (&t.background, &t.foreground, &t.accent, &t.color1);
    format!(
        r#"
.header {{ min-height: 34px; }}
.header .kind {{ padding: 0 10px 0 6px; font-size: 13px; font-weight: normal; }}
.header .name {{ font-weight: bold; }}
.statusline {{ min-height: 26px; }}
.statusline button.link.glyph {{ font-size: 14px; padding: 2px 8px; }}
.statusline .slant.from-mode {{ margin-right: 6px; }}
.statusline .slant.to-mode {{ margin-left: 6px; }}
gridview > child:selected > .tile {{ background: alpha({acc}, 0.16); }}
.statusline .time {{ padding: 0 10px; }}
button.link.danger-hover:hover {{ color: {red}; }}
.timeline {{ background: alpha({fg}, 0.035); }}
.pdf .page {{ background: white; box-shadow: 0 2px 14px alpha(black, 0.35); }}
.info-panel {{ border-left: 1px solid alpha({fg}, 0.08); padding: 14px 16px; min-width: 328px; }}
.info-panel .details-section {{ margin-bottom: 10px; }}
.edit-bar {{ background: alpha({fg}, 0.045); border-top: 1px solid alpha({fg}, 0.08); padding: 2px 10px; min-height: 28px; }}
.edit-bar button.link {{ padding: 3px 10px; opacity: 0.82; }}
.edit-bar button.link:hover {{ opacity: 1; background: alpha({fg}, 0.08); }}
.edit-bar button.link.primary {{ color: {acc}; font-weight: bold; opacity: 1; background: alpha({acc}, 0.12); }}
.save-backdrop {{ background: alpha(black, 0.55); }}
.save-dialog {{ min-width: 380px; padding: 16px 18px 14px 18px; border-left: 2px solid {acc}; }}
.save-dialog button.link.save-choice {{ padding: 7px 14px; border: 1px solid alpha({fg}, 0.16); opacity: 0.9; }}
.save-dialog button.link.save-choice:hover {{ opacity: 1; border-color: {acc}; background: alpha({acc}, 0.10); }}
.save-dialog button.link.save-choice.primary {{ border-color: {acc}; background: alpha({acc}, 0.14); color: {acc}; font-weight: bold; opacity: 1; }}
.tile {{ padding: 2px; }}
.tile .big-icon {{ font-size: 40px; }}
.empty {{ font-size: 13px; }}
.notice {{ background: {bg}; }}
/* fullscreen: the bars float over the picture */
.floating .header, .floating .statusline, .floating .timeline, .floating .edit-bar {{ background-color: alpha({bg}, 0.94); }}
"#
    )
}

fn main() -> glib::ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("vector-view: {e}");
            return glib::ExitCode::FAILURE;
        }
    };
    c_numbers();
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::NON_UNIQUE);
    let args = Rc::new(args);
    app.connect_activate(move |app| {
        gtk::Window::set_default_icon_name(APP_ID);
        let theme = vector_core::theme::Theme::load();
        if let Some(display) = gdk::Display::default() {
            let provider = gtk::CssProvider::new();
            provider.load_from_string(&format!(
                "{}{}",
                vector_core::style::css(&theme),
                css(&theme)
            ));
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_USER,
            );
        }
        // GTK sets the locale while starting
        c_numbers();
        let win = window::Win::new(app, theme);
        if let Some((w, h)) = args.size {
            win.window.set_default_size(w, h);
        }
        win.window.present();
        win.open_paths(args.paths.clone());
        if args.snapshot.is_some() || !args.keys.is_empty() {
            schedule_script(win.clone(), args.clone(), app.clone());
        }
        // the window holds itself until it closes
        let keep = std::cell::RefCell::new(Some(win.clone()));
        win.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
    });
    app.run_with_args::<&str>(&[])
}

/// libmpv refuses to start unless numbers use the C locale (`LC_NUMERIC`).
fn c_numbers() {
    unsafe extern "C" {
        fn setlocale(category: i32, locale: *const std::ffi::c_char) -> *mut std::ffi::c_char;
    }
    const LC_NUMERIC: i32 = 1;
    // SAFETY: called on the main thread before mpv starts; the string is static.
    unsafe { setlocale(LC_NUMERIC, c"C".as_ptr()) };
}

fn schedule_script(win: Rc<window::Win>, args: Rc<Args>, app: gtk::Application) {
    glib::spawn_future_local(async move {
        for _ in 0..200 {
            if !win.loading.get() {
                break;
            }
            glib::timeout_future(Duration::from_millis(25)).await;
        }
        glib::timeout_future(Duration::from_millis(300)).await;
        for (i, part) in args.keys.split("<wait>").enumerate() {
            if i > 0 {
                glib::timeout_future(Duration::from_millis(500)).await;
            }
            for key in vector_core::keymap::parse_keys(part) {
                win.handle_key(key);
                glib::timeout_future(Duration::from_millis(80)).await;
                for _ in 0..80 {
                    if !win.loading.get() {
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
        let mut result = Err(String::new());
        for _ in 0..40 {
            result = snapshot(&win.window, &out);
            if result.is_ok() {
                break;
            }
            glib::timeout_future(Duration::from_millis(100)).await;
        }
        if let Err(e) = result {
            eprintln!("vector-view: snapshot failed: {e}");
        }
        win.window.close();
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
    // popovers (the menu) are their own surfaces: write each open one next to it
    let mut n = 0;
    let mut stack = vec![window.clone().upcast::<gtk::Widget>()];
    while let Some(wd) = stack.pop() {
        let mut c = wd.first_child();
        while let Some(ch) = c {
            if let Some(pop) = ch.downcast_ref::<gtk::Popover>()
                && pop.is_visible()
            {
                n += 1;
                let (pw, ph) = (pop.width(), pop.height());
                let paintable = gtk::WidgetPaintable::new(Some(pop));
                let snap = gtk::Snapshot::new();
                paintable.snapshot(&snap, pw as f64, ph as f64);
                if let (Some(node), Some(r)) =
                    (snap.to_node(), pop.native().and_then(|n| n.renderer()))
                {
                    let tex = r.render_texture(
                        &node,
                        Some(&gtk::graphene::Rect::new(0.0, 0.0, pw as f32, ph as f32)),
                    );
                    tex.save_to_png(out.with_extension(format!("popover{n}.png")))
                        .map_err(|e| e.to_string())?;
                }
            } else {
                stack.push(ch.clone());
            }
            c = ch.next_sibling();
        }
    }
    Ok(())
}
