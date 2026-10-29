use crate::application::pivot_editor::{AggregationEdit, default_aggregation, operation_label};
use crate::domain::aggregation_sql::PIVOT_LIMITATIONS_URL;
use crate::domain::documents::Aggregation;
use crate::domain::pivot::editor_expression;
use crate::infrastructure::duckdb_store::ColumnSchema;
use crate::infrastructure::store::Engine;
use crate::infrastructure::sql_execution::AggregateFunction;
use crate::ui::hit::{Button, Hit};
use crate::ui::input_with_autocompletion;
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, TextOptions, rect, system, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::list::{self, Item, List, Palette};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

#[derive(Clone, Debug)]
pub struct AggregationPanelEvent {
    pub edit: AggregationEdit,
    pub revision: u64,
}

#[derive(Clone)]
enum PanelAction {
    Select(usize),
    Add(String),
    Custom,
    Move(isize),
    Remove,
}

#[derive(Default)]
pub struct PanelResult {
    pub handled: bool,
    pub event: Option<AggregationPanelEvent>,
}

fn aggregation_label(entry: &Aggregation) -> String {
    entry
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
        })
}

pub struct AggregationPanel {
    pub id: WidgetId,
    schema: Vec<ColumnSchema>,
    configured: Vec<Aggregation>,
    pub(crate) command: Palette,
    /// Searchable reference of the engine's aggregate functions, shown in
    /// place of the column list.
    pub(crate) functions: Palette,
    /// `None` while the list loads.
    aggregate_functions: Option<Result<Vec<AggregateFunction>, String>>,
    /// The functions in list order for the current search.
    ordered: Vec<AggregateFunction>,
    pub(crate) show_functions: bool,
    pub(crate) list: List,
    pub(crate) sql: Editor,
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
    /// The engine of the source the aggregations run on.
    engine: Engine,
    events: Vec<AggregationPanelEvent>,
}

impl AggregationPanel {
    pub fn new(schema: Vec<ColumnSchema>, configured: Vec<Aggregation>, pivot: bool, engine: Engine) -> Self {
        let sql = input_with_autocompletion::sql_columns(schema.iter().map(|column| column.storage_name.clone()));
        let mut panel = Self {
            id: WidgetId::new(),
            command: Palette::new("Find a column"),
            functions: Palette::new("Find a function"),
            aggregate_functions: None,
            ordered: Vec::new(),
            show_functions: false,
            list: List::new(),
            sql,
            schema,
            configured: Vec::new(),
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
            engine,
            events: Vec::new(),
        };
        panel.command.list.empty_text = "No source columns".into();
        panel.functions.list.empty_text = "No matching functions".into();
        panel.functions.filterable = false;
        panel.list.empty_text = "No aggregations configured".into();
        panel.list.select_on_click = true;
        let columns = panel.schema.iter().map(|column| Item::new(column.name.clone())).collect();
        panel.command.set_items(columns);
        panel.set_configured(configured);
        if !panel.configured.is_empty() {
            panel.select(0);
        }
        panel
    }

    pub fn set_functions(&mut self, functions: Result<Vec<AggregateFunction>, String>) {
        self.aggregate_functions = Some(functions);
        self.refresh_functions();
    }

    fn set_configured(&mut self, entries: Vec<Aggregation>) {
        self.configured = entries;
        self.list
            .set_items(self.configured.iter().map(|entry| Item::new(aggregation_label(entry))).collect());
        self.restore_list_selection();
    }

    pub fn take_event(&mut self) -> Option<AggregationPanelEvent> {
        (!self.events.is_empty()).then(|| self.events.remove(0))
    }

    fn input_changed(&mut self) {
        let sql = self.sql.text().to_owned();
        if sql != self.observed_sql {
            self.observed_sql = sql;
            self.revision += 1;
            self.error = None;
        }
    }

    fn dirty(&self) -> bool {
        self.custom || self.sql.text() != self.initial_sql
    }

    fn restore_list_selection(&mut self) {
        self.list.select_item(self.selected);
    }

    fn load_inputs(&mut self, sql: String) {
        self.revision += 1;
        self.initial_sql = sql.clone();
        self.observed_sql = sql.clone();
        self.sql.set_text(sql);
        self.error = None;
    }

