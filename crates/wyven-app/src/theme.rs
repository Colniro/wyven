//! Colors. Two built-in themes, "dark" and "light", plus every VS Code theme that is installed
//! (see `vscode_theme`); chosen in settings.toml or via the search field (`>Theme`).

use wyven_render::Rgba;
use wyven_syntax::Highlight;

/// Theme names as written in settings.toml, with their display names.
pub const THEMES: &[(&str, &str)] = &[("dark", "Wyven Dark"), ("light", "Wyven Light"), ("andromeda", "Wyven Andromeda")];

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
    /// Colors of their own for variables, parameters, operators and markup tags; `None`:
    /// variables and operators like the text, parameters like properties, tags like keywords.
    pub variable: Option<Rgba>,
    pub parameter: Option<Rgba>,
    pub operator: Option<Rgba>,
    pub tag: Option<Rgba>,
    /// Keywords and built-in types in groups of their own (control flow, modifiers, number,
    /// text and truth types …); `None`: all keywords alike.
    pub groups: Option<KeywordGroups>,
}

/// Colors for groups of keywords, so similar words look alike and different kinds differ.
pub struct KeywordGroups {
    /// if, else, for, while, return, try, throw, await …
    pub control: Rgba,
    /// public, private, static, readonly, async, override …
    pub modifier: Rgba,
    /// Whole numbers: int, long, short, byte, uint …
    pub integer: Rgba,
    /// Numbers with a fraction: float, double, decimal (close to `integer`).
    pub fraction: Rgba,
    /// string, char
    pub text: Rgba,
    /// bool
    pub truth: Rgba,
    /// void, object, var, dynamic
    pub other_type: Rgba,
}

impl KeywordGroups {
    /// The group color of a keyword or built-in type (C#, also TypeScript/Rust words).
    pub fn color_of(&self, word: &str) -> Option<Rgba> {
        Some(match word {
            "if" | "else" | "for" | "foreach" | "while" | "do" | "switch" | "case" | "default" | "break" | "continue"
            | "return" | "goto" | "try" | "catch" | "finally" | "throw" | "yield" | "await" | "when" | "loop" | "match"
            | "in" => self.control,
            "public" | "private" | "protected" | "internal" | "static" | "readonly" | "const" | "virtual" | "override"
            | "abstract" | "sealed" | "async" | "extern" | "unsafe" | "volatile" | "partial" | "required" | "pub"
            | "mut" | "export" => self.modifier,
            "int" | "uint" | "long" | "ulong" | "short" | "ushort" | "byte" | "sbyte" | "nint" | "nuint" | "i8" | "i16"
            | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "usize" | "isize" | "number" | "bigint" => self.integer,
            "float" | "double" | "decimal" | "f32" | "f64" => self.fraction,
            "string" | "char" | "str" | "String" => self.text,
            "bool" | "boolean" => self.truth,
            "void" | "object" | "var" | "dynamic" | "any" | "unknown" => self.other_type,
            _ => return None,
        })
    }
}

