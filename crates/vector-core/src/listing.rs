//! Reading a folder off the main thread, and watching it for changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gio::prelude::*;

use crate::entry::{ATTRIBUTES, Entry};

/// What a listing sends back, in order: batches, then `Done` or `Error`.
#[derive(Debug, Clone)]
pub enum ListEvent {
    /// Names from the folder's `.hidden` file; sent first.
    HiddenNames(std::collections::HashSet<String>),
    Batch(Vec<Entry>),
    /// Files looked at so far, for long scans (`biggest`).
    Progress(u64),
    Done,
    /// The folder could not be opened or read. Entries already sent stay valid.
    Error(String),
}

/// A listing in progress. Dropping it cancels the worker.
pub struct Listing {
    pub dir: PathBuf,
    pub events: async_channel::Receiver<ListEvent>,
    cancel: gio::Cancellable,
}

impl Drop for Listing {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The first batch is small so the view paints quickly; later ones are larger and sent at
/// most every 60 ms so a huge folder doesn't flood the main loop.
const FIRST_BATCH: usize = 300;
const BATCH: usize = 5000;
const BATCH_INTERVAL: Duration = Duration::from_millis(60);
const MAX_ERRORS_IN_A_ROW: usize = 64;

/// Names listed in a folder's `.hidden` file (one per line). Blocking; empty when missing.
pub fn read_hidden_file(dir: &Path) -> std::collections::HashSet<String> {
    std::fs::read_to_string(dir.join(".hidden"))
        .map(|t| parse_hidden_file(&t))
        .unwrap_or_default()
}

pub fn parse_hidden_file(text: &str) -> std::collections::HashSet<String> {
    text.lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Starts listing `dir` on a worker thread.
pub fn spawn(dir: &Path) -> Listing {
    let (tx, rx) = async_channel::unbounded();
    let cancel = gio::Cancellable::new();
    let worker_cancel = cancel.clone();
    let path = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("vector-list".into())
        .spawn(move || {
            let _ = tx.send_blocking(ListEvent::HiddenNames(read_hidden_file(&path)));
            let last = match list_blocking(&path, &worker_cancel, |batch| {
                tx.send_blocking(ListEvent::Batch(batch)).is_ok()
            }) {
                Ok(()) => ListEvent::Done,
                Err(e) => ListEvent::Error(e.message().to_string()),
            };
            if !worker_cancel.is_cancelled() {
                let _ = tx.send_blocking(last);
            }
        });
    if let Err(e) = spawned {
        // can't happen in practice; report it like any other listing failure
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(ListEvent::Error(e.to_string()));
        return Listing {
            dir: dir.to_path_buf(),
            events: rx,
            cancel,
        };
    }
    Listing {
        dir: dir.to_path_buf(),
        events: rx,
        cancel,
    }
}

/// Lists `dir`, handing batches to `send` until it returns false. Blocking.
pub fn list_blocking(
    dir: &Path,
    cancel: &gio::Cancellable,
    mut send: impl FnMut(Vec<Entry>) -> bool,
) -> Result<(), glib::Error> {
    let file = gio::File::for_path(dir);
    let en = file.enumerate_children(ATTRIBUTES, gio::FileQueryInfoFlags::NONE, Some(cancel))?;
    let mut batch = Vec::with_capacity(FIRST_BATCH);
    let mut limit = FIRST_BATCH;
    let mut last_send = Instant::now();
    let mut errors_in_a_row = 0;
    loop {
        match en.next_file(Some(cancel)) {
            Ok(Some(info)) => {
                errors_in_a_row = 0;
                batch.push(Entry::from_info(dir, &info));
            }
            Ok(None) => break,
            Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => return Err(e),
            // one unreadable entry (e.g. a stale NAS file) must not end the listing, but an
            // enumerator that only fails any more (I/O error, dead mount) must
            Err(e) => {
                errors_in_a_row += 1;
                if errors_in_a_row >= MAX_ERRORS_IN_A_ROW {
                    return Err(e);
                }
                continue;
            }
        }
        if batch.len() >= limit || last_send.elapsed() >= BATCH_INTERVAL {
            if !send(std::mem::take(&mut batch)) {
                return Ok(());
            }
            limit = BATCH;
            last_send = Instant::now();
        }
    }
    let _ = en.close(gio::Cancellable::NONE);
    if !batch.is_empty() {
        send(batch);
    }
    Ok(())
}

/// At most this many entries in a flat listing; it says so with an error at the end.
pub const FLAT_LIMIT: usize = 200_000;

/// Lists every file under `dir` (recursively, not following symlinks, folders themselves left
/// out) as one flat list; entry names are paths relative to `dir`. Dot files only with
/// `hidden`. Same events as [`spawn`].
pub fn spawn_flat(dir: &Path, hidden: bool) -> Listing {
    let (tx, rx) = async_channel::unbounded();
    let cancel = gio::Cancellable::new();
    let worker_cancel = cancel.clone();
    let root = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("vector-flat".into())
        .spawn(move || {
            let mut batch = Vec::with_capacity(FIRST_BATCH);
            let mut limit = FIRST_BATCH;
            let mut count = 0usize;
            let mut last_send = Instant::now();
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(!hidden)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .ignore(false)
                .parents(false)
                .follow_links(false)
                .build();
            for item in walker {
                if worker_cancel.is_cancelled() {
                    return;
                }
                let Ok(item) = item else { continue };
                if item.depth() == 0 || item.file_type().is_some_and(|t| t.is_dir()) {
                    continue;
                }
                let Ok(mut e) = Entry::query(item.path()) else {
                    continue;
                };
                let Ok(rel) = item.path().strip_prefix(&root) else {
                    continue;
                };
                e.name = rel.to_string_lossy().into_owned();
                batch.push(e);
                count += 1;
                if batch.len() >= limit || last_send.elapsed() >= BATCH_INTERVAL {
                    if tx
                        .send_blocking(ListEvent::Batch(std::mem::take(&mut batch)))
                        .is_err()
                    {
                        return;
                    }
                    limit = BATCH;
                    last_send = Instant::now();
                }
                if count >= FLAT_LIMIT {
                    let _ = tx.send_blocking(ListEvent::Batch(std::mem::take(&mut batch)));
                    let _ = tx.send_blocking(ListEvent::Error(format!(
                        "showing the first {FLAT_LIMIT} files"
                    )));
                    return;
                }
            }
            if !batch.is_empty() {
                let _ = tx.send_blocking(ListEvent::Batch(batch));
            }
            let _ = tx.send_blocking(ListEvent::Done);
        });
    if let Err(e) = spawned {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(ListEvent::Error(e.to_string()));
        return Listing {
            dir: dir.to_path_buf(),
            events: rx,
            cancel,
        };
    }
    Listing {
        dir: dir.to_path_buf(),
        events: rx,
        cancel,
    }
}

/// The `n` biggest files under `roots` as a listing, biggest first. With one root, names are
/// paths relative to it; with several, absolute paths. `one_file_system` keeps each walk on its
/// root's file system. Sends progress while it walks.
pub fn spawn_biggest(roots: Vec<PathBuf>, n: usize, one_file_system: bool) -> Listing {
    let (tx, rx) = async_channel::unbounded();
    let cancel = gio::Cancellable::new();
    let worker_cancel = cancel.clone();
    let dir = roots.first().cloned().unwrap_or_else(|| PathBuf::from("/"));
    let spawned = std::thread::Builder::new()
        .name("vector-biggest".into())
        .spawn(move || {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
            let stop = Arc::new(AtomicBool::new(false));
            let seen = Arc::new(AtomicU64::new(0));
            // cancel the walk when the listing is dropped; report progress meanwhile
            let (s2, seen2, c2, tx2) = (
                stop.clone(),
                seen.clone(),
                worker_cancel.clone(),
                tx.clone(),
            );
            let ticker = std::thread::spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    if c2.is_cancelled() {
                        s2.store(true, Ordering::Relaxed);
                        break;
                    }
                    let _ = tx2.send_blocking(ListEvent::Progress(seen2.load(Ordering::Relaxed)));
                    std::thread::sleep(Duration::from_millis(200));
                }
            });
            let mut all: Vec<(PathBuf, u64)> = Vec::new();
            for root in &roots {
                all.extend(crate::du::biggest_files(
                    root,
                    n,
                    one_file_system,
                    &stop,
                    &seen,
                ));
            }
            all.sort_by_key(|b| std::cmp::Reverse(b.1));
            all.truncate(n);
            let cancelled = stop.swap(true, Ordering::Relaxed);
            let _ = ticker.join();
            if cancelled {
                return;
            }
            let single = roots.len() == 1;
            let entries: Vec<Entry> = all
                .into_iter()
                .filter_map(|(p, _)| {
                    let mut e = Entry::query(&p).ok()?;
                    e.name = match (single, p.strip_prefix(&roots[0])) {
                        (true, Ok(rel)) => rel.to_string_lossy().into_owned(),
                        _ => p.to_string_lossy().into_owned(),
                    };
                    Some(e)
                })
                .collect();
            let _ = tx.send_blocking(ListEvent::Batch(entries));
            let _ = tx.send_blocking(ListEvent::Done);
        });
    if let Err(e) = spawned {
        let (tx, rx) = async_channel::bounded(1);
        let _ = tx.try_send(ListEvent::Error(e.to_string()));
        return Listing {
            dir,
            events: rx,
            cancel,
        };
    }
    Listing {
        dir,
        events: rx,
        cancel,
    }
}

