//! Bridge to Claude Code, the same way the VS Code integration works.
//!
//! Wyven runs a small MCP server on a WebSocket at 127.0.0.1 (only this PC) and announces it in
//! a lock file `~/.claude/ide/<port>.lock` (or under `CLAUDE_CONFIG_DIR`) with a random token.
//! Claude Code in Wyven's terminal finds it through `CLAUDE_CODE_SSE_PORT`; in another terminal
//! `/ide` lists it. Claude Code then calls our tools: it sees the open file, the selection and
//! the errors, and its edits arrive as `openDiff` – shown side by side, accepted with Ctrl+Enter
//! or rejected with Esc. Claude Code writes the file itself after an accept.
//!
//! A background thread speaks the protocol (handshake, tool list); tool calls go to the UI
//! thread, which knows the editor state, and the answers come back through a channel.

use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

/// What arrives from Claude Code for the UI thread.
pub enum BridgeEvent {
    Connected,
    Disconnected,
    /// A tool call; answer it with [`Bridge::reply`] using `id`.
    Call { id: Value, name: String, args: Value },
}

enum Outgoing {
    Reply { id: Value, result: Value },
    Notify { method: String, params: Value },
}

pub struct Bridge {
    pub port: u16,
    lock_path: PathBuf,
    token: String,
    out: Sender<Outgoing>,
    events: Receiver<BridgeEvent>,
    connected: Arc<AtomicBool>,
}

/// Claude Code's config folder: `CLAUDE_CONFIG_DIR` or `~/.claude`.
fn claude_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude"))
}

