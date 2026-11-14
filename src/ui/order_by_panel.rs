use crate::domain::documents::{Engine, SortKey};
use crate::domain::expression_completion::{self, ExpressionCatalog};
use crate::infrastructure::duckdb_store::quote_identifier;
use crate::ui::hit::{Button, Hit};
use crate::ui::input_with_autocompletion::{self, SharedExpressionCatalog};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Modifiers, Point, Rect, rect, text};
use crate::ui::widgets::hosted_editor::HostedEditor;
use crate::ui::widgets::list::{self, Item, List, Palette};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

/// A new sort the manager asks for. Its host runs it and reports back with
/// [`OrderByPanel::finish_success`] or [`OrderByPanel::finish_error`].
#[derive(Clone, Debug, PartialEq)]
pub struct OrderByEvent {
    pub sort: Vec<SortKey>,
}

/// What the manager asks of its host after input.
#[derive(Default)]
pub struct OrderByResult {
    pub handled: bool,
    pub event: Option<OrderByEvent>,
    pub close: bool,
}

#[derive(Clone, Copy)]
enum Action {
    Select(usize),
    Custom,
    Move(isize),
    Remove,
    Direction(Option<bool>),
}

/// The Order By manager: columns and SQL expressions to sort by, in priority
/// order, each ascending or descending.
pub struct OrderByPanel {
    columns: Vec<String>,
    configured: Vec<SortKey>,
    pub(crate) command: Palette,
    pub(crate) list: List,
    pub(crate) sql: HostedEditor,
    selected: Option<usize>,
    custom: bool,
    initial_sql: String,
    checking: bool,
    /// The key to select once the sort being checked is in place.
    preferred: Option<SortKey>,
    pub(crate) error: Option<String>,
    events: Vec<OrderByEvent>,
}

fn key_label(key: &SortKey) -> String {
    let direction = if key.descending { "Descending" } else { "Ascending" };
    format!("{} · {direction}", key.target_label())
}

impl OrderByPanel {
    /// `columns` are the result's columns and their SQL types.
    pub fn new(columns: Vec<(String, String)>, configured: Vec<SortKey>, engine: Engine) -> Self {
        let catalog: SharedExpressionCatalog = std::rc::Rc::new(std::cell::RefCell::new(ExpressionCatalog::new(
            columns.iter().map(|(name, sql_type)| (name.clone(), Some(sql_type.clone()))),
        )));
        let sql = input_with_autocompletion::expression_editor(catalog, expression_completion::dialect(engine));
        let mut panel = Self {
            columns: columns.into_iter().map(|(name, _)| name).collect(),
            configured: Vec::new(),
            command: Palette::new("Find a column"),
            list: List::new(),
            sql,
            selected: None,
            custom: false,
            initial_sql: String::new(),
            checking: false,
            preferred: None,
            error: None,
            events: Vec::new(),
        };
        panel.command.list.empty_text = "No available columns".into();
        panel.list.empty_text = "No sort keys configured".into();
        panel.list.select_on_click = true;
        panel.set_configured(configured);
        if !panel.configured.is_empty() {
            panel.select(0);
        }
        panel
    }

