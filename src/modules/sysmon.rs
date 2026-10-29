//! Laptop telemetry. Bar: `󰍛  52°C` (right click adds CPU% and RAM%).
//! Hover: summary. Click: live panel with history graphs and top processes.
//! Middle: btop. Everything is read from /proc and /sys in-process.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::util;

const HISTORY: usize = 90;
const DGPU: &str = "/sys/bus/pci/devices/0000:01:00.0";

#[derive(Clone, Default)]
pub struct Sample {
    pub cpu: f64,
    pub temp: i64,
    pub freq_ghz: f64,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub committed: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub fans: Vec<i64>,
    pub watts: f64,
    pub bat_status: String,
    pub profile: String,
    pub uptime: u64,
    pub gpu: String,
    pub load: String,
}

impl Sample {
    fn mem_pct(&self) -> f64 {
        if self.mem_total == 0 { 0.0 } else { self.mem_used as f64 * 100.0 / self.mem_total as f64 }
    }
    fn disk_pct(&self) -> f64 {
        if self.disk_total == 0 { 0.0 } else { self.disk_used as f64 * 100.0 / self.disk_total as f64 }
    }
}

fn hwmon(name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        if util::read_trim(e.path().join("name")).as_deref() == Some(name) {
            return Some(e.path());
        }
    }
    None
}

fn meminfo() -> HashMap<String, u64> {
    let mut m = HashMap::new();
    for l in util::read("/proc/meminfo").unwrap_or_default().lines() {
        let mut it = l.split_whitespace();
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            if let Ok(v) = v.parse() {
                m.insert(k.trim_end_matches(':').to_string(), v);
            }
        }
    }
    m
}

fn statvfs_kb(path: &str) -> (u64, u64) {
    let Ok(c) = std::ffi::CString::new(path) else { return (0, 0) };
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return (0, 0);
    }
    let f = s.f_frsize as u64;
    let total = s.f_blocks as u64 * f / 1024;
    let used = (s.f_blocks - s.f_bfree) as u64 * f / 1024;
    (used, total)
}

fn cpu_times() -> (u64, u64) {
    let line = util::read("/proc/stat").unwrap_or_default();
    let first = line.lines().next().unwrap_or("");
    let v: Vec<u64> = first.split_whitespace().skip(1).take(8).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 8 {
        return (0, 0);
    }
    let total: u64 = v.iter().sum();
    let busy = total - v[3] - v[4];
    (total, busy)
}

/// dGPU status without ever waking it (and never touching it mid-VM-handover).
fn gpu_status() -> String {
    let dev = Path::new(DGPU);
    let hold = util::home().join(".local/state/gpu-mode-hold").exists();
    let mut s = if !dev.exists() {
        return ui::dim("off");
    } else {
        let driver = std::fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        if driver == "vfio-pci" {
            "in win11 vm".to_string()
        } else if driver != "nvidia" || !Path::new("/sys/module/nvidia").exists() {
            ui::dim("switching")
        } else if util::read_trim(dev.join("power/runtime_status")).as_deref() == Some("active") {
            match util::run(
                &["nvidia-smi", "--query-gpu=temperature.gpu,utilization.gpu", "--format=csv,noheader,nounits"],
                Duration::from_secs(2),
            ) {
                Some(o) if o.ok => {
                    let parts: Vec<&str> = o.stdout.trim().split(", ").collect();
                    if parts.len() == 2 {
                        format!("{}%  {}  {}°", parts[1], ui::dim("·"), parts[0])
                    } else {
                        "awake".into()
                    }
                }
                _ => "awake".into(),
            }
        } else {
            ui::dim("asleep")
        }
    };
    if hold {
        s.push_str(&format!("  {}", ui::dim("· kept on")));
    }
    s
}

struct Inner {
    slot: Slot,
    theme: Rc<Theme>,
    last: RefCell<Sample>,
    prev: Cell<(u64, u64)>,
    cpu_hist: RefCell<VecDeque<f64>>,
    mem_hist: RefCell<VecDeque<f64>>,
    temp_hist: RefCell<VecDeque<f64>>,
    expanded: Cell<bool>,
    changed: Notify,
    warn_temp: i64,
    crit_temp: i64,
    gpu: RefCell<String>,
    gpu_busy: Cell<bool>,
    state_file: PathBuf,
}

pub struct Sysmon(Rc<Inner>);