impl Bridge {
    /// Starts listening and writes the lock file. `wake` wakes the UI thread.
    pub fn start(workspace: Option<&Path>, wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("Claude Code bridge: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = uuid::Uuid::new_v4().to_string();
        let lock_dir = claude_dir().ok_or("no home folder")?.join("ide");
        std::fs::create_dir_all(&lock_dir).map_err(|e| e.to_string())?;
        let lock_path = lock_dir.join(format!("{port}.lock"));
        let (out_tx, out_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let connected = Arc::new(AtomicBool::new(false));
        let bridge = Self { port, lock_path, token: token.clone(), out: out_tx, events: ev_rx, connected: connected.clone() };
        bridge.write_lock(workspace)?;
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        std::thread::Builder::new()
            .name("claude-bridge".into())
            .spawn(move || serve(listener, token, out_rx, ev_tx, wake, connected))
            .map_err(|e| e.to_string())?;
        log::info!("Claude Code bridge on 127.0.0.1:{port}");
        Ok(bridge)
    }

    /// (Re)writes the lock file, e.g. after opening another folder.
    pub fn write_lock(&self, workspace: Option<&Path>) -> Result<(), String> {
        let folders: Vec<String> = workspace.map(|p| p.display().to_string()).into_iter().collect();
        let lock = json!({
            "pid": std::process::id(),
            "workspaceFolders": folders,
            "ideName": "Wyven",
            "transport": "ws",
            "runningInWindows": cfg!(windows),
            "authToken": self.token,
        });
        std::fs::write(&self.lock_path, lock.to_string()).map_err(|e| format!("Claude Code lock file: {e}"))
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    pub fn poll(&self) -> Vec<BridgeEvent> {
        self.events.try_iter().collect()
    }

    pub fn reply(&self, id: Value, result: Value) {
        let _ = self.out.send(Outgoing::Reply { id, result });
    }

    pub fn notify(&self, method: &str, params: Value) {
        if self.is_connected() {
            let _ = self.out.send(Outgoing::Notify { method: method.into(), params });
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

/// A tool answer with one text block (the format Claude Code expects).
pub fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }] })
}

/// A tool answer with a JSON object as text, like the VS Code extension sends.
pub fn json_result(v: Value) -> Value {
    text_result(serde_json::to_string_pretty(&v).unwrap_or_default())
}

fn tools() -> Value {
    let s = |props: Value, required: Value| json!({ "type": "object", "properties": props, "required": required });
    let path = json!({ "type": "string", "description": "Path to the file" });
    json!([
        { "name": "openDiff", "description": "Open a git diff for the file",
          "inputSchema": s(json!({
              "old_file_path": { "type": "string", "description": "Path to the file to show diff for." },
              "new_file_path": { "type": "string", "description": "Path to the file to show diff for." },
              "new_file_contents": { "type": "string", "description": "Contents of the new file." },
              "tab_name": { "type": "string", "description": "Name of the diff tab." }
          }), json!(["old_file_path", "new_file_path", "new_file_contents", "tab_name"])) },
        { "name": "openFile", "description": "Open a file in the editor and optionally select a range of text",
          "inputSchema": s(json!({
              "filePath": path,
              "preview": { "type": "boolean" },
              "startText": { "type": "string", "description": "Text pattern to find the start of the selection range." },
              "endText": { "type": "string", "description": "Text pattern to find the end of the selection range." },
              "selectToEndOfLine": { "type": "boolean" },
              "makeFrontmost": { "type": "boolean" }
          }), json!(["filePath"])) },
        { "name": "getDiagnostics", "description": "Get language diagnostics from the editor",
          "inputSchema": s(json!({ "uri": { "type": "string", "description": "Optional file URI; all files if not provided." } }), json!([])) },
        { "name": "getCurrentSelection", "description": "Get the current text selection in the active editor", "inputSchema": s(json!({}), json!([])) },
        { "name": "getLatestSelection", "description": "Get the most recent text selection (even if not in the active editor)", "inputSchema": s(json!({}), json!([])) },
        { "name": "getOpenEditors", "description": "Get information about currently open editors", "inputSchema": s(json!({}), json!([])) },
        { "name": "getWorkspaceFolders", "description": "Get all workspace folders currently open in the IDE", "inputSchema": s(json!({}), json!([])) },
        { "name": "checkDocumentDirty", "description": "Check if a document has unsaved changes (is dirty)", "inputSchema": s(json!({ "filePath": path }), json!(["filePath"])) },
        { "name": "saveDocument", "description": "Save a document with unsaved changes", "inputSchema": s(json!({ "filePath": path }), json!(["filePath"])) },
        { "name": "close_tab", "description": "Close a tab by name", "inputSchema": s(json!({ "tab_name": { "type": "string" } }), json!(["tab_name"])) },
        { "name": "closeAllDiffTabs", "description": "Close all diff tabs in the editor", "inputSchema": s(json!({}), json!([])) }
    ])
}

fn send(ws: &mut WebSocket<TcpStream>, v: &Value) -> bool {
    ws.send(Message::Text(v.to_string().into())).is_ok()
}

fn serve(
    listener: TcpListener,
    token: String,
    out: Receiver<Outgoing>,
    events: Sender<BridgeEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    connected: Arc<AtomicBool>,
) {
    let emit = |ev: BridgeEvent| {
        if events.send(ev).is_ok() {
            wake();
        }
    };
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        #[allow(clippy::result_large_err)] // the type tungstenite asks for
        let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            let ok = req.headers().get("x-claude-code-ide-authorization").and_then(|v| v.to_str().ok()) == Some(token.as_str());
            if ok {
                Ok(resp)
            } else {
                log::warn!("Claude Code bridge: connection without the right token refused");
                let mut r = ErrorResponse::new(Some("Unauthorized".into()));
                *r.status_mut() = tungstenite::http::StatusCode::UNAUTHORIZED;
                Err(r)
            }
        };
        let Ok(mut ws) = tungstenite::accept_hdr(stream, check) else { continue };
        // Short read timeouts so outgoing messages are sent while waiting for input.
        let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(30)));
        connected.store(true, Ordering::Relaxed);
        emit(BridgeEvent::Connected);
        // Old answers and notifications belong to an earlier connection.
        while out.try_recv().is_ok() {}
        'conn: loop {
            for msg in out.try_iter() {
                let v = match msg {
                    Outgoing::Reply { id, result } => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Outgoing::Notify { method, params } => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
                };
                if !send(&mut ws, &v) {
                    break 'conn;
                }
            }
            let msg = match ws.read() {
                Ok(m) => m,
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    continue;
                }
                Err(_) => break,
            };
            let text = match msg {
                Message::Text(t) => t.to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
            let method = req["method"].as_str().unwrap_or("").to_string();
            let Some(id) = req.get("id").cloned() else { continue }; // notifications: nothing to do
            let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let ok = match method.as_str() {
                "initialize" => send(
                    &mut ws,
                    &reply(json!({
                        "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "Wyven", "version": env!("CARGO_PKG_VERSION") }
                    })),
                ),
                "tools/list" => send(&mut ws, &reply(json!({ "tools": tools() }))),
                "ping" => send(&mut ws, &reply(json!({}))),
                "tools/call" => {
                    let name = req["params"]["name"].as_str().unwrap_or("").to_string();
                    let args = req["params"]["arguments"].clone();
                    emit(BridgeEvent::Call { id: id.clone(), name, args });
                    true
                }
                _ => send(
                    &mut ws,
                    &json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Unknown method {method}") } }),
                ),
            };
            if !ok {
                break;
            }
        }
        connected.store(false, Ordering::Relaxed);
        emit(BridgeEvent::Disconnected);
    }
}

