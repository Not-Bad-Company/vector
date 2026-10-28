//! Lua configuration: `$XDG_CONFIG_HOME/vector/init.lua`, then `plugins/*.lua` in name order.
//!
//! Scripts get a `vector` table:
//!
//! ```lua
//! vector.map("gd", "cd ~/Downloads")          -- a key sequence runs a command line…
//! vector.map("<C-t>", function()               -- …or a function
//!   vector.sh("touch notes.md")
//! end)
//! vector.command("du", function(args)          -- :du
//!   vector.sh("du -sh " .. args, function(out) vector.notify(out) end)
//! end)
//! vector.on("cd", function(dir) end)           -- after every folder change
//! vector.cwd()         vector.cursor()         vector.selection()
//! vector.cd(path)      vector.open(path)       vector.cmd("sort size")
//! vector.notify(text)  vector.sh(cmd [, fn(out, code)])
//! ```
//!
//! Nothing here touches GTK: calls that change things are queued as [`Request`]s, which the
//! window carries out after the script returns. Scripts read a [`Host`] snapshot the window
//! fills in before each call.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Function, Lua, LuaString, RegistryKey, Value};

/// What the window knows when a script runs.
#[derive(Debug, Default, Clone)]
pub struct Host {
    pub cwd: PathBuf,
    pub cursor: Option<PathBuf>,
    /// The selection, else the cursor entry.
    pub selection: Vec<PathBuf>,
}

/// Something a script asked the window to do.
#[derive(Debug)]
pub enum Request {
    Cd(PathBuf),
    Open(PathBuf),
    /// A built-in command line, without the `:`.
    Command(String),
    Notify(String),
    /// `sh -c cmd` in the current folder, with the selection as `$1…`; the callback gets
    /// (stdout, exit code).
    Shell {
        cmd: String,
        callback: Option<Callback>,
    },
}

/// A Lua function kept for later (a `vector.sh` callback).
#[derive(Debug)]
pub struct Callback(RegistryKey);

#[derive(Default)]
struct State {
    host: Host,
    requests: Vec<Request>,
    mappings: Vec<(String, RegistryKey)>,
    commands: Vec<(String, RegistryKey)>,
    on_cd: Vec<RegistryKey>,
}

pub struct Script {
    lua: Lua,
    state: Rc<RefCell<State>>,
}

impl Script {
    /// A script engine with the `vector` table and nothing loaded.
    pub fn new() -> Script {
        let lua = Lua::new();
        let state: Rc<RefCell<State>> = Rc::default();
        if let Err(e) = install(&lua, &state) {
            // only fails when Lua is out of memory
            eprintln!("vector: lua: {e}");
        }
        Script { lua, state }
    }

