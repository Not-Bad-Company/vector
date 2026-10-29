//! AI: one entry for everything AI on this machine.
//!
//! `ai` (bar item): the Claude mark, the number of agents working, red when
//! one waits for you. Panel: live agents (what each is doing, for how long,
//! click to focus its terminal) and usage limits for Claude (per account),
//! Codex (per account) and Antigravity, all read without spending usage.
//!
//! `agents` (centre item): one live line about the agents: who needs you,
//! or what the busiest one is doing.
//!
//! Sources, none of which spends tokens:
//! - agents: ~/.claude*/sessions/<pid>.json (status, start, turn start) and
//!   the tail of each session transcript (title, latest tool call)
//! - Claude: GET api.anthropic.com/api/oauth/usage with the account's own
//!   token (never refreshed here: that would log a running Claude Code out),
//!   else Claude Code's cached copy in .claude.json
//! - Codex: GET chatgpt.com/backend-api/wham/usage with the account's token,
//!   else the rate limits in its newest session rollout
//! - Antigravity: `agy -p /usage` (local slash command, no model call)

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use serde_json::Value;

use super::{BarInfo, Ctx, Module};
use crate::panel::{Card, Panel};
use crate::slot::{Notify, Slot};
use crate::theme::Theme;
use crate::ui::{self, Tap};
use crate::{hypr, mode, util};

const SPARK: &str = "<span font_family='ClaudeSpark'>\u{e000}</span>";
/// The bar mark: a brain, since this is every AI, not only Claude.
const BRAIN: &str = "\u{f09d1}";
const CODEX: &str = "\u{f06a9}";
const AGY: &str = "<span font_family='AntigravityMark'>\u{e000}</span>";
const SPIN: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
/// How long a finished agent stays on the bar line.
const DONE_SHOWN: f64 = 120.0;
const M_AGENT: mode::State = mode::State { key: "silent", label: "AGENT", rank: 2 };

// ── agents ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agent {
    pub pid: u32,
    pub sid: String,
    pub name: String,
    pub account: String,
    pub cwd: String,
    pub status: String,
    pub waiting: Option<String>,
    pub started: f64,
    /// when the current status began (turn start while busy)
    pub since: f64,
    pub title: String,
    pub doing: String,
    pub prompt: String,
    /// latest sign of life: status change or tool call
    pub changed: f64,
    pub subs: Vec<SubAgent>,
}

/// A running subagent of a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubAgent {
    pub kind: String,
    pub description: String,
    pub doing: String,
    pub changed: f64,
}

fn subagents(dir: &Path) -> Vec<SubAgent> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".meta.json")) else { continue };
        let Some(meta) = util::read_json(&p) else { continue };
        if meta["finished"] == true {
            continue;
        }
        let log = dir.join(format!("{name}.jsonl"));
        let mtime = std::fs::metadata(&log).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        // a subagent left unfinished by a crash stops counting after 10 quiet minutes
        if util::now() - mtime > 600.0 {
            continue;
        }
        let (_, _, doing, at) = transcript_meta(&log);
        out.push(SubAgent {
            kind: meta["agentType"].as_str().unwrap_or("agent").to_string(),
            description: meta["description"].as_str().unwrap_or("").to_string(),
            doing,
            changed: at.max(mtime),
        });
    }
    out.sort_by(|a, b| b.changed.partial_cmp(&a.changed).unwrap_or(std::cmp::Ordering::Equal));
    out
}

impl Agent {
    pub fn needs_you(&self) -> bool {
        self.waiting.is_some() || matches!(self.status.as_str(), "waiting" | "requires_action" | "blocked" | "awaiting_approval" | "awaiting_user")
    }
    pub fn busy(&self) -> bool {
        self.status == "busy"
    }
    fn label(&self) -> String {
        if self.title.is_empty() { self.name.clone() } else { self.title.clone() }
    }
}

fn account_label(dir: &str) -> String {
    match dir {
        ".claude" => "claude".into(),
        d => d.strip_prefix(".claude-account").map(|n| format!("claude{n}")).unwrap_or_else(|| d.trim_start_matches('.').to_string()),
    }
}

fn proc_start(pid: u32) -> Option<String> {
    let s = util::read(format!("/proc/{pid}/stat"))?;
    let rest = s.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19).map(str::to_string)
}

type Meta = (String, String, String, f64);

/// transcript path -> (size, mtime, parsed): only changed files are read again.
/// Process-wide: the scans run on worker threads.
static META: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, std::time::SystemTime, Meta)>>> = std::sync::LazyLock::new(Default::default);

/// Title, last prompt and what the latest tool call does, from the transcript tail.
/// Cached per file until its size or mtime changes.
fn transcript_meta(path: &Path) -> Meta {
    let Ok(md) = std::fs::metadata(path) else { return Default::default() };
    let (size, mtime) = (md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH));
    if let Ok(c) = META.lock() {
        if let Some((_, _, m)) = c.get(path).filter(|(s, t, _)| *s == size && *t == mtime) {
            return m.clone();
        }
    }
    let m = read_meta(path);
    if let Ok(mut c) = META.lock() {
        if c.len() > 200 {
            c.clear();
        }
        c.insert(path.to_path_buf(), (size, mtime, m.clone()));
    }
    m
}

