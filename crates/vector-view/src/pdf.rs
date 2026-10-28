//! PDF pages: poppler renders them on a worker thread, one page at a time as they scroll into
//! view; the pages sit in a vertical strip, fitted to the width and zoomable.

use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};

use crate::canvas::Canvas;

#[link(name = "poppler-glib")]
unsafe extern "C" {
    fn poppler_document_new_from_file(
        uri: *const c_char,
        password: *const c_char,
        error: *mut *mut glib::ffi::GError,
    ) -> *mut c_void;
    fn poppler_document_get_n_pages(doc: *mut c_void) -> c_int;
    fn poppler_document_get_page(doc: *mut c_void, index: c_int) -> *mut c_void;
    fn poppler_page_get_size(page: *mut c_void, w: *mut f64, h: *mut f64);
    fn poppler_page_render(page: *mut c_void, cr: *mut c_void);
}

/// An open document. Not shared between threads: each thread opens its own.
pub struct Doc(*mut c_void);

impl Doc {
    pub fn open(path: &Path) -> Result<Doc, String> {
        let uri = gtk::gio::File::for_path(path).uri();
        let uri = CString::new(uri.as_str()).map_err(|e| e.to_string())?;
        let mut err: *mut glib::ffi::GError = std::ptr::null_mut();
        // SAFETY: valid C string; the error, if set, is taken over by glib::Error.
        let doc =
            unsafe { poppler_document_new_from_file(uri.as_ptr(), std::ptr::null(), &mut err) };
        if doc.is_null() {
            let msg = if err.is_null() {
                "can't open this PDF".to_string()
            } else {
                use glib::translate::FromGlibPtrFull;
                unsafe { glib::Error::from_glib_full(err) }
                    .message()
                    .to_string()
            };
            return Err(msg);
        }
        Ok(Doc(doc))
    }

    pub fn pages(&self) -> usize {
        // SAFETY: live document.
        unsafe { poppler_document_get_n_pages(self.0) }.max(0) as usize
    }

    fn page(&self, i: usize) -> Option<Page> {
        // SAFETY: live document; a NULL page means out of range.
        let p = unsafe { poppler_document_get_page(self.0, i as c_int) };
        (!p.is_null()).then_some(Page(p))
    }

    /// Page sizes in points.
    pub fn sizes(&self) -> Vec<(f64, f64)> {
        (0..self.pages())
            .map(|i| self.page(i).map_or((612.0, 792.0), |p| p.size()))
            .collect()
    }

    /// Page `i` drawn at `scale` device pixels per point (at most [`MAX_PIXELS`] on a side), on white: BGRA bytes.
    pub fn render(&self, i: usize, scale: f64) -> Option<Rendered> {
        let page = self.page(i)?;
        let (pw, ph) = page.size();
        // far zoomed in, the page is drawn at most MAX_PIXELS on its long side and scaled up
        let scale = scale.min(MAX_PIXELS / pw.max(ph).max(1.0));
        let (w, h) = ((pw * scale).ceil() as i32, (ph * scale).ceil() as i32);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
        {
            let cr = cairo::Context::new(&surface).ok()?;
            cr.set_source_rgb(1.0, 1.0, 1.0);
            cr.paint().ok()?;
            cr.scale(scale, scale);
            // SAFETY: live page and cairo context for the call.
            unsafe { poppler_page_render(page.0, cr.to_raw_none() as *mut c_void) };
        }
        surface.flush();
        let stride = surface.stride() as usize;
        let data = surface.data().ok()?.to_vec();
        Some(Rendered {
            data,
            width: w,
            height: h,
            stride,
        })
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        // SAFETY: we own one reference.
        unsafe { glib::gobject_ffi::g_object_unref(self.0 as *mut _) };
    }
}

struct Page(*mut c_void);

impl Page {
    fn size(&self) -> (f64, f64) {
        let (mut w, mut h) = (0.0, 0.0);
        // SAFETY: live page.
        unsafe { poppler_page_get_size(self.0, &mut w, &mut h) };
        (w, h)
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        // SAFETY: poppler_document_get_page returns a new reference.
        unsafe { glib::gobject_ffi::g_object_unref(self.0 as *mut _) };
    }
}

pub struct Rendered {
    pub data: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
}

impl Rendered {
    pub fn texture(self) -> gdk::Texture {
        gdk::MemoryTexture::new(
            self.width,
            self.height,
            gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &glib::Bytes::from_owned(self.data),
            self.stride,
        )
        .upcast()
    }
}

/// The long side of a rendered page never exceeds this many pixels (about 250 MB at most).
const MAX_PIXELS: f64 = 8192.0;

