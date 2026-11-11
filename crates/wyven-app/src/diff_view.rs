//! Side-by-side diff (like Visual Studio): left the last committed version, right the current
//! text. Both sides scroll together; filler lines keep them aligned.

use wyven_git::{side_by_side, DiffRow, RowKind};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Wrap};

use crate::editor::{EditorView, FontMetrics, Rect};
use crate::theme::Theme;

pub struct DiffView {
    pub title: String,
    /// Header text after the title (what the sides are, which keys work).
    pub note: String,
    /// The Claude Code diff tab this view shows, if it is one.
    pub claude_tab: Option<String>,
    left: EditorView,
    right: EditorView,
    rows: Vec<DiffRow>,
    area: Rect,
    header: Buffer,
    ui_font: FontMetrics,
    family: String,
}

impl DiffView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fs: &mut FontSystem,
        font: FontMetrics,
        ui_font: FontMetrics,
        family: String,
        theme: &Theme,
        title: &str,
        old: &str,
        new: &str,
    ) -> Self {
        let rows = side_by_side(old, new);
        let old_lines: Vec<&str> = old.split('\n').collect();
        let new_lines: Vec<&str> = new.split('\n').collect();
        let side = |pick_old: bool| {
            let mut text = String::new();
            let mut labels = Vec::with_capacity(rows.len());
            let mut bg = Vec::with_capacity(rows.len());
            for row in &rows {
                let (line, lines) = if pick_old { (row.old, &old_lines) } else { (row.new, &new_lines) };
                match line {
                    Some(l) => {
                        text.push_str(lines.get(l).copied().unwrap_or(""));
                        labels.push((l + 1).to_string());
                        bg.push(match (row.kind, pick_old) {
                            (RowKind::Same, _) => None,
                            (_, true) => Some(theme.diff_removed),
                            (_, false) => Some(theme.diff_added),
                        });
                    }
                    None => {
                        labels.push(String::new());
                        bg.push(Some(theme.diff_filler));
                    }
                }
                text.push('\n');
            }
            text.pop();
            (text, labels, bg)
        };
        let make = |fs: &mut FontSystem, pick_old: bool| {
            let (text, labels, bg) = side(pick_old);
            let mut v = EditorView::virtual_doc(title, &text, title, fs, font, family.clone());
            v.gutter_labels = Some(labels);
            v.line_bg = bg;
            v.show_cursor = false;
            v
        };
        let left = make(fs, true);
        let right = make(fs, false);
        let mut header = Buffer::new(fs, Metrics::new(ui_font.size, ui_font.line_h));
        header.set_wrap(Wrap::None);
        let note = "left: last commit   ·   right: current".to_string();
        Self { title: title.to_string(), note, claude_tab: None, left, right, rows, area: Rect::default(), header, ui_font, family }
    }

    pub fn changes(&self) -> usize {
        // Count blocks of consecutive changed rows.
        let mut n = 0;
        let mut prev_same = true;
        for r in &self.rows {
            let same = r.kind == RowKind::Same;
            if !same && prev_same {
                n += 1;
            }
            prev_same = same;
        }
        n
    }

    fn header_h(&self) -> f32 {
        (self.ui_font.line_h * 1.4).round()
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
        let h = self.header_h();
        let half = (area.w / 2.0).floor();
        self.left.set_area(Rect { x: area.x, y: area.y + h, w: half - 1.0, h: area.h - h });
        self.right.set_area(Rect { x: area.x + half, y: area.y + h, w: area.w - half, h: area.h - h });
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    pub fn scroll_by(&mut self, lines: isize) {
        self.left.scroll_by(lines);
        self.right.scroll_line = self.left.scroll_line;
    }

    pub fn page(&self) -> isize {
        self.left.page_lines() as isize
    }

    pub fn scroll_to(&mut self, line: usize) {
        self.left.scroll_line = 0;
        self.scroll_by(line as isize);
    }

    /// Scrolls to the next (or previous) block of changes. Returns false if there is none.
    pub fn jump_change(&mut self, forward: bool) -> bool {
        let here = self.left.scroll_line + 2;
        let starts: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(i, r)| r.kind != RowKind::Same && (*i == 0 || self.rows[i - 1].kind == RowKind::Same))
            .map(|(i, _)| i)
            .collect();
        let target = if forward {
            starts.iter().find(|&&s| s > here).or(starts.first())
        } else {
            starts.iter().rev().find(|&&s| s + 1 < here).or(starts.last())
        };
        match target {
            Some(&t) => {
                self.scroll_to(t.saturating_sub(2));
                true
            }
            None => false,
        }
    }

    /// Re-render both sides (theme changed).
    pub fn invalidate(&mut self) {
        self.left.invalidate();
        self.right.invalidate();
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let n = self.changes();
        let text = format!(
            "{}   ·   {}   ·   {n} change{}   ·   F8 / Shift+F8 jump{}",
            self.title,
            self.note,
            if n == 1 { "" } else { "s" },
            if self.claude_tab.is_some() { "" } else { ", Esc closes" }
        );
        self.header.set_text(&text, &attrs.color(theme.panel_text.to_glyphon()), Shaping::Advanced, None);
        self.header.shape_until_scroll(fs, false);
        self.left.prepare(fs, theme);
        self.right.prepare(fs, theme);
    }

    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        under: &mut Vec<Quad>,
        over: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let a = self.area;
        let h = self.header_h();
        under.push(Quad { x: a.x, y: a.y, w: a.w, h, color: theme.tab_bar });
        text.push(TextBlock {
            buffer: &self.header,
            x: a.x + self.ui_font.cell_w,
            y: a.y + (h - self.ui_font.line_h) / 2.0,
            color: theme.panel_text,
            clip: Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + h) as i32]),
        });
        self.left.draw(theme, scale, under, over, text);
        self.right.draw(theme, scale, under, over, text);
        let mid = a.x + (a.w / 2.0).floor() - 1.0;
        over.push(Quad { x: mid, y: a.y + h, w: (scale).round().max(1.0), h: a.h - h, color: theme.divider });
    }
}
