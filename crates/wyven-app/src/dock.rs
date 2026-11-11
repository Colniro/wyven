//! Dockable panels: Explorer, Search, Git, Debug and Terminal sit in three areas around the
//! editor (left, right, bottom). Each area shows one panel at a time; a strip of panel tabs on
//! top switches between them, and dragging a tab moves the panel to another area. Positions,
//! sizes and visibility are saved in `%APPDATA%\Wyven\layout.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    Explorer,
    Search,
    Git,
    Debug,
    Terminal,
}

impl Panel {
    pub const ALL: [Panel; 5] = [Panel::Explorer, Panel::Search, Panel::Git, Panel::Debug, Panel::Terminal];

    pub fn label(self) -> &'static str {
        match self {
            Panel::Explorer => "EXPLORER",
            Panel::Search => "SEARCH",
            Panel::Git => "GIT",
            Panel::Debug => "DEBUG",
            Panel::Terminal => "TERMINAL",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Area {
    Left,
    Right,
    Bottom,
}

impl Area {
    pub const ALL: [Area; 3] = [Area::Left, Area::Right, Area::Bottom];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreaState {
    pub panels: Vec<Panel>,
    pub active: Option<Panel>,
    pub visible: bool,
    /// Width of a side area in UI character cells, height of the bottom area in percent of
    /// the window; `None` = the default from settings.toml.
    #[serde(default)]
    pub size: Option<f32>,
}

impl AreaState {
    fn new(panels: &[Panel], visible: bool) -> Self {
        Self { panels: panels.to_vec(), active: panels.first().copied(), visible, size: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    pub left: AreaState,
    pub right: AreaState,
    pub bottom: AreaState,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            left: AreaState::new(&[Panel::Explorer, Panel::Search, Panel::Git, Panel::Debug], true),
            right: AreaState::new(&[], false),
            bottom: AreaState::new(&[Panel::Terminal], false),
        }
    }
}

fn layout_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("layout.json"))
}

impl Dock {
    /// The saved layout, or the default one.
    pub fn load() -> Self {
        let mut dock = layout_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Dock>(&s).ok())
            .unwrap_or_default();
        dock.normalize();
        dock
    }

    pub fn save(&self) {
        let Some(path) = layout_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self)
            && let Err(e) = std::fs::write(&path, json)
        {
            log::warn!("layout.json: {e}");
        }
    }

    pub fn area(&self, a: Area) -> &AreaState {
        match a {
            Area::Left => &self.left,
            Area::Right => &self.right,
            Area::Bottom => &self.bottom,
        }
    }

    pub fn area_mut(&mut self, a: Area) -> &mut AreaState {
        match a {
            Area::Left => &mut self.left,
            Area::Right => &mut self.right,
            Area::Bottom => &mut self.bottom,
        }
    }

    /// Every panel exactly once, valid active panels (after loading an old or edited file).
    fn normalize(&mut self) {
        let mut seen = Vec::new();
        for a in Area::ALL {
            let s = self.area_mut(a);
            s.panels.retain(|p| {
                let new = !seen.contains(p);
                if new {
                    seen.push(*p);
                }
                new
            });
        }
        let defaults = Dock::default();
        for p in Panel::ALL {
            if !seen.contains(&p) {
                let home = Area::ALL.into_iter().find(|&a| defaults.area(a).panels.contains(&p)).unwrap_or(Area::Left);
                self.area_mut(home).panels.push(p);
            }
        }
        for a in Area::ALL {
            let s = self.area_mut(a);
            if s.active.is_none_or(|p| !s.panels.contains(&p)) {
                s.active = s.panels.first().copied();
            }
            if s.panels.is_empty() {
                s.visible = false;
            }
        }
    }

    pub fn area_of(&self, p: Panel) -> Area {
        Area::ALL.into_iter().find(|&a| self.area(a).panels.contains(&p)).unwrap_or(Area::Left)
    }

    /// The panel's area is open and shows this panel.
    pub fn shown(&self, p: Panel) -> bool {
        let s = self.area(self.area_of(p));
        s.visible && s.active == Some(p)
    }

    pub fn show(&mut self, p: Panel) {
        let s = self.area_mut(self.area_of(p));
        s.visible = true;
        s.active = Some(p);
    }

    /// Closes the area the panel is in.
    pub fn hide(&mut self, p: Panel) {
        self.area_mut(self.area_of(p)).visible = false;
    }

    /// Moves a panel to `to` at `index` (end if too large); it becomes that area's shown panel.
    pub fn move_panel(&mut self, p: Panel, to: Area, index: usize) {
        let from = self.area_of(p);
        let s = self.area_mut(from);
        let old = s.panels.iter().position(|&q| q == p).unwrap_or(0);
        s.panels.retain(|&q| q != p);
        if s.active == Some(p) {
            s.active = s.panels.get(old.min(s.panels.len().saturating_sub(1))).copied();
        }
        if s.panels.is_empty() {
            s.visible = false;
        }
        // Moving within the same area: the removal shifted later positions.
        let index = if from == to && index > old { index - 1 } else { index };
        let t = self.area_mut(to);
        let index = index.min(t.panels.len());
        t.panels.insert(index, p);
        t.active = Some(p);
        t.visible = true;
    }
}

