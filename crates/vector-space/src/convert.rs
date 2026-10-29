//! `>`: convert anything to anything. A colour to its other notations, a
//! quantity to other units, a unix time to a date, and files: copy a file (or
//! a screenshot) and pick the format; the result lands next to the original,
//! at the highest quality the format allows, and goes on the clipboard.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::clip::{Clip, Kind};
use crate::color;
use crate::item::{Action, Icon, Item};
use crate::util::{self, Fuzzy};

// ── units ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Dim {
    Length,
    Mass,
    Temp,
    Data,
    Time,
    Speed,
    Volume,
    Area,
}

/// (names, dimension, factor to the base unit, shown in the list)
const UNITS: &[(&[&str], Dim, f64, bool)] = &[
    (&["nm", "nanometer", "nanometers"], Dim::Length, 1e-9, false),
    (&["um", "µm", "micrometer", "micron"], Dim::Length, 1e-6, false),
    (&["mm", "millimeter", "millimeters", "millimetre"], Dim::Length, 1e-3, true),
    (&["cm", "centimeter", "centimeters", "centimetre"], Dim::Length, 1e-2, true),
    (&["m", "meter", "meters", "metre", "metres"], Dim::Length, 1.0, true),
    (&["km", "kilometer", "kilometers", "kilometre"], Dim::Length, 1e3, true),
    (&["in", "inch", "inches", "\""], Dim::Length, 0.0254, true),
    (&["ft", "foot", "feet", "'"], Dim::Length, 0.3048, true),
    (&["yd", "yard", "yards"], Dim::Length, 0.9144, false),
    (&["mi", "mile", "miles"], Dim::Length, 1609.344, true),
    (&["nmi", "nautical mile"], Dim::Length, 1852.0, false),
    (&["mg", "milligram", "milligrams"], Dim::Mass, 1e-6, false),
    (&["g", "gram", "grams"], Dim::Mass, 1e-3, true),
    (&["kg", "kilo", "kilos", "kilogram", "kilograms"], Dim::Mass, 1.0, true),
    (&["t", "ton", "tonne", "tonnes"], Dim::Mass, 1e3, false),
    (&["oz", "ounce", "ounces"], Dim::Mass, 0.028349523125, true),
    (&["lb", "lbs", "pound", "pounds"], Dim::Mass, 0.45359237, true),
    (&["st", "stone"], Dim::Mass, 6.35029318, false),
    (&["c", "°c", "celsius"], Dim::Temp, 0.0, true),
    (&["f", "°f", "fahrenheit"], Dim::Temp, 1.0, true),
    (&["k", "kelvin"], Dim::Temp, 2.0, true),
    (&["bit", "bits"], Dim::Data, 0.125, false),
    (&["byte", "bytes", "B"], Dim::Data, 1.0, true),
    (&["kb", "kilobyte", "kilobytes"], Dim::Data, 1e3, true),
    (&["kib", "kibibyte"], Dim::Data, 1024.0, true),
    (&["mb", "megabyte", "megabytes"], Dim::Data, 1e6, true),
    (&["mib", "mebibyte"], Dim::Data, 1048576.0, true),
    (&["gb", "gigabyte", "gigabytes"], Dim::Data, 1e9, true),
    (&["gib", "gibibyte"], Dim::Data, 1073741824.0, true),
    (&["tb", "terabyte", "terabytes"], Dim::Data, 1e12, true),
    (&["tib", "tebibyte"], Dim::Data, 1099511627776.0, false),
    (&["mbit", "mbits", "megabit", "megabits"], Dim::Data, 125000.0, true),
    (&["gbit", "gigabit"], Dim::Data, 125000000.0, false),
    (&["ms", "millisecond", "milliseconds"], Dim::Time, 1e-3, true),
    (&["s", "sec", "secs", "second", "seconds"], Dim::Time, 1.0, true),
    (&["min", "mins", "minute", "minutes"], Dim::Time, 60.0, true),
    (&["h", "hr", "hrs", "hour", "hours"], Dim::Time, 3600.0, true),
    (&["d", "day", "days"], Dim::Time, 86400.0, true),
    (&["wk", "week", "weeks"], Dim::Time, 604800.0, true),
    (&["yr", "year", "years"], Dim::Time, 31557600.0, true),
    (&["m/s", "mps"], Dim::Speed, 1.0, true),
    (&["km/h", "kmh", "kph"], Dim::Speed, 1.0 / 3.6, true),
    (&["mph"], Dim::Speed, 0.44704, true),
    (&["kn", "knot", "knots"], Dim::Speed, 0.514444, true),
    (&["ml", "milliliter", "millilitre"], Dim::Volume, 1e-3, true),
    (&["cl", "centiliter"], Dim::Volume, 1e-2, false),
    (&["l", "liter", "liters", "litre", "litres"], Dim::Volume, 1.0, true),
    (&["m3", "m³", "cubic meter"], Dim::Volume, 1e3, false),
    (&["gal", "gallon", "gallons"], Dim::Volume, 3.785411784, true),
    (&["qt", "quart", "quarts"], Dim::Volume, 0.946352946, false),
    (&["pt", "pint", "pints"], Dim::Volume, 0.473176473, false),
    (&["cup", "cups"], Dim::Volume, 0.2365882365, true),
    (&["floz", "fl oz"], Dim::Volume, 0.0295735295625, true),
    (&["tbsp", "tablespoon", "tablespoons"], Dim::Volume, 0.01478676478125, false),
    (&["tsp", "teaspoon", "teaspoons"], Dim::Volume, 0.00492892159375, false),
    (&["cm2", "cm²"], Dim::Area, 1e-4, false),
    (&["m2", "m²", "sqm"], Dim::Area, 1.0, true),
    (&["km2", "km²"], Dim::Area, 1e6, true),
    (&["ha", "hectare", "hectares"], Dim::Area, 1e4, true),
    (&["acre", "acres"], Dim::Area, 4046.8564224, true),
    (&["ft2", "ft²", "sqft"], Dim::Area, 0.09290304, true),
];

