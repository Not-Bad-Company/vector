//! The terminal panel (F4): the user's shell in a VTE terminal under the file list, like
//! Dolphin's. It starts in the folder shown and follows it while the shell is idle.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use vte4::prelude::*;

pub struct TermPanel {
    pub widget: gtk::Box,
    term: vte4::Terminal,
    shell_pid: Cell<Option<i32>>,
    /// The folder the shell was last sent to (or started in).
    at: RefCell<PathBuf>,
}

/// `colorN` keys of the Omarchy theme file (or vector's theme.toml), for the 16 colours.
fn palette() -> Vec<gdk::RGBA> {
    let home = glib::home_dir();
    let files = [
        glib::user_config_dir()
            .join(vector_core::APP_NAME)
            .join("theme.toml"),
        home.join(".config/omarchy/current/theme/colors.toml"),
    ];
    let text = files
        .iter()
        .find_map(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default();
    let get = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = l.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
        })
    };
    (0..16)
        .map_while(|i| get(&format!("color{i}")).and_then(|c| gdk::RGBA::parse(c).ok()))
        .collect()
}

impl TermPanel {
    pub fn new(theme: &vector_core::theme::Theme) -> Rc<TermPanel> {
        let term = vte4::Terminal::new();
        term.set_vexpand(true);
        term.set_hexpand(true);
        term.set_scrollback_lines(10_000);
        term.set_font(Some(&gtk::pango::FontDescription::from_string(
            "JetBrainsMonoNL Nerd Font,JetBrainsMono Nerd Font,monospace 11",
        )));
        let fg = gdk::RGBA::parse(&theme.foreground).ok();
        // see-through: the window's own background shows, so the panel blends in
        let bg = Some(gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
        let pal = palette();
        let refs: Vec<&gdk::RGBA> = pal.iter().collect();
        term.set_colors(
            fg.as_ref(),
            bg.as_ref(),
            if refs.len() == 16 { &refs } else { &[] },
        );
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("termpanel");
        widget.append(&term);
        let panel = Rc::new(TermPanel {
            widget,
            term,
            shell_pid: Cell::new(None),
            at: RefCell::default(),
        });
        let weak = Rc::downgrade(&panel);
        panel.term.connect_child_exited(move |_, _| {
            // `exit` in the shell: the next F4 starts a fresh one
            if let Some(p) = weak.upgrade() {
                p.shell_pid.set(None);
                p.widget.set_visible(false);
            }
        });
        panel
    }

    pub fn is_open(&self) -> bool {
        self.widget.is_visible()
    }

    pub fn has_focus(&self) -> bool {
        self.term.has_focus()
    }

    pub fn focus(&self) {
        self.term.grab_focus();
    }

    /// Shows the panel, starting the shell in `dir` the first time.
    pub fn open(self: &Rc<Self>, dir: &Path) {
        self.widget.set_visible(true);
        if self.shell_pid.get().is_none() {
            self.spawn(dir);
        } else {
            self.follow(dir);
        }
    }

    pub fn close(&self) {
        self.widget.set_visible(false);
    }

    fn spawn(self: &Rc<Self>, dir: &Path) {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".into());
        *self.at.borrow_mut() = dir.to_path_buf();
        let weak = Rc::downgrade(self);
        self.term.spawn_async(
            vte4::PtyFlags::DEFAULT,
            dir.to_str(),
            &[&shell],
            &[],
            glib::SpawnFlags::DEFAULT,
            || {},
            -1,
            gio::Cancellable::NONE,
            move |res| {
                if let (Some(p), Ok(pid)) = (weak.upgrade(), res) {
                    p.shell_pid.set(Some(pid.0));
                }
            },
        );
    }

    /// Sends the shell to `dir`, but only while it waits at its prompt: a running program
    /// (an editor, a build) never gets typed into.
    pub fn follow(&self, dir: &Path) {
        if !self.is_open() || *self.at.borrow() == dir {
            return;
        }
        let Some(pid) = self.shell_pid.get() else {
            return;
        };
        if !shell_idle(pid) {
            return;
        }
        let Some(quoted) = dir.to_str().map(|d| d.replace('\'', "'\\''")) else {
            return;
        };
        // the leading space keeps the cd out of shell history (HIST_IGNORE_SPACE, ignorespace)
        self.term
            .feed_child(format!(" cd -- '{quoted}'\n").as_bytes());
        *self.at.borrow_mut() = dir.to_path_buf();
    }

    /// Types a command line into the shell (e.g. starting an agent).
    pub fn run(&self, line: &str) {
        self.term.feed_child(format!("{line}\n").as_bytes());
    }
}

/// Whether the shell is in the foreground of its terminal (nothing else running).
fn shell_idle(pid: i32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    // fields after the ")" of the command name: state ppid pgrp session tty_nr tpgid …
    let Some(rest) = stat.rsplit_once(')').map(|(_, r)| r) else {
        return false;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    fields.get(5).and_then(|t| t.parse::<i32>().ok()) == Some(pid)
}
