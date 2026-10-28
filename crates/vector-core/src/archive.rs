//! Zip and tar archives as read-only folders: list what is inside without extracting, pull
//! out single files for viewing, and extract all of it next to the archive.
//!
//! Safe by construction: entries are only ever written below the chosen folder (no `..`, no
//! absolute paths), links and device files inside archives are skipped, set-uid bits are
//! dropped, and nothing that exists is overwritten. Nothing inside an archive is run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::entry::{Entry, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    Tar,
    TarGz,
    TarXz,
    TarZst,
    TarBz2,
}

fn format(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let ends = |s: &[&str]| s.iter().any(|e| name.ends_with(e) && name.len() > e.len());
    // (.jar, .cbz and friends are zips too, but they belong to their own apps)
    Some(if ends(&[".zip"]) {
        Format::Zip
    } else if ends(&[".tar.gz", ".tgz"]) {
        Format::TarGz
    } else if ends(&[".tar.xz", ".txz"]) {
        Format::TarXz
    } else if ends(&[".tar.zst", ".tzst"]) {
        Format::TarZst
    } else if ends(&[".tar.bz2", ".tbz", ".tbz2"]) {
        Format::TarBz2
    } else if ends(&[".tar"]) {
        Format::Tar
    } else {
        return None;
    })
}

/// Whether the name is one of the archive kinds that can be browsed.
pub fn is_archive_name(path: &Path) -> bool {
    format(path).is_some()
}

/// For a path inside an archive (`/a/b.zip/docs/x.txt`): the archive file and the path within
/// it (`docs/x.txt`; empty for the archive itself). `None` for ordinary paths. Only names that
/// look like archives are checked on disk.
pub fn split(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut prefix = PathBuf::new();
    let mut parts = path.components();
    while let Some(c) = parts.next() {
        prefix.push(c);
        if is_archive_name(&prefix) && prefix.is_file() {
            return Some((prefix, parts.as_path().to_path_buf()));
        }
    }
    None
}

