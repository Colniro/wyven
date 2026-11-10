//! The find/replace box in the editor (Strg+F / Strg+H), top right like Visual Studio.
//!
//! Matches are recomputed whenever the query or the text changes and are shown in the editor;
//! the box itself only holds the query, the options and which match is current.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyvern_buffer::{Change, Document, Selection};
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Wrap};
use wyvern_search::{find_all, replacement_for, Query};

use crate::editor::{FontMetrics, Rect};
use crate::keymap::Keymap;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Find,
    Replace,
}

/// What the app should do after a key in the find box.
pub enum FindAction {
    None,
    /// Select this char range in the document and scroll to it.
    Select(usize, usize),
    /// The document text was changed (replace).
    Edited,
    Close,
}

pub struct FindBar {
    pub open: bool,
    pub focused: bool,
    pub replace_open: bool,
    field: Field,
    pub query: Query,
    pub replacement: String,
    /// Matches as char ranges, sorted.
    pub matches: Vec<(usize, usize)>,
    pub current: Option<usize>,
    error: Option<String>,
    /// (document id, version, query) the matches belong to.
    computed_for: Option<(usize, u64, Query)>,
    find_buf: Buffer,
    replace_buf: Buffer,
    /// Clickable areas of the option toggles, filled while drawing.
    pub toggle_rects: Vec<(Rect, char)>,
    pub rect: Rect,
}

impl FindBar {
    pub fn new(fs: &mut FontSystem, font: FontMetrics) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut find_buf = Buffer::new(fs, m);
        let mut replace_buf = Buffer::new(fs, m);
        find_buf.set_wrap(Wrap::None);
        replace_buf.set_wrap(Wrap::None);
        Self {
            open: false,
            focused: false,
            replace_open: false,
            field: Field::Find,
            query: Query::default(),
            replacement: String::new(),
            matches: Vec::new(),
            current: None,
            error: None,
            computed_for: None,
            find_buf,
            replace_buf,
            toggle_rects: Vec::new(),
            rect: Rect::default(),
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        let m = Metrics::new(font.size, font.line_h);
        self.find_buf.set_metrics(m);
        self.replace_buf.set_metrics(m);
    }

    /// Strg+F / Strg+H. `prefill` is the selection or the word under the cursor.
    pub fn show(&mut self, with_replace: bool, prefill: Option<String>) {
        self.open = true;
        self.focused = true;
        self.replace_open |= with_replace;
        self.field = if with_replace && !self.query.text.is_empty() && prefill.is_none() {
            Field::Replace
        } else {
            Field::Find
        };
        if let Some(text) = prefill.filter(|t| !t.is_empty() && !t.contains('\n')) {
            self.query.text = text;
        }
        self.computed_for = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focused = false;
        self.matches.clear();
        self.current = None;
        self.computed_for = None;
    }

    /// Recomputes matches if the text or the query changed. `doc_id` tells documents apart.
    pub fn refresh(&mut self, doc_id: usize, doc: &Document) {
        let key = (doc_id, doc.version(), self.query.clone());
        if self.computed_for.as_ref() == Some(&key) {
            return;
        }
        self.computed_for = Some(key);
        self.matches.clear();
        self.error = None;
        match self.query.compile() {
            Ok(Some(re)) => {
                let rope = doc.text();
                let text = rope.to_string();
                self.matches = find_all(&re, &text)
                    .into_iter()
                    .map(|r| (rope.byte_to_char(r.start), rope.byte_to_char(r.end)))
                    .collect();
            }
            Ok(None) => {}
            Err(e) => self.error = Some(e),
        }
        let head = doc.selection().primary().from();
        self.current = self
            .matches
            .iter()
            .position(|&(s, _)| s >= head)
            .or(if self.matches.is_empty() { None } else { Some(0) });
    }

    /// Moves to the next (or previous) match after the cursor.
    pub fn step(&mut self, doc: &Document, forward: bool) -> FindAction {
        if self.matches.is_empty() {
            return FindAction::None;
        }
        let sel = doc.selection().primary();
        let n = self.matches.len();
        // With only a cursor, a match starting right at it counts as "next"; with a selection
        // (usually the current match) the search continues after it.
        let i = if forward && sel.is_empty() {
            self.matches.iter().position(|&(s, _)| s >= sel.from()).unwrap_or(0)
        } else if forward {
            self.matches.iter().position(|&(s, _)| s > sel.from()).unwrap_or(0)
        } else {
            self.matches.iter().rposition(|&(s, _)| s < sel.from()).unwrap_or(n - 1)
        };
        self.current = Some(i);
        let (s, e) = self.matches[i];
        FindAction::Select(s, e)
    }

