//! The `?` panel: the app's help in tabs, with a search over all of it. Shared by
//! vector-find and vector-view (vector-view includes this file); the text is in
//! `vector_core::help`.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use vector_core::help::{Topic, search};

pub struct HelpPanel {
    pub root: gtk::Box,
    /// Typing goes here; the app keeps it focused while the panel is open.
    pub search: gtk::Entry,
    tabs: gtk::Box,
    body: gtk::Box,
    scroll: gtk::ScrolledWindow,
    topics: &'static [Topic],
    tab: Cell<usize>,
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

impl HelpPanel {
    /// `examples`: words to suggest in the empty search box.
    pub fn new(app_name: &str, topics: &'static [Topic], examples: &str) -> Rc<HelpPanel> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("card");
        root.add_css_class("help-panel");
        root.set_halign(gtk::Align::Center);
        root.set_valign(gtk::Align::Center);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        head.add_css_class("head");
        head.append(&label(&format!("{app_name} help"), &["title"]));
        let search = gtk::Entry::new();
        search.set_placeholder_text(Some(&format!("search: {examples}…")));
        search.set_hexpand(true);
        head.append(&search);
        root.append(&head);

        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("help-tabs");
        root.append(&tabs);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.add_css_class("body");
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&body));
        root.append(&scroll);

        let foot = label(
            "type to search · tab next topic · esc closes",
            &["faint", "foot"],
        );
        root.append(&foot);

        let panel = Rc::new(HelpPanel {
            root,
            search,
            tabs,
            body,
            scroll,
            topics,
            tab: Cell::new(0),
        });
        for (i, t) in topics.iter().enumerate() {
            let b = gtk::Button::with_label(t.title);
            b.add_css_class("link");
            b.set_focus_on_click(false);
            let weak = Rc::downgrade(&panel);
            b.connect_clicked(move |_| {
                if let Some(p) = weak.upgrade() {
                    p.search.set_text("");
                    p.show_tab(i);
                    p.search.grab_focus();
                }
            });
            panel.tabs.append(&b);
        }
        let weak = Rc::downgrade(&panel);
        panel.search.connect_changed(move |_| {
            if let Some(p) = weak.upgrade() {
                p.render();
            }
        });
        panel.show_tab(0);
        panel
    }

    /// Index of the tab called `title` (0 when there is none).
    pub fn tab_named(&self, title: &str) -> usize {
        self.topics
            .iter()
            .position(|t| t.title == title)
            .unwrap_or(0)
    }

    /// Opens on `tab` with an empty search, ready for typing.
    pub fn open(&self, tab: usize) {
        self.search.set_text("");
        self.show_tab(tab);
        // big enough to read, smaller than the window
        if let Some(w) = self
            .root
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
        {
            let (ww, wh) = (w.width(), w.height());
            self.root
                .set_size_request((ww - 160).clamp(560, 980), (wh - 140).clamp(360, 760));
        }
        self.search.grab_focus();
    }

    /// The next (or previous) tab.
    pub fn step_tab(&self, back: bool) {
        let n = self.topics.len();
        let i = if back {
            (self.tab.get() + n - 1) % n
        } else {
            (self.tab.get() + 1) % n
        };
        self.search.set_text("");
        self.show_tab(i);
    }

    fn show_tab(&self, i: usize) {
        self.tab.set(i.min(self.topics.len().saturating_sub(1)));
        self.render();
    }

    fn render(&self) {
        while let Some(c) = self.body.first_child() {
            self.body.remove(&c);
        }
        let query = self.search.text();
        let searching = !query.trim().is_empty();
        let mut child = self.tabs.first_child();
        let mut i = 0;
        while let Some(c) = child {
            if i == self.tab.get() && !searching {
                c.add_css_class("primary");
            } else {
                c.remove_css_class("primary");
            }
            child = c.next_sibling();
            i += 1;
        }
        if searching {
            let hits = search(self.topics, &query);
            if hits.is_empty() {
                self.body.append(&label(
                    &format!("nothing about \"{}\"", query.trim()),
                    &["dim"],
                ));
            }
            let mut last = usize::MAX;
            let mut grid = gtk::Grid::new();
            for (t, l) in hits {
                if t != last {
                    let h = label(self.topics[t].title, &["help-section"]);
                    self.body.append(&h);
                    grid = new_grid();
                    self.body.append(&grid);
                    last = t;
                }
                add_line(&grid, self.topics[t].lines[l], grid_rows(&grid));
            }
        } else {
            let topic = &self.topics[self.tab.get()];
            let intro = label(topic.intro, &["help-intro"]);
            intro.set_wrap(true);
            self.body.append(&intro);
            let grid = new_grid();
            self.body.append(&grid);
            for (r, line) in topic.lines.iter().enumerate() {
                add_line(&grid, *line, r as i32);
            }
        }
        self.scroll.vadjustment().set_value(0.0);
    }
}

fn new_grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_column_spacing(24);
    g.set_row_spacing(6);
    g
}

fn grid_rows(g: &gtk::Grid) -> i32 {
    let mut n = 0;
    let mut c = g.first_child();
    while let Some(w) = c {
        n += 1;
        c = w.next_sibling();
    }
    n / 2
}

fn add_line(grid: &gtk::Grid, (keys, text): (&str, &str), row: i32) {
    if keys.is_empty() {
        // a sub-heading
        let h = label(text, &["dim", "help-sub"]);
        grid.attach(&h, 0, row, 2, 1);
        // keeps the two-per-row count of grid_rows
        grid.attach(&gtk::Label::new(None), 2, row, 1, 1);
        return;
    }
    let k = label(keys, &["ok", "help-keys"]);
    k.set_valign(gtk::Align::Start);
    k.set_max_width_chars(28);
    k.set_wrap(true);
    let t = label(text, &[]);
    t.set_wrap(true);
    t.set_hexpand(true);
    grid.attach(&k, 0, row, 1, 1);
    grid.attach(&t, 1, row, 1, 1);
}
