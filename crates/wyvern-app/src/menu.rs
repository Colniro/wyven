//! Small popups at the cursor: a pick list (quick actions) and a one-line input (rename).

use wyvern_lsp::Value;
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::FontMetrics;
use crate::theme::Theme;

const ROWS: usize = 12;

fn line_buffer(fs: &mut FontSystem, font: FontMetrics) -> Buffer {
    let mut b = Buffer::new(fs, Metrics::new(font.size, font.line_h));
    b.set_wrap(Wrap::None);
    b
}

fn frame(theme: &Theme, x: f32, y: f32, w: f32, h: f32, scale: f32, quads: &mut Vec<Quad>) {
    let b = scale.round().max(1.0);
    quads.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: theme.popup_border });
    quads.push(Quad { x, y, w, h, color: theme.popup_bg });
}

/// A list to pick from with arrows, Enter or the digits 1–9.
pub struct Menu {
    title: String,
    pub items: Vec<(String, Value)>,
    selected: usize,
    scroll: usize,
    buffer: Buffer,
    width_cells: usize,
}

impl Menu {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, title: &str, items: Vec<(String, Value)>) -> Self {
        Self { title: title.into(), items, selected: 0, scroll: 0, buffer: line_buffer(fs, font), width_cells: 0 }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + ROWS {
            self.scroll = self.selected + 1 - ROWS;
        }
    }

    pub fn selected(&self) -> Option<Value> {
        self.items.get(self.selected).map(|(_, v)| v.clone())
    }

    /// Item for digit `n` (1-based, as shown in the list).
    pub fn numbered(&self, n: usize) -> Option<Value> {
        n.checked_sub(1).and_then(|i| self.items.get(self.scroll + i)).map(|(_, v)| v.clone())
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, family: &str) {
        let attrs = Attrs::new().family(Family::Name(family));
        let mut spans: Vec<(String, Rgba)> = vec![(format!("{}\n", self.title), theme.panel_dim)];
        let mut width = self.title.chars().count();
        for (i, (label, _)) in self.items.iter().enumerate().skip(self.scroll).take(ROWS) {
            let n = i - self.scroll + 1;
            let key = if n <= 9 { format!("{n} ") } else { "  ".into() };
            let label: String = label.chars().take(90).collect();
            width = width.max(label.chars().count() + 2);
            spans.push((key, theme.popup_key));
            spans.push((format!("{label}\n"), theme.panel_text));
        }
        self.width_cells = width;
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        x: f32,
        y: f32,
        window: (f32, f32),
        scale: f32,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let rows = self.items.len().min(ROWS) + 1;
        let pad = font.cell_w;
        let w = self.width_cells as f32 * font.cell_w + 2.0 * pad;
        let h = rows as f32 * font.line_h + pad * 0.5;
        let x = x.min(window.0 - w - 4.0).max(0.0);
        let y = if y + h > window.1 { (y - h - font.line_h * 1.5).max(0.0) } else { y };
        frame(theme, x, y, w, h, scale, quads);
        if !self.items.is_empty() {
            let sy = y + (1 + self.selected - self.scroll) as f32 * font.line_h;
            quads.push(Quad { x, y: sy, w, h: font.line_h, color: theme.row_selected });
        }
        text.push(TextBlock { buffer: &self.buffer, x: x + pad, y, color: theme.panel_text, clip: None });
    }
}

/// A one-line text input with a title (rename). The initial text is selected: typing replaces it.
pub struct Prompt {
    title: String,
    pub text: String,
    fresh: bool,
    buffer: Buffer,
}

impl Prompt {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, title: &str, text: &str) -> Self {
        Self { title: title.into(), text: text.into(), fresh: true, buffer: line_buffer(fs, font) }
    }

    pub fn type_text(&mut self, s: &str) {
        if self.fresh {
            self.text.clear();
            self.fresh = false;
        }
        self.text.push_str(s);
    }

    pub fn backspace(&mut self, word: bool) {
        if self.fresh || word {
            self.text.clear();
        } else {
            self.text.pop();
        }
        self.fresh = false;
    }

    /// Arrow keys keep the text and start editing at its end.
    pub fn keep(&mut self) {
        self.fresh = false;
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, family: &str) {
        let attrs = Attrs::new().family(Family::Name(family));
        let spans = [
            (format!("{}\n", self.title), theme.panel_dim),
            (self.text.clone(), theme.tab_text_active),
        ];
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        x: f32,
        y: f32,
        window: (f32, f32),
        scale: f32,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let pad = font.cell_w;
        let cells = self.title.chars().count().max(self.text.chars().count() + 2).max(30);
        let w = cells as f32 * font.cell_w + 2.0 * pad;
        let h = 2.0 * font.line_h + pad * 0.5;
        let x = x.min(window.0 - w - 4.0).max(0.0);
        let y = if y + h > window.1 { (y - h - font.line_h * 1.5).max(0.0) } else { y };
        frame(theme, x, y, w, h, scale, quads);
        let input_y = y + font.line_h;
        let text_w = self.text.chars().count() as f32 * font.cell_w;
        if self.fresh && !self.text.is_empty() {
            quads.push(Quad { x: x + pad, y: input_y, w: text_w, h: font.line_h, color: theme.selection });
        }
        quads.push(Quad {
            x: x + pad + text_w,
            y: input_y,
            w: (2.0 * scale).round(),
            h: font.line_h,
            color: theme.cursor,
        });
        text.push(TextBlock { buffer: &self.buffer, x: x + pad, y, color: theme.panel_text, clip: None });
    }
}
