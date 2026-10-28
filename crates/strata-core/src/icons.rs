//! Nerd Font icons for directory entries. Material Design (`nf-md-*`) glyphs only.

const FOLDER: &str = "\u{F024B}"; // nf-md-folder
const FILE: &str = "\u{F0214}"; // nf-md-file
const FOLDER_LINK: &str = "\u{F0339}"; // nf-md-link_variant
const FILE_LINK: &str = "\u{F1177}"; // nf-md-file_link

/// Icon for a folder or file. Symlinks get a link icon.
pub fn icon_for(name: &str, is_dir: bool, is_symlink: bool) -> &'static str {
    if is_symlink {
        return if is_dir { FOLDER_LINK } else { FILE_LINK };
    }
    if is_dir {
        return folder_icon(name);
    }
    file_icon(name)
}

/// Folders with their own icon, matched by name.
const FOLDERS: &[(&str, &str)] = &[
    ("Documents", "\u{F0C82}"),    // nf-md-folder_text
    ("Downloads", "\u{F024D}"),    // nf-md-folder_download
    ("Pictures", "\u{F024F}"),     // nf-md-folder_image
    ("Music", "\u{F1359}"),        // nf-md-folder_music
    ("Desktop", "\u{F07C0}"),      // nf-md-desktop_classic
    (".git", "\u{F02A2}"),         // nf-md-git
    ("node_modules", "\u{F0399}"), // nf-md-nodejs
    (".config", "\u{F107F}"),      // nf-md-folder_cog
    ("src", "\u{F10D6}"),          // nf-md-code_braces_box
];

fn folder_icon(name: &str) -> &'static str {
    FOLDERS
        .iter()
        .find(|(folder, _)| folder.eq_ignore_ascii_case(name))
        .map_or(FOLDER, |(_, icon)| icon)
}

/// Files with their own icon, matched by whole name.
const NAMES: &[(&str, &str)] = &[
    ("Makefile", "\u{F107B}"),      // nf-md-file_cog
    ("Dockerfile", "\u{F0868}"),    // nf-md-docker
    ("Cargo.toml", "\u{F1617}"),    // nf-md-language_rust
    ("LICENSE", "\u{F0FC3}"),       // nf-md-license
    (".gitignore", "\u{F02A2}"),    // nf-md-git
    (".git", "\u{F02A2}"),          // nf-md-git
    (".bashrc", "\u{F1183}"),       // nf-md-bash
    (".bash_profile", "\u{F1183}"), // nf-md-bash
    (".bash_aliases", "\u{F1183}"), // nf-md-bash
    (".zshrc", "\u{F1183}"),        // nf-md-bash
    (".zprofile", "\u{F1183}"),     // nf-md-bash
    (".profile", "\u{F1183}"),      // nf-md-bash
];

/// Files with their own icon, matched by extension (without the dot).
const EXTENSIONS: &[(&[&str], &str)] = &[
    // Images
    (
        &[
            "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic",
        ],
        "\u{F021F}", // nf-md-file_image
    ),
    // Video
    (
        &["mp4", "mkv", "webm", "mov", "avi", "wmv", "flv", "m4v"],
        "\u{F0567}", // nf-md-video
    ),
    // Audio
    (
        &[
            "mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "opus", "wma",
        ],
        "\u{F0387}", // nf-md-music_note
    ),
    // PDF
    (&["pdf"], "\u{F0226}"), // nf-md-file_pdf_box
    // Archives
    (
        &["zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar"],
        "\u{F05C4}", // nf-md-zip_box
    ),
    // Disc images
    (&["iso", "img"], "\u{F05EE}"), // nf-md-disc
    // Executables
    (&["exe", "appimage"], "\u{F08C6}"), // nf-md-application
    // Lock files
    (&["lock"], "\u{F033E}"), // nf-md-lock
    // Text and markdown
    (&["txt", "rst", "log"], "\u{F0219}"), // nf-md-file_document
    (&["md", "markdown"], "\u{F0354}"),    // nf-md-language_markdown
    // Office documents
    (&["odt", "docx"], "\u{F022C}"), // nf-md-file_word
    (&["ods", "xlsx"], "\u{F021B}"), // nf-md-file_excel
    (&["odp", "pptx"], "\u{F0227}"), // nf-md-file_powerpoint
    // Code
    (&["rs"], "\u{F1617}"),                            // nf-md-language_rust
    (&["py"], "\u{F0320}"),                            // nf-md-language_python
    (&["js", "mjs", "cjs", "jsx"], "\u{F031E}"),       // nf-md-language_javascript
    (&["ts", "tsx"], "\u{F06E6}"),                     // nf-md-language_typescript
    (&["c", "h"], "\u{F0671}"),                        // nf-md-language_c
    (&["cpp", "cc", "cxx", "hpp", "hh"], "\u{F0672}"), // nf-md-language_cpp
    (&["go"], "\u{F07D3}"),                            // nf-md-language_go
    (&["java"], "\u{F0B37}"),                          // nf-md-language_java
    (&["lua"], "\u{F08B1}"),                           // nf-md-language_lua
    (&["sh", "bash", "zsh", "fish"], "\u{F1183}"),     // nf-md-bash
    (&["html", "htm"], "\u{F031D}"),                   // nf-md-language_html5
    (&["css"], "\u{F031C}"),                           // nf-md-language_css3
    (&["json"], "\u{F0626}"),                          // nf-md-code_json
    (&["toml", "yaml", "yml"], "\u{F0174}"),           // nf-md-code_tags
    (&["xml"], "\u{F05C0}"),                           // nf-md-xml
    (&["sql"], "\u{F01BC}"),                           // nf-md-database
];

