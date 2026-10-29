//! Vector Notifications: the desktop's notification server, built into the bar.
//!
//! Apps talk `org.freedesktop.Notifications`; `makoctl`, Omarchy's scripts and
//! notification-center keep working through mako's own `fr.emersion.Mako`
//! interface, which is served here too (server.rs). popup.rs draws the cards,
//! the bell's panel reads this store directly. History survives restarts:
//! `~/.local/state/vector-bar/notifications.json`.
//!
//! The gestures are the same for every notification: click accepts it (its
//! default action, a rule's `on_click`, or else the app's window is focused),
//! a sideways swipe dismisses it, right click dismisses, middle click
//! dismisses all, hovering pauses the timeouts and shows the whole text.

pub mod popup;
mod server;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::slot::Notify;
use crate::util;

pub const DND: &str = "do-not-disturb";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub summary: String,
    /// as the app sent it (markup); see `markup()` / `plain()`
    pub body: String,
    /// 0 low, 1 normal, 2 critical
    pub urgency: u8,
    pub icon: String,
    pub actions: Vec<(String, String)>,
    pub category: String,
    pub desktop_entry: String,
    /// a picture file: the image-path hint, an app icon given as a file, or
    /// raw image data saved when the note went to the history
    pub image: String,
    /// progress, 0-100 (the "value" hint)
    pub value: Option<i32>,
    /// when it first arrived (unix seconds)
    pub time: f64,
    /// identical notifications folded into this one
    pub count: u32,
    /// went by on its own, never looked at
    pub unseen: bool,
    #[serde(skip)]
    pub pixels: Option<gdk::Texture>,
    /// ms; 0 = stays until dismissed
    #[serde(skip)]
    pub timeout: i32,
    #[serde(skip)]
    pub transient: bool,
    #[serde(skip)]
    pub resident: bool,
    /// x-canonical-private-synchronous / x-dunst-stack-tag: same tag replaces
    #[serde(skip)]
    pub tag: String,
    #[serde(skip)]
    pub on_click: Option<String>,
    /// block label override (rule `title`)
    #[serde(skip)]
    pub title: Option<String>,
    /// live but not drawn (silenced, or a rule's `hide`)
    #[serde(skip)]
    pub hidden: bool,
    #[serde(skip)]
    pub no_history: bool,
}

impl Note {
    /// What the card's block says: the app, or "VectorOS" for anonymous senders.
    pub fn label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        let a = plain(&self.app);
        if a.is_empty() || a == "notify-send" || a == "vector-bar" {
            "VectorOS".into()
        } else {
            a
        }
    }
}

// ── config ──────────────────────────────────────────────────────────────

/// `[notifications]` in config.toml.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// serve notifications (false: leave it to mako)
    pub enable: bool,
    /// top-right, top-left, top-center, bottom-right, bottom-left, bottom-center
    pub position: String,
    pub width: i32,
    /// cards on screen at once; the rest wait behind a "+N more" line
    pub max_visible: usize,
    pub max_history: usize,
    /// ms, by urgency; 0 = until dismissed. An app's own timeout wins.
    pub timeout_low: i32,
    pub timeout: i32,
    pub timeout_critical: i32,
    /// critical notifications still show while silenced
    pub critical_in_dnd: bool,
    pub rules: Vec<Rule>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enable: true,
            position: "top-right".into(),
            width: 400,
            max_visible: 4,
            max_history: 150,
            timeout_low: 4000,
            timeout: 6500,
            timeout_critical: 0,
            critical_in_dnd: true,
            rules: Vec::new(),
        }
    }
}

