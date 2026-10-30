//! vectorOS Settings: one window for the system, a page per area.
//!
//!   vector-settings [PAGE]      open (or raise) the window on PAGE
//!   vector-settings --list      the page names
//!
//! Single instance: a second call switches the open window to PAGE.

mod about;
mod bluetooth;
mod conf;
mod ai;
mod dbus;
mod input;
mod page;
mod pi;
mod sound;
mod ui;
mod wifi;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio};

use crate::page::{Ctx, PageDef};

const APP_ID: &str = "org.vectoros.Settings";

/// Every page, in sidebar order. `group` starts a new block in the sidebar.
fn pages() -> Vec<PageDef> {
    vec![
        PageDef { id: "wifi", icon: "\u{f05a9}", title: "wi-fi", group: "connections", build: wifi::page },
        PageDef { id: "bluetooth", icon: "\u{f00af}", title: "bluetooth", group: "connections", build: bluetooth::page },
        PageDef { id: "sound", icon: "\u{f057e}", title: "sound", group: "devices", build: sound::page },
        PageDef { id: "input", icon: "\u{f030c}", title: "keyboard & touchpad", group: "devices", build: input::page },
        PageDef { id: "pi", icon: "\u{f048b}", title: "pi", group: "vector", build: pi::page },
        PageDef { id: "ai", icon: "\u{f06a9}", title: "ai", group: "vector", build: ai::page },
        PageDef { id: "about", icon: "\u{f02fd}", title: "about", group: "system", build: about::page },
    ]
}

struct Win {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    rows: HashMap<&'static str, gtk::Box>,
    defs: Vec<PageDef>,
    built: RefCell<HashMap<&'static str, page::View>>,
    current: RefCell<&'static str>,
    ctx: Ctx,
}

thread_local! {
    static WIN: RefCell<Option<Rc<Win>>> = const { RefCell::new(None) };
}

fn css(t: &vector_core::theme::Theme) -> String {
    let (bg, fg, acc, red) = (&t.background, &t.foreground, &t.accent, &t.color1);
    format!(
        r#"
window.vector.settings {{ background: alpha({bg}, 0.97); }}
.side {{ border-right: 1px solid alpha({fg}, 0.08); padding: 10px 0; min-width: 210px; }}
.side .brand {{ padding: 6px 18px 14px 18px; }}
.side .brand .mark {{ color: {acc}; font-size: 15px; }}
.side .brand .word {{ font-weight: bold; }}
.side .grp {{ padding: 12px 18px 4px 18px; opacity: 0.42; font-size: 11px; }}
.side .row {{ padding: 7px 18px; }}
.side .row .icon {{ min-width: 20px; }}
.page-head {{ padding: 22px 28px 10px 28px; }}
.page-head .ptitle {{ color: {acc}; font-weight: bold; font-size: 18px; }}
.page-head .pdetail {{ opacity: 0.6; }}
.page-body {{ padding: 0 28px 28px 28px; }}
.page-foot {{ border-top: 1px solid alpha({fg}, 0.08); padding: 6px 20px; }}
.sec {{ margin-top: 18px; margin-bottom: 6px; }}
.sec-title {{ opacity: 0.5; }}
.group {{ border: 1px solid alpha({fg}, 0.09); background: alpha({fg}, 0.02); }}
.srow {{ border-left: 2px solid transparent; }}
.srow + .srow {{ border-top: 1px solid alpha({fg}, 0.05); }}
.srow .line {{ padding: 9px 14px; min-height: 22px; }}
.srow.clickable .line:hover {{ background: alpha({fg}, 0.05); }}
.srow.active {{ border-left-color: {acc}; background: alpha({acc}, 0.06); }}
.srow .icon {{ min-width: 20px; }}
.srow .meta {{ opacity: 0.55; }}
.srow .expand {{ padding: 2px 14px 12px 44px; }}
.kv {{ padding: 5px 14px; }}
.kv .k {{ opacity: 0.5; }}
.note {{ padding: 10px 14px; opacity: 0.6; }}
.msg {{ padding: 6px 14px; }}
.crit {{ color: {red}; }}
entry, passwordentry, dropdown > button {{
  background: alpha({fg}, 0.05); border: 1px solid alpha({fg}, 0.12); border-radius: 0;
  box-shadow: none; min-height: 28px; padding: 0 8px; color: {fg};
}}
entry:focus-within, passwordentry:focus-within {{ border-color: alpha({acc}, 0.7); }}
switch {{ border-radius: 0; background: alpha({fg}, 0.12); border: none; min-width: 38px; min-height: 18px; }}
switch:checked {{ background: {acc}; }}
switch slider {{ border-radius: 0; background: {bg}; min-width: 16px; min-height: 16px; margin: 1px; box-shadow: none; border: none; }}
scale trough {{ border-radius: 0; background: alpha({fg}, 0.12); min-height: 4px; }}
scale highlight {{ border-radius: 0; background: {acc}; }}
scale slider {{ border-radius: 0; background: {fg}; min-width: 10px; min-height: 14px; margin: -6px 0; box-shadow: none; border: none; }}
"#
    )
}

