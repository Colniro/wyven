//! The integrated terminal: a shell in a pseudo console (ConPTY on Windows), with Alacritty's
//! terminal emulation.
//!
//! Alacritty's event loop runs the shell's input/output on its own thread and updates the
//! terminal grid. The app only sends key bytes, asks for a [`Snapshot`] of the visible screen
//! when it draws, and gets woken up through the `wake` callback when there is new output.

use std::borrow::Cow;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};

/// What the app needs to know from the terminal.
#[derive(Debug, Clone, PartialEq)]
pub enum TermEvent {
    /// New output: redraw.
    Wakeup,
    Title(String),
    /// The program asked to copy text (OSC 52).
    Copy(String),
    /// The shell ended.
    Exited,
}

#[derive(Clone)]
struct Listener {
    tx: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            (self.wake)();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TermSize {
    cols: usize,
    lines: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Colors the app chooses (from its theme).
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub selection: [u8; 3],
    /// The 16 ANSI colors: black, red, green, yellow, blue, magenta, cyan, white, then bright.
    pub ansi: [[u8; 3]; 16],
}

/// One character cell as it should be drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellView {
    pub c: char,
    pub fg: [u8; 3],
    /// `None` = default background (nothing to draw).
    pub bg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Second half of a double-width character: draw nothing.
    pub spacer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Block,
    Beam,
    Underline,
}

/// The visible screen.
pub struct Snapshot {
    pub cols: usize,
    pub lines: Vec<Vec<CellView>>,
    /// Row and column on screen, if the cursor is visible.
    pub cursor: Option<(usize, usize, CursorStyle)>,
    /// Scrolled back by this many lines (0 = at the bottom).
    pub scrolled: usize,
}

/// Keys that are not plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    events: Receiver<Event>,
    size: TermSize,
    pub exited: bool,
    pub title: String,
}

