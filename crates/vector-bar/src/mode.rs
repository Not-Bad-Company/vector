//! The system "mode", nvim style: NORMAL most of the time; REC, DICTATE,
//! SILENT, AWAKE, VM... while those are on. Its colour paints the bar's mode
//! and clock blocks and the panels' header blocks.

use std::cell::RefCell;

use crate::slot::Notify;

/// One active state. Lower `rank` wins the colour.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub key: &'static str,
    pub label: &'static str,
    pub rank: u8,
}

pub const NORMAL: State = State { key: "normal", label: "NORMAL", rank: 255 };

thread_local! {
    static ACTIVE: RefCell<Vec<State>> = const { RefCell::new(Vec::new()) };
    static CHANGED: Notify = Notify::default();
}

pub fn changed() -> Notify {
    // during process exit the thread-local may be gone: hand out a dummy
    CHANGED.try_with(|c| c.clone()).unwrap_or_default()
}

/// Turns one state on or off.
pub fn set(state: State, on: bool) {
    let changed = ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        let had = a.iter().any(|s| s.key == state.key);
        if on && !had {
            a.push(state);
            a.sort_by_key(|s| s.rank);
            true
        } else if !on && had {
            a.retain(|s| s.key != state.key);
            true
        } else {
            false
        }
    });
    if changed {
        changed_notify();
    }
}

fn changed_notify() {
    changed().emit();
}

/// All active states, highest first (NORMAL when none).
pub fn active() -> Vec<State> {
    let v = ACTIVE.with(|a| a.borrow().clone());
    if v.is_empty() { vec![NORMAL] } else { v }
}

/// The colour key of the bar right now.
pub fn current() -> &'static str {
    active()[0].key
}

/// Every mode key with its theme colour (for the CSS).
/// Every mode key with its theme colour (for the CSS). Informational states
/// keep the accent (only their word shows); red is for what needs you now.
pub const COLORS: [(&str, &str); 8] = [
    ("normal", "accent"),
    ("rec", "red"),
    ("agent", "red"),
    ("dictate", "accent"),
    ("silent", "accent"),
    ("awake", "accent"),
    ("vm", "accent"),
    ("low", "red"),
];

pub fn reset() {
    ACTIVE.with(|a| a.borrow_mut().clear());
    changed_notify();
}