    fn compiled(&self) -> Option<regex::Regex> {
        self.query.compile().ok().flatten()
    }

    /// Alt+R: replaces the current match (if the selection is on it) and moves on.
    pub fn replace_one(&mut self, doc: &mut Document) -> FindAction {
        let Some(re) = self.compiled() else { return FindAction::None };
        let sel = doc.selection().primary();
        let on_match = self.matches.iter().any(|&(s, e)| s == sel.from() && e == sel.to());
        if !on_match {
            return self.step(doc, true);
        }
        let matched = doc.text().slice(sel.from()..sel.to()).to_string();
        let new = replacement_for(&re, &self.query, &matched, &self.replacement);
        doc.replace_range(sel.from(), sel.to(), &new);
        let after = sel.from() + new.chars().count();
        doc.set_selection(Selection::point(after));
        self.computed_for = None;
        FindAction::Edited
    }

    /// Alt+A: replaces every match in the document as one undo step.
    pub fn replace_all(&mut self, doc: &mut Document) -> (FindAction, usize) {
        let Some(re) = self.compiled() else { return (FindAction::None, 0) };
        let rope = doc.text();
        let changes: Vec<Change> = self
            .matches
            .iter()
            .map(|&(s, e)| {
                let matched = rope.slice(s..e).to_string();
                Change { start: s, end: e, text: replacement_for(&re, &self.query, &matched, &self.replacement) }
            })
            .collect();
        let n = changes.len();
        if n > 0 {
            doc.apply_changes(changes);
        }
        self.computed_for = None;
        (FindAction::Edited, n)
    }

    pub fn toggle(&mut self, option: char) {
        match option {
            'c' => self.query.case_sensitive = !self.query.case_sensitive,
            'w' => self.query.whole_word = !self.query.whole_word,
            'e' => self.query.regex = !self.query.regex,
            _ => {}
        }
        self.computed_for = None;
    }

