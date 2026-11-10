//! Talks to a language server and prints what it sends. For trying out servers.
//!
//! cargo run -p wyvern-lsp --example probe -- <server.exe> <project folder> <file.cs>

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wyvern_lsp::{initialize_params, json, path_to_uri, Client, Incoming};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (server, root, file) = (PathBuf::from(&args[1]), PathBuf::from(&args[2]), PathBuf::from(&args[3]));
    let (tx, rx) = mpsc::channel();
    let log_dir = std::env::temp_dir().join("wyvern-probe-logs");
    let client = Client::spawn(
        "probe",
        &server,
        &[
            "--stdio".into(),
            "--autoLoadProjects".into(),
            "--logLevel".into(),
            "Information".into(),
            "--extensionLogDirectory".into(),
            log_dir.display().to_string(),
        ],
        &root,
        move |m| {
            let _ = tx.send(m);
        },
    )
    .expect("spawn");
    let started = Instant::now();
    let init = client.request("initialize", initialize_params(&root, std::process::id()));
    let text = std::fs::read_to_string(&file).unwrap();
    let uri = path_to_uri(&file);
    let mut opened = false;
    let mut asked = false;
    while started.elapsed() < Duration::from_secs(40) {
        let Ok(msg) = rx.recv_timeout(Duration::from_millis(500)) else {
            if opened && !asked && started.elapsed() > Duration::from_secs(20) {
                asked = true;
                let id = client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
                println!("{:>6.1}s -> pull diagnostics (id {id})", started.elapsed().as_secs_f32());
                let id = client.request("textDocument/completion", json!({
                    "textDocument": { "uri": uri }, "position": { "line": 15, "character": 20 }
                }));
                println!("{:>6.1}s -> completion (id {id})", started.elapsed().as_secs_f32());
            }
            continue;
        };
        let t = started.elapsed().as_secs_f32();
        match msg {
            Incoming::Response { id, result } if id == init => {
                let caps = result.unwrap();
                println!("{t:>6.1}s initialize ok; capabilities: {}", caps["capabilities"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default());
                println!("        diagnosticProvider: {}", caps["capabilities"]["diagnosticProvider"]);
                println!("        textDocumentSync: {}", caps["capabilities"]["textDocumentSync"]);
                client.notify("initialized", json!({}));
                // PROBE_OPEN_PROJECT=1: ask Roslyn explicitly to load the .csproj files in root.
                if std::env::var("PROBE_OPEN_PROJECT").is_ok() {
                    let projects: Vec<String> = std::fs::read_dir(&root)
                        .unwrap()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x == "csproj"))
                        .map(|p| path_to_uri(&p))
                        .collect();
                    println!("        -> project/open {projects:?}");
                    client.notify("project/open", json!({ "projects": projects }));
                }
                client.notify("textDocument/didOpen", json!({
                    "textDocument": { "uri": uri, "languageId": "csharp", "version": 1, "text": text }
                }));
                opened = true;
            }
            Incoming::Response { id, result } => {
                let s = match result {
                    Ok(v) => {
                        let s = v.to_string();
                        s.chars().take(600).collect::<String>()
                    }
                    Err(e) => format!("ERROR {e}"),
                };
                println!("{t:>6.1}s <- response {id}: {s}");
            }
            Incoming::Request { id, method, params } => {
                println!("{t:>6.1}s <- request {method} {}", params.to_string().chars().take(200).collect::<String>());
                let result = if method == "workspace/configuration" {
                    let n = params["items"].as_array().map_or(0, Vec::len);
                    json!(vec![serde_json::Value::Null; n])
                } else {
                    serde_json::Value::Null
                };
                client.respond(id, result);
            }
            Incoming::Notification { method, params } => {
                let s = params.to_string();
                if method == "window/logMessage" || method == "$/progress" {
                    println!("{t:>6.1}s <- {method} {}", s.chars().take(140).collect::<String>());
                } else {
                    println!("{t:>6.1}s <- {method} {}", s.chars().take(400).collect::<String>());
                }
            }
            Incoming::Exited => {
                println!("{t:>6.1}s server exited");
                break;
            }
        }
    }
}
