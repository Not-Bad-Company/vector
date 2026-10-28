//! Image thumbnails: an existing freedesktop thumbnail when it is current, else the image
//! itself scaled on load. Decoding runs on a worker thread; the texture is built on the main
//! thread from plain bytes.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Freedesktop thumbnail for `path` that is at least as new as the file.
pub fn freedesktop(path: &Path, modified: i64) -> Option<PathBuf> {
    let uri = gio::File::for_path(path).uri();
    let hash = glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri.as_str())?;
    for size in ["x-large", "large", "normal"] {
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
    let src = freedesktop(path, modified).unwrap_or_else(|| path.to_path_buf());
    // JPEG and PNG fast, in-process; gdk-pixbuf (a loader process per image) for the rest
    if let Some(px) = vector_core::image::decode(&src, Some(size as u32)) {
        return Some(Pixels {
            stride: px.width as usize * 4,
            data: px.rgba,
            alpha: true,
            width: px.width as i32,
            height: px.height as i32,
        });
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