/// Height of the panel tab strip on top of an area.
pub fn strip_h(ui: FontMetrics) -> f32 {
    (ui.line_h * 1.6).round()
}

/// The part of an area below its tab strip, where the panel draws.
pub fn content(area: Rect, ui: FontMetrics) -> Rect {
    let h = strip_h(ui).min(area.h);
    Rect { x: area.x, y: area.y + h, w: area.w, h: area.h - h }
}

/// A panel tab on screen.
#[derive(Debug, Clone, Copy)]
pub struct TabSpan {
    pub area: Area,
    pub panel: Panel,
    pub index: usize,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

/// Draws the tab strips and knows where each tab is.
pub struct DockBar {
    labels: Vec<Buffer>,
    widths: [f32; 5],
    pub spans: Vec<TabSpan>,
    ui: FontMetrics,
    family: String,
}

impl DockBar {
    pub fn new(fs: &mut FontSystem, ui: FontMetrics, family: String) -> Self {
        let mut bar = Self { labels: Vec::new(), widths: [0.0; 5], spans: Vec::new(), ui, family };
        bar.shape(fs);
        bar
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, ui: FontMetrics) {
        self.ui = ui;
        self.shape(fs);
    }

    fn shape(&mut self, fs: &mut FontSystem) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        self.labels = Panel::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut b = Buffer::new(fs, Metrics::new(self.ui.size * 0.9, self.ui.line_h));
                b.set_wrap(Wrap::None);
                b.set_text(p.label(), &attrs, Shaping::Advanced, None);
                b.shape_until_scroll(fs, false);
                self.widths[i] = b.layout_runs().next().map_or(0.0, |r| r.line_w);
                b
            })
            .collect();
    }

    /// Places the tabs of every open area (`areas`: area, its whole rectangle, its panels).
    pub fn place(&mut self, areas: &[(Area, Rect, Vec<Panel>)]) {
        self.spans.clear();
        let pad = self.ui.cell_w * 1.2;
        let h = strip_h(self.ui);
        for (area, r, panels) in areas {
            let mut x = r.x + pad * 0.5;
            for (index, &panel) in panels.iter().enumerate() {
                let w = self.widths[panel.index()] + 2.0 * pad;
                self.spans.push(TabSpan { area: *area, panel, index, x0: x, x1: x + w, y0: r.y, y1: r.y + h });
                x += w;
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<TabSpan> {
        self.spans.iter().copied().find(|s| x >= s.x0 && x < s.x1 && y >= s.y0 && y < s.y1)
    }

    /// Strip backgrounds, tabs (the shown one highlighted, with the accent when focused).
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        areas: &[(Area, Rect, Option<Panel>)],
        focused: Option<Panel>,
        under: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let h = strip_h(self.ui);
        let pad = self.ui.cell_w * 1.2;
        for &(_, r, _) in areas {
            under.push(Quad { x: r.x, y: r.y, w: r.w, h, color: theme.panel_bg });
            under.push(Quad { x: r.x, y: r.y + h - 1.0, w: r.w, h: 1.0, color: theme.divider });
        }
        let accent_h = (2.0 * scale).round();
        for s in &self.spans {
            let Some(&(_, r, active)) = areas.iter().find(|a| a.0 == s.area) else { continue };
            let is_active = active == Some(s.panel);
            if s.x0 >= r.x + r.w {
                continue;
            }
            if is_active {
                let w = (s.x1.min(r.x + r.w)) - s.x0;
                if focused == Some(s.panel) {
                    under.push(Quad { x: s.x0, y: s.y1 - accent_h - 1.0, w, h: accent_h, color: theme.accent });
                } else {
                    under.push(Quad { x: s.x0, y: s.y1 - accent_h - 1.0, w, h: accent_h, color: theme.panel_dim });
                }
            }
            text.push(TextBlock {
                buffer: &self.labels[s.panel.index()],
                x: s.x0 + pad,
                y: s.y0 + (h - self.ui.line_h) / 2.0,
                color: if is_active { theme.panel_text } else { theme.panel_dim },
                clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + h) as i32]),
            });
        }
    }
}

