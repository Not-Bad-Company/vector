//! One directory entry, built from a GIO `FileInfo`.

use std::path::{Path, PathBuf};

use gio::prelude::*;

/// Attributes every listing asks GIO for. `fast-content-type` guesses from the name only, so a
/// 50 000 file folder is not sniffed file by file.
pub const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::size,\
standard::is-hidden,standard::is-backup,standard::is-symlink,standard::symlink-target,\
standard::fast-content-type,time::modified,time::changed,access::can-execute";

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
    /// When it arrived in its folder: the inode change time, which moving, copying or
    /// downloading it there sets (as does changing it). In the trash: when it was trashed.
    pub added: i64,
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
        let added =
            i64::try_from(info.attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED)).unwrap_or(0);
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
            added: if added > 0 { added } else { modified },
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

/// The type of a file whose name says nothing (no or unknown extension): sniffed from its
/// first bytes. Any other type is returned as it is. Blocking (reads 4 KB).
pub fn sniffed_type(path: &Path, name_type: &str) -> String {
    if !name_type.is_empty() && !gio::content_type_is_unknown(name_type) {
        return name_type.to_string();
    }
    use std::io::Read;
    // only regular files: opening a fifo would wait forever
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return name_type.to_string();
    }
    let mut head = Vec::with_capacity(4096);
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(4096).read_to_end(&mut head);
    }
    gio::content_type_guess(Some(path), head.as_slice())
        .0
        .to_string()
}

/// Lowercase-free extension of a file name ("" when none); the caller compares case-insensitively.
pub fn extension_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(0) | None => "",
        Some(i) => &name[i + 1..],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sniffs_files_without_a_telling_name() {
        let d = tempfile::tempdir().unwrap();
        let png = d.path().join("picture");
        std::fs::write(
            &png,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0",
        )
        .unwrap();
        assert_eq!(
            super::sniffed_type(&png, "application/octet-stream"),
            "image/png"
        );
        // a type the name already gives is kept
        assert_eq!(super::sniffed_type(&png, "text/plain"), "text/plain");
    }
}
