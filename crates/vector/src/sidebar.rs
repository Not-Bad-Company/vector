//! Sidebar: places (the standard folders, then the user's own from the GTK bookmarks, the
//! first nine numbered for `g1`…`g9`), recent folders, mounted drives and volumes, pins.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::prelude::*;
use gtk::{gio, glib};
use vector_core::bookmarks;

/// What the sidebar asks the window to do.
pub enum SidebarEvent {
    Open(PathBuf),
    OpenTab(PathBuf),
    Unpin(PathBuf),
    /// Take a folder off the recent list.
    Forget(PathBuf),
    Notify(String),
}

/// What a sidebar menu entry does with its row's path.
type RowAction = Box<dyn Fn(&Sidebar, PathBuf)>;

/// Which section a row is in, for its right-click menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Places,
    Devices,
    Pinned,
    Bookmarks,
    Recent,
}

pub struct Sidebar {
    widget: gtk::Widget,
    content: gtk::Box,
    rows: RefCell<Vec<(PathBuf, gtk::Widget)>>,
    places: gtk::Box,
    devices: gtk::Box,
    bookmark_rows: gtk::Box,
    pinned_rows: gtk::Box,
    recent_rows: gtk::Box,
    /// Standard places the user took off the sidebar.
    hidden: RefCell<Vec<PathBuf>>,
    monitor: gio::VolumeMonitor,
    bookmarks_monitor: RefCell<Option<gio::FileMonitor>>,
    sections: RefCell<Vec<(gtk::Label, gtk::Box)>>,
    current: RefCell<PathBuf>,
    on_open: Rc<dyn Fn(PathBuf)>,
    on_event: Rc<dyn Fn(SidebarEvent)>,
    /// Free and total bytes of mounted drives, filled in the background, for tooltips.
    space: RefCell<std::collections::HashMap<PathBuf, (u64, u64)>>,
    weak: RefCell<std::rc::Weak<Sidebar>>,
}

impl Sidebar {
    pub fn new(on_event: impl Fn(SidebarEvent) + 'static) -> Rc<Sidebar> {
        let on_event: Rc<dyn Fn(SidebarEvent)> = Rc::new(on_event);
        let ev = on_event.clone();
        let on_open = move |p: PathBuf| ev(SidebarEvent::Open(p));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.add_css_class("places");
        content.set_width_request(200);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_child(Some(&content));
        scroll.add_css_class("sidebar-scroll");
        // rows expand to the width they get; the sidebar itself keeps its width
        scroll.set_hexpand(false);
        scroll.set_propagate_natural_width(true);

        let sidebar = Rc::new(Sidebar {
            widget: scroll.upcast(),
            content,
            rows: RefCell::new(Vec::new()),
            places: gtk::Box::new(gtk::Orientation::Vertical, 0),
            devices: gtk::Box::new(gtk::Orientation::Vertical, 0),
            bookmark_rows: gtk::Box::new(gtk::Orientation::Vertical, 0),
            pinned_rows: gtk::Box::new(gtk::Orientation::Vertical, 0),
            recent_rows: gtk::Box::new(gtk::Orientation::Vertical, 0),
            hidden: RefCell::new(load_hidden()),
            monitor: gio::VolumeMonitor::get(),
            bookmarks_monitor: RefCell::new(None),
            sections: RefCell::new(Vec::new()),
            current: RefCell::new(PathBuf::new()),
            on_open: Rc::new(on_open),
            on_event,
            space: RefCell::default(),
            weak: RefCell::default(),
        });
        *sidebar.weak.borrow_mut() = Rc::downgrade(&sidebar);
        sidebar.connect_menu();
        sidebar.populate_places();
        sidebar.number_places();
        let content = sidebar.content.clone();
        // one list: the standard places, then the user's own
        let all = gtk::Box::new(gtk::Orientation::Vertical, 0);
        all.append(&sidebar.places);
        all.append(&sidebar.bookmark_rows);
        sidebar.add_section(&content, "places", &all);
        sidebar.add_section(&content, "recent", &sidebar.recent_rows);
        sidebar.rebuild_devices();
        sidebar.add_section(&content, "devices", &sidebar.devices);
        sidebar.add_section(&content, "pinned", &sidebar.pinned_rows);
        sidebar.load_bookmarks();
        sidebar.watch_volumes();
        sidebar.watch_bookmarks();
        sidebar
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
    }

