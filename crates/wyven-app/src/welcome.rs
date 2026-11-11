//! The start page: shown instead of an empty "Untitled" tab. Links to start (new file, open
//! file or folder, search, settings, shortcuts), the recent projects, and a few tips. Typing
//! simply starts a new file; opening anything replaces the page.

use std::path::PathBuf;

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Weight, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum WelcomeAction {
    Leader(crate::leader::Action),
    OpenRecent(PathBuf),
}

struct Row {
    label: Buffer,
    keys: Buffer,
    action: Option<WelcomeAction>,
    /// Section headings and tips are not clickable.
    heading: bool,
}

pub struct Welcome {
    title: Buffer,
    subtitle: Buffer,
    rows: Vec<Row>,
    /// Where each row was drawn (for hover and clicks).
    rects: Vec<Rect>,
    pub hover: Option<usize>,
    ui: FontMetrics,
    family: String,
    /// The recent list the rows were built from.
    built_for: Vec<PathBuf>,
}

impl Welcome {
    pub fn new(fs: &mut FontSystem, ui: FontMetrics, family: String) -> Self {
        let mut me = Self {
            title: Buffer::new(fs, Metrics::new(ui.size * 2.4, ui.line_h * 2.4)),
            subtitle: Buffer::new(fs, Metrics::new(ui.size, ui.line_h)),
            rows: Vec::new(),
            rects: Vec::new(),
            hover: None,
            ui,
            family,
            built_for: vec![PathBuf::from("?")],
        };
        me.title.set_wrap(Wrap::None);
        me.subtitle.set_wrap(Wrap::None);
        me
    }

    pub fn set_font(&mut self, ui: FontMetrics) {
        self.ui = ui;
        self.built_for = vec![PathBuf::from("?")];
    }

