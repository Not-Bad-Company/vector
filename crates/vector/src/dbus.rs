//! `org.freedesktop.FileManager1`: lets other programs ask the file manager to show folders
//! or files ("show in folder" in a browser, "open containing folder" for a download).

use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

const NAME: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";
const XML: &str = r#"<node>
  <interface name="org.freedesktop.FileManager1">
    <method name="ShowFolders"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItems"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
    <method name="ShowItemProperties"><arg type="as" name="URIs" direction="in"/><arg type="s" name="StartupId" direction="in"/></method>
  </interface>
</node>"#;

/// Opens a window on `folder`, selecting `item` when given.
pub type Open = Rc<dyn Fn(PathBuf, Option<PathBuf>)>;

/// Asks for the bus name; if another file manager holds it, waits in line for it.
pub fn own(open: Open) -> Option<gio::OwnerId> {
    let info = gio::DBusNodeInfo::for_xml(XML)
        .ok()?
        .lookup_interface(NAME)?;
    let id = gio::bus_own_name(
        gio::BusType::Session,
        NAME,
        gio::BusNameOwnerFlags::NONE,
        move |conn, _| {
            let open = open.clone();
            let _ = conn
                .register_object(PATH, &info)
                .method_call(move |_, _, _, _, method, params, invocation| {
                    let uris: Vec<String> = params.child_value(0).get().unwrap_or_default();
                    let paths = uris.iter().filter_map(|u| gio::File::for_uri(u).path());
                    for p in paths {
                        match method {
                            "ShowFolders" => open(p, None),
                            // items: their folder, with them selected
                            _ => match p.parent() {
                                Some(parent) => open(parent.to_path_buf(), Some(p.clone())),
                                None => open(p, None),
                            },
                        }
                    }
                    invocation.return_value(None);
                })
                .build();
        },
        |_, _| {},
        |_, _| {},
    );
    Some(id)
}