/// `[[notifications.rules]]`: every rule whose matches all hold applies, in
/// order (built-in rules first, so yours win). Patterns are case-insensitive
/// substrings, `a|b` alternatives, `^` / `$` anchors.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub app: Option<String>,
    pub summary: Option<String>,
    pub body: Option<String>,
    pub category: Option<String>,
    pub desktop_entry: Option<String>,
    /// low / normal / critical
    pub urgency: Option<String>,
    /// no card (still in the history)
    pub hide: Option<bool>,
    /// false: never kept in the history
    pub history: Option<bool>,
    pub timeout: Option<i32>,
    pub set_urgency: Option<String>,
    /// shell command run on click (instead of the app's default action)
    pub on_click: Option<String>,
    pub show_in_dnd: Option<bool>,
    /// block label
    pub title: Option<String>,
    /// show it as an on-screen display (bottom of the screen) instead of a card
    pub osd: Option<bool>,
    /// drop it completely
    pub ignore: Option<bool>,
}

fn rule(f: impl FnOnce(&mut Rule)) -> Rule {
    let mut r = Rule::default();
    f(&mut r);
    r
}

/// What mako's default config did for Omarchy, as rules.
fn builtin_rules() -> Vec<Rule> {
    let s = |x: &str| Some(x.to_string());
    let launch = |summary: &str, cmd: &str| {
        rule(|r| {
            r.summary = Some(format!("^{summary}"));
            r.on_click = Some(cmd.to_string());
        })
    };
    vec![
        // the media module already shows the track
        rule(|r| {
            r.app = s("^spotify$");
            r.hide = Some(true);
            r.history = Some(false);
        }),
        // this machine stays on Omarchy 3 on purpose
        rule(|r| {
            r.summary = s("upgrade to omarchy quattro");
            r.ignore = Some(true);
        }),
        launch("Setup Wi-Fi", "omarchy-launch-wifi"),
        launch(
            "Update System",
            "omarchy-launch-floating-terminal-with-presentation omarchy-update",
        ),
        launch("Learn Keybindings", "omarchy-menu-keybindings"),
        launch(
            "Install Dictation with Voxtype",
            "omarchy-launch-floating-terminal-with-presentation omarchy-voxtype-install",
        ),
        // silencing toggles: the mode block says it already; a short OSD confirms
        rule(|r| {
            r.summary = s("silenced notifications$|enabled notifications$");
            r.osd = Some(true);
        }),
    ]
}

pub fn matches(pat: &str, text: &str) -> bool {
    let t = text.to_lowercase();
    pat.split('|').any(|alt| {
        let a = alt.trim().to_lowercase();
        let (start, a) = match a.strip_prefix('^') {
            Some(r) => (true, r.to_string()),
            None => (false, a),
        };
        let (end, a) = match a.strip_suffix('$') {
            Some(r) => (true, r.to_string()),
            None => (false, a),
        };
        match (start, end) {
            (true, true) => t == a,
            (true, false) => t.starts_with(&a),
            (false, true) => t.ends_with(&a),
            _ => t.contains(&a),
        }
    })
}

fn urgency_name(u: u8) -> &'static str {
    match u {
        0 => "low",
        2 => "critical",
        _ => "normal",
    }
}

fn urgency_num(s: &str) -> Option<u8> {
    match s.to_lowercase().as_str() {
        "low" => Some(0),
        "normal" => Some(1),
        "critical" => Some(2),
        _ => None,
    }
}

#[derive(Default)]
struct Effects {
    hide: bool,
    show_in_dnd: bool,
    osd: bool,
    ignore: bool,
    timeout: Option<i32>,
}

