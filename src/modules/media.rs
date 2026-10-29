//! Now playing (MPRIS over D-Bus, event driven). Hidden when nothing plays.
//! Click: play/pause. Right: player panel (art, seek bar, controls, player
//! switch). Middle: next. Scroll: next/previous.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::gio;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::ui::{self, Tap};
use crate::util;

const MPRIS: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PATH: &str = "/org/mpris/MediaPlayer2";

#[derive(Clone, Default, PartialEq, Debug)]
pub struct Track {
    pub bus: String,
    pub identity: String,
    pub status: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub length_us: i64,
    pub track_id: String,
    pub can_seek: bool,
}

struct Inner {
    slot: Slot,
    conn: Option<gio::DBusConnection>,
    players: RefCell<Vec<Track>>,
    /// bus name the user picked in the panel; else the playing/first one
    chosen: RefCell<Option<String>>,
    changed: Notify,
    max: usize,
    pending: std::cell::Cell<bool>,
    subs: RefCell<Vec<gio::SignalSubscriptionId>>,
}

pub struct Media(Rc<Inner>);

fn props(v: &glib::Variant) -> HashMap<String, glib::Variant> {
    let mut m = HashMap::new();
    // GetAll returns (a{sv})
    let dict = if v.n_children() == 1 && v.type_().is_tuple() { v.child_value(0) } else { v.clone() };
    for i in 0..dict.n_children() {
        let e = dict.child_value(i);
        if let (Some(k), Some(val)) = (e.child_value(0).str().map(str::to_string), e.child_value(1).as_variant()) {
            m.insert(k, val);
        }
    }
    m
}

fn vstr(v: Option<&glib::Variant>) -> String {
    v.and_then(|v| v.str().map(str::to_string)).unwrap_or_default()
}

fn parse(bus: &str, identity: String, all: &glib::Variant) -> Track {
    let p = props(all);
    let meta = p.get("Metadata").map(props).unwrap_or_default();
    let artist = meta
        .get("xesam:artist")
        .and_then(|v| v.get::<Vec<String>>())
        .map(|a| a.join(", "))
        .unwrap_or_else(|| vstr(meta.get("xesam:artist")));
    let length = meta
        .get("mpris:length")
        .and_then(|v| v.get::<i64>().or_else(|| v.get::<u64>().map(|u| u as i64)))
        .unwrap_or(0);
    let track_id = meta
        .get("mpris:trackid")
        .and_then(|v| v.str().map(str::to_string).or_else(|| v.get::<glib::variant::ObjectPath>().map(|o| o.as_str().to_string())))
        .unwrap_or_default();
    Track {
        bus: bus.to_string(),
        identity,
        status: vstr(p.get("PlaybackStatus")),
        title: vstr(meta.get("xesam:title")),
        artist,
        album: vstr(meta.get("xesam:album")),
        art: vstr(meta.get("mpris:artUrl")),
        length_us: length,
        track_id,
        can_seek: p.get("CanSeek").and_then(|v| v.get::<bool>()).unwrap_or(false),
    }
}

impl Media {
    pub fn new(ctx: &Ctx) -> Media {
        let o = ctx.opts("media");
        let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok();
        let inner = Rc::new(Inner {
            slot: ctx.slot("media", &[]),
            conn,
            players: RefCell::default(),
            chosen: RefCell::default(),
            changed: Notify::default(),
            max: o.num("max_length", 45.0) as usize,
            pending: std::cell::Cell::new(false),
            subs: RefCell::default(),
        });
        inner.slot.set_visible(false);
        let w = Rc::downgrade(&inner);
        inner.slot.on_left(move || {
            if let Some(i) = w.upgrade() {
                i.call("PlayPause");
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.call("Next");
            }
        });
        let (w1, w2) = (Rc::downgrade(&inner), Rc::downgrade(&inner));
        inner.slot.on_scroll(
            move || {
                if let Some(i) = w1.upgrade() {
                    i.call("Next")
                }
            },
            move || {
                if let Some(i) = w2.upgrade() {
                    i.call("Previous")
                }
            },
        );
        let w = Rc::downgrade(&inner);
        inner.slot.on_right(move || {
            if let Some(i) = w.upgrade() {
                i.slot.toggle_panel(None, false);
            }
        });
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().and_then(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });

        if let Some(c) = &inner.conn {
            // any player's property change or a player appearing/leaving
            for (iface, member) in [("org.freedesktop.DBus.Properties", "PropertiesChanged"), ("org.freedesktop.DBus", "NameOwnerChanged")] {
                let w = Rc::downgrade(&inner);
                let path = if member == "PropertiesChanged" { Some(PATH) } else { None };
                let id = c.signal_subscribe(None, Some(iface), Some(member), path, None, gio::DBusSignalFlags::NONE, move |_, _, _, _, _, args| {
                    if let Some(i) = w.upgrade() {
                        if member == "NameOwnerChanged" && !args.child_value(0).str().is_some_and(|n| n.starts_with(MPRIS)) {
                            return;
                        }
                        i.schedule();
                    }
                });
                inner.subs.borrow_mut().push(id);
            }
        }
        inner.query();
        util::every(5000, &inner, |i| i.query());
        Media(inner)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(c) = &self.conn {
            for id in self.subs.borrow_mut().drain(..) {
                c.signal_unsubscribe(id);
            }
        }
    }
}

impl Inner {
    fn schedule(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        util::after(60, self, |i| {
            i.pending.set(false);
            i.query();
        });
    }

    fn query(self: &Rc<Self>) {
        let Some(c) = self.conn.clone() else { return };
        let w = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let names = c
                .call_future(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", "org.freedesktop.DBus", "ListNames", None, None, gio::DBusCallFlags::NONE, 1000)
                .await;
            let mut buses: Vec<String> = names
                .ok()
                .and_then(|v| v.child_value(0).get::<Vec<String>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n.starts_with(&format!("{MPRIS}.")))
                .collect();
            buses.sort();
            let mut tracks = Vec::new();
            for bus in buses {
                let all = c
                    .call_future(Some(&bus), PATH, "org.freedesktop.DBus.Properties", "GetAll", Some(&(PLAYER,).to_variant()), None, gio::DBusCallFlags::NONE, 800)
                    .await;
                let ident = c
                    .call_future(Some(&bus), PATH, "org.freedesktop.DBus.Properties", "Get", Some(&(MPRIS, "Identity").to_variant()), None, gio::DBusCallFlags::NONE, 500)
                    .await
                    .ok()
                    .and_then(|v| v.child_value(0).as_variant().and_then(|x| x.str().map(str::to_string)))
                    .unwrap_or_else(|| bus.trim_start_matches(&format!("{MPRIS}.")).split('.').next().unwrap_or("").to_string());
                if let Ok(all) = all {
                    tracks.push(parse(&bus, ident, &all));
                }
            }
            let Some(i) = w.upgrade() else { return };
            let changed = *i.players.borrow() != tracks;
            *i.players.borrow_mut() = tracks;
            i.render();
            if changed {
                i.changed.emit();
            }
        });
    }

    /// The player shown: the picked one if still there, else playing, else paused with a title.
    fn current(&self) -> Option<Track> {
        let ps = self.players.borrow();
        if let Some(c) = self.chosen.borrow().as_ref() {
            if let Some(t) = ps.iter().find(|t| &t.bus == c) {
                return Some(t.clone());
            }
        }
        ps.iter()
            .find(|t| t.status == "Playing")
            .or_else(|| ps.iter().find(|t| t.status == "Paused" && !t.title.is_empty()))
            .cloned()
    }

    fn render(&self) {
        let Some(t) = self.current().filter(|t| !t.title.is_empty() && t.status != "Stopped") else {
            self.slot.set_visible(false);
            return;
        };
        let meta = if t.artist.is_empty() { t.title.clone() } else { format!("{} — {}", t.artist, t.title) };
        let short = if meta.chars().count() > self.max { meta.chars().take(self.max).collect::<String>() + "…" } else { meta };
        let icon = if t.status == "Playing" { "󰐊" } else { "󰏤" };
        self.slot.set_markup(&format!("{icon}  {}", util::esc(&short)));
        self.slot.set_classes(&[if t.status == "Playing" { "playing" } else { "dim" }]);
        self.slot.set_visible(true);
    }

    fn call(&self, method: &str) {
        let (Some(c), Some(t)) = (self.conn.clone(), self.current()) else { return };
        c.call(Some(&t.bus), PATH, PLAYER, method, None, None, gio::DBusCallFlags::NONE, 2000, gio::Cancellable::NONE, |_| {});
    }

    fn tooltip(&self) -> Option<gtk::Widget> {
        let t = self.current()?;
        let s = ui::sheet();
        s.append(&ui::head("now playing", &format!("{} · {}", t.status.to_lowercase(), util::esc(&t.identity.to_lowercase()))).0);
        s.append(&ui::text(&t.title, &[]).tap(|l| l.set_margin_top(6)));
        if !t.artist.is_empty() {
            s.append(&ui::text(&t.artist, &["dim"]));
        }
        s.append(&ui::thint("click play/pause  ·  right player  ·  middle next  ·  scroll skip"));
        Some(s.upcast())
    }
}