    pub(crate) fn select(&mut self, index: usize) {
        let Some(aggregation) = self.configured.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.custom = false;
        match editor_expression(&aggregation, &self.schema, self.engine) {
            Ok(sql) => self.load_inputs(sql),
            Err(error) => {
                self.load_inputs(String::new());
                self.error = Some(error);
            }
        }
        self.restore_list_selection();
    }

    pub(crate) fn start_custom(&mut self, focus: &mut Option<WidgetId>) {
        self.request_action(PanelAction::Custom, focus);
    }

    fn request_action(&mut self, action: PanelAction, focus: &mut Option<WidgetId>) {
        if self.checking {
            return;
        }
        if self.dirty() && !matches!(action, PanelAction::Remove) {
            self.pending_action = Some(action);
            self.apply();
        } else {
            self.perform_action(action, focus);
        }
    }

    fn perform_action(&mut self, action: PanelAction, focus: &mut Option<WidgetId>) {
        match action {
            PanelAction::Select(index) => self.select(index),
            PanelAction::Custom => {
                self.custom = true;
                self.selected = None;
                self.load_inputs(String::new());
                self.restore_list_selection();
                *focus = Some(self.sql.id);
            }
            PanelAction::Add(column) => {
                let Some(schema) = self.schema.iter().find(|schema| schema.name == column) else {
                    return;
                };
                let dtype = crate::application::pivot_editor::data_type_from_duckdb(&schema.sql_type);
                self.emit_edit(AggregationEdit::Add(default_aggregation(column, &dtype)));
            }
            PanelAction::Move(direction) => {
                if let Some(aggregation) = self.selected.and_then(|index| self.configured.get(index)).cloned() {
                    self.emit_edit(AggregationEdit::Move {
                        aggregation,
                        direction,
                    });
                }
            }
            PanelAction::Remove => {
                if self.custom {
                    self.revert(focus);
                } else if let Some(aggregation) = self.selected.and_then(|index| self.configured.get(index)).cloned() {
                    self.emit_edit(AggregationEdit::Remove(aggregation));
                }
            }
        }
    }

    fn emit_edit(&mut self, edit: AggregationEdit) {
        self.input_changed();
        self.checking = true;
        self.checking_revision = self.revision;
        self.error = None;
        self.events.push(AggregationPanelEvent {
            edit,
            revision: self.revision,
        });
    }

