//! Project-wide search and replace (Strg+Shift+F), shown in the left sidebar instead of the
//! explorer.
//!
//! The search runs on a background thread while typing (after a short pause); a newer query
//! cancels the older search. Results arrive file by file.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};
use wyvern_search::{search_project, FileMatches, Query};

use crate::editor::{FontMetrics, Rect};
use crate::keymap::Keymap;
use crate::theme::Theme;

/// Pause after typing before the project is searched.
const DEBOUNCE: Duration = Duration::from_millis(180);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Find,
    Replace,
}

enum Msg {
    File(FileMatches),
    Done(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    File(usize),
    Line(usize, usize),
}

/// What the app should do after input in the panel.
pub enum PanelAction {
    None,
    /// Open the file and select the match: 0-based line, byte column, length in chars.
    Open { path: PathBuf, line: usize, column: usize, len: usize },
    ReplaceAll,
    Close,
}

pub struct SearchPanel {
    pub query: Query,
    pub replacement: String,
    pub replace_open: bool,
    field: Field,
    pub results: Vec<FileMatches>,
    searched: usize,
    running: bool,
    error: Option<String>,
    selected: usize,
    scroll: usize,
    generation: u64,
    cancel: Option<Arc<AtomicBool>>,
    /// When the next search should start (debounced typing).
    pub due: Option<Instant>,
    rx: Option<Receiver<(u64, Msg)>>,
    root: Option<PathBuf>,
    area: Rect,
    font: FontMetrics,
    family: String,
    top: Buffer,
    list: Buffer,
    /// Window y of the first result row (for clicks).
    list_y: f32,
}

impl SearchPanel {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut top = Buffer::new(fs, m);
        let mut list = Buffer::new(fs, m);
        top.set_wrap(Wrap::None);
        list.set_wrap(Wrap::None);
        Self {
            query: Query::default(),
            replacement: String::new(),
            replace_open: false,
            field: Field::Find,
            results: Vec::new(),
            searched: 0,
            running: false,
            error: None,
            selected: 0,
            scroll: 0,
            generation: 0,
            cancel: None,
            due: None,
            rx: None,
            root: None,
            area: Rect::default(),
            font,
            family,
            top,
            list,
            list_y: 0.0,
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.font = font;
        let m = Metrics::new(font.size, font.line_h);
        self.top.set_metrics(m);
        self.list.set_metrics(m);
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    /// Opens the panel, optionally with a prefilled query (selection or word).
    pub fn show(&mut self, root: &Path, with_replace: bool, prefill: Option<String>) {
        self.root = Some(root.to_path_buf());
        self.replace_open |= with_replace;
        self.field = Field::Find;
        if let Some(t) = prefill.filter(|t| !t.is_empty() && !t.contains('\n'))
            && t != self.query.text {
                self.query.text = t;
                self.due = Some(Instant::now());
            }
    }

    /// Searches again soon (after the query, options or files changed).
    pub fn schedule(&mut self) {
        self.due = Some(Instant::now() + DEBOUNCE);
    }

    /// Starts the search if it is due. `wake` is called from the search thread per result.
    pub fn start_if_due(&mut self, now: Instant, wake: impl Fn() + Send + 'static) -> Option<Instant> {
        let due = self.due?;
        if now < due {
            return Some(due);
        }
        self.due = None;
        if let Some(c) = self.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.generation += 1;
        self.results.clear();
        self.selected = 0;
        self.scroll = 0;
        self.searched = 0;
        self.error = None;
        self.running = false;
        let (Some(root), q) = (self.root.clone(), self.query.clone()) else { return None };
        let re = match q.compile() {
            Ok(Some(re)) => re,
            Ok(None) => return None,
            Err(e) => {
                self.error = Some(e);
                return None;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        let generation = self.generation;
        std::thread::Builder::new()
            .name("project-search".into())
            .spawn(move || {
                let searched = search_project(&root, &re, &cancel, |f| {
                    if tx.send((generation, Msg::File(f))).is_ok() {
                        wake();
                    }
                });
                let _ = tx.send((generation, Msg::Done(searched)));
                wake();
            })
            .ok();
        None
    }

    /// Takes results from the search thread.
    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok((generation, msg)) = rx.try_recv() {
            if generation != self.generation {
                continue;
            }
            match msg {
                Msg::File(f) => self.results.push(f),
                Msg::Done(n) => {
                    self.searched = n;
                    self.running = false;
                }
            }
        }
    }

    pub fn total(&self) -> usize {
        self.results.iter().map(FileMatches::count).sum()
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (fi, f) in self.results.iter().enumerate() {
            rows.push(Row::File(fi));
            for li in 0..f.lines.len() {
                rows.push(Row::Line(fi, li));
            }
        }
        rows
    }

    fn visible_rows(&self) -> usize {
        (((self.area.y + self.area.h - self.list_y) / self.font.line_h).floor() as usize).max(1)
    }

    fn open_row(&self, row: Row) -> PanelAction {
        let (fi, li) = match row {
            Row::File(fi) => (fi, 0),
            Row::Line(fi, li) => (fi, li),
        };
        let Some(f) = self.results.get(fi) else { return PanelAction::None };
        let Some(l) = f.lines.get(li) else { return PanelAction::None };
        PanelAction::Open { path: f.path.clone(), line: l.line, column: l.column, len: l.match_chars }
    }

    fn move_selection(&mut self, delta: isize) {
        let n = self.rows().len();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
        let vis = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + vis {
            self.scroll = self.selected + 1 - vis;
        }
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.rows().len().saturating_sub(self.visible_rows()) as isize;
        self.scroll = (self.scroll as isize + lines).clamp(0, max.max(0)) as usize;
    }

    pub fn paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        match self.field {
            Field::Find => {
                self.query.text.push_str(line);
                self.schedule();
            }
            Field::Replace => self.replacement.push_str(line),
        }
    }

    pub fn key(&mut self, event: &KeyEvent, mods: ModifiersState) -> PanelAction {
        let alt = mods.alt_key() && !mods.control_key();
        let ctrl = mods.control_key() && !mods.alt_key();
        let key = event.key_without_modifiers();
        if alt {
            if let Key::Character(c) = &key {
                match c.as_str() {
                    "c" => self.query.case_sensitive = !self.query.case_sensitive,
                    "w" => self.query.whole_word = !self.query.whole_word,
                    "e" => self.query.regex = !self.query.regex,
                    "a" => return PanelAction::ReplaceAll,
                    _ => return PanelAction::None,
                }
                self.schedule();
            }
            return PanelAction::None;
        }
        match &key {
            Key::Named(NamedKey::Escape) => return PanelAction::Close,
            Key::Named(NamedKey::ArrowDown) => self.move_selection(1),
            Key::Named(NamedKey::ArrowUp) => self.move_selection(-1),
            Key::Named(NamedKey::PageDown) => self.move_selection(10),
            Key::Named(NamedKey::PageUp) => self.move_selection(-10),
            Key::Named(NamedKey::Enter) => {
                let rows = self.rows();
                if let Some(&row) = rows.get(self.selected) {
                    return self.open_row(row);
                }
            }
            Key::Named(NamedKey::Tab) => {
                if self.replace_open {
                    self.field = if self.field == Field::Find { Field::Replace } else { Field::Find };
                }
            }
            Key::Character(c) if ctrl && c.as_str() == "h" => {
                self.replace_open = !self.replace_open;
                self.field = if self.replace_open { Field::Replace } else { Field::Find };
            }
            Key::Named(NamedKey::Backspace) => {
                let text = if self.field == Field::Find { &mut self.query.text } else { &mut self.replacement };
                if ctrl {
                    text.clear();
                } else {
                    text.pop();
                }
                if self.field == Field::Find {
                    self.schedule();
                }
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    if self.field == Field::Find {
                        self.query.text.push_str(&t);
                        self.schedule();
                    } else {
                        self.replacement.push_str(&t);
                    }
                }
            }
        }
        PanelAction::None
    }

