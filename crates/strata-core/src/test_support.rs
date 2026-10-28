//! Helpers shared by the unit tests.

use std::path::PathBuf;
use std::sync::Once;

use crate::entry::{Entry, Kind};

pub fn entry(name: &str, kind: Kind, size: u64, modified: i64) -> Entry {
    Entry {
        name: name.into(),
        path: PathBuf::from("/x").join(name),
        kind,
        size,
        modified,
        content_type: String::new(),
        hidden: name.starts_with('.'),
        symlink: false,
        broken: false,
        executable: false,
    }
}

/// Points HOME and XDG_DATA_HOME at a private directory before GLib reads them, so trash
/// tests use their own trash and never the user's. Call first in every test that uses GIO.
pub fn gio_env() -> PathBuf {
    static ONCE: Once = Once::new();
    let home = std::env::temp_dir().join(format!("strata-test-home-{}", std::process::id()));
    ONCE.call_once(|| {
        let data = home.join(".local/share");
        let _ = std::fs::create_dir_all(&data);
        // SAFETY: runs once, before any test thread has called into GLib (every GIO test calls
        // this first and Once blocks the others until it is done).
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("XDG_DATA_HOME", &data);
        }
    });
    home
}

/// A scratch directory on the same file system as the test home, so it can be trashed.
pub fn scratch() -> tempfile::TempDir {
    let home = gio_env();
    tempfile::Builder::new()
        .prefix("t")
        .tempdir_in(home)
        .expect("scratch dir")
}