fn unit(name: &str) -> Option<usize> {
    // "B" (bytes) is the one case-sensitive name; "b" alone means nothing
    UNITS.iter().position(|(names, ..)| names.contains(&name))
        .or_else(|| {
            let l = name.to_lowercase();
            UNITS.iter().position(|(names, ..)| names.iter().any(|n| *n != "B" && n.to_lowercase() == l))
        })
}

fn to_base(i: usize, v: f64) -> f64 {
    let (_, d, f, _) = UNITS[i];
    if d == Dim::Temp {
        match f as u8 {
            0 => v + 273.15,
            1 => (v - 32.0) * 5.0 / 9.0 + 273.15,
            _ => v,
        }
    } else {
        v * f
    }
}

fn from_base(i: usize, v: f64) -> f64 {
    let (_, d, f, _) = UNITS[i];
    if d == Dim::Temp {
        match f as u8 {
            0 => v - 273.15,
            1 => (v - 273.15) * 9.0 / 5.0 + 32.0,
            _ => v,
        }
    } else {
        v / f
    }
}

/// "12.5 km", "100f to c", "3gib in mb", "70 kg > lb".
fn parse_quantity(q: &str) -> Option<(f64, usize, Option<usize>)> {
    let q = q.trim();
    let num_end = q
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || *c == ',' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map(|(i, _)| i)?;
    if num_end == 0 {
        return None;
    }
    let v: f64 = q[..num_end].replace(',', ".").parse().ok()?;
    let rest = q[num_end..].trim();
    let (from, to) = match rest
        .split_once(" to ")
        .or_else(|| rest.split_once(" in "))
        .or_else(|| rest.split_once("->"))
        .or_else(|| rest.split_once('>'))
        .or_else(|| rest.split_once(" as "))
    {
        Some((a, b)) => (a.trim(), Some(b.trim())),
        None => (rest, None),
    };
    let fi = unit(from)?;
    let ti = match to {
        Some(t) if !t.is_empty() => {
            let ti = unit(t)?;
            (UNITS[ti].1 == UNITS[fi].1).then_some(ti)?;
            Some(ti)
        }
        _ => None,
    };
    Some((v, fi, ti))
}

fn fmt_num(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-4..1e15).contains(&a) {
        return format!("{v:.4e}");
    }
    // six significant digits, no trailing zeros
    let decimals = (5 - a.log10().floor() as i32).clamp(0, 10) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

fn unit_items(v: f64, fi: usize, ti: Option<usize>) -> Vec<Item> {
    let base = to_base(fi, v);
    let dim = UNITS[fi].1;
    let mut order: Vec<usize> = ti.into_iter().collect();
    order.extend((0..UNITS.len()).filter(|&i| UNITS[i].1 == dim && UNITS[i].3 && i != fi && Some(i) != ti));
    order
        .into_iter()
        .map(|i| {
            let r = fmt_num(from_base(i, base));
            let name = UNITS[i].0[0];
            let text = format!("{r} {name}");
            Item::new(text, Icon::Glyph("\u{f046d}"), Action::Copy(r))
                .sub(format!("{} {}", fmt_num(v), UNITS[fi].0[0]))
                .meta("⏎ copy")
        })
        .collect()
}

