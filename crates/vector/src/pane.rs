//! One folder view: the core [`DirModel`] and [`Selection`] mirrored into a GTK list or grid.
//! Listing runs on a worker thread and arrives in batches; the monitor keeps it current.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use vector_core::entry::Entry;
use vector_core::git::Badge;
use vector_core::listing::{self, DirChange, ListEvent, Listing, Watcher};
use vector_core::model::{DirModel, ViewOptions};
use vector_core::selection::Selection;

use crate::fmt;

/// What a pane tells the window.
pub enum PaneEvent {
    /// Cursor or selection moved.
    Cursor,
    /// The folder changed (navigation) or finished loading.
    Dir,
    /// Double click / Enter on a file.
    Activate(Entry),
    /// Right click at (x, y) in `widget` coordinates.
    Menu(gtk::Widget, f64, f64),
    /// Files dropped onto `dest`.
    Drop {
        paths: Vec<PathBuf>,
        dest: PathBuf,
        copy: bool,
    },
    /// The pane got focus by mouse.
    Focus,
    /// Listing error.
    Error(String),
    /// A column header was clicked: sort by it (again: reverse).
    SortBy(vector_core::sort::SortKey),
}

pub struct Pane {
    pub root: gtk::Box,
    header: gtk::Box,
    /// Why the folder couldn't be listed.
    error: RefCell<Option<String>>,
    stack: gtk::Stack,
    list: gtk::ListView,
    grid: gtk::GridView,
    empty: gtk::Label,
    store: gio::ListStore,
    sel: gtk::MultiSelection,
    pub model: RefCell<DirModel>,
    pub selection: RefCell<Selection>,
    objects: RefCell<HashMap<String, glib::BoxedAnyObject>>,
    listing: RefCell<Option<Listing>>,
    watcher: RefCell<Option<Watcher>>,
    back: RefCell<Vec<PathBuf>>,
    forward: RefCell<Vec<PathBuf>>,
    git: RefCell<HashMap<String, Badge>>,
    /// Folders whose size is known (their entry's `size` holds it).
    measured: RefCell<std::collections::HashSet<String>>,
    /// The folder shown is inside a zip or tar archive (read-only, no sizes, git or watching).
    pub in_archive: Cell<bool>,
    /// The sort from before the Trash was opened (the Trash shows newest-trashed first).
    sort_before_trash: Cell<Option<vector_core::sort::SortSpec>>,
    /// A compact-layout check is waiting for layout to settle.
    compact_queued: Cell<bool>,
    measure_cancel: RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    /// Free bytes on this folder's file system, and whether it is a network mount.
    pub fs_info: Cell<(Option<u64>, bool)>,
    /// Files looked at by a running `:biggest` scan.
    pub progress: Cell<Option<u64>>,
    /// `:biggest` view: (roots, how many, stay on one file system), and the sort to go back to.
    biggest: RefCell<Option<Biggest>>,
    grid_mode: Cell<bool>,
    /// Narrow (split) pane: no date column.
    compact: Cell<bool>,
    syncing: Cell<bool>,
    refresh_queued: Cell<bool>,
    flat: Cell<bool>,
    /// Yanked or cut paths (shown marked), and whether it is a cut.
    marks: RefCell<(std::collections::HashSet<PathBuf>, bool)>,
    pub loading: Cell<bool>,
    /// Name to put the cursor on once it shows up (after going to the parent folder).
    want_cursor: RefCell<Option<String>>,
    on_event: Box<dyn Fn(PaneEvent)>,
    /// Timings of the last load: (first rows shown, all rows listed), for `VECTOR_TIMING`.
    pub timing: Cell<(Option<f64>, Option<f64>)>,
}

