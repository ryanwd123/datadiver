use gpui_kit::component::button::Button;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Editor, Enter, InputEvent};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Selectable as _, Sizable as _, StyledExt as _,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::application::pivot_editor::{AggregationEdit, default_aggregation, operation_label};
use crate::domain::aggregation_sql::PIVOT_LIMITATIONS_URL;
use crate::domain::documents::Aggregation;
use crate::domain::pivot::editor_expression;
use crate::infrastructure::duckdb_store::ColumnSchema;
use crate::infrastructure::sql_execution::{AggregateFunction, aggregate_functions};
use crate::ui::input_with_autocompletion::InputWithAutocompletion;

gpui_kit::actions!(
    datadiver,
    [
        ApplyAggregation,
        AggregationNextFocus,
        AggregationPreviousFocus
    ]
);
struct AggregationBindings;
impl Global for AggregationBindings {}

#[derive(Clone, Debug)]
pub struct AggregationPanelEvent {
    pub edit: AggregationEdit,
    pub revision: u64,
}

struct ConfiguredAggregations {
    entries: Vec<Aggregation>,
    selected: Option<IndexPath>,
}

impl ListDelegate for ConfiguredAggregations {
    type Item = ListItem;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.entries.len()
    }
    fn render_item(
        &mut self,
        index: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let entry = self.entries.get(index.row)?;
        let label = entry
            .name
            .as_deref()
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                entry
                    .sql
                    .as_ref()
                    .map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "))
            })
            .unwrap_or_else(|| {
                format!(
                    "{} · {}",
                    entry.source_column,
                    operation_label(&entry.operation)
                )
            });
        Some(
            ListItem::new(format!("aggregation-{}", index.row))
                .h(px(34.))
                .selected(self.selected == Some(index))
                .child(label),
        )
    }
    fn set_selected_index(
        &mut self,
        index: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = index;
        cx.notify();
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().py_3().child("No aggregations configured")
    }
}

#[derive(Clone)]
enum PanelAction {
    Select(usize),
    Add(String),
    Custom,
    Move(isize),
    Remove,
}