    fn available(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| !self.configured.iter().any(|key| key.is_column(column)))
            .cloned()
            .collect()
    }

    fn set_configured(&mut self, keys: Vec<SortKey>) {
        self.configured = keys;
        self.list
            .set_items(self.configured.iter().map(|key| Item::new(key_label(key))).collect());
        let available = self.available().into_iter().map(Item::new).collect();
        self.command.set_items(available);
        self.list.select_item(self.selected);
    }

    pub fn take_event(&mut self) -> Option<OrderByEvent> {
        (!self.events.is_empty()).then(|| self.events.remove(0))
    }

    pub fn configured(&self) -> &[SortKey] {
        &self.configured
    }

    pub fn is_checking(&self) -> bool {
        self.checking
    }

    /// The SQL the editor shows for `key`: a column as its quoted name.
    fn editor_text(key: &SortKey) -> String {
        if key.sql { key.column.clone() } else { quote_identifier(&key.column) }
    }

    fn load(&mut self, sql: String) {
        self.initial_sql = sql.clone();
        self.sql.set_text(sql);
        self.error = None;
    }

    pub(crate) fn select(&mut self, index: usize) {
        let Some(key) = self.configured.get(index) else {
            return;
        };
        let sql = Self::editor_text(key);
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

    fn editing(&self) -> bool {
        self.custom || self.selected.is_some()
    }

    fn dirty(&self) -> bool {
        self.custom || (self.selected.is_some() && self.sql.text() != self.initial_sql)
    }

    /// The key the editor's text describes, keeping `descending`. A quoted
    /// column name sorts by that column.
    fn edited_key(&self, descending: bool) -> Result<SortKey, String> {
        let text = self.sql.text();
        let sql = text.trim();
        if sql.is_empty() {
            return Err("Write an expression to sort by".into());
        }
        if let Some(column) = self.columns.iter().find(|column| quote_identifier(column) == sql) {
            return Ok(SortKey::column(column.clone(), descending));
        }
        Ok(SortKey::expression(sql, descending))
    }

    /// The configured keys with the editor's change, and where the edited key is.
    fn with_edit(&self) -> Result<(Vec<SortKey>, Option<usize>), String> {
        let mut keys = self.configured.clone();
        if !self.dirty() {
            return Ok((keys, self.selected));
        }
        match self.selected {
            Some(index) => {
                let key = self.edited_key(keys[index].descending)?;
                if keys.iter().enumerate().any(|(other, existing)| other != index && existing.same_target(&key)) {
                    return Err("The sort already has this key".into());
                }
                keys[index] = key;
                Ok((keys, Some(index)))
            }
            None => {
                let key = self.edited_key(false)?;
                if keys.iter().any(|existing| existing.same_target(&key)) {
                    return Err("The sort already has this key".into());
                }
                keys.push(key);
                let index = keys.len() - 1;
                Ok((keys, Some(index)))
            }
        }
    }

    fn emit(&mut self, sort: Vec<SortKey>, preferred: Option<SortKey>) {
        self.checking = true;
        self.error = None;
        self.preferred = preferred;
        self.events.push(OrderByEvent { sort });
    }

    /// Sends the editor's change.
    pub(crate) fn apply(&mut self) {
        if self.checking || !self.dirty() {
            return;
        }
        match self.with_edit() {
            Ok((keys, index)) => {
                let preferred = index.and_then(|index| keys.get(index).cloned());
                self.emit(keys, preferred);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn add_column(&mut self, column: String) {
        if self.checking {
            return;
        }
        // An unapplied edit goes along with the new key.
        let (mut keys, _) = match self.with_edit() {
            Ok(edited) => edited,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let key = SortKey::column(column, false);
        if keys.iter().any(|existing| existing.same_target(&key)) {
            return;
        }
        keys.push(key.clone());
        self.emit(keys, Some(key));
    }

    fn act(&mut self, action: Action, focus: &mut Option<WidgetId>) {
        if self.checking {
            return;
        }
        match action {
            Action::Select(index) => {
                if self.dirty() {
                    match self.with_edit() {
                        Ok((keys, _)) => {
                            let preferred = keys.get(index).cloned();
                            self.emit(keys, preferred);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.list.select_item(self.selected);
                        }
                    }
                } else {
                    self.select(index);
                }
            }
            Action::Custom => {
                if self.dirty() {
                    self.apply();
                    return;
                }
                self.selected = None;
                self.custom = true;
                self.load(String::new());
                self.list.select_item(None);
                *focus = Some(self.sql.id);
            }
            // An unapplied edit is dropped with its key.
            Action::Remove => {
                if self.custom {
                    self.revert(focus);
                } else if let Some(index) = self.selected {
                    let mut keys = self.configured.clone();
                    keys.remove(index);
                    let preferred = keys.get(index.min(keys.len().saturating_sub(1))).cloned();
                    self.emit(keys, preferred);
                }
            }
            Action::Move(direction) => {
                let Ok((mut keys, Some(index))) = self.with_edit().map_err(|error| self.error = Some(error)) else {
                    return;
                };
                let Some(target) = index.checked_add_signed(direction).filter(|target| *target < keys.len()) else {
                    return;
                };
                keys.swap(index, target);
                let preferred = keys.get(target).cloned();
                self.emit(keys, preferred);
            }
            Action::Direction(descending) => {
                let Ok((mut keys, Some(index))) = self.with_edit().map_err(|error| self.error = Some(error)) else {
                    return;
                };
                let key = &mut keys[index];
                key.descending = descending.unwrap_or(!key.descending);
                let preferred = Some(key.clone());
                if keys != self.configured {
                    self.emit(keys, preferred);
                }
            }
        }
    }

    /// The sort the host ran, which is now the view's.
    pub(crate) fn finish_success(&mut self, sort: Vec<SortKey>) {
        let checked = std::mem::take(&mut self.checking);
        let preferred = self.preferred.take();
        if !checked && sort == self.configured {
            return;
        }
        // A sort changed elsewhere, such as from a column header, keeps an edit in progress.
        let keep_edit = !checked && self.dirty();
        let selected_key = self.selected.and_then(|index| self.configured.get(index).cloned());
        let target = preferred.or(selected_key);
        let next = target
            .and_then(|target| sort.iter().position(|key| key.same_target(&target)))
            .or_else(|| self.selected.and_then(|index| sort.len().checked_sub(1).map(|last| index.min(last))));
        self.set_configured(sort);
        if keep_edit {
            self.selected = next.filter(|_| !self.custom);
            self.list.select_item(self.selected);
            return;
        }
        match next {
            Some(index) => self.select(index),
            None => self.deselect(),
        }
    }

    /// The sort the host ran failed; the view keeps its previous sort.
    pub(crate) fn finish_error(&mut self, error: String) {
        self.checking = false;
        self.preferred = None;
        self.error = Some(error);
    }

    pub(crate) fn revert(&mut self, focus: &mut Option<WidgetId>) {
        if self.checking {
            return;
        }
        if let Some(index) = self.selected {
            self.select(index);
        } else if !self.configured.is_empty() {
            self.select(0);
        } else {
            self.deselect();
            self.focus_available(focus);
        }
    }

    pub fn focus_available(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.command.query.id);
    }

    pub(crate) fn start_custom(&mut self, focus: &mut Option<WidgetId>) {
        self.act(Action::Custom, focus);
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
            self.act(Action::Select(index), focus);
        }
    }

    fn take(&mut self, handled: bool) -> OrderByResult {
        OrderByResult {
            handled,
            event: self.take_event(),
            close: false,
        }
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> OrderByResult {
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
            let action = match key {
                "enter" if !m.modified() => Some(Action::Direction(None)),
                "delete" if !m.modified() => Some(Action::Remove),
                "left" | "[" if !m.modified() => Some(Action::Direction(Some(false))),
                "right" | "]" if !m.modified() => Some(Action::Direction(Some(true))),
                "up" | "down" if m.alt => Some(Action::Move(if key == "up" { -1 } else { 1 })),
                _ => None,
            };
            if let Some(action) = action {
                if self.selected.is_some() || matches!(action, Action::Remove) {
                    self.act(action, focus);
                }
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
                if let Some(column) = self.available().get(index).cloned() {
                    self.add_column(column);
                }
                return self.take(true);
            }
            return self.take(outcome.handled);
        }
        if sql_focused {
            let handled = self.sql.key(event);
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
            true
        } else {
            false
        }
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: Modifiers, focus: &mut Option<WidgetId>) -> OrderByResult {
        let shift = modifiers.shift;
        match hit {
            Hit::Button(Button::PanelCustom) => self.start_custom(focus),
            Hit::Button(Button::PanelRevert) => self.revert(focus),
            Hit::Button(Button::PanelApply) => self.apply(),
            Hit::Button(Button::PanelClose) => {
                return OrderByResult {
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
                    && let Some(column) = self.available().get(index).cloned()
                {
                    self.add_column(column);
                }
            }
            Hit::List(id, part) if *id == self.list.id => {
                let outcome = self.list.mouse_down(*part, pos, clicks);
                *focus = Some(self.list.id);
                if outcome.selected {
                    self.list_selected(focus);
                } else if outcome.confirmed.is_some() && self.selected == outcome.confirmed {
                    // A double click turns the key around.
                    self.act(Action::Direction(None), focus);
                }
            }
            Hit::CodeEditor(id) if *id == self.sql.id => {
                self.sql.mouse_down(pos, clicks, modifiers);
                *focus = Some(self.sql.id);
            }
            _ => return OrderByResult::default(),
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
        // The sort keys and the expression editor.
        let mut column = Column::new(right, 8.0);
        let header = column.next(button_height(font));
        let mut header_row = Row::new(header, 8.0);
        let close = header_row.right(button_width("Close", font));
        button(canvas, close, "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::PanelClose));
        canvas.text(header_row.rest(), "Sort priority", &font.semibold(), colors::FOREGROUND, Align::Left);
        let list_area = column.next(list_height);
        canvas.stroke_rounded(list_area, 5.0, colors::BORDER, 1.0);
        let configured_id = self.list.id;
        self.list.paint(canvas, list_area.inset(2.0), font, focus == Some(configured_id), |part| {
            Hit::List(configured_id, part)
        });
        canvas.text(
            column.next(line),
            "[ / ← ascending · ] / → descending · Enter or double-click flips · Alt+↑/↓ move · Delete remove",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
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
            if let Some(error) = self.error.clone() {
                canvas.paragraph(column.rest(), &error, &fonts.small, colors::DANGER);
            }
            return;
        }
        canvas.text(column.next(line), "SQL expression", font, colors::FOREGROUND, Align::Left);
        canvas.text(
            column.next(line),
            "Rows sort by its value, such as CASE status WHEN 'open' THEN 0 ELSE 1 END.",
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
        button(canvas, revert, "Revert", font, ButtonStyle::Outline, !self.checking, Hit::Button(Button::PanelRevert));
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

    fn panel(configured: Vec<SortKey>) -> OrderByPanel {
        let columns = ["a", "b", "c"].map(|name| (name.to_owned(), "INTEGER".to_owned()));
        OrderByPanel::new(columns.to_vec(), configured, Engine::DuckDb)
    }

    fn press(panel: &mut OrderByPanel, key: &str, focus: &mut Option<WidgetId>) -> Option<OrderByEvent> {
        panel.key(&KeyEvent::parse(key).unwrap(), focus).event
    }

    #[test]
    fn keys_edit_the_selected_sort_key() {
        let mut panel = panel(vec![SortKey::column("b", false)]);
        let mut focus = None;
        panel.focus_available(&mut focus);
        let added = press(&mut panel, "enter", &mut focus).unwrap();
        assert_eq!(added.sort, [SortKey::column("b", false), SortKey::column("a", false)]);
        panel.finish_success(added.sort);
        // The added key is selected, and its SQL is in the editor.
        assert_eq!(panel.sql.text(), "\"a\"");
        panel.key(&KeyEvent::parse("tab").unwrap(), &mut focus);
        assert_eq!(focus, Some(panel.list.id));
        let descending = press(&mut panel, "]", &mut focus).unwrap();
        assert_eq!(descending.sort[1], SortKey::column("a", true));
        panel.finish_success(descending.sort);
        let moved = press(&mut panel, "alt-up", &mut focus).unwrap();
        assert_eq!(moved.sort, [SortKey::column("a", true), SortKey::column("b", false)]);
    }

    #[test]
    fn the_editor_turns_a_key_into_an_expression_and_back() {
        let mut panel = panel(vec![SortKey::column("a", true)]);
        let mut focus = None;
        panel.focus_sql_for_test(&mut focus);
        panel.sql.set_text("a % 2");
        panel.apply();
        let event = panel.take_event().unwrap();
        assert_eq!(event.sort, [SortKey::expression("a % 2", true)]);
        // Nothing else is sent while the sort is being checked.
        panel.apply();
        assert!(panel.take_event().is_none());
        panel.finish_error("Binder Error".into());
        assert_eq!(panel.error.as_deref(), Some("Binder Error"));
        assert_eq!(panel.sql.text(), "a % 2");
        panel.sql.set_text("\"b\"");
        panel.apply();
        assert_eq!(panel.take_event().unwrap().sort, [SortKey::column("b", true)]);
    }

    #[test]
    fn a_custom_expression_is_added_ascending_and_duplicates_are_refused() {
        let mut panel = panel(vec![SortKey::expression("a + b", false)]);
        let mut focus = None;
        panel.start_custom(&mut focus);
        assert_eq!(focus, Some(panel.sql.id));
        panel.sql.set_text("a + b");
        panel.apply();
        assert!(panel.take_event().is_none());
        assert!(panel.error.is_some());
        panel.sql.set_text("  -c ");
        panel.apply();
        let event = panel.take_event().unwrap();
        assert_eq!(event.sort[1], SortKey::expression("-c", false));
        panel.finish_success(event.sort);
        assert_eq!(panel.selected, Some(1));
        // Delete on the list removes the selected key.
        panel.key(&KeyEvent::parse("tab").unwrap(), &mut focus);
        panel.key(&KeyEvent::parse("tab").unwrap(), &mut focus);
        assert_eq!(focus, Some(panel.list.id));
        let removed = press(&mut panel, "delete", &mut focus).unwrap();
        assert_eq!(removed.sort, [SortKey::expression("a + b", false)]);
    }

    #[test]
    fn a_sort_changed_elsewhere_keeps_an_edit_in_progress() {
        let mut panel = panel(vec![SortKey::column("a", false)]);
        panel.sql.set_text("a * 2");
        panel.finish_success(vec![SortKey::column("a", false), SortKey::column("c", true)]);
        assert_eq!(panel.configured().len(), 2);
        assert_eq!(panel.sql.text(), "a * 2");
        assert_eq!(panel.selected, Some(0));
    }

    impl OrderByPanel {
        fn focus_sql_for_test(&self, focus: &mut Option<WidgetId>) {
            *focus = Some(self.sql.id);
        }
    }
}
