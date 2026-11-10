//! Colors. Phase 1 ships one dark theme; theme files and a light theme come with the settings.

use wyvern_render::Rgba;
use wyvern_syntax::Highlight;

pub struct Theme {
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
}

impl Theme {
    pub const fn dark() -> Self {
        Self {
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
        }
    }

    /// Syntax colors, inspired by Visual Studio's dark theme.
    pub fn highlight(&self, h: Highlight) -> Rgba {
        use Highlight::*;
        match h {
            Keyword => Rgba::rgb(0x56, 0x9C, 0xD6),
            Type | Namespace => Rgba::rgb(0x4E, 0xC9, 0xB0),
            Function => Rgba::rgb(0xDC, 0xDC, 0xAA),
            String => Rgba::rgb(0xCE, 0x91, 0x78),
            Escape => Rgba::rgb(0xD7, 0xBA, 0x7D),
            Comment => Rgba::rgb(0x6A, 0x99, 0x55),
            Number => Rgba::rgb(0xB5, 0xCE, 0xA8),
            Constant => Rgba::rgb(0x4F, 0xC1, 0xFF),
            Property | Parameter => Rgba::rgb(0x9C, 0xDC, 0xFE),
            Variable | Operator | Label => self.text,
            Punctuation => Rgba::rgb(0xA8, 0xAE, 0xB8),
            Attribute => Rgba::rgb(0xC5, 0x86, 0xC0),
            Tag => Rgba::rgb(0x56, 0x9C, 0xD6),
            Heading => Rgba::rgb(0x56, 0x9C, 0xD6),
            Link => Rgba::rgb(0x37, 0x94, 0xFF),
        }
    }
}
