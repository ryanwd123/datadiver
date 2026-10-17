use crate::domain::actions::{launch_with_values, validate_action};
use crate::domain::documents::ViewAction;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

pub enum ActionsEditorEvent {
    Changed(Vec<ViewAction>),
    /// Run this action for the active row; it need not be saved.
    Run(ViewAction),
}

/// Adds, edits, and removes the active view's row actions.
pub struct ActionsEditor {
    pub(crate) actions: Vec<ViewAction>,
    /// Key of the action loaded into the form, or `None` when adding.
    editing: Option<String>,
    pub(crate) key: Editor,
    pub(crate) description: Editor,
    pub(crate) command: Editor,
    columns: Vec<String>,
    /// The active row's `(column, value)` pairs for previews, if there is one.
    row: Option<Vec<(String, String)>>,
    pub(crate) error: Option<String>,
    events: Vec<ActionsEditorEvent>,
}

impl ActionsEditor {
    pub fn new(actions: Vec<ViewAction>, columns: Vec<String>, row: Option<Vec<(String, String)>>) -> Self {
        Self {
            actions,
            editing: None,
            key: Editor::single_line().with_placeholder("k"),
            description: Editor::single_line().with_placeholder("Search the web"),
            command: Editor::single_line().with_placeholder("https://www.google.com/search?q={Name}"),
            columns,
            row,
            error: None,
            events: Vec::new(),
        }
    }

