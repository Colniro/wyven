//! A small Language Server Protocol client.
//!
//! The server runs as a child process and talks JSON-RPC over stdin/stdout. A reader thread
//! parses incoming messages and hands them to a callback (the app wakes its event loop there),
//! so the UI thread never blocks on the server.
//!
//! Positions: LSP counts columns in UTF-16 code units; [`to_lsp`] / [`from_lsp`] convert to
//! and from the rope's char positions.

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub use serde_json::{json, Value};
use wyven_buffer::Rope;

/// A message from the server.
#[derive(Debug)]
pub enum Incoming {
    Notification { method: String, params: Value },
    /// The server asks the client something; must be answered with [`Client::respond`].
    Request { id: Value, method: String, params: Value },
    Response { id: i64, result: Result<Value, String> },
    /// The server process ended (crashed or exited).
    Exited,
}

pub struct Client {
    name: String,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Child,
    next_id: AtomicI64,
}

impl Client {
    /// Starts the server. `sink` is called on the reader thread for every message.
    pub fn spawn(
        name: &str,
        program: &Path,
        args: &[String],
        cwd: &Path,
        sink: impl Fn(Incoming) + Send + 'static,
    ) -> io::Result<Self> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("piped stdin")));

        let label = name.to_string();
        std::thread::Builder::new()
            .name(format!("lsp-{name}-out"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    match read_message(&mut reader) {
                        Ok(Some(msg)) => {
                            if let Some(incoming) = classify(msg) {
                                sink(incoming);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            log::warn!("lsp {label}: read error: {e}");
                            break;
                        }
                    }
                }
                sink(Incoming::Exited);
            })?;

        let label = name.to_string();
        std::thread::Builder::new()
            .name(format!("lsp-{name}-err"))
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!("lsp {label} stderr: {line}");
                }
            })?;

        Ok(Self { name: name.to_string(), stdin, child, next_id: AtomicI64::new(1) })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn send(&self, msg: &Value) {
        let body = msg.to_string();
        let mut stdin = self.stdin.lock().unwrap();
        let result = write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body)
            .and_then(|_| stdin.flush());
        if let Err(e) = result {
            log::warn!("lsp {}: write failed: {e}", self.name);
        }
    }

    /// Sends a request; returns its id (the response arrives as [`Incoming::Response`]).
    pub fn request(&self, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    pub fn respond(&self, id: Value, result: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    /// Polite shutdown, then kill if the server does not exit by itself.
    pub fn shutdown(&mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = self.child.kill();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut len = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn classify(mut msg: Value) -> Option<Incoming> {
    let method = msg.get("method").and_then(Value::as_str).map(str::to_string);
    let params = msg.get_mut("params").map(Value::take).unwrap_or(Value::Null);
    match (method, msg.get("id").cloned()) {
        (Some(method), Some(id)) => Some(Incoming::Request { id, method, params }),
        (Some(method), None) => Some(Incoming::Notification { method, params }),
        (None, Some(id)) => {
            let id = id.as_i64()?;
            let result = match msg.get_mut("error") {
                Some(err) => Err(err.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                None => Ok(msg.get_mut("result").map(Value::take).unwrap_or(Value::Null)),
            };
            Some(Incoming::Response { id, result })
        }
        (None, None) => None,
    }
}

// ----- paths and positions -----

pub fn path_to_uri(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file:///{}", path.display()))
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    url::Url::parse(uri).ok()?.to_file_path().ok()
}

/// Char position → LSP `{line, character}` (UTF-16 columns).
pub fn to_lsp(rope: &Rope, pos: usize) -> Value {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    let start = rope.line_to_char(line);
    let col = rope.char_to_utf16_cu(pos) - rope.char_to_utf16_cu(start);
    json!({ "line": line, "character": col })
}

/// LSP `{line, character}` → char position, clamped to the document.
pub fn from_lsp(rope: &Rope, p: &Value) -> usize {
    let line = p.get("line").and_then(Value::as_u64).unwrap_or(0) as usize;
    let col = p.get("character").and_then(Value::as_u64).unwrap_or(0) as usize;
    if line >= rope.len_lines() {
        return rope.len_chars();
    }
    let start = rope.line_to_char(line);
    let line_end = start + wyven_buffer::line_len_chars(rope, line);
    let base = rope.char_to_utf16_cu(start);
    let target = (base + col).min(rope.char_to_utf16_cu(line_end));
    rope.utf16_cu_to_char(target).min(line_end)
}

/// Capabilities Wyven supports, sent in `initialize`.
pub fn initialize_params(root: &Path, process_id: u32) -> Value {
    let uri = path_to_uri(root);
    let name = root.file_name().map_or("project".into(), |n| n.to_string_lossy().into_owned());
    json!({
        "processId": process_id,
        "clientInfo": { "name": "Wyven", "version": env!("CARGO_PKG_VERSION") },
        // Wyven's interface is English: server texts ("3 references", compiler errors) too.
        "locale": "en",
        "rootUri": uri,
        "workspaceFolders": [{ "uri": uri, "name": name }],
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "workspace": {
                "configuration": true,
                "workspaceFolders": true,
                "applyEdit": true,
                "workspaceEdit": { "documentChanges": true }
            },
            "window": { "workDoneProgress": true },
            "textDocument": {
                "synchronization": { "didSave": true, "dynamicRegistration": false },
                "publishDiagnostics": { "relatedInformation": false },
                "diagnostic": { "dynamicRegistration": false },
                "completion": {
                    "completionItem": {
                        "snippetSupport": false,
                        "documentationFormat": ["plaintext", "markdown"],
                        "insertReplaceSupport": false
                    },
                    "contextSupport": true
                },
                "hover": { "contentFormat": ["plaintext", "markdown"] },
                "definition": { "linkSupport": true },
                "references": {},
                "inlayHint": { "dynamicRegistration": false },
                "codeLens": { "dynamicRegistration": false },
                "formatting": {},
                "rename": { "prepareSupport": false }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_positions_roundtrip() {
        // "ü" is 1 UTF-16 unit, "😀" is 2.
        let rope = Rope::from_str("aü😀b\nzweite");
        let pos = 4; // before "b"
        let lsp = to_lsp(&rope, pos);
        assert_eq!(lsp, json!({ "line": 0, "character": 5 }));
        assert_eq!(from_lsp(&rope, &lsp), pos);
        assert_eq!(from_lsp(&rope, &json!({ "line": 1, "character": 99 })), rope.len_chars());
    }

    #[test]
    fn parses_framed_messages() {
        let body = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        let raw = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let msg = read_message(&mut raw.as_bytes()).unwrap().unwrap();
        match classify(msg) {
            Some(Incoming::Response { id: 3, result: Ok(v) }) => assert_eq!(v["ok"], true),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn windows_paths_become_file_uris() {
        if cfg!(windows) {
            let uri = path_to_uri(Path::new(r"C:\Projekte\Mein Spiel\A.cs"));
            assert_eq!(uri, "file:///C:/Projekte/Mein%20Spiel/A.cs");
            assert_eq!(uri_to_path(&uri).unwrap(), PathBuf::from(r"C:\Projekte\Mein Spiel\A.cs"));
            // Roslyn sends the drive colon encoded.
            assert_eq!(
                uri_to_path("file:///c%3A/Projekte/A.cs").unwrap(),
                PathBuf::from(r"c:\Projekte\A.cs")
            );
        }
    }
}
