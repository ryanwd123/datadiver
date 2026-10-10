use crate::application::session::SessionIdentity;
use crate::domain::documents::{DocumentDefinition, SortKey};
use crate::domain::table_preferences::TablePreferences;

/// A location can be rebuilt without retaining a source, result, or database handle.
#[derive(Clone, Debug, PartialEq)]
pub struct NavigationEntry {
    pub identity: SessionIdentity,
    pub source_path: Option<String>,
    pub definition: Option<DocumentDefinition>,
    pub dirty: bool,
    pub search_text: String,
    pub sort: Vec<SortKey>,
    pub direct_table: TablePreferences,
}

#[derive(Default)]
pub struct NavigationHistory {
    entries: Vec<NavigationEntry>,
    cursor: Option<usize>,
}

impl NavigationHistory {
    const CAPACITY: usize = 100;

    pub fn remember_current(&mut self, entry: NavigationEntry) {
        if let Some(cursor) = self.cursor {
            self.entries[cursor] = entry;
        } else {
            self.push(entry);
        }
    }

    pub fn target(&self, forward: bool) -> Option<(usize, NavigationEntry)> {
        let cursor = self.cursor?;
        let index = if forward {
            cursor + 1
        } else {
            cursor.checked_sub(1)?
        };
        self.entries.get(index).cloned().map(|entry| (index, entry))
    }

    pub fn commit(&mut self, index: usize, entry: NavigationEntry) {
        self.entries[index] = entry;
        self.cursor = Some(index);
    }

    pub fn push(&mut self, entry: NavigationEntry) {
        if let Some(cursor) = self.cursor {
            if self.entries[cursor] == entry {
                return;
            }
            self.entries.truncate(cursor + 1);
        }
        self.entries.push(entry);
        if self.entries.len() > Self::CAPACITY {
            self.entries.remove(0);
        }
        self.cursor = Some(self.entries.len() - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> NavigationEntry {
        NavigationEntry {
            identity: SessionIdentity::RawSource,
            source_path: Some(name.into()),
            definition: None,
            dirty: false,
            search_text: String::new(),
            sort: Vec::new(),
            direct_table: TablePreferences::new(name.into()),
        }
    }

    #[test]
    fn boundaries_duplicate_suppression_and_branching() {
        let mut history = NavigationHistory::default();
        assert!(history.target(false).is_none());
        assert!(history.target(true).is_none());
        history.push(entry("a"));
        history.push(entry("a"));
        assert_eq!(history.entries.len(), 1);
        history.push(entry("b"));
        history.push(entry("c"));
        let (index, previous) = history.target(false).unwrap();
        assert_eq!(history.cursor, Some(2)); // Requesting a target does not commit it.
        history.commit(index, previous);
        let mut edited = entry("b");
        edited.search_text = "edited".into();
        history.remember_current(edited.clone());
        assert_eq!(history.target(true).unwrap().1, entry("c"));
        history.push(entry("d"));
        assert!(history.target(true).is_none());
        assert_eq!(history.target(false).unwrap().1, edited);
    }

    #[test]
    fn capacity_and_window_independence() {
        let mut first = NavigationHistory::default();
        let second = NavigationHistory::default();
        for index in 0..105 {
            first.push(entry(&index.to_string()));
        }
        assert_eq!(first.entries.len(), 100);
        assert_eq!(first.entries[0], entry("5"));
        assert_eq!(first.cursor, Some(99));
        assert!(second.target(false).is_none());
        for _ in 0..99 {
            let (index, previous) = first.target(false).unwrap();
            first.commit(index, previous);
        }
        assert!(first.target(false).is_none());
        assert_eq!(first.target(true).unwrap().1, entry("6"));
    }
}