    pub(crate) fn apply(&mut self) {
        if self.checking || (!self.custom && self.selected.is_none()) {
            return;
        }
        self.input_changed();
        let sql = self.sql.text().to_owned();
        let original = self.selected.and_then(|index| self.configured.get(index)).cloned();
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
            return;
        }
        let edit = match original {
            Some(original) => AggregationEdit::Replace {
                original,
                replacement,
            },
            None => AggregationEdit::Add(replacement),
        };
        self.emit_edit(edit);
    }

    pub(crate) fn can_finish(&self, revision: u64) -> bool {
        !self.closed && self.checking && self.revision == revision && self.sql.text() == self.observed_sql
    }

    pub(crate) fn finish_error(&mut self, revision: u64, error: String) {
        if !self.checking || self.checking_revision != revision {
            return;
        }
        self.checking = false;
        self.pending_action = None;
        if !self.closed {
            self.error = Some(error);
        }
    }

    pub(crate) fn finish_success(&mut self, entries: Vec<Aggregation>, preferred: Option<Aggregation>, focus: &mut Option<WidgetId>) {
        self.checking = false;
        let next = preferred
            .and_then(|entry| entries.iter().position(|saved| saved == &entry))
            .or_else(|| {
                self.selected
                    .and_then(|index| entries.len().checked_sub(1).map(|last| index.min(last)))
            });
        self.set_configured(entries);
        if let Some(index) = next {
            self.select(index);
        } else {
            self.selected = None;
            self.custom = false;
            self.load_inputs(String::new());
            self.restore_list_selection();
        }
        if let Some(action) = self.pending_action.take() {
            self.perform_action(action, focus);
        }
    }

    pub(crate) fn revert(&mut self, focus: &mut Option<WidgetId>) {
        self.checking = false;
        self.pending_action = None;
        if let Some(index) = self.selected {
            self.select(index);
        } else if !self.configured.is_empty() {
            self.select(0);
        } else {
            self.custom = false;
            self.load_inputs(String::new());
            self.focus_available(focus);
        }
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.revision += 1;
    }

    pub fn focus_available(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(if self.show_functions {
            self.functions.query.id
        } else {
            self.command.query.id
        });
    }

    pub(crate) fn show_functions(&mut self, show: bool, focus: &mut Option<WidgetId>) {
        self.show_functions = show;
        self.focus_available(focus);
    }

    /// Orders the functions for the search: functions named like it come
    /// first, exact names before prefixes; descriptions match too.
    fn refresh_functions(&mut self) {
        let Some(Ok(functions)) = &self.aggregate_functions else {
            self.ordered.clear();
            self.functions.set_items(Vec::new());
            return;
        };
        let query = self.functions.query_text().to_lowercase();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        let mut ordered = functions
            .iter()
            .filter(|function| {
                let searchable = format!("{} {}", function.name, function.description).to_lowercase();
                terms.iter().all(|term| searchable.contains(term))
            })
            .cloned()
            .collect::<Vec<_>>();
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
        let items = ordered
            .iter()
            .map(|function| {
                Item::new(function.name.clone())
                    .detail(function.signatures.join(" · "))
                    .description(function.description.clone())
            })
            .collect();
        self.ordered = ordered;
        self.functions.set_items(items);
    }

    /// Inserts `name()` at the cursor with the cursor between the parentheses,
    /// starting a custom aggregation when none is being edited.
    pub(crate) fn insert_function(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        let Some(name) = self.ordered.get(index).map(|function| function.name.clone()) else {
            return;
        };
        if self.checking {
            return;
        }
        if !self.custom && self.selected.is_none() {
            self.perform_action(PanelAction::Custom, focus);
        }
        self.sql.insert(&format!("{name}()"));
        let inside = self.sql.cursor().saturating_sub(1);
        self.sql.set_selection(inside..inside);
        self.input_changed();
        *focus = Some(self.sql.id);
    }

    pub(crate) fn focus_sql(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.sql.id);
    }

    fn editing(&self) -> bool {
        self.custom || self.selected.is_some()
    }

    fn cycle_focus(&mut self, backwards: bool, focus: &mut Option<WidgetId>) {
        let command = if self.show_functions {
            self.functions.query.id
        } else {
            self.command.query.id
        };
        let mut handles = vec![command, self.list.id];
        if self.editing() {
            handles.push(self.sql.id);
        }
        let current = handles
            .iter()
            .position(|handle| Some(*handle) == *focus)
            .unwrap_or(0);
        let next = if backwards {
            (current + handles.len() - 1) % handles.len()
        } else {
            (current + 1) % handles.len()
        };
        *focus = Some(handles[next]);
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        [self.command.query.id, self.functions.query.id, self.list.id, self.sql.id].contains(&id)
    }

    fn list_selected(&mut self, focus: &mut Option<WidgetId>) {
        let Some(index) = self.list.selected_item() else {
            return;
        };
        self.restore_list_selection();
        if self.selected != Some(index) || self.custom {
            self.request_action(PanelAction::Select(index), focus);
        }
    }

    fn take(&mut self, handled: bool) -> PanelResult {
        PanelResult {
            handled,
            event: self.take_event(),
        }
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> PanelResult {
        let m = event.modifiers;
        let key = event.key.as_str();
        let sql_focused = *focus == Some(self.sql.id);
        if key == "enter" && m.control && !m.shift && !m.alt {
            self.apply();
            return self.take(true);
        }
        if key == "tab" && !m.control && !m.alt && !(sql_focused && self.sql.completion.is_some()) {
            self.cycle_focus(m.shift, focus);
            return self.take(true);
        }
        if *focus == Some(self.list.id) {
            if key == "delete" && !m.modified() {
                self.request_action(PanelAction::Remove, focus);
                return self.take(true);
            }
            if m.alt && matches!(key, "up" | "down") {
                self.request_action(PanelAction::Move(if key == "up" { -1 } else { 1 }), focus);
                return self.take(true);
            }
            let outcome = self.list.key(event, true);
            if outcome.selected || outcome.confirmed.is_some() {
                self.list_selected(focus);
            }
            return self.take(outcome.handled);
        }
        if *focus == Some(self.functions.query.id) {
            let outcome = self.functions.key(event);
            if let Some(index) = outcome.confirmed {
                self.insert_function(index, focus);
                return self.take(true);
            }
            if outcome.query_changed {
                self.refresh_functions();
            }
            return self.take(outcome.handled);
        }
        if *focus == Some(self.command.query.id) {
            let outcome = self.command.key(event);
            if let Some(index) = outcome.confirmed {
                if let Some(column) = self.schema.get(index).map(|column| column.name.clone()) {
                    self.request_action(PanelAction::Add(column), focus);
                }
                return self.take(true);
            }
            return self.take(outcome.handled);
        }
        if sql_focused {
            let outcome = self.sql.key(event);
            if outcome.changed {
                self.input_changed();
            }
            return self.take(outcome.handled);
        }
        self.take(false)
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus == Some(self.command.query.id) {
            self.command.type_text(typed);
        } else if focus == Some(self.functions.query.id) {
            if self.functions.type_text(typed).query_changed {
                self.refresh_functions();
            }
        } else if focus == Some(self.sql.id) {
            self.sql.type_text(typed);
            self.input_changed();
        } else {
            return false;
        }
        true
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> PanelResult {
        match hit {
            Hit::Button(Button::PanelCustom) => self.start_custom(focus),
            Hit::Button(Button::PanelRevert) => self.revert(focus),
            Hit::Button(Button::PanelApply) => self.apply(),
            Hit::Button(Button::PanelShowColumns) => self.show_functions(false, focus),
            Hit::Button(Button::PanelShowFunctions) => self.show_functions(true, focus),
            Hit::Button(Button::PanelLimitations) => system::open_url(PIVOT_LIMITATIONS_URL),
            Hit::Editor(id, part) if *id == self.command.query.id => {
                self.command.query.mouse_down(*part, pos, clicks, shift);
                *focus = Some(*id);
            }
            Hit::Editor(id, part) if *id == self.functions.query.id => {
                self.functions.query.mouse_down(*part, pos, clicks, shift);
                *focus = Some(*id);
            }
            Hit::List(id, part) if *id == self.command.list.id => {
                let outcome = self.command.mouse_down(*part, pos, clicks, shift);
                *focus = Some(self.command.query.id);
                if let Some(index) = outcome.confirmed
                    && let Some(column) = self.schema.get(index).map(|column| column.name.clone())
                {
                    self.request_action(PanelAction::Add(column), focus);
                }
            }
            Hit::List(id, part) if *id == self.functions.list.id => {
                let outcome = self.functions.mouse_down(*part, pos, clicks, shift);
                *focus = Some(self.functions.query.id);
                if let Some(index) = outcome.confirmed {
                    self.insert_function(index, focus);
                }
            }
            Hit::List(id, part) if *id == self.list.id => {
                let outcome = self.list.mouse_down(*part, pos, clicks);
                *focus = Some(self.list.id);
                if outcome.selected {
                    self.list_selected(focus);
                }
            }
            Hit::Editor(id, part) if *id == self.sql.id => {
                let outcome = self.sql.mouse_down(*part, pos, clicks, shift);
                if outcome.changed {
                    self.input_changed();
                }
                *focus = Some(self.sql.id);
            }
            _ => return PanelResult::default(),
        }
        self.take(true)
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        self.command.mouse_drag(pos);
        self.functions.mouse_drag(pos);
        self.list.mouse_drag(pos);
        if self.sql.is_dragging() {
            self.sql.mouse_drag(pos);
        }
    }

    pub fn mouse_up(&mut self) {
        self.command.mouse_up();
        self.functions.mouse_up();
        self.list.mouse_up();
        self.sql.mouse_up();
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point) -> bool {
        match hit {
            Hit::List(id, _) if *id == self.command.list.id => self.command.list.wheel(delta),
            Hit::List(id, _) if *id == self.functions.list.id => self.functions.list.wheel(delta),
            Hit::List(id, _) if *id == self.list.id => self.list.wheel(delta),
            Hit::Editor(id, _) if *id == self.sql.id => self.sql.wheel(delta),
            _ => false,
        }
    }

    fn heights(window_height: f32) -> (f32, f32, f32) {
        let available = (window_height - 290.0).clamp(90.0, 440.0);
        let list = (available * 0.3).clamp(68.0, 150.0);
        let editor = (window_height - 560.0).clamp(70.0, 220.0);
        (available, list, editor)
    }

    pub fn height(&self, fonts: &Fonts, window_height: f32) -> f32 {
        let line = text::line_height(&fonts.ui);
        let (available, list, editor) = Self::heights(window_height);
        let left = button_height(&fonts.ui) * 2.0 + 16.0 + available + 80.0;
        let right = line + 8.0 + list + 8.0 + line + 16.0
            + if self.editing() {
                button_height(&fonts.ui) + line * 2.0 + editor + 32.0 + button_height(&fonts.ui)
                    + if self.error.is_some() { 70.0 } else { 0.0 }
            } else {
                line + 8.0
            };
        left.max(right)
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>, window_height: f32) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let (available, list_height, editor_height) = Self::heights(window_height);
        let mut row = Row::new(r, 16.0);
        let left = row.left(r.w * 0.3);
        let right = row.rest();
        let mut column = Column::new(left, 8.0);
        let toggles = column.next(button_height(&fonts.small));
        let mut toggles = Row::new(toggles, 4.0);
        let columns_button = toggles.left(button_width("Columns", &fonts.small));
        button(
            canvas,
            columns_button,
            "Columns",
            &fonts.small,
            if self.show_functions { ButtonStyle::Outline } else { ButtonStyle::Selected },
            true,
            Hit::Button(Button::PanelShowColumns),
        );
        let functions_button = toggles.left(button_width("Functions", &fonts.small));
        button(
            canvas,
            functions_button,
            "Functions",
            &fonts.small,
            if self.show_functions { ButtonStyle::Selected } else { ButtonStyle::Outline },
            true,
            Hit::Button(Button::PanelShowFunctions),
        );
        let custom = column.next(button_height(font));
        button(
            canvas,
            custom.with_w(button_width("+ Custom aggregation", font).min(custom.w)),
            "+ Custom aggregation",
            font,
            ButtonStyle::Outline,
            !self.checking,
            Hit::Button(Button::PanelCustom),
        );
        if self.show_functions {
            let hint = column.next(line);
            canvas.text(hint, "Enter inserts the function into the SQL expression.", &fonts.small, colors::MUTED, Align::Left);
            match &self.aggregate_functions {
                None => {
                    canvas.text(column.next(line), "Loading functions…", font, colors::FOREGROUND, Align::Left);
                }
                Some(Err(error)) => {
                    canvas.text(
                        column.next(line),
                        &format!("Could not list functions: {error}"),
                        font,
                        colors::DANGER,
                        Align::Left,
                    );
                }
                Some(Ok(_)) => {
                    let highlighted = self
                        .functions
                        .selected_item()
                        .and_then(|index| self.ordered.get(index).cloned());
                    let detail_height = if highlighted.is_some() { 160.0 } else { 0.0 };
                    let rest = column.rest();
                    let list_area = rect(rest.x, rest.y, rest.w, (rest.h - detail_height - 8.0).min(available.max(120.0)).max(80.0));
                    let (query_id, list_id) = (self.functions.query.id, self.functions.list.id);
                    self.functions.paint(canvas, list_area, font, focus == Some(query_id), |part| match part {
                        list::Part::Query(part) => Hit::Editor(query_id, part),
                        part => Hit::List(list_id, part),
                    });
                    column.skip(list_area.h + 8.0);
                    if let Some(function) = highlighted {
                        let detail = column.next(detail_height.min(column.rest().h));
                        canvas.stroke(detail, colors::BORDER, 1.0);
                        canvas.push_clip(detail.inset(8.0));
                        let mut y = detail.y + 8.0;
                        let width = detail.w - 16.0;
                        for (text_value, font, color) in std::iter::once((function.signatures.join("\n"), fonts.small.semibold(), colors::FOREGROUND))
                            .chain((!function.description.is_empty()).then(|| (function.description.clone(), fonts.small.clone(), colors::FOREGROUND)))
                            .chain(function.examples.iter().map(|example| (example.clone(), fonts.mono.sized(fonts.small.size), colors::MUTED)))
                        {
                            let layout = text::layout(&text_value, &font, TextOptions::wrapped(width));
                            canvas.draw_layout(&layout, crate::ui::platform::point(detail.x + 8.0, y), color);
                            y += layout.height + 4.0;
                        }
                        canvas.pop_clip();
                    }
                }
            }
        } else {
            let command_area = column.next(column.rest().h.min(available + 80.0));
            let (query_id, list_id) = (self.command.query.id, self.command.list.id);
            self.command.paint(canvas, command_area, font, focus == Some(query_id), |part| match part {
                list::Part::Query(part) => Hit::Editor(query_id, part),
                part => Hit::List(list_id, part),
            });
        }
        let mut column = Column::new(right, 8.0);
        canvas.text(column.next(line), "Aggregations in order", &font.semibold(), colors::FOREGROUND, Align::Left);
        let list_area = column.next(list_height);
        canvas.stroke_rounded(list_area, 5.0, colors::BORDER, 1.0);
        let configured_id = self.list.id;
        self.list.paint(canvas, list_area.inset(2.0), font, focus == Some(configured_id), |part| {
            Hit::List(configured_id, part)
        });
        canvas.text(column.next(line), "Alt+↑/↓ move · Delete remove", &fonts.small, colors::MUTED, Align::Left);
        canvas.fill(column.next(1.0), colors::BORDER);
        if !self.editing() {
            canvas.text(
                column.next(line),
                "Add a column or a custom aggregation to edit its SQL.",
                font,
                colors::MUTED,
                Align::Left,
            );
            return;
        }
        let header = column.next(button_height(font));
        let mut header_row = Row::new(header, 8.0);
        let duckdb = self.engine == Engine::DuckDb;
        if duckdb {
            let limitations = header_row.right(button_width("DuckDB pivot limitations", font));
            button(
                canvas,
                limitations,
                "DuckDB pivot limitations",
                font,
                ButtonStyle::Outline,
                true,
                Hit::Button(Button::PanelLimitations),
            );
        }
        let title = format!("{} SQL expression", self.engine.label());
        canvas.text(header_row.rest(), &title, font, colors::FOREGROUND, Align::Left);
        canvas.text(
            column.next(line),
            "Use AS to name a column, or keep the default name.",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
        if self.pivot {
            // Pivots refuse FILTER in both engines, so a view keeps working
            // when its file switches to DuckDB.
            let rule = if duckdb {
                "DuckDB pivot expression rules apply. FILTER is unsupported."
            } else {
                "FILTER is unsupported in pivots, as in DuckDB."
            };
            canvas.text(column.next(line), rule, &fonts.small, colors::MUTED, Align::Left);
        }
        let editor = column.next(editor_height);
        let sql_id = self.sql.id;
        self.sql
            .paint(canvas, editor, &fonts.mono, focus == Some(sql_id), |part| Hit::Editor(sql_id, part));
        if let Some(error) = self.error.clone() {
            let area = column.rest();
            let error_rect = rect(area.x, area.y, area.w, 70.0_f32.min(area.h - button_height(font) - 8.0).max(line));
            canvas.push_clip(error_rect);
            let height = canvas.paragraph(error_rect, &error, &fonts.small, colors::DANGER);
            canvas.pop_clip();
            column.skip(height.min(error_rect.h) + 8.0);
        }
        let buttons = column.next(button_height(font));
        let mut buttons = Row::new(buttons, 8.0);
        let revert = buttons.left(button_width("Revert", font));
        button(canvas, revert, "Revert", font, ButtonStyle::Outline, true, Hit::Button(Button::PanelRevert));
        let label = if self.checking { "Checking…" } else { "Apply · Ctrl+Enter" };
        let apply = buttons.right(button_width(label, font));
        button(canvas, apply, label, font, ButtonStyle::Outline, !self.checking, Hit::Button(Button::PanelApply));
    }

    pub fn paint_overlay(&mut self, canvas: &mut Canvas<Hit>, focus: Option<WidgetId>) {
        let id = self.sql.id;
        self.sql
            .paint_completion(canvas, focus == Some(id), |part| Hit::Editor(id, part));
    }
}
