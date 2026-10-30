//! Column names offered under a single-line input while a `"` is open, and
//! table names after `@` in the Ask AI prompt.

use diver_sql::Dialect;

use crate::domain::expression_completion::{self, ExpressionCatalog};
use crate::domain::query::{ColumnNameQuery, column_name_query, rank_columns};
use crate::ui::hit::Hit;
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Font, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{WidgetId, colors};

const VISIBLE_ITEMS: usize = 10;

/// How a single-line input reads and writes column names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnSyntax {
    /// The search language: a `"` at a word start, after `-`, or after a
    /// comparison operator opens a name, inserted as `"Column Name"`.
    Search,
    /// A SQL expression: a `"` where `diver_sql` expects a column opens it,
    /// inserted escaped.
    Sql,
    /// `@` opens a table name for the Ask AI prompt; the candidates are
    /// already SQL names and are inserted as `@name `.
    Mention,
}

struct OpenList {
    range: std::ops::Range<usize>,
    /// What each row shows, and what accepting it inserts.
    items: Vec<(String, String)>,
    selected: usize,
}

pub struct ColumnCompletion {
    syntax: ColumnSyntax,
    /// Candidate names in preference order, such as shown columns first.
    columns: Vec<String>,
    /// The SQL dialect of `Sql` inputs.
    dialect: Dialect,
    open: Option<OpenList>,
}

impl ColumnCompletion {
    pub fn new(syntax: ColumnSyntax) -> Self {
        Self {
            syntax,
            columns: Vec::new(),
            dialect: Dialect::DuckDb,
            open: None,
        }
    }

    pub fn set_columns(&mut self, columns: Vec<String>) {
        self.columns = columns;
    }

    pub fn set_dialect(&mut self, dialect: Dialect) {
        self.dialect = dialect;
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    #[cfg(test)]
    pub fn items(&self) -> Option<Vec<String>> {
        self.open
            .as_ref()
            .map(|open| open.items.iter().map(|(label, _)| label.clone()).collect())
    }

    fn query(&self, text: &str, cursor: usize) -> Option<ColumnNameQuery> {
        match self.syntax {
            ColumnSyntax::Search => column_name_query(text, cursor),
            ColumnSyntax::Mention => crate::domain::ai::mention_query(text, cursor),
            ColumnSyntax::Sql => None,
        }
    }

    /// Opens, filters, or closes the list for the input's text and cursor.
    pub fn refresh(&mut self, input: &Editor) {
        if self.syntax == ColumnSyntax::Sql {
            return self.refresh_sql(input.text(), input.cursor());
        }
        self.open = self.query(input.text(), input.cursor()).and_then(|query| {
            let names = self.columns.iter().map(String::as_str).collect::<Vec<_>>();
            let items = rank_columns(&names, &query.typed)
                .into_iter()
                .map(|name| {
                    let insert = match self.syntax {
                        ColumnSyntax::Mention => format!("@{name} "),
                        _ => format!("\"{name}\""),
                    };
                    (name.to_owned(), insert)
                })
                .collect::<Vec<_>>();
            (!items.is_empty()).then_some(OpenList {
                range: query.range,
                items,
                selected: 0,
            })
        });
    }

    /// A SQL input lists columns only inside a `"`: a bare word's list would
    /// take the Enter that commits the card.
    fn refresh_sql(&mut self, text: &str, cursor: usize) {
        let catalog = ExpressionCatalog::new(self.columns.iter().map(|name| (name.clone(), None)));
        let items = expression_completion::completions(text, cursor, &catalog, self.dialect);
        self.open = items
            .first()
            .filter(|item| text[item.range.clone()].starts_with('"'))
            .map(|first| OpenList {
                range: first.range.clone(),
                items: items.iter().map(|item| (item.label.clone(), item.insert.clone())).collect(),
                selected: 0,
            });
    }

    pub fn close(&mut self) -> bool {
        self.open.take().is_some()
    }

    /// Up and Down move, Enter or Tab inserts, Escape closes. Returns
    /// `(handled, accepted)`.
    pub fn key(&mut self, event: &KeyEvent, input: &mut Editor) -> (bool, bool) {
        let Some(open) = &mut self.open else {
            return (false, false);
        };
        if event.modifiers.control || event.modifiers.alt || event.modifiers.shift {
            return (false, false);
        }
        let count = open.items.len() as isize;
        match event.key.as_str() {
            "up" => {
                open.selected = (open.selected as isize - 1).rem_euclid(count) as usize;
                (true, false)
            }
            "down" => {
                open.selected = (open.selected as isize + 1).rem_euclid(count) as usize;
                (true, false)
            }
            "tab" | "enter" => {
                let accepted = self.accept(None, input);
                (accepted, accepted)
            }
            "escape" => (self.close(), false),
            _ => (false, false),
        }
    }

    /// Replaces the typed name, quotes included, with the chosen column.
    pub fn accept(&mut self, index: Option<usize>, input: &mut Editor) -> bool {
        let Some(open) = self.open.take() else {
            return false;
        };
        let Some((_, inserted)) = open.items.get(index.unwrap_or(open.selected)) else {
            return false;
        };
        input.replace(open.range.clone(), inserted);
        true
    }

    /// Draws the list under the input's caret; `id` is the input's.
    pub fn paint(&self, canvas: &mut Canvas<Hit>, caret: Option<Rect>, font: &Font, id: WidgetId) {
        let (Some(open), Some(caret)) = (&self.open, caret) else {
            return;
        };
        let row_height = (text::line_height(font) + 4.0).round();
        let first = open
            .selected
            .saturating_sub(VISIBLE_ITEMS - 1)
            .min(open.items.len().saturating_sub(VISIBLE_ITEMS));
        let shown = (open.items.len() - first).min(VISIBLE_ITEMS);
        let footer = open.items.len() > VISIBLE_ITEMS;
        let height = shown as f32 * row_height + 8.0 + if footer { row_height } else { 0.0 };
        let width = 320.0;
        let x = (caret.x - 4.0).min(canvas.width - width - 8.0).max(8.0);
        let mut y = caret.bottom() + 10.0;
        if y + height > canvas.height - 8.0 {
            y = (caret.y - height - 4.0).max(8.0);
        }
        let menu = rect(x, y, width, height);
        canvas.shadow(menu, 6.0);
        canvas.fill_rounded(menu, 6.0, colors::POPOVER);
        canvas.stroke_rounded(menu, 6.0, colors::BORDER, 1.0);
        canvas.hit(menu, Hit::Surface);
        for (offset, (name, _)) in open.items.iter().enumerate().skip(first).take(VISIBLE_ITEMS) {
            let row = rect(x + 4.0, y + 4.0 + (offset - first) as f32 * row_height, width - 8.0, row_height);
            if offset == open.selected {
                canvas.fill_rounded(row, 4.0, colors::SELECTED);
            } else if canvas.hovered(row) {
                canvas.fill_rounded(row, 4.0, colors::HOVER);
            }
            canvas.text(row.inset_xy(6.0, 0.0), name, font, colors::FOREGROUND, Align::Left);
            canvas.hit(row, Hit::ColumnCompletion(id, offset));
        }
        if footer {
            let noun = if self.syntax == ColumnSyntax::Mention {
                "tables"
            } else {
                "columns"
            };
            canvas.text(
                rect(x + 10.0, y + 4.0 + shown as f32 * row_height, width - 20.0, row_height),
                &format!("{} {noun}", open.items.len()),
                &font.sized((font.size * 0.8).round()),
                colors::MUTED,
                Align::Left,
            );
        }
    }
}
