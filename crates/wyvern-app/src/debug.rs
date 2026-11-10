//! Debugging .NET programs with netcoredbg.
//!
//! F5 builds the project (`dotnet build`), asks MSBuild for the built `.dll`, starts it with
//! `dotnet` in its own console window and attaches netcoredbg. A tiny startup hook
//! (`DOTNET_STARTUP_HOOKS`) makes the program wait until the debugger is attached, so
//! breakpoints early in `Main` are hit, and keeps the window open at the end.
//! netcoredbg's own `launch` cannot do this: it always redirects the program's console.
//!
//! The debug panel in the sidebar shows variables, watch expressions, the call stack and all
//! breakpoints. Building and the adapter run on other threads; results arrive through a
//! channel and a wake-up of the event loop.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyvern_dap::{find_netcoredbg, json, Client, Incoming, Value};
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

/// Debug commands (F5, F9, F10, F11 and the leader group Alt → d).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCmd {
    /// F5: start, or continue when paused.
    StartOrContinue,
    /// Shift+F5.
    Stop,
    /// F10.
    StepOver,
    /// F11.
    StepInto,
    /// Shift+F11.
    StepOut,
    /// Ctrl+F5.
    RunWithoutDebugging,
    /// F9.
    ToggleBreakpoint,
    ShowPanel,
    AddWatch,
    Attach,
}

/// Breakpoints by file: 0-based lines.
pub type Breakpoints = BTreeMap<PathBuf, Vec<usize>>;

const HOOK_PROJECT: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>netstandard2.1</TargetFramework>
    <AssemblyName>WyvernDebugHook</AssemblyName>
    <Nullable>disable</Nullable>
  </PropertyGroup>
</Project>
"#;

const HOOK_SOURCE: &str = r#"// Loaded by the .NET runtime (DOTNET_STARTUP_HOOKS) before Main. Wyvern starts the program
// in its own console window. When debugging (WYVERN_DEBUG_WAIT=1) wait for the debugger to
// attach (at most 30 s); always keep the window open when the program ends.
using System;
using System.Diagnostics;
using System.Threading;

internal class StartupHook
{
    public static void Initialize()
    {
        var watch = Stopwatch.StartNew();
        var wait = Environment.GetEnvironmentVariable("WYVERN_DEBUG_WAIT") == "1";
        while (wait && !Debugger.IsAttached && watch.ElapsedMilliseconds < 30000)
        {
            Thread.Sleep(20);
        }
        AppDomain.CurrentDomain.ProcessExit += (s, e) =>
        {
            try
            {
                Console.WriteLine();
                Console.Write("[Wyvern] Program ended. Press any key to close this window.");
                Console.ReadKey(true);
            }
            catch { }
        };
    }
}
"#;

/// What to start: the built program and where.
struct Launch {
    dll: PathBuf,
    project_dir: PathBuf,
    hook: PathBuf,
}

enum Msg {
    Built(Result<Launch, String>),
    Dap(Incoming),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Building,
    Starting,
    Running,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// 0-based.
    pub line: usize,
}

#[derive(Debug, Clone)]
struct Var {
    name: String,
    value: String,
    reference: i64,
    depth: usize,
    expanded: bool,
}

#[derive(Debug, Clone)]
struct Watch {
    expr: String,
    value: String,
}

/// What a pending request's response is for.
enum Pending {
    StackTrace,
    Scopes,
    /// Children go after `after` (index in `vars`), or replace the list if None.
    Variables { after: Option<usize>, depth: usize },
    Watch(usize),
    Hover { x: f32, y: f32, expr: String },
    Ignore,
}

/// What the app should do after debugger events.
pub enum DebugAction {
    /// Show the current line (0-based) of the stopped program.
    ShowLocation(PathBuf, usize),
    ClearLocation,
    Message(String),
    Hover { text: String, x: f32, y: f32 },
    /// The session ended.
    Ended,
}

/// Rows of the panel.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Header(String),
    Info(String),
    Var(usize),
    Watch(usize),
    Frame(usize),
    Breakpoint(PathBuf, usize),
}

/// What the app should do after input in the panel.
pub enum PanelAction {
    None,
    Close,
    Jump(PathBuf, usize),
    RemoveBreakpoint(PathBuf, usize),
}

