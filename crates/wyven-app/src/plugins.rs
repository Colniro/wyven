//! Plugins: WebAssembly modules from `%APPDATA%\Wyven\plugins`, run with wasmi in a sandbox.
//!
//! A plugin gets no imports at all: it cannot touch files, the network or other programs. It
//! only sees the JSON requests Wyven sends (see the `wyven-plugin` SDK) and answers with actions.
//! Every call has a fuel budget (against endless loops) and the memory is limited. A plugin that
//! traps or runs out of fuel is switched off until the plugins are reloaded.

use std::path::{Path, PathBuf};

use wasmi::{Config, Engine, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};
pub use wyven_plugin::{Action, Editor, Event};
use wyven_plugin::{Info, Request, Response};

/// Instructions per call: plenty for text work, stops a stuck plugin within about a second.
const FUEL: u64 = 400_000_000;
/// Memory a plugin may use.
const MEMORY: usize = 128 << 20;

struct Loaded {
    info: Info,
    path: PathBuf,
    store: Store<StoreLimits>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    handle: TypedFunc<(i32, i32), i64>,
    /// Stopped after a trap or running out of fuel.
    stopped: Option<String>,
}

pub struct Plugins {
    engine: Engine,
    list: Vec<Loaded>,
}

pub fn dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("plugins"))
}

/// `plugins\x.wasm` and `plugins\<folder>\*.wasm`.
fn find_wasm(dir: &Path) -> Vec<PathBuf> {
    let is_wasm = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wasm"));
    let mut found = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            found.extend(std::fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_wasm(p)));
        } else if is_wasm(&p) {
            found.push(p);
        }
    }
    found.sort();
    found
}

impl Plugins {
    pub fn new() -> Self {
        let mut config = Config::default();
        config.consume_fuel(true);
        Self { engine: Engine::new(&config), list: Vec::new() }
    }

    /// Loads every plugin in the plugins folder; returns one message per plugin that failed.
    pub fn load_all(&mut self) -> Vec<String> {
        self.list.clear();
        let Some(dir) = dir() else { return Vec::new() };
        let mut errors = Vec::new();
        for path in find_wasm(&dir) {
            match self.load(&path) {
                Ok(p) => {
                    log::info!("plugin {} ({} commands) from {}", p.info.name, p.info.commands.len(), path.display());
                    self.list.push(p);
                }
                Err(e) => errors.push(format!("Plugin {}: {e}", file_name(&path))),
            }
        }
        errors
    }