fn read_meta(path: &Path) -> Meta {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return Default::default() };
    let size = f.seek(SeekFrom::End(0)).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(size.saturating_sub(128_000)));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let (mut title, mut prompt, mut doing, mut doing_at) = (String::new(), String::new(), String::new(), 0.0);
    for line in text.lines().rev() {
        if !title.is_empty() && !prompt.is_empty() && !doing.is_empty() {
            break;
        }
        if title.is_empty() && (line.contains("\"ai-title\"") || line.contains("\"custom-title\"")) {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                title = v["customTitle"].as_str().or(v["aiTitle"].as_str()).unwrap_or("").to_string();
            }
        } else if prompt.is_empty() && line.contains("\"last-prompt\"") {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                prompt = v["lastPrompt"].as_str().unwrap_or("").to_string();
            }
        } else if doing.is_empty() && line.contains("\"tool_use\"") {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(items) = v["message"]["content"].as_array() else { continue };
            if let Some(c) = items.iter().rev().find(|c| c["type"] == "tool_use") {
                doing = describe_tool(&c["name"], &c["input"]);
                doing_at = v["timestamp"].as_str().and_then(util::parse_iso).unwrap_or(0.0);
            }
        }
    }
    (title, prompt, doing, doing_at)
}

fn describe_tool(name: &Value, input: &Value) -> String {
    let name = name.as_str().unwrap_or("");
    let file = |k: &str| input[k].as_str().and_then(|p| Path::new(p).file_name()).map(|f| f.to_string_lossy().to_string());
    if let Some(d) = input["description"].as_str() {
        return d.to_string();
    }
    match name {
        "Edit" | "MultiEdit" => file("file_path").map(|f| format!("editing {f}")),
        "Write" => file("file_path").map(|f| format!("writing {f}")),
        "Read" => file("file_path").map(|f| format!("reading {f}")),
        "Grep" | "Glob" => input["pattern"].as_str().map(|p| format!("searching {p}")),
        "Bash" => input["command"].as_str().map(|c| c.chars().take(48).collect()),
        "Agent" | "Task" => input["description"].as_str().map(|d| format!("subagent: {d}")),
        "WebSearch" | "WebFetch" => Some("on the web".into()),
        _ => None,
    }
    .unwrap_or_else(|| name.to_lowercase())
}

