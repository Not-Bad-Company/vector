//! What the viewer can show, and the list of files it steps through: the folder of the opened
//! file, in one of several orders.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use vector_core::sort::natural_cmp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    /// Videos and animated images (GIF): played by mpv.
    Video,
    Pdf,
    /// Word processor, spreadsheet and presentation files: shown as a PDF made by LibreOffice.
    Office,
}

const IMAGE: &[&str] = &[
    "avif", "bmp", "heic", "heif", "ico", "j2k", "jp2", "jpe", "jpeg", "jpg", "jxl", "pbm", "pgm",
    "png", "pnm", "ppm", "qoi", "svg", "svgz", "tga", "tif", "tiff", "webp", "xpm",
];
// no plain "ts": in a source folder those are TypeScript
const VIDEO: &[&str] = &[
    "3g2", "3gp", "avi", "flv", "gif", "m2ts", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "mts",
    "ogv", "webm", "wmv",
];
const OFFICE: &[&str] = &[
    "doc", "docx", "odt", "ott", "rtf", "xls", "xlsx", "ods", "ots", "ppt", "pptx", "odp", "otp",
    "odg",
];

pub fn ext(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn kind(p: &Path) -> Option<Kind> {
    let e = ext(p);
    let e = e.as_str();
    if IMAGE.contains(&e) {
        Some(Kind::Image)
    } else if VIDEO.contains(&e) {
        Some(Kind::Video)
    } else if e == "pdf" {
        Some(Kind::Pdf)
    } else if OFFICE.contains(&e) {
        Some(Kind::Office)
    } else {
        None
    }
}

/// The kind of a file without an extension, from its contents (its first 4 KB). Blocking.
fn sniff(path: &Path) -> Option<Kind> {
    if !ext(path).is_empty() {
        return None;
    }
    let ct = vector_core::entry::sniffed_type(path, "");
    match ct.as_str() {
        "image/gif" => Some(Kind::Video),
        "application/pdf" => Some(Kind::Pdf),
        t if t.starts_with("image/") => Some(Kind::Image),
        t if t.starts_with("video/") => Some(Kind::Video),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Name,
    Newest,
    Oldest,
    Largest,
    Smallest,
    Random,
    Type,
}

impl Order {
    pub const ALL: [Order; 7] = [
        Order::Name,
        Order::Newest,
        Order::Oldest,
        Order::Largest,
        Order::Smallest,
        Order::Random,
        Order::Type,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Order::Name => "name",
            Order::Newest => "newest",
            Order::Oldest => "oldest",
            Order::Largest => "largest",
            Order::Smallest => "smallest",
            Order::Random => "random",
            Order::Type => "type",
        }
    }

    pub fn parse(s: &str) -> Option<Order> {
        Order::ALL.into_iter().find(|o| o.label() == s.trim())
    }

    pub fn next(self) -> Order {
        let i = Order::ALL.iter().position(|o| *o == self).unwrap_or(0);
        Order::ALL[(i + 1) % Order::ALL.len()]
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub modified: i64,
}

impl Item {
    pub fn new(path: PathBuf) -> Option<Item> {
        let kind = kind(&path).or_else(|| sniff(&path))?;
        let m = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        Some(Item {
            path,
            kind,
            size: m.len(),
            modified,
        })
    }

    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Everything the viewer can show in `dir`, hidden files left out. Blocking.
pub fn scan(dir: &Path) -> Vec<Item> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Item::new(e.path()))
        .collect()
}

fn rank(k: Kind) -> u8 {
    match k {
        Kind::Image => 0,
        Kind::Video => 1,
        Kind::Pdf => 2,
        Kind::Office => 3,
    }
}

/// Sorts in place. `seed` drives the random order (a fresh one each time it is chosen).
pub fn sort(items: &mut [Item], order: Order, seed: u64) {
    let by_name = |a: &Item, b: &Item| natural_cmp(&a.name(), &b.name());
    match order {
        Order::Name => items.sort_by(by_name),
        Order::Newest => {
            items.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| by_name(a, b)))
        }
        Order::Oldest => {
            items.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| by_name(a, b)))
        }
        Order::Largest => items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| by_name(a, b))),
        Order::Smallest => items.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| by_name(a, b))),
        Order::Type => {
            items.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then_with(|| by_name(a, b)))
        }
        Order::Random => {
            // xorshift; Fisher-Yates
            let mut s = seed | 1;
            for i in (1..items.len()).rev() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                items.swap(i, (s % (i as u64 + 1)) as usize);
            }
        }
    }
}

/// Index of `path` in `items`, comparing by value.
pub fn position(items: &[Item], path: &Path) -> Option<usize> {
    items.iter().position(|i| i.path == path)
}

/// `n` steps from `i` in a list of `len`, wrapping around.
pub fn step(i: usize, n: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (i as isize + n).rem_euclid(len as isize) as usize
}