/// Comments with a marker get a color of their own (like the "Better Comments" extension):
/// `// !` or FIXME/BUG red, `// ?` blue, `// TODO` orange, `// *` green, `// NOTE` cyan,
/// `////` dimmed.
pub fn comment_marker_color(theme: &Theme, comment: &str) -> Option<Rgba> {
    let t = comment.trim_start();
    let body = t
        .strip_prefix("///")
        .or_else(|| t.strip_prefix("//"))
        .or_else(|| t.strip_prefix("/*"))
        .or_else(|| t.strip_prefix("<!--"))
        .or_else(|| t.strip_prefix('#'))
        .or_else(|| t.strip_prefix("--"))
        .unwrap_or(t);
    if body.starts_with("//") {
        return Some(theme.gutter_text);
    }
    let body = body.trim_start_matches('*').trim_start();
    let starts = |w: &str| body.len() >= w.len() && body[..w.len()].eq_ignore_ascii_case(w);
    let raw = t.trim_start_matches(['/', '#', '-', '<', '!']).trim_start();
    if raw.starts_with('!') || starts("FIXME") || starts("BUG") || starts("HACK") {
        Some(theme.error)
    } else if raw.starts_with('?') {
        Some(theme.info)
    } else if starts("TODO") {
        Some(theme.warning)
    } else if t.trim_start_matches(['/', '#']).trim_start().starts_with('*') && !t.starts_with("/*") {
        Some(theme.git_added)
    } else if starts("NOTE") || starts("INFO") {
        Some(theme.accent)
    } else {
        None
    }
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
    pub git_added: Rgba,
    pub git_modified: Rgba,
    pub git_deleted: Rgba,
    /// Line backgrounds in the diff view.
    pub diff_removed: Rgba,
    pub diff_added: Rgba,
    pub diff_filler: Rgba,
    /// Background of the line where the debugger is paused.
    pub debug_line: Rgba,
    /// Behind the cards (the gaps between panels and editors, the activity and status bar).
    pub frame: Rgba,
    /// Dark background (decides derived colors and the terminal's default palette).
    pub dark: bool,
    /// Terminal colors from a VS Code theme (black, red, … bright white).
    pub ansi: Option<[Rgba; 16]>,
    pub syntax: SyntaxColors,
}

impl Theme {
    /// Theme by settings name ("dark"/"dunkel", "light"/"hell").
    pub fn by_name(name: &str) -> Option<Self> {
        match name.trim().to_lowercase().as_str() {
            "dark" | "dunkel" | "wyven dark" | "wyven dunkel" => Some(Self::dark()),
            "light" | "hell" | "wyven light" | "wyven hell" => Some(Self::light()),
            "andromeda" | "wyven andromeda" => Some(Self::andromeda()),
            _ => None,
        }
    }