/// 10 or 13 digits: a unix time (seconds or milliseconds).
fn time_items(q: &str) -> Vec<Item> {
    let q = q.trim();
    if !(q.len() == 10 || q.len() == 13) || !q.chars().all(|c| c.is_ascii_digit()) {
        return vec![];
    }
    let Ok(n) = q.parse::<i64>() else { return vec![] };
    let secs = if q.len() == 13 { n / 1000 } else { n };
    let mut out = Vec::new();
    for (tz, label) in [(glib::TimeZone::local(), "local"), (glib::TimeZone::utc(), "utc")] {
        if let Ok(d) = glib::DateTime::from_unix_utc(secs).and_then(|d| d.to_timezone(&tz))
            && let Ok(s) = d.format("%Y-%m-%d %H:%M:%S")
        {
            out.push(Item::new(s.to_string(), Icon::Glyph("\u{f00f0}"), Action::Copy(s.to_string())).sub(format!("unix time, {label}")).meta("⏎ copy"));
        }
        if let Ok(d) = glib::DateTime::from_unix_utc(secs).and_then(|d| d.to_timezone(&tz))
            && let Ok(s) = d.format_iso8601()
            && label == "utc"
        {
            out.push(Item::new(s.to_string(), Icon::Glyph("\u{f00f0}"), Action::Copy(s.to_string())).sub("iso 8601").meta("⏎ copy"));
        }
    }
    out
}

// ── currencies ────────────────────────────────────────────────────────────

/// 166 currencies, daily (open.er-api.com, no key needed).
const RATES_URL: &str = "https://open.er-api.com/v6/latest/EUR";
/// A few coins, in euros.
const CRYPTO_URL: &str = "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin,ethereum,solana,litecoin,dogecoin,ripple&vs_currencies=eur";
const CRYPTO: &[(&str, &str)] = &[("BTC", "bitcoin"), ("ETH", "ethereum"), ("SOL", "solana"), ("LTC", "litecoin"), ("DOGE", "dogecoin"), ("XRP", "ripple")];
/// Currencies the euro replaced, at their fixed conversion rates (per euro).
const LEGACY: &[(&str, f64)] = &[
    ("NLG", 2.20371),
    ("DEM", 1.95583),
    ("FRF", 6.55957),
    ("BEF", 40.3399),
    ("LUF", 40.3399),
    ("ITL", 1936.27),
    ("ESP", 166.386),
    ("PTE", 200.482),
    ("ATS", 13.7603),
    ("FIM", 5.94573),
    ("IEP", 0.787564),
    ("GRD", 340.75),
    ("SIT", 239.64),
    ("CYP", 0.585274),
    ("MTL", 0.4293),
    ("SKK", 30.126),
    ("EEK", 15.6466),
    ("LVL", 0.702804),
    ("LTL", 3.4528),
    ("HRK", 7.5345),
];
/// Shown when no target is typed, in this order.
const MAIN_CURRENCIES: &[&str] = &["EUR", "USD", "GBP", "CHF", "JPY", "CNY", "SEK", "NOK", "DKK", "PLN", "CAD", "AUD"];

fn rates_file() -> PathBuf {
    util::cache_dir().join("rates.json")
}

fn crypto_file() -> PathBuf {
    util::cache_dir().join("crypto.json")
}

/// Units per euro: every currency, the old euro ones, a few coins; and the
/// date of the rates.
fn rates() -> Option<(std::collections::HashMap<String, f64>, String)> {
    let v: serde_json::Value = util::read_json(&rates_file())?;
    let mut m: std::collections::HashMap<String, f64> =
        v["rates"].as_object()?.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect();
    m.insert("EUR".into(), 1.0);
    for (code, per_euro) in LEGACY {
        m.insert(code.to_string(), *per_euro);
    }
    if let Some(c) = util::read_json::<serde_json::Value>(&crypto_file()) {
        for (code, id) in CRYPTO {
            if let Some(eur) = c[id]["eur"].as_f64().filter(|e| *e > 0.0) {
                m.insert(code.to_string(), 1.0 / eur);
            }
        }
    }
    let date = v["time_last_update_unix"]
        .as_i64()
        .and_then(|t| glib::DateTime::from_unix_local(t).ok())
        .and_then(|d| d.format("%d %b").ok())
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    Some((m, date))
}

fn fetch(url: &str, file: PathBuf, ok: fn(&serde_json::Value) -> bool) {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(8)))
        .build()
        .new_agent();
    let Ok(mut r) = agent.get(url).call() else { return };
    let Ok(body) = r.body_mut().read_to_string() else { return };
    if serde_json::from_str::<serde_json::Value>(&body).is_ok_and(|v| ok(&v)) {
        let _ = util::write_atomic(&file, body.as_bytes());
    }
}

/// Fetches the rates in the background when the copy is older than 12 hours.
pub fn refresh_rates() {
    let fresh = std::fs::metadata(rates_file())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < 12 * 3600);
    if fresh {
        return;
    }
    std::thread::spawn(|| {
        fetch(RATES_URL, rates_file(), |v| v["rates"].is_object());
        fetch(CRYPTO_URL, crypto_file(), |v| v["bitcoin"]["eur"].is_number());
    });
}