    /// Loads `init.lua` and `plugins/*.lua` from `dir`. Returns one message per file that
    /// failed; the others stay loaded.
    pub fn load_dir(&self, dir: &Path) -> Vec<String> {
        let mut files = vec![dir.join("init.lua")];
        if let Ok(rd) = std::fs::read_dir(dir.join("plugins")) {
            let mut plugins: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "lua"))
                .collect();
            plugins.sort();
            files.extend(plugins);
        }
        files
            .iter()
            .filter_map(|f| {
                let code = std::fs::read_to_string(f).ok()?;
                self.load(&code, &f.display().to_string()).err()
            })
            .collect()
    }

    /// Runs a chunk of Lua; `name` appears in error messages.
    pub fn load(&self, code: &str, name: &str) -> Result<(), String> {
        self.lua
            .load(code)
            .set_name(format!("@{name}"))
            .exec()
            .map_err(message)
    }

    /// The key sequences scripts mapped, in the order of their index.
    pub fn mappings(&self) -> Vec<String> {
        self.state
            .borrow()
            .mappings
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }

    pub fn has_command(&self, name: &str) -> bool {
        self.state.borrow().commands.iter().any(|(n, _)| n == name)
    }

    /// Names of the commands scripts added, for completion.
    pub fn command_names(&self) -> Vec<String> {
        self.state
            .borrow()
            .commands
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Runs mapping `i` with `host` as the current state.
    pub fn run_mapping(&self, i: usize, host: Host) -> Result<(), String> {
        let f = self.function(|s| s.mappings.get(i).map(|(_, k)| k))?;
        self.call(host, f.into_iter().map(|f| (f, Value::Nil)).collect())
    }

    /// Runs the command `name` with the rest of the command line.
    pub fn run_command(&self, name: &str, args: &str, host: Host) -> Result<(), String> {
        let f = self.function(|s| {
            s.commands
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, k)| k)
        })?;
        let args = Value::String(self.lua.create_string(args).map_err(message)?);
        self.call(host, f.into_iter().map(|f| (f, args.clone())).collect())
    }

    /// Tells the `cd` handlers about a new folder.
    pub fn changed_dir(&self, host: Host) -> Result<(), String> {
        let calls = {
            let s = self.state.borrow();
            s.on_cd
                .iter()
                .filter_map(|k| self.lua.registry_value::<Function>(k).ok())
                .collect::<Vec<_>>()
        };
        if calls.is_empty() {
            return Ok(());
        }
        let dir = self.path_value(&host.cwd)?;
        self.call(host, calls.into_iter().map(|f| (f, dir.clone())).collect())
    }

    /// Calls a `vector.sh` callback with the command's output and exit code.
    pub fn finish_shell(
        &self,
        cb: Callback,
        out: &str,
        code: i32,
        host: Host,
    ) -> Result<(), String> {
        let f: Function = self.lua.registry_value(&cb.0).map_err(message)?;
        let _ = self.lua.remove_registry_value(cb.0);
        self.state.borrow_mut().host = host;
        f.call::<()>((out, code)).map_err(message)
    }

    /// What the scripts asked for since the last call.
    pub fn take_requests(&self) -> Vec<Request> {
        std::mem::take(&mut self.state.borrow_mut().requests)
    }

    fn function(
        &self,
        pick: impl FnOnce(&State) -> Option<&RegistryKey>,
    ) -> Result<Option<Function>, String> {
        let s = self.state.borrow();
        pick(&s)
            .map(|k| self.lua.registry_value::<Function>(k))
            .transpose()
            .map_err(message)
    }

    fn call(&self, host: Host, calls: Vec<(Function, Value)>) -> Result<(), String> {
        self.state.borrow_mut().host = host;
        for (f, arg) in calls {
            f.call::<()>(arg).map_err(message)?;
        }
        Ok(())
    }

    fn path_value(&self, p: &Path) -> Result<Value, String> {
        self.lua
            .create_string(p.as_os_str().as_encoded_bytes())
            .map(Value::String)
            .map_err(message)
    }
}

impl Default for Script {
    fn default() -> Self {
        Script::new()
    }
}

/// The first line of a Lua error, which names the file and line.
fn message(e: mlua::Error) -> String {
    let text = match &e {
        mlua::Error::CallbackError { cause, .. } => cause.to_string(),
        _ => e.to_string(),
    };
    text.lines().next().unwrap_or_default().to_string()
}

