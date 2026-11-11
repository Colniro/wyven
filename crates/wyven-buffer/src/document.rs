//! A document: text + cursors + undo history + file information.
//!
//! Every editing command works for any number of cursors: it builds one [`Transaction`] with a
//! change per cursor and applies it in one go.

use std::io;
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::history::{EditKind, History};
use crate::selection::{Range, Selection};
use crate::text::{self, char_to_display_col, display_col_to_char, line_len_chars};
use crate::transaction::{Change, TextEdit, Transaction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// Smart Home: first non-blank char, pressing again goes to column 0.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp(usize),
    PageDown(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentMode {
    Toggle,
    Add,
    Remove,
}

pub struct Document {
    rope: Rope,
    selection: Selection,
    history: History,
    path: Option<PathBuf>,
    line_ending: LineEnding,
    /// The file started with a UTF-8 byte order mark (Visual Studio writes one); kept on save.
    bom: bool,
    pub tab_width: usize,
    pub indent_unit: String,
    /// The indentation was taken from the file's content (not a default).
    indent_detected: bool,
    version: u64,
    saved_version: u64,
    edits: Vec<TextEdit>,
}

impl Default for Document {
    fn default() -> Self {
        Self::from_str("")
    }
}

impl Document {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let normalized = text.replace("\r\n", "\n");
        let detected = detect_indent(&normalized);
        let indent_detected = detected.is_some();
        let indent_unit = detected.unwrap_or_else(|| "    ".into());
        Self {
            rope: Rope::from_str(&normalized),
            selection: Selection::point(0),
            history: History::default(),
            path: None,
            line_ending,
            bom: false,
            tab_width: 4,
            indent_unit,
            indent_detected,
            version: 0,
            saved_version: 0,
            edits: Vec::new(),
        }
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8(bytes).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "file is not valid UTF-8")
        })?;
        let bom = text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut doc = Self::from_str(text);
        doc.path = Some(path.to_path_buf());
        doc.bom = bom;
        Ok(doc)
    }

    /// Writes the file (atomically: temp file + rename) with the original line endings.
    pub fn save(&mut self) -> io::Result<()> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "document has no path"))?;
        let mut out = String::with_capacity(self.rope.len_bytes() + self.rope.len_lines() + 3);
        if self.bom {
            out.push('\u{feff}');
        }
        for chunk in self.rope.chunks() {
            match self.line_ending {
                LineEnding::Lf => out.push_str(chunk),
                LineEnding::CrLf => out.push_str(&chunk.replace('\n', "\r\n")),
            }
        }
        let tmp = path.with_extension(format!(
            "{}.wyven-tmp",
            path.extension().and_then(|e| e.to_str()).unwrap_or("")
        ));
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, &path)?;
        self.saved_version = self.version;
        Ok(())
    }

    /// Takes over the text and saved state of another view of the same file (split view).
    /// Its own undo history is dropped: it would not fit the new text.
    pub fn mirror_from(&mut self, other: &Document) {
        self.rope = other.rope.clone();
        self.line_ending = other.line_ending;
        self.bom = other.bom;
        self.version = other.version;
        self.saved_version = other.saved_version;
        self.path = other.path.clone();
        self.history = History::default();
        self.edits.clear();
        let len = self.rope.len_chars();
        self.selection = self.selection.map(|r| Range { anchor: r.anchor.min(len), head: r.head.min(len), goal: r.goal });
    }

    /// The file was renamed or moved on disk: just remember the new path.
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path);
    }

    pub fn save_as(&mut self, path: PathBuf) -> io::Result<()> {
        self.path = Some(path);
        self.save()
    }

    pub fn text(&self) -> &Rope {
        &self.rope
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn indent_detected(&self) -> bool {
        self.indent_detected
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Saves with other line breaks from now on (the file counts as changed).
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        if ending != self.line_ending {
            self.line_ending = ending;
            self.saved_version = self.version.wrapping_sub(1);
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    /// Increases with every text change.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Edits since the last call, for incremental re-parsing.
    pub fn take_edits(&mut self) -> Vec<TextEdit> {
        std::mem::take(&mut self.edits)
    }

    pub fn set_selection(&mut self, sel: Selection) {
        let len = self.rope.len_chars();
        self.selection =
            sel.map(|r| Range { anchor: r.anchor.min(len), head: r.head.min(len), goal: r.goal });
        self.history.break_group();
    }

    // ----- core -----

    fn apply_raw(&mut self, tx: &Transaction) -> Transaction {
        let edits = &mut self.edits;
        let inverse = tx.apply_with(&mut self.rope, &mut |e| edits.push(e));
        self.version += 1;
        inverse
    }

    fn apply(&mut self, tx: Transaction, new_sel: Selection, kind: EditKind, breaks_group: bool) {
        if tx.is_empty() {
            self.selection = new_sel;
            return;
        }
        let before = self.selection.clone();
        let inverse = self.apply_raw(&tx);
        self.history
            .record(tx, inverse, before, new_sel.clone(), kind, breaks_group);
        self.selection = new_sel;
    }

    /// Replaces every range with a computed change; `cursor` is the cursor position as a char
    /// offset inside the inserted text.
    fn replace_each(
        &mut self,
        kind: EditKind,
        breaks_group: bool,
        mut f: impl FnMut(&Rope, &Range) -> (Change, usize),
    ) {
        let mut cursors = Vec::new();
        let mut changes = Vec::new();
        for r in self.selection.ranges() {
            let (change, cursor) = f(&self.rope, r);
            cursors.push(cursor);
            changes.push(change);
        }
        // Positions in the new text: start of each change shifted by everything before it.
        let mut delta: isize = 0;
        let mut ranges = Vec::with_capacity(changes.len());
        for (c, cursor) in changes.iter().zip(&cursors) {
            let new_start = (c.start as isize + delta) as usize;
            ranges.push(Range::point(new_start + cursor));
            delta += c.text.chars().count() as isize - (c.end - c.start) as isize;
        }
        let new_sel = Selection::new(ranges, self.selection.primary_index());
        self.apply(Transaction::new(changes), new_sel, kind, breaks_group);
    }

    /// Applies changes from outside (formatting, rename from a language server) as one undo
    /// step. Overlapping changes are dropped; cursors move with the text.
    pub fn apply_changes(&mut self, mut changes: Vec<Change>) {
        let len = self.rope.len_chars();
        changes.retain(|c| c.start <= c.end && c.end <= len);
        changes.sort_by_key(|c| (c.start, c.end));
        let mut kept: Vec<Change> = Vec::with_capacity(changes.len());
        for c in changes {
            if kept.last().is_none_or(|last| last.end <= c.start) {
                kept.push(c);
            }
        }
        let tx = Transaction::new(kept);
        let sel = tx.map_selection(&self.selection);
        self.apply(tx, sel, EditKind::Other, true);
    }

    /// Selects `start..end` and replaces it with `text` (accepting a completion).
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        let n = text.chars().count();
        let end = end.min(self.rope.len_chars());
        let start = start.min(end);
        let tx = Transaction::new(vec![Change { start, end, text: text.to_string() }]);
        let sel = Selection::point(start + n);
        self.apply(tx, sel, EditKind::Other, true);
    }

    pub fn undo(&mut self) -> bool {
        let Some((txs, sel)) = self.history.undo() else { return false };
        for tx in &txs {
            self.apply_raw(tx);
        }
        self.selection = sel;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((txs, sel)) = self.history.redo() else { return false };
        for tx in &txs {
            self.apply_raw(tx);
        }
        self.selection = sel;
        true
    }

    // ----- typing -----

    /// Inserts text at every cursor, replacing selections.
    pub fn insert(&mut self, text: &str) {
        let n = text.chars().count();
        let single_char = n == 1 && text != "\n";
        let breaks = !single_char || text.chars().all(char::is_whitespace);
        let kind = if single_char { EditKind::Insert } else { EditKind::Other };
        self.replace_each(kind, breaks, |_, r| {
            (Change { start: r.from(), end: r.to(), text: text.to_string() }, n)
        });
    }

    /// Types one character with automatic pairs, like Visual Studio: `(` `[` `{` `"` `'` `` ` ``
    /// also insert their closing partner (before whitespace, a closing bracket, `;`, `,` or the
    /// line end); a typed closing character steps over the one already there; with a selection
    /// the pair goes around it. Anything else is inserted as usual.
    pub fn type_char(&mut self, text: &str) {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            self.insert(text);
            return;
        };
        let close = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' | '\'' | '`' => Some(c),
            _ => None,
        };
        let is_closer = matches!(c, ')' | ']' | '}' | '"' | '\'' | '`');
        if close.is_none() && !is_closer {
            self.insert(text);
            return;
        }
        let quote = matches!(c, '"' | '\'' | '`');
        self.replace_each(EditKind::Insert, false, |rope, r| {
            let next = (r.to() < rope.len_chars()).then(|| rope.char(r.to()));
            let prev = (r.from() > 0).then(|| rope.char(r.from() - 1));
            let plain = (Change { start: r.from(), end: r.to(), text: c.to_string() }, 1);
            if !r.is_empty() {
                // Around the selection (cursor before the closing character).
                let Some(cl) = close else { return plain };
                let inner: String = rope.slice(r.from()..r.to()).to_string();
                let n = inner.chars().count();
                return (Change { start: r.from(), end: r.to(), text: format!("{c}{inner}{cl}") }, n + 1);
            }
            // Step over a closing character that is already there (a quote only at the end of a
            // string: before `"abc"` a typed quote is a new one).
            let at_string_end = !quote || prev.is_some_and(|p| !p.is_whitespace() && !matches!(p, '(' | '[' | '{' | ',' | '='));
            if is_closer && next == Some(c) && at_string_end {
                return (Change { start: r.from(), end: r.from(), text: String::new() }, 1);
            }
            let Some(cl) = close else { return plain };
            let free_after = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ';' | ',' | '.' | ':' | '>'));
            // A quote right after a letter is an apostrophe (don't), not a string.
            let word_before = prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == c);
            // Rust lifetimes and generics: <'a>, &'a.
            let lifetime = c == '\'' &&matches!(prev, Some('<' | '&'));
            if !free_after || (quote && word_before) || lifetime {
                return plain;
            }
            (Change { start: r.from(), end: r.to(), text: format!("{c}{cl}") }, 1)
        });
    }

    /// Enter: new line with the indentation of the current line. Between brackets like `{|}`
    /// the closing bracket moves to its own line.
    pub fn newline(&mut self) {
        let unit = self.indent_unit.clone();
        self.replace_each(EditKind::Other, true, |rope, r| {
            let line = rope.char_to_line(r.from());
            let line_start = rope.line_to_char(line);
            let indent: String = text::indentation(rope, line)
                .chars()
                .take(r.from() - line_start)
                .collect();
            let before = (line_start..r.from())
                .rev()
                .map(|i| rope.char(i))
                .find(|c| !c.is_whitespace());
            let after = (r.to() < rope.len_chars()).then(|| rope.char(r.to()));
            let opens = matches!(before, Some('{' | '(' | '['));
            let closes = matches!(
                (before, after),
                (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))
            );
            let mut text = format!("\n{indent}");
            if opens {
                text.push_str(&unit);
            }
            let cursor = text.chars().count();
            if closes {
                text.push('\n');
                text.push_str(&indent);
            }
            (Change { start: r.from(), end: r.to(), text }, cursor)
        });
    }

    /// Tab: inserts indentation, or indents all lines when a selection spans several lines.
    pub fn indent(&mut self) {
        let multiline = self.selection.ranges().iter().any(|r| {
            !r.is_empty() && self.rope.char_to_line(r.from()) != self.rope.char_to_line(r.to())
        });
        if multiline {
            self.indent_lines();
            return;
        }
        let tab_width = self.tab_width;
        let unit = self.indent_unit.clone();
        self.replace_each(EditKind::Other, true, |rope, r| {
            let text = if unit == "\t" {
                "\t".to_string()
            } else {
                let line = rope.char_to_line(r.from());
                let col = char_to_display_col(
                    rope.line(line),
                    r.from() - rope.line_to_char(line),
                    tab_width,
                );
                " ".repeat(tab_width - col % tab_width)
            };
            let n = text.chars().count();
            (Change { start: r.from(), end: r.to(), text }, n)
        });
    }

    fn indent_lines(&mut self) {
        let lines = self.covered_lines();
        let changes = lines
            .iter()
            .filter(|&&l| line_len_chars(&self.rope, l) > 0)
            .map(|&l| {
                let p = self.rope.line_to_char(l);
                Change { start: p, end: p, text: self.indent_unit.clone() }
            })
            .collect();
        let tx = Transaction::new(changes);
        let sel = tx.map_selection(&self.selection);
        self.apply(tx, sel, EditKind::Other, true);
    }

    /// Shift+Tab: removes one indentation level from all covered lines.
    pub fn outdent(&mut self) {
        let lines = self.covered_lines();
        let mut changes = Vec::new();
        for l in lines {
            let start = self.rope.line_to_char(l);
            let mut width = 0;
            let mut n = 0;
            for c in self.rope.line(l).chars() {
                match c {
                    '\t' => {
                        n += 1;
                        break;
                    }
                    ' ' if width < self.tab_width => {
                        n += 1;
                        width += 1;
                    }
                    _ => break,
                }
            }
            if n > 0 {
                changes.push(Change { start, end: start + n, text: String::new() });
            }
        }
        let tx = Transaction::new(changes);
        let sel = tx.map_selection(&self.selection);
        self.apply(tx, sel, EditKind::Other, true);
    }

    /// Backspace. Inside leading whitespace it removes back to the previous indent stop.
    pub fn backspace(&mut self) {
        let tab_width = self.tab_width;
        self.replace_each(EditKind::Delete, false, |rope, r| {
            if !r.is_empty() {
                return (Change { start: r.from(), end: r.to(), text: String::new() }, 0);
            }
            let pos = r.head;
            if pos == 0 {
                return (Change { start: 0, end: 0, text: String::new() }, 0);
            }
            // Between an empty pair `(|)`: both go.
            if pos < rope.len_chars() {
                let (a, b) = (rope.char(pos - 1), rope.char(pos));
                if matches!((a, b), ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')) {
                    return (Change { start: pos - 1, end: pos + 1, text: String::new() }, 0);
                }
            }
            let line = rope.char_to_line(pos);
            let line_start = rope.line_to_char(line);
            let in_indent = pos > line_start && (line_start..pos).all(|i| rope.char(i) == ' ');
            let n = if in_indent {
                let col = pos - line_start;
                (col - 1) % tab_width + 1
            } else {
                1
            };
            (Change { start: pos - n, end: pos, text: String::new() }, 0)
        });
    }

    /// Delete key.
    pub fn delete_forward(&mut self) {
        self.replace_each(EditKind::Delete, false, |rope, r| {
            let end = if r.is_empty() {
                (r.head + 1).min(rope.len_chars())
            } else {
                r.to()
            };
            (Change { start: r.from(), end, text: String::new() }, 0)
        });
    }

    /// Strg+Backspace / Strg+Entf.
    pub fn delete_word(&mut self, forward: bool) {
        self.replace_each(EditKind::Other, true, |rope, r| {
            let (start, end) = if !r.is_empty() {
                (r.from(), r.to())
            } else if forward {
                (r.head, text::next_word_end(rope, r.head))
            } else {
                (text::prev_word_start(rope, r.head), r.head)
            };
            (Change { start, end, text: String::new() }, 0)
        });
    }

    // ----- cursor movement -----

    pub fn move_cursors(&mut self, movement: Movement, extend: bool) {
        let rope = &self.rope;
        let tab_width = self.tab_width;
        let sel = self.selection.map(|r| move_range(rope, r, movement, extend, tab_width));
        self.set_selection(sel);
    }

    pub fn set_cursor(&mut self, pos: usize) {
        self.set_selection(Selection::point(pos));
    }

    /// Mouse drag: keeps the anchor of the primary cursor.
    pub fn extend_primary_to(&mut self, pos: usize) {
        let anchor = self.selection.primary().anchor;
        self.set_selection(Selection::single(Range::new(anchor, pos)));
    }

    /// Alt+Klick: adds a cursor, or removes the cursor already there.
    pub fn toggle_cursor(&mut self, pos: usize) {
        let ranges = self.selection.ranges();
        if ranges.len() > 1
            && let Some(i) = ranges.iter().position(|r| r.is_empty() && r.head == pos) {
                let mut rest = ranges.to_vec();
                rest.remove(i);
                let sel = Selection::new(rest, 0);
                self.set_selection(sel);
                return;
            }
        let sel = self.selection.push(Range::point(pos));
        self.set_selection(sel);
    }

    /// Esc: back to one cursor.
    pub fn collapse_to_primary(&mut self) {
        let p = self.selection.primary();
        self.set_selection(Selection::point(p.head));
    }

    pub fn select_all(&mut self) {
        self.set_selection(Selection::single(Range::new(0, self.rope.len_chars())));
    }

    /// Double click: selects the word under `pos` (or the single char).
    pub fn select_word_at(&mut self, pos: usize) {
        let range = match text::word_at(&self.rope, pos) {
            Some((s, e)) => Range::new(s, e),
            None => Range::point(pos),
        };
        self.set_selection(Selection::single(range));
    }

    /// Strg+D: first press selects the word under the cursor, then adds the next occurrence.
    pub fn add_next_occurrence(&mut self) {
        let primary = self.selection.primary();
        if primary.is_empty() {
            if let Some((s, e)) = text::word_at(&self.rope, primary.head) {
                let mut ranges = self.selection.ranges().to_vec();
                ranges[self.selection.primary_index()] = Range::new(s, e);
                self.set_selection(Selection::new(ranges, self.selection.primary_index()));
            }
            return;
        }
        let needle = self.rope.slice(primary.from()..primary.to()).to_string();
        let haystack = self.rope.to_string();
        let after = self.selection.ranges().iter().map(|r| r.to()).max().unwrap_or(0);
        let after_byte = self.rope.char_to_byte(after);
        let found = haystack[after_byte..]
            .find(&needle)
            .map(|b| b + after_byte)
            .or_else(|| haystack.find(&needle));
        if let Some(byte) = found {
            let start = self.rope.byte_to_char(byte);
            let range = Range::new(start, start + primary.len());
            if !self.selection.ranges().contains(&range) {
                let sel = self.selection.push(range);
                self.set_selection(sel);
            }
        }
    }

    // ----- line commands -----

    /// Line numbers touched by any cursor, sorted and unique.
    fn covered_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        for (first, last) in self.line_blocks() {
            lines.extend(first..=last);
        }
        lines
    }

    /// Merged (first_line, last_line) blocks covered by the cursors. A selection ending at the
    /// very start of a line does not include that line.
    fn line_blocks(&self) -> Vec<(usize, usize)> {
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for r in self.selection.ranges() {
            let first = self.rope.char_to_line(r.from());
            let mut last = self.rope.char_to_line(r.to());
            if last > first && self.rope.line_to_char(last) == r.to() {
                last -= 1;
            }
            match blocks.last_mut() {
                Some(b) if first <= b.1 + 1 => b.1 = b.1.max(last),
                _ => blocks.push((first, last)),
            }
        }
        blocks
    }

    /// Last line index that has content or a line break (ropey counts an empty line after a
    /// trailing `\n`).
    fn last_line(&self) -> usize {
        self.rope.len_lines().saturating_sub(1)
    }

    /// Alt+↑ / Alt+↓.
    pub fn move_lines(&mut self, dir: Direction) {
        let blocks = self.line_blocks();
        let rope = &self.rope;
        let last_line = self.last_line();
        if (dir == Direction::Up && blocks.first().is_some_and(|b| b.0 == 0))
            || (dir == Direction::Down && blocks.last().is_some_and(|b| b.1 >= last_line))
        {
            return;
        }
        let mut changes = Vec::new();
        let mut shifts = Vec::new(); // (block, shift)
        for &(first, last) in &blocks {
            let block = rope
                .slice(rope.line_to_char(first)..rope.line_to_char(last + 1))
                .to_string();
            match dir {
                Direction::Up => {
                    let start = rope.line_to_char(first - 1);
                    let prev = rope.slice(start..rope.line_to_char(first)).to_string();
                    let text = if block.ends_with('\n') {
                        format!("{block}{prev}")
                    } else {
                        format!("{block}\n{}", prev.trim_end_matches('\n'))
                    };
                    shifts.push(-(prev.chars().count() as isize));
                    changes.push(Change { start, end: rope.line_to_char(last + 1), text });
                }
                Direction::Down => {
                    let start = rope.line_to_char(first);
                    let next_end = rope.line_to_char((last + 2).min(rope.len_lines()));
                    let next = rope.slice(rope.line_to_char(last + 1)..next_end).to_string();
                    let (text, shift) = if next.ends_with('\n') {
                        (format!("{next}{block}"), next.chars().count())
                    } else {
                        (
                            format!("{next}\n{}", block.trim_end_matches('\n')),
                            next.chars().count() + 1,
                        )
                    };
                    shifts.push(shift as isize);
                    changes.push(Change { start, end: next_end, text });
                }
            }
        }
        let sel = self.selection.map(|r| {
            let line = rope.char_to_line(r.from());
            let i = blocks.iter().position(|b| line >= b.0 && line <= b.1).unwrap_or(0);
            let s = shifts[i];
            Range {
                anchor: (r.anchor as isize + s) as usize,
                head: (r.head as isize + s) as usize,
                goal: r.goal,
            }
        });
        self.apply(Transaction::new(changes), sel, EditKind::Other, true);
    }

    /// Alt+Shift+↑ / Alt+Shift+↓: duplicates the lines; the cursor goes to the copy.
    pub fn duplicate_lines(&mut self, dir: Direction) {
        let blocks = self.line_blocks();
        let rope = &self.rope;
        let mut changes = Vec::new();
        let mut infos = Vec::new(); // (first, last, inserted_len, cursor_shift)
        for &(first, last) in &blocks {
            let block = rope
                .slice(rope.line_to_char(first)..rope.line_to_char(last + 1))
                .to_string();
            let (pos, text) = if block.ends_with('\n') {
                match dir {
                    Direction::Up => (rope.line_to_char(first), block.clone()),
                    Direction::Down => (rope.line_to_char(last + 1), block.clone()),
                }
            } else {
                match dir {
                    Direction::Up => (rope.line_to_char(first), format!("{block}\n")),
                    Direction::Down => (rope.len_chars(), format!("\n{block}")),
                }
            };
            let n = text.chars().count();
            let shift = if dir == Direction::Down { n } else { 0 };
            infos.push((first, last, n, shift));
            changes.push(Change { start: pos, end: pos, text });
        }
        let sel = self.selection.map(|r| {
            let line = rope.char_to_line(r.from());
            let mut offset = 0;
            for &(first, last, n, shift) in &infos {
                if line >= first && line <= last {
                    offset += shift;
                    break;
                }
                offset += n;
            }
            Range { anchor: r.anchor + offset, head: r.head + offset, goal: r.goal }
        });
        self.apply(Transaction::new(changes), sel, EditKind::Other, true);
    }

    /// Strg+Shift+L.
    pub fn delete_lines(&mut self) {
        let blocks = self.line_blocks();
        let rope = &self.rope;
        let changes: Vec<Change> = blocks
            .iter()
            .map(|&(first, last)| {
                let mut start = rope.line_to_char(first);
                let end = rope.line_to_char(last + 1);
                let ends_with_newline = end > 0 && rope.char(end - 1) == '\n';
                if !ends_with_newline && start > 0 {
                    start -= 1; // last line: remove the line break before it instead
                }
                Change { start, end, text: String::new() }
            })
            .collect();
        let tx = Transaction::new(changes);
        let sel = tx.map_selection(&self.selection.map(|r| {
            let line = rope.char_to_line(r.head);
            Range::point(rope.line_to_char(line))
        }));
        self.apply(tx, sel, EditKind::Other, true);
    }

    /// Strg+K Strg+C / Strg+K Strg+U / Strg+#.
    pub fn comment_lines(&mut self, token: &str, mode: CommentMode) {
        let lines: Vec<usize> = self
            .covered_lines()
            .into_iter()
            .filter(|&l| !self.rope.line(l).chars().all(char::is_whitespace))
            .collect();
        if lines.is_empty() {
            return;
        }
        let commented = |l: usize| {
            let indent = text::indentation(&self.rope, l).chars().count();
            let start = self.rope.line_to_char(l) + indent;
            let end = (start + token.chars().count()).min(self.rope.len_chars());
            self.rope.slice(start..end) == token
        };
        let remove = match mode {
            CommentMode::Add => false,
            CommentMode::Remove => true,
            CommentMode::Toggle => lines.iter().all(|&l| commented(l)),
        };
        let mut changes = Vec::new();
        if remove {
            for &l in lines.iter().filter(|&&l| commented(l)) {
                let indent = text::indentation(&self.rope, l).chars().count();
                let start = self.rope.line_to_char(l) + indent;
                let mut end = start + token.chars().count();
                if end < self.rope.len_chars() && self.rope.char(end) == ' ' {
                    end += 1;
                }
                changes.push(Change { start, end, text: String::new() });
            }
        } else {
            let col = lines
                .iter()
                .map(|&l| text::indentation(&self.rope, l).chars().count())
                .min()
                .unwrap_or(0);
            for &l in &lines {
                let p = self.rope.line_to_char(l) + col;
                changes.push(Change { start: p, end: p, text: format!("{token} ") });
            }
        }
        let tx = Transaction::new(changes);
        let sel = tx.map_selection(&self.selection);
        self.apply(tx, sel, EditKind::Other, true);
    }

    // ----- clipboard -----

    /// Text to copy. With no selection, whole lines are copied (`line_mode = true`).
    pub fn copy(&self) -> (String, bool) {
        let ranges = self.selection.ranges();
        if ranges.iter().all(Range::is_empty) {
            let mut seen = Vec::new();
            let mut out = String::new();
            for r in ranges {
                let line = self.rope.char_to_line(r.head);
                if seen.contains(&line) {
                    continue;
                }
                seen.push(line);
                out.push_str(&self.rope.line(line).to_string());
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            (out, true)
        } else {
            let parts: Vec<String> = ranges
                .iter()
                .filter(|r| !r.is_empty())
                .map(|r| self.rope.slice(r.from()..r.to()).to_string())
                .collect();
            (parts.join("\n"), false)
        }
    }

    pub fn cut(&mut self) -> (String, bool) {
        let copied = self.copy();
        if copied.1 {
            self.delete_lines();
        } else {
            self.replace_each(EditKind::Other, true, |_, r| {
                (Change { start: r.from(), end: r.to(), text: String::new() }, 0)
            });
        }
        copied
    }

    /// Pastes `text`. One line per cursor is distributed; whole-line copies go above the line.
    pub fn paste(&mut self, text: &str, line_mode: bool) {
        let text = text.replace("\r\n", "\n");
        let n = self.selection.len();
        let parts: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        if n > 1 && parts.len() == n {
            let mut i = 0;
            self.replace_each(EditKind::Other, true, |_, r| {
                let part = parts[i].to_string();
                i += 1;
                let len = part.chars().count();
                (Change { start: r.from(), end: r.to(), text: part }, len)
            });
        } else if line_mode && self.selection.ranges().iter().all(Range::is_empty) {
            self.replace_each(EditKind::Other, true, |rope, r| {
                let line_start = rope.line_to_char(rope.char_to_line(r.head));
                let len = text.chars().count();
                (
                    Change { start: line_start, end: line_start, text: text.clone() },
                    len + (r.head - line_start),
                )
            });
        } else {
            self.insert(&text);
        }
    }
}

