use crate::domain::documents::Engine;
use crate::domain::expression_completion::{self, ExpressionCatalog};
use crate::infrastructure::duckdb_store::quote_identifier;
use crate::ui::hit::{Button, Hit};
use crate::ui::input_with_autocompletion::{self, SharedExpressionCatalog};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Modifiers, Point, Rect, rect, text};
use crate::ui::widgets::hosted_editor::HostedEditor;
use crate::ui::widgets::list::{self, Item, List, Palette};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

/// The conditions the Having manager asks to run, and the one to select
/// once they are in place.
#[derive(Clone, Debug)]
pub struct HavingPanelEvent {
    pub having: Vec<String>,
    pub preferred: Option<String>,
    pub revision: u64,
}

/// What the manager asks of its host after input.
#[derive(Default)]
pub struct HavingResult {
    pub handled: bool,
    pub event: Option<HavingPanelEvent>,
    pub close: bool,
}

#[derive(Clone, Copy)]
enum Action {
    Select(usize),
    Custom,
    Move(isize),
    Remove,
}

/// The Having manager: SQL conditions over a query's result columns, which
/// keep the groups where they hold, like SQL's HAVING.
pub struct HavingPanel {
    /// Tells this manager apart from a later one in candidate checks.
    pub id: WidgetId,
    columns: Vec<String>,
    catalog: SharedExpressionCatalog,
    configured: Vec<String>,
    pub(crate) command: Palette,
    pub(crate) list: List,
    pub(crate) sql: HostedEditor,
    selected: Option<usize>,
    custom: bool,
    initial_sql: String,
    observed_sql: String,
    revision: u64,
    checking: bool,
    checking_revision: u64,
    closed: bool,
    pending_action: Option<Action>,
    pub(crate) error: Option<String>,
    events: Vec<HavingPanelEvent>,
}

