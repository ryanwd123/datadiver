use crate::domain::ai::{AiHeader, AiProvider, AiSettings};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, TextOptions, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, checkbox, colors};

pub enum AiSettingsEvent {
    Changed(AiSettings),
}

/// Adds, edits, tests, and removes AI providers.
pub struct AiSettingsEditor {
    pub(crate) settings: AiSettings,
    /// Name of the provider loaded into the form, or `None` when adding.
    editing: Option<String>,
    pub(crate) name: Editor,
    pub(crate) base_url: Editor,
    pub(crate) model: Editor,
    pub(crate) api_key: Editor,
    pub(crate) headers: Vec<(Editor, Editor)>,
    send_data: bool,
    pub(crate) error: Option<String>,
    /// Outcome of the last connection test: `Ok` text or an error.
    test: Option<Result<String, String>>,
    testing: bool,
    events: Vec<AiSettingsEvent>,
}

const SEND_HINT: &str = "Requests go to <base URL>/chat/completions. Always sent: table and column \
names, types, comments, and the SQL editor text; for tables named with @, row, null, and distinct \
counts. Only with data allowed: their min and max values and their most frequent or (for 25 or \
fewer) all values.";

impl AiSettingsEditor {
    pub fn new(settings: AiSettings) -> Self {
        let mut api_key = Editor::single_line().with_placeholder("sk-… or {env:OPENAI_API_KEY}");
        api_key.masked = true;
        let mut editor = Self {
            settings,
            editing: None,
            name: Editor::single_line().with_placeholder("AI Factory"),
            base_url: Editor::single_line().with_placeholder("https://openrouter.ai/api/v1"),
            model: Editor::single_line().with_placeholder("gpt-4.1"),
            api_key,
            headers: Vec::new(),
            send_data: false,
            error: None,
            test: None,
            testing: false,
            events: Vec::new(),
        };
        // With one provider there is nothing to choose, so open it for editing.
        if let [only] = editor.settings.providers.as_slice() {
            let only = only.clone();
            let mut focus = None;
            editor.fill_form(Some(&only), &mut focus);
        }
        editor
    }

