use crate::infrastructure::duckdb_store::ColumnSummary;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Color, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors, scroll};

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

pub struct InformationPanel {
    rows: usize,
    columns_count: usize,
    /// What the column profiles cover, shown above them.
    scope: String,
    columns: Vec<ColumnInfo>,
    busy: bool,
    error: Option<String>,
    pub(crate) sql: String,
    pub(crate) editor: Editor,
    table_scroll: scroll::Scroll,
    table_scroll_x: scroll::Scroll,
    table_area: Rect,
}

impl InformationPanel {
    pub fn new(schema: Vec<(String, String)>, rows: usize, scope: String, sql: String) -> Self {
        let columns = schema
            .into_iter()
            .map(|(name, dtype)| ColumnInfo {
                name,
                dtype: crate::domain::formatting::type_label(&dtype).into_owned(),
                summary: None,
                top: None,
            })
            .collect::<Vec<_>>();
        let mut editor = Editor::sql();
        editor.read_only = true;
        editor.auto_pairs = false;
        editor.framed = true;
        editor.set_text(sql.clone());
        editor.set_selection(0..0);
        Self {
            rows,
            columns_count: columns.len(),
            scope,
            columns,
            busy: true,
            error: None,
            sql,
            editor,
            table_scroll: scroll::Scroll::default(),
            table_scroll_x: scroll::Scroll::default(),
            table_area: Rect::default(),
        }
    }

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
                let value = if column == 5 { &summary.min } else { &summary.max };
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

