//! Screen capture: Hyprland's geometry (outputs, windows), the frozen grab of an
//! output (grim), the crop of a selection, and where the results are saved.

use std::path::{Path, PathBuf};
use std::process::Command;

use gtk::gdk;
use gtk::gdk_pixbuf::Pixbuf;
use gtk::prelude::*;

/// An output in logical pixels, as the compositor lays it out.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Hyprland's active workspace on this output.
    pub workspace: i64,
}

/// A mapped window on the output, in the output's local logical coordinates.
#[derive(Clone, Debug)]
pub struct WinRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    #[allow(dead_code)]
    pub title: String,
}

/// The output a capture starts on: `VECTOR_OUTPUT` (the AI workspace's headless output in tests),
/// else the focused one.
pub fn target_monitor() -> Option<Monitor> {
    let all = monitors();
    let want = std::env::var("VECTOR_OUTPUT").ok().filter(|s| !s.is_empty());
    match want {
        Some(name) => all.iter().find(|m| m.name == name).cloned(),
        None => all.iter().find(|m| m.name == focused_name(&all)).cloned(),
    }
    .or_else(|| all.first().cloned())
}

fn focused_name(all: &[Monitor]) -> String {
    hypr_json("monitors")
        .and_then(|v| v.as_array().cloned())
        .and_then(|a| a.into_iter().find(|m| m["focused"].as_bool() == Some(true)))
        .and_then(|m| m["name"].as_str().map(str::to_owned))
        .or_else(|| all.first().map(|m| m.name.clone()))
        .unwrap_or_default()
}

/// Outputs in logical pixels. A transform of 1 or 3 is a rotated panel: its size is swapped.
pub fn monitors() -> Vec<Monitor> {
    let Some(list) = hypr_json("monitors").and_then(|v| v.as_array().cloned()) else { return Vec::new() };
    list.iter()
        .filter_map(|m| {
            let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
            let (pw, ph) = (m["width"].as_f64()?, m["height"].as_f64()?);
            let (pw, ph) = if matches!(m["transform"].as_i64(), Some(1 | 3)) { (ph, pw) } else { (pw, ph) };
            Some(Monitor {
                name: m["name"].as_str()?.to_owned(),
                x: m["x"].as_i64()? as i32,
                y: m["y"].as_i64()? as i32,
                w: (pw / scale).round() as i32,
                h: (ph / scale).round() as i32,
                workspace: m["activeWorkspace"]["id"].as_i64().unwrap_or(1),
            })
        })
        .collect()
}

/// Mapped windows on the output's active workspace, most recently focused first. The first one
/// that contains a point is the one under the pointer.
pub fn windows_on(mon: &Monitor) -> Vec<WinRect> {
    let Some(list) = hypr_json("clients").and_then(|v| v.as_array().cloned()) else { return Vec::new() };
    let mut clients: Vec<_> = list
        .into_iter()
        .filter(|c| c["workspace"]["id"].as_i64() == Some(mon.workspace))
        .filter(|c| c["mapped"].as_bool().unwrap_or(true) && !c["hidden"].as_bool().unwrap_or(false))
        .collect();
    clients.sort_by_key(|c| c["focusHistoryID"].as_i64().unwrap_or(i64::MAX));
    clients
        .iter()
        .filter_map(|c| {
            let (x, y) = (c["at"][0].as_f64()?, c["at"][1].as_f64()?);
            let (w, h) = (c["size"][0].as_f64()?, c["size"][1].as_f64()?);
            if w < 2.0 || h < 2.0 {
                return None;
            }
            Some(WinRect {
                x: x - mon.x as f64,
                y: y - mon.y as f64,
                w,
                h,
                title: c["title"].as_str().unwrap_or_default().to_owned(),
            })
        })
        .collect()
}

fn hypr_json(what: &str) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args(["-j", what]).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

