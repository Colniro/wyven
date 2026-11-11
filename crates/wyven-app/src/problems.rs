//! The Problems panel, like Visual Studio's Error List: every error and warning of the project
//! (from the language servers and from the last build), grouped by file. A click or `Enter`
//! jumps there. The second view shows the output of the last build/test task.

use std::path::{Path, PathBuf};

use winit::event::KeyEvent;
use winit::keyboard::{Key, NamedKey};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub path: PathBuf,
    /// 0-based.
    pub line: usize,
    pub col: usize,
    /// 1 error, 2 warning, 3 info.
    pub severity: u8,
    pub message: String,
    pub from_build: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Problems,
    Output,
}

pub enum ProblemAction {
    None,
    Open(PathBuf, usize, usize),
    Close,
}

enum Row {
    File(PathBuf, usize),
    Item(usize),
}

/// Lines of task output kept.
const MAX_OUTPUT: usize = 5000;

pub struct ProblemsPanel {
    lsp: Vec<Problem>,
    build: Vec<Problem>,
    /// Both lists together, sorted by file, severity, line.
    all: Vec<Problem>,
    rows: Vec<Row>,
    pub view: View,
    output: Vec<String>,
    /// One line about the last or running task.
    pub task_line: String,
    pub roots: Vec<PathBuf>,
    selected: usize,
    scroll: usize,
    /// Output view: follow the end while it grows.
    follow: bool,
    area: Rect,
    font: FontMetrics,
    family: String,
    top: Buffer,
    list: Buffer,
    list_y: f32,
    /// Fingerprints of the shaped header and list (skip shaping when unchanged).
    keys: [u64; 2],
}

