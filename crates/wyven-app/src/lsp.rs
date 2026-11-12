//! Language servers from the app's point of view: finding and starting one server per language
//! (C#: Roslyn; JS/TS: TypeScript 7's own server; HTML/CSS/JSON: the servers from VS Code),
//! keeping open documents in sync, and turning server answers into simple events for the UI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use wyven_buffer::Rope;
use wyven_lsp::{initialize_params, json, path_to_uri, to_lsp, uri_to_path, Client, Incoming, Value};

/// Changes are sent to the server at most this often while typing.
const SYNC_DELAY: Duration = Duration::from_millis(60);
/// Errors are asked for after typing pauses this long.
const DIAGNOSTICS_DELAY: Duration = Duration::from_millis(350);
/// If the server never says "projects loaded", it counts as ready after this time anyway.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// After opening a file, hints, lenses and errors are asked for once more after this time.
const SECOND_LOOK: Duration = Duration::from_secs(2);
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
    /// `insert` is a snippet (placeholders like `$1`, `${2:name}`).
    pub snippet: bool,
    /// LSP CompletionItemKind (2 method, 5 field, 7 class, 10 property, 15 snippet …; 0 unknown).
    pub kind: u8,
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
    /// Inlay hints of a file for document version `version`: position, label, padding.
    InlayHints { path: PathBuf, version: u64, hints: Vec<(Value, String, bool, bool)> },
    /// CodeLens titles of a file ("3 references") at the declared names.
    CodeLens { path: PathBuf, version: u64, lenses: Vec<(Value, String)> },
    /// "Find all references": file and start position of each.
    References(Vec<(PathBuf, Value)>),
    /// "Go to implementation": file and start position of each.
    Implementations(Vec<(PathBuf, Value)>),
    /// Code actions offered for organizing usings/imports (raw).
    OrganizeActions(Vec<Value>),
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
    InlayHints { path: PathBuf, version: u64 },
    CodeLens { path: PathBuf, version: u64 },
    LensResolve { path: PathBuf, version: u64, start: Value },
    References,
    Implementation,
    Organize,
}

/// Locations from a `Location[]` or `LocationLink[]` answer: file and start position.
fn locations(value: &Value) -> Vec<(PathBuf, Value)> {
    let list = match value {
        Value::Array(a) => a.clone(),
        Value::Null => Vec::new(),
        v => vec![v.clone()],
    };
    list.iter()
        .filter_map(|loc| {
            let uri = loc["uri"].as_str().or(loc["targetUri"].as_str())?;
            let range = if loc["range"].is_object() { &loc["range"] } else { &loc["targetSelectionRange"] };
            Some((uri_to_path(uri)?, range["start"].clone()))
        })
        .collect()
}

struct OpenDoc {
    lsp_version: i64,
    /// `Document::version()` last sent to the server.
    synced: u64,
    /// Local version waiting to be sent, and since when.
    dirty_since: Option<Instant>,
    diagnostics_due: Option<Instant>,
    /// Line count of the text last sent (range for inlay hints).
    lines: usize,
    /// Ask once more shortly after opening: a server may still be loading its project then.
    second_look: Option<Instant>,
}

/// CodeLens of one file being resolved: document version, lenses still waiting, results.
type LensBatch = (u64, usize, Vec<(Value, String)>);

/// Inlay hint switches Roslyn asks for (`csharp|inlay_hints.<key>`): parameter names at
/// literals and `new`, inferred types of `var`, lambda parameters and `new()`, like Visual
/// Studio with inlay hints on.
const INLAY_HINT_SETTINGS: &[&str] = &[
    "dotnet_enable_inlay_hints_for_parameters",
    "dotnet_enable_inlay_hints_for_literal_parameters",
    "dotnet_enable_inlay_hints_for_object_creation_parameters",
    "dotnet_suppress_inlay_hints_for_parameters_that_differ_only_by_suffix",
    "dotnet_suppress_inlay_hints_for_parameters_that_match_method_intent",
    "dotnet_suppress_inlay_hints_for_parameters_that_match_argument_name",
    "csharp_enable_inlay_hints_for_types",
    "csharp_enable_inlay_hints_for_implicit_variable_types",
    "csharp_enable_inlay_hints_for_lambda_parameter_types",
    "csharp_enable_inlay_hints_for_implicit_object_creation",
];

