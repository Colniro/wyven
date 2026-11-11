//! Example Wyven plugin: a few text commands and one event.
//!
//! - Text Tools: Sort Lines (selected lines, or the whole file)
//! - Text Tools: UPPERCASE / lowercase (selection)
//! - Text Tools: Count Words (selection, or the whole file)
//! - Text Tools: Trim Trailing Whitespace on Save (on/off; uses the BeforeSave event)

use wyven_plugin::{export_plugin, Action, Editor, Event, Info, Plugin};

#[derive(Default)]
pub struct TextTools {
    trim_on_save: bool,
}

/// The whole lines the selection touches (or the whole file): (start, end) character offsets.
fn line_span(e: &Editor) -> (usize, usize) {
    if e.has_selection() {
        let end = if e.selection_end > e.selection_start && e.text.chars().nth(e.selection_end - 1) == Some('\n') {
            e.selection_end - 1
        } else {
            e.selection_end
        };
        (e.line_start(e.selection_start), e.line_end(end))
    } else {
        (0, e.text.chars().count())
    }
}

fn slice(text: &str, start: usize, end: usize) -> String {
    text.chars().skip(start).take(end - start).collect()
}

fn trim_lines(text: &str) -> String {
    text.split('\n').map(|l| l.trim_end_matches([' ', '\t'])).collect::<Vec<_>>().join("\n")
}

impl Plugin for TextTools {
    fn info(&self) -> Info {
        Info::new("Text Tools")
            .command("text-tools.sort", "Text Tools: Sort Lines")
            .command("text-tools.upper", "Text Tools: UPPERCASE")
            .command("text-tools.lower", "Text Tools: lowercase")
            .command("text-tools.count", "Text Tools: Count Words")
            .command("text-tools.trim", "Text Tools: Trim Trailing Whitespace on Save (on/off)")
            .event(Event::BeforeSave)
    }

    fn command(&mut self, id: &str, e: &Editor) -> Vec<Action> {
        match id {
            "text-tools.sort" => {
                let (start, end) = line_span(e);
                let mut lines: Vec<String> = slice(&e.text, start, end).split('\n').map(str::to_string).collect();
                lines.sort_by_key(|l| l.to_lowercase());
                vec![
                    Action::ReplaceRange { start, end, text: lines.join("\n") },
                    Action::Message { text: format!("Sorted {} lines", lines.len()) },
                ]
            }
            "text-tools.upper" | "text-tools.lower" if !e.has_selection() => {
                vec![Action::Message { text: "Select some text first".into() }]
            }
            "text-tools.upper" => {
                let (s, t) = (e.selection_start, e.selected_text().to_uppercase());
                let n = t.chars().count();
                vec![Action::ReplaceSelection { text: t }, Action::SetSelection { start: s, end: s + n }]
            }
            "text-tools.lower" => {
                let (s, t) = (e.selection_start, e.selected_text().to_lowercase());
                let n = t.chars().count();
                vec![Action::ReplaceSelection { text: t }, Action::SetSelection { start: s, end: s + n }]
            }
            "text-tools.count" => {
                let (what, text) = if e.has_selection() { ("Selection", e.selected_text()) } else { ("File", e.text.clone()) };
                let words = text.split_whitespace().count();
                let lines = if text.is_empty() { 0 } else { text.split('\n').count() };
                let chars = text.chars().count();
                vec![Action::Message { text: format!("{what}: {words} words, {lines} lines, {chars} characters") }]
            }
            "text-tools.trim" => {
                self.trim_on_save = !self.trim_on_save;
                let state = if self.trim_on_save { "on" } else { "off" };
                vec![Action::Message { text: format!("Trim trailing whitespace on save: {state}") }]
            }
            _ => Vec::new(),
        }
    }

    fn event(&mut self, event: Event, e: &Editor) -> Vec<Action> {
        if event != Event::BeforeSave || !self.trim_on_save {
            return Vec::new();
        }
        let trimmed = trim_lines(&e.text);
        if trimmed == e.text { Vec::new() } else { vec![Action::ReplaceAll { text: trimmed }] }
    }
}

export_plugin!(TextTools);

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str, s: usize, e: usize) -> Editor {
        Editor { text: text.into(), selection_start: s, selection_end: e, ..Editor::default() }
    }

    #[test]
    fn sorts_the_selected_lines_only() {
        let mut p = TextTools::default();
        let a = p.command("text-tools.sort", &ed("top\nc\nb\na\nend", 4, 9));
        assert_eq!(a[0], Action::ReplaceRange { start: 4, end: 9, text: "a\nb\nc".into() });
    }

    #[test]
    fn trims_on_save_only_when_switched_on() {
        let mut p = TextTools::default();
        assert!(p.event(Event::BeforeSave, &ed("a  \nb\t", 0, 0)).is_empty());
        p.command("text-tools.trim", &ed("", 0, 0));
        assert_eq!(p.event(Event::BeforeSave, &ed("a  \nb\t", 0, 0)), vec![Action::ReplaceAll { text: "a\nb".into() }]);
    }
}