fn apply_rules(rules: &[Rule], n: &mut Note) -> Effects {
    let mut e = Effects::default();
    let summary = plain(&n.summary);
    let body = plain(&n.body);
    for r in builtin_rules().iter().chain(rules) {
        let hit = r.app.as_ref().is_none_or(|p| matches(p, &n.app))
            && r.summary.as_ref().is_none_or(|p| matches(p, &summary))
            && r.body.as_ref().is_none_or(|p| matches(p, &body))
            && r.category.as_ref().is_none_or(|p| matches(p, &n.category))
            && r.desktop_entry
                .as_ref()
                .is_none_or(|p| matches(p, &n.desktop_entry))
            && r.urgency
                .as_ref()
                .is_none_or(|p| matches(p, urgency_name(n.urgency)));
        if !hit {
            continue;
        }
        if let Some(v) = r.hide {
            e.hide = v;
        }
        if let Some(v) = r.history {
            n.no_history = !v;
        }
        if let Some(v) = r.timeout {
            e.timeout = Some(v);
        }
        if let Some(u) = r.set_urgency.as_deref().and_then(urgency_num) {
            n.urgency = u;
        }
        if let Some(c) = &r.on_click {
            n.on_click = Some(c.clone());
        }
        if let Some(v) = r.show_in_dnd {
            e.show_in_dnd = v;
        }
        if let Some(t) = &r.title {
            n.title = Some(t.clone());
        }
        if let Some(v) = r.osd {
            e.osd = v;
        }
        if let Some(v) = r.ignore {
            e.ignore = v;
        }
    }
    e
}

// ── text ────────────────────────────────────────────────────────────────

/// Plain text: tags dropped, entities decoded, whitespace collapsed.
pub fn plain(t: &str) -> String {
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
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_entity(s: &str) -> bool {
    let inner = &s[1..s.len() - 1];
    matches!(inner, "amp" | "lt" | "gt" | "quot" | "apos")
        || inner
            .strip_prefix("#x")
            .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()))
        || inner
            .strip_prefix('#')
            .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

fn allowed_tag(tag: &str) -> Option<String> {
    let t = tag.trim();
    let lower = t.to_lowercase();
    match lower.as_str() {
        "b" | "/b" | "i" | "/i" | "u" | "/u" | "/a" => Some(format!("<{lower}>")),
        "br" | "br/" | "br /" => Some("\n".into()),
        _ if lower.starts_with("a ") => {
            let i = lower.find("href=")?;
            let v = &t[i + 5..];
            let q = v.chars().next()?;
            let href = if q == '"' || q == '\'' {
                v[1..].split(q).next()?
            } else {
                v.split_whitespace().next()?
            };
            Some(format!("<a href=\"{}\">", util::esc(href)))
        }
        _ => None,
    }
}

/// Body markup made safe for a label: `<b> <i> <u> <a href>` kept, other tags
/// dropped, stray `& < >` escaped. Anything pango still rejects becomes plain text.
pub fn markup(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(['<', '&', '>']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest.as_bytes()[0] {
            b'&' => match rest
                .find(';')
                .filter(|e| *e <= 10 && is_entity(&rest[..=*e]))
            {
                Some(e) => {
                    out.push_str(&rest[..=e]);
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push_str("&amp;");
                    rest = &rest[1..];
                }
            },
            b'>' => {
                out.push_str("&gt;");
                rest = &rest[1..];
            }
            _ => match rest.find('>') {
                Some(e) => {
                    if let Some(t) = allowed_tag(&rest[1..e]) {
                        out.push_str(&t);
                    }
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push_str("&lt;");
                    rest = &rest[1..];
                }
            },
        }
    }
    out.push_str(rest);
    let out = out.trim().to_string();
    if pango_valid(&out) {
        out
    } else {
        util::esc(&plain(s))
    }
}

/// Pango's parser does not know GtkLabel's `<a href="...">`, so swap `<a>` for
/// `<span>` while checking that tags and entities balance.
fn pango_valid(s: &str) -> bool {
    let mut check = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("<a href=\"") {
        check.push_str(&rest[..i]);
        rest = &rest[i + 9..];
        let Some(end) = rest.find("\">") else {
            return false;
        };
        check.push_str("<span>");
        rest = &rest[end + 2..];
    }
    check.push_str(rest);
    let check = check.replace("</a>", "</span>");
    gtk::pango::parse_markup(&check, '\0').is_ok()
}

