//! Language servers from the app's point of view: finding and starting the C# server, keeping
//! open documents in sync, and turning server answers into simple events for the UI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use wyvern_buffer::Rope;
use wyvern_lsp::{initialize_params, json, path_to_uri, to_lsp, uri_to_path, Client, Incoming, Value};

/// Changes are sent to the server at most this often while typing.
const SYNC_DELAY: Duration = Duration::from_millis(60);
/// Errors are asked for after typing pauses this long.
const DIAGNOSTICS_DELAY: Duration = Duration::from_millis(350);
/// If the server never says "projects loaded", it counts as ready after this time anyway.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// How deep to look for `.csproj` files when there is no solution file.
const PROJECT_SEARCH_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Starting,
    Loading(String),
    Ready,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub start: Value,
    pub end: Value,
    /// 1 error, 2 warning, 3 info, 4 hint.
    pub severity: u8,
    pub message: String,
    pub code: String,
    /// As sent by the server; handed back when asking for quick actions.
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub insert: String,
    pub filter: String,
    pub sort: String,
}

/// What the UI has to do after the server answered.
pub enum Event {
    Diagnostics(PathBuf),
    Completion { path: PathBuf, anchor: usize, items: Vec<CompletionItem> },
    Definition { path: PathBuf, start: Value },
    DefinitionNotFound,
    Hover { text: String, x: f32, y: f32 },
    Format { path: PathBuf, version: u64, edits: Vec<(Value, Value, String)> },
    /// Changes to apply, possibly across several files (rename, quick action).
    WorkspaceEdit { edit: Value, what: String },
    /// Quick actions offered at the cursor (raw LSP code actions).
    CodeActions(Vec<Value>),
    Status,
    Message(String),
}

enum Pending {
    Initialize,
    Completion { path: PathBuf, anchor: usize },
    Definition,
    Hover { x: f32, y: f32 },
    Format { path: PathBuf, version: u64 },
    Diagnostics { path: PathBuf },
    Rename { new_name: String },
    CodeActions,
    Resolve { title: String },
}

struct OpenDoc {
    lsp_version: i64,
    /// `Document::version()` last sent to the server.
    synced: u64,
    /// Local version waiting to be sent, and since when.
    dirty_since: Option<Instant>,
    diagnostics_due: Option<Instant>,
}

/// Finds a C# language server: env var, Roslyn from the VS Code C# extension, csharp-ls.
pub fn find_csharp_server() -> Option<(PathBuf, Vec<String>)> {
    let roslyn_args = |log_dir: &Path| {
        vec![
            "--stdio".into(),
            "--logLevel".into(),
            "Warning".into(),
            "--extensionLogDirectory".into(),
            log_dir.display().to_string(),
            "--clientProcessId".into(),
            std::process::id().to_string(),
        ]
    };
    let log_dir = std::env::temp_dir().join("wyvern-roslyn-logs");
    if let Some(p) = std::env::var_os("WYVERN_CSHARP_SERVER").map(PathBuf::from)
        && p.exists() {
            let is_roslyn = p.to_string_lossy().contains("CodeAnalysis.LanguageServer");
            return Some((p, if is_roslyn { roslyn_args(&log_dir) } else { Vec::new() }));
        }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let exts = PathBuf::from(home).join(".vscode").join("extensions");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&exts)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ms-dotnettools.csharp-"))
        })
        .collect();
    // Newest extension version first (folder names sort well enough by version).
    candidates.sort();
    candidates.reverse();
    let exe = if cfg!(windows) {
        "Microsoft.CodeAnalysis.LanguageServer.exe"
    } else {
        "Microsoft.CodeAnalysis.LanguageServer"
    };
    for dir in candidates {
        let server = dir.join(".roslyn").join(exe);
        if server.exists() {
            return Some((server, roslyn_args(&log_dir)));
        }
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(if cfg!(windows) { "csharp-ls.exe" } else { "csharp-ls" });
        if p.exists() {
            return Some((p, Vec::new()));
        }
    }
    None
}

