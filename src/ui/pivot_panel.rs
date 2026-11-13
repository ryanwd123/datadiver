use crate::application::pivot_editor::{PivotEdit, PivotFieldKind};
use crate::domain::documents::{Dimension, Engine};
use crate::domain::expression_completion::{self, ExpressionCatalog};
use crate::domain::pivot::dimension_editor_expression;
use crate::infrastructure::duckdb_store::ColumnSchema;
use crate::ui::hit::{Button, Hit};
use crate::ui::input_with_autocompletion;
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::list::{self, Item, List, Palette};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

#[derive(Clone, Debug)]
pub struct PivotPanelEvent {
    pub edit: PivotEdit,
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

/// What the panel asks of its host after input.
#[derive(Default)]
pub struct PanelResult {
    pub handled: bool,
    pub event: Option<PivotPanelEvent>,
    pub close: bool,
}

pub struct PivotPanel {
    /// Tells this panel apart from a later one in candidate checks.
    pub id: WidgetId,
    schema: Vec<ColumnSchema>,
    configured: Vec<Dimension>,
    pub(crate) command: Palette,
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
    kind: PivotFieldKind,
    blocked: Vec<Dimension>,
    events: Vec<PivotPanelEvent>,
}

fn dimension_label(entry: &Dimension) -> String {
    entry
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl PivotPanel {
    pub fn new(
        schema: Vec<ColumnSchema>,
        configured: Vec<Dimension>,
        kind: PivotFieldKind,
        blocked: Vec<Dimension>,
        engine: Engine,
    ) -> Self {
        let catalog = ExpressionCatalog::new(schema.iter().map(|c| (c.storage_name.clone(), Some(c.sql_type.clone()))));
        let sql = input_with_autocompletion::sql_expression(catalog, expression_completion::dialect(engine));
        let mut panel = Self {
            id: WidgetId::new(),
            schema,
            configured: Vec::new(),
            command: Palette::new("Find a column"),
            list: List::new(),
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
            events: Vec::new(),
        };
        panel.command.list.empty_text = "No available columns".into();
        panel.list.empty_text = "No dimensions configured".into();
        panel.list.select_on_click = true;
        panel.set_configured(configured);
        if !panel.configured.is_empty() {
            panel.select(0);
        }
        panel
    }

    fn set_configured(&mut self, entries: Vec<Dimension>) {
        self.configured = entries;
        self.list
            .set_items(self.configured.iter().map(|entry| Item::new(dimension_label(entry))).collect());
        let available = self.available_columns().into_iter().map(Item::new).collect();
        self.command.set_items(available);
        self.restore_list_selection();
    }

    pub fn kind(&self) -> PivotFieldKind {
        self.kind
    }

    pub fn take_event(&mut self) -> Option<PivotPanelEvent> {
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
        let Some(dimension) = self.configured.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.custom = false;
        match dimension_editor_expression(&dimension, &self.schema) {
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
                self.emit_edit(PivotEdit::Add(Dimension::Column(column)));
            }
            PanelAction::Move(direction) => {
                if let Some(dimension) = self.selected.and_then(|index| self.configured.get(index)).cloned() {
                    self.emit_edit(PivotEdit::Move {
                        field: dimension,
                        direction,
                    });
                }
            }
            PanelAction::Remove => {
                if self.custom {
                    self.revert(focus);
                } else if let Some(dimension) = self.selected.and_then(|index| self.configured.get(index)).cloned() {
                    self.emit_edit(PivotEdit::Remove(dimension));
                }
            }
        }
    }

    fn emit_edit(&mut self, edit: PivotEdit) {
        self.input_changed();
        self.checking = true;
        self.checking_revision = self.revision;
        self.error = None;
        self.events.push(PivotPanelEvent {
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
            Dimension::expression(sql)
        };
        if let Err(error) = replacement.validate() {
            self.error = Some(error);
            self.pending_action = None;
            return;
        }
        let edit = match original {
            Some(original) => PivotEdit::Replace {
                original,
                replacement,
            },
            None => PivotEdit::Add(replacement),
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

    pub(crate) fn finish_success(&mut self, entries: Vec<Dimension>, preferred: Option<Dimension>, focus: &mut Option<WidgetId>) {
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

    pub fn focus_available(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.command.query.id);
    }

    pub(crate) fn focus_sql(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.sql.id);
    }

    fn editing(&self) -> bool {
        self.custom || self.selected.is_some()
    }

    fn cycle_focus(&mut self, backwards: bool, focus: &mut Option<WidgetId>) {
        let mut handles = vec![self.command.query.id, self.list.id];
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
        id == self.command.query.id || id == self.list.id || id == self.sql.id
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
            close: false,
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
        if *focus == Some(self.command.query.id) {
            let outcome = self.command.key(event);
            if let Some(index) = outcome.confirmed {
                if let Some(column) = self.available_columns().get(index).cloned() {
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
            true
        } else if focus == Some(self.sql.id) {
            self.sql.type_text(typed);
            self.input_changed();
            true
        } else {
            false
        }
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> PanelResult {
        match hit {
            Hit::Button(Button::PanelCustom) => self.start_custom(focus),
            Hit::Button(Button::PanelRevert) => self.revert(focus),
            Hit::Button(Button::PanelApply) => self.apply(),
            Hit::Button(Button::PanelClose) => {
                self.close();
                return PanelResult {
                    handled: true,
                    event: None,
                    close: true,
                };
            }
            Hit::Editor(id, part) if *id == self.command.query.id => {
                self.command.query.mouse_down(*part, pos, clicks, shift);
                self.focus_available(focus);
            }
            Hit::List(id, part) if *id == self.command.list.id => {
                let outcome = self.command.mouse_down(*part, pos, clicks, shift);
                self.focus_available(focus);
                if let Some(index) = outcome.confirmed
                    && let Some(column) = self.available_columns().get(index).cloned()
                {
                    self.request_action(PanelAction::Add(column), focus);
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
        self.list.mouse_drag(pos);
        if self.sql.is_dragging() {
            self.sql.mouse_drag(pos);
        }
    }

    pub fn mouse_up(&mut self) {
        self.command.mouse_up();
        self.list.mouse_up();
        self.sql.mouse_up();
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point) -> bool {
        match hit {
            Hit::List(id, _) if *id == self.command.list.id => self.command.list.wheel(delta),
            Hit::List(id, _) if *id == self.list.id => self.list.wheel(delta),
            Hit::Editor(id, _) if *id == self.sql.id => self.sql.wheel(delta),
            _ => false,
        }
    }

    /// The caret of the focused input, for the window's input method.
    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        let id = focus?;
        [&self.sql, &self.command.query].into_iter().find(|input| input.id == id)?.caret_rect()
    }

    /// Paints the manager to fill `r`.
    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let title = match self.kind {
            PivotFieldKind::GroupBy => "Group By",
            PivotFieldKind::PivotBy => "Pivot By",
        };
        let list_height = (r.h * 0.28).clamp(68.0, 260.0);
        let mut row = Row::new(r, 16.0);
        let left = row.left(r.w * 0.3);
        let right = row.rest();
        // Available columns.
        let mut column = Column::new(left, 8.0);
        canvas.text(column.next(line), "Available columns", &font.semibold(), colors::FOREGROUND, Align::Left);
        let custom = column.next(button_height(font));
        button(
            canvas,
            custom.with_w(button_width("+ Custom expression", font).min(custom.w)),
            "+ Custom expression",
            font,
            ButtonStyle::Outline,
            !self.checking,
            Hit::Button(Button::PanelCustom),
        );
        let command_area = column.rest();
        let (query_id, list_id) = (self.command.query.id, self.command.list.id);
        self.command.paint(canvas, command_area, font, focus == Some(query_id), |part| match part {
            list::Part::Query(part) => Hit::Editor(query_id, part),
            part => Hit::List(list_id, part),
        });
        // Configured dimensions and the expression editor.
        let mut column = Column::new(right, 8.0);
        let header = column.next(button_height(font));
        let mut header_row = Row::new(header, 8.0);
        let close = header_row.right(button_width("Close", font));
        button(canvas, close, "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::PanelClose));
        canvas.text(header_row.rest(), &format!("{title} order"), &font.semibold(), colors::FOREGROUND, Align::Left);
        let list_area = column.next(list_height);
        canvas.stroke_rounded(list_area, 5.0, colors::BORDER, 1.0);
        let configured_id = self.list.id;
        self.list.paint(canvas, list_area.inset(2.0), font, focus == Some(configured_id), |part| {
            Hit::List(configured_id, part)
        });
        canvas.text(column.next(line), "Alt+↑/↓ move · Delete remove", &fonts.small, colors::MUTED, Align::Left);
        let separator = column.next(1.0);
        canvas.fill(separator, colors::BORDER);
        if !self.editing() {
            canvas.text(
                column.next(line),
                "Add a column or a custom expression to edit its SQL.",
                font,
                colors::MUTED,
                Align::Left,
            );
            return;
        }
        canvas.text(column.next(line), "SQL expression", font, colors::FOREGROUND, Align::Left);
        canvas.text(
            column.next(line),
            "Use AS to name a column, or keep the default name.",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
        // The editor takes the room left above the error and the buttons.
        let reserve = button_height(font) + 8.0 + if self.error.is_some() { 78.0 } else { 0.0 };
        let editor = column.next((column.rest().h - reserve).max(70.0));
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

    /// The SQL editor's completion menu, drawn above the dialog.
    pub fn paint_overlay(&mut self, canvas: &mut Canvas<Hit>, focus: Option<WidgetId>) {
        let id = self.sql.id;
        self.sql
            .paint_completion(canvas, focus == Some(id), |part| Hit::Editor(id, part));
    }
}
