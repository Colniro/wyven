use ropey::Rope;

use crate::selection::{Range, Selection};

/// Replace the chars `start..end` (in the text *before* the transaction) with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// One applied edit in byte offsets and (row, byte column) points, for incremental parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextEdit {
    pub start_byte: usize,
    pub old_end_byte: usize,
    pub new_end_byte: usize,
    pub start_point: (usize, usize),
    pub old_end_point: (usize, usize),
    pub new_end_point: (usize, usize),
}

fn point(rope: &Rope, byte: usize) -> (usize, usize) {
    let line = rope.byte_to_line(byte);
    (line, byte - rope.line_to_byte(line))
}

/// Where a position inside or at the edge of an insertion ends up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// A set of non-overlapping changes, all expressed in coordinates of the original text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transaction {
    changes: Vec<Change>,
}

impl Transaction {
    /// Builds a transaction. Changes are sorted; overlapping changes are a bug in the caller.
    pub fn new(mut changes: Vec<Change>) -> Self {
        changes.sort_by_key(|c| (c.start, c.end));
        changes.dedup_by(|b, a| a.start == b.start && a.end == b.end && a.text == b.text);
        debug_assert!(
            changes.windows(2).all(|w| w[0].end <= w[1].start),
            "overlapping changes: {changes:?}"
        );
        Self { changes }
    }

    /// One change per range of the selection, built by `f`.
    pub fn for_each_range(sel: &Selection, mut f: impl FnMut(&Range) -> Change) -> Self {
        Self::new(sel.ranges().iter().map(&mut f).collect())
    }

    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub fn is_empty(&self) -> bool {
        self.changes
            .iter()
            .all(|c| c.start == c.end && c.text.is_empty())
    }

    /// Applies the changes to `rope` and returns the transaction that undoes them.
    pub fn apply(&self, rope: &mut Rope) -> Transaction {
        self.apply_with(rope, &mut |_| {})
    }

    /// Like [`apply`](Self::apply), but reports every single edit in byte/point coordinates
    /// (what tree-sitter needs). Edits are reported in the order they are applied, each in
    /// coordinates of the text as it is at that moment.
    pub fn apply_with(&self, rope: &mut Rope, on_edit: &mut dyn FnMut(TextEdit)) -> Transaction {
        let removed: Vec<String> = self
            .changes
            .iter()
            .map(|c| rope.slice(c.start..c.end).to_string())
            .collect();

        let mut inverse = Vec::with_capacity(self.changes.len());
        let mut delta: isize = 0;
        for (c, old) in self.changes.iter().zip(&removed) {
            let new_start = (c.start as isize + delta) as usize;
            let inserted = c.text.chars().count();
            inverse.push(Change {
                start: new_start,
                end: new_start + inserted,
                text: old.clone(),
            });
            delta += inserted as isize - (c.end - c.start) as isize;
        }

        // Apply back to front so earlier offsets stay valid.
        for c in self.changes.iter().rev() {
            let start_byte = rope.char_to_byte(c.start);
            let old_end_byte = rope.char_to_byte(c.end);
            let start_point = point(rope, start_byte);
            let old_end_point = point(rope, old_end_byte);
            if c.end > c.start {
                rope.remove(c.start..c.end);
            }
            if !c.text.is_empty() {
                rope.insert(c.start, &c.text);
            }
            let new_end_byte = start_byte + c.text.len();
            on_edit(TextEdit {
                start_byte,
                old_end_byte,
                new_end_byte,
                start_point,
                old_end_point,
                new_end_point: point(rope, new_end_byte),
            });
        }
        Transaction { changes: inverse }
    }

    /// Where `pos` (in the old text) ends up in the new text.
    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for c in &self.changes {
            let inserted = c.text.chars().count() as isize;
            if pos < c.start || (pos == c.start && c.start == c.end && assoc == Assoc::Before) {
                break;
            }
            if pos > c.end || (pos == c.end && c.start < c.end) {
                delta += inserted - (c.end - c.start) as isize;
                continue;
            }
            // Inside the replaced range (or exactly at an insertion point).
            let new_start = c.start as isize + delta;
            return match assoc {
                Assoc::Before => new_start as usize,
                Assoc::After => (new_start + inserted) as usize,
            };
        }
        (pos as isize + delta) as usize
    }

    /// Maps every cursor through the transaction.
    pub fn map_selection(&self, sel: &Selection) -> Selection {
        sel.map(|r| {
            if r.is_empty() {
                Range::point(self.map_pos(r.head, Assoc::After))
            } else {
                let (a, h) = if r.anchor < r.head {
                    (Assoc::Before, Assoc::After)
                } else {
                    (Assoc::After, Assoc::Before)
                };
                Range::new(self.map_pos(r.anchor, a), self.map_pos(r.head, h))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(start: usize, end: usize, text: &str) -> Change {
        Change { start, end, text: text.into() }
    }

    #[test]
    fn apply_and_undo_multiple_changes() {
        let mut rope = Rope::from_str("hello world");
        let tx = Transaction::new(vec![change(0, 5, "HEY"), change(6, 6, "big ")]);
        let inverse = tx.apply(&mut rope);
        assert_eq!(rope.to_string(), "HEY big world");
        inverse.apply(&mut rope);
        assert_eq!(rope.to_string(), "hello world");
    }

    #[test]
    fn cursors_move_after_insertions() {
        let tx = Transaction::new(vec![change(2, 2, "ab"), change(5, 5, "c")]);
        assert_eq!(tx.map_pos(2, Assoc::After), 4);
        assert_eq!(tx.map_pos(2, Assoc::Before), 2);
        assert_eq!(tx.map_pos(5, Assoc::After), 8);
        assert_eq!(tx.map_pos(9, Assoc::After), 12);
    }

    #[test]
    fn positions_inside_deletions_collapse() {
        let tx = Transaction::new(vec![change(2, 6, "")]);
        assert_eq!(tx.map_pos(4, Assoc::After), 2);
        assert_eq!(tx.map_pos(6, Assoc::After), 2);
        assert_eq!(tx.map_pos(8, Assoc::After), 4);
    }
}
