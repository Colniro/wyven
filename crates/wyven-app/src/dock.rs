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

/// A panel tab on screen (in the left area only the shown panel's title: the activity bar
/// switches there).
#[derive(Debug, Clone, Copy)]
pub struct TabSpan {
    pub area: Area,
    pub panel: Panel,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

/// An icon button in a panel header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderButton {
    NewFile,
    NewFolder,
    Refresh,
    CollapseAll,
    /// Closes the area.
    Close,
}

impl HeaderButton {
    fn icon(self) -> char {
        use crate::chrome::*;
        match self {
            HeaderButton::NewFile => ICON_NEW_FILE,
            HeaderButton::NewFolder => ICON_NEW_FOLDER,
            HeaderButton::Refresh => ICON_REFRESH,
            HeaderButton::CollapseAll => ICON_COLLAPSE,
            HeaderButton::Close => ICON_CLOSE,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            HeaderButton::NewFile => "New File (Ctrl+N)",
            HeaderButton::NewFolder => "New Folder (Ctrl+Shift+N)",
            HeaderButton::Refresh => "Refresh",
            HeaderButton::CollapseAll => "Collapse Folders",
            HeaderButton::Close => "Close",
        }
    }
}

/// What is under the mouse in a header.
#[derive(Debug, Clone, Copy)]
pub enum HeaderHit {
    Tab(TabSpan),
    Button(Area, Panel, HeaderButton),
}

/// Draws the headers of the dock areas and knows where tabs and buttons are.
pub struct DockBar {
    labels: Vec<Buffer>,
    widths: [f32; 5],
    icons: Vec<(HeaderButton, Buffer)>,
    pub spans: Vec<TabSpan>,
    pub buttons: Vec<(Area, Panel, HeaderButton, Rect)>,
    pub hover: Option<(Area, HeaderButton)>,
    ui: FontMetrics,
    family: String,
    icon_family: String,
}

impl DockBar {
    pub fn new(fs: &mut FontSystem, ui: FontMetrics, family: String) -> Self {
        let icon_family = crate::chrome::icon_family(fs);
        let mut bar = Self {
            labels: Vec::new(),
            widths: [0.0; 5],
            icons: Vec::new(),
            spans: Vec::new(),
            buttons: Vec::new(),
            hover: None,
            ui,
            family,
            icon_family,
        };
        bar.shape(fs);
        bar
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, ui: FontMetrics) {
        self.ui = ui;
        self.shape(fs);
    }

    fn shape(&mut self, fs: &mut FontSystem) {
        let attrs = Attrs::new().family(Family::Name(&self.family)).weight(wyven_render::Weight::SEMIBOLD);
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
        let size = (self.ui.size * 1.05).round();
        self.icons = [HeaderButton::NewFile, HeaderButton::NewFolder, HeaderButton::Refresh, HeaderButton::CollapseAll, HeaderButton::Close]
            .into_iter()
            .map(|b| (b, crate::chrome::label(fs, &b.icon().to_string(), &self.icon_family, size, size * 1.2)))
            .collect();
    }

    fn buttons_for(area: Area, panel: Panel) -> Vec<HeaderButton> {
        let mut list = match panel {
            Panel::Explorer => vec![HeaderButton::NewFile, HeaderButton::NewFolder, HeaderButton::Refresh, HeaderButton::CollapseAll],
            Panel::Git => vec![HeaderButton::Refresh],
            _ => Vec::new(),
        };
        if area != Area::Left {
            list.push(HeaderButton::Close);
        }
        list
    }

