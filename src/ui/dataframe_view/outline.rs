//! The outline commands: collapse and expand a query's group-by fields
//! (Ctrl+Alt+Left/Right) and items (Alt+Left/Right), choose its layout, and
//! show subtotals.

use super::*;
use crate::domain::documents::{OutlineLayout, QueryConfig};
use crate::domain::frame::OutlineRow;
use crate::ui::widgets::list::Item;

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

    /// How many group-by fields the result shows, or a note on why the
    /// outline commands don't apply.
    fn outline_fields(&self) -> Result<usize, &'static str> {
        let config = self.query_config().ok_or("Outlines apply to query results: group by a field first")?;
        match config.shown_group_by().len() {
            0 | 1 => Err("An outline needs two shown group-by fields: the items of one collapse into the one before"),
            fields => Ok(fields),
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
                // In a flat result every row is a group of every field.
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
        let fields = match self.outline_fields() {
            Ok(fields) => fields,
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
            return self.outline_note("This item is collapsed already; Alt+Right expands it", cx);
        }
        let path = active.row.path[..target].to_vec();
        self.edit_query(|config| config.outline.get_or_insert_with(Default::default).set_collapsed(&path, true), cx);
    }

    /// Expands the collapsed item under the cursor.
    pub(crate) fn expand_item(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_fields() {
            Ok(fields) => fields,
            Err(note) => return self.outline_note(note, cx),
        };
        let active = match self.active_outline_row() {
            Ok(active) => active,
            Err(error) => return self.outline_note(error, cx),
        };
        if active.row.open || active.row.level >= fields {
            return self.outline_note("This item is expanded already; Alt+Left collapses it", cx);
        }
        let path = active.row.path;
        self.edit_query(|config| config.outline.get_or_insert_with(Default::default).set_collapsed(&path, false), cx);
    }

    /// Groups by only the fields before the cursor's field (the last shown
    /// one on a measure column), so the rest collapse into their totals.
    pub(crate) fn collapse_field(&mut self, cx: &mut Cx<Self>) {
        let Some(config) = self.query_config() else {
            return self.outline_note("Collapsing fields applies to query results", cx);
        };
        let shown = config.shown_group_by().len();
        if shown == 0 {
            return self.outline_note("Group by a field first (Ctrl+G)", cx);
        }
        let field = self.active_outline_row().ok().and_then(|active| active.field).unwrap_or(shown - 1);
        // A pivot needs a group by, and a query without aggregations shows only its groups.
        let fewest = usize::from(!config.pivot_by.is_empty() || config.aggregations.is_empty());
        let depth = field.max(fewest);
        if depth >= shown {
            return self.outline_note("The first field stays: without it the query would show nothing", cx);
        }
        if self.edit_query(|config| config.depth.replace(depth) != Some(depth), cx) {
            let names = |count: usize| if count == 1 { "field" } else { "fields" };
            self.outline_note(
                format!("Grouped by the first {depth} {}; Ctrl+Alt+Right expands the next", names(depth)),
                cx,
            );
        }
    }

    /// Shows the next collapsed group-by field again.
    pub(crate) fn expand_field(&mut self, cx: &mut Cx<Self>) {
        let Some(config) = self.query_config() else {
            return self.outline_note("Expanding fields applies to query results", cx);
        };
        let fields = config.group_by.len();
        let Some(depth) = config.depth.filter(|depth| *depth < fields) else {
            return self.outline_note("Every group-by field is shown", cx);
        };
        self.edit_query(|config| {
            config.depth = (depth + 1 < fields).then_some(depth + 1);
            true
        }, cx);
    }

    pub(crate) fn open_outline_layouts(&mut self, cx: &mut Cx<Self>) {
        if let Err(note) = self.outline_fields() {
            return self.outline_note(note, cx);
        }
        let current = self
            .query_config()
            .and_then(|config| config.outline.as_ref())
            .map(|outline| outline.layout);
        let items = OutlineLayout::ALL
            .iter()
            .map(|layout| {
                let mut item = Item::new(layout.label()).detail(match layout {
                    OutlineLayout::Tabular => "Each field in its own column, keys on every row",
                    OutlineLayout::Outline => "A row above each expanded item, its children's keys indented by column",
                    OutlineLayout::Compact => "One indented column of row labels",
                });
                item.check = Some(current == Some(*layout));
                item
            })
            .collect();
        self.palette("Outline layout", "Choose a layout", items, super::dialogs::PaletteKind::OutlineLayouts, cx);
    }

    /// Shows the query as an outline in `layout`. Layouts with a row above
    /// each expanded item start with every level's subtotals there.
    pub(crate) fn set_outline_layout(&mut self, layout: OutlineLayout, cx: &mut Cx<Self>) {
        let fields = match self.outline_fields() {
            Ok(fields) => fields,
            Err(note) => return self.outline_note(note, cx),
        };
        self.edit_query(|config| {
            let created = config.outline.is_none();
            let outline = config.outline.get_or_insert_with(Default::default);
            let changed = created || outline.layout != layout;
            outline.layout = layout;
            if layout != OutlineLayout::Tabular && outline.subtotals.is_empty() {
                outline.subtotals = (1..fields).collect();
            }
            changed
        }, cx);
    }

    /// Shows or hides the subtotals of the cursor's field: a subtotal row
    /// above each expanded item in the tabular layout, or the values on the
    /// item's row in the others.
    pub(crate) fn toggle_subtotals(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_fields() {
            Ok(fields) => fields,
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
            match outline.subtotals.iter().position(|subtotal| *subtotal == level) {
                Some(index) => {
                    outline.subtotals.remove(index);
                }
                None => {
                    outline.subtotals.push(level);
                    outline.subtotals.sort_unstable();
                    shown = true;
                }
            }
            true
        }, cx);
        self.outline_note(format!("Subtotals {} for field {level}", if shown { "shown" } else { "hidden" }), cx);
    }

    /// Collapses every item at the cursor's level, leaving the levels above
    /// expanded: Excel's "expand to level". Items expanded by hand go back.
    pub(crate) fn expand_to_level(&mut self, cx: &mut Cx<Self>) {
        let fields = match self.outline_fields() {
            Ok(fields) => fields,
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

    /// Turns the outline off; collapsed fields stay collapsed.
    pub(crate) fn flat_result(&mut self, cx: &mut Cx<Self>) {
        if !self.edit_query(|config| config.outline.take().is_some(), cx) {
            self.outline_note("The result is flat already", cx);
        }
    }
}
