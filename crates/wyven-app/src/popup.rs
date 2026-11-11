//! The key popup at the bottom right (leader menu, recent projects): a rounded card with a
//! colored title, keys as small key caps, labels in up to two columns, a line between the
//! sub-menus and the commands.

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, RoundRect, Shaping, TextBlock, Weight, Wrap};

use crate::editor::FontMetrics;
use crate::theme::Theme;

/// One line of the popup.
pub struct PopupEntry {
    pub key: String,
    pub label: String,
    /// Opens a further menu (shown first, with "›").
    pub group: bool,
}

struct Line {
    key: Buffer,
    key_w: f32,
    label: Buffer,
    group: bool,
}

pub struct Popup {
    title: Buffer,
    lines: Vec<Line>,
    color: Rgba,
    font: FontMetrics,
    family: String,
    /// Columns and rows of the grid, the widest label and key.
    cols: usize,
    rows_groups: usize,
    rows_cmds: usize,
    label_w: f32,
    title_w: f32,
}

const MAX_ROWS: usize = 9;

impl Popup {
    pub fn new(fs: &mut FontSystem, font: FontMetrics) -> Self {
        let mut title = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        title.set_wrap(Wrap::None);
        Self {
            title,
            lines: Vec::new(),
            color: Rgba::rgb(0xE0, 0xB4, 0x6C),
            font,
            family: String::new(),
            cols: 1,
            rows_groups: 0,
            rows_cmds: 0,
            label_w: 0.0,
            title_w: 0.0,
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.font = font;
        self.title.set_metrics(Metrics::new(font.size, font.line_h));
    }

    /// Fills the card: a title in `color` and the entries (sub-menus first).
    pub fn set(&mut self, fs: &mut FontSystem, family: &str, title: &str, color: Rgba, entries: &[PopupEntry]) {
        self.family = family.to_string();
        self.color = color;
        let bold = Attrs::new().family(Family::Name(family)).weight(Weight::SEMIBOLD);
        let attrs = Attrs::new().family(Family::Name(family));
        self.title.set_text(title, &bold, Shaping::Advanced, None);
        self.title.shape_until_scroll(fs, false);
        self.title_w = self.title.layout_runs().next().map_or(0.0, |r| r.line_w);
        let metrics = Metrics::new(self.font.size * 0.95, self.font.line_h);
        let mut make = |t: &str, a: &Attrs| {
            let mut b = Buffer::new(fs, metrics);
            b.set_wrap(Wrap::None);
            b.set_text(t, a, Shaping::Advanced, None);
            b.shape_until_scroll(fs, false);
            let w = b.layout_runs().next().map_or(0.0, |r| r.line_w);
            (b, w)
        };
        let mut sorted: Vec<&PopupEntry> = entries.iter().filter(|e| e.group).collect();
        sorted.extend(entries.iter().filter(|e| !e.group));
        self.lines.clear();
        self.label_w = 0.0;
        for e in sorted {
            let (key, key_w) = make(&e.key, &bold);
            let text = if e.group { format!("{} ›", e.label) } else { e.label.clone() };
            let (label, w) = make(&text, &attrs);
            self.label_w = self.label_w.max(w);
            self.lines.push(Line { key, key_w, label, group: e.group });
        }
        let groups = self.lines.iter().filter(|l| l.group).count();
        let cmds = self.lines.len() - groups;
        // Long lists get two columns.
        self.cols = if groups.max(cmds) > MAX_ROWS { 2 } else { 1 };
        self.rows_groups = groups.div_ceil(self.cols);
        self.rows_cmds = cmds.div_ceil(self.cols);
    }

    fn cell(&self) -> (f32, f32) {
        let cap = (self.font.line_h * 1.1).round();
        (cap + self.font.cell_w * 1.2 + self.label_w, (self.font.line_h * 1.35).round())
    }

    /// Size of the card.
    fn size(&self) -> (f32, f32) {
        let pad = self.font.cell_w * 1.6;
        let (cw, rh) = self.cell();
        let gap = self.font.cell_w * 3.0;
        let w = (self.cols as f32 * cw + (self.cols as f32 - 1.0) * gap).max(self.title_w) + 2.0 * pad;
        let sep = if self.rows_groups > 0 && self.rows_cmds > 0 { self.font.line_h * 0.7 } else { 0.0 };
        let h = pad + self.font.line_h * 1.6 + (self.rows_groups + self.rows_cmds) as f32 * rh + sep + pad * 0.7;
        (w, h)
    }

    /// Draws the card anchored to the bottom-right corner (`right`, `bottom`).
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        right: f32,
        bottom: f32,
        cards: &mut Vec<RoundRect>,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let (w, h) = self.size();
        let pad = self.font.cell_w * 1.6;
        let x = right - w - self.font.cell_w * 2.0;
        let y = bottom - h - self.font.line_h * 0.6;
        let radius = (10.0 * scale).round();
        let shadow = Rgba(0, 0, 0, if theme.dark { 0x99 } else { 0x40 });
        cards.push(RoundRect::shadow(x, y, w, h, shadow, radius, (14.0 * scale).round()));
        cards.push(RoundRect::new(x, y, w, h, theme.popup_border, radius));
        cards.push(RoundRect::new(x + 1.0, y + 1.0, w - 2.0, h - 2.0, theme.popup_bg, radius - 1.0));
        // A colored band at the top edge, the title, a line.
        cards.push(RoundRect::new(x + pad, y + (4.0 * scale).round(), (28.0 * scale).round(), (3.0 * scale).round(), self.color, 2.0 * scale));
        let ty = y + pad * 0.7;
        text.push(TextBlock { buffer: &self.title, x: x + pad, y: ty, color: self.color, clip: None });
        let line_y = (ty + self.font.line_h * 1.25).round();
        quads.push(Quad { x: x + pad, y: line_y, w: w - 2.0 * pad, h: 1.0, color: theme.divider });

        let (cw, rh) = self.cell();
        let gap = self.font.cell_w * 3.0;
        let cap = (self.font.line_h * 1.1).round();
        let top = line_y + self.font.line_h * 0.35;
        let groups: Vec<&Line> = self.lines.iter().filter(|l| l.group).collect();
        let cmds: Vec<&Line> = self.lines.iter().filter(|l| !l.group).collect();
        let sep = if !groups.is_empty() && !cmds.is_empty() { self.font.line_h * 0.7 } else { 0.0 };
        let sections = [(groups, top, self.rows_groups), (cmds, top + self.rows_groups as f32 * rh + sep, self.rows_cmds)];
        if sep > 0.0 {
            let sy = (top + self.rows_groups as f32 * rh + sep / 2.0).round();
            quads.push(Quad { x: x + pad, y: sy, w: w - 2.0 * pad, h: 1.0, color: theme.divider });
        }
        for (lines, y0, rows) in sections {
            for (i, l) in lines.iter().enumerate() {
                let (col, row) = i.checked_div(rows).map_or((0, i), |c| (c, i % rows));
                let cx = x + pad + col as f32 * (cw + gap);
                let cy = y0 + row as f32 * rh;
                // The key as a small key cap in the group's color.
                let cap_y = cy + (rh - cap) / 2.0;
                cards.push(RoundRect::new(cx, cap_y, cap, cap, self.color.with_alpha(if l.group { 0x55 } else { 0x2C }), (5.0 * scale).round()));
                let ky = cy + (rh - self.font.line_h) / 2.0;
                text.push(TextBlock { buffer: &l.key, x: (cx + (cap - l.key_w) / 2.0).round(), y: ky, color: self.color, clip: None });
                text.push(TextBlock {
                    buffer: &l.label,
                    x: cx + cap + self.font.cell_w * 1.0,
                    y: ky,
                    color: if l.group { theme.panel_text } else { theme.panel_text.with_alpha(0xE0) },
                    clip: None,
                });
            }
        }
    }
}
