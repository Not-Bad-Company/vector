//! File previews for the right-hand pane.

use std::cell::Cell;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gio, glib};

use sourceview5::prelude::*;
use vector_core::entry::{Entry, Kind};

pub struct Preview {
    root: gtk::Box,
    header: gtk::Box,
    icon: gtk::Label,
    title: gtk::Label,
    body: gtk::Box,
    details: gtk::Grid,
    generation: Cell<u64>,
    /// The audio player of the current preview, if playing.
    player: std::cell::RefCell<Option<Player>>,
    /// The entry on show: a refresh of the same unchanged file keeps the preview (and its audio).
    shown: std::cell::RefCell<Option<Entry>>,
    theme: vector_core::theme::Theme,
}

/// Files inside an archive larger than this are not copied out just to preview them.
const ARCHIVE_PREVIEW_MAX: u64 = 64 << 20;
/// Tar archives larger than this aren't read just to preview what is inside (enter opens them).
const ARCHIVE_INDEX_MAX: u64 = 256 << 20;

/// Details of an item inside an archive: what the archive's index says.
fn archive_rows(e: &Entry) -> Vec<(String, String)> {
    let mut rows = vec![("§archive".to_string(), String::new())];
    if !e.is_dir() {
        rows.push((
            "size".into(),
            format!("{} bytes", vector_core::details::group_digits(e.size)),
        ));
    }
    rows.push((
        "type".into(),
        gio::content_type_get_description(&e.content_type).to_string(),
    ));
    if let Some(t) = glib::DateTime::from_unix_local(e.modified)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d %H:%M").ok())
    {
        rows.push(("modified".into(), t.to_string()));
    }
    rows.push(("".into(), "read-only · X extracts".into()));
    rows
}

/// Whether a refresh shows the same thing: same file, not modified since. A folder's size is
/// left out: it changes while sizes are measured in the background, and the preview measures
/// it itself.
fn same_item(a: Option<&Entry>, b: Option<&Entry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.path == b.path
                && a.modified == b.modified
                && a.kind == b.kind
                && a.content_type == b.content_type
                && (a.is_dir() || a.size == b.size)
        }
        _ => false,
    }
}