    /// Click in the panel: result rows open the match.
    pub fn click(&mut self, y: f32) -> PanelAction {
        if y < self.list_y {
            return PanelAction::None;
        }
        let i = self.scroll + ((y - self.list_y) / self.font.line_h) as usize;
        let rows = self.rows();
        match rows.get(i) {
            Some(&row) => {
                self.selected = i;
                self.open_row(row)
            }
            None => PanelAction::None,
        }
    }

    fn header_lines(&self) -> usize {
        if self.replace_open { 5 } else { 4 }
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let mut top: Vec<(String, Rgba)> = vec![("SUCHEN\n".into(), theme.panel_dim)];
        let input = |text: &str, placeholder: &str, active: bool| -> (String, Rgba) {
            if text.is_empty() {
                (format!("{placeholder}\n"), theme.panel_dim)
            } else {
                (format!("{text}\n"), if active { theme.tab_text_active } else { theme.panel_text })
            }
        };
        top.push(input(&self.query.text, "Suchen im Projekt", focused && self.field == Field::Find));
        if self.replace_open {
            top.push(input(&self.replacement, "Ersetzen (Alt+A: alle)", focused && self.field == Field::Replace));
        }
        let on = |b: bool, s: &str| if b { format!("[{s}]") } else { format!(" {s} ") };
        top.push((
            format!(
                "{} {} {}   Alt+C/W/E{}\n",
                on(self.query.case_sensitive, "Aa"),
                on(self.query.whole_word, "W"),
                on(self.query.regex, ".*"),
                if self.replace_open { "" } else { " · Strg+H ersetzen" }
            ),
            theme.panel_dim,
        ));
        let status = if let Some(e) = &self.error {
            (e.clone(), theme.error)
        } else if self.query.text.is_empty() {
            (String::new(), theme.panel_dim)
        } else if self.running {
            (format!("Suche läuft … {} Treffer", self.total()), theme.panel_dim)
        } else if self.results.is_empty() {
            (format!("Keine Treffer ({} Dateien durchsucht)", self.searched), theme.panel_dim)
        } else {
            let more = if self.total() >= wyvern_search::MAX_MATCHES { "+" } else { "" };
            (format!("{}{more} Treffer in {} Dateien", self.total(), self.results.len()), theme.accent)
        };
        top.push(status);
        self.top.set_rich_text(
            top.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.top.shape_until_scroll(fs, false);

        // Result rows (only the visible ones).
        let root = self.root.clone().unwrap_or_default();
        let width_cells = ((self.area.w / self.font.cell_w) as usize).saturating_sub(2).max(10);
        let mut spans: Vec<(String, Rgba)> = Vec::new();
        let rows = self.rows();
        let vis = self.visible_rows().max(1);
        for row in rows.iter().skip(self.scroll).take(vis) {
            match *row {
                Row::File(fi) => {
                    let f = &self.results[fi];
                    let rel = f.path.strip_prefix(&root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
                    spans.push((format!("▾ {rel}"), theme.panel_text));
                    spans.push((format!("  {}\n", f.count()), theme.panel_dim));
                }
                Row::Line(fi, li) => {
                    let l = &self.results[fi].lines[li];
                    let num = format!("  {:>4}  ", l.line + 1);
                    let budget = width_cells.saturating_sub(num.chars().count());
                    spans.push((num, theme.panel_dim));
                    // Trim leading whitespace for display, keeping match positions right.
                    let trimmed = l.text.len() - l.text.trim_start().len();
                    let mut pos = trimmed;
                    let mut used = 0;
                    for r in &l.ranges {
                        if r.start < pos || used >= budget {
                            continue;
                        }
                        let before: String = l.text[pos..r.start].chars().take(budget - used).collect();
                        used += before.chars().count();
                        spans.push((before, theme.panel_text));
                        let hit: String = l.text[r.clone()].chars().take(budget.saturating_sub(used)).collect();
                        used += hit.chars().count();
                        spans.push((hit, theme.accent));
                        pos = r.end;
                    }
                    let rest: String = l.text[pos.min(l.text.len())..].chars().take(budget.saturating_sub(used)).collect();
                    spans.push((format!("{rest}\n"), theme.panel_text));
                }
            }
        }
        self.list.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.list.shape_until_scroll(fs, false);
    }

    pub fn draw<'a>(&'a mut self, theme: &Theme, focused: bool, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let lh = self.font.line_h;
        let pad = self.font.cell_w;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        let top_y = a.y + lh * 0.2;
        // Input backgrounds.
        let inputs = if self.replace_open { 2 } else { 1 };
        for i in 0..inputs {
            let active = focused && (if i == 0 { self.field == Field::Find } else { self.field == Field::Replace });
            let y = top_y + (1 + i) as f32 * lh;
            let color = if active { theme.row_selected } else { theme.popup_bg };
            under.push(Quad { x: a.x + pad * 0.5, y, w: a.w - pad, h: lh, color });
            if active {
                let len = if i == 0 { self.query.text.chars().count() } else { self.replacement.chars().count() };
                under.push(Quad { x: a.x + pad + len as f32 * pad, y, w: 2.0, h: lh, color: theme.cursor });
            }
        }
        self.list_y = top_y + self.header_lines() as f32 * lh + lh * 0.4;
        if self.selected >= self.scroll && self.selected < self.scroll + self.visible_rows() {
            let y = self.list_y + (self.selected - self.scroll) as f32 * lh;
            let color = if focused { theme.row_selected } else { theme.row_selected_unfocused };
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color });
        }
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        text.push(TextBlock { buffer: &self.top, x: a.x + pad, y: top_y, color: theme.panel_text, clip });
        text.push(TextBlock { buffer: &self.list, x: a.x + pad * 0.5, y: self.list_y, color: theme.panel_text, clip });
    }
}