fn currency(word: &str) -> Option<String> {
    let w = word.trim();
    let sym = match w {
        "€" | "euro" | "euros" => "EUR",
        "$" | "dollar" | "dollars" | "us$" => "USD",
        "£" | "pound" | "pounds" | "pound sterling" => "GBP",
        "gulden" | "guilder" | "guilders" | "ƒ" | "fl" | "hfl" => "NLG",
        "mark" | "dm" | "d-mark" | "deutschmark" => "DEM",
        "lira" | "lire" => "ITL",
        "peseta" | "pesetas" => "ESP",
        "escudo" | "escudos" => "PTE",
        "schilling" => "ATS",
        "drachma" | "drachmas" => "GRD",
        "bitcoin" | "₿" => "BTC",
        "ether" | "ethereum" => "ETH",
        "ruble" | "rubles" | "rouble" | "₽" => "RUB",
        "lira turca" | "₺" => "TRY",
        "real" | "reais" => "BRL",
        "rand" => "ZAR",
        "peso" | "pesos" => "MXN",
        "¥" | "yen" => "JPY",
        "₹" | "rupee" | "rupees" => "INR",
        "₩" | "won" => "KRW",
        "zł" | "zloty" => "PLN",
        "kr" => "SEK",
        "fr" | "franc" | "francs" => "CHF",
        "a$" => "AUD",
        "c$" => "CAD",
        "r$" => "BRL",
        "yuan" | "rmb" => "CNY",
        _ => "",
    };
    if !sym.is_empty() {
        return Some(sym.to_string());
    }
    let up = w.to_uppercase();
    ((up.len() == 3 || up == "DOGE") && up.chars().all(|c| c.is_ascii_alphabetic())).then_some(up)
}

/// "100 usd to eur", "€50", "50$ in gbp", "1.5k jpy".
fn parse_money(q: &str) -> Option<(f64, String, Option<String>)> {
    let q = q.trim().to_lowercase();
    let (left, to) = match q.split_once(" to ").or_else(|| q.split_once(" in ")).or_else(|| q.split_once('>')) {
        Some((a, b)) => (a.trim().to_string(), Some(b.trim().to_string())),
        None => (q.clone(), None),
    };
    // the amount, with the currency before or after it
    let start = left.find(|c: char| c.is_ascii_digit())?;
    let end = left[start..]
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
        .map(|e| start + e)
        .unwrap_or(left.len());
    let mut amount: f64 = left[start..end].replace(',', ".").parse().ok()?;
    let mut after = left[end..].trim();
    if let Some(rest) = after.strip_prefix('k')
        && (rest.is_empty() || rest.starts_with(' '))
    {
        amount *= 1000.0;
        after = rest.trim();
    }
    let before = left[..start].trim();
    let from = match (before.is_empty(), after.is_empty()) {
        (false, true) => currency(before)?,
        (true, false) => currency(after)?,
        _ => return None,
    };
    let to = match to {
        Some(t) if !t.is_empty() => Some(currency(&t)?),
        _ => None,
    };
    Some((amount, from, to))
}

fn money_items(q: &str) -> Vec<Item> {
    let Some((amount, from, to)) = parse_money(q) else { return vec![] };
    let Some((rates, date)) = rates() else {
        refresh_rates();
        return vec![Item::new("fetching exchange rates…", Icon::Glyph("\u{f0114}"), Action::Nothing).sub("open the converter again in a moment")];
    };
    let Some(&rf) = rates.get(&from) else { return vec![] };
    if to.as_ref().is_some_and(|t| !rates.contains_key(t)) {
        return vec![];
    }
    let mut order: Vec<String> = to.iter().cloned().collect();
    order.extend(MAIN_CURRENCIES.iter().map(|c| c.to_string()).filter(|c| *c != from && Some(c) != to.as_ref()));
    order
        .into_iter()
        .filter_map(|c| {
            let v = amount / rf * rates.get(&c)?;
            // coins and tiny amounts need more than two decimals
            let text = if v.abs() < 1.0 && v != 0.0 { fmt_num(v) } else { format!("{v:.2}") };
            Some(
                Item::new(format!("{text} {c}"), Icon::Glyph("\u{f0114}"), Action::Copy(text))
                    .sub(format!("{} {from} · rate of {date}", fmt_num(amount)))
                    .meta("⏎ copy"),
            )
        })
        .collect()
}

/// What a typed value converts to (no files): colours, quantities, times.
/// Also used inline by the app search.
pub fn value_items(q: &str, loose_hex: bool) -> Vec<Item> {
    let c = if loose_hex { color::parse_loose(q) } else { color::parse(q) };
    if let Some(c) = c {
        return color::items(c);
    }
    if let Some((v, fi, ti)) = parse_quantity(q) {
        return unit_items(v, fi, ti);
    }
    let money = money_items(q);
    if !money.is_empty() {
        return money;
    }
    time_items(q)
}

