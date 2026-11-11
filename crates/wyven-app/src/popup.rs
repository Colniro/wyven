//! A small floating box with a title and "key  label" lines (leader menu, recent projects).

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Wrap};

use crate::editor::FontMetrics;
use crate::theme::Theme;

pub struct Popup {
    buffer: Buffer,
    lines: usize,
    width_cells: usize,
}

impl Popup {
    pub fn new(fs: &mut FontSystem, font: FontMetrics) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        buffer.set_wrap(Wrap::None);
        Self { buffer, lines: 0, width_cells: 0 }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.buffer.set_metrics(Metrics::new(font.size, font.line_h));
    }

    /// Fills the box. `entries` are (key, label) pairs.
    pub fn set(
        &mut self,
        fs: &mut FontSystem,
        theme: &Theme,
        family: &str,
        title: &str,
        entries: &[(String, String)],
    ) {
        let attrs = Attrs::new().family(Family::Name(family));
        let mut spans: Vec<(String, wyven_render::Rgba)> = vec![(format!("{title}\n"), theme.panel_dim)];
        let key_w = entries.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(1);
        let mut width = title.chars().count();
        for (key, label) in entries {
            spans.push((format!("{key:<key_w$}  "), theme.popup_key));
            spans.push((format!("{label}\n"), theme.panel_text));
            width = width.max(key_w + 2 + label.chars().count());
        }
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
        self.lines = entries.len() + 1;
        self.width_cells = width;
    }

    /// Draws the box anchored to the bottom-right corner of the given area.
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        right: f32,
        bottom: f32,
        under: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let pad = font.cell_w * 1.5;
        let w = self.width_cells as f32 * font.cell_w + 2.0 * pad;
        let h = self.lines as f32 * font.line_h + 2.0 * pad * 0.6;
        let x = right - w - font.cell_w * 2.0;
        let y = bottom - h - font.line_h * 0.5;
        let b = (font.size / 10.0).max(1.0).round();
        under.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: theme.popup_border });
        under.push(Quad { x, y, w, h, color: theme.popup_bg });
        text.push(TextBlock {
            buffer: &self.buffer,
            x: x + pad,
            y: y + pad * 0.6,
            color: theme.panel_text,
            clip: None,
        });
    }
}
