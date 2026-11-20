//! Outline mode: turned on from the command menu, it collapses and expands
//! a query's group-by fields (Alt+Shift+Left/Right) and items
//! (Alt+Left/Right), chooses its layout, and shows subtotals.

use super::*;
use crate::domain::documents::{Dimension, Outline, OutlineLayout, OutlinePath, QueryConfig};
use crate::domain::frame::OutlineRow;
use crate::ui::widgets::list::Item;

const MODE_OFF: &str = "Outline mode is off: Ctrl+K, then O, turns it on";
const TOP_LEVEL: &str = "This is the outline's top level";

/// The row under the cursor as an outline row, and the field whose key the
/// cursor's column shows, if it shows one.
struct ActiveRow {
    field: Option<usize>,
    row: OutlineRow,
}

impl DataFrameView {
    fn query_config(&self) -> Option<&QueryConfig> {
        match self.session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Query(config)) => Some(config),
            _ => None,
        }
    }

    fn outline_note(&mut self, text: impl Into<String>, cx: &mut Cx<Self>) {
        self.copy_status = Some(text.into());
        cx.notify();
    }

    /// Applies `edit` to the open query and runs it again; `edit` returns
    /// whether it changed anything.
    fn edit_query(&mut self, edit: impl FnOnce(&mut QueryConfig) -> bool, cx: &mut Cx<Self>) -> bool {
        let Some(config) = self.query_config() else {
            self.outline_note("Outlines apply to query results: group by a field first", cx);
            return false;
        };
        let mut updated = config.clone();
        if !edit(&mut updated) {
            cx.notify();
            return false;
        }
        if let Err(error) = self.session.edit_definition(|definition| definition.view = ViewKind::Query(updated)) {
            self.error = Some(error);
            cx.notify();
            return false;
        }
        self.schedule_search(self.session.search_text.clone(), cx);
        cx.notify();
        true
    }

    /// The open query in outline mode, or a note on why the outline
    /// commands don't apply.
    fn outline_query(&self) -> Result<&QueryConfig, &'static str> {
        let config = self.query_config().ok_or("Outlines apply to query results: group by a field first")?;
        if config.outline.is_none() {
            return Err(MODE_OFF);
        }
        if config.shown_group_by().is_empty() {
            return Err("Group by a field first (G)");
        }
        Ok(config)
    }

    /// Turns outline mode on, in the compact layout, or off, back to the
    /// flat result of every group-by field.
    pub(crate) fn toggle_outline(&mut self, cx: &mut Cx<Self>) {
        let Some(config) = self.query_config() else {
            return self.outline_note("Outline mode applies to query results: group by a field first", cx);
        };
        if config.outline.is_none() && config.group_by.is_empty() {
            return self.outline_note("Group by a field first (G), then turn outline mode on", cx);
        }
        let on = config.outline.is_none();
        let changed = self.edit_query(|config| {
            if on {
                config.outline = Some(Outline::default());
            } else {
                config.outline = None;
                config.depth = None;
            }
            true
        }, cx);
        if changed {
            self.outline_note(
                if on {
                    "Outline mode on: Alt+Left/Right collapse and expand items, Alt+Shift+Left/Right whole fields"
                } else {
                    "Outline mode off"
                },
                cx,
            );
        }
    }

    fn active_outline_row(&self) -> Result<ActiveRow, String> {
        let config = self.query_config().ok_or("Outlines apply to query results")?;
        let selection = self.grid.selection().ok_or("Select a cell first")?;
        let result = &self.session.result;
        let fields = config.shown_group_by().len();
        let rows = match result.cached_rows(selection.active.row, 1) {
            Some(rows) => rows,
            None => result.fetch_rows(selection.active.row, 1)?,
        };
        let cached = rows.first().ok_or("The active row is no longer available")?;
        let store_column = result.column_indices.get(selection.active.column).copied();
        match &result.outline {
            Some(outline) => {
                let row = outline.row(cached).ok_or("The active row is not an outline row")?;
                let field = store_column.and_then(|index| outline.field_of(index)).map(|field| {
                    // The compact layout's one key column shows each row's own key.
                    if outline.layout == OutlineLayout::Compact { row.level - 1 } else { field }
                });
                Ok(ActiveRow { field, row })
            }
            None => {
                // In a flat result, such as one shown field, every row is a
                // group of every field.
                if result.column_count() < fields {
                    return Err("The result is still running".into());
                }
                let path = (0..fields)
                    .map(|index| cached.values.get(result.column_indices[index]).cloned().flatten())
                    .collect();
                Ok(ActiveRow {
                    field: (selection.active.column < fields).then_some(selection.active.column),
                    row: OutlineRow { level: fields, open: false, path },
                })
            }
        }
    }

    /// The shown group-by field whose key the cursor's column shows: the
    /// column's own, or in the compact layout the row's own field.
    pub(crate) fn active_group_field(&self) -> Option<usize> {
        let column = self.grid.selection()?.active.column;
        match &self.session.result.outline {
            Some(outline) if outline.layout == OutlineLayout::Compact => {
                (column == 0).then(|| self.active_outline_row().ok()?.field).flatten()
            }
            _ => Some(column),
        }
    }

    /// The level (from 1) a field-level command applies to: the cursor's
    /// field, or the row's own level on a column without keys.
    fn active_level(&self) -> Result<usize, String> {
        let active = self.active_outline_row()?;
        Ok(active.field.map_or(active.row.level, |field| field + 1))
    }

    /// Collapses the item under the cursor into its subtotal row: the item of
    /// the cursor's key column, or else the row's own item when it is
    /// expanded, or its parent.
    pub(crate) fn collapse_item(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_query() {
            Ok(config) => config.shown_group_by().len(),
            Err(note) => return self.outline_note(note, cx),
        };
        let active = match self.active_outline_row() {
            Ok(active) => active,
            Err(error) => return self.outline_note(error, cx),
        };
        let level = active.row.level;
        let target = match active.field {
            Some(field) if field + 1 < fields && (field + 1 < level || (field + 1 == level && active.row.open)) => field + 1,
            _ if active.row.open => level,
            _ => level - 1,
        };
        if target == 0 {
            return self.outline_note(TOP_LEVEL, cx);
        }
        let path = active.row.path[..target].to_vec();
        self.edit_query(|config| config.outline.get_or_insert_with(Default::default).set_collapsed(&path, true), cx);
    }

    /// Expands the collapsed item under the cursor. On a row of the lowest
    /// level, expands that item into the next field: a collapsed one, or a
    /// column chosen from a list.
    pub(crate) fn expand_item(&mut self, cx: &mut Cx<Self>) {
        let (fields, hidden) = match self.outline_query() {
            Ok(config) => (config.shown_group_by().len(), config.shown_group_by().len() < config.group_by.len()),
            Err(note) => return self.outline_note(note, cx),
        };
        let active = match self.active_outline_row() {
            Ok(active) => active,
            Err(error) => return self.outline_note(error, cx),
        };
        if active.row.open {
            return self.outline_note("This item is expanded already; Alt+Left collapses it", cx);
        }
        let path = active.row.path;
        if active.row.level < fields {
            self.edit_query(|config| config.outline.get_or_insert_with(Default::default).set_collapsed(&path, false), cx);
        } else if hidden {
            self.edit_query(|config| {
                show_next_field(config);
                expand_only(config, &path, fields);
                true
            }, cx);
        } else {
            self.open_expand_into(Some(path), cx);
        }
    }

    /// Groups by only the fields before the cursor's field (the last shown
    /// one on a measure column), so the rest collapse into their totals. The
    /// first field always stays.
    pub(crate) fn collapse_field(&mut self, cx: &mut Cx<Self>) {
        let shown = match self.outline_query() {
            Ok(config) => config.shown_group_by().len(),
            Err(note) => return self.outline_note(note, cx),
        };
        let field = self.active_outline_row().ok().and_then(|active| active.field).unwrap_or(shown - 1);
        let depth = field.max(1);
        if depth >= shown {
            return self.outline_note(TOP_LEVEL, cx);
        }
        if self.edit_query(|config| config.depth.replace(depth) != Some(depth), cx) {
            let names = if depth == 1 { "field" } else { "fields" };
            self.outline_note(format!("Grouped by the first {depth} {names}; Alt+Shift+Right expands the next"), cx);
        }
    }

    /// Shows the next collapsed group-by field again, or at the lowest
    /// level, lists the columns to expand every item into.
    pub(crate) fn expand_field(&mut self, cx: &mut Cx<Self>) {
        let hidden = match self.outline_query() {
            Ok(config) => config.shown_group_by().len() < config.group_by.len(),
            Err(note) => return self.outline_note(note, cx),
        };
        if hidden {
            self.edit_query(|config| {
                show_next_field(config);
                true
            }, cx);
        } else {
            self.open_expand_into(None, cx);
        }
    }

    /// Lists the source columns not grouped or pivoted yet, to group by next:
    /// for every item, or with `item`, opening only that one.
    fn open_expand_into(&mut self, item: Option<OutlinePath>, cx: &mut Cx<Self>) {
        let Some(config) = self.query_config() else {
            return;
        };
        let used = config.group_by.iter().chain(&config.pivot_by).collect::<Vec<_>>();
        let columns = self
            .session
            .source
            .schema
            .iter()
            .map(|column| column.name.clone())
            .filter(|name| !used.iter().any(|dimension| **dimension == Dimension::Column(name.clone())))
            .collect::<Vec<_>>();
        if columns.is_empty() {
            return self.outline_note("Every column is grouped or pivoted already", cx);
        }
        let items = columns.iter().map(Item::new).collect();
        let title = match &item {
            Some(path) => format!(
                "Expand {} into",
                path.last().cloned().flatten().unwrap_or_else(|| "NULL".into())
            ),
            None => "Expand every item into".into(),
        };
        self.palette(&title, "Find a column to group by next", items, super::dialogs::PaletteKind::ExpandInto(columns, item), cx);
    }

    /// Groups by `column` after the last field, opening only `item` when
    /// given, or every item.
    pub(crate) fn expand_into(&mut self, column: String, item: Option<OutlinePath>, cx: &mut Cx<Self>) {
        self.edit_query(|config| {
            let fields = config.group_by.len();
            config.group_by.push(Dimension::Column(column));
            config.depth = None;
            if let Some(path) = &item {
                expand_only(config, path, fields);
            }
            true
        }, cx);
    }

    pub(crate) fn open_outline_layouts(&mut self, cx: &mut Cx<Self>) {
        let current = match self.outline_query() {
            Ok(config) => config.outline.as_ref().map(|outline| outline.layout),
            Err(note) => return self.outline_note(note, cx),
        };
        let items = OutlineLayout::ALL
            .iter()
            .map(|layout| {
                let mut item = Item::new(layout.label()).detail(match layout {
                    OutlineLayout::Compact => "One indented column of row labels",
                    OutlineLayout::Outline => "A row above each expanded item, its children's keys indented by column",
                    OutlineLayout::Tabular => "Each field in its own column, keys on every row",
                });
                item.check = Some(current == Some(*layout));
                item
            })
            .collect();
        self.palette("Outline layout", "Choose a layout", items, super::dialogs::PaletteKind::OutlineLayouts, cx);
    }

    pub(crate) fn set_outline_layout(&mut self, layout: OutlineLayout, cx: &mut Cx<Self>) {
        if let Err(note) = self.outline_query() {
            return self.outline_note(note, cx);
        }
        self.edit_query(|config| {
            let outline = config.outline.get_or_insert_with(Default::default);
            std::mem::replace(&mut outline.layout, layout) != layout
        }, cx);
    }

    /// Shows or hides the subtotals of the cursor's field: a subtotal row
    /// above each expanded item in the tabular layout, or the values on the
    /// item's row in the others.
    pub(crate) fn toggle_subtotals(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_query() {
            Ok(config) => config.shown_group_by().len(),
            Err(note) => return self.outline_note(note, cx),
        };
        let level = match self.active_level() {
            Ok(level) => level,
            Err(error) => return self.outline_note(error, cx),
        };
        if level >= fields {
            return self.outline_note("The last field's rows are its groups; pick a field before it", cx);
        }
        let mut shown = false;
        self.edit_query(|config| {
            let outline = config.outline.get_or_insert_with(Default::default);
            match outline.hidden_subtotals.iter().position(|hidden| *hidden == level) {
                Some(index) => {
                    outline.hidden_subtotals.remove(index);
                    shown = true;
                }
                None => {
                    outline.hidden_subtotals.push(level);
                    outline.hidden_subtotals.sort_unstable();
                }
            }
            true
        }, cx);
        self.outline_note(format!("Subtotals {} for field {level}", if shown { "shown" } else { "hidden" }), cx);
    }

    /// Collapses every item at the cursor's level, leaving the levels above
    /// expanded: Excel's "expand to level". Items expanded by hand go back.
    pub(crate) fn expand_to_level(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_query() {
            Ok(config) => config.shown_group_by().len(),
            Err(note) => return self.outline_note(note, cx),
        };
        let level = match self.active_level() {
            Ok(level) => level,
            Err(error) => return self.outline_note(error, cx),
        };
        self.edit_query(|config| {
            let outline = config.outline.get_or_insert_with(Default::default);
            outline.level = (level < fields).then_some(level);
            outline.toggled.clear();
            true
        }, cx);
    }

    pub(crate) fn expand_all(&mut self, cx: &mut Cx<Self>) {
        if let Err(note) = self.outline_query() {
            return self.outline_note(note, cx);
        }
        let changed = self.edit_query(|config| match &mut config.outline {
            Some(outline) if outline.level.is_some() || !outline.toggled.is_empty() => {
                outline.level = None;
                outline.toggled.clear();
                true
            }
            _ => false,
        }, cx);
        if !changed {
            self.outline_note("Every item is expanded", cx);
        }
    }
}

/// Shows one more of the group-by fields.
fn show_next_field(config: &mut QueryConfig) {
    let next = config.shown_group_by().len() + 1;
    config.depth = (next < config.group_by.len()).then_some(next);
}

/// Opens the item at `path`, a group of the first `fields` fields, into the
/// field after them, leaving the other items at that level collapsed.
fn expand_only(config: &mut QueryConfig, path: &[Option<String>], fields: usize) {
    let outline = config.outline.get_or_insert_with(Default::default);
    if outline.level.is_none_or(|level| level > fields) {
        outline.level = Some(fields);
        outline.toggled.retain(|toggled| toggled.len() < fields);
    }
    outline.set_collapsed(path, false);
}
