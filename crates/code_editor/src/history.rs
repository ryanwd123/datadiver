//! Undo and redo. A group holds one or more transactions plus the selections
//! before and after; consecutive typing (or deleting) merges into one group.

use std::time::{Duration, Instant};

use crate::selection::Selections;
use crate::text::Transaction;

/// Typing pauses longer than this start a new undo group.
const MERGE_GAP: Duration = Duration::from_millis(1000);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Typing,
    Delete,
    Other,
}

struct Step {
    tx: Transaction,
    inverse: Transaction,
}

struct Group {
    id: u64,
    steps: Vec<Step>,
    before: Selections,
    after: Selections,
    kind: EditKind,
    time: Instant,
}

/// What to apply for an undo or redo, in order, and the selections afterwards.
pub struct Replay {
    pub txs: Vec<Transaction>,
    pub selections: Selections,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
    next_id: u64,
    /// Id of the top undo group when the text was saved (0: no edits).
    saved: Option<u64>,
    /// The next edit starts a new group.
    sealed: bool,
}

impl History {
    pub fn new() -> Self {
        History {
            next_id: 1,
            saved: Some(0),
            ..Default::default()
        }
    }

    fn top_id(&self) -> u64 {
        self.undo.last().map_or(0, |g| g.id)
    }

    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.top_id())
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.top_id());
        self.sealed = true;
    }

    /// The next edit starts a new undo group.
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.saved = if self.saved == Some(self.top_id()) { Some(0) } else { None };
        self.sealed = false;
    }

    pub fn record(
        &mut self,
        tx: Transaction,
        inverse: Transaction,
        before: Selections,
        after: Selections,
        kind: EditKind,
        now: Instant,
    ) {
        self.redo.clear();
        let saved = self.saved;
        let sealed = std::mem::take(&mut self.sealed);
        if let Some(g) = self.undo.last_mut()
            && !sealed
            && kind != EditKind::Other
            && g.kind == kind
            && g.after == before
            && now.duration_since(g.time) < MERGE_GAP
            && saved != Some(g.id)
        {
            g.steps.push(Step { tx, inverse });
            g.after = after;
            g.time = now;
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.undo.push(Group {
            id,
            steps: vec![Step { tx, inverse }],
            before,
            after,
            kind,
            time: now,
        });
    }

    pub fn undo(&mut self) -> Option<Replay> {
        let g = self.undo.pop()?;
        let replay = Replay {
            txs: g.steps.iter().rev().map(|s| s.inverse.clone()).collect(),
            selections: g.before.clone(),
        };
        self.redo.push(g);
        self.sealed = true;
        Some(replay)
    }

    pub fn redo(&mut self) -> Option<Replay> {
        let g = self.redo.pop()?;
        let replay = Replay {
            txs: g.steps.iter().map(|s| s.tx.clone()).collect(),
            selections: g.after.clone(),
        };
        self.undo.push(g);
        self.sealed = true;
        Some(replay)
    }
}
