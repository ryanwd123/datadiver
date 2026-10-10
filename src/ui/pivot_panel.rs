use gpui_kit::component::button::Button;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Editor, Enter, InputEvent};
use gpui_kit::component::list::{List, ListDelegate, ListEvent, ListItem, ListState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, StyledExt as _, WindowExt as _,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::application::pivot_editor::{PivotEdit, PivotFieldKind};
use crate::domain::documents::Dimension;
use crate::domain::pivot::dimension_editor_expression;
use crate::infrastructure::duckdb_store::ColumnSchema;
use crate::ui::input_with_autocompletion::InputWithAutocompletion;

gpui_kit::actions!(
    datadiver,
    [ApplyDimension, DimensionNextFocus, DimensionPreviousFocus]
);
struct DimensionBindings;
impl Global for DimensionBindings {}

#[derive(Clone, Debug)]
pub struct PivotPanelEvent {
    pub edit: PivotEdit,
    pub revision: u64,
}

struct ConfiguredDimensions {
    entries: Vec<Dimension>,
    selected: Option<IndexPath>,
}

impl ListDelegate for ConfiguredDimensions {
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
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some(
            ListItem::new(format!("dimension-{}", index.row))
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
        div().py_3().child("No dimensions configured")
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

pub struct PivotPanel {
    schema: Vec<ColumnSchema>,
    configured: Vec<Dimension>,
    command: Entity<CommandState>,
    list: Entity<ListState<ConfiguredDimensions>>,
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
    kind: PivotFieldKind,
    blocked: Vec<Dimension>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PivotPanelEvent> for PivotPanel {}

impl PivotPanel {
    pub fn new(
        schema: Vec<ColumnSchema>,
        configured: Vec<Dimension>,
        kind: PivotFieldKind,
        blocked: Vec<Dimension>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if !cx.has_global::<DimensionBindings>() {
            cx.bind_keys([
                KeyBinding::new("ctrl-enter", ApplyDimension, Some("PivotPanel")),
                KeyBinding::new("ctrl-enter", ApplyDimension, Some("PivotPanel > Input")),
                KeyBinding::new("tab", DimensionNextFocus, Some("PivotPanel")),
                KeyBinding::new("tab", DimensionNextFocus, Some("PivotPanel > Input")),
                KeyBinding::new("shift-tab", DimensionPreviousFocus, Some("PivotPanel")),
                KeyBinding::new(
                    "shift-tab",
                    DimensionPreviousFocus,
                    Some("PivotPanel > Input"),
                ),
            ]);
            cx.set_global(DimensionBindings);
        }
        let command = cx.new(|cx| CommandState::new(window, cx));
        let list = cx.new(|cx| {
            ListState::new(
                ConfiguredDimensions {
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
            kind,
            blocked,
            _subscriptions: vec![list_subscription, sql_subscription],
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
        let Some(dimension) = self.configured.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.custom = false;
        match dimension_editor_expression(&dimension, &self.schema) {
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
                self.emit_edit(PivotEdit::Add(Dimension::Column(column)), cx);
            }
            PanelAction::Move(direction) => {
                if let Some(dimension) = self
                    .selected
                    .and_then(|index| self.configured.get(index))
                    .cloned()
                {
                    self.emit_edit(
                        PivotEdit::Move {
                            field: dimension,
                            direction,
                        },
                        cx,
                    );
                }
            }
            PanelAction::Remove => {
                if self.custom {
                    self.revert(window, cx);
                } else if let Some(dimension) = self
                    .selected
                    .and_then(|index| self.configured.get(index))
                    .cloned()
                {
                    self.emit_edit(PivotEdit::Remove(dimension), cx);
                }
            }
        }
        cx.notify();
    }
    fn emit_edit(&mut self, edit: PivotEdit, cx: &mut Context<Self>) {
        self.input_changed(cx);
        self.checking = true;
        self.checking_revision = self.revision;
        self.error = None;
        cx.emit(PivotPanelEvent {
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
            Dimension::expression(sql)
        };
        if let Err(error) = replacement.validate() {
            self.error = Some(error);
            self.pending_action = None;
            cx.notify();
            return;
        }
        let edit = match original {
            Some(original) => PivotEdit::Replace {
                original,
                replacement,
            },
            None => PivotEdit::Add(replacement),
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
        entries: Vec<Dimension>,
        preferred: Option<Dimension>,
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
    fn available_columns(&self) -> Vec<String> {
        self.schema
            .iter()
            .map(|column| column.name.clone())
            .filter(|name| {
                !self
                    .configured
                    .iter()
                    .chain(&self.blocked)
                    .any(|field| field.column() == Some(name.as_str()))
            })
            .collect()
    }
    pub fn focus_available(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command
            .update(cx, |command, cx| command.focus(window, cx));
    }
    pub(crate) fn focus_sql(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sql
            .read(cx)
            .state()
            .clone()
            .update(cx, |state, cx| state.focus(window, cx));
    }

    fn cycle_focus(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.command.read(cx).focus_handle(cx);
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

impl Render for PivotPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source_columns = self.available_columns();
        let title = match self.kind {
            PivotFieldKind::GroupBy => "Group By",
            PivotFieldKind::PivotBy => "Pivot By",
        };
        let panel = cx.entity();
        let available_height = (window.bounds().size.height.as_f32() - 290.).clamp(90., 440.);
        let list_height = (available_height * 0.3).clamp(68., 150.);
        let editor_height = (window.bounds().size.height.as_f32() - 560.).clamp(70., 220.);
        let editing = self.custom || self.selected.is_some();
        div()
            .key_context("PivotPanel")
            .flex()
            .gap_4()
            .w_full()
            .on_action(cx.listener(|panel, _: &ApplyDimension, _, cx| {
                panel.apply(cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|panel, _: &DimensionNextFocus, window, cx| {
                panel.cycle_focus(false, window, cx)
            }))
            .on_action(
                cx.listener(|panel, _: &DimensionPreviousFocus, window, cx| {
                    panel.cycle_focus(true, window, cx)
                }),
            )
            .capture_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.to_ascii_lowercase();
                let list_focus = panel.list.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&list_focus) {
                    if key == "delete" {
                        panel.request_action(PanelAction::Remove, window, cx);
                        cx.stop_propagation();
                    } else if event.keystroke.modifiers.alt && matches!(key.as_str(), "up" | "down")
                    {
                        panel.request_action(
                            PanelAction::Move(if key == "up" { -1 } else { 1 }),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }
                }
            }))
            .on_action(cx.listener(|panel, _: &Enter, window, cx| {
                let command_focus = panel.command.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&command_focus) {
                    let selected = panel.command.read(cx).selected_index();
                    if let Some(column) =
                        selected.and_then(|index| panel.available_columns().get(index.row).cloned())
                    {
                        panel.request_action(PanelAction::Add(column), window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w(relative(0.3))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().font_semibold().child("Available columns"))
                    .child(
                        Button::new("dimension-custom")
                            .label("+ Custom expression")
                            .disabled(self.checking)
                            .on_click(
                                cx.listener(|panel, _, window, cx| panel.start_custom(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .max_h(px(available_height + 80.))
                            .overflow_y_scrollbar()
                            .child(
                                Command::new(&self.command)
                                    .items(
                                        source_columns
                                            .iter()
                                            .map(|column| CommandItem::new().label(column.clone())),
                                    )
                                    .placeholder("Find a column")
                                    .empty(|_, _, _| div().child("No available columns"))
                                    .on_confirm(move |index, window, cx| {
                                        if let Some(column) = source_columns.get(index.row) {
                                            panel.update(cx, |panel, cx| {
                                                panel.request_action(
                                                    PanelAction::Add(column.clone()),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        }
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(div().font_semibold().child(format!("{title} order")))
                            .child(Button::new("dimension-close").label("Close").on_click(
                                cx.listener(|panel, _, window, cx| {
                                    panel.close();
                                    window.close_dialog(cx);
                                }),
                            )),
                    )
                    .child(div().h(px(list_height)).child(List::new(&self.list)))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Alt+↑/↓ move · Delete remove"),
                    )
                    .child(div().border_t_1().border_color(cx.theme().border).pt_2())
                    .when(!editing, |content| {
                        content.child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child("Add a column or a custom expression to edit its SQL."),
                        )
                    })
                    .when(editing, |content| {
                        content
                            .child(div().child("SQL expression"))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "Use AS to name a column, or keep DuckDB’s default name.",
                                    ),
                            )
                            .child(
                                div()
                                    .id("dimension-sql")
                                    .h(px(editor_height))
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .child(Editor::new(self.sql.read(cx).state()).h(relative(1.)))
                                    .children(self.sql.read(cx).completion_menu(cx)),
                            )
                            .when_some(self.error.clone(), |content, error| {
                                content.child(
                                    div().max_h(px(70.)).overflow_y_scrollbar().child(
                                        div()
                                            .id("dimension-error")
                                            .text_sm()
                                            .text_color(cx.theme().danger)
                                            .child(error),
                                    ),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .child(
                                        Button::new("dimension-revert").label("Revert").on_click(
                                            cx.listener(|panel, _, window, cx| {
                                                panel.revert(window, cx)
                                            }),
                                        ),
                                    )
                                    .child(
                                        Button::new("dimension-apply")
                                            .label(if self.checking {
                                                "Checking…"
                                            } else {
                                                "Apply · Ctrl+Enter"
                                            })
                                            .disabled(self.checking)
                                            .on_click(
                                                cx.listener(|panel, _, _, cx| panel.apply(cx)),
                                            ),
                                    ),
                            )
                    }),
            )
    }
}