fn move_range(rope: &Rope, r: Range, movement: Movement, extend: bool, tab_width: usize) -> Range {
    let len = rope.len_chars();
    let head = r.head;
    let line = rope.char_to_line(head);
    let line_start = rope.line_to_char(line);
    let vertical = |r: Range, delta: isize| -> Range {
        let col = r.goal.unwrap_or_else(|| {
            char_to_display_col(rope.line(line), head - line_start, tab_width)
        });
        let target = line as isize + delta;
        let pos = if target < 0 {
            0
        } else if target as usize >= rope.len_lines() {
            len
        } else {
            let t = target as usize;
            rope.line_to_char(t) + display_col_to_char(rope.line(t), col, tab_width)
        };
        let mut moved = r.put(pos, extend);
        moved.goal = Some(col);
        moved
    };
    match movement {
        Movement::Left if !extend && !r.is_empty() => Range::point(r.from()),
        Movement::Right if !extend && !r.is_empty() => Range::point(r.to()),
        Movement::Left => r.put(head.saturating_sub(1), extend),
        Movement::Right => r.put((head + 1).min(len), extend),
        Movement::Up => vertical(r, -1),
        Movement::Down => vertical(r, 1),
        Movement::PageUp(n) => vertical(r, -(n as isize)),
        Movement::PageDown(n) => vertical(r, n as isize),
        Movement::WordLeft => r.put(text::prev_word_start(rope, head), extend),
        Movement::WordRight => r.put(text::next_word_end(rope, head), extend),
        Movement::LineStart => {
            let first_non_blank = line_start + text::indentation(rope, line).chars().count();
            let target = if head == first_non_blank { line_start } else { first_non_blank };
            r.put(target, extend)
        }
        Movement::LineEnd => r.put(line_start + line_len_chars(rope, line), extend),
        Movement::DocStart => r.put(0, extend),
        Movement::DocEnd => r.put(len, extend),
    }
}