pub struct LanguageServer {
    client: Client,
    rx: Receiver<Incoming>,
    pub status: Status,
    pending: HashMap<i64, Pending>,
    docs: HashMap<PathBuf, OpenDoc>,
    pub diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    initialized: bool,
    pull_diagnostics: bool,
    progress: HashSet<String>,
    root: PathBuf,
    /// When project loading started (for the timeout).
    loading_since: Option<Instant>,
}

/// What Roslyn should load: a solution, or else all project files.
#[derive(Debug, PartialEq)]
pub enum ToLoad {
    Solution(PathBuf),
    Projects(Vec<PathBuf>),
    None,
}

/// Looks for `.sln`/`.slnx` in `root`, otherwise `.csproj` files up to a few folders deep
/// (skipping `bin`, `obj` and other generated folders).
pub fn find_projects(root: &Path) -> ToLoad {
    let has_ext = |p: &Path, exts: &[&str]| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
    };
    let mut solutions: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_ext(p, &["sln", "slnx"]))
        .collect();
    solutions.sort();
    if let Some(s) = solutions.into_iter().next() {
        return ToLoad::Solution(s);
    }
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, has_ext: &dyn Fn(&Path, &[&str]) -> bool) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for e in read.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let ignored = crate::explorer::IGNORED.iter().any(|i| i.eq_ignore_ascii_case(&name));
                if depth > 0 && !ignored && !name.starts_with('.') {
                    walk(&p, depth - 1, out, has_ext);
                }
            } else if has_ext(&p, &["csproj"]) {
                out.push(p);
            }
        }
    }
    let mut projects = Vec::new();
    walk(root, PROJECT_SEARCH_DEPTH, &mut projects, &has_ext);
    projects.sort();
    if projects.is_empty() { ToLoad::None } else { ToLoad::Projects(projects) }
}

impl LanguageServer {
    /// Starts the C# server for a project. `wake` is called (on another thread) whenever a
    /// message arrives, so the UI can process it.
    pub fn start_csharp(root: &Path, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let (program, args) = find_csharp_server().ok_or_else(|| {
            "Kein C#-Language-Server gefunden: installiere die C#-Extension in VS Code \
             oder setze WYVERN_CSHARP_SERVER"
                .to_string()
        })?;
        log::info!("starting C# server {}", program.display());
        let (tx, rx) = mpsc::channel();
        let client = Client::spawn("csharp", &program, &args, root, move |m| {
            if tx.send(m).is_ok() {
                wake();
            }
        })
        .map_err(|e| format!("C#-Server startet nicht: {e}"))?;
        let mut server = Self {
            client,
            rx,
            status: Status::Starting,
            pending: HashMap::new(),
            docs: HashMap::new(),
            diagnostics: HashMap::new(),
            initialized: false,
            pull_diagnostics: false,
            progress: HashSet::new(),
            root: root.to_path_buf(),
            loading_since: None,
        };
        let id = server
            .client
            .request("initialize", initialize_params(root, std::process::id()));
        server.pending.insert(id, Pending::Initialize);
        Ok(server)
    }

    pub fn is_open(&self, path: &Path) -> bool {
        self.docs.contains_key(path)
    }

    pub fn status_text(&self) -> String {
        match &self.status {
            Status::Starting => "C#: startet …".into(),
            Status::Loading(m) if m.is_empty() => "C#: lädt Projekt …".into(),
            Status::Loading(m) => format!("C#: {m}"),
            Status::Ready => "C#".into(),
            Status::Failed(e) => format!("C#: {e}"),
        }
    }

    // ----- documents -----