/// The archive's name without its archive extension: `photos.tar.gz` → `photos`.
pub fn stem(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_lowercase();
    for ext in [
        ".tar.gz", ".tar.xz", ".tar.zst", ".tar.bz2", ".tgz", ".txz", ".tzst", ".tbz2", ".tbz",
        ".tar", ".zip",
    ] {
        if lower.ends_with(ext) && lower.len() > ext.len() {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name
}

/// Longest path and deepest nesting taken from an archive; longer entries are skipped.
const MAX_PATH: usize = 4096;
const MAX_DEPTH: usize = 128;
/// More entries than this and the archive isn't browsed (or extracted).
const MAX_ENTRIES: usize = 1_000_000;

/// One entry of an archive.
#[derive(Debug, Clone)]
struct Item {
    /// Relative, cleaned path inside the archive.
    path: PathBuf,
    dir: bool,
    size: u64,
    modified: i64,
    mode: u32,
}

/// What an archive holds: its entries plus the folders they imply.
#[derive(Debug, Default)]
pub struct Index {
    items: BTreeMap<PathBuf, Item>,
}

/// A path from an archive made safe: relative, no `..`, no root. `None` when it can't be.
fn clean(raw: &Path) -> Option<PathBuf> {
    // absurdly long or deep names are an attack on memory (every parent folder is indexed)
    if raw.as_os_str().len() > MAX_PATH || raw.components().count() > MAX_DEPTH {
        return None;
    }
    let mut out = PathBuf::new();
    for c in raw.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

impl Index {
    fn add(&mut self, item: Item) -> io::Result<()> {
        if self.items.len() >= MAX_ENTRIES {
            return Err(io::Error::other("too many entries to show"));
        }
        // every parent is a folder, whether the archive lists it or not
        let mut parent = item.path.parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            self.items.entry(p.to_path_buf()).or_insert_with(|| Item {
                path: p.to_path_buf(),
                dir: true,
                size: 0,
                modified: item.modified,
                mode: 0o755,
            });
            parent = p.parent();
        }
        self.items.insert(item.path.clone(), item);
        Ok(())
    }

    fn read(path: &Path) -> io::Result<Index> {
        let mut index = Index::default();
        match format(path).ok_or_else(|| io::Error::other("not an archive"))? {
            Format::Zip => {
                let mut z = zip::ZipArchive::new(BufReader::new(File::open(path)?))
                    .map_err(io::Error::other)?;
                for i in 0..z.len() {
                    let f = z.by_index_raw(i).map_err(io::Error::other)?;
                    let Some(p) = f.enclosed_name().and_then(|p| clean(&p)) else {
                        continue;
                    };
                    if f.is_symlink() {
                        continue;
                    }
                    index.add(Item {
                        path: p,
                        dir: f.is_dir(),
                        size: f.size(),
                        modified: f.last_modified().map_or(0, zip_time),
                        mode: f.unix_mode().unwrap_or(0o644),
                    })?;
                }
            }
            _ => {
                let mut t = tar::Archive::new(tar_reader(path)?);
                for entry in t.entries()? {
                    let e = entry?;
                    let h = e.header();
                    let kind = h.entry_type();
                    if !(kind.is_file() || kind.is_dir()) {
                        continue;
                    }
                    let Some(p) = e.path().ok().and_then(|p| clean(&p)) else {
                        continue;
                    };
                    index.add(Item {
                        path: p,
                        dir: kind.is_dir(),
                        size: h.size().unwrap_or(0),
                        modified: h.mtime().unwrap_or(0) as i64,
                        mode: h.mode().unwrap_or(0o644),
                    })?;
                }
            }
        }
        Ok(index)
    }

    /// Names at the top of the archive.
    fn top(&self) -> BTreeSet<PathBuf> {
        self.items
            .keys()
            .filter_map(|p| p.components().next().map(|c| PathBuf::from(c.as_os_str())))
            .collect()
    }
}

fn tar_reader(path: &Path) -> io::Result<Box<dyn Read>> {
    let f = BufReader::new(File::open(path)?);
    Ok(match format(path) {
        Some(Format::TarGz) => Box::new(flate2::read::GzDecoder::new(f)),
        Some(Format::TarXz) => Box::new(xz2::read::XzDecoder::new(f)),
        Some(Format::TarZst) => Box::new(zstd::stream::read::Decoder::new(f)?),
        Some(Format::TarBz2) => Box::new(bzip2::read::BzDecoder::new(f)),
        _ => Box::new(f),
    })
}

/// A zip timestamp (local time, no zone) as Unix seconds.
fn zip_time(t: zip::DateTime) -> i64 {
    glib::DateTime::from_local(
        t.year() as i32,
        t.month() as i32,
        t.day() as i32,
        t.hour() as i32,
        t.minute() as i32,
        t.second() as f64,
    )
    .map_or(0, |d| d.to_unix())
}

type Cached = (PathBuf, i64, u64, Arc<Index>);

/// The last few archives read, so moving between their folders doesn't read them again.
static CACHE: Mutex<Vec<Cached>> = Mutex::new(Vec::new());

/// The index of `path`, read once per change of the file. Blocking.
pub fn index(path: &Path) -> io::Result<Arc<Index>> {
    let m = std::fs::metadata(path)?;
    let stamp = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    if let Ok(c) = CACHE.lock()
        && let Some((.., i)) = c
            .iter()
            .find(|(p, s, l, _)| p == path && *s == stamp && *l == m.len())
    {
        return Ok(i.clone());
    }
    let index = Arc::new(Index::read(path)?);
    if let Ok(mut c) = CACHE.lock() {
        c.retain(|(p, ..)| p != path);
        c.insert(0, (path.to_path_buf(), stamp, m.len(), index.clone()));
        c.truncate(4);
    }
    Ok(index)
}

/// What is directly inside folder `inner` of `archive`, as entries whose paths continue the
/// archive's path (`…/b.zip/docs/x.txt`). Blocking.
pub fn list(archive: &Path, inner: &Path) -> io::Result<Vec<Entry>> {
    let index = index(archive)?;
    if !inner.as_os_str().is_empty() && !index.items.get(inner).is_some_and(|i| i.dir) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no such folder in the archive",
        ));
    }
    let base = archive.join(inner);
    Ok(index
        .items
        .values()
        .filter(|i| i.path.parent() == Some(inner))
        .map(|i| {
            let name = i
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Entry {
                path: base.join(&name),
                kind: if i.dir { Kind::Dir } else { Kind::File },
                size: if i.dir { 0 } else { i.size },
                modified: i.modified,
                content_type: if i.dir {
                    "inode/directory".into()
                } else {
                    gio::content_type_guess(Some(name.as_str()), None)
                        .0
                        .to_string()
                },
                hidden: name.starts_with('.'),
                symlink: false,
                broken: false,
                executable: !i.dir && i.mode & 0o111 != 0,
                name,
            }
        })
        .collect())
}

