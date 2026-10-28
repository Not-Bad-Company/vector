//! What `?` shows: the help of both apps as topics of (keys, explanation) lines, and a search
//! over all of it. Pure data; the apps draw it.

/// One tab of the help.
pub struct Topic {
    pub title: &'static str,
    /// A sentence or two above the lines.
    pub intro: &'static str,
    /// (keys or example, what it does). An empty first field is a sub-heading.
    pub lines: &'static [(&'static str, &'static str)],
}

/// Lines matching every word of `query` (case-insensitive, in the keys, the text, or the tab's
/// title and intro), as (topic, line) indexes in order. Sub-headings never match on their own.
pub fn search(topics: &[Topic], query: &str) -> Vec<(usize, usize)> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (t, topic) in topics.iter().enumerate() {
        for (l, (keys, text)) in topic.lines.iter().enumerate() {
            if keys.is_empty() {
                continue;
            }
            let hay = format!("{keys} {text} {} {}", topic.title, topic.intro).to_lowercase();
            if words.iter().all(|w| hay.contains(w.as_str())) {
                out.push((t, l));
            }
        }
    }
    out
}

/// Vector Find.
pub const FIND: &[Topic] = &[
    Topic {
        title: "start",
        intro: "Everything works with the mouse; keys are for when your hands are on the \
                keyboard. The badge at the bottom left shows the mode, which decides what keys do. \
                Type in the box above to search all of this help.",
        lines: &[
            ("", "modes"),
            (
                "NORMAL",
                "the default: keys move and act (j k move, d trashes, y copies…); esc comes back here",
            ),
            (
                "VISUAL  v",
                "moving extends a selection; then y, x, d, R… act on all of it",
            ),
            (
                "COMMAND  :",
                "type a command such as :sort size or :rename; tab completes",
            ),
            (
                "FILTER  /",
                "typing narrows the folder to what matches (see the filter tab)",
            ),
            (
                "SIDEBAR  ctrl+h",
                "j k move through the places, l opens, J K move one of your own places, ctrl+l goes back",
            ),
            (
                "FIND  GREP",
                "a list of search results (// or :find, :grep): j k move, enter opens, esc closes",
            ),
            (
                "RENAME  r",
                "typing a new name in place; enter keeps it, esc cancels",
            ),
            ("", "the screen"),
            (
                "top",
                "the path: click a part to go there, ctrl+l to type one",
            ),
            (
                "left",
                "places (numbered: g1…g9), recent folders, drives, pins; b adds this folder to places, right click hides, renames or reorders",
            ),
            (
                "right",
                "the preview (i): pictures, video, PDF, text, folder contents, details",
            ),
            (
                "bottom",
                "the mode, the git branch, messages, item counts, free space, the sort, the position",
            ),
            (
                "numbers",
                "left of each row: how far it is from the cursor (5j gets there); :number off hides them",
            ),
        ],
    },
    Topic {
        title: "move",
        intro: "Moving around. A number before a key repeats it: 5j moves five down.",
        lines: &[
            ("j k  ↓ ↑", "down, up"),
            ("gg G", "first, last (12G: the twelfth)"),
            (
                "l enter →",
                "open a folder, a file or an archive (in the grid: l and → move right)",
            ),
            (
                "h - backspace ←",
                "parent folder (in the grid: h and ← move left)",
            ),
            ("H L", "back, forward through the folders you visited"),
            ("g1 … g9", "go to place 1 to 9 (the numbers in the sidebar)"),
            (
                "b",
                "add this folder to the places in the sidebar (again: take it off)",
            ),
            ("ctrl+d ctrl+u", "half a page down, up"),
            ("ctrl+h ctrl+←", "into the sidebar"),
            (
                "ctrl+l ctrl+→",
                "from the sidebar back to the files; in the files: type a path",
            ),
            ("s  tab", "split into two panes, switch pane"),
            (
                "ctrl+t gt gT ctrl+w",
                "new tab, next tab, previous tab, close tab",
            ),
            (
                ":cd path",
                "go to a folder (~ and .. work), or to a file's folder with it selected",
            ),
            (
                ":cd trash  :cd downloads",
                "a bare name that isn't here opens the place or drive of that name, else the best :z match",
            ),
            (
                ":z words",
                "the folder you use most whose path has those words, from anywhere (:z vec docs)",
            ),
            (
                ":places reset",
                "bring back the standard places you hid (right click › hide from places)",
            ),
        ],
    },
    Topic {
        title: "files",
        intro: "Acting on the selection, or on the item under the cursor when nothing is selected.",
        lines: &[
            (
                "v  space  ctrl+a",
                "select a range, mark one and move on, select all; esc clears",
            ),
            (
                "y x p",
                "copy, cut, paste (ctrl+c ctrl+x ctrl+v too, also with other apps)",
            ),
            ("d  del", "move to the trash; u brings it back right away"),
            (
                "Trash (sidebar)  :cd trash",
                "what you trashed; the details show where each came from",
            ),
            (
                "gr  :restore",
                "in the Trash: put the selection back where it came from",
            ),
            (
                ":empty-trash",
                "delete everything in the trash for good (asks first)",
            ),
            ("D  shift+del", "delete permanently, after asking"),
            ("r  F2", "rename one"),
            (
                "R",
                "rename the selection by a pattern (see the rename tab)",
            ),
            ("u .", "undo, repeat the last change"),
            ("e", "edit the file in $EDITOR"),
            (
                "ge",
                "edit all names in this folder in $EDITOR, like a text file",
            ),
            ("Y", "copy the path"),
            ("m M", "pin to the top of this folder, pin to the sidebar"),
            ("gs", "measure folder sizes"),
            (
                "X gX",
                "extract an archive, extract and trash it (see archives)",
            ),
            (
                "enter",
                "open with the default app; no app set up for it: choose one",
            ),
            (":new name  :mkdir name", "create a file, a folder"),
        ],
    },
    Topic {
        title: "view",
        intro: "How the folder is shown. These are remembered between runs.",
        lines: &[
            ("i", "preview pane on or off"),
            ("w  :view list|grid", "list or grid"),
            ("zh  :hidden", "hidden files"),
            (
                "click a column  :sort name|size|mtime|type",
                "sort; click again or add rev to reverse",
            ),
            (
                "added  :sort added",
                "newest arrivals first: when a file was moved, copied or downloaded into the folder",
            ),
            (
                "Trash",
                "opens with what you trashed last on top (the date column says trashed)",
            ),
            (":flat", "every file below this folder in one list"),
            (
                ":biggest [n] [here|/|all]",
                "the biggest files here, on the system disk, or everywhere",
            ),
            ("F4", "a terminal panel under the files, following you"),
            ("gx  :term", "open your terminal here"),
            ("gm", "the context menu"),
            ("ctrl+r F5", "reload"),
            (
                "ZZ  :q :qa",
                "close the window, close the pane or tab, close everything",
            ),
        ],
    },
    Topic {
        title: "find",
        intro: "/ is for finding: one slash looks in this folder, two slashes in every folder \
                below it. From Home, // searches all your files.",
        lines: &[
            (
                "/name",
                "filter this folder as you type (see the filter tab for the language)",
            ),
            (
                "//name  enter",
                "fuzzy-find names in every folder below; rndahh finds random_ahh.pdf",
            ),
            (
                "/name  enter",
                "nothing here matches: searches every folder below instead",
            ),
            (
                ":grep text",
                "find files containing text, in every folder below",
            ),
            (
                "j k  enter  esc",
                "in the results: move, open (jumps to the file), close",
            ),
            (":biggest", "find what takes the space"),
        ],
    },
    Topic {
        title: "filter",
        intro: "/ filters the folder as you type. Separate terms with spaces: an item must match \
                all of them. esc clears the filter.",
        lines: &[
            ("report", "names containing \"report\""),
            ("*.rs", "names matching a pattern"),
            ("ext:pdf  ext:png,jpg", "by extension"),
            (
                "type:image  type:video|audio",
                "by kind: dir file link image video audio doc archive code text exec hidden",
            ),
            ("size>10m  size<1k", "by size (k, m, g)"),
            (
                "mod<7d  mod>1y",
                "by age: changed in the last 7 days, more than a year ago",
            ),
            ("!*.tmp", "everything except"),
            (
                "type:image !*.gif size>1m",
                "images over 1 MB that aren't GIFs",
            ),
        ],
    },
    Topic {
        title: "commands",
        intro: ": opens the command line; tab completes, and any unique start works (:bi is \
                :biggest).",
        lines: &[
            (":find text", "fuzzy-find file names below this folder"),
            (":grep pattern", "search inside files below this folder"),
            (":cd path", "go somewhere"),
            (
                ":sort key [rev]  :sort!",
                "sort by name, size, mtime or type",
            ),
            (":filter expr", "set the filter"),
            (":view list|grid", "list or grid"),
            (":hidden on|off", "hidden files"),
            (":number on|off", "relative line numbers, like nvim"),
            (":flat", "every file below this folder in one list"),
            (":biggest [n] [here|/|all]", "the biggest files"),
            (":new :touch :mkdir", "create a file or a folder"),
            (
                ":e  :e name",
                "edit names in $EDITOR; open or create a file there",
            ),
            (":rename pattern", "rename the selection by a pattern"),
            (
                ":extract  :extract!",
                "extract archives; ! also trashes them",
            ),
            (":open [name]", "open with the default app"),
            (":restore  :empty-trash", "in the Trash: put back; empty it"),
            (
                ":term  :agent [name]",
                "a terminal here; a coding agent in the terminal panel",
            ),
            (":q  :qa", "close the pane or tab; close the window"),
        ],
    },
    Topic {
        title: "archives",
        intro: "Zip and tar files (also .tar.gz, .xz, .zst, .bz2) open like folders, without \
                extracting anything. Nothing inside is run, and nothing in them can be changed. \
                To unzip / unpack / decompress: cursor on the archive, X.",
        lines: &[
            ("enter →", "open the archive, then its folders"),
            ("enter on a file", "open a read-only copy of just that file"),
            ("y then p elsewhere", "copy things out of the archive"),
            (
                "X  :extract  right click",
                "unpack (unzip, extract) next to the archive, keeping it",
            ),
            (
                "gX  :extract!",
                "extract, then move the archive to the trash (only if everything came out)",
            ),
            ("", "what extract does"),
            (
                "one folder inside",
                "becomes that folder next to the archive",
            ),
            (
                "loose files inside",
                "go into a new folder named after the archive",
            ),
            (
                "never",
                "overwrites anything, writes outside that folder, or keeps set-uid bits",
            ),
        ],
    },
    Topic {
        title: "rename",
        intro: "Select files (v or space), press R and type a pattern. You see every change \
                before it happens, and one u undoes all of it.",
        lines: &[
            (
                "photo-{n}",
                "photo-1.jpg, photo-2.jpg… (the extension is kept)",
            ),
            ("{n:3} {name}", "001 old name.jpg…"),
            ("{N}", "numbers padded to the count: 01…12"),
            (
                "{date}-{name}",
                "the date the file was changed, then its old name",
            ),
            (
                "p{n}.png",
                "a dot or {ext} in the pattern sets the extension yourself",
            ),
            ("s/2024/2025/", "replace text in the names"),
            ("ge", "anything else: edit the names in $EDITOR"),
        ],
    },
    Topic {
        title: "config",
        intro: "~/.config/vector/init.lua and plugins/*.lua run at start. Errors show in the \
                window with the file and line.",
        lines: &[
            (
                "vector.map(\"gd\", \"cd ~/Downloads\")",
                "a key for a command or a function",
            ),
            ("vector.command(\"name\", fn)", "a new :name command"),
            (
                "vector.on(\"cd\", fn(dir))",
                "run something whenever the folder changes",
            ),
            (
                "vector.cmd(\"view grid\")",
                "run a command, e.g. a grid for picture folders",
            ),
            (
                "vector.sh(cmd, fn(out, code))",
                "run a shell command without blocking the window",
            ),
            (
                "vector.cwd() .cursor() .selection()",
                "where you are and what is selected",
            ),
            ("vector.notify(text)", "a message"),
            (
                "theme.toml",
                "~/.config/vector/theme.toml: your own colours (else the Omarchy theme)",
            ),
        ],
    },
];

