//! File previews for the right-hand pane.

use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gio, glib};

use sourceview5::prelude::*;
use strata_core::entry::{Entry, Kind};

pub struct Preview {
    root: gtk::Box,
    header: gtk::Box,
    icon: gtk::Label,
    title: gtk::Label,
    meta: gtk::Label,
    body: gtk::Box,
    generation: Cell<u64>,
    theme: strata_core::theme::Theme,
}

impl Preview {
    pub fn new(theme: &strata_core::theme::Theme) -> Rc<Preview> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_widget_name("preview");
        root.add_css_class("preview");
        root.set_size_request(360, -1);
        root.set_hexpand(false);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Label::new(None);
        icon.add_css_class("icon");
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        title.add_css_class("title");
        header.append(&icon);
        header.append(&title);
        root.append(&header);
        let meta = gtk::Label::new(None);
        meta.set_xalign(0.0);
        meta.add_css_class("dim");
        root.append(&meta);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.set_vexpand(true);
        root.append(&body);
        Rc::new(Preview {
            root,
            header,
            icon,
            title,
            meta,
            body,
            generation: Cell::new(0),
            theme: theme.clone(),
        })
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    pub fn show(self: &Rc<Self>, entry: Option<&Entry>) {
        self.generation.set(self.generation.get().wrapping_add(1));
        let generation = self.generation.get();
        clear(&self.body);
        let Some(entry) = entry.cloned() else {
            self.header.set_visible(false);
            self.meta.set_text("");
            return;
        };
        self.header.set_visible(true);
        self.icon.set_text(strata_core::icons::icon_for(
            &entry.name,
            entry.is_dir(),
            entry.symlink,
        ));
        self.title.set_text(&entry.name);
        let first = if entry.is_dir() {
            "folder".to_string()
        } else {
            crate::fmt::size(entry.size)
        };
        let parts = [
            first,
            entry.content_type.clone(),
            crate::fmt::date(entry.modified),
        ];
        self.meta.set_text(
            &parts
                .iter()
                .filter(|s| !s.is_empty() && s.as_str() != "inode/directory")
                .cloned()
                .collect::<Vec<_>>()
                .join(" · "),
        );
        if entry.broken || entry.kind == Kind::Special {
            self.no_preview(&entry);
            return;
        }
        if entry.is_dir() {
            self.show_folder(entry, generation);
            return;
        }
        let ty = entry.content_type.as_str();
        if ty.starts_with("image/") {
            self.show_image(entry, generation, None);
        } else if ty.starts_with("video/") {
            self.show_generated(entry, generation, true);
        } else if ty == "application/pdf" {
            self.show_generated(entry, generation, false);
        } else if is_text_type(ty) || ty.is_empty() || ty == "application/octet-stream" {
            self.show_text(entry, generation);
        } else {
            self.no_preview(&entry);
        }
    }

    fn no_preview(&self, entry: &Entry) {
        let label = gtk::Label::new(Some("no preview"));
        label.add_css_class("empty");
        self.body.append(&label);
        let big = gtk::Label::new(Some(strata_core::icons::icon_for(
            &entry.name,
            entry.is_dir(),
            entry.symlink,
        )));
        big.add_css_class("dim");
        big.set_vexpand(true);
        big.set_valign(gtk::Align::Center);
        self.body.prepend(&big);
    }

    fn show_image(self: &Rc<Self>, entry: Entry, generation: u64, source: Option<PathBuf>) {
        let weak = Rc::downgrade(self);
        let path = source.unwrap_or_else(|| entry.path.clone());
        glib::MainContext::default().spawn_local(async move {
            let result = crate::thumbs::load(path, 512).await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() != generation {
                return;
            }
            match result {
                Ok(Some(texture)) => {
                    let picture = gtk::Picture::for_paintable(&texture);
                    picture.set_content_fit(gtk::ContentFit::Contain);
                    picture.set_can_shrink(true);
                    picture.set_valign(gtk::Align::Start);
                    this.body.append(&picture);
                }
                _ => this.no_preview(&entry),
            }
        });
    }