pub fn agents() -> Vec<Agent> {
    let mut out: Vec<Agent> = Vec::new();
    let home = util::home();
    let Ok(rd) = std::fs::read_dir(&home) else { return out };
    for d in rd.flatten() {
        let dn = d.file_name().to_string_lossy().to_string();
        if !dn.starts_with(".claude") {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(d.path().join("sessions")) else { continue };
        for f in sessions.flatten() {
            let Some(v) = util::read_json(f.path()) else { continue };
            let pid = v["pid"].as_u64().unwrap_or(0) as u32;
            let sid = v["sessionId"].as_str().unwrap_or("").to_string();
            if pid == 0 || sid.is_empty() || out.iter().any(|a| a.sid == sid) {
                continue;
            }
            // a reused pid is not this session
            if let Some(ps) = v["procStart"].as_str() {
                if proc_start(pid).as_deref() != Some(ps) {
                    continue;
                }
            } else if !Path::new(&format!("/proc/{pid}")).exists() {
                continue;
            }
            let cwd = v["cwd"].as_str().unwrap_or("").to_string();
            let tp = home.join(".claude/projects").join(cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect::<String>()).join(format!("{sid}.jsonl"));
            let (title, prompt, doing, doing_at) = transcript_meta(&tp);
            let subs = subagents(&tp.with_extension("").join("subagents"));
            let ms = |k: &str| v[k].as_f64().unwrap_or(0.0) / 1000.0;
            let since = ms("statusUpdatedAt").max(ms("startedAt"));
            let changed = subs.iter().map(|s| s.changed).fold(since.max(doing_at), f64::max);
            out.push(Agent {
                pid,
                sid,
                name: v["name"].as_str().unwrap_or("").to_string(),
                account: account_label(&dn),
                cwd,
                status: v["status"].as_str().unwrap_or("?").to_string(),
                waiting: v["waitingFor"].as_str().map(str::to_string),
                started: ms("startedAt"),
                since,
                title,
                doing,
                prompt,
                changed,
                subs,
            });
        }
    }
    out.sort_by(|a, b| a.started.partial_cmp(&b.started).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The Hyprland window holding an agent: walk up its parent processes until one owns a window.
fn focus_agent(pid: u32) {
    util::bg(
        move || {
            let clients = hypr::json("clients")?;
            let wins: Vec<(u64, String)> = clients.as_array()?.iter().filter_map(|c| Some((c["pid"].as_u64()?, c["address"].as_str()?.to_string()))).collect();
            let mut p = pid as u64;
            for _ in 0..12 {
                if let Some((_, addr)) = wins.iter().find(|(wp, _)| *wp == p) {
                    return Some(addr.clone());
                }
                let stat = util::read(format!("/proc/{p}/stat"))?;
                p = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
                if p <= 1 {
                    break;
                }
            }
            None
        },
        |addr| {
            if let Some(a) = addr {
                hypr::dispatch(&format!("focuswindow address:{a}"));
            }
        },
    );
}

// ── usage ────────────────────────────────────────────────────────────────────

/// One limit window: used %, reset time, window length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub name: String,
    pub pct: f64,
    pub reset: Option<f64>,
    pub span: f64,
}

impl Window {
    /// % used beyond an even pace through the window (positive = ahead).
    pub fn pace(&self) -> Option<f64> {
        let r = self.reset?;
        let elapsed = 1.0 - (r - util::now()) / self.span;
        (0.02..=1.0).contains(&elapsed).then(|| self.pct - elapsed * 100.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Account {
    pub label: String,
    pub detail: String,
    pub windows: Vec<Window>,
    pub extra: Option<String>,
    pub source: String,
    pub fetched: f64,
    pub error: Option<String>,
}

impl Account {
    fn worst(&self) -> Option<f64> {
        self.windows.iter().map(|w| w.pct).fold(None, |a, p| Some(a.map_or(p, |x: f64| x.max(p))))
    }
}

fn win(name: &str, v: &Value, span: f64) -> Option<Window> {
    let pct = v["utilization"].as_f64()?;
    let reset = v["resets_at"].as_str().and_then(util::parse_iso);
    // the window rolled over since the snapshot
    if reset.is_some_and(|r| r <= util::now()) {
        return Some(Window { name: name.into(), pct: 0.0, reset: None, span });
    }
    Some(Window { name: name.into(), pct, reset, span })
}

fn cache(name: &str) -> PathBuf {
    util::cache_dir().join("vector-bar").join(name)
}

fn claude_account(label: &str, dir: &Path, claude_json: &Path, force: bool) -> Account {
    let state_path = cache(&format!("claude-{label}.json"));
    let mut st = util::read_json(&state_path).unwrap_or(Value::Object(Default::default()));
    let now = util::now();
    let creds = util::read_json(dir.join(".credentials.json")).map(|v| v["claudeAiOauth"].clone()).unwrap_or_default();
    let token = creds["accessToken"].as_str().unwrap_or("").to_string();
    let expires = creds["expiresAt"].as_f64().unwrap_or(0.0) / 1000.0;
    let plan = creds["subscriptionType"].as_str().unwrap_or("").to_string();
    let attempt = st["attempt"].as_f64().unwrap_or(0.0);
    let err_at = st["error_at"].as_f64().unwrap_or(0.0);
    let mut error = st["error"].as_str().map(str::to_string);
    let due = force || (now - attempt >= 55.0 && now - err_at >= 300.0);
    if due {
        if token.is_empty() {
            error = Some("not logged in".into());
        } else if expires <= now + 30.0 {
            error = Some("token expired (account idle)".into());
        } else {
            st["attempt"] = now.into();
            let r = ureq::get("https://api.anthropic.com/api/oauth/usage")
                .header("Authorization", &format!("Bearer {token}"))
                .header("anthropic-beta", "oauth-2025-04-20")
                .config()
                .timeout_global(Some(Duration::from_secs(8)))
                .build()
                .call();
            match r.and_then(|mut r| r.body_mut().read_json::<Value>()) {
                Ok(v) => {
                    st["data"] = v;
                    st["fetched"] = now.into();
                    error = None;
                    st["error_at"] = 0.into();
                }
                Err(e) => {
                    error = Some(e.to_string().chars().take(80).collect());
                    st["error_at"] = now.into();
                }
            }
        }
        st["error"] = error.clone().map(Value::from).unwrap_or(Value::Null);
        if let Ok(s) = serde_json::to_string(&st) {
            let _ = util::write_atomic(&state_path, &s);
        }
    }
    let mut snap = st["data"].clone();
    let mut fetched = st["fetched"].as_f64().unwrap_or(0.0);
    let mut source = "live".to_string();
    // Claude Code's own cached copy may be newer
    let cj = util::read_json(claude_json).unwrap_or_default();
    let cc = &cj["cachedUsageUtilization"];
    let cc_at = cc["fetchedAtMs"].as_f64().unwrap_or(0.0) / 1000.0;
    if cc["utilization"].is_object() && cc_at > fetched {
        snap = cc["utilization"].clone();
        fetched = cc_at;
        source = "claude code".into();
    }
    let email = cj["oauthAccount"]["emailAddress"].as_str().unwrap_or("").to_string();
    let mut windows = Vec::new();
    windows.extend(win("5h", &snap["five_hour"], 5.0 * 3600.0));
    windows.extend(win("week", &snap["seven_day"], 7.0 * 86400.0));
    for (k, n) in [("seven_day_opus", "opus"), ("seven_day_sonnet", "sonnet")] {
        if let Some(w) = win(n, &snap[k], 7.0 * 86400.0).filter(|w| w.pct > 0.0) {
            windows.push(w);
        }
    }
    let ex = &snap["extra_usage"];
    let extra = if ex["is_enabled"] == true {
        Some(format!("{:.2} / {} {}", ex["used_credits"].as_f64().unwrap_or(0.0), ex["monthly_limit"], ex["currency"].as_str().unwrap_or("")))
    } else {
        None
    };
    let detail = [plan, email].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    Account { label: label.into(), detail, windows, extra, source, fetched, error }
}

fn codex_account(label: &str, home: &Path, force: bool) -> Account {
    let state_path = cache(&format!("codex-{label}.json"));
    let mut st = util::read_json(&state_path).unwrap_or(Value::Object(Default::default()));
    let now = util::now();
    let auth = util::read_json(home.join("auth.json")).unwrap_or_default();
    let token = auth["tokens"]["access_token"].as_str().unwrap_or("").to_string();
    let acct = auth["tokens"]["account_id"].as_str().unwrap_or("").to_string();
    let due = force || (now - st["attempt"].as_f64().unwrap_or(0.0) >= 55.0 && now - st["error_at"].as_f64().unwrap_or(0.0) >= 300.0);
    let mut error = st["error"].as_str().map(str::to_string);
    if due && !token.is_empty() {
        st["attempt"] = now.into();
        let r = ureq::get("https://chatgpt.com/backend-api/wham/usage")
            .header("Authorization", &format!("Bearer {token}"))
            .header("ChatGPT-Account-Id", &acct)
            .header("User-Agent", "codex_cli_rs")
            .config()
            .timeout_global(Some(Duration::from_secs(8)))
            .build()
            .call();
        match r.and_then(|mut r| r.body_mut().read_json::<Value>()) {
            Ok(v) => {
                st["data"] = v;
                st["fetched"] = now.into();
                error = None;
            }
            Err(e) => {
                error = Some(e.to_string().chars().take(80).collect());
                st["error_at"] = now.into();
            }
        }
        st["error"] = error.clone().map(Value::from).unwrap_or(Value::Null);
        if let Ok(s) = serde_json::to_string(&st) {
            let _ = util::write_atomic(&state_path, &s);
        }
    } else if token.is_empty() {
        error = Some("not logged in".into());
    }
    let d = &st["data"];
    let mut windows = Vec::new();
    fn add_win(windows: &mut Vec<Window>, now: f64, name: &str, w: &Value) {
        if let Some(pct) = w["used_percent"].as_f64() {
            let span = w["limit_window_seconds"].as_f64().or_else(|| w["window_minutes"].as_f64().map(|m| m * 60.0)).unwrap_or(0.0);
            let reset = w["reset_at"].as_f64().or_else(|| w["resets_at"].as_f64());
            let (pct, reset) = if reset.is_some_and(|r| r <= now) { (0.0, None) } else { (pct, reset) };
            let name = if span >= 86400.0 * 6.5 { format!("{}d", (span / 86400.0).round()) } else if span > 0.0 { format!("{}h", (span / 3600.0).round()) } else { name.to_string() };
            windows.push(Window { name, pct, reset, span: span.max(1.0) });
        }
    }
    let (mut source, mut fetched) = ("live".to_string(), st["fetched"].as_f64().unwrap_or(0.0));
    if d.is_object() {
        add_win(&mut windows, now, "primary", &d["rate_limit"]["primary_window"]);
        add_win(&mut windows, now, "secondary", &d["rate_limit"]["secondary_window"]);
    }
    if windows.is_empty() {
        // newest rollout's own record of the limits
        if let Some((rl, at)) = newest_rollout_limits(home) {
            add_win(&mut windows, now, "primary", &rl["primary"]);
            add_win(&mut windows, now, "secondary", &rl["secondary"]);
            source = "last session".into();
            fetched = at;
        }
    }
    let plan = d["plan_type"].as_str().unwrap_or("").to_string();
    let email = d["email"].as_str().unwrap_or("").to_string();
    let detail = [plan, email].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    Account { label: label.into(), detail, windows, extra: None, source, fetched, error }
}

fn newest_rollout_limits(home: &Path) -> Option<(Value, f64)> {
    fn walk(d: &Path, best: &mut Option<(std::time::SystemTime, PathBuf)>, depth: u8) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth < 4 {
                walk(&p, best, depth + 1);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                    if best.as_ref().is_none_or(|(t, _)| m > *t) {
                        *best = Some((m, p));
                    }
                }
            }
        }
    }
    let mut best = None;
    walk(&home.join("sessions"), &mut best, 0);
    let (t, p) = best?;
    let text = std::fs::read_to_string(p).ok()?;
    let line = text.lines().rev().find(|l| l.contains("\"rate_limits\""))?;
    let v: Value = serde_json::from_str(line).ok()?;
    let rl = v["payload"]["rate_limits"].clone();
    let rl = if rl.is_object() { rl } else { v["rate_limits"].clone() };
    Some((rl, t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)))
}

fn agy_usage(force: bool) -> Account {
    let path = cache("antigravity.json");
    let mut snap = util::read_json(&path).unwrap_or_default();
    let mut error = None;
    if force || util::now() - snap["fetched"].as_f64().unwrap_or(0.0) >= 110.0 {
        match util::run_in(&["agy", "-p", "/usage", "--log-file", "/dev/null"], Duration::from_secs(45), None, Some(Path::new("/tmp"))) {
            Some(o) => {
                let rows: Vec<Value> = o
                    .stdout
                    .lines()
                    .filter_map(|l| {
                        let p: Vec<&str> = l.split('\t').collect();
                        (p.len() == 4 && p[2].ends_with('%')).then(|| serde_json::json!({"group": p[0], "label": p[1], "remaining": p[2].trim_end_matches('%').parse::<f64>().unwrap_or(100.0), "reset": util::parse_iso(p[3])}))
                    })
                    .collect();
                if rows.is_empty() {
                    error = Some(o.stderr.lines().last().unwrap_or("agy returned no usage").chars().take(80).collect::<String>());
                } else {
                    snap = serde_json::json!({"rows": rows, "fetched": util::now()});
                    if let Ok(s) = serde_json::to_string(&snap) {
                        let _ = util::write_atomic(&path, &s);
                    }
                }
            }
            None => error = Some("agy timed out".into()),
        }
    }
    let mut windows = Vec::new();
    for r in snap["rows"].as_array().cloned().unwrap_or_default() {
        let label = r["label"].as_str().unwrap_or("").to_lowercase();
        let short = if label.contains("five") { "5h" } else { "week" };
        let g = r["group"].as_str().unwrap_or("").to_lowercase();
        // "Gemini Models" / "Claude and GPT models" / "Gemini 4 Argon"
        let group = if g.contains("argon") { "argon".to_string() } else if g.contains("claude") { "claude/gpt".into() } else { g.replace(" models", "") };
        let reset = r["reset"].as_f64();
        let pct = if reset.is_some_and(|x| x < util::now()) { 0.0 } else { 100.0 - r["remaining"].as_f64().unwrap_or(100.0) };
        windows.push(Window { name: format!("{group} {short}"), pct, reset: reset.filter(|x| *x > util::now()), span: if short == "5h" { 5.0 * 3600.0 } else { 7.0 * 86400.0 } });
    }
    Account { label: "antigravity".into(), detail: String::new(), windows, extra: None, source: "agy".into(), fetched: snap["fetched"].as_f64().unwrap_or(0.0), error }
}

#[derive(Clone, Default, PartialEq)]
struct Usage {
    claude: Vec<Account>,
    codex: Vec<Account>,
    agy: Option<Account>,
}

// ── module ───────────────────────────────────────────────────────────────────

struct Inner {
    slot: Slot,
    line: Slot,
    theme: Rc<Theme>,
    agents: RefCell<Vec<Agent>>,
    usage: RefCell<Usage>,
    changed: Notify,
    busy: Cell<bool>,
    usage_busy: Cell<bool>,
    spin: Cell<usize>,
    claude: Vec<(String, PathBuf, PathBuf)>,
    codex: Vec<(String, PathBuf)>,
    agy: bool,
    notified: RefCell<Vec<String>>,
}

pub struct Ai(Rc<Inner>);

thread_local! {
    static SHARED: RefCell<std::rc::Weak<Inner>> = RefCell::new(std::rc::Weak::new());
}

impl Ai {
    /// `ai` and `agents` are two items of one module: built once per reload.
    pub fn shared(ctx: &Ctx) -> Ai {
        if let Some(i) = SHARED.with(|s| s.borrow().upgrade()) {
            return Ai(i);
        }
        let a = Ai::new(ctx);
        SHARED.with(|s| *s.borrow_mut() = Rc::downgrade(&a.0));
        a
    }

    fn new(ctx: &Ctx) -> Ai {
        let o = ctx.opts("ai");
        let home = util::home();
        // claude1 is logged out: off unless listed
        let claude_names = o.list("claude").unwrap_or_else(|| vec!["claude2".into(), "claude3".into()]);
        let claude = claude_names
            .iter()
            .map(|n| {
                let dir = if n == "claude" { home.join(".claude") } else { home.join(format!(".claude-account{}", n.trim_start_matches("claude"))) };
                let cj = if n == "claude" { home.join(".claude.json") } else { dir.join(".claude.json") };
                (n.clone(), dir, cj)
            })
            .collect();
        let codex = o
            .list("codex")
            .unwrap_or_else(|| vec!["codex".into(), "codex2".into(), "codex3".into()])
            .into_iter()
            .map(|n| (n.clone(), home.join(format!(".{n}"))))
            .filter(|(_, p)| p.exists())
            .collect();
        let inner = Rc::new(Inner {
            slot: ctx.slot("ai", &[]),
            line: ctx.slot("agents", &["agents-line"]),
            theme: ctx.theme.clone(),
            agents: RefCell::default(),
            usage: RefCell::default(),
            changed: Notify::default(),
            busy: Cell::new(false),
            usage_busy: Cell::new(false),
            spin: Cell::new(0),
            claude,
            codex,
            agy: o.bool("antigravity", true) && util::which("agy"),
            notified: RefCell::default(),
        });
        inner.slot.set_markup(BRAIN);
        inner.line.set_visible(false);
        let w = Rc::downgrade(&inner);
        inner.slot.tooltip(move || w.upgrade().map(|i| i.tooltip()));
        let w = Rc::downgrade(&inner);
        inner.slot.panel(move |p| match w.upgrade() {
            Some(i) => panel(&i, p),
            None => ui::vbox(0, &[]).upcast(),
        });
        let w = Rc::downgrade(&inner);
        inner.slot.on_middle(move || {
            if let Some(i) = w.upgrade() {
                i.fetch_usage(true);
            }
        });
        // the centre line: click jumps to the agent it talks about
        let w = Rc::downgrade(&inner);
        inner.line.on_left(move || {
            if let Some(i) = w.upgrade() {
                let a = i.agents.borrow();
                if let Some(a) = Inner::pick(&a) {
                    focus_agent(a.pid);
                }
            }
        });
        let w = Rc::downgrade(&inner);
        inner.line.on_right(move || {
            if let Some(i) = w.upgrade() {
                i.slot.toggle_panel(None, false);
            }
        });
        let w = Rc::downgrade(&inner);
        inner.line.tooltip(move || w.upgrade().map(|i| i.agents_tip()));
        inner.poll_agents();
        inner.fetch_usage(false);
        util::every(o.num("agents_interval", 2.0) as u64 * 1000, &inner, |i| i.poll_agents());
        util::every(60_000, &inner, |i| i.fetch_usage(false));
        util::every(400, &inner, |i| i.tick());
        Ai(inner)
    }
}

impl Inner {
    fn poll_agents(self: &Rc<Self>) {
        if self.busy.replace(true) {
            return;
        }
        let w = Rc::downgrade(self);
        util::bg(agents, move |a| {
            let Some(i) = w.upgrade() else { return };
            i.busy.set(false);
            // one notification per wait
            {
                let mut seen = i.notified.borrow_mut();
                for ag in a.iter().filter(|a| a.needs_you()) {
                    if !seen.contains(&ag.sid) {
                        seen.push(ag.sid.clone());
                        util::spawn(&["notify-send", "-u", "critical", "-a", "vector-bar", "Agent needs you", &format!("{} · {}", ag.name, ag.label())]);
                    }
                }
                seen.retain(|s| a.iter().any(|x| &x.sid == s && x.needs_you()));
            }
            mode::set(M_AGENT, a.iter().any(|x| x.needs_you()));
            let changed = *i.agents.borrow() != a;
            *i.agents.borrow_mut() = a;
            i.render();
            if changed {
                i.changed.emit();
            }
        });
    }

    fn fetch_usage(self: &Rc<Self>, force: bool) {
        if self.usage_busy.replace(true) {
            return;
        }
        let (claude, codex, agy) = (self.claude.clone(), self.codex.clone(), self.agy);
        let w = Rc::downgrade(self);
        util::bg(
            move || {
                let cl = std::thread::scope(|s| {
                    let hs: Vec<_> = claude.iter().map(|(l, d, j)| s.spawn(move || claude_account(l, d, j, force))).collect();
                    let cx: Vec<_> = codex.iter().map(|(l, h)| s.spawn(move || codex_account(l, h, force))).collect();
                    let ag = agy.then(|| s.spawn(move || agy_usage(force)));
                    (
                        hs.into_iter().filter_map(|h| h.join().ok()).collect::<Vec<_>>(),
                        cx.into_iter().filter_map(|h| h.join().ok()).collect::<Vec<_>>(),
                        ag.and_then(|h| h.join().ok()),
                    )
                });
                Usage { claude: cl.0, codex: cl.1, agy: cl.2 }
            },
            move |u| {
                let Some(i) = w.upgrade() else { return };
                i.usage_busy.set(false);
                *i.usage.borrow_mut() = u;
                i.render();
                i.changed.emit();
            },
        );
    }

    fn tick(&self) {
        let a = self.agents.borrow();
        if a.iter().any(|x| x.busy()) {
            self.spin.set((self.spin.get() + 1) % SPIN.len());
            self.render_line(&a);
        }
    }

    fn render(&self) {
        let a = self.agents.borrow();
        let u = self.usage.borrow();
        let busy = a.iter().filter(|x| x.busy()).count();
        let waiting = a.iter().filter(|x| x.needs_you()).count();
        let mut text = BRAIN.to_string();
        if waiting > 0 {
            text += &format!(" 󰀦 {waiting}");
        } else if busy > 0 {
            text += &format!(" {busy}");
        }
        self.slot.set_markup(&text);
        // tint when every Claude account is (nearly) out
        let best = u.claude.iter().filter_map(|c| c.worst()).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |x| x.min(p))));
        let cls = if waiting > 0 {
            "crit"
        } else if best.is_some_and(|b| b >= 100.0) {
            "crit"
        } else if best.is_some_and(|b| b >= 90.0) {
            "warn"
        } else {
            ""
        };
        self.slot.set_classes(&[cls]);
        self.slot.bump();
        self.render_line(&a);
    }

    /// The one agent the line is about: whoever needs you, else the busy one
    /// that changed last, else one that finished in the last two minutes.
    fn pick(a: &[Agent]) -> Option<&Agent> {
        fn latest<'a>(it: impl Iterator<Item = &'a Agent>) -> Option<&'a Agent> {
            it.max_by(|x, y| x.changed.partial_cmp(&y.changed).unwrap_or(std::cmp::Ordering::Equal))
        }
        a.iter()
            .find(|x| x.needs_you())
            .or_else(|| latest(a.iter().filter(|x| x.busy())))
            .or_else(|| latest(a.iter().filter(|x| !x.busy() && util::now() - x.since < DONE_SHOWN)))
    }

    fn render_line(&self, a: &[Agent]) {
        let now = util::now();
        let Some(t) = Self::pick(a) else {
            self.line.set_visible(false);
            return;
        };
        if t.needs_you() {
            self.line.set_markup(&format!("󰀦  {}  {}", util::esc(&short(&t.label(), 40)), "needs your answer"));
            self.line.set_classes(&["crit"]);
            self.line.set_visible(true);
            return;
        }
        let others = a.iter().filter(|x| x.busy() && x.sid != t.sid).count();
        let more = if others > 0 { format!("  {}", ui::dim(&format!("+{others}"))) } else { String::new() };
        let text = if t.busy() {
            let what = t.subs.first().filter(|s| s.changed > t.changed - 1.0).map(|s| s.description.clone()).unwrap_or_else(|| t.doing.clone());
            format!(
                "{}  {}  {}  {}{more}",
                SPIN[self.spin.get()],
                util::esc(&short(&t.label(), 38)),
                ui::dim(&util::esc(&short(&what, 42))),
                ui::dim(&util::dur((now - t.since).max(0.0) as u64)),
            )
        } else {
            format!("{}  {}  {}{more}", ui::dim("󰄬"), util::esc(&short(&t.label(), 38)), ui::dim(&format!("done, {}", util::ago(t.since))))
        };
        self.line.set_markup(&text);
        self.line.set_classes(&[]);
        self.line.set_visible(true);
    }

    fn agents_tip(&self) -> gtk::Widget {
        let s = ui::sheet();
        let a = self.agents.borrow();
        let busy = a.iter().filter(|x| x.busy()).count();
        s.append(&ui::head("agents", &format!("{busy} of {} working", a.len())).0);
        for ag in a.iter() {
            s.append(&agent_line(ag, &self.theme, true).tap(|w| w.set_margin_top(4)));
            if ag.busy() && !ag.doing.is_empty() {
                let d = ui::label(&ui::dim(&format!("now  {}", util::esc(&short(&ag.doing, 56)))), &[]);
                d.set_margin_start(22);
                s.append(&d);
            }
            for sa in &ag.subs {
                let l = ui::label(&ui::dim(&format!("└ {}  {}", util::esc(&sa.kind), util::esc(&short(&sa.description, 44)))), &[]);
                l.set_margin_start(22);
                s.append(&l);
            }
        }
        s.append(&ui::thint("click jump to it  ·  right ai overview"));
        s.upcast()
    }

    fn tooltip(&self) -> gtk::Widget {
        let s = ui::sheet();
        let a = self.agents.borrow();
        let u = self.usage.borrow();
        let working = a.iter().filter(|x| x.busy()).count();
        s.append(&ui::head("ai", &format!("{working} of {} agents working", a.len())).0);
        if !a.is_empty() {
            s.append(&ui::tsection("agents"));
            for ag in a.iter() {
                s.append(&agent_line(ag, &self.theme, false));
            }
        }
        s.append(&ui::tsection("limits"));
        // one row per account: the window that binds now (highest use)
        let grid = gtk::Grid::new();
        grid.set_column_spacing(10);
        grid.set_row_spacing(3);
        let mut row = 0;
        let mut add = |name: &str, w: Option<&Window>, err: Option<&str>| {
            grid.attach(&ui::text(name, &["dim"]), 0, row, 1, 1);
            match w {
                Some(w) => {
                    let cls = ui::pct_class(w.pct);
                    grid.attach(&ui::text(&w.name, &["dim"]), 1, row, 1, 1);
                    let m = ui::meter(w.pct / 100.0, cls);
                    m.set_size_request(80, -1);
                    m.set_hexpand(false);
                    grid.attach(&m, 2, row, 1, 1);
                    grid.attach(&ui::text(&format!("{:.0}%", w.pct), &[cls]).tap(|l| l.set_xalign(1.0)), 3, row, 1, 1);
                    let reset = w.reset.map(util::until).unwrap_or_default();
                    grid.attach(&ui::text(&reset, &["dim"]), 4, row, 1, 1);
                }
                None => grid.attach(&ui::text(err.unwrap_or("no data yet"), &["dim"]), 1, row, 4, 1),
            }
            row += 1;
        };
        let worst = |ws: &[Window]| ws.iter().max_by(|a, b| a.pct.partial_cmp(&b.pct).unwrap_or(std::cmp::Ordering::Equal)).cloned();
        for c in u.claude.iter().chain(u.codex.iter()) {
            add(&c.label, worst(&c.windows).as_ref(), c.error.as_deref());
        }
        if let Some(g) = &u.agy {
            // per model group: its own binding window
            let mut groups: Vec<String> = Vec::new();
            for w in &g.windows {
                let grp = w.name.rsplit_once(' ').map(|x| x.0).unwrap_or(&w.name).to_string();
                if !groups.contains(&grp) {
                    groups.push(grp);
                }
            }
            for grp in groups {
                let ws: Vec<Window> = g.windows.iter().filter(|w| w.name.starts_with(&format!("{grp} "))).map(|w| Window { name: w.name.rsplit(' ').next().unwrap_or("").to_string(), ..w.clone() }).collect();
                add(&grp, worst(&ws).as_ref(), g.error.as_deref());
            }
        }
        s.append(&grid);
        s.append(&ui::thint("click overview  ·  middle refresh usage"));
        s.upcast()
    }
}

