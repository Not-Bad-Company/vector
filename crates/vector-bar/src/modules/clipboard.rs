//! Clipboard history (clipse's JSON; recording is clipse.service).
//! Click: panel with search, pins, images, delete. ↑↓ select, ⏎ copy.
//! `vector-bar toggle clipboard@center` opens it in the middle (SUPER ALT V).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use gtk::gdk;
use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::Slot;
use crate::ui::{self, Tap};
use crate::util;

const SHOWN: usize = 60;

fn clipse_dir() -> PathBuf {
    util::config_dir().join("clipse")
}

fn clipse_file() -> PathBuf {
    let d = clipse_dir();
    let name = util::read_json(d.join("config.json"))
        .and_then(|v| v["historyFile"].as_str().map(str::to_string))
        .unwrap_or_else(|| "clipboard_history.json".into());
    d.join(name)
}

#[derive(Clone, Debug)]
struct Clip {
    index: usize,
    value: String,
    recorded: String,
    pinned: bool,
    image: Option<String>,
}

fn load() -> Vec<Clip> {
    let Some(v) = util::read_json(clipse_file()) else {
        return Vec::new();
    };
    let Some(arr) = v["clipboardHistory"].as_array() else {
        return Vec::new();
    };
    arr.iter()
        .enumerate()
        .map(|(i, h)| {
            let fp = h["filePath"]
                .as_str()
                .filter(|p| !p.is_empty() && *p != "null")
                .map(str::to_string);
            Clip {
                index: i,
                value: h["value"].as_str().unwrap_or("").to_string(),
                recorded: h["recorded"].as_str().unwrap_or("").to_string(),
                pinned: h["pinned"].as_bool().unwrap_or(false),
                image: fp,
            }
        })
        .collect()
}

fn age(recorded: &str) -> String {
    let Ok(t) = chrono::NaiveDateTime::parse_from_str(
        recorded.get(..19).unwrap_or(""),
        "%Y-%m-%d %H:%M:%S",
    ) else {
        return String::new();
    };
    let s = (chrono::Local::now().naive_local() - t).num_seconds();
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86400)
    }
}

fn mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(clipse_file())
        .and_then(|m| m.modified())
        .ok()
}

fn copy(c: &Clip) {
    let (data, args): (Vec<u8>, Vec<&str>) = match &c.image {
        Some(p) => match std::fs::read(p) {
            Ok(d) => (d, vec!["wl-copy", "--type", "image/png"]),
            Err(_) => return,
        },
        None => (c.value.as_bytes().to_vec(), vec!["wl-copy"]),
    };
    std::thread::spawn(move || {
        if let Ok(mut ch) = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut si) = ch.stdin.take() {
                let _ = si.write_all(&data);
            }
            let _ = ch.wait();
        }
    });
}

/// Pin/unpin or delete one entry: atomic rewrite of clipse's JSON.
/// Returns false when the file changed underneath (a new copy came in).
fn edit(c: &Clip, delete: bool) -> bool {
    let path = clipse_file();
    let Some(mut doc) = util::read_json(&path) else {
        return false;
    };
    let Some(hist) = doc["clipboardHistory"].as_array_mut() else {
        return false;
    };
    if c.index >= hist.len() || hist[c.index]["recorded"].as_str() != Some(&c.recorded) {
        return false;
    }
    if delete {
        let gone = hist.remove(c.index);
        if let Some(fp) = gone["filePath"].as_str()
            && fp.starts_with(&*clipse_dir().to_string_lossy())
        {
            let _ = std::fs::remove_file(fp);
        }
    } else {
        let p = hist[c.index]["pinned"].as_bool().unwrap_or(false);
        hist[c.index]["pinned"] = serde_json::Value::Bool(!p);
    }
    util::write_atomic(&path, &doc.to_string()).is_ok()
}

pub struct Clipboard(Slot);

