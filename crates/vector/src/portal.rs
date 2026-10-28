//! The xdg-desktop-portal FileChooser backend: "open file" / "save as" dialogs of other apps
//! (browsers, Electron, Flatpak) show a vector window in picker mode.
//!
//! xdg-desktop-portal finds this through `vector.portal` and the user's portals.conf, starts
//! `vector --portal` over D-Bus, and calls OpenFile / SaveFile / SaveFiles here.

use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use vector_core::model::Allow;

const NAME: &str = "org.freedesktop.impl.portal.desktop.vector";
const PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";
const XML: &str = r#"<node>
  <interface name="org.freedesktop.impl.portal.FileChooser">
    <method name="OpenFile">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFile">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFiles">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
  </interface>
</node>"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Pick existing files (or folders with `directory`).
    Open,
    /// Pick one new or existing file name.
    Save,
    /// Pick a folder to save several files into.
    SaveFiles,
}

/// One named filter of the requesting app: ("Images", [*.png, image/jpeg]).
pub type NamedFilter = (String, Vec<Allow>);

pub struct Request {
    pub kind: Kind,
    pub title: String,
    pub accept: String,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<NamedFilter>,
    /// Index into `filters` to start with.
    pub current_filter: Option<usize>,
    pub current_name: Option<String>,
    pub folder: Option<PathBuf>,
}

/// Called with the picked paths, or `None` when cancelled. Exactly once.
pub type Respond = Box<dyn FnOnce(Option<Vec<PathBuf>>)>;

/// Shows a picker window for a request.
pub type Show = Rc<dyn Fn(Request, Respond)>;

pub fn own(show: Show) -> Option<gio::OwnerId> {
    let info = gio::DBusNodeInfo::for_xml(XML)
        .ok()?
        .lookup_interface(IFACE)?;
    Some(gio::bus_own_name(
        gio::BusType::Session,
        NAME,
        gio::BusNameOwnerFlags::NONE,
        move |conn, _| {
            let show = show.clone();
            let _ = conn
                .register_object(PATH, &info)
                .method_call(move |_, _, _, _, method, params, invocation| {
                    let kind = match method {
                        "SaveFile" => Kind::Save,
                        "SaveFiles" => Kind::SaveFiles,
                        _ => Kind::Open,
                    };
                    let req = parse(kind, &params);
                    let respond: Respond = Box::new(move |picked| {
                        let results = glib::VariantDict::new(None);
                        let response: u32 = match picked {
                            Some(paths) => {
                                let uris: Vec<String> = paths
                                    .iter()
                                    .map(|p| gio::File::for_path(p).uri().to_string())
                                    .collect();
                                results.insert_value("uris", &uris.to_variant());
                                0
                            }
                            None => 1,
                        };
                        // (ua{sv}): build the tuple by hand so the dict isn't boxed as a variant
                        let reply =
                            glib::Variant::tuple_from_iter([response.to_variant(), results.end()]);
                        invocation.return_value(Some(&reply));
                    });
                    show(req, respond);
                })
                .build();
        },
        |_, _| {},
        |_, _| {},
    ))
}

fn parse(kind: Kind, params: &glib::Variant) -> Request {
    let title: String = params.child_value(3).get().unwrap_or_default();
    let options = glib::VariantDict::new(Some(&params.child_value(4)));
    let get_bool = |k: &str| options.lookup::<bool>(k).ok().flatten().unwrap_or(false);
    let get_str = |k: &str| options.lookup::<String>(k).ok().flatten();
    let bytes_path = |k: &str| {
        // `ay`: read the raw bytes (a typed lookup refuses some encodings of it)
        options
            .lookup_value(k, None)
            .filter(|v| v.type_() == glib::VariantTy::BYTE_STRING)
            .map(|v| {
                let mut b = v.data().to_vec();
                // a NUL-terminated byte string
                while b.last() == Some(&0) {
                    b.pop();
                }
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(std::ffi::OsString::from_vec(b))
            })
    };
    let filters: Vec<(String, Vec<(u32, String)>)> = options
        .lookup_value("filters", None)
        .and_then(|v| v.get())
        .unwrap_or_default();
    let filters: Vec<NamedFilter> = filters
        .into_iter()
        .map(|(name, pats)| {
            let allow = pats
                .into_iter()
                .map(|(t, p)| {
                    if t == 1 {
                        Allow::Mime(p)
                    } else {
                        Allow::Glob(p)
                    }
                })
                .collect();
            (name, allow)
        })
        .collect();
    let current: Option<(String, Vec<(u32, String)>)> = options
        .lookup_value("current_filter", None)
        .and_then(|v| v.get());
    let current_filter = current.and_then(|(name, _)| filters.iter().position(|(n, _)| *n == name));
    let mut folder = bytes_path("current_folder");
    if folder.is_none() {
        folder = bytes_path("current_file").and_then(|f| f.parent().map(|p| p.to_path_buf()));
    }
    let default_accept = match kind {
        Kind::Open => "open",
        Kind::Save | Kind::SaveFiles => "save",
    };
    Request {
        kind,
        title,
        accept: get_str("accept_label")
            .map(|s| s.replace('_', "").to_lowercase())
            .unwrap_or_else(|| default_accept.into()),
        multiple: get_bool("multiple"),
        directory: get_bool("directory") || kind == Kind::SaveFiles,
        filters,
        current_filter,
        current_name: get_str("current_name"),
        folder,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(options: &str) -> glib::Variant {
        let text = format!("(objectpath '/r/1', 'app', '', 'Upload a photo', {options})");
        glib::Variant::parse(None, &text).expect("variant")
    }

    #[test]
    fn parses_folder_filters_and_flags() {
        let p = params(
            "{'multiple': <true>, 'current_folder': <b'/tmp/x/photos'>, \
             'filters': <[('Images', [(uint32 0, '*.png'), (uint32 1, 'image/jpeg')]), ('All', [(uint32 0, '*')])]>, \
             'current_filter': <('All', [(uint32 0, '*')])>, 'accept_label': <'_Upload'>}",
        );
        let r = parse(Kind::Open, &p);
        assert_eq!(r.folder, Some(PathBuf::from("/tmp/x/photos")));
        assert!(r.multiple && !r.directory);
        assert_eq!(r.title, "Upload a photo");
        assert_eq!(r.accept, "upload");
        assert_eq!(r.filters.len(), 2);
        assert_eq!(
            r.filters[0].1,
            vec![
                Allow::Glob("*.png".into()),
                Allow::Mime("image/jpeg".into())
            ]
        );
        assert_eq!(r.current_filter, Some(1));
    }

    #[test]
    fn save_name_and_folder_from_current_file() {
        let p = params("{'current_name': <'notes.txt'>, 'current_file': <b'/tmp/y/old.txt'>}");
        let r = parse(Kind::Save, &p);
        assert_eq!(r.current_name.as_deref(), Some("notes.txt"));
        assert_eq!(r.folder, Some(PathBuf::from("/tmp/y")));
        assert_eq!(r.accept, "save");
        assert!(parse(Kind::SaveFiles, &params("@a{sv} {}")).directory);
    }
}
