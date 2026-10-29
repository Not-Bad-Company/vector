//! Small shared helpers: paths, processes, Hyprland IPC, time, fuzzy matching.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

/// `$XDG_DATA_HOME/vector-space`: the clipboard history.
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("vector-space")
}

/// `$XDG_STATE_HOME/vector-space`: launch counts, recent emoji and searches.
pub fn state_dir() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("vector-space")
}

pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("vector-space")
}

pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// "now", "5m", "3h", "2d", "6w".
pub fn age(time: f64) -> String {
    let s = (now() - time).max(0.0) as u64;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        86400..1_209_600 => format!("{}d", s / 86400),
        _ => format!("{}w", s / 604_800),
    }
}

/// Writes through a temporary file and a rename, so readers never see half a file.
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn write_json<T: serde::Serialize>(path: &Path, v: &T) {
    if let Ok(s) = serde_json::to_vec(v)
        && let Err(e) = write_atomic(path, &s)
    {
        eprintln!("vector-space: {}: {e}", path.display());
    }
}

/// Starts a program detached (own session, reaped by a thread).
pub fn spawn(argv: &[&str]) {
    let Some((prog, args)) = argv.split_first() else {
        return;
    };
    let mut c = Command::new(prog);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match c.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("vector-space: {prog}: {e}"),
    }
}

/// Starts a program as its own systemd scope (through uwsm when there), so it
/// outlives a restart of vector-space.service.
pub fn launch(argv: &[&str]) {
    if which("uwsm") {
        let mut v = vec!["uwsm", "app", "--"];
        v.extend_from_slice(argv);
        spawn(&v);
    } else {
        spawn(argv);
    }
}

pub fn which(prog: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
}

/// Puts bytes on the clipboard with wl-copy (which stays to serve them).
pub fn wl_copy(data: Vec<u8>, mime: Option<&str>) {
    let mut c = Command::new("wl-copy");
    if let Some(m) = mime {
        c.args(["--type", m]);
    }
    c.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match c.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                if let Some(mut si) = child.stdin.take() {
                    let _ = si.write_all(&data);
                }
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("vector-space: wl-copy: {e}"),
    }
}

/// Pastes into the window that had focus before we opened: ctrl+v, or
/// ctrl+shift+v in a terminal. Waits for our surface to be gone first.
pub fn paste_later() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(180));
        let class = hypr_json("activewindow")
            .and_then(|v| v["class"].as_str().map(str::to_lowercase))
            .unwrap_or_default();
        let terminal = ["alacritty", "kitty", "ghostty", "foot", "wezterm", "konsole", "terminal"]
            .iter()
            .any(|t| class.contains(t));
        let mut c = Command::new("wtype");
        if terminal {
            c.args(["-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"]);
        } else {
            c.args(["-M", "ctrl", "-k", "v", "-m", "ctrl"]);
        }
        let _ = c.stdout(Stdio::null()).stderr(Stdio::null()).status();
    });
}

fn hypr_socket() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let run = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    Some(run.join("hypr").join(sig).join(".socket.sock"))
}

/// One Hyprland request ("j/monitors") and its reply.
pub fn hypr(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(hypr_socket()?).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

pub fn hypr_json(what: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&hypr(&format!("j/{what}"))?).ok()
}

/// One line, whitespace collapsed, at most `max` characters.
pub fn one_line(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (n, w) in s.split_whitespace().enumerate() {
        if n > 0 {
            out.push(' ');
        }
        out.push_str(w);
        if out.len() > max * 4 {
            break;
        }
    }
    if out.chars().count() > max {
        out = out.chars().take(max.saturating_sub(1)).collect::<String>() + "…";
    }
    out
}

pub fn escape(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// Markup with the matched characters (`hl`, char indices) in the accent.
pub fn highlight(s: &str, hl: &[u32]) -> String {
    if hl.is_empty() {
        return escape(s);
    }
    let open = format!("<span foreground=\"{}\" weight=\"bold\">", crate::style::accent());
    let mut out = String::new();
    let mut on = false;
    for (i, c) in s.chars().enumerate() {
        let want = hl.binary_search(&(i as u32)).is_ok();
        if want != on {
            out.push_str(if want { &open } else { "</span>" });
            on = want;
        }
        out.push_str(&escape(c.encode_utf8(&mut [0; 4])));
    }
    if on {
        out.push_str("</span>");
    }
    out
}

/// Fuzzy matching (nucleo), one pattern against many strings.
pub struct Fuzzy {
    pattern: Pattern,
    matcher: Matcher,
    buf: Vec<char>,
    pub empty: bool,
}

impl Fuzzy {
    pub fn new(query: &str) -> Fuzzy {
        Fuzzy {
            pattern: Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart),
            matcher: Matcher::new(Config::DEFAULT),
            buf: Vec::new(),
            empty: query.trim().is_empty(),
        }
    }

    pub fn score(&mut self, s: &str) -> Option<u32> {
        self.pattern
            .score(Utf32Str::new(s, &mut self.buf), &mut self.matcher)
    }

    /// Score and matched char indices (sorted).
    pub fn indices(&mut self, s: &str) -> Option<(u32, Vec<u32>)> {
        let mut idx = Vec::new();
        let sc = self
            .pattern
            .indices(Utf32Str::new(s, &mut self.buf), &mut self.matcher, &mut idx)?;
        idx.sort_unstable();
        idx.dedup();
        Some((sc, idx))
    }
}

/// Counts and last use per key, for ranking what is used often and lately.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Usage(pub std::collections::HashMap<String, (u32, f64)>);

impl Usage {
    pub fn load(name: &str) -> Usage {
        read_json(&state_dir().join(name)).unwrap_or_default()
    }

    pub fn bump(name: &str, key: &str) {
        let mut u = Usage::load(name);
        let e = u.0.entry(key.to_string()).or_insert((0, 0.0));
        e.0 = e.0.saturating_add(1);
        e.1 = now();
        if u.0.len() > 400 {
            let mut v: Vec<_> = u.0.iter().map(|(k, (_, t))| (k.clone(), *t)).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            for (k, _) in v.into_iter().skip(300) {
                u.0.remove(&k);
            }
        }
        write_json(&state_dir().join(name), &u);
    }

    /// 0..~40: grows with use, halves every week without use.
    pub fn bonus(&self, key: &str) -> u32 {
        let Some((n, t)) = self.0.get(key) else { return 0 };
        let weeks = ((now() - t) / 604_800.0).max(0.0);
        ((*n as f64 + 1.0).ln() * 12.0 * 0.5f64.powf(weeks)).min(40.0) as u32
    }

    pub fn last(&self, key: &str) -> f64 {
        self.0.get(key).map(|u| u.1).unwrap_or(0.0)
    }
}
