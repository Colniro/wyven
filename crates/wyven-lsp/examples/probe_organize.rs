//! Asks Roslyn for "organize imports" code actions and implementations and prints the answers.
//!
//! cargo run -p wyven-lsp --example probe_organize -- <server.exe> <project> <file.cs>

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wyven_lsp::{initialize_params, json, path_to_uri, Client, Incoming, Value};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (server, root, file) = (PathBuf::from(&a[1]), PathBuf::from(&a[2]), PathBuf::from(&a[3]));
    let (tx, rx) = mpsc::channel();
    let logs = std::env::temp_dir().join("wyven-probe-logs").display().to_string();
    let args = ["--stdio", "--autoLoadProjects", "--logLevel", "Warning", "--extensionLogDirectory", &logs].map(String::from);
    let client = Client::spawn("probe", &server, &args, &root, move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    let uri = path_to_uri(&file);
    let text = std::fs::read_to_string(&file).unwrap();
    let lines = text.lines().count() as u64;
    let init = client.request("initialize", initialize_params(&root, std::process::id()));
    let started = Instant::now();
    let (mut only_id, mut all_id, mut impl_id, mut resolve_id) = (-1, -1, -1, -1);
    while started.elapsed() < Duration::from_secs(90) {
        let Ok(msg) = rx.recv_timeout(Duration::from_millis(500)) else { continue };
        match msg {
            Incoming::Response { id, .. } if id == init => {
                client.notify("initialized", json!({}));
                client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "csharp", "version": 1, "text": text } }));
            }
            Incoming::Notification { method, .. } if method == "workspace/projectInitializationComplete" => {
                let whole = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": lines, "character": 0 } });
                only_id = client.request("textDocument/codeAction", json!({
                    "textDocument": { "uri": uri }, "range": whole,
                    "context": { "diagnostics": [], "only": ["source.organizeImports"], "triggerKind": 1 }
                }));
                all_id = client.request("textDocument/codeAction", json!({
                    "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                    "context": { "diagnostics": [], "triggerKind": 1 }
                }));
                impl_id = client.request("textDocument/implementation", json!({ "textDocument": { "uri": uri }, "position": { "line": 7, "character": 18 } }));
            }
            Incoming::Response { id, result } if id == only_id || id == all_id => {
                let label = if id == only_id { "only organizeImports" } else { "all at line 0" };
                let v = result.unwrap_or(Value::Null);
                for act in v.as_array().into_iter().flatten() {
                    println!("{label}: {} | kind {} | edit {} | data {}", act["title"], act["kind"], act.get("edit").is_some(), act.get("data").is_some());
                }
                if v.as_array().is_none_or(|a| a.is_empty()) {
                    println!("{label}: (none)");
                }
                if id == only_id && let Some(first) = v.as_array().and_then(|a| a.first()) {
                    resolve_id = client.request("codeAction/resolve", first.clone());
                }
            }
            Incoming::Response { id, result } if id == resolve_id => {
                println!("resolved: {}", result.map(|v| v["edit"].to_string()).unwrap_or_else(|e| e).chars().take(600).collect::<String>());
            }
            Incoming::Response { id, result } if id == impl_id => {
                println!("implementation: {}", result.map(|v| v.to_string()).unwrap_or_else(|e| e).chars().take(600).collect::<String>());
            }
            Incoming::Request { id, method, params } => {
                let r = if method == "workspace/configuration" { json!(vec![Value::Null; params["items"].as_array().map_or(0, Vec::len)]) } else { Value::Null };
                client.respond(id, r);
            }
            _ => {}
        }
        if impl_id >= 0 && started.elapsed() > Duration::from_secs(40) {
            break;
        }
    }
}
