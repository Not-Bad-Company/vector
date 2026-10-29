//! Clock. Click: calendar panel. Middle: alternate format. Right: time zone.

use std::cell::Cell;
use std::rc::Rc;

use chrono::{Datelike, Duration as CDuration, Local, NaiveDate};
use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::Slot;
use crate::ui;
use crate::util;

pub struct Clock(Rc<Inner>);

struct Inner {
    slot: Slot,
    format: String,
    alt: String,
    use_alt: Cell<bool>,
}

impl Clock {
    pub fn new(ctx: &Ctx) -> Clock {
        let o = ctx.opts("clock");
        let inner = Rc::new(Inner {
            slot: ctx.slot("clock", &[]),
            format: o.str("format", "%a %d %b · %H:%M"),
            alt: o.str("format_alt", "%d %B W%V %Y"),
            use_alt: Cell::new(false),
        });
        let right = o.str(
            "on_right",
            "omarchy-launch-floating-terminal-with-presentation omarchy-tz-select",
        );
        inner.slot.on_right(move || util::spawn_sh(&right));
        let w = Rc::downgrade(&inner);
        inner.slot.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.use_alt.set(!i.use_alt.get());
                i.tick();
            }
        });
        inner.slot.panel(calendar_panel);
        inner.tick();
        // tick on the next second boundary, then every second (cheap; keeps minutes exact)
        util::every(1000, &inner, |i| i.tick());
        Clock(inner)
    }
}

impl Inner {
    fn tick(&self) {
        let f = if self.use_alt.get() {
            &self.alt
        } else {
            &self.format
        };
        self.slot
            .set_markup(&util::esc(&Local::now().format(f).to_string()));
    }
}

impl Module for Clock {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.tick();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

fn calendar_panel(p: &Panel) -> gtk::Widget {
    p.set_width(300);
    let now = Local::now();
    let card = Card::new(&now.format("%A").to_string().to_lowercase(), "");
    let big = ui::text("", &["mono-big"]);
    big.set_margin_start(16);
    let sub = ui::text("", &["dim"]);
    sub.set_margin_start(16);
    sub.set_margin_bottom(6);
    card.body.append(&big);
    card.body.append(&sub);

    let month = Rc::new(Cell::new(
        NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or_default(),
    ));
    let nav = ui::hbox(4, &["chips"]);
    let mlabel = ui::text("", &[]);
    mlabel.set_hexpand(true);
    nav.append(&mlabel);
    let grid_holder = ui::vbox(0, &[]);
    card.body.append(&nav);
    card.body.append(&grid_holder);

    let draw: crate::ui::Redraw = Rc::default();
    let render = {
        let month = month.clone();
        let grid_holder = grid_holder.clone();
        let mlabel = mlabel.clone();
        move || {
            ui::clear(&grid_holder);
            let m = month.get();
            mlabel.set_text(&m.format("%B %Y").to_string().to_lowercase());
            grid_holder.append(&month_grid(m));
        }
    };
    let render: Rc<dyn Fn()> = Rc::new(render);
    *draw.borrow_mut() = Some(render.clone());
    let shift = |delta: i32, month: Rc<Cell<NaiveDate>>, render: Rc<dyn Fn()>| {
        move || {
            let m = month.get();
            let (mut y, mut mo) = (m.year(), m.month() as i32 + delta);
            while mo < 1 {
                mo += 12;
                y -= 1;
            }
            while mo > 12 {
                mo -= 12;
                y += 1;
            }
            month.set(NaiveDate::from_ymd_opt(y, mo as u32, 1).unwrap_or(m));
            render();
        }
    };
    let prev = shift(-1, month.clone(), render.clone());
    let next = shift(1, month.clone(), render.clone());
    nav.append(&ui::icon_button("", "previous month", prev.clone()));
    {
        let month = month.clone();
        let render = render.clone();
        nav.append(&ui::link("today", &[], move || {
            let t = Local::now();
            month.set(NaiveDate::from_ymd_opt(t.year(), t.month(), 1).unwrap_or_default());
            render();
        }));
    }
    nav.append(&ui::icon_button("", "next month", next.clone()));

    // scroll over the grid changes month
    let sc = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    {
        let (prev, next) = (prev.clone(), next.clone());
        sc.connect_scroll(move |_, _, dy| {
            if dy < 0.0 {
                prev()
            } else if dy > 0.0 {
                next()
            }
            glib::Propagation::Stop
        });
    }
    grid_holder.add_controller(sc);
    p.on_key(move |k, _| match k {
        gtk::gdk::Key::Left | gtk::gdk::Key::h | gtk::gdk::Key::Page_Up => {
            prev();
            true
        }
        gtk::gdk::Key::Right | gtk::gdk::Key::l | gtk::gdk::Key::Page_Down => {
            next();
            true
        }
        _ => false,
    });

    let tick = {
        let big = big.clone();
        let sub = sub.clone();
        let detail = card.detail.clone();
        move || {
            let n = Local::now();
            big.set_text(&n.format("%H:%M:%S").to_string());
            sub.set_text(&n.format("%-d %B %Y").to_string().to_lowercase());
            let doy = n.ordinal();
            detail.set_markup(&format!("week {} · day {doy}", n.iso_week().week()));
        }
    };
    tick();
    p.every(1000, tick);
    render();
    card.root.upcast()
}

fn month_grid(first: NaiveDate) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.add_css_class("cal");
    grid.set_column_homogeneous(true);
    let today = Local::now().date_naive();
    let wk = ui::text("wk", &["wk"]);
    wk.set_xalign(0.5);
    grid.attach(&wk, 0, 0, 1, 1);
    for (i, d) in ["mo", "tu", "we", "th", "fr", "sa", "su"]
        .iter()
        .enumerate()
    {
        let l = ui::text(d, &["wd"]);
        l.set_xalign(0.5);
        grid.attach(&l, i as i32 + 1, 0, 1, 1);
    }
    let offset = first.weekday().num_days_from_monday() as i64;
    let start = first - CDuration::days(offset);
    for row in 0..6 {
        let week_start = start + CDuration::days(row * 7);
        let w = ui::text(&week_start.iso_week().week().to_string(), &["wk"]);
        w.set_xalign(0.5);
        grid.attach(&w, 0, row as i32 + 1, 1, 1);
        for col in 0..7 {
            let d = week_start + CDuration::days(col);
            let l = ui::text(&d.day().to_string(), &[]);
            l.set_xalign(0.5);
            if d.month() != first.month() {
                l.add_css_class("out");
            } else if col >= 5 {
                l.add_css_class("weekend");
            }
            if d == today {
                l.add_css_class("today");
            }
            grid.attach(&l, col as i32 + 1, row as i32 + 1, 1, 1);
        }
    }
    grid.upcast()
}
