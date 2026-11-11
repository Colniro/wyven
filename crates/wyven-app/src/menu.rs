//! Small popups at the cursor: a pick list (quick actions) and a one-line input (rename).

use wyven_lsp::Value;
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

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

    /// Selects item `i` and scrolls it into view.
    pub fn select(&mut self, i: usize) {
        if i < self.items.len() {
            self.selected = i;
            self.scroll = i.saturating_sub(ROWS / 2).min(self.items.len().saturating_sub(ROWS));
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
    /// Shows dots instead of the text (API keys).
    pub masked: bool,
    fresh: bool,
    buffer: Buffer,
    /// Only the text (for the dialog card and inline fields).
    line: Buffer,
    title_buf: Buffer,
}

impl Prompt {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, title: &str, text: &str) -> Self {
        Self {
            title: title.into(),
            text: text.into(),
            fresh: true,
            buffer: line_buffer(fs, font),
            line: line_buffer(fs, font),
            title_buf: line_buffer(fs, font),
            masked: false,
        }
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
            (if self.masked { "•".repeat(self.text.chars().count()) } else { self.text.clone() }, theme.tab_text_active),
        ];
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
        let shown = if self.masked { "•".repeat(self.text.chars().count()) } else { self.text.clone() };
        self.line.set_text(&shown, &attrs, Shaping::Advanced, None);
        self.line.shape_until_scroll(fs, false);
        let bold = attrs.clone().weight(wyven_render::Weight::SEMIBOLD);
        self.title_buf.set_text(&self.title, &bold, Shaping::Advanced, None);
        self.title_buf.shape_until_scroll(fs, false);
    }

    fn text_w(&self) -> f32 {
        self.line.layout_runs().next().map_or(0.0, |r| r.line_w)
    }

    /// An input field: rounded box, the text, a selection while it is untouched, the cursor.
    #[allow(clippy::too_many_arguments)]
    fn field<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        r: (f32, f32, f32, f32),
        scale: f32,
        cards: &mut Vec<wyven_render::RoundRect>,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let (x, y, w, h) = r;
        let radius = (5.0 * scale).round();
        cards.push(wyven_render::RoundRect::new(x, y, w, h, theme.accent, radius));
        cards.push(wyven_render::RoundRect::new(x + 1.5 * scale, y + 1.5 * scale, w - 3.0 * scale, h - 3.0 * scale, theme.background, radius - 1.0));
        let pad = font.cell_w * 0.6;
        let ty = y + (h - font.line_h) / 2.0;
        let tw = self.text_w();
        if self.fresh && !self.text.is_empty() {
            quads.push(Quad { x: x + pad, y: ty, w: tw, h: font.line_h, color: theme.selection });
        }
        quads.push(Quad { x: x + pad + tw, y: ty, w: (2.0 * scale).round(), h: font.line_h, color: theme.cursor });
        text.push(TextBlock {
            buffer: &self.line,
            x: x + pad,
            y: ty,
            color: theme.tab_text_active,
            clip: Some([x as i32, y as i32, (x + w) as i32, (y + h) as i32]),
        });
    }

    /// The input as a dialog card in the upper middle of the window (new branch, watch, …).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_dialog<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        window: (f32, f32),
        scale: f32,
        cards: &mut Vec<wyven_render::RoundRect>,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let pad = font.cell_w * 2.0;
        let title_w = self.title_buf.layout_runs().next().map_or(0.0, |r| r.line_w);
        let w = (title_w + 2.0 * pad).max(font.cell_w * 56.0).min(window.0 - 40.0);
        let field_h = (font.line_h * 1.6).round();
        let h = pad + font.line_h * 1.3 + field_h + pad;
        let x = ((window.0 - w) / 2.0).round();
        let y = (window.1 * 0.22).round();
        let radius = (10.0 * scale).round();
        let shadow = wyven_render::Rgba(0, 0, 0, if theme.dark { 0x99 } else { 0x40 });
        cards.push(wyven_render::RoundRect::shadow(x, y, w, h, shadow, radius, (16.0 * scale).round()));
        cards.push(wyven_render::RoundRect::new(x, y, w, h, theme.popup_border, radius));
        cards.push(wyven_render::RoundRect::new(x + 1.0, y + 1.0, w - 2.0, h - 2.0, theme.popup_bg, radius - 1.0));
        text.push(TextBlock { buffer: &self.title_buf, x: x + pad, y: y + pad * 0.7, color: theme.panel_text, clip: None });
        let fy = y + pad * 0.7 + font.line_h * 1.4;
        self.field(theme, font, (x + pad, fy, w - 2.0 * pad, field_h), scale, cards, quads, text);
    }

    /// The input right in place (a row of the explorer): `r` is the row.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_inline<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        r: (f32, f32, f32, f32),
        scale: f32,
        cards: &mut Vec<wyven_render::RoundRect>,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        self.field(theme, font, r, scale, cards, quads, text);
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