    fn load(&self, path: &Path) -> Result<Loaded, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let module = Module::new(&self.engine, &bytes).map_err(|e| format!("not a valid WebAssembly module ({e})"))?;
        let mut store = Store::new(&self.engine, StoreLimitsBuilder::new().memory_size(MEMORY).build());
        store.limiter(|limits| limits);
        store.set_fuel(FUEL).map_err(|e| e.to_string())?;
        // No imports: anything the module wants from outside fails here.
        let linker = Linker::<StoreLimits>::new(&self.engine);
        let instance = linker
            .instantiate_and_start(&mut store, &module)
            .map_err(|e| format!("cannot start ({e}); plugins may not import anything"))?;
        let memory = instance.get_memory(&store, "memory").ok_or("exports no memory")?;
        let alloc = instance.get_typed_func::<i32, i32>(&store, "wyven_alloc").map_err(|_| "no wyven_alloc export")?;
        let handle =
            instance.get_typed_func::<(i32, i32), i64>(&store, "wyven_handle").map_err(|_| "no wyven_handle export")?;
        let mut p = Loaded {
            info: Info::default(),
            path: path.to_path_buf(),
            store,
            memory,
            alloc,
            handle,
            stopped: None,
        };
        let info = call(&mut p, &Request::Init)?.info.ok_or("answered Init without info")?;
        if info.name.trim().is_empty() {
            return Err("has no name".into());
        }
        p.info = info;
        Ok(p)
    }

    /// (plugin index, command id, title) of every running plugin.
    pub fn commands(&self) -> Vec<(usize, String, String)> {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, p)| p.stopped.is_none())
            .flat_map(|(i, p)| p.info.commands.iter().map(move |c| (i, c.id.clone(), c.title.clone())))
            .collect()
    }

    pub fn wants(&self, event: Event) -> bool {
        self.list.iter().any(|p| p.stopped.is_none() && p.info.events.contains(&event))
    }

    pub fn run_command(&mut self, index: usize, id: &str, editor: Editor) -> Result<Vec<Action>, String> {
        let p = self.list.get_mut(index).ok_or("plugin not loaded")?;
        if let Some(why) = &p.stopped {
            return Err(format!("Plugin {} is stopped: {why}", p.info.name));
        }
        let request = Request::Command { id: id.into(), editor };
        call(p, &request).map(|r| r.actions).map_err(|e| stop(p, e))
    }

    /// Tells every plugin that wants `event`; actions in plugin order, plus failures.
    pub fn event(&mut self, event: Event, editor: &Editor) -> (Vec<Action>, Vec<String>) {
        let (mut actions, mut errors) = (Vec::new(), Vec::new());
        for p in &mut self.list {
            if p.stopped.is_some() || !p.info.events.contains(&event) {
                continue;
            }
            let request = Request::Event { event, editor: editor.clone() };
            match call(p, &request) {
                Ok(r) => actions.extend(r.actions),
                Err(e) => errors.push(stop(p, e)),
            }
        }
        (actions, errors)
    }

    /// One line per plugin for "Plugins: Show Plugins".
    pub fn summary(&self) -> Vec<String> {
        self.list
            .iter()
            .map(|p| {
                let state = p.stopped.as_ref().map_or_else(String::new, |w| format!(" – stopped: {w}"));
                format!("{} ({} commands, {}){state}", p.info.name, p.info.commands.len(), file_name(&p.path))
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Switches a plugin off after an error; returns the message for the user.
fn stop(p: &mut Loaded, e: String) -> String {
    p.stopped = Some(e.clone());
    format!("Plugin {} stopped: {e} (Plugins: Reload Plugins starts it again)", p.info.name)
}

/// Sends one request and reads the answer.
fn call(p: &mut Loaded, request: &Request) -> Result<Response, String> {
    let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    let len = i32::try_from(bytes.len()).map_err(|_| "file too large for plugins")?;
    p.store.set_fuel(FUEL).map_err(|e| e.to_string())?;
    let trap = |e: wasmi::Error| {
        if p_fuel_out(&e) { "ran too long".to_string() } else { format!("crashed ({e})") }
    };
    let ptr = p.alloc.call(&mut p.store, len).map_err(trap)?;
    p.memory.write(&mut p.store, ptr as u32 as usize, &bytes).map_err(|e| format!("bad memory ({e})"))?;
    let packed = p.handle.call(&mut p.store, (ptr, len)).map_err(trap)? as u64;
    let (out_ptr, out_len) = ((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize);
    if out_len > MEMORY {
        return Err("answer too large".into());
    }
    let mut out = vec![0u8; out_len];
    p.memory.read(&p.store, out_ptr, &mut out).map_err(|e| format!("bad answer ({e})"))?;
    serde_json::from_slice(&out).map_err(|e| format!("answer is not valid JSON ({e})"))
}

fn p_fuel_out(e: &wasmi::Error) -> bool {
    e.to_string().to_lowercase().contains("fuel")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example plugin, if it was built (`plugins/text-tools`, see README).
    fn example() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugins/text-tools/target/wasm32-unknown-unknown/release/text_tools.wasm");
        p.is_file().then_some(p)
    }

    #[test]
    fn the_example_plugin_runs_in_the_sandbox() {
        let Some(path) = example() else {
            eprintln!("example plugin not built, skipping");
            return;
        };
        let mut host = Plugins::new();
        let p = host.load(&path).unwrap();
        host.list.push(p);
        let cmds = host.commands();
        assert!(cmds.iter().any(|(_, id, _)| id == "text-tools.sort"));
        let editor = Editor { text: "b\na\nc".into(), ..Editor::default() };
        let actions = host.run_command(0, "text-tools.sort", editor.clone()).unwrap();
        assert_eq!(actions[0], Action::ReplaceRange { start: 0, end: 5, text: "a\nb\nc".into() });
        // State lives between calls: switching trimming on makes BeforeSave act.
        assert!(host.event(Event::BeforeSave, &Editor { text: "x  ".into(), ..Editor::default() }).0.is_empty());
        host.run_command(0, "text-tools.trim", editor).unwrap();
        let (actions, errors) = host.event(Event::BeforeSave, &Editor { text: "x  ".into(), ..Editor::default() });
        assert!(errors.is_empty());
        assert_eq!(actions, vec![Action::ReplaceAll { text: "x".into() }]);
    }

    #[test]
    fn broken_modules_are_refused() {
        let dir = std::env::temp_dir().join(format!("wyven-plugin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.wasm");
        std::fs::write(&bad, b"not wasm").unwrap();
        let host = Plugins::new();
        assert!(host.load(&bad).err().unwrap().contains("not a valid"));
        // A valid module without the Wyven exports (just the 8-byte header).
        let empty = dir.join("empty.wasm");
        std::fs::write(&empty, b"\0asm\x01\0\0\0").unwrap();
        assert!(host.load(&empty).err().unwrap().contains("memory"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