    /// Shows the globally pinned items (files open in their folder, selected).
    pub fn set_pinned(&self, items: &[PathBuf]) {
        while let Some(child) = self.pinned_rows.first_child() {
            self.pinned_rows.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().is_some());
        for p in items {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            // no stat here (a pin may sit on a slow drive): folders are told apart by name only
            let looks_dir = vector_core::entry::extension_of(&name).is_empty();
            let glyph = vector_core::icons::icon_for(&name, looks_dir, false);
            self.add_row(&self.pinned_rows, p.clone(), &name, glyph, None);
            if let Some(row) = self.pinned_rows.last_child() {
                row.set_tooltip_text(Some(&p.display().to_string()));
            }
        }
        self.update_sections();
    }

    /// Shows the most used folders under "recent".
    pub fn set_recent(&self, dirs: &[PathBuf]) {
        while let Some(child) = self.recent_rows.first_child() {
            self.recent_rows.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().is_some());
        for p in dirs {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            let glyph = vector_core::icons::icon_for(&name, true, false);
            self.add_row(&self.recent_rows, p.clone(), &name, glyph, None);
        }
        self.update_sections();
    }

    /// Whether `p` is listed under places (standard or the user's own).
    pub fn is_place(&self, p: &Path) -> bool {
        self.place_rows().iter().any(|(r, _)| r == p)
    }

    /// The place numbered `n` (1-based), for `g1`…`g9`.
    pub fn place(&self, n: usize) -> Option<PathBuf> {
        self.place_rows()
            .into_iter()
            .nth(n.checked_sub(1)?)
            .map(|(p, _)| p)
    }

    fn place_rows(&self) -> Vec<(PathBuf, gtk::Widget)> {
        let parents = [
            self.places.upcast_ref::<gtk::Widget>().clone(),
            self.bookmark_rows.clone().upcast(),
        ];
        let rows = self.rows.borrow();
        let mut out = Vec::new();
        for parent in parents {
            let mut c = parent.first_child();
            while let Some(w) = c {
                if let Some((p, _)) = rows.iter().find(|(_, r)| *r == w) {
                    out.push((p.clone(), w.clone()));
                }
                c = w.next_sibling();
            }
        }
        out
    }

    /// Puts 1…9 next to the first nine places.
    fn number_places(&self) {
        for (i, (_, row)) in self.place_rows().iter().enumerate() {
            if let Some(num) = row.last_child().and_downcast::<gtk::Label>()
                && num.has_css_class("place-num")
            {
                num.set_text(&if i < 9 {
                    (i + 1).to_string()
                } else {
                    String::new()
                });
            }
        }
    }

    /// Whether `p` is one of the user's own places.
    pub fn is_bookmark(&self, p: &Path) -> bool {
        let rows = self.rows.borrow();
        rows.iter()
            .any(|(r, w)| r == p && w.parent().as_ref() == Some(self.bookmark_rows.upcast_ref()))
    }

    /// `b`: adds `p` to the places, or takes it off if it is one of the user's own.
    pub fn toggle_bookmark(&self, p: &Path) {
        if self.is_bookmark(p) {
            self.edit_bookmarks(|t| bookmarks::remove(t, p), "taken off places");
        } else if self.is_place(p) {
            (self.on_event)(SidebarEvent::Notify("already in places".into()));
        } else {
            self.edit_bookmarks(
                |t| bookmarks::add(t, p, None),
                "added to places (g1…g9 jump)",
            );
        }
    }

    /// Moves one of the user's places up or down. Returns whether it moved.
    pub fn shift_place(&self, p: &Path, up: bool) -> bool {
        if !self.is_bookmark(p) {
            (self.on_event)(SidebarEvent::Notify(
                "only your own places move (b adds one)".into(),
            ));
            return false;
        }
        let path = bookmarks::default_path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let moved = bookmarks::shift(&text, p, up) != text;
        if moved {
            self.edit_bookmarks(|t| bookmarks::shift(t, p, up), "moved");
        }
        moved
    }

