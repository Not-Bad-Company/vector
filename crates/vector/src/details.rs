//! Collects the rows of the preview's details table. Blocking: runs on a worker thread.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use gtk::{gio, glib};
use vector_core::details::{self, Row, group_digits};
use vector_core::entry::Entry;

/// Text statistics read at most this much of a file.
const TEXT_LIMIT: u64 = 8 << 20;
/// Folder sizes stop counting after this long.
const SIZE_DEADLINE: Duration = Duration::from_millis(1500);

/// Rows that need a scan or a helper program (folder totals, image header, ffprobe, pdfinfo,
/// text statistics). Shown above the quick rows once ready.
pub fn slow(e: &Entry) -> Vec<Row> {
    let mut rows = slow_rows(e);
    if !rows.is_empty() {
        let ct = e.content_type.as_str();
        let name = if e.is_dir() {
            "folder"
        } else if let Some((top, _)) = ct
            .split_once('/')
            .filter(|(t, _)| matches!(*t, "image" | "video" | "audio"))
        {
            top
        } else if ct == "application/pdf" {
            "document"
        } else {
            "text"
        };
        rows.insert(0, section(name));
    }
    rows
}

fn slow_rows(e: &Entry) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let ct = e.content_type.as_str();
    let p = &e.path;
    if e.is_dir() {
        rows.extend(folder(p));
    } else if ct.starts_with("image/") {
        // the header alone, in-process; gdk-pixbuf (slow, a loader process) only as a fallback
        if let Some((w, h, fmt)) = vector_core::image::dimensions(p) {
            rows.extend(details::image(w as u64, h as u64, fmt));
        } else if let Some((fmt, w, h)) = gtk::gdk_pixbuf::Pixbuf::file_info(p) {
            rows.extend(details::image(
                w as u64,
                h as u64,
                fmt.name().as_deref().unwrap_or(""),
            ));
        }
    } else if ct.starts_with("video/") || ct.starts_with("audio/") {
        let out = run(
            Command::new("ffprobe").args([
                "-v",
                "error",
                "-show_entries",
                "format=duration,bit_rate,format_long_name:stream=codec_type,codec_name,width,height,r_frame_rate,channels,sample_rate,pix_fmt",
                "-of",
                "default=nw=1",
            ]),
            p,
        );
        rows.extend(details::ffprobe(&out));
    } else if ct == "application/pdf" {
        rows.extend(details::pdfinfo(&run(&mut Command::new("pdfinfo"), p)));
    } else if !e.broken && (ct.starts_with("text/") || gio::content_type_is_a(ct, "text/plain")) {
        use std::io::Read;
        if let Ok(f) = std::fs::File::open(p) {
            let mut bytes = Vec::new();
            if f.take(TEXT_LIMIT).read_to_end(&mut bytes).is_ok() {
                rows.extend(details::text(&bytes, e.size <= TEXT_LIMIT));
            }
        }
    }
    rows
}

/// Rows from one stat call: shown at once.
pub fn quick(e: &Entry) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let ct = e.content_type.as_str();
    let p = &e.path;
    if let Ok(m) = std::fs::symlink_metadata(p) {
        if !e.is_dir() {
            rows.push(("size".into(), format!("{} bytes", group_digits(e.size))));
        }
        let desc = gio::content_type_get_description(ct);
        rows.push((
            "type".into(),
            if ct.is_empty() {
                desc.to_string()
            } else {
                format!("{desc} · {ct}")
            },
        ));
        rows.push(("permissions".into(), details::permissions(m.mode())));
        rows.push((
            "owner".into(),
            // screenshots (VECTOR_DEMO) don't show the account they were taken on
            if std::env::var_os("VECTOR_DEMO").is_some() {
                "user:user".into()
            } else {
                format!("{}:{}", user_name(m.uid()), group_name(m.gid()))
            },
        ));
        if m.file_type().is_symlink()
            && let Ok(t) = std::fs::read_link(p)
        {
            rows.push(("link to".into(), t.display().to_string()));
        }
        rows.push((
            "inode".into(),
            format!(
                "{} · {} link{}",
                m.ino(),
                m.nlink(),
                if m.nlink() == 1 { "" } else { "s" }
            ),
        ));
        rows.push(("path".into(), p.display().to_string()));
        rows.push(section("time"));
        rows.push(("modified".into(), timestamp(m.mtime())));
        if let Ok(created) = m.created() {
            let secs = created
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            rows.push(("created".into(), timestamp(secs)));
        }
        rows.push(("accessed".into(), timestamp(m.atime())));
    } else {
        rows.push(("path".into(), p.display().to_string()));
    }
    rows.insert(0, section("file"));
    rows
}

