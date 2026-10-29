//! The image canvas: draws one texture fitted to the widget, or zoomed around a point and
//! panned. Also hosts interactive crop selection and in-memory pixel edits (crop, rotate,
//! flip, undo/reset). All coordinates are logical pixels; "100 %" means one image pixel per
//! device pixel.

use std::cell::{Cell, RefCell};
use std::path::Path;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, gdk_pixbuf, glib, graphene, gsk};

/// Zoom limits, as image pixels per device pixel.
const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 64.0;

/// Which part of the crop box a drag is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropDrag {
    New,
    Move,
    N,
    S,
    E,
    W,
    NW,
    NE,
    SW,
    SE,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub original: RefCell<Option<gdk::Texture>>,
        pub undo_stack: RefCell<Vec<gdk::Texture>>,
        pub dirty: Cell<bool>,
        pub editing: Cell<bool>,
        /// Crop rectangle in image pixels: `(x1, y1, x2, y2)` with `x1 <= x2`, `y1 <= y2`.
        pub crop: Cell<Option<(f64, f64, f64, f64)>>,
        pub accent: Cell<Option<gdk::RGBA>>,
        /// `None`: fitted to the widget.
        pub scale: Cell<Option<f64>>,
        /// Image point at the widget's centre (image pixels) while zoomed.
        pub center: Cell<(f64, f64)>,
        /// Called after every resize (the fitted zoom changes with it).
        pub on_resize: RefCell<Option<Box<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Canvas {
        const NAME: &'static str = "VectorViewCanvas";
        type Type = super::Canvas;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Canvas {}

    impl WidgetImpl for Canvas {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            self.obj().keep_on_screen();
            if let Some(f) = self.on_resize.borrow().as_ref() {
                f();
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(tex) = self.texture.borrow().clone() else {
                return;
            };
            let (iw, ih) = (tex.width() as f64, tex.height() as f64);
            let s = obj.scale();
            let (cx, cy) = obj.center();
            let (w, h) = (obj.width() as f64, obj.height() as f64);
            let ix0 = w / 2.0 - cx * s;
            let iy0 = h / 2.0 - cy * s;
            let rect =
                graphene::Rect::new(ix0 as f32, iy0 as f32, (iw * s) as f32, (ih * s) as f32);
            let device = s * obj.scale_factor() as f64;
            let filter = if device >= 3.0 {
                gsk::ScalingFilter::Nearest
            } else if device < 1.0 {
                gsk::ScalingFilter::Trilinear
            } else {
                gsk::ScalingFilter::Linear
            };
            snapshot.append_scaled_texture(&tex, filter, &rect);

            if let Some((x1, y1, x2, y2)) = self.crop.get() {
                let (wx1, wy1) = (ix0 + x1 * s, iy0 + y1 * s);
                let (wx2, wy2) = (ix0 + x2 * s, iy0 + y2 * s);
                let (cw, ch) = ((wx2 - wx1).max(1.0), (wy2 - wy1).max(1.0));
                let dim = gdk::RGBA::new(0.0, 0.0, 0.0, 0.58);
                // Top, bottom, left, right dimmed regions over the image
                let img_r = ix0 + iw * s;
                let img_b = iy0 + ih * s;
                if wy1 > iy0 {
                    snapshot.append_color(
                        &dim,
                        &graphene::Rect::new(
                            ix0 as f32,
                            iy0 as f32,
                            (iw * s) as f32,
                            (wy1 - iy0) as f32,
                        ),
                    );
                }
                if img_b > wy2 {
                    snapshot.append_color(
                        &dim,
                        &graphene::Rect::new(
                            ix0 as f32,
                            wy2 as f32,
                            (iw * s) as f32,
                            (img_b - wy2) as f32,
                        ),
                    );
                }
                if wx1 > ix0 {
                    snapshot.append_color(
                        &dim,
                        &graphene::Rect::new(ix0 as f32, wy1 as f32, (wx1 - ix0) as f32, ch as f32),
                    );
                }
                if img_r > wx2 {
                    snapshot.append_color(
                        &dim,
                        &graphene::Rect::new(
                            wx2 as f32,
                            wy1 as f32,
                            (img_r - wx2) as f32,
                            ch as f32,
                        ),
                    );
                }
                // Rule-of-thirds grid inside crop rectangle
                let grid_col = gdk::RGBA::new(1.0, 1.0, 1.0, 0.26);
                for i in 1..=2 {
                    let gx = wx1 + cw * (i as f64 / 3.0);
                    snapshot.append_color(
                        &grid_col,
                        &graphene::Rect::new(gx as f32, wy1 as f32, 1.0, ch as f32),
                    );
                    let gy = wy1 + ch * (i as f64 / 3.0);
                    snapshot.append_color(
                        &grid_col,
                        &graphene::Rect::new(wx1 as f32, gy as f32, cw as f32, 1.0),
                    );
                }
                // Border & handles
                let acc = self
                    .accent
                    .get()
                    .unwrap_or_else(|| gdk::RGBA::new(0.45, 0.75, 1.0, 1.0));
                let b = 2.0f32;
                snapshot.append_color(
                    &acc,
                    &graphene::Rect::new(wx1 as f32, wy1 as f32, cw as f32, b),
                );
                snapshot.append_color(
                    &acc,
                    &graphene::Rect::new(
                        wx1 as f32,
                        (wy2 as f32 - b).max(wy1 as f32),
                        cw as f32,
                        b,
                    ),
                );
                snapshot.append_color(
                    &acc,
                    &graphene::Rect::new(wx1 as f32, wy1 as f32, b, ch as f32),
                );
                snapshot.append_color(
                    &acc,
                    &graphene::Rect::new(
                        (wx2 as f32 - b).max(wx1 as f32),
                        wy1 as f32,
                        b,
                        ch as f32,
                    ),
                );
                let hs = 8.0f32;
                let hh = hs / 2.0;
                let mx = (wx1 + wx2) / 2.0;
                let my = (wy1 + wy2) / 2.0;
                for (hx, hy) in [
                    (wx1, wy1),
                    (wx2, wy1),
                    (wx1, wy2),
                    (wx2, wy2),
                    (mx, wy1),
                    (mx, wy2),
                    (wx1, my),
                    (wx2, my),
                ] {
                    snapshot.append_color(
                        &acc,
                        &graphene::Rect::new(hx as f32 - hh, hy as f32 - hh, hs, hs),
                    );
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct Canvas(ObjectSubclass<imp::Canvas>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Canvas {
    fn default() -> Self {
        let c: Canvas = glib::Object::new();
        c.set_hexpand(true);
        c.set_vexpand(true);
        c.set_overflow(gtk::Overflow::Hidden);
        c
    }
}

impl Canvas {
    pub fn set_accent(&self, rgba: gdk::RGBA) {
        self.imp().accent.set(Some(rgba));
    }

    pub fn set_texture(&self, tex: Option<gdk::Texture>, keep_zoom: bool) {
        self.imp().original.replace(tex.clone());
        self.imp().undo_stack.borrow_mut().clear();
        self.imp().dirty.set(false);
        self.imp().crop.set(None);
        self.imp().texture.replace(tex);
        if !keep_zoom {
            self.imp().scale.set(None);
        }
        self.keep_on_screen();
        self.queue_draw();
    }

    fn replace_edited_texture(&self, tex: gdk::Texture, reset_zoom: bool) {
        if let Some(prev) = self.imp().texture.replace(Some(tex)) {
            self.imp().undo_stack.borrow_mut().push(prev);
        }
        self.imp().dirty.set(true);
        self.imp().crop.set(None);
        if reset_zoom {
            self.imp().scale.set(None);
        }
        self.keep_on_screen();
        self.queue_draw();
    }

    pub fn is_dirty(&self) -> bool {
        self.imp().dirty.get()
    }

    pub fn can_undo(&self) -> bool {
        !self.imp().undo_stack.borrow().is_empty()
    }

    pub fn mark_saved(&self) {
        self.imp().original.replace(self.texture());
        self.imp().undo_stack.borrow_mut().clear();
        self.imp().dirty.set(false);
        self.imp().crop.set(None);
        self.queue_draw();
    }

    pub fn is_editing(&self) -> bool {
        self.imp().editing.get()
    }

    pub fn set_editing(&self, on: bool) {
        self.imp().editing.set(on);
        if !on {
            self.imp().crop.set(None);
        }
        self.queue_draw();
    }

    pub fn crop(&self) -> Option<(f64, f64, f64, f64)> {
        self.imp().crop.get()
    }

    pub fn crop_dimensions(&self) -> Option<(u32, u32)> {
        let (x1, y1, x2, y2) = self.imp().crop.get()?;
        let w = (x2.round() - x1.round()).max(0.0) as u32;
        let h = (y2.round() - y1.round()).max(0.0) as u32;
        (w >= 2 && h >= 2).then_some((w, h))
    }

    pub fn set_crop(&self, rect: Option<(f64, f64, f64, f64)>) {
        let clamped = rect.and_then(|(ax, ay, bx, by)| {
            let (iw, ih) = self.image_size();
            let x1 = ax.min(bx).clamp(0.0, iw);
            let x2 = ax.max(bx).clamp(0.0, iw);
            let y1 = ay.min(by).clamp(0.0, ih);
            let y2 = ay.max(by).clamp(0.0, ih);
            (x2 - x1 >= 2.0 && y2 - y1 >= 2.0).then_some((x1, y1, x2, y2))
        });
        self.imp().crop.set(clamped);
        self.queue_draw();
    }

    /// Starts a default centered crop box covering 80% of the image if none is active.
    pub fn init_default_crop(&self) {
        if self.imp().crop.get().is_none() && self.texture().is_some() {
            let (iw, ih) = self.image_size();
            let (mx, my) = (iw * 0.1, ih * 0.1);
            self.set_crop(Some((mx, my, iw - mx, ih - my)));
        }
    }

    /// Converts widget logical coordinates `(wx, wy)` to image pixel coordinates.
    pub fn widget_to_image(&self, wx: f64, wy: f64) -> (f64, f64) {
        let (iw, ih) = self.image_size();
        let s = self.scale();
        let (cx, cy) = self.center();
        let (w, h) = (self.width() as f64, self.height() as f64);
        let ix = (cx + (wx - w / 2.0) / s).clamp(0.0, iw);
        let iy = (cy + (wy - h / 2.0) / s).clamp(0.0, ih);
        (ix, iy)
    }

    /// Hit-tests widget coordinates `(wx, wy)` against the current crop rectangle.
    pub fn hit_crop(&self, wx: f64, wy: f64) -> CropDrag {
        let Some((x1, y1, x2, y2)) = self.imp().crop.get() else {
            return CropDrag::New;
        };
        let s = self.scale();
        let (cx, cy) = self.center();
        let (w, h) = (self.width() as f64, self.height() as f64);
        let ix0 = w / 2.0 - cx * s;
        let iy0 = h / 2.0 - cy * s;
        let (wx1, wy1) = (ix0 + x1 * s, iy0 + y1 * s);
        let (wx2, wy2) = (ix0 + x2 * s, iy0 + y2 * s);
        let tol = 12.0;
        let near_l = (wx - wx1).abs() <= tol;
        let near_r = (wx - wx2).abs() <= tol;
        let near_t = (wy - wy1).abs() <= tol;
        let near_b = (wy - wy2).abs() <= tol;
        let in_x = wx >= wx1 - tol && wx <= wx2 + tol;
        let in_y = wy >= wy1 - tol && wy <= wy2 + tol;
        match (near_l, near_r, near_t, near_b) {
            (true, _, true, _) => CropDrag::NW,
            (_, true, true, _) => CropDrag::NE,
            (true, _, _, true) => CropDrag::SW,
            (_, true, _, true) => CropDrag::SE,
            (true, _, _, _) if in_y => CropDrag::W,
            (_, true, _, _) if in_y => CropDrag::E,
            (_, _, true, _) if in_x => CropDrag::N,
            (_, _, _, true) if in_x => CropDrag::S,
            _ if wx > wx1 && wx < wx2 && wy > wy1 && wy < wy2 => CropDrag::Move,
            _ => CropDrag::New,
        }
    }

    fn download_pixels(&self) -> Option<vector_core::image::Pixels> {
        let tex = self.texture()?;
        let (w, h) = (tex.width() as u32, tex.height() as u32);
        if w == 0 || h == 0 {
            return None;
        }
        let mut dl = gdk::TextureDownloader::new(&tex);
        dl.set_format(gdk::MemoryFormat::R8g8b8a8);
        let (bytes, stride) = dl.download_bytes();
        let src = bytes.as_ref();
        let row_bytes = w as usize * 4;
        let mut rgba = Vec::with_capacity(row_bytes * h as usize);
        for y in 0..h as usize {
            let start = y * stride;
            rgba.extend_from_slice(&src[start..start + row_bytes]);
        }
        Some(vector_core::image::Pixels {
            width: w,
            height: h,
            rgba,
        })
    }

    fn pixels_to_texture(px: vector_core::image::Pixels) -> gdk::Texture {
        let stride = px.width as usize * 4;
        gdk::MemoryTexture::new(
            px.width as i32,
            px.height as i32,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(px.rgba),
            stride,
        )
        .upcast()
    }

    /// Crops the image to the active crop rectangle. Returns whether a crop was applied.
    pub fn apply_crop(&self) -> bool {
        let Some((fx1, fy1, fx2, fy2)) = self.imp().crop.get() else {
            return false;
        };
        let Some(px) = self.download_pixels() else {
            return false;
        };
        let x1 = (fx1.round() as u32).min(px.width.saturating_sub(1));
        let y1 = (fy1.round() as u32).min(px.height.saturating_sub(1));
        let x2 = (fx2.round() as u32).clamp(x1 + 1, px.width);
        let y2 = (fy2.round() as u32).clamp(y1 + 1, px.height);
        let (nw, nh) = (x2 - x1, y2 - y1);
        if nw < 2 || nh < 2 || (nw == px.width && nh == px.height) {
            self.imp().crop.set(None);
            self.queue_draw();
            return false;
        }
        let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
        for y in y1..y2 {
            let start = (y as usize * px.width as usize + x1 as usize) * 4;
            let end = start + nw as usize * 4;
            out.extend_from_slice(&px.rgba[start..end]);
        }
        let cropped = vector_core::image::Pixels {
            width: nw,
            height: nh,
            rgba: out,
        };
        self.replace_edited_texture(Self::pixels_to_texture(cropped), true);
        true
    }

    /// Rotates the image 90° clockwise (`cw = true`) or counter-clockwise (`cw = false`).
    pub fn rotate(&self, cw: bool) -> bool {
        let Some(px) = self.download_pixels() else {
            return false;
        };
        let rotated = vector_core::image::orient(px, if cw { 6 } else { 8 });
        self.replace_edited_texture(Self::pixels_to_texture(rotated), true);
        true
    }

    /// Flips the image horizontally (`horizontal = true`) or vertically (`horizontal = false`).
    pub fn flip(&self, horizontal: bool) -> bool {
        let Some(px) = self.download_pixels() else {
            return false;
        };
        let flipped = vector_core::image::orient(px, if horizontal { 2 } else { 4 });
        self.replace_edited_texture(Self::pixels_to_texture(flipped), false);
        true
    }

    pub fn undo(&self) -> bool {
        let Some(prev) = self.imp().undo_stack.borrow_mut().pop() else {
            return false;
        };
        let still_dirty = !self.imp().undo_stack.borrow().is_empty();
        self.imp().texture.replace(Some(prev));
        self.imp().dirty.set(still_dirty);
        self.imp().crop.set(None);
        self.imp().scale.set(None);
        self.keep_on_screen();
        self.queue_draw();
        true
    }

    pub fn reset_edits(&self) -> bool {
        if !self.is_dirty() && self.crop().is_none() {
            return false;
        }
        if let Some(orig) = self.imp().original.borrow().clone() {
            self.imp().texture.replace(Some(orig));
        }
        self.imp().undo_stack.borrow_mut().clear();
        self.imp().dirty.set(false);
        self.imp().crop.set(None);
        self.imp().scale.set(None);
        self.keep_on_screen();
        self.queue_draw();
        true
    }

    /// Writes the current texture to `dest` atomically (using the format implied by `dest`'s
    /// extension, falling back to PNG).
    pub fn save_to_path(&self, dest: &Path) -> Result<(), String> {
        let px = self
            .download_pixels()
            .ok_or_else(|| "no image loaded".to_string())?;
        let parent = dest.parent().unwrap_or(Path::new("."));
        let file_name = dest
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("image.png");
        let tmp = parent.join(format!(".{file_name}.vector-tmp-{}", std::process::id()));
        let ext = dest
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("png")
            .to_lowercase();
        let write_res: Result<(), String> = if ext == "png" {
            let tex = self.texture().ok_or_else(|| "no texture".to_string())?;
            tex.save_to_png(&tmp).map_err(|e| e.to_string())
        } else {
            let fmt = match ext.as_str() {
                "jpg" | "jpeg" => "jpeg",
                "webp" => "webp",
                "bmp" => "bmp",
                "tif" | "tiff" => "tiff",
                "ico" => "ico",
                _ => "png",
            };
            let bytes = glib::Bytes::from_owned(px.rgba);
            let pb = gdk_pixbuf::Pixbuf::from_bytes(
                &bytes,
                gdk_pixbuf::Colorspace::Rgb,
                true,
                8,
                px.width as i32,
                px.height as i32,
                px.width as i32 * 4,
            );
            let opts: &[(&str, &str)] = if fmt == "jpeg" {
                &[("quality", "95")]
            } else {
                &[]
            };
            pb.savev(&tmp, fmt, opts).map_err(|e| e.to_string())
        };
        if let Err(e) = write_res {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, dest).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            e.to_string()
        })
    }

    pub fn connect_resized(&self, f: impl Fn() + 'static) {
        self.imp().on_resize.replace(Some(Box::new(f)));
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        self.imp().texture.borrow().clone()
    }

    pub fn image_size(&self) -> (f64, f64) {
        self.imp()
            .texture
            .borrow()
            .as_ref()
            .map_or((1.0, 1.0), |t| (t.width() as f64, t.height() as f64))
    }

    fn fit_scale(&self) -> f64 {
        let (iw, ih) = self.image_size();
        let (w, h) = (self.width().max(1) as f64, self.height().max(1) as f64);
        (w / iw).min(h / ih)
    }

    /// Logical pixels per image pixel.
    pub fn scale(&self) -> f64 {
        self.imp().scale.get().unwrap_or_else(|| self.fit_scale())
    }

    pub fn is_fitted(&self) -> bool {
        self.imp().scale.get().is_none()
    }

    /// Zoom as shown to the user: image pixels per device pixel, in percent.
    pub fn percent(&self) -> u32 {
        (self.scale() * self.scale_factor() as f64 * 100.0).round() as u32
    }

    fn center(&self) -> (f64, f64) {
        if self.is_fitted() {
            let (iw, ih) = self.image_size();
            (iw / 2.0, ih / 2.0)
        } else {
            self.imp().center.get()
        }
    }

    /// Keeps the picture on screen: centred on an axis where it is smaller than the widget,
    /// edges no further in than the widget's edges where it is larger.
    pub(crate) fn keep_on_screen(&self) {
        if self.is_fitted() {
            return;
        }
        let (iw, ih) = self.image_size();
        let s = self.scale();
        let (w, h) = (self.width() as f64, self.height() as f64);
        let (mut cx, mut cy) = self.imp().center.get();
        let axis = |c: f64, view: f64, size: f64| {
            let half = view / 2.0 / s;
            if size * s <= view {
                size / 2.0
            } else {
                c.clamp(half, size - half)
            }
        };
        cx = axis(cx, w, iw);
        cy = axis(cy, h, ih);
        self.imp().center.set((cx, cy));
    }

    pub fn fit(&self) {
        self.imp().scale.set(None);
        self.queue_draw();
    }

    /// Zooms by `factor` keeping the image point under widget point (`x`, `y`) in place.
    pub fn zoom_at(&self, factor: f64, x: f64, y: f64) {
        if self.texture().is_none() {
            return;
        }
        let sf = self.scale_factor() as f64;
        let old = self.scale();
        let new = (old * factor).clamp(MIN_ZOOM / sf, MAX_ZOOM / sf);
        let (cx, cy) = self.center();
        let (w, h) = (self.width() as f64, self.height() as f64);
        let (dx, dy) = (x - w / 2.0, y - h / 2.0);
        let (px, py) = (cx + dx / old, cy + dy / old);
        self.imp().center.set((px - dx / new, py - dy / new));
        self.imp().scale.set(Some(new));
        self.keep_on_screen();
        self.queue_draw();
    }

    pub fn zoom(&self, factor: f64) {
        self.zoom_at(
            factor,
            self.width() as f64 / 2.0,
            self.height() as f64 / 2.0,
        );
    }

    /// One image pixel per device pixel.
    pub fn actual_size(&self) {
        let target = 1.0 / self.scale_factor() as f64;
        self.zoom(target / self.scale());
    }

    /// Moves the picture by (`dx`, `dy`) logical pixels.
    pub fn pan(&self, dx: f64, dy: f64) {
        if self.is_fitted() {
            // panning a fitted picture starts zoomed at the fitted size
            let (iw, ih) = self.image_size();
            self.imp().center.set((iw / 2.0, ih / 2.0));
            self.imp().scale.set(Some(self.fit_scale()));
        }
        let s = self.scale();
        let (cx, cy) = self.imp().center.get();
        self.imp().center.set((cx - dx / s, cy - dy / s));
        self.keep_on_screen();
        self.queue_draw();
    }

    /// Whether the picture is larger than the widget (so dragging moves it).
    pub fn can_pan(&self) -> bool {
        let (iw, ih) = self.image_size();
        let s = self.scale();
        iw * s > self.width() as f64 + 0.5 || ih * s > self.height() as f64 + 0.5
    }
}
