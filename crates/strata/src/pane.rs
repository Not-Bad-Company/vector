//! One folder view: the core [`DirModel`] and [`Selection`] mirrored into a GTK list or grid.
//! Listing runs on a worker thread and arrives in batches; the monitor keeps it current.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Instant;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use strata_core::entry::Entry;
use strata_core::git::Badge;
use strata_core::listing::{self, DirChange, ListEvent, Listing, Watcher};
use strata_core::model::{DirModel, ViewOptions};
use strata_core::selection::Selection;

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
}

pub struct Pane {
    pub root: gtk::Box,
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
    pub sizes: RefCell<HashMap<String, u64>>,
    grid_mode: Cell<bool>,
    syncing: Cell<bool>,
    pub loading: Cell<bool>,
    /// Name to put the cursor on once it shows up (after going to the parent folder).
    want_cursor: RefCell<Option<String>>,
    on_event: Box<dyn Fn(PaneEvent)>,
    /// Timings of the last load: (first rows shown, all rows listed), for `STRATA_TIMING`.
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
        root.append(&stack);

        let pane = Rc::new(Pane {
            root,
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
            sizes: RefCell::default(),
            grid_mode: Cell::new(false),
            syncing: Cell::new(false),
            loading: Cell::new(false),
            want_cursor: RefCell::new(None),
            on_event: Box::new(on_event),
            timing: Cell::new((None, None)),
        });
        pane.list
            .set_factory(Some(&row_factory(Rc::downgrade(&pane))));
        pane.grid
            .set_factory(Some(&tile_factory(Rc::downgrade(&pane))));
        pane.connect_signals();
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

    pub fn set_grid(&self, grid: bool) {
        self.grid_mode.set(grid);
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
        let cur = self.dir();
        if cur != dir && self.store.n_items() > 0 || self.loading.get() {
            self.back.borrow_mut().push(cur);
            self.forward.borrow_mut().clear();
        }
        self.load(dir);
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
        let opts = ViewOptions {
            filter: String::new(),
            ..opts
        };
        *self.model.borrow_mut() = DirModel::new(dir, opts);
        *self.selection.borrow_mut() = Selection::default();
        self.objects.borrow_mut().clear();
        self.git.borrow_mut().clear();
        self.sizes.borrow_mut().clear();
        *self.want_cursor.borrow_mut() = None;
        self.store.remove_all();
        self.loading.set(true);
        self.update_stack();
        self.watcher.replace(None);
        let weak = Rc::downgrade(self);
        match listing::watch(dir, move |change| {
            if let Some(p) = weak.upgrade() {
                p.apply_change(change);
            }
        }) {
            Ok(w) => {
                self.watcher.replace(Some(w));
            }
            Err(_) => {
                // some file systems can't be watched; the listing still works
            }
        }
        let l = listing::spawn(dir);
        let rx = l.events.clone();
        self.listing.replace(Some(l));
        let started = Instant::now();
        self.timing.set((None, None));
        let weak = Rc::downgrade(self);
        let dir = dir.to_path_buf();
        glib::spawn_future_local(async move {
            let mut first = true;
            while let Ok(ev) = rx.recv().await {
                let Some(p) = weak.upgrade() else { return };
                if p.dir() != dir {
                    return;
                }
                match ev {
                    ListEvent::Batch(batch) => {
                        p.model.borrow_mut().extend(batch);
                        p.refresh();
                        if first {
                            first = false;
                            p.note_first_paint(started);
                        }
                    }
                    ListEvent::Done => {
                        p.loading.set(false);
                        let t = p.timing.get();
                        p.timing
                            .set((t.0, Some(started.elapsed().as_secs_f64() * 1000.0)));
                        p.listing.replace(None);
                        p.refresh();
                        p.load_git();
                        (p.on_event)(PaneEvent::Dir);
                        return;
                    }
                    ListEvent::Error(e) => {
                        p.loading.set(false);
                        p.listing.replace(None);
                        p.update_stack();
                        (p.on_event)(PaneEvent::Error(e));
                        (p.on_event)(PaneEvent::Dir);
                        return;
                    }
                }
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
            let Ok(Some(badges)) = gio::spawn_blocking(move || strata_core::git::badges(&d)).await
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
            self.refresh();
        }
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
        if empty && !self.loading.get() {
            let filtered = !self.model.borrow().options().filter.is_empty();
            self.empty.set_text(if filtered {
                "no matches"
            } else {
                "empty folder"
            });
            self.stack.set_visible_child_name("empty");
        } else {
            self.stack
                .set_visible_child_name(if self.grid_mode.get() { "grid" } else { "list" });
        }
    }

    /// Copies the core selection into the GTK selection and scrolls to the cursor.
    pub fn push_selection(&self, scroll: bool) {
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
            if let Some(pos) = unsafe { c.data::<Cell<u32>>("strata-pos") } {
                // SAFETY: only ever set to Cell<u32> by the factories below
                return Some(unsafe { pos.as_ref() }.get());
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
    // SAFETY: the key is private to this module and always holds a Cell<u32>
    unsafe { w.set_data("strata-pos", Cell::new(u32::MAX)) };
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

fn set_pos(w: &gtk::Widget, pos: u32) {
    // SAFETY: see common_setup
    if let Some(cell) = unsafe { w.data::<Cell<u32>>("strata-pos") } {
        unsafe { cell.as_ref() }.set(pos);
    }
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
        let badge = label(&["badge"]);
        badge.set_xalign(0.5);
        let size = label(&["meta"]);
        size.set_width_chars(9);
        size.set_xalign(1.0);
        let date = label(&["meta"]);
        date.set_width_chars(16);
        date.set_xalign(1.0);
        for w in [&icon, &name, &badge, &size, &date] {
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
        set_pos(&row, item.position());
        let labels: Vec<gtk::Label> =
            std::iter::successors(row.first_child(), |w| w.next_sibling())
                .filter_map(|w| w.downcast::<gtk::Label>().ok())
                .collect();
        let [icon, name, badge, size, date] = labels.as_slice() else {
            return;
        };
        icon.set_text(strata_core::icons::icon_for(&e.name, e.is_dir(), e.symlink));
        name.set_text(&e.name);
        if e.hidden {
            row.add_css_class("hidden-file");
        } else {
            row.remove_css_class("hidden-file");
        }
        let (b, class) = badge_text(pane.git_badge(&e.name));
        badge.set_text(b);
        for c in ["crit", "warn", "good", "dim"] {
            badge.remove_css_class(c);
        }
        if !class.is_empty() {
            badge.add_css_class(class);
        }
        let s = if e.is_dir() {
            pane.sizes
                .borrow()
                .get(&e.name)
                .map(|&n| fmt::size(n))
                .unwrap_or_default()
        } else if e.broken {
            "broken".into()
        } else {
            fmt::size(e.size)
        };
        size.set_text(&s);
        date.set_text(&fmt::date(e.modified));
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
        tile.set_size_request(100, -1);
        let pic_stack = gtk::Stack::new();
        pic_stack.set_size_request(96, 72);
        let icon = label(&["big-icon"]);
        icon.set_xalign(0.5);
        let picture = gtk::Picture::new();
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        pic_stack.add_named(&icon, Some("icon"));
        pic_stack.add_named(&picture, Some("picture"));
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
        set_pos(&tile, item.position());
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
                .and_downcast::<gtk::Picture>(),
        ) else {
            return;
        };
        name.set_text(&e.name);
        icon.set_text(strata_core::icons::icon_for(&e.name, e.is_dir(), e.symlink));
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