impl Pane {
    pub fn new(opts: ViewOptions, on_event: impl Fn(PaneEvent) + 'static) -> Rc<Pane> {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let sel = gtk::MultiSelection::new(Some(store.clone()));
        let list = gtk::ListView::new(Some(sel.clone()), None::<gtk::ListItemFactory>);
        let grid = gtk::GridView::new(Some(sel.clone()), None::<gtk::ListItemFactory>);
        grid.set_max_columns(24);
        grid.set_min_columns(1);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        let scroll_list = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let scroll_grid = gtk::ScrolledWindow::builder()
            .child(&grid)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let empty = gtk::Label::new(Some("empty folder"));
        empty.add_css_class("empty");
        empty.set_valign(gtk::Align::Start);
        empty.set_xalign(0.0);
        stack.add_named(&scroll_list, Some("list"));
        stack.add_named(&scroll_grid, Some("grid"));
        stack.add_named(&empty, Some("empty"));
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("pane");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.add_css_class("colheader");
        root.append(&header);
        root.append(&stack);

        let pane = Rc::new(Pane {
            root,
            header,
            error: RefCell::new(None),
            stack,
            list,
            grid,
            empty,
            store,
            sel,
            model: RefCell::new(DirModel::new("/", opts)),
            selection: RefCell::new(Selection::default()),
            objects: RefCell::default(),
            listing: RefCell::new(None),
            watcher: RefCell::new(None),
            back: RefCell::default(),
            forward: RefCell::default(),
            git: RefCell::default(),
            measured: RefCell::default(),
            in_archive: Cell::new(false),
            sort_before_trash: Cell::new(None),
            compact_queued: Cell::new(false),
            measure_cancel: RefCell::new(None),
            fs_info: Cell::new((None, false)),
            progress: Cell::new(None),
            biggest: RefCell::new(None),
            grid_mode: Cell::new(false),
            compact: Cell::new(false),
            syncing: Cell::new(false),
            refresh_queued: Cell::new(false),
            flat: Cell::new(false),
            marks: RefCell::default(),
            loading: Cell::new(false),
            want_cursor: RefCell::new(None),
            on_event: Box::new(on_event),
            timing: Cell::new((None, None)),
        });
        pane.list
            .set_factory(Some(&row_factory(Rc::downgrade(&pane))));
        pane.grid
            .set_factory(Some(&tile_factory(Rc::downgrade(&pane))));
        // the list's horizontal adjustment follows the pane's real width: narrow panes drop
        // the date column so names keep their room
        if let Some(sw) = pane.list.parent().and_downcast::<gtk::ScrolledWindow>() {
            let weak = Rc::downgrade(&pane);
            sw.hadjustment().connect_changed(move |a| {
                let Some(p) = weak.upgrade().filter(|_| a.page_size() > 0.0) else {
                    return;
                };
                if p.compact_queued.replace(true) {
                    return;
                }
                // decided once layout has settled, from the width at that moment (not the one
                // that queued it: the first sizes of a new window are tiny); with a band around
                // the threshold so a resize hovering near it doesn't switch back and forth
                let (weak, a) = (Rc::downgrade(&p), a.clone());
                glib::idle_add_local_once(move || {
                    let Some(p) = weak.upgrade() else { return };
                    p.compact_queued.set(false);
                    let w = a.page_size();
                    if w <= 0.0 {
                        return;
                    }
                    let compact = if p.compact.get() {
                        w < 760.0
                    } else {
                        w < 700.0
                    };
                    p.set_compact(compact);
                });
            });
        }
        pane.connect_signals();
        pane.build_header();
        pane
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.sel.connect_selection_changed(move |_, _, _| {
            if let Some(p) = weak.upgrade() {
                p.pull_selection();
            }
        });
        for view in [
            self.list.upcast_ref::<gtk::Widget>(),
            self.grid.upcast_ref(),
        ] {
            let weak = Rc::downgrade(self);
            let activate = move |pos: u32| {
                if let Some(p) = weak.upgrade() {
                    p.selection
                        .borrow_mut()
                        .set_cursor(&p.model.borrow(), pos as usize);
                    let entry = p.model.borrow().get(pos as usize).cloned();
                    if let Some(e) = entry {
                        if e.is_dir() {
                            p.open_dir(&e.path);
                        } else {
                            (p.on_event)(PaneEvent::Activate(e));
                        }
                    }
                }
            };
            if let Some(l) = view.downcast_ref::<gtk::ListView>() {
                let a = activate.clone();
                l.connect_activate(move |_, pos| a(pos));
            } else if let Some(g) = view.downcast_ref::<gtk::GridView>() {
                g.connect_activate(move |_, pos| activate(pos));
            }
            // right click on empty space; rows handle their own (they also move the cursor)
            let menu = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let weak = Rc::downgrade(self);
            menu.connect_pressed(move |g, _, x, y| {
                if let Some(p) = weak.upgrade()
                    && let Some(w) = g.widget()
                {
                    (p.on_event)(PaneEvent::Menu(w, x, y));
                }
            });
            view.add_controller(menu);
            let focus = gtk::EventControllerFocus::new();
            let weak = Rc::downgrade(self);
            focus.connect_enter(move |_| {
                if let Some(p) = weak.upgrade() {
                    (p.on_event)(PaneEvent::Focus);
                }
            });
            view.add_controller(focus);
        }
        // drops on the background go into this folder
        let drop = gtk::DropTarget::new(
            gdk::FileList::static_type(),
            gdk::DragAction::COPY | gdk::DragAction::MOVE,
        );
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |t, value, _, _| {
            let Some(p) = weak.upgrade() else {
                return false;
            };
            let dest = p.dir();
            p.drop_files(t, value, dest)
        });
        self.root.add_controller(drop);
    }

    pub fn drop_files(&self, t: &gtk::DropTarget, value: &glib::Value, dest: PathBuf) -> bool {
        let Ok(list) = value.get::<gdk::FileList>() else {
            return false;
        };
        let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
        if paths.is_empty() {
            return false;
        }
        let copy = match t.current_drop().map(|d| d.actions()) {
            Some(a) if a == gdk::DragAction::COPY => true,
            Some(a) if a == gdk::DragAction::MOVE => false,
            // no modifier: move on the same file system, copy across (like other file managers)
            _ => !same_device(&paths[0], &dest),
        };
        (self.on_event)(PaneEvent::Drop { paths, dest, copy });
        true
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn dir(&self) -> PathBuf {
        self.model.borrow().dir().to_path_buf()
    }

    pub fn is_grid(&self) -> bool {
        self.grid_mode.get()
    }

    /// The sort to remember between runs: the one from before the Trash, while it is open.
    pub fn remembered_sort(&self) -> vector_core::sort::SortSpec {
        self.sort_before_trash
            .get()
            .unwrap_or(self.model.borrow().options().sort)
    }

    pub fn set_grid(self: &Rc<Self>, grid: bool) {
        self.grid_mode.set(grid);
        self.build_header();
        self.update_stack();
        self.push_selection(true);
    }

    pub fn focus_view(&self) {
        if self.grid_mode.get() {
            self.grid.grab_focus();
        } else {
            self.list.grab_focus();
        }
    }

    pub fn set_marks(&self, paths: std::collections::HashSet<PathBuf>, cut: bool) {
        *self.marks.borrow_mut() = (paths, cut);
        self.rebind();
    }

    pub fn set_compact(self: &Rc<Self>, compact: bool) {
        if self.compact.replace(compact) != compact {
            self.build_header();
            self.rebind();
        }
    }

    /// Re-reads the pins of this folder (after one changed).
    pub fn reload_pins(&self) {
        let dir = self.dir();
        self.model
            .borrow_mut()
            .set_pinned(crate::pinstore::in_folder(&dir));
        self.rebind();
    }

    /// Re-draws the column titles (after the sort changed).
    pub fn sort_changed(self: &Rc<Self>) {
        self.build_header();
    }

    pub fn set_focused(&self, focused: bool) {
        if focused {
            self.root.add_css_class("focused-pane");
        } else {
            self.root.remove_css_class("focused-pane");
        }
    }

    pub fn git_badge(&self, name: &str) -> Option<Badge> {
        self.git.borrow().get(name).copied()
    }

    // ── navigation ──────────────────────────────────────────────────────────

    /// Opens `dir`, remembering the current folder for `back`.
    pub fn open_dir(self: &Rc<Self>, dir: &Path) {
        self.flat.set(false);
        self.leave_biggest();
        let cur = self.dir();
        if cur != dir && self.store.n_items() > 0 || self.loading.get() {
            self.back.borrow_mut().push(cur);
            self.forward.borrow_mut().clear();
        }
        self.load(dir);
    }

    /// Toggles the flat view: every file below this folder in one list.
    pub fn toggle_flat(self: &Rc<Self>) {
        self.flat.set(!self.flat.get());
        let cur = self.dir();
        self.load(&cur);
    }

    /// A list that isn't one folder (flat or biggest): no watcher, no :edit.
    pub fn is_flat(&self) -> bool {
        self.flat.get() || self.biggest.borrow().is_some()
    }

    pub fn is_biggest(&self) -> bool {
        self.biggest.borrow().is_some()
    }

    /// Shows the `n` biggest files under `roots`, biggest first.
    pub fn show_biggest(self: &Rc<Self>, roots: Vec<PathBuf>, n: usize, one_fs: bool) {
        let Some(first) = roots.first().cloned() else {
            return;
        };
        let before = self
            .biggest
            .borrow()
            .as_ref()
            .map(|b| b.sort_before)
            .unwrap_or(self.model.borrow().options().sort);
        self.flat.set(false);
        *self.biggest.borrow_mut() = Some(Biggest {
            roots,
            n,
            one_fs,
            sort_before: before,
        });
        self.model
            .borrow_mut()
            .set_sort(vector_core::sort::SortSpec {
                key: vector_core::sort::SortKey::Size,
                reverse: false,
                dirs_first: true,
            });
        self.load(&first);
    }

    fn leave_biggest(&self) {
        if let Some(b) = self.biggest.borrow_mut().take() {
            self.model.borrow_mut().set_sort(b.sort_before);
        }
    }

    /// Measures folder sizes in the background: `only` these entries (gs), or every folder
    /// in view, skipping network mounts unless asked for explicitly.
    pub fn measure(self: &Rc<Self>, only: Option<Vec<(String, PathBuf)>>) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let explicit = only.is_some();
        let dirs: Vec<(String, PathBuf)> = only.unwrap_or_else(|| {
            self.model
                .borrow()
                .all()
                .iter()
                .filter(|e| e.is_dir())
                .map(|e| (e.name.clone(), e.path.clone()))
                .collect()
        });
        // the last known size shows at once; it is measured again when older than a few
        // minutes (or when asked for with gs)
        let mut todo = Vec::new();
        for (name, path) in dirs {
            // a symlinked folder shows its target's size
            let real = if self.model.borrow().find(&name).is_some_and(|e| e.symlink) {
                std::fs::canonicalize(&path).unwrap_or(path)
            } else {
                path
            };
            match cached_size(&real) {
                Some((n, age)) => {
                    self.model.borrow_mut().set_size(&name, n);
                    self.objects.borrow_mut().remove(&name);
                    self.measured.borrow_mut().insert(name.clone());
                    if explicit || age > SIZE_FRESH_FOR {
                        todo.push((name, real));
                    }
                }
                None => todo.push((name, real)),
            }
        }
        if let Some(c) = self.measure_cancel.borrow_mut().take() {
            c.store(true, Ordering::Relaxed);
        }
        if todo.is_empty() {
            self.refresh();
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *self.measure_cancel.borrow_mut() = Some(cancel.clone());
        let (tx, rx) = async_channel::unbounded::<(String, PathBuf, u64)>();
        let worker_cancel = cancel.clone();
        let _ = std::thread::Builder::new()
            .name("vector-du".into())
            .spawn(move || {
                for (name, path) in todo {
                    match vector_core::du::dir_size(&path, &worker_cancel) {
                        Some(n) => {
                            if tx.send_blocking((name, path, n)).is_err() {
                                return;
                            }
                        }
                        None => return,
                    }
                }
            });
        self.refresh();
        let weak = Rc::downgrade(self);
        let dir = self.dir();
        glib::spawn_future_local(async move {
            while let Ok((name, path, n)) = rx.recv().await {
                let Some(p) = weak.upgrade() else { return };
                if p.dir() != dir || cancel.load(Ordering::Relaxed) {
                    return;
                }
                remember_size(&path, n);
                p.model.borrow_mut().set_size(&name, n);
                p.objects.borrow_mut().remove(&name);
                p.measured.borrow_mut().insert(name);
                p.schedule_refresh();
            }
            if let Some(p) = weak.upgrade()
                && p.dir() == dir
            {
                p.measure_cancel.borrow_mut().take();
                p.schedule_refresh();
            }
        });
    }

    fn is_measuring(&self) -> bool {
        self.measure_cancel.borrow().is_some()
    }

    /// Reads free space and whether the folder is on a network mount.
    fn load_fs_info(self: &Rc<Self>) {
        let dir = self.dir();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let f = gio::File::for_path(&dir);
            let info = f
                .query_filesystem_info_future(
                    "filesystem::free,filesystem::remote",
                    glib::Priority::DEFAULT,
                )
                .await;
            let Some(p) = weak.upgrade() else { return };
            if p.dir() != dir {
                return;
            }
            let (free, remote) = match info {
                Ok(i) => (
                    i.has_attribute("filesystem::free")
                        .then(|| i.attribute_uint64("filesystem::free")),
                    i.boolean("filesystem::remote"),
                ),
                Err(_) => (None, false),
            };
            p.fs_info.set((free, remote));
            if !p.is_flat() && !p.loading.get() {
                p.measure(None);
            }
            (p.on_event)(PaneEvent::Cursor);
        });
    }

    pub fn go_parent(self: &Rc<Self>) {
        let cur = self.dir();
        if let Some(parent) = cur.parent() {
            let name = cur.file_name().map(|n| n.to_string_lossy().into_owned());
            self.open_dir(parent);
            *self.want_cursor.borrow_mut() = name;
        }
    }

    pub fn go_back(self: &Rc<Self>) {
        let prev = self.back.borrow_mut().pop();
        if let Some(prev) = prev {
            self.forward.borrow_mut().push(self.dir());
            self.load(&prev);
        }
    }

    pub fn go_forward(self: &Rc<Self>) {
        let next = self.forward.borrow_mut().pop();
        if let Some(next) = next {
            self.back.borrow_mut().push(self.dir());
            self.load(&next);
        }
    }

    pub fn reload(self: &Rc<Self>) {
        let cur = self.dir();
        let name = self
            .selection
            .borrow()
            .cursor_entry(&self.model.borrow())
            .map(|e| e.name.clone());
        self.load(&cur);
        *self.want_cursor.borrow_mut() = name;
    }

    /// Lists `dir` from scratch. Nothing here waits on I/O.
    fn load(self: &Rc<Self>, dir: &Path) {
        let opts = self.model.borrow().options().clone();
        let mut opts = ViewOptions {
            filter: String::new(),
            ..opts
        };
        // the Trash opens with what was trashed last on top; leaving it restores the sort
        match (
            vector_core::trash::is_trash(dir),
            self.sort_before_trash.get(),
        ) {
            (true, None) => {
                self.sort_before_trash.set(Some(opts.sort));
                opts.sort.key = vector_core::sort::SortKey::Added;
                opts.sort.reverse = false;
            }
            (false, Some(before)) => {
                self.sort_before_trash.set(None);
                opts.sort = before;
            }
            _ => {}
        }
        *self.model.borrow_mut() = DirModel::new(dir, opts);
        self.in_archive
            .set(vector_core::archive::split(dir).is_some());
        self.model
            .borrow_mut()
            .set_pinned(crate::pinstore::in_folder(dir));
        *self.selection.borrow_mut() = Selection::default();
        self.objects.borrow_mut().clear();
        self.git.borrow_mut().clear();
        self.measured.borrow_mut().clear();
        if let Some(c) = self.measure_cancel.borrow_mut().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.progress.set(None);
        self.fs_info.set((None, false));
        *self.error.borrow_mut() = None;
        *self.want_cursor.borrow_mut() = None;
        self.store.remove_all();
        self.loading.set(true);
        // column titles follow the sort and the folder ("trashed" in the Trash)
        self.build_header();
        self.update_stack();
        self.watcher.replace(None);
        let flat = self.flat.get();
        let weak = Rc::downgrade(self);
        // a flat listing spans many folders; it is a snapshot, refreshed with reload
        let biggest = self.biggest.borrow().clone();
        let watched = if flat || biggest.is_some() {
            Err(glib::Error::new(gio::IOErrorEnum::NotSupported, "flat"))
        } else {
            listing::watch(dir, move |change| {
                if let Some(p) = weak.upgrade() {
                    p.apply_change(change);
                }
            })
        };
        match watched {
            Ok(w) => {
                self.watcher.replace(Some(w));
            }
            Err(_) => {
                // some file systems can't be watched; the listing still works
            }
        }
        let l = if let Some(b) = biggest {
            listing::spawn_biggest(b.roots, b.n, b.one_fs)
        } else if flat {
            listing::spawn_flat(dir, self.model.borrow().options().show_hidden)
        } else {
            listing::spawn(dir)
        };
        let rx = l.events.clone();
        self.listing.replace(Some(l));
        let started = Instant::now();
        self.timing.set((None, None));
        let weak = Rc::downgrade(self);
        let dir = dir.to_path_buf();
        glib::spawn_future_local(async move {
            // The first batch is shown at once; later ones are merged and shown at most every
            // REFRESH_EVERY, so a huge folder costs a few sorts and splices instead of one per
            // batch, and the main loop gets to draw and handle input in between.
            const REFRESH_EVERY: std::time::Duration = std::time::Duration::from_millis(250);
            let mut shown_once = false;
            let mut last_refresh = Instant::now();
            let mut pending: Vec<Entry> = Vec::new();
            while let Ok(first_ev) = rx.recv().await {
                let mut events = vec![first_ev];
                while let Ok(more) = rx.try_recv() {
                    events.push(more);
                }
                let Some(p) = weak.upgrade() else { return };
                if p.dir() != dir {
                    return;
                }
                let mut end = None;
                for ev in events {
                    match ev {
                        ListEvent::Batch(mut batch) => {
                            // folders show (and sort by) their last known size at once, not
                            // after the listing is done
                            for e in batch.iter_mut().filter(|e| e.is_dir() && !e.symlink) {
                                if let Some((n, _)) = cached_size(&e.path) {
                                    e.size = n;
                                    p.measured.borrow_mut().insert(e.name.clone());
                                }
                            }
                            pending.extend(batch)
                        }
                        ListEvent::Progress(n) => {
                            p.progress.set(Some(n));
                            (p.on_event)(PaneEvent::Cursor);
                        }
                        ListEvent::HiddenNames(names) => {
                            p.model.borrow_mut().set_hidden_names(names)
                        }
                        other => end = Some(other),
                    }
                }
                let due = !shown_once || last_refresh.elapsed() >= REFRESH_EVERY;
                if (due || end.is_some()) && !pending.is_empty() {
                    p.model.borrow_mut().extend(std::mem::take(&mut pending));
                    if end.is_none() {
                        p.refresh();
                    }
                    last_refresh = Instant::now();
                    if !shown_once {
                        shown_once = true;
                        p.note_first_paint(started);
                    }
                }
                match end {
                    Some(ListEvent::Done) => {
                        p.loading.set(false);
                        let t = p.timing.get();
                        p.timing
                            .set((t.0, Some(started.elapsed().as_secs_f64() * 1000.0)));
                        p.listing.replace(None);
                        p.refresh();
                        p.progress.set(None);
                        if !p.in_archive.get() {
                            p.load_git();
                            // free space, then folder sizes (skipped on network mounts)
                            p.load_fs_info();
                        }
                        (p.on_event)(PaneEvent::Dir);
                        return;
                    }
                    Some(ListEvent::Error(e)) => {
                        if p.model.borrow().total() == 0 {
                            *p.error.borrow_mut() = Some(e.clone());
                        }
                        p.loading.set(false);
                        p.listing.replace(None);
                        p.refresh();
                        (p.on_event)(PaneEvent::Error(e));
                        (p.on_event)(PaneEvent::Dir);
                        return;
                    }
                    _ => {}
                }
                drop(p);
                // let GTK draw and handle input before the next round
                glib::timeout_future(std::time::Duration::from_millis(1)).await;
            }
        });
        (self.on_event)(PaneEvent::Dir);
    }

    /// Records when the first rows were actually drawn (after the next frame).
    fn note_first_paint(self: &Rc<Self>, started: Instant) {
        let weak = Rc::downgrade(self);
        let Some(clock) = self.root.frame_clock() else {
            return;
        };
        let id: Rc<Cell<Option<glib::SignalHandlerId>>> = Rc::default();
        let id2 = id.clone();
        let clock2 = clock.clone();
        let handler = clock.connect_after_paint(move |_| {
            if let Some(p) = weak.upgrade() {
                let t = p.timing.get();
                if t.0.is_none() {
                    p.timing
                        .set((Some(started.elapsed().as_secs_f64() * 1000.0), t.1));
                }
            }
            if let Some(h) = id2.take() {
                clock2.disconnect(h);
            }
        });
        id.set(Some(handler));
    }

    fn load_git(self: &Rc<Self>) {
        let dir = self.dir();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let d = dir.clone();
            let Ok(Some(badges)) = gio::spawn_blocking(move || vector_core::git::badges(&d)).await
            else {
                return;
            };
            let Some(p) = weak.upgrade() else { return };
            if p.dir() == dir && !badges.is_empty() {
                *p.git.borrow_mut() = badges;
                p.rebind();
            }
        });
    }

    fn apply_change(self: &Rc<Self>, change: DirChange) {
        let touches_hidden_file = matches!(&change,
            DirChange::Upsert(e) if e.name == ".hidden")
            || matches!(&change, DirChange::Remove(n) if n == ".hidden");
        if touches_hidden_file {
            self.reload_hidden_names();
        }
        match change {
            DirChange::Upsert(e) => {
                self.objects.borrow_mut().remove(&e.name);
                self.model.borrow_mut().upsert(e);
            }
            DirChange::Remove(name) => {
                self.objects.borrow_mut().remove(&name);
                self.model.borrow_mut().remove(&name);
            }
            DirChange::Gone => {
                // the folder was deleted or unmounted: go to the closest parent that exists
                let mut up = self.dir();
                while !up.is_dir() {
                    if !up.pop() {
                        break;
                    }
                }
                self.load(&up);
                return;
            }
        }
        if !self.loading.get() {
            self.schedule_refresh();
        }
    }

    /// Many monitor events can arrive at once (a copy, a busy dotfile folder); show them in one
    /// refresh on the next idle instead of one per event, so input never waits behind them.
    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_queued.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
            if let Some(p) = weak.upgrade() {
                p.refresh_queued.set(false);
                p.refresh();
            }
        });
    }

    fn reload_hidden_names(self: &Rc<Self>) {
        let dir = self.dir();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let d = dir.clone();
            let Ok(names) = gio::spawn_blocking(move || listing::read_hidden_file(&d)).await else {
                return;
            };
            let Some(p) = weak.upgrade() else { return };
            if p.dir() == dir {
                p.model.borrow_mut().set_hidden_names(names);
                p.schedule_refresh();
            }
        });
    }

    // ── model → view ────────────────────────────────────────────────────────

    /// Mirrors the shown entries into the store, reusing objects of unchanged entries.
    pub fn refresh(&self) {
        let objs: Vec<glib::BoxedAnyObject> = {
            let model = self.model.borrow();
            let mut cache = self.objects.borrow_mut();
            model
                .iter()
                .map(|e| {
                    cache
                        .entry(e.name.clone())
                        .or_insert_with(|| glib::BoxedAnyObject::new(e.clone()))
                        .clone()
                })
                .collect()
        };
        self.syncing.set(true);
        self.store.splice(0, self.store.n_items(), &objs);
        self.syncing.set(false);
        self.selection.borrow_mut().prune(&self.model.borrow());
        let want = self.want_cursor.borrow().clone();
        if let Some(name) = want
            && self.model.borrow().index_of(&name).is_some()
        {
            self.selection
                .borrow_mut()
                .set_cursor_name(&self.model.borrow(), &name);
            *self.want_cursor.borrow_mut() = None;
        }
        self.update_stack();
        self.push_selection(true);
        (self.on_event)(PaneEvent::Cursor);
    }

    /// Re-binds every row (badges or folder sizes changed) without changing the order.
    pub fn rebind(&self) {
        self.objects.borrow_mut().clear();
        self.refresh();
    }

    fn update_stack(&self) {
        let empty = self.model.borrow().is_empty();
        // a slow folder must not look empty, and an unreadable one must say why
        let text = if let Some(e) = self.error.borrow().as_ref() {
            Some(
                locked_text(&self.dir())
                    .filter(|_| is_permission(e))
                    .unwrap_or_else(|| format!("can't open this folder: {e}")),
            )
        } else if empty && self.loading.get() {
            Some(match self.progress.get() {
                Some(n) => format!("scanning… {} files", vector_core::details::group_digits(n)),
                None => "loading…".to_string(),
            })
        } else if empty {
            let m = self.model.borrow();
            Some(if !m.options().filter.is_empty() {
                format!("nothing matches {}", m.options().filter)
            } else if m.total() > 0 {
                format!("{} hidden items · zh shows them", m.total())
            } else {
                "empty folder".to_string()
            })
        } else {
            None
        };
        match text {
            Some(t) => {
                self.empty.set_text(&t);
                self.stack.set_visible_child_name("empty");
            }
            None => self.stack.set_visible_child_name(if self.grid_mode.get() {
                "grid"
            } else {
                "list"
            }),
        }
    }

    /// Column titles over the list; the sorted one is highlighted with its direction, and a
    /// click sorts by that column (again: reverse).
    fn build_header(self: &Rc<Self>) {
        use vector_core::sort::SortKey;
        while let Some(c) = self.header.first_child() {
            self.header.remove(&c);
        }
        let sort = self.model.borrow().options().sort;
        let col = |text: &str, key: SortKey, width: i32, xalign: f32, hexpand: bool| {
            let active = sort.key == key;
            // name and type read a→z, size and date biggest/newest first
            let natural_down = matches!(key, SortKey::Size | SortKey::Modified | SortKey::Added);
            let arrow = if active {
                if sort.reverse ^ natural_down {
                    " ↓"
                } else {
                    " ↑"
                }
            } else {
                ""
            };
            let l = gtk::Label::new(Some(&format!("{text}{arrow}")));
            l.set_xalign(xalign);
            l.set_hexpand(hexpand);
            if width > 0 {
                l.set_width_chars(width);
            }
            if active {
                l.add_css_class("ok");
            }
            l.set_cursor_from_name(Some("pointer"));
            let click = gtk::GestureClick::new();
            let weak = Rc::downgrade(self);
            click.connect_released(move |_, _, _, _| {
                if let Some(p) = weak.upgrade() {
                    (p.on_event)(PaneEvent::SortBy(key));
                }
            });
            l.add_controller(click);
            l
        };
        if self.grid_mode.get() {
            // the grid has no columns: the same sort keys in one row
            let title = gtk::Label::new(Some("sort"));
            title.add_css_class("faint");
            self.header.append(&title);
            for (text, key) in [
                ("name", SortKey::Name),
                ("size", SortKey::Size),
                ("modified", SortKey::Modified),
                (
                    if vector_core::trash::is_trash(&self.dir()) {
                        "trashed"
                    } else {
                        "added"
                    },
                    SortKey::Added,
                ),
                ("type", SortKey::Type),
            ] {
                let l = col(text, key, 0, 0.0, false);
                l.set_margin_start(10);
                self.header.append(&l);
            }
            return;
        }
        let icon = gtk::Label::new(None);
        icon.add_css_class("icon");
        self.header.append(&icon);
        self.header
            .append(&col("name", SortKey::Name, 16, 0.0, true));
        // the other date to sort by; the date column shows whichever is chosen
        let trash = vector_core::trash::is_trash(&self.dir());
        let added_name = if trash { "trashed" } else { "added" };
        if !self.compact.get() {
            let other = if sort.key == SortKey::Added {
                col("modified", SortKey::Modified, 0, 1.0, false)
            } else {
                col(added_name, SortKey::Added, 0, 1.0, false)
            };
            other.add_css_class("faint-col");
            other.set_margin_end(12);
            self.header.append(&other);
        }
        let badge = gtk::Label::new(None);
        badge.add_css_class("badge");
        self.header.append(&badge);
        self.header
            .append(&col("size", SortKey::Size, 9, 1.0, false));
        let dot = gtk::Label::new(Some("·"));
        dot.add_css_class("sep-dot");
        // keeps the column titles aligned with the rows; invisible in the header
        dot.set_opacity(0.0);
        dot.set_visible(!self.compact.get());
        self.header.append(&dot);
        let date = if sort.key == SortKey::Added {
            col(added_name, SortKey::Added, 16, 1.0, false)
        } else {
            col("modified", SortKey::Modified, 16, 1.0, false)
        };
        date.set_visible(!self.compact.get());
        self.header.append(&date);
    }

    /// Gives a just-bound row the cursor mark if it is the cursor row.
    fn register_row(&self, w: &gtk::Widget, pos: u32) {
        let cur = {
            let m = self.model.borrow();
            self.selection.borrow().cursor(&m).map(|c| c as u32)
        };
        if cur == Some(pos) {
            w.add_css_class("cursor");
        } else {
            w.remove_css_class("cursor");
        }
    }

    /// Marks the cursor row (accent) apart from other selected rows (grey). Walks the rows
    /// GTK has on screen; each knows the position it is bound to.
    fn mark_cursor(&self) {
        let cur = {
            let m = self.model.borrow();
            self.selection.borrow().cursor(&m).map(|c| c as u32)
        };
        for view in [
            self.list.upcast_ref::<gtk::Widget>(),
            self.grid.upcast_ref(),
        ] {
            let mut row = view.first_child();
            while let Some(r) = row {
                if let Some(content) = r.first_child() {
                    let pos = position_of(&content);
                    if pos.is_some() && pos == cur {
                        content.add_css_class("cursor");
                    } else {
                        content.remove_css_class("cursor");
                    }
                }
                row = r.next_sibling();
            }
        }
    }

    /// Copies the core selection into the GTK selection and scrolls to the cursor.
    pub fn push_selection(&self, scroll: bool) {
        self.mark_cursor();
        let model = self.model.borrow();
        let s = self.selection.borrow();
        let n = model.len() as u32;
        let set = gtk::Bitset::new_empty();
        if s.has_selection() {
            for (i, e) in model.iter().enumerate() {
                if s.is_selected(&e.name) {
                    set.add(i as u32);
                }
            }
        } else if let Some(c) = s.cursor(&model) {
            set.add(c as u32);
        }
        self.syncing.set(true);
        self.sel.set_selection(&set, &gtk::Bitset::new_range(0, n));
        self.syncing.set(false);
        if scroll && let Some(c) = s.cursor(&model) {
            let flags = if self.has_focus() {
                gtk::ListScrollFlags::FOCUS
            } else {
                gtk::ListScrollFlags::NONE
            };
            if self.grid_mode.get() {
                self.grid.scroll_to(c as u32, flags, None);
            } else {
                self.list.scroll_to(c as u32, flags, None);
            }
        }
    }

    fn has_focus(&self) -> bool {
        self.list.has_focus() || self.grid.has_focus() || self.root.focus_child().is_some()
    }

    /// GTK selection changed by mouse or GTK's own keys: copy it into the core.
    fn pull_selection(&self) {
        if self.syncing.get() {
            return;
        }
        let picked: Vec<u32> = {
            let bits = self.sel.selection();
            (0..bits.size()).map(|i| bits.nth(i as u32)).collect()
        };
        {
            let model = self.model.borrow();
            let mut s = self.selection.borrow_mut();
            match picked.as_slice() {
                [] => s.clear(),
                [one] => {
                    s.clear();
                    s.set_cursor(&model, *one as usize);
                }
                many => {
                    s.end_visual();
                    s.set_selected(
                        many.iter()
                            .filter_map(|&i| model.get(i as usize))
                            .map(|e| e.name.clone()),
                    );
                    // the cursor follows the item GTK focused, if it is one of them
                    let focus = self.focused_position();
                    if let Some(f) = focus.filter(|f| many.contains(f)) {
                        s.set_cursor(&model, f as usize);
                    }
                }
            }
        }
        (self.on_event)(PaneEvent::Cursor);
    }

    fn focused_position(&self) -> Option<u32> {
        let w = self.root.root()?.focus()?;
        let mut cur = Some(w);
        while let Some(c) = cur {
            if let Some(pos) = position_of(&c) {
                return Some(pos);
            }
            cur = c.parent();
        }
        None
    }

    /// Paths an action applies to: the selection, else the cursor entry.
    pub fn targets(&self) -> Vec<Entry> {
        let m = self.model.borrow();
        self.selection
            .borrow()
            .targets(&m)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn cursor_entry(&self) -> Option<Entry> {
        let m = self.model.borrow();
        self.selection.borrow().cursor_entry(&m).cloned()
    }

    /// Selects `paths` that are in this folder (after a paste), cursor on the first.
    pub fn select_paths(&self, paths: &[PathBuf]) {
        let dir = self.dir();
        let names: Vec<String> = paths
            .iter()
            .filter(|p| p.parent() == Some(dir.as_path()))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        let Some(first) = names.first().cloned() else {
            return;
        };
        if self.model.borrow().index_of(&first).is_none() {
            // the monitor hasn't delivered it yet; select it when it arrives
            *self.want_cursor.borrow_mut() = Some(first);
            return;
        }
        {
            let m = self.model.borrow();
            let mut s = self.selection.borrow_mut();
            s.clear();
            s.set_cursor_name(&m, &first);
            if names.len() > 1 {
                s.set_selected(names);
            }
        }
        self.push_selection(true);
        (self.on_event)(PaneEvent::Cursor);
    }

    /// Rows that fit in the view, for page up/down.
    pub fn page_rows(&self) -> isize {
        let h = self.stack.height();
        ((h / 30).max(2) - 1) as isize
    }

    pub fn columns(&self) -> isize {
        if !self.grid_mode.get() {
            return 1;
        }
        // tiles are 116 px wide including padding (see tile_factory)
        (self.grid.width() / 116).max(1) as isize
    }
}