fn short(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n { one.chars().take(n - 1).collect::<String>() + "…" } else { one }
}

fn agent_line(a: &Agent, t: &Theme, long: bool) -> gtk::Widget {
    let row = ui::hbox(8, &[]);
    let (icon, cls) = if a.needs_you() { ("󰀦", "crit") } else if a.busy() { ("󰔟", "ok") } else { ("󰄬", "dim") };
    row.append(&ui::text(icon, &[cls]));
    let title = ui::text(&short(&a.label(), if long { 44 } else { 34 }), if a.busy() || a.needs_you() { &[] } else { &["dim"] });
    title.set_hexpand(true);
    row.append(&title);
    let since = util::dur((util::now() - a.since).max(0.0) as u64);
    let state = if a.needs_you() { ui::colored("needs you", &t.color1) } else if a.busy() { format!("busy {since}") } else { ui::dim(&format!("done {since} ago")) };
    row.append(&ui::label(&state, &["dim"]));
    row.upcast()
}

impl Module for Ai {
    fn view(&self, bar: &BarInfo) -> gtk::Widget {
        let _ = bar;
        self.0.slot.view()
    }
    fn refresh(&self) {
        self.0.poll_agents();
        self.0.fetch_usage(true);
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.slot.clone()]
    }
}

/// The centre activity line: the same module, its second item.
pub struct AgentsLine(Ai);

