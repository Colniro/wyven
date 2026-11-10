//! The central search field (`Strg+Q`).
//!
//! One input, four modes chosen by the first character:
//! - no prefix: files of the project (fuzzy, e.g. `prgcs` finds `Program.cs`)
//! - `>` commands, with their shortcut
//! - `@` symbols of the current file (classes, methods, properties …)
//! - `:` go to line (`42` or `42:7`)

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use wyvern_buffer::{CommentMode, Direction};
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};
use wyvern_syntax::Symbol;

use crate::editor::{FontMetrics, Rect};
use crate::keymap::Command;
use crate::leader;
use crate::theme::Theme;

const MAX_RESULTS: usize = 100;
const VISIBLE_ROWS: usize = 12;

#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Key(Command),
    Leader(leader::Action),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    OpenFile(String),
    Run(Cmd),
    Line(usize, Option<usize>),
    Byte(usize),
    Nothing,
}

struct Entry {
    label: String,
    detail: String,
    action: Action,
    hits: Vec<u32>,
}

/// Everything the palette may search, gathered by the app when the query changes.
pub struct Sources<'a> {
    pub files: Option<Arc<Vec<String>>>,
    pub root: Option<&'a Path>,
    /// Open files, most recently used first.
    pub open: Vec<PathBuf>,
    pub symbols: Vec<Symbol>,
    pub line_count: usize,
    pub has_symbols: bool,
}

fn commands() -> Vec<(&'static str, &'static str, Cmd)> {
    use leader::Action as L;
    use Cmd::{Key as K, Leader as Ld};
    use Command as C;
    vec![
        ("Datei: Neue Datei", "Strg+N", K(C::NewFile)),
        ("Datei: Öffnen …", "Strg+O", K(C::OpenFile)),
        ("Datei: Speichern", "Strg+S", K(C::Save)),
        ("Datei: Speichern unter …", "Strg+Shift+S", K(C::SaveAs)),
        ("Datei: Tab schließen", "Strg+W", K(C::CloseTab)),
        ("Datei: Alle schließen", "Alt f w", Ld(L::CloseAll)),
        ("Datei: Geschlossenen Tab wieder öffnen", "Strg+Shift+T", K(C::ReopenClosed)),
        ("Projekt: Ordner öffnen …", "Alt p o", Ld(L::OpenFolder)),
        ("Projekt: Zuletzt geöffnet", "Alt p r", Ld(L::RecentProjects)),
        ("Projekt: Explorer fokussieren", "Alt p e", Ld(L::FocusExplorer)),
        ("Ansicht: Explorer ein/aus", "Alt e", Ld(L::ToggleExplorer)),
        ("Ansicht: Schrift größer", "Strg++", K(C::FontSize(1))),
        ("Ansicht: Schrift kleiner", "Strg+-", K(C::FontSize(-1))),
        ("Ansicht: Schriftgröße zurücksetzen", "Strg+0", K(C::FontSizeReset)),
        ("Gehe zu: Zeile …", "Strg+G", K(C::OpenPalette(":"))),
        ("Gehe zu: Symbol in Datei …", "Alt j s", K(C::OpenPalette("@"))),
        ("Gehe zu: Datei …", "Strg+Q", K(C::OpenPalette(""))),
        ("Bearbeiten: Rückgängig", "Strg+Z", K(C::Undo)),
        ("Bearbeiten: Wiederholen", "Strg+Y", K(C::Redo)),
        ("Bearbeiten: Alles auswählen", "Strg+A", K(C::SelectAll)),
        ("Bearbeiten: Nächstes Vorkommen auswählen", "Strg+D", K(C::AddNextOccurrence)),
        ("Bearbeiten: Zeile nach oben verschieben", "Alt+↑", K(C::MoveLines(Direction::Up))),
        ("Bearbeiten: Zeile nach unten verschieben", "Alt+↓", K(C::MoveLines(Direction::Down))),
        ("Bearbeiten: Zeile nach oben duplizieren", "Alt+Shift+↑", K(C::DuplicateLines(Direction::Up))),
        ("Bearbeiten: Zeile nach unten duplizieren", "Alt+Shift+↓", K(C::DuplicateLines(Direction::Down))),
        ("Bearbeiten: Zeile löschen", "Strg+Shift+L", K(C::DeleteLines)),
        ("Bearbeiten: Auskommentieren", "Strg+K Strg+C", K(C::Comment(CommentMode::Add))),
        ("Bearbeiten: Kommentar entfernen", "Strg+K Strg+U", K(C::Comment(CommentMode::Remove))),
        ("Bearbeiten: Kommentar umschalten", "Strg+#", K(C::Comment(CommentMode::Toggle))),
    ]
}

