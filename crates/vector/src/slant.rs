//! The statusline's slanted block edges, nvim lualine style (shared with vector-view).

use gtk::prelude::*;

/// A slanted edge between two statusline blocks, drawn in its CSS `color` over its CSS
/// background: `\` after a block on the left, `/` before one on the right.
pub fn slant(right: bool, classes: &[&str]) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("slant");
    for c in classes {
        area.add_css_class(c);
    }
    area.set_content_width(10);
    area.set_draw_func(move |area, cr, w, h| {
        let (w, h) = (w as f64, h as f64);
        cr.set_source_color(&area.color());
        if right {
            cr.move_to(w, 0.0);
            cr.line_to(w, h);
            cr.line_to(0.0, h);
        } else {
            cr.move_to(0.0, 0.0);
            cr.line_to(w, 0.0);
            cr.line_to(0.0, h);
        }
        cr.close_path();
        let _ = cr.fill();
    });
    area
}
