//! Installed applications (desktop entries), ranked by match and by use.

use gio::prelude::*;

use crate::{calc, convert};
use crate::item::{Action, Icon, Item, Mode};
use crate::util::{self, Fuzzy, Usage};

const USAGE: &str = "launches.json";

pub struct App {
    pub id: String,
    pub name: String,
    pub comment: String,
    /// generic name, keywords, executable: matched but not shown
    pub extra: String,
    pub icon: Option<gio::Icon>,
    /// (action id, label)
    pub actions: Vec<(String, String)>,
}

/// An open window (Hyprland client).
pub struct Window {
    pub address: String,
    pub title: String,
    pub class: String,
    pub workspace: String,
}

pub fn windows() -> Vec<Window> {
    let Some(v) = util::hypr_json("clients") else { return vec![] };
    let mut out: Vec<(i64, Window)> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|c| c["mapped"].as_bool() != Some(false) && !c["title"].as_str().unwrap_or("").is_empty())
                .map(|c| {
                    (
                        c["focusHistoryID"].as_i64().unwrap_or(99),
                        Window {
                            address: c["address"].as_str().unwrap_or("").to_string(),
                            title: c["title"].as_str().unwrap_or("").to_string(),
                            class: c["class"].as_str().unwrap_or("").to_string(),
                            workspace: c["workspace"]["name"].as_str().unwrap_or("").to_string(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|w| w.0);
    out.into_iter().map(|w| w.1).collect()
}

/// System actions, matched by name only (never listed unasked).
const SYSTEM: &[(&str, &str, &str, &[&str])] = &[
    ("lock screen", "lock", "\u{f033e}", &["omarchy-system-lock"]),
    ("suspend", "sleep", "\u{f04b2}", &["systemctl", "suspend"]),
    ("log out", "logout exit session", "\u{f0343}", &["omarchy-system-logout"]),
    ("restart", "reboot", "\u{f0709}", &["omarchy-system-reboot"]),
    ("shut down", "power off poweroff shutdown", "\u{f0425}", &["omarchy-system-shutdown"]),
];

/// The app whose icon a window class should show.
fn app_for<'a>(apps: &'a [App], class: &str) -> Option<&'a App> {
    let c = class.to_lowercase();
    if c.is_empty() {
        return None;
    }
    apps.iter().find(|a| {
        let id = a.id.trim_end_matches(".desktop").to_lowercase();
        id == c || id.ends_with(&format!(".{c}")) || a.name.to_lowercase() == c
    })
}

pub fn load() -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for info in gio::AppInfo::all() {
        if !info.should_show() {
            continue;
        }
        let Some(id) = info.id().map(|s| s.to_string()) else { continue };
        if out.iter().any(|a| a.id == id) {
            continue;
        }
        let mut extra = Vec::new();
        let mut actions = Vec::new();
        if let Some(d) = info.downcast_ref::<gio_unix::DesktopAppInfo>() {
            if let Some(g) = d.generic_name() {
                extra.push(g.to_string());
            }
            extra.extend(d.keywords().iter().map(|k| k.to_string()));
            for a in d.list_actions() {
                actions.push((a.to_string(), d.action_name(&a).to_string()));
            }
        }
        if let Some(exe) = info.executable().file_name() {
            extra.push(exe.to_string_lossy().to_string());
        }
        out.push(App {
            name: info.display_name().to_string(),
            comment: info.description().map(|s| s.to_string()).unwrap_or_default(),
            extra: extra.join(" "),
            icon: info.icon(),
            id,
            actions,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn icon(a: &App) -> Icon {
    a.icon.clone().map(Icon::Gicon).unwrap_or(Icon::Glyph("\u{f08c6}"))
}

pub fn query(apps: &[App], wins: &[Window], q: &str) -> Vec<Item> {
    let usage = Usage::load(USAGE);
    let q = q.trim();
    if q.is_empty() {
        // most used first, then the rest by name
        let mut v: Vec<&App> = apps.iter().collect();
        v.sort_by(|a, b| {
            usage
                .bonus(&b.id)
                .cmp(&usage.bonus(&a.id))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        return v
            .into_iter()
            .map(|a| {
                Item::new(&a.name, icon(a), Action::Launch { id: a.id.clone(), action: None })
                    .sub(util::one_line(&a.comment, 90))
            })
            .collect();
    }
    let mut fz = Fuzzy::new(q);
    let ql = q.to_lowercase();
    let mut scored: Vec<(u32, Item)> = Vec::new();
    for a in apps {
        let name = fz.indices(&a.name);
        let extra = fz.score(&a.extra).map(|s| s * 3 / 4);
        let best = match (&name, extra) {
            (Some((s, _)), Some(e)) => (*s).max(e),
            (Some((s, _)), None) => *s,
            (None, Some(e)) => e,
            (None, None) => continue,
        };
        let nl = a.name.to_lowercase();
        // what you type is usually the start of the name or of a word in it;
        // letters scattered through a name only count when nothing fits better
        let quality = if nl.starts_with(&ql) {
            3
        } else if nl.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(&ql)) {
            2
        } else if nl.contains(&ql) || a.extra.to_lowercase().contains(&ql) {
            1
        } else {
            0
        };
        let boost = quality * 1000;
        let it = Item::new(&a.name, icon(a), Action::Launch { id: a.id.clone(), action: None })
            .sub(util::one_line(&a.comment, 90))
            .hl(name.map(|n| n.1).unwrap_or_default());
        scored.push((best + boost + usage.bonus(&a.id), it));
        for (aid, label) in &a.actions {
            let full = format!("{} › {}", a.name, label);
            if let Some((s, hl)) = fz.indices(&full)
                && s + 8 >= best
            {
                let it = Item::new(full, icon(a), Action::Launch { id: a.id.clone(), action: Some(aid.clone()) })
                    .sub("action")
                    .hl(hl);
                let ll = label.to_lowercase();
                let q2 = if ll.starts_with(&ql) { 2000 } else if ll.contains(&ql) { 1000 } else { 0 };
                scored.push((q2 + s / 2 + usage.bonus(&format!("{}:{aid}", a.id)), it));
            }
        }
    }
    scored.sort_by_key(|x| std::cmp::Reverse(x.0));
    // a few loose matches after the real ones (typos), not a page of noise
    let good = scored.iter().filter(|s| s.0 >= 1000).count();
    let loose = if good > 0 { 3 } else { 8 };
    let mut items: Vec<Item> = scored.into_iter().take(good + loose).map(|s| s.1).collect();
    // open windows, then system actions, below the apps
    let mut more: Vec<(u32, Item)> = Vec::new();
    for w in wins {
        let hay = format!("{} {}", w.class, w.title);
        let hl_ok = hay.to_lowercase().contains(&ql);
        let Some((s, hl)) = fz.indices(&w.title).or_else(|| fz.score(&hay).map(|s| (s, vec![]))) else { continue };
        if !hl_ok && s < 60 {
            continue;
        }
        let icon = app_for(apps, &w.class).map(icon).unwrap_or(Icon::Glyph("\u{f05b6}"));
        let it = Item::new(&w.title, icon, Action::Focus(w.address.clone()))
            .sub(format!("{} · workspace {}", w.class.to_lowercase(), w.workspace))
            .meta("window")
            .hl(hl);
        more.push((s + if hl_ok { 1000 } else { 0 }, it));
    }
    more.sort_by_key(|x| std::cmp::Reverse(x.0));
    let mut more: Vec<Item> = more.into_iter().take(6).map(|x| x.1).collect();
    if let Some(f) = more.first_mut() {
        f.section = Some("windows".into());
    }
    let mut sys: Vec<Item> = SYSTEM
        .iter()
        .filter(|(name, words, ..)| ql.len() >= 3 && (name.starts_with(&ql) || words.split(' ').any(|w| w.starts_with(&ql))))
        .map(|(name, _, glyph, cmd)| Item::new(*name, Icon::Glyph(glyph), Action::Run(cmd)).meta("system"))
        .collect();
    if let Some(f) = sys.first_mut() {
        f.section = Some("system".into());
    }
    if !items.is_empty() && (!more.is_empty() || !sys.is_empty()) {
        // the apps get their heading too, once there is more than apps
        items[0].section = Some("apps".into());
    }
    items.extend(more);
    items.extend(sys);
    let values = convert::value_items(q, false);
    if !values.is_empty() {
        let n = values.len().min(4);
        let mut v: Vec<Item> = values.into_iter().take(n).collect();
        v[0].section = Some("convert".into());
        if let Some(f) = items.first_mut()
            && f.section.is_none()
        {
            f.section = Some("apps".into());
        }
        items.splice(0..0, v);
    }
    if calc::looks_like_math(q)
        && let Some(v) = calc::eval(q)
    {
        let r = calc::format(v);
        items.insert(0, Item::new(format!("= {r}"), Icon::Glyph("\u{f00ec}"), Action::Copy(r)).sub(q.to_string()).meta("⏎ copy"));
    }
    items.push(
        Item::new(format!("search the web for “{q}”"), Icon::Glyph("\u{f0349}"), Action::Search(q.to_string()))
            .meta(Mode::Web.prefix()),
    );
    items
}

pub fn launch(id: &str, action: Option<&str>) {
    let key = match action {
        Some(a) => format!("{id}:{a}"),
        None => id.to_string(),
    };
    Usage::bump(USAGE, &key);
    if util::which("uwsm") {
        util::launch(&[&key]);
        return;
    }
    // no uwsm: GIO starts it
    if let Some(d) = gio_unix::DesktopAppInfo::new(id) {
        match action {
            Some(a) => d.launch_action(a, gio::AppLaunchContext::NONE),
            None => {
                let _ = d.launch(&[], gio::AppLaunchContext::NONE);
            }
        }
    }
}