/// Where the shown item `index` ends up after item `removed` left a list now `len` long, and
/// whether it was the one removed (so the next one must be loaded).
pub fn after_remove(index: usize, removed: usize, len: usize) -> (usize, bool) {
    match removed.cmp(&index) {
        std::cmp::Ordering::Less => (index - 1, false),
        std::cmp::Ordering::Equal => (index.min(len.saturating_sub(1)), true),
        std::cmp::Ordering::Greater => (index, false),
    }
}

/// Human size: `3.1 MB`.
pub fn size(b: u64) -> String {
    let f = b as f64;
    match b {
        0..1024 => format!("{b} B"),
        1024..1_048_576 => format!("{:.0} KB", f / 1024.0),
        1_048_576..1_073_741_824 => format!("{:.1} MB", f / 1_048_576.0),
        _ => format!("{:.1} GB", f / 1_073_741_824.0),
    }
}

/// `0:12`, `1:02:03`.
pub fn time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
pub fn cmp_names(a: &Item, b: &Item) -> std::cmp::Ordering {
    natural_cmp(&a.name(), &b.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, kind: Kind, size: u64, modified: i64) -> Item {
        Item {
            path: PathBuf::from(format!("/x/{name}")),
            kind,
            size,
            modified,
        }
    }

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind(Path::new("a.JPG")), Some(Kind::Image));
        assert_eq!(kind(Path::new("a.gif")), Some(Kind::Video));
        assert_eq!(kind(Path::new("a.mkv")), Some(Kind::Video));
        assert_eq!(kind(Path::new("a.pdf")), Some(Kind::Pdf));
        assert_eq!(kind(Path::new("a.docx")), Some(Kind::Office));
        assert_eq!(kind(Path::new("a.pptx")), Some(Kind::Office));
        assert_eq!(kind(Path::new("a.txt")), None);
        assert_eq!(kind(Path::new("noext")), None);
    }

    #[test]
    fn orders() {
        let mut v = vec![
            item("b10.png", Kind::Image, 5, 3),
            item("b2.mp4", Kind::Video, 50, 1),
            item("a.pdf", Kind::Pdf, 1, 2),
        ];
        let names = |v: &[Item]| v.iter().map(Item::name).collect::<Vec<_>>();
        sort(&mut v, Order::Name, 0);
        assert_eq!(names(&v), ["a.pdf", "b2.mp4", "b10.png"]);
        sort(&mut v, Order::Newest, 0);
        assert_eq!(names(&v), ["b10.png", "a.pdf", "b2.mp4"]);
        sort(&mut v, Order::Largest, 0);
        assert_eq!(names(&v), ["b2.mp4", "b10.png", "a.pdf"]);
        sort(&mut v, Order::Type, 0);
        assert_eq!(names(&v), ["b10.png", "b2.mp4", "a.pdf"]);
        sort(&mut v, Order::Random, 42);
        assert_eq!(v.len(), 3);
        v.sort_by(cmp_names);
        assert_eq!(names(&v), ["a.pdf", "b2.mp4", "b10.png"]);
    }

    #[test]
    fn order_cycle_and_parse() {
        assert_eq!(Order::Type.next(), Order::Name);
        assert_eq!(Order::parse("newest\n"), Some(Order::Newest));
        assert_eq!(Order::parse("bogus"), None);
    }

    #[test]
    fn files_without_extension_by_content() {
        let d = std::env::temp_dir().join(format!("vector-view-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let png = d.join("picture");
        std::fs::write(
            &png,
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0",
        )
        .unwrap();
        std::fs::write(d.join("notes"), b"just text").unwrap();
        assert_eq!(Item::new(png).map(|i| i.kind), Some(Kind::Image));
        assert!(Item::new(d.join("notes")).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn stepping_wraps() {
        assert_eq!(step(0, -1, 3), 2);
        assert_eq!(step(2, 1, 3), 0);
        assert_eq!(step(0, 1, 0), 0);
    }

    #[test]
    fn removal_keeps_the_shown_item() {
        // removed before the shown one: it moves up, no reload
        assert_eq!(after_remove(3, 1, 4), (2, false));
        // removed after: nothing changes
        assert_eq!(after_remove(1, 3, 4), (1, false));
        // the shown one: the next takes its place, or the last when it was last
        assert_eq!(after_remove(2, 2, 4), (2, true));
        assert_eq!(after_remove(4, 4, 4), (3, true));
    }

    #[test]
    fn formats() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3_250_000), "3.1 MB");
        assert_eq!(time(72.9), "1:12");
        assert_eq!(time(3723.0), "1:02:03");
    }

    #[test]
    fn scans_supported_files_only() {
        let t = std::env::temp_dir().join(format!("vector-view-scan-{}", std::process::id()));
        std::fs::create_dir_all(&t).unwrap();
        for n in ["a.png", "b.txt", ".c.png", "d.pdf"] {
            std::fs::write(t.join(n), b"x").unwrap();
        }
        std::fs::create_dir_all(t.join("e.png")).unwrap();
        let mut v = scan(&t);
        sort(&mut v, Order::Name, 0);
        assert_eq!(
            v.iter().map(Item::name).collect::<Vec<_>>(),
            ["a.png", "d.pdf"]
        );
        std::fs::remove_dir_all(&t).unwrap();
    }
}