    pub const fn dark() -> Self {
        Self {
            name: "dark",
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
            git_added: Rgba::rgb(0x48, 0x9E, 0x5A),
            git_modified: Rgba::rgb(0x2F, 0x7F, 0xC8),
            git_deleted: Rgba::rgb(0xD0, 0x4A, 0x4A),
            diff_removed: Rgba(0xC0, 0x40, 0x40, 0x40),
            diff_added: Rgba(0x40, 0xA0, 0x50, 0x40),
            diff_filler: Rgba(0x00, 0x00, 0x00, 0x40),
            debug_line: Rgba(0xE8, 0xB0, 0x3A, 0x40),
            frame: Rgba::rgb(0x0D, 0x0F, 0x13),
            dark: true,
            ansi: None,
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
                variable: None,
                parameter: None,
                operator: None,
                tag: None,
                groups: None,
            },
        }
    }

    /// Wyven Andromeda: the colors of the Andromeda theme (EliverLara) on its gray-blue
    /// background, with extra colors for variables, parameters, operators and punctuation, a
    /// faint cyan glow on the current line, and cyan as accent.
    pub const fn andromeda() -> Self {
        const CYAN: Rgba = Rgba::rgb(0x5E, 0xD4, 0xC0);
        const PURPLE: Rgba = Rgba::rgb(0xB4, 0x86, 0xD8);
        const PINK: Rgba = Rgba::rgb(0xE0, 0x6F, 0xB4);
        const YELLOW: Rgba = Rgba::rgb(0xE6, 0xD3, 0x8A);
        const GREEN: Rgba = Rgba::rgb(0xA4, 0xCF, 0x8C);
        const ORANGE: Rgba = Rgba::rgb(0xE0, 0xA6, 0x6A);
        const RED: Rgba = Rgba::rgb(0xDE, 0x83, 0x72);
        const BLUE: Rgba = Rgba::rgb(0x8C, 0xB6, 0xE6);
        const TEXT: Rgba = Rgba::rgb(0xD5, 0xCE, 0xD9);
        Self {
            name: "andromeda",
            background: Rgba::rgb(0x23, 0x26, 0x2E),
            text: TEXT,
            gutter_text: Rgba::rgb(0x5C, 0x5F, 0x6B),
            gutter_current: CYAN,
            // A faint cyan glow.
            current_line: Rgba::rgb(0x23, 0x32, 0x38),
            selection: Rgba(0x3D, 0x43, 0x52, 0xE0),
            cursor: CYAN,
            status_bg: Rgba::rgb(0x1B, 0x1D, 0x24),
            status_text: Rgba::rgb(0x9A, 0x96, 0xA3),
            status_accent: CYAN,
            panel_bg: Rgba::rgb(0x20, 0x23, 0x2B),
            panel_text: TEXT,
            panel_dim: Rgba::rgb(0x7E, 0x7A, 0x88),
            row_selected: Rgba::rgb(0x2C, 0x3B, 0x45),
            row_selected_unfocused: Rgba::rgb(0x29, 0x2D, 0x37),
            accent: CYAN,
            tab_bar: Rgba::rgb(0x1E, 0x20, 0x27),
            tab_active: Rgba::rgb(0x23, 0x26, 0x2E),
            tab_text: Rgba::rgb(0x8A, 0x86, 0x93),
            tab_text_active: Rgba::rgb(0xEC, 0xE7, 0xEF),
            divider: Rgba::rgb(0x2D, 0x31, 0x3B),
            popup_bg: Rgba::rgb(0x27, 0x2B, 0x34),
            popup_border: Rgba::rgb(0x3B, 0x40, 0x4D),
            popup_key: YELLOW,
            error: RED,
            warning: YELLOW,
            info: BLUE,
            git_added: GREEN,
            git_modified: YELLOW,
            git_deleted: RED,
            diff_removed: Rgba(0xEE, 0x5D, 0x43, 0x38),
            diff_added: Rgba(0x96, 0xE0, 0x72, 0x30),
            diff_filler: Rgba(0x00, 0x00, 0x00, 0x40),
            debug_line: Rgba(0xFF, 0xE6, 0x6D, 0x30),
            frame: Rgba::rgb(0x19, 0x1B, 0x21),
            dark: true,
            ansi: Some([
                Rgba::rgb(0x23, 0x26, 0x2E),
                RED,
                GREEN,
                YELLOW,
                BLUE,
                PINK,
                CYAN,
                TEXT,
                Rgba::rgb(0x74, 0x6F, 0x77),
                Rgba::rgb(0xFF, 0x7A, 0x63),
                Rgba::rgb(0xB0, 0xF0, 0x8C),
                Rgba::rgb(0xFF, 0xF0, 0x8F),
                Rgba::rgb(0x9C, 0xCB, 0xFF),
                Rgba::rgb(0xFF, 0x4D, 0xC4),
                Rgba::rgb(0x5C, 0xF5, 0xDB),
                Rgba::rgb(0xFF, 0xFF, 0xFF),
            ]),
            syntax: SyntaxColors {
                keyword: PURPLE,
                type_: YELLOW,
                function: YELLOW,
                string: GREEN,
                escape: ORANGE,
                comment: Rgba::rgb(0x8B, 0x8D, 0x95),
                number: ORANGE,
                constant: RED,
                property: BLUE,
                punctuation: Rgba::rgb(0xB9, 0xA2, 0xB4),
                attribute: ORANGE,
                link: BLUE,
                variable: Some(CYAN),
                parameter: Some(Rgba::rgb(0xE8, 0xBC, 0x92)),
                operator: Some(RED),
                tag: Some(Rgba::rgb(0xE0, 0x73, 0x95)),
                groups: Some(KeywordGroups {
                    // Control flow pink-violet, modifiers a quiet lavender, declarations purple.
                    control: Rgba::rgb(0xD4, 0x84, 0xC0),
                    modifier: Rgba::rgb(0x98, 0x8F, 0xBF),
                    // Built-in types in teal-blue shades: whole and fractional numbers close
                    // together, text greener, truth warmer.
                    integer: Rgba::rgb(0x6F, 0xBF, 0xD6),
                    fraction: Rgba::rgb(0x84, 0xAE, 0xDE),
                    text: Rgba::rgb(0x7C, 0xC9, 0xAE),
                    truth: Rgba::rgb(0xD6, 0xA6, 0x82),
                    other_type: Rgba::rgb(0xA3, 0xAD, 0xC2),
                }),
            },
        }
    }

    pub const fn light() -> Self {
        Self {
            name: "light",
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
            git_added: Rgba::rgb(0x2E, 0xA0, 0x43),
            git_modified: Rgba::rgb(0x1F, 0x6F, 0xC5),
            git_deleted: Rgba::rgb(0xCF, 0x22, 0x2E),
            diff_removed: Rgba(0xFF, 0x80, 0x80, 0x50),
            diff_added: Rgba(0x80, 0xE0, 0x90, 0x60),
            diff_filler: Rgba(0x00, 0x00, 0x00, 0x14),
            debug_line: Rgba(0xF5, 0xC0, 0x40, 0x70),
            frame: Rgba::rgb(0xE3, 0xE6, 0xEB),
            dark: false,
            ansi: None,
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
                variable: None,
                parameter: None,
                operator: None,
                tag: None,
                groups: None,
            },
        }
    }

    /// Colors for the integrated terminal.
    pub fn terminal_palette(&self) -> wyven_terminal::Palette {
        let rgb = |c: Rgba| [c.0, c.1, c.2];
        let hex = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        if let Some(a) = self.ansi {
            let sel = self.selection;
            return wyven_terminal::Palette {
                foreground: rgb(self.text),
                background: rgb(self.panel_bg),
                cursor: rgb(self.cursor),
                selection: [sel.0, sel.1, sel.2],
                ansi: a.map(rgb),
            };
        }
        let ansi = if !self.dark {
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
        wyven_terminal::Palette {
            foreground: rgb(self.text),
            background: rgb(self.panel_bg),
            cursor: rgb(self.cursor),
            selection: [sel.0, sel.1, sel.2],
            ansi: ansi.map(hex),
        }
    }

    /// A built-in theme or a VS Code theme by its label ("Dracula Theme").
    pub fn resolve(name: &str) -> Result<Self, String> {
        if let Some(t) = Self::by_name(name) {
            return Ok(t);
        }
        let want = name.trim().to_lowercase();
        let entry = crate::vscode_theme::discover()
            .into_iter()
            .find(|e| e.label.to_lowercase() == want)
            .ok_or_else(|| format!("Unknown theme “{name}”"))?;
        crate::vscode_theme::load(&entry.label, &entry.path)
    }

    pub fn display_name(&self) -> &'static str {
        THEMES.iter().find(|(n, _)| *n == self.name).map_or(self.name, |(_, d)| d)
    }

    /// The color of a token: like [`Theme::highlight`], with keyword groups and colored
    /// comments (`text`: the token's text).
    pub fn token_color(&self, h: Highlight, text: &str, colored_comments: bool) -> Rgba {
        match h {
            Highlight::Comment if colored_comments => comment_marker_color(self, text).unwrap_or(self.syntax.comment),
            Highlight::Keyword | Highlight::Type => {
                self.syntax.groups.as_ref().and_then(|g| g.color_of(text.trim())).unwrap_or_else(|| self.highlight(h))
            }
            _ => self.highlight(h),
        }
    }

    pub fn highlight(&self, h: Highlight) -> Rgba {
        use Highlight::*;
        let s = &self.syntax;
        match h {
            Keyword | Heading => s.keyword,
            Tag => s.tag.unwrap_or(s.keyword),
            Type | Namespace => s.type_,
            Function => s.function,
            String => s.string,
            Escape => s.escape,
            Comment => s.comment,
            Number => s.number,
            Constant => s.constant,
            Property => s.property,
            Parameter => s.parameter.unwrap_or(s.property),
            Variable => s.variable.unwrap_or(self.text),
            Operator => s.operator.unwrap_or(self.text),
            Label => self.text,
            Punctuation => s.punctuation,
            Attribute => s.attribute,
            Link => s.link,
        }
    }
}