pub struct Debugger {
    pub phase: Phase,
    client: Option<Client>,
    child: Option<Child>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    pending: HashMap<i64, Pending>,
    thread: i64,
    pub frames: Vec<Frame>,
    frame: usize,
    vars: Vec<Var>,
    watches: Vec<Watch>,
    /// Breakpoints to send once the adapter is ready (and kept in sync after that).
    breakpoints: Breakpoints,
    configured: bool,
    stop_reason: String,
    exit_code: Option<i64>,
    /// Process the adapter attaches to (the program started by F5, or one picked by the user).
    attach_pid: Option<u32>,
    /// Ctrl+F5: build and start, but without the debugger.
    run_only: bool,
    // Panel.
    area: Rect,
    font: FontMetrics,
    family: String,
    buffer: Buffer,
    rows: Vec<Row>,
    selected: usize,
    scroll: usize,
    list_y: f32,
}

impl Debugger {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        buffer.set_wrap(Wrap::None);
        let (tx, rx) = mpsc::channel();
        Self {
            phase: Phase::Idle,
            client: None,
            child: None,
            tx,
            rx,
            wake: None,
            pending: HashMap::new(),
            thread: 0,
            frames: Vec::new(),
            frame: 0,
            vars: Vec::new(),
            watches: Vec::new(),
            breakpoints: Breakpoints::new(),
            configured: false,
            stop_reason: String::new(),
            exit_code: None,
            attach_pid: None,
            run_only: false,
            area: Rect::default(),
            font,
            family,
            buffer,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            list_y: 0.0,
        }
    }

    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    pub fn active(&self) -> bool {
        self.phase != Phase::Idle
    }

    pub fn status_text(&self) -> String {
        match self.phase {
            Phase::Idle => match self.exit_code {
                Some(c) => format!("not running (last exit code {c})"),
                None => "not running".into(),
            },
            Phase::Building => "building …".into(),
            Phase::Starting => "starting …".into(),
            Phase::Running => "running".into(),
            Phase::Stopped => {
                let at = self.frames.first().map_or(String::new(), |f| format!(" in {}", f.name));
                format!("paused ({}){at}", self.stop_reason)
            }
        }
    }

    // ----- session -----

    /// F5: builds `project` and starts it under the debugger.
    pub fn start(&mut self, project: PathBuf, breakpoints: Breakpoints) {
        self.build_and_run(project, breakpoints, false);
    }

    /// Ctrl+F5: builds `project` and starts it in its own window, without the debugger.
    pub fn run_without_debugging(&mut self, project: PathBuf) {
        self.build_and_run(project, Breakpoints::new(), true);
    }

    fn build_and_run(&mut self, project: PathBuf, breakpoints: Breakpoints, run_only: bool) {
        if self.active() {
            return;
        }
        self.run_only = run_only;
        self.breakpoints = breakpoints;
        self.phase = Phase::Building;
        self.exit_code = None;
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        std::thread::Builder::new()
            .name("debug-build".into())
            .spawn(move || {
                let result = build(&project);
                let _ = tx.send(Msg::Built(result));
                if let Some(w) = wake {
                    w();
                }
            })
            .ok();
    }

    /// Alt → d → a: attaches to a running .NET process (e.g. a Godot game).
    pub fn attach(&mut self, pid: u32, breakpoints: Breakpoints) -> Result<(), String> {
        if self.active() {
            return Err("A debug session is already running".into());
        }
        self.breakpoints = breakpoints;
        self.exit_code = None;
        self.spawn_adapter(Path::new("."))?;
        self.phase = Phase::Starting;
        self.attach_pid = Some(pid);
        Ok(())
    }

    fn spawn_adapter(&mut self, cwd: &Path) -> Result<(), String> {
        let exe = find_netcoredbg().map_err(|searched| {
            let list: Vec<String> = searched.iter().map(|p| p.display().to_string()).collect();
            format!("netcoredbg not found (searched: {})", list.join(", "))
        })?;
        let (tx, wake) = (self.tx.clone(), self.wake.clone());
        let client = Client::spawn(&exe, &["--interpreter=vscode"], cwd, move |m| {
            let _ = tx.send(Msg::Dap(m));
            if let Some(w) = &wake {
                w();
            }
        })
        .map_err(|e| format!("netcoredbg does not start: {e}"))?;
        self.client = Some(client);
        self.configured = false;
        self.request(
            "initialize",
            json!({
                "clientID": "wyvern", "clientName": "Wyvern", "adapterID": "coreclr",
                "linesStartAt1": true, "columnsStartAt1": true, "pathFormat": "path",
            }),
            Pending::Ignore,
        );
        Ok(())
    }

    fn request(&mut self, command: &str, args: Value, pending: Pending) {
        if let Some(c) = &self.client {
            let seq = c.request(command, args);
            self.pending.insert(seq, pending);
        }
    }

    /// Shift+F5.
    pub fn stop(&mut self) {
        if self.client.is_some() {
            self.request("disconnect", json!({ "terminateDebuggee": true }), Pending::Ignore);
        }
        self.end();
    }

    fn end(&mut self) {
        if let Some(mut c) = self.client.take() {
            // Give the adapter a moment to detach cleanly, then make sure it is gone.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                c.kill();
            });
        }
        if let Some(mut child) = self.child.take()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
        }
        self.phase = Phase::Idle;
        self.pending.clear();
        self.frames.clear();
        self.vars.clear();
        self.attach_pid = None;
        for w in &mut self.watches {
            w.value.clear();
        }
    }

    pub fn continue_(&mut self) {
        if self.phase == Phase::Stopped {
            let t = self.thread;
            self.request("continue", json!({ "threadId": t }), Pending::Ignore);
        }
    }

    pub fn step(&mut self, command: &str) {
        if self.phase == Phase::Stopped {
            let t = self.thread;
            self.request(command, json!({ "threadId": t }), Pending::Ignore);
        }
    }

    /// Breakpoints of one file changed (F9, click, edits).
    pub fn set_breakpoints(&mut self, path: &Path, lines: &[usize]) {
        if lines.is_empty() {
            self.breakpoints.remove(path);
        } else {
            self.breakpoints.insert(path.to_path_buf(), lines.to_vec());
        }
        if self.configured {
            self.send_breakpoints(path, lines);
        }
    }

    fn send_breakpoints(&mut self, path: &Path, lines: &[usize]) {
        let bps: Vec<Value> = lines.iter().map(|l| json!({ "line": l + 1 })).collect();
        self.request(
            "setBreakpoints",
            json!({ "source": { "path": windows_path(path) }, "breakpoints": bps }),
            Pending::Ignore,
        );
    }

    pub fn add_watch(&mut self, expr: String) {
        self.watches.push(Watch { expr, value: String::new() });
        if self.phase == Phase::Stopped {
            let i = self.watches.len() - 1;
            self.evaluate_watch(i);
        }
    }

    fn evaluate_watch(&mut self, i: usize) {
        let Some(frame) = self.frames.get(self.frame).map(|f| f.id) else { return };
        let expr = self.watches[i].expr.clone();
        self.request("evaluate", json!({ "expression": expr, "frameId": frame, "context": "watch" }), Pending::Watch(i));
    }

    /// Value of an expression under the mouse while paused.
    pub fn hover(&mut self, expr: String, x: f32, y: f32) {
        let Some(frame) = self.frames.get(self.frame).map(|f| f.id) else { return };
        self.request(
            "evaluate",
            json!({ "expression": expr, "frameId": frame, "context": "hover" }),
            Pending::Hover { x, y, expr },
        );
    }

    fn select_frame(&mut self, i: usize) -> Option<DebugAction> {
        let f = self.frames.get(i)?.clone();
        self.frame = i;
        self.vars.clear();
        self.request("scopes", json!({ "frameId": f.id }), Pending::Scopes);
        for w in 0..self.watches.len() {
            self.evaluate_watch(w);
        }
        f.path.map(|p| DebugAction::ShowLocation(p, f.line))
    }

    /// Handles everything that arrived from the build thread and the adapter.
    pub fn poll(&mut self) -> Vec<DebugAction> {
        let mut out = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Built(Err(e)) => {
                    self.phase = Phase::Idle;
                    out.push(DebugAction::Message(e));
                }
                Msg::Built(Ok(launch)) => {
                    if self.phase != Phase::Building {
                        continue; // stopped while building
                    }
                    if self.run_only {
                        self.phase = Phase::Idle;
                        let name = launch.dll.file_stem().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                        match spawn_program(&launch, false) {
                            Ok(_) => out.push(DebugAction::Message(format!("{name} started without the debugger"))),
                            Err(e) => out.push(DebugAction::Message(e)),
                        }
                        continue;
                    }
                    match self.launch(&launch) {
                        Ok(()) => self.phase = Phase::Starting,
                        Err(e) => {
                            self.end();
                            out.push(DebugAction::Message(e));
                        }
                    }
                }
                Msg::Dap(incoming) => self.handle(incoming, &mut out),
            }
        }
        out
    }

    fn launch(&mut self, l: &Launch) -> Result<(), String> {
        let child = spawn_program(l, true)?;
        self.attach_pid = Some(child.id());
        self.child = Some(child);
        self.spawn_adapter(&l.project_dir)
    }

    fn handle(&mut self, incoming: Incoming, out: &mut Vec<DebugAction>) {
        match incoming {
            Incoming::Response { request_seq, command, result } => {
                let pending = self.pending.remove(&request_seq).unwrap_or(Pending::Ignore);
                match (command.as_str(), result) {
                    ("initialize", Ok(_)) => {
                        let pid = self.attach_pid.unwrap_or(0);
                        self.request("attach", json!({ "processId": pid }), Pending::Ignore);
                    }
                    ("attach", Err(e)) => {
                        out.push(DebugAction::Message(format!("Attach failed: {e}")));
                        self.end();
                        out.push(DebugAction::Ended);
                    }
                    (_, Ok(body)) => self.response(pending, body, out),
                    (_, Err(e)) => match pending {
                        Pending::Watch(i) => {
                            if let Some(w) = self.watches.get_mut(i) {
                                w.value = format!("⚠ {e}");
                            }
                        }
                        Pending::Hover { .. } | Pending::Ignore => log::debug!("dap {command} failed: {e}"),
                        _ => out.push(DebugAction::Message(format!("Debugger: {command} failed: {e}"))),
                    },
                }
            }
            Incoming::Event { event, body } => match event.as_str() {
                "initialized" => {
                    let all: Vec<(PathBuf, Vec<usize>)> =
                        self.breakpoints.iter().map(|(p, l)| (p.clone(), l.clone())).collect();
                    for (path, lines) in all {
                        self.send_breakpoints(&path, &lines);
                    }
                    self.request(
                        "setExceptionBreakpoints",
                        json!({ "filters": ["user-unhandled"] }),
                        Pending::Ignore,
                    );
                    self.request("configurationDone", json!({}), Pending::Ignore);
                    self.configured = true;
                    self.phase = Phase::Running;
                }
                "stopped" => {
                    self.phase = Phase::Stopped;
                    self.thread = body["threadId"].as_i64().unwrap_or(self.thread);
                    self.stop_reason = body["reason"].as_str().unwrap_or("paused").to_string();
                    if let Some(text) = body["text"].as_str().or(body["description"].as_str())
                        && self.stop_reason == "exception"
                    {
                        out.push(DebugAction::Message(format!("Exception: {text}")));
                    }
                    let t = self.thread;
                    self.request("stackTrace", json!({ "threadId": t, "startFrame": 0, "levels": 50 }), Pending::StackTrace);
                }
                "continued" => {
                    self.phase = Phase::Running;
                    self.frames.clear();
                    self.vars.clear();
                    out.push(DebugAction::ClearLocation);
                }
                "exited" => self.exit_code = body["exitCode"].as_i64(),
                "terminated" => {
                    let code = self.exit_code.map_or(String::new(), |c| format!(" with code {c}"));
                    self.end();
                    out.push(DebugAction::ClearLocation);
                    out.push(DebugAction::Message(format!("Program ended{code}")));
                    out.push(DebugAction::Ended);
                }
                "output" => {
                    // The program writes to its own window; only the adapter's notes come here.
                    if body["category"] == "console"
                        && let Some(text) = body["output"].as_str()
                    {
                        log::info!("netcoredbg: {}", text.trim());
                    }
                }
                _ => {}
            },
            Incoming::Request { seq, command, .. } => {
                if let Some(c) = &self.client {
                    c.respond(seq, &command, false, json!({}));
                }
            }
            Incoming::Exited => {
                if self.active() {
                    self.end();
                    out.push(DebugAction::ClearLocation);
                    out.push(DebugAction::Message("Debugger stopped".into()));
                    out.push(DebugAction::Ended);
                }
            }
        }
    }

    fn response(&mut self, pending: Pending, body: Value, out: &mut Vec<DebugAction>) {
        match pending {
            Pending::StackTrace => {
                self.frames = body["stackFrames"]
                    .as_array()
                    .map(|a| a.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .map(|f| Frame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("?").to_string(),
                        path: f["source"]["path"].as_str().map(PathBuf::from),
                        line: (f["line"].as_u64().unwrap_or(1) as usize).saturating_sub(1),
                    })
                    .collect();
                // Show the first frame with source code (skips framework frames).
                let first = self.frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
                if let Some(a) = self.select_frame(first) {
                    out.push(a);
                }
            }
            Pending::Scopes => {
                if let Some(r) = body["scopes"][0]["variablesReference"].as_i64() {
                    self.request("variables", json!({ "variablesReference": r }), Pending::Variables { after: None, depth: 0 });
                }
            }
            Pending::Variables { after, depth } => {
                let vars: Vec<Var> = body["variables"]
                    .as_array()
                    .map(|a| a.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .map(|v| Var {
                        name: v["name"].as_str().unwrap_or("?").to_string(),
                        value: v["value"].as_str().unwrap_or("").to_string(),
                        reference: v["variablesReference"].as_i64().unwrap_or(0),
                        depth,
                        expanded: false,
                    })
                    .collect();
                match after {
                    None => self.vars = vars,
                    Some(i) if i < self.vars.len() => {
                        self.vars[i].expanded = true;
                        let tail = self.vars.split_off(i + 1);
                        self.vars.extend(vars);
                        self.vars.extend(tail);
                    }
                    Some(_) => {}
                }
            }
            Pending::Watch(i) => {
                if let Some(w) = self.watches.get_mut(i) {
                    w.value = body["result"].as_str().unwrap_or("").to_string();
                }
            }
            Pending::Hover { x, y, expr } => {
                let value = body["result"].as_str().unwrap_or("");
                let ty = body["type"].as_str().map_or(String::new(), |t| format!("  ({t})"));
                out.push(DebugAction::Hover { text: format!("{expr} = {value}{ty}"), x, y });
            }
            Pending::Ignore => {}
        }
    }

    fn toggle_var(&mut self, i: usize) {
        let Some(v) = self.vars.get(i).cloned() else { return };
        if v.reference <= 0 {
            return;
        }
        if v.expanded {
            let end = self.vars[i + 1..].iter().position(|c| c.depth <= v.depth).map_or(self.vars.len(), |p| i + 1 + p);
            self.vars.drain(i + 1..end);
            self.vars[i].expanded = false;
        } else {
            self.request(
                "variables",
                json!({ "variablesReference": v.reference }),
                Pending::Variables { after: Some(i), depth: v.depth + 1 },
            );
        }
    }

    // ----- panel -----

    pub fn set_font(&mut self, font: FontMetrics) {
        self.font = font;
        self.buffer.set_metrics(Metrics::new(font.size, font.line_h));
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    fn build_rows(&mut self, breakpoints: &Breakpoints) {
        let mut rows = vec![Row::Header("VARIABLES".into())];
        if self.vars.is_empty() {
            rows.push(Row::Info(if self.phase == Phase::Stopped { "(none)" } else { "(only while paused)" }.into()));
        }
        rows.extend((0..self.vars.len()).map(Row::Var));
        rows.push(Row::Header("WATCH".into()));
        if self.watches.is_empty() {
            rows.push(Row::Info("Alt → d → w adds an expression".into()));
        }
        rows.extend((0..self.watches.len()).map(Row::Watch));
        rows.push(Row::Header("CALL STACK".into()));
        if self.frames.is_empty() {
            rows.push(Row::Info("(only while paused)".into()));
        }
        rows.extend((0..self.frames.len()).map(Row::Frame));
        rows.push(Row::Header("BREAKPOINTS".into()));
        if breakpoints.values().all(Vec::is_empty) {
            rows.push(Row::Info("F9 or a click on a line number sets one".into()));
        }
        for (path, lines) in breakpoints {
            for &l in lines {
                rows.push(Row::Breakpoint(path.clone(), l));
            }
        }
        self.rows = rows;
        let selectable = |r: &Row| !matches!(r, Row::Header(_) | Row::Info(_));
        if !self.rows.get(self.selected).is_some_and(selectable) {
            self.selected = self.rows.iter().position(selectable).unwrap_or(0);
        }
    }

    fn visible_rows(&self) -> usize {
        (((self.area.y + self.area.h - self.list_y) / self.font.line_h).floor() as usize).max(1)
    }

    fn move_selection(&mut self, delta: isize) {
        let selectable: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !matches!(r, Row::Header(_) | Row::Info(_)))
            .map(|(i, _)| i)
            .collect();
        if selectable.is_empty() {
            return;
        }
        let cur = selectable.iter().position(|&i| i == self.selected).unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, selectable.len() as isize - 1) as usize;
        self.selected = selectable[next];
        let vis = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + vis {
            self.scroll = self.selected + 1 - vis;
        }
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.rows.len().saturating_sub(self.visible_rows()) as isize;
        self.scroll = (self.scroll as isize + lines).clamp(0, max.max(0)) as usize;
    }

    fn activate(&mut self, row: usize) -> PanelAction {
        match self.rows.get(row).cloned() {
            Some(Row::Var(i)) => {
                self.toggle_var(i);
                PanelAction::None
            }
            Some(Row::Frame(i)) => match self.select_frame(i) {
                Some(DebugAction::ShowLocation(p, l)) => PanelAction::Jump(p, l),
                _ => PanelAction::None,
            },
            Some(Row::Breakpoint(p, l)) => PanelAction::Jump(p, l),
            _ => PanelAction::None,
        }
    }

    pub fn key(&mut self, event: &KeyEvent) -> PanelAction {
        match event.key_without_modifiers() {
            Key::Named(NamedKey::Escape) => PanelAction::Close,
            Key::Named(NamedKey::ArrowDown) => {
                self.move_selection(1);
                PanelAction::None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_selection(-1);
                PanelAction::None
            }
            Key::Named(NamedKey::Enter | NamedKey::Space) => self.activate(self.selected),
            Key::Named(NamedKey::ArrowRight) => match self.rows.get(self.selected) {
                Some(&Row::Var(i)) if !self.vars[i].expanded => self.activate(self.selected),
                _ => PanelAction::None,
            },
            Key::Named(NamedKey::ArrowLeft) => match self.rows.get(self.selected) {
                Some(&Row::Var(i)) if self.vars[i].expanded => self.activate(self.selected),
                _ => PanelAction::None,
            },
            Key::Named(NamedKey::Delete | NamedKey::Backspace) => match self.rows.get(self.selected).cloned() {
                Some(Row::Watch(i)) => {
                    self.watches.remove(i);
                    PanelAction::None
                }
                Some(Row::Breakpoint(p, l)) => PanelAction::RemoveBreakpoint(p, l),
                _ => PanelAction::None,
            },
            _ => PanelAction::None,
        }
    }

    pub fn click(&mut self, y: f32) -> PanelAction {
        if y < self.list_y {
            return PanelAction::None;
        }
        let row = self.scroll + ((y - self.list_y) / self.font.line_h) as usize;
        if matches!(self.rows.get(row), None | Some(Row::Header(_)) | Some(Row::Info(_))) {
            return PanelAction::None;
        }
        self.selected = row;
        self.activate(row)
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool, breakpoints: &Breakpoints) {
        self.build_rows(breakpoints);
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let status_color = match self.phase {
            Phase::Stopped => theme.accent,
            Phase::Idle => theme.panel_dim,
            _ => theme.git_added,
        };
        let mut spans: Vec<(String, Rgba)> = vec![
            ("DEBUG  ·  ".into(), theme.panel_dim),
            (format!("{}\n", self.status_text()), status_color),
            ("F5 start/continue · F10 over · F11 into\nShift+F11 out · Shift+F5 stop · F9 breakpoint\n".into(), theme.panel_dim),
        ];
        let header_lines = 3;
        let vis = self.visible_rows();
        for (i, row) in self.rows.iter().enumerate().skip(self.scroll).take(vis) {
            let (text, color) = match row {
                Row::Header(h) => (h.clone(), theme.panel_dim),
                Row::Info(t) => (format!("  {t}"), theme.panel_dim),
                Row::Var(v) => {
                    let v = &self.vars[*v];
                    let arrow = if v.reference > 0 { if v.expanded { "▾ " } else { "▸ " } } else { "  " };
                    (format!("{}{arrow}{} = {}", "  ".repeat(v.depth + 1), v.name, v.value), theme.panel_text)
                }
                Row::Watch(w) => {
                    let w = &self.watches[*w];
                    let value = if w.value.is_empty() { "…" } else { &w.value };
                    (format!("  {} = {value}", w.expr), theme.panel_text)
                }
                Row::Frame(f) => {
                    let fr = &self.frames[*f];
                    let marker = if *f == self.frame { "➜ " } else { "  " };
                    let place = fr.path.as_ref().map_or(String::new(), |p| {
                        format!("  {}:{}", p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()), fr.line + 1)
                    });
                    (format!("{marker}{}{place}", fr.name), if fr.path.is_some() { theme.panel_text } else { theme.panel_dim })
                }
                Row::Breakpoint(p, l) => {
                    let name = p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    (format!("  ● {name}:{}", l + 1), theme.panel_text)
                }
            };
            let color = if i == self.selected && focused { theme.tab_text_active } else { color };
            spans.push((format!("{text}\n"), color));
        }
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
        let top_y = self.area.y + self.font.line_h * 0.2;
        self.list_y = top_y + header_lines as f32 * self.font.line_h;
    }

    pub fn draw<'a>(&'a self, theme: &Theme, focused: bool, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let lh = self.font.line_h;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        if self.selected >= self.scroll && self.selected < self.scroll + self.visible_rows() {
            let y = self.list_y + (self.selected - self.scroll) as f32 * lh;
            let color = if focused { theme.row_selected } else { theme.row_selected_unfocused };
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color });
        }
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        text.push(TextBlock {
            buffer: &self.buffer,
            x: a.x + self.font.cell_w,
            y: a.y + lh * 0.2,
            color: theme.panel_text,
            clip,
        });
    }
}

