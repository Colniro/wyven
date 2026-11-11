//! The terminal panel below the editor: draws a [`wyven_terminal::Terminal`] and turns keys
//! and mouse input into what the shell expects.

use std::path::{Path, PathBuf};

use winit::event::KeyEvent;
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Weight, Style, Wrap};
use wyven_terminal::{CursorStyle, Key, Mods, Snapshot, Terminal};

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

pub struct TerminalPanel {
    pub term: Option<Terminal>,
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
        Self {
            term: None,
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
        self.resize_term();
    }

    fn header_h(&self) -> f32 {
        (self.font.line_h * 1.2).round()
    }

    fn content(&self) -> Rect {
        let pad = (self.font.cell_w * 0.75).round();
        let h = self.header_h();
        Rect { x: self.area.x + pad, y: self.area.y + h, w: self.area.w - 2.0 * pad, h: self.area.h - h }
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
        if let Some(t) = &mut self.term {
            t.resize(cols, rows, cell);
        }
        self.dirty = true;
    }

    /// Starts the shell (again). `wake` is called when output arrives.
    pub fn start(&mut self, wake: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
        let (cols, rows) = self.grid_size();
        let cell = (self.font.cell_w.round() as u16, self.font.line_h.round() as u16);
        let t = Terminal::spawn(&self.shell, &self.args, &self.cwd, cols, rows, cell, &self.env, wake)
            .map_err(|e| format!("Terminal “{}” does not start: {e}", self.shell))?;
        self.term = Some(t);
        self.dirty = true;
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
        let Some(t) = &self.term else { return KeyResult::Handled };
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
        let title = match &self.term {
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
        let Some(t) = &self.term else { return };
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
        text.push(TextBlock {
            buffer: &self.buffer,
            x: c.x,
            y: c.y,
            color: theme.text,
            clip: Some([a.x as i32, (a.y + hh) as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]),
        });
    }
}
