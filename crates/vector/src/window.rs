//! The main window: sidebar, tabs with one or two panes, path bar, preview, statusline,
//! the `:` / `/` line, dialogs, the context menu, and the key handling that ties them to the
//! core keymap and operation queue.

use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use vector_core::command::{self, Command, Toggle};
use vector_core::edit;
use vector_core::keymap::{Action, Feed, Key, Keymap, Mode};
use vector_core::model::ViewOptions;
use vector_core::ops::{Conflict, ErrorKind, Op, OpEvent, OpQueue};
use vector_core::script::{Host, Request, Script};
use vector_core::search::{self, SearchOptions};
use vector_core::sort::SortSpec;
use vector_core::theme::Theme;

use crate::pane::{Pane, PaneEvent};
use crate::preview::Preview;
use crate::sidebar::{Sidebar, SidebarEvent};

struct Tab {
    panes: RefCell<Vec<Rc<Pane>>>,
    active: Cell<usize>,
    container: gtk::Box,
    /// The tab in the tab bar: a label button and a close button.
    item: gtk::Box,
    button: gtk::Button,
}

/// File picker state (this window answers an app's open/save dialog).
struct Pick {
    kind: crate::portal::Kind,
    multiple: bool,
    directory: bool,
    filters: Vec<crate::portal::NamedFilter>,
    filter: usize,
    filter_button: Option<gtk::Button>,
    name: Option<gtk::Entry>,
    respond: Option<crate::portal::Respond>,
}

struct Register {
    paths: Vec<PathBuf>,
    cut: bool,
}

/// One `:find`/`:grep` result: relative path, line (grep), matching text (grep).
type Hit = (PathBuf, Option<u64>, String);

/// Results of `:find` / `:grep`, shown in the panel above the command line.
struct Picker {
    title: &'static str,
    root: PathBuf,
    /// (relative path, line number for grep)
    items: Vec<(PathBuf, Option<u64>, String)>,
    index: usize,
}

/// A modal card over the window.
struct Dialog {
    actions: Vec<DialogAction>,
    entry: Option<gtk::Entry>,
}

pub struct Win {
    pub window: gtk::ApplicationWindow,
    sidebar: Rc<Sidebar>,
    tabs: RefCell<Vec<Rc<Tab>>>,
    current: Cell<usize>,
    tab_bar: gtk::Box,
    stack: gtk::Stack,
    crumbs: gtk::Box,
    preview: Rc<Preview>,
    preview_box: gtk::Box,
    mode_label: gtk::Label,
    pending_label: gtk::Label,
    message: gtk::Label,
    info: gtk::Label,
    bar: Segments,
    progress: gtk::ProgressBar,
    cmd_box: gtk::Box,
    prompt: gtk::Label,
    cmd_entry: gtk::Entry,
    list_box: gtk::Box,
    overlay_layer: gtk::Box,
    mode: Cell<Mode>,
    keymap: RefCell<Keymap>,
    register: RefCell<Option<Register>>,
    last_change: Cell<Option<Action>>,
    ops: OpQueue,
    completions: RefCell<(Vec<String>, usize, usize)>,
    picker: RefCell<Option<Picker>>,
    dialog: RefCell<Option<Dialog>>,
    help: Rc<crate::help::HelpPanel>,
    help_open: Cell<bool>,
    preview_timer: RefCell<Option<glib::SourceId>>,
    edit_poll: RefCell<Option<glib::SourceId>>,
    search_cancel: RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>,
    settings: RefCell<crate::settings::Settings>,
    /// Set while the line's text is changed by code, so `changed` doesn't restart completion.
    setting_line: Cell<bool>,
    pick_slot: gtk::Box,
    pick: RefCell<Option<Pick>>,
    /// The rename card is open (statusline shows RENAME).
    renaming: Cell<bool>,
    #[cfg(feature = "terminal")]
    term: Rc<crate::termpanel::TermPanel>,
    /// Keyboard focus is in the sidebar, on this row.
    side_cursor: Cell<Option<usize>>,
    notice: gtk::Label,
    notice_timer: RefCell<Option<glib::SourceId>>,
    /// The user's init.lua and plugins.
    script: Script,
    /// The folder the `cd` handlers last heard of.
    script_dir: RefCell<PathBuf>,
    /// Folders by how often and lately they were visited: the recent list and `:z`.
    frecency: RefCell<vector_core::frecency::Frecency>,
    /// A `cd` handler is running.
    in_cd_hook: Cell<bool>,
    /// Where a `cd` handler sent us; arriving there doesn't call the handlers again.
    hook_target: RefCell<Option<PathBuf>>,
    /// How many of the scripts' mappings are in the keymap.
    script_keys: Cell<usize>,
    weak_self: RefCell<std::rc::Weak<Win>>,
}