// ── files ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cat {
    Image,
    Video,
    Audio,
    Doc,
    Pdf,
    Markdown,
}

const IMAGE: &[&str] = &["png", "jpg", "jpeg", "webp", "avif", "gif", "bmp", "tiff", "tif", "heic", "heif", "jxl", "ico", "svg", "tga", "qoi"];
const VIDEO: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v", "flv", "wmv", "mpg", "mpeg", "ts", "3gp"];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "ogg", "opus", "m4a", "aac", "wma", "aiff", "alac"];
const DOC: &[&str] = &["docx", "doc", "odt", "rtf", "xlsx", "xls", "ods", "csv", "pptx", "ppt", "odp"];

fn ext(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn cat(p: &Path) -> Option<Cat> {
    let e = ext(p);
    let e = e.as_str();
    Some(if IMAGE.contains(&e) {
        Cat::Image
    } else if VIDEO.contains(&e) {
        Cat::Video
    } else if AUDIO.contains(&e) {
        Cat::Audio
    } else if DOC.contains(&e) {
        Cat::Doc
    } else if e == "pdf" {
        Cat::Pdf
    } else if matches!(e, "md" | "markdown" | "html" | "htm" | "txt") {
        Cat::Markdown
    } else {
        return None;
    })
}

/// (format, what it means)
fn targets(c: Cat, from: &str) -> Vec<(&'static str, &'static str)> {
    let all: Vec<(&str, &str)> = match c {
        Cat::Image => vec![
            ("png", "lossless"),
            ("jpg", "quality 100, 4:4:4"),
            ("webp", "lossless"),
            ("avif", "lossless"),
            ("jxl", "lossless"),
            ("gif", "256 colours"),
            ("tiff", "lossless"),
            ("bmp", "uncompressed"),
            ("ico", "icon, 16 to 256 px"),
            ("pdf", "one page"),
        ],
        Cat::Video => vec![
            ("mp4", "h.264 + aac, or the streams as they are"),
            ("mkv", "the streams as they are"),
            ("webm", "vp9 + opus"),
            ("mov", "h.264 + aac, or the streams as they are"),
            ("gif", "15 fps, up to 800 px wide"),
            ("mp3", "the sound, vbr best"),
            ("flac", "the sound, lossless"),
            ("wav", "the sound, uncompressed"),
            ("m4a", "the sound, aac 320k"),
        ],
        Cat::Audio => vec![
            ("mp3", "vbr best"),
            ("flac", "lossless"),
            ("wav", "uncompressed"),
            ("ogg", "vorbis q8"),
            ("opus", "192k"),
            ("m4a", "aac 320k"),
        ],
        Cat::Doc => vec![("pdf", "libreoffice"), ("docx", "word"), ("odt", "writer"), ("xlsx", "excel"), ("ods", "calc"), ("pptx", "powerpoint"), ("odp", "impress")],
        Cat::Pdf => vec![("png", "each page, 200 dpi"), ("jpg", "each page, 200 dpi"), ("txt", "the text")],
        Cat::Markdown => vec![("html", "pandoc"), ("docx", "pandoc"), ("odt", "pandoc"), ("pdf", "pandoc + libreoffice")],
    };
    let docs_fit = |t: &str| -> bool {
        // writer, calc and impress files only convert inside their own kind
        let w = ["docx", "doc", "odt", "rtf"];
        let s = ["xlsx", "xls", "ods", "csv"];
        let p = ["pptx", "ppt", "odp"];
        t == "pdf" || [w.as_slice(), s.as_slice(), p.as_slice()].iter().any(|g| g.contains(&from) && g.contains(&t))
    };
    all.into_iter()
        .filter(|(t, _)| *t != from && !(from == "jpeg" && *t == "jpg") && !(from == "tif" && *t == "tiff"))
        .filter(|(t, _)| c != Cat::Doc || docs_fit(t))
        .filter(|(t, _)| !(from == "md" || from == "markdown") || *t != "md")
        .collect()
}

/// The files to convert: a typed path, else the newest file or image on the clipboard.
pub enum Source {
    Files(Vec<PathBuf>),
    Image(Clip),
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => util::home().join(rest),
        None => PathBuf::from(p),
    }
}

/// Splits "~/a.png webp" into a path and the filter.
fn typed_path(q: &str) -> Option<(PathBuf, String)> {
    let q = q.trim();
    if !(q.starts_with('/') || q.starts_with("~/")) {
        return None;
    }
    let p = expand(q);
    if p.is_file() {
        return Some((p, String::new()));
    }
    let (a, b) = q.rsplit_once(' ')?;
    let p = expand(a.trim());
    p.is_file().then(|| (p, b.trim().to_string()))
}