// ----- the editor side: answering tool calls -----

/// A diff Claude Code is waiting for (Ctrl+Enter accepts, Esc rejects).
pub struct PendingDiff {
    id: Value,
    tab_name: String,
    new_contents: String,
}

fn severity_name(s: u8) -> &'static str {
    match s {
        1 => "Error",
        2 => "Warning",
        3 => "Information",
        _ => "Hint",
    }
}

/// Line and column (0-based, like LSP) of a char offset.
fn position(rope: &wyven_buffer::Rope, pos: usize) -> Value {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    json!({ "line": line, "character": pos - rope.line_to_char(line) })
}

impl crate::State {
    /// Starts the bridge (once, at startup) and tells the terminal how to find it.
    pub(crate) fn start_claude_bridge(&mut self) {
        let proxy = self.proxy.clone();
        match Bridge::start(self.project_root(), move || {
            let _ = proxy.send_event(crate::UserEvent::Claude);
        }) {
            Ok(b) => {
                self.terminal.env = vec![
                    ("CLAUDE_CODE_SSE_PORT".into(), b.port.to_string()),
                    ("ENABLE_IDE_INTEGRATION".into(), "true".into()),
                ];
                self.claude = Some(b);
            }
            Err(e) => log::warn!("{e}"),
        }
    }

    /// After opening another folder: Claude Code matches IDEs by their folders.
    pub(crate) fn claude_workspace_changed(&mut self) {
        let root = self.project_root().map(Path::to_path_buf);
        if let Some(b) = &self.claude
            && let Err(e) = b.write_lock(root.as_deref())
        {
            log::warn!("{e}");
        }
    }

    /// Alt → a → c: Claude Code in Wyven's terminal (it connects by itself there).
    pub(crate) fn launch_claude_code(&mut self) {
        self.terminal_visible = true;
        self.focus = crate::Focus::Terminal;
        self.relayout();
        if self.terminal.term.is_none() {
            self.start_terminal();
        }
        if let Some(t) = &self.terminal.term {
            t.type_text("claude\r", false);
        }
        self.window.request_redraw();
    }

    pub(crate) fn claude_poll(&mut self) {
        let Some(bridge) = &self.claude else { return };
        for ev in bridge.poll() {
            match ev {
                BridgeEvent::Connected => {
                    self.claude_sel = None; // send the selection again
                    self.show("Claude Code connected");
                }
                BridgeEvent::Disconnected => {
                    if self.claude_diff.take().is_some() && self.diff.as_ref().is_some_and(|d| d.claude_tab.is_some()) {
                        self.diff = None;
                    }
                    self.show("Claude Code disconnected");
                }
                BridgeEvent::Call { id, name, args } => {
                    log::info!("Claude Code calls {name}");
                    if let Some(result) = self.claude_call(&id, &name, &args)
                        && let Some(b) = &self.claude
                    {
                        b.reply(id, result);
                    }
                }
            }
        }
        self.window.request_redraw();
    }

    fn tab_by_arg(&self, args: &Value, key: &str) -> Option<usize> {
        args[key].as_str().and_then(|p| self.tab_for(Path::new(p)))
    }

