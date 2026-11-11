//! The window's chrome: the activity bar with colored icons at the left edge, icon buttons,
//! colored file badges, and the colors that belong to each panel.
//!
//! Icons come from "Segoe Fluent Icons", which ships with Windows 11 (Segoe MDL2 Assets on
//! Windows 10 has the same code points for the ones used here).

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Rgba, RoundRect, Shaping, TextBlock, Wrap};

use crate::dock::Panel;
use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

pub const ICON_EXPLORER: char = '\u{E8C8}';
pub const ICON_SEARCH: char = '\u{E721}';
pub const ICON_GIT: char = '\u{F003}';
pub const ICON_DEBUG: char = '\u{EBE8}';
pub const ICON_TERMINAL: char = '\u{E756}';
pub const ICON_SETTINGS: char = '\u{E713}';
pub const ICON_NEW_FILE: char = '\u{ED0E}';
pub const ICON_NEW_FOLDER: char = '\u{E8F4}';
pub const ICON_REFRESH: char = '\u{E72C}';
pub const ICON_COLLAPSE: char = '\u{E92C}';
pub const ICON_CLOSE: char = '\u{E711}';
pub const ICON_ADD: char = '\u{E710}';
pub const ICON_PROBLEMS: char = '\u{E7BA}';
pub const ICON_OUTPUT: char = '\u{E8A5}';
pub const ICON_CHEVRON_DOWN: char = '\u{E70D}';
pub const ICON_PLAY: char = '\u{E768}';
pub const ICON_UNDO: char = '\u{E7A7}';

/// The icon font: Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets.
pub fn icon_family(fs: &FontSystem) -> String {
    for name in ["Segoe Fluent Icons", "Segoe MDL2 Assets"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "Segoe Fluent Icons".to_string()
}

pub fn panel_icon(p: Panel) -> char {
    match p {
        Panel::Explorer => ICON_EXPLORER,
        Panel::Search => ICON_SEARCH,
        Panel::Git => ICON_GIT,
        Panel::Debug => ICON_DEBUG,
        Panel::Terminal => ICON_TERMINAL,
        Panel::Problems => ICON_PROBLEMS,
    }
}

/// Each panel's own color (a bit darker on light themes).
pub fn panel_color(p: Panel, dark: bool) -> Rgba {
    let (d, l) = match p {
        Panel::Explorer => (Rgba::rgb(0x5A, 0xA9, 0xFF), Rgba::rgb(0x1F, 0x6F, 0xD6)),
        Panel::Search => (Rgba::rgb(0xF5, 0xA6, 0x4A), Rgba::rgb(0xC0, 0x6A, 0x00)),
        Panel::Git => (Rgba::rgb(0xF2, 0x6B, 0x4F), Rgba::rgb(0xC8, 0x3A, 0x1E)),
        Panel::Debug => (Rgba::rgb(0x5C, 0xD6, 0x8E), Rgba::rgb(0x1E, 0x9A, 0x55)),
        Panel::Terminal => (Rgba::rgb(0xB9, 0x92, 0xF7), Rgba::rgb(0x7A, 0x4C, 0xD1)),
        Panel::Problems => (Rgba::rgb(0xE8, 0xC5, 0x47), Rgba::rgb(0xA8, 0x84, 0x00)),
    };
    if dark { d } else { l }
}

/// A fingerprint of colored text (and `extra`, e.g. width and font size): when it is the same
/// as last frame, the text need not be shaped again.
pub fn spans_key<'a>(spans: impl IntoIterator<Item = (&'a str, Rgba)>, extra: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    extra.hash(&mut h);
    for (t, c) in spans {
        t.hash(&mut h);
        (c.0, c.1, c.2, c.3).hash(&mut h);
    }
    h.finish()
}

/// Width and font size as one number for [`spans_key`].
pub fn size_key(width: f32, size: f32) -> u64 {
    ((width.to_bits() as u64) << 32) | size.to_bits() as u64
}

/// Color of a leader group in the key popup (by its letter).
pub fn group_color(group: &str, dark: bool) -> Rgba {
    let p = match group {
        "e" | "p" | "f" => Some(Panel::Explorer),
        "s" | "j" => Some(Panel::Search),
        "g" => Some(Panel::Git),
        "d" => Some(Panel::Debug),
        "t" | "a" => Some(Panel::Terminal),
        _ => None,
    };
    match p {
        Some(p) => panel_color(p, dark),
        None if dark => Rgba::rgb(0xE0, 0xB4, 0x6C),
        None => Rgba::rgb(0xA0, 0x64, 0x00),
    }
}