/// A section title row: the key starts with `§`.
fn section(name: &str) -> Row {
    (format!("§{name}"), String::new())
}

fn folder(p: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    let Ok(rd) = std::fs::read_dir(p) else {
        return rows;
    };
    let (mut dirs, mut files, mut hidden) = (0u64, 0u64, 0u64);
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            hidden += 1;
        }
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    rows.push(("contains".into(), contains(dirs, files, hidden)));
    let (bytes, n, done) = size_until(p, Instant::now() + SIZE_DEADLINE);
    let more = if done {
        ""
    } else {
        "+ (still counting after 1.5 s; gs measures all)"
    };
    rows.push((
        "total size".into(),
        format!(
            "{} · {} bytes{more}",
            crate::fmt::size(bytes),
            group_digits(bytes)
        ),
    ));
    rows.push((
        "files below".into(),
        format!("{}{}", group_digits(n), if done { "" } else { "+" }),
    ));
    rows
}

/// Bytes and file count below `p` (not following symlinks), stopping at `deadline`.
fn size_until(p: &Path, deadline: Instant) -> (u64, u64, bool) {
    let mut stack = vec![p.to_path_buf()];
    let (mut bytes, mut n) = (0u64, 0u64);
    while let Some(dir) = stack.pop() {
        if Instant::now() > deadline {
            return (bytes, n, false);
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                bytes += m.len();
                n += 1;
            }
        }
    }
    (bytes, n, true)
}

/// Runs `cmd <path>` with a 5 s limit; empty output on any failure.
fn run(cmd: &mut Command, p: &Path) -> String {
    let child = cmd
        .arg(p)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match child {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

fn timestamp(secs: i64) -> String {
    glib::DateTime::from_unix_local(secs)
        .and_then(|d| d.format("%Y-%m-%d %H:%M:%S"))
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn lookup(file: &str, id: u32) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|l| {
        let mut parts = l.split(':');
        let name = parts.next()?;
        let _pw = parts.next()?;
        (parts.next()?.parse::<u32>().ok()? == id).then(|| name.to_string())
    })
}

pub fn user_name(uid: u32) -> String {
    lookup("/etc/passwd", uid).unwrap_or_else(|| uid.to_string())
}

fn group_name(gid: u32) -> String {
    lookup("/etc/group", gid).unwrap_or_else(|| gid.to_string())
}

/// `2 folders · 1 file`, leaving out what there is none of (short enough for the pane).
fn contains(dirs: u64, files: u64, hidden: u64) -> String {
    let part = |n: u64, one: &str, many: &str| {
        (n > 0).then(|| format!("{} {}", group_digits(n), if n == 1 { one } else { many }))
    };
    let parts: Vec<String> = [
        part(dirs, "folder", "folders"),
        part(files, "file", "files"),
        part(hidden, "hidden", "hidden"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn contains_leaves_out_zeroes() {
        assert_eq!(super::contains(0, 50_000, 0), "50 000 files");
        assert_eq!(super::contains(2, 1, 3), "2 folders · 1 file · 3 hidden");
        assert_eq!(super::contains(0, 0, 0), "nothing");
    }
}