fn build(app: &gtk::Application) -> Rc<Win> {
    let theme = vector_core::theme::Theme::load();
    if let Some(d) = gdk::Display::default() {
        let p = gtk::CssProvider::new();
        p.load_from_string(&format!("{}{}", vector_core::style::css(&theme), css(&theme)));
        gtk::style_context_add_provider_for_display(&d, &p, gtk::STYLE_PROVIDER_PRIORITY_USER);
    }
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("vectorOS settings"));
    window.add_css_class("vector");
    window.add_css_class("settings");
    window.set_default_size(1000, 720);

    let root = ui::hbox(0, &[]);
    let side = ui::vbox(0, &["side"]);
    let brand = ui::hbox(8, &["brand"]);
    brand.append(&ui::text("\u{f055c}", &["mark"]));
    brand.append(&ui::markup("vector<span alpha=\"60%\">OS</span>", &["word"]));
    brand.append(&ui::text("settings", &["dim"]));
    side.append(&brand);
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::None);
    root.append(&side);
    root.append(&stack);
    window.set_child(Some(&root));

    let defs = pages();
    let mut rows = HashMap::new();
    let mut group = "";
    for d in &defs {
        if d.group != group {
            group = d.group;
            side.append(&ui::text(group, &["grp"]));
        }
        let r = ui::hbox(10, &["row"]);
        r.append(&ui::text(d.icon, &["icon"]));
        r.append(&ui::text(d.title, &[]));
        r.set_cursor_from_name(Some("pointer"));
        let id = d.id;
        let g = gtk::GestureClick::new();
        g.connect_released(move |_, _, _, _| show(id));
        r.add_controller(g);
        side.append(&r);
        rows.insert(d.id, r);
    }

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, k, _, m| {
        let ctrl = m.contains(gdk::ModifierType::CONTROL_MASK);
        match k {
            gdk::Key::Escape => {
                if let Some(w) = WIN.with(|w| w.borrow().clone()) {
                    w.window.close();
                }
            }
            gdk::Key::q | gdk::Key::w if ctrl => {
                if let Some(w) = WIN.with(|w| w.borrow().clone()) {
                    w.window.close();
                }
            }
            gdk::Key::Page_Down if ctrl => step(1),
            gdk::Key::Page_Up if ctrl => step(-1),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    window.connect_close_request(|_| {
        // pages drop their D-Bus agents and timers with the window
        if let Some(w) = WIN.with(|w| w.borrow_mut().take()) {
            for (_, v) in w.built.borrow_mut().drain() {
                v.stop();
            }
        }
        glib::Propagation::Proceed
    });

    Rc::new(Win {
        window,
        stack,
        rows,
        defs,
        built: RefCell::default(),
        current: RefCell::new(""),
        ctx: Ctx::new(),
    })
}

fn step(by: i32) {
    let Some(w) = WIN.with(|w| w.borrow().clone()) else { return };
    let cur = *w.current.borrow();
    let n = w.defs.len() as i32;
    let i = w.defs.iter().position(|d| d.id == cur).unwrap_or(0) as i32;
    let next = w.defs[(i + by).rem_euclid(n) as usize].id;
    show(next);
}

fn show(id: &str) {
    let Some(w) = WIN.with(|w| w.borrow().clone()) else { return };
    let Some(d) = w.defs.iter().find(|d| d.id == id) else { return };
    let id = d.id;
    if !w.built.borrow().contains_key(id) {
        let v = (d.build)(&w.ctx);
        w.stack.add_named(&v.root, Some(id));
        w.built.borrow_mut().insert(id, v);
    }
    let prev = w.current.replace(id);
    if prev != id
        && let Some(v) = w.built.borrow().get(prev)
    {
        v.hidden();
    }
    w.stack.set_visible_child_name(id);
    for (k, r) in &w.rows {
        if *k == id {
            r.add_css_class("active");
        } else {
            r.remove_css_class("active");
        }
    }
    if let Some(v) = w.built.borrow().get(id) {
        v.shown();
    }
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--list") {
        for d in pages() {
            println!("{}", d.id);
        }
        return glib::ExitCode::SUCCESS;
    }
    if matches!(args.get(1).map(String::as_str), Some("-h" | "--help")) {
        println!("usage: vector-settings [PAGE] | --list");
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::new(Some(APP_ID), gio::ApplicationFlags::HANDLES_COMMAND_LINE);
    app.connect_command_line(|app, cl| {
        let args: Vec<String> = cl
            .arguments()
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        let want = args.first().cloned();
        let w = WIN.with(|w| w.borrow().clone()).unwrap_or_else(|| {
            let w = build(app);
            WIN.with(|x| *x.borrow_mut() = Some(w.clone()));
            w
        });
        let first = w.defs[0].id;
        let id = want
            .as_deref()
            .map(|a| match a {
                "wi-fi" | "network" => "wifi",
                other => other,
            })
            .filter(|a| w.defs.iter().any(|d| d.id == *a))
            .unwrap_or_else(|| {
                let cur = *w.current.borrow();
                if cur.is_empty() { first } else { cur }
            })
            .to_string();
        show(&id);
        w.window.present();
        glib::ExitCode::SUCCESS
    });
    app.run()
}
