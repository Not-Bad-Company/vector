//! Notification bell. `󰂞 3` while notifications are on screen, `󰂚 2` in the
//! accent for ones that went by unseen, dim when there is nothing, crossed out
//! when silenced. Click: panel (on screen with actions, the day's history,
//! silence switch). Middle: dismiss all. Right: silence.

use std::rc::Rc;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::mako::{self, Mako, Note};
use crate::notify::{self, plain};
use crate::panel::{Card, Panel};
use crate::slot::Slot;
use crate::ui::{self, Tap};
use crate::util;

struct Inner {
    slot: Slot,
    mako: Rc<Mako>,
    listener: u64,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.mako.changed.disconnect(self.listener);
    }
}

pub struct Notifications(Rc<Inner>);

fn unseen(m: &Mako) -> usize {
    m.store().map(|s| s.unseen()).unwrap_or(0)
}

fn render(slot: &Slot, m: &Mako) {
    let n = m.on_screen().len();
    let missed = unseen(m);
    if m.dnd() {
        slot.set_markup(&if missed > 0 {
            format!("󰂛 {missed}")
        } else {
            "󰂛".into()
        });
        slot.set_classes(&["dim"]);
    } else if n > 0 {
        slot.set_markup(&format!("󰂞 {n}"));
        slot.set_classes(&[]);
    } else if missed > 0 {
        slot.set_markup(&format!("󰂚 {missed}"));
        slot.set_classes(&["unseen"]);
    } else {
        slot.set_markup("󰂚");
        slot.set_classes(&["dim"]);
    }
    slot.bump();
}

/// The notification-center TUI's "hide copy notifications" switch.
fn hide_copy() -> bool {
    let p = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| util::home().join(".local/state"));
    util::read_json(p.join("notification-center.json"))
        .and_then(|v| v["hide_copy"].as_bool())
        .unwrap_or(true)
}

fn keep(n: &Note, hide: bool) -> bool {
    !hide
        || !(n.app == "clipboard"
            || n.summary
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w == "copied"))
}

fn status(m: &Mako, on_screen: usize) -> String {
    let missed = unseen(m);
    let mut parts = Vec::new();
    if m.dnd() {
        parts.push("silenced".to_string());
    }
    if on_screen > 0 {
        parts.push(format!("{on_screen} on screen"));
    }
    if missed > 0 {
        parts.push(format!("{missed} missed"));
    }
    if parts.is_empty() {
        "all caught up".into()
    } else {
        parts.join(" · ")
    }
}

impl Notifications {
    pub fn new(ctx: &Ctx) -> Notifications {
        let m = mako::get();
        let slot = ctx.slot("notifications", &[]);
        let s2 = slot.clone();
        let m2 = Rc::downgrade(&m);
        let listener = m.changed.connect(move || {
            if let Some(m) = m2.upgrade() {
                render(&s2, &m);
            }
        });
        render(&slot, &m);
        slot.on_middle(|| mako::get().dismiss_all());
        slot.on_right(|| {
            let m = mako::get();
            m.set_dnd(!m.dnd());
        });
        slot.tooltip(|| {
            let m = mako::get();
            let notes = m.on_screen();
            let s = ui::sheet();
            s.append(&ui::head("notifications", &status(&m, notes.len())).0);
            if !notes.is_empty() && !m.dnd() {
                s.append(&ui::tsection("on screen"));
                for n in notes.iter().take(6) {
                    let app: String = n.label().to_lowercase().chars().take(9).collect();
                    s.append(&ui::tkv(&app, &util::esc(&plain(&n.summary))));
                }
                if notes.len() > 6 {
                    s.append(&ui::text(
                        &format!("and {} more", notes.len() - 6),
                        &["dim"],
                    ));
                }
            }
            if let Some(st) = m.store() {
                let missed: Vec<Note> = st
                    .history()
                    .into_iter()
                    .filter(|n| n.unseen)
                    .take(5)
                    .collect();
                if !missed.is_empty() {
                    s.append(&ui::tsection("missed"));
                    for n in &missed {
                        let app: String = n.label().to_lowercase().chars().take(9).collect();
                        s.append(&ui::tkv(
                            &app,
                            &format!(
                                "{}  {}",
                                util::esc(&plain(&n.summary)),
                                ui::dim(&when(n.time))
                            ),
                        ));
                    }
                }
            }
            s.append(&ui::thint(
                "click notifications  ·  middle clear all  ·  right silence",
            ));
            Some(s.upcast())
        });
        slot.panel(panel);
        Notifications(Rc::new(Inner {
            slot,
            mako: m,
            listener,
        }))
    }
}