/// First page, fitted into `size`×`size` pixels (for thumbnails). Blocking.
pub fn first_page(path: &Path, size: i32) -> Option<Rendered> {
    let doc = Doc::open(path).ok()?;
    let (w, h) = doc.page(0)?.size();
    doc.render(0, size as f64 / w.max(h))
}

enum Job {
    Open(PathBuf, u64),
    Render(usize, f64, u64),
    Close,
}

enum Done {
    Opened(Result<Vec<(f64, f64)>, String>, u64),
    Page(usize, f64, Rendered, u64),
}

/// Of the queued jobs, the ones still worth doing: nothing before the last open or close, and
/// for each page only its latest render.
fn coalesce(jobs: Vec<Job>) -> Vec<Job> {
    let start = jobs
        .iter()
        .rposition(|j| matches!(j, Job::Open(..) | Job::Close))
        .unwrap_or(0);
    let mut out: Vec<Job> = Vec::new();
    for job in jobs.into_iter().skip(start) {
        if let Job::Render(i, ..) = job {
            out.retain(|j| !matches!(j, Job::Render(k, ..) if *k == i));
        }
        out.push(job);
    }
    out
}

/// Top and height of every page in the strip, logical pixels.
fn offsets(sizes: &[(f64, f64)], scale: f64) -> Vec<(f64, f64)> {
    let mut y = GAP as f64;
    sizes
        .iter()
        .map(|(_, h)| {
            let top = y;
            y += h * scale + GAP as f64;
            (top, h * scale)
        })
        .collect()
}

/// For each page: rendered (`Some(true)`), let go (`Some(false)`) or left as it is, for a view
/// from `top` to `bottom`. Pages within one page height of the view are kept drawn; pages more
/// than four away are dropped.
fn wanted(offsets: &[(f64, f64)], top: f64, bottom: f64) -> Vec<Option<bool>> {
    offsets
        .iter()
        .map(|&(y, h)| {
            if y + h >= top - h && y <= bottom + h {
                Some(true)
            } else if y + h < top - 4.0 * h || y > bottom + 4.0 * h {
                Some(false)
            } else {
                None
            }
        })
        .collect()
}

/// Gap between pages, logical pixels.
const GAP: i32 = 14;
/// Margin left and right of the pages when fitted to the width.
const SIDE: f64 = 32.0;
/// Pages never fit wider than this (logical pixels): a page stays readable, not poster-sized.
const MAX_WIDTH: f64 = 920.0;

pub struct PdfView {
    pub widget: gtk::ScrolledWindow,
    column: gtk::Box,
    /// The pages, drawn by the image canvas: sized in logical pixels whatever the texture.
    pages: RefCell<Vec<Canvas>>,
    sizes: RefCell<Vec<(f64, f64)>>,
    /// Device pixels per point each page was last asked for (0 = not drawn).
    asked: RefCell<Vec<f64>>,
    /// Zoom on top of fit-to-width.
    pub zoom: Cell<f64>,
    generation: Cell<u64>,
    /// Where to scroll to (as a fraction of the strip) once a zoom has been laid out.
    restore: Cell<Option<f64>>,
    jobs: std::sync::mpsc::Sender<Job>,
    /// Called when the page at the top, the zoom or the document changes.
    pub on_change: RefCell<Option<Box<dyn Fn()>>>,
    pub error: RefCell<Option<String>>,
}

