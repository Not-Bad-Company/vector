//! Clipboard history. `wl-paste --watch vector-space clip-store` runs
//! `record()` on every copy; the launcher reads and edits the same store:
//! `$XDG_DATA_HOME/vector-space/clipboard/history.json` plus one blob file per
//! image, file list or other binary entry. Every write holds `history.lock`.

use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::util;

/// Unpinned entries kept.
const KEEP: usize = 750;
/// Text up to this size lives in the index; longer text goes to a blob.
const INLINE: usize = 64 * 1024;
/// Anything bigger is not recorded.
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Text,
    Image,
    Files,
    Data,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Clip {
    pub id: u64,
    pub kind: Kind,
    pub mime: String,
    /// the text, its start when the text is in a blob, or the file URIs
    pub text: String,
    /// file name under `blobs/`
    pub blob: String,
    pub size: u64,
    pub hash: String,
    pub time: f64,
    pub pinned: bool,
    /// image size
    pub w: u32,
    pub h: u32,
}

impl Clip {
    pub fn blob_path(&self) -> Option<PathBuf> {
        (!self.blob.is_empty()).then(|| dir().join("blobs").join(&self.blob))
    }

    /// Local paths of a file list.
    pub fn files(&self) -> Vec<PathBuf> {
        self.text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("file://"))
            .map(|p| PathBuf::from(percent_decode(p)))
            .collect()
    }

    /// The bytes to put back on the clipboard, and their type.
    pub fn payload(&self) -> Option<(Vec<u8>, String)> {
        match self.kind {
            Kind::Text if self.blob.is_empty() => Some((self.text.clone().into_bytes(), "text/plain;charset=utf-8".into())),
            Kind::Files => Some((self.text.clone().into_bytes(), "text/uri-list".into())),
            _ => Some((std::fs::read(self.blob_path()?).ok()?, self.mime.clone())),
        }
    }

    /// What search looks at.
    pub fn haystack(&self) -> String {
        match self.kind {
            Kind::Text => self.text.clone(),
            Kind::Image => format!("image {} {}x{}", self.mime, self.w, self.h),
            Kind::Files => self
                .files()
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" "),
            Kind::Data => format!("data {}", self.mime),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub next: u64,
    pub items: Vec<Clip>,
}

pub fn dir() -> PathBuf {
    util::data_dir().join("clipboard")
}

pub fn index() -> PathBuf {
    dir().join("history.json")
}

pub fn load() -> History {
    util::read_json(&index()).unwrap_or_default()
}

/// Runs `f` on the history under the lock and saves it.
pub fn edit<R>(f: impl FnOnce(&mut History) -> R) -> R {
    let _ = std::fs::create_dir_all(dir().join("blobs"));
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir().join("history.lock"));
    if let Ok(l) = &lock {
        unsafe {
            libc::flock(l.as_raw_fd(), libc::LOCK_EX);
        }
    }
    let mut h = load();
    let r = f(&mut h);
    prune(&mut h);
    util::write_json(&index(), &h);
    r
}

fn prune(h: &mut History) {
    let mut unpinned = 0;
    let mut gone = Vec::new();
    h.items.retain(|c| {
        if c.pinned {
            return true;
        }
        unpinned += 1;
        if unpinned > KEEP {
            gone.push(c.blob.clone());
            false
        } else {
            true
        }
    });
    for b in gone {
        remove_blob(h, &b);
    }
}

/// Deletes a blob file nobody else uses.
fn remove_blob(h: &History, blob: &str) {
    if !blob.is_empty() && !blob.contains('/') && !h.items.iter().any(|c| c.blob == blob) {
        let _ = std::fs::remove_file(dir().join("blobs").join(blob));
    }
}

pub fn delete(id: u64) {
    edit(|h| {
        if let Some(i) = h.items.iter().position(|c| c.id == id) {
            let c = h.items.remove(i);
            remove_blob(h, &c.blob);
        }
    });
}

pub fn toggle_pin(id: u64) {
    edit(|h| {
        if let Some(c) = h.items.iter_mut().find(|c| c.id == id) {
            c.pinned = !c.pinned;
        }
    });
}

pub fn clear_unpinned() {
    edit(|h| {
        let gone: Vec<String> = h.items.iter().filter(|c| !c.pinned).map(|c| c.blob.clone()).collect();
        h.items.retain(|c| c.pinned);
        for b in gone {
            remove_blob(h, &b);
        }
    });
}

/// FNV-1a: stable across builds, good enough to spot a repeated copy.
fn hash(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}{:x}", data.len())
}

fn ext(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "image/avif" => "avif",
        "image/tiff" => "tiff",
        m if m.starts_with("text/") => "txt",
        _ => "bin",
    }
}

