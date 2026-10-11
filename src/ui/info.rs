use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::infrastructure::duckdb_store::ColumnSummary;

pub const TOP_VALUE_COUNT: usize = 5;

/// Share of the window the Information dialog fills.
pub const DIALOG_FRACTION: f32 = 0.9;

struct ColumnInfo {
    name: String,
    dtype: String,
    summary: Option<ColumnSummary>,
    top: Option<Vec<(Option<String>, usize)>>,
}

/// The profile table's columns: key, header, width, and right alignment.
const PROFILE_COLUMNS: [(&str, &str, f32, bool); 8] = [
    ("column", "Column", 180.0, false),
    ("type", "Type", 110.0, false),
    ("nulls", "Nulls", 90.0, true),
    ("null_percent", "Null %", 80.0, true),
    ("unique", "Unique", 90.0, true),
    ("min", "Min", 160.0, false),
    ("max", "Max", 160.0, false),
    ("top", "Top values", 520.0, false),
];

struct ProfileRows {
    columns: Vec<ColumnInfo>,
    busy: bool,
}

/// A value as slicers label it, so blanks and empty strings stay visible.
fn value_label(value: Option<&str>) -> String {
    match value {
        None => "(Blank)".into(),
        Some("") => "(Empty)".into(),
        Some(text) => text.chars().take(60).collect(),
    }
}

fn top_text(values: &[(Option<String>, usize)]) -> String {
    if values.is_empty() {
        return "No rows".into();
    }
    values
        .iter()
        .map(|(value, count)| format!("{} ({count})", value_label(value.as_deref())))
        .collect::<Vec<_>>()
        .join(" · ")
}

impl ProfileRows {
    fn text(&self, row: usize, column: usize) -> String {
        let Some(info) = self.columns.get(row) else {
            return String::new();
        };
        let pending = || if self.busy { "Calculating…" } else { "—" }.to_owned();
        let summary = info.summary.as_ref();
        match column {
            0 => info.name.clone(),
            1 => info.dtype.clone(),
            2 => summary.map_or_else(pending, |summary| summary.nulls.to_string()),
            3 => summary.map_or_else(pending, |summary| {
                let percent = if summary.rows == 0 {
                    0.0
                } else {
                    summary.nulls as f64 * 100.0 / summary.rows as f64
                };
                format!("{percent:.1}%")
            }),
            4 => summary.map_or_else(pending, |summary| summary.unique.to_string()),
            5 | 6 => summary.map_or_else(pending, |summary| {
                let value = if column == 5 {
                    &summary.min
                } else {
                    &summary.max
                };
                value
                    .as_deref()
                    .map(|value| value_label(Some(value)))
                    .unwrap_or_default()
            }),
            _ => match &info.top {
                Some(values) => top_text(values),
                None if info.summary.is_some() || self.busy => "Calculating…".into(),
                None => "—".into(),
            },
        }
    }
}

impl TableDelegate for ProfileRows {
    fn columns_count(&self, _: &App) -> usize {
        PROFILE_COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width, right) = PROFILE_COLUMNS[col_ix];
        let column = Column::new(key, name).width(px(width));
        let column = if right { column.text_right() } else { column };
        if col_ix == 0 {
            column.fixed_left()
        } else {
            column
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let text = self.text(row_ix, col_ix);
        div()
            .w_full()
            .truncate()
            .when(PROFILE_COLUMNS[col_ix].3, |cell| cell.text_right())
            .child(text)
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.text(row_ix, col_ix)
    }
}

pub struct InformationPanel {
    rows: usize,
    columns: usize,
    /// What the column profiles cover, shown above them.
    scope: String,
    table: Entity<TableState<ProfileRows>>,
    error: Option<String>,
    sql: String,
    pub(crate) editor: Entity<EditorState>,
}

impl InformationPanel {
    pub fn new(
        schema: Vec<(String, String)>,
        rows: usize,
        scope: String,
        sql: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let columns = schema
            .into_iter()
            .map(|(name, dtype)| ColumnInfo {
                name,
                dtype: crate::domain::formatting::type_label(&dtype).into_owned(),
                summary: None,
                top: None,
            })
            .collect::<Vec<_>>();
        let columns_len = columns.len();
        let table = cx.new(|cx| {
            TableState::new(
                ProfileRows {
                    columns,
                    busy: true,
                },
                window,
                cx,
            )
            .col_movable(false)
            .sortable(false)
        });
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
                .soft_wrap(false)
                .default_value(sql.clone())
        });
        Self {
            rows,
            columns: columns_len,
            scope,
            table,
            error: None,
            sql,
            editor,
        }
    }

    pub fn finish(
        &mut self,
        summaries: Result<Vec<ColumnSummary>, String>,
        cx: &mut Context<Self>,
    ) {
        match summaries {
            Ok(summaries) => self.table.update(cx, |table, cx| {
                let rows = table.delegate_mut();
                rows.busy = false;
                for column in &mut rows.columns {
                    column.summary = summaries
                        .iter()
                        .find(|summary| summary.name == column.name)
                        .cloned();
                }
                cx.notify();
            }),
            Err(error) => {
                self.table.update(cx, |table, cx| {
                    table.delegate_mut().busy = false;
                    cx.notify();
                });
                self.error = Some(error);
            }
        }
        cx.notify();
    }

    /// Publishes one column's most frequent values.
    pub fn finish_top(
        &mut self,
        name: &str,
        values: Result<Vec<(Option<String>, usize)>, String>,
        cx: &mut Context<Self>,
    ) {
        match values {
            Ok(values) => self.table.update(cx, |table, cx| {
                if let Some(column) = table
                    .delegate_mut()
                    .columns
                    .iter_mut()
                    .find(|column| column.name == name)
                {
                    column.top = Some(values);
                }
                cx.notify();
            }),
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// One profile row's cells as the table shows them.
    #[cfg(test)]
    pub(crate) fn row_text(&self, name: &str, cx: &App) -> Option<Vec<String>> {
        let rows = self.table.read(cx).delegate();
        let row = rows.columns.iter().position(|column| column.name == name)?;
        Some(
            (0..PROFILE_COLUMNS.len())
                .map(|column| rows.text(row, column))
                .collect(),
        )
    }
}

impl Render for InformationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The dialog's title and padding take about 110px of its height.
        let height = (window.bounds().size.height.as_f32() * DIALOG_FRACTION - 110.0).max(240.0);
        // Two summary lines, the SQL heading, and the gaps between them.
        let fixed = 116.0;
        let flexible = (height - fixed).max(128.0);
        let table_height = (self.columns.max(1) as f32 * 36.0 + 48.0).clamp(96.0, flexible * 0.65);
        let editor_height = (flexible - table_height).max(64.0);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(format!(
                "{} {} × {} {}",
                self.rows,
                if self.rows == 1 { "row" } else { "rows" },
                self.columns,
                if self.columns == 1 {
                    "column"
                } else {
                    "columns"
                }
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.scope.clone()),
            )
            .child(
                div().h(px(table_height)).child(
                    DataTable::new(&self.table)
                        .stripe(true)
                        .scrollbar_visible(true, true),
                ),
            )
            .children(self.error.clone().map(|error| div().child(error)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("SQL · Read-only")
                    .child(
                        Button::new("information-copy-sql")
                            .label("Copy SQL")
                            .on_click(cx.listener(|panel, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(panel.sql.clone()));
                            })),
                    ),
            )
            .child(
                Editor::new(&self.editor)
                    .readonly(true)
                    .h(px(editor_height))
                    .aria_label("View SQL"),
            )
    }
}