/// A `:biggest` view: where it looks, how many, whether it stays on one file system, and the
/// sort to go back to afterwards.
#[derive(Clone)]
struct Biggest {
    roots: Vec<PathBuf>,
    n: usize,
    one_fs: bool,
    sort_before: vector_core::sort::SortSpec,
}

/// Measurements younger than this are trusted without measuring again.
const SIZE_FRESH_FOR: i64 = 300;

/// Whether a listing error means "not allowed" rather than a broken folder.
pub fn is_permission(error: &str) -> bool {
    error.contains("Permission denied") || error.contains("Operation not permitted")
}

/// The calm explanation for a folder this user may not read.
fn locked_text(dir: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(dir).ok()?;
    Some(format!(
        "\u{F033E}  locked · owned by {} ({}) · gx opens a terminal here, where sudo works",
        crate::details::user_name(m.uid()),
        vector_core::details::permissions(m.mode()),
    ))
}

thread_local! {
    /// Folder sizes from this and earlier runs (`~/.cache/vector/sizes`), loaded on first use.
    static INDEX: RefCell<Option<vector_core::du::SizeIndex>> = const { RefCell::new(None) };
    static SAVE_QUEUED: Cell<bool> = const { Cell::new(false) };
}

fn index_path() -> PathBuf {
    glib::user_cache_dir()
        .join(vector_core::APP_NAME)
        .join("sizes")
}

