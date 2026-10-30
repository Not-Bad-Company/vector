//! About: the vectorOS version, the machine, and updates.

use std::rc::Rc;

use gtk::prelude::*;

use crate::page::{Ctx, Frame, View};
use crate::ui;

fn gib(b: f64) -> String {
    format!("{:.1} GiB", b / 1024f64.powi(3))
}

fn version() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    std::fs::read_to_string(format!("{home}/.local/share/vectoros/version"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

pub fn page(_: &Ctx) -> View {
    let f = Frame::new("about");
    f.detail.set_markup(&ui::esc(&format!("vectorOS {}", version())));

    let mark = ui::hbox(14, &["about-mark"]);
    mark.set_margin_top(10);
    let glyph = ui::text("\u{f055c}", &["title"]);
    glyph.add_css_class("about-glyph");
    mark.append(&glyph);
    let names = ui::vbox(2, &[]);
    names.append(&ui::markup("<b>vector</b><span alpha=\"60%\"><b>OS</b></span>", &["about-word"]));
    names.append(&ui::text("Arch Linux · Hyprland · the Vector apps", &["dim"]));
    mark.append(&names);
    f.body.append(&mark);

    f.body.append(&ui::section("this machine"));
    let hw = ui::group();
    hw.append(&ui::text("reading…", &["note"]));
    f.body.append(&hw);

    f.body.append(&ui::section("updates"));
    let up = ui::group();
    for (icon, name, meta, cmd) in [
        ("\u{f06b0}", "update everything", "packages, AUR, firmware hooks", "vector-update"),
        ("\u{f0ab7}", "install", "packages, AUR, web apps", "vector-install"),
        ("\u{f05e8}", "remove", "packages, web apps, themes", "vector-remove"),
    ] {
        let r = ui::Row::new(icon, name, &ui::dim(meta));
        r.on_click(move || ui::spawn(&[cmd]));
        up.append(&r.root);
    }
    f.body.append(&up);

    let fill = {
        let hw = hw.clone();
        Rc::new(move || {
            let hw = hw.clone();
            glib::spawn_future_local(async move {
                let (ok, out, _) = ui::run(
                    ["fastfetch", "-s", "host:cpu:gpu:memory:disk:kernel:wm:uptime:packages", "--format", "json"]
                        .map(String::from)
                        .to_vec(),
                )
                .await;
                ui::clear(&hw);
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&out).map_err(|_| ()).and_then(|v| if ok { Ok(v) } else { Err(()) }) else {
                    hw.append(&ui::text("couldn't read the system (fastfetch)", &["note"]));
                    return;
                };
                let get = |t: &str| {
                    v.as_array()
                        .and_then(|a| a.iter().find(|x| x["type"] == t))
                        .map(|x| x["result"].clone())
                        .unwrap_or_default()
                };
                let host = get("Host");
                hw.append(&ui::kv("model", &ui::esc(host["family"].as_str().or(host["name"].as_str()).unwrap_or("?"))));
                let cpu = get("CPU");
                hw.append(&ui::kv(
                    "processor",
                    &format!(
                        "{} {}",
                        ui::esc(cpu["cpu"].as_str().unwrap_or("?")),
                        ui::dim(&format!("{} threads", cpu["cores"]["logical"].as_i64().unwrap_or(0)))
                    ),
                ));
                for g in get("GPU").as_array().into_iter().flatten() {
                    hw.append(&ui::kv("graphics", &ui::esc(g["name"].as_str().unwrap_or("?"))));
                }
                let m = get("Memory");
                if let (Some(u), Some(t)) = (m["used"].as_f64(), m["total"].as_f64()) {
                    hw.append(&ui::kv("memory", &format!("{} {}", gib(t), ui::dim(&format!("{} in use", gib(u))))));
                }
                for d in get("Disk").as_array().into_iter().flatten() {
                    if d["mountpoint"] != "/" {
                        continue;
                    }
                    if let (Some(fr), Some(t)) = (d["bytes"]["free"].as_f64(), d["bytes"]["total"].as_f64()) {
                        let pct = 100.0 - fr / t * 100.0;
                        let free = format!("{} free", gib(fr));
                        let s = if pct > 90.0 {
                            format!("{} <span foreground=\"#d77\">{}</span>", gib(t), ui::esc(&free))
                        } else {
                            format!("{} {}", gib(t), ui::dim(&free))
                        };
                        hw.append(&ui::kv("disk", &s));
                    }
                }
                let k = get("Kernel");
                hw.append(&ui::kv("kernel", &ui::esc(k["release"].as_str().unwrap_or("?"))));
                let wm = get("WM");
                hw.append(&ui::kv(
                    "compositor",
                    &ui::esc(&format!("{} {}", wm["prettyName"].as_str().unwrap_or("?"), wm["version"].as_str().unwrap_or(""))),
                ));
                let p = get("Packages");
                hw.append(&ui::kv("packages", &format!("{}", p["all"].as_i64().unwrap_or(0))));
                if let Some(ms) = get("Uptime")["uptime"].as_i64() {
                    let mins = ms / 60_000;
                    let s = if mins >= 1440 {
                        format!("{} d {} h", mins / 1440, mins % 1440 / 60)
                    } else {
                        format!("{} h {} min", mins / 60, mins % 60)
                    };
                    hw.append(&ui::kv("up for", &s));
                }
            });
        })
    };
    let fill2 = fill.clone();
    View::new(&f.root).on_show(move || fill2())
}
