//! The docking compass, as in Visual Studio: while a panel or an editor tab is dragged, drop
//! targets appear in the middle of the window part under the mouse (a diamond) and, for panels,
//! at the window edges. Hovering a target shades the place where it would land.
//!
//! Panels can land left, right or at the bottom, or as a tab in the area under the mouse
//! (center). Editor tabs split the editor to the left, right, top or bottom.

use wyven_render::{Rgba, RoundRect};

use crate::dock::Area;
use crate::editor::Rect;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
    /// As a tab of the area under the mouse.
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Target {
    pub side: Side,
    /// The button to drop on.
    pub button: Rect,
    /// Where the window would land (shaded while hovering).
    pub zone: Rect,
    /// The dock area a panel lands in.
    pub area: Option<Area>,
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

/// Targets for a dragged panel. `content`: everything right of the activity bar and above the
/// status bar; `areas`: the open dock areas; `under`: the card under the mouse, and the area it
/// is (None for the editor).
pub fn panel_targets(content: Rect, areas: &[(Area, Rect)], under: Option<(Rect, Option<Area>)>, t: f32) -> Vec<Target> {
    let open = |a: Area| areas.iter().find(|(x, _)| *x == a).map(|(_, r)| *r);
    let zone = |side: Side| -> Rect {
        match side {
            Side::Left => open(Area::Left).unwrap_or(Rect { w: (content.w * 0.22).round(), ..content }),
            Side::Right => open(Area::Right).unwrap_or(Rect {
                x: content.x + content.w - (content.w * 0.22).round(),
                w: (content.w * 0.22).round(),
                ..content
            }),
            _ => open(Area::Bottom).unwrap_or(Rect {
                y: content.y + (content.h * 0.68).round(),
                h: content.h - (content.h * 0.68).round(),
                ..content
            }),
        }
    };
    let area_of = |side: Side| match side {
        Side::Left => Area::Left,
        Side::Right => Area::Right,
        _ => Area::Bottom,
    };
    let mut out = Vec::new();
    // The diamond over the window part under the mouse.
    if let Some((card, card_area)) = under {
        let mut sides = vec![Side::Left, Side::Right, Side::Bottom];
        if card_area.is_some() {
            sides.push(Side::Center);
        }
        for (side, button) in diamond(center(card), t, &sides) {
            let (zone, area) = match (side, card_area) {
                (Side::Center, Some(a)) => (card, a),
                _ => (zone(side), area_of(side)),
            };
            out.push(Target { side, button, zone, area: Some(area) });
        }
    }
    // The window edges.
    let edge = (t * 0.5).round();
    let mid_y = content.y + content.h / 2.0 - t / 2.0;
    for (side, button) in [
        (Side::Left, Rect { x: content.x + edge, y: mid_y, w: t, h: t }),
        (Side::Right, Rect { x: content.x + content.w - edge - t, y: mid_y, w: t, h: t }),
        (Side::Bottom, Rect { x: content.x + content.w / 2.0 - t / 2.0, y: content.y + content.h - edge - t, w: t, h: t }),
    ] {
        out.push(Target { side, button, zone: zone(side), area: Some(area_of(side)) });
    }
    out
}

/// Targets for a dragged editor tab: split the editor card `card`.
pub fn tab_targets(card: Rect, t: f32) -> Vec<Target> {
    let half_w = (card.w / 2.0).round();
    let half_h = (card.h / 2.0).round();
    diamond(center(card), t, &[Side::Left, Side::Right, Side::Top, Side::Bottom])
        .into_iter()
        .map(|(side, button)| {
            let zone = match side {
                Side::Left => Rect { w: half_w, ..card },
                Side::Right => Rect { x: card.x + card.w - half_w, w: half_w, ..card },
                Side::Top => Rect { h: half_h, ..card },
                _ => Rect { y: card.y + card.h - half_h, h: half_h, ..card },
            };
            Target { side, button, zone, area: None }
        })
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
        .copied()
}

/// Buttons (a small window with the landing side filled) and the shaded zone of `hovered`.
pub fn draw(targets: &[Target], hovered: Option<Target>, color: Rgba, theme: &Theme, scale: f32, cards: &mut Vec<RoundRect>) {
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
        let on = hovered == Some(*t);
        cards.push(RoundRect::shadow(b.x, b.y, b.w, b.h, shadow, r, (6.0 * scale).round()));
        cards.push(RoundRect::new(b.x, b.y, b.w, b.h, if on { color } else { theme.popup_border }, r));
        cards.push(RoundRect::new(b.x + 1.0, b.y + 1.0, b.w - 2.0, b.h - 2.0, if on { color.with_alpha(0x50) } else { theme.popup_bg }, r - 1.0));
        // The pictogram: a little window, the part where the window lands filled.
        let m = (b.w * 0.24).round();
        let win = Rect { x: b.x + m, y: b.y + m, w: b.w - 2.0 * m, h: b.h - 2.0 * m };
        let line = (1.5 * scale).max(1.0);
        cards.push(RoundRect::outline(win.x, win.y, win.w, win.h, theme.panel_dim, 2.0 * scale, line));
        let part = match t.side {
            Side::Left => Rect { w: (win.w * 0.4).round(), ..win },
            Side::Right => Rect { x: win.x + win.w - (win.w * 0.4).round(), w: (win.w * 0.4).round(), ..win },
            Side::Top => Rect { h: (win.h * 0.45).round(), ..win },
            Side::Bottom => Rect { y: win.y + win.h - (win.h * 0.45).round(), h: (win.h * 0.45).round(), ..win },
            Side::Center => win,
        };
        cards.push(RoundRect::new(part.x, part.y, part.w, part.h, color.with_alpha(if on { 0xFF } else { 0xB0 }), 2.0 * scale));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panels_get_a_diamond_and_edge_targets() {
        let content = Rect { x: 50.0, y: 0.0, w: 1000.0, h: 800.0 };
        let left = Rect { x: 50.0, y: 0.0, w: 200.0, h: 800.0 };
        let areas = [(Area::Left, left)];
        // Over the explorer area: the center target puts the panel in there as a tab.
        let t = panel_targets(content, &areas, Some((left, Some(Area::Left))), 40.0);
        assert_eq!(t.len(), 4 + 3);
        let c = t.iter().find(|x| x.side == Side::Center).unwrap();
        assert_eq!(c.area, Some(Area::Left));
        assert_eq!(c.zone, left);
        // The right target's zone: a strip at the right edge, since nothing is docked there.
        let r = t.iter().find(|x| x.side == Side::Right).unwrap();
        assert!(r.zone.x > 800.0);
        // Over the editor: no center target.
        let editor = Rect { x: 250.0, y: 0.0, w: 800.0, h: 800.0 };
        assert!(panel_targets(content, &areas, Some((editor, None)), 40.0).iter().all(|x| x.side != Side::Center));
        // Hitting a button.
        let b = c.button;
        assert_eq!(hit(&t, b.x + 1.0, b.y + 1.0).map(|x| x.side), Some(Side::Center));
        assert_eq!(hit(&t, 600.0, 400.0), None);
    }

    #[test]
    fn tabs_split_the_editor_card() {
        let card = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let t = tab_targets(card, 40.0);
        let top = t.iter().find(|x| x.side == Side::Top).unwrap();
        assert_eq!(top.zone, Rect { x: 0.0, y: 0.0, w: 800.0, h: 300.0 });
        assert!(t.iter().all(|x| x.area.is_none()));
    }
}