impl Terminal {
    /// Starts `shell` in `cwd`. `wake` is called from the terminal's thread on new output.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        shell: &str,
        args: &[String],
        cwd: &Path,
        cols: usize,
        lines: usize,
        cell: (u16, u16),
        extra_env: &[(String, String)],
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let size = TermSize { cols: cols.max(2), lines: lines.max(1) };
        let (tx, events) = mpsc::channel();
        let listener = Listener { tx, wake: Arc::new(wake) };
        let config = Config { scrolling_history: 5000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));

        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "Wyven".to_string());
        for (k, v) in extra_env {
            env.insert(k.clone(), v.clone());
        }
        let options = tty::Options {
            shell: Some(tty::Shell::new(shell.to_string(), args.to_vec())),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
            ..Default::default()
        };
        let pty = tty::new(&options, window_size(size, cell), 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Self { term, notifier, events, size, exited: false, title: String::new() })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.size.cols, self.size.lines)
    }

    /// Handles what the terminal thread reported since the last call.
    pub fn poll(&mut self) -> Vec<TermEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.events.try_recv() {
            match e {
                Event::Wakeup => out.push(TermEvent::Wakeup),
                Event::Title(t) => {
                    self.title = t.clone();
                    out.push(TermEvent::Title(t));
                }
                Event::ResetTitle => self.title.clear(),
                Event::PtyWrite(s) => self.notifier.notify(s.into_bytes()),
                Event::ClipboardStore(_, text) => out.push(TermEvent::Copy(text)),
                Event::Exit | Event::ChildExit(_) => {
                    if !self.exited {
                        self.exited = true;
                        out.push(TermEvent::Exited);
                    }
                }
                Event::TextAreaSizeRequest(f) => {
                    let reply = f(window_size(self.size, (8, 16)));
                    self.notifier.notify(reply.into_bytes());
                }
                _ => {}
            }
        }
        out
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if !self.exited {
            self.notifier.notify(bytes);
        }
    }

    /// Typed text (already layout-resolved by the OS, e.g. AltGr characters).
    pub fn type_text(&self, text: &str, alt: bool) {
        self.scroll_to_bottom();
        let mut bytes = Vec::with_capacity(text.len() + 1);
        if alt {
            bytes.push(0x1b);
        }
        bytes.extend_from_slice(text.as_bytes());
        self.write(bytes);
    }

    /// A control key combination like Strg+C.
    pub fn ctrl_char(&self, c: char) {
        let c = c.to_ascii_lowercase();
        let byte = match c {
            'a'..='z' => Some(c as u8 - b'a' + 1),
            '@' | ' ' | '2' => Some(0),
            '[' | '3' => Some(0x1b),
            '\\' | '4' => Some(0x1c),
            ']' | '5' => Some(0x1d),
            '^' | '6' => Some(0x1e),
            '_' | '-' | '7' => Some(0x1f),
            _ => None,
        };
        if let Some(b) = byte {
            self.scroll_to_bottom();
            self.write(vec![b]);
        }
    }

    pub fn key(&self, key: Key, mods: Mods) {
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = encode_key(key, mods, app_cursor) {
            self.scroll_to_bottom();
            self.write(bytes);
        }
    }

    /// Pastes text; with bracketed paste the program can tell it from typing.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        let data = if bracketed {
            format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', ""))
        } else {
            text
        };
        self.scroll_to_bottom();
        self.write(data.into_bytes());
    }

    pub fn resize(&mut self, cols: usize, lines: usize, cell: (u16, u16)) {
        let size = TermSize { cols: cols.max(2), lines: lines.max(1) };
        if size == self.size {
            return;
        }
        self.size = size;
        self.notifier.on_resize(window_size(size, cell));
        self.term.lock().resize(size);
    }

    /// Mouse wheel. In full-screen programs (alt screen) arrow keys are sent instead.
    pub fn scroll(&self, lines: i32) {
        let mut term = self.term.lock();
        if term.mode().contains(TermMode::ALT_SCREEN) {
            drop(term);
            let key = if lines > 0 { Key::Up } else { Key::Down };
            for _ in 0..lines.unsigned_abs().min(10) {
                self.key(key, Mods::default());
            }
        } else {
            term.scroll_display(Scroll::Delta(lines));
        }
    }

    fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    // ----- selection (screen row/col) -----

    fn grid_point(term: &Term<Listener>, row: usize, col: usize) -> Point {
        let offset = term.grid().display_offset() as i32;
        Point::new(Line(row as i32 - offset), Column(col.min(term.columns().saturating_sub(1))))
    }

    pub fn select_start(&self, row: usize, col: usize) {
        let mut term = self.term.lock();
        let p = Self::grid_point(&term, row, col);
        term.selection = Some(Selection::new(SelectionType::Simple, p, Side::Left));
    }

    pub fn select_to(&self, row: usize, col: usize) {
        let mut term = self.term.lock();
        let p = Self::grid_point(&term, row, col);
        if let Some(sel) = term.selection.as_mut() {
            sel.update(p, Side::Right);
        }
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// The visible screen with resolved colors.
    pub fn snapshot(&self, palette: &Palette) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = term.columns();
        let rows = term.screen_lines();
        let offset = content.display_offset;
        let colors = content.colors;
        let selection = content.selection;
        let resolve = |c: Color, is_fg: bool| -> [u8; 3] {
            let rgb = |r: Rgb| [r.r, r.g, r.b];
            match c {
                Color::Spec(r) => rgb(r),
                Color::Indexed(i) => colors[i as usize].map_or_else(|| indexed(i, palette), rgb),
                Color::Named(n) => {
                    let idx = n as usize;
                    if let Some(r) = colors[idx] {
                        return rgb(r);
                    }
                    match n {
                        NamedColor::Foreground | NamedColor::BrightForeground => palette.foreground,
                        NamedColor::Background => palette.background,
                        NamedColor::Cursor => palette.cursor,
                        NamedColor::DimForeground => dim(palette.foreground),
                        _ if idx < 16 => palette.ansi[idx],
                        _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&idx) => {
                            dim(palette.ansi[idx - NamedColor::DimBlack as usize])
                        }
                        _ if is_fg => palette.foreground,
                        _ => palette.background,
                    }
                }
            }
        };
        let mut lines = vec![
            vec![
                CellView { c: ' ', fg: palette.foreground, bg: None, bold: false, italic: false, underline: false, spacer: false };
                cols
            ];
            rows
        ];
        for indexed_cell in content.display_iter {
            let point = indexed_cell.point;
            let row = (point.line.0 + offset as i32) as usize;
            let col = point.column.0;
            if row >= rows || col >= cols {
                continue;
            }
            let cell = indexed_cell.cell;
            let mut fg = resolve(cell.fg, true);
            let bg_color = cell.bg;
            let default_bg = matches!(bg_color, Color::Named(NamedColor::Background));
            let mut bg = if default_bg { None } else { Some(resolve(bg_color, false)) };
            if cell.flags.contains(Flags::INVERSE) {
                let b = bg.unwrap_or(palette.background);
                bg = Some(fg);
                fg = b;
            }
            if cell.flags.contains(Flags::DIM) {
                fg = dim(fg);
            }
            let selected = selection.is_some_and(|s| s.contains(point));
            if selected {
                bg = Some(palette.selection);
            }
            let hidden = cell.flags.contains(Flags::HIDDEN);
            lines[row][col] = CellView {
                c: if hidden { ' ' } else { cell.c },
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::UNDERLINE | Flags::DOUBLE_UNDERLINE | Flags::UNDERCURL),
                spacer: cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            };
        }
        let cursor = if content.mode.contains(TermMode::SHOW_CURSOR) && offset == 0 {
            let p = content.cursor.point;
            let style = match content.cursor.shape {
                CursorShape::Hidden => None,
                CursorShape::Beam => Some(CursorStyle::Beam),
                CursorShape::Underline => Some(CursorStyle::Underline),
                _ => Some(CursorStyle::Block),
            };
            let row = p.line.0 + offset as i32;
            style.filter(|_| row >= 0 && (row as usize) < rows).map(|s| (row as usize, p.column.0, s))
        } else {
            None
        };
        Snapshot { cols, lines, cursor, scrolled: offset }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