    /// Keys while the find box has focus.
    pub fn key(&mut self, event: &KeyEvent, mods: ModifiersState, doc: &mut Document) -> FindAction {
        let alt = mods.alt_key() && !mods.control_key();
        let ctrl = mods.control_key() && !mods.alt_key();
        let key = event.key_without_modifiers();
        if alt {
            if let Key::Character(c) = &key {
                match c.as_str() {
                    "c" | "w" | "e" => self.toggle(c.chars().next().unwrap()),
                    "r" => return self.replace_one(doc),
                    "a" => return self.replace_all(doc).0,
                    _ => {}
                }
            }
            return FindAction::None;
        }
        let text = match self.field {
            Field::Find => &mut self.query.text,
            Field::Replace => &mut self.replacement,
        };
        match &key {
            Key::Named(NamedKey::Escape) => return FindAction::Close,
            Key::Named(NamedKey::Enter) if self.field == Field::Replace && !mods.shift_key() => {
                return self.replace_one(doc);
            }
            Key::Named(NamedKey::Enter) => return self.step(doc, !mods.shift_key()),
            Key::Named(NamedKey::F3) => return self.step(doc, !mods.shift_key()),
            Key::Named(NamedKey::Tab) => {
                if self.replace_open {
                    self.field = if self.field == Field::Find { Field::Replace } else { Field::Find };
                }
                return FindAction::None;
            }
            Key::Named(NamedKey::Backspace) => {
                if ctrl {
                    text.clear();
                } else {
                    text.pop();
                }
            }
            Key::Character(c) if ctrl && c.as_str() == "a" => {}
            Key::Character(c) if ctrl && c.as_str() == "h" => {
                self.replace_open = true;
                self.field = Field::Replace;
                return FindAction::None;
            }
            Key::Character(c) if ctrl && c.as_str() == "f" => {
                self.field = Field::Find;
                return FindAction::None;
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    text.push_str(&t);
                } else {
                    return FindAction::None;
                }
            }
        }
        if self.field == Field::Find {
            self.computed_for = None;
        }
        FindAction::None
    }

    pub fn paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or("");
        match self.field {
            Field::Find => {
                self.query.text.push_str(line);
                self.computed_for = None;
            }
            Field::Replace => self.replacement.push_str(line),
        }
    }

    /// Selection text or the word under the cursor, to prefill the query.
    pub fn prefill_from(doc: &Document) -> Option<String> {
        let r = doc.selection().primary();
        let rope = doc.text();
        if !r.is_empty() {
            return Some(rope.slice(r.from()..r.to()).to_string());
        }
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let (mut s, mut e) = (r.head, r.head);
        while s > 0 && is_word(rope.char(s - 1)) {
            s -= 1;
        }
        while e < rope.len_chars() && is_word(rope.char(e)) {
            e += 1;
        }
        (e > s).then(|| rope.slice(s..e).to_string())
    }

    pub fn count_text(&self) -> String {
        if let Some(e) = &self.error {
            return e.clone();
        }
        match (self.matches.len(), self.current) {
            (0, _) if self.query.text.is_empty() => String::new(),
            (0, _) => "Keine Treffer".into(),
            (n, Some(i)) => format!("{} von {n}", i + 1),
            (n, None) => format!("{n} Treffer"),
        }
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, family: &str) {
        let attrs = Attrs::new().family(Family::Name(family));
        let find_line = if self.query.text.is_empty() { "Suchen".to_string() } else { self.query.text.clone() };
        let find_color = if self.query.text.is_empty() { theme.panel_dim } else { theme.tab_text_active };
        let count = self.count_text();
        let count_color = if self.error.is_some() { theme.error } else { theme.panel_dim };
        let spans = [(find_line, find_color), (format!("   {count}"), count_color)];
        self.find_buf.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.find_buf.shape_until_scroll(fs, false);
        let (rep, rep_color) = if self.replacement.is_empty() {
            ("Ersetzen  (Alt+R eins · Alt+A alle)".to_string(), theme.panel_dim)
        } else {
            (self.replacement.clone(), theme.tab_text_active)
        };
        self.replace_buf.set_rich_text(
            [(rep.as_str(), attrs.clone().color(rep_color.to_glyphon()))],
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.replace_buf.shape_until_scroll(fs, false);
    }

    fn row_h(font: FontMetrics) -> f32 {
        (font.line_h * 1.35).round()
    }

    /// Computes where the box and its option toggles are (top right of the editor area).
    pub fn place(&mut self, font: FontMetrics, editor: Rect, scale: f32) {
        let cw = font.cell_w;
        let pad = cw;
        let toggles_w = 9.0 * cw;
        let w = (46.0 * cw).min(editor.w - 2.0 * cw);
        let rows = if self.replace_open { 2.0 } else { 1.0 };
        let row_h = Self::row_h(font);
        let h = rows * row_h + pad * 0.6;
        let x = editor.x + editor.w - w - 2.0 * cw;
        let y = editor.y + 0.5 * font.line_h;
        self.rect = Rect { x, y, w, h };
        let b = scale.round().max(1.0);
        self.toggle_rects = (0..3)
            .map(|i| {
                let tx = x + w - toggles_w - pad * 0.5 + i as f32 * 3.0 * cw;
                (Rect { x: tx, y: y + pad * 0.3, w: 2.6 * cw, h: row_h - 2.0 * b }, ['c', 'w', 'e'][i])
            })
            .collect();
    }

    /// Draws the box at the place computed by [`place`](Self::place).
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        font: FontMetrics,
        scale: f32,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let Rect { x, y, w, h } = self.rect;
        let cw = font.cell_w;
        let lh = font.line_h;
        let pad = cw;
        let toggles_w = 9.0 * cw;
        let rows = if self.replace_open { 2 } else { 1 };
        let row_h = Self::row_h(font);
        let b = scale.round().max(1.0);
        let border = if self.focused { theme.accent } else { theme.popup_border };
        quads.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: border });
        quads.push(Quad { x, y, w, h, color: theme.popup_bg });
        let field_w = w - toggles_w - 2.0 * pad;
        for (i, field) in [Field::Find, Field::Replace].iter().enumerate().take(rows) {
            let fy = y + pad * 0.3 + i as f32 * row_h;
            let active = self.focused && self.field == *field;
            let bg = if active { theme.panel_bg } else { theme.popup_bg };
            quads.push(Quad { x: x + pad * 0.5, y: fy, w: field_w + pad, h: row_h - 2.0 * b, color: bg });
            if active {
                let len = match field {
                    Field::Find => self.query.text.chars().count(),
                    Field::Replace => self.replacement.chars().count(),
                };
                quads.push(Quad {
                    x: x + pad + len as f32 * cw,
                    y: fy + (row_h - lh) / 2.0,
                    w: (2.0 * scale).round(),
                    h: lh,
                    color: theme.cursor,
                });
            }
        }
        let opts = [self.query.case_sensitive, self.query.whole_word, self.query.regex];
        for ((r, _), on) in self.toggle_rects.iter().zip(opts) {
            if on {
                quads.push(Quad { x: r.x, y: r.y, w: r.w, h: r.h, color: theme.accent.with_alpha(0x55) });
            }
        }
        let clip = Some([x as i32, y as i32, (x + w - toggles_w - pad * 0.5) as i32, (y + h) as i32]);
        let ty = y + pad * 0.3 + (row_h - lh) / 2.0 - b;
        text.push(TextBlock { buffer: &self.find_buf, x: x + pad, y: ty, color: theme.panel_text, clip });
        if self.replace_open {
            text.push(TextBlock { buffer: &self.replace_buf, x: x + pad, y: ty + row_h, color: theme.panel_text, clip });
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.open && x >= self.rect.x && x < self.rect.x + self.rect.w && y >= self.rect.y && y < self.rect.y + self.rect.h
    }

    /// Mouse click inside the box: toggles an option or focuses the box.
    pub fn click(&mut self, x: f32, y: f32) {
        self.focused = true;
        if let Some(&(_, key)) = self.toggle_rects.iter().find(|(r, _)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) {
            self.toggle(key);
        }
    }
}

