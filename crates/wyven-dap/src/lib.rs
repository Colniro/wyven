//! A small Debug Adapter Protocol client.
//!
//! The adapter (netcoredbg) runs as a child process and talks DAP over stdin/stdout: the same
//! `Content-Length` framing as LSP, but with `seq`/`type` messages. A reader thread hands every
//! message to a callback, so the UI thread never waits for the debugger.

use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

pub use serde_json::{json, Value};

/// A message from the adapter.
#[derive(Debug)]
pub enum Incoming {
    Event { event: String, body: Value },
    Response { request_seq: i64, command: String, result: Result<Value, String> },
    /// The adapter asks the client something (e.g. `runInTerminal`).
    Request { seq: i64, command: String, arguments: Value },
    /// The adapter process ended.
    Exited,
}

pub struct Client {
    stdin: Arc<Mutex<ChildStdin>>,
    child: Child,
    seq: AtomicI64,
}

impl Client {
    /// Starts the adapter. `sink` is called on the reader thread for every message.
    pub fn spawn(
        program: &Path,
        args: &[&str],
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

        std::thread::Builder::new().name("dap-out".into()).spawn(move || {
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
                        log::warn!("dap: read error: {e}");
                        break;
                    }
                }
            }
            sink(Incoming::Exited);
        })?;
        std::thread::Builder::new().name("dap-err".into()).spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log::debug!("dap stderr: {line}");
            }
        })?;
        Ok(Self { stdin, child, seq: AtomicI64::new(1) })
    }

    fn send(&self, mut msg: Value) -> i64 {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        msg["seq"] = json!(seq);
        let body = msg.to_string();
        log::debug!("dap -> {body}");
        let mut stdin = self.stdin.lock().unwrap();
        let result = write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).and_then(|_| stdin.flush());
        if let Err(e) = result {
            log::warn!("dap: write failed: {e}");
        }
        seq
    }

    /// Sends a request; returns its `seq` (the response carries it as `request_seq`).
    pub fn request(&self, command: &str, arguments: Value) -> i64 {
        self.send(json!({ "type": "request", "command": command, "arguments": arguments }))
    }

    /// Answers a reverse request from the adapter.
    pub fn respond(&self, request_seq: i64, command: &str, success: bool, body: Value) {
        self.send(json!({
            "type": "response",
            "request_seq": request_seq,
            "command": command,
            "success": success,
            "body": body,
        }));
    }

    pub fn kill(&mut self) {
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
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    log::debug!("dap <- {}", String::from_utf8_lossy(&body));
    serde_json::from_slice(&body).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn classify(mut msg: Value) -> Option<Incoming> {
    let take = |msg: &mut Value, key: &str| msg.get_mut(key).map(Value::take).unwrap_or(Value::Null);
    match msg["type"].as_str()? {
        "event" => Some(Incoming::Event { event: msg["event"].as_str()?.to_string(), body: take(&mut msg, "body") }),
        "response" => {
            let request_seq = msg["request_seq"].as_i64()?;
            let command = msg["command"].as_str().unwrap_or("").to_string();
            let result = if msg["success"].as_bool().unwrap_or(false) {
                Ok(take(&mut msg, "body"))
            } else {
                Err(msg["body"]["error"]["format"]
                    .as_str()
                    .or(msg["message"].as_str())
                    .unwrap_or("failed")
                    .to_string())
            };
            Some(Incoming::Response { request_seq, command, result })
        }
        "request" => Some(Incoming::Request {
            seq: msg["seq"].as_i64()?,
            command: msg["command"].as_str()?.to_string(),
            arguments: take(&mut msg, "arguments"),
        }),
        _ => None,
    }
}

/// Where netcoredbg is: `netcoredbg` on PATH first, then `%APPDATA%\Wyven\netcoredbg`.
/// Returns the places searched if it is not found.
pub fn find_netcoredbg() -> Result<PathBuf, Vec<PathBuf>> {
    let exe = if cfg!(windows) { "netcoredbg.exe" } else { "netcoredbg" };
    let mut searched = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
        searched.push(PathBuf::from("PATH"));
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(base) = base {
        let p = base.join(if cfg!(windows) { "Wyven" } else { "wyven" }).join("netcoredbg").join(exe);
        if p.is_file() {
            return Ok(p);
        }
        searched.push(p);
    }
    Err(searched)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_classified() {
        let ev = classify(json!({"seq": 3, "type": "event", "event": "stopped", "body": {"reason": "breakpoint", "threadId": 1}}));
        assert!(matches!(ev, Some(Incoming::Event { ref event, .. }) if event == "stopped"));
        let ok = classify(json!({"seq": 4, "type": "response", "request_seq": 2, "command": "launch", "success": true}));
        assert!(matches!(ok, Some(Incoming::Response { request_seq: 2, result: Ok(_), .. })));
        let err = classify(json!({"seq": 5, "type": "response", "request_seq": 3, "command": "evaluate", "success": false, "message": "error: x not found"}));
        assert!(matches!(err, Some(Incoming::Response { result: Err(ref e), .. }) if e.contains("not found")));
    }

    #[test]
    fn framing_round_trips() {
        let body = r#"{"seq":1,"type":"event","event":"initialized"}"#;
        let raw = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut r = io::Cursor::new(raw.into_bytes());
        let msg = read_message(&mut r).unwrap().unwrap();
        assert_eq!(msg["event"], "initialized");
        assert!(read_message(&mut r).unwrap().is_none());
    }
}
