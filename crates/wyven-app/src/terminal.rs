//! The terminal panel below the editor: draws a [`wyven_terminal::Terminal`] and turns keys
//! and mouse input into what the shell expects.
//!
//! Several terminals can be open, like in VS Code: "+" in the header starts another one, and
//! a list at the right edge (shown from two terminals on) switches between them; its × ends one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Weight, Style, Wrap};
use wyven_terminal::{CursorStyle, Key, Mods, Snapshot, TermEvent, Terminal};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

/// What a key press in the terminal turned into, beyond bytes for the shell.
pub enum KeyResult {
    Handled,
    Copy(String),
    Paste,
    Restart,
}

/// Default shell: PowerShell 7 if installed, else Windows PowerShell; `$SHELL` elsewhere.
pub fn default_shell() -> String {
    if cfg!(windows) {
        let in_path = |exe: &str| {
            std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).exists()))
        };
        if in_path("pwsh.exe") { "pwsh.exe".into() } else { "powershell.exe".into() }
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into())
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// A shell to start a terminal with ("+ ˅" in the header).
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub shell: String,
    pub args: Vec<String>,
}

/// The shells installed on this computer.
pub fn profiles() -> Vec<Profile> {
    let p = |name: &str, shell: &str, args: &[&str]| Profile { name: name.into(), shell: shell.into(), args: args.iter().map(|a| a.to_string()).collect() };
    let mut out = Vec::new();
    if cfg!(windows) {
        let in_path = |exe: &str| std::env::var_os("PATH").is_some_and(|v| std::env::split_paths(&v).any(|d| d.join(exe).exists()));
        if in_path("pwsh.exe") {
            out.push(p("PowerShell 7", "pwsh.exe", &["-NoLogo"]));
        }
        out.push(p("Windows PowerShell", "powershell.exe", &["-NoLogo"]));
        out.push(p("Command Prompt", "cmd.exe", &[]));
        for dir in [std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()), std::env::var("LOCALAPPDATA").map(|l| format!(r"{l}\Programs")).unwrap_or_default()] {
            let bash = Path::new(&dir).join(r"Git\bin\bash.exe");
            if bash.exists() {
                out.push(p("Git Bash", &bash.to_string_lossy(), &["--login", "-i"]));
                break;
            }
        }
        let wsl = Path::new(&std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join(r"System32\wsl.exe");
        if wsl.exists() {
            out.push(p("WSL", &wsl.to_string_lossy(), &[]));
        }
    } else {
        for (name, path) in [("bash", "/bin/bash"), ("zsh", "/bin/zsh"), ("fish", "/usr/bin/fish"), ("sh", "/bin/sh")] {
            if Path::new(path).exists() {
                out.push(p(name, path, &[]));
            }
        }
    }
    out
}

/// Colors for terminals in the list (name, RGB).
pub const COLORS: [(&str, [u8; 3]); 6] = [
    ("Red", [0xF2, 0x6B, 0x6B]),
    ("Green", [0x5C, 0xD6, 0x8E]),
    ("Yellow", [0xE8, 0xC5, 0x47]),
    ("Blue", [0x5A, 0xA9, 0xFF]),
    ("Purple", [0xB9, 0x92, 0xF7]),
    ("Cyan", [0x4F, 0xD1, 0xD9]),
];

/// What belongs to one terminal besides the shell process.
#[derive(Debug, Clone, Default)]
struct Meta {
    shell: String,
    args: Vec<String>,
    /// Given by the user (Rename …).
    name: Option<String>,
    color: Option<[u8; 3]>,
}

pub struct TerminalPanel {
    /// The open terminals; `active` is shown.
    terms: Vec<Terminal>,
    /// Parallel to `terms`.
    meta: Vec<Meta>,
    active: usize,
    /// Called when a shell has output (kept to start further terminals).
    wake: Option<Wake>,
    /// The list at the right edge: one label per terminal, the row under the mouse.
    labels: Vec<Buffer>,
    close_icon: Buffer,
    pub list_hover: Option<usize>,
    ui: FontMetrics,
    shell: String,
    args: Vec<String>,
    cwd: PathBuf,
    area: Rect,
    font: FontMetrics,
    family: String,
    header: Buffer,
    buffer: Buffer,
    snapshot: Option<Snapshot>,
    pub dirty: bool,
    /// Mouse is dragging a selection.
    pub selecting: bool,
    /// Extra environment for the shell (e.g. the port Claude Code uses to find Wyven).
    pub env: Vec<(String, String)>,
}

