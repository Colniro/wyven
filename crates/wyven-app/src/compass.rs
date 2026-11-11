//! The docking compass, as in Visual Studio: while a panel or an editor tab is dragged, drop
//! targets appear in the middle of the window part under the mouse (a diamond) and, for panels,
//! at the four window edges. Hovering a target shades the place where it would land.
//!
//! Over a pane the diamond has five targets: left, top, right and bottom split the pane (the
//! panel takes half of it), the middle adds the panel as a tab. Over the editor four targets
//! dock the panel next to the editor. The edge targets put it along a whole window edge.
//! Editor tabs split the editor to the left, right, top or bottom.

use wyven_render::{Rgba, RoundRect};

use crate::dock::{self, DockTarget, NodePath};
use crate::editor::Rect;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    /// As a tab of the pane under the mouse.
    Center,
}

impl Side {
    fn dock(self) -> Option<dock::Side> {
        Some(match self {
            Side::Left => dock::Side::Left,
            Side::Right => dock::Side::Right,
            Side::Top => dock::Side::Top,
            Side::Bottom => dock::Side::Bottom,
            Side::Center => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub side: Side,
    /// The button to drop on.
    pub button: Rect,
    /// Where the window would land (shaded while hovering).
    pub zone: Rect,
    /// Where a panel docks (None for editor tabs).
    pub dock: Option<DockTarget>,
}

/// The window part under the mouse while a panel is dragged.
#[derive(Debug, Clone, PartialEq)]
pub enum Under {
    Pane(NodePath, Rect),
    Editor(NodePath, Rect),
}

fn diamond(c: (f32, f32), t: f32, sides: &[Side]) -> Vec<(Side, Rect)> {
    let step = (t * 1.18).round();
    sides
        .iter()
        .map(|&s| {
            let (dx, dy) = match s {
                Side::Left => (-step, 0.0),
                Side::Right => (step, 0.0),
                Side::Top => (0.0, -step),
                Side::Bottom => (0.0, step),
                Side::Center => (0.0, 0.0),
            };
            (s, Rect { x: c.0 + dx - t / 2.0, y: c.1 + dy - t / 2.0, w: t, h: t })
        })
        .collect()
}

fn center(r: Rect) -> (f32, f32) {
    (r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// The part of `r` at `side`: `wf` of its width (left/right) or `hf` of its height.
fn strip(r: Rect, side: Side, wf: f32, hf: f32) -> Rect {
    let w = (r.w * wf).round();
    let h = (r.h * hf).round();
    match side {
        Side::Left => Rect { w, ..r },
        Side::Right => Rect { x: r.x + r.w - w, w, ..r },
        Side::Top => Rect { h, ..r },
        Side::Bottom => Rect { y: r.y + r.h - h, h, ..r },
        Side::Center => r,
    }
}

const FOUR: [Side; 4] = [Side::Left, Side::Top, Side::Right, Side::Bottom];

/// Targets for a dragged panel. `content`: everything right of the activity bar and above the
/// status bar; `under`: the pane or editor under the mouse.
pub fn panel_targets(content: Rect, under: Option<Under>, t: f32) -> Vec<Target> {
    let mut out = Vec::new();
    match under {
        Some(Under::Pane(path, r)) => {
            for (side, button) in diamond(center(r), t, &[Side::Left, Side::Top, Side::Right, Side::Bottom, Side::Center]) {
                let dock = match side.dock() {
                    Some(s) => DockTarget::Beside(path.clone(), s),
                    None => DockTarget::Into(path.clone()),
                };
                out.push(Target { side, button, zone: strip(r, side, 0.5, 0.5), dock: Some(dock) });
            }
        }
        Some(Under::Editor(path, r)) => {
            for (side, button) in diamond(center(r), t, &FOUR) {
                let dock = side.dock().map(|s| DockTarget::Beside(path.clone(), s));
                out.push(Target { side, button, zone: strip(r, side, 0.22, 0.3), dock });
            }
        }
        None => {}
    }
    // The window edges.
    let edge = (t * 0.5).round();
    let mid_x = content.x + content.w / 2.0 - t / 2.0;
    let mid_y = content.y + content.h / 2.0 - t / 2.0;
    for (side, button) in [
        (Side::Left, Rect { x: content.x + edge, y: mid_y, w: t, h: t }),
        (Side::Top, Rect { x: mid_x, y: content.y + edge, w: t, h: t }),
        (Side::Right, Rect { x: content.x + content.w - edge - t, y: mid_y, w: t, h: t }),
        (Side::Bottom, Rect { x: mid_x, y: content.y + content.h - edge - t, w: t, h: t }),
    ] {
        let dock = side.dock().map(DockTarget::Edge);
        out.push(Target { side, button, zone: strip(content, side, 0.2, 0.3), dock });
    }
    out
}

/// Targets for a dragged editor tab: split the editor card `card`.
pub fn tab_targets(card: Rect, t: f32) -> Vec<Target> {
    diamond(center(card), t, &FOUR)
        .into_iter()
        .map(|(side, button)| Target { side, button, zone: strip(card, side, 0.5, 0.5), dock: None })
        .collect()
}

/// The target under the mouse (a little larger than its button).
pub fn hit(targets: &[Target], x: f32, y: f32) -> Option<Target> {
    targets
        .iter()
        .find(|t| {
            let m = t.button.w * 0.12;
            x >= t.button.x - m && x < t.button.x + t.button.w + m && y >= t.button.y - m && y < t.button.y + t.button.h + m
        })
        .cloned()
}

/// Buttons (a small window with the landing side filled) and the shaded zone of `hovered`.
pub fn draw(targets: &[Target], hovered: Option<&Target>, color: Rgba, theme: &Theme, scale: f32, cards: &mut Vec<RoundRect>) {
    if let Some(h) = hovered {
        let z = h.zone;
        let r = (7.0 * scale).round();
        cards.push(RoundRect::new(z.x, z.y, z.w, z.h, color.with_alpha(0x36), r));
        cards.push(RoundRect::outline(z.x, z.y, z.w, z.h, color.with_alpha(0xC0), r, (2.0 * scale).round()));
    }
    let shadow = Rgba(0, 0, 0, if theme.dark { 0x80 } else { 0x30 });
    for t in targets {
        let b = t.button;
        let r = (7.0 * scale).round();
        let on = hovered.is_some_and(|h| h.button == t.button);
        cards.push(RoundRect::shadow(b.x, b.y, b.w, b.h, shadow, r, (6.0 * scale).round()));
        cards.push(RoundRect::new(b.x, b.y, b.w, b.h, if on { color } else { theme.popup_border }, r));
        cards.push(RoundRect::new(b.x + 1.0, b.y + 1.0, b.w - 2.0, b.h - 2.0, if on { color.with_alpha(0x50) } else { theme.popup_bg }, r - 1.0));
        // The pictogram: a little window, the part where the window lands filled; window edge
        // targets show a bar along the edge.
        let m = (b.w * 0.24).round();
        let win = Rect { x: b.x + m, y: b.y + m, w: b.w - 2.0 * m, h: b.h - 2.0 * m };
        let line = (1.5 * scale).max(1.0);
        cards.push(RoundRect::outline(win.x, win.y, win.w, win.h, theme.panel_dim, 2.0 * scale, line));
        let edge = matches!(t.dock, Some(DockTarget::Edge(_)));
        let f = if edge { 0.28 } else { 0.45 };
        let part = strip(win, t.side, f, f);
        cards.push(RoundRect::new(part.x, part.y, part.w, part.h, color.with_alpha(if on { 0xFF } else { 0xB0 }), 2.0 * scale));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes_get_five_targets_the_editor_four_and_edges_four() {
        let content = Rect { x: 50.0, y: 0.0, w: 1000.0, h: 800.0 };
        let left = Rect { x: 50.0, y: 0.0, w: 200.0, h: 800.0 };
        let t = panel_targets(content, Some(Under::Pane(vec![0], left)), 40.0);
        assert_eq!(t.len(), 5 + 4);
        let c = t.iter().find(|x| x.side == Side::Center).unwrap();
        assert_eq!(c.dock, Some(DockTarget::Into(vec![0])));
        assert_eq!(c.zone, left);
        let b = t.iter().find(|x| x.side == Side::Bottom && x.dock == Some(DockTarget::Beside(vec![0], dock::Side::Bottom))).unwrap();
        assert_eq!(b.zone, Rect { y: 400.0, h: 400.0, ..left });
        // Window edges.
        let e = t.iter().find(|x| x.dock == Some(DockTarget::Edge(dock::Side::Right))).unwrap();
        assert!(e.zone.x > 800.0);
        // Over the editor: no center target.
        let editor = Rect { x: 250.0, y: 0.0, w: 800.0, h: 800.0 };
        let t2 = panel_targets(content, Some(Under::Editor(vec![1], editor)), 40.0);
        assert_eq!(t2.len(), 8);
        assert!(t2.iter().all(|x| x.side != Side::Center));
        // Hitting a button.
        let bt = c.button;
        assert_eq!(hit(&t, bt.x + 1.0, bt.y + 1.0).map(|x| x.side), Some(Side::Center));
        assert_eq!(hit(&t, 600.0, 300.0), None);
    }

    #[test]
    fn tabs_split_the_editor_card() {
        let card = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let t = tab_targets(card, 40.0);
        let top = t.iter().find(|x| x.side == Side::Top).unwrap();
        assert_eq!(top.zone, Rect { x: 0.0, y: 0.0, w: 800.0, h: 300.0 });
        assert!(t.iter().all(|x| x.dock.is_none()));
    }
}