impl Win {
    pub fn new(app: &gtk::Application, start: &Path, theme: Theme) -> Rc<Win> {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("vector");
        window.set_title(Some(vector_core::APP_NAME));
        window.set_default_size(1280, 800);
        window.set_decorated(false);

        let tab_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tab_bar.add_css_class("tabs");
        tab_bar.set_visible(false);
        let crumbs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        crumbs.set_hexpand(true);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("header");
        header.append(&crumbs);
        header.set_tooltip_text(Some("click or ctrl+l to type a path"));
        header.set_cursor_from_name(Some("text"));

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        let preview = Preview::new(&theme);
        let preview_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        preview_box.append(preview.widget());
        let settings = crate::settings::Settings::load();
        crate::pane::set_numbers(settings.numbers);
        preview_box.set_visible(settings.preview);
        // the preview can be dragged wider or narrower
        let body = gtk::Paned::new(gtk::Orientation::Horizontal);
        body.set_start_child(Some(&stack));
        body.set_end_child(Some(&preview_box));
        body.set_resize_start_child(true);
        body.set_resize_end_child(false);
        body.set_shrink_end_child(false);
        body.set_shrink_start_child(false);

        let mode_label = gtk::Label::new(Some("NORMAL"));
        mode_label.add_css_class("mode");
        let pending_label = gtk::Label::new(None);
        pending_label.add_css_class("dim");
        let message = gtk::Label::new(None);
        message.add_css_class("message");
        message.set_hexpand(true);
        message.set_xalign(0.0);
        message.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let info = gtk::Label::new(None);
        info.add_css_class("dim");
        let status = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        status.add_css_class("statusline");
        status.add_css_class("lualine");
        // nvim's lualine: the mode block, the git branch, the message; on the right what the
        // folder holds, the sort, and the position in the mode's colour again
        let bar = Segments::new(&status);
        status.append(&mode_label);
        status.append(&bar.after_mode);
        status.append(&bar.branch);
        status.append(&bar.after_branch);
        for w in [&message, &pending_label, &info] {
            status.append(w);
        }
        status.append(&bar.before_sort);
        status.append(&bar.sort);
        status.append(&bar.before_pos);
        status.append(&bar.pos);
        let progress = gtk::ProgressBar::new();
        progress.add_css_class("progress");
        progress.set_visible(false);

        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list_box.add_css_class("completions");
        list_box.set_visible(false);
        let prompt = gtk::Label::new(Some(":"));
        prompt.add_css_class("prompt");
        let cmd_entry = gtk::Entry::new();
        cmd_entry.set_hexpand(true);
        let cmd_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        cmd_box.add_css_class("cmdline");
        cmd_box.append(&prompt);
        cmd_box.append(&cmd_entry);
        cmd_box.set_visible(false);

        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.append(&tab_bar);
        main.append(&header);
        #[cfg(feature = "terminal")]
        let term = {
            let t = crate::termpanel::TermPanel::new(&theme);
            t.widget.set_visible(false);
            t
        };
        // the terminal panel sits under the list, in a split the user can drag
        #[cfg(feature = "terminal")]
        {
            let split = gtk::Paned::new(gtk::Orientation::Vertical);
            split.set_start_child(Some(&body));
            split.set_end_child(Some(&term.widget));
            split.set_resize_end_child(false);
            split.set_shrink_end_child(false);
            split.set_vexpand(true);
            main.append(&split);
        }
        #[cfg(not(feature = "terminal"))]
        main.append(&body);
        main.append(&progress);
        main.append(&list_box);
        main.append(&cmd_box);
        let pick_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pick_slot.add_css_class("pickbar");
        pick_slot.set_visible(false);
        main.append(&pick_slot);
        main.append(&status);

        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let overlay = gtk::Overlay::new();
        overlay.add_css_class("root");
        overlay.set_child(Some(&outer));
        let overlay_layer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        overlay_layer.set_halign(gtk::Align::Center);
        overlay_layer.set_valign(gtk::Align::Center);
        overlay_layer.set_visible(false);
        overlay.add_overlay(&overlay_layer);
        let notice = gtk::Label::new(None);
        notice.add_css_class("notice");
        notice.set_halign(gtk::Align::Center);
        notice.set_valign(gtk::Align::End);
        notice.set_margin_bottom(44);
        notice.set_visible(false);
        notice.set_can_target(false);
        notice.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        notice.set_max_width_chars(90);
        overlay.add_overlay(&notice);
        window.set_child(Some(&overlay));

        let win = Rc::new_cyclic(|weak: &std::rc::Weak<Win>| {
            let w = weak.clone();
            let sidebar = Sidebar::new(move |ev| {
                let Some(win) = w.upgrade() else { return };
                match ev {
                    // a pinned file opens its folder with the file selected
                    SidebarEvent::Open(path) => {
                        win.run_command(Command::Cd(path.display().to_string()))
                    }
                    SidebarEvent::OpenTab(path) => win.new_tab(&path),
                    SidebarEvent::Unpin(path) => {
                        crate::pinstore::with(|p| p.toggle_global(&path));
                        crate::pinstore::save();
                        win.refresh_pins();
                        win.notify("unpinned");
                    }
                    SidebarEvent::Forget(path) => {
                        win.frecency.borrow_mut().forget(&path);
                        win.save_frecency();
                        win.refresh_recent();
                    }
                    SidebarEvent::Notify(text) => {
                        win.notify(&text);
                        for p in win.all_panes() {
                            p.rebind();
                        }
                    }
                }
            });
            outer.append(sidebar.widget());
            outer.append(&main);
            Win {
                window,
                sidebar,
                tabs: RefCell::default(),
                current: Cell::new(0),
                tab_bar,
                stack,
                crumbs,
                preview,
                preview_box,
                mode_label,
                pending_label,
                message,
                info,
                bar,
                progress,
                cmd_box,
                prompt,
                cmd_entry,
                list_box,
                overlay_layer,
                mode: Cell::new(Mode::Normal),
                keymap: RefCell::default(),
                register: RefCell::new(None),
                last_change: Cell::new(None),
                ops: OpQueue::new(),
                completions: RefCell::default(),
                picker: RefCell::new(None),
                dialog: RefCell::new(None),
                help: crate::help::HelpPanel::new(
                    "Vector Find",
                    vector_core::help::FIND,
                    "unzip, rename, find, hidden, trash",
                ),
                help_open: Cell::new(false),
                preview_timer: RefCell::new(None),
                edit_poll: RefCell::new(None),
                search_cancel: RefCell::new(None),
                setting_line: Cell::new(false),
                pick_slot,
                pick: RefCell::new(None),
                renaming: Cell::new(false),
                side_cursor: Cell::new(None),
                #[cfg(feature = "terminal")]
                term,
                notice,
                notice_timer: RefCell::new(None),
                settings: RefCell::new(settings),
                script: Script::new(),
                script_dir: RefCell::default(),
                frecency: RefCell::new(load_frecency()),
                in_cd_hook: Cell::new(false),
                hook_target: RefCell::default(),
                script_keys: Cell::new(0),
                weak_self: RefCell::new(weak.clone()),
            }
        });
        win.sidebar.set_pinned(&crate::pinstore::global());
        win.connect_keys();
        win.connect_entries();
        win.connect_actions();
        win.watch_ops();
        win.new_tab(start);
        win.refresh_recent();
        #[cfg(feature = "terminal")]
        if win.settings.borrow().terminal {
            win.term.open(&win.pane().dir());
        }
        // audio from the preview stops with the window
        let weak = Rc::downgrade(&win);
        win.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade() {
                w.preview.stop_audio();
            }
            glib::Propagation::Proceed
        });
        win.load_scripts();
        win
    }

    fn tab(&self) -> Rc<Tab> {
        let tabs = self.tabs.borrow();
        tabs[self.current.get().min(tabs.len() - 1)].clone()
    }

    /// The pane with focus in the current tab.
    pub fn pane(&self) -> Rc<Pane> {
        let tab = self.tab();
        let panes = tab.panes.borrow();
        panes[tab.active.get().min(panes.len() - 1)].clone()
    }

    // ── tabs and panes ──────────────────────────────────────────────────────

    fn make_pane(&self, dir: &Path, opts: ViewOptions, grid: bool) -> Rc<Pane> {
        let weak = self.weak_self.borrow().clone();
        let pane_cell: Rc<RefCell<std::rc::Weak<Pane>>> = Rc::default();
        let pc = pane_cell.clone();
        let pane = Pane::new(opts, move |ev| {
            let (Some(win), Some(pane)) = (weak.upgrade(), pc.borrow().upgrade()) else {
                return;
            };
            win.on_pane_event(&pane, ev);
        });
        *pane_cell.borrow_mut() = Rc::downgrade(&pane);
        pane.set_grid(grid);
        pane.open_dir(dir);
        pane
    }

    pub fn new_tab(&self, dir: &Path) {
        let (opts, grid) = self
            .tabs
            .borrow()
            .first()
            .map(|_| {
                let p = self.pane();
                let o = p.model.borrow().options().clone();
                (
                    ViewOptions {
                        filter: String::new(),
                        ..o
                    },
                    p.is_grid(),
                )
            })
            .unwrap_or_else(|| {
                let s = self.settings.borrow();
                (
                    ViewOptions {
                        sort: s.sort,
                        show_hidden: s.show_hidden,
                        filter: String::new(),
                    },
                    s.grid,
                )
            });
        // a file picker's type filter holds in its new tabs too
        let allow = self
            .tabs
            .borrow()
            .first()
            .and_then(|_| self.pane().model.borrow().allow().cloned());
        let pane = self.make_pane(dir, opts, grid);
        pane.model.borrow_mut().set_allow(allow);
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.append(pane.widget());
        pane.widget().set_hexpand(true);
        let button = gtk::Button::new();
        button.add_css_class("link");
        let close = gtk::Button::with_label("\u{F0156}");
        close.add_css_class("link");
        close.add_css_class("x");
        close.set_tooltip_text(Some("close tab (ctrl+w, :q)"));
        let item = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        item.add_css_class("tab");
        item.append(&button);
        item.append(&close);
        let tab = Rc::new(Tab {
            panes: RefCell::new(vec![pane]),
            active: Cell::new(0),
            container,
            item,
            button,
        });
        let index_of = {
            let weak = self.weak_self.borrow().clone();
            let t = Rc::downgrade(&tab);
            move || {
                let (win, t) = (weak.upgrade()?, t.upgrade()?);
                let i = win.tabs.borrow().iter().position(|x| Rc::ptr_eq(x, &t))?;
                Some((win, i))
            }
        };
        let find = index_of.clone();
        close.connect_clicked(move |_| {
            if let Some((win, i)) = find() {
                win.close_tab_at(i);
            }
        });
        // middle click closes, like in browsers
        let middle = gtk::GestureClick::builder()
            .button(gdk::BUTTON_MIDDLE)
            .build();
        let find = index_of.clone();
        middle.connect_released(move |_, _, _, _| {
            if let Some((win, i)) = find() {
                win.close_tab_at(i);
            }
        });
        tab.item.add_controller(middle);
        tab.button.connect_clicked(move |_| {
            if let Some((win, i)) = index_of() {
                win.switch_tab(i);
            }
        });
        self.tab_bar.append(&tab.item);
        self.stack.add_child(&tab.container);
        self.tabs.borrow_mut().push(tab);
        let n = self.tabs.borrow().len();
        self.switch_tab(n - 1);
    }

    fn switch_tab(&self, i: usize) {
        let n = self.tabs.borrow().len();
        if n == 0 {
            return;
        }
        self.current.set(i % n);
        let tab = self.tab();
        self.stack.set_visible_child(&tab.container);
        self.update_panes_focus();
        self.update_chrome();
        self.pane().focus_view();
    }

    fn close_tab(&self) {
        self.close_tab_at(self.current.get());
    }

    fn close_tab_at(&self, i: usize) {
        if self.tabs.borrow().len() <= 1 {
            self.window.close();
            return;
        }
        if i >= self.tabs.borrow().len() {
            return;
        }
        let tab = self.tabs.borrow_mut().remove(i);
        self.stack.remove(&tab.container);
        self.tab_bar.remove(&tab.item);
        let cur = self.current.get();
        let next = if cur > i || cur >= self.tabs.borrow().len() {
            cur.saturating_sub(1)
        } else {
            cur
        };
        self.switch_tab(next);
    }

    /// `:q`, like nvim: closes the split pane, else the tab, else the window.
    fn quit_one(&self) {
        let tab = self.tab();
        if tab.panes.borrow().len() > 1 {
            let i = tab.active.get();
            let gone = tab.panes.borrow_mut().remove(i);
            tab.container.remove(gone.widget());
            tab.active.set(0);
            if let Some(p) = tab.panes.borrow().first() {
                p.widget().remove_css_class("pane-split");
            }
            self.update_panes_focus();
            self.pane().focus_view();
            self.update_chrome();
        } else {
            self.close_tab();
        }
    }

    fn toggle_split(&self) {
        let tab = self.tab();
        let count = tab.panes.borrow().len();
        if count == 2 {
            let other = tab.panes.borrow_mut().remove(1 - tab.active.get());
            tab.container.remove(other.widget());
            tab.active.set(0);
            if let Some(p) = tab.panes.borrow().first() {
                p.widget().remove_css_class("pane-split");
            }
        } else {
            let cur = self.pane();
            let opts = cur.model.borrow().options().clone();
            let pane = self.make_pane(
                &cur.dir(),
                ViewOptions {
                    filter: String::new(),
                    ..opts
                },
                cur.is_grid(),
            );
            pane.widget().set_hexpand(true);
            pane.widget().add_css_class("pane-split");
            tab.container.append(pane.widget());
            tab.panes.borrow_mut().push(pane);
            tab.active.set(1);
        }
        self.update_panes_focus();
        self.pane().focus_view();
        self.update_chrome();
    }

    fn other_pane(&self) {
        let tab = self.tab();
        if tab.panes.borrow().len() == 2 {
            tab.active.set(1 - tab.active.get());
            self.update_panes_focus();
            self.pane().focus_view();
            self.update_chrome();
        }
    }

    fn update_panes_focus(&self) {
        let tab = self.tab();
        let split = tab.panes.borrow().len() > 1;
        for (i, p) in tab.panes.borrow().iter().enumerate() {
            p.set_focused(!split || i == tab.active.get());
        }
    }

    /// After a pin changed: folders re-sort, the sidebar lists the global pins.
    fn refresh_pins(&self) {
        for tab in self.tabs.borrow().iter() {
            for p in tab.panes.borrow().iter() {
                p.reload_pins();
            }
        }
        self.sidebar.set_pinned(&crate::pinstore::global());
    }

    fn all_panes(&self) -> Vec<Rc<Pane>> {
        self.tabs
            .borrow()
            .iter()
            .flat_map(|t| t.panes.borrow().clone())
            .collect()
    }

    fn on_pane_event(&self, pane: &Rc<Pane>, ev: PaneEvent) {
        // panes report while they are being built, before their tab exists
        if self.tabs.borrow().is_empty()
            || self
                .tabs
                .borrow()
                .iter()
                .all(|t| t.panes.try_borrow().is_err())
        {
            return;
        }
        let is_current = Rc::ptr_eq(pane, &self.pane());
        match ev {
            PaneEvent::Cursor => {
                if is_current {
                    self.update_status();
                    self.schedule_preview();
                }
            }
            PaneEvent::Dir => {
                self.update_chrome();
                if pane.timing.get().1.is_some() && std::env::var_os("VECTOR_TIMING").is_some() {
                    let (a, b) = pane.timing.get();
                    eprintln!(
                        "vector timing: {} entries, first paint {:.1} ms, full list {:.1} ms",
                        pane.model.borrow().total(),
                        a.unwrap_or(-1.0),
                        b.unwrap_or(-1.0)
                    );
                }
            }
            PaneEvent::Activate(e) => self.open_file(&e.path),
            PaneEvent::OpenTab(dir) => self.new_tab(&dir),
            PaneEvent::Menu(w, x, y) => {
                if !is_current {
                    self.focus_pane(pane);
                }
                self.show_menu(&w, x, y);
            }
            PaneEvent::Drop { paths, dest, copy } => {
                let sources: Vec<PathBuf> = paths
                    .into_iter()
                    .filter(|p| p.parent() != Some(dest.as_path()) || copy)
                    .collect();
                if !sources.is_empty() {
                    self.transfer(sources, dest, !copy);
                }
            }
            PaneEvent::Focus => {
                if !is_current {
                    self.focus_pane(pane);
                }
            }
            // a locked folder explains itself in the list; no red line for it
            PaneEvent::Error(e) if crate::pane::is_permission(&e) => {}
            PaneEvent::Error(e) => self.flash_error(&e),
            PaneEvent::SortBy(key) => {
                let cur = pane.model.borrow().options().sort;
                let reverse = cur.key == key && !cur.reverse;
                self.run_command(Command::Sort { key, reverse });
            }
        }
    }

    fn focus_pane(&self, pane: &Rc<Pane>) {
        let tab = self.tab();
        let i = tab.panes.borrow().iter().position(|p| Rc::ptr_eq(p, pane));
        if let Some(i) = i {
            tab.active.set(i);
            self.update_panes_focus();
            self.update_chrome();
        }
    }

    // ── chrome: path bar, tab labels, statusline ────────────────────────────

    fn update_chrome(&self) {
        let pane = self.pane();
        let dir = pane.dir();
        self.sidebar.set_current(&dir);
        publish_cwd(&dir);
        if *self.script_dir.borrow() != dir {
            *self.script_dir.borrow_mut() = dir.clone();
            self.visit(&dir);
            // a handler that moves on (vector.cd) doesn't hear about its own move: no loops
            if self.hook_target.take().as_ref() != Some(&dir) {
                self.in_cd_hook.set(true);
                self.with_script(|s, h| s.changed_dir(h));
                self.in_cd_hook.set(false);
            }
        }
        #[cfg(feature = "terminal")]
        self.term.follow(&dir);
        // a picker keeps the requesting app's title ("Upload a photo")
        if self.pick.borrow().is_none() {
            self.window.set_title(Some(&format!(
                "{} — {}",
                display_path(&dir),
                vector_core::APP_NAME
            )));
        }
        while let Some(c) = self.crumbs.first_child() {
            self.crumbs.remove(&c);
        }
        // the full path, every part clickable, like nvim's winbar: home (or the root) first,
        // the folder itself last and bold
        let home = glib::home_dir();
        let (mut acc, first, rest): (PathBuf, String, Vec<String>) = match dir.strip_prefix(&home) {
            Ok(below) => (
                home.clone(),
                "\u{F02DC} ~".into(),
                below
                    .iter()
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
            ),
            Err(_) => (
                PathBuf::from("/"),
                "\u{F02CA}".into(),
                dir.iter()
                    .skip(1)
                    .map(|s| s.to_string_lossy().into_owned())
                    .collect(),
            ),
        };
        let parts: Vec<String> = std::iter::once(first).chain(rest).collect();
        let n = parts.len();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                acc.push(&part);
                let sep = gtk::Label::new(Some("\u{203A}"));
                sep.add_css_class("crumb-sep");
                self.crumbs.append(&sep);
            }
            let b = gtk::Button::with_label(&if i == n - 1 && i > 0 {
                format!("\u{F0770} {part}")
            } else {
                part
            });
            b.add_css_class("link");
            b.add_css_class("crumb");
            if i == n - 1 {
                b.add_css_class("primary");
            }
            let target = acc.clone();
            let weak = self.weak_self.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.pane().open_dir(&target);
                    win.pane().focus_view();
                }
            });
            self.crumbs.append(&b);
        }
        let tabs = self.tabs.borrow();
        self.tab_bar.set_visible(tabs.len() > 1);
        for (i, t) in tabs.iter().enumerate() {
            let p = t.panes.borrow()[t.active.get().min(t.panes.borrow().len() - 1)].clone();
            let d = p.dir();
            let name = d
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into());
            // two tabs with the same folder name: add the parent
            let same = tabs
                .iter()
                .filter(|o| {
                    let od = o.panes.borrow()[o.active.get().min(o.panes.borrow().len() - 1)].dir();
                    od.file_name() == d.file_name() && od != d
                })
                .count();
            let label = match (same, d.parent().and_then(|p| p.file_name())) {
                (n, Some(parent)) if n > 0 => format!("{}/{name}", parent.to_string_lossy()),
                _ => name,
            };
            t.button.set_label(&format!("{} \u{F024B} {label}", i + 1));
            t.button.set_tooltip_text(Some(&display_path(&d)));
            if i == self.current.get() {
                t.button.add_css_class("primary");
                t.item.add_css_class("active");
            } else {
                t.button.remove_css_class("primary");
                t.item.remove_css_class("active");
            }
        }
        drop(tabs);
        self.update_status();
        self.schedule_preview();
    }

    fn update_status(&self) {
        let mode = self.mode.get();
        let picker_title = if self.renaming.get() {
            Some("RENAME")
        } else if self.side_cursor.get().is_some() {
            Some("SIDEBAR")
        } else {
            self.picker.borrow().as_ref().map(|p| p.title)
        };
        self.mode_label
            .set_text(picker_title.unwrap_or(mode.label()));
        let colour = match (picker_title, mode) {
            (Some("RENAME"), _) => "m-rename",
            (Some("SIDEBAR"), _) => "m-sidebar",
            (Some(_), _) | (None, Mode::Filter) => "m-filter",
            (None, Mode::Normal) => "m-normal",
            (None, Mode::Visual) => "m-visual",
            (None, Mode::Command) => "m-command",
        };
        self.bar.set_mode(colour);
        self.pending_label
            .set_text(&self.keymap.borrow().pending_text());
        let pane = self.pane();
        let m = pane.model.borrow();
        let sel = pane.selection.borrow();
        let mut parts = Vec::new();
        if pane.loading.get() {
            parts.push(format!("loading {}", m.total()));
        } else {
            let hidden = m.total().saturating_sub(m.len());
            parts.push(match m.len() {
                1 => "1 item".to_string(),
                n => format!("{n} items"),
            });
            if hidden > 0 && m.options().filter.is_empty() && !m.options().show_hidden {
                parts.push(format!("{hidden} hidden"));
            }
        }
        let nsel = sel.selected_names().count();
        self.bar.pos.set_text(&if nsel > 0 {
            format!("{nsel} selected")
        } else if let (Some(i), false) = (sel.cursor(&m), m.is_empty()) {
            format!("{}/{}", i + 1, m.len())
        } else {
            "0/0".into()
        });
        self.bar.set_branch(pane.branch.borrow().as_deref());
        if let Some(n) = pane.progress.get() {
            parts.push(format!(
                "scanning {} files",
                vector_core::details::group_digits(n)
            ));
        }
        if pane.is_biggest() {
            parts.push("biggest files".into());
        } else if pane.is_flat() {
            parts.push("flat".into());
        }
        if !m.options().filter.is_empty() {
            parts.push(format!("filter {}", m.options().filter));
        }
        let s = m.options().sort;
        let down = s.reverse
            ^ matches!(
                s.key,
                vector_core::sort::SortKey::Size
                    | vector_core::sort::SortKey::Modified
                    | vector_core::sort::SortKey::Added
            );
        let key = if s.key == vector_core::sort::SortKey::Added
            && vector_core::trash::is_trash(&pane.dir())
        {
            "trashed"
        } else {
            s.key.label()
        };
        self.bar
            .sort
            .set_text(&format!("{key} {}", if down { "↓" } else { "↑" }));
        if let Some(free) = pane.fs_info.get().0 {
            parts.push(format!("{} free", crate::fmt::size(free)));
        }
        if let Some(r) = self.register.borrow().as_ref() {
            parts.push(format!(
                "{} {}",
                if r.cut { "cut" } else { "yanked" },
                r.paths.len()
            ));
        }
        self.info.set_text(&parts.join(" · "));
    }

    fn flash(&self, text: &str) {
        // every message goes to the toast, where it is seen (and goes away by itself)
        self.show_notice(text, false);
    }

    fn flash_error(&self, text: &str) {
        self.show_notice(text, true);
    }

    fn schedule_preview(&self) {
        if !self.preview_box.is_visible() {
            return;
        }
        if let Some(id) = self.preview_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak_self.borrow().clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(60), move || {
            if let Some(win) = weak.upgrade() {
                win.preview_timer.borrow_mut().take();
                let e = win.pane().cursor_entry();
                win.preview.show(e.as_ref());
            }
        });
        *self.preview_timer.borrow_mut() = Some(id);
    }

    /// Remembers the current pane's view (and the preview) for the next start.
    fn save_settings(&self) {
        let pane = self.pane();
        let o = pane.model.borrow().options().clone();
        let s = crate::settings::Settings {
            preview: self.preview_box.is_visible(),
            grid: pane.is_grid(),
            show_hidden: o.show_hidden,
            // (the trash's own newest-first order isn't remembered as the general one)
            sort: pane.remembered_sort(),
            #[cfg(feature = "terminal")]
            terminal: self.term.is_open(),
            #[cfg(not(feature = "terminal"))]
            terminal: false,
            numbers: crate::pane::numbers(),
        };
        if *self.settings.borrow() != s {
            s.save();
            *self.settings.borrow_mut() = s;
        }
    }

    fn toggle_preview(&self) {
        let show = !self.preview_box.is_visible();
        self.preview_box.set_visible(show);
        self.save_settings();
        if show {
            self.schedule_preview();
        } else {
            self.preview.show(None);
        }
    }

    // ── keys ────────────────────────────────────────────────────────────────

    fn connect_keys(&self) {
        let ctl = gtk::EventControllerKey::new();
        ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak_self.borrow().clone();
        ctl.connect_key_pressed(move |_, keyval, _, state| {
            let Some(win) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            // the terminal panel gets every key except the one that toggles it
            #[cfg(feature = "terminal")]
            if win.term.has_focus() {
                let toggle = keyval == gdk::Key::F4
                    || (keyval == gdk::Key::grave
                        && state.contains(gdk::ModifierType::CONTROL_MASK));
                if toggle {
                    win.run_action(Action::ToggleTerminal, 1);
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            // picker: ctrl+enter accepts (open the selection, save, or pick this folder)
            if win.pick.borrow().is_some()
                && matches!(keyval, gdk::Key::Return | gdk::Key::KP_Enter)
                && state.contains(gdk::ModifierType::CONTROL_MASK)
            {
                win.accept_pick();
                return glib::Propagation::Stop;
            }
            // typing in a text field (the save name): the field gets the keys, Esc leaves it
            if win.dialog.borrow().is_none() && win.editing_text() {
                if keyval == gdk::Key::Escape {
                    win.pane().focus_view();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            match to_key(keyval, state) {
                Some(k) if win.handle_key(k, false) => glib::Propagation::Stop,
                _ => glib::Propagation::Proceed,
            }
        });
        self.window.add_controller(ctl);
    }

    /// Whether keyboard focus is in a text field other than the command line.
    fn editing_text(&self) -> bool {
        let Some(focus) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return false;
        };
        let in_cmd = focus.is_ancestor(&self.cmd_entry)
            || focus == self.cmd_entry.clone().upcast::<gtk::Widget>();
        !in_cmd && (focus.is::<gtk::Text>() || focus.is::<gtk::Entry>())
    }

    /// Handles one key. `synthetic` keys (from `--keys`) also type into the command line.
    /// Returns whether the key was used.
    pub fn handle_key(&self, key: Key, synthetic: bool) -> bool {
        if self.dialog.borrow().is_some() {
            return self.dialog_key(key, synthetic);
        }
        if self.side_cursor.get().is_some() && self.mode.get() == Mode::Normal {
            return self.sidebar_key(key);
        }
        if self.picker.borrow().is_some()
            && !matches!(self.mode.get(), Mode::Command | Mode::Filter)
        {
            return self.picker_key(key);
        }
        match self.mode.get() {
            Mode::Command | Mode::Filter => self.line_key(key, synthetic),
            Mode::Normal | Mode::Visual => {
                let feed = self.keymap.borrow_mut().feed(key);
                let used = match feed {
                    Feed::Run { action, count } => {
                        self.run_action(action, count);
                        true
                    }
                    Feed::Pending => true,
                    Feed::Unbound => false,
                };
                self.update_status();
                used
            }
        }
    }

    fn line_key(&self, key: Key, synthetic: bool) -> bool {
        match key {
            Key::Esc => {
                if self.mode.get() == Mode::Filter {
                    self.pane().model.borrow_mut().set_filter("");
                    self.pane().refresh();
                }
                self.leave_line();
                true
            }
            Key::Enter if self.mode.get() == Mode::Filter => {
                let text = self.cmd_entry.text().to_string();
                let nothing_here = self.pane().model.borrow().is_empty();
                self.leave_line();
                // `//name`: every folder below; a filter matching nothing here: look below too
                if let Some(q) = text.strip_prefix('/') {
                    self.find_below(q.trim());
                } else if nothing_here && !text.trim().is_empty() {
                    self.pane().model.borrow_mut().set_filter("");
                    self.pane().refresh();
                    self.find_below(text.trim());
                }
                true
            }
            Key::Enter => {
                let text = self.cmd_entry.text().to_string();
                let was = self.mode.get();
                self.leave_line();
                let line = text.trim();
                let (name, args) = line.split_once(' ').unwrap_or((line, ""));
                if was == Mode::Command && self.script.has_command(name) {
                    let args = args.trim().to_string();
                    self.with_script(|s, h| s.run_command(name, &args, h));
                } else if was == Mode::Command {
                    match command::parse(&text) {
                        Ok(c) => self.run_command(c),
                        Err(command::ParseError::Empty) => {}
                        Err(e) => self.flash_error(&e.to_string()),
                    }
                }
                true
            }
            Key::Tab | Key::BackTab if self.mode.get() == Mode::Command => {
                self.complete(key == Key::BackTab);
                true
            }
            Key::Backspace if self.cmd_entry.text().is_empty() => {
                self.line_key(Key::Esc, synthetic)
            }
            Key::Char(c) if synthetic => {
                let mut t = self.cmd_entry.text().to_string();
                t.push(c);
                self.cmd_entry.set_text(&t);
                self.cmd_entry.set_position(-1);
                true
            }
            Key::Backspace if synthetic => {
                let mut t = self.cmd_entry.text().to_string();
                t.pop();
                self.cmd_entry.set_text(&t);
                true
            }
            _ => false,
        }
    }

    fn enter_line(&self, mode: Mode) {
        self.mode.set(mode);
        self.prompt
            .set_text(if mode == Mode::Filter { "/" } else { ":" });
        let current = if mode == Mode::Filter {
            self.pane().model.borrow().options().filter.clone()
        } else {
            String::new()
        };
        self.cmd_entry.set_text(&current);
        self.cmd_box.set_visible(true);
        self.cmd_entry.grab_focus();
        self.cmd_entry.set_position(-1);
        self.close_picker();
        self.update_status();
    }

    fn leave_line(&self) {
        self.mode.set(Mode::Normal);
        self.cmd_box.set_visible(false);
        self.clear_list();
        self.completions.borrow_mut().0.clear();
        self.pane().focus_view();
        self.update_status();
    }

    fn connect_entries(&self) {
        // clicking the path bar (not a crumb) types a path
        let click = gtk::GestureClick::new();
        let weak = self.weak_self.borrow().clone();
        click.connect_released(move |g, _, _, _| {
            if let Some(win) = weak.upgrade() {
                g.set_state(gtk::EventSequenceState::Claimed);
                win.type_path();
            }
        });
        self.crumbs.add_controller(click);

        let weak = self.weak_self.borrow().clone();
        self.cmd_entry.connect_changed(move |e| {
            let Some(win) = weak.upgrade() else { return };
            if win.setting_line.get() {
                return;
            }
            match win.mode.get() {
                Mode::Filter => {
                    let pane = win.pane();
                    let text = e.text();
                    // `//`: a search below this folder (on enter), not a filter of it
                    if let Some(rest) = text.strip_prefix('/') {
                        pane.model.borrow_mut().set_filter("");
                        pane.refresh();
                        if rest.is_empty() {
                            win.flash(
                                "// finds in every folder below this one · type a name, enter",
                            );
                        }
                        return;
                    }
                    pane.model.borrow_mut().set_filter(&text);
                    pane.refresh();
                }
                Mode::Command => {
                    // typing invalidates the completion cycle; show fresh candidates
                    let fresh = win.completions.borrow().2 != e.text().len();
                    if fresh {
                        win.completions.borrow_mut().0.clear();
                        win.show_completions_for(&e.text(), false);
                    }
                }
                _ => {}
            }
        });
    }

    fn clear_list(&self) {
        while let Some(c) = self.list_box.first_child() {
            self.list_box.remove(&c);
        }
        self.list_box.set_visible(false);
    }

    fn fill_list(&self, rows: &[String], active: Option<usize>) {
        self.clear_list();
        let max = 10;
        let start = active.map_or(0, |a| a.saturating_sub(max - 1));
        for (i, text) in rows.iter().enumerate().skip(start).take(max) {
            let l = gtk::Label::new(Some(text));
            l.set_xalign(0.0);
            l.add_css_class("row");
            l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            if Some(i) == active {
                l.add_css_class("active");
            }
            self.list_box.append(&l);
        }
        if rows.len() > max {
            let more = gtk::Label::new(Some(&format!(
                "{} of {}",
                active.map_or(0, |a| a + 1),
                rows.len()
            )));
            more.add_css_class("section");
            more.set_xalign(0.0);
            self.list_box.append(&more);
        }
        self.list_box.set_visible(!rows.is_empty());
    }

    /// Lists completions for `line` off the main thread (they may read folders), then shows
    /// them; with `apply`, also puts the first/next one into the line.
    fn show_completions_for(&self, line: &str, apply: bool) {
        let cwd = self.pane().dir();
        let line_owned = line.to_string();
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let l = line_owned.clone();
            let Ok(c) = gio::spawn_blocking(move || {
                let home = glib::home_dir();
                let lister = |p: &Path| -> Vec<(String, bool)> {
                    std::fs::read_dir(p)
                        .map(|rd| {
                            rd.flatten()
                                .take(5000)
                                .map(|e| {
                                    let dir = std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir());
                                    (e.file_name().to_string_lossy().into_owned(), dir)
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                };
                command::complete(
                    &l,
                    &command::CompletionCtx {
                        cwd: &cwd,
                        home: &home,
                        list_dir: &lister,
                    },
                )
            })
            .await
            else {
                return;
            };
            let Some(win) = weak.upgrade() else { return };
            if win.cmd_entry.text() != line_owned {
                return;
            }
            let prefix = line_owned[..c.start].to_string();
            let full: Vec<String> = c
                .candidates
                .iter()
                .map(|x| format!("{prefix}{x}"))
                .collect();
            *win.completions.borrow_mut() = (full.clone(), usize::MAX, line_owned.len());
            win.fill_list(&c.candidates, None);
            if apply && !full.is_empty() {
                win.cycle_completion(false);
            }
        });
    }

    fn complete(&self, back: bool) {
        if self.completions.borrow().0.is_empty() {
            self.show_completions_for(&self.cmd_entry.text(), true);
        } else {
            self.cycle_completion(back);
        }
    }

    fn cycle_completion(&self, back: bool) {
        let (text, shown) = {
            let mut c = self.completions.borrow_mut();
            let n = c.0.len();
            if n == 0 {
                return;
            }
            c.1 = match (c.1, back) {
                (usize::MAX, false) => 0,
                (usize::MAX, true) => n - 1,
                (i, false) => (i + 1) % n,
                (i, true) => (i + n - 1) % n,
            };
            c.2 = c.0[c.1].len();
            (c.0[c.1].clone(), c.1)
        };
        self.setting_line.set(true);
        self.cmd_entry.set_text(&text);
        self.setting_line.set(false);
        self.cmd_entry.set_position(-1);
        let rows: Vec<String> = {
            let c = self.completions.borrow();
            let start = text.len() - text.rsplit(' ').next().map_or(0, str::len);
            c.0.iter()
                .map(|x| x.get(start..).unwrap_or(x).to_string())
                .collect()
        };
        self.fill_list(&rows, Some(shown));
    }

    // ── actions ─────────────────────────────────────────────────────────────

    fn run_action(&self, action: Action, count: usize) {
        let pane = self.pane();
        let rows = |d: isize| d * count as isize;
        let mv = |delta: isize| {
            pane.selection
                .borrow_mut()
                .move_by(&pane.model.borrow(), delta);
            pane.push_selection(true);
            self.update_status();
            self.schedule_preview();
        };
        match action {
            Action::Down => mv(rows(pane.columns())),
            Action::Up => mv(rows(-pane.columns())),
            Action::PageDown => mv(rows(pane.page_rows())),
            Action::PageUp => mv(rows(-pane.page_rows())),
            Action::Top => {
                pane.selection
                    .borrow_mut()
                    .set_cursor(&pane.model.borrow(), 0);
                mv(0);
            }
            Action::Bottom => {
                let n = pane.model.borrow().len();
                let i = if count == 0 {
                    n.saturating_sub(1)
                } else {
                    count - 1
                };
                pane.selection
                    .borrow_mut()
                    .set_cursor(&pane.model.borrow(), i);
                mv(0);
            }
            Action::Open => {
                if let Some(e) = pane.cursor_entry() {
                    if e.is_dir() {
                        self.leave_visual();
                        pane.open_dir(&e.path);
                    } else {
                        self.open_file(&e.path);
                    }
                }
            }
            Action::Parent => {
                self.leave_visual();
                // at the top of the tree, h goes on into the sidebar
                if pane.dir().parent().is_none() && !pane.is_flat() {
                    self.enter_sidebar();
                } else {
                    pane.go_parent();
                }
            }
            // the grid moves sideways; the list goes up and into folders
            Action::Left if pane.is_grid() => mv(rows(-1)),
            Action::Right if pane.is_grid() => mv(rows(1)),
            Action::Left => self.run_action(Action::Parent, 1),
            Action::Right => self.run_action(Action::Open, 1),
            Action::FocusSidebar => {
                self.leave_visual();
                self.enter_sidebar();
            }
            Action::Back => pane.go_back(),
            Action::Forward => pane.go_forward(),
            Action::Yank | Action::Cut => {
                let paths: Vec<PathBuf> = pane.targets().into_iter().map(|e| e.path).collect();
                if paths.is_empty() {
                    return;
                }
                // an archive is read-only: taking things out of it is always a copy
                let cut = action == Action::Cut && !pane.in_archive.get();
                self.notify(&format!(
                    "{} {} · p or ctrl+v pastes",
                    if cut { "cut" } else { "copied" },
                    count_label(&paths)
                ));
                // paths inside an archive mean nothing to other apps: keep them in vector
                if !pane.in_archive.get() {
                    self.set_file_clipboard(&paths, cut);
                }
                *self.register.borrow_mut() = Some(Register { paths, cut });
                self.update_marks();
                self.leave_visual();
                pane.selection.borrow_mut().clear();
                pane.push_selection(false);
            }
            Action::Paste
            | Action::Trash
            | Action::DeletePermanently
            | Action::Rename
            | Action::Edit
                if self.read_only() => {}
            Action::Paste => {
                self.last_change.set(Some(action));
                self.paste();
            }
            Action::Restore => self.run_command(Command::Restore),
            Action::Place(n) => match self.sidebar.place(n as usize) {
                Some(p) => self.run_command(Command::Cd(p.display().to_string())),
                None => self.flash(&format!("no place {n} · b adds this folder to places")),
            },
            Action::Bookmark => {
                self.sidebar.toggle_bookmark(&self.pane().dir());
                self.refresh_recent();
            }
            Action::Trash if vector_core::trash::is_trash(&pane.dir()) => {
                self.flash("already in the trash · gr puts it back, D deletes it for good")
            }
            Action::BatchRename if self.read_only() => {}
            Action::BatchRename => {
                self.enter_line(Mode::Command);
                self.cmd_entry.set_text("rename ");
                self.cmd_entry.set_position(-1);
            }
            Action::Extract | Action::ExtractReplace => {
                self.extract_archives(action == Action::ExtractReplace)
            }
            Action::Trash => {
                self.last_change.set(Some(action));
                let paths: Vec<PathBuf> = pane.targets().into_iter().map(|e| e.path).collect();
                self.leave_visual();
                if !paths.is_empty() {
                    self.ops.submit(Op::Trash { paths });
                }
            }
            Action::DeletePermanently => {
                let paths: Vec<PathBuf> = pane.targets().into_iter().map(|e| e.path).collect();
                self.leave_visual();
                self.confirm_delete(paths, "delete permanently");
            }
            Action::Rename => self.rename_dialog(),
            Action::Undo => match self.ops.undo_label() {
                Some(label) => {
                    self.ops.undo();
                    self.flash(&format!("undo {label}"));
                }
                None => self.flash("nothing to undo"),
            },
            Action::Repeat => {
                if let Some(a) = self.last_change.get() {
                    self.run_action(a, 1);
                }
            }
            Action::VisualStart => {
                if self.mode.get() == Mode::Visual {
                    self.leave_visual();
                } else {
                    self.mode.set(Mode::Visual);
                    pane.selection
                        .borrow_mut()
                        .start_visual(&pane.model.borrow());
                    pane.push_selection(false);
                }
                self.update_status();
            }
            Action::Escape => {
                let nothing_to_clear = self.mode.get() == Mode::Normal
                    && !pane.selection.borrow().has_selection()
                    && pane.model.borrow().options().filter.is_empty();
                if nothing_to_clear && self.pick.borrow().is_some() {
                    self.finish_pick(None);
                    return;
                }
                self.leave_visual();
                pane.selection.borrow_mut().clear();
                if !pane.model.borrow().options().filter.is_empty() {
                    pane.model.borrow_mut().set_filter("");
                    pane.refresh();
                }
                pane.push_selection(false);
                self.message.set_text("");
                self.update_status();
            }
            Action::CommandStart => self.enter_line(Mode::Command),
            Action::FilterStart => self.enter_line(Mode::Filter),
            Action::ToggleHidden => self.run_command(Command::Hidden(Toggle::Toggle)),
            Action::ToggleView => self.run_command(Command::View(None)),
            Action::TogglePreview => self.toggle_preview(),
            Action::ToggleSplit => self.toggle_split(),
            Action::OtherPane => self.other_pane(),
            Action::NewTab => self.new_tab(&pane.dir()),
            Action::CloseTab => self.close_tab(),
            Action::NextTab => {
                let i = self.current.get() + count;
                self.switch_tab(i);
            }
            Action::PrevTab => {
                let n = self.tabs.borrow().len();
                let i = (self.current.get() + n * count - count) % n.max(1);
                self.switch_tab(i);
            }
            Action::ToggleSelect => {
                if let Some(e) = pane.cursor_entry() {
                    pane.selection.borrow_mut().toggle(&e.name);
                    pane.selection.borrow_mut().move_by(&pane.model.borrow(), 1);
                    pane.push_selection(true);
                    self.update_status();
                }
            }
            Action::SelectAll => {
                let names: Vec<String> =
                    pane.model.borrow().iter().map(|e| e.name.clone()).collect();
                pane.selection.borrow_mut().set_selected(names);
                pane.push_selection(false);
                self.update_status();
            }
            Action::FolderSize => self.folder_sizes(),
            Action::Edit => self.run_command(Command::Edit(None)),
            Action::EditFile => match pane.cursor_entry() {
                // inside an archive: a read-only copy, in its viewer
                Some(e) if !e.is_dir() && pane.in_archive.get() => self.open_file(&e.path),
                Some(e) if !e.is_dir() => self.open_in_editor(&e.path),
                Some(_) => self.flash("e edits files; ge edits the names in this folder"),
                None => {}
            },
            Action::CopyPath => self.copy_path(true),
            Action::Terminal => self.run_command(Command::Term),
            Action::Reload => pane.reload(),
            Action::PathEdit => self.type_path(),
            Action::Help => self.show_help(),
            Action::ToggleTerminal => self.toggle_terminal(),
            Action::PinHere | Action::PinGlobal => {
                let items: Vec<PathBuf> = pane.targets().into_iter().map(|e| e.path).collect();
                if items.is_empty() {
                    return;
                }
                let global = action == Action::PinGlobal;
                let now = crate::pinstore::with(|p| {
                    items
                        .iter()
                        .map(|i| {
                            if global {
                                p.toggle_global(i)
                            } else {
                                p.toggle_folder(i)
                            }
                        })
                        .last()
                        .unwrap_or(false)
                });
                crate::pinstore::save();
                self.notify(&format!(
                    "{} {} {}",
                    if now { "pinned" } else { "unpinned" },
                    count_label(&items),
                    if global {
                        "everywhere"
                    } else {
                        "in this folder"
                    }
                ));
                self.refresh_pins();
            }
            Action::Menu => {
                let w = pane.widget().clone().upcast::<gtk::Widget>();
                let (x, y) = (w.width() as f64 / 3.0, w.height() as f64 / 4.0);
                self.show_menu(&w, x, y);
            }
            Action::User(i) => self.with_script(|s, h| s.run_mapping(usize::from(i), h)),
            Action::Quit => self.window.close(),
        }
    }

    // ── scripts (init.lua) ──────────────────────────────────────────────────

    fn load_scripts(&self) {
        let dir = glib::user_config_dir().join(vector_core::APP_NAME);
        self.script.set_host(self.host());
        let errors = self.script.load_dir(&dir);
        // the first folder was shown before the `cd` handlers existed
        self.in_cd_hook.set(true);
        self.with_script(|s, h| s.changed_dir(h));
        self.in_cd_hook.set(false);
        if !errors.is_empty() {
            self.notify_error(&errors.join("\n"));
        }
    }

    /// Puts mappings the scripts added (at load or later) into the keymap.
    fn bind_script_keys(&self) {
        let maps = self.script.mappings();
        let mut keymap = self.keymap.borrow_mut();
        for (i, keys) in maps.iter().enumerate().skip(self.script_keys.get()) {
            if let Ok(i) = u16::try_from(i) {
                keymap.bind(keys, Action::User(i));
            }
        }
        self.script_keys.set(maps.len());
    }

    /// What scripts see: the folder, the cursor and the selection.
    fn host(&self) -> Host {
        let pane = self.pane();
        Host {
            cwd: pane.dir(),
            cursor: pane.cursor_entry().map(|e| e.path),
            selection: pane.targets().into_iter().map(|e| e.path).collect(),
        }
    }

    /// Calls into the scripts, then does what they asked for.
    fn with_script(&self, f: impl FnOnce(&Script, Host) -> Result<(), String>) {
        if let Err(e) = f(&self.script, self.host()) {
            self.notify_error(&e);
        }
        self.apply_requests();
    }

    fn apply_requests(&self) {
        self.bind_script_keys();
        for r in self.script.take_requests() {
            match r {
                Request::Cd(p) => {
                    if self.in_cd_hook.get() {
                        *self.hook_target.borrow_mut() = Some(p.clone());
                    }
                    self.run_command(Command::Cd(p.display().to_string()))
                }
                Request::Open(p) => self.open_file(&p),
                Request::Command(line) => match command::parse(&line) {
                    Ok(c) => self.run_command(c),
                    Err(command::ParseError::Empty) => {}
                    Err(e) => self.flash_error(&e.to_string()),
                },
                Request::Notify(text) => self.notify(&text),
                Request::Shell { cmd, callback } => self.script_shell(&cmd, callback),
            }
        }
    }

    /// `vector.sh`: `sh -c cmd` in this folder with the selection as `$1…`, never blocking
    /// the window. Without a callback a failure shows its output.
    fn script_shell(&self, cmd: &str, callback: Option<vector_core::script::Callback>) {
        let host = self.host();
        let mut argv: Vec<&OsStr> = vec![OsStr::new("sh"), OsStr::new("-c"), OsStr::new(cmd)];
        argv.push(OsStr::new("sh"));
        argv.extend(host.selection.iter().map(|p| p.as_os_str()));
        let launcher = gio::SubprocessLauncher::new(
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
        );
        launcher.set_cwd(&host.cwd);
        let proc = match launcher.spawn(&argv) {
            Ok(p) => p,
            Err(e) => return self.notify_error(&format!("sh: {}", e.message())),
        };
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let out = proc.communicate_future(None).await;
            let Some(win) = weak.upgrade() else { return };
            let out = out
                .ok()
                .and_then(|(o, _)| o)
                .map(|o| String::from_utf8_lossy(&o).trim_end().to_string())
                .unwrap_or_default();
            let code = if proc.has_exited() {
                proc.exit_status()
            } else {
                -1
            };
            match callback {
                Some(cb) => win.with_script(|s, h| s.finish_shell(cb, &out, code, h)),
                None if code != 0 => {
                    let first = out.lines().next().unwrap_or("").to_string();
                    win.notify_error(&format!("sh exited with {code}: {first}"));
                }
                None => {}
            }
        });
    }

    /// F4: open and focus the terminal; again (from inside it): hide it.
    #[cfg(feature = "terminal")]
    fn toggle_terminal(&self) {
        if self.term.is_open() && self.term.has_focus() {
            self.term.close();
            self.pane().focus_view();
        } else {
            self.term.open(&self.pane().dir());
            self.term.focus();
        }
        self.save_settings();
    }

    #[cfg(not(feature = "terminal"))]
    fn toggle_terminal(&self) {
        self.notify("this build has no terminal panel (built without VTE)");
    }

    /// `:agent`: a coding agent in this folder, in the terminal panel when there is one.
    fn start_agent(&self, name: Option<String>) {
        let found = match name {
            Some(n) => glib::find_program_in_path(&n).map(|_| n),
            None => vector_core::command::AGENTS
                .iter()
                .find(|a| glib::find_program_in_path(a).is_some())
                .map(|a| a.to_string()),
        };
        let Some(agent) = found else {
            self.notify_error("no coding agent found (claude, codex, gemini, aider, opencode…)");
            return;
        };
        #[cfg(feature = "terminal")]
        {
            self.term.open(&self.pane().dir());
            self.term.focus();
            self.term.run(&agent);
            self.save_settings();
        }
        #[cfg(not(feature = "terminal"))]
        {
            let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
            launcher.set_cwd(&self.pane().dir());
            if let Some(argv) = crate::terminal::argv(&[OsStr::new(&agent)]) {
                let refs: Vec<&OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
                let _ = launcher.spawn(&refs);
            }
        }
    }

    fn enter_sidebar(&self) {
        let i = self.sidebar.current_index();
        self.side_cursor.set(Some(i));
        self.sidebar.set_cursor(Some(i));
        self.flash("sidebar · j k move · l opens · esc back");
        self.update_status();
    }

    fn leave_sidebar(&self) {
        self.side_cursor.set(None);
        self.sidebar.set_cursor(None);
        self.message.set_text("");
        self.pane().focus_view();
        self.update_status();
    }

    /// Keys while the sidebar has the keyboard cursor.
    fn sidebar_key(&self, key: Key) -> bool {
        let n = self.sidebar.len();
        let Some(i) = self.side_cursor.get() else {
            return false;
        };
        let go = |j: usize| {
            let j = j.min(n.saturating_sub(1));
            self.side_cursor.set(Some(j));
            self.sidebar.set_cursor(Some(j));
        };
        match key {
            Key::Char('j') | Key::Down => go(i + 1),
            Key::Char('k') | Key::Up => go(i.saturating_sub(1)),
            Key::Char('G') | Key::End => go(n),
            Key::Char('g') | Key::Home => go(0),
            // move one of the user's own places, the cursor going along
            Key::Char('K') | Key::Char('J') => {
                let up = key == Key::Char('K');
                if let Some((p, _)) = self.sidebar.cursor_row(i)
                    && self.sidebar.shift_place(&p, up)
                {
                    go(if up { i.saturating_sub(1) } else { i + 1 });
                }
            }
            Key::Char('l') | Key::Enter | Key::Right => {
                let target = self.sidebar.set_cursor(Some(i));
                self.leave_sidebar();
                if let Some(p) = target {
                    self.run_command(Command::Cd(p.display().to_string()));
                }
            }
            Key::Esc
            | Key::Tab
            | Key::Char('h')
            | Key::Left
            | Key::Ctrl('l')
            | Key::CtrlRight
            | Key::Ctrl('h')
            | Key::CtrlLeft => self.leave_sidebar(),
            // everything else (":" and friends) works as usual once back in the list
            _ => {
                self.leave_sidebar();
                return self.handle_key(key, false);
            }
        }
        true
    }

    /// Counts a visit to `dir` for the recent list and `:z`.
    fn visit(&self, dir: &Path) {
        if dir == Path::new("/")
            || *dir == glib::home_dir()
            || vector_core::trash::is_trash(dir)
            || dir.starts_with(glib::user_cache_dir())
        {
            return;
        }
        self.frecency.borrow_mut().visit(dir, now());
        self.save_frecency();
        self.refresh_recent();
    }

    fn save_frecency(&self) {
        let text = self.frecency.borrow().to_text();
        // off the main thread: the state folder could be anywhere
        gio::spawn_blocking(move || {
            let p = frecency_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, text);
        });
    }

    /// The five folders used most that aren't places already.
    fn refresh_recent(&self) {
        let top = self.frecency.borrow().top(5, now(), |p| {
            self.sidebar.is_place(p) || vector_core::trash::is_trash(p)
        });
        self.sidebar.set_recent(&top);
        let cur = self.pane().dir();
        self.sidebar.set_current(&cur);
    }

    fn leave_visual(&self) {
        if self.mode.get() == Mode::Visual {
            self.mode.set(Mode::Normal);
            self.pane().selection.borrow_mut().end_visual();
            self.update_status();
        }
    }

    fn run_command(&self, c: Command) {
        let pane = self.pane();
        let dir = pane.dir();
        match c {
            Command::New(_) | Command::Mkdir(_) | Command::Edit(_) if self.read_only() => {}
            Command::Extract { replace } => self.extract_archives(replace),
            Command::Restore => self.restore_from_trash(),
            Command::EmptyTrash => self.empty_trash(),
            Command::BatchRename(_) if self.read_only() => {}
            Command::BatchRename(pattern) => self.batch_rename(&pattern),
            Command::New(name) => {
                self.ops.submit(Op::NewFile {
                    path: dir.join(name),
                });
            }
            Command::Mkdir(name) => {
                self.ops.submit(Op::Mkdir {
                    path: dir.join(name),
                });
            }
            Command::Cd(p) => {
                let target = resolve(&dir, &p);
                let t = target.clone();
                let weak = self.weak_self.borrow().clone();
                glib::spawn_future_local(async move {
                    let kind =
                        gio::spawn_blocking(move || std::fs::metadata(&t).map(|m| m.is_dir()).ok())
                            .await
                            .ok()
                            .flatten();
                    let Some(win) = weak.upgrade() else { return };
                    // a bare name that isn't here: a place, drive or bookmark by that name
                    let place = (kind.is_none() && !p.contains('/'))
                        .then(|| win.sidebar.find(&p))
                        .flatten();
                    if let Some(place) = place {
                        win.pane().open_dir(&place);
                        win.pane().focus_view();
                        return;
                    }
                    // then a folder visited before, like :z
                    if kind.is_none() && !p.contains('/') {
                        win.run_command(Command::Z(p.clone()));
                        return;
                    }
                    match (kind, target.parent()) {
                        (Some(true), _) => {
                            win.pane().open_dir(&target);
                        }
                        // a file: open its folder with the file selected
                        (Some(false), Some(parent)) => {
                            win.pane().open_dir(parent);
                            win.pane().select_paths(std::slice::from_ref(&target));
                        }
                        _ => win.flash_error(&format!(
                            "no such folder or file: {} · //{} searches below this folder",
                            display_path(&target),
                            p.trim_start_matches("./")
                        )),
                    }
                    win.pane().focus_view();
                });
            }
            Command::Sort { key, reverse } => {
                let spec = SortSpec {
                    key,
                    reverse,
                    ..SortSpec::default()
                };
                let date_was_added =
                    pane.model.borrow().options().sort.key == vector_core::sort::SortKey::Added;
                pane.model.borrow_mut().set_sort(spec);
                // the date column switches between modified and added: rows already drawn
                // must be drawn again, not just moved
                if date_was_added != (key == vector_core::sort::SortKey::Added) {
                    pane.rebind();
                } else {
                    pane.refresh();
                }
                pane.sort_changed();
                self.save_settings();
            }
            Command::Hidden(t) => {
                let cur = pane.model.borrow().options().show_hidden;
                let show = match t {
                    Toggle::Toggle => !cur,
                    Toggle::On => true,
                    Toggle::Off => false,
                };
                pane.model.borrow_mut().set_show_hidden(show);
                pane.refresh();
                self.save_settings();
                self.flash(if show {
                    "showing hidden files"
                } else {
                    "hiding hidden files"
                });
            }
            Command::Z(q) => {
                let f = self.frecency.borrow().clone();
                let weak = self.weak_self.borrow().clone();
                glib::spawn_future_local(async move {
                    let query = q.clone();
                    let found = gio::spawn_blocking(move || f.find(&query, now(), |p| p.is_dir()))
                        .await
                        .ok()
                        .flatten();
                    let Some(win) = weak.upgrade() else { return };
                    match found {
                        Some(d) => {
                            win.pane().open_dir(&d);
                            win.pane().focus_view();
                        }
                        None => win.flash_error(&format!(
                            "no folder visited before matches \"{q}\" · //{q} searches below this folder"
                        )),
                    }
                });
            }
            Command::PlacesReset => {
                self.sidebar.reset_hidden();
                self.refresh_recent();
            }
            Command::Number(t) => {
                let on = match t {
                    Toggle::Toggle => !crate::pane::numbers(),
                    Toggle::On => true,
                    Toggle::Off => false,
                };
                crate::pane::set_numbers(on);
                for tab in self.tabs.borrow().iter() {
                    for p in tab.panes.borrow().iter() {
                        p.numbers_changed();
                    }
                }
                self.save_settings();
            }
            Command::View(grid) => {
                let grid = grid.unwrap_or(!pane.is_grid());
                if grid != pane.is_grid() {
                    pane.set_grid(grid);
                    self.save_settings();
                }
                pane.focus_view();
            }
            Command::Open(None) => self.run_action(Action::Open, 1),
            Command::Open(Some(name)) => {
                let p = resolve(&dir, &name);
                self.open_file(&p);
            }
            Command::Term => self.open_terminal(&dir),
            Command::Edit(None) => self.start_edit(),
            Command::Edit(Some(name)) => {
                let p = resolve(&dir, &name);
                self.open_in_editor(&p);
            }
            Command::Quit => self.quit_one(),
            Command::QuitAll => self.window.close(),
            Command::Find(q) => self.search(q, false),
            Command::Agent(name) => self.start_agent(name),
            Command::Filter(f) => self.set_filter(&f),
            Command::Biggest { n, scope } => {
                use vector_core::command::Scope;
                let (roots, one_fs): (Vec<PathBuf>, bool) = match scope {
                    // below this folder, whatever drives are mounted inside it
                    Scope::Here => (vec![dir.clone()], false),
                    Scope::System => (vec![PathBuf::from("/")], true),
                    Scope::All => {
                        let mut roots = vec![PathBuf::from("/")];
                        for m in gio::VolumeMonitor::get().mounts() {
                            if let Some(p) = m.root().path()
                                && !roots.contains(&p)
                            {
                                roots.push(p);
                            }
                        }
                        (roots, true)
                    }
                };
                self.notify(&format!("finding the {n} biggest files…"));
                pane.show_biggest(roots, n, one_fs);
            }
            Command::Flat => {
                pane.toggle_flat();
                self.flash(if pane.is_flat() {
                    "flat: every file below this folder"
                } else {
                    "flat off"
                });
            }
            Command::Grep(q) => self.search(q, true),
        }
    }

    fn open_file(&self, path: &Path) {
        if path.is_dir() {
            self.pane().open_dir(path);
            return;
        }
        if self.pick_file(path) {
            return;
        }
        // an archive opens like a folder; nothing in it is extracted or run
        if vector_core::archive::is_archive_name(path) && path.is_file() {
            self.pane().open_dir(path);
            return;
        }
        if let Some((archive, inner)) = vector_core::archive::split(path)
            && !inner.as_os_str().is_empty()
        {
            self.open_from_archive(archive, inner);
            return;
        }
        let ct = self.content_type_of(path);
        if ct.is_empty() || gio::content_type_is_unknown(&ct) {
            // the name says nothing (no extension): look at the contents first
            let weak = self.weak_self.borrow().clone();
            let p = path.to_path_buf();
            glib::spawn_future_local(async move {
                let (q, t) = (p.clone(), ct.clone());
                let sniffed = gio::spawn_blocking(move || vector_core::entry::sniffed_type(&q, &t))
                    .await
                    .unwrap_or(ct);
                if let Some(win) = weak.upgrade() {
                    win.open_typed(&p, sniffed);
                }
            });
            return;
        }
        self.open_typed(path, ct);
    }

    /// Opens a file of a known type with its default app, or asks which app to use.
    fn open_typed(&self, path: &Path, ct: String) {
        let Some(app) = gio::AppInfo::default_for_type(&ct, false) else {
            // nothing is set up for this type: ask instead of failing quietly
            self.open_with_dialog(path.to_path_buf(), &ct);
            return;
        };
        if vector_core::no_launch() {
            self.flash(&format!(
                "test run: would open {} in {}",
                file_name(path),
                app.display_name()
            ));
            return;
        }
        let ctx = WidgetExt::display(&self.window).app_launch_context();
        let weak = self.weak_self.borrow().clone();
        let (path, ct) = (path.to_path_buf(), ct.to_string());
        let uri = gio::File::for_path(&path).uri();
        let name = app.display_name();
        app.launch_uris_async(&[&uri], Some(&ctx), gio::Cancellable::NONE, move |r| {
            if let (Err(e), Some(win)) = (r, weak.upgrade()) {
                // say why, then offer the other apps
                win.flash_error(&format!("{name}: {}", e.message()));
                win.open_with_dialog(path, &ct);
            }
        });
    }

    /// Inside an archive nothing can be changed; says so (and how to get the files out).
    fn read_only(&self) -> bool {
        let ro = self.pane().in_archive.get();
        if ro {
            self.flash("read-only: this is inside an archive · X extracts it, y and p copy out");
        }
        ro
    }

    /// Opens a file from inside an archive: a read-only copy of just that file, in a private
    /// folder, handed to its usual app.
    fn open_from_archive(&self, archive: PathBuf, inner: PathBuf) {
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let r =
                gio::spawn_blocking(move || vector_core::archive::extract_one(&archive, &inner))
                    .await;
            let Some(win) = weak.upgrade() else { return };
            match r {
                Ok(Ok(copy)) => win.open_file(&copy),
                Ok(Err(e)) => win.flash_error(&format!("can't open from the archive: {e}")),
                Err(_) => win.flash_error("can't open from the archive"),
            }
        });
    }

    /// Copies items from inside archives into `dest` (extracting just them).
    fn copy_out(&self, paths: Vec<PathBuf>, dest: PathBuf) {
        if vector_core::archive::split(&dest).is_some() {
            self.flash("read-only: this is inside an archive");
            return;
        }
        self.flash("copying out of the archive…");
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let d = dest.clone();
            let r = gio::spawn_blocking(move || -> std::io::Result<usize> {
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let mut n = 0;
                for p in &paths {
                    let Some((archive, inner)) = vector_core::archive::split(p) else {
                        continue;
                    };
                    let strip = inner.parent().unwrap_or(Path::new("")).to_path_buf();
                    n += vector_core::archive::extract(&archive, &inner, &strip, &d, &cancel)?
                        .written;
                }
                Ok(n)
            })
            .await;
            let Some(win) = weak.upgrade() else { return };
            match r {
                Ok(Ok(n)) => {
                    win.notify(&format!("copied {n} file{} out of the archive", plural(n)));
                    win.pane().reload();
                }
                Ok(Err(e)) => win.flash_error(&format!("copy out of the archive: {e}")),
                Err(_) => win.flash_error("copy out of the archive failed"),
            }
        });
    }

    /// `X` / `:extract`: extracts the archives under the cursor or selected, next to them (an
    /// archive holding one folder becomes that folder, anything else a folder named after the
    /// archive; never over existing files). `replace` then moves each archive to the trash.
    /// Inside an archive, the selection is copied out next to the archive instead.
    fn extract_archives(&self, replace: bool) {
        let pane = self.pane();
        let targets = pane.targets();
        if pane.in_archive.get() {
            let Some((archive, _)) = vector_core::archive::split(&pane.dir()) else {
                return;
            };
            let dest = archive.parent().unwrap_or(Path::new("/")).to_path_buf();
            let paths: Vec<PathBuf> = targets.into_iter().map(|e| e.path).collect();
            if !paths.is_empty() {
                self.copy_out(paths, dest);
            }
            return;
        }
        let archives: Vec<PathBuf> = targets
            .into_iter()
            .filter(|e| !e.is_dir() && vector_core::archive::is_archive_name(&e.path))
            .map(|e| e.path)
            .collect();
        if archives.is_empty() {
            self.flash("X extracts zip and tar archives");
            return;
        }
        self.leave_visual();
        for archive in archives {
            let name = file_name(&archive);
            self.flash(&format!("extracting {name}…"));
            let weak = self.weak_self.borrow().clone();
            glib::spawn_future_local(async move {
                let a = archive.clone();
                let r = gio::spawn_blocking(
                    move || -> std::io::Result<(PathBuf, vector_core::archive::Extracted)> {
                        let (dest, from) = vector_core::archive::plan(&a)?;
                        std::fs::create_dir_all(&dest)?;
                        let cancel = std::sync::atomic::AtomicBool::new(false);
                        let r = vector_core::archive::extract(&a, &from, &from, &dest, &cancel)?;
                        Ok((dest, r))
                    },
                )
                .await;
                let Some(win) = weak.upgrade() else { return };
                match r {
                    Ok(Ok((dest, r))) => {
                        let n = r.written;
                        let mut msg =
                            format!("extracted {n} file{} to {}", plural(n), file_name(&dest));
                        let left = r.skipped + r.existing;
                        if left > 0 {
                            msg.push_str(&format!(" · {left} left out (links or unsafe names)"));
                        }
                        // the archive only goes when everything in it came out
                        if replace && left > 0 {
                            msg.push_str(&format!(" · {name} kept"));
                        } else if replace {
                            match gio::File::for_path(&archive)
                                .trash_future(glib::Priority::DEFAULT)
                                .await
                            {
                                Ok(()) => msg.push_str(&format!(" · {name} moved to the trash")),
                                Err(e) => msg.push_str(&format!(" · {name} kept: {}", e.message())),
                            }
                        }
                        win.notify(&msg);
                        win.pane().reload();
                    }
                    Ok(Err(e)) => win.flash_error(&format!("extract {name}: {e}")),
                    Err(_) => win.flash_error(&format!("extract {name} failed")),
                }
            });
        }
    }

    /// `gr` / `:restore`: the selected trash items go back where they came from (with their
    /// folder recreated; never over something that is there now).
    fn restore_from_trash(&self) {
        let pane = self.pane();
        if !vector_core::trash::is_trash(&pane.dir()) {
            self.flash("gr restores things in the Trash (sidebar, or :cd trash)");
            return;
        }
        let names: Vec<String> = pane.targets().into_iter().map(|e| e.name).collect();
        if names.is_empty() {
            return;
        }
        if vector_core::no_launch() {
            self.flash(&format!("test run: would restore {} item(s)", names.len()));
            return;
        }
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let (mut done, mut failed) = (0, Vec::new());
            for n in &names {
                let uri = format!("trash:///{}", glib::Uri::escape_string(n, None, false));
                let args = ["gio", "trash", "--restore", uri.as_str()];
                let args: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
                let ok = match gio::Subprocess::newv(&args, gio::SubprocessFlags::STDERR_SILENCE) {
                    Ok(p) => p.wait_check_future().await.is_ok(),
                    Err(_) => false,
                };
                if ok {
                    done += 1;
                } else {
                    failed.push(n.clone());
                }
            }
            let Some(win) = weak.upgrade() else { return };
            if failed.is_empty() {
                win.notify(&format!("restored {done} item{}", plural(done)));
            } else {
                win.flash_error(&format!(
                    "restored {done}; not restored (something is in the way?): {}",
                    failed.join(", ")
                ));
            }
            win.pane().reload();
        });
    }

    /// `:empty-trash`: deletes everything in the trash, after asking.
    fn empty_trash(&self) {
        self.show_dialog(
            "empty the trash?",
            &[],
            "everything in it is deleted for good",
            vec![
                cancel(),
                button("empty trash", "danger", |w, _| {
                    if vector_core::no_launch() {
                        w.flash("test run: would empty the trash");
                        return;
                    }
                    let weak = w.weak_self.borrow().clone();
                    glib::spawn_future_local(async move {
                        let args: Vec<&std::ffi::OsStr> = ["gio", "trash", "--empty"]
                            .iter()
                            .map(std::ffi::OsStr::new)
                            .collect();
                        let ok = match gio::Subprocess::newv(&args, gio::SubprocessFlags::NONE) {
                            Ok(p) => p.wait_check_future().await.is_ok(),
                            Err(_) => false,
                        };
                        if let Some(win) = weak.upgrade() {
                            if ok {
                                win.notify("trash emptied");
                            } else {
                                win.flash_error("couldn't empty the trash");
                            }
                            win.pane().reload();
                        }
                    });
                }),
            ],
            None,
        );
    }

    /// The type of a file as the listing has it (from its name), else a guess from the name.
    /// Unknown types are sniffed from the contents by the caller.
    fn content_type_of(&self, path: &Path) -> String {
        let pane = self.pane();
        let model = pane.model.borrow();
        path.file_name()
            .and_then(|n| model.find(&n.to_string_lossy()))
            .filter(|e| e.path == path && !e.content_type.is_empty())
            .map(|e| e.content_type.clone())
            .unwrap_or_else(|| gio::content_type_guess(Some(path), None).0.to_string())
    }

    /// Lets the user pick an app for a file no app is set up for: apps that say they handle
    /// the type first, then all others. Typing filters; Enter opens the first match.
    fn open_with_dialog(&self, path: PathBuf, ct: &str) {
        let ct = glib::GString::from(ct);
        let mut apps = gio::AppInfo::all_for_type(&ct);
        let known: Vec<_> = apps.iter().filter_map(|a| a.id()).collect();
        let mut rest: Vec<gio::AppInfo> = gio::AppInfo::all()
            .into_iter()
            .filter(|a| a.should_show() && a.id().is_some_and(|id| !known.contains(&id)))
            .collect();
        rest.sort_by_key(|a| a.display_name().to_lowercase());
        apps.retain(|a| a.should_show());
        apps.extend(rest);
        let apps = Rc::new(apps);
        let name = file_name(&path);
        let desc = gio::content_type_get_description(&ct).to_string();
        // "always" needs something to remember: the type, or for unknown types the extension
        let by_ext = gio::content_type_is_unknown(&ct)
            .then(|| path.extension().map(|e| e.to_string_lossy().to_lowercase()))
            .flatten();
        let kind = match &by_ext {
            Some(e) => format!(".{e} files"),
            None if gio::content_type_is_unknown(&ct) => String::new(),
            None => desc,
        };
        let always = gtk::CheckButton::with_label(&format!("always use it for {kind}"));
        always.add_css_class("always");
        always.set_visible(!kind.is_empty());
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("type to filter apps"));
        let launch = {
            let (path, ct, always, by_ext) =
                (path.clone(), ct.clone(), always.clone(), by_ext.clone());
            move |win: &Win, app: &gio::AppInfo| {
                let remember = || match &by_ext {
                    Some(e) => app.set_as_default_for_extension(e),
                    None => app.set_as_default_for_type(&ct),
                };
                if always.is_active()
                    && let Err(e) = remember()
                {
                    win.flash_error(&format!("{}: {}", app.display_name(), e.message()));
                }
                if vector_core::no_launch() {
                    win.flash(&format!("test run: would open with {}", app.display_name()));
                    return;
                }
                let ctx = WidgetExt::display(&win.window).app_launch_context();
                if let Err(e) = app.launch(&[gio::File::for_path(&path)], Some(&ctx)) {
                    win.flash_error(&format!("{}: {}", app.display_name(), e.message()));
                }
            }
        };
        let launch = Rc::new(launch);
        let matches = |a: &gio::AppInfo, q: &str| {
            q.is_empty() || a.display_name().to_lowercase().contains(&q.to_lowercase())
        };
        let open = {
            let (apps, launch) = (apps.clone(), launch.clone());
            button("open", "primary", move |win, text| {
                let q = text.unwrap_or_default();
                match apps.iter().find(|a| matches(a, q.trim())) {
                    Some(app) => launch(win, app),
                    None => win.flash_error(&format!("no app matches \"{}\"", q.trim())),
                }
            })
        };
        self.show_dialog(
            &format!("open {name} with"),
            &[],
            "",
            vec![cancel(), open],
            Some(entry.clone()),
        );
        // the app list and the checkbox go between the entry and the buttons
        let Some(card) = self.overlay_layer.first_child().and_downcast::<gtk::Box>() else {
            return;
        };
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.set_margin_top(8);
        let mut rows = Vec::new();
        for app in apps.iter() {
            let b = gtk::Button::with_label(&app.display_name());
            b.add_css_class("link");
            b.add_css_class("app-row");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let weak = self.weak_self.borrow().clone();
            let (app, launch) = (app.clone(), launch.clone());
            b.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.close_dialog(0);
                    launch(&win, &app);
                }
            });
            list.append(&b);
            rows.push(b);
        }
        if apps.is_empty() {
            let l = gtk::Label::new(Some("no apps installed"));
            l.add_css_class("empty");
            list.append(&l);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_max_content_height(300);
        scroll.set_propagate_natural_height(true);
        scroll.set_child(Some(&list));
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("body");
        body.append(&scroll);
        body.append(&always);
        let foot = card.last_child();
        card.insert_child_after(&body, foot.and_then(|f| f.prev_sibling()).as_ref());
        // the row Enter opens: the first one that matches
        let mark = move |rows: &[gtk::Button], apps: &[gio::AppInfo], q: &str| {
            let mut first = true;
            for (row, app) in rows.iter().zip(apps.iter()) {
                let on = matches(app, q.trim());
                row.set_visible(on);
                if on && first {
                    row.add_css_class("target");
                    first = false;
                } else {
                    row.remove_css_class("target");
                }
            }
        };
        mark(&rows, &apps, "");
        let apps2 = apps.clone();
        entry.connect_changed(move |e| mark(&rows, &apps2, &e.text()));
        entry.grab_focus();
    }

    /// Turns this window into a file picker for an app's open/save dialog.
    pub fn set_picker(&self, req: crate::portal::Request, respond: crate::portal::Respond) {
        use crate::portal::Kind;
        self.window.set_title(Some(if req.title.is_empty() {
            "pick a file"
        } else {
            &req.title
        }));
        let bar = &self.pick_slot;
        while let Some(c) = bar.first_child() {
            bar.remove(&c);
        }
        let title = gtk::Label::new(Some(&if req.title.is_empty() {
            req.accept.clone()
        } else {
            req.title.to_lowercase()
        }));
        title.add_css_class("title");
        bar.append(&title);
        let filter_button = (!req.filters.is_empty()).then(|| {
            let b = gtk::Button::new();
            b.add_css_class("link");
            b.set_tooltip_text(Some("file types (click to change)"));
            let weak = self.weak_self.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.cycle_pick_filter();
                }
            });
            bar.append(&b);
            b
        });
        let hint = match (req.kind, req.directory) {
            (Kind::Save, _) => "type a name · enter saves",
            (_, true) => "enter opens · ctrl+enter picks this folder",
            _ if req.multiple => "enter picks · space or v selects several · ctrl+enter picks them",
            _ => "enter picks",
        };
        let hint = gtk::Label::new(Some(hint));
        hint.add_css_class("dim");
        hint.set_hexpand(true);
        hint.set_xalign(0.0);
        hint.set_margin_start(12);
        hint.set_ellipsize(gtk::pango::EllipsizeMode::End);
        bar.append(&hint);
        let name = (req.kind == Kind::Save).then(|| {
            let e = gtk::Entry::new();
            e.set_width_chars(36);
            e.set_text(req.current_name.as_deref().unwrap_or(""));
            let weak = self.weak_self.borrow().clone();
            e.connect_activate(move |_| {
                if let Some(w) = weak.upgrade() {
                    w.accept_pick();
                }
            });
            bar.append(&e);
            e
        });
        for (label, primary) in [("cancel", false), (req.accept.as_str(), true)] {
            let b = gtk::Button::with_label(label);
            b.add_css_class("link");
            if primary {
                b.add_css_class("primary");
            }
            let weak = self.weak_self.borrow().clone();
            b.connect_clicked(move |_| {
                if let Some(w) = weak.upgrade() {
                    if primary {
                        w.accept_pick()
                    } else {
                        w.finish_pick(None)
                    }
                }
            });
            bar.append(&b);
        }
        bar.set_visible(true);
        let filter = req.current_filter.unwrap_or(0);
        *self.pick.borrow_mut() = Some(Pick {
            kind: req.kind,
            multiple: req.multiple,
            directory: req.directory,
            filters: req.filters,
            filter,
            filter_button,
            name: name.clone(),
            respond: Some(respond),
        });
        self.apply_pick_filter();
        // closing the window cancels
        let weak = self.weak_self.borrow().clone();
        self.window.connect_close_request(move |_| {
            if let Some(w) = weak.upgrade()
                && let Some(r) = w.pick.borrow_mut().as_mut().and_then(|p| p.respond.take())
            {
                r(None);
            }
            glib::Propagation::Proceed
        });
        if let Some(e) = name {
            e.grab_focus();
            let text = e.text();
            let stem = text
                .rfind('.')
                .filter(|&i| i > 0)
                .map_or(text.chars().count(), |i| text[..i].chars().count());
            e.select_region(0, stem as i32);
        }
    }

    fn apply_pick_filter(&self) {
        let (allow, label) = match self.pick.borrow().as_ref() {
            Some(p) if !p.filters.is_empty() => {
                let (name, allow) = &p.filters[p.filter.min(p.filters.len() - 1)];
                let more = if p.filters.len() > 1 {
                    format!(" ({}/{})", p.filter + 1, p.filters.len())
                } else {
                    String::new()
                };
                (
                    Some(allow.clone()),
                    format!("{}{more}", name.to_lowercase()),
                )
            }
            _ => (None, String::new()),
        };
        if let Some(b) = self
            .pick
            .borrow()
            .as_ref()
            .and_then(|p| p.filter_button.clone())
        {
            b.set_label(&label);
        }
        let pane = self.pane();
        pane.model.borrow_mut().set_allow(allow);
        pane.refresh();
    }

    fn cycle_pick_filter(&self) {
        if let Some(p) = self.pick.borrow_mut().as_mut()
            && !p.filters.is_empty()
        {
            p.filter = (p.filter + 1) % p.filters.len();
        }
        self.apply_pick_filter();
    }

    /// A file was opened (Enter, l, double click) while picking: pick it (or the selection),
    /// or in save mode take its name. Returns whether the picker handled it.
    fn pick_file(&self, path: &Path) -> bool {
        use crate::portal::Kind;
        let (kind, multiple, directory, name) = match self.pick.borrow().as_ref() {
            Some(p) => (p.kind, p.multiple, p.directory, p.name.clone()),
            None => return false,
        };
        match kind {
            Kind::Open if !directory => {
                let pane = self.pane();
                let mut files: Vec<PathBuf> = if multiple && pane.selection.borrow().has_selection()
                {
                    pane.targets()
                        .into_iter()
                        .filter(|e| !e.is_dir())
                        .map(|e| e.path)
                        .collect()
                } else {
                    Vec::new()
                };
                if files.is_empty() {
                    files.push(path.to_path_buf());
                }
                self.finish_pick(Some(files));
            }
            Kind::Save => {
                if let (Some(e), Some(n)) = (name, path.file_name()) {
                    e.set_text(&n.to_string_lossy());
                    e.grab_focus();
                }
            }
            _ => {}
        }
        true
    }

    /// The accept button: open the selection, save under the typed name, or pick the folder.
    fn accept_pick(&self) {
        use crate::portal::Kind;
        let (kind, directory, name) = match self.pick.borrow().as_ref() {
            Some(p) => (p.kind, p.directory, p.name.clone()),
            None => return,
        };
        let pane = self.pane();
        match kind {
            Kind::Save => {
                let typed = name.map(|e| e.text().to_string()).unwrap_or_default();
                if typed.trim().is_empty() {
                    self.notify_error("type a file name");
                    return;
                }
                let target = resolve(&pane.dir(), typed.trim());
                if target.exists() {
                    let t = target.clone();
                    self.show_dialog(
                        &format!("replace {}?", file_name(&target)),
                        &[],
                        "the existing file is overwritten by the app",
                        vec![
                            cancel(),
                            button("replace", "danger", move |w, _| {
                                w.finish_pick(Some(vec![t.clone()]))
                            }),
                        ],
                        None,
                    );
                    return;
                }
                self.finish_pick(Some(vec![target]));
            }
            _ if directory => {
                let dirs: Vec<PathBuf> = pane
                    .targets()
                    .into_iter()
                    .filter(|e| e.is_dir())
                    .map(|e| e.path)
                    .collect();
                let explicit = pane.selection.borrow().has_selection();
                self.finish_pick(Some(if explicit && !dirs.is_empty() {
                    dirs
                } else {
                    vec![pane.dir()]
                }));
            }
            _ => {
                let files: Vec<PathBuf> = pane
                    .targets()
                    .into_iter()
                    .filter(|e| !e.is_dir())
                    .map(|e| e.path)
                    .collect();
                if files.is_empty() {
                    self.notify("pick a file (enter on a folder opens it)");
                    return;
                }
                self.finish_pick(Some(files));
            }
        }
    }

    fn finish_pick(&self, picked: Option<Vec<PathBuf>>) {
        let respond = self
            .pick
            .borrow_mut()
            .as_mut()
            .and_then(|p| p.respond.take());
        if let Some(r) = respond {
            r(picked);
        }
        self.window.close();
    }

    /// `:cd ` in the command line with the current path filled in and selected, so typing or
    /// pasting replaces it and Tab completes.
    fn type_path(&self) {
        let path = format!(
            "{}/",
            display_path(&self.pane().dir()).trim_end_matches('/')
        );
        self.enter_line(Mode::Command);
        self.setting_line.set(true);
        self.cmd_entry.set_text(&format!("cd {path}"));
        self.setting_line.set(false);
        self.cmd_entry.select_region(3, -1);
    }

    /// Copies paths to the clipboard: the selected items, else (with `folder`) the current
    /// folder, else the item under the cursor.
    fn copy_path(&self, folder: bool) {
        let pane = self.pane();
        let selected = pane.selection.borrow().has_selection();
        let text = if selected || !folder {
            let paths: Vec<String> = pane
                .targets()
                .iter()
                .map(|e| e.path.display().to_string())
                .collect();
            if paths.is_empty() {
                pane.dir().display().to_string()
            } else {
                paths.join("\n")
            }
        } else {
            pane.dir().display().to_string()
        };
        self.window.clipboard().set_text(&text);
        let n = text.lines().count();
        self.notify(&if n == 1 {
            format!("copied {text}")
        } else {
            format!("copied {n} paths")
        });
    }

    /// Opens `path` in $VISUAL / $EDITOR (nvim by default) in a new terminal; like vim, a path
    /// that doesn't exist yet is created when the editor saves it.
    fn open_in_editor(&self, path: &Path) {
        if vector_core::no_launch() {
            self.flash(&format!("test run: would edit {}", file_name(path)));
            return;
        }
        let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
        if let Some(parent) = path.parent() {
            launcher.set_cwd(parent);
        }
        let script = r#"exec ${VISUAL:-${EDITOR:-nvim}} "$1""#;
        let cmd = [
            OsStr::new("sh"),
            OsStr::new("-c"),
            OsStr::new(script),
            OsStr::new("sh"),
            path.as_os_str(),
        ];
        let Some(argv) = crate::terminal::argv(&cmd) else {
            self.notify_error("no terminal found; set $TERMINAL or install xdg-terminal-exec");
            return;
        };
        let refs: Vec<&OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
        if let Err(e) = launcher.spawn(&refs) {
            self.flash_error(&format!("editor: {}", e.message()));
        }
    }

    fn open_terminal(&self, dir: &Path) {
        if vector_core::no_launch() {
            self.flash("test run: would open a terminal here");
            return;
        }
        let launcher = gio::SubprocessLauncher::new(gio::SubprocessFlags::NONE);
        launcher.set_cwd(dir);
        let Some(argv) = crate::terminal::argv(&[]) else {
            self.notify_error("no terminal found; set $TERMINAL or install xdg-terminal-exec");
            return;
        };
        let refs: Vec<&OsStr> = argv.iter().map(|a| a.as_os_str()).collect();
        if let Err(e) = launcher.spawn(&refs) {
            self.flash_error(&format!("terminal: {}", e.message()));
        }
    }

    // ── paste, transfers, deletes ───────────────────────────────────────────

    /// Pastes vector's own yank/cut, or files copied in another app (Nautilus, a browser…)
    /// from the system clipboard.
    fn paste(&self) {
        let dest = self.pane().dir();
        let clipboard = self.window.clipboard();
        if clipboard.is_local()
            || !clipboard
                .formats()
                .contains_type(gdk::FileList::static_type())
        {
            let Some((paths, cut)) = self
                .register
                .borrow()
                .as_ref()
                .map(|r| (r.paths.clone(), r.cut))
            else {
                self.notify("nothing to paste");
                return;
            };
            if cut {
                // a cut pastes once
                *self.register.borrow_mut() = None;
                clipboard.set_content(None::<&gdk::ContentProvider>).ok();
                self.update_marks();
            }
            self.transfer(paths, dest, cut);
            return;
        }
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            // Nautilus marks a cut with x-special/gnome-copied-files ("cut\nfile://…")
            let mut cut = false;
            if clipboard
                .formats()
                .contain_mime_type("x-special/gnome-copied-files")
                && let Ok((stream, _)) = clipboard
                    .read_future(&["x-special/gnome-copied-files"], glib::Priority::DEFAULT)
                    .await
                && let Ok(bytes) = stream.read_bytes_future(4, glib::Priority::DEFAULT).await
            {
                cut = bytes.as_ref() == b"cut\n" || bytes.as_ref().starts_with(b"cut");
            }
            let files = clipboard
                .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
                .await;
            let Some(win) = weak.upgrade() else { return };
            let paths: Vec<PathBuf> = files
                .ok()
                .and_then(|v| v.get::<gdk::FileList>().ok())
                .map(|l| l.files().iter().filter_map(|f| f.path()).collect())
                .unwrap_or_default();
            if paths.is_empty() {
                win.notify("nothing to paste");
                return;
            }
            win.transfer(paths, dest, cut);
        });
    }

    /// Puts files on the system clipboard the way GNOME apps expect: as a file list (text/uri-list)
    /// and x-special/gnome-copied-files (which says copy or cut), plus plain paths for terminals.
    fn set_file_clipboard(&self, paths: &[PathBuf], cut: bool) {
        // test runs (scripts/shot.sh) must not overwrite the user's clipboard
        if std::env::var_os("VECTOR_NO_CLIPBOARD").is_some() {
            return;
        }
        let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
        let uris: Vec<String> = files.iter().map(|f| f.uri().to_string()).collect();
        let gnome = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
        let plain: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let providers = [
            gdk::ContentProvider::for_value(&gdk::FileList::from_array(&files).to_value()),
            gdk::ContentProvider::for_bytes(
                "x-special/gnome-copied-files",
                &glib::Bytes::from_owned(gnome.into_bytes()),
            ),
            gdk::ContentProvider::for_bytes(
                "text/plain;charset=utf-8",
                &glib::Bytes::from_owned(plain.join("\n").into_bytes()),
            ),
        ];
        if self
            .window
            .clipboard()
            .set_content(Some(&gdk::ContentProvider::new_union(&providers)))
            .is_err()
        {
            self.flash_error("couldn't set the clipboard");
        }
    }

    /// Shows which files are yanked or cut in every pane.
    fn update_marks(&self) {
        let (paths, cut) = match self.register.borrow().as_ref() {
            Some(r) => (r.paths.iter().cloned().collect(), r.cut),
            None => (std::collections::HashSet::new(), false),
        };
        for tab in self.tabs.borrow().iter() {
            for p in tab.panes.borrow().iter() {
                p.set_marks(paths.clone(), cut);
            }
        }
    }

    /// A short notice above the statusline for things that just happened.
    fn notify(&self, text: &str) {
        self.show_notice(text, false);
    }

    fn notify_error(&self, text: &str) {
        self.show_notice(text, true);
    }

    fn show_notice(&self, text: &str, crit: bool) {
        self.notice.set_text(text);
        if crit {
            self.notice.add_css_class("crit");
        } else {
            self.notice.remove_css_class("crit");
        }
        self.notice.set_visible(true);
        if let Some(id) = self.notice_timer.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak_self.borrow().clone();
        let id = glib::timeout_add_local_once(
            Duration::from_millis(if crit { 5000 } else { 2500 }),
            move || {
                if let Some(win) = weak.upgrade() {
                    win.notice_timer.borrow_mut().take();
                    win.notice.set_visible(false);
                }
            },
        );
        *self.notice_timer.borrow_mut() = Some(id);
    }

    /// Copies or moves `paths` into `dest`, asking first when names are taken.
    fn transfer(&self, paths: Vec<PathBuf>, dest: PathBuf, cut: bool) {
        // dropped or pasted into an archive: it is read-only
        if vector_core::archive::split(&dest).is_some() {
            self.flash("read-only: this is inside an archive");
            return;
        }
        if paths
            .iter()
            .any(|p| vector_core::archive::split(p).is_some_and(|(_, i)| !i.as_os_str().is_empty()))
        {
            self.copy_out(paths, dest);
            return;
        }
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let (p, d) = (paths.clone(), dest.clone());
            let taken = gio::spawn_blocking(move || vector_core::ops::conflicts(&p, &d))
                .await
                .unwrap_or_default();
            let Some(win) = weak.upgrade() else { return };
            let make = move |conflict: Conflict| {
                if cut {
                    Op::Move {
                        sources: paths.clone(),
                        dest: dest.clone(),
                        conflict,
                    }
                } else {
                    Op::Copy {
                        sources: paths.clone(),
                        dest: dest.clone(),
                        conflict,
                    }
                }
            };
            if taken.is_empty() {
                win.ops.submit(make(Conflict::Skip));
                return;
            }
            let lines: Vec<String> = taken.iter().map(|p| file_name(p)).collect();
            let make = Rc::new(make);
            let (m1, m2) = (make.clone(), make.clone());
            win.show_dialog(
                &format!(
                    "{} already exist{}",
                    count_label(&taken),
                    if taken.len() == 1 { "s" } else { "" }
                ),
                &lines,
                "same-name files inside folders are replaced too",
                vec![
                    cancel(),
                    button("skip", "", move |w, _| {
                        w.ops.submit(m1(Conflict::Skip));
                    }),
                    button("replace", "danger", move |w, _| {
                        w.ops.submit(m2(Conflict::Overwrite));
                    }),
                    button("keep both", "primary", move |w, _| {
                        w.ops.submit(make(Conflict::KeepBoth));
                    }),
                ],
                None,
            );
        });
    }

    fn confirm_delete(&self, paths: Vec<PathBuf>, title: &str) {
        if paths.is_empty() {
            return;
        }
        let lines: Vec<String> = paths.iter().map(|p| file_name(p)).collect();
        let what = if paths.len() == 1 {
            "1 item".to_string()
        } else {
            format!("{} items", paths.len())
        };
        self.show_dialog(
            &format!("{title} · {what}"),
            &lines,
            "this can't be undone",
            vec![
                cancel(),
                button("delete", "danger", move |w, _| {
                    w.ops.submit(Op::DeletePermanently {
                        paths: paths.clone(),
                    });
                }),
            ],
            None,
        );
    }

    fn rename_dialog(&self) {
        let Some(e) = self.pane().cursor_entry() else {
            return;
        };
        let entry = gtk::Entry::new();
        entry.set_text(&e.name);
        entry.set_width_chars(40);
        let path = e.path.clone();
        self.show_dialog(
            "rename",
            &[],
            "",
            vec![
                cancel(),
                button("rename", "primary", move |w, text| {
                    if let Some(t) = text.filter(|t| !t.is_empty()) {
                        w.ops.submit(Op::Rename {
                            path: path.clone(),
                            new_name: t,
                        });
                    }
                }),
            ],
            Some(entry.clone()),
        );
        self.renaming.set(true);
        self.update_status();
        entry.grab_focus();
        // select the name without its extension, like other file managers
        let stem = if e.is_dir() {
            e.name.chars().count()
        } else {
            e.name
                .rfind('.')
                .filter(|&i| i > 0)
                .map_or(e.name.chars().count(), |i| e.name[..i].chars().count())
        };
        entry.select_region(0, stem as i32);
    }

    /// gs: measure the selected folders now, also on network mounts.
    fn folder_sizes(&self) {
        let pane = self.pane();
        let dirs: Vec<(String, PathBuf)> = pane
            .targets()
            .into_iter()
            .filter(|e| e.is_dir())
            .map(|e| (e.name, e.path))
            .collect();
        if dirs.is_empty() {
            return;
        }
        pane.measure(Some(dirs));
    }

    fn watch_ops(&self) {
        let rx = self.ops.events.clone();
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = rx.recv().await {
                let Some(win) = weak.upgrade() else { return };
                match ev {
                    OpEvent::Started { label, .. } => {
                        win.flash(&format!("{label}…"));
                        win.progress.set_fraction(0.0);
                    }
                    OpEvent::Progress { done, total, .. } => {
                        if total > 0 {
                            win.progress.set_visible(true);
                            win.progress.set_fraction(done as f64 / total as f64);
                        }
                    }
                    OpEvent::Finished { report, .. } => {
                        win.progress.set_visible(false);
                        let unsupported: Vec<PathBuf> = report
                            .errors
                            .iter()
                            .filter(|e| e.kind == ErrorKind::TrashUnsupported)
                            .map(|e| e.path.clone())
                            .collect();
                        let others: Vec<String> = report
                            .errors
                            .iter()
                            .filter(|e| e.kind != ErrorKind::TrashUnsupported)
                            // "rename a → b: name is taken", not "…: b: name is taken"
                            .map(|e| match e.path.file_name() {
                                Some(n) if report.label.contains(&*n.to_string_lossy()) => {
                                    e.message.clone()
                                }
                                _ => e.to_string(),
                            })
                            .collect();
                        if report.cancelled {
                            win.notify(&format!("{} cancelled", report.label));
                        } else if !others.is_empty() {
                            win.notify_error(&format!("{}: {}", report.label, others.join("; ")));
                        } else {
                            let undo = if report.undoable { " · u to undo" } else { "" };
                            win.notify(&format!("{}{undo}", report.label));
                        }
                        if !unsupported.is_empty() {
                            win.confirm_delete(
                                unsupported,
                                "no trash on this drive — delete permanently",
                            );
                        }
                        if !report.created.is_empty() {
                            win.pane().select_paths(&report.created);
                        }
                        // folder sizes above whatever changed are out of date now
                        for p in &report.touched {
                            crate::pane::sizes_changed(p);
                        }
                        if !report.touched.is_empty() {
                            for p in win.all_panes() {
                                p.remeasure();
                            }
                        }
                        win.update_status();
                    }
                }
            }
        });
    }

    // ── :find / :grep ───────────────────────────────────────────────────────

    /// `//name` and filters that match nothing here: a fuzzy search of every folder below.
    fn find_below(&self, q: &str) {
        if q.is_empty() {
            return;
        }
        if self.pane().in_archive.get() {
            self.flash("searching works outside archives; / filters this folder");
            return;
        }
        self.search(q.to_string(), false);
    }

    fn search(&self, query: String, content: bool) {
        if let Some(c) = self.search_cancel.borrow_mut().take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *self.search_cancel.borrow_mut() = Some(cancel.clone());
        let root = self.pane().dir();
        let opts = SearchOptions {
            hidden: self.pane().model.borrow().options().show_hidden,
            ..Default::default()
        };
        self.flash(if content {
            "searching contents…"
        } else {
            "finding…"
        });
        let weak = self.weak_self.borrow().clone();
        glib::spawn_future_local(async move {
            let r = root.clone();
            let q = query.clone();
            let res = gio::spawn_blocking(move || -> Result<(Vec<Hit>, bool), String> {
                let run = |opts: &SearchOptions| -> Result<Vec<Hit>, String> {
                    if content {
                        search::grep(&r, &q, opts, &cancel, 500)
                            .map(|v| {
                                v.into_iter()
                                    .map(|h| (h.path, Some(h.line), h.text))
                                    .collect()
                            })
                            .map_err(|e| e.to_string())
                    } else {
                        Ok(search::find(&r, &q, opts, &cancel, 500)
                            .into_iter()
                            .map(|h| (h.path, None, String::new()))
                            .collect())
                    }
                };
                let found = run(&opts)?;
                // nothing outside hidden folders: look inside them too (~/.local, ~/.config…)
                if found.is_empty() && !opts.hidden {
                    let all = SearchOptions {
                        hidden: true,
                        ..opts.clone()
                    };
                    return Ok((run(&all)?, true));
                }
                Ok((found, false))
            })
            .await;
            let Some(win) = weak.upgrade() else { return };
            match res {
                Ok(Ok((items, _))) if items.is_empty() => {
                    win.notify("no matches, hidden folders included")
                }
                Ok(Ok((items, in_hidden))) => {
                    win.flash(&format!(
                        "{} matches{} · enter opens · esc closes",
                        items.len(),
                        if in_hidden { " in hidden folders" } else { "" }
                    ));
                    *win.picker.borrow_mut() = Some(Picker {
                        title: if content { "GREP" } else { "FIND" },
                        root,
                        items,
                        index: 0,
                    });
                    win.render_picker();
                }
                Ok(Err(e)) => win.flash_error(&e),
                Err(_) => win.flash_error("search failed"),
            }
        });
    }

    fn render_picker(&self) {
        let rows: Vec<String> = match self.picker.borrow().as_ref() {
            Some(p) => p
                .items
                .iter()
                .map(|(path, line, text)| match line {
                    Some(l) => format!("{}:{l}  {text}", path.display()),
                    None => path.display().to_string(),
                })
                .collect(),
            None => return,
        };
        let idx = self.picker.borrow().as_ref().map(|p| p.index);
        self.fill_list(&rows, idx);
        self.update_status();
    }

    fn close_picker(&self) {
        if self.picker.borrow_mut().take().is_some() {
            self.clear_list();
            self.update_status();
        }
    }

    fn picker_key(&self, key: Key) -> bool {
        let n = self.picker.borrow().as_ref().map_or(0, |p| p.items.len());
        let step = |d: isize| {
            if let Some(p) = self.picker.borrow_mut().as_mut() {
                p.index = (p.index as isize + d).rem_euclid(n.max(1) as isize) as usize;
            }
        };
        match key {
            Key::Esc => self.close_picker(),
            Key::Char('j') | Key::Down | Key::Tab | Key::Ctrl('n') => {
                step(1);
                self.render_picker();
            }
            Key::Char('k') | Key::Up | Key::BackTab | Key::Ctrl('p') => {
                step(-1);
                self.render_picker();
            }
            Key::Enter | Key::Char('l') => {
                let pick = self
                    .picker
                    .borrow()
                    .as_ref()
                    .and_then(|p| p.items.get(p.index).map(|i| p.root.join(&i.0)));
                self.close_picker();
                if let Some(path) = pick
                    && let Some(parent) = path.parent()
                {
                    let pane = self.pane();
                    pane.open_dir(parent);
                    pane.select_paths(std::slice::from_ref(&path));
                }
            }
            Key::Char(':') => {
                self.close_picker();
                self.enter_line(Mode::Command);
            }
            _ => return true,
        }
        true
    }

    // ── :edit ───────────────────────────────────────────────────────────────

    fn set_filter(&self, f: &str) {
        let pane = self.pane();
        if let Err(e) = vector_core::filter::Filter::parse(f) {
            self.flash_error(&format!("filter: {e}"));
            return;
        }
        pane.model.borrow_mut().set_filter(f);
        pane.selection
            .borrow_mut()
            .set_cursor(&pane.model.borrow(), 0);
        pane.refresh();
        if f.is_empty() {
            self.flash("filter cleared");
        }
    }

    fn start_edit(&self) {
        let pane = self.pane();
        if pane.is_flat() {
            self.flash_error("edit works on one folder; leave the flat view first (:flat)");
            return;
        }
        let dir = pane.dir();
        let listed: Vec<edit::Listed> = pane
            .model
            .borrow()
            .iter()
            .enumerate()
            .map(|(i, e)| edit::Listed {
                id: i + 1,
                name: e.name.clone(),
                is_dir: e.is_dir(),
            })
            .collect();
        let text = edit::render(&display_path(&dir), &listed);
        let file = glib::user_runtime_dir().join(format!("vector-edit-{}.txt", std::process::id()));
        let marker = file.with_extension("txt.done");
        let _ = std::fs::remove_file(&marker);
        if let Err(e) = std::fs::write(&file, text) {
            self.flash_error(&format!("edit: {e}"));
            return;
        }
        // The terminal may hand off to an already running instance and return at once, so the
        // shell inside it marks when nvim has really exited.
        let script = r#"${VISUAL:-${EDITOR:-nvim}} "$1"; : > "$1.done""#;
        let mut argv: Vec<std::ffi::OsString> = match std::env::var_os("VECTOR_EDIT_CMD") {
            // tests: run the editor command directly, without a terminal
            Some(cmd) => vec![
                "sh".into(),
                "-c".into(),
                format!("{} \"$1\"; : > \"$1.done\"", cmd.to_string_lossy()).into(),
            ],
            None if vector_core::no_launch() => {
                self.flash("test run: would open the editor (set VECTOR_EDIT_CMD)");
                return;
            }
            None => {
                let cmd = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new(script)];
                match crate::terminal::argv(&cmd) {
                    Some(v) => v,
                    None => {
                        self.notify_error(
                            "no terminal found; set $TERMINAL or install xdg-terminal-exec",
                        );
                        return;
                    }
                }
            }
        };
        argv.push("sh".into());
        argv.push(file.clone().into());
        let refs: Vec<&std::ffi::OsStr> = argv.iter().map(|s| s.as_os_str()).collect();
        if let Err(e) = gio::Subprocess::newv(&refs, gio::SubprocessFlags::NONE) {
            self.flash_error(&format!("edit: {}", e.message()));
            return;
        }
        self.flash("editing in your editor — save and quit to apply");
        if let Some(id) = self.edit_poll.borrow_mut().take() {
            id.remove();
        }
        let weak = self.weak_self.borrow().clone();
        let id = glib::timeout_add_local(Duration::from_millis(250), move || {
            let Some(win) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !marker.exists() {
                return glib::ControlFlow::Continue;
            }
            win.edit_poll.borrow_mut().take();
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let _ = std::fs::remove_file(&file);
            let _ = std::fs::remove_file(&marker);
            win.finish_edit(&dir, &listed, &text);
            glib::ControlFlow::Break
        });
        *self.edit_poll.borrow_mut() = Some(id);
    }

    /// `:rename PATTERN`: new names for the selection (or the item under the cursor), shown in
    /// the same confirmation as `ge` and applied the same way (swaps and clashes handled, one
    /// undo).
    fn batch_rename(&self, pattern: &str) {
        let pane = self.pane();
        if pane.is_flat() {
            self.flash_error("rename works in one folder; leave the flat view first (:flat)");
            return;
        }
        let targets: std::collections::HashSet<String> =
            pane.targets().into_iter().map(|e| e.name).collect();
        if targets.is_empty() {
            self.flash("select what to rename (v or space), then :rename photo-{n}");
            return;
        }
        let model = pane.model.borrow();
        let picked: Vec<&vector_core::entry::Entry> =
            model.iter().filter(|e| targets.contains(&e.name)).collect();
        let items: Vec<vector_core::rename::Item> = picked
            .iter()
            .map(|e| vector_core::rename::Item {
                name: &e.name,
                dir: e.is_dir(),
                modified: e.modified,
            })
            .collect();
        let new = match vector_core::rename::apply(pattern, &items) {
            Ok(n) => n,
            Err(e) => {
                drop(model);
                self.flash_error(&e);
                return;
            }
        };
        let renamed: std::collections::HashMap<&str, &str> = picked
            .iter()
            .zip(&new)
            .map(|(e, n)| (e.name.as_str(), n.as_str()))
            .collect();
        let listed: Vec<edit::Listed> = model
            .iter()
            .enumerate()
            .map(|(i, e)| edit::Listed {
                id: i + 1,
                name: e.name.clone(),
                is_dir: e.is_dir(),
            })
            .collect();
        // the buffer `ge` would show, with the new names typed in
        let edited: Vec<edit::Listed> = listed
            .iter()
            .map(|l| edit::Listed {
                id: l.id,
                name: renamed
                    .get(l.name.as_str())
                    .map_or_else(|| l.name.clone(), |n| n.to_string()),
                is_dir: l.is_dir,
            })
            .collect();
        drop(model);
        let dir = pane.dir();
        let text = edit::render(&display_path(&dir), &edited);
        self.finish_edit(&dir, &listed, &text);
    }

    fn finish_edit(&self, dir: &Path, listed: &[edit::Listed], text: &str) {
        match edit::plan(listed, text) {
            Err(e) => self.flash_error(&format!("edit not applied: {e}")),
            Ok(plan) if plan.is_empty() => self.flash("no changes"),
            Ok(plan) => {
                let lines = edit::describe(&plan);
                let dir = dir.to_path_buf();
                self.show_dialog(
                    &format!(
                        "apply {} change{}",
                        lines.len(),
                        if lines.len() == 1 { "" } else { "s" }
                    ),
                    &lines,
                    if edit::trashes(&plan) {
                        "deleted entries go to the trash"
                    } else {
                        ""
                    },
                    vec![
                        cancel(),
                        button("apply", "primary", move |w, _| {
                            w.ops.submit(Op::ApplyEdit {
                                dir: dir.clone(),
                                plan: plan.clone(),
                            });
                        }),
                    ],
                    None,
                );
            }
        }
    }

    // ── dialogs ─────────────────────────────────────────────────────────────

    /// Shows a card with a title, a list, a note and buttons. The first button is what Esc
    /// does, the last what Enter does; with an entry, callbacks get its text.
    fn show_dialog(
        &self,
        title: &str,
        lines: &[String],
        note: &str,
        buttons: Vec<Button>,
        entry: Option<gtk::Entry>,
    ) {
        while let Some(c) = self.overlay_layer.first_child() {
            self.overlay_layer.remove(&c);
        }
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("card");
        card.set_size_request(440, -1);
        let head = gtk::Label::new(Some(title));
        head.add_css_class("title");
        head.set_xalign(0.0);
        head.set_wrap(true);
        let head_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        head_box.add_css_class("head");
        head_box.append(&head);
        card.append(&head_box);
        let max = 12;
        for l in lines.iter().take(max) {
            let row = gtk::Label::new(Some(l));
            row.set_xalign(0.0);
            row.add_css_class("row");
            row.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            card.append(&row);
        }
        if lines.len() > max {
            let more = gtk::Label::new(Some(&format!("and {} more", lines.len() - max)));
            more.add_css_class("section");
            more.set_xalign(0.0);
            card.append(&more);
        }
        if let Some(e) = &entry {
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("body");
            b.append(e);
            card.append(&b);
        }
        if !note.is_empty() {
            let n = gtk::Label::new(Some(note));
            n.add_css_class("dim");
            n.set_xalign(0.0);
            n.set_wrap(true);
            let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
            b.add_css_class("body");
            b.set_margin_top(10);
            b.append(&n);
            card.append(&b);
        }
        let foot = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        foot.add_css_class("foot");
        foot.set_halign(gtk::Align::End);
        let n = buttons.len();
        let mut actions = Vec::with_capacity(n);
        for (i, b) in buttons.into_iter().enumerate() {
            let w = gtk::Button::with_label(&b.label);
            w.add_css_class("link");
            if !b.class.is_empty() {
                w.add_css_class(b.class);
            }
            let weak = self.weak_self.borrow().clone();
            w.connect_clicked(move |_| {
                if let Some(win) = weak.upgrade() {
                    win.close_dialog(i);
                }
            });
            foot.append(&w);
            actions.push(b.run);
        }
        card.append(&foot);
        self.overlay_layer.append(&card);
        self.overlay_layer.set_visible(true);
        *self.dialog.borrow_mut() = Some(Dialog { actions, entry });
    }

    /// `?`: the help panel, on the topic for what you are doing (typing searches all of it).
    fn show_help(&self) {
        let topic = if self.side_cursor.get().is_some() {
            "move"
        } else {
            match self.mode.get() {
                Mode::Visual => "files",
                Mode::Filter => "find",
                Mode::Command => "commands",
                Mode::Normal if self.pane().in_archive.get() => "archives",
                Mode::Normal => "start",
            }
        };
        self.leave_visual();
        while let Some(c) = self.overlay_layer.first_child() {
            self.overlay_layer.remove(&c);
        }
        self.overlay_layer.append(&self.help.root);
        self.overlay_layer.set_visible(true);
        *self.dialog.borrow_mut() = Some(Dialog {
            actions: vec![Box::new(|_, _| {})],
            entry: Some(self.help.search.clone()),
        });
        self.help_open.set(true);
        self.help.open(self.help.tab_named(topic));
    }

    fn close_dialog(&self, choice: usize) {
        let Some(d) = self.dialog.borrow_mut().take() else {
            return;
        };
        self.help_open.set(false);
        self.renaming.set(false);
        self.update_status();
        self.overlay_layer.set_visible(false);
        while let Some(c) = self.overlay_layer.first_child() {
            self.overlay_layer.remove(&c);
        }
        let text = d.entry.as_ref().map(|e| e.text().to_string());
        if let Some(run) = d.actions.get(choice) {
            run(self, text);
        }
        self.pane().focus_view();
    }

    fn dialog_key(&self, key: Key, synthetic: bool) -> bool {
        let (n, has_entry) = match self.dialog.borrow().as_ref() {
            Some(d) => (d.actions.len(), d.entry.clone()),
            None => return false,
        };
        match key {
            // the help: tab moves between topics, enter (like esc) closes it
            Key::Tab | Key::BackTab if self.help_open.get() => {
                self.help.step_tab(key == Key::BackTab)
            }
            Key::Esc => self.close_dialog(0),
            Key::Enter => self.close_dialog(n.saturating_sub(1)),
            Key::Char('y') if has_entry.is_none() => self.close_dialog(n.saturating_sub(1)),
            Key::Char('n') if has_entry.is_none() => self.close_dialog(0),
            Key::Char(c) if synthetic => {
                if let Some(e) = has_entry {
                    let pos = e.position();
                    e.delete_selection();
                    let mut p = e.position().min(pos);
                    e.insert_text(&c.to_string(), &mut p);
                    e.set_position(p);
                }
            }
            // typing goes to the entry; everything else is swallowed while the card is up
            _ => return has_entry.is_none(),
        }
        true
    }

    // ── context menu ────────────────────────────────────────────────────────

    fn connect_actions(&self) {
        let simple: &[(&str, Action)] = &[
            ("open", Action::Open),
            ("yank", Action::Yank),
            ("cut", Action::Cut),
            ("paste", Action::Paste),
            ("rename", Action::Rename),
            ("trash", Action::Trash),
            ("delete", Action::DeletePermanently),
            ("size", Action::FolderSize),
            ("terminal", Action::Terminal),
            ("hidden", Action::ToggleHidden),
            ("edit", Action::Edit),
            ("split", Action::ToggleSplit),
            ("preview", Action::TogglePreview),
            ("view", Action::ToggleView),
            ("new-tab", Action::NewTab),
            ("pin-here", Action::PinHere),
            ("pin-global", Action::PinGlobal),
            ("extract", Action::Extract),
            ("extract-replace", Action::ExtractReplace),
            ("restore", Action::Restore),
        ];
        for &(name, action) in simple {
            let a = gio::SimpleAction::new(name, None);
            let weak = self.weak_self.borrow().clone();
            a.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.run_action(action, 1);
                }
            });
            self.window.add_action(&a);
        }
        let a = gio::SimpleAction::new("empty-trash", None);
        let weak = self.weak_self.borrow().clone();
        a.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.run_command(Command::EmptyTrash);
            }
        });
        self.window.add_action(&a);
        let a = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
        let weak = self.weak_self.borrow().clone();
        a.connect_activate(move |_, v| {
            let (Some(win), Some(id)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) else {
                return;
            };
            let Some(app) = gio::AppInfo::all()
                .into_iter()
                .find(|a| a.id().as_deref() == Some(id.as_str()))
            else {
                return;
            };
            let files: Vec<gio::File> = win
                .pane()
                .targets()
                .iter()
                .map(|e| gio::File::for_path(&e.path))
                .collect();
            if vector_core::no_launch() {
                win.flash(&format!("test run: would open with {}", app.name()));
                return;
            }
            let ctx = WidgetExt::display(&win.window).app_launch_context();
            if let Err(e) = app.launch(&files, Some(&ctx)) {
                win.flash_error(&format!("{}: {}", app.name(), e.message()));
            }
        });
        self.window.add_action(&a);
        let a = gio::SimpleAction::new("filter", Some(glib::VariantTy::STRING));
        let weak = self.weak_self.borrow().clone();
        a.connect_activate(move |_, v| {
            if let (Some(win), Some(f)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) {
                win.set_filter(&f);
            }
        });
        self.window.add_action(&a);
        let a = gio::SimpleAction::new("flat", None);
        let weak = self.weak_self.borrow().clone();
        a.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.run_command(Command::Flat);
            }
        });
        self.window.add_action(&a);
        let a = gio::SimpleAction::new("copy-path", None);
        let weak = self.weak_self.borrow().clone();
        a.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                win.copy_path(false);
            }
        });
        self.window.add_action(&a);
        for (name, cmd) in [("new-folder", "mkdir "), ("new-file", "new ")] {
            let a = gio::SimpleAction::new(name, None);
            let weak = self.weak_self.borrow().clone();
            a.connect_activate(move |_, _| {
                if let Some(win) = weak.upgrade() {
                    win.enter_line(Mode::Command);
                    win.cmd_entry.set_text(cmd);
                    win.cmd_entry.set_position(-1);
                }
            });
            self.window.add_action(&a);
        }
    }

    fn show_menu(&self, w: &gtk::Widget, x: f64, y: f64) {
        let pane = self.pane();
        let targets = pane.targets();
        let menu = gio::Menu::new();
        let first = gio::Menu::new();
        let has_items = !targets.is_empty();
        if has_items {
            first.append(Some("open"), Some("win.open"));
            if let Some(e) = targets.first().filter(|e| !e.is_dir()) {
                let with = gio::Menu::new();
                let ct = if e.content_type.is_empty() {
                    "application/octet-stream"
                } else {
                    &e.content_type
                };
                let default = gio::AppInfo::default_for_type(ct, false);
                let mut apps = gio::AppInfo::all_for_type(ct);
                if let Some(d) = &default {
                    apps.retain(|a| a.id() != d.id());
                    apps.insert(0, d.clone());
                }
                for app in apps.iter().take(12) {
                    if let Some(id) = app.id() {
                        let item = gio::MenuItem::new(Some(&app.name()), None);
                        item.set_action_and_target_value(
                            Some("win.open-with"),
                            Some(&id.to_variant()),
                        );
                        with.append_item(&item);
                    }
                }
                if with.n_items() > 0 {
                    first.append_submenu(Some("open with"), &with);
                }
            }
        }
        menu.append_section(None, &first);
        let edit_items = gio::Menu::new();
        if has_items {
            edit_items.append(Some("copy"), Some("win.yank"));
            edit_items.append(Some("cut"), Some("win.cut"));
        }
        if self.register.borrow().is_some() {
            edit_items.append(Some("paste"), Some("win.paste"));
        }
        if has_items {
            edit_items.append(Some("rename"), Some("win.rename"));
        }
        if vector_core::trash::is_trash(&pane.dir()) {
            if has_items {
                edit_items.append(Some("restore (put back)"), Some("win.restore"));
            }
            edit_items.append(Some("empty trash"), Some("win.empty-trash"));
        }
        if targets
            .iter()
            .any(|e| !e.is_dir() && vector_core::archive::is_archive_name(&e.path))
            || pane.in_archive.get()
        {
            edit_items.append(Some("extract here"), Some("win.extract"));
            if !pane.in_archive.get() {
                edit_items.append(
                    Some("extract, archive to trash"),
                    Some("win.extract-replace"),
                );
            }
        }
        menu.append_section(None, &edit_items);
        let more = gio::Menu::new();
        if targets.iter().any(|e| e.is_dir()) {
            more.append(Some("folder size"), Some("win.size"));
        }
        more.append(Some("copy path"), Some("win.copy-path"));
        more.append(Some("new folder"), Some("win.new-folder"));
        more.append(Some("new file"), Some("win.new-file"));
        more.append(Some("open terminal here"), Some("win.terminal"));
        more.append(Some("edit names in editor"), Some("win.edit"));
        menu.append_section(None, &more);
        // "show only…": filters of the folder, in one submenu so they read as filters
        let only = gio::Menu::new();
        if let Some(e) = targets.first() {
            let ext = vector_core::entry::extension_of(&e.name).to_lowercase();
            if !e.is_dir() && !ext.is_empty() {
                let item = gio::MenuItem::new(Some(&format!(".{ext} files")), None);
                item.set_action_and_target_value(
                    Some("win.filter"),
                    Some(&format!("ext:{ext}").to_variant()),
                );
                only.append_item(&item);
            }
            if let Some(class) = vector_core::filter::Class::of(e) {
                let label = match class.name() {
                    "dir" => "folders".to_string(),
                    "archive" => "archives (zip, tar…)".to_string(),
                    n => format!("{n} files"),
                };
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("win.filter"),
                    Some(&format!("type:{}", class.name()).to_variant()),
                );
                only.append_item(&item);
            }
        }
        let view = gio::Menu::new();
        if only.n_items() > 0 {
            view.append_submenu(Some("show only"), &only);
        }
        if !pane.model.borrow().options().filter.is_empty() {
            let item = gio::MenuItem::new(Some("show everything (clear filter)"), None);
            item.set_action_and_target_value(Some("win.filter"), Some(&String::new().to_variant()));
            view.append_item(&item);
        }
        let flat = gio::MenuItem::new(
            Some(if pane.is_flat() {
                "back to folder view"
            } else {
                "list every file below (flat)"
            }),
            None,
        );
        flat.set_action_and_target_value(Some("win.flat"), None);
        view.append_item(&flat);
        if has_items {
            view.append(Some("pin to the top of this folder"), Some("win.pin-here"));
            view.append(Some("pin to the sidebar"), Some("win.pin-global"));
        }
        menu.append_section(None, &view);
        if has_items {
            let danger = gio::Menu::new();
            danger.append(Some("move to trash"), Some("win.trash"));
            danger.append(Some("delete permanently"), Some("win.delete"));
            menu.append_section(None, &danger);
        }
        let Some(root) = self.window.child() else {
            return;
        };
        let pt = w
            .compute_point(&root, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or(gtk::graphene::Point::new(x as f32, y as f32));
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_has_arrow(false);
        pop.set_parent(&root);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(
            pt.x() as i32,
            pt.y() as i32,
            1,
            1,
        )));
        pop.set_halign(gtk::Align::Start);
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }
}

