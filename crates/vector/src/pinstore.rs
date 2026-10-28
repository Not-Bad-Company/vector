//! The pins of this user, in `$XDG_STATE_HOME/vector/pins`, shared by every window.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use gtk::glib;
use vector_core::pins::Pins;

thread_local! {
    static PINS: RefCell<Option<Pins>> = const { RefCell::new(None) };
}

fn path() -> PathBuf {
    // $XDG_STATE_HOME, by the XDG base directory spec
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join(vector_core::APP_NAME)
        .join("pins")
}

/// Runs `f` on the pins, loading them on first use.
pub fn with<T>(f: impl FnOnce(&mut Pins) -> T) -> T {
    PINS.with(|p| {
        let mut p = p.borrow_mut();
        let pins = p.get_or_insert_with(|| {
            Pins::parse(&std::fs::read_to_string(path()).unwrap_or_default())
        });
        f(pins)
    })
}

/// Writes the pins (a small file, on every change).
pub fn save() {
    let text = with(|p| p.render());
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn in_folder(dir: &Path) -> std::collections::HashSet<String> {
    with(|p| p.in_folder(dir))
}

pub fn global() -> Vec<PathBuf> {
    with(|p| p.global().to_vec())
}