    pub fn take_events(&mut self) -> Vec<ActionsEditorEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn focus(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.key.id);
    }

    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }

    fn inputs(&self) -> [WidgetId; 3] {
        [self.key.id, self.description.id, self.command.id]
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        self.inputs().contains(&id)
    }

    fn input_mut(&mut self, id: WidgetId) -> Option<&mut Editor> {
        [&mut self.key, &mut self.description, &mut self.command]
            .into_iter()
            .find(|input| input.id == id)
    }

    pub(crate) fn fill_form(&mut self, action: Option<&ViewAction>, focus: &mut Option<WidgetId>) {
        self.editing = action.map(|action| action.key.clone());
        self.key.set_text(action.map(|action| action.key.clone()).unwrap_or_default());
        self.description
            .set_text(action.map(|action| action.description.clone()).unwrap_or_default());
        self.command
            .set_text(action.map(|action| action.command.clone()).unwrap_or_default());
        self.error = None;
        self.focus(focus);
    }

    fn form_action(&self) -> ViewAction {
        ViewAction {
            key: self.key.text().trim().to_owned(),
            description: self.description.text().trim().to_owned(),
            command: self.command.text().trim().to_owned(),
        }
    }

    pub(crate) fn run_form(&mut self) {
        let mut action = self.form_action();
        if action.command.is_empty() {
            self.set_error(Some("action command is required".into()));
            return;
        }
        if action.description.is_empty() {
            action.description = "Action".into();
        }
        self.events.push(ActionsEditorEvent::Run(action));
    }

    /// Example actions built on the first column, so they preview real values.
    fn examples(&self) -> [ViewAction; 3] {
        let column = self.columns.first().map_or("Column", String::as_str);
        let example = |key: &str, description: &str, command: String| ViewAction {
            key: key.into(),
            description: description.into(),
            command,
        };
        [
            example(
                "g",
                "Search the web",
                format!("https://www.google.com/search?q={{{column}}}"),
            ),
            example(
                "n",
                "Open note in Notepad",
                format!("notepad \"C:/notes/{{{column}}}.txt\""),
            ),
            example(
                "e",
                "Open folder in Explorer",
                format!("explorer \"C:/data/{{{column}}}\""),
            ),
        ]
    }

    /// Loads an example into the form as a new action, keeping its key when free.
    pub(crate) fn use_example(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        let Some(mut example) = self.examples().into_iter().nth(index) else {
            return;
        };
        if self.actions.iter().any(|action| action.key == example.key) {
            example.key.clear();
        }
        self.fill_form(Some(&example), focus);
        self.editing = None;
    }

    pub(crate) fn preview(&self) -> Option<Result<String, String>> {
        let command = self.command.text();
        if command.trim().is_empty() {
            return None;
        }
        Some(match &self.row {
            Some(row) => launch_with_values(command, row).map(|launch| launch.preview()),
            None => Err("select a row to preview".into()),
        })
    }

    pub(crate) fn save(&mut self, focus: &mut Option<WidgetId>) {
        let action = self.form_action();
        if let Err(error) = validate_action(&action, &self.actions, self.editing.as_deref()) {
            self.set_error(Some(error));
            return;
        }
        match self
            .editing
            .as_ref()
            .and_then(|key| self.actions.iter().position(|saved| saved.key == *key))
        {
            Some(index) => self.actions[index] = action,
            None => self.actions.push(action),
        }
        self.events.push(ActionsEditorEvent::Changed(self.actions.clone()));
        self.fill_form(None, focus);
    }

    pub(crate) fn delete(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        if index >= self.actions.len() {
            return;
        }
        let removed = self.actions.remove(index);
        self.events.push(ActionsEditorEvent::Changed(self.actions.clone()));
        if self.editing.as_ref() == Some(&removed.key) {
            self.fill_form(None, focus);
        }
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        let Some(id) = *focus else {
            return false;
        };
        if event.key == "tab" && !event.modifiers.control && !event.modifiers.alt {
            let inputs = self.inputs();
            if let Some(position) = inputs.iter().position(|input| *input == id) {
                let next = if event.modifiers.shift {
                    (position + inputs.len() - 1) % inputs.len()
                } else {
                    (position + 1) % inputs.len()
                };
                *focus = Some(inputs[next]);
                return true;
            }
        }
        let Some(input) = self.input_mut(id) else {
            return false;
        };
        let outcome = input.key(event);
        if outcome.submit {
            self.save(focus);
        }
        outcome.handled
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        match focus.and_then(|id| self.input_mut(id)) {
            Some(input) => {
                input.type_text(typed);
                true
            }
            None => false,
        }
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Editor(id, part) => {
                let id = *id;
                let Some(input) = self.input_mut(id) else {
                    return false;
                };
                input.mouse_down(*part, pos, clicks, shift);
                *focus = Some(id);
            }
            Hit::Button(Button::ActionRun(index)) => {
                if let Some(action) = self.actions.get(*index).cloned() {
                    self.events.push(ActionsEditorEvent::Run(action));
                }
            }
            Hit::Button(Button::ActionEdit(index)) => {
                let action = self.actions.get(*index).cloned();
                self.fill_form(action.as_ref(), focus);
            }
            Hit::Button(Button::ActionDelete(index)) => self.delete(*index, focus),
            Hit::Button(Button::ActionExample(index)) => self.use_example(*index, focus),
            Hit::Button(Button::ActionCancelEdit) => self.fill_form(None, focus),
            Hit::Button(Button::ActionRunForm) => self.run_form(),
            Hit::Button(Button::ActionSave) => self.save(focus),
            _ => return false,
        }
        true
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        for input in [&mut self.key, &mut self.description, &mut self.command] {
            if input.is_dragging() {
                input.mouse_drag(pos);
            }
        }
    }

    pub fn mouse_up(&mut self) {
        for input in [&mut self.key, &mut self.description, &mut self.command] {
            input.mouse_up();
        }
    }

    fn field_height(fonts: &Fonts) -> f32 {
        text::line_height(&fonts.small) + 4.0 + Editor::line_box_height(&fonts.ui)
    }

    pub fn height(&self, fonts: &Fonts, width: f32) -> f32 {
        // Mirrors the layout of `paint`, whose column puts GAP after each item.
        const GAP: f32 = 12.0;
        let line = text::line_height(&fonts.ui);
        let list = if self.actions.is_empty() {
            line + GAP
        } else {
            ((line + 10.0).round() + GAP) * self.actions.len() as f32
        };
        let hint = text::layout(
            HINT,
            &fonts.small,
            crate::ui::platform::TextOptions::wrapped(width),
        )
        .height;
        let optional = |shown: bool, height: f32| if shown { height + GAP } else { 0.0 };
        list + (Self::field_height(fonts) + GAP) * 2.0
            + optional(self.preview().is_some(), line)
            + hint
            + 4.0
            + optional(!self.columns.is_empty(), line)
            + line
            + GAP
            + (button_height(&fonts.small) + GAP) * self.examples().len() as f32
            + optional(self.error.is_some(), line)
            + button_height(&fonts.ui)
    }

    fn field(canvas: &mut Canvas<Hit>, r: Rect, label: &str, input: &mut Editor, fonts: &Fonts, focus: Option<WidgetId>) {
        let label_height = text::line_height(&fonts.small) + 4.0;
        canvas.text(rect(r.x, r.y, r.w, label_height), label, &fonts.small, colors::MUTED, Align::Left);
        let id = input.id;
        input.paint(
            canvas,
            rect(r.x, r.y + label_height, r.w, Editor::line_box_height(&fonts.ui)),
            &fonts.ui,
            focus == Some(id),
            |part| Hit::Editor(id, part),
        );
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let small_button = button_height(&fonts.small);
        let mut column = Column::new(r, 12.0);
        let row_height = (line + 10.0).round();
        if self.actions.is_empty() {
            canvas.text(column.next(line), "No actions yet", font, colors::MUTED, Align::Left);
        } else {
            for (index, action) in self.actions.iter().enumerate() {
                let row = column.next(row_height);
                if self.editing.as_ref() == Some(&action.key) {
                    canvas.fill_rounded(row, 4.0, colors::STRIPE);
                }
                let mut items = Row::new(row.inset_xy(8.0, (row_height - small_button) / 2.0), 8.0);
                for (label, hit) in [
                    ("Delete", Button::ActionDelete(index)),
                    ("Edit", Button::ActionEdit(index)),
                    ("Run", Button::ActionRun(index)),
                ] {
                    let r = items.right(button_width(label, &fonts.small));
                    button(canvas, r, label, &fonts.small, ButtonStyle::Ghost, true, Hit::Button(hit));
                }
                let key_width = (text::measure(&action.key, font).0 + 12.0).max(22.0);
                let key = items.left(key_width);
                canvas.stroke_rounded(key, 4.0, colors::BORDER, 1.0);
                canvas.text(key, &action.key, font, colors::FOREGROUND, Align::Center);
                let description_width = text::measure(&action.description, font).0 + 4.0;
                let description = items.left(description_width.min(items.rest().w * 0.5));
                canvas.text(description, &action.description, font, colors::FOREGROUND, Align::Left);
                canvas.text(items.rest(), &action.command, &fonts.small, colors::MUTED, Align::Left);
            }
            column.skip(0.0);
        }
        let fields = column.next(Self::field_height(fonts));
        let mut fields_row = Row::new(fields, 8.0);
        let key_area = fields_row.left(70.0);
        Self::field(canvas, key_area, "Key", &mut self.key, fonts, focus);
        let description_area = fields_row.rest();
        Self::field(canvas, description_area, "Description", &mut self.description, fonts, focus);
        let command_area = column.next(Self::field_height(fonts));
        Self::field(canvas, command_area, "Command or URL", &mut self.command, fonts, focus);
        if let Some(preview) = self.preview() {
            let (text_value, color) = match preview {
                Ok(text) => (format!("→ {text}"), colors::MUTED),
                Err(error) => (format!("Preview: {error}"), colors::DANGER),
            };
            canvas.text(column.next(line), &text_value, &fonts.small, color, Align::Left);
        }
        let hint_area = column.rest();
        let hint_height = canvas.paragraph(hint_area, HINT, &fonts.small, colors::MUTED);
        column.skip(hint_height + 4.0);
        if !self.columns.is_empty() {
            canvas.text(
                column.next(line),
                &format!("Columns: {}", self.columns.join(", ")),
                &fonts.small,
                colors::MUTED,
                Align::Left,
            );
        }
        canvas.text(column.next(line), "Examples", &fonts.small.semibold(), colors::FOREGROUND, Align::Left);
        for (index, example) in self.examples().into_iter().enumerate() {
            let row = column.next(small_button);
            let mut items = Row::new(row, 8.0);
            let use_button = items.right(button_width("Use", &fonts.small));
            button(canvas, use_button, "Use", &fonts.small, ButtonStyle::Ghost, true, Hit::Button(Button::ActionExample(index)));
            let description = items.left(text::measure(&example.description, &fonts.small).0 + 4.0);
            canvas.text(description, &example.description, &fonts.small, colors::FOREGROUND, Align::Left);
            canvas.text(items.rest(), &example.command, &fonts.small, colors::MUTED, Align::Left);
        }
        if let Some(error) = self.error.clone() {
            canvas.text(column.next(line), &error, font, colors::DANGER, Align::Left);
        }
        let buttons = column.next(button_height(font));
        let mut buttons = Row::new(buttons, 8.0);
        let save_label = if self.editing.is_some() { "Update action" } else { "Add action" };
        let save = buttons.right(button_width(save_label, font));
        button(canvas, save, save_label, font, ButtonStyle::Primary, true, Hit::Button(Button::ActionSave));
        let run = buttons.right(button_width("Run", font));
        button(canvas, run, "Run", font, ButtonStyle::Outline, true, Hit::Button(Button::ActionRunForm));
        if self.editing.is_some() {
            let cancel = buttons.right(button_width("Cancel edit", font));
            button(canvas, cancel, "Cancel edit", font, ButtonStyle::Outline, true, Hit::Button(Button::ActionCancelEdit));
        }
    }
}

