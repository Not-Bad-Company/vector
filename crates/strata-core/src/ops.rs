//! The operation queue: copy, move, trash, permanent delete, rename, new folder, new file and
//! `:edit` plans, run one at a time on a worker thread with GIO, with progress events and undo
//! for trash, rename and move.
//!
//! Nothing here asks the user anything. Callers decide conflicts up front ([`conflicts`] lists
//! them) and confirm permanent deletes and overwrites before submitting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gio::prelude::*;

use crate::edit::{Change, Plan};

pub type OpId = u64;

/// What to do when the destination name is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Skip,
    /// Replace files; merge folders into the existing folder.
    Overwrite,
    /// Pick a free name like `name (2).txt`.
    KeepBoth,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Copy {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        conflict: Conflict,
    },
    Move {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        conflict: Conflict,
    },
    Trash {
        paths: Vec<PathBuf>,
    },
    /// Cannot be undone. Only submit after the user confirmed.
    DeletePermanently {
        paths: Vec<PathBuf>,
    },
    Rename {
        path: PathBuf,
        new_name: String,
    },
    Mkdir {
        path: PathBuf,
    },
    NewFile {
        path: PathBuf,
    },
    /// Applies an `:edit` plan inside `dir`, stopping at the first failure.
    ApplyEdit {
        dir: PathBuf,
        plan: Plan,
    },
    Undo(Undo),
}

impl Op {
    /// Short lowercase description for the status line.
    pub fn label(&self) -> String {
        fn n(v: &[PathBuf]) -> String {
            match v {
                [one] => name_of(one),
                _ => format!("{} items", v.len()),
            }
        }
        match self {
            Op::Copy { sources, .. } => format!("copy {}", n(sources)),
            Op::Move { sources, .. } => format!("move {}", n(sources)),
            Op::Trash { paths } => format!("trash {}", n(paths)),
            Op::DeletePermanently { paths } => format!("delete {}", n(paths)),
            Op::Rename { path, new_name } => format!("rename {} → {new_name}", name_of(path)),
            Op::Mkdir { path } => format!("new folder {}", name_of(path)),
            Op::NewFile { path } => format!("new file {}", name_of(path)),
            Op::ApplyEdit { plan, .. } => format!("apply {} changes", plan.changes.len()),
            Op::Undo(_) => "undo".into(),
        }
    }
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Where a trashed item went, so it can be put back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trashed {
    pub orig: PathBuf,
    /// The item inside `Trash/files`.
    pub file: PathBuf,
    /// Its `.trashinfo` file.
    pub info: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStep {
    Restore(Trashed),
    /// Move `now` back to `back`.
    MoveBack {
        now: PathBuf,
        back: PathBuf,
    },
    /// Remove a file or folder this operation created, only if it is still empty.
    RemoveCreated(PathBuf),
}

/// How to undo one finished operation; steps run in reverse order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Undo {
    pub label: String,
    pub steps: Vec<UndoStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The file system has no trash (e.g. /tmp, some network mounts). Offer a permanent delete.
    TrashUnsupported,
    Exists,
    NotFound,
    Cancelled,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError {
    pub path: PathBuf,
    pub kind: ErrorKind,
    pub message: String,
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", name_of(&self.path), self.message)
    }
}

impl OpError {
    fn new(path: &Path, kind: ErrorKind, message: impl Into<String>) -> OpError {
        OpError {
            path: path.to_path_buf(),
            kind,
            message: message.into(),
        }
    }

    fn from_glib(path: &Path, e: &glib::Error) -> OpError {
        let kind = if e.matches(gio::IOErrorEnum::Cancelled) {
            ErrorKind::Cancelled
        } else if e.matches(gio::IOErrorEnum::Exists) {
            ErrorKind::Exists
        } else if e.matches(gio::IOErrorEnum::NotFound) {
            ErrorKind::NotFound
        } else {
            ErrorKind::Other
        };
        OpError::new(path, kind, e.message())
    }
}

/// The result of one operation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub label: String,
    pub errors: Vec<OpError>,
    /// Paths that now exist because of this operation (paste targets, new files), so the view
    /// can select them.
    pub created: Vec<PathBuf>,
    pub cancelled: bool,
    /// Whether an undo record was pushed.
    pub undoable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpEvent {
    Started {
        id: OpId,
        label: String,
    },
    /// `done`/`total` are bytes for copies and items otherwise.
    Progress {
        id: OpId,
        done: u64,
        total: u64,
        current: String,
    },
    Finished {
        id: OpId,
        report: Report,
    },
}

struct Job {
    id: OpId,
    op: Op,
    cancel: gio::Cancellable,
}

/// Runs operations in submission order on one worker thread.
pub struct OpQueue {
    jobs: async_channel::Sender<Job>,
    pub events: async_channel::Receiver<OpEvent>,
    next: AtomicU64,
    running: Arc<Mutex<HashMap<OpId, gio::Cancellable>>>,
    undo: Arc<Mutex<Vec<Undo>>>,
}

impl Default for OpQueue {
    fn default() -> Self {
        OpQueue::new()
    }
}

impl OpQueue {
    pub fn new() -> OpQueue {
        let (jobs, job_rx) = async_channel::unbounded::<Job>();
        let (tx, events) = async_channel::unbounded();
        let running: Arc<Mutex<HashMap<OpId, gio::Cancellable>>> = Arc::default();
        let undo: Arc<Mutex<Vec<Undo>>> = Arc::default();
        let (w_running, w_undo) = (running.clone(), undo.clone());
        let spawned = std::thread::Builder::new()
            .name("strata-ops".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv_blocking() {
                    let label = job.op.label();
                    let _ = tx.send_blocking(OpEvent::Started {
                        id: job.id,
                        label: label.clone(),
                    });
                    let mut w = Worker::new(job.id, &tx, &job.cancel);
                    let (mut report, record) = w.run(job.op);
                    report.label = label;
                    report.cancelled = job.cancel.is_cancelled();
                    if let Some(record) = record.filter(|u| !u.steps.is_empty())
                        && let Ok(mut u) = w_undo.lock()
                    {
                        u.push(record);
                        report.undoable = true;
                    }
                    if let Ok(mut r) = w_running.lock() {
                        r.remove(&job.id);
                    }
                    let _ = tx.send_blocking(OpEvent::Finished { id: job.id, report });
                }
            });
        if spawned.is_err() {
            // no worker: every submit will fail to send and report nothing; the queue stays
            // usable as a value so the UI does not have to handle this separately
            jobs.close();
        }
        OpQueue {
            jobs,
            events,
            next: AtomicU64::new(1),
            running,
            undo,
        }
    }