impl PdfView {
    pub fn new() -> Rc<PdfView> {
        let (jobs, rx) = std::sync::mpsc::channel::<Job>();
        let (done_tx, done_rx) = async_channel::unbounded::<Done>();
        std::thread::spawn(move || {
            let mut doc: Option<Doc> = None;
            let mut current = 0;
            while let Ok(first) = rx.recv() {
                let mut queue = vec![first];
                queue.extend(rx.try_iter());
                for job in coalesce(queue) {
                    match job {
                        Job::Open(path, generation) => {
                            current = generation;
                            doc = None;
                            let r = Doc::open(&path).map(|d| {
                                let s = d.sizes();
                                doc = Some(d);
                                s
                            });
                            let _ = done_tx.send_blocking(Done::Opened(r, generation));
                        }
                        Job::Close => doc = None,
                        Job::Render(i, scale, generation) if generation == current => {
                            if let Some(r) = doc.as_ref().and_then(|d| d.render(i, scale)) {
                                let _ = done_tx.send_blocking(Done::Page(i, scale, r, generation));
                            }
                        }
                        Job::Render(..) => {}
                    }
                }
            }
        });

        let column = gtk::Box::new(gtk::Orientation::Vertical, GAP);
        column.set_halign(gtk::Align::Center);
        column.set_margin_top(GAP);
        column.set_margin_bottom(GAP);
        let widget = gtk::ScrolledWindow::new();
        widget.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        widget.set_child(Some(&column));
        widget.add_css_class("pdf");
        let view = Rc::new(PdfView {
            widget,
            column,
            pages: RefCell::default(),
            sizes: RefCell::default(),
            asked: RefCell::default(),
            zoom: Cell::new(1.0),
            generation: Cell::new(0),
            restore: Cell::new(None),
            jobs,
            on_change: RefCell::default(),
            error: RefCell::default(),
        });
        let weak = Rc::downgrade(&view);
        glib::spawn_future_local(async move {
            while let Ok(done) = done_rx.recv().await {
                let Some(v) = weak.upgrade() else { break };
                v.finished(done);
            }
        });
        let vadj = view.widget.vadjustment();
        let weak = Rc::downgrade(&view);
        vadj.connect_value_changed(move |_| {
            if let Some(v) = weak.upgrade() {
                v.request_visible();
                v.changed();
            }
        });
        // a zoom changes the strip's length: put the view back where it was
        let weak = Rc::downgrade(&view);
        vadj.connect_changed(move |a| {
            if let Some(v) = weak.upgrade()
                && let Some(frac) = v.restore.take()
            {
                a.set_value(frac * a.upper() - a.page_size() / 2.0);
            }
        });
        // the width decides the page size
        let weak = Rc::downgrade(&view);
        view.widget
            .hadjustment()
            .connect_page_size_notify(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.layout();
                }
            });
        view
    }

    pub fn open(&self, path: &Path) {
        self.reset();
        let _ = self
            .jobs
            .send(Job::Open(path.to_path_buf(), self.generation.get()));
    }

    /// Lets the document go (another kind of file is on show).
    pub fn close(&self) {
        if !self.sizes.borrow().is_empty() || self.error.borrow().is_some() {
            self.reset();
            let _ = self.jobs.send(Job::Close);
        }
    }

    fn reset(&self) {
        self.generation.set(self.generation.get() + 1);
        self.zoom.set(1.0);
        self.widget.set_cursor_from_name(Some("default"));
        self.restore.set(None);
        while let Some(c) = self.column.first_child() {
            self.column.remove(&c);
        }
        self.pages.borrow_mut().clear();
        self.sizes.borrow_mut().clear();
        self.asked.borrow_mut().clear();
        *self.error.borrow_mut() = None;
    }

    fn finished(&self, done: Done) {
        match done {
            Done::Opened(r, g) if g == self.generation.get() => match r {
                Ok(sizes) => {
                    for _ in &sizes {
                        let p = Canvas::default();
                        p.set_hexpand(false);
                        p.set_vexpand(false);
                        p.add_css_class("page");
                        self.column.append(&p);
                        self.pages.borrow_mut().push(p);
                    }
                    *self.asked.borrow_mut() = vec![0.0; sizes.len()];
                    *self.sizes.borrow_mut() = sizes;
                    self.widget.vadjustment().set_value(0.0);
                    self.layout();
                    self.changed();
                }
                Err(e) => {
                    *self.error.borrow_mut() = Some(e);
                    self.changed();
                }
            },
            Done::Page(i, scale, r, g) if g == self.generation.get() => {
                let asked = self.asked.borrow().get(i).copied().unwrap_or(0.0);
                // a page let go meanwhile stays empty; an older zoom beats an empty page
                if let Some(p) = self.pages.borrow().get(i)
                    && asked != 0.0
                    && (p.texture().is_none() || asked == scale)
                {
                    p.set_texture(Some(r.texture()), false);
                }
            }
            _ => {}
        }
    }

    fn changed(&self) {
        if let Some(f) = self.on_change.borrow().as_ref() {
            f();
        }
    }

    /// Logical pixels per point.
    fn scale(&self) -> f64 {
        let widest = self
            .sizes
            .borrow()
            .iter()
            .map(|s| s.0)
            .fold(1.0f64, f64::max);
        let w = (self.widget.width() as f64 - SIDE * 2.0).clamp(100.0, MAX_WIDTH);
        (w / widest).min(3.0) * self.zoom.get()
    }

    fn layout(&self) {
        let s = self.scale();
        for (p, (w, h)) in self.pages.borrow().iter().zip(self.sizes.borrow().iter()) {
            p.set_size_request((w * s).round() as i32, (h * s).round() as i32);
        }
        self.request_visible();
    }

    /// Asks for the pages on screen (and one either side) at the current scale; lets pages far
    /// away go, to keep memory flat.
    fn request_visible(&self) {
        let adj = self.widget.vadjustment();
        let s = self.scale();
        let device = s * self.widget.scale_factor() as f64;
        let offsets = offsets(&self.sizes.borrow(), s);
        let pages = self.pages.borrow();
        let mut asked = self.asked.borrow_mut();
        let view = wanted(&offsets, adj.value(), adj.value() + adj.page_size());
        for (i, want) in view.into_iter().enumerate() {
            match want {
                Some(true) if asked[i] != device => {
                    asked[i] = device;
                    let _ = self
                        .jobs
                        .send(Job::Render(i, device, self.generation.get()));
                }
                Some(false) if asked[i] != 0.0 => {
                    asked[i] = 0.0;
                    pages[i].set_texture(None, false);
                }
                _ => {}
            }
        }
    }

    /// (page at the top of the view, 1-based; pages).
    pub fn position(&self) -> (usize, usize) {
        let offsets = offsets(&self.sizes.borrow(), self.scale());
        let mid = self.widget.vadjustment().value() + self.widget.height() as f64 / 3.0;
        let n = offsets.len();
        let i = offsets
            .iter()
            .position(|(y, h)| y + h + GAP as f64 > mid)
            .unwrap_or(n.saturating_sub(1));
        ((i + 1).min(n), n)
    }

    pub fn percent(&self) -> u32 {
        (self.scale() * self.widget.scale_factor() as f64 / 96.0 * 72.0 * 100.0).round() as u32
    }

    /// Whether a drag should move the pages: zoomed in, or wider than the view.
    pub fn can_pan(&self) -> bool {
        let h = self.widget.hadjustment();
        self.zoom.get() > 1.0 || h.upper() > h.page_size() + 1.0
    }

    /// The hand over a page that can be dragged; the normal arrow otherwise.
    pub fn update_cursor(&self) {
        self.widget
            .set_cursor_from_name(Some(if self.can_pan() { "grab" } else { "default" }));
    }

    pub fn set_zoom(&self, z: f64) {
        let adj = self.widget.vadjustment();
        // the middle of the view, as a fraction of the strip
        self.restore.set(Some(
            (adj.value() + adj.page_size() / 2.0) / adj.upper().max(1.0),
        ));
        self.zoom.set(z.clamp(0.25, 8.0));
        self.layout();
        self.update_cursor();
        self.changed();
    }

    pub fn scroll(&self, pages: f64) {
        let adj = self.widget.vadjustment();
        adj.set_value(adj.value() + pages * adj.page_size() * 0.9);
    }

    pub fn scroll_px(&self, px: f64) {
        let adj = self.widget.vadjustment();
        adj.set_value(adj.value() + px);
    }

    pub fn to_start(&self, end: bool) {
        let adj = self.widget.vadjustment();
        adj.set_value(if end { adj.upper() } else { 0.0 });
    }

    /// The page at the top, as drawn (for copying).
    pub fn current_texture(&self) -> Option<gdk::Texture> {
        let (i, _) = self.position();
        self.pages.borrow().get(i.checked_sub(1)?)?.texture()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescing_keeps_the_latest_work() {
        let kinds = |jobs: Vec<Job>| {
            coalesce(jobs)
                .into_iter()
                .map(|j| match j {
                    Job::Open(_, g) => format!("open{g}"),
                    Job::Close => "close".into(),
                    Job::Render(i, s, _) => format!("r{i}@{s}"),
                })
                .collect::<Vec<_>>()
        };
        let p = || PathBuf::from("/x.pdf");
        // renders for an older document go
        assert_eq!(
            kinds(vec![
                Job::Render(0, 1.0, 1),
                Job::Open(p(), 2),
                Job::Render(0, 1.0, 2)
            ]),
            ["open2", "r0@1"]
        );
        // zoom steps: only the last scale per page is drawn
        assert_eq!(
            kinds(vec![
                Job::Render(0, 1.0, 1),
                Job::Render(1, 1.0, 1),
                Job::Render(0, 1.5, 1),
                Job::Render(0, 2.0, 1),
            ]),
            ["r1@1", "r0@2"]
        );
        assert_eq!(kinds(vec![Job::Render(0, 1.0, 1), Job::Close]), ["close"]);
    }

    #[test]
    fn strip_layout_and_visibility() {
        let sizes = vec![(600.0, 800.0); 10];
        let o = offsets(&sizes, 0.5);
        assert_eq!(o[0], (14.0, 400.0));
        assert_eq!(o[1], (428.0, 400.0));
        // view over page 3: pages 2 to 4 drawn (one either side), far ones dropped
        let (top, bottom) = (o[2].0, o[2].0 + 400.0);
        let w = wanted(&o, top, bottom);
        assert_eq!(&w[1..=3], &[Some(true); 3]);
        // two pages away: left as it is
        assert_eq!(w[0], None);
        assert_eq!(w[9], Some(false));
        assert_eq!(w[5], None);
    }
}
