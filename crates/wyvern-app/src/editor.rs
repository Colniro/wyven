//! The code editor view: draws a [`Document`] with line numbers, highlighting, selections and
//! cursors, and maps mouse positions back to text positions.
//!
//! Only the visible lines are shaped. The shaped text is cached and rebuilt only when the
//! text, the scroll position, the size or the current line changes; cursors and selections
//! are cheap rectangles drawn every frame.

use std::path::Path;

use wyvern_buffer::{char_to_display_col, display_col_to_char, line_len_chars, Document};
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};
use wyvern_syntax::{flatten, language_for_path, Syntax};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub size: f32,
    pub line_h: f32,
    pub cell_w: f32,
}

#[derive(PartialEq)]
struct CacheKey {
    version: u64,
    scroll_line: usize,
    rows: usize,
    current_line: usize,
    font: (u32, u32),
    gutter_cells: usize,
}

/// A problem reported by the language server, as a char range in the current text.
#[derive(Debug, Clone)]
pub struct DiagSpan {
    pub start: usize,
    pub end: usize,
    /// 1 error, 2 warning, 3 info.
    pub severity: u8,
    pub message: String,
    /// The server's own diagnostic object (sent back when asking for quick actions).
    pub raw: wyvern_lsp::Value,
}

pub struct EditorView {
    pub doc: Document,
    pub diagnostics: Vec<DiagSpan>,
    syntax: Option<Syntax>,
    pub scroll_line: usize,
    pub scroll_x: f32,
    area: Rect,
    font: FontMetrics,
    family: String,
    gutter_cells: usize,
    text_buf: Buffer,
    gutter_buf: Buffer,
    cache: Option<CacheKey>,
}

impl EditorView {
    pub fn new(doc: Document, fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let syntax = doc
            .path()
            .and_then(language_for_path)
            .map(|cfg| Syntax::new(cfg, doc.text()));
        let metrics = Metrics::new(font.size, font.line_h);
        let mut text_buf = Buffer::new(fs, metrics);
        let mut gutter_buf = Buffer::new(fs, metrics);
        for b in [&mut text_buf, &mut gutter_buf] {
            b.set_wrap(Wrap::None);
            b.set_monospace_width(Some(font.cell_w));
        }
        Self {
            doc,
            diagnostics: Vec::new(),
            syntax,
            scroll_line: 0,
            scroll_x: 0.0,
            area: Rect::default(),
            font,
            family,
            gutter_cells: 4,
            text_buf,
            gutter_buf,
            cache: None,
        }
    }