/// A condition on one line, for the list.
pub fn condition_label(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl HavingPanel {
    /// `columns` are the result's columns and their SQL types.
    pub fn new(columns: Vec<(String, String)>, configured: Vec<String>, engine: Engine) -> Self {
        let catalog: SharedExpressionCatalog = std::rc::Rc::new(std::cell::RefCell::new(ExpressionCatalog::new(
            columns.iter().map(|(name, sql_type)| (name.clone(), Some(sql_type.clone()))),
        )));
        let sql = input_with_autocompletion::expression_editor(catalog.clone(), expression_completion::dialect(engine));
        let mut panel = Self {
            id: WidgetId::new(),
            columns: columns.into_iter().map(|(name, _)| name).collect(),
            catalog,
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
            events: Vec::new(),
        };
        panel.command.list.empty_text = "No result columns".into();
        panel.command.set_items(panel.columns.iter().cloned().map(Item::new).collect());
        panel.list.empty_text = "No conditions".into();
        panel.list.select_on_click = true;
        panel.set_configured(configured);
        if !panel.configured.is_empty() {
            panel.select(0);
        }
        panel
    }

    fn set_configured(&mut self, entries: Vec<String>) {
        self.configured = entries;
        self.list
            .set_items(self.configured.iter().map(|sql| Item::new(condition_label(sql))).collect());
        self.list.select_item(self.selected);
    }

    /// The result's columns changed, such as when its query finished after
    /// the manager opened.
    pub fn set_columns(&mut self, columns: Vec<(String, String)>) {
        let names: Vec<String> = columns.iter().map(|(name, _)| name.clone()).collect();
        if names == self.columns {
            return;
        }
        *self.catalog.borrow_mut() =
            ExpressionCatalog::new(columns.iter().map(|(name, sql_type)| (name.clone(), Some(sql_type.clone()))));
        self.command.set_items(names.iter().cloned().map(Item::new).collect());
        self.columns = names;
    }

    pub fn configured(&self) -> &[String] {
        &self.configured
    }

    pub fn take_event(&mut self) -> Option<HavingPanelEvent> {
        (!self.events.is_empty()).then(|| self.events.remove(0))
    }

    fn input_changed(&mut self) {
        let sql = self.sql.text();
        if sql != self.observed_sql {
            self.observed_sql = sql;
            self.revision += 1;
            self.error = None;
        }
    }

    fn dirty(&self) -> bool {
        self.custom || (self.selected.is_some() && self.sql.text() != self.initial_sql)
    }

    fn editing(&self) -> bool {
        self.custom || self.selected.is_some()
    }

    fn load(&mut self, sql: String) {
        self.revision += 1;
        self.initial_sql = sql.clone();
        self.observed_sql = sql.clone();
        self.sql.set_text(sql);
        self.error = None;
    }

    pub(crate) fn select(&mut self, index: usize) {
        let Some(sql) = self.configured.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        self.custom = false;
        self.load(sql);
        self.list.select_item(self.selected);
    }

    fn deselect(&mut self) {
        self.selected = None;
        self.custom = false;
        self.load(String::new());
        self.list.select_item(None);
    }

    pub(crate) fn start_custom(&mut self, focus: &mut Option<WidgetId>) {
        self.request(Action::Custom, focus);
    }

    fn request(&mut self, action: Action, focus: &mut Option<WidgetId>) {
        if self.checking {
            return;
        }
        // An unapplied edit runs first, unless its condition is being removed.
        if self.dirty() && !matches!(action, Action::Remove) {
            self.pending_action = Some(action);
            self.apply();
        } else {
            self.perform(action, focus);
        }
    }

    fn perform(&mut self, action: Action, focus: &mut Option<WidgetId>) {
        match action {
            Action::Select(index) => self.select(index),
            Action::Custom => {
                self.selected = None;
                self.custom = true;
                self.load(String::new());
                self.list.select_item(None);
                *focus = Some(self.sql.id);
            }
            Action::Move(direction) => {
                let Some(index) = self.selected else { return };
                let Some(target) = index.checked_add_signed(direction).filter(|target| *target < self.configured.len()) else {
                    return;
                };
                let mut having = self.configured.clone();
                having.swap(index, target);
                let preferred = having.get(target).cloned();
                self.emit(having, preferred);
            }
            Action::Remove => {
                if self.custom {
                    self.revert(focus);
                } else if let Some(index) = self.selected {
                    let mut having = self.configured.clone();
                    having.remove(index);
                    let preferred = having.get(index.min(having.len().saturating_sub(1))).cloned();
                    self.emit(having, preferred);
                }
            }
        }
    }

    fn emit(&mut self, having: Vec<String>, preferred: Option<String>) {
        self.input_changed();
        self.checking = true;
        self.checking_revision = self.revision;
        self.error = None;
        self.events.push(HavingPanelEvent {
            having,
            preferred,
            revision: self.revision,
        });
    }

    /// Sends the editor's condition.
    pub(crate) fn apply(&mut self) {
        if self.checking || !self.editing() {
            return;
        }
        self.input_changed();
        let text = self.sql.text();
        let sql = text.trim();
        let fail = |panel: &mut Self, error: &str| {
            panel.error = Some(error.into());
            panel.pending_action = None;
        };
        if sql.is_empty() {
            return fail(self, "Write a condition, such as \"Count\" > 1");
        }
        let mut having = self.configured.clone();
        if having.iter().enumerate().any(|(index, existing)| Some(index) != self.selected && existing == sql) {
            return fail(self, "This condition is already there");
        }
        match self.selected {
            Some(index) => having[index] = sql.to_owned(),
            None => having.push(sql.to_owned()),
        }
        self.emit(having, Some(sql.to_owned()));
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

    pub(crate) fn finish_success(&mut self, having: Vec<String>, preferred: Option<String>, focus: &mut Option<WidgetId>) {
        self.checking = false;
        let next = preferred
            .and_then(|sql| having.iter().position(|saved| *saved == sql))
            .or_else(|| self.selected.and_then(|index| having.len().checked_sub(1).map(|last| index.min(last))));
        self.set_configured(having);
        match next {
            Some(index) => self.select(index),
            None => self.deselect(),
        }
        if let Some(action) = self.pending_action.take() {
            self.perform(action, focus);
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
            self.deselect();
            self.focus_available(focus);
        }
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.revision += 1;
    }

    pub fn focus_available(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.command.query.id);
    }

    /// Writes a result column's quoted name into the condition at the caret.
    fn insert_column(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        if self.checking {
            return;
        }
        let Some(column) = self.columns.get(index).cloned() else {
            return;
        };
        if !self.editing() {
            self.perform(Action::Custom, focus);
        }
        self.sql.type_text(&quote_identifier(&column));
        self.input_changed();
        *focus = Some(self.sql.id);
    }

    fn cycle_focus(&mut self, backwards: bool, focus: &mut Option<WidgetId>) {
        let mut handles = vec![self.command.query.id, self.list.id];
        if self.editing() {
            handles.push(self.sql.id);
        }
        let current = handles.iter().position(|handle| Some(*handle) == *focus).unwrap_or(0);
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
        self.list.select_item(self.selected);
        if self.selected != Some(index) || self.custom {
            self.request(Action::Select(index), focus);
        }
    }

    fn take(&mut self, handled: bool) -> HavingResult {
        HavingResult {
            handled,
            event: self.take_event(),
            close: false,
        }
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> HavingResult {
        let m = event.modifiers;
        let key = event.key.as_str();
        let sql_focused = *focus == Some(self.sql.id);
        if key == "enter" && m.control && !m.shift && !m.alt {
            self.apply();
            return self.take(true);
        }
        if key == "tab" && !m.control && !m.alt && !(sql_focused && self.sql.completion_labels().is_some()) {
            self.cycle_focus(m.shift, focus);
            return self.take(true);
        }
        if *focus == Some(self.list.id) {
            if key == "delete" && !m.modified() {
                self.request(Action::Remove, focus);
                return self.take(true);
            }
            if m.alt && matches!(key, "up" | "down") {
                self.request(Action::Move(if key == "up" { -1 } else { 1 }), focus);
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
                self.insert_column(index, focus);
                return self.take(true);
            }
            return self.take(outcome.handled);
        }
        if sql_focused {
            let handled = self.sql.key(event);
            self.input_changed();
            return self.take(handled);
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

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: Modifiers, focus: &mut Option<WidgetId>) -> HavingResult {
        let shift = modifiers.shift;
        match hit {
            Hit::Button(Button::PanelCustom) => self.start_custom(focus),
            Hit::Button(Button::PanelRevert) => self.revert(focus),
            Hit::Button(Button::PanelApply) => self.apply(),
            Hit::Button(Button::PanelClose) => {
                self.close();
                return HavingResult {
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
                if let Some(index) = outcome.confirmed {
                    self.insert_column(index, focus);
                }
            }
            Hit::List(id, part) if *id == self.list.id => {
                let outcome = self.list.mouse_down(*part, pos, clicks);
                *focus = Some(self.list.id);
                if outcome.selected {
                    self.list_selected(focus);
                }
            }
            Hit::CodeEditor(id) if *id == self.sql.id => {
                self.sql.mouse_down(pos, clicks, modifiers);
                self.input_changed();
                *focus = Some(self.sql.id);
            }
            _ => return HavingResult::default(),
        }
        self.take(true)
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        self.command.mouse_drag(pos);
        self.list.mouse_drag(pos);
        if self.sql.is_dragging() {
            self.sql.mouse_move(pos, true);
        }
    }

    /// The pointer moved without a button down: the editor shows what it is over.
    pub fn mouse_move(&mut self, pos: Point, hit: Option<&Hit>) {
        let over = hit == Some(&Hit::CodeEditor(self.sql.id));
        self.sql.mouse_move(pos, over && self.editing());
    }

    /// The window that paints the manager exists now, and the user's editor options.
    pub fn attach(&mut self, host: Option<windows::Win32::Foundation::HWND>, settings: &crate::infrastructure::json_store::EditorSettings) {
        self.sql.set_host(host);
        self.sql.set_settings(settings);
    }

    pub fn mouse_up(&mut self) {
        self.command.mouse_up();
        self.list.mouse_up();
        self.sql.mouse_up();
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, modifiers: Modifiers) -> bool {
        match hit {
            Hit::List(id, _) if *id == self.command.list.id => self.command.list.wheel(delta),
            Hit::List(id, _) if *id == self.list.id => self.list.wheel(delta),
            Hit::CodeEditor(id) if *id == self.sql.id => self.sql.wheel(delta, modifiers),
            _ => false,
        }
    }

    /// The caret of the focused input, for the window's input method.
    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        let id = focus?;
        if id == self.sql.id {
            self.sql.caret_rect()
        } else {
            self.command.query.caret_rect().filter(|_| id == self.command.query.id)
        }
    }

    /// Paints the manager to fill `r`.
    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let list_height = (r.h * 0.28).clamp(68.0, 260.0);
        let mut row = Row::new(r, 16.0);
        let left = row.left(r.w * 0.3);
        let right = row.rest();
        // The result's columns, to write into a condition.
        let mut column = Column::new(left, 8.0);
        canvas.text(column.next(line), "Result columns", &font.semibold(), colors::FOREGROUND, Align::Left);
        let custom = column.next(button_height(font));
        button(
            canvas,
            custom.with_w(button_width("+ Condition", font).min(custom.w)),
            "+ Condition",
            font,
            ButtonStyle::Outline,
            !self.checking,
            Hit::Button(Button::PanelCustom),
        );
        canvas.text(column.next(line), "Enter writes a column into the condition", &fonts.small, colors::MUTED, Align::Left);
        let command_area = column.rest();
        let (query_id, list_id) = (self.command.query.id, self.command.list.id);
        self.command.paint(canvas, command_area, font, focus == Some(query_id), |part| match part {
            list::Part::Query(part) => Hit::Editor(query_id, part),
            part => Hit::List(list_id, part),
        });
        // The conditions and the SQL editor.
        let mut column = Column::new(right, 8.0);
        let header = column.next(button_height(font));
        let mut header_row = Row::new(header, 8.0);
        let close = header_row.right(button_width("Close", font));
        button(canvas, close, "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::PanelClose));
        canvas.text(header_row.rest(), "Having conditions", &font.semibold(), colors::FOREGROUND, Align::Left);
        let list_area = column.next(list_height);
        canvas.stroke_rounded(list_area, 5.0, colors::BORDER, 1.0);
        let configured_id = self.list.id;
        self.list.paint(canvas, list_area.inset(2.0), font, focus == Some(configured_id), |part| {
            Hit::List(configured_id, part)
        });
        canvas.text(
            column.next(line),
            "Groups must meet every condition · Alt+↑/↓ move · Delete remove",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
        let separator = column.next(1.0);
        canvas.fill(separator, colors::BORDER);
        if !self.editing() {
            canvas.text(
                column.next(line),
                "Add a condition to keep only the groups that meet it.",
                font,
                colors::MUTED,
                Align::Left,
            );
            if let Some(error) = self.error.clone() {
                canvas.paragraph(column.rest(), &error, &fonts.small, colors::DANGER);
            }
            return;
        }
        canvas.text(column.next(line), "SQL condition", font, colors::FOREGROUND, Align::Left);
        canvas.text(
            column.next(line),
            "Uses the result's columns, after grouping: \"Count\" > 1 AND \"Sum of sales\" >= 100",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
        // The editor takes the room left above the error and the buttons.
        let reserve = button_height(font) + 8.0 + if self.error.is_some() { 78.0 } else { 0.0 };
        let editor = column.next((column.rest().h - reserve).max(70.0));
        let sql_id = self.sql.id;
        canvas.stroke_rounded(editor, 4.0, if focus == Some(sql_id) { colors::FOCUS } else { colors::INPUT_BORDER }, 1.0);
        self.sql.paint(canvas, editor.inset(1.0), &fonts.mono, focus == Some(sql_id), Hit::CodeEditor(sql_id));
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
    pub fn paint_overlay(&mut self, canvas: &mut Canvas<Hit>) {
        if self.editing() {
            let id = self.sql.id;
            self.sql.paint_popups(canvas, Hit::CodeEditor(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(configured: &[&str]) -> HavingPanel {
        let columns = ["region", "Count"].map(|name| (name.to_owned(), "BIGINT".to_owned()));
        HavingPanel::new(columns.to_vec(), configured.iter().map(|sql| sql.to_string()).collect(), Engine::DuckDb)
    }

    #[test]
    fn a_column_is_written_into_a_new_condition_and_applied() {
        let mut panel = panel(&[]);
        let mut focus = None;
        panel.focus_available(&mut focus);
        panel.type_text("cou", focus);
        panel.key(&KeyEvent::parse("enter").unwrap(), &mut focus);
        assert_eq!(focus, Some(panel.sql.id));
        assert_eq!(panel.sql.text(), "\"Count\"");
        panel.type_text(" > 1", focus);
        let event = panel.key(&KeyEvent::parse("ctrl-enter").unwrap(), &mut focus).event.unwrap();
        assert_eq!(event.having, ["\"Count\" > 1"]);
        assert!(panel.can_finish(event.revision));
        panel.finish_success(event.having, event.preferred, &mut focus);
        assert_eq!(panel.selected, Some(0));
    }

    #[test]
    fn blank_and_repeated_conditions_are_refused_and_errors_keep_the_text() {
        let mut panel = panel(&["\"Count\" > 1"]);
        let mut focus = None;
        panel.start_custom(&mut focus);
        panel.apply();
        assert!(panel.take_event().is_none() && panel.error.is_some());
        panel.sql.set_text("\"Count\" > 1");
        panel.apply();
        assert!(panel.take_event().is_none() && panel.error.is_some());
        panel.sql.set_text("missing > 1");
        panel.apply();
        let event = panel.take_event().unwrap();
        assert_eq!(event.having, ["\"Count\" > 1", "missing > 1"]);
        panel.finish_error(event.revision, "Binder Error: missing".into());
        assert_eq!(panel.sql.text(), "missing > 1");
        assert_eq!(panel.configured(), ["\"Count\" > 1"]);
    }

    #[test]
    fn delete_and_alt_arrows_edit_the_list() {
        let mut panel = panel(&["a > 1", "b > 2"]);
        let mut focus = Some(panel.list.id);
        let moved = panel.key(&KeyEvent::parse("alt-down").unwrap(), &mut focus).event.unwrap();
        assert_eq!(moved.having, ["b > 2", "a > 1"]);
        panel.finish_success(moved.having, moved.preferred, &mut focus);
        assert_eq!(panel.selected, Some(1));
        let removed = panel.key(&KeyEvent::parse("delete").unwrap(), &mut focus).event.unwrap();
        assert_eq!(removed.having, ["b > 2"]);
    }
}
