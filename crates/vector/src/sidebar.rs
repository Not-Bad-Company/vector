//! Sidebar: places, mounted drives and volumes from GIO, GTK bookmarks.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::prelude::*;
use gtk::{gio, glib};
use vector_core::bookmarks;

pub struct Sidebar {
    widget: gtk::Widget,
    content: gtk::Box,
    rows: RefCell<Vec<(PathBuf, gtk::Widget)>>,
    places: gtk::Box,
    devices: gtk::Box,
    bookmark_rows: gtk::Box,
    monitor: gio::VolumeMonitor,
    bookmarks_monitor: RefCell<Option<gio::FileMonitor>>,
    sections: RefCell<Vec<(gtk::Label, gtk::Box)>>,
    current: RefCell<PathBuf>,
    on_open: Rc<dyn Fn(PathBuf)>,
}

impl Sidebar {
    pub fn new(on_open: impl Fn(PathBuf) + 'static) -> Rc<Sidebar> {
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
            monitor: gio::VolumeMonitor::get(),
            bookmarks_monitor: RefCell::new(None),
            sections: RefCell::new(Vec::new()),
            current: RefCell::new(PathBuf::new()),
            on_open: Rc::new(on_open),
        });
        sidebar.populate_places();
        let content = sidebar.content.clone();
        sidebar.add_section(&content, "places", &sidebar.places);
        sidebar.rebuild_devices();
        sidebar.add_section(&content, "devices", &sidebar.devices);
        sidebar.add_section(&content, "bookmarks", &sidebar.bookmark_rows);
        sidebar.load_bookmarks();
        sidebar.watch_volumes();
        sidebar.watch_bookmarks();
        sidebar
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.widget
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
        if let Some(home) = glib::home_dir().to_str() {
            self.add_row(&self.places, PathBuf::from(home), "Home", "\u{F02DC}", None);
        }
        for (dir, name) in [
            (glib::UserDirectory::Desktop, "Desktop"),
            (glib::UserDirectory::Documents, "Documents"),
            (glib::UserDirectory::Downloads, "Downloads"),
            (glib::UserDirectory::Music, "Music"),
            (glib::UserDirectory::Pictures, "Pictures"),
            (glib::UserDirectory::Videos, "Videos"),
        ] {
            if let Some(path) = glib::user_special_dir(dir)
                && path.exists()
            {
                self.add_row(
                    &self.places,
                    path,
                    name,
                    vector_core::icons::icon_for(name, true, false),
                    None,
                );
            }
        }
        self.add_row(&self.places, PathBuf::from("/"), "Root", "\u{F02CA}", None);
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
        let icon = gtk::Label::new(Some(glyph));
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
        let click = gtk::GestureClick::new();
        let callback = self.on_open.clone();
        let target = path.clone();
        click.connect_released(move |_, _, _, _| callback(target.clone()));
        row.add_controller(click);
        if *self.current.borrow() == path {
            row.add_css_class("active");
        }
        parent.append(&row);
        self.rows.borrow_mut().push((path, row.upcast()));
    }

    fn rebuild_devices(&self) {
        while let Some(child) = self.devices.first_child() {
            self.devices.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().as_ref() != Some(self.devices.upcast_ref()));
        for mount in self.monitor.mounts() {
            if let Some(path) = mount.root().path() {
                let action =
                    (mount.can_unmount() || mount.can_eject()).then(|| Eject::Mount(mount.clone()));
                self.add_row(&self.devices, path, &mount.name(), "\u{F02CA}", action);
            }
        }
        for volume in self.monitor.volumes() {
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
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    let volume = volume_mount.clone();
                    let callback = callback.clone();
                    let mounted_volume = volume.clone();
                    volume.mount(
                        gio::MountMountFlags::NONE,
                        None::<&gio::MountOperation>,
                        None::<&gio::Cancellable>,
                        move |result| {
                            if result.is_ok()
                                && let Some(path) =
                                    mounted_volume.get_mount().and_then(|m| m.root().path())
                            {
                                callback(path);
                            }
                        },
                    );
                });
                row.add_controller(click);
                self.devices.append(&row);
            }
        }
        self.update_sections();
    }

    fn load_bookmarks(self: &Rc<Self>) {
        let this = self.clone();
        let file = gio::File::for_path(bookmarks::default_path());
        glib::MainContext::default().spawn_local(async move {
            if let Ok((bytes, _)) = file.load_contents_future().await {
                let text = String::from_utf8_lossy(&bytes);
                this.show_bookmarks(&text);
            }
        });
    }

    fn show_bookmarks(&self, text: &str) {
        while let Some(child) = self.bookmark_rows.first_child() {
            self.bookmark_rows.remove(&child);
        }
        self.rows
            .borrow_mut()
            .retain(|(_, row)| row.parent().as_ref() != Some(self.bookmark_rows.upcast_ref()));
        for bookmark in bookmarks::parse(text) {
            self.add_row(
                &self.bookmark_rows,
                bookmark.path,
                &bookmark.label,
                vector_core::icons::icon_for(&bookmark.label, true, false),
                None,
            );
        }
        self.update_sections();
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
