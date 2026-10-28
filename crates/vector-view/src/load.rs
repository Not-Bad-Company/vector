//! Decoding off the main thread: full images (EXIF orientation applied), thumbnails for the
//! grid, and office documents converted to PDF by LibreOffice.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};

use crate::playlist::Kind;

pub struct Pixels {
    data: Vec<u8>,
    alpha: bool,
    width: i32,
    height: i32,
    stride: usize,
}

impl Pixels {
    fn from_pixbuf(pb: gdk_pixbuf::Pixbuf) -> Option<Pixels> {
        let pb = pb.apply_embedded_orientation().unwrap_or(pb);
        Some(Pixels {
            data: pb.read_pixel_bytes().to_vec(),
            alpha: pb.has_alpha(),
            width: pb.width(),
            height: pb.height(),
            stride: usize::try_from(pb.rowstride()).ok()?,
        })
    }

    /// JPEG and PNG through vector-core's fast in-process decoders (upright, scaled down while
    /// decoding when `max` is given).
    fn fast(path: &Path, max: Option<u32>) -> Option<Pixels> {
        let px = vector_core::image::decode(path, max)?;
        Some(Pixels {
            stride: px.width as usize * 4,
            data: px.rgba,
            alpha: true,
            width: px.width as i32,
            height: px.height as i32,
        })
    }

    fn texture(self) -> gdk::Texture {
        let format = if self.alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        gdk::MemoryTexture::new(
            self.width,
            self.height,
            format,
            &glib::Bytes::from_owned(self.data),
            self.stride,
        )
        .upcast()
    }
}

/// The whole image. `Err` carries a message for the user.
pub async fn image(path: PathBuf) -> Result<gdk::Texture, String> {
    let px = gio::spawn_blocking(move || {
        if let Some(px) = Pixels::fast(&path, None) {
            return Ok(px);
        }
        // other formats: gdk-pixbuf (slower: a loader process per image)
        gdk_pixbuf::Pixbuf::from_file(&path)
            .map_err(|e| e.message().to_string())
            .and_then(|pb| Pixels::from_pixbuf(pb).ok_or_else(|| "can't read this image".into()))
    })
    .await
    .map_err(|_| "decoder crashed".to_string())??;
    Ok(px.texture())
}

