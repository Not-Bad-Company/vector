//! View settings that survive a restart: preview pane, list or grid, hidden files, sort.
//! Stored as a small key file in `$XDG_CONFIG_HOME/vector/settings.ini`.

use std::path::PathBuf;

use gtk::glib;
use vector_core::sort::{SortKey, SortSpec};

/// Defaults: preview off (`i` shows it), list view, hidden files hidden, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Settings {
    pub preview: bool,
    pub grid: bool,
    pub show_hidden: bool,
    pub sort: SortSpec,
    /// The terminal panel was open.
    pub terminal: bool,
}

const GROUP: &str = "view";

fn path() -> PathBuf {
    glib::user_config_dir()
        .join(vector_core::APP_NAME)
        .join("settings.ini")
}

impl Settings {
    /// Reads the settings; anything missing or unreadable keeps its default.
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let kf = glib::KeyFile::new();
        if kf.load_from_file(path(), glib::KeyFileFlags::NONE).is_err() {
            return s;
        }
        let b = |k: &str, d: bool| kf.boolean(GROUP, k).unwrap_or(d);
        s.preview = b("preview", s.preview);
        s.grid = b("grid", s.grid);
        s.show_hidden = b("hidden", s.show_hidden);
        s.terminal = b("terminal", s.terminal);
        s.sort.reverse = b("sort-reverse", s.sort.reverse);
        s.sort.key = match kf.string(GROUP, "sort").as_deref().unwrap_or("") {
            "size" => SortKey::Size,
            "mtime" => SortKey::Modified,
            "type" => SortKey::Type,
            _ => SortKey::Name,
        };
        s
    }

    /// Writes the settings; a failure only means they aren't remembered.
    pub fn save(&self) {
        let kf = glib::KeyFile::new();
        kf.set_boolean(GROUP, "preview", self.preview);
        kf.set_boolean(GROUP, "grid", self.grid);
        kf.set_boolean(GROUP, "hidden", self.show_hidden);
        kf.set_boolean(GROUP, "terminal", self.terminal);
        kf.set_string(GROUP, "sort", self.sort.key.label());
        kf.set_boolean(GROUP, "sort-reverse", self.sort.reverse);
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = kf.save_to_file(&p);
    }
}