    /// Tells the server about an open file. Before `initialize` finished this does nothing;
    /// the app calls it again for all open files when the status changes.
    pub fn open(&mut self, path: &Path, text: String, version: u64) {
        if self.docs.contains_key(path) || !self.initialized {
            return;
        }
        self.client.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(path), "languageId": "csharp", "version": 1, "text": text
            }}),
        );
        self.docs.insert(
            path.to_path_buf(),
            OpenDoc {
                lsp_version: 1,
                synced: version,
                dirty_since: None,
                diagnostics_due: Some(Instant::now()),
            },
        );
    }

    pub fn close(&mut self, path: &Path) {
        if self.docs.remove(path).is_some() {
            self.client.notify(
                "textDocument/didClose",
                json!({ "textDocument": { "uri": path_to_uri(path) } }),
            );
        }
        self.diagnostics.remove(path);
    }

    pub fn saved(&mut self, path: &Path) {
        if self.docs.contains_key(path) {
            self.client.notify(
                "textDocument/didSave",
                json!({ "textDocument": { "uri": path_to_uri(path) } }),
            );
        }
    }

    /// Notes that the document changed locally; it is sent shortly after.
    pub fn changed(&mut self, path: &Path, version: u64) {
        if let Some(d) = self.docs.get_mut(path)
            && d.synced != version && d.dirty_since.is_none() {
                d.dirty_since = Some(Instant::now());
            }
    }

    /// Sends the full text if it differs from what the server has.
    pub fn sync(&mut self, path: &Path, rope: &Rope, version: u64) {
        let Some(d) = self.docs.get_mut(path) else { return };
        if d.synced == version {
            d.dirty_since = None;
            return;
        }
        d.lsp_version += 1;
        d.synced = version;
        d.dirty_since = None;
        d.diagnostics_due = Some(Instant::now() + DIAGNOSTICS_DELAY);
        self.client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": d.lsp_version },
                "contentChanges": [{ "text": rope.to_string() }]
            }),
        );
    }

    /// Documents whose pending change is due to be sent.
    pub fn due_syncs(&self, now: Instant) -> Vec<PathBuf> {
        self.docs
            .iter()
            .filter(|(_, d)| d.dirty_since.is_some_and(|t| now >= t + SYNC_DELAY))
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Asks for diagnostics where typing has paused. Returns the next time to check.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        let mut next: Option<Instant> = None;
        if let Some(since) = self.loading_since {
            if now >= since + LOAD_TIMEOUT {
                log::warn!("C# server did not report loaded projects; continuing anyway");
                self.loading_since = None;
                self.status = Status::Ready;
                for d in self.docs.values_mut() {
                    d.diagnostics_due = Some(now);
                }
            } else {
                next = Some(since + LOAD_TIMEOUT);
            }
        }
        let mut ask = Vec::new();
        for (path, d) in &mut self.docs {
            if let Some(t) = d.dirty_since {
                let due = t + SYNC_DELAY;
                next = Some(next.map_or(due, |n| n.min(due)));
            }
            if let Some(due) = d.diagnostics_due {
                if d.dirty_since.is_none() && now >= due {
                    d.diagnostics_due = None;
                    ask.push(path.clone());
                } else {
                    next = Some(next.map_or(due, |n| n.min(due)));
                }
            }
        }
        if self.pull_diagnostics && self.status == Status::Ready {
            for path in ask {
                let id = self.client.request(
                    "textDocument/diagnostic",
                    json!({ "textDocument": { "uri": path_to_uri(&path) } }),
                );
                self.pending.insert(id, Pending::Diagnostics { path });
            }
        }
        next
    }

    // ----- requests -----

    fn position_params(path: &Path, rope: &Rope, pos: usize) -> Value {
        json!({ "textDocument": { "uri": path_to_uri(path) }, "position": to_lsp(rope, pos) })
    }

    pub fn completion(&mut self, path: &Path, rope: &Rope, pos: usize, anchor: usize, trigger: Option<char>) {
        let mut params = Self::position_params(path, rope, pos);
        params["context"] = match trigger {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            None => json!({ "triggerKind": 1 }),
        };
        let id = self.client.request("textDocument/completion", params);
        self.pending.insert(id, Pending::Completion { path: path.to_path_buf(), anchor });
    }

    pub fn definition(&mut self, path: &Path, rope: &Rope, pos: usize) {
        let id = self
            .client
            .request("textDocument/definition", Self::position_params(path, rope, pos));
        self.pending.insert(id, Pending::Definition);
    }

    pub fn hover(&mut self, path: &Path, rope: &Rope, pos: usize, x: f32, y: f32) {
        let id = self
            .client
            .request("textDocument/hover", Self::position_params(path, rope, pos));
        self.pending.insert(id, Pending::Hover { x, y });
    }

    pub fn format(&mut self, path: &Path, version: u64, tab_size: usize, spaces: bool) {
        let id = self.client.request(
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "options": { "tabSize": tab_size, "insertSpaces": spaces }
            }),
        );
        self.pending.insert(id, Pending::Format { path: path.to_path_buf(), version });
    }

    pub fn rename(&mut self, path: &Path, rope: &Rope, pos: usize, new_name: &str) {
        let mut params = Self::position_params(path, rope, pos);
        params["newName"] = json!(new_name);
        let id = self.client.request("textDocument/rename", params);
        self.pending.insert(id, Pending::Rename { new_name: new_name.to_string() });
    }

    /// Quick actions for a char range; `diagnostics` are the server's own problem objects there.
    pub fn code_actions(&mut self, path: &Path, rope: &Rope, start: usize, end: usize, diagnostics: Vec<Value>) {
        let id = self.client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": { "start": to_lsp(rope, start), "end": to_lsp(rope, end) },
                "context": { "diagnostics": diagnostics, "triggerKind": 1 }
            }),
        );
        self.pending.insert(id, Pending::CodeActions);
    }

    /// Asks the server to fill in the changes of a quick action.
    pub fn resolve_action(&mut self, action: Value) {
        let title = action["title"].as_str().unwrap_or("").to_string();
        let id = self.client.request("codeAction/resolve", action);
        self.pending.insert(id, Pending::Resolve { title });
    }

    pub fn shutdown(&mut self) {
        self.client.shutdown();
    }

    // ----- incoming -----

    /// Handles everything the server sent since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Incoming::Response { id, result } => {
                    if let Some(p) = self.pending.remove(&id) {
                        self.response(p, result, &mut events);
                    }
                }
                Incoming::Request { id, method, params } => {
                    let result = match method.as_str() {
                        "workspace/configuration" => {
                            let n = params["items"].as_array().map_or(0, Vec::len);
                            json!(vec![Value::Null; n])
                        }
                        "workspace/applyEdit" => json!({ "applied": false }),
                        _ => Value::Null,
                    };
                    self.client.respond(id, result);
                }
                Incoming::Notification { method, params } => {
                    self.notification(&method, params, &mut events);
                }
                Incoming::Exited => {
                    self.status = Status::Failed("Server beendet".into());
                    events.push(Event::Status);
                }
            }
        }
        events
    }

    fn notification(&mut self, method: &str, params: Value, events: &mut Vec<Event>) {
        match method {
            "$/progress" => {
                let token = params["token"].to_string();
                let value = &params["value"];
                match value["kind"].as_str() {
                    Some("begin") | Some("report") => {
                        self.progress.insert(token);
                        if self.status != Status::Ready {
                            let msg = value["message"]
                                .as_str()
                                .or(value["title"].as_str())
                                .unwrap_or("")
                                .to_string();
                            self.status = Status::Loading(msg);
                            events.push(Event::Status);
                        }
                    }
                    Some("end") => {
                        self.progress.remove(&token);
                    }
                    _ => {}
                }
            }
            // Roslyn: all projects are loaded.
            "workspace/projectInitializationComplete" => {
                if let Some(since) = self.loading_since {
                    log::info!("C# projects loaded after {:.1} s", since.elapsed().as_secs_f32());
                }
                self.status = Status::Ready;
                self.loading_since = None;
                for d in self.docs.values_mut() {
                    d.diagnostics_due = Some(Instant::now());
                }
                events.push(Event::Status);
            }
            "textDocument/publishDiagnostics" => {
                if let Some(path) = params["uri"].as_str().and_then(uri_to_path) {
                    let list = parse_diagnostics(&params["diagnostics"]);
                    let key = self.doc_key(&path);
                    self.diagnostics.insert(key.clone(), list);
                    events.push(Event::Diagnostics(key));
                }
            }
            "window/showMessage" => {
                if let Some(m) = params["message"].as_str() {
                    events.push(Event::Message(m.to_string()));
                }
            }
            "window/logMessage"
                if params["type"].as_u64() == Some(1) => {
                    log::warn!("C# server: {}", params["message"]);
                }
            _ => {}
        }
    }

    /// Server URIs may differ in drive-letter case (`c:` vs `C:`); map back to our key.
    fn doc_key(&self, path: &Path) -> PathBuf {
        let lower = path.to_string_lossy().to_lowercase();
        self.docs
            .keys()
            .find(|k| k.to_string_lossy().to_lowercase() == lower)
            .cloned()
            .unwrap_or_else(|| path.to_path_buf())
    }

    fn response(&mut self, pending: Pending, result: Result<Value, String>, events: &mut Vec<Event>) {
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                log::debug!("C# server error: {e}");
                match pending {
                    Pending::Initialize => {
                        self.status = Status::Failed(e);
                        events.push(Event::Status);
                    }
                    Pending::Rename { .. } | Pending::Resolve { .. } => {
                        events.push(Event::Message(format!("Nicht möglich: {e}")));
                    }
                    _ => {}
                }
                return;
            }
        };
        match pending {
            Pending::Initialize => {
                let caps = &value["capabilities"];
                self.pull_diagnostics = !caps["diagnosticProvider"].is_null();
                self.client.notify("initialized", json!({}));
                self.initialized = true;
                // Roslyn: load the projects explicitly. Its automatic discovery
                // (--autoLoadProjects) skips projects that already have a `bin` folder.
                // csharp-ls loads by itself and has no "projects loaded" signal.
                let roslyn = !caps["_vs_onAutoInsertProvider"].is_null();
                self.status = if !roslyn {
                    Status::Ready
                } else {
                    match find_projects(&self.root) {
                        ToLoad::Solution(sln) => {
                            log::info!("opening solution {}", sln.display());
                            self.client.notify("solution/open", json!({ "solution": path_to_uri(&sln) }));
                            self.loading_since = Some(Instant::now());
                            Status::Loading(String::new())
                        }
                        ToLoad::Projects(list) => {
                            log::info!("opening {} project(s)", list.len());
                            let uris: Vec<String> = list.iter().map(|p| path_to_uri(p)).collect();
                            self.client.notify("project/open", json!({ "projects": uris }));
                            self.loading_since = Some(Instant::now());
                            Status::Loading(String::new())
                        }
                        // Loose .cs files: Roslyn handles them on its own, nothing to wait for.
                        ToLoad::None => Status::Ready,
                    }
                };
                events.push(Event::Status);
            }
            Pending::Diagnostics { path } => {
                let list = parse_diagnostics(&value["items"]);
                self.diagnostics.insert(path.clone(), list);
                events.push(Event::Diagnostics(path));
            }
            Pending::Completion { path, anchor } => {
                let items = value
                    .get("items")
                    .unwrap_or(&value)
                    .as_array()
                    .map(|a| a.iter().map(parse_completion).collect())
                    .unwrap_or_default();
                events.push(Event::Completion { path, anchor, items });
            }
            Pending::Definition => {
                let first = match &value {
                    Value::Array(a) => a.first().cloned(),
                    Value::Null => None,
                    v => Some(v.clone()),
                };
                let target = first.and_then(|loc| {
                    let uri = loc["uri"].as_str().or(loc["targetUri"].as_str())?.to_string();
                    let range = if loc["range"].is_object() {
                        loc["range"].clone()
                    } else {
                        loc["targetSelectionRange"].clone()
                    };
                    Some((uri, range["start"].clone()))
                });
                match target.and_then(|(uri, start)| Some((uri_to_path(&uri)?, start))) {
                    Some((path, start)) => events.push(Event::Definition { path, start }),
                    None => events.push(Event::DefinitionNotFound),
                }
            }
            Pending::Hover { x, y } => {
                let text = hover_text(&value["contents"]);
                if !text.trim().is_empty() {
                    events.push(Event::Hover { text, x, y });
                }
            }
            Pending::Rename { new_name } => {
                if value.is_null() {
                    events.push(Event::Message("Hier kann nichts umbenannt werden".into()));
                } else {
                    events.push(Event::WorkspaceEdit { edit: value, what: format!("Umbenannt in „{new_name}“") });
                }
            }
            Pending::CodeActions => {
                let actions: Vec<Value> = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            // Bare commands (fix-all and similar) need client support we lack.
                            .filter(|x| x.get("edit").is_some() || x.get("data").is_some())
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(Event::CodeActions(actions));
            }
            Pending::Resolve { title } => {
                if value["edit"].is_object() {
                    events.push(Event::WorkspaceEdit { edit: value["edit"].clone(), what: title });
                } else {
                    events.push(Event::Message(format!("„{title}“ liefert keine Änderung")));
                }
            }
            Pending::Format { path, version } => {
                let edits = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                (
                                    e["range"]["start"].clone(),
                                    e["range"]["end"].clone(),
                                    e["newText"].as_str().unwrap_or("").to_string(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(Event::Format { path, version, edits });
            }
        }
    }
}