    pub fn take_events(&mut self) -> Vec<AiSettingsEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn focus(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.name.id);
    }

    fn inputs(&self) -> Vec<WidgetId> {
        let mut ids = vec![self.name.id, self.model.id, self.base_url.id, self.api_key.id];
        for (name, value) in &self.headers {
            ids.push(name.id);
            ids.push(value.id);
        }
        ids
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        self.inputs().contains(&id)
    }

    fn input_mut(&mut self, id: WidgetId) -> Option<&mut Editor> {
        if let Some(input) = [&mut self.name, &mut self.base_url, &mut self.model, &mut self.api_key]
            .into_iter()
            .find(|input| input.id == id)
        {
            return Some(input);
        }
        self.headers
            .iter_mut()
            .flat_map(|(name, value)| [name, value])
            .find(|input| input.id == id)
    }

    pub(crate) fn fill_form(&mut self, provider: Option<&AiProvider>, focus: &mut Option<WidgetId>) {
        self.editing = provider.map(|provider| provider.name.clone());
        let blank = AiProvider::default();
        let provider = provider.unwrap_or(&blank);
        self.name.set_text(provider.name.clone());
        self.base_url.set_text(provider.base_url.clone());
        self.model.set_text(provider.model.clone());
        self.api_key.set_text(provider.api_key.clone());
        self.headers.clear();
        for header in &provider.headers {
            self.push_header(header);
        }
        self.send_data = provider.send_data;
        self.error = None;
        self.test = None;
        self.testing = false;
        self.focus(focus);
    }

    fn push_header(&mut self, header: &AiHeader) {
        let mut name = Editor::single_line().with_placeholder("Header name");
        name.set_text(header.name.clone());
        let mut value = Editor::single_line().with_placeholder("Value");
        value.set_text(header.value.clone());
        self.headers.push((name, value));
    }

    pub(crate) fn add_header(&mut self, focus: &mut Option<WidgetId>) {
        self.push_header(&AiHeader::default());
        if let Some((name, _)) = self.headers.last() {
            *focus = Some(name.id);
        }
    }

    pub(crate) fn form_provider(&self) -> AiProvider {
        AiProvider {
            name: self.name.text().trim().to_owned(),
            base_url: self.base_url.text().trim().to_owned(),
            model: self.model.text().trim().to_owned(),
            api_key: self.api_key.text().trim().to_owned(),
            headers: self
                .headers
                .iter()
                .map(|(name, value)| AiHeader {
                    name: name.text().trim().to_owned(),
                    value: value.text().trim().to_owned(),
                })
                .filter(|header| !header.name.is_empty() || !header.value.is_empty())
                .collect(),
            send_data: self.send_data,
        }
    }

    pub(crate) fn save(&mut self, focus: &mut Option<WidgetId>) {
        let provider = self.form_provider();
        if let Err(error) = provider.validate(&self.settings.providers, self.editing.as_deref()) {
            self.error = Some(error);
            return;
        }
        let position = self.editing.as_ref().and_then(|name| {
            self.settings
                .providers
                .iter()
                .position(|saved| &saved.name == name)
        });
        if let Some(index) = position {
            let old = std::mem::replace(&mut self.settings.providers[index], provider.clone());
            if self.settings.default_provider.as_ref() == Some(&old.name) {
                self.settings.default_provider = Some(provider.name.clone());
            }
        } else {
            self.settings.providers.push(provider.clone());
        }
        self.events.push(AiSettingsEvent::Changed(self.settings.clone()));
        self.fill_form(Some(&provider), focus);
    }

    pub(crate) fn delete(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        if index >= self.settings.providers.len() {
            return;
        }
        let removed = self.settings.providers.remove(index);
        if self.settings.default_provider.as_ref() == Some(&removed.name) {
            self.settings.default_provider = None;
        }
        self.events.push(AiSettingsEvent::Changed(self.settings.clone()));
        if self.editing.as_ref() == Some(&removed.name) {
            self.fill_form(None, focus);
        }
    }

    pub(crate) fn make_default(&mut self, index: usize) {
        let Some(provider) = self.settings.providers.get(index) else {
            return;
        };
        self.settings.default_provider = Some(provider.name.clone());
        self.events.push(AiSettingsEvent::Changed(self.settings.clone()));
    }

    /// The form as typed, saved or not, to test; `None` when it is invalid.
    pub(crate) fn begin_test(&mut self) -> Option<AiProvider> {
        let provider = self.form_provider();
        if let Err(error) = provider.validate(&[], None) {
            self.test = Some(Err(error));
            return None;
        }
        self.test = None;
        self.testing = true;
        Some(provider)
    }

    pub(crate) fn finish_test(&mut self, result: Result<String, String>) {
        self.test = Some(result);
        self.testing = false;
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
        let main = [self.name.id, self.base_url.id, self.model.id, self.api_key.id].contains(&id);
        let Some(input) = self.input_mut(id) else {
            return false;
        };
        let outcome = input.key(event);
        if outcome.submit && main {
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

    /// A click; returns whether it was used and whether a test should start.
    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> (bool, Option<AiProvider>) {
        match hit {
            Hit::Editor(id, part) => {
                let id = *id;
                let Some(input) = self.input_mut(id) else {
                    return (false, None);
                };
                input.mouse_down(*part, pos, clicks, shift);
                *focus = Some(id);
            }
            Hit::Button(Button::ProviderDefault(index)) => self.make_default(*index),
            Hit::Button(Button::ProviderEdit(index)) => {
                let provider = self.settings.providers.get(*index).cloned();
                self.fill_form(provider.as_ref(), focus);
            }
            Hit::Button(Button::ProviderDelete(index)) => self.delete(*index, focus),
            Hit::Button(Button::HeaderRemove(index)) => {
                if *index < self.headers.len() {
                    self.headers.remove(*index);
                }
            }
            Hit::Button(Button::HeaderAdd) => self.add_header(focus),
            Hit::Button(Button::ProviderSendData) => self.send_data = !self.send_data,
            Hit::Button(Button::ProviderNew) => self.fill_form(None, focus),
            Hit::Button(Button::ProviderTest) => return (true, self.begin_test()),
            Hit::Button(Button::ProviderSave) => self.save(focus),
            _ => return (false, None),
        }
        (true, None)
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        for id in self.inputs() {
            if let Some(input) = self.input_mut(id)
                && input.is_dragging()
            {
                input.mouse_drag(pos);
            }
        }
    }

    pub fn mouse_up(&mut self) {
        for id in self.inputs() {
            if let Some(input) = self.input_mut(id) {
                input.mouse_up();
            }
        }
    }

    fn field_height(fonts: &Fonts) -> f32 {
        text::line_height(&fonts.small) + 4.0 + Editor::line_box_height(&fonts.ui)
    }

    pub fn height(&self, fonts: &Fonts, width: f32) -> f32 {
        let line = text::line_height(&fonts.ui);
        let row = (line + 10.0).round();
        let providers = if self.settings.providers.is_empty() {
            text::layout(EMPTY_HINT, &fonts.ui, TextOptions::wrapped(width)).height
        } else {
            row * self.settings.providers.len() as f32
        };
        let hint = text::layout(SEND_HINT, &fonts.small, TextOptions::wrapped(width)).height;
        providers
            + 12.0
            + Self::field_height(fonts) * 3.0
            + 36.0
            + line
            + 4.0
            + (Editor::line_box_height(&fonts.ui) + 4.0) * self.headers.len() as f32
            + button_height(&fonts.small)
            + 12.0
            + line
            + 12.0
            + hint
            + 12.0
            + if self.testing || self.test.is_some() { line + 12.0 } else { 0.0 }
            + if self.error.is_some() { line + 12.0 } else { 0.0 }
            + button_height(&fonts.ui)
            + 12.0
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
        let small = &fonts.small;
        let mut column = Column::new(r, 12.0);
        if self.settings.providers.is_empty() {
            let height = canvas.paragraph(column.rest(), EMPTY_HINT, font, colors::MUTED);
            column.skip(height + 12.0);
        }
        let default_name = self.settings.active().map(|provider| provider.name.clone());
        let row_height = (line + 10.0).round();
        let providers = self.settings.providers.clone();
        for (index, provider) in providers.iter().enumerate() {
            let row = column.next(row_height);
            let editing = self.editing.as_ref() == Some(&provider.name);
            let is_default = default_name.as_ref() == Some(&provider.name);
            if editing {
                canvas.fill_rounded(row, 4.0, colors::STRIPE);
            }
            let mut items = Row::new(row.inset_xy(8.0, (row_height - button_height(small)) / 2.0), 8.0);
            for (label, hit) in [
                ("Delete", Button::ProviderDelete(index)),
                ("Edit", Button::ProviderEdit(index)),
            ] {
                let r = items.right(button_width(label, small));
                button(canvas, r, label, small, ButtonStyle::Ghost, true, Hit::Button(hit));
            }
            if !is_default {
                let r = items.right(button_width("Make default", small));
                button(canvas, r, "Make default", small, ButtonStyle::Ghost, true, Hit::Button(Button::ProviderDefault(index)));
            }
            let name = items.left(text::measure(&provider.name, font).0 + 4.0);
            canvas.text(name, &provider.name, font, colors::FOREGROUND, Align::Left);
            if is_default {
                let label = items.left(text::measure("default", small).0 + 4.0);
                canvas.text(label, "default", small, colors::MUTED, Align::Left);
            }
            canvas.text(
                items.rest(),
                &format!("{} · {}", provider.model, provider.base_url),
                small,
                colors::MUTED,
                Align::Left,
            );
        }
        let names = column.next(Self::field_height(fonts));
        let mut names = Row::new(names, 8.0);
        let half = (names.area.w - 8.0) / 2.0;
        let name_area = names.left(half);
        Self::field(canvas, name_area, "Name", &mut self.name, fonts, focus);
        let model_area = names.left(half);
        Self::field(canvas, model_area, "Model", &mut self.model, fonts, focus);
        let base = column.next(Self::field_height(fonts));
        Self::field(canvas, base, "Base URL (the part before /chat/completions)", &mut self.base_url, fonts, focus);
        let key = column.next(Self::field_height(fonts));
        Self::field(
            canvas,
            key,
            "API key · {env:NAME} reads an environment variable; leave empty for local servers",
            &mut self.api_key,
            fonts,
            focus,
        );
        canvas.text(column.next(line), "Headers", small, colors::MUTED, Align::Left);
        let input_height = Editor::line_box_height(font);
        for index in 0..self.headers.len() {
            let row = column.next(input_height);
            let mut items = Row::new(row, 8.0);
            let remove = items.right(button_width("Remove", small));
            button(
                canvas,
                rect(remove.x, remove.y + (input_height - button_height(small)) / 2.0, remove.w, button_height(small)),
                "Remove",
                small,
                ButtonStyle::Ghost,
                true,
                Hit::Button(Button::HeaderRemove(index)),
            );
            let name_area = items.left(220.0);
            let value_area = items.rest();
            let (name, value) = &mut self.headers[index];
            let (name_id, value_id) = (name.id, value.id);
            name.paint(canvas, name_area, font, focus == Some(name_id), |part| Hit::Editor(name_id, part));
            value.paint(canvas, value_area, font, focus == Some(value_id), |part| Hit::Editor(value_id, part));
        }
        let add = column.next(button_height(small));
        button(
            canvas,
            add.with_w(button_width("Add header", small)),
            "Add header",
            small,
            ButtonStyle::Outline,
            true,
            Hit::Button(Button::HeaderAdd),
        );
        let send = column.next(line + 4.0);
        checkbox(
            canvas,
            send,
            "Allow sending data to improve SQL output",
            self.send_data,
            font,
            Hit::Button(Button::ProviderSendData),
        );
        let hint_height = canvas.paragraph(column.rest(), SEND_HINT, small, colors::MUTED);
        column.skip(hint_height + 12.0);
        if self.testing {
            canvas.text(column.next(line), "Testing…", small, colors::MUTED, Align::Left);
        }
        if let Some(test) = self.test.clone() {
            let (text_value, color) = match test {
                Ok(text) => (format!("✓ {text}"), colors::MUTED),
                Err(error) => (format!("Test failed: {error}"), colors::DANGER),
            };
            canvas.text(column.next(line), &text_value, small, color, Align::Left);
        }
        if let Some(error) = self.error.clone() {
            canvas.text(column.next(line), &error, font, colors::DANGER, Align::Left);
        }
        let buttons = column.next(button_height(font));
        let mut buttons = Row::new(buttons, 8.0);
        let save_label = if self.editing.is_some() { "Update provider" } else { "Add provider" };
        let save = buttons.right(button_width(save_label, font));
        button(canvas, save, save_label, font, ButtonStyle::Primary, true, Hit::Button(Button::ProviderSave));
        let test = buttons.right(button_width("Test connection", font));
        button(canvas, test, "Test connection", font, ButtonStyle::Outline, !self.testing, Hit::Button(Button::ProviderTest));
        if self.editing.is_some() {
            let new = buttons.right(button_width("New provider", font));
            button(canvas, new, "New provider", font, ButtonStyle::Outline, true, Hit::Button(Button::ProviderNew));
        }
    }
}

const EMPTY_HINT: &str = "No providers yet. Any OpenAI-compatible endpoint works: OpenAI, \
OpenRouter, company gateways, Ollama, LM Studio.";

#[cfg(test)]
mod tests {
    use super::{AiSettingsEditor, AiSettingsEvent};
    use crate::domain::ai::{AiProvider, AiSettings};

    #[test]
    fn adds_edits_renames_defaults_and_deletes_providers() {
        let mut editor = AiSettingsEditor::new(AiSettings::default());
        let mut emitted = Vec::<AiSettings>::new();
        let collect = |editor: &mut AiSettingsEditor, emitted: &mut Vec<AiSettings>| {
            for event in editor.take_events() {
                let AiSettingsEvent::Changed(settings) = event;
                emitted.push(settings);
            }
        };
        let fill = |editor: &mut AiSettingsEditor, name: &str, headers: &[(&str, &str)]| {
            let mut focus = None;
            editor.name.set_text(name);
            editor.base_url.set_text("https://ai.example.com/v1");
            editor.model.set_text("gpt-6-sole");
            editor.api_key.set_text("{env:AI_KEY}");
            editor.headers.clear();
            for (n, v) in headers {
                editor.add_header(&mut focus);
                let (name_input, value_input) = editor.headers.last_mut().unwrap();
                name_input.set_text(*n);
                value_input.set_text(*v);
            }
            // A blank header row is dropped.
            editor.add_header(&mut focus);
            editor.save(&mut focus);
        };
        fill(
            &mut editor,
            "AI Factory",
            &[("OpenAI-Organization", "org"), ("X-AI-Factory-Client", "dd")],
        );
        collect(&mut editor, &mut emitted);
        let saved = emitted.last().unwrap().clone();
        assert_eq!(saved.providers.len(), 1);
        let provider: &AiProvider = &saved.providers[0];
        assert_eq!(provider.api_key, "{env:AI_KEY}");
        assert_eq!(provider.headers.len(), 2);
        assert_eq!(provider.headers[1].name, "X-AI-Factory-Client");

        // Saving again updates in place; a rename keeps it the default.
        editor.make_default(0);
        fill(&mut editor, "Factory", &[]);
        collect(&mut editor, &mut emitted);
        let saved = emitted.last().unwrap().clone();
        assert_eq!(saved.providers.len(), 1);
        assert_eq!(saved.default_provider.as_deref(), Some("Factory"));
        assert!(saved.providers[0].headers.is_empty());

        // A second provider with the same name is rejected.
        let mut focus = None;
        editor.fill_form(None, &mut focus);
        let count = emitted.len();
        fill(&mut editor, "Factory", &[]);
        collect(&mut editor, &mut emitted);
        assert_eq!(emitted.len(), count);
        assert!(editor.error.as_deref().unwrap().contains("already"));

        editor.delete(0, &mut focus);
        collect(&mut editor, &mut emitted);
        let saved = emitted.last().unwrap().clone();
        assert!(saved.providers.is_empty());
        assert_eq!(saved.default_provider, None);
    }
}