    pub fn language_name(&self) -> &'static str {
        self.syntax.as_ref().map_or("Text", |s| s.config().name)
    }

    pub fn line_comment(&self) -> &'static str {
        self.syntax
            .as_ref()
            .and_then(|s| s.config().line_comment)
            .unwrap_or("//")
    }

    pub fn file_name(&self) -> String {
        self.doc
            .path()
            .and_then(Path::file_name)
            .map_or("Unbenannt".into(), |n| n.to_string_lossy().into_owned())
    }

    /// An unnamed, never edited, empty document (can be replaced when opening a file).
    pub fn is_untouched(&self) -> bool {
        self.doc.path().is_none() && self.doc.version() == 0 && self.doc.text().len_chars() == 0
    }

    /// Saves under a new path and re-detects the language.
    pub fn save_as(&mut self, path: std::path::PathBuf) -> std::io::Result<()> {
        self.doc.save_as(path)?;
        self.syntax = self
            .doc
            .path()
            .and_then(language_for_path)
            .map(|cfg| Syntax::new(cfg, self.doc.text()));
        self.doc.take_edits();
        self.cache = None;
        Ok(())
    }

    pub fn is_csharp(&self) -> bool {
        self.language_name() == "C#"
    }

    /// Window position (left, top) of a char position, and the line height.
    pub fn screen_pos(&self, pos: usize) -> (f32, f32) {
        let rope = self.doc.text();
        let line = rope.char_to_line(pos.min(rope.len_chars()));
        let col = char_to_display_col(rope.line(line), pos - rope.line_to_char(line), self.doc.tab_width);
        let x = self.text_x() - self.scroll_x + col as f32 * self.font.cell_w;
        let y = self.area.y + (line as f32 - self.scroll_line as f32) * self.font.line_h;
        (x, y)
    }

    pub fn line_height(&self) -> f32 {
        self.font.line_h
    }

    /// The most severe problem touching the primary cursor.
    pub fn diagnostic_at_cursor(&self) -> Option<&DiagSpan> {
        let head = self.doc.selection().primary().head;
        self.diagnostics
            .iter()
            .filter(|d| d.start <= head && head <= d.end.max(d.start + 1))
            .min_by_key(|d| d.severity)
    }

    /// Start of the identifier the cursor is in or right after (for completion).
    pub fn word_start(&self) -> usize {
        let rope = self.doc.text();
        let mut i = self.doc.selection().primary().head;
        while i > 0 {
            let c = rope.char(i - 1);
            if c.is_alphanumeric() || c == '_' {
                i -= 1;
            } else {
                break;
            }
        }
        i
    }

    pub fn symbols(&self) -> Vec<wyvern_syntax::Symbol> {
        self.syntax
            .as_ref()
            .map_or_else(Vec::new, |s| s.symbols(self.doc.text()))
    }

    /// Puts the cursor at a char position and scrolls it to about a third of the view.
    pub fn jump_to(&mut self, pos: usize) {
        self.doc.set_cursor(pos.min(self.doc.text().len_chars()));
        let line = self.doc.text().char_to_line(self.doc.selection().primary().head);
        self.scroll_line = line.saturating_sub(self.full_rows() / 3);
        self.ensure_cursor_visible();
    }

    /// 1-based line and optional 1-based column, clamped to the document.
    pub fn jump_to_line(&mut self, line: usize, col: Option<usize>) {
        let rope = self.doc.text();
        let line = line.saturating_sub(1).min(rope.len_lines().saturating_sub(1));
        let len = line_len_chars(rope, line);
        let col = col.map_or(0, |c| c.saturating_sub(1)).min(len);
        let pos = rope.line_to_char(line) + col;
        self.jump_to(pos);
    }

    pub fn jump_to_byte(&mut self, byte: usize) {
        let rope = self.doc.text();
        let pos = rope.byte_to_char(byte.min(rope.len_bytes()));
        self.jump_to(pos);
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, font: FontMetrics) {
        self.font = font;
        let metrics = Metrics::new(font.size, font.line_h);
        for b in [&mut self.text_buf, &mut self.gutter_buf] {
            b.set_metrics(metrics);
            b.set_monospace_width(Some(font.cell_w));
        }
        let _ = fs;
        self.cache = None;
    }

    /// Visible line count (the last one may be cut off).
    pub fn rows(&self) -> usize {
        ((self.area.h / self.font.line_h).ceil() as usize).max(1)
    }

    fn full_rows(&self) -> usize {
        ((self.area.h / self.font.line_h).floor() as usize).max(1)
    }

    fn gutter_w(&self) -> f32 {
        (self.gutter_cells as f32 + 2.0) * self.font.cell_w
    }

    fn text_x(&self) -> f32 {
        self.area.x + self.gutter_w() + self.font.cell_w * 0.5
    }

    /// Call after every command that may have changed the text.
    pub fn after_edit(&mut self) {
        let edits = self.doc.take_edits();
        if let Some(syntax) = &mut self.syntax {
            syntax.update(self.doc.text(), &edits);
        }
        let max = self.doc.text().len_lines().saturating_sub(1);
        self.scroll_line = self.scroll_line.min(max);
    }

    pub fn page_lines(&self) -> usize {
        self.full_rows().saturating_sub(1).max(1)
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.doc.text().len_lines().saturating_sub(1) as isize;
        self.scroll_line = (self.scroll_line as isize + lines).clamp(0, max) as usize;
    }

    pub fn scroll_x_by(&mut self, dx: f32) {
        self.scroll_x = (self.scroll_x + dx).max(0.0);
    }

    /// Scrolls so the primary cursor is visible, with a little margin.
    pub fn ensure_cursor_visible(&mut self) {
        let rope = self.doc.text();
        let head = self.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        let rows = self.full_rows();
        let margin = 2.min(rows / 3);
        if line < self.scroll_line + margin {
            self.scroll_line = line.saturating_sub(margin);
        } else if line + margin >= self.scroll_line + rows {
            self.scroll_line = line + margin + 1 - rows;
        }

        let col = char_to_display_col(
            rope.line(line),
            head - rope.line_to_char(line),
            self.doc.tab_width,
        ) as f32;
        let x = col * self.font.cell_w;
        let visible = self.area.w - self.gutter_w() - self.font.cell_w;
        if x < self.scroll_x {
            self.scroll_x = (x - 4.0 * self.font.cell_w).max(0.0);
        } else if x > self.scroll_x + visible - 2.0 * self.font.cell_w {
            self.scroll_x = x - visible + 6.0 * self.font.cell_w;
        }
    }

    /// Text position under a window coordinate.
    pub fn hit_test(&self, x: f32, y: f32) -> usize {
        let rope = self.doc.text();
        let row = ((y - self.area.y) / self.font.line_h).floor().max(0.0) as usize;
        let line = (self.scroll_line + row).min(rope.len_lines().saturating_sub(1));
        let col = ((x - self.text_x() + self.scroll_x) / self.font.cell_w)
            .round()
            .max(0.0) as usize;
        rope.line_to_char(line) + display_col_to_char(rope.line(line), col, self.doc.tab_width)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.area.x && x < self.area.x + self.area.w && y >= self.area.y
            && y < self.area.y + self.area.h
    }

    /// Re-shapes the visible text if anything relevant changed.
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme) {
        let rope = self.doc.text();
        let digits = rope.len_lines().max(1).to_string().len().max(3);
        self.gutter_cells = digits;
        let current_line = rope.char_to_line(self.doc.selection().primary().head);
        let key = CacheKey {
            version: self.doc.version(),
            scroll_line: self.scroll_line,
            rows: self.rows(),
            current_line,
            font: (self.font.size.to_bits(), self.font.cell_w.to_bits()),
            gutter_cells: digits,
        };
        if self.cache.as_ref() == Some(&key) {
            return;
        }

        let first = self.scroll_line.min(rope.len_lines().saturating_sub(1));
        let last = (first + self.rows()).min(rope.len_lines());
        let start_byte = rope.line_to_byte(first);
        let end_byte = rope.line_to_byte(last);
        let spans = match &self.syntax {
            Some(s) => flatten(&s.highlights(rope, start_byte..end_byte), start_byte..end_byte),
            None => Vec::new(),
        };

        // Build colored runs for the visible lines; tabs become spaces so the grid holds.
        let tab_width = self.doc.tab_width;
        let mut runs: Vec<(String, Rgba)> = Vec::new();
        let mut push = |text: &str, color: Rgba| match runs.last_mut() {
            Some((t, c)) if *c == color => t.push_str(text),
            _ => runs.push((text.to_string(), color)),
        };
        let mut span_i = 0;
        for line in first..last {
            let mut byte = rope.line_to_byte(line);
            let mut col = 0;
            for ch in rope.line(line).chars() {
                if ch == '\n' {
                    break;
                }
                while span_i < spans.len() && spans[span_i].end <= byte {
                    span_i += 1;
                }
                let color = spans
                    .get(span_i)
                    .filter(|s| s.start <= byte)
                    .map_or(theme.text, |s| theme.highlight(s.highlight));
                if ch == '\t' {
                    let n = tab_width - col % tab_width;
                    push(&" ".repeat(n), color);
                    col += n;
                } else {
                    let mut tmp = [0u8; 4];
                    push(ch.encode_utf8(&mut tmp), color);
                    col += 1;
                }
                byte += ch.len_utf8();
            }
            push("\n", theme.text);
        }

        let family = Family::Name(&self.family);
        let base = Attrs::new().family(family);
        self.text_buf.set_rich_text(
            runs.iter()
                .map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Advanced,
            None,
        );
        self.text_buf.shape_until_scroll(fs, false);

        let mut gutter: Vec<(String, Rgba)> = Vec::new();
        for line in first..last {
            let color = if line == current_line { theme.gutter_current } else { theme.gutter_text };
            gutter.push((format!("{:>width$}\n", line + 1, width = digits), color));
        }
        self.gutter_buf.set_rich_text(
            gutter
                .iter()
                .map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Basic,
            None,
        );
        self.gutter_buf.shape_until_scroll(fs, false);
        self.cache = Some(key);
    }

    /// Adds the editor's rectangles and text to the frame.
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        under: &mut Vec<Quad>,
        over: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let a = self.area;
        let rope = self.doc.text();
        let lh = self.font.line_h;
        let cw = self.font.cell_w;
        let text_left = self.area.x + self.gutter_w();
        let text_x = self.text_x() - self.scroll_x;
        let first = self.scroll_line;
        let last = (first + self.rows()).min(rope.len_lines());
        let tab = self.doc.tab_width;
        let line_y = |line: usize| a.y + (line - first) as f32 * lh;
        let col_of = |pos: usize| {
            let line = rope.char_to_line(pos);
            (line, char_to_display_col(rope.line(line), pos - rope.line_to_char(line), tab))
        };
        // Horizontal clip of a rectangle to the text area.
        let clip = |x: f32, w: f32| -> Option<(f32, f32)> {
            let x0 = x.max(text_left);
            let x1 = (x + w).min(a.x + a.w);
            (x1 > x0).then_some((x0, x1 - x0))
        };

        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.background });

        let primary = self.doc.selection().primary();
        let (cur_line, _) = col_of(primary.head);
        if primary.is_empty() && (first..last).contains(&cur_line) {
            under.push(Quad {
                x: text_left,
                y: line_y(cur_line),
                w: a.w - (text_left - a.x),
                h: lh,
                color: theme.current_line,
            });
        }

        for r in self.doc.selection().ranges() {
            if r.is_empty() {
                continue;
            }
            let (l0, c0) = col_of(r.from());
            let (l1, c1) = col_of(r.to());
            for line in l0.max(first)..=l1.min(last.saturating_sub(1)) {
                let start = if line == l0 { c0 } else { 0 };
                let end = if line == l1 {
                    c1
                } else {
                    let len = line_len_chars(rope, line);
                    char_to_display_col(rope.line(line), len, tab) + 1
                };
                if let Some((x, w)) = clip(text_x + start as f32 * cw, (end - start) as f32 * cw) {
                    under.push(Quad { x, y: line_y(line), w, h: lh, color: theme.selection });
                }
            }
        }

        let cursor_w = (2.0 * scale).round().max(1.0);
        for r in self.doc.selection().ranges() {
            let (line, col) = col_of(r.head);
            if !(first..last).contains(&line) {
                continue;
            }
            if let Some((x, w)) = clip(text_x + col as f32 * cw - cursor_w / 2.0, cursor_w) {
                over.push(Quad { x, y: line_y(line), w, h: lh, color: theme.cursor });
            }
        }

        // Squiggles under problems: a thin zigzag made of short steps.
        let thick = scale.round().max(1.0);
        let step = (2.0 * scale).round().max(2.0);
        for d in &self.diagnostics {
            let color = match d.severity {
                1 => theme.error,
                2 => theme.warning,
                _ => theme.info,
            };
            let (l0, c0) = col_of(d.start);
            let (l1, c1) = col_of(d.end.max(d.start + 1).min(rope.len_chars()));
            for line in l0.max(first)..=l1.min(last.saturating_sub(1)) {
                let s = if line == l0 { c0 } else { 0 };
                let e = if line == l1 {
                    c1.max(s + 1)
                } else {
                    char_to_display_col(rope.line(line), line_len_chars(rope, line), tab)
                };
                let base = line_y(line) + lh - 2.0 * step;
                let x0 = text_x + s as f32 * cw;
                let x1 = text_x + e as f32 * cw;
                let mut x = x0;
                let mut up = false;
                while x < x1 {
                    let w = step.min(x1 - x);
                    if let Some((cx, cw2)) = clip(x, w) {
                        let y = if up { base } else { base + step - thick };
                        over.push(Quad { x: cx, y, w: cw2, h: thick, color });
                        // Vertical connector so it reads as a wave.
                        over.push(Quad { x: cx, y: base, w: thick, h: step, color });
                    }
                    x += step;
                    up = !up;
                }
            }
        }

        let bottom = (a.y + a.h) as i32;
        text.push(TextBlock {
            buffer: &self.gutter_buf,
            x: a.x + cw,
            y: a.y,
            color: theme.gutter_text,
            clip: Some([a.x as i32, a.y as i32, text_left as i32, bottom]),
        });
        text.push(TextBlock {
            buffer: &self.text_buf,
            x: text_x,
            y: a.y,
            color: theme.text,
            clip: Some([text_left as i32, a.y as i32, (a.x + a.w) as i32, bottom]),
        });
    }
}