fn install(lua: &Lua, state: &Rc<RefCell<State>>) -> mlua::Result<()> {
    let v = lua.create_table()?;

    let st = state.clone();
    v.set(
        "map",
        lua.create_function(move |lua, (keys, what): (String, Value)| {
            let f = as_function(lua, what)?;
            let key = lua.create_registry_value(f)?;
            let mut s = st.borrow_mut();
            // mapping the same keys again replaces the old one but keeps its index
            if let Some(slot) = s.mappings.iter_mut().find(|(k, _)| *k == keys) {
                slot.1 = key;
            } else {
                s.mappings.push((keys, key));
            }
            Ok(())
        })?,
    )?;
    let st = state.clone();
    v.set(
        "command",
        lua.create_function(move |lua, (name, f): (String, Function)| {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                return Err(mlua::Error::runtime(format!("bad command name: {name:?}")));
            }
            let key = lua.create_registry_value(f)?;
            st.borrow_mut().commands.push((name, key));
            Ok(())
        })?,
    )?;
    let st = state.clone();
    v.set(
        "on",
        lua.create_function(move |lua, (event, f): (String, Function)| {
            match event.as_str() {
                "cd" => st.borrow_mut().on_cd.push(lua.create_registry_value(f)?),
                _ => return Err(mlua::Error::runtime(format!("unknown event: {event}"))),
            }
            Ok(())
        })?,
    )?;

    let st = state.clone();
    v.set(
        "cwd",
        lua.create_function(move |lua, ()| {
            lua.create_string(st.borrow().host.cwd.as_os_str().as_encoded_bytes())
        })?,
    )?;
    let st = state.clone();
    v.set(
        "cursor",
        lua.create_function(move |lua, ()| {
            st.borrow()
                .host
                .cursor
                .as_ref()
                .map(|p| lua.create_string(p.as_os_str().as_encoded_bytes()))
                .transpose()
        })?,
    )?;
    let st = state.clone();
    v.set(
        "selection",
        lua.create_function(move |lua, ()| {
            let s = st.borrow();
            let items = s
                .host
                .selection
                .iter()
                .map(|p| lua.create_string(p.as_os_str().as_encoded_bytes()))
                .collect::<mlua::Result<Vec<_>>>()?;
            lua.create_sequence_from(items)
        })?,
    )?;

    let request =
        |lua: &Lua, st: &Rc<RefCell<State>>, name: &str, make: fn(LuaString, &Path) -> Request| {
            let st = st.clone();
            lua.create_function(move |_, s: LuaString| {
                let cwd = st.borrow().host.cwd.clone();
                let r = make(s, &cwd);
                st.borrow_mut().requests.push(r);
                Ok(())
            })
            .and_then(|f| v.set(name, f))
        };
    request(lua, state, "cd", |s, cwd| {
        Request::Cd(resolve(cwd, &path(s)))
    })?;
    request(lua, state, "open", |s, cwd| {
        Request::Open(resolve(cwd, &path(s)))
    })?;
    request(lua, state, "cmd", |s, _| {
        Request::Command(s.to_string_lossy().trim_start_matches(':').to_string())
    })?;
    request(lua, state, "notify", |s, _| {
        Request::Notify(s.to_string_lossy())
    })?;

    let st = state.clone();
    v.set(
        "sh",
        lua.create_function(move |lua, (cmd, cb): (String, Option<Function>)| {
            let callback = cb
                .map(|f| lua.create_registry_value(f).map(Callback))
                .transpose()?;
            st.borrow_mut()
                .requests
                .push(Request::Shell { cmd, callback });
            Ok(())
        })?,
    )?;

    // print() shows up as a notice instead of vanishing into stdout
    let st = state.clone();
    lua.globals().set(
        "print",
        lua.create_function(move |_, args: mlua::Variadic<Value>| {
            let text: Vec<String> = args
                .iter()
                .map(|a| a.to_string().unwrap_or_else(|_| a.type_name().to_string()))
                .collect();
            st.borrow_mut()
                .requests
                .push(Request::Notify(text.join(" ")));
            Ok(())
        })?,
    )?;
    lua.globals().set("vector", v)
}

/// A mapping's target: a function, or a command line that becomes one.
fn as_function(lua: &Lua, what: Value) -> mlua::Result<Function> {
    match what {
        Value::Function(f) => Ok(f),
        Value::String(s) => {
            let line = s.to_string_lossy();
            let cmd = lua
                .globals()
                .get::<mlua::Table>("vector")?
                .get::<Function>("cmd")?;
            lua.create_function(move |_, ()| cmd.call::<()>(line.clone()))
        }
        other => Err(mlua::Error::runtime(format!(
            "map: expected a function or a command, got {}",
            other.type_name()
        ))),
    }
}

/// A Lua string as a path (any bytes).
fn path(s: LuaString) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(&s.as_bytes()))
}