/// A short colored badge for a file name ("C#", "{}", "MD" …), always two cells wide.
pub fn file_badge(name: &str, dark: bool) -> (&'static str, Rgba) {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let pick = |d: (u8, u8, u8), l: (u8, u8, u8)| if dark { Rgba::rgb(d.0, d.1, d.2) } else { Rgba::rgb(l.0, l.1, l.2) };
    match ext.as_str() {
        "cs" | "csx" => ("C#", pick((0xB1, 0x80, 0xF5), (0x78, 0x3B, 0xD0))),
        "csproj" | "sln" | "slnx" | "props" | "targets" => ("PJ", pick((0xB1, 0x80, 0xF5), (0x78, 0x3B, 0xD0))),
        "rs" => ("RS", pick((0xF0, 0x8A, 0x4B), (0xB5, 0x4A, 0x10))),
        "toml" => ("TL", pick((0x9A, 0xA4, 0xB2), (0x5A, 0x63, 0x70))),
        "js" | "mjs" | "cjs" | "jsx" => ("JS", pick((0xF2, 0xD3, 0x4F), (0x9A, 0x7B, 0x00))),
        "ts" | "mts" | "cts" | "tsx" => ("TS", pick((0x4F, 0x9D, 0xF2), (0x1F, 0x5F, 0xB5))),
        "json" | "jsonc" => ("{}", pick((0xF2, 0xD3, 0x4F), (0x9A, 0x7B, 0x00))),
        "html" | "htm" => ("<>", pick((0xF2, 0x7B, 0x50), (0xC0, 0x45, 0x1B))),
        "css" | "scss" | "less" => ("# ", pick((0x4F, 0xB6, 0xF2), (0x1F, 0x6F, 0xB5))),
        "md" | "markdown" => ("MD", pick((0x6F, 0xB3, 0xF5), (0x2A, 0x6C, 0xC0))),
        "py" => ("PY", pick((0x5C, 0xA6, 0xE8), (0x2A, 0x62, 0xA8))),
        "xml" | "xaml" | "axaml" | "config" => ("<>", pick((0x8F, 0xD0, 0x7A), (0x3F, 0x8A, 0x2B))),
        "ps1" | "psm1" | "sh" | "bat" | "cmd" => (">_", pick((0x8F, 0xD0, 0x7A), (0x3F, 0x8A, 0x2B))),
        "txt" | "log" => ("TX", pick((0x9A, 0xA4, 0xB2), (0x5A, 0x63, 0x70))),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => ("IM", pick((0xE8, 0x7E, 0xC8), (0xB0, 0x3A, 0x8C))),
        "gitignore" | "gitattributes" => ("GI", pick((0xF2, 0x6B, 0x4F), (0xC8, 0x3A, 0x1E))),
        "lock" => ("LK", pick((0x9A, 0xA4, 0xB2), (0x5A, 0x63, 0x70))),
        _ => ("··", pick((0x7A, 0x82, 0x8F), (0x80, 0x86, 0x90))),
    }
}

/// An icon for a file in the explorer (Segoe Fluent Icons) and its color.
pub fn file_icon(name: &str, dark: bool) -> (char, Rgba) {
    let (_, color) = file_badge(name, dark);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let icon = match ext.as_str() {
        "cs" | "csx" | "rs" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "py" | "html" | "htm" | "css"
        | "scss" | "less" | "xaml" | "axaml" | "ps1" | "psm1" | "sh" | "bat" | "cmd" => '\u{E943}', // code
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "ico" | "webp" => '\u{EB9F}', // picture
        "json" | "jsonc" | "toml" | "csproj" | "sln" | "slnx" | "props" | "targets" | "config" | "xml" | "lock" | "yml" | "yaml" => '\u{E713}', // settings
        "gitignore" | "gitattributes" => ICON_GIT,
        "md" | "markdown" | "txt" | "log" => '\u{E8A5}', // document
        _ => '\u{E7C3}', // page
    };
    (icon, color)
}

pub const ICON_FOLDER: char = '\u{E8B7}';
pub const ICON_FOLDER_OPEN: char = '\u{E838}';