/// A change to a watched folder.
#[derive(Debug, Clone, PartialEq)]
pub enum DirChange {
    /// Created or changed: insert or replace by name.
    Upsert(Entry),
    Remove(String),
    /// The folder itself was deleted, moved away or unmounted.
    Gone,
}

/// Watches one folder. Changes are delivered on the thread-default main context of the thread
/// that called [`watch`]. Dropping it stops watching.
pub struct Watcher {
    monitor: gio::FileMonitor,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.monitor.cancel();
    }
}

/// Starts watching `dir`. New and changed entries are queried asynchronously, so the callback
/// never waits on I/O.
pub fn watch(dir: &Path, on_change: impl Fn(DirChange) + 'static) -> Result<Watcher, glib::Error> {
    let folder = gio::File::for_path(dir);
    let monitor =
        folder.monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)?;
    let dir = dir.to_path_buf();
    let on_change = std::rc::Rc::new(on_change);
    let gens: Gens = Gens::default();
    monitor.connect_changed(move |_, file, other, event| {
        use gio::FileMonitorEvent as E;
        let is_self = file.path().as_deref() == Some(dir.as_path());
        match event {
            E::Deleted | E::MovedOut | E::Unmounted if is_self => on_change(DirChange::Gone),
            E::Deleted | E::MovedOut => {
                if let Some(name) = file.basename() {
                    remove(&gens, &*on_change, name.to_string_lossy().into_owned());
                }
            }
            E::Renamed => {
                if is_self {
                    on_change(DirChange::Gone);
                    return;
                }
                if let Some(name) = file.basename() {
                    remove(&gens, &*on_change, name.to_string_lossy().into_owned());
                }
                if let Some(new) = other
                    && new.parent().and_then(|p| p.path()).as_deref() == Some(dir.as_path())
                {
                    query_and_send(&dir, new, &gens, on_change.clone());
                }
            }
            E::Created | E::ChangesDoneHint | E::AttributeChanged | E::MovedIn if !is_self => {
                query_and_send(&dir, file, &gens, on_change.clone());
            }
            _ => {}
        }
    });
    Ok(Watcher { monitor })
}