impl TerminalPanel {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, ui_font: FontMetrics, family: String) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        buffer.set_wrap(Wrap::None);
        buffer.set_monospace_width(Some(font.cell_w));
        let mut header = Buffer::new(fs, Metrics::new(ui_font.size, ui_font.line_h));
        header.set_wrap(Wrap::None);
        let icons = crate::chrome::icon_family(fs);
        let size = (ui_font.size * 0.85).round();
        let close_icon = crate::chrome::label(fs, &crate::chrome::ICON_CLOSE.to_string(), &icons, size, size * 1.2);
        Self {
            terms: Vec::new(),
            meta: Vec::new(),
            active: 0,
            wake: None,
            labels: Vec::new(),
            close_icon,
            list_hover: None,
            ui: ui_font,
            shell: default_shell(),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            area: Rect::default(),
            font,
            family,
            header,
            buffer,
            snapshot: None,
            dirty: true,
            selecting: false,
            env: Vec::new(),
        }
    }

    pub fn configure(&mut self, shell: &str, args: &[String], cwd: &Path) {
        self.shell = if shell.trim().is_empty() { default_shell() } else { shell.trim().to_string() };
        self.args = args.to_vec();
        self.cwd = cwd.to_path_buf();
    }

    pub fn set_font(&mut self, font: FontMetrics, ui_font: FontMetrics) {
        self.font = font;
        self.buffer.set_metrics(Metrics::new(font.size, font.line_h));
        self.buffer.set_monospace_width(Some(font.cell_w));
        self.header.set_metrics(Metrics::new(ui_font.size, ui_font.line_h));
        self.ui = ui_font;
        self.labels.clear();
        self.resize_term();
    }

    /// The shown terminal.
    pub fn term(&self) -> Option<&Terminal> {
        self.terms.get(self.active)
    }

    pub fn count(&self) -> usize {
        self.terms.len()
    }

    pub fn active(&self) -> usize {
        self.active
    }

    /// Shows terminal `i`.
    pub fn select(&mut self, i: usize) {
        if i < self.terms.len() && i != self.active {
            self.active = i;
            self.selecting = false;
            self.resize_term();
        }
    }

    /// Ends terminal `i`. Returns false when none is left.
    pub fn close(&mut self, i: usize) -> bool {
        if i < self.terms.len() {
            self.terms.remove(i);
            self.meta.remove(i);
            if self.active > i || self.active >= self.terms.len() {
                self.active = self.active.saturating_sub(1);
            }
            self.labels.clear();
            self.list_hover = None;
            self.resize_term();
        }
        !self.terms.is_empty()
    }

    /// Output of every terminal; `Exited` comes with the terminal's number.
    pub fn poll(&mut self) -> Vec<(usize, TermEvent)> {
        let mut out = Vec::new();
        for (i, t) in self.terms.iter_mut().enumerate() {
            out.extend(t.poll().into_iter().map(|e| (i, e)));
        }
        self.labels.clear();
        self.dirty = true;
        out
    }

    /// Width of the terminal list at the right edge (none for a single terminal).
    fn list_w(&self) -> f32 {
        if self.terms.len() < 2 {
            return 0.0;
        }
        (self.ui.cell_w * 22.0).min(self.area.w * 0.35).round()
    }

    fn row_h(&self) -> f32 {
        (self.ui.line_h * 1.5).round()
    }

    /// The list row under a point: the terminal's number and whether it is its ×.
    pub fn list_hit(&self, x: f32, y: f32) -> Option<(usize, bool)> {
        let lw = self.list_w();
        let a = self.area;
        if lw == 0.0 || x < a.x + a.w - lw || x >= a.x + a.w || y < a.y || y >= a.y + a.h {
            return None;
        }
        let top = a.y + self.header_h() * 0.4;
        let i = ((y - top) / self.row_h()).floor();
        if i < 0.0 || i as usize >= self.terms.len() {
            return None;
        }
        let close = x >= a.x + a.w - self.row_h() - self.ui.cell_w * 0.4;
        Some((i as usize, close))
    }

    /// A name for the list: the shell's title (the last part of a path), else the shell.
    fn name(&self, i: usize) -> String {
        let t = &self.terms[i];
        let m = &self.meta[i];
        if let Some(n) = &m.name {
            return if t.exited { format!("{n} (exited)") } else { n.clone() };
        }
        let raw = if t.title.trim().is_empty() { m.shell.as_str() } else { t.title.trim() };
        let last = raw.rsplit(['\\', '/']).next().unwrap_or(raw);
        let last = last.strip_suffix(".exe").unwrap_or(last);
        if t.exited { format!("{last} (exited)") } else { last.to_string() }
    }

    fn header_h(&self) -> f32 {
        (self.font.line_h * 1.2).round()
    }

    fn content(&self) -> Rect {
        let pad = (self.font.cell_w * 0.75).round();
        let h = self.header_h();
        let w = self.area.w - 2.0 * pad - self.list_w();
        Rect { x: self.area.x + pad, y: self.area.y + h, w: w.max(self.font.cell_w * 2.0), h: self.area.h - h }
    }

    fn grid_size(&self) -> (usize, usize) {
        let c = self.content();
        (((c.w / self.font.cell_w).floor() as usize).max(2), ((c.h / self.font.line_h).floor() as usize).max(1))
    }

    pub fn set_area(&mut self, area: Rect) {
        if area != self.area {
            self.area = area;
            self.resize_term();
        }
    }

    fn resize_term(&mut self) {
        let (cols, rows) = self.grid_size();
        let cell = (self.font.cell_w.round() as u16, self.font.line_h.round() as u16);
        if let Some(t) = self.terms.get_mut(self.active) {
            t.resize(cols, rows, cell);
        }
        self.dirty = true;
    }

    fn spawn(&self, shell: &str, args: &[String], wake: &Wake) -> Result<Terminal, String> {
        let (cols, rows) = self.grid_size();
        let cell = (self.font.cell_w.round() as u16, self.font.line_h.round() as u16);
        let w = wake.clone();
        Terminal::spawn(shell, args, &self.cwd, cols, rows, cell, &self.env, move || w())
            .map_err(|e| format!("Terminal “{shell}” does not start: {e}"))
    }

    /// Gives terminal `i` a name of its own (empty: back to the shell's title).
    pub fn rename(&mut self, i: usize, name: &str) {
        if let Some(m) = self.meta.get_mut(i) {
            m.name = (!name.trim().is_empty()).then(|| name.trim().to_string());
            self.labels.clear();
        }
    }

    pub fn set_color(&mut self, i: usize, color: Option<[u8; 3]>) {
        if let Some(m) = self.meta.get_mut(i) {
            m.color = color;
        }
    }

    /// The name shown for terminal `i` (for the rename field).
    pub fn display_name(&self, i: usize) -> String {
        if i < self.terms.len() { self.meta[i].name.clone().unwrap_or_else(|| self.name(i)) } else { String::new() }
    }

    /// Starts the shell (again, in place of the shown terminal). `wake` is called when output
    /// arrives.
    pub fn start(&mut self, wake: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
        let wake: Wake = Arc::new(wake);
        // A restart keeps the terminal's shell, name and color.
        let (shell, args) = match self.meta.get(self.active) {
            Some(m) if !self.terms.is_empty() => (m.shell.clone(), m.args.clone()),
            _ => (self.shell.clone(), self.args.clone()),
        };
        let t = self.spawn(&shell, &args, &wake)?;
        self.wake = Some(wake);
        if self.terms.is_empty() {
            self.terms.push(t);
            self.meta.push(Meta { shell, args, ..Meta::default() });
            self.active = 0;
        } else {
            self.terms[self.active] = t;
        }
        self.labels.clear();
        self.dirty = true;
        Ok(())
    }

    /// Starts one more terminal and shows it (after [`TerminalPanel::start`]).
    pub fn add(&mut self) -> Result<(), String> {
        let (shell, args) = (self.shell.clone(), self.args.clone());
        self.add_with(&shell, &args, None)
    }

    /// One more terminal with another shell (a profile); `name` shows in the list.
    pub fn add_with(&mut self, shell: &str, args: &[String], name: Option<&str>) -> Result<(), String> {
        let Some(wake) = self.wake.clone() else { return Err("The terminal is not running yet".into()) };
        // The list appears: the new shell gets the narrower size right away.
        self.terms.push(self.spawn(shell, args, &wake)?);
        self.meta.push(Meta { shell: shell.to_string(), args: args.to_vec(), name: name.map(str::to_string), color: None });
        self.active = self.terms.len() - 1;
        self.labels.clear();
        self.resize_term();
        Ok(())
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    /// Screen cell under a window position.
    pub fn cell_at(&self, x: f32, y: f32) -> (usize, usize) {
        let c = self.content();
        let col = ((x - c.x) / self.font.cell_w).max(0.0) as usize;
        let row = ((y - c.y) / self.font.line_h).max(0.0) as usize;
        let (cols, rows) = self.grid_size();
        (row.min(rows - 1), col.min(cols - 1))
    }

    /// Keys while the terminal has focus. Everything goes to the shell except copy/paste.
    pub fn key(&mut self, event: &KeyEvent, mods: ModifiersState) -> KeyResult {
        let Some(t) = self.terms.get(self.active) else { return KeyResult::Handled };
        let ctrl = mods.control_key();
        let alt = mods.alt_key();
        let shift = mods.shift_key();
        let altgr = ctrl && alt; // Windows reports AltGr as Strg+Alt
        let key = event.key_without_modifiers();
        if t.exited {
            if key == WKey::Named(NamedKey::Enter) {
                return KeyResult::Restart;
            }
            return KeyResult::Handled;
        }
        if ctrl && shift && !alt
            && let WKey::Character(c) = &key {
                match c.as_str() {
                    "c" => return t.selection_text().map_or(KeyResult::Handled, KeyResult::Copy),
                    "v" => return KeyResult::Paste,
                    _ => {}
                }
            }
        let m = Mods { ctrl, alt: alt && !altgr, shift };
        let special = match &key {
            WKey::Named(n) => match n {
                NamedKey::Enter => Some(Key::Enter),
                NamedKey::Backspace => Some(Key::Backspace),
                NamedKey::Tab => Some(Key::Tab),
                NamedKey::Escape => Some(Key::Escape),
                NamedKey::ArrowUp => Some(Key::Up),
                NamedKey::ArrowDown => Some(Key::Down),
                NamedKey::ArrowLeft => Some(Key::Left),
                NamedKey::ArrowRight => Some(Key::Right),
                NamedKey::Home => Some(Key::Home),
                NamedKey::End => Some(Key::End),
                NamedKey::PageUp => Some(Key::PageUp),
                NamedKey::PageDown => Some(Key::PageDown),
                NamedKey::Insert => Some(Key::Insert),
                NamedKey::Delete => Some(Key::Delete),
                NamedKey::F1 => Some(Key::F(1)),
                NamedKey::F2 => Some(Key::F(2)),
                NamedKey::F3 => Some(Key::F(3)),
                NamedKey::F4 => Some(Key::F(4)),
                NamedKey::F5 => Some(Key::F(5)),
                NamedKey::F6 => Some(Key::F(6)),
                NamedKey::F7 => Some(Key::F(7)),
                NamedKey::F8 => Some(Key::F(8)),
                NamedKey::F9 => Some(Key::F(9)),
                NamedKey::F10 => Some(Key::F(10)),
                NamedKey::F11 => Some(Key::F(11)),
                NamedKey::F12 => Some(Key::F(12)),
                NamedKey::Space if ctrl && !altgr => {
                    t.ctrl_char(' ');
                    return KeyResult::Handled;
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(k) = special {
            if k == Key::Escape {
                t.clear_selection();
            }
            t.key(k, m);
            self.dirty = true;
            return KeyResult::Handled;
        }
        // Strg+letter → control character (but AltGr+key is a typed character).
        if ctrl && !altgr {
            if let WKey::Character(c) = &key
                && let Some(ch) = c.chars().next() {
                    t.ctrl_char(ch);
                }
            return KeyResult::Handled;
        }
        if let Some(text) = event.text.as_ref().filter(|s| !s.chars().any(char::is_control)) {
            t.type_text(text, alt && !altgr);
        }
        KeyResult::Handled
    }

    /// Re-reads the screen if something changed.
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        if self.labels.len() != self.terms.len() {
            let names: Vec<String> = (0..self.terms.len()).map(|i| format!("{}  {}", i + 1, self.name(i))).collect();
            let m = Metrics::new(self.ui.size * 0.95, self.ui.line_h);
            self.labels = names
                .iter()
                .map(|n| {
                    let mut b = Buffer::new(fs, m);
                    b.set_wrap(Wrap::None);
                    b.set_text(n, &attrs, Shaping::Advanced, None);
                    b.shape_until_scroll(fs, false);
                    b
                })
                .collect();
        }
        let title = match self.terms.get(self.active) {
            Some(t) if t.exited => "process exited – Enter restarts it".to_string(),
            Some(t) if !t.title.is_empty() => t.title.clone(),
            _ => self.shell.clone(),
        };
        let hint = if focused { "   (Alt → t closes · Ctrl+Shift+C/V copy/paste)" } else { "" };
        self.header.set_text(&format!("{title}{hint}"), &attrs, Shaping::Advanced, None);
        self.header.shape_until_scroll(fs, false);
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let Some(t) = self.terms.get(self.active) else { return };
        let palette = theme.terminal_palette();
        let mut snap = t.snapshot(&palette);
        // Block cursor: draw the character under it in the background color.
        if let (true, Some((r, c, CursorStyle::Block))) = (focused, snap.cursor)
            && let Some(cell) = snap.lines.get_mut(r).and_then(|l| l.get_mut(c)) {
                cell.fg = palette.background;
            }
        let mut spans: Vec<(String, [u8; 3], bool, bool)> = Vec::new();
        for line in &snap.lines {
            for cell in line.iter().filter(|c| !c.spacer) {
                match spans.last_mut() {
                    Some((s, fg, b, i)) if *fg == cell.fg && *b == cell.bold && *i == cell.italic => s.push(cell.c),
                    _ => spans.push((cell.c.to_string(), cell.fg, cell.bold, cell.italic)),
                }
            }
            match spans.last_mut() {
                Some((s, ..)) => s.push('\n'),
                None => spans.push(("\n".into(), palette.foreground, false, false)),
            }
        }
        self.buffer.set_rich_text(
            spans.iter().map(|(s, fg, bold, italic)| {
                let mut a = attrs.clone().color(Rgba::rgb(fg[0], fg[1], fg[2]).to_glyphon());
                if *bold {
                    a = a.weight(Weight::BOLD);
                }
                if *italic {
                    a = a.style(Style::Italic);
                }
                (s.as_str(), a)
            }),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
        self.snapshot = Some(snap);
    }

    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        focused: bool,
        scale: f32,
        under: &mut Vec<Quad>,
        over: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let a = self.area;
        let bg = theme.terminal_palette().background;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: Rgba::rgb(bg[0], bg[1], bg[2]) });
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: (scale).round().max(1.0), color: theme.divider });
        let hh = self.header_h();
        text.push(TextBlock {
            buffer: &self.header,
            x: a.x + self.font.cell_w * 0.75,
            y: a.y + (hh - self.header_h() * 0.85) / 2.0,
            color: if focused { theme.panel_text } else { theme.panel_dim },
            clip: Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + hh) as i32]),
        });
        let c = self.content();
        let (cw, lh) = (self.font.cell_w, self.font.line_h);
        if let Some(snap) = &self.snapshot {
            for (r, line) in snap.lines.iter().enumerate() {
                let y = c.y + r as f32 * lh;
                let mut col = 0;
                while col < line.len() {
                    let Some(color) = line[col].bg else {
                        col += 1;
                        continue;
                    };
                    let start = col;
                    while col < line.len() && line[col].bg == Some(color) {
                        col += 1;
                    }
                    under.push(Quad {
                        x: c.x + start as f32 * cw,
                        y,
                        w: (col - start) as f32 * cw,
                        h: lh,
                        color: Rgba::rgb(color[0], color[1], color[2]),
                    });
                }
                for (i, cell) in line.iter().enumerate().filter(|(_, c)| c.underline) {
                    let u = Rgba::rgb(cell.fg[0], cell.fg[1], cell.fg[2]);
                    over.push(Quad { x: c.x + i as f32 * cw, y: y + lh - 2.0 * scale, w: cw, h: scale.max(1.0), color: u });
                }
            }
            if let Some((r, col, style)) = snap.cursor {
                let (x, y) = (c.x + col as f32 * cw, c.y + r as f32 * lh);
                let cur = theme.terminal_palette().cursor;
                let color = Rgba::rgb(cur[0], cur[1], cur[2]);
                let t = (scale * 2.0).round().max(1.0);
                match (focused, style) {
                    (true, CursorStyle::Block) => under.push(Quad { x, y, w: cw, h: lh, color }),
                    (true, CursorStyle::Beam) => over.push(Quad { x, y, w: t, h: lh, color }),
                    (true, CursorStyle::Underline) => over.push(Quad { x, y: y + lh - t, w: cw, h: t, color }),
                    (false, _) => {
                        // Not focused: hollow box.
                        let b = scale.round().max(1.0);
                        over.push(Quad { x, y, w: cw, h: b, color });
                        over.push(Quad { x, y: y + lh - b, w: cw, h: b, color });
                        over.push(Quad { x, y, w: b, h: lh, color });
                        over.push(Quad { x: x + cw - b, y, w: b, h: lh, color });
                    }
                }
            }
        }
        let lw = self.list_w();
        text.push(TextBlock {
            buffer: &self.buffer,
            x: c.x,
            y: c.y,
            color: theme.text,
            clip: Some([a.x as i32, (a.y + hh) as i32, (a.x + a.w - lw) as i32, (a.y + a.h) as i32]),
        });
        // The terminal list at the right edge: the shown one highlighted with a colored bar.
        if lw > 0.0 {
            let lx = a.x + a.w - lw;
            under.push(Quad { x: lx, y: a.y, w: 1.0, h: a.h, color: theme.divider });
            let rh = self.row_h();
            let top = a.y + hh * 0.4;
            let accent = crate::chrome::panel_color(crate::dock::Panel::Terminal, theme.dark);
            let icon_size = (self.ui.size * 0.85).round();
            for (i, label) in self.labels.iter().enumerate() {
                let y = top + i as f32 * rh;
                if y + rh > a.y + a.h {
                    break;
                }
                let hover = self.list_hover.map(|h| h == i);
                if i == self.active {
                    under.push(Quad { x: lx + 1.0, y, w: lw - 1.0, h: rh, color: theme.tab_active });
                    under.push(Quad { x: lx + 1.0, y: y + rh * 0.2, w: (2.0 * scale).round(), h: rh * 0.6, color: accent });
                } else if hover.is_some() {
                    under.push(Quad { x: lx + 1.0, y, w: lw - 1.0, h: rh, color: theme.tab_active.with_alpha(0x80) });
                }
                let ty = y + (rh - self.ui.line_h) / 2.0;
                let close_x = a.x + a.w - rh;
                if let Some(c) = self.meta.get(i).and_then(|m| m.color) {
                    let d = (self.ui.line_h * 0.4).round();
                    under.push(Quad { x: lx + self.ui.cell_w * 0.5, y: y + (rh - d) / 2.0, w: (3.0 * scale).round(), h: d, color: Rgba::rgb(c[0], c[1], c[2]) });
                }
                text.push(TextBlock {
                    buffer: label,
                    x: lx + self.ui.cell_w * 1.2,
                    y: ty,
                    color: if i == self.active { theme.panel_text } else { theme.panel_dim },
                    clip: Some([lx as i32, y as i32, (if hover.is_some() { close_x } else { a.x + a.w }) as i32, (y + rh) as i32]),
                });
                if hover.is_some() {
                    let iw = self.close_icon.layout_runs().next().map_or(icon_size, |r| r.line_w);
                    text.push(TextBlock {
                        buffer: &self.close_icon,
                        x: (close_x + (rh - iw) / 2.0).round(),
                        y: (y + (rh - icon_size * 1.2) / 2.0).round(),
                        color: theme.panel_text,
                        clip: None,
                    });
                }
            }
        }
    }
}