pub struct Palette {
    pub query: String,
    selected: usize,
    scroll: usize,
    entries: Vec<Entry>,
    matcher: Matcher,
    input: Buffer,
    list: Buffer,
    /// Files mode was asked before the index was ready.
    pub waiting_for_index: bool,
    /// Box position from the last draw, for mouse clicks.
    rows_rect: Rect,
    row_h: f32,
}

impl Palette {
    pub fn new(fs: &mut FontSystem, font: FontMetrics) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut input = Buffer::new(fs, m);
        let mut list = Buffer::new(fs, m);
        input.set_wrap(Wrap::None);
        list.set_wrap(Wrap::None);
        Self {
            query: String::new(),
            selected: 0,
            scroll: 0,
            entries: Vec::new(),
            matcher: Matcher::new(Config::DEFAULT),
            input,
            list,
            waiting_for_index: false,
            rows_rect: Rect::default(),
            row_h: font.line_h,
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        let m = Metrics::new(font.size, font.line_h);
        self.input.set_metrics(m);
        self.list.set_metrics(m);
    }

    pub fn reset(&mut self, prefix: &str) {
        self.query = prefix.to_string();
        self.selected = 0;
        self.scroll = 0;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(n) as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + VISIBLE_ROWS {
            self.scroll = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    pub fn selected_action(&self) -> Action {
        self.entries.get(self.selected).map_or(Action::Nothing, |e| e.action.clone())
    }

    /// Action of the row under a window coordinate.
    pub fn action_at(&self, x: f32, y: f32) -> Option<Action> {
        let r = self.rows_rect;
        if x < r.x || x >= r.x + r.w || y < r.y || y >= r.y + r.h {
            return None;
        }
        let i = self.scroll + ((y - r.y) / self.row_h) as usize;
        self.entries.get(i).map(|e| e.action.clone())
    }

    fn mode(&self) -> (&'static str, &str) {
        for prefix in [">", "@", ":"] {
            if let Some(rest) = self.query.strip_prefix(prefix) {
                return (prefix, rest.trim_start());
            }
        }
        ("", self.query.trim())
    }

    /// Recomputes the result list for the current query.
    pub fn update(&mut self, src: &Sources<'_>) {
        let (mode, q) = self.mode();
        let q = q.to_string();
        self.waiting_for_index = false;
        self.entries = match mode {
            ">" => self.match_commands(&q),
            "@" => self.match_symbols(&q, src),
            ":" => line_entries(&q, src.line_count),
            _ => self.match_files(&q, src),
        };
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        if self.query.is_empty() {
            self.selected = 0;
        }
        self.scroll = self.scroll.min(self.selected);
    }

    fn pattern(q: &str) -> Pattern {
        Pattern::parse(q, CaseMatching::Smart, Normalization::Smart)
    }

    /// Scores `candidates` against `q`; returns (index, score, hit positions) best first.
    fn rank<'c>(
        matcher: &mut Matcher,
        q: &str,
        candidates: impl Iterator<Item = &'c str>,
    ) -> Vec<(usize, u32, Vec<u32>)> {
        let pattern = Self::pattern(q);
        let mut buf = Vec::new();
        let mut scored: Vec<(usize, u32, usize)> = candidates
            .enumerate()
            .filter_map(|(i, c)| {
                let score = pattern.score(Utf32Str::new(c, &mut buf), matcher)?;
                Some((i, score, c.len()))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        scored.truncate(MAX_RESULTS);
        scored.into_iter().map(|(i, s, _)| (i, s, Vec::new())).collect()
    }

    fn hits(matcher: &mut Matcher, q: &str, text: &str) -> Vec<u32> {
        let pattern = Self::pattern(q);
        let mut buf = Vec::new();
        let mut hits = Vec::new();
        pattern.indices(Utf32Str::new(text, &mut buf), matcher, &mut hits);
        hits.sort_unstable();
        hits.dedup();
        hits
    }

    fn match_files(&mut self, q: &str, src: &Sources<'_>) -> Vec<Entry> {
        let Some(root) = src.root else {
            return vec![Entry {
                label: "Kein Ordner offen – Alt → p → o öffnet einen".into(),
                detail: String::new(),
                action: Action::Run(Cmd::Leader(leader::Action::OpenFolder)),
                hits: Vec::new(),
            }];
        };
        let rel = |p: &Path| p.strip_prefix(root).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
        let open: Vec<String> = src.open.iter().filter_map(|p| rel(p)).collect();
        let Some(files) = &src.files else {
            self.waiting_for_index = true;
            return open
                .into_iter()
                .map(|f| file_entry(f, "offen", Vec::new()))
                .collect();
        };
        if q.is_empty() {
            let mut out: Vec<Entry> =
                open.iter().map(|f| file_entry(f.clone(), "offen", Vec::new())).collect();
            out.extend(
                files
                    .iter()
                    .filter(|f| !open.contains(f))
                    .take(MAX_RESULTS)
                    .map(|f| file_entry(f.clone(), "", Vec::new())),
            );
            return out;
        }
        self.matcher.config = Config::DEFAULT.match_paths();
        let ranked = Self::rank(&mut self.matcher, q, files.iter().map(String::as_str));
        ranked
            .into_iter()
            .map(|(i, _, _)| {
                let f = files[i].clone();
                let hits = Self::hits(&mut self.matcher, q, &f);
                let detail = if open.contains(&f) { "offen" } else { "" };
                file_entry(f, detail, hits)
            })
            .collect()
    }

    fn match_commands(&mut self, q: &str) -> Vec<Entry> {
        self.matcher.config = Config::DEFAULT;
        let cmds = commands();
        let order: Vec<(usize, Vec<u32>)> = if q.is_empty() {
            (0..cmds.len()).map(|i| (i, Vec::new())).collect()
        } else {
            Self::rank(&mut self.matcher, q, cmds.iter().map(|c| c.0))
                .into_iter()
                .map(|(i, _, _)| (i, Self::hits(&mut self.matcher, q, cmds[i].0)))
                .collect()
        };
        order
            .into_iter()
            .map(|(i, hits)| Entry {
                label: cmds[i].0.into(),
                detail: cmds[i].1.into(),
                action: Action::Run(cmds[i].2.clone()),
                hits,
            })
            .collect()
    }

    fn match_symbols(&mut self, q: &str, src: &Sources<'_>) -> Vec<Entry> {
        if !src.has_symbols {
            return vec![hint("Für diese Dateiart gibt es noch keine Symbole", "")];
        }
        if src.symbols.is_empty() {
            return vec![hint("Keine Symbole in dieser Datei", "")];
        }
        self.matcher.config = Config::DEFAULT;
        let order: Vec<(usize, Vec<u32>)> = if q.is_empty() {
            (0..src.symbols.len()).map(|i| (i, Vec::new())).collect()
        } else {
            Self::rank(&mut self.matcher, q, src.symbols.iter().map(|s| s.name.as_str()))
                .into_iter()
                .map(|(i, _, _)| (i, Self::hits(&mut self.matcher, q, &src.symbols[i].name)))
                .collect()
        };
        order
            .into_iter()
            .map(|(i, hits)| {
                let s = &src.symbols[i];
                Entry {
                    label: s.name.clone(),
                    detail: format!("{} · Zeile {}", s.kind, s.line + 1),
                    action: Action::Byte(s.byte),
                    hits,
                }
            })
            .collect()
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, family: &str, width_cells: usize) {
        let attrs = Attrs::new().family(Family::Name(family));
        let (input_text, input_color) = if self.query.is_empty() {
            (
                "Datei suchen …   > Befehle   @ Symbole   : Zeile".to_string(),
                theme.panel_dim,
            )
        } else {
            (self.query.clone(), theme.tab_text_active)
        };
        self.input.set_rich_text(
            [(input_text.as_str(), attrs.clone().color(input_color.to_glyphon()))],
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.input.shape_until_scroll(fs, false);

        let mut spans: Vec<(String, Rgba)> = Vec::new();
        let rows = self.entries.iter().skip(self.scroll).take(VISIBLE_ROWS);
        if self.entries.is_empty() {
            let msg = if self.waiting_for_index { "Projekt wird gelesen …" } else { "Keine Treffer" };
            spans.push((msg.into(), theme.panel_dim));
        }
        for e in rows {
            let detail_len = e.detail.chars().count();
            let max_label = width_cells.saturating_sub(detail_len + 3).max(8);
            let chars: Vec<char> = e.label.chars().collect();
            // Long paths: keep the end (file name), cut the beginning.
            let start = chars.len().saturating_sub(max_label);
            if start > 0 {
                spans.push(("…".into(), theme.panel_dim));
            }
            for (i, ch) in chars.iter().enumerate().skip(start) {
                let hit = e.hits.binary_search(&(i as u32)).is_ok();
                let color = if hit { theme.accent } else { theme.panel_text };
                match spans.last_mut() {
                    Some((t, c)) if *c == color => t.push(*ch),
                    _ => spans.push((ch.to_string(), color)),
                }
            }
            let used = chars.len() - start + usize::from(start > 0);
            let pad = width_cells.saturating_sub(used + detail_len);
            spans.push((format!("{}{}\n", " ".repeat(pad), e.detail), theme.panel_dim));
        }
        self.list.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.list.shape_until_scroll(fs, false);
    }

    /// Box width in cells for a window width.
    pub fn width_cells(window_w: f32, font: FontMetrics) -> usize {
        ((window_w * 0.6 / font.cell_w) as usize).clamp(30, 100)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a mut self,
        theme: &Theme,
        font: FontMetrics,
        window_w: f32,
        top: f32,
        scale: f32,
        quads: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let cells = Self::width_cells(window_w, font);
        let pad = font.cell_w;
        let w = cells as f32 * font.cell_w + 2.0 * pad;
        let x = ((window_w - w) / 2.0).round();
        let rows = self.entries.len().clamp(1, VISIBLE_ROWS);
        let input_h = font.line_h * 1.5;
        let h = input_h + rows as f32 * font.line_h + pad;
        let y = top;
        let b = scale.round().max(1.0);
        quads.push(Quad { x: x - b, y: y - b, w: w + 2.0 * b, h: h + 2.0 * b, color: theme.popup_border });
        quads.push(Quad { x, y, w, h, color: theme.popup_bg });
        quads.push(Quad { x, y, w, h: input_h, color: theme.panel_bg });
        let input_y = y + (input_h - font.line_h) / 2.0;
        // Cursor at the end of the query.
        let cursor_x = x + pad + self.query.chars().count() as f32 * font.cell_w;
        quads.push(Quad { x: cursor_x, y: input_y, w: (2.0 * scale).round(), h: font.line_h, color: theme.cursor });

        let list_y = y + input_h + pad * 0.5;
        self.rows_rect = Rect { x, y: list_y, w, h: rows as f32 * font.line_h };
        self.row_h = font.line_h;
        if !self.entries.is_empty() && self.selected >= self.scroll {
            let sy = list_y + (self.selected - self.scroll) as f32 * font.line_h;
            quads.push(Quad { x, y: sy, w, h: font.line_h, color: theme.row_selected });
        }
        let clip = Some([x as i32, y as i32, (x + w) as i32, (y + h) as i32]);
        text.push(TextBlock { buffer: &self.input, x: x + pad, y: input_y, color: theme.panel_text, clip });
        text.push(TextBlock { buffer: &self.list, x: x + pad, y: list_y, color: theme.panel_text, clip });
    }
}

fn file_entry(path: String, detail: &str, hits: Vec<u32>) -> Entry {
    Entry { action: Action::OpenFile(path.clone()), label: path, detail: detail.into(), hits }
}

fn hint(label: &str, detail: &str) -> Entry {
    Entry { label: label.into(), detail: detail.into(), action: Action::Nothing, hits: Vec::new() }
}

fn line_entries(q: &str, line_count: usize) -> Vec<Entry> {
    let mut parts = q.split([':', ',']).map(str::trim);
    let line = parts.next().and_then(|s| s.parse::<usize>().ok());
    let col = parts.next().and_then(|s| s.parse::<usize>().ok());
    match line {
        Some(l) if l >= 1 => {
            let l = l.min(line_count.max(1));
            let label = match col {
                Some(c) => format!("Gehe zu Zeile {l}, Spalte {c}"),
                None => format!("Gehe zu Zeile {l}"),
            };
            vec![Entry { label, detail: format!("von {line_count}"), action: Action::Line(l, col), hits: Vec::new() }]
        }
        _ => vec![hint(&format!("Zeilennummer eingeben (1–{line_count}), optional :Spalte"), "")],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(files: &[&str]) -> Sources<'static> {
        Sources {
            files: Some(Arc::new(files.iter().map(|s| s.to_string()).collect())),
            root: Some(Path::new("C:/p")),
            open: Vec::new(),
            symbols: Vec::new(),
            line_count: 50,
            has_symbols: true,
        }
    }

    fn palette() -> Palette {
        let mut fs = FontSystem::new();
        Palette::new(&mut fs, FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 })
    }

    #[test]
    fn fuzzy_file_search_ranks_best_match_first() {
        let mut p = palette();
        p.reset("prgcs");
        p.update(&sources(&["README.md", "Models/Player.cs", "Program.cs", "src/progress.cs"]));
        assert_eq!(p.selected_action(), Action::OpenFile("Program.cs".into()));
    }

    #[test]
    fn line_mode_parses_line_and_column() {
        let mut p = palette();
        p.reset(":12:4");
        p.update(&sources(&[]));
        assert_eq!(p.selected_action(), Action::Line(12, Some(4)));
        p.reset(":999");
        p.update(&sources(&[]));
        assert_eq!(p.selected_action(), Action::Line(50, None));
    }

    #[test]
    fn command_mode_finds_commands() {
        let mut p = palette();
        p.reset(">speich unter");
        p.update(&sources(&[]));
        assert_eq!(p.selected_action(), Action::Run(Cmd::Key(Command::SaveAs)));
    }
}