fn clip_source(clips: &[Clip]) -> Option<(Source, f64)> {
    clips.iter().find_map(|c| match c.kind {
        Kind::Files => {
            let f: Vec<PathBuf> = c.files().into_iter().filter(|p| p.is_file() && cat(p).is_some()).collect();
            (!f.is_empty()).then_some((Source::Files(f), c.time))
        }
        Kind::Image if c.blob_path().is_some_and(|p| p.exists()) => Some((Source::Image(c.clone()), c.time)),
        _ => None,
    })
}

pub fn query(q: &str, clips: &[Clip]) -> Vec<Item> {
    let values = value_items(q, true);
    if !values.is_empty() {
        return values;
    }
    let (src, filter, when) = match typed_path(q) {
        Some((p, f)) => (Some(Source::Files(vec![p])), f, None),
        None => match clip_source(clips) {
            Some((s, t)) => (Some(s), q.trim().to_string(), Some(t)),
            None => (None, q.trim().to_string(), None),
        },
    };
    let Some(src) = src else {
        return if q.trim().is_empty() {
            vec![]
        } else {
            vec![Item::new("…", Icon::Glyph("\u{f0453}"), Action::Nothing).sub("not a colour, a quantity, a unix time or a file")]
        };
    };
    let (paths, image) = match &src {
        Source::Files(f) => (f.clone(), None),
        Source::Image(c) => (c.blob_path().into_iter().collect(), Some(c.id)),
    };
    // every file must share a kind; the first one decides
    let Some(first) = paths.first().cloned() else { return vec![] };
    let Some(c) = cat(&first) else { return vec![] };
    let paths: Vec<PathBuf> = paths.into_iter().filter(|p| cat(p) == Some(c)).collect();
    let from = ext(&first);
    let name = if image.is_some() {
        "clipboard image".to_string()
    } else if paths.len() == 1 {
        first.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    } else {
        format!("{} files", paths.len())
    };
    let mut fz = Fuzzy::new(&filter);
    let mut out: Vec<(u32, Item)> = Vec::new();
    for (t, what) in targets(c, &from) {
        let score = if fz.empty {
            0
        } else {
            match fz.score(&format!("{t} {what}")) {
                Some(s) => s + if t.starts_with(&filter.to_lowercase()) { 100 } else { 0 },
                None => continue,
            }
        };
        let it = Item::new(
            format!("{name}  →  {t}"),
            Icon::Glyph(match c {
                Cat::Image => "\u{f02e9}",
                Cat::Video => "\u{f0567}",
                Cat::Audio => "\u{f075a}",
                _ => "\u{f0219}",
            }),
            Action::Convert { src: paths.clone(), image, to: t.to_string() },
        )
        .sub(what)
        .meta("⏎ convert");
        out.push((score, it));
    }
    if !fz.empty {
        out.sort_by_key(|x| std::cmp::Reverse(x.0));
    }
    let mut items: Vec<Item> = out.into_iter().map(|x| x.1).collect();
    if let Some(first) = items.first_mut() {
        first.section = Some(match when {
            Some(t) => format!("copied {}", util::age(t)),
            None => "typed path".into(),
        });
    }
    items
}

/// A free name next to `src`: "name.webp", else "name (1).webp".
fn out_path(src: &Path, to: &str, image_clip: bool) -> PathBuf {
    let dir = if image_clip {
        let pics = std::env::var_os("XDG_PICTURES_DIR").map(PathBuf::from).unwrap_or_else(|| util::home().join("Pictures"));
        let _ = std::fs::create_dir_all(&pics);
        pics
    } else {
        src.parent().map(Path::to_path_buf).unwrap_or_else(util::home)
    };
    let stem = if image_clip {
        glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("clipboard-%Y%m%d-%H%M%S").ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "clipboard".into())
    } else {
        src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "converted".into())
    };
    let mut p = dir.join(format!("{stem}.{to}"));
    let mut n = 1;
    while p.exists() {
        p = dir.join(format!("{stem} ({n}).{to}"));
        n += 1;
    }
    p
}

fn run(argv: &[&str]) -> Result<(), String> {
    let out = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", argv[0]))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_string())
    }
}

fn ff(src: &Path, dst: &Path, opts: &[&str]) -> Result<(), String> {
    let (s, d) = (src.to_string_lossy(), dst.to_string_lossy());
    let mut a = vec!["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-i", &s];
    a.extend_from_slice(opts);
    a.push(&d);
    let r = run(&a);
    if r.is_err() {
        let _ = std::fs::remove_file(dst);
    }
    r
}