/// Where a dragged panel would land: over a tab strip it goes between those tabs, near the
/// left or right window edge or the bottom into that area (at the end).
pub fn drop_target(
    x: f32,
    y: f32,
    body: Rect,
    spans: &[TabSpan],
    strips: &[(Area, Rect)],
) -> Option<(Area, usize)> {
    for &(area, r) in strips {
        if x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h {
            let index = spans
                .iter()
                .filter(|s| s.area == area)
                .find(|s| x < (s.x0 + s.x1) / 2.0)
                .map_or_else(|| spans.iter().filter(|s| s.area == area).count(), |s| s.index);
            return Some((area, index));
        }
    }
    if x < body.x || x >= body.x + body.w || y < body.y || y >= body.y + body.h {
        return None;
    }
    let end = usize::MAX;
    if x < body.x + body.w * 0.2 {
        Some((Area::Left, end))
    } else if x >= body.x + body.w * 0.8 {
        Some((Area::Right, end))
    } else if y >= body.y + body.h * 0.65 {
        Some((Area::Bottom, end))
    } else {
        None
    }
}

/// The rectangle that lights up for a drop target: the open area, or the zone it would take.
pub fn drop_zone(area: Area, body: Rect, open: Option<Rect>) -> Rect {
    if let Some(r) = open {
        return r;
    }
    match area {
        Area::Left => Rect { x: body.x, y: body.y, w: body.w * 0.2, h: body.h },
        Area::Right => Rect { x: body.x + body.w * 0.8, y: body.y, w: body.w * 0.2, h: body.h },
        Area::Bottom => Rect { x: body.x, y: body.y + body.h * 0.65, w: body.w, h: body.h * 0.35 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_move_between_areas_and_stay_unique() {
        let mut d = Dock::default();
        assert!(d.shown(Panel::Explorer));
        assert!(!d.shown(Panel::Terminal));
        d.move_panel(Panel::Git, Area::Right, 0);
        assert_eq!(d.area_of(Panel::Git), Area::Right);
        assert!(d.shown(Panel::Git) && d.shown(Panel::Explorer));
        // The last panel leaves: the area closes.
        d.move_panel(Panel::Terminal, Area::Left, 1);
        assert!(!d.bottom.visible);
        assert_eq!(d.left.panels, vec![Panel::Explorer, Panel::Terminal, Panel::Search, Panel::Debug]);
        assert_eq!(d.left.active, Some(Panel::Terminal));
        // Within the same area, to the end.
        d.move_panel(Panel::Explorer, Area::Left, usize::MAX);
        assert_eq!(d.left.panels, vec![Panel::Terminal, Panel::Search, Panel::Debug, Panel::Explorer]);
        // Moving the shown panel away shows a neighbour.
        d.move_panel(Panel::Explorer, Area::Bottom, 0);
        assert_eq!(d.left.active, Some(Panel::Debug));
        let all: usize = Area::ALL.iter().map(|&a| d.area(a).panels.len()).sum();
        assert_eq!(all, 5);
    }

    #[test]
    fn a_broken_saved_layout_is_repaired() {
        let json = r#"{"left":{"panels":["Git","Git"],"active":"Terminal","visible":true},
                       "right":{"panels":[],"active":null,"visible":true},
                       "bottom":{"panels":["Terminal"],"active":"Terminal","visible":false}}"#;
        let mut d: Dock = serde_json::from_str(json).unwrap();
        d.normalize();
        assert_eq!(d.left.panels, vec![Panel::Git, Panel::Explorer, Panel::Search, Panel::Debug]);
        assert_eq!(d.left.active, Some(Panel::Git));
        assert!(!d.right.visible);
    }

    #[test]
    fn drop_targets_follow_the_cursor() {
        let body = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };
        let span = |area, index, x0: f32, x1: f32| TabSpan { area, panel: Panel::Git, index, x0, x1, y0: 0.0, y1: 20.0 };
        let spans = [span(Area::Left, 0, 0.0, 50.0), span(Area::Left, 1, 50.0, 100.0)];
        let strips = [(Area::Left, Rect { x: 0.0, y: 0.0, w: 200.0, h: 20.0 })];
        assert_eq!(drop_target(10.0, 10.0, body, &spans, &strips), Some((Area::Left, 0)));
        assert_eq!(drop_target(80.0, 10.0, body, &spans, &strips), Some((Area::Left, 2)));
        assert_eq!(drop_target(150.0, 10.0, body, &spans, &strips), Some((Area::Left, 2)));
        assert_eq!(drop_target(900.0, 300.0, body, &spans, &strips), Some((Area::Right, usize::MAX)));
        assert_eq!(drop_target(500.0, 700.0, body, &spans, &strips), Some((Area::Bottom, usize::MAX)));
        assert_eq!(drop_target(500.0, 300.0, body, &spans, &strips), None);
    }
}