/// Adds one copy to the top, or moves an identical earlier one there.
pub fn add(kind: Kind, mime: &str, data: &[u8]) -> Option<u64> {
    if data.is_empty() || data.len() > MAX_BYTES {
        return None;
    }
    let text_like = matches!(kind, Kind::Text | Kind::Files);
    let text = if text_like {
        let t = String::from_utf8_lossy(data).to_string();
        if t.trim().is_empty() {
            return None;
        }
        t
    } else {
        String::new()
    };
    let hash = hash(data);
    edit(|h| {
        if let Some(i) = h.items.iter().position(|c| c.hash == hash) {
            let mut c = h.items.remove(i);
            c.time = util::now();
            let id = c.id;
            h.items.insert(0, c);
            return Some(id);
        }
        h.next += 1;
        let mut c = Clip {
            id: h.next,
            kind,
            mime: mime.to_string(),
            size: data.len() as u64,
            hash: hash.clone(),
            time: util::now(),
            ..Clip::default()
        };
        if kind == Kind::Text && text.len() > INLINE || !text_like {
            let name = format!("{hash}.{}", ext(mime));
            let path = dir().join("blobs").join(&name);
            if util::write_atomic(&path, data).is_err() {
                return None;
            }
            c.blob = name;
        }
        if text_like {
            c.text = if c.blob.is_empty() {
                text
            } else {
                text.chars().take(4000).collect()
            };
        }
        if kind == Kind::Image
            && let Ok(sz) = imagesize::blob_size(data)
        {
            c.w = sz.width as u32;
            c.h = sz.height as u32;
        }
        let id = c.id;
        h.items.insert(0, c);
        Some(id)
    })
}