    /// Shapes the page (again when the recent projects changed).
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, recent: &[PathBuf]) {
        if self.built_for == recent {
            return;
        }
        self.built_for = recent.to_vec();
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let bold = attrs.clone().weight(Weight::BOLD);
        self.title.set_metrics(Metrics::new(self.ui.size * 2.4, self.ui.line_h * 2.4));
        self.title.set_text("Wyven", &bold, Shaping::Advanced, None);
        self.title.shape_until_scroll(fs, false);
        self.subtitle.set_metrics(Metrics::new(self.ui.size, self.ui.line_h));
        self.subtitle.set_text("Start typing for a new file, or pick something below.", &attrs, Shaping::Advanced, None);
        self.subtitle.shape_until_scroll(fs, false);

        use crate::leader::Action as L;
        let mut list: Vec<(String, String, Option<WelcomeAction>, bool)> = vec![
            ("Start".into(), String::new(), None, true),
            ("New File".into(), "Ctrl+N".into(), Some(WelcomeAction::Leader(L::NewFile)), false),
            ("Open File …".into(), "Ctrl+O".into(), Some(WelcomeAction::Leader(L::OpenFile)), false),
            ("Open Folder …".into(), "Alt → p → o".into(), Some(WelcomeAction::Leader(L::OpenFolder)), false),
            ("Go to File / Command …".into(), "Ctrl+Q".into(), Some(WelcomeAction::Leader(L::Palette(""))), false),
            ("Settings".into(), "Alt → ,".into(), Some(WelcomeAction::Leader(L::OpenSettings)), false),
            ("All Keyboard Shortcuts".into(), "Alt → h".into(), Some(WelcomeAction::Leader(L::CheatSheet)), false),
            ("Report a Bug / Suggest a Feature".into(), "GitHub".into(), Some(WelcomeAction::Leader(L::ReportBug)), false),
        ];
        if !recent.is_empty() {
            list.push(("Recent".into(), String::new(), None, true));
            for p in recent.iter().take(8) {
                let name = p.file_name().map_or_else(|| p.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                let dir = p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                list.push((name, dir, Some(WelcomeAction::OpenRecent(p.clone())), false));
            }
        }
        list.push(("Tips".into(), String::new(), None, true));
        for tip in [
            "Tap Alt: a menu shows every command with its key.",
            "Ctrl+Shift+B builds, ▶ at the top right runs the project.",
            "Drag a panel's header to dock it anywhere, or out into its own window.",
            "Drop files or folders from Windows onto the window to open them.",
        ] {
            list.push((tip.into(), String::new(), None, false));
        }
        let m = Metrics::new(self.ui.size, self.ui.line_h);
        let mk = |fs: &mut FontSystem, t: &str, a: &Attrs| {
            let mut b = Buffer::new(fs, m);
            b.set_wrap(Wrap::None);
            b.set_text(t, a, Shaping::Advanced, None);
            b.shape_until_scroll(fs, false);
            b
        };
        let _ = theme;
        self.rows = list
            .into_iter()
            .map(|(label, keys, action, heading)| Row {
                label: mk(fs, &label, if heading { &bold } else { &attrs }),
                keys: mk(fs, &keys, &attrs),
                action,
                heading,
            })
            .collect();
        self.rects.clear();
        self.hover = None;
    }

    fn row_h(&self) -> f32 {
        (self.ui.line_h * 1.55).round()
    }

    /// Places the rows in `area` (the editor) and returns the page's left edge and top.
    fn place(&mut self, area: Rect) -> (f32, f32, f32) {
        let w = (self.ui.cell_w * 72.0).min(area.w - self.ui.cell_w * 4.0).max(self.ui.cell_w * 20.0);
        let x = (area.x + (area.w - w) / 2.0).round();
        let rh = self.row_h();
        let header = self.ui.line_h * 2.4 + self.ui.line_h * 2.2;
        let gaps = self.rows.iter().filter(|r| r.heading).count() as f32 * rh * 0.6;
        let total = header + self.rows.len() as f32 * rh + gaps;
        let top = (area.y + ((area.h - total) * 0.35).max(self.ui.line_h)).round();
        self.rects.clear();
        let mut y = top + header;
        for r in &self.rows {
            if r.heading {
                y += rh * 0.6;
            }
            self.rects.push(Rect { x, y, w, h: rh });
            y += rh;
        }
        (x, top, w)
    }

    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rects
            .iter()
            .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .filter(|&i| self.rows[i].action.is_some())
    }

    pub fn action(&self, i: usize) -> Option<WelcomeAction> {
        self.rows.get(i)?.action.clone()
    }

    pub fn draw<'a>(&'a mut self, theme: &Theme, area: Rect, scale: f32, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        under.push(Quad { x: area.x, y: area.y, w: area.w, h: area.h, color: theme.background });
        let (x, top, w) = self.place(area);
        let clip = Some([area.x as i32, area.y as i32, (area.x + area.w) as i32, (area.y + area.h) as i32]);
        text.push(TextBlock { buffer: &self.title, x, y: top, color: theme.accent, clip });
        text.push(TextBlock { buffer: &self.subtitle, x, y: top + self.ui.line_h * 2.6, color: theme.panel_dim, clip });
        let pad = self.ui.cell_w;
        for (i, (r, rect)) in self.rows.iter().zip(&self.rects).enumerate() {
            let ty = rect.y + (rect.h - self.ui.line_h) / 2.0;
            if self.hover == Some(i) {
                under.push(Quad { x: rect.x - pad, y: rect.y, w: rect.w + 2.0 * pad, h: rect.h, color: theme.tab_active });
                under.push(Quad { x: rect.x - pad, y: rect.y + rect.h * 0.2, w: (2.0 * scale).round(), h: rect.h * 0.6, color: theme.accent });
            }
            let color: Rgba = if r.heading {
                theme.panel_text
            } else if r.action.is_some() {
                theme.accent
            } else {
                theme.panel_dim
            };
            text.push(TextBlock { buffer: &r.label, x: rect.x, y: ty, color, clip });
            let kw = r.keys.layout_runs().next().map_or(0.0, |run| run.line_w);
            if kw > 0.0 {
                // Keys (or the folder of a recent project) right-aligned, dim.
                let kx = (rect.x + w - kw).max(rect.x + self.ui.cell_w * 24.0);
                text.push(TextBlock { buffer: &r.keys, x: kx, y: ty, color: theme.panel_dim, clip: Some([kx as i32, rect.y as i32, (rect.x + w) as i32, (rect.y + rect.h) as i32]) });
            }
            if r.heading {
                under.push(Quad { x: rect.x, y: rect.y + rect.h - 1.0, w, h: 1.0, color: theme.divider });
            }
        }
    }
}
