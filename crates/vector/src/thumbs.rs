//! Image thumbnails: an existing freedesktop thumbnail when it is current, else the image
//! itself scaled on load. Decoding runs on a worker thread; the texture is built on the main
//! thread from plain bytes.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Freedesktop thumbnail for `path` that is at least as new as the file and at least `size`
/// pixels (a 256 px thumbnail would look blurry in the preview).
pub fn freedesktop(path: &Path, modified: i64, size: i32) -> Option<PathBuf> {
    let uri = gio::File::for_path(path).uri();
    let hash = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())?;
    for (dir, px) in [("x-large", 512), ("large", 256), ("normal", 128)] {
        if px < size {
            continue;
        }
        let size = dir;
        let p = glib::user_cache_dir()
            .join("thumbnails")
            .join(size)
            .join(format!("{hash}.png"));
        let fresh = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|t| t.as_secs() as i64 >= modified);
        if fresh {
            return Some(p);
        }
    }
    None
}

static PREVIEW: std::sync::LazyLock<vector_core::pool::Pool> =
    std::sync::LazyLock::new(|| vector_core::pool::Pool::new("vector-preview", 2));
static TILES: std::sync::LazyLock<vector_core::pool::Pool> =
    std::sync::LazyLock::new(|| vector_core::pool::Pool::new("vector-tiles", 3));

/// Runs blocking preview work (details, text) on the preview workers.
pub async fn preview_work<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    PREVIEW.run(f).await
}

/// Size of the freedesktop "large" thumbnails.
const THUMB_LARGE: i32 = 256;

/// Scales `px` to a 256 px thumbnail, writes it to `~/.cache/thumbnails/large` the way the
/// freedesktop spec asks (URI and modified time inside, written atomically), returns it.
fn save_thumbnail(path: &Path, modified: i64, px: Pixels) -> Pixels {
    use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
    let pb = Pixbuf::from_bytes(
        &glib::Bytes::from_owned(px.data),
        Colorspace::Rgb,
        true,
        8,
        px.width,
        px.height,
        px.stride as i32,
    );
    let k = THUMB_LARGE as f64 / px.width.max(px.height) as f64;
    let (w, h) = (
        ((px.width as f64 * k).round() as i32).max(1),
        ((px.height as f64 * k).round() as i32).max(1),
    );
    let small = pb.scale_simple(w, h, InterpType::Bilinear).unwrap_or(pb);
    // thumbnails of our own caches (archive copies) would only be clutter
    let private =
        path.starts_with(glib::user_runtime_dir()) || path.starts_with(glib::user_cache_dir());
    if !private {
        let uri = gio::File::for_path(path).uri();
        if let Some(hash) = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())
        {
            let dir = glib::user_cache_dir().join("thumbnails/large");
            let out = dir.join(format!("{hash}.png"));
            // unique per call: two threads may thumbnail the same file at once
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = dir.join(format!(".{hash}.{}-{n}.png", std::process::id()));
            let mtime = modified.to_string();
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            // the spec: thumbnails are private (folders 0700, files 0600)
            let made = dir.is_dir()
                || std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&dir)
                    .is_ok();
            if made
                && small
                    .savev(
                        &tmp,
                        "png",
                        &[
                            ("tEXt::Thumb::URI", uri.as_str()),
                            ("tEXt::Thumb::MTime", &mtime),
                        ],
                    )
                    .is_ok()
            {
                let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
                if std::fs::rename(&tmp, &out).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    Pixels {
        stride: small.rowstride() as usize,
        data: small.read_pixel_bytes().to_vec(),
        alpha: small.has_alpha(),
        width: small.width(),
        height: small.height(),
    }
}

struct Pixels {
    data: Vec<u8>,
    alpha: bool,
    width: i32,
    height: i32,
    stride: usize,
}

fn decode(path: &Path, size: i32) -> Option<Pixels> {
    let modified = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let cached = freedesktop(path, modified, size);
    let src = cached.clone().unwrap_or_else(|| path.to_path_buf());
    // JPEG, PNG, WebP, HEIC and AVIF in-process; gdk-pixbuf (a loader process per image) for the rest
    if let Some(px) = vector_core::image::decode(&src, Some(size as u32)) {
        let px = Pixels {
            stride: px.width as usize * 4,
            data: px.rgba,
            alpha: true,
            width: px.width as i32,
            height: px.height as i32,
        };
        // grid tiles: keep a freedesktop thumbnail, so the folder is quick next time (and in
        // other apps)
        if cached.is_none() && size <= THUMB_LARGE && px.width.max(px.height) > THUMB_LARGE {
            return Some(save_thumbnail(path, modified, px));
        }
        return Some(px);
    }
    let pb = gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(&src, size, size, true).ok()?;
    let stride = usize::try_from(pb.rowstride()).ok()?;
    Some(Pixels {
        data: pb.read_pixel_bytes().to_vec(),
        alpha: pb.has_alpha(),
        width: pb.width(),
        height: pb.height(),
        stride,
    })
}

/// Loads `path` scaled to fit `size`×`size`. `Ok(None)` when it can't be decoded.
pub async fn load(
    path: PathBuf,
    size: i32,
) -> Result<Option<gdk::Texture>, Box<dyn std::any::Any + Send>> {
    // previews (big) and grid tiles (many) have their own workers, so a page of thumbnails
    // never holds up the preview, and neither waits behind GIO's own work
    let pool = if size > 256 { &*PREVIEW } else { &*TILES };
    let Some(px) = pool.run(move || decode(&path, size)).await else {
        return Ok(None);
    };
    Ok(px.map(|p| {
        let format = if p.alpha {
            gdk::MemoryFormat::R8g8b8a8
        } else {
            gdk::MemoryFormat::R8g8b8
        };
        gdk::MemoryTexture::new(
            p.width,
            p.height,
            format,
            &glib::Bytes::from_owned(p.data),
            p.stride,
        )
        .upcast()
    }))
}
