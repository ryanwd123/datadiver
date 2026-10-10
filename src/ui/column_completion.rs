use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{Enter, IndentInline, InputEvent, InputState, MoveDown, MoveUp};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::domain::query::{
    ColumnNameQuery, column_name_query, rank_columns, sql_column_name_query,
};
use crate::infrastructure::duckdb_store::quote_identifier;

const VISIBLE_ITEMS: usize = 10;

/// How a single-line input reads and writes column names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnSyntax {
    /// The search language: a `"` at a word start, after `-`, or after a
    /// comparison operator opens a name, inserted as `"Column Name"`.
    Search,
    /// SQL: any `"` outside a string opens an identifier, inserted escaped.
    Sql,
    /// `@` opens a table name for the Ask AI prompt; the candidates are
    /// already SQL names and are inserted as `@name `.
    Mention,
}

pub enum ColumnCompletionEvent {
    /// A column name replaced the typed text.
    Accepted,
}

struct OpenList {
    range: std::ops::Range<usize>,
    items: Vec<String>,
    selected: usize,
}

/// Column names offered under a single-line input while a `"` is open.
/// Wrap the element holding the input with [`with_column_completion`].
pub struct ColumnCompletion {
    input: Entity<InputState>,
    syntax: ColumnSyntax,
    /// Candidate names in preference order, such as shown columns first.
    columns: Vec<String>,
    open: Option<OpenList>,
    _subscription: Subscription,
}

impl EventEmitter<ColumnCompletionEvent> for ColumnCompletion {}

impl ColumnCompletion {
    pub fn new(input: Entity<InputState>, syntax: ColumnSyntax, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => this.refresh(cx),
            InputEvent::Blur => {
                this.close(cx);
            }
            _ => {}
        });
        Self {
            input,
            syntax,
            columns: Vec::new(),
            open: None,
            _subscription: subscription,
        }
    }

    pub fn set_columns(&mut self, columns: Vec<String>) {
        self.columns = columns;
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    #[cfg(all(test, feature = "snapshots"))]
    pub fn items(&self) -> Option<Vec<String>> {
        self.open.as_ref().map(|open| open.items.clone())
    }

    fn query(&self, text: &str, cursor: usize) -> Option<ColumnNameQuery> {
        match self.syntax {
            ColumnSyntax::Search => column_name_query(text, cursor),
            ColumnSyntax::Sql => sql_column_name_query(text, cursor),
            ColumnSyntax::Mention => crate::domain::ai::mention_query(text, cursor),
        }
    }

    /// Opens, filters, or closes the list for the input's text and cursor.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        let open = self
            .query(&input.value(), input.cursor())
            .and_then(|query| {
                let names = self.columns.iter().map(String::as_str).collect::<Vec<_>>();
                let items = rank_columns(&names, &query.typed)
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                (!items.is_empty()).then_some(OpenList {
                    range: query.range,
                    items,
                    selected: 0,
                })
            });
        if open.is_some() || self.open.is_some() {
            self.open = open;
            cx.notify();
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.open.take().is_some();
        if open {
            cx.notify();
        }
        open
    }

    fn select(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        let count = open.items.len() as isize;
        open.selected = (open.selected as isize + delta).rem_euclid(count) as usize;
        cx.notify();
        true
    }

    /// Replaces the typed name, quotes included, with the chosen column.
    pub fn accept(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(open) = self.open.take() else {
            return false;
        };
        let Some(name) = open.items.get(index.unwrap_or(open.selected)) else {
            return false;
        };
        let inserted = match self.syntax {
            ColumnSyntax::Search => format!("\"{name}\""),
            ColumnSyntax::Sql => quote_identifier(name),
            ColumnSyntax::Mention => format!("@{name} "),
        };
        self.input.update(cx, |input, cx| {
            input.set_selected_range(open.range.clone(), cx);
            input.replace(inserted, window, cx);
            input.focus(window, cx);
        });
        cx.emit(ColumnCompletionEvent::Accepted);
        cx.notify();
        true
    }

    fn on_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        handled(self.select(-1, cx), cx);
    }

    fn on_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        handled(self.select(1, cx), cx);
    }

    fn on_tab(&mut self, _: &IndentInline, window: &mut Window, cx: &mut Context<Self>) {
        handled(self.accept(None, window, cx), cx);
    }

    fn on_enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        handled(self.accept(None, window, cx), cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" && self.close(cx) {
            cx.stop_propagation();
        }
    }

    fn popup(&self, entity: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        let open = self.open.as_ref()?;
        // Placed under the text cursor, as the editors place their menus, with
        // extra room for the padding and border below a single-line input's text.
        let input = self.input.read(cx);
        let (cursor, line_height) = input.cursor_layout()?;
        let origin = cursor.origin + input.scroll_offset() + point(px(-4.), line_height + px(12.));
        let first = open
            .selected
            .saturating_sub(VISIBLE_ITEMS - 1)
            .min(open.items.len().saturating_sub(VISIBLE_ITEMS));
        let theme = cx.theme();
        let noun = if self.syntax == ColumnSyntax::Mention {
            "tables"
        } else {
            "columns"
        };
        let list = div()
            .id("column-completion")
            .w(px(320.))
            .py_1()
            .flex()
            .flex_col()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .children(
                open.items
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(VISIBLE_ITEMS)
                    .map(|(index, name)| {
                        let entity = entity.clone();
                        div()
                            .id(("column-completion-item", index))
                            .px_2()
                            .py(px(2.))
                            .truncate()
                            .when(index == open.selected, |item| {
                                item.bg(theme.accent).text_color(theme.accent_foreground)
                            })
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                entity.update(cx, |this, cx| this.accept(Some(index), window, cx));
                                cx.stop_propagation();
                            })
                            .child(name.clone())
                    }),
            )
            .when(open.items.len() > VISIBLE_ITEMS, |list| {
                list.child(
                    div()
                        .px_2()
                        .text_xs()
                        .opacity(0.6)
                        .child(format!("{} {noun}", open.items.len())),
                )
            });
        Some(
            deferred(
                anchored()
                    .position(origin)
                    .snap_to_window_with_margin(px(8.))
                    .child(list),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// Keys the list handles stop there; otherwise they reach the input as usual.
fn handled(handled: bool, cx: &mut App) {
    if handled {
        cx.stop_propagation();
    } else {
        cx.propagate();
    }
}

/// Adds the list's keys to `element`, which holds the input, and floats the
/// list below it: Up and Down move, Enter or Tab inserts, Escape closes.
pub fn with_column_completion(
    element: Div,
    completion: &Entity<ColumnCompletion>,
    window: &mut Window,
    cx: &App,
) -> Div {
    element
        .capture_action(window.listener_for(completion, ColumnCompletion::on_up))
        .capture_action(window.listener_for(completion, ColumnCompletion::on_down))
        .capture_action(window.listener_for(completion, ColumnCompletion::on_tab))
        .capture_action(window.listener_for(completion, ColumnCompletion::on_enter))
        .capture_key_down(window.listener_for(completion, ColumnCompletion::on_key_down))
        .children(completion.read(cx).popup(completion, cx))
}
