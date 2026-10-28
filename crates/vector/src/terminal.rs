//! Starting a terminal: `xdg-terminal-exec` when installed (it follows the user's choice),
//! else `$TERMINAL`, else the first common terminal found.

use std::ffi::{OsStr, OsString};

use gtk::glib;

/// Terminals tried in order, with the flag that runs a command in them.
const KNOWN: &[(&str, &[&str])] = &[
    ("foot", &["-e"]),
    ("kitty", &["-e"]),
    ("alacritty", &["-e"]),
    ("ghostty", &["-e"]),
    ("wezterm", &["start", "--"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];

/// The command line that opens a terminal, running `cmd` in it if given. `None` when no
/// terminal is installed.
pub fn argv(cmd: &[&OsStr]) -> Option<Vec<OsString>> {
    let with = |prog: OsString, flag: &[&str]| -> Vec<OsString> {
        let mut v = vec![prog];
        if !cmd.is_empty() {
            v.extend(flag.iter().map(OsString::from));
            v.extend(cmd.iter().map(|c| c.to_os_string()));
        }
        v
    };
    if glib::find_program_in_path("xdg-terminal-exec").is_some() {
        return Some(with("xdg-terminal-exec".into(), &[]));
    }
    if let Some(t) = std::env::var_os("TERMINAL").filter(|t| !t.is_empty()) {
        let name = std::path::Path::new(&t)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let flag = KNOWN
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(&["-e"][..], |(_, f)| f);
        return Some(with(t, flag));
    }
    KNOWN
        .iter()
        .find(|(name, _)| glib::find_program_in_path(name).is_some())
        .map(|(name, flag)| with((*name).into(), flag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_flags_are_sane() {
        for (name, flag) in KNOWN {
            assert!(!name.is_empty() && !flag.is_empty());
        }
    }
}
