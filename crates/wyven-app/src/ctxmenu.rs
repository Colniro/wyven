//! Right-click menus: a rounded card with a shadow, one row per command with its shortcut on the
//! right, thin lines between groups. Mouse (hover, click) and keys (↑ ↓ Enter Esc) work.

use std::path::PathBuf;

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, RoundRect, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::keymap::Command;
use crate::theme::Theme;

/// What a menu row does.
#[derive(Debug, Clone, PartialEq)]
pub enum CtxAction {
    /// In a folder of the explorer (the row's folder, or the file's folder).
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
    /// Shows the file in Windows' File Explorer.
    RevealInOs(PathBuf),
    OpenTerminalHere(PathBuf),
    RemoveRoot(PathBuf),
    RefreshExplorer,
    CollapseExplorer,
    CloseTab(usize),
    CloseOthers(usize),
    CloseToRight(usize),
    CloseAll,
    RevealInExplorer(PathBuf),
    SplitRight,
    SplitDown,
    /// An editor command (cut, go to definition, …).
    Run(Command),
}

enum Row {
    Item { label: Box<Buffer>, keys: Box<Buffer>, action: CtxAction },
    Separator,
}

pub struct ContextMenu {
    rows: Vec<Row>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    row_h: f32,
    sep_h: f32,
    keys_x: f32,
    pub hover: Option<usize>,
    font: FontMetrics,
}

/// Builds a menu: `None` is a separator line, `Some((label, keys, action))` a row.
pub struct MenuBuilder {
    items: Vec<Option<(String, String, CtxAction)>>,
}

impl MenuBuilder {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    pub fn item(mut self, label: &str, keys: &str, action: CtxAction) -> Self {
        self.items.push(Some((label.into(), keys.into(), action)));
        self
    }

    pub fn separator(mut self) -> Self {
        if self.items.last().is_some_and(Option::is_some) {
            self.items.push(None);
        }
        self
    }

    /// The menu at the mouse position, kept inside the window.
    pub fn build(self, fs: &mut FontSystem, font: FontMetrics, family: &str, x: f32, y: f32, window: (f32, f32)) -> ContextMenu {
        let attrs = Attrs::new().family(Family::Name(family));
        let line = |fs: &mut FontSystem, t: &str| {
            let mut b = Buffer::new(fs, Metrics::new(font.size * 0.95, font.line_h));
            b.set_wrap(Wrap::None);
            b.set_text(t, &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(fs, false);
            let w = b.layout_runs().next().map_or(0.0, |r| r.line_w);
            (b, w)
        };
        let mut rows = Vec::new();
        let (mut label_w, mut keys_w) = (0.0f32, 0.0f32);
        let mut items = self.items;
        while items.last().is_some_and(Option::is_none) {
            items.pop();
        }
        for it in items {
            match it {
                Some((label, keys, action)) => {
                    let (lb, lw) = line(fs, &label);
                    let (kb, kw) = line(fs, &keys);
                    label_w = label_w.max(lw);
                    keys_w = keys_w.max(kw);
                    rows.push(Row::Item { label: Box::new(lb), keys: Box::new(kb), action });
                }
                None => rows.push(Row::Separator),
            }
        }
        let pad = font.cell_w * 1.4;
        let row_h = (font.line_h * 1.35).round();
        let sep_h = (font.line_h * 0.6).round();
        let w = (pad * 2.0 + label_w + if keys_w > 0.0 { font.cell_w * 4.0 + keys_w } else { 0.0 }).max(font.cell_w * 22.0);
        let h = rows.iter().map(|r| if matches!(r, Row::Separator) { sep_h } else { row_h }).sum::<f32>() + pad * 0.6;
        let x = x.min(window.0 - w - 4.0).max(4.0);
        let y = if y + h > window.1 - 4.0 { (y - h).max(4.0) } else { y };
        ContextMenu { rows, x, y, w, h, row_h, sep_h, keys_x: w - pad - keys_w, hover: None, font }
    }
}

impl ContextMenu {
    fn row_rects(&self) -> Vec<(usize, Rect, bool)> {
        let mut y = self.y + self.font.cell_w * 0.42;
        let mut out = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            let sep = matches!(r, Row::Separator);
            let h = if sep { self.sep_h } else { self.row_h };
            out.push((i, Rect { x: self.x, y, w: self.w, h }, sep));
            y += h;
        }
        out
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// The item row under the mouse.
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.row_rects()
            .into_iter()
            .find(|(_, r, sep)| !sep && x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(i, _, _)| i)
    }