// ── the store ───────────────────────────────────────────────────────────

struct Live {
    note: Note,
    deadline: Option<Instant>,
    total: Duration,
}

#[derive(Default)]
struct St {
    /// newest first
    live: Vec<Live>,
    /// newest first
    history: Vec<Note>,
    modes: Vec<String>,
    next: u32,
    cfg: Config,
    paused: Option<Instant>,
}

pub struct Store {
    st: RefCell<St>,
    pub changed: Notify,
    conn: RefCell<Option<gtk::gio::DBusConnection>>,
    serving: Cell<bool>,
    started: Cell<bool>,
    ticking: Cell<bool>,
    save_pending: Cell<bool>,
}

thread_local! {
    static STORE: Rc<Store> = Rc::new(Store {
        st: RefCell::new(St { modes: vec!["default".into()], next: 1, ..Default::default() }),
        changed: Notify::default(),
        conn: RefCell::default(),
        serving: Cell::new(false),
        started: Cell::new(false),
        ticking: Cell::new(false),
        save_pending: Cell::new(false),
    });
}

pub fn get() -> Rc<Store> {
    STORE.with(|s| s.clone())
}

/// Built-in server switched on in the config (the bell then reads the store).
pub fn enabled() -> bool {
    get().st.borrow().cfg.enable
}

fn state_file() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| util::home().join(".local/state"));
    base.join("vector-bar/notifications.json")
}

fn image_dir() -> std::path::PathBuf {
    util::cache_dir().join("vector-bar/notifications")
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    history: Vec<Note>,
    silenced: bool,
    next: u32,
}

/// Called on every (re)load with the current config. The first call loads the
/// history and starts serving.
pub fn configure(cfg: &Config, app: &gtk::Application) {
    let s = get();
    s.st.borrow_mut().cfg = cfg.clone();
    if !cfg.enable {
        if !crate::running("org.freedesktop.Notifications")
            && std::path::Path::new("/usr/bin/mako").exists()
        {
            util::spawn(&["/usr/bin/mako"]);
        }
        s.changed.emit();
        return;
    }
    if s.started.get() {
        s.changed.emit();
        return;
    }
    s.started.set(true);
    if let Some(saved) =
        util::read(state_file()).and_then(|t| serde_json::from_str::<Saved>(&t).ok())
    {
        let mut st = s.st.borrow_mut();
        st.history = saved.history;
        st.next = saved
            .next
            .max(st.history.iter().map(|n| n.id).max().unwrap_or(0) + 1)
            .max(1);
        if saved.silenced {
            st.modes = vec!["default".into(), DND.into()];
        }
    }
    popup::init(app);
    server::start(&s);
}

impl Store {
    fn cfg(&self) -> Config {
        self.st.borrow().cfg.clone()
    }

    /// On screen: live and drawn, newest first.
    pub fn visible(&self) -> Vec<Note> {
        self.st
            .borrow()
            .live
            .iter()
            .filter(|l| !l.note.hidden)
            .map(|l| l.note.clone())
            .collect()
    }

    /// Every live notification (mako's ListNotifications), newest first.
    pub fn live(&self) -> Vec<Note> {
        self.st
            .borrow()
            .live
            .iter()
            .map(|l| l.note.clone())
            .collect()
    }

    pub fn history(&self) -> Vec<Note> {
        self.st.borrow().history.clone()
    }

    pub fn unseen(&self) -> usize {
        self.st.borrow().history.iter().filter(|n| n.unseen).count()
    }

    pub fn modes(&self) -> Vec<String> {
        self.st.borrow().modes.clone()
    }

    pub fn dnd(&self) -> bool {
        self.st.borrow().modes.iter().any(|m| m == DND)
    }