impl Module for Media {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.query();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

fn fmt_us(us: i64) -> String {
    let s = (us / 1_000_000).max(0);
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Album art from a file:// url, or downloaded once into the cache for http(s).
fn load_art(url: &str, pic: &gtk::Picture) {
    if let Some(path) = url.strip_prefix("file://") {
        let path = glib::Uri::unescape_string(path, None::<&str>).map(|s| s.to_string()).unwrap_or(path.to_string());
        pic.set_filename(Some(path));
        return;
    }
    if !url.starts_with("http") {
        pic.set_paintable(None::<&gtk::gdk::Paintable>);
        return;
    }
    let dest = util::cache_dir().join("vector-bar/art").join(format!("{:x}", md5ish(url)));
    if dest.exists() {
        pic.set_filename(Some(&dest));
        return;
    }
    let url = url.to_string();
    let pic = pic.downgrade();
    util::bg(
        move || {
            let bytes = ureq::get(&url).call().ok()?.body_mut().read_to_vec().ok()?;
            std::fs::create_dir_all(dest.parent()?).ok()?;
            std::fs::write(&dest, bytes).ok()?;
            Some(dest)
        },
        move |p| {
            if let (Some(p), Some(pic)) = (p, pic.upgrade()) {
                pic.set_filename(Some(&p));
            }
        },
    );
}

fn md5ish(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(360);
    let card = Card::new("now playing", "");
    let top = ui::hbox(12, &[]).tap(|b| {
        b.set_margin_start(16);
        b.set_margin_end(16);
        b.set_margin_bottom(8);
    });
    let art = gtk::Picture::new();
    art.set_size_request(84, 84);
    art.set_content_fit(gtk::ContentFit::Cover);
    art.add_css_class("thumb-art");
    let art_frame = gtk::Frame::new(None);
    art_frame.set_child(Some(&art));
    art_frame.set_size_request(84, 84);
    top.append(&art_frame);
    let info = ui::vbox(2, &[]);
    info.set_valign(gtk::Align::Center);
    let title = ui::text("", &[]).tap(|l| {
        l.set_wrap(true);
        l.set_max_width_chars(26);
        l.set_lines(2);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    });
    let artist = ui::text("", &["dim"]).tap(|l| ui::ellipsize(l, 28));
    let album = ui::text("", &["faint"]).tap(|l| ui::ellipsize(l, 28));
    info.append(&title);
    info.append(&artist);
    info.append(&album);
    top.append(&info);
    card.body.append(&top);

    // seek bar
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    seek.set_draw_value(false);
    seek.set_margin_start(16);
    seek.set_margin_end(16);
    card.body.append(&seek);
    let times = ui::hbox(0, &[]).tap(|b| {
        b.set_margin_start(16);
        b.set_margin_end(16);
    });
    let pos_l = ui::text("0:00", &["dim"]);
    let len_l = ui::text("", &["dim"]);
    times.append(&pos_l);
    times.append(&ui::spacer());
    times.append(&len_l);
    card.body.append(&times);

    let controls = ui::hbox(6, &[]).tap(|b| {
        b.set_halign(gtk::Align::Center);
        b.set_margin_top(6);
        b.set_margin_bottom(4);
    });
    let mk = |glyph: &str, tip: &str, m: &'static str| {
        let w = Rc::downgrade(i);
        ui::icon_button(glyph, tip, move || {
            if let Some(i) = w.upgrade() {
                i.call(m);
                i.schedule();
            }
        })
        .tap(|b| b.add_css_class("big"))
    };
    controls.append(&mk("󰒮", "previous", "Previous"));
    let play = mk("󰐊", "play / pause", "PlayPause");
    controls.append(&play);
    controls.append(&mk("󰒭", "next", "Next"));
    card.body.append(&controls);

    let players = ui::hbox(0, &["chips"]);
    card.body.append(&players);

    let dragging = Rc::new(std::cell::Cell::new(false));
    let last_art = Rc::new(RefCell::new(String::new()));
    let fill = {
        let i = Rc::downgrade(i);
        let (title, artist, album, art, play, players, detail, len_l) = (title.clone(), artist.clone(), album.clone(), art.clone(), play.clone(), players.clone(), card.detail.clone(), len_l.clone());
        let last_art = last_art.clone();
        let seek = seek.clone();
        move || {
            let Some(i) = i.upgrade() else { return };
            let Some(t) = i.current() else {
                title.set_text("nothing playing");
                return;
            };
            detail.set_markup(&format!("{} · {}", t.status.to_lowercase(), util::esc(&t.identity.to_lowercase())));
            title.set_text(&t.title);
            artist.set_text(&t.artist);
            album.set_text(&t.album);
            album.set_visible(!t.album.is_empty());
            play.set_label(if t.status == "Playing" { "󰏤" } else { "󰐊" });
            if *last_art.borrow() != t.art {
                *last_art.borrow_mut() = t.art.clone();
                load_art(&t.art, &art);
            }
            len_l.set_text(&if t.length_us > 0 { fmt_us(t.length_us) } else { String::new() });
            seek.set_sensitive(t.can_seek && t.length_us > 0);
            ui::clear(&players);
            let all = i.players.borrow().clone();
            if all.len() > 1 {
                for pl in all {
                    let w = Rc::downgrade(&i);
                    let bus = pl.bus.clone();
                    players.append(&ui::chip(&pl.identity.to_lowercase(), pl.bus == t.bus, move || {
                        if let Some(i) = w.upgrade() {
                            *i.chosen.borrow_mut() = Some(bus.clone());
                            i.render();
                            i.changed.emit();
                        }
                    })
                    .tap(|c| c.set_margin_end(4)));
                }
            }
        }
    };
    let fill = Rc::new(fill);
    fill();
    let f2 = fill.clone();
    p.watch(&i.changed, move || f2());

    // position is not signalled: poll it while the panel is open
    let poll = {
        let i = Rc::downgrade(i);
        let (seek, pos_l, dragging) = (seek.clone(), pos_l.clone(), dragging.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            let (Some(c), Some(t)) = (i.conn.clone(), i.current()) else { return };
            if dragging.get() {
                return;
            }
            let (seek, pos_l) = (seek.clone(), pos_l.clone());
            c.call(Some(&t.bus), PATH, "org.freedesktop.DBus.Properties", "Get", Some(&(PLAYER, "Position").to_variant()), None, gio::DBusCallFlags::NONE, 500, gio::Cancellable::NONE, move |r| {
                let pos = r.ok().and_then(|v| v.child_value(0).as_variant()).and_then(|v| v.get::<i64>()).unwrap_or(0);
                pos_l.set_text(&fmt_us(pos));
                if t.length_us > 0 {
                    seek.set_value(pos as f64 / t.length_us as f64);
                }
            });
        }
    };
    poll();
    p.every(1000, poll);

    // seeking: SetPosition when the user lets go of the slider
    let gesture = gtk::GestureClick::new();
    {
        let d = dragging.clone();
        gesture.connect_pressed(move |_, _, _, _| d.set(true));
    }
    {
        let (d, i, seek) = (dragging.clone(), Rc::downgrade(i), seek.clone());
        gesture.connect_released(move |_, _, _, _| {
            d.set(false);
            let Some(i) = i.upgrade() else { return };
            let (Some(c), Some(t)) = (i.conn.clone(), i.current()) else { return };
            if t.track_id.is_empty() {
                return;
            }
            let pos = (seek.value() * t.length_us as f64) as i64;
            if let Ok(path) = glib::variant::ObjectPath::try_from(t.track_id.clone()) {
                c.call(Some(&t.bus), PATH, PLAYER, "SetPosition", Some(&(path, pos).to_variant()), None, gio::DBusCallFlags::NONE, 1000, gio::Cancellable::NONE, |_| {});
            }
        });
    }
    seek.add_controller(gesture);
    let _ = Duration::ZERO;
    card.root.upcast()
}
