//! Build, test, run and clean the project, like Visual Studio's Build menu: finds the tool
//! (`dotnet` for .sln/.csproj, `cargo` for Cargo.toml, `npm` for package.json), runs it in the
//! background and reads its output line by line. Errors and warnings in the output (MSBuild,
//! TypeScript and Rust formats) become entries of the Problems panel.
//!
//! "Run" starts in a terminal instead (programs may want input).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;

use crate::problems::Problem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    Build,
    Test,
    Run,
    Clean,
}

impl TaskKind {
    pub fn verb(self) -> &'static str {
        match self {
            TaskKind::Build => "Build",
            TaskKind::Test => "Test",
            TaskKind::Run => "Run",
            TaskKind::Clean => "Clean",
        }
    }

    fn running(self) -> &'static str {
        match self {
            TaskKind::Build => "Building",
            TaskKind::Test => "Testing",
            TaskKind::Run => "Running",
            TaskKind::Clean => "Cleaning",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Dotnet,
    Cargo,
    Npm,
}

/// The build tool of a project and the file or folder it works on.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub kind: ToolKind,
    /// The .sln/.csproj file, or the folder with Cargo.toml / package.json.
    pub target: PathBuf,
}

impl Tool {
    /// The folder relative paths in the output refer to.
    pub fn dir(&self) -> &Path {
        if self.target.is_file() { self.target.parent().unwrap_or(&self.target) } else { &self.target }
    }

    /// Program and arguments for a task (`None`: the tool has no such task).
    pub fn command(&self, task: TaskKind) -> Option<(String, Vec<String>)> {
        let t = self.target.to_string_lossy().into_owned();
        let args: Vec<&str> = match (self.kind, task) {
            (ToolKind::Dotnet, TaskKind::Build) => vec!["build", &t, "-nologo"],
            (ToolKind::Dotnet, TaskKind::Test) => vec!["test", &t, "-nologo"],
            (ToolKind::Dotnet, TaskKind::Clean) => vec!["clean", &t, "-nologo"],
            (ToolKind::Dotnet, TaskKind::Run) => {
                if t.ends_with(".sln") {
                    vec!["run"]
                } else {
                    vec!["run", "--project", &t]
                }
            }
            (ToolKind::Cargo, TaskKind::Build) => vec!["build", "--message-format=short"],
            (ToolKind::Cargo, TaskKind::Test) => vec!["test", "--message-format=short"],
            (ToolKind::Cargo, TaskKind::Run) => vec!["run"],
            (ToolKind::Cargo, TaskKind::Clean) => vec!["clean"],
            (ToolKind::Npm, TaskKind::Build) => vec!["run", "build"],
            (ToolKind::Npm, TaskKind::Test) => vec!["test"],
            (ToolKind::Npm, TaskKind::Run) => vec!["start"],
            (ToolKind::Npm, TaskKind::Clean) => return None,
        };
        let program = match self.kind {
            ToolKind::Dotnet => "dotnet",
            ToolKind::Cargo => "cargo",
            ToolKind::Npm => {
                if cfg!(windows) {
                    "npm.cmd"
                } else {
                    "npm"
                }
            }
        };
        Some((program.to_string(), args.into_iter().map(str::to_string).collect()))
    }

    /// The command as one line for a terminal (for Run), starting in the right folder.
    pub fn terminal_line(&self, task: TaskKind) -> Option<String> {
        let (program, args) = self.command(task)?;
        let program = program.trim_end_matches(".cmd").to_string();
        let quoted: Vec<String> = args.iter().map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect();
        let dir = self.dir().to_string_lossy().into_owned();
        Some(format!("cd \"{dir}\"; {program} {}", quoted.join(" ")))
    }
}