    /// Your own places can be dragged onto each other to reorder them.
    fn make_draggable(&self, row: &gtk::Box, path: &Path) {
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        let value = format!("{PLACE_DRAG}{}", path.display());
        drag.connect_prepare(move |_, _, _| {
            Some(gtk::gdk::ContentProvider::for_value(&value.to_value()))
        });
        let r = row.downgrade();
        drag.connect_drag_begin(move |src, _| {
            if let Some(r) = r.upgrade() {
                src.set_icon(Some(&gtk::WidgetPaintable::new(Some(&r))), 0, 0);
            }
        });
        row.add_controller(drag);

        let drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let target = path.to_path_buf();
        let weak = self.weak.borrow().clone();
        drop.connect_drop(move |d, value, _, _| {
            if let Some(w) = d.widget() {
                w.remove_css_class("drop-target");
            }
            let (Some(s), Ok(v)) = (weak.upgrade(), value.get::<String>()) else {
                return false;
            };
            let Some(from) = v.strip_prefix(PLACE_DRAG).map(PathBuf::from) else {
                return false;
            };
            s.edit_bookmarks(|t| bookmarks::move_to(t, &from, &target), "moved");
            true
        });
        drop.connect_enter(|d, _, _| {
            if let Some(w) = d.widget() {
                w.add_css_class("drop-target");
            }
            gtk::gdk::DragAction::MOVE
        });
        drop.connect_leave(|d| {
            if let Some(w) = d.widget() {
                w.remove_css_class("drop-target");
            }
        });
        row.add_controller(drop);
    }

    /// Takes a standard place off the sidebar (`:places reset` brings it back).
    fn hide_place(&self, p: PathBuf) {
        self.hidden.borrow_mut().push(p);
        self.save_hidden("hidden from places · :places reset shows it again");
    }

    pub fn reset_hidden(&self) {
        self.hidden.borrow_mut().clear();
        self.save_hidden("all places shown");
    }