/// Starts the built program with `dotnet` in its own console window.
fn spawn_program(l: &Launch, wait_for_debugger: bool) -> Result<Child, String> {
    let mut cmd = Command::new("dotnet");
    cmd.arg(&l.dll).current_dir(&l.project_dir).env("DOTNET_STARTUP_HOOKS", &l.hook);
    if wait_for_debugger {
        cmd.env("WYVERN_DEBUG_WAIT", "1");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        cmd.creation_flags(CREATE_NEW_CONSOLE);
    }
    cmd.spawn().map_err(|e| format!("Program does not start: {e}"))
}

/// netcoredbg matches source paths exactly as stored in the PDB: Windows paths with `\`.
fn windows_path(path: &Path) -> String {
    let s = path.display().to_string();
    if cfg!(windows) { s.replace('/', "\\") } else { s }
}

fn dotnet(args: &[&str], cwd: &Path) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("dotnet");
    cmd.args(args).current_dir(cwd).env("DOTNET_CLI_UI_LANGUAGE", "en").env("DOTNET_NOLOGO", "1");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().map_err(|e| format!("dotnet cannot be started: {e}"))
}

/// Builds the startup hook once into the config folder.
fn ensure_hook() -> Result<PathBuf, String> {
    let dir = crate::config::config_dir().ok_or("no config folder")?.join("debughook");
    let dll = dir.join("bin").join("Debug").join("netstandard2.1").join("WyvernDebugHook.dll");
    let source = dir.join("StartupHook.cs");
    let current = std::fs::read_to_string(&source).ok();
    if dll.is_file() && current.as_deref() == Some(HOOK_SOURCE) {
        return Ok(dll);
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("WyvernDebugHook.csproj"), HOOK_PROJECT).map_err(|e| e.to_string())?;
    std::fs::write(&source, HOOK_SOURCE).map_err(|e| e.to_string())?;
    let out = dotnet(&["build", "-c", "Debug", "-v", "q"], &dir)?;
    if !out.status.success() || !dll.is_file() {
        return Err(format!("Debug helper does not build: {}", first_error(&out)));
    }
    Ok(dll)
}