/// Settings for TypeScript's server, in VS Code's format (sections `js/ts`, `typescript`,
/// `javascript`): the same hints as for C#, plus "N references" lenses.
pub fn typescript_settings() -> Value {
    json!({
        "inlayHints": {
            "parameterNames": { "enabled": "literals", "suppressWhenArgumentMatchesName": true },
            "parameterTypes": { "enabled": false },
            "variableTypes": { "enabled": true, "suppressWhenTypeMatchesName": true },
            "propertyDeclarationTypes": { "enabled": false },
            "functionLikeReturnTypes": { "enabled": false },
            "enumMemberValues": { "enabled": false }
        },
        "referencesCodeLens": { "enabled": true, "showOnAllFunctions": true },
        "implementationsCodeLens": { "enabled": false }
    })
}

/// Our answer to one `workspace/configuration` item: `true` for Roslyn's inlay hint switches
/// above, TypeScript's settings block, the server's own default (null) for everything else.
fn config_value(section: &str) -> Value {
    match section {
        "js/ts" | "typescript" | "javascript" => return typescript_settings(),
        // VS Code's CSS/HTML/JSON servers expect objects here; null makes the CSS server fail.
        "css" | "scss" | "less" => {
            return json!({ "validate": true, "lint": {}, "completion": { "completePropertyWithSemicolon": true } });
        }
        "html" => return json!({ "validate": { "scripts": true, "styles": true }, "format": { "enable": true } }),
        "json" => return json!({ "validate": { "enable": true }, "format": { "enable": true } }),
        _ => {}
    }
    let key = section.rsplit('.').next().unwrap_or("");
    if section.contains("inlay_hints") && INLAY_HINT_SETTINGS.contains(&key) { json!(true) } else { Value::Null }
}

/// Which language server a file needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    CSharp,
    /// JavaScript, TypeScript, TSX.
    TypeScript,
    Html,
    Css,
    Json,
}

impl ServerKind {
    /// By the editor's language name (wyven-syntax).
    pub fn for_language(language: &str) -> Option<Self> {
        Some(match language {
            "C#" => Self::CSharp,
            "JavaScript" | "TypeScript" | "TSX" => Self::TypeScript,
            "HTML" => Self::Html,
            "CSS" => Self::Css,
            "JSON" => Self::Json,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CSharp => "C#",
            Self::TypeScript => "TypeScript",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Json => "JSON",
        }
    }
}

/// The LSP `languageId` of a file.
pub fn language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("cs" | "csx") => "csharp",
        Some("ts" | "mts" | "cts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("html" | "htm") => "html",
        Some("css") => "css",
        Some("scss") => "scss",
        Some("less") => "less",
        Some("jsonc") => "jsonc",
        Some("json") => "json",
        _ => "plaintext",
    }
}

/// Wyven's own copy of the web language servers (npm packages in the config folder).
fn web_servers_dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("servers").join("node_modules"))
}

fn find_on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe)).find(|p| p.is_file())
}