fn window_size(size: TermSize, cell: (u16, u16)) -> WindowSize {
    WindowSize {
        num_lines: size.lines as u16,
        num_cols: size.cols as u16,
        cell_width: cell.0.max(1),
        cell_height: cell.1.max(1),
    }
}

fn dim(c: [u8; 3]) -> [u8; 3] {
    c.map(|v| (v as u16 * 2 / 3) as u8)
}

/// xterm 256-color palette: 16 theme colors, a 6×6×6 cube, 24 grays.
fn indexed(i: u8, palette: &Palette) -> [u8; 3] {
    match i {
        0..=15 => palette.ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            [level(i / 36), level((i / 6) % 6), level(i % 6)]
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            [v, v, v]
        }
    }
}

/// Bytes a key sends, like xterm. Modifiers on special keys use the `CSI 1;m X` form.
pub fn encode_key(key: Key, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    let csi = |final_byte: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 { format!("\x1b[{n};{m}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt_prefix = |mut b: Vec<u8>| {
        if mods.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    Some(match key {
        Key::Enter => alt_prefix(vec![b'\r']),
        Key::Backspace if mods.ctrl => alt_prefix(vec![0x08]),
        Key::Backspace => alt_prefix(vec![0x7f]),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        Key::Up => csi('A'),
        Key::Down => csi('B'),
        Key::Right => csi('C'),
        Key::Left => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n @ 1..=4) => {
            let c = b"PQRS"[(n - 1) as usize] as char;
            if m > 1 { format!("\x1b[1;{m}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
        }
        Key::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][(n - 5) as usize]),
        Key::F(_) => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_encode_like_xterm() {
        let none = Mods::default();
        assert_eq!(encode_key(Key::Up, none, false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, none, true).unwrap(), b"\x1bOA");
        let ctrl = Mods { ctrl: true, ..none };
        assert_eq!(encode_key(Key::Right, ctrl, false).unwrap(), b"\x1b[1;5C");
        assert_eq!(encode_key(Key::Delete, none, false).unwrap(), b"\x1b[3~");
        assert_eq!(encode_key(Key::Tab, Mods { shift: true, ..none }, false).unwrap(), b"\x1b[Z");
        assert_eq!(encode_key(Key::Backspace, none, false).unwrap(), vec![0x7f]);
        assert_eq!(encode_key(Key::F(5), none, false).unwrap(), b"\x1b[15~");
    }

    #[test]
    fn palette_256_colors() {
        let p = Palette {
            foreground: [1, 1, 1],
            background: [0, 0, 0],
            cursor: [1, 1, 1],
            selection: [2, 2, 2],
            ansi: [[9, 9, 9]; 16],
        };
        assert_eq!(indexed(3, &p), [9, 9, 9]);
        assert_eq!(indexed(16, &p), [0, 0, 0]);
        assert_eq!(indexed(231, &p), [255, 255, 255]);
        assert_eq!(indexed(232, &p), [8, 8, 8]);
    }

    /// Starts a real shell, runs a command and finds its output on screen.
    #[test]
    #[cfg(windows)]
    fn runs_a_real_shell() {
        let cwd = std::env::temp_dir();
        let mut t = Terminal::spawn("cmd.exe", &[], &cwd, 80, 24, (8, 16), &[], || {}).unwrap();
        t.type_text("echo wyven-terminal-ok", false);
        t.key(Key::Enter, Mods::default());
        let palette = Palette {
            foreground: [200, 200, 200],
            background: [0, 0, 0],
            cursor: [255, 255, 255],
            selection: [50, 50, 120],
            ansi: [[128, 128, 128]; 16],
        };
        let start = std::time::Instant::now();
        loop {
            t.poll();
            let snap = t.snapshot(&palette);
            let text: String = snap
                .lines
                .iter()
                .map(|l| l.iter().map(|c| c.c).collect::<String>() + "\n")
                .collect();
            if text.matches("wyven-terminal-ok").count() >= 2 {
                break; // the typed command and its output
            }
            assert!(start.elapsed().as_secs() < 15, "no output:\n{text}");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
