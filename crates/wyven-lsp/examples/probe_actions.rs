//! Asks a server for code actions and a rename at a position and prints the raw answers.
//!
//! cargo run -p wyven-lsp --example probe_actions -- <server.exe> <project> <file.cs> <line> <col>

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wyven_lsp::{initialize_params, json, path_to_uri, Client, Incoming, Value};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (server, root, file) = (PathBuf::from(&a[1]), PathBuf::from(&a[2]), PathBuf::from(&a[3]));
    let (line, col): (u64, u64) = (a[4].parse().unwrap(), a[5].parse().unwrap());
    let (tx, rx) = mpsc::channel();
    let client = Client::spawn(
        "probe",
        &server,
        &["--stdio".into(), "--autoLoadProjects".into(), "--logLevel".into(), "Warning".into(),
          "--extensionLogDirectory".into(), std::env::temp_dir().join("wyven-probe-logs").display().to_string()],
        &root,
        move |m| { let _ = tx.send(m); },
    ).unwrap();
    let uri = path_to_uri(&file);
    let text = std::fs::read_to_string(&file).unwrap();
    let init = client.request("initialize", initialize_params(&root, std::process::id()));
    let started = Instant::now();
    let (mut diag_id, mut action_id, mut resolve_id, mut rename_id) = (-1, -1, -1, -1);
    let pos = json!({ "line": line, "character": col });
    while started.elapsed() < Duration::from_secs(60) {
        let Ok(msg) = rx.recv_timeout(Duration::from_millis(500)) else { continue };
        match msg {
            Incoming::Response { id, result } if id == init => {
                let _ = result;
                client.notify("initialized", json!({}));
                client.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri, "languageId": "csharp", "version": 1, "text": text } }));
            }
            Incoming::Notification { method, .. } if method == "workspace/projectInitializationComplete" => {
                diag_id = client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
            }
            Incoming::Response { id, result } if id == diag_id => {
                let items = result.unwrap()["items"].clone();
                let here: Vec<Value> = items.as_array().unwrap().iter().filter(|d| d["range"]["start"]["line"] == line).cloned().collect();
                println!("diagnostics on line: {}", here.len());
                action_id = client.request("textDocument/codeAction", json!({
                    "textDocument": { "uri": uri },
                    "range": { "start": pos, "end": pos },
                    "context": { "diagnostics": here, "triggerKind": 1 }
                }));
                rename_id = client.request("textDocument/rename", json!({ "textDocument": { "uri": uri }, "position": { "line": 6, "character": 18 }, "newName": "Spieler" }));
            }
            Incoming::Response { id, result } if id == action_id => {
                let v = result.unwrap();
                for (i, act) in v.as_array().unwrap().iter().enumerate() {
                    let mut short = act.clone();
                    if let Some(o) = short.as_object_mut() { o.remove("diagnostics"); }
                    let s = short.to_string();
                    println!("action {i}: {}", s.chars().take(700).collect::<String>());
                }
                if let Some(first) = v.as_array().unwrap().iter().find(|a| a.get("data").is_some() && a.get("edit").is_none()) {
                    resolve_id = client.request("codeAction/resolve", first.clone());
                }
            }
            Incoming::Response { id, result } if id == resolve_id => {
                println!("resolved: {}", result.map(|v| v.to_string()).unwrap_or_else(|e| e).chars().take(900).collect::<String>());
            }
            Incoming::Response { id, result } if id == rename_id => {
                println!("rename: {}", result.map(|v| v.to_string()).unwrap_or_else(|e| e).chars().take(900).collect::<String>());
            }
            Incoming::Request { id, method, params } => {
                let r = if method == "workspace/configuration" { json!(vec![Value::Null; params["items"].as_array().map_or(0, Vec::len)]) } else { Value::Null };
                client.respond(id, r);
            }
            _ => {}
        }
        if resolve_id >= 0 && rename_id >= 0 && started.elapsed() > Duration::from_secs(25) { break; }
    }
}
