//! The settings page: a tab like in VS Code with all options of `settings.toml`, grouped, with
//! a filter field on top. Every change is written to the file right away (comments stay) and
//! applied. Mouse: checkboxes, choice buttons, − / + for numbers, click a field to type.

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Weight, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::settings::Settings;
use crate::theme::Theme;

pub enum Kind {
    Toggle,
    /// (value in the file, label on the button)
    Choice(&'static [(&'static str, &'static str)]),
    Number { min: f32, max: f32, step: f32, decimals: usize },
    Text { empty: &'static str },
}

pub struct Item {
    pub group: &'static str,
    pub table: Option<&'static str>,
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}

const fn item(
    group: &'static str,
    table: Option<&'static str>,
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
) -> Item {
    Item { group, table, key, label, help, kind }
}

const fn num(min: f32, max: f32, step: f32, decimals: usize) -> Kind {
    Kind::Number { min, max, step, decimals }
}

const E: Option<&str> = Some("editor");
const F: Option<&str> = Some("font");
const U: Option<&str> = Some("ui");
const A: Option<&str> = Some("ai");
const T: Option<&str> = Some("terminal");
const UP: Option<&str> = Some("update");

pub const ITEMS: &[Item] = &[
    item("Appearance", None, "theme", "Color theme", "Alt → v → c picks any theme, installed VS Code themes included (with preview). Alt → v → t switches dark/light.",
         Kind::Choice(&[("dark", "Dark"), ("light", "Light")])),
    item("Font", F, "family", "Font family", "Font for code and interface (Cascadia Mono, JetBrains Mono, Consolas, …).",
         Kind::Text { empty: "automatic" }),
    item("Font", F, "size", "Font size", "In points. Ctrl++ / Ctrl+- change it only until restart.", num(6.0, 48.0, 1.0, 1)),
    item("Font", F, "line_height", "Line height", "As a multiple of the font size.", num(1.0, 3.0, 0.1, 2)),
    item("Editor", E, "tab_width", "Tab width", "Width of a tab in columns.", num(1.0, 16.0, 1.0, 0)),
    item("Editor", E, "indent", "Indentation", "For files that have no indentation yet.",
         Kind::Choice(&[("spaces", "Spaces"), ("tabs", "Tabs")])),
    item("Editor", E, "auto_complete", "Suggestions while typing", "Ctrl+Space always shows them.", Kind::Toggle),
    item("Editor", E, "hover", "Hover info", "Show info when the mouse rests on a name for a moment.", Kind::Toggle),
    item("Editor", E, "minimap", "Minimap", "On the right: the shape of the code, a narrow marker bar, or nothing (Alt → v → m).",
         Kind::Choice(&[("blocks", "Blocks"), ("bar", "Bar"), ("off", "Off")])),
    item("Editor", E, "sticky_scroll", "Sticky scroll", "Keep the current namespace, class and method at the top (Alt → v → s).", Kind::Toggle),
    item("Editor", E, "inlay_hints", "Inlay hints", "Gray parameter names and types in the code (Alt → v → i).", Kind::Toggle),
    item("Editor", E, "code_lens", "CodeLens", "\"3 references\" above classes and methods; a click lists them (Alt → v → l).", Kind::Toggle),
    item("Interface", U, "font_scale", "Interface font size", "Size of panels, tabs and status bar relative to the code font.", num(0.6, 1.5, 0.05, 2)),
    item("Interface", U, "explorer_width", "Side panel width", "Width of the left and right panel areas in characters (dragging their edge overrides it).",
         num(12.0, 80.0, 1.0, 0)),
    item("AI", A, "enabled", "AI suggestions", "Gray suggestions while typing: Tab accepts, Ctrl+→ one word, Esc dismisses (Alt → a → t).", Kind::Toggle),
    item("AI", A, "provider", "Provider", "Local: a free model on this PC via Ollama, nothing leaves the PC. Claude: Anthropic's API with your own paid key (Alt → a → k).",
         Kind::Choice(&[("local", "Local (Ollama)"), ("claude", "Claude")])),
    item("AI", A, "local_model", "Local model", "Ollama model; use a base model (install: ollama pull qwen2.5-coder:7b-base).",
         Kind::Text { empty: "qwen2.5-coder:7b-base" }),
    item("AI", A, "model", "Claude model", "Haiku is fast and cheap, Sonnet better and a bit slower.",
         Kind::Choice(&[("haiku", "Haiku"), ("sonnet", "Sonnet")])),
    item("AI", A, "delay_ms", "Delay", "Pause after typing, in milliseconds, before a suggestion is requested.", num(100.0, 5000.0, 50.0, 0)),
    item("Terminal", T, "shell", "Shell", "For example cmd.exe, pwsh.exe or the path to Git's bash.exe. Applies to new terminals.",
         Kind::Text { empty: "automatic (PowerShell)" }),
    item("Terminal", T, "height", "Bottom panel height", "In percent of the window (dragging its edge overrides it).", num(15.0, 85.0, 5.0, 0)),
    item("Updates", UP, "url", "Update address", "Where update.json is: https://… or a folder path. Empty = no update checks.",
         Kind::Text { empty: "none (no update checks)" }),
    item("Updates", UP, "check", "Check automatically", "At most once a day; Wyven always asks before installing. \"Help: Check for Updates\" checks now.", Kind::Toggle),
];

/// The current value as shown and edited (strings without quotes).
pub fn value_of(s: &Settings, item: &Item) -> String {
    match (item.table, item.key) {
        // Old spellings ("dunkel", "hell") still count.
        (None, "theme") => Theme::by_name(&s.theme).map_or_else(|| s.theme.clone(), |t| t.name.to_string()),
        (F, "family") => s.font.family.clone(),
        (F, "size") => fmt(s.font.size, 1),
        (F, "line_height") => fmt(s.font.line_height, 2),
        (E, "tab_width") => s.editor.tab_width.to_string(),
        (E, "indent") => match s.editor.indent.as_str() {
            "tabs" | "tab" => "tabs".into(),
            _ => "spaces".into(),
        },
        (E, "auto_complete") => s.editor.auto_complete.to_string(),
        (E, "hover") => s.editor.hover.to_string(),
        (E, "minimap") => s.editor.minimap.clone(),
        (E, "sticky_scroll") => s.editor.sticky_scroll.to_string(),
        (E, "inlay_hints") => s.editor.inlay_hints.to_string(),
        (E, "code_lens") => s.editor.code_lens.to_string(),
        (U, "font_scale") => fmt(s.ui.font_scale, 2),
        (U, "explorer_width") => fmt(s.ui.explorer_width, 0),
        (A, "enabled") => s.ai.enabled.to_string(),
        (A, "provider") => s.ai.provider.clone(),
        (A, "local_model") => s.ai.local_model.clone(),
        (A, "model") => s.ai.model.clone(),
        (A, "delay_ms") => s.ai.delay_ms.to_string(),
        (T, "shell") => s.terminal.shell.clone(),
        (T, "height") => fmt(s.terminal.height, 0),
        (UP, "url") => s.update.url.clone(),
        (UP, "check") => s.update.check.to_string(),
        _ => String::new(),
    }
}

/// A number with at most `decimals` places and no trailing zeros ("14", "1.5").
fn fmt(v: f32, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// What the page asks the app to do.
#[derive(Debug, PartialEq)]
pub enum PageAction {
    None,
    /// Write `key = value` (TOML) into `[table]` and apply.
    Store { table: Option<&'static str>, key: &'static str, value: String },
    OpenFile,
    Message(String),
}

/// The TOML text for a new value of an item, or why the input is not valid.
pub fn toml_value(item: &Item, raw: &str) -> Result<String, String> {
    match &item.kind {
        Kind::Toggle => Ok(if raw == "true" { "true" } else { "false" }.into()),
        Kind::Choice(_) | Kind::Text { .. } => Ok(toml::Value::String(raw.trim().to_string()).to_string()),
        Kind::Number { min, max, decimals, .. } => {
            let v: f32 = raw.trim().replace(',', ".").parse().map_err(|_| format!("{}: “{raw}” is not a number", item.label))?;
            Ok(fmt(v.clamp(*min, *max), *decimals))
        }
    }
}

fn matches(item: &Item, filter: &str) -> bool {
    let f = filter.trim().to_lowercase();
    f.is_empty()
        || [item.group, item.label, item.help, item.key].iter().any(|t| t.to_lowercase().contains(&f))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    Filter,
    OpenFile,
    Toggle(usize),
    Choose(usize, &'static str),
    Step(usize, f32),
    Edit(usize),
}

#[derive(Clone, Copy)]
enum Paint {
    Title,
    Text,
    Dim,
    Accent,
    OnAccent,
    Field,
    Border,
    Selected,
}

struct Label {
    buffer: Buffer,
    x: f32,
    y: f32,
    paint: Paint,
}

pub struct SettingsPage {
    pub filter: String,
    /// The field being typed into: item and text so far.
    editing: Option<(usize, String)>,
    scroll_y: f32,
    area: Rect,
    ui: FontMetrics,
    family: String,
    labels: Vec<Label>,
    boxes: Vec<(Rect, Paint)>,
    hits: Vec<(Rect, Hit)>,
    content_h: f32,
    built_for: Option<String>,
}

impl SettingsPage {
    pub fn new(ui: FontMetrics, family: String) -> Self {
        Self {
            filter: String::new(),
            editing: None,
            scroll_y: 0.0,
            area: Rect::default(),
            ui,
            family,
            labels: Vec::new(),
            boxes: Vec::new(),
            hits: Vec::new(),
            content_h: 0.0,
            built_for: None,
        }
    }

    pub fn set_font(&mut self, ui: FontMetrics, family: String) {
        self.ui = ui;
        self.family = family;
        self.built_for = None;
    }

    pub fn set_area(&mut self, area: Rect) {
        if area != self.area {
            self.area = area;
            self.built_for = None;
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    fn pad(&self) -> f32 {
        (self.ui.cell_w * 3.0).round()
    }

    pub fn scroll_by(&mut self, lines: f32) {
        let max = (self.content_h - self.area.h).max(0.0);
        self.scroll_y = (self.scroll_y + lines * self.ui.line_h).clamp(0.0, max);
    }

    fn text(&self, fs: &mut FontSystem, s: &str, scale: f32, bold: bool, wrap: Option<f32>) -> (Buffer, f32, f32) {
        let m = Metrics::new(self.ui.size * scale, self.ui.line_h * scale);
        let mut b = Buffer::new(fs, m);
        b.set_wrap(if wrap.is_some() { Wrap::Word } else { Wrap::None });
        b.set_size(wrap, None);
        let mut attrs = Attrs::new().family(Family::Name(&self.family));
        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        b.set_text(s, &attrs, Shaping::Advanced, None);
        b.shape_until_scroll(fs, false);
        let w = b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        let h = b.layout_runs().last().map_or(m.line_height, |r| r.line_top + r.line_height);
        (b, w, h)
    }

    /// Lays the page out again when the settings, the filter, the field being edited or the
    /// size changed.
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, settings: &Settings) {
        let key = format!("{settings:?}|{}|{:?}|{:?}|{}", self.filter, self.editing, self.area, theme.name);
        if self.built_for.as_deref() == Some(key.as_str()) {
            return;
        }
        self.labels.clear();
        self.boxes.clear();
        self.hits.clear();
        let pad = self.pad();
        let lh = self.ui.line_h;
        let cw = self.ui.cell_w;
        let x0 = pad;
        let width = (self.area.w - 2.0 * pad).clamp(cw * 20.0, cw * 110.0);
        let mut y = pad * 0.6;

        let (b, _, h) = self.text(fs, "Settings", 1.5, true, None);
        self.labels.push(Label { buffer: b, x: x0, y, paint: Paint::Title });
        let (b, w, _) = self.text(fs, "Open settings.toml", 1.0, false, None);
        let lx = x0 + width - w;
        let ly = y + (h - lh) / 2.0;
        self.hits.push((Rect { x: lx, y: ly, w, h: lh }, Hit::OpenFile));
        self.labels.push(Label { buffer: b, x: lx, y: ly, paint: Paint::Accent });
        y += h + lh * 0.6;

        // Filter field.
        let fh = (lh * 1.5).round();
        let field = Rect { x: x0, y, w: width, h: fh };
        self.boxes.push((field, Paint::Border));
        self.boxes.push((Rect { x: field.x + 1.0, y: field.y + 1.0, w: field.w - 2.0, h: field.h - 2.0 }, Paint::Field));
        self.hits.push((field, Hit::Filter));
        let (shown, paint) = match (self.filter.is_empty(), self.editing.is_none()) {
            (true, true) => ("Search settings▏".to_string(), Paint::Dim),
            (true, false) => ("Search settings".to_string(), Paint::Dim),
            (false, true) => (format!("{}▏", self.filter), Paint::Text),
            (false, false) => (self.filter.clone(), Paint::Text),
        };
        let (b, _, _) = self.text(fs, &shown, 1.0, false, None);
        self.labels.push(Label { buffer: b, x: x0 + cw, y: y + (fh - lh) / 2.0, paint });
        y += fh + lh;

        let mut group = "";
        let mut any = false;
        for (i, it) in ITEMS.iter().enumerate() {
            if !matches(it, &self.filter) {
                continue;
            }
            any = true;
            if it.group != group {
                group = it.group;
                y += lh * 0.4;
                let (b, _, h) = self.text(fs, group, 1.2, true, None);
                self.labels.push(Label { buffer: b, x: x0, y, paint: Paint::Text });
                y += h + lh * 0.2;
                self.boxes.push((Rect { x: x0, y, w: width, h: 1.0 }, Paint::Border));
                y += lh * 0.5;
            }
            let (b, _, h) = self.text(fs, it.label, 1.0, true, None);
            self.labels.push(Label { buffer: b, x: x0, y, paint: Paint::Text });
            y += h;
            let (b, _, h) = self.text(fs, it.help, 0.95, false, Some(width));
            self.labels.push(Label { buffer: b, x: x0, y, paint: Paint::Dim });
            y += h + lh * 0.3;
            let value = value_of(settings, it);
            let ch = (lh * 1.4).round();
            let ty = y + (ch - lh) / 2.0;
            match &it.kind {
                Kind::Toggle => {
                    let on = value == "true";
                    let s = (lh * 0.95).round();
                    let bx = Rect { x: x0, y: y + (ch - s) / 2.0, w: s, h: s };
                    self.boxes.push((bx, if on { Paint::Selected } else { Paint::Border }));
                    if !on {
                        self.boxes.push((Rect { x: bx.x + 1.0, y: bx.y + 1.0, w: s - 2.0, h: s - 2.0 }, Paint::Field));
                    }
                    if on {
                        let (b, w, _) = self.text(fs, "✓", 1.0, true, None);
                        self.labels.push(Label { buffer: b, x: bx.x + (s - w) / 2.0, y: ty, paint: Paint::OnAccent });
                    }
                    let (b, w, _) = self.text(fs, if on { "On" } else { "Off" }, 1.0, false, None);
                    self.labels.push(Label { buffer: b, x: x0 + s + cw, y: ty, paint: Paint::Text });
                    self.hits.push((Rect { x: x0, y, w: s + cw * 2.0 + w, h: ch }, Hit::Toggle(i)));
                }
                Kind::Choice(options) => {
                    let mut x = x0;
                    for &(v, label) in options.iter() {
                        let (b, w, _) = self.text(fs, label, 1.0, false, None);
                        let bw = w + cw * 2.4;
                        let r = Rect { x, y, w: bw, h: ch };
                        let selected = v == value;
                        self.boxes.push((r, if selected { Paint::Selected } else { Paint::Border }));
                        if !selected {
                            self.boxes.push((Rect { x: r.x + 1.0, y: r.y + 1.0, w: r.w - 2.0, h: r.h - 2.0 }, Paint::Field));
                        }
                        let paint = if selected { Paint::OnAccent } else { Paint::Text };
                        self.labels.push(Label { buffer: b, x: x + cw * 1.2, y: ty, paint });
                        self.hits.push((r, Hit::Choose(i, v)));
                        x += bw + cw * 0.6;
                    }
                }
                Kind::Number { step, .. } => {
                    let bw = cw * 3.0;
                    let fw = cw * 10.0;
                    let minus = Rect { x: x0, y, w: bw, h: ch };
                    let f = Rect { x: x0 + bw + cw * 0.4, y, w: fw, h: ch };
                    let plus = Rect { x: f.x + fw + cw * 0.4, y, w: bw, h: ch };
                    for (r, sign, hit) in [(minus, "−", Hit::Step(i, -step)), (plus, "+", Hit::Step(i, *step))] {
                        self.boxes.push((r, Paint::Border));
                        self.boxes.push((Rect { x: r.x + 1.0, y: r.y + 1.0, w: r.w - 2.0, h: r.h - 2.0 }, Paint::Field));
                        let (b, w, _) = self.text(fs, sign, 1.0, true, None);
                        self.labels.push(Label { buffer: b, x: r.x + (r.w - w) / 2.0, y: ty, paint: Paint::Text });
                        self.hits.push((r, hit));
                    }
                    self.field(fs, i, f, &value, "", ty);
                }
                Kind::Text { empty } => {
                    let f = Rect { x: x0, y, w: width.min(cw * 60.0), h: ch };
                    self.field(fs, i, f, &value, empty, ty);
                }
            }
            y += ch + lh * 1.1;
        }
        if !any {
            let (b, _, _) = self.text(fs, "No settings match the search.", 1.0, false, None);
            self.labels.push(Label { buffer: b, x: x0, y, paint: Paint::Dim });
            y += lh;
        }
        self.content_h = y + pad;
        let max = (self.content_h - self.area.h).max(0.0);
        self.scroll_y = self.scroll_y.min(max);
        self.built_for = Some(key);
    }

    /// A text or number field; shows the typed text with a caret while it is edited.
    fn field(&mut self, fs: &mut FontSystem, i: usize, r: Rect, value: &str, empty: &str, ty: f32) {
        let editing = self.editing.as_ref().filter(|(e, _)| *e == i).map(|(_, t)| t.clone());
        self.boxes.push((r, if editing.is_some() { Paint::Selected } else { Paint::Border }));
        self.boxes.push((Rect { x: r.x + 1.0, y: r.y + 1.0, w: r.w - 2.0, h: r.h - 2.0 }, Paint::Field));
        let (shown, paint) = match editing {
            Some(t) => (format!("{t}▏"), Paint::Text),
            None if value.is_empty() => (empty.to_string(), Paint::Dim),
            None => (value.to_string(), Paint::Text),
        };
        let (b, _, _) = self.text(fs, &shown, 1.0, false, None);
        self.labels.push(Label { buffer: b, x: r.x + self.ui.cell_w, y: ty, paint });
        self.hits.push((r, Hit::Edit(i)));
    }

    pub fn draw<'a>(&'a self, theme: &Theme, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.background });
        let (ox, oy) = (a.x, a.y - self.scroll_y);
        let color = |p: Paint| match p {
            Paint::Title | Paint::Text => theme.text,
            Paint::Dim => theme.gutter_text,
            Paint::Accent => theme.accent,
            Paint::OnAccent => theme.background,
            Paint::Field => theme.current_line,
            Paint::Border => theme.divider,
            Paint::Selected => theme.accent,
        };
        for &(r, p) in &self.boxes {
            let y = oy + r.y;
            if y + r.h < a.y || y > a.y + a.h {
                continue;
            }
            // Clip to the page.
            let y0 = y.max(a.y);
            let y1 = (y + r.h).min(a.y + a.h);
            under.push(Quad { x: ox + r.x, y: y0, w: r.w.min(a.w), h: y1 - y0, color: color(p) });
        }
        for l in &self.labels {
            let y = oy + l.y;
            if y > a.y + a.h || y + self.ui.line_h * 3.0 < a.y - 200.0 {
                continue;
            }
            text.push(TextBlock {
                buffer: &l.buffer,
                x: ox + l.x,
                y,
                color: color(l.paint),
                clip: Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]),
            });
        }
    }

    fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        let (cx, cy) = (x - self.area.x, y - self.area.y + self.scroll_y);
        self.hits.iter().find(|(r, _)| cx >= r.x && cx < r.x + r.w && cy >= r.y && cy < r.y + r.h).map(|&(_, h)| h)
    }

    /// Ends typing into a field: the new value, if it changed.
    pub fn commit(&mut self, settings: &Settings) -> PageAction {
        let Some((i, typed)) = self.editing.take() else { return PageAction::None };
        let it = &ITEMS[i];
        if typed.trim() == value_of(settings, it) {
            return PageAction::None;
        }
        match toml_value(it, &typed) {
            Ok(value) => PageAction::Store { table: it.table, key: it.key, value },
            Err(e) => PageAction::Message(e),
        }
    }

    pub fn click(&mut self, x: f32, y: f32, settings: &Settings) -> PageAction {
        let hit = self.hit_at(x, y);
        if let Some(Hit::Edit(i)) = hit
            && self.editing.as_ref().is_some_and(|(e, _)| *e == i)
        {
            return PageAction::None; // already typing there
        }
        let committed = self.commit(settings);
        let action = match hit {
            None | Some(Hit::Filter) => PageAction::None,
            Some(Hit::OpenFile) => PageAction::OpenFile,
            Some(Hit::Edit(i)) => {
                self.editing = Some((i, value_of(settings, &ITEMS[i])));
                PageAction::None
            }
            Some(Hit::Toggle(i)) => {
                let it = &ITEMS[i];
                let on = value_of(settings, it) == "true";
                PageAction::Store { table: it.table, key: it.key, value: (!on).to_string() }
            }
            Some(Hit::Choose(i, v)) => {
                let it = &ITEMS[i];
                match toml_value(it, v) {
                    Ok(value) if v != value_of(settings, it) => PageAction::Store { table: it.table, key: it.key, value },
                    _ => PageAction::None,
                }
            }
            Some(Hit::Step(i, d)) => {
                let it = &ITEMS[i];
                let v: f32 = value_of(settings, it).parse().unwrap_or(0.0);
                match toml_value(it, &(v + d).to_string()) {
                    Ok(value) => PageAction::Store { table: it.table, key: it.key, value },
                    Err(e) => PageAction::Message(e),
                }
            }
        };
        // A click elsewhere first saves what was typed.
        if committed != PageAction::None && action == PageAction::None { committed } else { action }
    }

    /// Typed text goes into the field being edited, else into the filter.
    pub fn type_text(&mut self, t: &str) {
        let t: String = t.chars().filter(|c| !c.is_control()).collect();
        match &mut self.editing {
            Some((_, s)) => s.push_str(&t),
            None => {
                self.filter.push_str(&t);
                self.scroll_y = 0.0;
            }
        }
    }

    pub fn backspace(&mut self) {
        match &mut self.editing {
            Some((_, s)) => {
                s.pop();
            }
            None => {
                self.filter.pop();
            }
        }
    }

    /// Esc: stop typing into a field (without saving), else clear the filter.
    pub fn escape(&mut self) {
        if self.editing.take().is_none() {
            self.filter.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_a_value_and_stores_valid_toml() {
        let s = Settings::default();
        for it in ITEMS {
            let v = value_of(&s, it);
            assert!(!v.is_empty() || matches!(it.kind, Kind::Text { .. }), "{} has no value", it.key);
            let stored = toml_value(it, &v).unwrap();
            let table = it.table.map_or(String::new(), |t| format!("[{t}]\n"));
            let doc = format!("{table}{} = {stored}\n", it.key);
            let parsed = Settings::parse(&doc).unwrap_or_else(|e| panic!("{}: {e}", it.key));
            assert_eq!(value_of(&parsed, it), v, "{} round trip", it.key);
        }
    }

    #[test]
    fn numbers_are_clamped_and_formatted() {
        let size = ITEMS.iter().find(|i| i.key == "size").unwrap();
        assert_eq!(toml_value(size, "13,5").unwrap(), "13.5");
        assert_eq!(toml_value(size, "100").unwrap(), "48");
        assert!(toml_value(size, "big").is_err());
        let delay = ITEMS.iter().find(|i| i.key == "delay_ms").unwrap();
        assert_eq!(toml_value(delay, "412.7").unwrap(), "413");
        let shell = ITEMS.iter().find(|i| i.key == "shell").unwrap();
        let path = r"C:\Program Files\Git\bin\bash.exe";
        let doc = format!("[terminal]\nshell = {}\n", toml_value(shell, path).unwrap());
        assert_eq!(Settings::parse(&doc).unwrap().terminal.shell, path);
    }

    #[test]
    fn the_filter_finds_by_label_help_and_group() {
        let hits = |f: &str| ITEMS.iter().filter(|i| matches(i, f)).map(|i| i.key).collect::<Vec<_>>();
        assert_eq!(hits("minimap"), vec!["minimap"]);
        assert!(hits("ai").contains(&"delay_ms"));
        assert!(hits("OLLAMA").contains(&"local_model"));
        assert_eq!(hits("").len(), ITEMS.len());
    }
}
