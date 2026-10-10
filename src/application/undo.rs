//! Undo and redo of a view's configuration: group, pivot, aggregations, sort,
//! slicers, filters, totals, and column layout.
//!
//! States are recorded as results publish, so every kind of edit is covered
//! without each one recording itself. Search text, column widths and formats,
//! the view's name, and its actions are not part of a state: undo leaves them
//! as they are.
use crate::application::navigation_history::NavigationEntry;
use crate::domain::documents::ViewKind;

#[derive(Default)]
pub struct UndoHistory {
    /// The last recorded state, or `None` before the first one.
    current: Option<NavigationEntry>,
    undo: Vec<NavigationEntry>,
    redo: Vec<NavigationEntry>,
}

impl UndoHistory {
    const CAPACITY: usize = 100;

    /// Starts over, as at a new location or after a save, with `state` as the
    /// first state, or with the next recorded one.
    pub fn reset(&mut self, state: Option<NavigationEntry>) {
        self.undo.clear();
        self.redo.clear();
        self.current = state;
    }

    /// Records `state` as current. When its configuration differs from the
    /// previous state's, the previous state becomes undoable and redo is cleared.
    pub fn record(&mut self, state: NavigationEntry) {
        if let Some(previous) = self.current.take()
            && !same_configuration(&previous, &state)
        {
            self.undo.push(previous);
            if self.undo.len() > Self::CAPACITY {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.current = Some(state);
    }

    /// Steps back, returning the state to restore.
    pub fn undo(&mut self) -> Option<NavigationEntry> {
        let previous = self.undo.pop()?;
        self.redo.extend(self.current.replace(previous.clone()));
        Some(previous)
    }

    /// Steps forward again after an undo, returning the state to restore.
    pub fn redo(&mut self) -> Option<NavigationEntry> {
        let next = self.redo.pop()?;
        self.undo.extend(self.current.replace(next.clone()));
        Some(next)
    }
}

fn same_configuration(a: &NavigationEntry, b: &NavigationEntry) -> bool {
    configuration(a) == configuration(b)
}

/// The parts of a state that undo restores.
fn configuration(state: &NavigationEntry) -> NavigationEntry {
    let mut state = state.clone();
    state.search_text.clear();
    state.dirty = false;
    state.identity = crate::application::session::SessionIdentity::RawSource;
    state.direct_table.widths.clear();
    if let Some(definition) = &mut state.definition {
        definition.id.clear();
        definition.name.clear();
        definition.actions.clear();
        match &mut definition.view {
            ViewKind::Table(config) => config.presentation.clear(),
            // SQL text belongs to the editor, which has its own undo.
            ViewKind::Sql(config) => {
                config.text.clear();
                config.auto_save = false;
                config.auto_run = false;
            }
            ViewKind::Query(_) => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::session::SessionIdentity;
    use crate::domain::documents::{DocumentDefinition, SortKey};
    use crate::domain::table_preferences::TablePreferences;

    fn state(sort: &str) -> NavigationEntry {
        NavigationEntry {
            identity: SessionIdentity::DraftTable,
            source_path: Some("a.csv".into()),
            definition: Some(DocumentDefinition::draft_table("A".into(), "a.csv".into())),
            dirty: false,
            search_text: String::new(),
            sort: (!sort.is_empty())
                .then(|| SortKey {
                    column: sort.into(),
                    descending: false,
                })
                .into_iter()
                .collect(),
            direct_table: TablePreferences::new("a.csv".into()),
        }
    }

    fn sort(state: &NavigationEntry) -> &str {
        state.sort.first().map_or("", |key| key.column.as_str())
    }

    #[test]
    fn changes_undo_and_redo_in_order_and_a_new_change_clears_redo() {
        let mut history = UndoHistory::default();
        assert!(history.undo().is_none());
        for column in ["", "a", "b"] {
            history.record(state(column));
        }
        assert_eq!(sort(&history.undo().unwrap()), "a");
        assert_eq!(sort(&history.undo().unwrap()), "");
        assert!(history.undo().is_none());
        assert_eq!(sort(&history.redo().unwrap()), "a");
        // Recording the restored state again is not a change.
        history.record(state("a"));
        assert_eq!(sort(&history.redo().unwrap()), "b");
        assert!(history.redo().is_none());
        assert_eq!(sort(&history.undo().unwrap()), "a");
        history.record(state("a"));
        history.record(state("c"));
        assert!(history.redo().is_none());
        assert_eq!(sort(&history.undo().unwrap()), "a");
    }

    #[test]
    fn search_widths_names_and_dirtiness_are_not_changes() {
        let mut history = UndoHistory::default();
        history.record(state(""));
        let mut cosmetic = state("");
        cosmetic.search_text = "north".into();
        cosmetic.dirty = true;
        cosmetic.direct_table.widths.insert("a".into(), 80.);
        cosmetic.definition.as_mut().unwrap().name = "Renamed".into();
        history.record(cosmetic);
        assert!(history.undo().is_none());
    }

    #[test]
    fn reset_forgets_both_directions_and_capacity_drops_the_oldest() {
        let mut history = UndoHistory::default();
        history.record(state(""));
        history.record(state("a"));
        history.reset(Some(state("b")));
        assert!(history.undo().is_none());
        assert!(history.redo().is_none());
        for index in 0..=UndoHistory::CAPACITY + 1 {
            history.record(state(&format!("c{index}")));
        }
        let mut steps = 0;
        while history.undo().is_some() {
            steps += 1;
        }
        assert_eq!(steps, UndoHistory::CAPACITY);
    }
}
