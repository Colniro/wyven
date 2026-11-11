//! Asks Roslyn for inlay hints and code lenses and prints which settings it requests.
//!
//! cargo run -p wyven-lsp --example probe_hints -- <server.exe> <project.csproj> <file.cs> [on]
//! With `on`, every configuration item that mentions inlay hints is answered with `true`.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wyven_lsp::{initialize_params, json, path_to_uri, Client, Incoming, Value};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (server, project, file) = (PathBuf::from(&args[1]), PathBuf::from(&args[2]), PathBuf::from(&args[3]));
    let hints_on = args.get(4).is_some_and(|a| a == "on");
    let root = project.parent().unwrap().to_path_buf();
    let (tx, rx) = mpsc::channel();
    let log_dir = std::env::temp_dir().join("wyven-probe-logs");
    let client = Client::spawn(
        "probe",
        &server,
        &if server.to_string_lossy().ends_with("tsc.exe") {
            vec!["--lsp".into(), "--stdio".into()]
        } else {
            vec!["--stdio".into(), "--logLevel".into(), "Warning".into(), "--extensionLogDirectory".into(), log_dir.display().to_string()]
        },
        &root,
        move |m| {
            let _ = tx.send(m);
        },
    )
    .expect("spawn");
    let started = Instant::now();
    client.request("initialize", initialize_params(&root, std::process::id()));
    let text = std::fs::read_to_string(&file).unwrap();
    let uri = path_to_uri(&file);
    let lines = text.lines().count();
    let mut asked = false;
    let mut answers = 0;
    while started.elapsed() < Duration::from_secs(60) && answers < 2 {
        let Ok(msg) = rx.recv_timeout(Duration::from_millis(500)) else {
            if !asked && started.elapsed() > Duration::from_secs(3) && !server.to_string_lossy().contains("CodeAnalysis") {
                asked = true;
                client.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": { "uri": uri, "languageId": "typescript", "version": 1, "text": text } }),
                );
                std::thread::sleep(Duration::from_millis(1500));
                client.request(
                    "textDocument/inlayHint",
                    json!({ "textDocument": { "uri": uri }, "range": { "start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0} } }),
                );
                client.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
            }
            continue;
        };
        match msg {
            Incoming::Response { id, result } => {
                if id == 1 {
                    client.notify("initialized", json!({}));
                    client.notify("project/open", json!({ "projects": [path_to_uri(&project)] }));
                } else {
                    answers += 1;
                    let v = result.unwrap_or_else(|e| json!({ "error": e }));
                    println!("== response {id} ==\n{}", serde_json_pretty(&v));
                }
            }
            Incoming::Request { id, method, params } => {
                let result = if method == "workspace/configuration" {
                    let items = params["items"].as_array().cloned().unwrap_or_default();
                    for it in &items {
                        println!("config asked: {}", it["section"]);
                    }
                    let answers: Vec<Value> = items
                        .iter()
                        .map(|it| {
                            let s = it["section"].as_str().unwrap_or("");
                            if hints_on && s.contains("inlay_hints") {
                                json!(true)
                            } else if hints_on && matches!(s, "js/ts" | "typescript" | "javascript") {
                                json!({
                                    "inlayHints": {
                                        "parameterNames": { "enabled": "literals" },
                                        "variableTypes": { "enabled": true }
                                    },
                                    "referencesCodeLens": { "enabled": true, "showOnAllFunctions": true }
                                })
                            } else {
                                Value::Null
                            }
                        })
                        .collect();
                    json!(answers)
                } else {
                    Value::Null
                };
                client.respond(id, result);
            }
            Incoming::Notification { method, params } if method == "textDocument/publishDiagnostics" => {
                println!("diagnostics: {}", params.to_string().chars().take(600).collect::<String>());
            }
            Incoming::Notification { method, .. }
                if (method == "workspace/projectInitializationComplete" || (!asked && started.elapsed() > Duration::from_secs(3) && !server.to_string_lossy().contains("CodeAnalysis")))
                    && !asked =>
            {
                asked = true;
                println!("projects loaded after {:?}", started.elapsed());
                client.notify(
                    "textDocument/didOpen",
                    json!({ "textDocument": { "uri": uri, "languageId": "csharp", "version": 1, "text": text } }),
                );
                std::thread::sleep(Duration::from_millis(500));
                client.request(
                    "textDocument/inlayHint",
                    json!({ "textDocument": { "uri": uri }, "range": { "start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0} } }),
                );
                client.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
            }
            _ => {}
        }
    }
}

fn serde_json_pretty(v: &Value) -> String {
    let s = v.to_string();
    s.chars().take(2500).collect()
}
