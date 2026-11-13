//! Find: the search box highlights what it matches and steps between the
//! matching rows, leaving every row in place.

use std::sync::Arc;

use super::*;
use crate::domain::query;
use crate::domain::selection::Cell;
use crate::infrastructure::store::ViewQuery;

/// How long typing rests before the matching rows are counted.
const FIND_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct FindState {
    /// The search box finds rather than filters.
    pub(crate) on: bool,
    /// What the search box holds while finding; the session's search stays empty.
    pub(crate) text: String,
    /// Display positions of the matching rows, ascending. `None` until they
    /// are counted, and while there is nothing to find.
    pub(crate) matches: Option<Arc<[usize]>>,
    pub(super) revision: u64,
    /// A step asked for before the matches were counted.
    pub(super) pending_step: Option<bool>,
}

impl FindState {
    fn clear(&mut self) {
        self.text.clear();
        self.matches = None;
        self.pending_step = None;
        self.revision += 1;
    }
}

impl DataFrameView {
    /// The text whose words the grid highlights.
    pub(crate) fn highlight_text(&self) -> &str {
        if self.find.on { &self.find.text } else { &self.session.search_text }
    }

    /// Shows the session's search text in the search box. While finding, the
    /// box keeps what it is finding, unless the session brings a filter of
    /// its own, which ends finding so the filter is not applied unseen.
    pub(crate) fn show_search_text(&mut self, text: &str) {
        if self.find.on {
            if text.is_empty() {
                return;
            }
            self.find.on = false;
            self.find.clear();
            self.search_input.placeholder = "Search rows".into();
        }
        self.search_input.set_text(text);
    }

