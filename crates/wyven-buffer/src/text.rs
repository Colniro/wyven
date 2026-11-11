//! Helpers for lines, columns and character classes.

use ropey::{Rope, RopeSlice};
use unicode_width::UnicodeWidthChar;

/// Number of chars in `line`, without its trailing `\n`.
pub fn line_len_chars(rope: &Rope, line: usize) -> usize {
    let slice = rope.line(line);
    let len = slice.len_chars();
    if len > 0 && slice.char(len - 1) == '\n' {
        len - 1
    } else {
        len
    }
}

/// Screen cells of `c` at column `col` (tabs to the next stop, wide chars two).
pub fn char_width(c: char, col: usize, tab_width: usize) -> usize {
    match c {
        '\t' => tab_width - col % tab_width,
        c => c.width().unwrap_or(0).max(1),
    }
}

/// On-screen column of char `char_col` in `line` (tabs expanded, wide chars count double).
pub fn char_to_display_col(line: RopeSlice<'_>, char_col: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for c in line.chars().take(char_col) {
        if c == '\n' {
            break;
        }
        col += char_width(c, col, tab_width);
    }
    col
}

/// Char index in `line` that is closest to on-screen column `display_col`.
pub fn display_col_to_char(line: RopeSlice<'_>, display_col: usize, tab_width: usize) -> usize {
    let mut col = 0;
    for (i, c) in line.chars().enumerate() {
        if c == '\n' {
            return i;
        }
        let w = char_width(c, col, tab_width);
        if col + w > display_col {
            // Round to the nearer edge of the char.
            return if display_col - col > w / 2 { i + 1 } else { i };
        }
        col += w;
    }
    line.len_chars()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharClass {
    Word,
    Punct,
    Space,
    Newline,
}

pub(crate) fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

/// Start of the word left of `pos` (Strg+←): skips spaces, then one run of the same class.
pub(crate) fn prev_word_start(rope: &Rope, pos: usize) -> usize {
    let mut i = pos;
    while i > 0 && class(rope.char(i - 1)) == CharClass::Space {
        i -= 1;
    }
    if i > 0 && class(rope.char(i - 1)) == CharClass::Newline {
        return if i == pos { i - 1 } else { i };
    }
    if i > 0 {
        let k = class(rope.char(i - 1));
        while i > 0 && class(rope.char(i - 1)) == k {
            i -= 1;
        }
    }
    i
}

/// End of the word right of `pos` (Strg+→).
pub(crate) fn next_word_end(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    let mut i = pos;
    while i < len && class(rope.char(i)) == CharClass::Space {
        i += 1;
    }
    if i < len && class(rope.char(i)) == CharClass::Newline {
        return if i == pos { i + 1 } else { i };
    }
    if i < len {
        let k = class(rope.char(i));
        while i < len && class(rope.char(i)) == k {
            i += 1;
        }
    }
    i
}

/// The word (letters/digits/_) touching `pos`, as a char range.
pub(crate) fn word_at(rope: &Rope, pos: usize) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    let is_word = |i: usize| i < len && class(rope.char(i)) == CharClass::Word;
    let mut start = pos;
    if !is_word(pos) && pos > 0 && is_word(pos - 1) {
        start = pos - 1;
    }
    if !is_word(start) {
        return None;
    }
    while start > 0 && is_word(start - 1) {
        start -= 1;
    }
    let mut end = start;
    while is_word(end) {
        end += 1;
    }
    Some((start, end))
}

/// Leading whitespace of a line.
pub(crate) fn indentation(rope: &Rope, line: usize) -> String {
    rope.line(line)
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_columns_expand_tabs() {
        let rope = Rope::from_str("\tab\tc");
        let line = rope.line(0);
        assert_eq!(char_to_display_col(line, 1, 4), 4);
        assert_eq!(char_to_display_col(line, 4, 4), 8);
        assert_eq!(display_col_to_char(line, 8, 4), 4);
        assert_eq!(display_col_to_char(line, 1, 4), 0);
        assert_eq!(display_col_to_char(line, 3, 4), 1);
    }

    #[test]
    fn word_jumps() {
        let rope = Rope::from_str("foo.bar  baz\nqux");
        assert_eq!(next_word_end(&rope, 0), 3);
        assert_eq!(next_word_end(&rope, 3), 4);
        assert_eq!(next_word_end(&rope, 7), 12);
        assert_eq!(next_word_end(&rope, 12), 13);
        assert_eq!(prev_word_start(&rope, 12), 9);
        assert_eq!(prev_word_start(&rope, 9), 4);
        assert_eq!(prev_word_start(&rope, 13), 12);
        assert_eq!(word_at(&rope, 5), Some((4, 7)));
        assert_eq!(word_at(&rope, 7), Some((4, 7)));
        assert_eq!(word_at(&rope, 8), None);
    }
}