/// Writes the entries of `archive` at or below `from` (all of it when empty) into `dest`,
/// with the leading `strip` taken off their paths: `from` `docs/a` with `strip` `docs` gives
/// `dest/a/…`; with `strip` `docs/a`, the folder's contents land in `dest` itself.
/// Existing files are never replaced. Blocking; `cancel` stops it between entries.
pub fn extract(
    archive: &Path,
    from: &Path,
    strip: &Path,
    dest: &Path,
    cancel: &std::sync::atomic::AtomicBool,
) -> io::Result<Extracted> {
    use std::sync::atomic::Ordering;
    let wanted = |p: &Path| from.as_os_str().is_empty() || p.starts_with(from);
    let target = |p: &Path| -> Option<PathBuf> {
        let rel = p.strip_prefix(strip).ok()?;
        if rel.as_os_str().is_empty() {
            // the stripped folder itself: dest
            return Some(dest.to_path_buf());
        }
        // belt and braces: the cleaned path can't leave dest, but check anyway
        let out = dest.join(rel);
        out.starts_with(dest).then_some(out)
    };
    let mut report = Extracted::default();
    // links, devices and unsafe names inside the archive
    let mut left_out = 0;
    // one file asked for: stop as soon as it is out
    let done = std::cell::Cell::new(false);
    let mut write = |p: &Path, dir: bool, mode: u32, data: &mut dyn Read| -> io::Result<()> {
        let Some(out) = target(p) else {
            report.skipped += 1;
            return Ok(());
        };
        if dir {
            return std::fs::create_dir_all(&out);
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        use std::os::unix::fs::OpenOptionsExt;
        // the archive's permissions (a private 0600 file stays private), without set-uid,
        // set-gid, sticky or group/other write; the umask applies on top
        let mut f = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode & 0o755)
            .open(&out)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                report.existing += 1;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if let Err(e) = io::copy(data, &mut f) {
            // no half-written files left behind
            drop(f);
            let _ = std::fs::remove_file(&out);
            return Err(e);
        }
        report.written += 1;
        if p == from {
            done.set(true);
        }
        Ok(())
    };
    match format(archive).ok_or_else(|| io::Error::other("not an archive"))? {
        Format::Zip => {
            let mut z = zip::ZipArchive::new(BufReader::new(File::open(archive)?))
                .map_err(io::Error::other)?;
            for i in 0..z.len() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut f = z.by_index(i).map_err(io::Error::other)?;
                let Some(p) = f.enclosed_name().and_then(|p| clean(&p)) else {
                    left_out += 1;
                    continue;
                };
                if !wanted(&p) {
                    continue;
                }
                if f.is_symlink() {
                    left_out += 1;
                    continue;
                }
                let (dir, mode) = (f.is_dir(), f.unix_mode().unwrap_or(0o644));
                write(&p, dir, mode, &mut f)?;
                if done.get() {
                    break;
                }
            }
        }
        _ => {
            let mut t = tar::Archive::new(tar_reader(archive)?);
            for entry in t.entries()? {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut e = entry?;
                let kind = e.header().entry_type();
                let Some(p) = e.path().ok().and_then(|p| clean(&p)) else {
                    left_out += 1;
                    continue;
                };
                if !wanted(&p) {
                    continue;
                }
                // links, devices and the like are not written (counted, so gX keeps the archive)
                if !(kind.is_file() || kind.is_dir()) {
                    left_out += 1;
                    continue;
                }
                let mode = e.header().mode().unwrap_or(0o644);
                write(&p, kind.is_dir(), mode, &mut e)?;
                if done.get() {
                    break;
                }
            }
        }
    }
    drop(write);
    report.skipped += left_out;
    Ok(report)
}

/// What an extraction did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Extracted {
    pub written: usize,
    /// Entries not written: links, devices, unsafe names.
    pub skipped: usize,
    /// Files left alone because something by that name was already there.
    pub existing: usize,
}

/// Where "extract here" puts things, next to the archive: an archive holding one folder is
/// extracted as that folder, anything else into a new folder named after the archive. Never
/// an existing name (`name (2)`, …). Returns (folder to extract into, part of the archive to
/// extract, which is also what to strip). Blocking.
pub fn plan(archive: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let index = index(archive)?;
    let parent = archive.parent().unwrap_or(Path::new("/"));
    let top = index.top();
    let single = (top.len() == 1)
        .then(|| top.into_iter().next())
        .flatten()
        .filter(|t| index.items.get(t).is_some_and(|i| i.dir));
    let unique = |name: &str| {
        (1..)
            .map(|n| {
                parent.join(if n == 1 {
                    name.to_string()
                } else {
                    format!("{name} ({n})")
                })
            })
            .find(|p| std::fs::symlink_metadata(p).is_err())
            .expect("a free name")
    };
    Ok(match single {
        Some(t) => (unique(&t.to_string_lossy()), t.clone()),
        None => (unique(&stem(archive)), PathBuf::new()),
    })
}