/// Per-name counters, bumped on every event for a name. An async query whose name was touched
/// again before it finished is dropped, so a slow answer can't resurrect a removed entry.
type Gens = std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, u64>>>;

fn bump(gens: &Gens, name: &str) -> u64 {
    let mut g = gens.borrow_mut();
    let n = g.entry(name.to_string()).or_insert(0);
    *n += 1;
    *n
}

fn remove(gens: &Gens, on_change: &dyn Fn(DirChange), name: String) {
    bump(gens, &name);
    on_change(DirChange::Remove(name));
}

fn query_and_send(
    dir: &Path,
    file: &gio::File,
    gens: &Gens,
    on_change: std::rc::Rc<dyn Fn(DirChange)>,
) {
    let dir = dir.to_path_buf();
    let name = file.basename().map(|n| n.to_string_lossy().into_owned());
    let started = name.as_deref().map(|n| bump(gens, n));
    let gens = gens.clone();
    file.query_info_async(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NONE,
        glib::Priority::DEFAULT,
        gio::Cancellable::NONE,
        move |res| {
            let Some(name) = name else { return };
            let current = gens.borrow().get(&name).copied();
            if current != started {
                return;
            }
            gens.borrow_mut().remove(&name);
            match res {
                Ok(info) => on_change(DirChange::Upsert(Entry::from_info(&dir, &info))),
                // gone again before we could look at it
                Err(_) => on_change(DirChange::Remove(name)),
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::Kind;
    use crate::test_support::scratch;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn collect(dir: &Path) -> (Vec<Entry>, usize) {
        let mut all = Vec::new();
        let mut batches = 0;
        list_blocking(dir, &gio::Cancellable::new(), |b| {
            batches += 1;
            all.extend(b);
            true
        })
        .expect("listing");
        (all, batches)
    }

    #[test]
    fn lists_kinds_symlinks_and_hidden() {
        let t = scratch();
        let d = t.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "hello").unwrap();
        std::fs::write(d.join(".dot"), "").unwrap();
        std::os::unix::fs::symlink("sub", d.join("link-dir")).unwrap();
        std::os::unix::fs::symlink("missing", d.join("broken")).unwrap();
        std::fs::write(d.join("ünï 日本.txt"), "").unwrap();
        let (mut v, _) = collect(d);
        v.sort_by(|a, b| a.name.cmp(&b.name));
        let get = |n: &str| v.iter().find(|e| e.name == n).unwrap();
        assert_eq!(v.len(), 6);
        assert_eq!(get("sub").kind, Kind::Dir);
        assert_eq!(get("a.txt").size, 5);
        assert!(get("a.txt").modified > 0);
        assert_eq!(get("a.txt").content_type, "text/plain");
        assert!(get(".dot").hidden);
        assert!(get("link-dir").symlink && get("link-dir").is_dir() && !get("link-dir").broken);
        assert!(get("broken").symlink && get("broken").broken);
        assert_eq!(get("ünï 日本.txt").path, d.join("ünï 日本.txt"));
    }

    #[test]
    fn big_folder_comes_in_batches() {
        let t = scratch();
        for i in 0..12_000 {
            std::fs::write(t.path().join(format!("f{i}")), "").unwrap();
        }
        let (v, batches) = collect(t.path());
        assert_eq!(v.len(), 12_000);
        assert!(batches >= 3, "{batches} batches");
    }

    #[test]
    fn missing_folder_is_an_error() {
        let t = scratch();
        assert!(list_blocking(&t.path().join("nope"), &gio::Cancellable::new(), |_| true).is_err());
    }

    #[test]
    fn spawn_delivers_everything_then_done() {
        let t = scratch();
        std::fs::write(t.path().join("x"), "").unwrap();
        let l = spawn(t.path());
        let mut n = 0;
        loop {
            match l.events.recv_blocking().unwrap() {
                ListEvent::Batch(b) => n += b.len(),
                ListEvent::HiddenNames(_) | ListEvent::Progress(_) => {}
                ListEvent::Done => break,
                ListEvent::Error(e) => panic!("{e}"),
            }
        }
        assert_eq!(n, 1);
        let bad = spawn(&t.path().join("nope"));
        assert!(matches!(
            bad.events.recv_blocking().unwrap(),
            ListEvent::HiddenNames(_)
        ));
        assert!(matches!(
            bad.events.recv_blocking().unwrap(),
            ListEvent::Error(_)
        ));
    }

    /// Runs `ctx` until `done` holds or 5 s pass.
    fn pump(ctx: &glib::MainContext, done: impl Fn() -> bool) {
        let start = Instant::now();
        while !done() && start.elapsed() < Duration::from_secs(5) {
            ctx.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn watcher_sees_create_change_rename_delete_and_gone() {
        let t = scratch();
        let d = t.path().join("w");
        std::fs::create_dir(&d).unwrap();
        let ctx = glib::MainContext::new();
        ctx.with_thread_default(|| {
            let seen: Rc<RefCell<Vec<DirChange>>> = Rc::default();
            let s = seen.clone();
            let _w = watch(&d, move |c| s.borrow_mut().push(c)).unwrap();
            let has = |f: &dyn Fn(&DirChange) -> bool| seen.borrow().iter().any(f);

            std::fs::write(d.join("new.txt"), "x").unwrap();
            pump(&ctx, || {
                has(&|c| matches!(c, DirChange::Upsert(e) if e.name == "new.txt"))
            });
            assert!(has(
                &|c| matches!(c, DirChange::Upsert(e) if e.name == "new.txt")
            ));

            std::fs::rename(d.join("new.txt"), d.join("renamed.txt")).unwrap();
            pump(&ctx, || {
                has(&|c| matches!(c, DirChange::Upsert(e) if e.name == "renamed.txt"))
            });
            assert!(has(&|c| *c == DirChange::Remove("new.txt".into())));
            assert!(has(
                &|c| matches!(c, DirChange::Upsert(e) if e.name == "renamed.txt")
            ));

            std::fs::remove_file(d.join("renamed.txt")).unwrap();
            pump(&ctx, || {
                has(&|c| *c == DirChange::Remove("renamed.txt".into()))
            });
            assert!(has(&|c| *c == DirChange::Remove("renamed.txt".into())));

            std::fs::remove_dir(&d).unwrap();
            pump(&ctx, || has(&|c| *c == DirChange::Gone));
            assert!(has(&|c| *c == DirChange::Gone));
        })
        .unwrap();
    }

    #[test]
    fn hidden_file_parsing() {
        let set = parse_hidden_file("a b\r\n\nc.txt\n");
        assert_eq!(set, ["a b".to_string(), "c.txt".to_string()].into());
        let t = scratch();
        std::fs::write(t.path().join(".hidden"), "x\n").unwrap();
        assert!(read_hidden_file(t.path()).contains("x"));
        assert!(read_hidden_file(&t.path().join("none")).is_empty());
    }

    #[test]
    fn flat_listing_walks_the_tree() {
        let t = scratch();
        let d = t.path();
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join(".dot")).unwrap();
        std::fs::write(d.join("top.exe"), "").unwrap();
        std::fs::write(d.join("a/b/deep.exe"), "").unwrap();
        std::fs::write(d.join(".dot/secret"), "").unwrap();
        std::os::unix::fs::symlink(d.join("a"), d.join("loop")).unwrap();
        let collect = |hidden| {
            let l = spawn_flat(d, hidden);
            let mut names = Vec::new();
            loop {
                match l.events.recv_blocking().unwrap() {
                    ListEvent::Batch(b) => names.extend(b.into_iter().map(|e| e.name)),
                    ListEvent::Progress(_) => {}
                    ListEvent::Done => break,
                    other => panic!("{other:?}"),
                }
            }
            names.sort();
            names
        };
        assert_eq!(collect(false), ["a/b/deep.exe", "loop", "top.exe"]);
        assert_eq!(
            collect(true),
            [".dot/secret", "a/b/deep.exe", "loop", "top.exe"]
        );
    }

    #[test]
    fn biggest_listing() {
        let t = scratch();
        let d = t.path();
        std::fs::create_dir_all(d.join("x")).unwrap();
        std::fs::write(d.join("x/big"), vec![0u8; 5000]).unwrap();
        std::fs::write(d.join("small"), vec![0u8; 10]).unwrap();
        std::fs::write(d.join("mid"), vec![0u8; 500]).unwrap();
        let l = spawn_biggest(vec![d.to_path_buf()], 2, true);
        let mut names = Vec::new();
        loop {
            match l.events.recv_blocking().unwrap() {
                ListEvent::Batch(b) => names.extend(b.into_iter().map(|e| (e.name, e.size))),
                ListEvent::Progress(_) => {}
                ListEvent::Done => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(
            names,
            [("x/big".to_string(), 5000), ("mid".to_string(), 500)]
        );
    }
}
