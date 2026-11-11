//! Undo/redo. Consecutive typing is grouped into one undo step.

use std::time::{Duration, Instant};

use crate::selection::Selection;
use crate::transaction::Transaction;

/// What kind of edit a step was; consecutive steps of the same kind may be grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditKind {
    Insert,
    Delete,
    Other,
}

const GROUP_TIMEOUT: Duration = Duration::from_millis(1000);

struct Step {
    forward: Transaction,
    inverse: Transaction,
}

struct Group {
    steps: Vec<Step>,
    sel_before: Selection,
    sel_after: Selection,
    kind: EditKind,
    last_edit: Instant,
}

#[derive(Default)]
pub(crate) struct History {
    groups: Vec<Group>,
    /// Number of groups that are currently applied; groups after this index can be redone.
    current: usize,
    /// Set by `break_group` (e.g. after the cursor moved) so the next edit starts a new group.
    sealed: bool,
}

impl History {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(
        &mut self,
        forward: Transaction,
        inverse: Transaction,
        sel_before: Selection,
        sel_after: Selection,
        kind: EditKind,
        breaks_group: bool,
    ) {
        self.groups.truncate(self.current);
        let now = Instant::now();
        let step = Step { forward, inverse };
        if let Some(last) = self.groups.last_mut() {
            let can_group = !self.sealed
                && !breaks_group
                && kind != EditKind::Other
                && last.kind == kind
                && last.sel_after == sel_before
                && now.duration_since(last.last_edit) < GROUP_TIMEOUT;
            if can_group {
                last.steps.push(step);
                last.sel_after = sel_after;
                last.last_edit = now;
                return;
            }
        }
        self.groups.push(Group {
            steps: vec![step],
            sel_before,
            sel_after,
            kind,
            last_edit: now,
        });
        self.current = self.groups.len();
        self.sealed = false;
    }

    pub(crate) fn break_group(&mut self) {
        self.sealed = true;
    }

    /// Returns the transactions to apply (in order) and the selection to restore.
    pub(crate) fn undo(&mut self) -> Option<(Vec<Transaction>, Selection)> {
        if self.current == 0 {
            return None;
        }
        self.current -= 1;
        self.sealed = true;
        let group = &self.groups[self.current];
        let txs = group.steps.iter().rev().map(|s| s.inverse.clone()).collect();
        Some((txs, group.sel_before.clone()))
    }

    pub(crate) fn redo(&mut self) -> Option<(Vec<Transaction>, Selection)> {
        if self.current >= self.groups.len() {
            return None;
        }
        let group = &self.groups[self.current];
        self.current += 1;
        self.sealed = true;
        let txs = group.steps.iter().map(|s| s.forward.clone()).collect();
        Some((txs, group.sel_after.clone()))
    }
}