impl ProblemsPanel {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut top = Buffer::new(fs, m);
        let mut list = Buffer::new(fs, m);
        top.set_wrap(Wrap::None);
        list.set_wrap(Wrap::None);
        Self {
            lsp: Vec::new(),
            build: Vec::new(),
            all: Vec::new(),
            rows: Vec::new(),
            view: View::Problems,
            output: Vec::new(),
            task_line: String::new(),
            roots: Vec::new(),
            selected: 0,
            scroll: 0,
            follow: true,
            area: Rect::default(),
            font,
            family,
            top,
            list,
            list_y: 0.0,
            keys: [0; 2],
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

    /// The language servers' diagnostics (all files).
    pub fn set_lsp(&mut self, list: Vec<Problem>) {
        if list != self.lsp {
            self.lsp = list;
            self.rebuild();
        }
    }

    pub fn set_build(&mut self, list: Vec<Problem>) {
        self.build = list;
        self.rebuild();
    }

    pub fn add_build(&mut self, p: Problem) {
        if !self.build.contains(&p) {
            self.build.push(p);
            self.rebuild();
        }
    }

    pub fn clear_output(&mut self) {
        self.output.clear();
        self.follow = true;
        if self.view == View::Output {
            self.scroll = 0;
        }
    }

    pub fn push_output(&mut self, line: String) {
        self.output.push(line);
        if self.output.len() > MAX_OUTPUT {
            self.output.drain(..self.output.len() - MAX_OUTPUT);
        }
        if self.view == View::Output && self.follow {
            self.scroll = self.output.len().saturating_sub(self.visible_rows());
        }
    }

    /// Errors and warnings in total.
    pub fn counts(&self) -> (usize, usize) {
        (self.all.iter().filter(|p| p.severity == 1).count(), self.all.iter().filter(|p| p.severity == 2).count())
    }

    pub fn toggle_view(&mut self) {
        self.view = if self.view == View::Problems { View::Output } else { View::Problems };
        self.selected = 0;
        self.scroll = if self.view == View::Output { self.output.len().saturating_sub(self.visible_rows()) } else { 0 };
        self.follow = true;
    }

    fn rebuild(&mut self) {
        let mut all: Vec<Problem> = self.lsp.clone();
        for b in &self.build {
            // The language server reports most build errors too.
            if !all.iter().any(|p| p.path == b.path && p.line == b.line && p.severity == b.severity) {
                all.push(b.clone());
            }
        }
        let key = |p: &Path| p.to_string_lossy().to_lowercase();
        all.sort_by(|a, b| key(&a.path).cmp(&key(&b.path)).then(a.severity.cmp(&b.severity)).then(a.line.cmp(&b.line)));
        self.rows.clear();
        let mut i = 0;
        while i < all.len() {
            let path = all[i].path.clone();
            let n = all[i..].iter().take_while(|p| p.path == path).count();
            self.rows.push(Row::File(path, n));
            self.rows.extend((i..i + n).map(Row::Item));
            i += n;
        }
        self.all = all;
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
    }

    fn visible_rows(&self) -> usize {
        let top = self.font.line_h * 1.5;
        (((self.area.h - top) / self.font.line_h).floor() as usize).max(1)
    }

    fn len(&self) -> usize {
        if self.view == View::Output { self.output.len() } else { self.rows.len() }
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let max = self.len().saturating_sub(self.visible_rows());
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        self.follow = self.scroll >= max;
    }

    fn keep_visible(&mut self) {
        let vis = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + vis {
            self.scroll = self.selected + 1 - vis;
        }
    }

    fn open(&self, row: usize) -> ProblemAction {
        let i = match self.rows.get(row) {
            Some(Row::Item(i)) => *i,
            // A file row: its first problem.
            Some(Row::File(..)) => match self.rows.get(row + 1) {
                Some(Row::Item(i)) => *i,
                _ => return ProblemAction::None,
            },
            None => return ProblemAction::None,
        };
        let p = &self.all[i];
        ProblemAction::Open(p.path.clone(), p.line, p.col)
    }

    pub fn click(&mut self, y: f32) -> ProblemAction {
        if self.view == View::Output || y < self.list_y {
            return ProblemAction::None;
        }
        let row = self.scroll + ((y - self.list_y) / self.font.line_h) as usize;
        if row < self.rows.len() {
            self.selected = row;
            return self.open(row);
        }
        ProblemAction::None
    }

    pub fn key(&mut self, event: &KeyEvent) -> ProblemAction {
        let n = self.len();
        match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) if n > 0 => {
                if self.view == View::Output {
                    self.scroll_by(1);
                } else {
                    self.selected = (self.selected + 1).min(n - 1);
                    self.keep_visible();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.view == View::Output {
                    self.scroll_by(-1);
                } else {
                    self.selected = self.selected.saturating_sub(1);
                    self.keep_visible();
                }
            }
            Key::Named(NamedKey::PageDown) => self.scroll_by(self.visible_rows() as isize),
            Key::Named(NamedKey::PageUp) => self.scroll_by(-(self.visible_rows() as isize)),
            Key::Named(NamedKey::Enter) if self.view == View::Problems => return self.open(self.selected),
            Key::Named(NamedKey::Tab) => self.toggle_view(),
            Key::Named(NamedKey::Escape) => return ProblemAction::Close,
            _ => {}
        }
        ProblemAction::None
    }