pub struct AggregationPanel {
    schema: Vec<ColumnSchema>,
    configured: Vec<Aggregation>,
    command: Entity<CommandState>,
    /// Searchable reference of DuckDB's aggregate functions, shown in place of
    /// the column list.
    functions: Entity<CommandState>,
    /// `None` while the list loads.
    aggregate_functions: Option<Result<Vec<AggregateFunction>, String>>,
    show_functions: bool,
    list: Entity<ListState<ConfiguredAggregations>>,
    pub(crate) sql: Entity<InputWithAutocompletion>,
    selected: Option<usize>,
    custom: bool,
    initial_sql: String,
    observed_sql: String,
    revision: u64,
    checking: bool,
    checking_revision: u64,
    closed: bool,
    pending_action: Option<PanelAction>,
    pub(crate) error: Option<String>,
    pivot: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AggregationPanelEvent> for AggregationPanel {}

impl AggregationPanel {
    pub fn new(
        schema: Vec<ColumnSchema>,
        configured: Vec<Aggregation>,
        pivot: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if !cx.has_global::<AggregationBindings>() {
            cx.bind_keys([
                KeyBinding::new("ctrl-enter", ApplyAggregation, Some("AggregationPanel")),
                KeyBinding::new(
                    "ctrl-enter",
                    ApplyAggregation,
                    Some("AggregationPanel > Input"),
                ),
                KeyBinding::new("tab", AggregationNextFocus, Some("AggregationPanel")),
                KeyBinding::new(
                    "tab",
                    AggregationNextFocus,
                    Some("AggregationPanel > Input"),
                ),
                KeyBinding::new(
                    "shift-tab",
                    AggregationPreviousFocus,
                    Some("AggregationPanel"),
                ),
                KeyBinding::new(
                    "shift-tab",
                    AggregationPreviousFocus,
                    Some("AggregationPanel > Input"),
                ),
            ]);
            cx.set_global(AggregationBindings);
        }
        let command = cx.new(|cx| CommandState::new(window, cx));
        let functions = cx.new(|cx| CommandState::new(window, cx));
        // The detail under the list follows the highlighted function.
        let functions_subscription = cx.observe(&functions, |_, _, cx| cx.notify());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { aggregate_functions() })
                .await;
            let _ = this.update(cx, |panel, cx| {
                panel.aggregate_functions = Some(result);
                cx.notify();
            });
        })
        .detach();
        let list = cx.new(|cx| {
            ListState::new(
                ConfiguredAggregations {
                    entries: configured.clone(),
                    selected: None,
                },
                window,
                cx,
            )
        });
        let sql = cx.new(|cx| {
            InputWithAutocompletion::sql_columns(
                schema.iter().map(|column| column.storage_name.clone()),
                window,
                cx,
            )
        });
        let list_subscription =
            cx.subscribe_in(&list, window, |this, _, event: &ListEvent, window, cx| {
                if let ListEvent::Select(index) | ListEvent::Confirm(index) = event {
                    this.restore_list_selection(window, cx);
                    if this.selected != Some(index.row) || this.custom {
                        this.request_action(PanelAction::Select(index.row), window, cx);
                    }
                }
            });
        let sql_state = sql.read(cx).state().clone();
        let sql_subscription = cx.subscribe(&sql_state, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.input_changed(cx);
            }
        });
        let mut panel = Self {
            schema,
            configured,
            command,
            functions,
            aggregate_functions: None,
            show_functions: false,
            list,
            sql,
            selected: None,
            custom: false,
            initial_sql: String::new(),
            observed_sql: String::new(),
            revision: 0,
            checking: false,
            checking_revision: 0,
            closed: false,
            pending_action: None,
            error: None,
            pivot,
            _subscriptions: vec![list_subscription, sql_subscription, functions_subscription],
        };
        if !panel.configured.is_empty() {
            panel.select(0, window, cx);
        }
        panel
    }

    fn input_changed(&mut self, cx: &mut Context<Self>) {
        let sql = self.sql.read(cx).state().read(cx).value().to_string();
        if sql != self.observed_sql {
            self.observed_sql = sql;
            self.revision += 1;
            self.error = None;
            cx.notify();
        }
    }
    fn dirty(&self, cx: &App) -> bool {
        self.custom || self.sql.read(cx).state().read(cx).value().as_ref() != self.initial_sql
    }
    fn restore_list_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| {
            list.set_selected_index(self.selected.map(IndexPath::new), window, cx)
        });
    }
    fn load_inputs(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        self.revision += 1;
        self.initial_sql = sql.clone();
        self.observed_sql = sql.clone();
        self.sql
            .read(cx)
            .state()
            .clone()
            .update(cx, |state, cx| state.set_value(sql, window, cx));
        self.error = None;
    }
    pub(crate) fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(aggregation) = self.configured.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.custom = false;
        match editor_expression(&aggregation, &self.schema) {
            Ok(sql) => self.load_inputs(sql, window, cx),
            Err(error) => {
                self.load_inputs(String::new(), window, cx);
                self.error = Some(error);
            }
        }
        self.restore_list_selection(window, cx);
        cx.notify();
    }
    pub(crate) fn start_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_action(PanelAction::Custom, window, cx);
    }
    fn request_action(&mut self, action: PanelAction, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking {
            return;
        }
        if self.dirty(cx) && !matches!(action, PanelAction::Remove) {
            self.pending_action = Some(action);
            self.apply(cx);
        } else {
            self.perform_action(action, window, cx);
        }
    }
    fn perform_action(&mut self, action: PanelAction, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            PanelAction::Select(index) => self.select(index, window, cx),
            PanelAction::Custom => {
                self.custom = true;
                self.selected = None;
                self.load_inputs(String::new(), window, cx);
                self.restore_list_selection(window, cx);
                self.focus_sql(window, cx);
            }
            PanelAction::Add(column) => {
                let Some(schema) = self.schema.iter().find(|schema| schema.name == column) else {
                    return;
                };
                let dtype =
                    crate::application::pivot_editor::data_type_from_duckdb(&schema.duckdb_type);
                self.emit_edit(
                    AggregationEdit::Add(default_aggregation(column, &dtype)),
                    cx,
                );
            }
            PanelAction::Move(direction) => {
                if let Some(aggregation) = self
                    .selected
                    .and_then(|index| self.configured.get(index))
                    .cloned()
                {
                    self.emit_edit(
                        AggregationEdit::Move {
                            aggregation,
                            direction,
                        },
                        cx,
                    );
                }
            }
            PanelAction::Remove => {
                if self.custom {
                    self.revert(window, cx);
                } else if let Some(aggregation) = self
                    .selected
                    .and_then(|index| self.configured.get(index))
                    .cloned()
                {
                    self.emit_edit(AggregationEdit::Remove(aggregation), cx);
                }
            }
        }
        cx.notify();
    }
    fn emit_edit(&mut self, edit: AggregationEdit, cx: &mut Context<Self>) {
        self.input_changed(cx);
        self.checking = true;
        self.checking_revision = self.revision;
        self.error = None;
        cx.emit(AggregationPanelEvent {
            edit,
            revision: self.revision,
        });
        cx.notify();
    }
    pub(crate) fn apply(&mut self, cx: &mut Context<Self>) {
        if self.checking || (!self.custom && self.selected.is_none()) {
            return;
        }
        self.input_changed(cx);
        let sql = self.sql.read(cx).state().read(cx).value().to_string();
        let original = self
            .selected
            .and_then(|index| self.configured.get(index))
            .cloned();
        let replacement = if sql == self.initial_sql
            && let Some(original) = &original
        {
            original.clone()
        } else {
            Aggregation::sql_expression(sql, None)
        };
        if let Err(error) = replacement.validate_expression(self.pivot) {
            self.error = Some(error);
            self.pending_action = None;
            cx.notify();
            return;
        }
        let edit = match original {
            Some(original) => AggregationEdit::Replace {
                original,
                replacement,
            },
            None => AggregationEdit::Add(replacement),
        };
        self.emit_edit(edit, cx);
    }
    pub(crate) fn can_finish(&self, revision: u64, cx: &App) -> bool {
        !self.closed
            && self.checking
            && self.revision == revision
            && self.sql.read(cx).state().read(cx).value().as_ref() == self.observed_sql
    }
    pub(crate) fn finish_error(&mut self, revision: u64, error: String, cx: &mut Context<Self>) {
        if !self.checking || self.checking_revision != revision {
            return;
        }
        self.checking = false;
        self.pending_action = None;
        if !self.closed {
            self.error = Some(error);
        }
        cx.notify();
    }
    pub(crate) fn finish_success(
        &mut self,
        entries: Vec<Aggregation>,
        preferred: Option<Aggregation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.checking = false;
        let next = preferred
            .and_then(|entry| entries.iter().position(|saved| saved == &entry))
            .or_else(|| {
                self.selected
                    .and_then(|index| entries.len().checked_sub(1).map(|last| index.min(last)))
            });
        self.configured = entries.clone();
        self.list.update(cx, |list, cx| {
            list.delegate_mut().entries = entries;
            cx.notify();
        });
        if let Some(index) = next {
            self.select(index, window, cx);
        } else {
            self.selected = None;
            self.custom = false;
            self.load_inputs(String::new(), window, cx);
            self.restore_list_selection(window, cx);
        }
        if let Some(action) = self.pending_action.take() {
            self.perform_action(action, window, cx);
        }
        cx.notify();
    }
    pub(crate) fn revert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.checking = false;
        self.pending_action = None;
        if let Some(index) = self.selected {
            self.select(index, window, cx);
        } else if !self.configured.is_empty() {
            self.select(0, window, cx);
        } else {
            self.custom = false;
            self.load_inputs(String::new(), window, cx);
            self.focus_available(window, cx);
        }
        cx.notify();
    }
    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.revision += 1;
    }
    pub fn focus_available(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = if self.show_functions {
            &self.functions
        } else {
            &self.command
        };
        list.update(cx, |list, cx| list.focus(window, cx));
    }

    pub(crate) fn show_functions(
        &mut self,
        show: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_functions = show;
        self.focus_available(window, cx);
        cx.notify();
    }

    /// Inserts `name()` at the cursor with the cursor between the parentheses,
    /// starting a custom aggregation when none is being edited.
    pub(crate) fn insert_function(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .ordered_functions(cx)
            .get(index)
            .map(|function| function.name.clone())
        else {
            return;
        };
        if self.checking {
            return;
        }
        if !self.custom && self.selected.is_none() {
            self.perform_action(PanelAction::Custom, window, cx);
        }
        let state = self.sql.read(cx).state().clone();
        state.update(cx, |state, cx| {
            state.insert(format!("{name}()"), window, cx);
            let inside = state.cursor().saturating_sub(1);
            state.set_selected_range(inside..inside, cx);
        });
        self.focus_sql(window, cx);
        cx.notify();
    }
    pub(crate) fn focus_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sql
            .read(cx)
            .state()
            .clone()
            .update(cx, |state, cx| state.focus(window, cx));
    }

    fn cycle_focus(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let command = if self.show_functions {
            self.functions.read(cx).focus_handle(cx)
        } else {
            self.command.read(cx).focus_handle(cx)
        };
        let list = self.list.read(cx).focus_handle(cx);
        let sql = self.sql.read(cx).state().read(cx).focus_handle(cx);
        let handles = if self.custom || self.selected.is_some() {
            vec![command, list, sql]
        } else {
            vec![command, list]
        };
        let current = handles
            .iter()
            .position(|handle| window.focused(cx).as_ref() == Some(handle))
            .unwrap_or(0);
        let next = if backwards {
            (current + handles.len() - 1) % handles.len()
        } else {
            (current + 1) % handles.len()
        };
        window.focus(&handles[next], cx);
        cx.stop_propagation();
    }
}