    pub fn set_modes(self: &Rc<Self>, mut modes: Vec<String>) {
        modes.dedup();
        if modes.is_empty() {
            modes.push("default".into());
        }
        let was = self.dnd();
        self.st.borrow_mut().modes = modes;
        let now = self.dnd();
        if was != now {
            // silenced: hide what is up; back on: show what is still live
            let crit_ok = self.cfg().critical_in_dnd;
            for l in self.st.borrow_mut().live.iter_mut() {
                l.note.hidden = now && !(crit_ok && l.note.urgency >= 2);
            }
        }
        server::modes_changed(self);
        self.save_soon();
        self.changed.emit();
    }

    pub fn set_dnd(self: &Rc<Self>, on: bool) {
        let mut m = self.modes();
        m.retain(|x| x != DND);
        if on {
            m.push(DND.into());
        }
        self.set_modes(m);
    }

    /// Fraction of its time left (1 = just arrived), None when it stays.
    pub fn remaining(&self, id: u32) -> Option<f64> {
        let st = self.st.borrow();
        let l = st.live.iter().find(|l| l.note.id == id)?;
        let d = l.deadline?;
        let now = st.paused.unwrap_or_else(Instant::now);
        let left = d.saturating_duration_since(now).as_secs_f64();
        Some((left / l.total.as_secs_f64().max(0.001)).clamp(0.0, 1.0))
    }

    pub fn paused(&self) -> bool {
        self.st.borrow().paused.is_some()
    }

    /// Hovering the cards stops the clocks.
    pub fn pause(self: &Rc<Self>, on: bool) {
        let mut st = self.st.borrow_mut();
        match (on, st.paused) {
            (true, None) => st.paused = Some(Instant::now()),
            (false, Some(since)) => {
                let d = since.elapsed();
                for l in st.live.iter_mut() {
                    if let Some(x) = l.deadline.as_mut() {
                        *x += d;
                    }
                }
                st.paused = None;
            }
            _ => {}
        }
    }

    fn timeout_for(&self, n: &Note, requested: i32, rule: Option<i32>) -> i32 {
        let c = self.cfg();
        if let Some(t) = rule {
            return t.max(0);
        }
        if n.urgency >= 2 {
            return c.timeout_critical.max(0);
        }
        if requested > 0 {
            return requested;
        }
        if requested == 0 {
            return 0;
        }
        if n.urgency == 0 {
            c.timeout_low
        } else {
            c.timeout
        }
        .max(0)
    }

    /// A Notify call. Returns the id the app gets back.
    pub fn notify(self: &Rc<Self>, mut n: Note, replaces: u32, requested_timeout: i32) -> u32 {
        let cfg = self.cfg();
        let fx = apply_rules(&cfg.rules, &mut n);
        let fx = lua_hook(&mut n, fx);
        if fx.ignore {
            return self.take_id(replaces);
        }
        if fx.osd {
            crate::osd::message(&n.label(), &plain(&n.summary), &plain(&n.body));
            return self.take_id(replaces);
        }
        n.timeout = self.timeout_for(&n, requested_timeout, fx.timeout);
        n.time = util::now();
        n.count = n.count.max(1);
        let dnd = self.dnd() && !fx.show_in_dnd && !(cfg.critical_in_dnd && n.urgency >= 2);
        n.hidden = fx.hide || dnd;
        let deadline =
            (n.timeout > 0).then(|| Instant::now() + Duration::from_millis(n.timeout as u64));
        let total = Duration::from_millis(n.timeout.max(1) as u64);

        let mut st = self.st.borrow_mut();
        // replace in place: by id, or by a synchronous tag
        let pos = if replaces != 0 {
            st.live.iter().position(|l| l.note.id == replaces)
        } else {
            None
        }
        .or_else(|| {
            (!n.tag.is_empty())
                .then(|| {
                    st.live
                        .iter()
                        .position(|l| l.note.tag == n.tag && l.note.app == n.app)
                })
                .flatten()
        });
        let id;
        if let Some(p) = pos {
            id = st.live[p].note.id;
            n.id = id;
            n.time = st.live[p].note.time;
            st.live[p] = Live {
                note: n,
                deadline,
                total,
            };
        } else if let Some(p) = st.live.iter().position(|l| {
            !l.note.hidden
                && l.note.app == n.app
                && l.note.summary == n.summary
                && l.note.body == n.body
                && n.value.is_none()
        }) {
            // the same thing again: fold it into the card that is up
            let mut l = st.live.remove(p);
            l.note.count += 1;
            l.note.time = util::now();
            l.deadline = deadline;
            l.total = total;
            id = l.note.id;
            st.live.insert(0, l);
        } else {
            id = if replaces != 0 { replaces } else { st.next };
            if replaces == 0 || replaces >= st.next {
                st.next = id + 1;
            }
            n.id = id;
            st.live.insert(
                0,
                Live {
                    note: n,
                    deadline,
                    total,
                },
            );
        }
        drop(st);
        self.tick();
        server::notifications_changed(self);
        self.changed.emit();
        id
    }