impl AgentsLine {
    pub fn shared(ctx: &Ctx) -> AgentsLine {
        AgentsLine(Ai::shared(ctx))
    }
}

impl Module for AgentsLine {
    fn view(&self, _: &BarInfo) -> gtk::Widget {
        self.0.0.line.view()
    }
    fn refresh(&self) {
        self.0.refresh();
    }
    fn slots(&self) -> Vec<Slot> {
        vec![self.0.0.line.clone()]
    }
}

// ── panel ────────────────────────────────────────────────────────────────────

fn usage_block(acc: &Account, mark: &str, t: &Theme) -> gtk::Widget {
    let b = ui::vbox(0, &[]);
    let head = ui::hbox(8, &["kv"]);
    head.append(&ui::label(mark, &[]));
    head.append(&ui::text(&acc.label, &["acct"]));
    let d = ui::label(&ui::dim(&util::esc(&acc.detail)), &[]);
    d.set_hexpand(true);
    ui::ellipsize(&d, 30);
    head.append(&d);
    if let Some(w) = acc.windows.iter().map(|w| w.pct).reduce(f64::max) {
        if w >= 100.0 {
            head.append(&ui::label(&ui::colored("limit reached", &t.color1), &[]));
        }
    }
    b.append(&head);
    // one label column per account, as wide as its longest label (antigravity's are long)
    let kw = acc.windows.iter().map(|w| w.name.chars().count()).max().unwrap_or(4).max(6) as i32;
    for w in &acc.windows {
        let mut detail = String::new();
        if let Some(r) = w.reset {
            detail = format!("{} · {}", util::until(r), util::local(r, "%a %H:%M").to_lowercase());
        }
        if let (Some(p), true) = (w.pace(), w.span > 86400.0) {
            detail += &format!(" · {}{:.0}", if p >= 0.0 { "+" } else { "−" }, p.abs());
        }
        let row = ui::meter_row(&w.name, w.pct / 100.0, &format!("{:.0}%", w.pct), &detail, ui::pct_class(w.pct));
        if let Some(k) = row.first_child().and_downcast::<gtk::Label>() {
            k.set_width_chars(kw);
        }
        b.append(&row);
    }
    if let Some(e) = &acc.extra {
        b.append(&ui::kv("extra", e).0);
    }
    let note = if acc.fetched > 0.0 { format!("{}, {}", acc.source, util::ago(acc.fetched)) } else { "no data yet".into() };
    let note = match &acc.error {
        Some(e) if acc.source != "live" || util::now() - acc.fetched > 600.0 => format!("{note} · {e}"),
        _ => note,
    };
    b.append(&ui::kv("data", &ui::dim(&util::esc(&note))).0);
    b.upcast()
}