const HINT: &str = "{Column} inserts the active row's value; {{ and }} insert braces. \
http(s) URLs open in the browser with values URL-encoded. Other commands start a program \
directly (no shell), with quoted arguments kept together; use cmd /C … for shell features.";

#[cfg(test)]
mod tests {
    use super::{ActionsEditor, ActionsEditorEvent, ViewAction};

    #[test]
    fn adds_rejects_duplicates_updates_and_deletes() {
        let saved = vec![ViewAction {
            key: "a".into(),
            description: "Old".into(),
            command: "echo {Name}".into(),
        }];
        let row = Some(vec![("Name".to_owned(), "Ada Lovelace".to_owned())]);
        let mut editor = ActionsEditor::new(saved, vec!["Name".into()], row);
        let mut focus = None;
        let mut emitted = Vec::new();
        let mut fill = |editor: &mut ActionsEditor, key: &str, description: &str, command: &str| {
            editor.key.set_text(key);
            editor.description.set_text(description);
            editor.command.set_text(command);
            editor.save(&mut focus);
            for event in editor.take_events() {
                if let ActionsEditorEvent::Changed(actions) = event {
                    emitted.push(actions);
                }
            }
        };
        fill(&mut editor, "a", "Clash", "echo x");
        assert!(editor.error.as_deref().unwrap().contains("already used"));
        fill(&mut editor, " g ", "Search", "https://example.com/?q={Name}");
        let first = editor.actions[0].clone();
        let mut focus = None;
        editor.fill_form(Some(&first), &mut focus);
        fill(&mut editor, "a", "Renamed", "echo {Name}");
        let mut focus = None;
        editor.delete(0, &mut focus);
        if let Some(ActionsEditorEvent::Changed(actions)) = editor.take_events().pop() {
            emitted.push(actions);
        }
        assert_eq!(emitted.len(), 3);
        assert_eq!(emitted[0].len(), 2);
        assert_eq!(emitted[0][1].key, "g");
        assert_eq!(emitted[1][0].description, "Renamed");
        assert_eq!(emitted[2].len(), 1);
        assert_eq!(emitted[2][0].key, "g");
    }