/// Extracts one file from the archive to a private cache folder (read-only copy) for viewing
/// or opening, and returns its path. Done once per archive version. Blocking.
pub fn extract_one(archive: &Path, inner: &Path) -> io::Result<PathBuf> {
    let inner = clean(inner).ok_or_else(|| io::Error::other("bad path in the archive"))?;
    let m = std::fs::metadata(archive)?;
    let key = format!(
        "{}\0{}\0{:?}",
        archive.display(),
        m.len(),
        m.modified().ok()
    );
    let hash = glib::compute_checksum_for_string(glib::ChecksumType::Md5, &key)
        .map(|s| s.to_string())
        .unwrap_or_default();
    let root = cache_dir().join(hash);
    let out = root.join(&inner);
    if out.is_file() {
        return Ok(out);
    }
    // extracted into a private scratch folder, then moved into place whole: a second caller
    // never sees a half-written copy, and a failed one leaves nothing behind
    static SCRATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = root.join(format!(".part-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let from_parent = inner.parent().unwrap_or(Path::new("")).to_path_buf();
    let result = extract(
        archive,
        &inner,
        &from_parent,
        &scratch,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .and_then(|_| {
        let got = scratch.join(inner.file_name().unwrap_or_default());
        if !got.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "not in the archive",
            ));
        }
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&got, std::fs::Permissions::from_mode(0o444));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&got, &out)?;
        Ok(out)
    });
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Where single files from archives are copied for viewing (private, in RAM on most systems):
/// one folder per running process, so one window never deletes what another has open.
fn cache_dir() -> PathBuf {
    glib::user_runtime_dir()
        .join("vector/archives")
        .join(std::process::id().to_string())
}