impl Preview {
    pub fn new(theme: &vector_core::theme::Theme) -> Rc<Preview> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.set_widget_name("preview");
        root.add_css_class("preview");
        root.set_size_request(360, -1);
        root.set_vexpand(true);
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
        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.append(&body);
        let details = gtk::Grid::new();
        details.add_css_class("details");
        details.set_column_spacing(16);
        details.set_row_spacing(4);
        root.append(&details);
        Rc::new(Preview {
            root,
            header,
            icon,
            title,
            body,
            details,
            generation: Cell::new(0),
            player: Default::default(),
            shown: Default::default(),
            theme: theme.clone(),
        })
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.root.upcast_ref()
    }

    pub fn show(self: &Rc<Self>, entry: Option<&Entry>) {
        if same_item(self.shown.borrow().as_ref(), entry) {
            return;
        }
        *self.shown.borrow_mut() = entry.cloned();
        self.generation.set(self.generation.get().wrapping_add(1));
        self.stop_audio();
        let generation = self.generation.get();
        clear(&self.body);
        self.body.set_vexpand(false);
        while let Some(c) = self.details.first_child() {
            self.details.remove(&c);
        }
        let Some(entry) = entry.cloned() else {
            self.header.set_visible(false);
            return;
        };
        self.header.set_visible(true);
        self.icon.set_text(vector_core::icons::icon_for(
            &entry.name,
            entry.is_dir(),
            entry.symlink,
        ));
        self.title.set_text(&entry.name);
        // inside an archive: what the archive says, and a preview of a read-only copy
        if let Some((archive, inner)) = vector_core::archive::split(&entry.path)
            .filter(|(_, inner)| !inner.as_os_str().is_empty())
        {
            self.fill_details(&archive_rows(&entry));
            if entry.is_dir() {
                self.show_archive_folder(entry, generation, archive, inner);
            } else if entry.size <= ARCHIVE_PREVIEW_MAX {
                let weak = Rc::downgrade(self);
                glib::MainContext::default().spawn_local(async move {
                    let copy = crate::thumbs::preview_work(move || {
                        vector_core::archive::extract_one(&archive, &inner).ok()
                    })
                    .await
                    .flatten();
                    let Some(this) = weak.upgrade() else { return };
                    if this.generation.get() != generation {
                        return;
                    }
                    match copy {
                        Some(path) => this.show_body(Entry { path, ..entry }, generation),
                        None => this.no_preview(&entry),
                    }
                });
            } else {
                self.no_preview(&entry);
            }
            return;
        }
        // an archive file: what is inside, like a folder
        if !entry.is_dir() && vector_core::archive::is_archive_name(&entry.path) {
            self.load_details(&entry, generation);
            // a zip has an index at the end; a big compressed tar would have to be unpacked
            // whole just to show its contents
            let zip = entry.extension().eq_ignore_ascii_case("zip");
            if zip || entry.size <= ARCHIVE_INDEX_MAX {
                let path = entry.path.clone();
                self.show_archive_folder(entry, generation, path, PathBuf::new());
            } else {
                self.no_preview(&entry);
            }
            return;
        }
        // a name that says nothing (no extension): look at the contents first
        if !entry.is_dir()
            && (entry.content_type.is_empty() || gio::content_type_is_unknown(&entry.content_type))
        {
            let weak = Rc::downgrade(self);
            glib::MainContext::default().spawn_local(async move {
                let (p, t) = (entry.path.clone(), entry.content_type.clone());
                let ct =
                    crate::thumbs::preview_work(move || vector_core::entry::sniffed_type(&p, &t))
                        .await
                        .unwrap_or_default();
                if let Some(this) = weak.upgrade()
                    && this.generation.get() == generation
                {
                    let entry = Entry {
                        content_type: ct,
                        ..entry
                    };
                    this.load_details(&entry, generation);
                    this.show_body(entry, generation);
                }
            });
            return;
        }
        self.load_details(&entry, generation);
        self.show_body(entry, generation);
    }

    /// The preview proper for a file or folder on disk (or a copy out of an archive).
    fn show_body(self: &Rc<Self>, entry: Entry, generation: u64) {
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
        } else if ty.starts_with("audio/") {
            self.show_audio(entry, generation);
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
        let big = gtk::Label::new(Some(vector_core::icons::icon_for(
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
            .join("vector/thumbs")
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
        let task = crate::thumbs::preview_work(move || {
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
                let bytes = result.flatten();
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
        self.body.set_vexpand(true);
        self.body.append(&scroll);
    }

    /// The contents of a folder inside an archive (or of the archive itself).
    fn show_archive_folder(
        self: &Rc<Self>,
        entry: Entry,
        generation: u64,
        archive: PathBuf,
        inner: PathBuf,
    ) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let listed = crate::thumbs::preview_work(move || {
                vector_core::archive::list(&archive, &inner).map_err(|e| e.to_string())
            })
            .await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            match listed {
                Some(Ok(mut entries)) => {
                    entries.sort_by(|a, b| {
                        vector_core::sort::compare(a, b, &vector_core::sort::SortSpec::default())
                    });
                    this.fill_list(entries);
                }
                _ => this.no_preview(&entry),
            }
        });
    }

    /// A folder's contents under its title (at most 200, scrolling inside the preview).
    fn fill_list(&self, mut entries: Vec<Entry>) {
        entries.truncate(200);
        // a long folder scrolls inside the preview; the details stay visible
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        // inside the folder: lined up under its name, right of its icon
        list.set_margin_start(30);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        self.body.set_vexpand(true);
        self.body.append(&scroll);
        for child in entries {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let glyph = gtk::Label::new(Some(vector_core::icons::icon_for(
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
            list.append(&row);
        }
    }

    fn show_folder(self: &Rc<Self>, entry: Entry, generation: u64) {
        let path = entry.path.clone();
        let weak = Rc::downgrade(self);
        let task = crate::thumbs::preview_work(move || {
            let mut entries = Vec::new();
            let listed =
                vector_core::listing::list_blocking(&path, &gio::Cancellable::new(), |batch| {
                    entries.extend(batch);
                    true
                });
            entries.retain(|e| !e.hidden);
            entries.sort_by(|a, b| {
                vector_core::sort::compare(a, b, &vector_core::sort::SortSpec::default())
            });
            listed.map(|()| entries)
        });
        glib::MainContext::default().spawn_local(async move {
            let result = task.await;
            let Some(this) = weak.upgrade() else {
                return;
            };
            if this.generation.get() == generation {
                if let Some(Ok(entries)) = result {
                    this.fill_list(entries);
                } else {
                    this.no_preview(&entry);
                }
            }
        });
    }
}

impl Preview {
    /// Gathers the details table on a worker thread (stat, image header, ffprobe, pdfinfo,
    /// text statistics) and shows it under the preview.
    fn load_details(self: &Rc<Self>, entry: &Entry, generation: u64) {
        let weak = Rc::downgrade(self);
        let e = entry.clone();
        glib::MainContext::default().spawn_local(async move {
            let q = e.clone();
            let Some(quick) = crate::thumbs::preview_work(move || crate::details::quick(&q)).await
            else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            this.fill_details(&quick);
            drop(this);
            let Some(slow) = crate::thumbs::preview_work(move || crate::details::slow(&e)).await
            else {
                return;
            };
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation || slow.is_empty() {
                return;
            }
            let all: Vec<_> = slow.into_iter().chain(quick).collect();
            this.fill_details(&all);
        });
    }

    fn fill_details(&self, rows: &[(String, String)]) {
        while let Some(c) = self.details.first_child() {
            self.details.remove(&c);
        }
        for (i, (k, v)) in rows.iter().enumerate() {
            if let Some(title) = k.strip_prefix('§') {
                let l = gtk::Label::new(Some(title));
                l.add_css_class("dim");
                l.add_css_class("details-section");
                l.set_xalign(0.0);
                if i > 0 {
                    l.set_margin_top(12);
                }
                self.details.attach(&l, 0, i as i32, 2, 1);
                continue;
            }
            let kl = gtk::Label::new(Some(k));
            kl.add_css_class("dim");
            kl.set_xalign(0.0);
            kl.set_valign(gtk::Align::Start);
            let vl = gtk::Label::new(Some(v));
            vl.set_xalign(0.0);
            // no wrapping: a wrapped label asks for a huge height at its minimum width
            vl.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            vl.set_width_chars(8);
            // a long value (a path) must not widen the pane; it ellipsizes instead
            vl.set_max_width_chars(38);
            vl.set_tooltip_text(Some(v));
            vl.set_hexpand(true);
            self.details.attach(&kl, 0, i as i32, 1, 1);
            self.details.attach(&vl, 1, i as i32, 1, 1);
        }
    }
}

/// A running audio preview: the player process, where playback started, the playhead.
struct Player {
    process: Option<gio::Subprocess>,
    /// Seconds into the file when the process started, and when.
    from: f64,
    started: std::time::Instant,
    duration: f64,
    tick: Option<glib::SourceId>,
    playhead: gtk::Box,
    wave: gtk::Picture,
    button: gtk::Button,
    time: gtk::Label,
    path: PathBuf,
}

impl Preview {
    /// Audio: a waveform (ffmpeg) with play/pause; a click on the waveform seeks.
    fn show_audio(self: &Rc<Self>, entry: Entry, generation: u64) {
        let path = entry.path.clone();
        let uri = gio::File::for_path(&path).uri();
        let out = glib::user_cache_dir()
            .join(vector_core::APP_NAME)
            .join("thumbs")
            .join(format!("{}-{}-wave.png", md5(&uri), entry.modified));
        let colour = self.theme.accent.clone();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let (p, o) = (path.clone(), out.clone());
            let made = gio::spawn_blocking(move || {
                if fs::metadata(&o).is_err() {
                    if let Some(d) = o.parent() {
                        let _ = fs::create_dir_all(d);
                    }
                    let filter = format!(
                        "aformat=channel_layouts=mono,showwavespic=s=720x110:colors={colour}"
                    );
                    let _ = std::process::Command::new("ffmpeg")
                        .args(["-v", "error", "-y", "-i"])
                        .arg(&p)
                        .args(["-filter_complex", &filter, "-frames:v", "1"])
                        .arg(&o)
                        .stdin(std::process::Stdio::null())
                        .status();
                }
                let duration = std::process::Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-show_entries",
                        "format=duration",
                        "-of",
                        "csv=p=0",
                    ])
                    .arg(&p)
                    .output()
                    .ok()
                    .and_then(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .trim()
                            .parse::<f64>()
                            .ok()
                    })
                    .unwrap_or(0.0);
                (fs::metadata(&o).is_ok(), duration)
            })
            .await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            let Ok((has_wave, duration)) = made else {
                this.no_preview(&entry);
                return;
            };
            let wave = gtk::Picture::new();
            if has_wave {
                wave.set_filename(Some(&out));
            }
            wave.set_content_fit(gtk::ContentFit::Fill);
            wave.set_can_shrink(true);
            wave.set_size_request(-1, 90);
            wave.add_css_class("wave");
            let playhead = gtk::Box::new(gtk::Orientation::Vertical, 0);
            playhead.add_css_class("playhead");
            playhead.set_halign(gtk::Align::Start);
            playhead.set_visible(false);
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&wave));
            overlay.add_overlay(&playhead);
            let button = gtk::Button::with_label("\u{F040A}  play");
            button.add_css_class("link");
            let time = gtk::Label::new(Some(&format!(
                "0:00 / {}",
                vector_core::details::duration(duration)
            )));
            time.add_css_class("dim");
            let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            bar.append(&button);
            bar.append(&time);
            this.body.append(&overlay);
            this.body.append(&bar);
            *this.player.borrow_mut() = Some(Player {
                process: None,
                from: 0.0,
                started: std::time::Instant::now(),
                duration,
                tick: None,
                playhead,
                wave: wave.clone(),
                button: button.clone(),
                time,
                path,
            });
            let w = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                if let Some(p) = w.upgrade() {
                    p.toggle_audio();
                }
            });
            let click = gtk::GestureClick::new();
            let w = Rc::downgrade(&this);
            click.connect_released(move |g, _, x, _| {
                let (Some(p), Some(widget)) = (w.upgrade(), g.widget()) else {
                    return;
                };
                let width = widget.width().max(1) as f64;
                let dur = p.player.borrow().as_ref().map_or(0.0, |pl| pl.duration);
                p.play_audio_from((x / width).clamp(0.0, 1.0) * dur);
            });
            overlay.add_controller(click);
            overlay.set_cursor_from_name(Some("pointer"));
        });
    }

    fn playing(&self) -> bool {
        self.player
            .borrow()
            .as_ref()
            .is_some_and(|p| p.process.is_some())
    }

    fn position(&self) -> f64 {
        self.player.borrow().as_ref().map_or(0.0, |p| {
            if p.process.is_some() {
                p.from + p.started.elapsed().as_secs_f64()
            } else {
                p.from
            }
        })
    }

    fn toggle_audio(self: &Rc<Self>) {
        if self.playing() {
            let pos = self.position();
            self.kill_player();
            if let Some(p) = self.player.borrow_mut().as_mut() {
                p.from = pos;
                p.button.set_label("\u{F040A}  play");
            }
        } else {
            let from = self.position();
            let dur = self.player.borrow().as_ref().map_or(0.0, |p| p.duration);
            self.play_audio_from(if dur > 0.0 && from >= dur - 0.2 {
                0.0
            } else {
                from
            });
        }
    }

    /// (Re)starts playback at `secs` with mpv, or ffplay when mpv isn't installed.
    fn play_audio_from(self: &Rc<Self>, secs: f64) {
        self.kill_player();
        let Some(path) = self.player.borrow().as_ref().map(|p| p.path.clone()) else {
            return;
        };
        let start = format!("{secs:.2}");
        let mpv = glib::find_program_in_path("mpv").is_some();
        let mut argv: Vec<std::ffi::OsString> = if mpv {
            vec![
                "mpv".into(),
                "--no-video".into(),
                "--really-quiet".into(),
                "--no-terminal".into(),
                format!("--start={start}").into(),
            ]
        } else {
            vec![
                "ffplay".into(),
                "-nodisp".into(),
                "-autoexit".into(),
                "-loglevel".into(),
                "quiet".into(),
                "-ss".into(),
                start.into(),
            ]
        };
        argv.push(path.into_os_string());
        let refs: Vec<&std::ffi::OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
        let Ok(process) = gio::Subprocess::newv(
            &refs,
            gio::SubprocessFlags::STDIN_PIPE
                | gio::SubprocessFlags::STDOUT_SILENCE
                | gio::SubprocessFlags::STDERR_SILENCE,
        ) else {
            return;
        };
        let weak = Rc::downgrade(self);
        let tick = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(p) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            p.update_playhead();
            glib::ControlFlow::Continue
        });
        // when the file ends, the button goes back to play
        let weak = Rc::downgrade(self);
        let done = process.clone();
        process.wait_async(gio::Cancellable::NONE, move |_| {
            let Some(p) = weak.upgrade() else { return };
            let finished = p
                .player
                .borrow()
                .as_ref()
                .and_then(|pl| pl.process.clone())
                .is_some_and(|cur| cur == done);
            if finished {
                let pos = p.position();
                p.kill_player();
                if let Some(pl) = p.player.borrow_mut().as_mut() {
                    pl.from = if pl.duration > 0.0 && pos >= pl.duration - 0.5 {
                        0.0
                    } else {
                        pos
                    };
                    pl.button.set_label("\u{F040A}  play");
                }
                p.update_playhead();
            }
        });
        if let Some(p) = self.player.borrow_mut().as_mut() {
            p.process = Some(process);
            p.from = secs;
            p.started = std::time::Instant::now();
            p.tick = Some(tick);
            p.button.set_label("\u{F03E4}  pause");
            p.playhead.set_visible(true);
        }
        self.update_playhead();
    }

    fn update_playhead(&self) {
        let pos = self.position();
        let pl = self.player.borrow();
        let Some(p) = pl.as_ref() else { return };
        let frac = if p.duration > 0.0 {
            (pos / p.duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        p.playhead
            .set_margin_start((frac * p.wave.width() as f64) as i32);
        p.playhead.set_visible(pos > 0.0);
        p.time.set_text(&format!(
            "{} / {}",
            vector_core::details::duration(pos.min(p.duration.max(pos))),
            vector_core::details::duration(p.duration)
        ));
    }

    fn kill_player(&self) {
        if let Some(p) = self.player.borrow_mut().as_mut() {
            if let Some(proc_) = p.process.take() {
                proc_.force_exit();
            }
            if let Some(t) = p.tick.take() {
                t.remove();
            }
        }
    }

    /// Stops playback (the preview moved on, or closed).
    pub fn stop_audio(&self) {
        self.kill_player();
        self.player.borrow_mut().take();
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

fn ensure_scheme(theme: &vector_core::theme::Theme) -> Option<sourceview5::StyleScheme> {
    let manager = sourceview5::StyleSchemeManager::default();
    if let Some(s) = manager.scheme("vector") {
        return Some(s);
    }
    let dir = glib::user_cache_dir().join("vector/styles");
    if fs::create_dir_all(&dir).is_err() {
        return None;
    }
    if fs::write(dir.join("vector.xml"), scheme_xml(theme)).is_err() {
        return None;
    }
    manager.append_search_path(dir.to_str()?);
    manager.scheme("vector")
}

fn scheme_xml(theme: &vector_core::theme::Theme) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<style-scheme id="vector" name="Vector" version="1.0"><author>vector</author><description>Theme-matched preview</description>
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
    fn folder_size_changes_keep_the_preview() {
        let mut a = Entry::query(std::path::Path::new("/tmp")).expect("/tmp");
        let mut b = a.clone();
        b.size += 4096;
        assert!(same_item(Some(&a), Some(&b)));
        // a file whose size changed is shown again
        a.kind = Kind::File;
        b.kind = Kind::File;
        assert!(!same_item(Some(&a), Some(&b)));
        b.size = a.size;
        b.modified += 1;
        assert!(!same_item(Some(&a), Some(&b)));
        assert!(same_item(None, None));
        assert!(!same_item(Some(&a), None));
    }

    #[test]
    fn detects_text_types() {
        assert!(is_text_type("text/plain"));
        assert!(is_text_type("application/json"));
        assert!(is_text_type(""));
        assert!(!is_text_type("application/pdf"));
    }
    #[test]
    fn scheme_uses_theme_colours() {
        let xml = scheme_xml(&vector_core::theme::Theme::default());
        assert!(xml.contains("id=\"vector\""));
        assert!(xml.contains("#cdd6f480"));
    }
}