/// What a dialog button does; gets the entry's text when the dialog has one.
type DialogAction = Box<dyn Fn(&Win, Option<String>)>;

pub struct Button {
    label: String,
    class: &'static str,
    run: DialogAction,
}

fn button(
    label: &str,
    class: &'static str,
    run: impl Fn(&Win, Option<String>) + 'static,
) -> Button {
    Button {
        label: label.into(),
        class,
        run: Box::new(run),
    }
}

fn cancel() -> Button {
    button("cancel", "", |_, _| {})
}

fn to_key(keyval: gdk::Key, state: gdk::ModifierType) -> Option<Key> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = state.contains(gdk::ModifierType::ALT_MASK);
    if alt {
        return None;
    }
    use gdk::Key as K;
    let k = match keyval {
        K::Return | K::KP_Enter => Key::Enter,
        K::Escape => Key::Esc,
        K::Tab => Key::Tab,
        K::ISO_Left_Tab => Key::BackTab,
        K::BackSpace => Key::Backspace,
        K::Delete if shift => Key::ShiftDelete,
        K::Delete => Key::Delete,
        // shift+arrows: let the list do range selection
        K::Left if ctrl && !shift => Key::CtrlLeft,
        K::Right if ctrl && !shift => Key::CtrlRight,
        K::Up | K::Down | K::Left | K::Right | K::Home | K::End if shift || ctrl => return None,
        K::Up => Key::Up,
        K::Down => Key::Down,
        K::Left => Key::Left,
        K::Right => Key::Right,
        K::Home => Key::Home,
        K::End => Key::End,
        K::Page_Up => Key::PageUp,
        K::Page_Down => Key::PageDown,
        K::F1 => Key::F(1),
        K::F2 => Key::F(2),
        K::F3 => Key::F(3),
        K::F5 => Key::F(5),
        _ => {
            let c = keyval.to_unicode()?;
            if ctrl {
                Key::Ctrl(c.to_ascii_lowercase())
            } else if c.is_control() {
                return None;
            } else {
                Key::Char(c)
            }
        }
    };
    Some(k)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn count_label(paths: &[PathBuf]) -> String {
    match paths {
        [one] => file_name(one),
        _ => format!("{} items", paths.len()),
    }
}

