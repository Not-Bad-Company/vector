//! Colour formats for the picker: hex, rgb() and hsl() strings.

/// The format that is copied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Hex,
    Rgb,
    Hsl,
}

pub fn hex(r: u8, g: u8, b: u8) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

pub fn rgb(r: u8, g: u8, b: u8) -> String {
    format!("rgb({r}, {g}, {b})")
}

/// CSS hsl(): hue in degrees, saturation and lightness in percent, rounded.
pub fn hsl(r: u8, g: u8, b: u8) -> String {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d == 0.0 {
        (0.0, 0.0)
    } else {
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        let h = if max == r {
            ((g - b) / d).rem_euclid(6.0)
        } else if max == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        (h * 60.0, s)
    };
    format!("hsl({:.0}, {:.0}%, {:.0}%)", h.round() % 360.0, (s * 100.0).round(), (l * 100.0).round())
}

/// The string copied for a colour in `format`.
pub fn format(format: Format, r: u8, g: u8, b: u8) -> String {
    match format {
        Format::Hex => hex(r, g, b),
        Format::Rgb => rgb(r, g, b),
        Format::Hsl => hsl(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(hex(129, 161, 193), "#81a1c1");
        assert_eq!(rgb(129, 161, 193), "rgb(129, 161, 193)");
        assert_eq!(hsl(255, 0, 0), "hsl(0, 100%, 50%)");
        assert_eq!(hsl(129, 161, 193), "hsl(210, 34%, 63%)");
        assert_eq!(hsl(128, 128, 128), "hsl(0, 0%, 50%)");
    }
}