    #[test]
    fn previews_examples_and_runs_unsaved_form() {
        let saved = vec![ViewAction {
            key: "g".into(),
            description: "Taken".into(),
            command: "echo".into(),
        }];
        let row = Some(vec![("Name".to_owned(), "Ada Lovelace".to_owned())]);
        let mut editor = ActionsEditor::new(saved, vec!["Name".into()], row);
        let mut focus = None;
        assert_eq!(editor.preview(), None);
        // The example's key `g` is taken, so it is left blank.
        editor.use_example(0, &mut focus);
        assert_eq!(editor.key.text(), "");
        assert_eq!(
            editor.preview(),
            Some(Ok("https://www.google.com/search?q=Ada%20Lovelace".into()))
        );
        editor.use_example(1, &mut focus);
        assert_eq!(editor.key.text(), "n");
        assert_eq!(
            editor.preview(),
            Some(Ok("notepad \"C:/notes/Ada Lovelace.txt\"".into()))
        );
        editor.command.set_text("echo {Missing}");
        assert_eq!(
            editor.preview(),
            Some(Err("column Missing is missing".into()))
        );
        editor.run_form();
        let runs = editor
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                ActionsEditorEvent::Run(action) => Some(action),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].command, "echo {Missing}");
        assert_eq!(runs[0].description, "Open note in Notepad");
    }
}