impl Module for Notifications {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.mako.refresh();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

/// "now", "12m", "14:05", "yesterday 09:12", "3 Oct".
fn when(ts: f64) -> String {
    if ts <= 0.0 {
        return String::new();
    }
    let age = util::now() - ts;
    if age < 60.0 {
        return "now".into();
    }
    if age < 3600.0 {
        return format!("{}m", (age / 60.0) as i64);
    }
    match day(ts) {
        0 => util::local(ts, "%H:%M"),
        1 => util::local(ts, "yesterday %H:%M"),
        d if d < 7 => util::local(ts, "%a %H:%M"),
        _ => util::local(ts, "%-d %b"),
    }
}

/// Whole days between `ts` and today (0 = today).
fn day(ts: f64) -> i64 {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(ts as i64, 0).single() else {
        return 99;
    };
    (Local::now().date_naive() - t.date_naive()).num_days()
}

fn note_row(n: &Note, live: bool, m: &Rc<Mako>) -> gtk::Widget {
    let b = ui::vbox(2, &["row", "nrow"]);
    if n.unseen {
        b.add_css_class("active");
    }
    let app = n.label();
    let mut summary = plain(&n.summary);
    // "Bluetooth · Bluetooth connected" -> "Bluetooth · connected"
    if summary.eq_ignore_ascii_case(&app) {
        summary.clear();
    } else if summary
        .to_lowercase()
        .starts_with(&format!("{} ", app.to_lowercase()))
    {
        summary = summary[app.len() + 1..].to_string();
    }
    let top = ui::hbox(8, &[]);
    top.append(&ui::text(
        &app.to_lowercase(),
        &[if n.urgency >= 2 {
            "crit"
        } else if n.urgency == 0 {
            "dim"
        } else {
            "app"
        }],
    ));
    let sum = ui::text(&summary, &[]);
    sum.set_hexpand(true);
    ui::ellipsize(&sum, 28);
    top.append(&sum);
    if n.count > 1 {
        top.append(&ui::text(&format!("×{}", n.count), &["ok"]));
    }
    let t = when(n.time);
    if !t.is_empty() {
        top.append(&ui::text(&t, &["meta"]));
    }
    let (m2, id) = (m.clone(), n.id);
    let x = if live {
        ui::icon_button("󰅖", "dismiss", move || m2.dismiss(id))
    } else {
        ui::icon_button("󰅖", "forget", move || {
            if let Some(s) = m2.store() {
                s.forget(id);
            }
        })
    };
    x.add_css_class("x");
    x.set_valign(gtk::Align::Start);
    top.append(&x);
    b.append(&top);

    let mid = ui::hbox(10, &[]);
    let body = notify::markup(&n.body);
    if !body.is_empty() {
        let l = ui::label(&body, &["body-text"]);
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_max_width_chars(44);
        l.set_lines(if live { 4 } else { 2 });
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        l.set_hexpand(true);
        mid.append(&l);
    }
    let tex = n.pixels.clone().or_else(|| {
        (!n.image.is_empty())
            .then(|| gtk::gdk::Texture::from_filename(&n.image).ok())
            .flatten()
    });
    if let Some(t) = tex {
        mid.append(&notify::popup::thumb(&t, 36, 36, "nthumb"));
    }
    if mid.first_child().is_some() {
        b.append(&mid);
    }
    if let Some(v) = n.value {
        let row = ui::hbox(10, &[]);
        row.append(&ui::meter(v as f64 / 100.0, ""));
        row.append(&ui::text(&format!("{v}%"), &["dim"]));
        b.append(&row);
    }
    if live {
        let acts: Vec<_> = n
            .actions
            .iter()
            .filter(|(k, l)| k != "default" && !l.is_empty())
            .collect();
        if !acts.is_empty() {
            let row = ui::hbox(4, &[]);
            for (k, label) in acts {
                let (m2, id, k) = (m.clone(), n.id, k.clone());
                row.append(&ui::link(&label.to_lowercase(), &["primary"], move || {
                    m2.invoke(id, &k)
                }));
            }
            b.append(&row);
        }
        // click accepts, like on the card itself
        let (m2, id) = (m.clone(), n.id);
        ui::on_click(&b, move || m2.invoke(id, "default"));
    } else {
        b.add_css_class("past");
    }
    b.upcast()
}

fn panel(p: &Panel) -> gtk::Widget {
    p.set_width(400);
    let m = mako::get();
    let card = Card::new("notifications", "");
    let sw = ui::switch(m.dnd(), |on| mako::get().set_dnd(on));
    card.controls.append(&ui::text("silence", &["dim"]));
    card.controls.append(&sw);
    let live = ui::vbox(0, &[]);
    let hist = ui::vbox(0, &[]);
    card.body.append(&live);
    card.body.append(&hist);
    let hide = hide_copy();

    let fill = {
        let (live, hist, detail, sw) =
            (live.clone(), hist.clone(), card.detail.clone(), sw.clone());
        move || {
            let m = mako::get();
            let notes: Vec<Note> = m
                .on_screen()
                .into_iter()
                .filter(|n| keep(n, hide))
                .collect();
            detail.set_markup(&status(&m, notes.len()));
            if sw.is_active() != m.dnd() {
                sw.set_active(m.dnd());
            }
            ui::clear(&live);
            if !notes.is_empty() {
                live.append(&ui::section("on screen").tap(|l| l.add_css_class("first")));
                for n in &notes {
                    live.append(&note_row(n, true, &m));
                }
            }
            let (hist, m2, on_screen) = (
                hist.clone(),
                m.clone(),
                notes.iter().map(|n| n.id).collect::<Vec<_>>(),
            );
            m.history(move |h| {
                ui::clear(&hist);
                let h: Vec<_> = h
                    .into_iter()
                    .filter(|n| keep(n, hide) && !on_screen.contains(&n.id))
                    .take(40)
                    .collect();
                if h.is_empty() && on_screen.is_empty() {
                    hist.append(&ui::text("all caught up", &["empty-note"]));
                    return;
                }
                // sections by day: today, yesterday, earlier
                let mut last: Option<&str> = None;
                let mut i = 0;
                while i < h.len() {
                    let sec = match day(h[i].time) {
                        _ if h[i].time <= 0.0 => "earlier",
                        0 => "today",
                        1 => "yesterday",
                        _ => "earlier",
                    };
                    if last != Some(sec) {
                        let l = ui::section(sec);
                        if last.is_none() && on_screen.is_empty() {
                            l.add_css_class("first");
                        }
                        hist.append(&l);
                        last = Some(sec);
                    }
                    // runs of the same app + title (sleep lock on/off...) become one row
                    let mut j = i + 1;
                    while j < h.len()
                        && h[j].app == h[i].app
                        && plain(&h[j].summary) == plain(&h[i].summary)
                    {
                        j += 1;
                    }
                    let mut n = h[i].clone();
                    n.count = h[i..j].iter().map(|x| x.count.max(1)).sum();
                    n.unseen = h[i..j].iter().any(|x| x.unseen);
                    hist.append(&note_row(&n, false, &m2));
                    i = j;
                }
            });
        }
    };
    let fill = Rc::new(fill);
    fill();
    let f2 = fill.clone();
    p.watch(&m.changed, move || f2());
    // looked at: nothing is "missed" any more
    p.on_close(|| {
        if let Some(s) = mako::get().store() {
            s.mark_seen();
        }
    });

    card.foot_add(&ui::link("clear all", &["danger"], || {
        let m = mako::get();
        m.dismiss_all();
        if let Some(s) = m.store() {
            s.clear_history();
        }
    }));
    let f3 = fill.clone();
    card.foot_add(&ui::link("restore last", &[], move || {
        mako::get().restore();
        f3();
    }));
    card.foot_add(&ui::spacer());
    let pc = p.clone();
    card.foot_add(&ui::link("center", &[], move || {
        pc.close();
        util::spawn_sh("xdg-terminal-exec notification-center");
    }));
    card.root.upcast()
}