    /// Queues `op`; returns its id, or `None` if the worker is gone.
    pub fn submit(&self, op: Op) -> Option<OpId> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let cancel = gio::Cancellable::new();
        if let Ok(mut r) = self.running.lock() {
            r.insert(id, cancel.clone());
        }
        self.jobs.try_send(Job { id, op, cancel }).ok().map(|()| id)
    }

    /// Cancels a queued or running operation. What finished already stays done.
    pub fn cancel(&self, id: OpId) {
        if let Ok(r) = self.running.lock()
            && let Some(c) = r.get(&id)
        {
            c.cancel();
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo.lock().map(|u| !u.is_empty()).unwrap_or(false)
    }

    /// Label of what `undo` would revert.
    pub fn undo_label(&self) -> Option<String> {
        self.undo
            .lock()
            .ok()
            .and_then(|u| u.last().map(|r| r.label.clone()))
    }

    /// Queues the reversal of the last undoable operation.
    pub fn undo(&self) -> Option<OpId> {
        let record = self.undo.lock().ok()?.pop()?;
        self.submit(Op::Undo(record))
    }
}

/// Blocking: names under `dest` that `sources` would collide with (same-folder copies excluded,
/// they always get a new name).
pub fn conflicts(sources: &[PathBuf], dest: &Path) -> Vec<PathBuf> {
    sources
        .iter()
        .filter_map(|s| {
            let target = dest.join(s.file_name()?);
            (target != *s && exists(&target)).then_some(target)
        })
        .collect()
}

fn exists(p: &Path) -> bool {
    // symlink_metadata: a broken link is still a name that is taken
    std::fs::symlink_metadata(p).is_ok()
}

/// `name (2).ext`, `name (3).ext`, … — the first that is free in `dir`.
pub fn free_name(dir: &Path, name: &str) -> String {
    let (stem, ext) = split_ext(name);
    (2..)
        .map(|i| format!("{stem} ({i}){ext}"))
        .find(|n| !exists(&dir.join(n)))
        .unwrap_or_else(|| name.to_string())
}

/// Splits `report.tar.gz` into `report` + `.tar.gz`, `a.txt` into `a` + `.txt`. Dotfiles and
/// names without a dot have no extension.
fn split_ext(name: &str) -> (&str, &str) {
    let Some(dot) = name.rfind('.').filter(|&i| i > 0) else {
        return (name, "");
    };
    let (stem, ext) = name.split_at(dot);
    if let Some(tar) = stem.strip_suffix(".tar").filter(|s| !s.is_empty()) {
        return (tar, &name[tar.len()..]);
    }
    (stem, ext)
}

const PROGRESS_EVERY: Duration = Duration::from_millis(50);

struct Worker<'a> {
    id: OpId,
    tx: &'a async_channel::Sender<OpEvent>,
    cancel: &'a gio::Cancellable,
    done: u64,
    total: u64,
    last: Instant,
    report: Report,
    undo: Undo,
}

const NOFOLLOW: gio::FileQueryInfoFlags = gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS;

impl<'a> Worker<'a> {
    fn new(id: OpId, tx: &'a async_channel::Sender<OpEvent>, cancel: &'a gio::Cancellable) -> Self {
        Worker {
            id,
            tx,
            cancel,
            done: 0,
            total: 0,
            last: Instant::now() - PROGRESS_EVERY,
            report: Report::default(),
            undo: Undo::default(),
        }
    }

    fn run(&mut self, op: Op) -> (Report, Option<Undo>) {
        self.undo.label = op.label();
        let undoable = matches!(
            op,
            Op::Trash { .. }
                | Op::Rename { .. }
                | Op::Move { .. }
                | Op::ApplyEdit { .. }
                | Op::Undo(_)
        );
        match op {
            Op::Copy {
                sources,
                dest,
                conflict,
            } => self.copy_all(&sources, &dest, conflict),
            Op::Move {
                sources,
                dest,
                conflict,
            } => self.move_all(&sources, &dest, conflict),
            Op::Trash { paths } => {
                self.total = paths.len() as u64;
                for p in &paths {
                    if self.cancelled() {
                        break;
                    }
                    if let Err(e) = self.trash(p) {
                        self.report.errors.push(e);
                    }
                    self.step(1, p);
                }
            }
            Op::DeletePermanently { paths } => {
                self.total = paths.len() as u64;
                for p in &paths {
                    if self.cancelled() {
                        break;
                    }
                    if let Err(e) = self.delete_tree(&gio::File::for_path(p), p) {
                        self.report.errors.push(e);
                    }
                    self.step(1, p);
                }
            }
            Op::Rename { path, new_name } => {
                if let Err(e) = self.rename(&path, &new_name) {
                    self.report.errors.push(e);
                }
            }
            Op::Mkdir { path } => {
                match gio::File::for_path(&path).make_directory_with_parents(Some(self.cancel)) {
                    Ok(()) => self.report.created.push(path),
                    Err(e) => self.report.errors.push(OpError::from_glib(&path, &e)),
                }
            }
            Op::NewFile { path } => match self.new_file(&path) {
                Ok(()) => self.report.created.push(path),
                Err(e) => self.report.errors.push(e),
            },
            Op::ApplyEdit { dir, plan } => self.apply_edit(&dir, &plan),
            Op::Undo(record) => self.undo_steps(record),
        }
        if self.total > 0 {
            let _ = self.tx.send_blocking(OpEvent::Progress {
                id: self.id,
                done: self.done.min(self.total),
                total: self.total,
                current: String::new(),
            });
        }
        let report = std::mem::take(&mut self.report);
        let undo = undoable.then(|| std::mem::take(&mut self.undo));
        (report, undo)
    }

    fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn progress(&mut self, current: &Path) {
        if self.last.elapsed() >= PROGRESS_EVERY {
            self.last = Instant::now();
            let _ = self.tx.send_blocking(OpEvent::Progress {
                id: self.id,
                done: self.done,
                total: self.total,
                current: name_of(current),
            });
        }
    }

    fn step(&mut self, n: u64, current: &Path) {
        self.done += n;
        self.progress(current);
    }

    // ── copy ────────────────────────────────────────────────────────────────

    fn copy_all(&mut self, sources: &[PathBuf], dest: &Path, conflict: Conflict) {
        self.total = sources.iter().map(|s| tree_size(s)).sum();
        for src in sources {
            if self.cancelled() {
                break;
            }
            let Some(name) = src.file_name() else {
                self.report
                    .errors
                    .push(OpError::new(src, ErrorKind::Other, "no file name"));
                continue;
            };
            if inside(dest, src) {
                self.report.errors.push(OpError::new(
                    src,
                    ErrorKind::Other,
                    "can't copy a folder into itself",
                ));
                continue;
            }
            let mut target = dest.join(name);
            let mut overwrite = false;
            if exists(&target) {
                match conflict {
                    _ if target == *src => target = dest.join(free_name(dest, &name_of(src))),
                    Conflict::Skip => continue,
                    Conflict::KeepBoth => target = dest.join(free_name(dest, &name_of(src))),
                    Conflict::Overwrite => overwrite = true,
                }
            }
            match self.copy_tree(src, &target, overwrite) {
                Ok(()) => self.report.created.push(target),
                Err(e) => self.report.errors.push(e),
            }
        }
    }