impl Sysmon {
    pub fn new(ctx: &Ctx) -> Sysmon {
        let o = ctx.opts("sysmon");
        let state_file = util::home().join(".local/state/vector-bar/sysmon-expanded");
        // first run: carry over Waybar's expanded mode
        let expanded = state_file.exists()
            || (!state_file.parent().is_some_and(|p| p.join(".migrated").exists())
                && util::cache_dir().join("waybar-sysmon-expanded").exists());
        let inner = Rc::new(Inner {
            slot: ctx.slot("sysmon", &[]),
            theme: ctx.theme.clone(),
            last: RefCell::default(),
            prev: Cell::new(cpu_times()),
            cpu_hist: RefCell::default(),
            mem_hist: RefCell::default(),
            temp_hist: RefCell::default(),
            expanded: Cell::new(expanded),
            changed: Notify::default(),
            warn_temp: o.num("warn_temp", 75.0) as i64,
            crit_temp: o.num("crit_temp", 90.0) as i64,
            gpu: RefCell::new(String::new()),
            gpu_busy: Cell::new(false),
            state_file,
        });
        if let Some(p) = inner.state_file.parent() {
            let _ = std::fs::create_dir_all(p);
            let _ = std::fs::write(p.join(".migrated"), "");
        }
        inner.save_mode();
        let w = Rc::downgrade(&inner);
        inner.slot.on_right(move || {
            if let Some(i) = w.upgrade() {
                i.expanded.set(!i.expanded.get());
                i.save_mode();
                i.render();
            }
        });
        let mid = o.str("on_middle", "omarchy-launch-or-focus-tui btop");
        inner.slot.on_middle(move || util::spawn_sh(&mid));
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        inner.sample();
        inner.poll_gpu();
        util::every(o.num("interval", 3.0) as u64 * 1000, &inner, |i| i.sample());
        util::every(15_000, &inner, |i| i.poll_gpu());
        Sysmon(inner)
    }
}

impl Inner {
    fn save_mode(&self) {
        if self.expanded.get() {
            let _ = std::fs::write(&self.state_file, "");
        } else {
            let _ = std::fs::remove_file(&self.state_file);
        }
    }

    fn poll_gpu(self: &Rc<Self>) {
        if self.gpu_busy.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        util::bg(gpu_status, move |g| {
            if let Some(i) = w.upgrade() {
                i.gpu_busy.set(false);
                *i.gpu.borrow_mut() = g;
            }
        });
    }

