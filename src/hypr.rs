//! Hyprland IPC: requests over `.socket.sock`, events from `.socket2.sock`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use crate::util;

fn dir() -> Option<PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    Some(util::runtime_dir().join("hypr").join(sig))
}

/// Sends one request ("j/workspaces", "dispatch workspace 3") and returns the reply.
pub fn request(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(dir()?.join(".socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

pub fn json(cmd: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&request(&format!("j/{cmd}"))?).ok()
}

pub fn dispatch(args: &str) {
    let a = args.to_string();
    std::thread::spawn(move || {
        let _ = request(&format!("dispatch {a}"));
    });
}

type Listener = Box<dyn Fn(&str, &str) -> bool>;

thread_local! {
    static LISTENERS: std::cell::RefCell<Vec<Listener>> = std::cell::RefCell::default();
    static STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Calls `f(name, data)` for every Hyprland event (`name>>data`). One socket
/// is shared by all listeners and reconnects if Hyprland restarts it.
/// `f` returns false once its owner is gone, which unregisters it.
pub fn events(f: impl Fn(&str, &str) -> bool + 'static) {
    LISTENERS.with(|l| l.borrow_mut().push(Box::new(f)));
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    let (tx, rx) = async_channel::unbounded::<String>();
    std::thread::spawn(move || {
        loop {
            if let Some(path) = dir().map(|d| d.join(".socket2.sock")) {
                if let Ok(s) = UnixStream::connect(path) {
                    for line in BufReader::new(s).lines() {
                        let Ok(line) = line else { break };
                        if tx.send_blocking(line).is_err() {
                            return;
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    glib::spawn_future_local(async move {
        while let Ok(line) = rx.recv().await {
            if let Some((name, data)) = line.split_once(">>") {
                let mut list = LISTENERS.with(|l| std::mem::take(&mut *l.borrow_mut()));
                list.retain(|f| f(name, data));
                // listeners added while dispatching went into the fresh list
                LISTENERS.with(|l| {
                    let mut cur = l.borrow_mut();
                    list.append(&mut cur);
                    *cur = list;
                });
            }
        }
    });
}