    pub fn finish(&mut self, summaries: Result<Vec<ColumnSummary>, String>) {
        self.busy = false;
        match summaries {
            Ok(summaries) => {
                for column in &mut self.columns {
                    column.summary = summaries
                        .iter()
                        .find(|summary| summary.name == column.name)
                        .cloned();
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Publishes one column's most frequent values.
    pub fn finish_top(&mut self, name: &str, values: Result<Vec<(Option<String>, usize)>, String>) {
        match values {
            Ok(values) => {
                if let Some(column) = self.columns.iter_mut().find(|column| column.name == name) {
                    column.top = Some(values);
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// One profile row's cells as the table shows them.
    #[cfg(test)]
    pub(crate) fn row_text(&self, name: &str) -> Option<Vec<String>> {
        let row = self.columns.iter().position(|column| column.name == name)?;
        Some(
            (0..PROFILE_COLUMNS.len())
                .map(|column| self.text(row, column))
                .collect(),
        )
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, shift: bool, line: f32) -> bool {
        if matches!(hit, Hit::Editor(id, _) if *id == self.editor.id) {
            return self.editor.wheel(delta);
        }
        let row_height = line + 12.0;
        let content = self.columns.len() as f32 * row_height;
        let width: f32 = PROFILE_COLUMNS.iter().map(|column| column.2).sum();
        if shift || delta.x != 0.0 {
            let amount = if shift { delta.y } else { delta.x };
            return self
                .table_scroll_x
                .scroll_by(-amount * line * 3.0, self.table_area.w, width);
        }
        self.table_scroll
            .scroll_by(-delta.y * row_height, self.table_area.h - row_height, content)
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Button(Button::CopySql) => {
                crate::ui::platform::system::write_clipboard(&self.sql);
                true
            }
            Hit::Editor(id, part) if *id == self.editor.id => {
                self.editor.mouse_down(*part, pos, clicks, shift);
                *focus = Some(*id);
                true
            }
            _ => false,
        }
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let mut column = Column::new(r, 8.0);
        canvas.text(
            column.next(line),
            &format!(
                "{} {} × {} {}",
                self.rows,
                if self.rows == 1 { "row" } else { "rows" },
                self.columns_count,
                if self.columns_count == 1 { "column" } else { "columns" }
            ),
            font,
            colors::FOREGROUND,
            Align::Left,
        );
        canvas.text(column.next(line), &self.scope, &fonts.small, colors::MUTED, Align::Left);
        // Two summary lines, the SQL heading, and the gaps between them.
        let fixed = line * 2.0 + button_height(font) + 40.0 + if self.error.is_some() { line + 8.0 } else { 0.0 };
        let flexible = (r.h - fixed).max(128.0);
        let row_height = line + 12.0;
        let table_height = (self.columns.len().max(1) as f32 * row_height + row_height + scroll::BAR_WIDTH)
            .clamp(96.0, flexible * 0.65);
        let table = column.next(table_height);
        self.paint_table(canvas, table, fonts, row_height);
        if let Some(error) = &self.error {
            canvas.text(column.next(line), error, font, colors::DANGER, Align::Left);
        }
        let header = column.next(button_height(font));
        let mut header = Row::new(header, 8.0);
        let copy = header.right(button_width("Copy SQL", font));
        button(canvas, copy, "Copy SQL", font, ButtonStyle::Outline, true, Hit::Button(Button::CopySql));
        canvas.text(header.rest(), "SQL · Read-only", font, colors::FOREGROUND, Align::Left);
        let editor = column.rest();
        let id = self.editor.id;
        self.editor
            .paint(canvas, editor, &fonts.mono, focus == Some(id), |part| Hit::Editor(id, part));
    }

    fn paint_table(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, row_height: f32) {
        let font = &fonts.ui;
        self.table_area = r;
        canvas.stroke(r, colors::BORDER, 1.0);
        let inner = r.inset(1.0);
        let total_width: f32 = PROFILE_COLUMNS.iter().map(|column| column.2).sum();
        let body_height = inner.h - row_height - scroll::BAR_WIDTH;
        let content = self.columns.len() as f32 * row_height;
        self.table_scroll.clamp(body_height, content);
        self.table_scroll_x.clamp(inner.w, total_width);
        let fixed_width = PROFILE_COLUMNS[0].2;
        canvas.push_clip(inner);
        let draw_row = |canvas: &mut Canvas<Hit>, y: f32, cells: &dyn Fn(usize) -> String, background: Option<Color>, bold: bool| {
            let row = rect(inner.x, y, inner.w, row_height);
            if let Some(background) = background {
                canvas.fill(row, background);
            }
            let font = if bold { font.semibold() } else { font.clone() };
            let mut x = inner.x + fixed_width - self.table_scroll_x.offset;
            canvas.push_clip(rect(inner.x + fixed_width, y, inner.w - fixed_width, row_height));
            for (index, (_, _, width, right)) in PROFILE_COLUMNS.iter().enumerate().skip(1) {
                let cell = rect(x + 8.0, y, width - 16.0, row_height);
                canvas.text(cell, &cells(index), &font, colors::FOREGROUND, if *right { Align::Right } else { Align::Left });
                x += width;
            }
            canvas.pop_clip();
            canvas.text(rect(inner.x + 8.0, y, fixed_width - 16.0, row_height), &cells(0), &font, colors::FOREGROUND, Align::Left);
            canvas.fill(rect(inner.x + fixed_width - 1.0, y, 1.0, row_height), colors::BORDER);
        };
        let header_y = inner.y;
        draw_row(canvas, header_y, &|index| PROFILE_COLUMNS[index].1.to_owned(), Some(colors::STRIPE), true);
        canvas.push_clip(rect(inner.x, inner.y + row_height, inner.w, body_height));
        let first = (self.table_scroll.offset / row_height).floor() as usize;
        let count = (body_height / row_height).ceil() as usize + 1;
        for row in first..(first + count).min(self.columns.len()) {
            let y = inner.y + row_height + row as f32 * row_height - self.table_scroll.offset;
            draw_row(
                canvas,
                y,
                &|index| self.text(row, index),
                (row % 2 == 1).then_some(colors::STRIPE),
                false,
            );
        }
        canvas.pop_clip();
        canvas.pop_clip();
        canvas.hit(r, Hit::DialogScroll);
        let vertical = rect(inner.right() - scroll::BAR_WIDTH, inner.y + row_height, scroll::BAR_WIDTH, body_height);
        scroll::vertical(canvas, vertical, body_height, content, &self.table_scroll, Hit::DialogScroll);
        let horizontal = rect(inner.x, inner.bottom() - scroll::BAR_WIDTH, inner.w, scroll::BAR_WIDTH);
        scroll::horizontal(canvas, horizontal, inner.w, total_width, &self.table_scroll_x, Hit::DialogScroll);
    }
}
