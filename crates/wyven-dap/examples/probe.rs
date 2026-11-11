//! Drives netcoredbg through a whole session and prints what comes back.
//!
//! `cargo run -p wyven-dap --example probe -- <app.dll> <source.cs> <line> [console]`
//! `console` (optional): value for the launch `console` field, e.g. "externalTerminal".
//! `cargo run -p wyven-dap --example probe -- pid:<process id> <source.cs> <line>` attaches.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use wyven_dap::{find_netcoredbg, json, Client, Incoming, Value};

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (dll, source, line) = (&args[0], &args[1], args[2].parse::<u64>().unwrap());
    let console = args.get(3).cloned();
    let exe = find_netcoredbg().expect("netcoredbg not found");
    println!("adapter: {}", exe.display());
    let (tx, rx) = mpsc::channel();
    let attach_pid = dll.strip_prefix("pid:").map(|p| p.parse::<u64>().unwrap());
    let cwd = if attach_pid.is_some() { Path::new(source).parent().unwrap() } else { Path::new(dll).parent().unwrap() };
    let client = Client::spawn(&exe, &["--interpreter=vscode"], cwd, move |m| {
        let _ = tx.send(m);
    })
    .unwrap();
    client.request(
        "initialize",
        json!({"clientID": "wyven", "adapterID": "coreclr", "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path", "supportsRunInTerminalRequest": true}),
    );
    let mut launch = json!({"program": dll, "cwd": cwd, "args": [], "stopAtEntry": false, "justMyCode": true});
    if let Some(c) = console {
        launch["console"] = json!(c);
    }
    let mut frame_id = None;
    let mut stops = 0;
    let mut thread_id = 1;
    loop {
        let Ok(msg) = rx.recv_timeout(Duration::from_secs(30)) else {
            println!("timeout");
            break;
        };
        match msg {
            Incoming::Response { command, result, .. } => {
                let short = |v: &Value| v.to_string().chars().take(600).collect::<String>();
                match &result {
                    Ok(v) => println!("<- {command}: {}", short(v)),
                    Err(e) => println!("<- {command} FAILED: {e}"),
                }
                match command.as_str() {
                    "initialize" => match attach_pid {
                        Some(pid) => {
                            client.request("attach", json!({"processId": pid}));
                        }
                        None => {
                            client.request("launch", launch.clone());
                        }
                    },
                    "stackTrace" => {
                        let v = result.unwrap_or_default();
                        frame_id = v["stackFrames"][0]["id"].as_i64();
                        if let Some(f) = frame_id {
                            client.request("scopes", json!({"frameId": f}));
                            client.request("evaluate", json!({"expression": "a + b * 10", "frameId": f, "context": "hover"}));
                        }
                    }
                    "scopes" => {
                        let v = result.unwrap_or_default();
                        if let Some(r) = v["scopes"][0]["variablesReference"].as_i64() {
                            client.request("variables", json!({"variablesReference": r}));
                        }
                    }
                    "variables" => {
                        if stops == 1 {
                            client.request("next", json!({"threadId": thread_id}));
                        } else {
                            client.request("continue", json!({"threadId": thread_id}));
                        }
                    }
                    _ => {}
                }
            }
            Incoming::Event { event, body } => {
                println!("event {event}: {}", body.to_string().chars().take(300).collect::<String>());
                match event.as_str() {
                    "initialized" => {
                        client.request(
                            "setBreakpoints",
                            json!({"source": {"path": source}, "breakpoints": [{"line": line}]}),
                        );
                        client.request("configurationDone", json!({}));
                    }
                    "stopped" => {
                        stops += 1;
                        let thread = body["threadId"].as_i64().unwrap_or(1);
                        thread_id = thread;
                        client.request("stackTrace", json!({"threadId": thread, "startFrame": 0, "levels": 20}));
                    }
                    "terminated" => break,
                    _ => {}
                }
            }
            Incoming::Request { seq, command, arguments } => {
                println!("reverse request {command}: {arguments}");
                client.respond(seq, &command, false, json!({}));
            }
            Incoming::Exited => {
                println!("adapter exited");
                break;
            }
        }
    }
    let _ = frame_id;
}