    fn show_generated(self: &Rc<Self>, entry: Entry, generation: u64, video: bool) {
        let path = entry.path.clone();
        let uri = gio::File::for_path(&path).uri();
        let mtime = entry.modified;
        let output = glib::user_cache_dir()
            .join("strata/thumbs")
            .join(format!("{}-{mtime}.png", md5(&uri)));
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = generate_thumbnail(&path, &output, video).await;
            if let Some(this) = weak.upgrade()
                && this.generation.get() == generation
            {
                if result {
                    this.show_image(entry, generation, Some(output));
                } else {
                    this.no_preview(&entry);
                }
            }
        });
    }

    fn show_text(self: &Rc<Self>, entry: Entry, generation: u64) {
        let path = entry.path.clone();
        let weak = Rc::downgrade(self);
        let task = gio::spawn_blocking(move || {
            let mut bytes = Vec::new();
            fs::File::open(path)
                .ok()?
                .take(64 * 1024)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(bytes)
        });
        glib::MainContext::default().spawn_local(async move {
            let result = task.await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() == generation {
                let bytes = result.ok().flatten();
                if let Some(bytes) = bytes.filter(|b| !b[..b.len().min(8192)].contains(&0)) {
                    this.install_text(&entry, &String::from_utf8_lossy(&bytes));
                } else {
                    this.no_preview(&entry);
                }
            }
        });
    }

    fn install_text(&self, entry: &Entry, text: &str) {
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_text(text);
        let manager = sourceview5::LanguageManager::default();
        if let Some(language) = manager.guess_language(Some(&entry.name), Some(&entry.content_type))
        {
            buffer.set_language(Some(&language));
        }
        let scheme = ensure_scheme(&self.theme);
        if let Some(scheme) = scheme {
            buffer.set_style_scheme(Some(&scheme));
        }
        let view = sourceview5::View::with_buffer(&buffer);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(gtk::WrapMode::None);
        view.set_show_line_numbers(true);
        view.set_vexpand(true);
        view.set_hexpand(true);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        scroll.set_child(Some(&view));
        scroll.set_vexpand(true);
        self.body.append(&scroll);
    }

    fn show_folder(self: &Rc<Self>, entry: Entry, generation: u64) {
        let path = entry.path.clone();
        let weak = Rc::downgrade(self);
        let task = gio::spawn_blocking(move || {
            let mut entries = Vec::new();
            let listed =
                strata_core::listing::list_blocking(&path, &gio::Cancellable::new(), |batch| {
                    entries.extend(batch);
                    true
                });
            entries.retain(|e| !e.hidden);
            entries.sort_by(|a, b| {
                strata_core::sort::compare(a, b, &strata_core::sort::SortSpec::default())
            });
            listed.map(|()| entries)
        });
        glib::MainContext::default().spawn_local(async move {
            let result = task.await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() == generation {
                if let Ok(Ok(mut entries)) = result {
                    let count = entries.len();
                    this.meta.set_text(&format!("{count} items"));
                    entries.truncate(200);
                    for child in entries {
                        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let glyph = gtk::Label::new(Some(strata_core::icons::icon_for(
                            &child.name,
                            child.is_dir(),
                            child.symlink,
                        )));
                        let name = gtk::Label::new(Some(&child.name));
                        name.set_xalign(0.0);
                        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                        if child.hidden {
                            name.add_css_class("dim");
                        }
                        row.append(&glyph);
                        row.append(&name);
                        this.body.append(&row);
                    }
                } else {
                    this.no_preview(&entry);
                }
            }
        });
    }
}

fn clear(widget: &gtk::Box) {
    while let Some(child) = widget.first_child() {
        widget.remove(&child);
    }
}

fn is_text_type(content_type: &str) -> bool {
    gio::content_type_is_a(content_type, "text/plain")
        || [
            "application/json",
            "application/xml",
            "text/xml",
            "application/javascript",
            "text/javascript",
            "application/x-shellscript",
            "text/x-shellscript",
            "application/toml",
            "text/x-toml",
            "application/yaml",
            "text/yaml",
            "application/x-desktop",
        ]
        .contains(&content_type)
        || content_type.is_empty()
}

fn md5(uri: &str) -> String {
    glib::compute_checksum_for_string(glib::ChecksumType::Md5, uri)
        .unwrap_or_default()
        .to_string()
}

async fn generate_thumbnail(input: &Path, output: &Path, video: bool) -> bool {
    if fs::metadata(output).is_ok() {
        return true;
    }
    if let Some(parent) = output.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    let (program, args): (&str, Vec<String>) = if video {
        (
            "ffmpegthumbnailer",
            vec![
                "-i".into(),
                input.display().to_string(),
                "-o".into(),
                output.display().to_string(),
                "-s".into(),
                "512".into(),
                "-t".into(),
                "10%".into(),
            ],
        )
    } else {
        (
            "pdftoppm",
            vec![
                "-png".into(),
                "-singlefile".into(),
                "-f".into(),
                "1".into(),
                "-l".into(),
                "1".into(),
                "-scale-to".into(),
                "768".into(),
                input.display().to_string(),
                output.with_extension("").display().to_string(),
            ],
        )
    };
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let command: Vec<&std::ffi::OsStr> = std::iter::once(std::ffi::OsStr::new(program))
        .chain(argv.iter().map(std::ffi::OsStr::new))
        .collect();
    let Ok(process) = gio::Subprocess::newv(&command, gio::SubprocessFlags::NONE) else {
        return false;
    };
    process.wait_check_future().await.is_ok()
}

fn ensure_scheme(theme: &strata_core::theme::Theme) -> Option<sourceview5::StyleScheme> {
    let manager = sourceview5::StyleSchemeManager::default();
    if let Some(s) = manager.scheme("strata") {
        return Some(s);
    }
    let dir = glib::user_cache_dir().join("strata/styles");
    if fs::create_dir_all(&dir).is_err() {
        return None;
    }
    if fs::write(dir.join("strata.xml"), scheme_xml(theme)).is_err() {
        return None;
    }
    manager.append_search_path(dir.to_str()?);
    manager.scheme("strata")
}

fn scheme_xml(theme: &strata_core::theme::Theme) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<style-scheme id="strata" name="Strata" version="1.0"><author>strata</author><description>Theme-matched preview</description>
<style name="text" foreground="{fg}"/><style name="def:comment" foreground="{fg}80"/><style name="def:keyword" foreground="{acc}"/><style name="def:preprocessor" foreground="{acc}"/><style name="def:string" foreground="{c2}"/><style name="def:number" foreground="{c3}"/><style name="def:constant" foreground="{c3}"/><style name="def:special-char" foreground="{c3}"/><style name="def:type" foreground="{acc}"/><style name="def:function" foreground="{fg}" bold="true"/><style name="def:error" foreground="{c1}"/><style name="line-numbers" foreground="{fg}66"/></style-scheme>"#,
        fg = theme.foreground,
        acc = theme.accent,
        c1 = theme.color1,
        c2 = theme.color2,
        c3 = theme.color3
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_text_types() {
        assert!(is_text_type("text/plain"));
        assert!(is_text_type("application/json"));
        assert!(is_text_type(""));
        assert!(!is_text_type("application/pdf"));
    }
    #[test]
    fn scheme_uses_theme_colours() {
        let xml = scheme_xml(&strata_core::theme::Theme::default());
        assert!(xml.contains("id=\"strata\""));
        assert!(xml.contains("#cdd6f480"));
    }
}