    fn sample(&self) {
        let (total, busy) = cpu_times();
        let (pt, pb) = self.prev.replace((total, busy));
        let cpu = if total > pt { (busy - pb) as f64 * 100.0 / (total - pt) as f64 } else { 0.0 };
        let raw_temp = hwmon("coretemp").and_then(|d| util::read_num(d.join("temp1_input"))).unwrap_or(0) / 1000;
        // package temp is a hotspot that spikes for a second: average with the
        // previous value so a turbo burst doesn't flash the bar
        let prev_temp = self.last.borrow().temp;
        let temp = if prev_temp > 0 { (prev_temp + raw_temp + 1) / 2 } else { raw_temp };
        let mi = meminfo();
        let g = |k: &str| mi.get(k).copied().unwrap_or(0);
        let mut freqs = Vec::new();
        if let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
            for e in rd.flatten() {
                if let Some(f) = util::read_num(e.path().join("scaling_cur_freq")) {
                    freqs.push(f as f64);
                }
            }
        }
        let freq_ghz = if freqs.is_empty() { 0.0 } else { freqs.iter().sum::<f64>() / freqs.len() as f64 / 1e6 };
        let fans = hwmon("asus")
            .map(|d| (1..=4).filter_map(|n| util::read_num(d.join(format!("fan{n}_input")))).collect())
            .unwrap_or_default();
        let (du, dt) = statvfs_kb("/");
        let bat = hwmon("BAT0");
        let s = Sample {
            cpu,
            temp,
            freq_ghz,
            mem_used: g("MemTotal").saturating_sub(g("MemAvailable")),
            mem_total: g("MemTotal"),
            swap_used: g("SwapTotal").saturating_sub(g("SwapFree")),
            committed: g("Committed_AS"),
            disk_used: du,
            disk_total: dt,
            fans,
            watts: bat.and_then(|d| util::read_num(d.join("power1_input"))).unwrap_or(0) as f64 / 1e6,
            bat_status: util::read_trim("/sys/class/power_supply/BAT0/status").unwrap_or_default().to_lowercase(),
            profile: util::read_trim("/sys/firmware/acpi/platform_profile").unwrap_or_default(),
            uptime: util::read("/proc/uptime")
                .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()))
                .unwrap_or(0.0) as u64,
            gpu: self.gpu.borrow().clone(),
            load: util::read("/proc/loadavg")
                .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
                .unwrap_or_default(),
        };
        for (h, v) in [
            (&self.cpu_hist, s.cpu),
            (&self.mem_hist, s.mem_pct()),
            (&self.temp_hist, s.temp as f64),
        ] {
            let mut h = h.borrow_mut();
            h.push_back(v);
            while h.len() > HISTORY {
                h.pop_front();
            }
        }
        *self.last.borrow_mut() = s;
        self.render();
        self.changed.emit();
    }

    fn render(&self) {
        let s = self.last.borrow();
        let mem = s.mem_pct();
        let cls = if s.temp >= self.crit_temp || mem >= 95.0 {
            "crit"
        } else if s.temp >= self.warn_temp || mem >= 85.0 {
            "warn"
        } else {
            ""
        };
        self.slot.set_classes(&[cls]);
        let text = if self.expanded.get() {
            format!("󰍛  {:.0}%  {}°C  󰘚 {:.0}%", s.cpu, s.temp, mem)
        } else {
            format!("󰍛  {}°C", s.temp)
        };
        self.slot.set_markup(&text);
    }

    fn temp_markup(&self, t: i64) -> String {
        let s = format!("{t}°");
        if t >= self.crit_temp {
            ui::colored(&s, &self.theme.color1)
        } else if t >= self.warn_temp {
            ui::colored(&s, &self.theme.color3)
        } else {
            s
        }
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = self.last.borrow();
        let sh = ui::sheet();
        let host = util::read_trim("/proc/sys/kernel/hostname").unwrap_or_default();
        sh.append(&ui::head(&host, &format!("up {}{}", util::dur(s.uptime), if s.profile.is_empty() { String::new() } else { format!(" · {}", s.profile) })).0);
        // last 90 samples of cpu
        let hist: Vec<f64> = self.cpu_hist.borrow().iter().copied().collect();
        let g = spark(Rc::new(move || hist.clone()), 100.0, &self.theme.accent, 28);
        g.set_size_request(260, 28);
        sh.append(&g);
        let tcls = if s.temp >= self.crit_temp { "crit" } else if s.temp >= self.warn_temp { "warn" } else { "" };
        sh.append(&ui::tmeter("cpu", s.cpu / 100.0, &format!("{:.0}%", s.cpu), &format!("{:.1} GHz · load {}", s.freq_ghz, s.load), ui::pct_class(s.cpu)));
        sh.append(&ui::tmeter("temp", s.temp as f64 / 100.0, &format!("{}°", s.temp), &format!("fans {}", fans(&s.fans)), tcls));
        let mp = s.mem_pct();
        sh.append(&ui::tmeter("mem", mp / 100.0, &format!("{mp:.0}%"), &of(s.mem_used, s.mem_total), if mp >= 95.0 { "crit" } else if mp >= 85.0 { "warn" } else { "" }));
        let dp = s.disk_pct();
        sh.append(&ui::tmeter("disk", dp / 100.0, &format!("{dp:.0}%"), &of(s.disk_used, s.disk_total), if dp >= 92.0 { "crit" } else if dp >= 80.0 { "warn" } else { "" }));
        sh.append(&ui::tsection("hardware"));
        if !s.gpu.is_empty() {
            sh.append(&ui::tkv("gpu", &s.gpu));
        }
        sh.append(&ui::tkv("swap", &format!("{}  {}", util::size_kb(s.swap_used), ui::dim(&format!("· {} committed", util::size_kb(s.committed))))));
        sh.append(&ui::tkv("power", &format!("{:.1} W {}", s.watts, ui::dim(&s.bat_status))));
        sh.append(&ui::thint("click details  ·  middle btop  ·  right cpu and mem on the bar"));
        sh.upcast()
    }
}

fn of(used: u64, total: u64) -> String {
    let u = util::size_kb(used);
    let num = u.split(' ').next().unwrap_or("").to_string();
    format!("{num} {}", ui::dim(&format!("of {}", util::size_kb(total))))
}