/// Vector View.
pub const VIEW: &[Topic] = &[
    Topic {
        title: "start",
        intro: "Images, videos, PDFs and Word, Excel and PowerPoint files. Opening one file \
                opens its whole folder: ← → step through it. Type above to search this help.",
        lines: &[
            ("", "modes"),
            ("NORMAL", "every file opens fitted to the window"),
            (
                "ZOOM  z",
                "the zoom and position stay while you change files, to compare details",
            ),
            ("", "the screen"),
            ("top", "name, position in the folder, order, size, zoom"),
            (
                "bottom",
                "buttons (hover shows their key), the timeline for videos",
            ),
            (
                "tab",
                "hide or show both bars (in fullscreen: keep them shown)",
            ),
        ],
    },
    Topic {
        title: "move",
        intro: "Stepping through the folder.",
        lines: &[
            ("← →", "previous, next file (wraps around)"),
            ("home end", "first, last file (top, bottom of a PDF)"),
            (
                "g",
                "gallery of the folder; enter or a click opens, esc goes back",
            ),
            (
                "o",
                "order: name, newest, oldest, largest, smallest, random, type",
            ),
        ],
    },
    Topic {
        title: "zoom",
        intro: "Images and videos zoom toward the pointer.",
        lines: &[
            ("wheel  + -  pinch", "zoom"),
            ("0  middle click", "fit to the window"),
            ("1", "actual size (one image pixel per screen pixel)"),
            ("drag  h j k l", "pan; also for a zoomed PDF"),
            ("z", "keep the zoom between files"),
        ],
    },
    Topic {
        title: "video",
        intro: "Videos and GIFs play in mpv inside the window.",
        lines: &[
            ("space", "pause, play"),
            ("[ ]  shift+← shift+→", "back, forward 5 seconds"),
            (", .", "one frame back, forward"),
            ("↑ ↓", "volume"),
            ("click or drag the timeline", "seek"),
            ("s", "save the frame to Pictures/Screenshots and copy it"),
            (
                "~/.config/vector/mpv/mpv.conf",
                "mpv options, e.g. hwdec or the GPU to use",
            ),
        ],
    },
    Topic {
        title: "documents",
        intro: "PDFs scroll as a strip of pages. Word, Excel and PowerPoint files are shown \
                through LibreOffice (converted in the background, then cached).",
        lines: &[
            ("↑ ↓  j k  wheel", "scroll"),
            ("space  pgdn pgup", "page down, up"),
            ("ctrl+wheel  + -", "zoom"),
            (
                "e",
                "open in the default app (LibreOffice to edit office files)",
            ),
        ],
    },
    Topic {
        title: "file",
        intro: "Doing something with the file on show.",
        lines: &[
            ("e", "open in the default app"),
            ("m  right click", "menu: open with any app, order, gallery…"),
            ("ctrl+o", "show in folder"),
            ("ctrl+c", "copy the image, frame or page"),
            ("i", "details: size, dimensions, codecs, pages"),
            ("del del", "move to the trash (press twice)"),
            (
                "f  double click",
                "fullscreen: the bars hide, and come back when the pointer nears the top or bottom edge",
            ),
            ("esc  q", "leave fullscreen, close"),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_spans_tabs_and_needs_every_word() {
        // "zip" is only in the archives tab's intro: its lines are found
        let hits = search(FIND, "zip");
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|&(t, _)| FIND[t].title == "archives"));
        let both = search(FIND, "delete permanently");
        assert_eq!(both.len(), 1);
        assert_eq!(FIND[both[0].0].lines[both[0].1].0, "D  shift+del");
        assert!(search(FIND, "   ").is_empty());
        assert!(search(FIND, "nosuchthingatall").is_empty());
    }

    #[test]
    fn sub_headings_are_not_results() {
        for &(t, l) in &search(FIND, "modes") {
            assert!(!FIND[t].lines[l].0.is_empty());
        }
    }

    #[test]
    fn every_command_is_documented() {
        let text: String = FIND
            .iter()
            .flat_map(|t| t.lines.iter().map(|(k, _)| *k))
            .collect::<Vec<_>>()
            .join(" ");
        for c in crate::command::COMMANDS {
            let short = match *c {
                "quit" => ":q",
                "quitall" => ":qa",
                "edit" => ":e",
                other => other,
            };
            assert!(text.contains(short), "{c} is missing from the help");
        }
    }
}