/// The first `error` line of a build, or the last output line.
fn first_error(out: &std::process::Output) -> String {
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    text.lines()
        .find(|l| l.contains(": error "))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .unwrap_or("unknown error")
        .trim()
        .to_string()
}

fn build(project: &Path) -> Result<Launch, String> {
    let project_dir = project.parent().ok_or("project has no folder")?.to_path_buf();
    let hook = ensure_hook()?;
    let p = project.display().to_string();
    let out = dotnet(&["build", &p, "-v", "q", "-clp:NoSummary"], &project_dir)?;
    if !out.status.success() {
        return Err(format!("Build failed: {}", first_error(&out)));
    }
    let out = dotnet(&["msbuild", &p, "-getProperty:TargetPath"], &project_dir)?;
    let dll = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if !dll.is_file() {
        return Err(format!("Built program not found ({})", dll.display()));
    }
    Ok(Launch { dll, project_dir, hook })
}

/// Projects that can be started (`OutputType` Exe/WinExe or a web SDK), the one containing
/// `near` first.
pub fn runnable_projects(root: &Path, near: Option<&Path>) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for e in read.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                let ignored = crate::explorer::IGNORED.iter().any(|i| i.eq_ignore_ascii_case(&name));
                if depth > 0 && !ignored && !name.starts_with('.') {
                    walk(&p, depth - 1, out);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("csproj")) {
                out.push(p);
            }
        }
    }
    let mut all = Vec::new();
    walk(root, 4, &mut all);
    let mut out: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| {
            let text = std::fs::read_to_string(p).unwrap_or_default();
            text.contains("<OutputType>Exe") || text.contains("<OutputType>WinExe") || text.contains("Microsoft.NET.Sdk.Web")
        })
        .collect();
    out.sort();
    if let Some(near) = near {
        // The deepest project folder that contains the file wins.
        out.sort_by_key(|p| {
            let dir = p.parent().unwrap_or(p);
            if near.starts_with(dir) { std::cmp::Reverse(dir.components().count()) } else { std::cmp::Reverse(0) }
        });
    }
    out
}

