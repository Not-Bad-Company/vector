//! Small helpers: running commands, background work, formatting.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}

pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"))
}

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `~/x` -> `$HOME/x`.
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Runs a shell command line fully detached (own session, no stdio), like a
/// Waybar `on-click`. The child is reaped on a thread so no zombies pile up.
pub fn spawn_sh(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    let mut c = Command::new("sh");
    c.arg("-c")
        .arg(cmd)
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
        Err(e) => eprintln!("vector-bar: {cmd}: {e}"),
    }
}

/// Runs a program detached with arguments (no shell quoting involved).
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
        Err(e) => eprintln!("vector-bar: {prog}: {e}"),
    }
}

/// Output of a finished command.
pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a program and waits for it, killing it after `timeout`. Blocking: call
/// from a worker thread. `None` if it could not start or timed out.
pub fn run(argv: &[&str], timeout: Duration) -> Option<Out> {
    run_in(argv, timeout, None, None)
}

/// [`run`] with optional stdin text and working directory.
pub fn run_in(
    argv: &[&str],
    timeout: Duration,
    stdin: Option<&str>,
    cwd: Option<&Path>,
) -> Option<Out> {
    let (prog, args) = argv.split_first()?;
    let mut c = Command::new(prog);
    c.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let mut child = c.spawn().ok()?;
    if let (Some(text), Some(mut sin)) = (stdin, child.stdin.take()) {
        use std::io::Write;
        let _ = sin.write_all(text.as_bytes());
    }
    let mut so = child.stdout.take()?;
    let mut se = child.stderr.take()?;
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = so.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = se.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    status.map(|st| Out {
        ok: st.success(),
        stdout,
        stderr,
    })
}

/// Runs a shell line and returns trimmed stdout when it succeeded.
pub fn sh_out(cmd: &str, timeout: Duration) -> Option<String> {
    let o = run(&["sh", "-c", cmd], timeout)?;
    o.ok.then(|| o.stdout.trim().to_string())
}

/// Runs `work` on a worker thread and `done` with its result on the GTK thread.
pub fn bg<T, W, D>(work: W, done: D)
where
    T: Send + 'static,
    W: FnOnce() -> T + Send + 'static,
    D: FnOnce(T) + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(work());
    });
    glib::spawn_future_local(async move {
        if let Ok(v) = rx.recv().await {
            done(v);
        }
    });
}

/// Calls `f` every `ms` for as long as `owner` is alive (timers must not keep
/// a module alive after a config reload dropped it).
pub fn every<T: 'static>(ms: u64, owner: &std::rc::Rc<T>, f: impl Fn(&std::rc::Rc<T>) + 'static) {
    let weak = std::rc::Rc::downgrade(owner);
    glib::timeout_add_local(Duration::from_millis(ms), move || match weak.upgrade() {
        Some(t) => {
            f(&t);
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
}

/// Calls `f` once after `ms` if `owner` is still alive.
pub fn after<T: 'static>(
    ms: u64,
    owner: &std::rc::Rc<T>,
    f: impl FnOnce(&std::rc::Rc<T>) + 'static,
) {
    let weak = std::rc::Rc::downgrade(owner);
    glib::timeout_add_local_once(Duration::from_millis(ms), move || {
        if let Some(t) = weak.upgrade() {
            f(&t);
        }
    });
}

pub fn read(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

pub fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    read(path).map(|s| s.trim().to_string())
}

pub fn read_num(path: impl AsRef<Path>) -> Option<i64> {
    read_trim(path)?.parse().ok()
}

pub fn read_json(path: impl AsRef<Path>) -> Option<serde_json::Value> {
    serde_json::from_str(&read(path)?).ok()
}

/// Writes atomically (temp file + rename).
pub fn write_atomic(path: impl AsRef<Path>, data: &str) -> std::io::Result<()> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Pango escape.
pub fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// "40 MB", "1.5 GB", "31 GB", "440 GB" from KiB (same as tooltip-lib.sh `size`).
pub fn size_kb(kb: u64) -> String {
    let g = kb as f64 / 1_048_576.0;
    if g >= 1000.0 {
        let t = g / 1024.0;
        return format!("{} TB", trim0(format!("{:.2}", t)));
    }
    if g >= 1.0 {
        let s = if g >= 100.0 {
            format!("{:.0}", g)
        } else {
            format!("{:.1}", g)
        };
        return format!("{} GB", trim0(s));
    }
    format!("{:.0} MB", kb as f64 / 1024.0)
}

fn trim0(s: String) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0').trim_end_matches('.');
        t.to_string()
    } else {
        s
    }
}

/// Bytes, compact: "512 B", "3.4 KB", "12 MB".
pub fn size_bytes(b: u64) -> String {
    let b = b as f64;
    if b < 1024.0 {
        format!("{b:.0} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1048576.0)
    } else {
        format!("{:.1} GB", b / 1073741824.0)
    }
}

/// "just now", "5m ago", "2h 03m ago", "3d ago".
pub fn ago(ts: f64) -> String {
    if ts <= 0.0 {
        return "never".into();
    }
    let s = (now() - ts).max(0.0) as i64;
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86400 {
        format!("{}h {:02}m ago", s / 3600, s % 3600 / 60)
    } else {
        format!("{}d ago", s / 86400)
    }
}

/// Time left until `ts`: "now", "42m", "3h05m", "4d6h".
pub fn until(ts: f64) -> String {
    let s = (ts - now()) as i64;
    if s <= 0 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86400 {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    } else {
        format!("{}d{}h", s / 86400, s % 86400 / 3600)
    }
}

/// Seconds as "1h 05m", "12m", "40s".
pub fn dur(secs: u64) -> String {
    if secs >= 86400 {
        format!("{}d {}h", secs / 86400, secs % 86400 / 3600)
    } else if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Local time of a unix timestamp, formatted with strftime.
pub fn local(ts: f64, fmt: &str) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts as i64, 0) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => {
            t.format(fmt).to_string()
        }
        chrono::LocalResult::None => String::new(),
    }
}