fn panel(i: &Rc<Inner>, p: &Panel) -> gtk::Widget {
    p.set_width(460);
    let card = Card::new("ai", "");
    let agents_box = ui::vbox(0, &[]);
    let usage_box = ui::vbox(0, &[]);
    card.body.append(&agents_box);
    card.body.append(&usage_box);

    let fill_agents = {
        let (i, b, detail, pc) = (Rc::downgrade(i), agents_box.clone(), card.detail.clone(), p.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            ui::clear(&b);
            let a = i.agents.borrow().clone();
            let working = a.iter().filter(|x| x.busy()).count();
            let waiting = a.iter().filter(|x| x.needs_you()).count();
            detail.set_markup(&if waiting > 0 {
                ui::colored(&format!("{waiting} waiting for you"), &i.theme.color1)
            } else {
                format!("{working} of {} agents working", a.len())
            });
            b.append(&ui::section("agents").tap(|l| l.add_css_class("first")));
            if a.is_empty() {
                b.append(&ui::text("no agents running", &["empty-note"]));
            }
            for ag in a {
                let (icon, meta) = if ag.needs_you() {
                    ("󰀦", ui::colored("needs you", &i.theme.color1))
                } else if ag.busy() {
                    (SPIN[i.spin.get()], format!("busy {}", util::dur((util::now() - ag.since).max(0.0) as u64)))
                } else {
                    ("󰒲", ui::dim(&format!("idle {}", util::dur((util::now() - ag.since).max(0.0) as u64))))
                };
                let row = ui::vbox(1, &["row"]);
                if ag.needs_you() || ag.busy() {
                    row.add_css_class("active");
                }
                let top = ui::hbox(8, &[]);
                top.append(&ui::text(icon, &["icon", if ag.needs_you() { "crit" } else if ag.busy() { "ok" } else { "dim" }]));
                let t = ui::text(&ag.label(), &[]);
                t.set_hexpand(true);
                ui::ellipsize(&t, 32);
                top.append(&t);
                top.append(&ui::label(&meta, &["meta"]));
                row.append(&top);
                let sub = ui::label(
                    &ui::dim(&util::esc(&format!(
                        "{} · {} · {}{}",
                        ag.name,
                        ag.account,
                        ag.cwd.replace(&*util::home().to_string_lossy(), "~"),
                        if ag.doing.is_empty() { String::new() } else { format!(" · {}", short(&ag.doing, 40)) }
                    ))),
                    &[],
                );
                sub.set_margin_start(30);
                ui::ellipsize(&sub, 56);
                row.append(&sub);
                let up = util::dur((util::now() - ag.started).max(0.0) as u64);
                row.set_tooltip_text(Some(&format!("running {up}\n{}", short(&ag.prompt, 200))));
                let (pid, pc2) = (ag.pid, pc.clone());
                ui::on_click(&row, move || {
                    pc2.close();
                    focus_agent(pid);
                });
                b.append(&row);
            }
        }
    };
    let fill_usage = {
        let (i, b) = (Rc::downgrade(i), usage_box.clone());
        move || {
            let Some(i) = i.upgrade() else { return };
            ui::clear(&b);
            let u = i.usage.borrow().clone();
            if !u.claude.is_empty() {
                b.append(&ui::section("claude"));
                for a in &u.claude {
                    b.append(&usage_block(a, SPARK, &i.theme));
                }
                let best = u.claude.iter().filter_map(|c| c.worst().map(|w| (w, c.label.clone()))).min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                if let Some((_, l)) = best {
                    b.append(&ui::kv("best", &format!("{l} {}", ui::dim("has the most headroom now"))).0);
                }
            }
            if !u.codex.is_empty() {
                b.append(&ui::section("codex"));
                for a in &u.codex {
                    b.append(&usage_block(a, CODEX, &i.theme));
                }
            }
            if let Some(g) = &u.agy {
                b.append(&ui::section("antigravity"));
                b.append(&usage_block(g, AGY, &i.theme));
            }
            if u.claude.is_empty() && u.codex.is_empty() && u.agy.is_none() {
                b.append(&ui::text("loading usage…", &["empty-note"]));
            }
        }
    };
    let (fa, fu) = (Rc::new(fill_agents), Rc::new(fill_usage));
    let hovered_early = Rc::new(Cell::new(false));
    fa();
    fu();
    {
        let (fa, fu) = (fa.clone(), fu.clone());
        let h = hovered_early.clone();
        p.watch(&i.changed, move || {
            if !h.get() {
                fa();
            }
            fu();
        });
    }
    // live durations, but never rebuilt under the pointer (it would eat clicks)
    let hovered = hovered_early.clone();
    let m = gtk::EventControllerMotion::new();
    {
        let h = hovered.clone();
        m.connect_enter(move |_, _, _| h.set(true));
        let h = hovered.clone();
        m.connect_leave(move |_| h.set(false));
    }
    agents_box.add_controller(m);
    let (fa2, h2) = (fa.clone(), hovered.clone());
    p.every(1000, move || {
        if !h2.get() {
            fa2()
        }
    });
    let w = Rc::downgrade(i);
    card.foot_add(&ui::link("refresh usage", &[], move || {
        if let Some(i) = w.upgrade() {
            i.fetch_usage(true);
        }
    }));
    card.foot_add(&ui::spacer());
    card.foot_add(&ui::text("click an agent to jump to it", &["key"]));
    card.root.upcast()
}
