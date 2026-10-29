//! Colours: read hex, rgb()/rgba(), hsl()/hsla() or Hyprland's rgba(rrggbbaa)
//! and 0xaarrggbb, and offer every other notation to copy. `#` mode, and
//! inline in the app search when what you typed is a colour.

use crate::item::{Action, Icon, Item};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// 0..=1
    pub a: f64,
}

fn hex_digits(s: &str) -> Option<Rgba> {
    if !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let x = |i: usize, n: usize| u8::from_str_radix(&s[i..i + n], 16).ok();
    let dup = |v: u8| v * 17;
    Some(match s.len() {
        3 => Rgba { r: dup(x(0, 1)?), g: dup(x(1, 1)?), b: dup(x(2, 1)?), a: 1.0 },
        4 => Rgba { r: dup(x(0, 1)?), g: dup(x(1, 1)?), b: dup(x(2, 1)?), a: dup(x(3, 1)?) as f64 / 255.0 },
        6 => Rgba { r: x(0, 2)?, g: x(2, 2)?, b: x(4, 2)?, a: 1.0 },
        8 => Rgba { r: x(0, 2)?, g: x(2, 2)?, b: x(4, 2)?, a: x(6, 2)? as f64 / 255.0 },
        _ => return None,
    })
}

/// Numbers inside "name( ... )", split on commas, spaces or slashes.
fn args<'a>(s: &'a str, names: &[&str]) -> Option<Vec<&'a str>> {
    let (name, rest) = s.split_once('(')?;
    if !names.contains(&name.trim()) {
        return None;
    }
    let inner = rest.trim_end().strip_suffix(')')?;
    Some(inner.split([',', ' ', '/']).map(str::trim).filter(|p| !p.is_empty()).collect())
}

fn channel(s: &str) -> Option<u8> {
    match s.strip_suffix('%') {
        Some(p) => Some((p.parse::<f64>().ok()?.clamp(0.0, 100.0) * 2.55).round() as u8),
        None => {
            let v = s.parse::<f64>().ok()?;
            (0.0..=255.0).contains(&v).then_some(v.round() as u8)
        }
    }
}

fn alpha(s: &str) -> Option<f64> {
    match s.strip_suffix('%') {
        Some(p) => Some((p.parse::<f64>().ok()? / 100.0).clamp(0.0, 1.0)),
        None => {
            let v = s.parse::<f64>().ok()?;
            (0.0..=1.0).contains(&v).then_some(v)
        }
    }
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let f = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (f(r), f(g), f(b))
}

pub fn parse(src: &str) -> Option<Rgba> {
    let s = src.trim().to_lowercase();
    if let Some(h) = s.strip_prefix('#') {
        return hex_digits(h);
    }
    if let Some(h) = s.strip_prefix("0x") {
        // Hyprland's legacy 0xaarrggbb
        return match h.len() {
            8 => {
                let c = hex_digits(&format!("{}{}", &h[2..], &h[..2]))?;
                Some(c)
            }
            6 => hex_digits(h),
            _ => None,
        };
    }
    if let Some(a) = args(&s, &["rgb", "rgba"]) {
        // Hyprland's rgba(rrggbbaa) / rgb(rrggbb)
        if a.len() == 1 {
            return hex_digits(a[0]);
        }
        if !(3..=4).contains(&a.len()) {
            return None;
        }
        return Some(Rgba {
            r: channel(a[0])?,
            g: channel(a[1])?,
            b: channel(a[2])?,
            a: a.get(3).map(|x| alpha(x)).unwrap_or(Some(1.0))?,
        });
    }
    if let Some(a) = args(&s, &["hsl", "hsla"]) {
        if !(3..=4).contains(&a.len()) {
            return None;
        }
        let h = a[0].trim_end_matches("deg").parse::<f64>().ok()?;
        let sat = a[1].trim_end_matches('%').parse::<f64>().ok()? / 100.0;
        let l = a[2].trim_end_matches('%').parse::<f64>().ok()? / 100.0;
        if !(0.0..=1.0).contains(&sat) || !(0.0..=1.0).contains(&l) {
            return None;
        }
        let (r, g, b) = hsl_to_rgb(h, sat, l);
        return Some(Rgba { r, g, b, a: a.get(3).map(|x| alpha(x)).unwrap_or(Some(1.0))? });
    }
    None
}

/// A colour in `#` mode, where the hash is optional.
pub fn parse_loose(src: &str) -> Option<Rgba> {
    let s = src.trim();
    parse(s).or_else(|| if s.len() >= 3 { hex_digits(&s.to_lowercase()) } else { None })
}

fn hsl(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l * 100.0);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s * 100.0, l * 100.0)
}

fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Every notation of a colour: (text, label).
pub fn formats(c: Rgba) -> Vec<(String, &'static str)> {
    let a8 = (c.a * 255.0).round() as u8;
    let (h, s, l) = hsl(c);
    let mut v = vec![
        (format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b), "hex"),
        (format!("rgb({}, {}, {})", c.r, c.g, c.b), "css rgb"),
        (format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, num(c.a)), "css rgba"),
        (format!("hsl({}, {}%, {}%)", h.round(), s.round(), l.round()), "css hsl"),
        (format!("rgba({:02x}{:02x}{:02x}{:02x})", c.r, c.g, c.b, a8), "hyprland"),
        (format!("0x{:02x}{:02x}{:02x}{:02x}", a8, c.r, c.g, c.b), "hyprland legacy / argb"),
        (
            format!("{}, {}, {}, {}", num(c.r as f64 / 255.0), num(c.g as f64 / 255.0), num(c.b as f64 / 255.0), num(c.a)),
            "float",
        ),
    ];
    if c.a < 1.0 {
        v.insert(1, (format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, a8), "hex with alpha"));
        v.insert(5, (format!("hsla({}, {}%, {}%, {})", h.round(), s.round(), l.round(), num(c.a)), "css hsla"));
    }
    v
}

pub fn items(c: Rgba) -> Vec<Item> {
    formats(c)
        .into_iter()
        .map(|(text, label)| Item::new(text.clone(), Icon::Color(c), Action::Copy(text)).sub(label).meta("⏎ copy"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let red = Rgba { r: 255, g: 0, b: 0, a: 1.0 };
        assert_eq!(parse("#ff0000"), Some(red));
        assert_eq!(parse("#F00"), Some(red));
        assert_eq!(parse("rgb(255, 0, 0)"), Some(red));
        assert_eq!(parse("rgb(100% 0% 0%)"), Some(red));
        assert_eq!(parse("hsl(0, 100%, 50%)"), Some(red));
        assert_eq!(parse("rgba(ff000080)").map(|c| (c.r, (c.a * 255.0).round() as u8)), Some((255, 128)));
        assert_eq!(parse("0x80ff0000").map(|c| (c.r, c.g, (c.a * 255.0).round() as u8)), Some((255, 0, 128)));
        assert_eq!(parse("rgba(1, 2, 3, 0.5)").map(|c| c.a), Some(0.5));
        assert_eq!(parse("firefox"), None);
        assert_eq!(parse("#ggg"), None);
        assert_eq!(parse_loose("c9a467").map(|c| c.r), Some(0xc9));
        assert_eq!(parse_loose("ab"), None);
    }

    #[test]
    fn formats_round_trip() {
        let c = parse("#c9a467").unwrap();
        for (text, _) in formats(c) {
            if text.contains(", ") && !text.contains('(') {
                continue; // floats
            }
            let back = parse(&text).unwrap_or_else(|| panic!("{text}"));
            assert!((back.r as i32 - c.r as i32).abs() <= 2, "{text}");
        }
    }
}
