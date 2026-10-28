//! One directory entry, built from a GIO `FileInfo`.

use std::path::{Path, PathBuf};

use gio::prelude::*;

/// Attributes every listing asks GIO for. `fast-content-type` guesses from the name only, so a
/// 50 000 file folder is not sniffed file by file.
pub const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::size,\
standard::is-hidden,standard::is-backup,standard::is-symlink,standard::symlink-target,\
standard::fast-content-type,time::modified,access::can-execute";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Dir,
    /// Anything else: sockets, fifos, devices, mountables.
    Special,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// File name, lossily decoded for display and sorting.
    pub name: String,
    pub path: PathBuf,
    /// Kind of the symlink target for symlinks, `File` for broken links.
    pub kind: Kind,
    pub size: u64,
    /// Seconds since the Unix epoch, 0 when unknown.
    pub modified: i64,
    pub content_type: String,
    pub hidden: bool,
    pub symlink: bool,
    /// A symlink whose target does not exist.
    pub broken: bool,
    pub executable: bool,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// Lowercase extension without the dot, empty for none. Dotfiles like `.bashrc` have none.
    pub fn extension(&self) -> &str {
        extension_of(&self.name)
    }

    /// Builds an entry for a child of `dir` from an info queried with [`ATTRIBUTES`] while
    /// following symlinks.
    pub fn from_info(dir: &Path, info: &gio::FileInfo) -> Entry {
        let raw = info.name();
        let name = raw.to_string_lossy().into_owned();
        let symlink = info.is_symlink();
        let kind = match info.file_type() {
            gio::FileType::Directory => Kind::Dir,
            gio::FileType::Regular => Kind::File,
            // GIO reports the link itself when its target is missing
            gio::FileType::SymbolicLink => Kind::File,
            _ => Kind::Special,
        };
        let broken = symlink && info.file_type() == gio::FileType::SymbolicLink;
        let modified =
            i64::try_from(info.attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED)).unwrap_or(0);
        let executable = kind == Kind::File && info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_EXECUTE);
        Entry {
            path: dir.join(&raw),
            // `.hidden` files are applied by the model, which re-reads them when they change;
            // GIO caches them, so its is-hidden can lag behind an edit
            hidden: info.is_backup() || name.starts_with('.'),
            size: if kind == Kind::Dir {
                0
            } else {
                u64::try_from(info.size()).unwrap_or(0)
            },
            content_type: info
                .attribute_string(gio::FILE_ATTRIBUTE_STANDARD_FAST_CONTENT_TYPE)
                .map(|s| s.to_string())
                .unwrap_or_default(),
            name,
            kind,
            modified,
            symlink,
            broken,
            executable,
        }
    }

    /// Queries one path. Blocking: call off the main thread, or from tests.
    pub fn query(path: &Path) -> Result<Entry, glib::Error> {
        let file = gio::File::for_path(path);
        let info = file.query_info(
            ATTRIBUTES,
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )?;
        let dir = path.parent().unwrap_or(Path::new("/"));
        Ok(Entry::from_info(dir, &info))
    }
}

/// Lowercase-free extension of a file name ("" when none); the caller compares case-insensitively.
pub fn extension_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(i) => &name[i + 1..],
    }
}