    fn save_hidden(&self, done: &str) {
        let text: String = self
            .hidden
            .borrow()
            .iter()
            .map(|p| format!("{}\n", p.display()))
            .collect();
        let file = hidden_path();
        if let Some(d) = file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&file, text) {
            (self.on_event)(SidebarEvent::Notify(format!("places: {e}")));
            return;
        }
        while let Some(child) = self.places.first_child() {
            self.places.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().is_some());
        self.populate_places();
        self.number_places();
        let cur = self.current.borrow().clone();
        self.set_current(&cur);
        (self.on_event)(SidebarEvent::Notify(done.to_string()));
    }

    pub fn set_current(&self, path: &Path) {
        *self.current.borrow_mut() = path.to_path_buf();
        for (row_path, row) in self.rows.borrow().iter() {
            if row_path == path {
                row.add_css_class("active");
            } else {
                row.remove_css_class("active");
            }
        }
    }

    fn add_section(&self, parent: &gtk::Box, title: &str, rows: &gtk::Box) {
        let label = gtk::Label::new(Some(title));
        label.add_css_class("section");
        label.set_xalign(0.0);
        parent.append(&label);
        parent.append(rows);
        self.sections.borrow_mut().push((label, rows.clone()));
        self.update_sections();
    }

    /// Hides sections without rows (shown again when e.g. a drive is plugged in).
    fn update_sections(&self) {
        for (label, rows) in self.sections.borrow().iter() {
            let any = rows.first_child().is_some();
            label.set_visible(any);
            rows.set_visible(any);
        }
    }

    fn populate_places(&self) {
        let hidden = self.hidden.borrow().clone();
        let add = |path: PathBuf, name: &str, glyph: &str| {
            if !hidden.contains(&path) {
                self.add_row(&self.places, path, name, glyph, None);
            }
        };
        if let Some(home) = glib::home_dir().to_str() {
            add(PathBuf::from(home), "Home", "\u{F02DC}");
        }
        for (dir, name) in [
            (glib::UserDirectory::Desktop, "Desktop"),
            (glib::UserDirectory::Documents, "Documents"),
            (glib::UserDirectory::Downloads, "Downloads"),
            (glib::UserDirectory::Music, "Music"),
            (glib::UserDirectory::Pictures, "Pictures"),
            (glib::UserDirectory::Videos, "Videos"),
        ] {
            // a folder set to home itself (XDG_DESKTOP_DIR=$HOME, common) would be Home twice
            if let Some(path) = glib::user_special_dir(dir)
                && path.exists()
                && path != glib::home_dir()
            {
                add(path, name, vector_core::icons::icon_for(name, true, false));
            }
        }
        add(PathBuf::from("/"), "Root", "\u{F02CA}");
        let trash = vector_core::trash::files_dir();
        if trash.is_dir() {
            add(trash, "Trash", "\u{F0A7A}");
        }
    }

    fn add_row(
        &self,
        parent: &gtk::Box,
        path: PathBuf,
        name: &str,
        glyph: &str,
        eject: Option<Eject>,
    ) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("row");
        row.set_cursor_from_name(Some("pointer"));
        let custom = crate::iconstore::get(&path);
        let icon = gtk::Label::new(Some(custom.as_deref().unwrap_or(glyph)));
        icon.add_css_class("icon");
        row.append(&icon);
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&label);
        if let Some(action) = eject {
            let button = gtk::Button::with_label("\u{F01EA}");
            button.add_css_class("link");
            button.add_css_class("x");
            button.set_tooltip_text(Some("Eject"));
            button.connect_clicked(move |_| action.run());
            row.append(&button);
        }
        if parent == &self.places || parent == &self.bookmark_rows {
            let num = gtk::Label::new(None);
            num.add_css_class("place-num");
            row.append(&num);
        }
        if parent == &self.bookmark_rows {
            self.make_draggable(&row, &path);
        }
        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_PRIMARY)
            .build();
        let callback = self.on_open.clone();
        let target = path.clone();
        if parent == &self.bookmark_rows {
            click.connect_released(move |_, _, _, _| callback(target.clone()));
        } else {
            let r = row.downgrade();
            click.connect_pressed(move |g, _, x, y| {
                if let Some(row) = r.upgrade()
                    && let Some(hit) = row.pick(x, y, gtk::PickFlags::DEFAULT)
                    && (hit.has_css_class("x")
                        || hit.parent().is_some_and(|p| p.has_css_class("x")))
                {
                    return;
                }
                g.set_state(gtk::EventSequenceState::Claimed);
                callback(target.clone());
            });
        }
        row.add_controller(click);
        if *self.current.borrow() == path {
            row.add_css_class("active");
        }
        // hover: the path, the last known folder size, a drive's free space
        row.set_has_tooltip(true);
        let weak = self.weak.borrow().clone();
        let tip_path = path.clone();
        row.connect_query_tooltip(move |_, _, _, _, tip| {
            let mut lines = vec![tip_path.display().to_string()];
            if let Some(n) = crate::pane::known_size(&tip_path) {
                lines.push(format!("{} in total", crate::fmt::size(n)));
            }
            if let Some((free, total)) = weak
                .upgrade()
                .and_then(|s| s.space.borrow().get(&tip_path).copied())
            {
                lines.push(format!(
                    "{} free of {}",
                    crate::fmt::size(free),
                    crate::fmt::size(total)
                ));
            }
            tip.set_text(Some(&lines.join("\n")));
            true
        });
        parent.append(&row);
        self.rows.borrow_mut().push((path, row.upcast()));
    }

    /// Reads free space of every listed drive in the background (for tooltips).
    fn load_space(&self) {
        let paths: Vec<PathBuf> = self.rows.borrow().iter().map(|(p, _)| p.clone()).collect();
        for p in paths {
            let weak = self.weak.borrow().clone();
            glib::spawn_future_local(async move {
                let f = gio::File::for_path(&p);
                if let Ok(info) = f
                    .query_filesystem_info_future(
                        "filesystem::free,filesystem::size",
                        glib::Priority::LOW,
                    )
                    .await
                    && let Some(s) = weak.upgrade()
                {
                    let free = info.attribute_uint64("filesystem::free");
                    let total = info.attribute_uint64("filesystem::size");
                    if total > 0 {
                        s.space.borrow_mut().insert(p, (free, total));
                    }
                }
            });
        }
    }

    fn section_of(&self, row: &gtk::Widget) -> Option<Section> {
        let parent = row.parent()?;
        [
            (&self.places, Section::Places),
            (&self.devices, Section::Devices),
            (&self.pinned_rows, Section::Pinned),
            (&self.bookmark_rows, Section::Bookmarks),
            (&self.recent_rows, Section::Recent),
        ]
        .into_iter()
        .find(|(b, _)| parent == *b.upcast_ref::<gtk::Widget>())
        .map(|(_, s)| s)
    }

    /// Right click: a menu for the row under the pointer, or "bookmark this folder".
    fn connect_menu(self: &Rc<Self>) {
        let actions = gio::SimpleActionGroup::new();
        let weak = Rc::downgrade(self);
        let add = |name: &str, f: RowAction| {
            let a = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let weak = weak.clone();
            a.connect_activate(move |_, v| {
                if let (Some(s), Some(p)) = (weak.upgrade(), v.and_then(|v| v.get::<String>())) {
                    f(&s, PathBuf::from(p));
                }
            });
            actions.add_action(&a);
        };
        add("open", Box::new(|s, p| (s.on_open)(p)));
        add(
            "open-tab",
            Box::new(|s, p| (s.on_event)(SidebarEvent::OpenTab(p))),
        );
        add(
            "unpin",
            Box::new(|s, p| (s.on_event)(SidebarEvent::Unpin(p))),
        );
        add(
            "bookmark",
            Box::new(|s, p| s.edit_bookmarks(|t| bookmarks::add(t, &p, None), "added to places")),
        );
        add(
            "unbookmark",
            Box::new(|s, p| s.edit_bookmarks(|t| bookmarks::remove(t, &p), "taken off places")),
        );
        add("rename", Box::new(|s, p| s.ask_name(p)));
        add("hide", Box::new(|s, p| s.hide_place(p)));
        add(
            "up",
            Box::new(|s, p| {
                s.shift_place(&p, true);
            }),
        );
        add(
            "down",
            Box::new(|s, p| {
                s.shift_place(&p, false);
            }),
        );
        add(
            "forget",
            Box::new(|s, p| (s.on_event)(SidebarEvent::Forget(p))),
        );
        add("icon", Box::new(|s, p| s.ask_icon(p)));
        self.widget.insert_action_group("side", Some(&actions));

        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_SECONDARY)
            .build();
        let weak = Rc::downgrade(self);
        click.connect_pressed(move |g, _, x, y| {
            let Some(s) = weak.upgrade() else { return };
            g.set_state(gtk::EventSequenceState::Claimed);
            let hit = s.content.pick(x, y, gtk::PickFlags::DEFAULT);
            let row = hit.and_then(|w| {
                let mut cur = Some(w);
                while let Some(c) = cur {
                    if c.has_css_class("row") {
                        return Some(c);
                    }
                    cur = c.parent();
                }
                None
            });
            let target = row.as_ref().and_then(|r| {
                let rows = s.rows.borrow();
                rows.iter().find(|(_, w)| w == r).map(|(p, _)| p.clone())
            });
            let menu = gio::Menu::new();
            let item = |label: &str, action: &str, p: &Path| {
                let i = gio::MenuItem::new(Some(label), None);
                i.set_action_and_target_value(
                    Some(&format!("side.{action}")),
                    Some(&p.display().to_string().to_variant()),
                );
                menu.append_item(&i);
            };
            match (row.as_ref().and_then(|r| s.section_of(r)), target) {
                (Some(section), Some(p)) => {
                    item("open in new tab", "open-tab", &p);
                    match section {
                        Section::Bookmarks => {
                            item("rename", "rename", &p);
                            item("change icon", "icon", &p);
                            item("move up", "up", &p);
                            item("move down", "down", &p);
                            item("take off places", "unbookmark", &p);
                        }
                        Section::Places => item("hide from places", "hide", &p),
                        Section::Recent => {
                            item("add to places", "bookmark", &p);
                            item("forget", "forget", &p);
                        }
                        Section::Pinned => {
                            item("change icon", "icon", &p);
                            item("unpin", "unpin", &p);
                        }
                        Section::Devices => item("add to places", "bookmark", &p),
                    }
                }
                _ => {
                    let cur = s.current.borrow().clone();
                    item("add this folder to places", "bookmark", &cur);
                }
            }
            let pop = gtk::PopoverMenu::from_model(Some(&menu));
            pop.set_has_arrow(false);
            pop.set_parent(&s.content);
            pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
        });
        self.content.add_controller(click);
    }

    /// Rewrites the GTK bookmarks file (the file monitor then refreshes the list).
    fn edit_bookmarks(&self, f: impl FnOnce(&str) -> String, done: &str) {
        let path = bookmarks::default_path();
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let new = f(&text);
        if new == text {
            return;
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match std::fs::write(&path, &new) {
            Ok(()) => {
                self.show_bookmarks(&new);
                (self.on_event)(SidebarEvent::Notify(done.to_string()));
            }
            Err(e) => (self.on_event)(SidebarEvent::Notify(format!("bookmarks: {e}"))),
        }
    }

    /// Visible rows in screen order (for keyboard navigation).
    fn visible_rows(&self) -> Vec<(PathBuf, gtk::Widget)> {
        let mut out = Vec::new();
        for b in [
            &self.places,
            &self.bookmark_rows,
            &self.recent_rows,
            &self.devices,
            &self.pinned_rows,
        ] {
            if !b.is_visible() {
                continue;
            }
            let mut c = b.first_child();
            while let Some(w) = c {
                if let Some(p) = self
                    .rows
                    .borrow()
                    .iter()
                    .find(|(_, r)| *r == w)
                    .map(|(p, _)| p.clone())
                {
                    out.push((p, w.clone()));
                }
                c = w.next_sibling();
            }
        }
        out
    }

    /// The place, drive or bookmark called `name` (case-insensitive): `:cd trash`, `:cd nas`.
    pub fn find(&self, name: &str) -> Option<PathBuf> {
        let want = name.trim().to_lowercase();
        self.rows.borrow().iter().find_map(|(p, w)| {
            let label = w
                .first_child()
                .and_then(|i| i.next_sibling())
                .and_downcast::<gtk::Label>()
                .map(|l| l.text().to_lowercase());
            (label.as_deref() == Some(want.as_str())).then(|| p.clone())
        })
    }

    /// The row at keyboard position `i`.
    pub fn cursor_row(&self, i: usize) -> Option<(PathBuf, gtk::Widget)> {
        self.visible_rows().into_iter().nth(i)
    }

    pub fn len(&self) -> usize {
        self.visible_rows().len()
    }

    /// Shows the keyboard cursor on row `i` (None: hide it). Returns the row's path.
    pub fn set_cursor(&self, i: Option<usize>) -> Option<PathBuf> {
        let rows = self.visible_rows();
        let mut picked = None;
        for (n, (p, w)) in rows.iter().enumerate() {
            if Some(n) == i {
                w.add_css_class("cursor");
                picked = Some(p.clone());
            } else {
                w.remove_css_class("cursor");
            }
        }
        picked
    }

    /// Index of the row showing the current folder, to start keyboard navigation there.
    pub fn current_index(&self) -> usize {
        let cur = self.current.borrow().clone();
        self.visible_rows()
            .iter()
            .position(|(p, _)| *p == cur)
            .unwrap_or(0)
    }

    fn row_of(&self, p: &Path) -> Option<gtk::Widget> {
        self.rows
            .borrow()
            .iter()
            .find(|(r, _)| r == p)
            .map(|(_, w)| w.clone())
    }

    /// A small popover with an entry to rename a bookmark.
    fn ask_name(&self, p: PathBuf) {
        let Some(row) = self.row_of(&p) else { return };
        let entry = gtk::Entry::new();
        let current = row
            .first_child()
            .and_then(|i| i.next_sibling())
            .and_downcast::<gtk::Label>()
            .map(|l| l.text().to_string());
        entry.set_text(current.as_deref().unwrap_or(""));
        let pop = gtk::Popover::new();
        pop.set_child(Some(&entry));
        pop.set_has_arrow(false);
        pop.set_parent(&row);
        let weak = self.weak.borrow().clone();
        let pp = pop.clone();
        entry.connect_activate(move |e| {
            if let Some(s) = weak.upgrade() {
                let name = e.text().to_string();
                s.edit_bookmarks(|t| bookmarks::rename(t, &p, &name), "bookmark renamed");
            }
            pp.popdown();
        });
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
        entry.grab_focus();
    }

    /// A grid of icons to pick from.
    fn ask_icon(&self, p: PathBuf) {
        let Some(row) = self.row_of(&p) else { return };
        let grid = gtk::Grid::new();
        grid.set_row_spacing(2);
        grid.set_column_spacing(2);
        let pop = gtk::Popover::new();
        pop.set_has_arrow(false);
        let choices = crate::iconstore::CHOICES
            .iter()
            .map(|g| Some(*g))
            .chain(std::iter::once(None));
        for (i, glyph) in choices.enumerate() {
            let b = gtk::Button::with_label(glyph.unwrap_or("default"));
            b.add_css_class("link");
            let (weak, pp, path) = (self.weak.borrow().clone(), pop.clone(), p.clone());
            b.connect_clicked(move |_| {
                crate::iconstore::set(&path, glyph);
                if let Some(s) = weak.upgrade() {
                    s.refresh_icons();
                }
                pp.popdown();
            });
            let (col, line) = ((i % 8) as i32, (i / 8) as i32);
            if glyph.is_none() {
                grid.attach(&b, 0, line + 1, 8, 1);
            } else {
                grid.attach(&b, col, line, 1, 1);
            }
        }
        pop.set_child(Some(&grid));
        pop.set_parent(&row);
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    /// Re-applies custom icons to every row.
    fn refresh_icons(&self) {
        for (p, row) in self.rows.borrow().iter() {
            if let (Some(g), Some(icon)) = (
                crate::iconstore::get(p),
                row.first_child().and_downcast::<gtk::Label>(),
            ) {
                icon.set_text(&g);
            }
        }
        (self.on_event)(SidebarEvent::Notify("icon changed".into()));
    }

    fn rebuild_devices(&self) {
        while let Some(child) = self.devices.first_child() {
            self.devices.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().is_some());
        // screenshots (VECTOR_DEMO) leave the machine's own drives out
        let hide = std::env::var_os("VECTOR_DEMO").is_some();
        let (mounts, volumes) = if hide {
            (Vec::new(), Vec::new())
        } else {
            (self.monitor.mounts(), self.monitor.volumes())
        };
        for mount in mounts {
            if let Some(path) = mount.root().path() {
                let action =
                    (mount.can_unmount() || mount.can_eject()).then(|| Eject::Mount(mount.clone()));
                self.add_row(&self.devices, path, &mount.name(), "\u{F02CA}", action);
            }
        }
        for volume in volumes {
            if volume.get_mount().is_none() && volume.can_mount() {
                let volume_mount = volume.clone();
                let name = volume.name().to_string();
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.add_css_class("row");
                row.set_cursor_from_name(Some("pointer"));
                let icon = gtk::Label::new(Some("\u{F02CA}"));
                icon.add_css_class("icon");
                row.append(&icon);
                let label = gtk::Label::new(Some(&name));
                label.set_xalign(0.);
                label.set_hexpand(true);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                row.append(&label);
                let callback = self.on_open.clone();
                let click = gtk::GestureClick::builder()
                    .button(gtk::gdk::BUTTON_PRIMARY)
                    .build();
                let mounting = Rc::new(std::cell::Cell::new(false));
                let lbl = label.downgrade();
                click.connect_pressed(move |g, _, _, _| {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(path) = volume_mount.get_mount().and_then(|m| m.root().path()) {
                        callback(path);
                        return;
                    }
                    if mounting.replace(true) {
                        return;
                    }
                    if let Some(l) = lbl.upgrade() {
                        l.set_text(&format!("{name}…"));
                    }
                    let volume = volume_mount.clone();
                    let callback = callback.clone();
                    let mounted_volume = volume.clone();
                    let mounting = mounting.clone();
                    let lbl = lbl.clone();
                    let orig_name = name.clone();
                    volume.mount(
                        gio::MountMountFlags::NONE,
                        None::<&gio::MountOperation>,
                        None::<&gio::Cancellable>,
                        move |result| {
                            mounting.set(false);
                            if let Some(path) =
                                mounted_volume.get_mount().and_then(|m| m.root().path())
                            {
                                callback(path);
                            } else if result.is_err()
                                && let Some(l) = lbl.upgrade()
                            {
                                l.set_text(&orig_name);
                            }
                        },
                    );
                });
                row.add_controller(click);
                self.devices.append(&row);
            }
        }
        self.update_sections();
        self.load_space();
    }

    fn load_bookmarks(self: &Rc<Self>) {
        if let Ok(text) = std::fs::read_to_string(bookmarks::default_path()) {
            self.show_bookmarks(&text);
        }
    }

    fn show_bookmarks(&self, text: &str) {
        // the keyboard cursor stays on its place when the list is redrawn
        let cursor = self
            .rows
            .borrow()
            .iter()
            .find(|(_, w)| w.has_css_class("cursor"))
            .map(|(p, _)| p.clone());
        while let Some(child) = self.bookmark_rows.first_child() {
            self.bookmark_rows.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().is_some());
        for bookmark in bookmarks::parse(text) {
            self.add_row(
                &self.bookmark_rows,
                bookmark.path,
                &bookmark.label,
                vector_core::icons::icon_for(&bookmark.label, true, false),
                None,
            );
        }
        self.number_places();
        self.update_sections();
        if let Some(c) = cursor
            && let Some(row) = self.row_of(&c)
        {
            row.add_css_class("cursor");
        }
    }

    fn watch_bookmarks(self: &Rc<Self>) {
        let file = gio::File::for_path(bookmarks::default_path());
        if let Ok(monitor) =
            file.monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
        {
            let this = self.clone();
            monitor.connect_changed(move |_, _, _, _| {
                let file = gio::File::for_path(bookmarks::default_path());
                let this = this.clone();
                glib::MainContext::default().spawn_local(async move {
                    if let Ok((bytes, _)) = file.load_contents_future().await {
                        this.show_bookmarks(&String::from_utf8_lossy(&bytes));
                    }
                });
            });
            self.bookmarks_monitor.replace(Some(monitor));
        }
    }

    fn watch_volumes(self: &Rc<Self>) {
        for signal in [
            "mount-added",
            "mount-removed",
            "mount-changed",
            "volume-added",
            "volume-removed",
        ] {
            let this = self.clone();
            self.monitor.connect_local(signal, false, move |_| {
                this.rebuild_devices();
                None
            });
        }
    }
}

#[derive(Clone)]
enum Eject {
    Mount(gio::Mount),
}
impl Eject {
    fn run(&self) {
        match self {
            Eject::Mount(mount) if mount.can_eject() => mount.eject_with_operation(
                gio::MountUnmountFlags::NONE,
                None::<&gio::MountOperation>,
                None::<&gio::Cancellable>,
                |_| {},
            ),
            Eject::Mount(mount) => mount.unmount_with_operation(
                gio::MountUnmountFlags::NONE,
                None::<&gio::MountOperation>,
                None::<&gio::Cancellable>,
                |_| {},
            ),
        }
    }
}

/// What a dragged place carries, before its path (so file drops elsewhere ignore it).
const PLACE_DRAG: &str = "vector-place:";

/// The standard places the user hid, one path per line.
fn hidden_path() -> PathBuf {
    glib::user_config_dir()
        .join(vector_core::APP_NAME)
        .join("hidden-places")
}

fn load_hidden() -> Vec<PathBuf> {
    std::fs::read_to_string(hidden_path())
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}
