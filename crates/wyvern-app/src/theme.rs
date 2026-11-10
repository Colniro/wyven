//! Colors. Two built-in themes, "dunkel" and "hell"; chosen in settings.toml or via the
//! search field (`>Theme`). Importing VS Code themes is planned for phase 4.

use wyvern_render::Rgba;
use wyvern_syntax::Highlight;

/// Theme names as written in settings.toml, with their display names.
pub const THEMES: &[(&str, &str)] = &[("dunkel", "Wyvern Dunkel"), ("hell", "Wyvern Hell")];

/// Syntax colors of one theme.
pub struct SyntaxColors {
    pub keyword: Rgba,
    pub type_: Rgba,
    pub function: Rgba,
    pub string: Rgba,
    pub escape: Rgba,
    pub comment: Rgba,
    pub number: Rgba,
    pub constant: Rgba,
    pub property: Rgba,
    pub punctuation: Rgba,
    pub attribute: Rgba,
    pub link: Rgba,
}

pub struct Theme {
    pub name: &'static str,
    pub background: Rgba,
    pub text: Rgba,
    pub gutter_text: Rgba,
    pub gutter_current: Rgba,
    pub current_line: Rgba,
    pub selection: Rgba,
    pub cursor: Rgba,
    pub status_bg: Rgba,
    pub status_text: Rgba,
    pub status_accent: Rgba,
    pub panel_bg: Rgba,
    pub panel_text: Rgba,
    pub panel_dim: Rgba,
    pub row_selected: Rgba,
    pub row_selected_unfocused: Rgba,
    pub accent: Rgba,
    pub tab_bar: Rgba,
    pub tab_active: Rgba,
    pub tab_text: Rgba,
    pub tab_text_active: Rgba,
    pub divider: Rgba,
    pub popup_bg: Rgba,
    pub popup_border: Rgba,
    pub popup_key: Rgba,
    pub error: Rgba,
    pub warning: Rgba,
    pub info: Rgba,
    pub syntax: SyntaxColors,
}