/// ISO-8601 to unix seconds.
pub fn parse_iso(s: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis() as f64 / 1000.0)
}

/// Human count: 1.2B, 3.4M, 12k.
pub fn human(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.1}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.1}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

/// Is a program on PATH?
pub fn which(prog: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(prog).is_file()))
        .unwrap_or(false)
}

/// Does a process with exactly this comm name run?
pub fn proc_running(comm: &str) -> bool {
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return false;
    };
    rd.flatten().any(|e| {
        let n = e.file_name();
        n.to_str()
            .is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit()))
            && read_trim(e.path().join("comm")).as_deref() == Some(comm)
    })
}

/// First process whose full command line starts with `prefix`.
pub fn proc_cmdline_starts(prefix: &str) -> Option<u32> {
    let rd = std::fs::read_dir("/proc").ok()?;
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if let Ok(raw) = std::fs::read(e.path().join("cmdline")) {
            let line = String::from_utf8_lossy(&raw).replace('\0', " ");
            if line.starts_with(prefix) {
                return Some(pid);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size_kb(40 * 1024), "40 MB");
        assert_eq!(size_kb(1_572_864), "1.5 GB");
        assert_eq!(size_kb(31 * 1_048_576), "31 GB");
        assert_eq!(size_kb(2 * 1_048_576), "2 GB");
        assert_eq!(size_kb(440 * 1_048_576), "440 GB");
    }

    #[test]
    fn run_times_out() {
        let t = Instant::now();
        assert!(run(&["sleep", "5"], Duration::from_millis(200)).is_none());
        assert!(t.elapsed() < Duration::from_secs(2));
        let o = run(&["echo", "hi"], Duration::from_secs(2)).unwrap();
        assert_eq!(o.stdout.trim(), "hi");
    }
}

/// A long-running command whose stdout lines are delivered on the GTK thread.
/// Restarted after `restart` if it exits; killed when the returned handle drops.
pub struct Stream {
    pid: std::sync::Arc<std::sync::atomic::AtomicI32>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::SeqCst);
        let pid = self.pid.load(Ordering::SeqCst);
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
    }
}

/// `on_line(Some(line))` per line, `on_line(None)` when the command ended.
pub fn stream(cmd: &str, restart: Duration, on_line: impl Fn(Option<String>) + 'static) -> Stream {
    use std::io::BufRead;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    let pid = Arc::new(AtomicI32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = async_channel::unbounded::<Option<String>>();
    let cmd = cmd.to_string();
    let (p2, s2) = (pid.clone(), stop.clone());
    std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(&cmd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            unsafe {
                use std::os::unix::process::CommandExt;
                c.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
            if let Ok(mut child) = c.spawn() {
                p2.store(child.id() as i32, Ordering::SeqCst);
                if let Some(out) = child.stdout.take() {
                    for line in std::io::BufReader::new(out).lines() {
                        let Ok(line) = line else { break };
                        if tx.send_blocking(Some(line)).is_err() {
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                p2.store(0, Ordering::SeqCst);
            }
            if tx.send_blocking(None).is_err() {
                return;
            }
            std::thread::sleep(restart);
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(m) = rx.recv().await {
            on_line(m);
        }
    });
    Stream { pid, stop }
}

/// One pass over /proc: the pid of the first process for each wanted `comm`
/// name (comm is at most 15 characters, so match prefixes of longer names).
pub fn find_comms(names: &[&str]) -> Vec<Option<u32>> {
    let mut out = vec![None; names.len()];
    let Ok(rd) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(comm) = read_trim(e.path().join("comm")) else {
            continue;
        };
        for (i, n) in names.iter().enumerate() {
            if out[i].is_none() && (comm == *n || (comm.len() == 15 && n.starts_with(&comm))) {
                out[i] = Some(pid);
            }
        }
        if out.iter().all(Option::is_some) {
            break;
        }
    }
    out
}

/// "2.6 / 7.9 GB": used and total in the total's unit (compact meter detail).
pub fn used_of(used_kb: u64, total_kb: u64) -> String {
    let t = size_kb(total_kb);
    let unit = t.rsplit(' ').next().unwrap_or("GB").to_string();
    let div = match unit.as_str() {
        "TB" => 1_073_741_824.0,
        "GB" => 1_048_576.0,
        _ => 1024.0,
    };
    let u = used_kb as f64 / div;
    let tn = total_kb as f64 / div;
    let f = |x: f64| {
        if x >= 100.0 {
            format!("{x:.0}")
        } else {
            format!("{x:.1}")
        }
    };
    format!("{} / {} {unit}", f(u), f(tn))
}