    fn copy_tree(&mut self, src: &Path, dst: &Path, overwrite: bool) -> Result<(), OpError> {
        let sf = gio::File::for_path(src);
        let df = gio::File::for_path(dst);
        let info = sf
            .query_info("standard::type", NOFOLLOW, Some(self.cancel))
            .map_err(|e| OpError::from_glib(src, &e))?;
        if info.file_type() == gio::FileType::Directory {
            match df.make_directory(Some(self.cancel)) {
                Ok(()) => {}
                // merging into an existing folder
                Err(e) if e.matches(gio::IOErrorEnum::Exists) && overwrite && is_real_dir(dst) => {}
                Err(e) => return Err(OpError::from_glib(dst, &e)),
            }
            let children = list_names(&sf, self.cancel).map_err(|e| OpError::from_glib(src, &e))?;
            let mut first_err = None;
            for child in children {
                if self.cancelled() {
                    return Err(OpError::new(src, ErrorKind::Cancelled, "cancelled"));
                }
                if let Err(e) = self.copy_tree(&src.join(&child), &dst.join(&child), overwrite) {
                    if e.kind == ErrorKind::Cancelled {
                        return Err(e);
                    }
                    first_err.get_or_insert(e);
                }
            }
            let _ = sf.copy_attributes(
                &df,
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                Some(self.cancel),
            );
            return first_err.map_or(Ok(()), Err);
        }
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        if overwrite {
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        let base = self.done;
        let (tx, id, total, cancel) = (self.tx, self.id, self.total, self.cancel);
        let mut last = self.last;
        let current = name_of(src);
        let mut copied = 0u64;
        let res = sf.copy(
            &df,
            flags,
            Some(cancel),
            Some(&mut |cur: i64, _tot: i64| {
                copied = u64::try_from(cur).unwrap_or(0);
                if last.elapsed() >= PROGRESS_EVERY {
                    last = Instant::now();
                    let _ = tx.send_blocking(OpEvent::Progress {
                        id,
                        done: base + copied,
                        total,
                        current: current.clone(),
                    });
                }
            }),
        );
        self.last = last;
        res.map_err(|e| OpError::from_glib(src, &e))?;
        self.done = base + tree_size(dst).max(copied);
        self.progress(src);
        Ok(())
    }

    // ── move ────────────────────────────────────────────────────────────────

    fn move_all(&mut self, sources: &[PathBuf], dest: &Path, conflict: Conflict) {
        self.total = sources.len() as u64;
        for src in sources {
            if self.cancelled() {
                break;
            }
            let Some(name) = src.file_name() else {
                self.report
                    .errors
                    .push(OpError::new(src, ErrorKind::Other, "no file name"));
                continue;
            };
            let mut target = dest.join(name);
            if target == *src {
                continue;
            }
            if inside(dest, src) {
                self.report.errors.push(OpError::new(
                    src,
                    ErrorKind::Other,
                    "can't move a folder into itself",
                ));
                continue;
            }
            let mut overwrite = false;
            if exists(&target) {
                match conflict {
                    Conflict::Skip => continue,
                    Conflict::KeepBoth => target = dest.join(free_name(dest, &name_of(src))),
                    Conflict::Overwrite => overwrite = true,
                }
            }
            let merge = overwrite && is_real_dir(&target) && is_real_dir(src);
            let res = if merge {
                // the merged items are mixed with what was there; no undo for them
                self.merge_move(src, &target)
            } else {
                self.move_one(src, &target, overwrite).map(|()| {
                    self.undo.steps.push(UndoStep::MoveBack {
                        now: target.clone(),
                        back: src.clone(),
                    });
                })
            };
            match res {
                Ok(()) => self.report.created.push(target),
                Err(e) => self.report.errors.push(e),
            }
            self.step(1, src);
        }
    }

    fn move_one(&mut self, src: &Path, dst: &Path, overwrite: bool) -> Result<(), OpError> {
        let sf = gio::File::for_path(src);
        let df = gio::File::for_path(dst);
        let mut flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        if overwrite {
            flags |= gio::FileCopyFlags::OVERWRITE;
        }
        match sf.move_(&df, flags, Some(self.cancel), None) {
            Ok(()) => Ok(()),
            // a folder to another file system: copy everything, then remove the original only
            // if every byte arrived
            Err(e) if e.matches(gio::IOErrorEnum::WouldRecurse) => {
                // progress counts bytes while this item copies, then items again
                let (done, total) = (self.done, self.total);
                self.done = 0;
                self.total = tree_size(src);
                let copied = self.copy_tree(src, dst, overwrite);
                self.done = done;
                self.total = total;
                copied?;
                self.delete_tree(&sf, src)
            }
            Err(e) => Err(OpError::from_glib(src, &e)),
        }
    }

    fn merge_move(&mut self, src: &Path, dst: &Path) -> Result<(), OpError> {
        let sf = gio::File::for_path(src);
        let children = list_names(&sf, self.cancel).map_err(|e| OpError::from_glib(src, &e))?;
        let mut first_err = None;
        for child in children {
            let (s, d) = (src.join(&child), dst.join(&child));
            let r = if is_real_dir(&s) && is_real_dir(&d) {
                self.merge_move(&s, &d)
            } else {
                self.move_one(&s, &d, true)
            };
            if let Err(e) = r {
                first_err.get_or_insert(e);
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        sf.delete(Some(self.cancel))
            .map_err(|e| OpError::from_glib(src, &e))
    }

    // ── trash, delete, rename, create ───────────────────────────────────────

    fn trash(&mut self, p: &Path) -> Result<(), OpError> {
        let started = std::time::SystemTime::now();
        gio::File::for_path(p)
            .trash(Some(self.cancel))
            .map_err(|e| {
                let mut err = OpError::from_glib(p, &e);
                if e.matches(gio::IOErrorEnum::NotSupported) {
                    err.kind = ErrorKind::TrashUnsupported;
                }
                err
            })?;
        if let Some(t) = find_trashed(p, started) {
            self.undo.steps.push(UndoStep::Restore(t));
        }
        Ok(())
    }

    fn delete_tree(&mut self, f: &gio::File, p: &Path) -> Result<(), OpError> {
        let info = f
            .query_info("standard::type", NOFOLLOW, Some(self.cancel))
            .map_err(|e| OpError::from_glib(p, &e))?;
        if info.file_type() == gio::FileType::Directory {
            let children = list_names(f, self.cancel).map_err(|e| OpError::from_glib(p, &e))?;
            for child in children {
                if self.cancelled() {
                    return Err(OpError::new(p, ErrorKind::Cancelled, "cancelled"));
                }
                let cp = p.join(&child);
                self.delete_tree(&gio::File::for_path(&cp), &cp)?;
            }
        }
        f.delete(Some(self.cancel))
            .map_err(|e| OpError::from_glib(p, &e))
    }

    fn rename(&mut self, path: &Path, new_name: &str) -> Result<(), OpError> {
        if new_name.is_empty()
            || new_name == "."
            || new_name == ".."
            || new_name.contains(['/', '\0'])
        {
            return Err(OpError::new(
                path,
                ErrorKind::Other,
                format!("invalid name {new_name:?}"),
            ));
        }
        let Some(parent) = path.parent() else {
            return Err(OpError::new(path, ErrorKind::Other, "no parent folder"));
        };
        let target = parent.join(new_name);
        if target == path {
            return Ok(());
        }
        if exists(&target) {
            return Err(OpError::new(&target, ErrorKind::Exists, "name is taken"));
        }
        self.move_one(path, &target, false)?;
        self.undo.steps.push(UndoStep::MoveBack {
            now: target.clone(),
            back: path.to_path_buf(),
        });
        self.report.created.push(target);
        Ok(())
    }

    fn new_file(&mut self, path: &Path) -> Result<(), OpError> {
        let f = gio::File::for_path(path);
        if let Some(parent) = f.parent()
            && let Err(e) = parent.make_directory_with_parents(Some(self.cancel))
            && !e.matches(gio::IOErrorEnum::Exists)
        {
            return Err(OpError::from_glib(path, &e));
        }
        let stream = f
            .create(gio::FileCreateFlags::NONE, Some(self.cancel))
            .map_err(|e| OpError::from_glib(path, &e))?;
        stream
            .close(Some(self.cancel))
            .map_err(|e| OpError::from_glib(path, &e))
    }

    fn apply_edit(&mut self, dir: &Path, plan: &Plan) {
        self.total = plan.changes.len() as u64;
        for change in &plan.changes {
            if self.cancelled() {
                break;
            }
            let res = match change {
                Change::Delete { name } => self.trash(&dir.join(name)),
                Change::Create { path, dir: true } => {
                    let p = dir.join(path);
                    gio::File::for_path(&p)
                        .make_directory(Some(self.cancel))
                        .map_err(|e| OpError::from_glib(&p, &e))
                        .map(|()| {
                            self.undo.steps.push(UndoStep::RemoveCreated(p.clone()));
                            self.report.created.push(p)
                        })
                }
                Change::Create { path, dir: false } => {
                    let p = dir.join(path);
                    self.new_file(&p).map(|()| {
                        self.undo.steps.push(UndoStep::RemoveCreated(p.clone()));
                        self.report.created.push(p)
                    })
                }
                Change::Rename { from, to } => {
                    let (s, d) = (dir.join(from), dir.join(to));
                    if exists(&d) {
                        Err(OpError::new(&d, ErrorKind::Exists, "name is taken"))
                    } else {
                        self.move_one(&s, &d, false).map(|()| {
                            self.undo.steps.push(UndoStep::MoveBack { now: d, back: s });
                        })
                    }
                }
            };
            self.step(1, dir);
            if let Err(e) = res {
                // the plan's later steps assume this one happened
                self.report.errors.push(e);
                break;
            }
        }
    }

    /// Runs the steps last to first. Steps that fail or are not reached (cancel) stay in
    /// `self.undo`, so the next undo can try them again.
    fn undo_steps(&mut self, record: Undo) {
        self.total = record.steps.len() as u64;
        self.undo.label = record.label.clone();
        let mut left = Vec::new();
        for step in record.steps.into_iter().rev() {
            if self.cancelled() {
                left.push(step);
                continue;
            }
            let res = match &step {
                UndoStep::MoveBack { now, back } => {
                    if exists(back) {
                        Err(OpError::new(back, ErrorKind::Exists, "name is taken again"))
                    } else {
                        self.move_one(now, back, false)
                            .map(|()| self.report.created.push(back.clone()))
                    }
                }
                UndoStep::Restore(t) => self.restore(t),
                UndoStep::RemoveCreated(p) => remove_if_untouched(p),
            };
            if let Err(e) = res {
                self.report.errors.push(e);
                left.push(step);
            }
            self.step(1, Path::new(""));
        }
        left.reverse();
        self.undo.steps = left;
    }

    fn restore(&mut self, t: &Trashed) -> Result<(), OpError> {
        if exists(&t.orig) {
            return Err(OpError::new(
                &t.orig,
                ErrorKind::Exists,
                "name is taken again",
            ));
        }
        if let Some(parent) = t.orig.parent()
            && let Err(e) =
                gio::File::for_path(parent).make_directory_with_parents(Some(self.cancel))
            && !e.matches(gio::IOErrorEnum::Exists)
        {
            return Err(OpError::from_glib(&t.orig, &e));
        }
        self.move_one(&t.file, &t.orig, false)?;
        let _ = std::fs::remove_file(&t.info);
        self.report.created.push(t.orig.clone());
        Ok(())
    }
}

/// Whether `dest` is `src` or below it, also through symlinks.
fn inside(dest: &Path, src: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    dest.starts_with(src) || real(dest).starts_with(real(src))
}

/// A real folder, not a symlink to one.
fn is_real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// Removes `p` if it is an empty file or an empty folder; otherwise leaves it and says why.
fn remove_if_untouched(p: &Path) -> Result<(), OpError> {
    let m = std::fs::symlink_metadata(p)
        .map_err(|e| OpError::new(p, ErrorKind::NotFound, e.to_string()))?;
    let empty = if m.is_dir() {
        std::fs::read_dir(p)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    } else {
        m.is_file() && m.len() == 0
    };
    if !empty {
        return Err(OpError::new(
            p,
            ErrorKind::Other,
            "changed since it was created; left in place",
        ));
    }
    let r = if m.is_dir() {
        std::fs::remove_dir(p)
    } else {
        std::fs::remove_file(p)
    };
    r.map_err(|e| OpError::new(p, ErrorKind::Other, e.to_string()))
}

fn list_names(f: &gio::File, cancel: &gio::Cancellable) -> Result<Vec<PathBuf>, glib::Error> {
    let en = f.enumerate_children("standard::name", NOFOLLOW, Some(cancel))?;
    let mut out = Vec::new();
    while let Some(info) = en.next_file(Some(cancel))? {
        out.push(info.name());
    }
    Ok(out)
}

/// Bytes under `p`, not following symlinks. Unreadable parts count as zero. Blocking.
pub fn tree_size(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else {
        return 0;
    };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p)
        .map(|rd| rd.flatten().map(|e| tree_size(&e.path())).sum())
        .unwrap_or(0)
}

// ── finding a trashed item (freedesktop trash spec, which GIO implements) ──

/// Locates the trash entry GIO just made for `orig`: the newest `.trashinfo` whose `Path=` is
/// `orig`, in the home trash or the trash at the top of `orig`'s file system.
fn find_trashed(orig: &Path, since: std::time::SystemTime) -> Option<Trashed> {
    // GIO names a second `a.txt` in the trash `a.2.txt`, so match on the part before the
    // extension; Path= decides
    let name = orig.file_name()?.to_string_lossy().into_owned();
    let base = match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name,
    };
    let real_orig = orig
        .parent()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .map(|p| p.join(orig.file_name().unwrap_or_default()));
    let mut best: Option<(std::time::SystemTime, Trashed)> = None;
    for (trash, top) in trash_dirs(orig) {
        let Ok(rd) = std::fs::read_dir(trash.join("info")) else {
            continue;
        };
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = fname.strip_suffix(".trashinfo") else {
                continue;
            };
            if !stem.starts_with(&base) {
                continue;
            }
            let Ok(mtime) = e.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            // a little slack for coarse file system timestamps
            if mtime + Duration::from_secs(2) < since {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else {
                continue;
            };
            let Some(path) = trashinfo_path(&text) else {
                continue;
            };
            let full = if path.is_absolute() {
                path
            } else {
                top.join(path)
            };
            if full != orig && Some(&full) != real_orig.as_ref() {
                continue;
            }
            let t = Trashed {
                orig: orig.to_path_buf(),
                file: trash.join("files").join(stem),
                info: e.path(),
            };
            if best.as_ref().is_none_or(|(m, _)| mtime >= *m) {
                best = Some((mtime, t));
            }
        }
    }
    best.map(|(_, t)| t)
}

fn trashinfo_path(text: &str) -> Option<PathBuf> {
    let raw = text.lines().find_map(|l| l.strip_prefix("Path="))?;
    let bytes = percent_decode(raw.trim())?;
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// (trash dir, top dir its relative paths are based on) candidates for `orig`.
fn trash_dirs(orig: &Path) -> Vec<(PathBuf, PathBuf)> {
    use std::os::unix::fs::MetadataExt;
    let mut out = vec![(glib::user_data_dir().join("Trash"), PathBuf::from("/"))];
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let parent = orig.parent().unwrap_or(Path::new("/"));
    if let (Some(uid), Ok(dev)) = (uid, std::fs::metadata(parent).map(|m| m.dev())) {
        let mut top = parent.to_path_buf();
        while let Some(up) = top.parent() {
            if std::fs::metadata(up).map(|m| m.dev()).ok() != Some(dev) {
                break;
            }
            top = up.to_path_buf();
        }
        out.push((top.join(".Trash").join(uid.to_string()), top.clone()));
        out.push((top.join(format!(".Trash-{uid}")), top));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::scratch;
    use std::fs;

    /// Submits `op` and waits for its report, collecting progress events.
    fn run(q: &OpQueue, op: Op) -> (Report, Vec<OpEvent>) {
        let id = q.submit(op).unwrap();
        let mut events = Vec::new();
        loop {
            let ev = q.events.recv_blocking().unwrap();
            if let OpEvent::Finished { id: done, report } = &ev
                && *done == id
            {
                return (report.clone(), events);
            }
            events.push(ev);
        }
    }

    fn ok(q: &OpQueue, op: Op) -> Report {
        let (r, _) = run(q, op);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        r
    }

    fn tree(d: &Path) {
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::write(d.join("src/a.txt"), "aaaa").unwrap();
        fs::write(d.join("src/sub/b.txt"), "bb").unwrap();
        std::os::unix::fs::symlink("a.txt", d.join("src/link")).unwrap();
        fs::create_dir(d.join("dst")).unwrap();
    }

    #[test]
    fn copy_file_and_folder_with_progress() {
        let t = scratch();
        let d = t.path();
        tree(d);
        let q = OpQueue::new();
        let (r, events) = run(
            &q,
            Op::Copy {
                sources: vec![d.join("src")],
                dest: d.join("dst"),
                conflict: Conflict::Skip,
            },
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.created, [d.join("dst/src")]);
        assert_eq!(
            fs::read_to_string(d.join("dst/src/sub/b.txt")).unwrap(),
            "bb"
        );
        assert!(
            fs::symlink_metadata(d.join("dst/src/link"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(d.join("src/a.txt").exists());
        let last = events.iter().rev().find_map(|e| match e {
            OpEvent::Progress { done, total, .. } => Some((*done, *total)),
            _ => None,
        });
        // 4 + 2 bytes of files, plus the symlink's own 5
        assert_eq!(last, Some((11, 11)));
        assert!(!r.undoable);
    }

    #[test]
    fn copy_into_same_folder_keeps_both_and_into_itself_fails() {
        let t = scratch();
        let d = t.path();
        tree(d);
        let q = OpQueue::new();
        let r = ok(
            &q,
            Op::Copy {
                sources: vec![d.join("src/a.txt")],
                dest: d.join("src"),
                conflict: Conflict::Skip,
            },
        );
        assert_eq!(r.created, [d.join("src/a (2).txt")]);
        let r = ok(
            &q,
            Op::Copy {
                sources: vec![d.join("src/a.txt")],
                dest: d.join("src"),
                conflict: Conflict::Skip,
            },
        );
        assert_eq!(r.created, [d.join("src/a (3).txt")]);
        let (r, _) = run(
            &q,
            Op::Copy {
                sources: vec![d.join("src")],
                dest: d.join("src/sub"),
                conflict: Conflict::Skip,
            },
        );
        assert_eq!(r.errors.len(), 1);
        assert!(!d.join("src/sub/src").exists());
    }

    #[test]
    fn copy_conflicts_skip_overwrite_keep_both() {
        let t = scratch();
        let d = t.path();
        tree(d);
        fs::write(d.join("dst/a.txt"), "old").unwrap();
        assert_eq!(
            conflicts(&[d.join("src/a.txt"), d.join("src/sub")], &d.join("dst")),
            [d.join("dst/a.txt")]
        );
        let q = OpQueue::new();
        let src = vec![d.join("src/a.txt")];
        ok(
            &q,
            Op::Copy {
                sources: src.clone(),
                dest: d.join("dst"),
                conflict: Conflict::Skip,
            },
        );
        assert_eq!(fs::read_to_string(d.join("dst/a.txt")).unwrap(), "old");
        ok(
            &q,
            Op::Copy {
                sources: src.clone(),
                dest: d.join("dst"),
                conflict: Conflict::KeepBoth,
            },
        );
        assert_eq!(fs::read_to_string(d.join("dst/a (2).txt")).unwrap(), "aaaa");
        ok(
            &q,
            Op::Copy {
                sources: src,
                dest: d.join("dst"),
                conflict: Conflict::Overwrite,
            },
        );
        assert_eq!(fs::read_to_string(d.join("dst/a.txt")).unwrap(), "aaaa");
    }

    #[test]
    fn copy_overwrite_merges_folders() {
        let t = scratch();
        let d = t.path();
        tree(d);
        fs::create_dir_all(d.join("dst/src/sub")).unwrap();
        fs::write(d.join("dst/src/keep.txt"), "keep").unwrap();
        fs::write(d.join("dst/src/sub/b.txt"), "old").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::Copy {
                sources: vec![d.join("src")],
                dest: d.join("dst"),
                conflict: Conflict::Overwrite,
            },
        );
        assert_eq!(
            fs::read_to_string(d.join("dst/src/keep.txt")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(d.join("dst/src/sub/b.txt")).unwrap(),
            "bb"
        );
        assert!(d.join("dst/src/a.txt").exists());
    }

    #[test]
    fn move_and_undo() {
        let t = scratch();
        let d = t.path();
        tree(d);
        let q = OpQueue::new();
        let r = ok(
            &q,
            Op::Move {
                sources: vec![d.join("src/a.txt"), d.join("src/sub")],
                dest: d.join("dst"),
                conflict: Conflict::Skip,
            },
        );
        assert!(r.undoable);
        assert!(d.join("dst/a.txt").exists() && d.join("dst/sub/b.txt").exists());
        assert!(!d.join("src/a.txt").exists());
        assert_eq!(q.undo_label().as_deref(), Some("move 2 items"));
        let id = q.undo().unwrap();
        let r = loop {
            if let OpEvent::Finished { id: i, report } = q.events.recv_blocking().unwrap()
                && i == id
            {
                break report;
            }
        };
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(d.join("src/a.txt").exists() && d.join("src/sub/b.txt").exists());
        assert!(!d.join("dst/a.txt").exists());
        assert!(!q.can_undo());
    }

    #[test]
    fn move_conflicts_and_merge() {
        let t = scratch();
        let d = t.path();
        tree(d);
        fs::write(d.join("dst/a.txt"), "old").unwrap();
        fs::create_dir_all(d.join("dst/sub")).unwrap();
        fs::write(d.join("dst/sub/other.txt"), "o").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::Move {
                sources: vec![d.join("src/a.txt")],
                dest: d.join("dst"),
                conflict: Conflict::Skip,
            },
        );
        assert!(d.join("src/a.txt").exists());
        ok(
            &q,
            Op::Move {
                sources: vec![d.join("src/a.txt")],
                dest: d.join("dst"),
                conflict: Conflict::KeepBoth,
            },
        );
        assert!(d.join("dst/a (2).txt").exists() && !d.join("src/a.txt").exists());
        ok(
            &q,
            Op::Move {
                sources: vec![d.join("src/sub")],
                dest: d.join("dst"),
                conflict: Conflict::Overwrite,
            },
        );
        assert!(d.join("dst/sub/other.txt").exists() && d.join("dst/sub/b.txt").exists());
        assert!(!d.join("src/sub").exists());
        // moving onto itself is a no-op, not an error
        ok(
            &q,
            Op::Move {
                sources: vec![d.join("dst/a.txt")],
                dest: d.join("dst"),
                conflict: Conflict::Skip,
            },
        );
        let (r, _) = run(
            &q,
            Op::Move {
                sources: vec![d.join("dst")],
                dest: d.join("dst/sub"),
                conflict: Conflict::Skip,
            },
        );
        assert_eq!(r.errors.len(), 1);
    }

    #[test]
    fn trash_and_undo_restore() {
        let t = scratch();
        let d = t.path();
        tree(d);
        let q = OpQueue::new();
        let r = ok(
            &q,
            Op::Trash {
                paths: vec![d.join("src/a.txt"), d.join("src/sub")],
            },
        );
        assert!(r.undoable);
        assert!(!d.join("src/a.txt").exists() && !d.join("src/sub").exists());
        // the scratch dir lives in the test home, so this is the test's own trash
        let trash = crate::test_support::gio_env().join(".local/share/Trash");
        assert!(trash.join("files").exists());
        q.undo().unwrap();
        loop {
            if let OpEvent::Finished { report, .. } = q.events.recv_blocking().unwrap() {
                assert!(report.errors.is_empty(), "{:?}", report.errors);
                break;
            }
        }
        assert_eq!(fs::read_to_string(d.join("src/a.txt")).unwrap(), "aaaa");
        assert_eq!(fs::read_to_string(d.join("src/sub/b.txt")).unwrap(), "bb");
    }

    #[test]
    fn trash_same_name_twice_restores_the_right_one() {
        let t = scratch();
        let d = t.path();
        fs::write(d.join("dup.txt"), "first").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::Trash {
                paths: vec![d.join("dup.txt")],
            },
        );
        fs::write(d.join("dup.txt"), "second").unwrap();
        ok(
            &q,
            Op::Trash {
                paths: vec![d.join("dup.txt")],
            },
        );
        q.undo().unwrap();
        while !matches!(q.events.recv_blocking().unwrap(), OpEvent::Finished { .. }) {}
        assert_eq!(fs::read_to_string(d.join("dup.txt")).unwrap(), "second");
        // the first is still trashed and its undo now fails cleanly: the name is taken
        q.undo().unwrap();
        let r = loop {
            if let OpEvent::Finished { report, .. } = q.events.recv_blocking().unwrap() {
                break report;
            }
        };
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].kind, ErrorKind::Exists);
        assert_eq!(fs::read_to_string(d.join("dup.txt")).unwrap(), "second");
    }

    #[test]
    fn trash_unsupported_is_reported() {
        // /tmp is a system-internal mount without trash, so GIO refuses
        let t = tempfile::tempdir_in("/tmp").unwrap();
        crate::test_support::gio_env();
        let p = t.path().join("x");
        fs::write(&p, "").unwrap();
        let q = OpQueue::new();
        let (r, _) = run(
            &q,
            Op::Trash {
                paths: vec![p.clone()],
            },
        );
        if r.errors.is_empty() {
            // a system where /tmp has a trash: nothing to check
            return;
        }
        assert_eq!(r.errors[0].kind, ErrorKind::TrashUnsupported);
        assert!(p.exists());
    }

    #[test]
    fn delete_permanently_recursive_without_following_links() {
        let t = scratch();
        let d = t.path();
        tree(d);
        fs::create_dir(d.join("outside")).unwrap();
        fs::write(d.join("outside/keep"), "k").unwrap();
        std::os::unix::fs::symlink(d.join("outside"), d.join("src/dirlink")).unwrap();
        let q = OpQueue::new();
        let r = ok(
            &q,
            Op::DeletePermanently {
                paths: vec![d.join("src")],
            },
        );
        assert!(!r.undoable);
        assert!(!d.join("src").exists());
        assert!(d.join("outside/keep").exists());
    }

    #[test]
    fn rename_mkdir_new_file_and_undo_rename() {
        let t = scratch();
        let d = t.path();
        tree(d);
        let q = OpQueue::new();
        ok(
            &q,
            Op::Rename {
                path: d.join("src/a.txt"),
                new_name: "ä b.txt".into(),
            },
        );
        assert!(d.join("src/ä b.txt").exists());
        let (r, _) = run(
            &q,
            Op::Rename {
                path: d.join("src/ä b.txt"),
                new_name: "sub".into(),
            },
        );
        assert_eq!(r.errors[0].kind, ErrorKind::Exists);
        let (r, _) = run(
            &q,
            Op::Rename {
                path: d.join("src/ä b.txt"),
                new_name: "x/y".into(),
            },
        );
        assert_eq!(r.errors.len(), 1);
        q.undo().unwrap();
        while !matches!(q.events.recv_blocking().unwrap(), OpEvent::Finished { .. }) {}
        assert!(d.join("src/a.txt").exists());

        let r = ok(
            &q,
            Op::Mkdir {
                path: d.join("new/nested"),
            },
        );
        assert_eq!(r.created, [d.join("new/nested")]);
        let (r, _) = run(
            &q,
            Op::Mkdir {
                path: d.join("new/nested"),
            },
        );
        assert_eq!(r.errors[0].kind, ErrorKind::Exists);
        ok(
            &q,
            Op::NewFile {
                path: d.join("deep/er/file.txt"),
            },
        );
        assert_eq!(fs::read(d.join("deep/er/file.txt")).unwrap(), b"");
        let (r, _) = run(
            &q,
            Op::NewFile {
                path: d.join("deep/er/file.txt"),
            },
        );
        assert_eq!(r.errors[0].kind, ErrorKind::Exists);
    }

    #[test]
    fn apply_edit_plan_and_undo() {
        let t = scratch();
        let d = t.path();
        fs::write(d.join("a"), "A").unwrap();
        fs::write(d.join("b"), "B").unwrap();
        fs::write(d.join("gone"), "").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        let listed: Vec<_> = ["a", "b", "gone", "sub"]
            .iter()
            .enumerate()
            .map(|(i, n)| crate::edit::Listed {
                id: i + 1,
                name: n.to_string(),
                is_dir: *n == "sub",
            })
            .collect();
        let plan =
            crate::edit::plan(&listed, "/001 b\n/002 a\n/004 sub/\nsub/new.txt\nfresh/\n").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::ApplyEdit {
                dir: d.to_path_buf(),
                plan,
            },
        );
        assert_eq!(fs::read_to_string(d.join("a")).unwrap(), "B");
        assert_eq!(fs::read_to_string(d.join("b")).unwrap(), "A");
        assert!(!d.join("gone").exists());
        assert!(d.join("sub/new.txt").exists() && d.join("fresh").is_dir());
        q.undo().unwrap();
        while !matches!(q.events.recv_blocking().unwrap(), OpEvent::Finished { .. }) {}
        assert_eq!(fs::read_to_string(d.join("a")).unwrap(), "A");
        assert!(d.join("gone").exists());
    }

    #[test]
    fn cancel_stops_a_big_copy() {
        let t = scratch();
        let d = t.path();
        fs::create_dir(d.join("many")).unwrap();
        for i in 0..2000 {
            fs::write(d.join(format!("many/{i}")), "x").unwrap();
        }
        fs::create_dir(d.join("out")).unwrap();
        let q = OpQueue::new();
        let id = q
            .submit(Op::Copy {
                sources: vec![d.join("many")],
                dest: d.join("out"),
                conflict: Conflict::Skip,
            })
            .unwrap();
        q.cancel(id);
        let r = loop {
            if let OpEvent::Finished { report, .. } = q.events.recv_blocking().unwrap() {
                break report;
            }
        };
        assert!(r.cancelled);
        let copied = fs::read_dir(d.join("out/many"))
            .map(|r| r.count())
            .unwrap_or(0);
        assert!(copied < 2000, "{copied}");
    }

    #[test]
    fn free_names() {
        let t = scratch();
        let d = t.path();
        fs::write(d.join("r.tar.gz"), "").unwrap();
        fs::write(d.join("r (2).tar.gz"), "").unwrap();
        assert_eq!(free_name(d, "r.tar.gz"), "r (3).tar.gz");
        assert_eq!(free_name(d, ".bashrc"), ".bashrc (2)");
        assert_eq!(free_name(d, "folder"), "folder (2)");
        assert_eq!(split_ext(".tar"), (".tar", ""));
    }

    #[test]
    fn copy_through_symlinked_destination_into_itself_is_refused() {
        let t = scratch();
        let d = t.path();
        tree(d);
        std::os::unix::fs::symlink(d.join("src/sub"), d.join("lnk")).unwrap();
        let q = OpQueue::new();
        let (r, _) = run(
            &q,
            Op::Copy {
                sources: vec![d.join("src")],
                dest: d.join("lnk"),
                conflict: Conflict::KeepBoth,
            },
        );
        assert_eq!(r.errors.len(), 1);
        let (r, _) = run(
            &q,
            Op::Move {
                sources: vec![d.join("src")],
                dest: d.join("lnk"),
                conflict: Conflict::KeepBoth,
            },
        );
        assert_eq!(r.errors.len(), 1);
        assert!(d.join("src/a.txt").exists());
    }

    #[test]
    fn overwrite_never_merges_through_a_symlink() {
        let t = scratch();
        let d = t.path();
        fs::create_dir_all(d.join("from/proj")).unwrap();
        fs::write(d.join("from/proj/new"), "n").unwrap();
        fs::create_dir_all(d.join("elsewhere")).unwrap();
        fs::create_dir_all(d.join("dst")).unwrap();
        std::os::unix::fs::symlink(d.join("elsewhere"), d.join("dst/proj")).unwrap();
        let q = OpQueue::new();
        let _ = run(
            &q,
            Op::Copy {
                sources: vec![d.join("from/proj")],
                dest: d.join("dst"),
                conflict: Conflict::Overwrite,
            },
        );
        let _ = run(
            &q,
            Op::Move {
                sources: vec![d.join("from/proj")],
                dest: d.join("dst"),
                conflict: Conflict::Overwrite,
            },
        );
        assert!(!d.join("elsewhere/new").exists());
    }

    #[test]
    fn failed_undo_steps_stay_undoable() {
        let t = scratch();
        let d = t.path();
        fs::write(d.join("a"), "").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::Rename {
                path: d.join("a"),
                new_name: "b".into(),
            },
        );
        fs::write(d.join("a"), "blocker").unwrap();
        q.undo().unwrap();
        while !matches!(q.events.recv_blocking().unwrap(), OpEvent::Finished { .. }) {}
        assert!(q.can_undo());
        fs::remove_file(d.join("a")).unwrap();
        q.undo().unwrap();
        while !matches!(q.events.recv_blocking().unwrap(), OpEvent::Finished { .. }) {}
        assert!(d.join("a").exists() && !d.join("b").exists());
        assert!(!q.can_undo());
    }

    #[test]
    fn undo_of_edit_removes_untouched_creations_only() {
        let t = scratch();
        let d = t.path();
        let plan = crate::edit::plan(&[], "empty.txt\nkept.txt\nnewdir/\n").unwrap();
        let q = OpQueue::new();
        ok(
            &q,
            Op::ApplyEdit {
                dir: d.to_path_buf(),
                plan,
            },
        );
        fs::write(d.join("kept.txt"), "typed into it").unwrap();
        q.undo().unwrap();
        let r = loop {
            if let OpEvent::Finished { report, .. } = q.events.recv_blocking().unwrap() {
                break report;
            }
        };
        assert_eq!(r.errors.len(), 1);
        assert!(!d.join("empty.txt").exists() && !d.join("newdir").exists());
        assert_eq!(
            fs::read_to_string(d.join("kept.txt")).unwrap(),
            "typed into it"
        );
    }

    #[test]
    fn not_supported_outside_trash_is_not_trash_unsupported() {
        let e = glib::Error::new(gio::IOErrorEnum::NotSupported, "nope");
        assert_eq!(
            OpError::from_glib(Path::new("/x"), &e).kind,
            ErrorKind::Other
        );
    }
}