/// A text change from a WorkspaceEdit, still in LSP positions.
pub type LspEdit = (Value, Value, String);

/// Splits a WorkspaceEdit into per-file edit lists. File operations (create, rename, delete)
/// are not supported yet and counted as skipped. Line endings are normalized to `\n`.
pub fn parse_workspace_edit(edit: &Value) -> (Vec<(PathBuf, Vec<LspEdit>)>, usize) {
    let mut skipped = 0;
    let mut raw: Vec<(&str, &[Value])> = Vec::new();
    if let Some(changes) = edit["documentChanges"].as_array() {
        for c in changes {
            match (c["textDocument"]["uri"].as_str(), c["edits"].as_array()) {
                (Some(uri), Some(edits)) => raw.push((uri, edits)),
                _ => skipped += 1,
            }
        }
    } else if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            raw.push((uri, edits.as_array().map(Vec::as_slice).unwrap_or(&[])));
        }
    }
    let mut out = Vec::new();
    for (uri, edits) in raw {
        let Some(path) = uri_to_path(uri) else {
            skipped += 1;
            continue;
        };
        let edits = edits
            .iter()
            .map(|e| {
                (
                    e["range"]["start"].clone(),
                    e["range"]["end"].clone(),
                    e["newText"].as_str().unwrap_or("").replace("\r\n", "\n").replace('\r', ""),
                )
            })
            .collect();
        out.push((path, edits));
    }
    (out, skipped)
}

