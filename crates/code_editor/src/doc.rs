//! A document: the text buffer, its selections and its undo history, edited
//! only through transactions so every change can be undone and observed.

use std::time::Instant;

use crate::commands::Outcome;
use crate::history::{EditKind, History};
use crate::selection::Selections;
use crate::text::{Buffer, Change, Decoded, Transaction};

pub struct Document {
    pub buffer: Buffer,
    pub sels: Selections,
    pub history: History,
}

impl Default for Document {
    fn default() -> Self {
        Document::new("")
    }
}

impl Document {
    pub fn new(text: &str) -> Self {
        Document {
            buffer: Buffer::new(text),
            sels: Selections::default(),
            history: History::new(),
        }
    }

    pub fn from_decoded(d: Decoded) -> Self {
        Document {
            buffer: Buffer::from_decoded(d),
            sels: Selections::default(),
            history: History::new(),
        }
    }

    /// Applies a command's outcome as one undoable step. Returns the applied
    /// changes (empty when the outcome changed only the selections).
    pub fn edit(&mut self, o: Outcome, kind: EditKind) -> Vec<Change> {
        self.apply_tx(o.tx, o.sels, kind, Instant::now())
    }

    pub fn apply_tx(
        &mut self,
        tx: Transaction,
        after: Selections,
        kind: EditKind,
        now: Instant,
    ) -> Vec<Change> {
        if tx.is_empty() {
            self.sels = after;
            return Vec::new();
        }
        let before = std::mem::replace(&mut self.sels, after.clone());
        let (inverse, changes) = self.buffer.apply(&tx);
        self.history.record(tx, inverse, before, after, kind, now);
        changes
    }

    /// Moves the selections without editing; the next edit starts a new undo
    /// group.
    pub fn set_selections(&mut self, sels: Selections) {
        if sels != self.sels {
            self.history.seal();
        }
        self.sels = sels;
    }

    pub fn undo(&mut self) -> Option<Vec<Change>> {
        let r = self.history.undo()?;
        Some(self.replay(r))
    }

    pub fn redo(&mut self) -> Option<Vec<Change>> {
        let r = self.history.redo()?;
        Some(self.replay(r))
    }

    fn replay(&mut self, r: crate::history::Replay) -> Vec<Change> {
        let mut changes = Vec::new();
        for tx in &r.txs {
            changes.extend(self.buffer.apply(tx).1);
        }
        self.sels = r.selections;
        changes
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// Replaces the text and forgets the history (opening a file).
    pub fn reset(&mut self, buffer: Buffer) {
        self.buffer = buffer;
        self.sels = Selections::default();
        self.history = History::new();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::commands::{self, tests::{parse, show}};

    fn doc(marked: &str) -> Document {
        let (buffer, sels) = parse(marked);
        Document {
            buffer,
            sels,
            history: History::new(),
        }
    }

    fn type_at(d: &mut Document, s: &str, now: Instant) {
        let o = commands::insert(&d.sels, s);
        d.apply_tx(o.tx, o.sels, EditKind::Typing, now);
    }

    #[test]
    fn typing_merges_into_one_undo_step() {
        let mut d = doc("|");
        let t = Instant::now();
        for (i, c) in ["a", "b", "c"].iter().enumerate() {
            type_at(&mut d, c, t + Duration::from_millis(100 * i as u64));
        }
        assert_eq!(show(&d.buffer, &d.sels), "abc|");
        d.undo();
        assert_eq!(show(&d.buffer, &d.sels), "|");
        d.redo();
        assert_eq!(show(&d.buffer, &d.sels), "abc|");
    }

    #[test]
    fn pause_or_caret_move_splits_typing() {
        let mut d = doc("|");
        let t = Instant::now();
        type_at(&mut d, "a", t);
        type_at(&mut d, "b", t + Duration::from_secs(5));
        d.undo();
        assert_eq!(show(&d.buffer, &d.sels), "a|");

        let mut d = doc("|x");
        type_at(&mut d, "a", t);
        let (_, moved) = parse("ax|");
        d.set_selections(moved);
        type_at(&mut d, "b", t);
        d.undo();
        assert_eq!(show(&d.buffer, &d.sels), "ax|");
    }

    #[test]
    fn multi_cursor_undo_redo_round_trip() {
        let mut d = doc("a|b\nc|d");
        let o = commands::backspace(&d.buffer, &d.sels, Default::default());
        d.edit(o, EditKind::Delete);
        assert_eq!(show(&d.buffer, &d.sels), "|b\n|d");
        let o = commands::insert(&d.sels, "XY");
        d.edit(o, EditKind::Other);
        assert_eq!(show(&d.buffer, &d.sels), "XY|b\nXY|d");
        d.undo();
        d.undo();
        assert_eq!(show(&d.buffer, &d.sels), "a|b\nc|d");
        d.redo();
        d.redo();
        assert_eq!(show(&d.buffer, &d.sels), "XY|b\nXY|d");
        assert!(d.redo().is_none());
    }

    #[test]
    fn dirty_tracking() {
        let mut d = doc("|");
        assert!(!d.is_dirty());
        type_at(&mut d, "a", Instant::now());
        assert!(d.is_dirty());
        d.history.mark_saved();
        assert!(!d.is_dirty());
        // Typing after a save does not merge into the saved group.
        type_at(&mut d, "b", Instant::now());
        assert!(d.is_dirty());
        d.undo();
        assert!(!d.is_dirty());
        d.undo();
        assert!(d.is_dirty());
        d.redo();
        assert!(!d.is_dirty());
    }
}