/// The frozen image of one output, as grim sees it at this instant. Raw PPM: grim skips the
/// PNG encode (about 15 ms faster than PNG at level 0), and the RGB bytes go straight into a
/// pixbuf with no decode.
pub fn grab(mon: &Monitor) -> Result<Pixbuf, String> {
    let out = Command::new("grim")
        .args(["-o", &mon.name, "-t", "ppm", "-"])
        .output()
        .map_err(|e| format!("grim: {e}"))?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(format!("grim: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    ppm_to_pixbuf(&out.stdout).ok_or_else(|| "grim: unreadable PPM".to_owned())
}

/// Parses a binary PPM (P6, 8 bits) into an RGB pixbuf.
fn ppm_to_pixbuf(data: &[u8]) -> Option<Pixbuf> {
    // header: "P6", then width, height, maxval, separated by whitespace, then one byte
    let mut fields = Vec::with_capacity(4);
    let mut i = 0;
    while fields.len() < 4 {
        while i < data.len() && data[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < data.len() && !data[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&data[start..i]).ok()?.to_owned());
    }
    i += 1; // the single whitespace byte before the pixel data
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let (w, h): (i32, i32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let stride = w as usize * 3;
    let pixels = data.get(i..i + stride * h as usize)?;
    Some(Pixbuf::from_bytes(
        &gtk::glib::Bytes::from(pixels),
        gtk::gdk_pixbuf::Colorspace::Rgb,
        false,
        8,
        w,
        h,
        stride as i32,
    ))
}

/// Copies the part of `frozen` at logical rect (x, y, w, h) of an output with logical size `mon`.
pub fn crop(frozen: &Pixbuf, mon: &Monitor, x: f64, y: f64, w: f64, h: f64) -> Option<Pixbuf> {
    let (sx, sy) = (frozen.width() as f64 / mon.w as f64, frozen.height() as f64 / mon.h as f64);
    let px = (x * sx).round().max(0.0) as i32;
    let py = (y * sy).round().max(0.0) as i32;
    let pw = ((w * sx).round() as i32).min(frozen.width() - px);
    let ph = ((h * sy).round() as i32).min(frozen.height() - py);
    if pw < 1 || ph < 1 {
        return None;
    }
    frozen.new_subpixbuf(px, py, pw, ph).copy()
}

/// Puts an image on the clipboard. The daemon owns it while it runs.
#[allow(deprecated)] // Texture::for_pixbuf: the replacement needs the encoded bytes, which we do not keep
pub fn copy_to_clipboard(pb: &Pixbuf) {
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_texture(&gdk::Texture::for_pixbuf(pb));
    }
}

/// `$OMARCHY_SCREENSHOT_DIR`, else the XDG pictures directory, else ~/Pictures. Created if missing.
pub fn pictures_dir() -> PathBuf {
    let dir = std::env::var_os("OMARCHY_SCREENSHOT_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(xdg_pictures)
        .unwrap_or_else(|| crate::util::home().join("Pictures"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn xdg_pictures() -> Option<PathBuf> {
    let text = std::fs::read_to_string(crate::util::config_dir().join("user-dirs.dirs")).ok()?;
    let line = text.lines().find(|l| l.starts_with("XDG_PICTURES_DIR="))?;
    let raw = line.trim_start_matches("XDG_PICTURES_DIR=").trim().trim_matches('"');
    let home = crate::util::home();
    let path = raw.replace("$HOME", &home.to_string_lossy());
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// A timestamped file name in `dir`, as the Omarchy screenshot flow names them.
pub fn timestamped(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    let stamp = gtk::glib::DateTime::now_local()
        .ok()
        .and_then(|t| t.format("%Y-%m-%d_%H-%M-%S").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());
    dir.join(format!("{prefix}-{stamp}.{ext}"))
}

pub fn save_png(pb: &Pixbuf, path: &Path) -> Result<(), String> {
    pb.savev(path, "png", &[]).map_err(|e| format!("save {}: {e}", path.display()))
}
