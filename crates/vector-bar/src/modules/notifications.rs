//! Notification bell (mako, event driven). Dim when empty, `󰂞 3` while
//! notifications are on screen, crossed out when silenced.
//! Click: panel (on screen with actions, history, silence switch).
//! Middle: dismiss all. Right: silence.

use std::rc::Rc;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::mako::{self, Mako, Note};
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

fn render(slot: &Slot, m: &Mako) {
    let n = m.on_screen().len();
    if m.dnd() {
        slot.set_markup("󰂛");
        slot.set_classes(&["dim"]);
    } else if n > 0 {
        slot.set_markup(&format!("󰂞 {n}"));
        slot.set_classes(&[]);
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
            let detail = if m.dnd() {
                format!(
                    "silenced{}",
                    if notes.is_empty() {
                        String::new()
                    } else {
                        format!(", {} on screen", notes.len())
                    }
                )
            } else if notes.is_empty() {
                "all caught up".into()
            } else {
                format!("{} on screen", notes.len())
            };
            s.append(&ui::head("notifications", &detail).0);
            if !notes.is_empty() && !m.dnd() {
                s.append(&ui::tsection("on screen"));
                for n in notes.iter().take(6) {
                    let app: String = n.app.to_lowercase().chars().take(7).collect();
                    s.append(&ui::tkv(&app, &util::esc(&clean(&n.summary))));
                }
                if notes.len() > 6 {
                    s.append(&ui::text(
                        &format!("and {} more", notes.len() - 6),
                        &["dim"],
                    ));
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

/// Plain text: tags dropped, entities decoded, whitespace collapsed.
fn clean(t: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in t.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn note_row(n: &Note, live: bool, m: &Rc<Mako>) -> gtk::Widget {
    let b = ui::vbox(2, &["row"]);
    let mut app = clean(&n.app);
    let mut summary = clean(&n.summary);
    // "Bluetooth · Bluetooth connected" -> "Bluetooth · connected"
    if summary.eq_ignore_ascii_case(&app) {
        summary.clear();
    } else if summary
        .to_lowercase()
        .starts_with(&format!("{} ", app.to_lowercase()))
    {
        summary = summary[app.len() + 1..].to_string();
    }
    if app.is_empty() || app == "notify-send" || app == "notification" {
        // generic sender: lead with the summary
        app = std::mem::take(&mut summary);
    }
    let top = ui::hbox(8, &[]);
    top.append(&ui::text(
        &app.to_lowercase(),
        &[if n.urgency >= 2 { "crit" } else { "app" }],
    ));
    let sum = ui::text(&summary, &[]);
    sum.set_hexpand(true);
    ui::ellipsize(&sum, 30);
    top.append(&sum);
    if live {
        let (m2, id) = (m.clone(), n.id);
        let x = ui::icon_button("󰅖", "dismiss", move || m2.dismiss(id));
        x.add_css_class("x");
        x.set_valign(gtk::Align::Start);
        top.append(&x);
    }
    b.append(&top);
    let body = clean(&n.body);
    if !body.is_empty() {
        let l = ui::text(&body, &["body-text"]);
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_max_width_chars(44);
        l.set_lines(2);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.append(&l);
    }
    if live {
        let acts: Vec<_> = n.actions.iter().filter(|(k, _)| k != "default").collect();
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
        if n.actions.iter().any(|(k, _)| k == "default") {
            let (m2, id) = (m.clone(), n.id);
            ui::on_click(&b, move || m2.invoke(id, "default"));
        }
    } else {
        b.add_css_class("past");
    }
    b.upcast()
}

fn panel(p: &Panel) -> gtk::Widget {
    p.set_width(380);
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
            detail.set_markup(&if m.dnd() {
                "silenced".to_string()
            } else if notes.is_empty() {
                "all caught up".into()
            } else {
                format!("{} on screen", notes.len())
            });
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
                    .take(15)
                    .collect();
                if h.is_empty() && on_screen.is_empty() {
                    hist.append(&ui::text("all caught up", &["empty-note"]));
                    return;
                }
                if !h.is_empty() {
                    hist.append(&ui::section("earlier"));
                    // runs of the same app + title (sleep lock on/off...) become one row
                    let mut i = 0;
                    while i < h.len() {
                        let mut j = i + 1;
                        while j < h.len()
                            && h[j].app == h[i].app
                            && clean(&h[j].summary) == clean(&h[i].summary)
                        {
                            j += 1;
                        }
                        let row = note_row(&h[i], false, &m2);
                        if j - i > 1 {
                            if let Some(top) = row.first_child().and_downcast::<gtk::Box>() {
                                top.append(&ui::text(&format!("×{}", j - i), &["dim"]));
                            }
                        }
                        hist.append(&row);
                        i = j;
                    }
                }
            });
        }
    };
    let fill = Rc::new(fill);
    fill();
    let f2 = fill.clone();
    p.watch(&m.changed, move || f2());

    card.foot_add(&ui::link("clear all", &["danger"], || {
        mako::get().dismiss_all()
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