fn with_index<T>(f: impl FnOnce(&mut vector_core::du::SizeIndex) -> T) -> T {
    INDEX.with(|i| {
        let mut i = i.borrow_mut();
        let idx = i.get_or_insert_with(|| {
            // a few MB at most, read once
            let text = std::fs::read_to_string(index_path()).unwrap_or_default();
            vector_core::du::SizeIndex::parse(&text)
        });
        f(idx)
    })
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Last known size of a folder, if it was ever measured (sidebar tooltips).
pub fn known_size(p: &Path) -> Option<u64> {
    with_index(|i| i.get(p)).map(|(n, _)| n)
}

/// Last known size and its age in seconds.
fn cached_size(p: &Path) -> Option<(u64, i64)> {
    with_index(|i| i.get(p)).map(|(n, at)| (n, now() - at))
}

fn remember_size(p: &Path, n: u64) {
    with_index(|i| i.insert(p, n, now()));
    // write the index a few seconds after the last change, off the main thread
    if SAVE_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    glib::timeout_add_local_once(std::time::Duration::from_secs(5), || {
        SAVE_QUEUED.with(|q| q.set(false));
        let text = with_index(|i| i.render());
        gio::spawn_blocking(move || save_index_text(&text));
    });
}

fn save_index_text(text: &str) {
    let p = index_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // write then rename, so a crash never leaves half an index
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// Writes the size index now (at exit).
pub fn save_index() {
    if INDEX.with(|i| i.borrow().is_some()) {
        save_index_text(&with_index(|i| i.render()));
    }
}

fn same_device(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::symlink_metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

// ── row and tile widgets ────────────────────────────────────────────────────

fn entry_of(item: &gtk::ListItem) -> Option<Entry> {
    let obj = item.item()?.downcast::<glib::BoxedAnyObject>().ok()?;
    let e = obj.try_borrow::<Entry>().ok()?.clone();
    Some(e)
}

/// Shared per-row setup: remembers its position for focus tracking, drag source, drop target
/// for folders, right click.
fn common_setup(w: &gtk::Widget, item: &gtk::ListItem, pane: &Weak<Pane>) {
    // SAFETY: the key is private to this module and always holds a WeakRef<ListItem>
    unsafe { w.set_data("vector-item", item.downgrade()) };
    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let (p, it) = (pane.clone(), item.downgrade());
    drag.connect_prepare(move |_, _, _| {
        let pane = p.upgrade()?;
        let item = it.upgrade()?;
        let e = entry_of(&item)?;
        let sel = pane.selection.borrow();
        let paths: Vec<PathBuf> = if sel.is_selected(&e.name) {
            pane.targets().into_iter().map(|e| e.path).collect()
        } else {
            vec![e.path]
        };
        let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
        Some(gdk::ContentProvider::for_value(
            &gdk::FileList::from_array(&files).to_value(),
        ))
    });
    w.add_controller(drag);

    let drop = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    let (p, it) = (pane.clone(), item.downgrade());
    drop.connect_accept(move |_, _| {
        let Some(e) = it.upgrade().and_then(|i| entry_of(&i)) else {
            return false;
        };
        e.is_dir() && p.upgrade().is_some()
    });
    let (p, it) = (pane.clone(), item.downgrade());
    drop.connect_drop(move |t, value, _, _| {
        let (Some(pane), Some(e)) = (p.upgrade(), it.upgrade().and_then(|i| entry_of(&i))) else {
            return false;
        };
        pane.drop_files(t, value, e.path)
    });
    let wk = w.downgrade();
    drop.connect_enter(move |_, _, _| {
        if let Some(w) = wk.upgrade() {
            w.add_css_class("drop-target");
        }
        gdk::DragAction::MOVE
    });
    let wk = w.downgrade();
    drop.connect_leave(move |_| {
        if let Some(w) = wk.upgrade() {
            w.remove_css_class("drop-target");
        }
    });
    w.add_controller(drop);

    let menu = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let (p, it) = (pane.clone(), item.downgrade());
    menu.connect_pressed(move |g, _, x, y| {
        let (Some(pane), Some(item)) = (p.upgrade(), it.upgrade()) else {
            return;
        };
        g.set_state(gtk::EventSequenceState::Claimed);
        let pos = item.position();
        let already = entry_of(&item).is_some_and(|e| pane.selection.borrow().is_selected(&e.name));
        if !already {
            pane.selection.borrow_mut().clear();
        }
        pane.selection
            .borrow_mut()
            .set_cursor(&pane.model.borrow(), pos as usize);
        pane.push_selection(false);
        (pane.on_event)(PaneEvent::Cursor);
        if let Some(w) = g.widget() {
            (pane.on_event)(PaneEvent::Menu(w, x, y));
        }
    });
    w.add_controller(menu);
}

/// Where the row's item is in the list now. Asked of the list item itself: GTK moves rows
/// to new positions (after a sort) without binding them again, so a remembered position
/// goes stale and would light up the wrong row.
fn position_of(w: &gtk::Widget) -> Option<u32> {
    // SAFETY: see common_setup
    let item = unsafe { w.data::<glib::WeakRef<gtk::ListItem>>("vector-item") }?;
    let item = unsafe { item.as_ref() }.upgrade()?;
    let pos = item.position();
    (pos != gtk::INVALID_LIST_POSITION).then_some(pos)
}

fn label(classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

fn badge_text(b: Option<Badge>) -> (&'static str, &'static str) {
    match b {
        Some(Badge::Conflicted) => ("!", "crit"),
        Some(Badge::Modified) => ("M", "warn"),
        Some(Badge::Added) => ("A", "good"),
        Some(Badge::Untracked) => ("?", "dim"),
        None => ("", ""),
    }
}

fn row_factory(pane: Weak<Pane>) -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    let p = pane.clone();
    f.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("file");
        let icon = label(&["icon"]);
        let name = label(&["name"]);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        // the name always keeps room for this many characters; other columns give way first
        name.set_width_chars(16);
        let badge = label(&["badge"]);
        badge.set_xalign(0.5);
        let size = label(&["meta"]);
        size.set_width_chars(9);
        size.set_xalign(1.0);
        let date = label(&["meta"]);
        date.set_width_chars(16);
        date.set_ellipsize(gtk::pango::EllipsizeMode::End);
        date.set_xalign(1.0);
        let dot = label(&["sep-dot"]);
        dot.set_text("·");
        for w in [&icon, &name, &badge, &size, &dot, &date] {
            row.append(w);
        }
        item.set_child(Some(&row));
        common_setup(row.upcast_ref(), item, &p);
    });
    f.connect_bind(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(row), Some(e), Some(pane)) = (item.child(), entry_of(item), pane.upgrade())
        else {
            return;
        };
        pane.register_row(&row, item.position());
        let labels: Vec<gtk::Label> =
            std::iter::successors(row.first_child(), |w| w.next_sibling())
                .filter_map(|w| w.downcast::<gtk::Label>().ok())
                .collect();
        let [icon, name, badge, size, dot, date] = labels.as_slice() else {
            return;
        };
        match crate::iconstore::get(&e.path) {
            Some(g) => icon.set_text(&g),
            None => icon.set_text(vector_core::icons::icon_for(&e.name, e.is_dir(), e.symlink)),
        }
        name.set_text(&e.name);
        let (marked, cut) = {
            let m = pane.marks.borrow();
            (m.0.contains(&e.path), m.1)
        };
        for (class, on) in [("marked", marked), ("cut", marked && cut)] {
            if on {
                row.add_css_class(class);
            } else {
                row.remove_css_class(class);
            }
        }
        if pane.model.borrow().is_hidden(&e) {
            row.add_css_class("hidden-file");
        } else {
            row.remove_css_class("hidden-file");
        }
        let (b, class) = match pane.git_badge(&e.name) {
            None if pane.model.borrow().is_pinned(&e.name) => ("\u{F0403}", "ok"),
            g => badge_text(g),
        };
        badge.set_text(b);
        for c in ["crit", "warn", "good", "dim", "ok"] {
            badge.remove_css_class(c);
        }
        if !class.is_empty() {
            badge.add_css_class(class);
        }
        let s = if e.is_dir() {
            if pane.measured.borrow().contains(&e.name) {
                fmt::size(e.size)
            } else if pane.is_measuring() {
                "…".into()
            } else {
                String::new()
            }
        } else if e.broken {
            "broken".into()
        } else {
            fmt::size(e.size)
        };
        size.set_text(&s);
        // the date column shows the date the folder is sorted by
        let by_added = pane.model.borrow().options().sort.key == vector_core::sort::SortKey::Added;
        date.set_text(&fmt::date(if by_added { e.added } else { e.modified }));
        date.set_visible(!pane.compact.get());
        dot.set_visible(!pane.compact.get() && !s.is_empty() && e.modified > 0);
    });
    f
}