/// Draws the "Aa W .*" labels over the toggle areas (needs its own buffer because the find
/// box text is clipped before the toggles).
pub struct ToggleLabels {
    buffer: Buffer,
}

impl ToggleLabels {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: &str, theme: &Theme) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(font.size, font.line_h));
        buffer.set_wrap(Wrap::None);
        let mut s = Self { buffer };
        s.set(fs, family, theme);
        s
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.buffer.set_metrics(Metrics::new(font.size, font.line_h));
    }

    pub fn set(&mut self, fs: &mut FontSystem, family: &str, theme: &Theme) {
        let attrs = Attrs::new().family(Family::Name(family)).color(theme.panel_text.to_glyphon());
        self.buffer.set_text(" Aa  W  .*", &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(fs, false);
    }

    pub fn draw<'a>(&'a self, bar: &FindBar, font: FontMetrics, theme: &Theme, text: &mut Vec<TextBlock<'a>>) {
        if let Some((first, _)) = bar.toggle_rects.first() {
            let y = first.y + (first.h - font.line_h) / 2.0;
            text.push(TextBlock { buffer: &self.buffer, x: first.x - font.cell_w * 0.3, y, color: theme.panel_text, clip: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> FindBar {
        let mut fs = FontSystem::new();
        FindBar::new(&mut fs, FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 })
    }

    #[test]
    fn finds_steps_and_wraps() {
        let mut doc = Document::from_str("a wert b Wert c wert");
        let mut f = bar();
        f.show(false, Some("wert".into()));
        f.refresh(0, &doc);
        assert_eq!(f.matches, vec![(2, 6), (9, 13), (16, 20)]);
        assert_eq!(f.count_text(), "1 von 3");
        let FindAction::Select(s, e) = f.step(&doc, true) else { panic!() };
        doc.set_selection(Selection::single(wyvern_buffer::Range::new(s, e)));
        let FindAction::Select(s, _) = f.step(&doc, true) else { panic!() };
        assert_eq!(s, 9);
        doc.set_selection(Selection::point(17));
        let FindAction::Select(s, _) = f.step(&doc, true) else { panic!() };
        assert_eq!(s, 2, "wraps to the first match");
        f.toggle('c');
        f.refresh(0, &doc);
        assert_eq!(f.matches.len(), 2, "case sensitive now");
    }

    #[test]
    fn replace_one_and_all_with_regex_groups() {
        let mut doc = Document::from_str("x=1; y=2; z=3;");
        let mut f = bar();
        f.show(true, Some(r"(\w)=(\d)".into()));
        f.toggle('e');
        f.replacement = "$2:$1".into();
        f.refresh(0, &doc);
        assert_eq!(f.matches.len(), 3);
        // First Alt+R only moves to the match, the second replaces it.
        let FindAction::Select(s, e) = f.replace_one(&mut doc) else { panic!() };
        doc.set_selection(Selection::single(wyvern_buffer::Range::new(s, e)));
        f.replace_one(&mut doc);
        assert_eq!(doc.text().to_string(), "1:x; y=2; z=3;");
        f.refresh(0, &doc);
        let (_, n) = f.replace_all(&mut doc);
        assert_eq!(n, 2);
        assert_eq!(doc.text().to_string(), "1:x; 2:y; 3:z;");
        doc.undo();
        assert_eq!(doc.text().to_string(), "1:x; y=2; z=3;", "replace all is one undo step");
    }

    #[test]
    fn broken_regex_shows_message() {
        let doc = Document::from_str("abc");
        let mut f = bar();
        f.show(false, Some("(".into()));
        f.toggle('e');
        f.refresh(0, &doc);
        assert!(f.count_text().starts_with("Ungültiger regulärer Ausdruck"));
    }
}