fn fans(f: &[i64]) -> String {
    if f.iter().all(|x| *x == 0) {
        ui::dim("off")
    } else {
        format!("{} {}", f.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" / "), ui::dim("rpm"))
    }
}

impl Module for Sysmon {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.poll_gpu();
        self.0.sample();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

/// A line graph of recent values (0..max).
pub fn spark(hist: Rc<dyn Fn() -> Vec<f64>>, max: f64, color: &str, height: i32) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_height(height);
    d.set_hexpand(true);
    let rgba = gtk::gdk::RGBA::parse(color).unwrap_or(gtk::gdk::RGBA::WHITE);
    d.set_draw_func(move |_, cr, w, h| {
        let v = hist();
        if v.len() < 2 {
            return;
        }
        let (w, h) = (w as f64, h as f64);
        let n = HISTORY.max(v.len()) as f64 - 1.0;
        let x0 = w - (v.len() as f64 - 1.0) / n * w;
        let y = |val: f64| h - 1.0 - (val / max).clamp(0.0, 1.0) * (h - 2.0);
        cr.move_to(x0, h);
        for (i, val) in v.iter().enumerate() {
            cr.line_to(x0 + i as f64 / n * w, y(*val));
        }
        cr.line_to(w, h);
        cr.close_path();
        cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 0.16);
        let _ = cr.fill();
        for (i, val) in v.iter().enumerate() {
            let x = x0 + i as f64 / n * w;
            if i == 0 { cr.move_to(x, y(*val)) } else { cr.line_to(x, y(*val)) }
        }
        cr.set_source_rgba(rgba.red() as f64, rgba.green() as f64, rgba.blue() as f64, 0.9);
        cr.set_line_width(1.2);
        let _ = cr.stroke();
    });
    d
}

#[derive(Clone)]
struct Proc {
    pid: u32,
    name: String,
    ticks: u64,
    rss_kb: u64,
}