impl Theme {
    /// Theme by settings name ("dunkel"/"dark", "hell"/"light").
    pub fn by_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "dunkel" | "dark" | "wyvern dunkel" => Some(Self::dark()),
            "hell" | "light" | "wyvern hell" => Some(Self::light()),
            _ => None,
        }
    }

    pub const fn dark() -> Self {
        Self {
            name: "dunkel",
            background: Rgba::rgb(0x16, 0x18, 0x1D),
            text: Rgba::rgb(0xD4, 0xD7, 0xDE),
            gutter_text: Rgba::rgb(0x4B, 0x52, 0x5E),
            gutter_current: Rgba::rgb(0xC8, 0xCC, 0xD4),
            current_line: Rgba::rgb(0x1E, 0x21, 0x28),
            selection: Rgba(0x26, 0x4F, 0x78, 0xC0),
            cursor: Rgba::rgb(0xE8, 0xEA, 0xEE),
            status_bg: Rgba::rgb(0x10, 0x12, 0x16),
            status_text: Rgba::rgb(0x8A, 0x91, 0x9E),
            status_accent: Rgba::rgb(0xC8, 0xA0, 0x60),
            panel_bg: Rgba::rgb(0x12, 0x14, 0x18),
            panel_text: Rgba::rgb(0xB8, 0xBD, 0xC6),
            panel_dim: Rgba::rgb(0x6B, 0x72, 0x7F),
            row_selected: Rgba::rgb(0x23, 0x3A, 0x55),
            row_selected_unfocused: Rgba::rgb(0x22, 0x26, 0x2E),
            accent: Rgba::rgb(0xE0, 0xB4, 0x6C),
            tab_bar: Rgba::rgb(0x10, 0x12, 0x16),
            tab_active: Rgba::rgb(0x16, 0x18, 0x1D),
            tab_text: Rgba::rgb(0x7A, 0x81, 0x8D),
            tab_text_active: Rgba::rgb(0xE2, 0xE5, 0xEA),
            divider: Rgba::rgb(0x24, 0x28, 0x30),
            popup_bg: Rgba::rgb(0x1C, 0x1F, 0x26),
            popup_border: Rgba::rgb(0x34, 0x3A, 0x46),
            popup_key: Rgba::rgb(0xE0, 0xB4, 0x6C),
            error: Rgba::rgb(0xF1, 0x4C, 0x4C),
            warning: Rgba::rgb(0xCC, 0xA7, 0x00),
            info: Rgba::rgb(0x3F, 0x9C, 0xD6),
            // Inspired by Visual Studio's dark theme.
            syntax: SyntaxColors {
                keyword: Rgba::rgb(0x56, 0x9C, 0xD6),
                type_: Rgba::rgb(0x4E, 0xC9, 0xB0),
                function: Rgba::rgb(0xDC, 0xDC, 0xAA),
                string: Rgba::rgb(0xCE, 0x91, 0x78),
                escape: Rgba::rgb(0xD7, 0xBA, 0x7D),
                comment: Rgba::rgb(0x6A, 0x99, 0x55),
                number: Rgba::rgb(0xB5, 0xCE, 0xA8),
                constant: Rgba::rgb(0x4F, 0xC1, 0xFF),
                property: Rgba::rgb(0x9C, 0xDC, 0xFE),
                punctuation: Rgba::rgb(0xA8, 0xAE, 0xB8),
                attribute: Rgba::rgb(0xC5, 0x86, 0xC0),
                link: Rgba::rgb(0x37, 0x94, 0xFF),
            },
        }
    }

    pub const fn light() -> Self {
        Self {
            name: "hell",
            background: Rgba::rgb(0xFB, 0xFB, 0xFC),
            text: Rgba::rgb(0x1F, 0x23, 0x28),
            gutter_text: Rgba::rgb(0xA0, 0xA6, 0xAF),
            gutter_current: Rgba::rgb(0x2B, 0x30, 0x37),
            current_line: Rgba::rgb(0xF0, 0xF2, 0xF5),
            selection: Rgba(0xAD, 0xD6, 0xFF, 0xD0),
            cursor: Rgba::rgb(0x1F, 0x23, 0x28),
            status_bg: Rgba::rgb(0xEC, 0xEE, 0xF1),
            status_text: Rgba::rgb(0x5A, 0x61, 0x6B),
            status_accent: Rgba::rgb(0x9A, 0x62, 0x00),
            panel_bg: Rgba::rgb(0xF3, 0xF4, 0xF6),
            panel_text: Rgba::rgb(0x2B, 0x30, 0x37),
            panel_dim: Rgba::rgb(0x7A, 0x81, 0x8B),
            row_selected: Rgba::rgb(0xCC, 0xE4, 0xFF),
            row_selected_unfocused: Rgba::rgb(0xE3, 0xE6, 0xEA),
            accent: Rgba::rgb(0xB0, 0x6E, 0x00),
            tab_bar: Rgba::rgb(0xEC, 0xEE, 0xF1),
            tab_active: Rgba::rgb(0xFB, 0xFB, 0xFC),
            tab_text: Rgba::rgb(0x6A, 0x71, 0x7B),
            tab_text_active: Rgba::rgb(0x1F, 0x23, 0x28),
            divider: Rgba::rgb(0xDA, 0xDD, 0xE2),
            popup_bg: Rgba::rgb(0xFF, 0xFF, 0xFF),
            popup_border: Rgba::rgb(0xC8, 0xCD, 0xD4),
            popup_key: Rgba::rgb(0xB0, 0x6E, 0x00),
            error: Rgba::rgb(0xD3, 0x2F, 0x2F),
            warning: Rgba::rgb(0xB5, 0x89, 0x00),
            info: Rgba::rgb(0x1F, 0x6F, 0xC5),
            // Inspired by Visual Studio's light theme.
            syntax: SyntaxColors {
                keyword: Rgba::rgb(0x00, 0x00, 0xFF),
                type_: Rgba::rgb(0x26, 0x7F, 0x99),
                function: Rgba::rgb(0x79, 0x5E, 0x26),
                string: Rgba::rgb(0xA3, 0x15, 0x15),
                escape: Rgba::rgb(0xB7, 0x6E, 0x00),
                comment: Rgba::rgb(0x00, 0x80, 0x00),
                number: Rgba::rgb(0x09, 0x86, 0x58),
                constant: Rgba::rgb(0x00, 0x70, 0xC1),
                property: Rgba::rgb(0x00, 0x10, 0x80),
                punctuation: Rgba::rgb(0x55, 0x5B, 0x63),
                attribute: Rgba::rgb(0xAF, 0x00, 0xDB),
                link: Rgba::rgb(0x00, 0x66, 0xCC),
            },
        }
    }

    /// Colors for the integrated terminal.
    pub fn terminal_palette(&self) -> wyvern_terminal::Palette {
        let rgb = |c: Rgba| [c.0, c.1, c.2];
        let hex = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        let ansi = if self.name == "hell" {
            // VS Code light terminal colors.
            [
                0x000000, 0xCD3131, 0x107C10, 0x949800, 0x0451A5, 0xBC05BC, 0x0598BC, 0x555555,
                0x666666, 0xCD3131, 0x14CE14, 0xB5BA00, 0x0451A5, 0xBC05BC, 0x0598BC, 0xA5A5A5,
            ]
        } else {
            // "Campbell", the Windows Terminal default.
            [
                0x0C0C0C, 0xC50F1F, 0x13A10E, 0xC19C00, 0x0037DA, 0x881798, 0x3A96DD, 0xCCCCCC,
                0x767676, 0xE74856, 0x16C60C, 0xF9F1A5, 0x3B78FF, 0xB4009E, 0x61D6D6, 0xF2F2F2,
            ]
        };
        let sel = self.selection;
        wyvern_terminal::Palette {
            foreground: rgb(self.text),
            background: rgb(self.panel_bg),
            cursor: rgb(self.cursor),
            selection: [sel.0, sel.1, sel.2],
            ansi: ansi.map(hex),
        }
    }

    pub fn display_name(&self) -> &'static str {
        THEMES.iter().find(|(n, _)| *n == self.name).map_or(self.name, |(_, d)| d)
    }

    pub fn highlight(&self, h: Highlight) -> Rgba {
        use Highlight::*;
        let s = &self.syntax;
        match h {
            Keyword | Tag | Heading => s.keyword,
            Type | Namespace => s.type_,
            Function => s.function,
            String => s.string,
            Escape => s.escape,
            Comment => s.comment,
            Number => s.number,
            Constant => s.constant,
            Property | Parameter => s.property,
            Variable | Operator | Label => self.text,
            Punctuation => s.punctuation,
            Attribute => s.attribute,
            Link => s.link,
        }
    }
}