/// A one-line text buffer (for icons and labels drawn at changing places).
pub fn label(fs: &mut FontSystem, text: &str, family: &str, size: f32, line_h: f32) -> Buffer {
    let mut b = Buffer::new(fs, Metrics::new(size, line_h));
    b.set_wrap(Wrap::None);
    b.set_text(text, &Attrs::new().family(Family::Name(family)), Shaping::Advanced, None);
    b.shape_until_scroll(fs, false);
    b
}

/// What a slot of the activity bar does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActItem {
    Panel(Panel),
    Settings,
}

impl ActItem {
    pub fn tooltip(self) -> &'static str {
        match self {
            ActItem::Panel(Panel::Explorer) => "Explorer (Alt → e)",
            ActItem::Panel(Panel::Search) => "Search (Ctrl+Shift+F)",
            ActItem::Panel(Panel::Git) => "Git (Alt → g → g)",
            ActItem::Panel(Panel::Debug) => "Run and Debug (Alt → d → d)",
            ActItem::Panel(Panel::Problems) => "Problems (Alt → r → p, build: Ctrl+Shift+B)",
            ActItem::Panel(Panel::Terminal) => "Terminal (Alt → t)",
            ActItem::Settings => "Settings (Alt → ,)",
        }
    }
}

/// The column of icons at the left window edge.
pub struct ActivityBar {
    icons: Vec<(ActItem, Buffer)>,
    pub slots: Vec<(ActItem, Rect)>,
    pub hover: Option<ActItem>,
    /// The line between the panels docked left/top of the editor and those at the bottom
    /// (or right, or floating): x, y, width.
    divider: Option<(f32, f32, f32)>,
    /// A count on a panel's icon (errors on Problems): panel, text, color, shaped text.
    badge: Option<(Panel, String, Rgba, Buffer)>,
    family: String,
    ui: FontMetrics,
}

impl ActivityBar {
    pub fn new(fs: &mut FontSystem, ui: FontMetrics) -> Self {
        let family = icon_family(fs);
        let mut bar = Self { icons: Vec::new(), slots: Vec::new(), hover: None, divider: None, badge: None, family, ui };
        bar.shape(fs);
        bar
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, ui: FontMetrics) {
        self.ui = ui;
        self.shape(fs);
    }

    fn icon_size(&self) -> f32 {
        (self.ui.size * 1.45).round()
    }

    fn shape(&mut self, fs: &mut FontSystem) {
        let size = self.icon_size();
        let items: Vec<ActItem> =
            Panel::ALL.iter().map(|&p| ActItem::Panel(p)).chain(std::iter::once(ActItem::Settings)).collect();
        self.icons = items
            .into_iter()
            .map(|item| {
                let c = match item {
                    ActItem::Panel(p) => panel_icon(p),
                    ActItem::Settings => ICON_SETTINGS,
                };
                (item, label(fs, &c.to_string(), &self.family, size, size * 1.2))
            })
            .collect();
    }

    /// Lays the icons out in the bar `r`: the panels of `upper` (docked left of the editor)
    /// from the top, a line, the panels of `lower` (docked at the bottom), settings at the end.
    pub fn place(&mut self, r: Rect, upper: &[Panel], lower: &[Panel]) {
        self.slots.clear();
        self.divider = None;
        let slot = r.w.round();
        let mut y = r.y + (slot * 0.25).round();
        for p in upper {
            self.slots.push((ActItem::Panel(*p), Rect { x: r.x, y, w: r.w, h: slot }));
            y += slot;
        }
        if !lower.is_empty() {
            let gap = (slot * 0.4).round();
            if !upper.is_empty() {
                let w = (r.w * 0.5).round();
                self.divider = Some(((r.x + (r.w - w) / 2.0).round(), (y + gap / 2.0).round(), w));
                y += gap;
            }
            for p in lower {
                self.slots.push((ActItem::Panel(*p), Rect { x: r.x, y, w: r.w, h: slot }));
                y += slot;
            }
        }
        self.slots.push((ActItem::Settings, Rect { x: r.x, y: r.y + r.h - slot * 1.1, w: r.w, h: slot }));
    }

