//! The image canvas: draws one texture fitted to the widget, or zoomed around a point and
//! panned. All coordinates are logical pixels; "100 %" means one image pixel per device pixel.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Zoom limits, as image pixels per device pixel.
const MIN_ZOOM: f64 = 0.02;
const MAX_ZOOM: f64 = 64.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Canvas {
        pub texture: RefCell<Option<gdk::Texture>>,
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
            let rect = graphene::Rect::new(
                (w / 2.0 - cx * s) as f32,
                (h / 2.0 - cy * s) as f32,
                (iw * s) as f32,
                (ih * s) as f32,
            );
            let device = s * obj.scale_factor() as f64;
            let filter = if device >= 3.0 {
                gsk::ScalingFilter::Nearest
            } else if device < 1.0 {
                gsk::ScalingFilter::Trilinear
            } else {
                gsk::ScalingFilter::Linear
            };
            snapshot.append_scaled_texture(&tex, filter, &rect);
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
    pub fn set_texture(&self, tex: Option<gdk::Texture>, keep_zoom: bool) {
        self.imp().texture.replace(tex);
        if !keep_zoom {
            self.imp().scale.set(None);
        }
        self.keep_on_screen();
        self.queue_draw();
    }

    pub fn connect_resized(&self, f: impl Fn() + 'static) {
        self.imp().on_resize.replace(Some(Box::new(f)));
    }

    pub fn texture(&self) -> Option<gdk::Texture> {
        self.imp().texture.borrow().clone()
    }

    fn image_size(&self) -> (f64, f64) {
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