/// Running processes that may be .NET programs: (name, pid).
pub fn dotnet_processes() -> Vec<(String, u32)> {
    let mut cmd = Command::new("tasklist");
    cmd.args(["/fo", "csv", "/nh"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let Ok(out) = cmd.output() else { return Vec::new() };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut list: Vec<(String, u32)> = text
        .lines()
        .filter_map(|l| {
            let mut fields = l.split("\",\"").map(|f| f.trim_matches('"'));
            let name = fields.next()?.to_string();
            let pid = fields.next()?.parse().ok()?;
            Some((name, pid))
        })
        .filter(|(n, _)| {
            let n = n.to_lowercase();
            n.starts_with("dotnet") || n.starts_with("godot")
        })
        .collect();
    list.sort();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runnable_projects_prefer_the_one_with_the_file() {
        let dir = std::env::temp_dir().join(format!("wyvern-debug-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (sub, kind) in [("App", "Exe"), ("Tool", "Exe"), ("Lib", "Library")] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
            std::fs::write(
                dir.join(sub).join(format!("{sub}.csproj")),
                format!("<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>{kind}</OutputType></PropertyGroup></Project>"),
            )
            .unwrap();
        }
        let found = runnable_projects(&dir, Some(&dir.join("Tool").join("Program.cs")));
        let names: Vec<String> = found.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["Tool.csproj", "App.csproj"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A whole session against the real tools: `cargo test -p wyvern-app -- --ignored debug_session`.
    #[test]
    #[ignore = "needs dotnet and netcoredbg; opens a console window"]
    fn debug_session_stops_steps_and_stops() {
        let dir = std::env::temp_dir().join(format!("wyvern-debug-session-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project = dir.join("App.csproj");
        std::fs::write(
            &project,
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>",
        )
        .unwrap();
        let source = dir.join("Program.cs");
        std::fs::write(
            &source,
            "class P
{
    static void Main()
    {
        int a = 2;
        int b = a * 21;
        System.Console.WriteLine(b);
    }
}
",
        )
        .unwrap();
        let mut fs = FontSystem::new();
        let font = FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let mut d = Debugger::new(&mut fs, font, "monospace".into());
        d.set_wake(|| {});
        let mut bps = Breakpoints::new();
        bps.insert(source.clone(), vec![5]); // int b = a * 21;
        d.start(project, bps);
        let wait_for = |d: &mut Debugger, want: &dyn Fn(&DebugAction) -> bool| -> bool {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(90);
            while std::time::Instant::now() < until {
                for a in d.poll() {
                    if let DebugAction::Message(m) = &a {
                        eprintln!("message: {m}");
                    }
                    if want(&a) {
                        return true;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        };
        assert!(wait_for(&mut d, &|a| matches!(a, DebugAction::ShowLocation(_, 5))), "no stop at line 6");
        // Locals arrive right after the stop.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while d.vars.is_empty() && std::time::Instant::now() < until {
            d.poll();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let a = d.vars.iter().find(|v| v.name == "a").map(|v| v.value.clone());
        assert_eq!(a.as_deref(), Some("2"));
        d.step("next");
        assert!(wait_for(&mut d, &|a| matches!(a, DebugAction::ShowLocation(_, 6))), "step did not reach line 7");
        d.stop();
        assert_eq!(d.phase, Phase::Idle);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_use_backslashes_on_windows() {
        if cfg!(windows) {
            assert_eq!(windows_path(Path::new("C:/a/b.cs")), "C:\\a\\b.cs");
        }
    }
}