fn tile_factory(pane: Weak<Pane>) -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    let p = pane.clone();
    f.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tile.add_css_class("tile");
        tile.set_valign(gtk::Align::Start);
        tile.set_size_request(100, -1);
        let pic_stack = gtk::Stack::new();
        pic_stack.set_size_request(96, 72);
        let icon = label(&["big-icon"]);
        icon.set_xalign(0.5);
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        // the thumbnail fills a fixed box, so every tile is as tall and names line up
        let frame = gtk::Overlay::new();
        let fixed = gtk::Box::new(gtk::Orientation::Vertical, 0);
        fixed.set_size_request(96, 72);
        frame.set_child(Some(&fixed));
        frame.add_overlay(&picture);
        pic_stack.add_named(&icon, Some("icon"));
        pic_stack.add_named(&frame, Some("picture"));
        let name = label(&["name"]);
        name.set_xalign(0.5);
        name.set_justify(gtk::Justification::Center);
        name.set_wrap(true);
        name.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        name.set_lines(2);
        name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name.set_max_width_chars(12);
        name.set_width_chars(12);
        tile.append(&pic_stack);
        tile.append(&name);
        item.set_child(Some(&tile));
        common_setup(tile.upcast_ref(), item, &p);
    });
    f.connect_bind(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(tile), Some(e)) = (item.child(), entry_of(item)) else {
            return;
        };
        if let Some(p) = pane.upgrade() {
            p.register_row(&tile, item.position());
        }
        let Some(pic_stack) = tile.first_child().and_downcast::<gtk::Stack>() else {
            return;
        };
        let Some(name) = pic_stack.next_sibling().and_downcast::<gtk::Label>() else {
            return;
        };
        let (Some(icon), Some(picture)) = (
            pic_stack.child_by_name("icon").and_downcast::<gtk::Label>(),
            pic_stack
                .child_by_name("picture")
                .and_then(|frame| frame.last_child())
                .and_downcast::<gtk::Picture>(),
        ) else {
            return;
        };
        name.set_text(&e.name);
        icon.set_text(vector_core::icons::icon_for(&e.name, e.is_dir(), e.symlink));
        pic_stack.set_visible_child_name("icon");
        picture.set_paintable(None::<&gdk::Paintable>);
        if e.content_type.starts_with("image/") && !e.broken {
            let path = e.path.clone();
            let want = path.clone();
            let (ps, pic) = (pic_stack.downgrade(), picture.downgrade());
            let it = item.downgrade();
            let _ = pane.upgrade();
            glib::spawn_future_local(async move {
                let Ok(Some(tex)) = crate::thumbs::load(path, 192).await else {
                    return;
                };
                let (Some(ps), Some(pic), Some(item)) = (ps.upgrade(), pic.upgrade(), it.upgrade())
                else {
                    return;
                };
                // the tile may show another file by now
                if entry_of(&item).map(|e| e.path) == Some(want) {
                    pic.set_paintable(Some(&tex));
                    ps.set_visible_child_name("picture");
                }
            });
        }
    });
    f
}