    /// Answers a tool call; `None` when the answer comes later (openDiff).
    fn claude_call(&mut self, id: &Value, name: &str, args: &Value) -> Option<Value> {
        Some(match name {
            "openDiff" => {
                self.claude_open_diff(id.clone(), args);
                return None;
            }
            "close_tab" => {
                let tab = args["tab_name"].as_str().unwrap_or("").to_string();
                self.claude_close_diffs(Some(&tab));
                text_result("TAB_CLOSED")
            }
            "closeAllDiffTabs" => {
                let n = self.claude_close_diffs(None);
                text_result(format!("CLOSED_{n}_DIFF_TABS"))
            }
            "getDiagnostics" => {
                let only = args["uri"].as_str().and_then(wyven_lsp::uri_to_path);
                let same = |a: &Path, b: &Path| a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase();
                let mut files = Vec::new();
                for server in self.servers.values() {
                    for (path, list) in &server.diagnostics {
                        if only.as_deref().is_some_and(|o| !same(o, path)) {
                            continue;
                        }
                        let diagnostics: Vec<Value> = list
                            .iter()
                            .map(|d| {
                                json!({
                                    "message": d.message,
                                    "severity": severity_name(d.severity),
                                    "range": { "start": d.start, "end": d.end },
                                    "code": d.code,
                                    "source": server.kind.label(),
                                })
                            })
                            .collect();
                        files.push(json!({ "uri": wyven_lsp::path_to_uri(path), "diagnostics": diagnostics }));
                    }
                }
                json_result(Value::Array(files))
            }
            "openFile" => self.claude_open_file(args),
            "getOpenEditors" => {
                let tabs: Vec<Value> = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| {
                        let p = t.doc.path()?;
                        Some(json!({
                            "uri": wyven_lsp::path_to_uri(p),
                            "isActive": i == self.active,
                            "label": t.file_name(),
                            "languageId": crate::lsp::language_id(p),
                            "isDirty": t.doc.is_dirty(),
                        }))
                    })
                    .collect();
                json_result(json!({ "tabs": tabs }))
            }
            "getWorkspaceFolders" => {
                let folders: Vec<Value> = self
                    .project_root()
                    .map(|p| {
                        json!({
                            "name": p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                            "uri": wyven_lsp::path_to_uri(p),
                            "path": p.display().to_string(),
                        })
                    })
                    .into_iter()
                    .collect();
                let root = self.project_root().map(|p| p.display().to_string());
                json_result(json!({ "success": true, "folders": folders, "rootPath": root }))
            }
            "getCurrentSelection" | "getLatestSelection" => json_result(
                self.claude_selection_json()
                    .unwrap_or_else(|| json!({ "success": false, "message": "No active editor found" })),
            ),
            "checkDocumentDirty" => {
                let path = args["filePath"].as_str().unwrap_or("");
                json_result(match self.tab_by_arg(args, "filePath") {
                    Some(i) => json!({
                        "success": true, "filePath": path, "isDirty": self.tabs[i].doc.is_dirty(), "isUntitled": false
                    }),
                    None => json!({ "success": false, "message": format!("Document not open: {path}") }),
                })
            }
            "saveDocument" => {
                let path = args["filePath"].as_str().unwrap_or("");
                json_result(match self.tab_by_arg(args, "filePath") {
                    Some(i) => {
                        let was = self.active;
                        self.active = i;
                        self.save();
                        self.active = was;
                        let saved = !self.tabs[i].doc.is_dirty();
                        let message = if saved { "Document saved successfully" } else { "Document could not be saved" };
                        json!({ "success": saved, "filePath": path, "saved": saved, "message": message })
                    }
                    None => json!({ "success": false, "message": format!("Document not open: {path}") }),
                })
            }
            _ => json!({ "content": [{ "type": "text", "text": format!("Unknown tool {name}") }], "isError": true }),
        })
    }

    /// The active file's selection in Claude Code's format.
    fn claude_selection_json(&self) -> Option<Value> {
        let ed = self.editor();
        let path = ed.doc.path()?;
        let rope = ed.doc.text();
        let r = ed.doc.selection().primary();
        let to = r.to().min(rope.len_chars());
        let from = r.from().min(to);
        Some(json!({
            "success": true,
            "text": rope.slice(from..to).to_string(),
            "filePath": path.display().to_string(),
            "fileUrl": wyven_lsp::path_to_uri(path),
            "selection": { "start": position(rope, from), "end": position(rope, to), "isEmpty": from == to },
        }))
    }

    /// Tells Claude Code when the cursor or selection moved (called while drawing).
    pub(crate) fn claude_send_selection(&mut self) {
        let Some(bridge) = &self.claude else { return };
        if !bridge.is_connected() {
            return;
        }
        let ed = self.editor();
        let Some(path) = ed.doc.path() else { return };
        let r = ed.doc.selection().primary();
        let key = (path.to_path_buf(), r.anchor, r.head, ed.doc.version());
        if self.claude_sel.as_ref() == Some(&key) {
            return;
        }
        if let Some(mut v) = self.claude_selection_json() {
            if let Some(o) = v.as_object_mut() {
                o.remove("success");
            }
            bridge.notify("selection_changed", v);
        }
        self.claude_sel = Some(key);
    }

    fn claude_open_file(&mut self, args: &Value) -> Value {
        let Some(path) = args["filePath"].as_str().map(PathBuf::from) else {
            return text_result("filePath is missing");
        };
        if !path.is_file() {
            return text_result(format!("File not found: {}", path.display()));
        }
        self.open_path(&path);
        let start_text = args["startText"].as_str().filter(|t| !t.is_empty());
        let mut selected = false;
        if let Some(start_text) = start_text {
            let ed = self.editor_mut();
            let text = ed.doc.text().to_string();
            if let Some(start_b) = text.find(start_text) {
                let end_b = match args["endText"].as_str().filter(|t| !t.is_empty()) {
                    Some(e) => text[start_b..].find(e).map_or(start_b + start_text.len(), |i| start_b + i + e.len()),
                    None => start_b + start_text.len(),
                };
                let rope = ed.doc.text();
                let from = rope.byte_to_char(start_b);
                let mut to = rope.byte_to_char(end_b);
                if args["selectToEndOfLine"].as_bool() == Some(true) {
                    let line = rope.char_to_line(to);
                    to = rope.line_to_char(line) + wyven_buffer::line_len_chars(rope, line);
                }
                ed.doc.set_selection(wyven_buffer::Selection::single(wyven_buffer::Range::new(from, to)));
                ed.ensure_cursor_visible();
                selected = true;
            }
        }
        if args["makeFrontmost"].as_bool() != Some(false) {
            self.focus = crate::Focus::Editor;
            self.window.focus_window();
        }
        self.window.request_redraw();
        if start_text.is_some() && !selected {
            text_result(format!("Opened file: {} (the text to select was not found)", path.display()))
        } else {
            text_result(format!("Opened file: {}", path.display()))
        }
    }

    /// Claude Code proposes an edit: show it side by side and wait for Ctrl+Enter / Esc.
    fn claude_open_diff(&mut self, id: Value, args: &Value) {
        self.claude_reject(); // only one proposal at a time
        let old_path = PathBuf::from(args["old_file_path"].as_str().unwrap_or(""));
        let new_contents = args["new_file_contents"].as_str().unwrap_or("").to_string();
        let tab_name = args["tab_name"].as_str().unwrap_or("").to_string();
        if old_path.is_file() {
            self.open_path(&old_path); // also closes any other diff
        }
        let old = match self.tab_for(&old_path) {
            Some(i) => self.tabs[i].doc.text().to_string(),
            None => std::fs::read_to_string(&old_path).unwrap_or_default(),
        }
        .replace("\r\n", "\n");
        let new = new_contents.replace("\r\n", "\n");
        let name = old_path.file_name().map_or_else(|| tab_name.clone(), |n| n.to_string_lossy().to_string());
        let fs = self.renderer.font_system();
        let mut view = crate::diff_view::DiffView::new(
            fs,
            self.font,
            self.ui_font,
            self.family.clone(),
            &self.theme,
            &format!("Claude Code: {name}"),
            &old,
            &new,
        );
        view.note = "left: now   ·   right: Claude's proposal   ·   Ctrl+Enter accepts, Esc rejects".into();
        view.claude_tab = Some(tab_name.clone());
        view.set_area(self.layout.editor);
        view.jump_change(true);
        self.diff = Some(view);
        self.claude_diff = Some(PendingDiff { id, tab_name, new_contents });
        self.focus = crate::Focus::Editor;
        self.completion = None;
        self.find.close();
        self.window.focus_window();
        self.show("Claude Code proposes a change: Ctrl+Enter accepts, Esc rejects");
        self.window.request_redraw();
    }

    /// Ctrl+Enter in Claude's diff: Claude Code writes the file itself after this answer.
    pub(crate) fn claude_accept(&mut self) {
        let Some(p) = self.claude_diff.take() else { return };
        self.diff = None;
        if let Some(b) = &self.claude {
            b.reply(
                p.id,
                json!({ "content": [
                    { "type": "text", "text": "FILE_SAVED" },
                    { "type": "text", "text": p.new_contents },
                ] }),
            );
        }
        self.show("Change accepted");
        self.window.request_redraw();
    }

    fn claude_reject(&mut self) {
        let Some(p) = self.claude_diff.take() else { return };
        if let Some(b) = &self.claude {
            b.reply(
                p.id,
                json!({ "content": [
                    { "type": "text", "text": "DIFF_REJECTED" },
                    { "type": "text", "text": p.tab_name },
                ] }),
            );
        }
    }

    /// A Claude diff that is no longer shown (Esc, another tab, …) counts as rejected.
    pub(crate) fn claude_check_diff(&mut self) {
        let Some(p) = &self.claude_diff else { return };
        let shown = self.diff.as_ref().and_then(|d| d.claude_tab.as_deref()) == Some(p.tab_name.as_str());
        if !shown {
            self.claude_reject();
            self.show("Change rejected");
        }
    }

    /// Closes Claude's diff (one tab or all); returns how many were closed.
    fn claude_close_diffs(&mut self, tab: Option<&str>) -> usize {
        let open = self.diff.as_ref().and_then(|d| d.claude_tab.clone());
        let matches = open.as_deref().is_some_and(|t| tab.is_none_or(|want| want == t));
        if !matches {
            return 0;
        }
        if self.claude_diff.as_ref().is_some_and(|p| Some(p.tab_name.as_str()) == open.as_deref()) {
            self.claude_reject();
        }
        self.diff = None;
        self.window.request_redraw();
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_tools_and_calls_work_with_the_token_only() {
        let dir = std::env::temp_dir().join(format!("wyven-claude-test-{}", std::process::id()));
        // SAFETY: tests in this module run single-threaded for this variable.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", &dir) };
        let bridge = Bridge::start(Some(Path::new("C:/project")), || {}).unwrap();
        let lock: Value = serde_json::from_str(&std::fs::read_to_string(&bridge.lock_path).unwrap()).unwrap();
        assert_eq!(lock["ideName"], "Wyven");
        assert_eq!(lock["transport"], "ws");
        let token = lock["authToken"].as_str().unwrap().to_string();
        let url = format!("ws://127.0.0.1:{}", bridge.port);

        // Without the token: refused.
        assert!(tungstenite::connect(url.as_str()).is_err());

        let mut req = tungstenite::client::IntoClientRequest::into_client_request(url.as_str()).unwrap();
        req.headers_mut().insert("x-claude-code-ide-authorization", token.parse().unwrap());
        let (mut ws, _) = tungstenite::connect(req).unwrap();
        let call = |ws: &mut WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>, v: Value| -> Value {
            ws.send(Message::Text(v.to_string().into())).unwrap();
            loop {
                if let Message::Text(t) = ws.read().unwrap() {
                    return serde_json::from_str(&t).unwrap();
                }
            }
        };
        let init = call(&mut ws, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}));
        assert_eq!(init["result"]["serverInfo"]["name"], "Wyven");
        let list = call(&mut ws, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"openDiff") && names.contains(&"getDiagnostics"));

        // A tool call reaches the UI side, and its reply goes back.
        ws.send(Message::Text(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "getWorkspaceFolders", "arguments": {}}}).to_string().into())).unwrap();
        let ev = loop {
            if let Some(ev) = bridge.poll().into_iter().find(|e| matches!(e, BridgeEvent::Call { .. })) {
                break ev;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let BridgeEvent::Call { id, name, .. } = ev else { unreachable!() };
        assert_eq!(name, "getWorkspaceFolders");
        bridge.reply(id, text_result("ok"));
        let answer = loop {
            if let Message::Text(t) = ws.read().unwrap() {
                break serde_json::from_str::<Value>(&t).unwrap();
            }
        };
        assert_eq!(answer["id"], 3);
        assert_eq!(answer["result"]["content"][0]["text"], "ok");

        let lock_path = bridge.lock_path.clone();
        drop(bridge);
        assert!(!lock_path.exists(), "the lock file goes away with the bridge");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