    /// Sets the badge (`None` removes it); shapes the text only when it changed.
    pub fn set_badge(&mut self, fs: &mut FontSystem, ui_family: &str, badge: Option<(Panel, String, Rgba)>) {
        match badge {
            None => self.badge = None,
            Some((p, t, c)) => {
                if self.badge.as_ref().is_some_and(|b| b.0 == p && b.1 == t) {
                    if let Some(b) = &mut self.badge {
                        b.2 = c;
                    }
                    return;
                }
                let size = (self.ui.size * 0.72).round();
                let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.2));
                buf.set_wrap(Wrap::None);
                let attrs = Attrs::new().family(Family::Name(ui_family)).weight(wyven_render::Weight::BOLD);
                buf.set_text(&t, &attrs, Shaping::Advanced, None);
                buf.shape_until_scroll(fs, false);
                self.badge = Some((p, t, c, buf));
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<ActItem> {
        self.slots.iter().find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h).map(|(i, _)| *i)
    }

    /// `shown`: panels on screen; `focused`: the panel with the keyboard focus.
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        shown: &[Panel],
        focused: Option<Panel>,
        cards: &mut Vec<RoundRect>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let size = self.icon_size();
        if let Some((p, _, color, buf)) = &self.badge
            && let Some((_, r)) = self.slots.iter().find(|(i, _)| *i == ActItem::Panel(*p))
        {
            let size = (self.ui.size * 0.72).round();
            let tw = buf.layout_runs().next().map_or(size, |run| run.line_w);
            let h = (size * 1.3).round();
            let w = (tw + size * 0.7).max(h).round();
            let x = (r.x + r.w * 0.58).round();
            let y = (r.y + r.h * 0.12).round();
            cards.push(RoundRect::new(x, y, w, h, *color, h / 2.0));
            text.push(TextBlock {
                buffer: buf,
                x: (x + (w - tw) / 2.0).round(),
                y: (y + (h - size * 1.2) / 2.0).round(),
                color: Rgba::rgb(0xFF, 0xFF, 0xFF),
                clip: None,
            });
        }
        if let Some((x, y, w)) = self.divider {
            let gray = if theme.dark { Rgba::rgb(0x6E, 0x6E, 0x6E) } else { Rgba::rgb(0xA8, 0xA8, 0xA8) };
            cards.push(RoundRect::new(x, y, w, (1.5 * scale).max(1.0), gray, scale));
        }
        for (item, r) in &self.slots {
            let Some((_, buf)) = self.icons.iter().find(|(i, _)| i == item) else { continue };
            let (on, color) = match item {
                ActItem::Panel(p) => (shown.contains(p), panel_color(*p, theme.dark)),
                ActItem::Settings => (false, theme.panel_text),
            };
            let pad = (6.0 * scale).round();
            if self.hover == Some(*item) || on {
                let a = if on { 0x26 } else { 0x16 };
                let c = if on { color.with_alpha(a) } else { theme.panel_text.with_alpha(a) };
                cards.push(RoundRect::new(r.x + pad, r.y + pad * 0.5, r.w - 2.0 * pad, r.h - pad, c, (6.0 * scale).round()));
            }
            if on {
                // A pill at the left edge marks the open panels; brighter for the focused one.
                let h = (r.h * if focused == Some(match item { ActItem::Panel(p) => *p, _ => Panel::Explorer }) { 0.5 } else { 0.32 }).round();
                cards.push(RoundRect::new(r.x + 1.0, r.y + (r.h - h) / 2.0, (3.0 * scale).round(), h, color, 2.0 * scale));
            }
            let glyph_color = if on || self.hover == Some(*item) { color } else { theme.panel_dim };
            let w = buf.layout_runs().next().map_or(size, |run| run.line_w);
            text.push(TextBlock {
                buffer: buf,
                x: (r.x + (r.w - w) / 2.0).round(),
                y: (r.y + (r.h - size * 1.2) / 2.0).round(),
                color: glyph_color,
                clip: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_are_two_cells_and_colored_by_type() {
        let dark = true;
        assert_eq!(file_badge("Program.cs", dark).0, "C#");
        assert_eq!(file_badge("README.MD", dark).0, "MD");
        assert_eq!(file_badge("Makefile", dark).0, "··");
        for name in ["a.rs", "a.json", "a.css", "a.html", "a.ps1", "a.bin"] {
            assert_eq!(file_badge(name, dark).0.chars().count(), 2, "{name}");
        }
        assert_ne!(file_badge("a.cs", dark).1, file_badge("a.json", dark).1);
    }
}