/// Program and arguments for a language server, or why it is missing.
pub fn find_server(kind: ServerKind) -> Result<(PathBuf, Vec<String>), String> {
    let setup = "install with: cd %APPDATA%\\Wyven\\servers && npm install typescript vscode-langservers-extracted";
    match kind {
        ServerKind::CSharp => find_csharp_server().ok_or_else(|| {
            "No C# language server found: install the C# extension in VS Code or set WYVEN_CSHARP_SERVER".to_string()
        }),
        ServerKind::TypeScript => {
            // TypeScript 7 is native and has its own server: `tsc --lsp --stdio`.
            let dir = web_servers_dir().ok_or("no config folder")?.join("@typescript");
            let exe = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path().join("lib").join(if cfg!(windows) { "tsc.exe" } else { "tsc" }))
                .find(|p| p.is_file())
                .ok_or_else(|| format!("No TypeScript server found ({setup})"))?;
            Ok((exe, vec!["--lsp".into(), "--stdio".into()]))
        }
        ServerKind::Html | ServerKind::Css | ServerKind::Json => {
            let name = match kind {
                ServerKind::Html => "html",
                ServerKind::Css => "css",
                _ => "json",
            };
            let script = web_servers_dir()
                .ok_or("no config folder")?
                .join("vscode-langservers-extracted")
                .join("bin")
                .join(format!("vscode-{name}-language-server"));
            if !script.is_file() {
                return Err(format!("No {} language server found ({setup})", kind.label()));
            }
            let node = find_on_path(if cfg!(windows) { "node.exe" } else { "node" })
                .ok_or("Node.js is needed for the HTML/CSS/JSON servers but was not found")?;
            Ok((node, vec![script.display().to_string(), "--stdio".into()]))
        }
    }
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
    let log_dir = std::env::temp_dir().join("wyven-roslyn-logs");
    if let Some(p) = std::env::var_os("WYVEN_CSHARP_SERVER").map(PathBuf::from)
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
    pub kind: ServerKind,
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
    /// Ask for inlay hints / CodeLens together with diagnostics.
    pub want_inlay_hints: bool,
    pub want_code_lens: bool,
    /// CodeLens being resolved per file: version, lenses still waiting, results so far.
    lens_results: HashMap<PathBuf, LensBatch>,
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
    /// Starts the server for a language. `wake` is called (on another thread) whenever a
    /// message arrives, so the UI can process it.
    pub fn start(kind: ServerKind, root: &Path, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let (program, args) = find_server(kind)?;
        log::info!("starting {} server {}", kind.label(), program.display());
        let (tx, rx) = mpsc::channel();
        let client = Client::spawn(kind.label(), &program, &args, root, move |m| {
            if tx.send(m).is_ok() {
                wake();
            }
        })
        .map_err(|e| format!("{} server does not start: {e}", kind.label()))?;
        let mut server = Self {
            kind,
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
            want_inlay_hints: true,
            want_code_lens: true,
            lens_results: HashMap::new(),
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
        let name = self.kind.label();
        match &self.status {
            Status::Starting => format!("{name}: starting …"),
            Status::Loading(m) if m.is_empty() => format!("{name}: loading project …"),
            Status::Loading(m) => format!("{name}: {m}"),
            Status::Ready => name.into(),
            Status::Failed(e) => format!("{name}: {e}"),
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
                "uri": path_to_uri(path), "languageId": language_id(path), "version": 1, "text": text
            }}),
        );
        self.docs.insert(
            path.to_path_buf(),
            OpenDoc {
                lsp_version: 1,
                synced: version,
                dirty_since: None,
                diagnostics_due: Some(Instant::now()),
                lines: text.lines().count() + 1,
                second_look: Some(Instant::now() + SECOND_LOOK),
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
        d.lines = rope.len_lines();
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
        let mut ask: Vec<(PathBuf, u64, usize)> = Vec::new();
        for (path, d) in &mut self.docs {
            if let Some(t) = d.second_look {
                if now >= t {
                    d.second_look = None;
                    d.diagnostics_due = Some(now);
                } else {
                    next = Some(next.map_or(t, |n| n.min(t)));
                }
            }
            if let Some(t) = d.dirty_since {
                let due = t + SYNC_DELAY;
                next = Some(next.map_or(due, |n| n.min(due)));
            }
            if let Some(due) = d.diagnostics_due {
                if d.dirty_since.is_none() && now >= due {
                    d.diagnostics_due = None;
                    ask.push((path.clone(), d.synced, d.lines));
                } else {
                    next = Some(next.map_or(due, |n| n.min(due)));
                }
            }
        }
        if self.status == Status::Ready {
            for (path, version, lines) in ask {
                let uri = path_to_uri(&path);
                if self.pull_diagnostics {
                    let id = self.client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
                    self.pending.insert(id, Pending::Diagnostics { path: path.clone() });
                }
                if self.want_inlay_hints {
                    let id = self.client.request(
                        "textDocument/inlayHint",
                        json!({
                            "textDocument": { "uri": uri },
                            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": lines, "character": 0 } }
                        }),
                    );
                    self.pending.insert(id, Pending::InlayHints { path: path.clone(), version });
                }
                if self.want_code_lens {
                    let id = self.client.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
                    self.pending.insert(id, Pending::CodeLens { path: path.clone(), version });
                }
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

    /// "Find all references" at a char position.
    pub fn references(&mut self, path: &Path, rope: &Rope, pos: usize) {
        let mut params = Self::position_params(path, rope, pos);
        params["context"] = json!({ "includeDeclaration": false });
        let id = self.client.request("textDocument/references", params);
        self.pending.insert(id, Pending::References);
    }

    /// "Go to implementation" at a char position.
    pub fn implementation(&mut self, path: &Path, rope: &Rope, pos: usize) {
        let id = self.client.request("textDocument/implementation", Self::position_params(path, rope, pos));
        self.pending.insert(id, Pending::Implementation);
    }

    /// Code actions for organizing usings/imports in `start..end` (Roslyn offers them on the
    /// using lines, TypeScript as `source.organizeImports`).
    pub fn organize_actions(&mut self, path: &Path, rope: &Rope, start: usize, end: usize) {
        let id = self.client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": { "start": to_lsp(rope, start), "end": to_lsp(rope, end) },
                "context": { "diagnostics": [], "only": ["source.organizeImports", "source", "quickfix", "refactor"], "triggerKind": 1 }
            }),
        );
        self.pending.insert(id, Pending::Organize);
    }

    /// Asks again for hints and lenses of all open files (after switching them on).
    pub fn refresh_extras(&mut self) {
        let now = Instant::now();
        for d in self.docs.values_mut() {
            d.diagnostics_due = Some(now);
        }
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
                            let items = params["items"].as_array().cloned().unwrap_or_default();
                            json!(items.iter().map(|it| config_value(it["section"].as_str().unwrap_or(""))).collect::<Vec<_>>())
                        }
                        "workspace/applyEdit" => json!({ "applied": false }),
                        // The server has new results (project loaded, settings changed).
                        "workspace/inlayHint/refresh" | "workspace/codeLens/refresh" | "workspace/diagnostic/refresh" => {
                            self.refresh_extras();
                            Value::Null
                        }
                        _ => Value::Null,
                    };
                    self.client.respond(id, result);
                }
                Incoming::Notification { method, params } => {
                    self.notification(&method, params, &mut events);
                }
                Incoming::Exited => {
                    self.status = Status::Failed("server stopped".into());
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
                    log::warn!("{} server: {}", self.kind.label(), params["message"]);
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
                        events.push(Event::Message(format!("Not possible: {e}")));
                    }
                    Pending::LensResolve { path, version, .. } => self.lens_done(path, version, None, events),
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
                let roslyn = self.kind == ServerKind::CSharp && !caps["_vs_onAutoInsertProvider"].is_null();
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
                    events.push(Event::Message("Nothing to rename here".into()));
                } else {
                    events.push(Event::WorkspaceEdit { edit: value, what: format!("Renamed to “{new_name}”") });
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
                    events.push(Event::Message(format!("“{title}” makes no change")));
                }
            }
            Pending::InlayHints { path, version } => {
                let hints = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|h| {
                                let label = match &h["label"] {
                                    Value::String(s) => s.clone(),
                                    Value::Array(parts) => parts.iter().filter_map(|p| p["value"].as_str()).collect(),
                                    _ => String::new(),
                                };
                                let pad = |k: &str| h[k].as_bool().unwrap_or(false);
                                (h["position"].clone(), label.trim().to_string(), pad("paddingLeft"), pad("paddingRight"))
                            })
                            .filter(|(_, l, _, _)| !l.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(Event::InlayHints { path: self.doc_key(&path), version, hints });
            }
            Pending::CodeLens { path, version } => {
                let lenses = value.as_array().cloned().unwrap_or_default();
                if lenses.is_empty() {
                    events.push(Event::CodeLens { path, version, lenses: Vec::new() });
                    return;
                }
                self.lens_results.insert(path.clone(), (version, lenses.len(), Vec::new()));
                for lens in lenses {
                    let start = lens["range"]["start"].clone();
                    if let Some(title) = lens["command"]["title"].as_str() {
                        // Already resolved by the server.
                        let title = title.to_string();
                        self.lens_done(path.clone(), version, Some((start, title)), events);
                    } else {
                        let id = self.client.request("codeLens/resolve", lens);
                        self.pending.insert(id, Pending::LensResolve { path: path.clone(), version, start });
                    }
                }
            }
            Pending::LensResolve { path, version, start } => {
                let title = value["command"]["title"].as_str().map(|t| (start, t.to_string()));
                self.lens_done(path, version, title, events);
            }
            Pending::Implementation => events.push(Event::Implementations(locations(&value))),
            Pending::Organize => {
                let actions = value
                    .as_array()
                    .map(|a| a.iter().filter(|x| x.get("edit").is_some() || x.get("data").is_some()).cloned().collect())
                    .unwrap_or_default();
                events.push(Event::OrganizeActions(actions));
            }
            Pending::References => {
                let refs = value
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|loc| Some((uri_to_path(loc["uri"].as_str()?)?, loc["range"]["start"].clone())))
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(Event::References(refs));
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

