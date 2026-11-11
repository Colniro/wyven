//! Colors written in code (`#1E90FF`, `#fff8`, `rgb(30, 144, 255)`, `rgba(…)`): found per line
//! for the small preview squares in CSS, HTML and XAML files, and the picker's palette.

use std::sync::OnceLock;

use regex::Regex;
use wyven_render::Rgba;

/// A color in a line: char start, char end, the color.
pub type Found = (usize, usize, Rgba);

fn pattern() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| {
        Regex::new(
            r"(?i)#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{4}|[0-9a-f]{3})\b|rgba?\(\s*\d{1,3}\s*,\s*\d{1,3}\s*,\s*\d{1,3}\s*(?:,\s*(?:0|1|0?\.\d+)\s*)?\)",
        )
        .unwrap()
    })
}

fn parse(text: &str) -> Option<Rgba> {
    if let Some(hex) = text.strip_prefix('#') {
        let d = |i: usize, n: usize| u8::from_str_radix(&hex[i..i + n], 16).ok();
        let short = |i: usize| d(i, 1).map(|v| v * 17);
        return match hex.len() {
            3 => Some(Rgba(short(0)?, short(1)?, short(2)?, 0xFF)),
            4 => Some(Rgba(short(0)?, short(1)?, short(2)?, short(3)?)),
            6 => Some(Rgba(d(0, 2)?, d(2, 2)?, d(4, 2)?, 0xFF)),
            8 => Some(Rgba(d(0, 2)?, d(2, 2)?, d(4, 2)?, d(6, 2)?)),
            _ => None,
        };
    }
    let inner = text.split_once('(')?.1.trim_end_matches(')');
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let c = |i: usize| parts.get(i)?.parse::<u16>().ok().map(|v| v.min(255) as u8);
    let a = parts.get(3).and_then(|v| v.parse::<f32>().ok()).map_or(0xFF, |v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some(Rgba(c(0)?, c(1)?, c(2)?, a))
}

/// The colors written in a line of code.
pub fn find(line: &str) -> Vec<Found> {
    pattern()
        .find_iter(line)
        .filter_map(|m| {
            let color = parse(m.as_str())?;
            let start = line[..m.start()].chars().count();
            Some((start, start + m.as_str().chars().count(), color))
        })
        .collect()
}

/// `#RRGGBB` (or `#RRGGBBAA` when not opaque).
pub fn hex(c: Rgba) -> String {
    if c.3 == 0xFF { format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2) } else { format!("#{:02X}{:02X}{:02X}{:02X}", c.0, c.1, c.2, c.3) }
}

fn hsl(h: f32, s: f32, l: f32) -> Rgba {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let v = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgba(v(r), v(g), v(b), 0xFF)
}

/// Columns and rows of the picker grid.
pub const COLS: usize = 12;
pub const ROWS: usize = 6;

/// The picker's colors, row by row: 5 lightness rows over 12 hues, then grays.
pub fn palette() -> Vec<Rgba> {
    let mut out = Vec::with_capacity(COLS * ROWS);
    for l in [0.85, 0.7, 0.55, 0.4, 0.25] {
        for i in 0..COLS {
            out.push(hsl(i as f32 * 30.0, 0.75, l));
        }
    }
    for i in 0..COLS {
        let v = (i as f32 / (COLS - 1) as f32 * 255.0).round() as u8;
        out.push(Rgba(v, v, v, 0xFF));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_and_rgb_colors_are_found() {
        let f = find("  color: #fff; background: #1E90FFcc; border: rgb(255, 0, 10); x: #12345");
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], (9, 13, Rgba(255, 255, 255, 255)));
        assert_eq!(f[1].2, Rgba(0x1E, 0x90, 0xFF, 0xCC));
        assert_eq!(f[2].2, Rgba(255, 0, 10, 255));
        assert_eq!(find("rgba(0,0,0,.5)")[0].2, Rgba(0, 0, 0, 128));
        assert_eq!(hex(Rgba(30, 144, 255, 255)), "#1E90FF");
        assert_eq!(palette().len(), COLS * ROWS);
        // Not a color: an id or a longer hex word.
        assert!(find("#main-header").is_empty());
    }
}