fn wl_paste(args: &[&str]) -> Option<Vec<u8>> {
    let mut child = Command::new("wl-paste")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = Vec::new();
    let mut so = child.stdout.take()?;
    // a source app that never answers must not hang the recorder
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let r = so.by_ref().take(MAX_BYTES as u64 + 1).read_to_end(&mut buf);
        let _ = tx.send((r, buf));
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok((Ok(_), buf)) => out = buf,
        _ => {
            let _ = child.kill();
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    (ok && !out.is_empty()).then_some(out)
}

/// Picks what to keep from the current selection's offered types.
pub fn choose(types: &[String]) -> Option<(Kind, String)> {
    let has = |t: &str| types.iter().any(|x| x == t);
    // password managers mark secrets; never keep them
    if types.iter().any(|t| t == "x-kde-passwordManagerHint") {
        return None;
    }
    if has("text/uri-list") {
        return Some((Kind::Files, "text/uri-list".into()));
    }
    let text = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING", "STRING", "TEXT"]
        .into_iter()
        .find(|t| has(t));
    let image = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/avif", "image/bmp", "image/tiff", "image/svg+xml"]
        .into_iter()
        .find(|t| has(t))
        .map(str::to_string)
        .or_else(|| types.iter().find(|t| t.starts_with("image/")).cloned());
    match (text, image) {
        // an image with only a text/html wrapper (browser "copy image"): the image
        (None, Some(i)) => Some((Kind::Image, i)),
        (Some(t), _) => Some((Kind::Text, t.to_string())),
        (None, None) => types
            .iter()
            .find(|t| t.contains('/') && !t.starts_with("text/") && !t.starts_with("application/x-moz") && !t.starts_with("chromium/"))
            .map(|t| (Kind::Data, t.clone()))
            .or_else(|| types.iter().find(|t| t.starts_with("text/")).map(|t| (Kind::Text, t.clone()))),
    }
}

/// `vector-space clip-store`: run by `wl-paste --watch` with the selection on
/// stdin. Reads the offered types itself and keeps the best one.
pub fn record() {
    // drain what wl-paste hands us so it never sees a broken pipe
    let mut sink = Vec::new();
    let _ = std::io::stdin().take(MAX_BYTES as u64 + 1).read_to_end(&mut sink);
    match std::env::var("CLIPBOARD_STATE").as_deref() {
        Ok("data") | Err(_) => {}
        _ => return, // nil, clear, sensitive
    }
    let Some(list) = wl_paste(&["--list-types"]) else { return };
    let types: Vec<String> = String::from_utf8_lossy(&list).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let Some((kind, mime)) = choose(&types) else { return };
    let Some(data) = wl_paste(&["--no-newline", "--type", &mime]) else { return };
    let kind = if kind == Kind::Files && !String::from_utf8_lossy(&data).lines().any(|l| l.trim().starts_with("file://")) {
        Kind::Text
    } else {
        kind
    };
    let mime = if kind == Kind::Text { "text/plain;charset=utf-8".to_string() } else { mime };
    add(kind, &mime, &data);
}

/// `vector-space clip-add <mime> < file`: add bytes by hand (tests, scripts).
pub fn add_stdin(mime: &str) -> bool {
    let mut data = Vec::new();
    if std::io::stdin().read_to_end(&mut data).is_err() {
        return false;
    }
    let kind = if mime.starts_with("image/") {
        Kind::Image
    } else if mime == "text/uri-list" {
        Kind::Files
    } else if mime.starts_with("text/") {
        Kind::Text
    } else {
        Kind::Data
    };
    add(kind, mime, &data).is_some()
}

/// First run: bring over clipse's history (text and images, pins kept).
pub fn import_clipse() {
    if index().exists() {
        return;
    }
    let cdir = util::config_dir().join("clipse");
    let name = util::read_json::<serde_json::Value>(&cdir.join("config.json"))
        .and_then(|v| v["historyFile"].as_str().map(str::to_string))
        .unwrap_or_else(|| "clipboard_history.json".into());
    let Some(v) = util::read_json::<serde_json::Value>(&cdir.join(name)) else {
        // nothing to import: start empty, and do not look again
        edit(|_| {});
        return;
    };
    let Some(arr) = v["clipboardHistory"].as_array() else {
        edit(|_| {});
        return;
    };
    // oldest first, so the newest ends on top
    for e in arr.iter().rev() {
        let pinned = e["pinned"].as_bool().unwrap_or(false);
        let file = e["filePath"].as_str().filter(|p| !p.is_empty() && *p != "null");
        let id = match file {
            Some(p) => std::fs::read(p).ok().and_then(|d| {
                let mime = if p.ends_with(".jpg") || p.ends_with(".jpeg") { "image/jpeg" } else { "image/png" };
                add(Kind::Image, mime, &d)
            }),
            None => {
                let t = e["value"].as_str().unwrap_or("");
                let kind = if t.lines().all(|l| l.starts_with("file://")) && !t.is_empty() { Kind::Files } else { Kind::Text };
                add(kind, if kind == Kind::Files { "text/uri-list" } else { "text/plain;charset=utf-8" }, t.as_bytes())
            }
        };
        let time = e["recorded"]
            .as_str()
            .and_then(parse_clipse_time)
            .unwrap_or_else(util::now);
        if let Some(id) = id {
            edit(|h| {
                if let Some(c) = h.items.iter_mut().find(|c| c.id == id) {
                    c.pinned = pinned;
                    c.time = time;
                }
            });
        }
    }
}

/// "2026-10-28 14:03:11.123..." (local time) as unix seconds.
fn parse_clipse_time(s: &str) -> Option<f64> {
    let d = glib::DateTime::from_iso8601(&s.get(..19)?.replace(' ', "T"), Some(&glib::TimeZone::local())).ok()?;
    Some(d.to_unix() as f64)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Video/image thumbnail for a file entry, cached; `None` until made.
pub fn thumb(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let key = hash(format!("{}:{mtime}", path.display()).as_bytes());
    let out = util::cache_dir().join("thumbs").join(format!("{key}.png"));
    out.exists().then_some(out)
}

/// Makes the thumbnail for a video in the background; `done` runs on the GTK thread.
pub fn make_thumb(path: &Path, done: impl FnOnce(Option<PathBuf>) + 'static) {
    let Some(meta) = std::fs::metadata(path).ok() else { return done(None) };
    let Some(mtime) = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()) else {
        return done(None);
    };
    let key = hash(format!("{}:{}", path.display(), mtime.as_secs()).as_bytes());
    let out = util::cache_dir().join("thumbs").join(format!("{key}.png"));
    let (p, o) = (path.to_path_buf(), out.clone());
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(o.parent().unwrap_or(Path::new("/")));
        let ok = Command::new("ffmpegthumbnailer")
            .args(["-s", "512", "-q", "8", "-i"])
            .arg(&p)
            .arg("-o")
            .arg(&o)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let _ = tx.send_blocking(ok.then_some(o));
    });
    glib::spawn_future_local(async move {
        done(rx.recv().await.ok().flatten());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn picks_types() {
        assert_eq!(choose(&t(&["text/plain", "UTF8_STRING"])).map(|c| c.0), Some(Kind::Text));
        assert_eq!(choose(&t(&["image/png"])).map(|c| c.0), Some(Kind::Image));
        assert_eq!(choose(&t(&["text/html", "image/png"])).map(|c| c.0), Some(Kind::Image));
        assert_eq!(choose(&t(&["text/uri-list", "text/plain"])).map(|c| c.0), Some(Kind::Files));
        assert_eq!(choose(&t(&["text/plain", "x-kde-passwordManagerHint"])), None);
        assert_eq!(choose(&t(&["application/x-blender"])).map(|c| c.0), Some(Kind::Data));
    }

    #[test]
    fn decodes() {
        assert_eq!(percent_decode("/tmp/a%20b%C3%A9"), "/tmp/a bé");
        assert_eq!(percent_decode("/x%2"), "/x%2");
    }

    #[test]
    fn stable_hash() {
        assert_eq!(hash(b"abc"), hash(b"abc"));
        assert_ne!(hash(b"abc"), hash(b"abd"));
    }
}