/// Converts one file. Blocking; runs on a worker thread.
fn convert_one(src: &Path, to: &str, image_clip: bool) -> Result<PathBuf, String> {
    let c = cat(src).ok_or("unknown kind of file")?;
    let dst = out_path(src, to, image_clip);
    let (s, d) = (src.to_string_lossy().to_string(), dst.to_string_lossy().to_string());
    if c == Cat::Video && matches!(to, "mp3" | "flac" | "wav" | "m4a") && !has_audio(src) {
        return Err("the video has no sound".into());
    }
    match (c, to) {
        (Cat::Image, _) | (Cat::Pdf, "png" | "jpg") => {
            let mut a: Vec<&str> = vec!["magick"];
            if matches!(c, Cat::Pdf) || ext(src) == "svg" {
                a.extend(["-density", if c == Cat::Pdf { "200" } else { "300" }]);
            }
            a.push(&s);
            match to {
                "jpg" => a.extend(["-background", "white", "-flatten", "-quality", "100", "-sampling-factor", "4:4:4"]),
                "webp" => a.extend(["-define", "webp:lossless=true", "-quality", "100"]),
                "avif" | "jxl" => a.extend(["-quality", "100"]),
                "png" => a.extend(["-define", "png:compression-level=9"]),
                "ico" => a.extend(["-background", "none", "-resize", "256x256", "-define", "icon:auto-resize=256,128,64,48,32,16"]),
                _ => {}
            }
            a.push(&d);
            run(&a)?;
        }
        (Cat::Video, "mp4" | "mov" | "mkv") => {
            // the streams as they are when the container allows it
            if ff(src, &dst, &["-map", "0", "-c", "copy", "-movflags", "+faststart"]).is_err() {
                if to == "mkv" {
                    ff(src, &dst, &["-map", "0", "-c:v", "libx264", "-crf", "16", "-preset", "slow", "-c:a", "flac", "-c:s", "copy"])?;
                } else {
                    ff(src, &dst, &["-c:v", "libx264", "-crf", "16", "-preset", "slow", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "320k", "-movflags", "+faststart"])?;
                }
            }
        }
        (Cat::Video, "webm") => ff(src, &dst, &["-c:v", "libvpx-vp9", "-crf", "24", "-b:v", "0", "-row-mt", "1", "-c:a", "libopus", "-b:a", "192k"])?,
        (Cat::Video, "gif") => ff(
            src,
            &dst,
            &["-vf", "fps=15,scale='min(800,iw)':-1:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a", "-loop", "0"],
        )?,
        (Cat::Video | Cat::Audio, "mp3") => ff(src, &dst, &["-vn", "-c:a", "libmp3lame", "-q:a", "0"])?,
        (Cat::Video | Cat::Audio, "flac") => ff(src, &dst, &["-vn", "-c:a", "flac", "-compression_level", "8"])?,
        (Cat::Video | Cat::Audio, "wav") => ff(src, &dst, &["-vn", "-c:a", "pcm_s16le"])?,
        (Cat::Video | Cat::Audio, "m4a") => ff(src, &dst, &["-vn", "-c:a", "aac", "-b:a", "320k"])?,
        (Cat::Audio, "ogg") => ff(src, &dst, &["-vn", "-c:a", "libvorbis", "-q:a", "8"])?,
        (Cat::Audio, "opus") => ff(src, &dst, &["-vn", "-c:a", "libopus", "-b:a", "192k"])?,
        (Cat::Pdf, "txt") => run(&["pdftotext", "-layout", &s, &d])?,
        (Cat::Doc, _) => office(src, &dst, to)?,
        (Cat::Markdown, "pdf") => {
            let tmp = dst.with_extension("odt");
            run(&["pandoc", &s, "-o", &tmp.to_string_lossy()])?;
            let r = office(&tmp, &dst, "pdf");
            let _ = std::fs::remove_file(&tmp);
            r?;
        }
        (Cat::Markdown, _) => run(&["pandoc", "--standalone", &s, "-o", &d])?,
        _ => return Err(format!("can't make {to} from this")),
    }
    if dst.exists() {
        Ok(dst)
    } else {
        // pdf pages: magick wrote name-0.png, name-1.png...
        let stem = dst.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let first = dst.with_file_name(format!("{stem}-0.{to}"));
        if first.exists() { Ok(first) } else { Err("nothing was written".into()) }
    }
}

fn has_audio(p: &Path) -> bool {
    Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a", "-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(p)
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(true)
}