/// Removes the viewing copies of processes that have exited. Blocking.
pub fn clear_cache() {
    let base = glib::user_runtime_dir().join("vector/archives");
    let Ok(rd) = std::fs::read_dir(&base) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let alive = name
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
            .is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists());
        if !alive {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            if name.ends_with('/') {
                z.add_directory(*name, opts).unwrap();
            } else {
                z.start_file(*name, opts).unwrap();
                z.write_all(data).unwrap();
            }
        }
        z.finish().unwrap();
    }

    fn make_tar_gz(path: &Path, files: &[(&str, &[u8])]) {
        let gz = flate2::write::GzEncoder::new(File::create(path).unwrap(), Default::default());
        let mut t = tar::Builder::new(gz);
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            // append_data refuses `..`; write the raw name to test that hostile entries are
            // skipped on our side
            let mut h2 = h.clone();
            {
                let gnu = h2.as_gnu_mut().unwrap();
                gnu.name[..name.len()].copy_from_slice(name.as_bytes());
            }
            h2.set_cksum();
            t.append(&h2, *data).unwrap();
        }
        t.into_inner().unwrap().finish().unwrap();
    }

    fn names(v: &[Entry]) -> Vec<String> {
        let mut n: Vec<_> = v.iter().map(|e| e.name.clone()).collect();
        n.sort();
        n
    }

    #[test]
    fn kinds_and_stems() {
        assert!(is_archive_name(Path::new("a.ZIP")));
        assert!(is_archive_name(Path::new("a.tar.zst")));
        assert!(!is_archive_name(Path::new("a.gz")));
        assert!(!is_archive_name(Path::new(".zip")));
        assert_eq!(stem(Path::new("/x/photos.tar.gz")), "photos");
        assert_eq!(stem(Path::new("Report.ZIP")), "Report");
    }

    #[test]
    fn lists_zip_folders_without_extracting() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("pack.zip");
        make_zip(
            &a,
            &[
                ("readme.txt", b"hi"),
                ("docs/a.md", b"# a"),
                ("docs/deep/b.md", b"b"),
            ],
        );
        let top = list(&a, Path::new("")).unwrap();
        assert_eq!(names(&top), ["docs", "readme.txt"]);
        let docs = top.iter().find(|e| e.name == "docs").unwrap();
        assert!(docs.is_dir());
        assert_eq!(docs.path, a.join("docs"));
        assert_eq!(
            names(&list(&a, Path::new("docs")).unwrap()),
            ["a.md", "deep"]
        );
        assert!(list(&a, Path::new("nope")).is_err());
        // nothing was written next to the archive
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
        assert_eq!(
            split(&a.join("docs/a.md")),
            Some((a.clone(), PathBuf::from("docs/a.md")))
        );
        assert_eq!(split(&a), Some((a.clone(), PathBuf::new())));
        assert_eq!(split(d.path()), None);
    }

    #[test]
    fn hostile_tar_entries_are_skipped() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("evil.tar.gz");
        make_tar_gz(
            &a,
            &[
                ("ok.txt", b"fine"),
                ("../escape.txt", b"bad"),
                ("/abs.txt", b"bad"),
            ],
        );
        assert_eq!(names(&list(&a, Path::new("")).unwrap()), ["ok.txt"]);
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let r = extract(&a, Path::new(""), Path::new(""), &out, &Default::default()).unwrap();
        assert_eq!((r.written, r.skipped), (1, 2));
        assert!(!d.path().join("escape.txt").exists());
        assert!(!Path::new("/abs.txt").exists());
        assert_eq!(std::fs::read(out.join("ok.txt")).unwrap(), b"fine");
    }

    #[test]
    fn extract_plans_and_never_overwrites() {
        let d = tempfile::tempdir().unwrap();
        // one folder at the top: extracted as that folder
        let one = d.path().join("one.zip");
        make_zip(&one, &[("proj/", b""), ("proj/main.rs", b"fn main(){}")]);
        let (dest, from) = plan(&one).unwrap();
        assert_eq!(
            (dest.clone(), from.clone()),
            (d.path().join("proj"), PathBuf::from("proj"))
        );
        std::fs::create_dir(&dest).unwrap();
        extract(&one, &from, &from, &dest, &Default::default()).unwrap();
        assert!(d.path().join("proj/main.rs").is_file());
        assert!(!d.path().join("proj/proj").exists());
        // again: proj exists, so "proj (2)"
        assert_eq!(plan(&one).unwrap().0, d.path().join("proj (2)"));
        // loose files: a folder named after the archive
        let loose = d.path().join("loose.tar.gz");
        make_tar_gz(&loose, &[("a.txt", b"1"), ("b.txt", b"2")]);
        assert_eq!(
            plan(&loose).unwrap(),
            (d.path().join("loose"), PathBuf::new())
        );
        // an existing file is left alone
        let out = d.path().join("loose");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("a.txt"), b"mine").unwrap();
        let none = Path::new("");
        let r = extract(&loose, none, none, &out, &Default::default()).unwrap();
        assert_eq!((r.written, r.existing), (1, 1));
        assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"mine");
    }

    #[test]
    fn links_are_counted_and_modes_kept() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("keys.tar.gz");
        let gz = flate2::write::GzEncoder::new(File::create(&a).unwrap(), Default::default());
        let mut t = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        h.set_size(3);
        h.set_mode(0o600);
        h.set_cksum();
        t.append_data(&mut h, "top/secret.key", &b"key"[..])
            .unwrap();
        let mut l = tar::Header::new_gnu();
        l.set_entry_type(tar::EntryType::Symlink);
        l.set_size(0);
        t.append_link(&mut l, "top/link", "/etc/passwd").unwrap();
        t.into_inner().unwrap().finish().unwrap();
        let out = d.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let r = extract(&a, Path::new(""), Path::new(""), &out, &Default::default()).unwrap();
        assert_eq!((r.written, r.skipped), (1, 1));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(out.join("top/secret.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "a private file stays private");
        assert!(std::fs::symlink_metadata(out.join("top/link")).is_err());
    }

    #[test]
    fn absurd_names_are_skipped() {
        let deep: PathBuf = (0..200).map(|_| "d").collect();
        assert_eq!(clean(&deep), None);
        assert_eq!(clean(Path::new("a/./b")), Some(PathBuf::from("a/b")));
    }

    #[test]
    fn one_file_for_viewing() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("pack.zip");
        make_zip(&a, &[("docs/a.md", b"# a")]);
        let p = extract_one(&a, Path::new("docs/a.md")).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"# a");
        assert!(p.starts_with(glib::user_runtime_dir()));
        assert!(extract_one(&a, Path::new("docs/none")).is_err());
    }
}