    /// Switches the search box between narrowing the rows and finding in
    /// them, carrying its text across.
    pub(crate) fn toggle_find(&mut self, cx: &mut Cx<Self>) {
        let on = !self.find.on;
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.search_finds = on;
            self.repository.save_settings(&settings)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save settings: {error}"));
        }
        let text = self.search_input.text().to_owned();
        self.find.on = on;
        if on {
            self.search_input.placeholder = "Find in rows".into();
            self.find.text = text;
            self.copy_status = Some("Search finds without filtering · Enter and Shift+Enter, or F3 and Shift+F3, step between matches".into());
            // Publishing the unfiltered rows counts the matches.
            self.schedule_search(String::new(), cx);
        } else {
            self.search_input.placeholder = "Search rows".into();
            self.find.clear();
            self.copy_status = Some("Search filters the rows".into());
            self.schedule_search(text, cx);
        }
        cx.notify();
    }

    /// The text to find changed: highlight it now and count its rows once
    /// typing rests.
    pub(crate) fn set_find_text(&mut self, text: String, debounce: bool, cx: &mut Cx<Self>) {
        self.find.text = text;
        self.find.matches = None;
        self.find.pending_step = None;
        self.find.revision += 1;
        self.grid.set_highlights(query::highlight_terms(&self.find.text));
        cx.notify();
        if !debounce {
            self.count_find_matches(cx);
            return;
        }
        let revision = self.find.revision;
        cx.timer(FIND_DEBOUNCE, move |view, cx| {
            if view.find.revision == revision {
                view.count_find_matches(cx);
            }
        })
        .detach();
    }

    /// Finds which of the displayed rows match, on a worker.
    pub(crate) fn count_find_matches(&mut self, cx: &mut Cx<Self>) {
        self.find.revision += 1;
        self.find.matches = None;
        if !self.find.on || self.find.text.trim().is_empty() {
            self.find.pending_step = None;
            return;
        }
        let tokens = match query::parse(&self.find.text) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let hidden = match self.session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(config)) => config.hidden.clone(),
            _ => Vec::new(),
        };
        let revision = self.find.revision;
        let result = self.session.result.clone();
        let result_revision = result.result_revision;
        cx.spawn(
            move || matching_positions(&result, ViewQuery { tokens, hidden, ..Default::default() }),
            move |view: &mut Self, cx, positions| {
                if view.find.revision != revision || view.session.result.result_revision != result_revision {
                    return;
                }
                match positions {
                    Ok(positions) => {
                        view.find.matches = Some(positions.into());
                        if let Some(forward) = view.find.pending_step.take() {
                            view.find_step(forward, cx);
                        }
                    }
                    Err(error) => {
                        view.find.pending_step = None;
                        view.error = Some(format!("Find failed: {error}"));
                    }
                }
                cx.notify();
            },
        )
        .detach();
    }

    /// Moves to the next or previous matching row, wrapping at the ends, and
    /// lands on the first cell there that shows a match.
    pub(crate) fn find_step(&mut self, forward: bool, cx: &mut Cx<Self>) {
        if !self.find.on {
            self.copy_status = Some("Turn on Search: find without filtering to step between matches".into());
            cx.notify();
            return;
        }
        if self.find.text.trim().is_empty() {
            return;
        }
        let Some(matches) = self.find.matches.clone() else {
            // Still counting; step once the matches are known.
            self.find.pending_step = Some(forward);
            return;
        };
        if matches.is_empty() {
            return;
        }
        let active = self.grid.selection().map(|selection| selection.active);
        let index = match active {
            None => {
                if forward { 0 } else { matches.len() - 1 }
            }
            Some(cell) if forward => {
                let next = matches.partition_point(|&row| row <= cell.row);
                if next == matches.len() { 0 } else { next }
            }
            Some(cell) => {
                let before = matches.partition_point(|&row| row < cell.row);
                if before == 0 { matches.len() - 1 } else { before - 1 }
            }
        };
        let row = matches[index];
        let column = self.find_column(row).or(active.map(|cell| cell.column)).unwrap_or(0);
        self.grid.jump_to_cell(Cell { row, column });
        cx.notify();
    }

    /// The first column of `row` that highlights a word being found.
    fn find_column(&self, row: usize) -> Option<usize> {
        let result = &self.session.result;
        let values = result.fetch_rows(row, 1).ok()?.into_iter().next()?.values;
        let names = result.columns.iter().map(|column| column.0.as_str()).collect::<Vec<_>>();
        let terms = query::highlight_terms(&self.find.text).for_columns(&names);
        (0..names.len()).find(|&column| {
            let text = values.get(result.column_indices[column]).and_then(Option::as_deref).unwrap_or_default();
            terms.get(column).is_some_and(|terms| !query::highlight_ranges(text, terms).is_empty())
        })
    }

    /// What the status bar says about the matches, while finding.
    pub(crate) fn find_status(&self) -> Option<String> {
        if !self.find.on || self.find.text.trim().is_empty() {
            return None;
        }
        let Some(matches) = &self.find.matches else {
            return Some("Finding…".into());
        };
        let count = crate::domain::formatting::group_numeric_text(&matches.len().to_string());
        if matches.is_empty() {
            return Some("No matching rows".into());
        }
        let position = self
            .grid
            .selection()
            .and_then(|selection| matches.binary_search(&selection.active.row).ok());
        Some(match position {
            Some(index) => format!(
                "Match {} of {count}",
                crate::domain::formatting::group_numeric_text(&(index + 1).to_string())
            ),
            None if matches.len() == 1 => "1 matching row".into(),
            None => format!("{count} matching rows"),
        })
    }
}

/// The display positions of `result`'s rows that `view` keeps, ascending.
fn matching_positions(result: &ResultSnapshot, view: ViewQuery) -> Result<Vec<usize>, String> {
    use crate::infrastructure::store::RowIds;
    let ids = result
        .store
        .lock()
        .map_err(|_| "store lock poisoned".to_owned())?
        .query_row_ids(&view)?;
    Ok(match &result.row_ids {
        // A position is its rowid.
        RowIds::Natural(count) => {
            let mut positions = ids
                .into_iter()
                .filter_map(|id| usize::try_from(id).ok().filter(|position| position < count))
                .collect::<Vec<_>>();
            positions.sort_unstable();
            positions
        }
        RowIds::Listed(shown) => {
            let matching = ids.into_iter().collect::<std::collections::HashSet<_>>();
            shown
                .iter()
                .enumerate()
                .filter_map(|(position, id)| matching.contains(id).then_some(position))
                .collect()
        }
    })
}
