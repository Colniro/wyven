/// One cursor with an optional selection. `anchor` stays put, `head` is where the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub anchor: usize,
    pub head: usize,
    /// Display column the cursor wants to be in when moving up/down through shorter lines.
    pub goal: Option<usize>,
}

impl Range {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head, goal: None }
    }

    pub fn point(pos: usize) -> Self {
        Self::new(pos, pos)
    }

    pub fn from(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn to(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn len(&self) -> usize {
        self.to() - self.from()
    }

    /// Moves the head; keeps the anchor only when `extend` (Shift held).
    pub fn put(self, head: usize, extend: bool) -> Self {
        Self {
            anchor: if extend { self.anchor } else { head },
            head,
            goal: None,
        }
    }

    fn overlaps(&self, other: &Range) -> bool {
        // Two cursors at the same spot, or ranges that share at least one char.
        self.from() == other.from() || (self.from() < other.to() && other.from() < self.to())
    }

    fn merge(self, other: Range) -> Range {
        let from = self.from().min(other.from());
        let to = self.to().max(other.to());
        if self.head < self.anchor {
            Range::new(to, from)
        } else {
            Range::new(from, to)
        }
    }
}

/// All cursors of a document. Always sorted by position, never overlapping, never empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    ranges: Vec<Range>,
    primary: usize,
}

impl Selection {
    pub fn single(range: Range) -> Self {
        Self { ranges: vec![range], primary: 0 }
    }

    pub fn point(pos: usize) -> Self {
        Self::single(Range::point(pos))
    }

    /// Builds a selection from any ranges; sorts and merges overlaps.
    pub fn new(ranges: Vec<Range>, primary: usize) -> Self {
        assert!(!ranges.is_empty(), "a selection needs at least one range");
        let primary = primary.min(ranges.len() - 1);
        let mut sel = Self { ranges, primary };
        sel.normalize();
        sel
    }

    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Applies `f` to every range and re-normalizes.
    pub fn map(&self, mut f: impl FnMut(Range) -> Range) -> Self {
        Self::new(self.ranges.iter().map(|r| f(*r)).collect(), self.primary)
    }

    pub fn push(&self, range: Range) -> Self {
        let mut ranges = self.ranges.clone();
        ranges.push(range);
        let primary = ranges.len() - 1;
        Self::new(ranges, primary)
    }

    /// Only the primary cursor stays (Esc).
    pub fn into_primary(&self) -> Self {
        Self::single(self.primary())
    }

    fn normalize(&mut self) {
        let primary = self.ranges[self.primary];
        self.ranges.sort_by_key(|r| (r.from(), r.to()));
        let mut merged: Vec<Range> = Vec::with_capacity(self.ranges.len());
        let mut primary_idx = 0;
        for r in self.ranges.drain(..) {
            let is_primary = r == primary;
            match merged.last_mut() {
                Some(last) if last.overlaps(&r) => {
                    *last = last.merge(r);
                    if is_primary {
                        primary_idx = merged.len() - 1;
                    }
                }
                _ => {
                    merged.push(r);
                    if is_primary {
                        primary_idx = merged.len() - 1;
                    }
                }
            }
        }
        self.ranges = merged;
        self.primary = primary_idx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_overlapping_and_duplicate_cursors() {
        let sel = Selection::new(
            vec![Range::new(5, 8), Range::point(2), Range::new(7, 10), Range::point(2)],
            0,
        );
        assert_eq!(sel.ranges(), &[Range::point(2), Range::new(5, 10)]);
        assert_eq!(sel.primary(), Range::new(5, 10));
    }

    #[test]
    fn touching_ranges_stay_separate() {
        let sel = Selection::new(vec![Range::new(0, 3), Range::new(3, 6)], 1);
        assert_eq!(sel.len(), 2);
        assert_eq!(sel.primary(), Range::new(3, 6));
    }
}