/// LibreOffice names its output itself: convert in a private folder, then move.
fn office(src: &Path, dst: &Path, to: &str) -> Result<(), String> {
    let tmp = util::cache_dir().join(format!("convert-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&tmp);
    let r = run(&["soffice", "--headless", "--norestore", "--convert-to", to, "--outdir", &tmp.to_string_lossy(), &src.to_string_lossy()]);
    let made = std::fs::read_dir(&tmp).ok().and_then(|mut d| d.next()).and_then(|e| e.ok()).map(|e| e.path());
    let res = match (r, made) {
        (Ok(()), Some(m)) => std::fs::rename(&m, dst).or_else(|_| std::fs::copy(&m, dst).map(|_| ())).map_err(|e| e.to_string()),
        (Err(e), _) => Err(e),
        (Ok(()), None) => Err("libreoffice wrote nothing".into()),
    };
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

fn notify(summary: &str, body: &str, icon: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "Vector Space", "-i", icon, summary, body])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `vector-space to FORMAT FILE...`: convert without the window; prints
/// each result, nothing goes on the clipboard.
pub fn cli(to: &str, files: &[String]) -> bool {
    let mut ok = true;
    for f in files {
        match convert_one(Path::new(f), to, false) {
            Ok(p) => println!("{}", p.display()),
            Err(e) => {
                eprintln!("{f}: {e}");
                ok = false;
            }
        }
    }
    ok
}

/// Converts in the background; the results go on the clipboard and a
/// notification says where they are.
pub fn start(src: Vec<PathBuf>, image: Option<u64>, to: String) {
    std::thread::spawn(move || {
        let what = if src.len() == 1 {
            src[0].file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
        } else {
            format!("{} files", src.len())
        };
        let what = if image.is_some() { "clipboard image".to_string() } else { what };
        let slow = src.iter().any(|p| matches!(cat(p), Some(Cat::Video | Cat::Doc)));
        if slow {
            notify(&format!("converting {what} to {to}"), "you'll get a note when it's done", "view-refresh");
        }
        let mut done = Vec::new();
        let mut failed = Vec::new();
        for p in &src {
            match convert_one(p, &to, image.is_some()) {
                Ok(d) => done.push(d),
                Err(e) => failed.push(format!("{}: {e}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())),
            }
        }
        if !done.is_empty() {
            if image.is_some() {
                if let Ok(bytes) = std::fs::read(&done[0]) {
                    let mime = format!("image/{}", if to == "jpg" { "jpeg" } else { &to });
                    util::wl_copy(bytes, Some(&mime));
                }
            } else {
                let uris: String = done.iter().filter_map(|p| glib::filename_to_uri(p, None).ok()).map(|u| format!("{u}\n")).collect();
                util::wl_copy(uris.into_bytes(), Some("text/uri-list"));
            }
            let shown = done.iter().take(3).map(|p| p.display().to_string().replacen(&util::home().display().to_string(), "~", 1)).collect::<Vec<_>>().join("\n");
            notify(&format!("{what} → {to}"), &format!("{shown}\non the clipboard"), "emblem-ok-symbolic");
        }
        if !failed.is_empty() {
            notify(&format!("couldn't convert {what} to {to}"), &failed.join("\n"), "dialog-error");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantities() {
        let (v, f, t) = parse_quantity("10 km to mi").unwrap();
        assert!((from_base(t.unwrap(), to_base(f, v)) - 6.21371).abs() < 1e-4);
        let (v, f, t) = parse_quantity("100f in c").unwrap();
        assert!((from_base(t.unwrap(), to_base(f, v)) - 37.7778).abs() < 1e-3);
        let (v, f, _) = parse_quantity("3gib").unwrap();
        assert_eq!(to_base(f, v), 3.0 * 1073741824.0);
        assert!(parse_quantity("firefox").is_none());
        assert!(parse_quantity("10 km to kg").is_none());
        assert!(parse_quantity("5").is_none());
        assert_eq!(fmt_num(6.213711922), "6.21371");
        assert_eq!(fmt_num(1000.0), "1000");
    }

    #[test]
    fn money() {
        assert_eq!(parse_money("100 usd to eur"), Some((100.0, "USD".into(), Some("EUR".into()))));
        assert_eq!(parse_money("€50"), Some((50.0, "EUR".into(), None)));
        assert_eq!(parse_money("50$ in gbp"), Some((50.0, "USD".into(), Some("GBP".into()))));
        assert_eq!(parse_money("1.5k jpy"), Some((1500.0, "JPY".into(), None)));
        assert_eq!(parse_money("hello"), None);
        assert_eq!(parse_money("100 gulden to eur"), Some((100.0, "NLG".into(), Some("EUR".into()))));
        assert_eq!(parse_money("100 usd in inr"), Some((100.0, "USD".into(), Some("INR".into()))));
    }

    #[test]
    fn targets_by_kind() {
        let t: Vec<&str> = targets(Cat::Image, "png").iter().map(|x| x.0).collect();
        assert!(t.contains(&"jpg") && !t.contains(&"png"));
        let t: Vec<&str> = targets(Cat::Doc, "docx").iter().map(|x| x.0).collect();
        assert!(t.contains(&"pdf") && t.contains(&"odt") && !t.contains(&"xlsx"));
    }

    #[test]
    fn free_names() {
        let d = tempfile_dir();
        let src = d.join("a.png");
        std::fs::write(&src, b"x").unwrap();
        assert_eq!(out_path(&src, "jpg", false), d.join("a.jpg"));
        std::fs::write(d.join("a.jpg"), b"x").unwrap();
        assert_eq!(out_path(&src, "jpg", false), d.join("a (1).jpg"));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("vs-convert-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
