//! The autocomplete list under the cursor.
//!
//! The server is asked once when the list opens; while typing, the list is filtered locally
//! (fuzzy), so it reacts instantly.

use std::path::PathBuf;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::FontMetrics;
use crate::lsp::CompletionItem;
use crate::theme::Theme;

const ROWS: usize = 10;

pub struct Completion {
    pub path: PathBuf,
    /// Start of the word being completed (char position).
    pub anchor: usize,
    items: Vec<CompletionItem>,
    /// Indices into `items` matching the typed prefix, with hit positions in the label.
    shown: Vec<(usize, Vec<u32>)>,
    selected: usize,
    scroll: usize,
    matcher: Matcher,
    buffer: Buffer,
    width_cells: usize,
}

impl Completion {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, path: PathBuf, anchor: usize, items: Vec<CompletionItem>) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        buffer.set_wrap(Wrap::None);
        let mut items = items;
        items.sort_by(|a, b| a.sort.cmp(&b.sort).then_with(|| a.label.cmp(&b.label)));
        Self {
            path,
            anchor,
            items,
            shown: Vec::new(),
            selected: 0,
            scroll: 0,
            matcher: Matcher::new(Config::DEFAULT),
            buffer,
            width_cells: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// Filters by the text typed since the list opened.
    pub fn filter(&mut self, prefix: &str) {
        self.shown.clear();
        if prefix.is_empty() {
            self.shown = (0..self.items.len()).map(|i| (i, Vec::new())).collect();
        } else {
            let pattern = Pattern::parse(prefix, CaseMatching::Smart, Normalization::Smart);
            let mut buf = Vec::new();
            let mut scored: Vec<(usize, u32)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| {
                    let s = pattern.score(Utf32Str::new(&it.filter, &mut buf), &mut self.matcher)?;
                    // Exact prefix matches first, like Visual Studio.
                    let (f, p) = (it.filter.to_lowercase(), prefix.to_lowercase());
                    // Exactly the typed word first (a snippet before the keyword), then prefixes.
                    let bonus = if f == p {
                        if it.snippet { 4000 } else { 3000 }
                    } else if f.starts_with(&p) {
                        1000
                    } else {
                        0
                    };
                    Some((i, s + bonus))
                })
                .collect();
            scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| self.items[a.0].sort.cmp(&self.items[b.0].sort)));
            for (i, _) in scored {
                let mut hits = Vec::new();
                pattern.indices(Utf32Str::new(&self.items[i].label, &mut buf), &mut self.matcher, &mut hits);
                hits.sort_unstable();
                hits.dedup();
                self.shown.push((i, hits));
            }
        }
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.shown.is_empty() {
            return;
        }
        let n = self.shown.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + ROWS {
            self.scroll = self.selected + 1 - ROWS;
        }
    }

    /// The selected item.
    pub fn selected(&self) -> Option<&CompletionItem> {
        self.shown.get(self.selected).map(|(i, _)| &self.items[*i])
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, family: &str) {
        let attrs = Attrs::new().family(Family::Name(family));
        let rows: Vec<&(usize, Vec<u32>)> = self.shown.iter().skip(self.scroll).take(ROWS).collect();
        let label_w = rows.iter().map(|(i, _)| self.items[*i].label.chars().count()).max().unwrap_or(10).min(40);
        let detail_w = rows.iter().map(|(i, _)| self.items[*i].detail.chars().count()).max().unwrap_or(0).min(36);
        self.width_cells = label_w + if detail_w > 0 { detail_w + 3 } else { 0 };
        let mut spans: Vec<(String, Rgba)> = Vec::new();
        for (i, hits) in rows {
            let it = &self.items[*i];
            for (ci, ch) in it.label.chars().take(label_w).enumerate() {
                let color = if hits.binary_search(&(ci as u32)).is_ok() { theme.accent } else { theme.panel_text };
                match spans.last_mut() {
                    Some((t, c)) if *c == color => t.push(ch),
                    _ => spans.push((ch.to_string(), color)),
                }
            }
            let used = it.label.chars().count().min(label_w);
            let detail: String = it.detail.chars().take(detail_w).collect();
            let pad = label_w - used + if detail_w > 0 { 3 } else { 0 };
            spans.push((format!("{}{detail}\n", " ".repeat(pad)), theme.panel_dim));
        }
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
    }

    /// Draws the list with its top-left corner at (x, y); flips above `y_above` if there is no
    /// room below the cursor line.
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        x: f32,
        y: f32,
        y_above: f32,
        window: (f32, f32),
        scale: f32,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let rows = self.shown.len().clamp(1, ROWS);
        let pad = font.cell_w * 0.75;
        let w = self.width_cells as f32 * font.cell_w + 2.0 * pad;
        let h = rows as f32 * font.line_h;
        let x = x.min(window.0 - w - 4.0).max(0.0);
        let y = if y + h > window.1 { (y_above - h).max(0.0) } else { y };
        let b = scale.round().max(1.0);
        quads.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: theme.popup_border });
        quads.push(Quad { x, y, w, h, color: theme.popup_bg });
        if self.selected >= self.scroll {
            let sy = y + (self.selected - self.scroll) as f32 * font.line_h;
            quads.push(Quad { x, y: sy, w, h: font.line_h, color: theme.row_selected });
        }
        text.push(TextBlock {
            buffer: &self.buffer,
            x: x + pad,
            y,
            color: theme.panel_text,
            clip: Some([x as i32, y as i32, (x + w) as i32, (y + h) as i32]),
        });
    }
}