fn parse_diagnostics(list: &Value) -> Vec<Diagnostic> {
    list.as_array()
        .map(|a| {
            a.iter()
                .map(|d| Diagnostic {
                    start: d["range"]["start"].clone(),
                    end: d["range"]["end"].clone(),
                    severity: d["severity"].as_u64().unwrap_or(1) as u8,
                    message: d["message"].as_str().unwrap_or("").to_string(),
                    code: match &d["code"] {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        _ => String::new(),
                    },
                    raw: d.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn kind_name(kind: u64) -> &'static str {
    match kind {
        2 => "Methode",
        3 => "Funktion",
        4 => "Konstruktor",
        5 => "Feld",
        6 => "Variable",
        7 => "Klasse",
        8 => "Interface",
        9 => "Namespace",
        10 => "Eigenschaft",
        13 => "Enum",
        14 => "Schlüsselwort",
        15 => "Snippet",
        20 => "Enum-Wert",
        21 => "Konstante",
        22 => "Struct",
        23 => "Ereignis",
        25 => "Typparameter",
        _ => "",
    }
}

fn parse_completion(item: &Value) -> CompletionItem {
    let label = item["label"].as_str().unwrap_or("").to_string();
    let insert = item["textEdit"]["newText"]
        .as_str()
        .or(item["insertText"].as_str())
        .unwrap_or(&label)
        .to_string();
    let detail = item["detail"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| kind_name(item["kind"].as_u64().unwrap_or(0)).to_string());
    CompletionItem {
        filter: item["filterText"].as_str().unwrap_or(&label).to_string(),
        sort: item["sortText"].as_str().unwrap_or(&label).to_string(),
        insert,
        detail,
        label,
    }
}

/// Hover contents (MarkupContent, MarkedString or an array of them) as plain text.
fn hover_text(contents: &Value) -> String {
    let raw = match contents {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(hover_text).collect::<Vec<_>>().join("\n"),
        Value::Object(o) => o.get("value").and_then(Value::as_str).unwrap_or("").to_string(),
        _ => String::new(),
    };
    raw.lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .map(|l| unescape_markdown(&l.replace("&nbsp;", " ")))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `Language\-Server\.` → `Language-Server.` (Markdown escapes before punctuation).
fn unescape_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies parsed edits to a document the way the app does.
    fn apply(text: &str, edits: &[LspEdit]) -> String {
        let mut doc = wyvern_buffer::Document::from_str(text);
        let rope = doc.text().clone();
        let changes = edits
            .iter()
            .map(|(s, e, t)| wyvern_buffer::Change {
                start: wyvern_lsp::from_lsp(&rope, s),
                end: wyvern_lsp::from_lsp(&rope, e),
                text: t.clone(),
            })
            .collect();
        doc.apply_changes(changes);
        doc.text().to_string()
    }

    #[test]
    fn roslyn_rename_edit_applies() {
        // Real answer of Roslyn for renaming `Player` to `Spieler` (minimal edits "Play" → "Spiel").
        let uri = if cfg!(windows) { "file:///C:/p/Program.cs" } else { "file:///p/Program.cs" };
        let edit = json!({ "documentChanges": [{
            "edits": [
                { "newText": "Spiel", "range": { "start": { "line": 1, "character": 13 }, "end": { "line": 1, "character": 17 } } },
                { "newText": "Spiel", "range": { "start": { "line": 3, "character": 14 }, "end": { "line": 3, "character": 18 } } }
            ],
            "textDocument": { "uri": uri, "version": null }
        }]});
        let (files, skipped) = parse_workspace_edit(&edit);
        assert_eq!((files.len(), skipped), (1, 0));
        let text = "namespace N {\npublic class Player {}\nclass P {\n  var p = new Player();\n}\n";
        assert_eq!(
            apply(text, &files[0].1),
            "namespace N {\npublic class Spieler {}\nclass P {\n  var p = new Spieler();\n}\n"
        );
    }

    #[test]
    fn roslyn_quick_action_newlines_are_normalized() {
        // "Feld generieren" from Roslyn: inserts "\r\n        private int valu;\r\n\r" at a line end.
        let edit = json!({ "changes": { "file:///C:/p/A.cs": [{
            "newText": "\r\n    private int valu;\r\n\r",
            "range": { "start": { "line": 0, "character": 9 }, "end": { "line": 1, "character": 0 } }
        }]}});
        let (files, _) = parse_workspace_edit(&edit);
        assert_eq!(apply("class A {\n}\n", &files[0].1), "class A {\n    private int valu;\n}\n");
    }

    #[test]
    fn file_operations_are_skipped() {
        let edit = json!({ "documentChanges": [{ "kind": "create", "uri": "file:///C:/p/B.cs" }] });
        let (files, skipped) = parse_workspace_edit(&edit);
        assert_eq!((files.len(), skipped), (0, 1));
    }

    #[test]
    fn finds_solution_first_then_projects_but_not_in_bin() {
        let root = std::env::temp_dir().join(format!("wyvern-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/App/bin/Debug")).unwrap();
        std::fs::write(root.join("src/App/App.csproj"), "<Project />").unwrap();
        std::fs::write(root.join("src/App/bin/Debug/Copy.csproj"), "<Project />").unwrap();
        assert_eq!(find_projects(&root), ToLoad::Projects(vec![root.join("src/App/App.csproj")]));
        std::fs::write(root.join("Game.sln"), "").unwrap();
        assert_eq!(find_projects(&root), ToLoad::Solution(root.join("Game.sln")));
        let _ = std::fs::remove_dir_all(&root);
        let empty = std::env::temp_dir().join(format!("wyvern-noproj-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(find_projects(&empty), ToLoad::None);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn hover_markdown_becomes_plain_text() {
        let v = json!({ "kind": "markdown", "value": "```csharp\nclass A\n```\n  \nTest des Language\\-Servers\\." });
        assert_eq!(hover_text(&v), "class A\n  \nTest des Language-Servers.");
    }
}
