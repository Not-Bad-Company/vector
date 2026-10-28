//! The details panel (`i`): file facts plus what ffprobe, pdfinfo or the image header say.
//! Blocking: runs on a worker thread.

use std::process::Command;

use gtk::{gdk_pixbuf, gio, glib};
use vector_core::details::{self, Row, group_digits};

use crate::playlist::{Item, Kind};

fn run(cmd: &mut Command, item: &Item) -> String {
    cmd.arg(&item.path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Sections of rows: the file itself, then what kind of media it is.
pub fn rows(item: &Item) -> Vec<(String, Vec<Row>)> {
    let mut rows: Vec<Row> = vec![("name".into(), item.name())];
    if let Some(d) = item.path.parent() {
        rows.push(("folder".into(), d.display().to_string()));
    }
    rows.push(("size".into(), format!("{} bytes", group_digits(item.size))));
    let ct = gio::content_type_guess(Some(&item.path), None).0;
    rows.push((
        "type".into(),
        gio::content_type_get_description(&ct).to_string(),
    ));
    if let Some(t) = glib::DateTime::from_unix_local(item.modified)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d %H:%M").ok())
    {
        rows.push(("modified".into(), t.to_string()));
    }
    let mut media: Vec<Row> = Vec::new();
    match item.kind {
        Kind::Image => {
            if let Some((fmt, w, h)) = gdk_pixbuf::Pixbuf::file_info(&item.path) {
                media.extend(details::image(
                    w as u64,
                    h as u64,
                    fmt.name().as_deref().unwrap_or(""),
                ));
            }
        }
        Kind::Video => media.extend(details::ffprobe(&run(
            Command::new("ffprobe").args([
                "-v",
                "error",
                "-show_entries",
                "format=duration,bit_rate,format_long_name:stream=codec_type,codec_name,width,height,r_frame_rate,channels,sample_rate,pix_fmt",
                "-of",
                "default=nw=1",
            ]),
            item,
        ))),
        Kind::Pdf => media.extend(details::pdfinfo(&run(&mut Command::new("pdfinfo"), item))),
        Kind::Office => {}
    }
    let name = match item.kind {
        Kind::Image => "image",
        Kind::Video => "video",
        Kind::Pdf | Kind::Office => "document",
    };
    let mut out = vec![("file".to_string(), rows)];
    if !media.is_empty() {
        out.insert(0, (name.to_string(), media));
    }
    out
}