impl Clipboard {
    pub fn new(ctx: &Ctx) -> Clipboard {
        let slot = ctx.slot("clipboard", &[]);
        slot.set_markup("\u{f014c}");
        slot.tooltip(|| {
            let s = ui::sheet();
            let n = load().len();
            s.append(
                &ui::head(
                    "clipboard",
                    &format!("{n} item{}", if n == 1 { "" } else { "s" }),
                )
                .0,
            );
            s.append(&ui::thint("click open  ·  super alt v"));
            Some(s.upcast())
        });
        slot.panel(panel);
        Clipboard(slot)
    }
}

impl Module for Clipboard {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.view()
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.clone()]
    }
}

struct PState {
    all: Vec<Clip>,
    shown: Vec<Clip>,
    sel: usize,
    query: String,
    mtime: Option<std::time::SystemTime>,
    thumbs: HashMap<String, Option<gdk::Texture>>,
}

fn panel(p: &Panel) -> gtk::Widget {
    p.set_width(440);
    let card = Card::new("clipboard", "");
    let search = gtk::SearchEntry::new();
    search.set_hexpand(true);
    search.set_placeholder_text(Some("search"));
    card.body.append(&ui::hbox(0, &["search"]).tap(|b| {
        b.set_margin_top(4);
        b.append(&search);
    }));
    let list = ui::vbox(0, &[]);
    card.body.append(&list);
    let st = Rc::new(RefCell::new(PState {
        all: load(),
        shown: vec![],
        sel: 0,
        query: String::new(),
        mtime: mtime(),
        thumbs: HashMap::new(),
    }));
    let draw: crate::ui::Redraw = Rc::default();
    let redraw = {
        let d = draw.clone();
        Rc::new(move || {
            let f = d.borrow().clone();
            if let Some(f) = f {
                f();
            }
        }) as Rc<dyn Fn()>
    };

    let render = {
        let (st, list, detail, pc, rr) = (
            st.clone(),
            list.clone(),
            card.detail.clone(),
            p.clone(),
            redraw.clone(),
        );
        move || {
            let mut s = st.borrow_mut();
            let q = s.query.to_lowercase();
            let matches: Vec<Clip> = s
                .all
                .iter()
                .filter(|h| {
                    q.is_empty()
                        || if h.image.is_some() {
                            "image".contains(&q)
                        } else {
                            h.value.to_lowercase().contains(&q)
                        }
                })
                .cloned()
                .collect();
            let mut shown: Vec<Clip> = matches.iter().filter(|h| h.pinned).cloned().collect();
            let pinned_n = shown.len();
            shown.extend(matches.iter().filter(|h| !h.pinned).take(SHOWN).cloned());
            s.sel = s.sel.min(shown.len().saturating_sub(1));
            let total = s.all.len();
            detail.set_markup(&if total > 0 {
                format!("{total} item{}", if total == 1 { "" } else { "s" })
            } else {
                String::new()
            });
            ui::clear(&list);
            if shown.is_empty() {
                list.append(&ui::text(
                    if q.is_empty() {
                        "nothing copied yet"
                    } else {
                        "nothing matches"
                    },
                    &["empty-note"],
                ));
            }
            for (idx, h) in shown.iter().enumerate() {
                if pinned_n > 0 && idx == 0 {
                    list.append(&ui::section("pinned"));
                }
                if pinned_n > 0 && idx == pinned_n {
                    list.append(&ui::section("recent"));
                }
                let (icon, name) = match &h.image {
                    Some(_) => ("\u{f02e9}", "image".to_string()),
                    None => {
                        let one: String = h
                            .value
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(200)
                            .collect();
                        (
                            "\u{f014c}",
                            if one.is_empty() {
                                "(blank)".into()
                            } else {
                                one
                            },
                        )
                    }
                };
                let row = ui::Row::new(icon, &name, &age(&h.recorded));
                row.set_active(h.pinned);
                if idx == s.sel {
                    row.root.add_css_class("sel");
                }
                for (glyph, tip, delete) in [
                    (
                        if h.pinned { "\u{f0404}" } else { "\u{f0403}" },
                        if h.pinned { "unpin" } else { "pin" },
                        false,
                    ),
                    ("\u{f0156}", "delete", true),
                ] {
                    let (h2, st2, rr2) = (h.clone(), st.clone(), rr.clone());
                    let b = ui::icon_button(glyph, tip, move || {
                        edit(&h2, delete);
                        let mut s = st2.borrow_mut();
                        s.all = load();
                        s.mtime = mtime();
                        drop(s);
                        rr2();
                    });
                    b.add_css_class("x");
                    row.trail.append(&b);
                }
                let (h2, pc2) = (h.clone(), pc.clone());
                ui::on_click(&row.root, move || {
                    pc2.close();
                    copy(&h2);
                });
                list.append(&row.root);
                if let Some(path) = &h.image {
                    let tex = s
                        .thumbs
                        .entry(path.clone())
                        .or_insert_with(|| gdk::Texture::from_filename(path).ok())
                        .clone();
                    if let Some(tex) = tex {
                        let (w, hh) = (tex.width().max(1) as f64, tex.height().max(1) as f64);
                        let scale = (320.0 / w).min(120.0 / hh).min(1.0);
                        let pic = gtk::Picture::for_paintable(&tex);
                        pic.set_can_shrink(true);
                        pic.set_content_fit(gtk::ContentFit::Contain);
                        pic.set_size_request((w * scale) as i32, (hh * scale) as i32);
                        pic.set_halign(gtk::Align::Start);
                        let holder = ui::hbox(0, &["thumb"]);
                        holder.append(&pic);
                        let (h2, pc2) = (h.clone(), pc.clone());
                        ui::on_click(&holder, move || {
                            pc2.close();
                            copy(&h2);
                        });
                        list.append(&holder);
                    }
                }
            }
            s.shown = shown;
        }
    };
    *draw.borrow_mut() = Some(Rc::new(render));
    redraw();

    {
        let (st, rr) = (st.clone(), redraw.clone());
        search.connect_search_changed(move |e| {
            let mut s = st.borrow_mut();
            s.query = e.text().to_string();
            s.sel = 0;
            drop(s);
            rr();
        });
    }
    {
        let (st, pc) = (st.clone(), p.clone());
        search.connect_activate(move |_| {
            let c = st.borrow().shown.get(st.borrow().sel).cloned();
            if let Some(c) = c {
                pc.close();
                copy(&c);
            }
        });
    }
    {
        let (st, rr, search2) = (st.clone(), redraw.clone(), search.clone());
        p.on_key(move |k, _| {
            let n = st.borrow().shown.len();
            match k {
                gdk::Key::Down | gdk::Key::Up if n > 0 => {
                    let mut s = st.borrow_mut();
                    s.sel = if k == gdk::Key::Down {
                        (s.sel + 1) % n
                    } else {
                        (s.sel + n - 1) % n
                    };
                    drop(s);
                    rr();
                    true
                }
                gdk::Key::Escape if !search2.text().is_empty() => {
                    search2.set_text("");
                    true
                }
                _ => false,
            }
        });
    }
    // new copies while the panel is open
    {
        let (st, rr) = (st.clone(), redraw.clone());
        p.every(1000, move || {
            let m = mtime();
            if m != st.borrow().mtime {
                let mut s = st.borrow_mut();
                s.mtime = m;
                s.all = load();
                drop(s);
                rr();
            }
        });
    }
    let focused = Cell::new(false);
    let s2 = search.clone();
    glib::idle_add_local_once(move || {
        s2.grab_focus();
        let _ = focused;
    });

    let (st2, rr2) = (st.clone(), redraw.clone());
    card.foot_add(&ui::link("clear unpinned", &["danger"], move || {
        let _ = util::run(&["clipse", "-clear"], Duration::from_secs(5));
        let mut s = st2.borrow_mut();
        s.all = load();
        s.mtime = mtime();
        drop(s);
        rr2();
    }));
    card.foot_add(&ui::spacer());
    card.foot_add(&ui::text("↑↓ select  ⏎ copy  esc close", &["key"]));
    card.root.upcast()
}
