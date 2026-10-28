//! Reading a folder off the main thread, and watching it for changes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gio::prelude::*;

use crate::entry::{ATTRIBUTES, Entry};

/// What a listing sends back, in order: batches, then `Done` or `Error`.
#[derive(Debug, Clone)]
pub enum ListEvent {
    Batch(Vec<Entry>),
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

/// Starts listing `dir` on a worker thread.
pub fn spawn(dir: &Path) -> Listing {
    let (tx, rx) = async_channel::unbounded();
    let cancel = gio::Cancellable::new();
    let worker_cancel = cancel.clone();
    let path = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("strata-list".into())
        .spawn(move || {
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
    loop {
        match en.next_file(Some(cancel)) {
            Ok(Some(info)) => batch.push(Entry::from_info(dir, &info)),
            Ok(None) => break,
            // one unreadable entry (e.g. a stale NAS file) must not end the listing
            Err(e) if e.matches(gio::IOErrorEnum::Cancelled) => return Err(e),
            Err(_) => continue,
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
    monitor.connect_changed(move |_, file, other, event| {
        use gio::FileMonitorEvent as E;
        let is_self = file.path().as_deref() == Some(dir.as_path());
        match event {
            E::Deleted | E::MovedOut | E::Unmounted if is_self => on_change(DirChange::Gone),
            E::Deleted | E::MovedOut => {
                if let Some(name) = file.basename() {
                    on_change(DirChange::Remove(name.to_string_lossy().into_owned()));
                }
            }
            E::Renamed => {
                if is_self {
                    on_change(DirChange::Gone);
                    return;
                }
                if let Some(name) = file.basename() {
                    on_change(DirChange::Remove(name.to_string_lossy().into_owned()));
                }
                if let Some(new) = other
                    && new.parent().and_then(|p| p.path()).as_deref() == Some(dir.as_path())
                {
                    query_and_send(&dir, new, on_change.clone());
                }
            }
            E::Created | E::ChangesDoneHint | E::AttributeChanged | E::MovedIn if !is_self => {
                query_and_send(&dir, file, on_change.clone());
            }
            _ => {}
        }
    });
    Ok(Watcher { monitor })
}

fn query_and_send(dir: &Path, file: &gio::File, on_change: std::rc::Rc<dyn Fn(DirChange)>) {
    let dir = dir.to_path_buf();
    let name = file.basename().map(|n| n.to_string_lossy().into_owned());
    file.query_info_async(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NONE,
        glib::Priority::DEFAULT,
        gio::Cancellable::NONE,
        move |res| match res {
            Ok(info) => on_change(DirChange::Upsert(Entry::from_info(&dir, &info))),
            // gone again before we could look at it
            Err(_) => {
                if let Some(name) = name {
                    on_change(DirChange::Remove(name));
                }
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
                ListEvent::Done => break,
                ListEvent::Error(e) => panic!("{e}"),
            }
        }
        assert_eq!(n, 1);
        let bad = spawn(&t.path().join("nope"));
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
}