/// `~/x`, absolute, or relative to `cwd`.
fn resolve(cwd: &Path, p: &Path) -> PathBuf {
    if let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    cwd.join(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(cwd: &str) -> Host {
        Host {
            cwd: cwd.into(),
            cursor: Some(format!("{cwd}/a.txt").into()),
            selection: vec![format!("{cwd}/a.txt").into(), format!("{cwd}/b c").into()],
        }
    }

    #[test]
    fn mappings_commands_and_requests() {
        let s = Script::new();
        s.load(
            r#"
            vector.map("gd", "cd ~/x")
            vector.map("<C-t>", function() vector.open("notes.md") end)
            vector.map("gd", ":sort size")   -- replaces, same index
            vector.command("count", function(args)
              vector.notify(args .. #vector.selection() .. vector.cursor())
            end)
            "#,
            "init.lua",
        )
        .unwrap();
        assert_eq!(s.mappings(), ["gd", "<C-t>"]);
        assert!(s.has_command("count") && !s.has_command("nope"));
        s.run_mapping(0, host("/w")).unwrap();
        s.run_mapping(1, host("/w")).unwrap();
        s.run_command("count", "n=", host("/w")).unwrap();
        let r = s.take_requests();
        assert!(matches!(&r[0], Request::Command(c) if c == "sort size"));
        assert!(matches!(&r[1], Request::Open(p) if p == Path::new("/w/notes.md")));
        assert!(matches!(&r[2], Request::Notify(t) if t == "n=2/w/a.txt"));
        assert!(s.take_requests().is_empty());
    }

    #[test]
    fn cd_events_shell_callbacks_and_print() {
        let s = Script::new();
        s.load(
            r#"
            vector.on("cd", function(dir) vector.sh("ls", function(out, code)
              print(dir, out, code)
            end) end)
            "#,
            "init.lua",
        )
        .unwrap();
        s.changed_dir(host("/p")).unwrap();
        let mut r = s.take_requests();
        let Some(Request::Shell {
            cmd,
            callback: Some(cb),
        }) = r.pop()
        else {
            panic!("expected a shell request");
        };
        assert_eq!(cmd, "ls");
        s.finish_shell(cb, "x", 0, host("/p")).unwrap();
        assert!(matches!(&s.take_requests()[..], [Request::Notify(t)] if t == "/p x 0"));
    }

    #[test]
    fn errors_name_the_file_and_line() {
        let s = Script::new();
        let e = s.load("vector.on('nope', print)", "init.lua").unwrap_err();
        assert!(e.contains("unknown event: nope"), "{e}");
        let e = s.load("\n\nlocal x = (", "plugins/a.lua").unwrap_err();
        assert!(e.contains("plugins/a.lua:3"), "{e}");
        s.load("vector.map('x', function() error('boom') end)", "b.lua")
            .unwrap();
        let e = s.run_mapping(0, Host::default()).unwrap_err();
        assert!(e.contains("b.lua:1") && e.contains("boom"), "{e}");
        assert!(s.load("vector.command('a b', print)", "c.lua").is_err());
    }

    #[test]
    fn load_dir_runs_init_then_plugins_in_order() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("plugins")).unwrap();
        std::fs::write(t.path().join("init.lua"), "order = 'i'").unwrap();
        std::fs::write(t.path().join("plugins/b.lua"), "order = order .. 'b'").unwrap();
        std::fs::write(t.path().join("plugins/a.lua"), "order = order .. 'a'").unwrap();
        std::fs::write(t.path().join("plugins/bad.lua"), "(").unwrap();
        std::fs::write(t.path().join("plugins/z.txt"), "(").unwrap();
        let s = Script::new();
        let errors = s.load_dir(t.path());
        assert_eq!(errors.len(), 1);
        s.load("vector.notify(order)", "t").unwrap();
        assert!(matches!(&s.take_requests()[..], [Request::Notify(t)] if t == "iab"));
        assert!(Script::new().load_dir(&t.path().join("missing")).is_empty());
    }
}