impl LanguageServer {
    /// One CodeLens of a file is resolved (or failed); when all are, the file's list is done.
    fn lens_done(&mut self, path: PathBuf, version: u64, lens: Option<(Value, String)>, events: &mut Vec<Event>) {
        let Some((v, waiting, done)) = self.lens_results.get_mut(&path) else { return };
        if *v != version {
            return; // a newer request replaced this one
        }
        if let Some(l) = lens.filter(|(_, t)| !t.trim().is_empty()) {
            done.push(l);
        }
        *waiting = waiting.saturating_sub(1);
        if *waiting == 0 {
            let (_, _, lenses) = self.lens_results.remove(&path).unwrap_or_default();
            let key = self.doc_key(&path);
            events.push(Event::CodeLens { path: key, version, lenses });
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
        2 => "method",
        3 => "function",
        4 => "constructor",
        5 => "field",
        6 => "variable",
        7 => "class",
        8 => "interface",
        9 => "namespace",
        10 => "property",
        13 => "enum",
        14 => "keyword",
        15 => "snippet",
        20 => "enum member",
        21 => "constant",
        22 => "struct",
        23 => "event",
        25 => "type parameter",
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
        snippet: item["insertTextFormat"].as_u64() == Some(2),
        kind: item["kind"].as_u64().unwrap_or(0).min(255) as u8,
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
        let mut doc = wyven_buffer::Document::from_str(text);
        let rope = doc.text().clone();
        let changes = edits
            .iter()
            .map(|(s, e, t)| wyven_buffer::Change {
                start: wyven_lsp::from_lsp(&rope, s),
                end: wyven_lsp::from_lsp(&rope, e),
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
        let root = std::env::temp_dir().join(format!("wyven-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/App/bin/Debug")).unwrap();
        std::fs::write(root.join("src/App/App.csproj"), "<Project />").unwrap();
        std::fs::write(root.join("src/App/bin/Debug/Copy.csproj"), "<Project />").unwrap();
        assert_eq!(find_projects(&root), ToLoad::Projects(vec![root.join("src/App/App.csproj")]));
        std::fs::write(root.join("Game.sln"), "").unwrap();
        assert_eq!(find_projects(&root), ToLoad::Solution(root.join("Game.sln")));
        let _ = std::fs::remove_dir_all(&root);
        let empty = std::env::temp_dir().join(format!("wyven-noproj-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(find_projects(&empty), ToLoad::None);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn files_find_their_server() {
        assert_eq!(ServerKind::for_language("TSX"), Some(ServerKind::TypeScript));
        assert_eq!(ServerKind::for_language("Markdown"), None);
        assert_eq!(language_id(Path::new("a/App.tsx")), "typescriptreact");
        assert_eq!(language_id(Path::new("style.SCSS")), "scss");
        assert_eq!(language_id(Path::new("Program.cs")), "csharp");
    }

    #[test]
    fn inlay_hint_settings_are_switched_on() {
        assert_eq!(config_value("csharp|inlay_hints.dotnet_enable_inlay_hints_for_parameters"), json!(true));
        assert_eq!(config_value("csharp|inlay_hints.csharp_enable_inlay_hints_for_implicit_variable_types"), json!(true));
        assert_eq!(config_value("csharp|inlay_hints.dotnet_enable_inlay_hints_for_other_parameters"), Value::Null);
        assert_eq!(config_value("csharp|code_style.formatting.indentation_and_spacing.tab_width"), Value::Null);
        assert!(config_value("css")["lint"].is_object());
        assert_eq!(config_value("typescript")["inlayHints"]["parameterNames"]["enabled"], "literals");
    }

    #[test]
    fn hover_markdown_becomes_plain_text() {
        let v = json!({ "kind": "markdown", "value": "```csharp\nclass A\n```\n  \nTest des Language\\-Servers\\." });
        assert_eq!(hover_text(&v), "class A\n  \nTest des Language-Servers.");
    }
}