    /// Places tabs and buttons of every open area: area, its rectangle, its panels, the shown one.
    pub fn place(&mut self, areas: &[(Area, Rect, Vec<Panel>, Option<Panel>)]) {
        self.spans.clear();
        self.buttons.clear();
        let pad = self.ui.cell_w * 1.2;
        let h = strip_h(self.ui);
        for (area, r, panels, active) in areas {
            let mut x = r.x + pad * 0.5;
            for &panel in panels {
                // The left area shows only its panel's title; the activity bar switches.
                if *area == Area::Left && Some(panel) != *active {
                    continue;
                }
                let w = self.widths[panel.index()] + 2.0 * pad;
                self.spans.push(TabSpan { area: *area, panel, x0: x, x1: x + w, y0: r.y, y1: r.y + h });
                x += w;
            }
            if let Some(p) = active {
                let bw = (h * 0.95).round();
                let mut bx = r.x + r.w - pad * 0.5;
                for b in Self::buttons_for(*area, *p).into_iter().rev() {
                    bx -= bw;
                    self.buttons.push((*area, *p, b, Rect { x: bx, y: r.y + (h - bw) / 2.0, w: bw, h: bw }));
                }
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<HeaderHit> {
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if let Some((a, p, b, _)) = self.buttons.iter().find(|(_, _, _, r)| inside(r)) {
            return Some(HeaderHit::Button(*a, *p, *b));
        }
        self.spans.iter().copied().find(|s| x >= s.x0 && x < s.x1 && y >= s.y0 && y < s.y1).map(HeaderHit::Tab)
    }

    /// Header backgrounds, tabs (the shown one in its panel's color), buttons.
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        areas: &[(Area, Rect, Option<Panel>)],
        focused: Option<Panel>,
        under: &mut Vec<Quad>,
        cards: &mut Vec<wyven_render::RoundRect>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let h = strip_h(self.ui);
        let pad = self.ui.cell_w * 1.2;
        for &(_, r, _) in areas {
            under.push(Quad { x: r.x, y: r.y, w: r.w, h, color: theme.panel_bg });
            under.push(Quad { x: r.x + pad * 0.5, y: r.y + h - 1.0, w: r.w - pad, h: 1.0, color: theme.divider });
        }
        let accent_h = (2.0 * scale).round();
        for s in &self.spans {
            let Some(&(_, r, active)) = areas.iter().find(|a| a.0 == s.area) else { continue };
            let is_active = active == Some(s.panel);
            if s.x0 >= r.x + r.w {
                continue;
            }
            let color = crate::chrome::panel_color(s.panel, theme.dark);
            if is_active && s.area != Area::Left {
                let w = (s.x1.min(r.x + r.w)) - s.x0 - pad;
                let c = if focused == Some(s.panel) { color } else { color.with_alpha(0x90) };
                cards.push(wyven_render::RoundRect::new(s.x0 + pad * 0.5, s.y1 - accent_h - 2.0, w, accent_h, c, accent_h / 2.0));
            }
            let text_color = if s.area == Area::Left {
                if focused == Some(s.panel) { color } else { theme.panel_text }
            } else if is_active {
                theme.panel_text
            } else {
                theme.panel_dim
            };
            text.push(TextBlock {
                buffer: &self.labels[s.panel.index()],
                x: s.x0 + pad,
                y: s.y0 + (h - self.ui.line_h) / 2.0,
                color: text_color,
                clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + h) as i32]),
            });
        }
        let size = (self.ui.size * 1.05).round();
        for (area, _, b, r) in &self.buttons {
            let Some((_, buf)) = self.icons.iter().find(|(k, _)| k == b) else { continue };
            let hover = self.hover == Some((*area, *b));
            if hover {
                cards.push(wyven_render::RoundRect::new(r.x, r.y, r.w, r.h, theme.panel_text.with_alpha(0x22), (4.0 * scale).round()));
            }
            let w = buf.layout_runs().next().map_or(size, |run| run.line_w);
            text.push(TextBlock {
                buffer: buf,
                x: (r.x + (r.w - w) / 2.0).round(),
                y: (r.y + (r.h - size * 1.2) / 2.0).round(),
                color: if hover { theme.panel_text } else { theme.panel_dim },
                clip: None,
            });
        }
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
}