fn file_icon(name: &str) -> &'static str {
    if let Some((_, icon)) = NAMES
        .iter()
        .find(|(file, _)| file.eq_ignore_ascii_case(name))
    {
        return icon;
    }
    if name
        .as_bytes()
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"readme"))
    {
        return "\u{F0219}"; // nf-md-file_document
    }
    let Some(ext) = extension(name) else {
        return FILE;
    };
    EXTENSIONS
        .iter()
        .find(|(exts, _)| exts.iter().any(|e| e.eq_ignore_ascii_case(ext)))
        .map_or(FILE, |(_, icon)| icon)
}

/// Last extension of a name, without the dot. Leading dots don't start an extension.
fn extension(name: &str) -> Option<&str> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        None
    } else {
        Some(ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> &'static str {
        icon_for(name, false, false)
    }

    fn dir(name: &str) -> &'static str {
        icon_for(name, true, false)
    }

    #[test]
    fn generic_folder_and_file() {
        assert_eq!(dir("projects"), FOLDER);
        assert_eq!(dir("Videos"), FOLDER);
        assert_eq!(file("notes"), FILE);
        assert_eq!(file("unknown.xyz"), FILE);
    }

    #[test]
    fn special_folders() {
        assert_eq!(dir("Documents"), "\u{F0C82}");
        assert_eq!(dir("Downloads"), "\u{F024D}");
        assert_eq!(dir("Pictures"), "\u{F024F}");
        assert_eq!(dir("Music"), "\u{F1359}");
        assert_eq!(dir("Desktop"), "\u{F07C0}");
        assert_eq!(dir(".git"), "\u{F02A2}");
        assert_eq!(dir("node_modules"), "\u{F0399}");
        assert_eq!(dir(".config"), "\u{F107F}");
        assert_eq!(dir("src"), "\u{F10D6}");
    }

    #[test]
    fn special_folders_are_case_insensitive() {
        assert_eq!(dir("documents"), dir("Documents"));
        assert_eq!(dir("DOWNLOADS"), dir("Downloads"));
        assert_eq!(dir("node_MODULES"), dir("node_modules"));
    }

    #[test]
    fn special_folder_names_do_not_apply_to_files() {
        assert_eq!(file("Music"), FILE);
        assert_eq!(file("src"), FILE);
    }

    #[test]
    fn images() {
        for name in ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.svg"] {
            assert_eq!(file(name), "\u{F021F}", "{name}");
        }
    }

    #[test]
    fn video() {
        for name in ["a.mp4", "b.MKV", "c.webm", "d.mov"] {
            assert_eq!(file(name), "\u{F0567}", "{name}");
        }
    }

    #[test]
    fn audio() {
        for name in ["a.mp3", "b.FLAC", "c.ogg", "d.wav", "e.m4a"] {
            assert_eq!(file(name), "\u{F0387}", "{name}");
        }
    }

    #[test]
    fn pdf() {
        assert_eq!(file("manual.pdf"), "\u{F0226}");
        assert_eq!(file("MANUAL.PDF"), "\u{F0226}");
    }

    #[test]
    fn archives() {
        for name in ["a.zip", "b.tar", "c.gz", "d.xz", "e.zst", "f.7z", "g.rar"] {
            assert_eq!(file(name), "\u{F05C4}", "{name}");
        }
    }

    #[test]
    fn multi_dot_archive_uses_last_extension() {
        assert_eq!(file("a.tar.gz"), "\u{F05C4}");
        assert_eq!(file("backup.TAR.ZST"), "\u{F05C4}");
    }

    #[test]
    fn text_and_markdown() {
        assert_eq!(file("notes.txt"), "\u{F0219}");
        assert_eq!(file("CHANGELOG.md"), "\u{F0354}");
        assert_eq!(file("guide.MARKDOWN"), "\u{F0354}");
    }

    #[test]
    fn code() {
        assert_eq!(file("main.rs"), "\u{F1617}");
        assert_eq!(file("app.py"), "\u{F0320}");
        assert_eq!(file("index.js"), "\u{F031E}");
        assert_eq!(file("index.TS"), "\u{F06E6}");
        assert_eq!(file("util.h"), "\u{F0671}");
        assert_eq!(file("util.cpp"), "\u{F0672}");
        assert_eq!(file("main.go"), "\u{F07D3}");
        assert_eq!(file("Main.java"), "\u{F0B37}");
        assert_eq!(file("init.lua"), "\u{F08B1}");
        assert_eq!(file("build.sh"), "\u{F1183}");
        assert_eq!(file("build.fish"), "\u{F1183}");
        assert_eq!(file("page.html"), "\u{F031D}");
        assert_eq!(file("style.css"), "\u{F031C}");
        assert_eq!(file("data.json"), "\u{F0626}");
        assert_eq!(file("config.toml"), "\u{F0174}");
        assert_eq!(file("ci.yaml"), "\u{F0174}");
        assert_eq!(file("ci.yml"), "\u{F0174}");
        assert_eq!(file("feed.xml"), "\u{F05C0}");
        assert_eq!(file("schema.sql"), "\u{F01BC}");
    }

    #[test]
    fn office_documents() {
        assert_eq!(file("letter.odt"), "\u{F022C}");
        assert_eq!(file("letter.docx"), "\u{F022C}");
        assert_eq!(file("sheet.ods"), "\u{F021B}");
        assert_eq!(file("sheet.XLSX"), "\u{F021B}");
        assert_eq!(file("slides.odp"), "\u{F0227}");
        assert_eq!(file("slides.pptx"), "\u{F0227}");
    }

    #[test]
    fn disc_images_and_executables() {
        assert_eq!(file("ubuntu.iso"), "\u{F05EE}");
        assert_eq!(file("disk.img"), "\u{F05EE}");
        assert_eq!(file("setup.exe"), "\u{F08C6}");
        assert_eq!(file("Tool.AppImage"), "\u{F08C6}");
    }

    #[test]
    fn lock_files() {
        assert_eq!(file("Cargo.lock"), "\u{F033E}");
        assert_eq!(file("yarn.LOCK"), "\u{F033E}");
    }

    #[test]
    fn special_names() {
        assert_eq!(file("Makefile"), "\u{F107B}");
        assert_eq!(file("makefile"), "\u{F107B}");
        assert_eq!(file("Dockerfile"), "\u{F0868}");
        assert_eq!(file("Cargo.toml"), "\u{F1617}");
        assert_eq!(file("LICENSE"), "\u{F0FC3}");
        assert_eq!(file("license"), "\u{F0FC3}");
        assert_eq!(file(".gitignore"), "\u{F02A2}");
        assert_eq!(file("README"), "\u{F0219}");
        assert_eq!(file("README.md"), "\u{F0219}");
        assert_eq!(file("readme-old.txt"), "\u{F0219}");
    }

    #[test]
    fn special_names_beat_extensions() {
        // Cargo.toml would otherwise match the toml code icon.
        assert_ne!(file("Cargo.toml"), "\u{F0174}");
    }

    #[test]
    fn dotfiles_without_extension() {
        assert_eq!(file(".bashrc"), "\u{F1183}");
        assert_eq!(file(".zshrc"), "\u{F1183}");
        assert_eq!(file(".vimrc"), FILE);
        assert_eq!(file(".hidden"), FILE);
    }

    #[test]
    fn dotfile_with_extension() {
        assert_eq!(file(".config.json"), "\u{F0626}");
    }

    #[test]
    fn name_without_extension() {
        assert_eq!(file("notes"), FILE);
        assert_eq!(file("trailing."), FILE);
    }

    #[test]
    fn symlinked_folder() {
        assert_eq!(icon_for("projects", true, true), FOLDER_LINK);
        assert_eq!(icon_for("Documents", true, true), FOLDER_LINK);
    }

    #[test]
    fn symlinked_file() {
        assert_eq!(icon_for("main.rs", false, true), FILE_LINK);
        assert_eq!(icon_for("photo.png", false, true), FILE_LINK);
    }

    #[test]
    fn extension_helper() {
        assert_eq!(extension("a.tar.gz"), Some("gz"));
        assert_eq!(extension(".bashrc"), None);
        assert_eq!(extension("plain"), None);
        assert_eq!(extension("trailing."), None);
    }
}