/// Finds the build tool for a project folder (also one folder level deeper).
pub fn detect(root: &Path) -> Option<Tool> {
    fn in_dir(dir: &Path) -> Option<Tool> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort();
        let ext = |p: &PathBuf, e: &str| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(e));
        if let Some(sln) = files.iter().find(|p| ext(p, "sln") || ext(p, "slnx")) {
            return Some(Tool { kind: ToolKind::Dotnet, target: sln.clone() });
        }
        if let Some(proj) = files.iter().find(|p| ext(p, "csproj") || ext(p, "fsproj") || ext(p, "vbproj")) {
            return Some(Tool { kind: ToolKind::Dotnet, target: proj.clone() });
        }
        let named = |n: &str| files.iter().any(|p| p.file_name().is_some_and(|f| f == n));
        if named("Cargo.toml") {
            return Some(Tool { kind: ToolKind::Cargo, target: dir.to_path_buf() });
        }
        if named("package.json") {
            return Some(Tool { kind: ToolKind::Npm, target: dir.to_path_buf() });
        }
        None
    }
    if let Some(t) = in_dir(root) {
        return Some(t);
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .collect();
    dirs.sort();
    dirs.iter().find_map(|d| in_dir(d))
}

/// What the background task reports.
pub enum TaskMsg {
    Line(String),
    /// Finished: exit code (None: killed or did not start) and the reason it did not start.
    Done(Option<i32>, Option<String>),
}

/// Runs one task at a time in the background.
pub struct TaskRunner {
    tx: Sender<TaskMsg>,
    rx: Receiver<TaskMsg>,
    child: Option<Arc<Mutex<Child>>>,
    /// The running task, its tool and when it started.
    pub running: Option<(TaskKind, Tool, Instant)>,
    /// The last finished task: kind, success, duration.
    pub last: Option<(TaskKind, bool, Duration)>,
}

impl Default for TaskRunner {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, child: None, running: None, last: None }
    }
}

impl TaskRunner {
    pub fn status(&self) -> Option<String> {
        self.running.as_ref().map(|(k, _, t)| format!("{} … {:.0} s", k.running(), t.elapsed().as_secs_f32()))
    }

    /// Starts a task; `wake` is called when output arrives.
    pub fn start(&mut self, tool: &Tool, task: TaskKind, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        self.stop();
        let (program, args) = tool.command(task).ok_or_else(|| format!("{} is not available for this project", task.verb()))?;
        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .current_dir(tool.dir())
            .env("NO_COLOR", "1")
            .env("DOTNET_CLI_UI_LANGUAGE", "en")
            .env("CARGO_TERM_COLOR", "never")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().map_err(|e| format!("{program} does not start: {e}"))?;
        let streams: Vec<Box<dyn std::io::Read + Send>> = [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        let child = Arc::new(Mutex::new(child));
        let readers: Vec<_> = streams
            .into_iter()
            .map(|s| {
                let (tx, wake) = (self.tx.clone(), wake.clone());
                std::thread::spawn(move || {
                    for line in BufReader::new(s).split(b'\n').map_while(Result::ok) {
                        let line = String::from_utf8_lossy(&line).trim_end_matches('\r').to_string();
                        if tx.send(TaskMsg::Line(line)).is_err() {
                            return;
                        }
                        wake();
                    }
                })
            })
            .collect();
        let (tx, waiter) = (self.tx.clone(), child.clone());
        std::thread::spawn(move || {
            for r in readers {
                let _ = r.join();
            }
            let code = waiter.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
            let _ = tx.send(TaskMsg::Done(code, None));
            wake();
        });
        self.child = Some(child);
        self.running = Some((task, tool.clone(), Instant::now()));
        Ok(())
    }

    /// Ends the running task.
    pub fn stop(&mut self) {
        if let Some(c) = self.child.take()
            && let Ok(mut c) = c.lock()
        {
            let _ = c.kill();
        }
    }

    pub fn poll(&mut self) -> Vec<TaskMsg> {
        let msgs: Vec<TaskMsg> = self.rx.try_iter().collect();
        for m in &msgs {
            if let TaskMsg::Done(code, _) = m
                && let Some((kind, _, started)) = self.running.take()
            {
                self.child = None;
                self.last = Some((kind, *code == Some(0), started.elapsed()));
            }
        }
        msgs
    }
}

fn patterns() -> &'static [Regex; 3] {
    static P: std::sync::OnceLock<[Regex; 3]> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        [
            // MSBuild / C# / TypeScript (tsc --pretty false): file(line,col): error CODE: message [project]
            Regex::new(r"^\s*(?P<file>.+?)\((?P<line>\d+),(?P<col>\d+)(?:,\d+,\d+)?\)\s*:\s*(?P<sev>error|warning)\s*(?P<code>[A-Za-z]*\d+)?\s*:\s*(?P<msg>.*?)(?:\s+\[[^\]]+\])?\s*$").unwrap(),
            // Rust (--message-format=short): file:line:col: error[CODE]: message
            Regex::new(r"^(?P<file>(?:[A-Za-z]:)?[^:]+):(?P<line>\d+):(?P<col>\d+): (?P<sev>error|warning)(?:\[(?P<code>\w+)\])?: (?P<msg>.*)$").unwrap(),
            // TypeScript (pretty): file:line:col - error TS1234: message
            Regex::new(r"^(?P<file>(?:[A-Za-z]:)?[^:]+):(?P<line>\d+):(?P<col>\d+) - (?P<sev>error|warning) (?P<code>\w+): (?P<msg>.*)$").unwrap(),
        ]
    })
}

