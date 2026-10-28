//! Icons the user picked for folders and files, in `$XDG_STATE_HOME/vector/icons`
//! (`glyph<TAB>/path` lines). Shown in the sidebar and the file list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtk::glib;

thread_local! {
    static ICONS: RefCell<Option<HashMap<PathBuf, String>>> = const { RefCell::new(None) };
}

/// Nerd Font glyphs offered in the "change icon" menu (nf-md names in comments).
pub const CHOICES: &[&str] = &[
    "\u{F024B}", // folder
    "\u{F069D}", // folder_star
    "\u{F0250}", // folder_lock
    "\u{F0870}", // folder_network
    "\u{F0C82}", // folder_text
    "\u{F024D}", // folder_download
    "\u{F024F}", // folder_image
    "\u{F1359}", // folder_music
    "\u{F107F}", // folder_cog
    "\u{F02A2}", // git
    "\u{F0174}", // code_tags
    "\u{F018D}", // console
    "\u{F02DC}", // home
    "\u{F02CA}", // harddisk
    "\u{F015F}", // cloud
    "\u{F01BC}", // database
    "\u{F04CE}", // star
    "\u{F02D1}", // heart
    "\u{F0403}", // pin
    "\u{F00C0}", // bookmark
    "\u{F00D6}", // briefcase
    "\u{F0474}", // school
    "\u{F0297}", // gamepad_variant
    "\u{F0381}", // movie
    "\u{F075A}", // music
    "\u{F02E9}", // image
    "\u{F14F7}", // book_open_variant
    "\u{F0093}", // flask
    "\u{F03D8}", // palette
    "\u{F05B7}", // wrench
    "\u{F0463}", // rocket
    "\u{F06A9}", // robot
];

fn path() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join(vector_core::APP_NAME)
        .join("icons")
}

fn with<T>(f: impl FnOnce(&mut HashMap<PathBuf, String>) -> T) -> T {
    ICONS.with(|i| {
        let mut i = i.borrow_mut();
        let map = i.get_or_insert_with(|| {
            std::fs::read_to_string(path())
                .unwrap_or_default()
                .lines()
                .filter_map(|l| {
                    let (g, p) = l.split_once('\t')?;
                    p.starts_with('/')
                        .then(|| (PathBuf::from(p), g.to_string()))
                })
                .collect()
        });
        f(map)
    })
}

pub fn get(p: &Path) -> Option<String> {
    with(|m| m.get(p).cloned())
}

/// Sets (or with `None` clears) the icon of `p` and saves.
pub fn set(p: &Path, glyph: Option<&str>) {
    let text: String = with(|m| {
        match glyph {
            Some(g) => {
                m.insert(p.to_path_buf(), g.to_string());
            }
            None => {
                m.remove(p);
            }
        }
        m.iter()
            .filter_map(|(p, g)| {
                p.to_str()
                    .filter(|s| !s.contains(['\n', '\t']))
                    .map(|s| format!("{g}\t{s}\n"))
            })
            .collect()
    });
    let f = path();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = f.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &f);
    }
}