fn procs() -> Vec<Proc> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Some(stat) = util::read(e.path().join("stat")) else { continue };
        let (Some(a), Some(b)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[a + 1..b].to_string();
        let f: Vec<&str> = stat[b + 2..].split_whitespace().collect();
        if f.len() < 22 {
            continue;
        }
        let ticks = f[11].parse::<u64>().unwrap_or(0) + f[12].parse::<u64>().unwrap_or(0);
        let rss_kb = f[21].parse::<u64>().unwrap_or(0) * 4;
        out.push(Proc { pid, name, ticks, rss_kb });
    }
    out
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(380);
    let host = util::read_trim("/proc/sys/kernel/hostname").unwrap_or_default();
    let card = Card::new(&host, "");
    card.controls.append(&ui::link("btop", &[], {
        let p = p.clone();
        move || {
            p.close();
            util::spawn_sh("omarchy-launch-or-focus-tui btop");
        }
    }));
    let body = &card.body;
    let t = &i.theme;

    // cpu
    body.append(&ui::section("cpu").tap(|l| l.add_css_class("first")));
    let cpu_kv = ui::kv("now", "");
    body.append(&cpu_kv.0);
    let w = Rc::downgrade(i);
    let g_cpu = spark(Rc::new(move || w.upgrade().map(|i| i.cpu_hist.borrow().iter().copied().collect()).unwrap_or_default()), 100.0, &t.accent, 38);
    body.append(&ui::hbox(0, &["spark"]).tap(|b| b.append(&g_cpu)));
    let temp_kv = ui::kv("temp", "");
    body.append(&temp_kv.0);
    let w = Rc::downgrade(i);
    let g_temp = spark(Rc::new(move || w.upgrade().map(|i| i.temp_hist.borrow().iter().copied().collect()).unwrap_or_default()), 100.0, &t.color3, 24);
    body.append(&ui::hbox(0, &["spark"]).tap(|b| b.append(&g_temp)));

    // memory
    body.append(&ui::section("memory"));
    let mem_box = ui::vbox(0, &[]);
    body.append(&mem_box);
    let w = Rc::downgrade(i);
    let g_mem = spark(Rc::new(move || w.upgrade().map(|i| i.mem_hist.borrow().iter().copied().collect()).unwrap_or_default()), 100.0, &t.color6, 24);
    body.append(&ui::hbox(0, &["spark"]).tap(|b| b.append(&g_mem)));

    // hardware
    body.append(&ui::section("hardware"));
    let hw = ui::vbox(0, &[]);
    body.append(&hw);

    // processes
    body.append(&ui::section("top processes"));
    let top = ui::vbox(0, &[]);
    body.append(&top);
    let prev: Rc<RefCell<HashMap<u32, u64>>> = Rc::new(RefCell::new(procs().into_iter().map(|p| (p.pid, p.ticks)).collect()));
    let prev_total = Rc::new(Cell::new(cpu_times().0));

    let fill = {
        let i = Rc::downgrade(i);
        let detail = card.detail.clone();
        let (cpu_v, temp_v) = (cpu_kv.1.clone(), temp_kv.1.clone());
        let (g_cpu, g_temp, g_mem) = (g_cpu.clone(), g_temp.clone(), g_mem.clone());
        let (mem_box, hw) = (mem_box.clone(), hw.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            let s = i.last.borrow().clone();
            detail.set_markup(&format!("up {}{}", util::dur(s.uptime), if s.profile.is_empty() { String::new() } else { format!(" · {}", s.profile) }));
            cpu_v.set_markup(&format!("{:.0}%  {}", s.cpu, ui::dim(&format!("· {:.1} GHz · load {}", s.freq_ghz, s.load))));
            temp_v.set_markup(&format!("{}  {}", i.temp_markup(s.temp), ui::dim(&format!("· fans {}", fans(&s.fans)))));
            ui::clear(&mem_box);
            let mp = s.mem_pct();
            mem_box.append(&ui::meter_row("ram", mp / 100.0, &format!("{mp:.0}%"), &of(s.mem_used, s.mem_total), if mp >= 95.0 { "crit" } else if mp >= 85.0 { "warn" } else { "" }));
            let dp = s.disk_pct();
            mem_box.append(&ui::meter_row("disk", dp / 100.0, &format!("{dp:.0}%"), &of(s.disk_used, s.disk_total), if dp >= 92.0 { "crit" } else if dp >= 80.0 { "warn" } else { "" }));
            mem_box.append(&ui::kv("swap", &format!("{}  {}", util::size_kb(s.swap_used), ui::dim(&format!("· {} committed", util::size_kb(s.committed))))).0);
            ui::clear(&hw);
            if !s.gpu.is_empty() {
                hw.append(&ui::kv("gpu", &s.gpu).0);
            }
            hw.append(&ui::kv("power", &format!("{:.1} W {}", s.watts, ui::dim(&s.bat_status))).0);
            g_cpu.queue_draw();
            g_temp.queue_draw();
            g_mem.queue_draw();
        }
    };
    let fill = Rc::new(fill);
    fill();
    let f2 = fill.clone();
    p.watch(&i.changed, move || f2());

    let top_fill = {
        let top = top.clone();
        let p2 = p.clone();
        move || {
            let now = procs();
            let total = cpu_times().0;
            let dt = total.saturating_sub(prev_total.replace(total)).max(1) as f64;
            let ncpu = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
            let mut rows: Vec<(f64, Proc)> = {
                let pv = prev.borrow();
                now.iter()
                    .map(|p| {
                        let d = p.ticks.saturating_sub(*pv.get(&p.pid).unwrap_or(&p.ticks)) as f64;
                        // /proc/stat sums all cpus: scale to "% of one core" like top
                        (d / dt * 100.0 * ncpu, p.clone())
                    })
                    .collect()
            };
            *prev.borrow_mut() = now.iter().map(|p| (p.pid, p.ticks)).collect();
            rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(b.1.rss_kb.cmp(&a.1.rss_kb)));
            ui::clear(&top);
            for (pct, pr) in rows.into_iter().take(6) {
                let row = ui::Row::new("", &pr.name, &format!("{pct:>3.0}%  {}", util::size_kb(pr.rss_kb)));
                row.icon.set_visible(false);
                let pid = pr.pid;
                let name = pr.name.clone();
                let p3 = p2.clone();
                let kill = ui::icon_button("󰅖", &format!("end {name} ({pid})"), move || {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGTERM);
                    }
                    let _ = &p3;
                });
                kill.add_css_class("x");
                row.trail.append(&kill);
                row.root.set_tooltip_text(Some(&format!("pid {pid}")));
                top.append(&row.root);
            }
        }
    };
    let top_fill = Rc::new(top_fill);
    top_fill();
    let tf = top_fill.clone();
    p.every(2000, move || tf());
    card.root.upcast()
}