/// Removes terminal color codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// An error or warning in a line of build output; relative paths count from `dir`.
pub fn parse_line(line: &str, dir: &Path) -> Option<Problem> {
    let line = strip_ansi(line);
    let c = patterns().iter().find_map(|r| r.captures(&line))?;
    let file = c["file"].trim();
    let path = Path::new(file);
    let path = if path.is_absolute() { path.to_path_buf() } else { dir.join(path) };
    let code = c.name("code").map_or("", |m| m.as_str());
    let msg = c["msg"].trim();
    Some(Problem {
        path,
        line: c["line"].parse::<usize>().ok()?.saturating_sub(1),
        col: c["col"].parse::<usize>().ok()?.saturating_sub(1),
        severity: if &c["sev"] == "error" { 1 } else { 2 },
        message: if code.is_empty() { msg.to_string() } else { format!("{code}: {msg}") },
        from_build: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msbuild_rust_and_typescript_lines() {
        let dir = Path::new(r"C:\proj");
        let p = parse_line(r"C:\proj\Program.cs(12,5): error CS0103: The name 'x' does not exist in the current context [C:\proj\App.csproj]", dir).unwrap();
        assert_eq!((p.line, p.col, p.severity), (11, 4, 1));
        assert_eq!(p.message, "CS0103: The name 'x' does not exist in the current context");
        assert_eq!(p.path, Path::new(r"C:\proj\Program.cs"));
        let r = parse_line("src\\main.rs:3:9: warning: unused variable: `a`", dir).unwrap();
        assert_eq!((r.line, r.col, r.severity), (2, 8, 2));
        assert_eq!(r.path, dir.join("src\\main.rs"));
        let e = parse_line("src/lib.rs:10:1: error[E0425]: cannot find value `y` in this scope", dir).unwrap();
        assert_eq!(e.message, "E0425: cannot find value `y` in this scope");
        let t = parse_line("src/app.ts:4:7 - error TS2304: Cannot find name 'foo'.", dir).unwrap();
        assert_eq!((t.line, t.message.as_str()), (3, "TS2304: Cannot find name 'foo'."));
        let t2 = parse_line("\u{1b}[96msrc/app.ts\u{1b}[0m(4,7): error TS2304: Cannot find name 'foo'.", dir).unwrap();
        assert_eq!(t2.line, 3);
        assert!(parse_line("Build succeeded.", dir).is_none());
        assert!(parse_line("    0 Warning(s)", dir).is_none());
    }

    #[test]
    fn tools_are_found_and_commands_built() {
        let base = std::env::temp_dir().join(format!("wyven-tasks-{}", std::process::id()));
        let sub = base.join("App");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("App.csproj"), "<Project/>").unwrap();
        let t = detect(&base).unwrap();
        assert_eq!(t.kind, ToolKind::Dotnet);
        assert_eq!(t.dir(), sub.as_path());
        let (p, a) = t.command(TaskKind::Build).unwrap();
        assert_eq!(p, "dotnet");
        assert_eq!(a[0], "build");
        assert!(t.terminal_line(TaskKind::Run).unwrap().contains("dotnet run --project"));
        std::fs::write(base.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect(&base).unwrap().kind, ToolKind::Cargo);
        assert!(Tool { kind: ToolKind::Npm, target: base.clone() }.command(TaskKind::Clean).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }
}
