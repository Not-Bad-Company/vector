//! Lowercase, compact formatting for sizes and dates.

/// `512 b`, `12.3 kb`, `4.0 gb`.
pub fn size(n: u64) -> String {
    if n < 1024 {
        return format!("{n} b");
    }
    let units = ["kb", "mb", "gb", "tb", "pb"];
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < units.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

/// `28 oct 2026 14:02`; empty for unknown.
pub fn date(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    glib::DateTime::from_unix_local(secs)
        .and_then(|d| d.format("%-d %b %Y %H:%M"))
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0 b");
        assert_eq!(size(1023), "1023 b");
        assert_eq!(size(12_595), "12.3 kb");
        assert_eq!(size(3 * 1024 * 1024), "3.0 mb");
        assert_eq!(size(u64::MAX), "16384.0 pb");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "");
        let d = date(1_700_000_000);
        assert!(d.contains("2023") && d == d.to_lowercase(), "{d}");
    }
}