impl AggregationPanel {
    /// The functions in list order: the search matches descriptions too, so
    /// functions named like the search come first, exact names before prefixes.
    fn ordered_functions(&self, cx: &App) -> Vec<AggregateFunction> {
        let Some(Ok(functions)) = &self.aggregate_functions else {
            return Vec::new();
        };
        let query = self.functions.read(cx).query(cx).to_lowercase();
        let mut ordered = functions.clone();
        ordered.sort_by_key(|function| {
            let name = function.name.to_lowercase();
            if name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else if name.contains(&query) {
                2
            } else {
                3
            }
        });
        ordered
    }

    fn render_functions(&self, available_height: f32, cx: &mut Context<Self>) -> AnyElement {
        match &self.aggregate_functions {
            None => return div().child("Loading functions…").into_any_element(),
            Some(Err(error)) => {
                return div()
                    .text_color(cx.theme().danger)
                    .child(format!("Could not list functions: {error}"))
                    .into_any_element();
            }
            Some(Ok(_)) => {}
        }
        let functions = self.ordered_functions(cx);
        let muted = cx.theme().muted_foreground;
        let highlighted = self
            .functions
            .read(cx)
            .selected_index()
            .and_then(|index| functions.get(index.row).cloned());
        let panel = cx.entity();
        let list = div()
            .max_h(px((available_height - 60.).max(120.)))
            .overflow_y_scrollbar()
            .child(
                Command::new(&self.functions)
                    .items(functions.into_iter().map(move |function| {
                        let signatures = function.signatures.join(" · ");
                        let description = function.description.clone();
                        let name = function.name.clone();
                        CommandItem::new()
                            .label(function.name.clone())
                            .keywords([function.description.clone()])
                            .child(move |_, _| {
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .min_w_0()
                                            .child(div().font_semibold().child(name.clone()))
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_sm()
                                                    .text_color(muted)
                                                    .child(signatures.clone()),
                                            ),
                                    )
                                    .when(!description.is_empty(), |row| {
                                        row.child(
                                            div()
                                                .truncate()
                                                .text_sm()
                                                .text_color(muted)
                                                .child(description.clone()),
                                        )
                                    })
                            })
                    }))
                    .placeholder("Find a function")
                    .empty(|_, _, _| div().child("No matching functions"))
                    .on_confirm(move |index, window, cx| {
                        panel.update(cx, |panel, cx| panel.insert_function(index.row, window, cx));
                    }),
            );
        let detail = highlighted.map(|function| {
            div()
                .id("aggregation-function-detail")
                .flex_shrink_0()
                .max_h(px(160.))
                .overflow_hidden()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .border_1()
                .border_color(cx.theme().border)
                .text_sm()
                .child(div().font_semibold().child(function.signatures.join("\n")))
                .when(!function.description.is_empty(), |detail| {
                    detail.child(div().whitespace_normal().child(function.description))
                })
                .children(function.examples.into_iter().map(|example| {
                    div()
                        .whitespace_normal()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(muted)
                        .child(example)
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(list)
            .children(detail)
            .into_any_element()
    }
}

impl Render for AggregationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source_columns = self
            .schema
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let panel = cx.entity();
        let available_height = (window.bounds().size.height.as_f32() - 290.).clamp(90., 440.);
        let list_height = (available_height * 0.3).clamp(68., 150.);
        let editor_height = (window.bounds().size.height.as_f32() - 560.).clamp(70., 220.);
        let editing = self.custom || self.selected.is_some();
        div().key_context("AggregationPanel").flex().gap_4().w_full()
            .on_action(cx.listener(|panel, _: &ApplyAggregation, _, cx| { panel.apply(cx); cx.stop_propagation(); }))
            .on_action(cx.listener(|panel, _: &AggregationNextFocus, window, cx| panel.cycle_focus(false, window, cx)))
            .on_action(cx.listener(|panel, _: &AggregationPreviousFocus, window, cx| panel.cycle_focus(true, window, cx)))
            .capture_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.to_ascii_lowercase();
                let list_focus = panel.list.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&list_focus) {
                    if key == "delete" {
                        panel.request_action(PanelAction::Remove, window, cx); cx.stop_propagation();
                    } else if event.keystroke.modifiers.alt && matches!(key.as_str(), "up" | "down") {
                        panel.request_action(PanelAction::Move(if key == "up" { -1 } else { 1 }), window, cx); cx.stop_propagation();
                    }
                }
            }))
            .on_action(cx.listener(|panel, _: &Enter, window, cx| {
                let functions_focus = panel.functions.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&functions_focus) {
                    if let Some(index) = panel.functions.read(cx).selected_index() {
                        panel.insert_function(index.row, window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let command_focus = panel.command.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&command_focus) {
                    let selected = panel.command.read(cx).selected_index();
                    if let Some(column) = selected.and_then(|index| panel.schema.get(index.row)).map(|column| column.name.clone()) {
                        panel.request_action(PanelAction::Add(column), window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(div().w(relative(0.3)).min_w_0().flex().flex_col().gap_2()
                .child(div().flex().gap_1()
                    .child(Button::new("aggregation-show-columns").label("Columns").small().selected(!self.show_functions)
                        .on_click(cx.listener(|panel, _, window, cx| panel.show_functions(false, window, cx))))
                    .child(Button::new("aggregation-show-functions").label("Functions").small().selected(self.show_functions)
                        .on_click(cx.listener(|panel, _, window, cx| panel.show_functions(true, window, cx)))))
                .child(Button::new("aggregation-custom").label("+ Custom aggregation").disabled(self.checking)
                    .on_click(cx.listener(|panel, _, window, cx| panel.start_custom(window, cx))))
                .when(!self.show_functions, |column| column
                    .child(div().max_h(px(available_height + 80.)).overflow_y_scrollbar()
                        .child(Command::new(&self.command)
                            .items(source_columns.iter().map(|column| CommandItem::new().label(column.clone())))
                            .placeholder("Find a column").empty(|_, _, _| div().child("No source columns"))
                            .on_confirm(move |index, window, cx| {
                                if let Some(column) = source_columns.get(index.row) {
                                    panel.update(cx, |panel, cx| panel.request_action(PanelAction::Add(column.clone()), window, cx));
                                }
                            }))))
                .when(self.show_functions, |column| column
                    .child(div().text_sm().text_color(cx.theme().muted_foreground)
                        .child("Enter inserts the function into the SQL expression."))
                    .child(self.render_functions(available_height, cx))))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                .child(div().font_semibold().child("Aggregations in order"))
                .child(div().h(px(list_height)).child(List::new(&self.list)))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Alt+↑/↓ move · Delete remove"))
                .child(div().border_t_1().border_color(cx.theme().border).pt_2())
                .when(!editing, |content| content.child(div().text_color(cx.theme().muted_foreground)
                    .child("Add a column or a custom aggregation to edit its SQL.")))
                .when(editing, |content| content
                    .child(div().flex().items_center().justify_between().child("SQL expression")
                        .child(Button::new("aggregation-limitations").label("DuckDB pivot limitations")
                            .on_click(|_, _, cx| cx.open_url(PIVOT_LIMITATIONS_URL))))
                    .child(div().text_sm().text_color(cx.theme().muted_foreground)
                        .child("Use AS to name a column, or keep DuckDB’s default name."))
                    .when(self.pivot, |content| content.child(div().text_sm().text_color(cx.theme().muted_foreground)
                        .child("DuckDB pivot expression rules apply. FILTER is unsupported.")))
                    .child(div().id("aggregation-sql").h(px(editor_height)).border_1().border_color(cx.theme().border)
                        .child(Editor::new(self.sql.read(cx).state()).h(relative(1.)))
                        .children(self.sql.read(cx).completion_menu(cx)))
                    .when_some(self.error.clone(), |content, error| content.child(div().max_h(px(70.)).overflow_y_scrollbar().child(div().id("aggregation-error").text_sm().text_color(cx.theme().danger).child(error))))
                    .child(div().flex().justify_between()
                        .child(Button::new("aggregation-revert").label("Revert").on_click(cx.listener(|panel, _, window, cx| panel.revert(window, cx))))
                        .child(Button::new("aggregation-apply").label(if self.checking { "Checking…" } else { "Apply · Ctrl+Enter" })
                            .disabled(self.checking).on_click(cx.listener(|panel, _, _, cx| panel.apply(cx)))))))
    }
}