/// A tooltip with wrapped text (hover information).
pub struct Tooltip {
    buffer: Buffer,
    pub x: f32,
    pub y: f32,
    size: (f32, f32),
}

impl Tooltip {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: &str, theme: &Theme, text: &str, x: f32, y: f32) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        let max_w = 80.0 * font.cell_w;
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(max_w), None);
        let attrs = Attrs::new().family(Family::Name(family)).color(theme.panel_text.to_glyphon());
        let text: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
        buffer.set_text(&text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(fs, false);
        let (mut w, mut lines) = (0.0f32, 0usize);
        for run in buffer.layout_runs() {
            w = w.max(run.line_w);
            lines += 1;
        }
        Self { buffer, x, y, size: (w, lines.max(1) as f32 * font.line_h) }
    }

    pub fn draw<'a>(&'a self, theme: &Theme, font: FontMetrics, window: (f32, f32), scale: f32, quads: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let pad = font.cell_w;
        let (w, h) = (self.size.0 + 2.0 * pad, self.size.1 + pad);
        let x = self.x.min(window.0 - w - 4.0).max(0.0);
        // Above the mouse if possible, so the hovered word stays visible.
        let y = if self.y - h - font.line_h * 0.5 > 0.0 { self.y - h - font.line_h * 0.5 } else { self.y + font.line_h };
        let b = scale.round().max(1.0);
        quads.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: theme.popup_border });
        quads.push(Quad { x, y, w, h, color: theme.popup_bg });
        text.push(TextBlock { buffer: &self.buffer, x: x + pad, y: y + pad * 0.5, color: theme.panel_text, clip: None });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, sort: &str, snippet: bool) -> CompletionItem {
        CompletionItem { label: label.into(), detail: String::new(), insert: label.into(), filter: label.into(), sort: sort.into(), snippet }
    }

    #[test]
    fn the_exact_word_comes_first_and_its_snippet_before_the_keyword() {
        let mut fs = FontSystem::new();
        let font = FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let items = vec![item("IFormatProvider", "a", false), item("if", "b", false), item("ifelse", "~ifelse", true), item("if", "~if", true)];
        let mut c = Completion::new(&mut fs, font, PathBuf::new(), 0, items);
        c.filter("if");
        let first = c.selected().unwrap();
        assert!(first.label == "if" && first.snippet);
        c.move_selection(1);
        assert!(c.selected().is_some_and(|i| i.label == "if" && !i.snippet));
    }
}