/// `$XDG_RUNTIME_DIR/vector/<pid>`: the folder this window shows, for scripts like
/// terminal-cwd (open a terminal where vector is).
fn cwd_file() -> PathBuf {
    glib::user_runtime_dir()
        .join(vector_core::APP_NAME)
        .join(std::process::id().to_string())
}

/// Removes this process's folder file (on exit).
pub fn forget_cwd() {
    let _ = std::fs::remove_file(cwd_file());
}

fn publish_cwd(dir: &Path) {
    let f = cwd_file();
    if let Some(parent) = f.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // tmpfs, a few bytes: cheap enough to do on navigation
    let _ = std::fs::write(&f, dir.as_os_str().as_encoded_bytes());
}

/// `~/x` for paths under home.
pub fn display_path(p: &Path) -> String {
    let home = glib::home_dir();
    match p.strip_prefix(&home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

/// Resolves a typed path against `cwd`, with `~` for home.
fn resolve(cwd: &Path, typed: &str) -> PathBuf {
    let home = glib::home_dir();
    let p = if typed == "~" {
        home
    } else if let Some(rest) = typed.strip_prefix("~/") {
        home.join(rest)
    } else {
        cwd.join(typed)
    };
    // normalise "a/../b" without touching the file system
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The statusline's coloured blocks and the slanted edges between them.
struct Segments {
    status: gtk::Box,
    after_mode: gtk::DrawingArea,
    branch: gtk::Label,
    after_branch: gtk::DrawingArea,
    before_sort: gtk::DrawingArea,
    sort: gtk::Label,
    before_pos: gtk::DrawingArea,
    pos: gtk::Label,
}

impl Segments {
    fn new(status: &gtk::Box) -> Segments {
        let seg = |classes: &[&str]| {
            let l = gtk::Label::new(None);
            for c in classes {
                l.add_css_class(c);
            }
            l
        };
        Segments {
            status: status.clone(),
            after_mode: crate::slant::slant(false, &["from-mode"]),
            branch: seg(&["seg"]),
            after_branch: crate::slant::slant(false, &["from-seg"]),
            before_sort: crate::slant::slant(true, &["to-seg"]),
            sort: seg(&["seg"]),
            before_pos: crate::slant::slant(true, &["to-mode", "on-seg"]),
            pos: seg(&["zone"]),
        }
    }

    /// `m-normal`, `m-visual`…: the colour of the mode block and the position block.
    fn set_mode(&self, class: &str) {
        for c in [
            "m-normal",
            "m-visual",
            "m-command",
            "m-filter",
            "m-sidebar",
            "m-rename",
        ] {
            if c != class {
                self.status.remove_css_class(c);
            }
        }
        self.status.add_css_class(class);
        self.after_mode.queue_draw();
        self.before_pos.queue_draw();
    }

    fn set_branch(&self, branch: Option<&str>) {
        self.branch
            .set_text(&branch.map(|b| format!("\u{E725} {b}")).unwrap_or_default());
        self.branch.set_visible(branch.is_some());
        self.after_branch.set_visible(branch.is_some());
        // the mode block slants into the branch block, or straight into the bar
        if branch.is_some() {
            self.after_mode.add_css_class("on-seg");
        } else {
            self.after_mode.remove_css_class("on-seg");
        }
    }
}

fn frecency_path() -> PathBuf {
    // $XDG_STATE_HOME, by the XDG base directory spec
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"))
        .join(vector_core::APP_NAME)
        .join("frecency")
}

fn load_frecency() -> vector_core::frecency::Frecency {
    vector_core::frecency::Frecency::parse(
        &std::fs::read_to_string(frecency_path()).unwrap_or_default(),
    )
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}