    fn short(&self, path: &Path) -> (String, String) {
        let name = path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
        let dir = path.parent().map_or(String::new(), |d| {
            self.roots
                .iter()
                .find_map(|r| d.strip_prefix(r).ok().map(|rel| rel.to_string_lossy().into_owned()))
                .unwrap_or_else(|| d.to_string_lossy().into_owned())
        });
        (name, dir)
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let (errors, warnings) = self.counts();
        let mut top: Vec<(String, Rgba)> = Vec::new();
        let tab = |on: bool| if on { theme.panel_text } else { theme.panel_dim };
        top.push((format!("Problems  {errors} ⊗  {warnings} ⚠"), tab(self.view == View::Problems)));
        top.push(("   ·   ".into(), theme.panel_dim));
        top.push(("Output".into(), tab(self.view == View::Output)));
        if !self.task_line.is_empty() {
            top.push((format!("   ·   {}", self.task_line), theme.accent));
        }
        if focused {
            top.push(("   (Tab switches · Enter opens)".into(), theme.panel_dim));
        }
        let extra = crate::chrome::size_key(self.area.w, self.font.size);
        let k = crate::chrome::spans_key(top.iter().map(|(t, c)| (t.as_str(), *c)), extra);
        if k != self.keys[0] {
            self.keys[0] = k;
            self.top.set_rich_text(top.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))), &attrs, Shaping::Advanced, None);
            self.top.shape_until_scroll(fs, false);
        }

        let vis = self.visible_rows();
        let mut spans: Vec<(String, Rgba)> = Vec::new();
        match self.view {
            View::Output => {
                if self.output.is_empty() {
                    spans.push(("No task has run yet. Alt → r → b builds, Alt → r → t tests.\n".into(), theme.panel_dim));
                }
                for l in self.output.iter().skip(self.scroll).take(vis) {
                    let lower = l.to_lowercase();
                    let color = if lower.contains(": error") || lower.contains("error:") || lower.contains("failed") {
                        theme.error
                    } else if lower.contains(": warning") || lower.contains("warning:") {
                        theme.warning
                    } else {
                        theme.panel_text
                    };
                    spans.push((format!("{l}\n"), color));
                }
            }
            View::Problems => {
                if self.rows.is_empty() {
                    spans.push(("No problems.\n".into(), theme.panel_dim));
                }
                for row in self.rows.iter().skip(self.scroll).take(vis) {
                    match row {
                        Row::File(path, n) => {
                            let (name, dir) = self.short(path);
                            spans.push((name.to_string(), theme.panel_text));
                            spans.push((format!("  {dir}  "), theme.panel_dim));
                            spans.push((format!("{n}\n"), theme.panel_dim));
                        }
                        Row::Item(i) => {
                            let p = &self.all[*i];
                            let (icon, color) = match p.severity {
                                1 => ("⊗", theme.error),
                                2 => ("⚠", theme.warning),
                                _ => ("ⓘ", theme.accent),
                            };
                            spans.push((format!("   {icon} "), color));
                            let msg = p.message.lines().next().unwrap_or("");
                            spans.push((msg.to_string(), theme.panel_text));
                            spans.push((format!("  ({}, {}){}\n", p.line + 1, p.col + 1, if p.from_build { "  build" } else { "" }), theme.panel_dim));
                        }
                    }
                }
            }
        }
        let k = crate::chrome::spans_key(spans.iter().map(|(t, c)| (t.as_str(), *c)), extra);
        if k != self.keys[1] {
            self.keys[1] = k;
            self.list.set_rich_text(spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))), &attrs, Shaping::Advanced, None);
            self.list.shape_until_scroll(fs, false);
        }
    }

    pub fn draw<'a>(&'a mut self, theme: &Theme, focused: bool, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let lh = self.font.line_h;
        let pad = self.font.cell_w;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        let top_y = a.y + lh * 0.2;
        self.list_y = top_y + lh * 1.3;
        if self.view == View::Problems && !self.rows.is_empty() && self.selected >= self.scroll && self.selected < self.scroll + self.visible_rows() {
            let y = self.list_y + (self.selected - self.scroll) as f32 * lh;
            let color = if focused { theme.row_selected } else { theme.row_selected_unfocused };
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color });
        }
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        text.push(TextBlock { buffer: &self.top, x: a.x + pad, y: top_y, color: theme.panel_text, clip });
        text.push(TextBlock { buffer: &self.list, x: a.x + pad, y: self.list_y, color: theme.panel_text, clip });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str, line: usize, severity: u8, from_build: bool) -> Problem {
        Problem { path: PathBuf::from(path), line, col: 0, severity, message: format!("m{line}"), from_build }
    }

    #[test]
    fn problems_are_grouped_by_file_and_build_duplicates_dropped() {
        let mut fs = FontSystem::new();
        let font = FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let mut panel = ProblemsPanel::new(&mut fs, font, "x".into());
        panel.set_area(Rect { x: 0.0, y: 0.0, w: 400.0, h: 400.0 });
        panel.set_lsp(vec![p("b.cs", 5, 2, false), p("a.cs", 9, 1, false), p("a.cs", 2, 2, false)]);
        panel.set_build(vec![p("a.cs", 9, 1, true), p("c.cs", 1, 1, true)]);
        assert_eq!(panel.counts(), (2, 2));
        // a.cs (error first), b.cs, c.cs.
        assert_eq!(panel.rows.len(), 3 + 4);
        assert!(matches!(&panel.rows[0], Row::File(f, 2) if f == Path::new("a.cs")));
        match panel.open(1) {
            ProblemAction::Open(f, line, _) => assert_eq!((f, line), (PathBuf::from("a.cs"), 9)),
            _ => panic!(),
        }
        // A file row opens its first problem.
        assert!(matches!(panel.open(3), ProblemAction::Open(f, 5, _) if f == Path::new("b.cs")));
    }
}
