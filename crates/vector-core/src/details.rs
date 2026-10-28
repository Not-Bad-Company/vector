//! Facts shown under the preview: exact sizes, permissions, image dimensions, media streams,
//! PDF info, text statistics. Parsing and formatting only; the app runs the tools.

/// One row of the details table.
pub type Row = (String, String);

fn row(k: &str, v: impl Into<String>) -> Row {
    (k.to_string(), v.into())
}

/// `33512` → `33 512`.
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// `0o100644` → `rw-r--r-- 644` (file type bits ignored; setuid/setgid/sticky shown like ls).
pub fn permissions(mode: u32) -> String {
    let bit = |m: u32, c: char| if mode & m != 0 { c } else { '-' };
    let exec =
        |m: u32, special: u32, lower: char, upper: char| match (mode & m != 0, mode & special != 0)
        {
            (true, true) => lower,
            (false, true) => upper,
            (true, false) => 'x',
            (false, false) => '-',
        };
    let s: String = [
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        exec(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        exec(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        exec(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect();
    format!("{s} {:o}", mode & 0o7777)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `16:9`, or `1.85:1` when the reduced ratio has large terms.
pub fn aspect(w: u64, h: u64) -> String {
    if w == 0 || h == 0 {
        return String::new();
    }
    let g = gcd(w, h);
    let (a, b) = (w / g, h / g);
    if a <= 32 && b <= 32 {
        format!("{a}:{b}")
    } else {
        format!("{:.2}:1", w as f64 / h as f64)
    }
}

/// Rows for an image of `w`×`h` pixels in `format` (e.g. "png").
pub fn image(w: u64, h: u64, format: &str) -> Vec<Row> {
    let mp = (w * h) as f64 / 1_000_000.0;
    let mut v = vec![row("dimensions", format!("{w} × {h} px"))];
    v.push(row(
        "pixels",
        format!("{} ({mp:.2} MP)", group_digits(w * h)),
    ));
    let a = aspect(w, h);
    if !a.is_empty() {
        v.push(row("aspect", a));
    }
    if !format.is_empty() {
        v.push(row("format", format.to_uppercase()));
    }
    v
}

/// `3725.5` seconds → `1:02:05`, `65` → `1:05`.
pub fn duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return String::new();
    }
    let t = secs.round() as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Rows from `ffprobe -show_entries format=duration,bit_rate,format_long_name:stream=codec_type,
/// codec_name,width,height,r_frame_rate,channels,sample_rate,pix_fmt -of default=nw=1`.
pub fn ffprobe(out: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut streams: Vec<Vec<(&str, &str)>> = Vec::new();
    let mut format: Vec<(&str, &str)> = Vec::new();
    // stream blocks start at codec_name; format keys come after the streams
    for line in out.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "codec_name" => streams.push(vec![(k, v)]),
            "duration" | "bit_rate" | "format_long_name" => format.push((k, v)),
            _ => {
                if let Some(s) = streams.last_mut() {
                    s.push((k, v));
                }
            }
        }
    }
    let get = |s: &[(&str, &str)], key: &str| {
        s.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.to_string())
    };
    if let Some(d) = get(&format, "duration").and_then(|d| d.parse::<f64>().ok()) {
        rows.push(row("duration", duration(d)));
    }
    for s in &streams {
        let codec = get(s, "codec_name").unwrap_or_default();
        match get(s, "codec_type").as_deref() {
            Some("video") => {
                if let (Some(w), Some(h)) = (get(s, "width"), get(s, "height")) {
                    rows.push(row("resolution", format!("{w} × {h} px")));
                }
                if let Some(fps) = get(s, "r_frame_rate").and_then(|r| frame_rate(&r)) {
                    rows.push(row("frame rate", format!("{fps} fps")));
                }
                let pix = get(s, "pix_fmt")
                    .map(|p| format!(" · {p}"))
                    .unwrap_or_default();
                rows.push(row("video", format!("{codec}{pix}")));
            }
            Some("audio") => {
                let ch = get(s, "channels").map(|c| match c.as_str() {
                    "1" => "mono".to_string(),
                    "2" => "stereo".to_string(),
                    n => format!("{n} channels"),
                });
                let rate = get(s, "sample_rate")
                    .and_then(|r| r.parse::<f64>().ok())
                    .map(|r| format!("{} kHz", r / 1000.0));
                let parts: Vec<String> = [Some(codec), ch, rate].into_iter().flatten().collect();
                rows.push(row("audio", parts.join(" · ")));
            }
            Some("subtitle") => rows.push(row("subtitles", codec)),
            _ => {}
        }
    }
    if let Some(b) = get(&format, "bit_rate").and_then(|b| b.parse::<f64>().ok()) {
        rows.push(row("bit rate", format!("{:.0} kb/s", b / 1000.0)));
    }
    if let Some(f) = get(&format, "format_long_name") {
        // ffprobe names the one demuxer for .mov and .mp4 after QuickTime
        let f = if f == "QuickTime / MOV" {
            "MPEG-4 / QuickTime".to_string()
        } else {
            f
        };
        rows.push(row("container", f));
    }
    rows
}

/// `30000/1001` → `29.97`, `25/1` → `25`; `0/0` → None.
fn frame_rate(r: &str) -> Option<String> {
    let (n, d) = r.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    if n <= 0.0 || d <= 0.0 {
        return None;
    }
    let f = n / d;
    Some(if (f - f.round()).abs() < 0.005 {
        format!("{}", f.round())
    } else {
        format!("{f:.2}")
    })
}

/// Rows from `pdfinfo` output.
pub fn pdfinfo(out: &str) -> Vec<Row> {
    let field = |name: &str| {
        out.lines()
            .find_map(|l| {
                l.strip_prefix(name)
                    .and_then(|r| r.strip_prefix(':'))
                    .map(|v| v.trim().to_string())
            })
            .filter(|v| !v.is_empty())
    };
    let mut rows = Vec::new();
    if let Some(p) = field("Pages") {
        rows.push(row("pages", p));
    }
    if let Some(s) = field("Page size") {
        // "595.276 x 841.89 pts (A4)": add millimetres
        let mm = s
            .split_whitespace()
            .collect::<Vec<_>>()
            .get(..3)
            .and_then(|p| Some((p[0].parse::<f64>().ok()?, p[2].parse::<f64>().ok()?)))
            .map(|(w, h)| format!(" · {:.0} × {:.0} mm", w * 25.4 / 72.0, h * 25.4 / 72.0))
            .unwrap_or_default();
        rows.push(row("page size", format!("{s}{mm}")));
    }
    for (name, key) in [
        ("Title", "title"),
        ("Author", "author"),
        ("Producer", "producer"),
        ("PDF version", "version"),
    ] {
        if let Some(v) = field(name) {
            rows.push(row(key, v));
        }
    }
    if field("Encrypted").is_some_and(|e| e.starts_with("yes")) {
        rows.push(row("encrypted", "yes"));
    }
    rows
}

/// Text statistics of a whole file (`complete`) or of its first part.
pub fn text(bytes: &[u8], complete: bool) -> Vec<Row> {
    let lines = bytes.iter().filter(|&&b| b == b'\n').count()
        + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"));
    let (enc, chars) = match std::str::from_utf8(bytes) {
        Ok(s) => (
            if s.is_ascii() { "ascii" } else { "utf-8" },
            Some(s.chars().count()),
        ),
        // a cut in the middle of a character at the end of a partial read is still utf-8
        Err(e) if !complete && e.error_len().is_none() => ("utf-8", None),
        Err(_) => ("not utf-8", None),
    };
    let crlf = bytes.windows(2).filter(|w| w == b"\r\n").count();
    let endings = if crlf == 0 {
        "lf"
    } else if crlf == bytes.iter().filter(|&&b| b == b'\n').count() {
        "crlf"
    } else {
        "mixed lf / crlf"
    };
    let more = if complete { "" } else { "+" };
    let mut rows = vec![row(
        "lines",
        format!("{}{more}", group_digits(lines as u64)),
    )];
    if let Some(c) = chars.filter(|_| complete) {
        rows.push(row("characters", group_digits(c as u64)));
    }
    rows.push(row("encoding", enc));
    if lines > 1 || crlf > 0 {
        rows.push(row("line endings", endings));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|(k, _)| k.as_str()).collect()
    }

    fn val<'a>(rows: &'a [Row], k: &str) -> &'a str {
        rows.iter()
            .find(|(x, _)| x == k)
            .map_or("", |(_, v)| v.as_str())
    }

    #[test]
    fn numbers_and_modes() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(33_512), "33 512");
        assert_eq!(group_digits(1_234_567_890), "1 234 567 890");
        assert_eq!(permissions(0o100644), "rw-r--r-- 644");
        assert_eq!(permissions(0o40755), "rwxr-xr-x 755");
        assert_eq!(permissions(0o4755), "rwsr-xr-x 4755");
        assert_eq!(permissions(0o1777), "rwxrwxrwt 1777");
        assert_eq!(permissions(0o2640), "rw-r-S--- 2640");
    }

    #[test]
    fn images() {
        let r = image(1920, 1080, "png");
        assert_eq!(val(&r, "dimensions"), "1920 × 1080 px");
        assert_eq!(val(&r, "pixels"), "2 073 600 (2.07 MP)");
        assert_eq!(val(&r, "aspect"), "16:9");
        assert_eq!(val(&r, "format"), "PNG");
        assert_eq!(aspect(1000, 541), "1.85:1");
        assert_eq!(aspect(0, 5), "");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(3.0), "0:03");
        assert_eq!(duration(65.4), "1:05");
        assert_eq!(duration(3725.5), "1:02:06");
        assert_eq!(duration(f64::NAN), "");
    }

    #[test]
    fn ffprobe_video_and_audio() {
        let out = "codec_name=h264\ncodec_type=video\nwidth=640\nheight=360\npix_fmt=yuv420p\nr_frame_rate=25/1\n\
                   codec_name=aac\ncodec_type=audio\nsample_rate=44100\nchannels=1\nr_frame_rate=0/0\n\
                   format_long_name=QuickTime / MOV\nduration=3.000000\nbit_rate=847605\n";
        let r = ffprobe(out);
        assert_eq!(
            keys(&r),
            [
                "duration",
                "resolution",
                "frame rate",
                "video",
                "audio",
                "bit rate",
                "container"
            ]
        );
        assert_eq!(val(&r, "resolution"), "640 × 360 px");
        assert_eq!(val(&r, "frame rate"), "25 fps");
        assert_eq!(val(&r, "video"), "h264 · yuv420p");
        assert_eq!(val(&r, "audio"), "aac · mono · 44.1 kHz");
        assert_eq!(val(&r, "bit rate"), "848 kb/s");
        let ntsc = ffprobe("codec_name=h264\ncodec_type=video\nr_frame_rate=30000/1001\n");
        assert_eq!(val(&ntsc, "frame rate"), "29.97 fps");
        assert!(ffprobe("garbage\n").is_empty());
    }

    #[test]
    fn pdf() {
        let out = "Title:           sample\nAuthor:          \nProducer:        tool\nPages:           12\n\
                   Encrypted:       no\nPage size:       595.276 x 841.89 pts (A4)\nPDF version:     1.7\n";
        let r = pdfinfo(out);
        assert_eq!(
            keys(&r),
            ["pages", "page size", "title", "producer", "version"]
        );
        assert_eq!(
            val(&r, "page size"),
            "595.276 x 841.89 pts (A4) · 210 × 297 mm"
        );
    }

    #[test]
    fn text_stats() {
        let r = text(b"one\ntwo\n", true);
        assert_eq!(val(&r, "lines"), "2");
        assert_eq!(val(&r, "characters"), "8");
        assert_eq!(val(&r, "encoding"), "ascii");
        assert_eq!(val(&r, "line endings"), "lf");
        let r = text("é\r\nx".as_bytes(), true);
        assert_eq!(
            (
                val(&r, "lines"),
                val(&r, "encoding"),
                val(&r, "line endings")
            ),
            ("2", "utf-8", "crlf")
        );
        // a partial read cut inside a character
        let r = text(&"aé".as_bytes()[..2], false);
        assert_eq!((val(&r, "lines"), val(&r, "encoding")), ("1+", "utf-8"));
        assert_eq!(val(&text(b"\xff\xfe", true), "encoding"), "not utf-8");
        assert_eq!(
            val(&text(b"a\nb\r\n", true), "line endings"),
            "mixed lf / crlf"
        );
    }
}