fn md5(s: &str) -> String {
    glib::compute_checksum_for_string(glib::ChecksumType::Md5, s)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn modified(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// The freedesktop thumbnail path (`large`, 256 px) for `path`.
fn freedesktop(path: &Path) -> PathBuf {
    let uri = gio::File::for_path(path).uri();
    glib::user_cache_dir()
        .join("thumbnails/large")
        .join(format!("{}.png", md5(uri.as_str())))
}

fn is_fresh(thumb: &Path, source: &Path) -> bool {
    std::fs::metadata(thumb).is_ok() && modified(thumb) >= modified(source)
}

thread_local! {
    /// Thumbnailer processes running now (scrolling fast must not start hundreds).
    static THUMBNAILERS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// A LibreOffice conversion is running (one at a time: they share a profile).
    static CONVERTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// At most this many video thumbnailers at once.
const MAX_THUMBNAILERS: usize = 2;

/// A thumbnail fitted into `size`×`size`, or `None` when the file can't be pictured (yet: an
/// office document is pictured once it has been converted for viewing).
///
/// Office documents not yet converted are converted first (in the background, one at a time,
/// at low priority) while `wanted` says the thumbnail is still on screen.
pub async fn thumbnail(
    path: PathBuf,
    kind: Kind,
    size: i32,
    wanted: impl Fn() -> bool,
) -> Option<gdk::Texture> {
    let (p, k) = (path.clone(), kind);
    let (thumb, fresh, office) = gio::spawn_blocking(move || {
        let thumb = freedesktop(&p);
        let fresh = is_fresh(&thumb, &p);
        if k == Kind::Video
            && !fresh
            && let Some(dir) = thumb.parent()
        {
            let _ = std::fs::create_dir_all(dir);
        }
        let office = (k == Kind::Office)
            .then(|| converted(&p).filter(|c| c.exists()))
            .flatten();
        (thumb, fresh, office)
    })
    .await
    .ok()?;
    let office = match (kind, office) {
        (Kind::Office, None) => Some(self::office(path.clone(), wanted).await.ok()?),
        (_, o) => o,
    };
    if kind == Kind::Video && !fresh {
        while THUMBNAILERS.get() >= MAX_THUMBNAILERS {
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }
        THUMBNAILERS.set(THUMBNAILERS.get() + 1);
        let args = [
            "ffmpegthumbnailer",
            "-i",
            path.to_str().unwrap_or_default(),
            "-o",
            thumb.to_str().unwrap_or_default(),
            "-s",
            "256",
            "-t",
            "10%",
        ];
        let args: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
        let ok = match gio::Subprocess::newv(&args, gio::SubprocessFlags::STDERR_SILENCE) {
            Ok(p) => p.wait_check_future().await.is_ok(),
            Err(_) => false,
        };
        THUMBNAILERS.set(THUMBNAILERS.get() - 1);
        if !ok {
            return None;
        }
    }
    let px = gio::spawn_blocking(move || match kind {
        Kind::Pdf | Kind::Office => {
            let r = crate::pdf::first_page(office.as_deref().unwrap_or(&path), size)?;
            Some(Err(r))
        }
        _ => {
            let src = if kind == Kind::Video || is_fresh(&thumb, &path) {
                thumb
            } else {
                path
            };
            if let Some(px) = Pixels::fast(&src, Some(size as u32)) {
                return Some(Ok(px));
            }
            let pb = gdk_pixbuf::Pixbuf::from_file_at_scale(&src, size, size, true).ok()?;
            Pixels::from_pixbuf(pb).map(Ok)
        }
    })
    .await
    .ok()??;
    Some(match px {
        Ok(p) => p.texture(),
        Err(r) => r.texture(),
    })
}

/// Where the PDF made from office document `path` is kept (per path, size and time).
pub fn converted(path: &Path) -> Option<PathBuf> {
    let m = std::fs::metadata(path).ok()?;
    let key = format!("{}\0{}\0{}", path.display(), m.len(), modified(path));
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    Some(
        glib::user_cache_dir()
            .join("vector/view/office")
            .join(md5(&key))
            .join(format!("{stem}.pdf")),
    )
}

/// Converts an office document to PDF with LibreOffice (headless, its own profile so a
/// running LibreOffice is left alone; one conversion at a time, at most two minutes each).
/// `wanted` is asked before starting, so a file the user has moved past is skipped.
pub async fn office(path: PathBuf, wanted: impl Fn() -> bool) -> Result<PathBuf, String> {
    let p = path.clone();
    let out = gio::spawn_blocking(move || {
        let out = converted(&p)?;
        Some((out.exists(), out))
    })
    .await
    .ok()
    .flatten()
    .ok_or("can't read this file")?;
    if out.0 {
        return Ok(out.1);
    }
    let out = out.1;
    let program = ["soffice", "libreoffice"]
        .into_iter()
        .find(|p| glib::find_program_in_path(p).is_some())
        .ok_or("install LibreOffice to preview office documents")?;
    while CONVERTING.get() {
        glib::timeout_future(std::time::Duration::from_millis(100)).await;
    }
    if !wanted() {
        return Err("skipped".into());
    }
    CONVERTING.set(true);
    let r = convert(program, &path, &out).await;
    CONVERTING.set(false);
    r
}

async fn convert(program: &str, path: &Path, out: &Path) -> Result<PathBuf, String> {
    // written to a scratch folder first and moved in whole, so a half-written PDF never counts
    let dir = out.parent().ok_or("bad cache path")?.to_path_buf();
    let scratch = dir.join(".converting");
    let s = scratch.clone();
    gio::spawn_blocking(move || {
        let _ = std::fs::remove_dir_all(&s);
        std::fs::create_dir_all(&s)
    })
    .await
    .map_err(|_| "can't write the cache")?
    .map_err(|e| e.to_string())?;
    let profile = glib::user_cache_dir().join("vector/view/libreoffice");
    let profile = gio::File::for_path(&profile).uri();
    // low priority: conversions for the gallery shouldn't slow the desktop down
    let args = [
        "nice",
        "-n",
        "15",
        program,
        &format!("-env:UserInstallation={profile}"),
        "--headless",
        "--norestore",
        "--convert-to",
        "pdf",
        "--outdir",
        scratch.to_str().ok_or("bad cache path")?,
        path.to_str().ok_or("bad file name")?,
    ];
    let args: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    let p = gio::Subprocess::newv(
        &args,
        gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .map_err(|e| e.message().to_string())?;
    let timeout = glib::timeout_future(std::time::Duration::from_secs(120));
    let done = p.wait_future();
    with_timeout(done, timeout, &p).await?;
    let (from, to) = (
        scratch.join(out.file_name().ok_or("bad name")?),
        out.to_path_buf(),
    );
    gio::spawn_blocking(move || {
        let r = std::fs::rename(&from, &to);
        let _ = std::fs::remove_dir_all(scratch);
        r.map(|_| to)
    })
    .await
    .map_err(|_| "can't write the cache")?
    .map_err(|_| "LibreOffice could not convert this file".to_string())
}

/// Waits for LibreOffice, killing it if it takes longer than `timeout`.
async fn with_timeout(
    done: impl std::future::Future<Output = Result<(), glib::Error>>,
    timeout: impl std::future::Future<Output = ()>,
    p: &gio::Subprocess,
) -> Result<(), String> {
    use std::pin::pin;
    let (mut done, mut timeout) = (pin!(done), pin!(timeout));
    std::future::poll_fn(|cx| {
        if let std::task::Poll::Ready(r) = done.as_mut().poll(cx) {
            return std::task::Poll::Ready(r.map_err(|e| e.message().to_string()));
        }
        if timeout.as_mut().poll(cx).is_ready() {
            p.force_exit();
            return std::task::Poll::Ready(Err("LibreOffice took too long".into()));
        }
        std::task::Poll::Pending
    })
    .await
}