    pub fn action(&self, row: usize) -> Option<CtxAction> {
        match self.rows.get(row)? {
            Row::Item { action, .. } => Some(action.clone()),
            Row::Separator => None,
        }
    }

    /// ↑ / ↓: the next item, skipping lines.
    pub fn move_hover(&mut self, delta: isize) {
        let items: Vec<usize> =
            self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Item { .. })).map(|(i, _)| i).collect();
        if items.is_empty() {
            return;
        }
        let pos = self.hover.and_then(|h| items.iter().position(|&i| i == h));
        let next = match pos {
            Some(p) => (p as isize + delta).rem_euclid(items.len() as isize) as usize,
            None if delta > 0 => 0,
            None => items.len() - 1,
        };
        self.hover = Some(items[next]);
    }

    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        cards: &mut Vec<RoundRect>,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let radius = (8.0 * scale).round();
        let shadow = Rgba(0, 0, 0, if theme.dark { 0x90 } else { 0x38 });
        cards.push(RoundRect::shadow(self.x, self.y, self.w, self.h, shadow, radius, (10.0 * scale).round()));
        cards.push(RoundRect::new(self.x, self.y, self.w, self.h, theme.popup_border, radius));
        cards.push(RoundRect::new(self.x + 1.0, self.y + 1.0, self.w - 2.0, self.h - 2.0, theme.popup_bg, radius - 1.0));
        let pad = self.font.cell_w * 1.4;
        for (i, r, sep) in self.row_rects() {
            if sep {
                quads.push(Quad { x: r.x + pad * 0.6, y: (r.y + r.h / 2.0).round(), w: r.w - pad * 1.2, h: 1.0, color: theme.divider });
                continue;
            }
            let Row::Item { label, keys, .. } = &self.rows[i] else { continue };
            if self.hover == Some(i) {
                cards.push(RoundRect::new(r.x + pad * 0.35, r.y + 1.0, r.w - pad * 0.7, r.h - 2.0, theme.row_selected, (5.0 * scale).round()));
            }
            let ty = r.y + (r.h - self.font.line_h) / 2.0;
            text.push(TextBlock { buffer: label, x: r.x + pad, y: ty, color: theme.panel_text, clip: None });
            text.push(TextBlock { buffer: keys, x: r.x + self.keys_x, y: ty, color: theme.panel_dim, clip: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_separators_and_keyboard_moves() {
        let mut fs = FontSystem::new();
        let font = FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let m = MenuBuilder::new()
            .separator()
            .item("Cut", "Ctrl+X", CtxAction::Run(Command::Cut))
            .separator()
            .separator()
            .item("Copy", "Ctrl+C", CtxAction::Run(Command::Copy))
            .separator()
            .build(&mut fs, font, "monospace", 100.0, 100.0, (1000.0, 800.0));
        // Leading, doubled and trailing lines are dropped.
        assert_eq!(m.rows.len(), 3);
        let mut m = m;
        m.move_hover(1);
        assert_eq!(m.action(m.hover.unwrap()), Some(CtxAction::Run(Command::Cut)));
        m.move_hover(1);
        assert_eq!(m.action(m.hover.unwrap()), Some(CtxAction::Run(Command::Copy)));
        m.move_hover(1);
        assert_eq!(m.action(m.hover.unwrap()), Some(CtxAction::Run(Command::Cut)), "wraps around");
        // A menu near the bottom right opens up and to the left.
        let fs2 = &mut fs;
        let low = MenuBuilder::new().item("A", "", CtxAction::CloseAll).build(fs2, font, "monospace", 990.0, 790.0, (1000.0, 800.0));
        assert!(low.x + low.w <= 1000.0 && low.y + low.h <= 800.0);
    }
}