/// Tabs if most indented lines start with a tab, 4 spaces if most start with spaces, `None`
/// if the text has no indented lines.
fn detect_indent(text: &str) -> Option<String> {
    let (mut tabs, mut spaces) = (0, 0);
    for line in text.lines().take(1000) {
        if line.starts_with('\t') {
            tabs += 1;
        } else if line.starts_with("  ") {
            spaces += 1;
        }
    }
    if tabs == 0 && spaces == 0 {
        None
    } else if tabs > spaces {
        Some("\t".into())
    } else {
        Some("    ".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirror_takes_text_and_saved_state() {
        let mut a = Document::from_str("hello world");
        a.set_cursor(11);
        a.insert("!");
        let mut b = Document::from_str("hello");
        b.set_cursor(5);
        b.mirror_from(&a);
        assert_eq!(b.text().to_string(), "hello world!");
        assert_eq!(b.version(), a.version());
        assert_eq!(b.is_dirty(), a.is_dirty());
        assert_eq!(b.selection().primary().head, 5, "own cursor stays");
        assert!(!b.undo(), "no foreign history");
    }

    fn doc(text: &str, ranges: &[(usize, usize)]) -> Document {
        let mut d = Document::from_str(text);
        let ranges = ranges.iter().map(|&(a, h)| Range::new(a, h)).collect();
        d.set_selection(Selection::new(ranges, 0));
        d
    }

    fn heads(d: &Document) -> Vec<usize> {
        d.selection().ranges().iter().map(|r| r.head).collect()
    }

    #[test]
    fn multi_cursor_typing_and_undo() {
        let mut d = doc("ab\ncd\n", &[(1, 1), (4, 4)]);
        d.insert("X");
        d.insert("Y");
        assert_eq!(d.text().to_string(), "aXYb\ncXYd\n");
        assert_eq!(heads(&d), vec![3, 8]);
        assert!(d.undo());
        assert_eq!(d.text().to_string(), "ab\ncd\n");
        assert_eq!(heads(&d), vec![1, 4]);
        assert!(d.redo());
        assert_eq!(d.text().to_string(), "aXYb\ncXYd\n");
    }

    #[test]
    fn typing_replaces_selection() {
        let mut d = doc("hello world", &[(0, 5)]);
        d.insert("bye");
        assert_eq!(d.text().to_string(), "bye world");
        assert_eq!(heads(&d), vec![3]);
    }

    #[test]
    fn newline_keeps_indent_and_splits_braces() {
        let mut d = doc("    if (x) {}", &[(12, 12)]);
        d.newline();
        assert_eq!(d.text().to_string(), "    if (x) {\n        \n    }");
        assert_eq!(heads(&d), vec![21]);
    }

    #[test]
    fn backspace_removes_indent_level() {
        let mut d = doc("        x", &[(8, 8)]);
        d.backspace();
        assert_eq!(d.text().to_string(), "    x");
        d.backspace();
        assert_eq!(d.text().to_string(), "x");
    }

    #[test]
    fn move_lines_up_and_down() {
        let mut d = doc("a\nb\nc", &[(2, 2)]);
        d.move_lines(Direction::Up);
        assert_eq!(d.text().to_string(), "b\na\nc");
        assert_eq!(heads(&d), vec![0]);
        d.move_lines(Direction::Down);
        d.move_lines(Direction::Down);
        assert_eq!(d.text().to_string(), "a\nc\nb");
        assert_eq!(heads(&d), vec![4]);
        d.move_lines(Direction::Up);
        assert_eq!(d.text().to_string(), "a\nb\nc");
    }

    #[test]
    fn duplicate_and_delete_lines() {
        let mut d = doc("one\ntwo", &[(5, 5)]);
        d.duplicate_lines(Direction::Down);
        assert_eq!(d.text().to_string(), "one\ntwo\ntwo");
        assert_eq!(heads(&d), vec![9]);
        d.delete_lines();
        assert_eq!(d.text().to_string(), "one\ntwo");
        d.set_cursor(1);
        d.duplicate_lines(Direction::Up);
        assert_eq!(d.text().to_string(), "one\none\ntwo");
        assert_eq!(heads(&d), vec![1]);
    }

    #[test]
    fn toggle_comment() {
        let mut d = doc("    a\n    b\n", &[(0, 9)]);
        d.comment_lines("//", CommentMode::Toggle);
        assert_eq!(d.text().to_string(), "    // a\n    // b\n");
        d.comment_lines("//", CommentMode::Toggle);
        assert_eq!(d.text().to_string(), "    a\n    b\n");
    }

    #[test]
    fn ctrl_d_selects_word_then_next_occurrence() {
        let mut d = doc("foo bar foo foo", &[(1, 1)]);
        d.add_next_occurrence();
        assert_eq!(d.selection().ranges(), &[Range::new(0, 3)]);
        d.add_next_occurrence();
        d.add_next_occurrence();
        assert_eq!(d.selection().len(), 3);
        d.insert("x");
        assert_eq!(d.text().to_string(), "x bar x x");
    }

    #[test]
    fn up_down_keeps_goal_column() {
        let mut d = doc("abcdef\nab\nabcdef", &[(5, 5)]);
        d.move_cursors(Movement::Down, false);
        assert_eq!(heads(&d), vec![9]);
        d.move_cursors(Movement::Down, false);
        assert_eq!(heads(&d), vec![15]);
    }

    #[test]
    fn copy_without_selection_copies_line_and_paste_inserts_above() {
        let mut d = doc("a\nb\n", &[(2, 2)]);
        let (text, line_mode) = d.copy();
        assert_eq!((text.as_str(), line_mode), ("b\n", true));
        d.set_cursor(0);
        d.paste(&text, line_mode);
        assert_eq!(d.text().to_string(), "b\na\nb\n");
    }

    #[test]
    fn external_changes_keep_cursor_on_its_text() {
        let mut d = doc("int  x=1;\nfoo();", &[(11, 11)]);
        d.apply_changes(vec![
            Change { start: 3, end: 5, text: " ".into() },
            Change { start: 6, end: 7, text: " = ".into() },
        ]);
        assert_eq!(d.text().to_string(), "int x = 1;\nfoo();");
        assert_eq!(heads(&d), vec![12]);
        d.undo();
        assert_eq!(d.text().to_string(), "int  x=1;\nfoo();");
    }

    #[test]
    fn byte_order_mark_is_kept() {
        let dir = std::env::temp_dir().join("wyven-buffer-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bom.cs");
        std::fs::write(&path, "\u{feff}class A {}\r\n").unwrap();
        let mut d = Document::open(&path).unwrap();
        assert_eq!(d.text().to_string(), "class A {}\n");
        d.save().unwrap();
        assert_eq!(std::fs::read(&path).unwrap()[..3], [0xEF, 0xBB, 0xBF]);
    }

    #[test]
    fn crlf_roundtrip() {
        let dir = std::env::temp_dir().join("wyven-buffer-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crlf.txt");
        std::fs::write(&path, "a\r\nb\r\n").unwrap();
        let mut d = Document::open(&path).unwrap();
        assert_eq!(d.text().to_string(), "a\nb\n");
        d.set_cursor(1);
        d.insert("!");
        d.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a!\r\nb\r\n");
        assert!(!d.is_dirty());
    }

    #[test]
    fn edits_are_reported_for_parsing() {
        let mut d = doc("ab\ncd", &[(1, 1), (4, 4)]);
        d.insert("XY");
        let edits = d.take_edits();
        assert_eq!(edits.len(), 2);
        // Applied back to front: the second cursor first.
        assert_eq!(edits[0].start_byte, 4);
        assert_eq!(edits[0].start_point, (1, 1));
        assert_eq!(edits[1].start_byte, 1);
        assert_eq!(edits[1].new_end_byte, 3);
    }

    #[test]
    fn brackets_and_quotes_close_themselves() {
        let typed = |text: &str, at: usize, c: &str| {
            let mut d = Document::from_str(text);
            d.set_cursor(at);
            d.type_char(c);
            (d.text().to_string(), d.selection().primary().head)
        };
        assert_eq!(typed("foo", 3, "("), ("foo()".into(), 4));
        assert_eq!(typed("x = ;", 4, "\""), ("x = \"\";".into(), 5));
        assert_eq!(typed("if x {}", 6, "{"), ("if x {{}}".into(), 7));
        // Before a word: no pair.
        assert_eq!(typed("abc", 0, "("), ("(abc".into(), 1));
        // An apostrophe.
        assert_eq!(typed("don", 3, "'"), ("don'".into(), 4));
        // Typing the closing character steps over it.
        assert_eq!(typed("f()", 2, ")"), ("f()".into(), 3));
        assert_eq!(typed("\"\"", 1, "\""), ("\"\"".into(), 2));
        // Around a selection.
        let mut d = Document::from_str("say hi");
        d.set_selection(Selection::single(Range::new(4, 6)));
        d.type_char("\"");
        assert_eq!(d.text().to_string(), "say \"hi\"");
        // A quote before an existing string starts a new one; lifetimes get no pair.
        assert_eq!(typed("x = \"abc\"", 4, "\""), ("x = \"\"abc\"".into(), 5));
        assert_eq!(typed("fn f<>", 5, "'"), ("fn f<'>".into(), 6));
        // Backspace in an empty pair removes both.
        let mut d = Document::from_str("f()");
        d.set_cursor(2);
        d.backspace();
        assert_eq!(d.text().to_string(), "f");
    }
}