    fn take_id(&self, replaces: u32) -> u32 {
        if replaces != 0 {
            return replaces;
        }
        let mut st = self.st.borrow_mut();
        let id = st.next;
        st.next += 1;
        id
    }

    /// The expiry clock: runs only while something is waiting to time out.
    fn tick(self: &Rc<Self>) {
        if self.ticking.get() {
            return;
        }
        self.ticking.set(true);
        let w = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(150), move || {
            let Some(s) = w.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let (due, any) = {
                let st = s.st.borrow();
                if st.paused.is_some() {
                    (Vec::new(), true)
                } else {
                    let now = Instant::now();
                    let due: Vec<u32> = st
                        .live
                        .iter()
                        .filter(|l| l.deadline.is_some_and(|d| d <= now))
                        .map(|l| l.note.id)
                        .collect();
                    (due, st.live.iter().any(|l| l.deadline.is_some()))
                }
            };
            for id in due {
                s.close(id, 1);
            }
            if any {
                glib::ControlFlow::Continue
            } else {
                s.ticking.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// Takes a note off the screen. Reasons as in the spec: 1 expired,
    /// 2 dismissed by the user, 3 closed by the app.
    pub fn close(self: &Rc<Self>, id: u32, reason: u32) {
        self.close_with(id, reason, true);
    }

    fn close_with(self: &Rc<Self>, id: u32, reason: u32, history: bool) {
        let note = {
            let mut st = self.st.borrow_mut();
            let Some(p) = st.live.iter().position(|l| l.note.id == id) else {
                return;
            };
            st.live.remove(p).note
        };
        if history && !note.transient && !note.no_history {
            let mut n = note.clone();
            n.unseen = reason == 1 || n.hidden;
            if n.image.is_empty()
                && let Some(t) = &n.pixels
            {
                let _ = std::fs::create_dir_all(image_dir());
                let p = image_dir().join(format!("{}-{}.png", n.id, n.time as u64));
                if t.save_to_png(&p).is_ok() {
                    n.image = p.to_string_lossy().to_string();
                }
            }
            n.pixels = None;
            let max = self.cfg().max_history;
            let mut st = self.st.borrow_mut();
            st.history.retain(|h| h.id != n.id);
            st.history.insert(0, n);
            st.history.truncate(max);
        }
        server::closed(self, id, reason);
        server::notifications_changed(self);
        self.save_soon();
        self.changed.emit();
    }

    pub fn dismiss(self: &Rc<Self>, id: u32) {
        self.close(id, 2);
    }

    pub fn dismiss_all(self: &Rc<Self>) {
        let ids: Vec<u32> = self.st.borrow().live.iter().map(|l| l.note.id).collect();
        for id in ids {
            self.close(id, 2);
        }
    }

    /// mako's DismissNotifications: id 0 = the newest.
    pub fn dismiss_by(self: &Rc<Self>, id: u32, group: bool, all: bool, history: bool) {
        let target = {
            let st = self.st.borrow();
            st.live
                .iter()
                .find(|l| id == 0 || l.note.id == id)
                .map(|l| (l.note.id, l.note.app.clone()))
        };
        let Some((tid, app)) = target else { return };
        let ids: Vec<u32> = if all {
            self.st.borrow().live.iter().map(|l| l.note.id).collect()
        } else if group {
            self.st
                .borrow()
                .live
                .iter()
                .filter(|l| l.note.app == app)
                .map(|l| l.note.id)
                .collect()
        } else {
            vec![tid]
        };
        for i in ids {
            self.close_with(i, 2, history);
        }
    }

    /// An action button (or "default"). id 0 = the newest.
    pub fn invoke(self: &Rc<Self>, id: u32, key: &str) {
        let n = {
            let st = self.st.borrow();
            st.live
                .iter()
                .find(|l| id == 0 || l.note.id == id)
                .map(|l| l.note.clone())
        };
        let Some(n) = n else { return };
        if !n.actions.iter().any(|(k, _)| k == key) {
            return;
        }
        server::action(self, n.id, key);
        if !n.resident {
            self.close(n.id, 2);
        }
    }

    /// The click: accept it.
    pub fn activate(self: &Rc<Self>, id: u32) {
        let Some(n) = self.live().into_iter().find(|n| n.id == id) else {
            return;
        };
        if let Some(cmd) = &n.on_click {
            util::spawn_sh(cmd);
            self.close(id, 2);
        } else if n.actions.iter().any(|(k, _)| k == "default") {
            self.invoke(id, "default");
        } else {
            focus_app(&n);
            self.close(id, 2);
        }
    }

    /// mako's RestoreNotification: the newest history entry back on screen.
    pub fn restore(self: &Rc<Self>) {
        let n = {
            let mut st = self.st.borrow_mut();
            if st.history.is_empty() {
                return;
            }
            st.history.remove(0)
        };
        let mut n = n;
        n.unseen = false;
        n.hidden = false;
        if !n.image.is_empty() && n.pixels.is_none() {
            n.pixels = gdk::Texture::from_filename(&n.image).ok();
        }
        let t = if n.urgency >= 2 {
            0
        } else {
            self.cfg().timeout
        };
        n.timeout = t;
        let deadline = (t > 0).then(|| Instant::now() + Duration::from_millis(t as u64));
        self.st.borrow_mut().live.insert(
            0,
            Live {
                note: n,
                deadline,
                total: Duration::from_millis(t.max(1) as u64),
            },
        );
        self.tick();
        server::notifications_changed(self);
        self.save_soon();
        self.changed.emit();
    }

    /// The panel was looked at.
    pub fn mark_seen(self: &Rc<Self>) {
        let any = self.st.borrow().history.iter().any(|n| n.unseen);
        if any {
            for n in self.st.borrow_mut().history.iter_mut() {
                n.unseen = false;
            }
            self.save_soon();
            self.changed.emit();
        }
    }

    pub fn forget(self: &Rc<Self>, id: u32) {
        self.st.borrow_mut().history.retain(|n| n.id != id);
        self.save_soon();
        self.changed.emit();
    }

    pub fn clear_history(self: &Rc<Self>) {
        self.st.borrow_mut().history.clear();
        self.save_soon();
        self.changed.emit();
    }

    fn save_soon(self: &Rc<Self>) {
        if self.save_pending.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(1200), move || {
            if let Some(s) = w.upgrade() {
                s.save_pending.set(false);
                s.save();
            }
        });
    }

    fn save(&self) {
        let (data, keep) = {
            let st = self.st.borrow();
            let saved = Saved {
                history: st.history.clone(),
                silenced: st.modes.iter().any(|m| m == DND),
                next: st.next,
            };
            let keep: Vec<String> = st
                .history
                .iter()
                .map(|n| n.image.clone())
                .filter(|p| !p.is_empty())
                .collect();
            (serde_json::to_string(&saved).unwrap_or_default(), keep)
        };
        let p = state_file();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = util::write_atomic(&p, &data);
        // saved pictures nothing refers to any more
        if let Ok(rd) = std::fs::read_dir(image_dir()) {
            for e in rd.flatten() {
                let path = e.path().to_string_lossy().to_string();
                if !keep.contains(&path) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// A click on a notification without an action: bring its app forward.
fn focus_app(n: &Note) {
    let Some(clients) = crate::hypr::json("clients") else {
        return;
    };
    let Some(list) = clients.as_array() else {
        return;
    };
    let want: Vec<String> = [n.desktop_entry.as_str(), n.app.as_str()]
        .iter()
        .map(|s| s.to_lowercase().trim_end_matches(".desktop").to_string())
        .filter(|s| s.len() >= 3 && s != "notify-send")
        .collect();
    if want.is_empty() {
        return;
    }
    let hit = list.iter().find(|c| {
        let class = c["class"].as_str().unwrap_or("").to_lowercase();
        let init = c["initialClass"].as_str().unwrap_or("").to_lowercase();
        want.iter().any(|w| {
            class == *w
                || init == *w
                || class.ends_with(&format!(".{w}"))
                || class.contains(w.as_str())
        })
    });
    if let Some(addr) = hit.and_then(|c| c["address"].as_str()) {
        crate::hypr::dispatch(&format!("focuswindow address:{addr}"));
    }
}

/// `bar.on_notification(function(n) ... end)` in Lua: may change fields, or
/// return { hide = true, osd = true, ignore = true, timeout = ms }.
fn lua_hook(n: &mut Note, mut fx: Effects) -> Effects {
    let Some(out) = crate::lua::notification_hook(n) else {
        return fx;
    };
    let s = |k: &str| out.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let b = |k: &str| out.get(k).and_then(|v| v.as_bool());
    if let Some(v) = s("app") {
        n.app = v;
    }
    if let Some(v) = s("summary") {
        n.summary = v;
    }
    if let Some(v) = s("body") {
        n.body = v;
    }
    if let Some(v) = s("title") {
        n.title = Some(v);
    }
    if let Some(v) = s("on_click") {
        n.on_click = Some(v);
    }
    if let Some(u) = s("urgency").as_deref().and_then(urgency_num) {
        n.urgency = u;
    }
    if let Some(v) = b("hide") {
        fx.hide = v;
    }
    if let Some(v) = b("osd") {
        fx.osd = v;
    }
    if let Some(v) = b("ignore") {
        fx.ignore = v;
    }
    if let Some(v) = b("history") {
        n.no_history = !v;
    }
    if let Some(v) = out.get("timeout").and_then(|v| v.as_i64()) {
        fx.timeout = Some(v as i32);
    }
    fx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns() {
        assert!(matches("spotify", "Spotify"));
        assert!(matches("^spot", "Spotify"));
        assert!(!matches("^tify", "Spotify"));
        assert!(matches("a|fy$", "Spotify"));
        assert!(matches("^spotify$", "spotify"));
        assert!(!matches("^spotify$", "spotify2"));
    }

    #[test]
    fn markup_is_safe() {
        assert_eq!(markup("<b>hi</b> & <img src=x/>you"), "<b>hi</b> &amp; you");
        assert_eq!(markup("a < b"), "a &lt; b");
        assert_eq!(
            markup("<a href=\"https://x.y/?a=1&b=2\">link</a>"),
            "<a href=\"https://x.y/?a=1&amp;b=2\">link</a>"
        );
        assert_eq!(markup("<b>open"), "open");
        assert_eq!(markup("line<br>two"), "line\ntwo");
    }

    #[test]
    fn plain_text() {
        assert_eq!(plain("<b>a</b>  &amp;\n b"), "a & b");
    }
}
