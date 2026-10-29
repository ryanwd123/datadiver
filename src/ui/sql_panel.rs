use std::ops::Range;
use std::sync::{Arc, RwLock};

use super::column_completion::{ColumnCompletion, ColumnSyntax};
use super::input_with_autocompletion;
use crate::domain::ai::{AiEvent, SqlCheck, SqlProposal, insertion, relation_name};
use crate::domain::sql_editor::{Relation, current_statement};
use crate::infrastructure::path_completion::PathRoots;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Color, Font, Point, Rect, TextOptions, point, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors, scroll};

pub enum SqlPanelEvent {
    Changed(String),
    AutoSave(bool),
    AutoRun(bool),
    Save,
    Run(bool),
    Stop,
    Close,
    GridFocus,
    Ask(SqlAsk),
    /// The ask strip opened; the view says which provider it will use.
    AskOpened,
}

/// The provider a request will go to, shown in the ask strip.
#[derive(Clone)]
pub struct AskProvider {
    pub name: String,
    pub model: String,
    pub send_data: bool,
}

/// A request for SQL from the ask strip.
pub struct SqlAsk {
    pub prompt: String,
    /// SQL selected in the editor, to be rewritten.
    pub selection: Option<String>,
    pub editor_text: String,
    pub revision: u64,
}

pub enum AskState {
    Idle,
    Working(String),
    Proposal(SqlProposal),
    Failed(String),
}

#[derive(Clone, Copy)]
enum AskKey {
    Accept,
    Run,
    Dismiss,
}

/// Where a proposal goes: the selection or cursor when it was asked for,
/// while the text is unchanged.
struct AskTarget {
    range: Range<usize>,
    text: String,
}

/// Find and replace in the SQL editor.
pub(crate) struct Find {
    pub(crate) query: Editor,
    pub(crate) replacement: Editor,
    pub(crate) replace: bool,
    pub(crate) case_sensitive: bool,
}

pub struct SqlPanel {
    pub editor: Editor,
    pub catalog: Arc<RwLock<Vec<Relation>>>,
    pub path_roots: Arc<RwLock<PathRoots>>,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
    pub auto_save: bool,
    pub auto_run: bool,
    pub startup_script: bool,
    pub ask_open: bool,
    pub ask_input: Editor,
    /// The `@` table list under the prompt.
    pub mentions: ColumnCompletion,
    /// `None` until the view looks it up, or when there is no provider.
    pub ask_provider: Option<AskProvider>,
    /// The database relations belong to unless qualified, for their SQL names.
    pub default_catalog: String,
    pub ask: AskState,
    ask_target: Option<AskTarget>,
    ask_revision: u64,
    /// The steps of the latest request, shown when `show_log` is on.
    pub ask_log: Vec<AiEvent>,
    ask_endpoint: String,
    pub show_log: bool,
    log_scroll: scroll::Scroll,
    log_area: Rect,
    log_height: f32,
    proposal_scroll: scroll::Scroll,
    pub(crate) find: Option<Find>,
    events: Vec<SqlPanelEvent>,
}

impl SqlPanel {
    pub fn new(text: String) -> Self {
        let catalog = Arc::new(RwLock::new(Vec::new()));
        let path_roots = Arc::new(RwLock::new(PathRoots::default()));
        let mut editor = input_with_autocompletion::sql_catalog(catalog.clone(), path_roots.clone());
        editor.set_text(text);
        editor.set_selection(0..0);
        Self {
            editor,
            catalog,
            path_roots,
            busy: false,
            status: "Ctrl+Enter · selection or current statement".into(),
            error: None,
            auto_save: false,
            auto_run: false,
            startup_script: false,
            ask_open: false,
            ask_input: Editor::single_line().with_placeholder("Describe the query you want, then Enter"),
            mentions: ColumnCompletion::new(ColumnSyntax::Mention),
            ask_provider: None,
            default_catalog: String::new(),
            ask: AskState::Idle,
            ask_target: None,
            ask_revision: 0,
            ask_log: Vec::new(),
            ask_endpoint: String::new(),
            show_log: false,
            log_scroll: scroll::Scroll::default(),
            log_area: Rect::default(),
            log_height: 0.0,
            proposal_scroll: scroll::Scroll::default(),
            find: None,
            events: Vec::new(),
        }
    }

    pub fn take_events(&mut self) -> Vec<SqlPanelEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        id == self.editor.id
            || id == self.ask_input.id
            || self
                .find
                .as_ref()
                .is_some_and(|find| id == find.query.id || id == find.replacement.id)
    }

    /// Shows the ask strip and focuses its prompt.
    pub fn open_ask(&mut self, focus: &mut Option<WidgetId>) {
        self.ask_open = true;
        let names = self.mention_names();
        self.mentions.set_columns(names);
        self.events.push(SqlPanelEvent::AskOpened);
        *focus = Some(self.ask_input.id);
    }

    pub fn submit_ask(&mut self) {
        let prompt = self.ask_input.text().trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        let text = self.editor.text().to_owned();
        let range = self.editor.selected_range();
        let selection = (!range.is_empty()).then(|| text[range.clone()].to_owned());
        self.ask_target = Some(AskTarget {
            range,
            text: text.clone(),
        });
        self.ask_revision += 1;
        self.ask = AskState::Working("Writing SQL…".into());
        self.ask_log.clear();
        self.ask_endpoint.clear();
        self.events.push(SqlPanelEvent::Ask(SqlAsk {
            prompt,
            selection,
            editor_text: text,
            revision: self.ask_revision,
        }));
    }

    pub fn set_ask_working(&mut self, revision: u64, text: String, endpoint: String) {
        if revision == self.ask_revision && matches!(self.ask, AskState::Working(_)) {
            self.ask = AskState::Working(text);
            self.ask_endpoint = endpoint;
        }
    }

    /// Replaces the log of request `revision` with `log`, following new steps.
    pub fn set_ask_log(&mut self, revision: u64, log: Vec<AiEvent>) -> bool {
        if revision != self.ask_revision || log.len() == self.ask_log.len() {
            return false;
        }
        self.ask_log = log;
        self.log_scroll.offset = f32::MAX;
        true
    }

    /// Shows the outcome of request `revision`, unless it was cancelled or replaced.
    pub fn finish_ask(&mut self, revision: u64, result: Result<SqlProposal, String>) {
        if revision != self.ask_revision || !matches!(self.ask, AskState::Working(_)) {
            return;
        }
        self.ask = match result {
            Ok(proposal) => AskState::Proposal(proposal),
            Err(error) => AskState::Failed(error),
        };
    }

    /// Puts the proposal in the editor, selected, so Ctrl+Enter runs just it.
    pub fn accept_ask(&mut self, focus: &mut Option<WidgetId>) -> bool {
        let AskState::Proposal(proposal) = &self.ask else {
            return false;
        };
        let sql = proposal.sql.clone();
        let current = self.editor.text().to_owned();
        let range = match self.ask_target.take() {
            Some(target) if target.text == current => target.range,
            _ => self.editor.selected_range(),
        };
        let (text, inner) = insertion(&current, range.clone(), &sql);
        self.editor.replace(range.clone(), &text);
        self.editor
            .set_selection(range.start + inner.start..range.start + inner.end);
        self.events.push(SqlPanelEvent::Changed(self.editor.text().to_owned()));
        self.ask = AskState::Idle;
        self.ask_open = false;
        self.ask_input.set_text("");
        self.focus(focus);
        true
    }

    /// Tables as `@` mentions name them: the default database's first.
    fn mention_names(&self) -> Vec<String> {
        let catalog = self.catalog.read().map(|c| c.clone()).unwrap_or_default();
        let (mut own, mut other): (Vec<_>, Vec<_>) = catalog
            .iter()
            .map(|r| {
                (
                    r.catalog == self.default_catalog,
                    relation_name(&r.catalog, &r.schema, &r.name, &self.default_catalog),
                )
            })
            .partition(|(own, _)| *own);
        own.append(&mut other);
        own.into_iter().map(|(_, name)| name).collect()
    }

    /// Handles a key typed in the ask prompt; false when the prompt is not
    /// focused or the key does nothing there.
    fn ask_key(&mut self, key: AskKey, focus: &mut Option<WidgetId>) -> bool {
        if *focus != Some(self.ask_input.id) {
            return false;
        }
        // The open `@` list takes Tab and Escape first.
        if self.mentions.is_open() {
            match key {
                AskKey::Accept => return self.mentions.accept(None, &mut self.ask_input),
                AskKey::Dismiss => return self.mentions.close(),
                AskKey::Run => {}
            }
        }
        match key {
            AskKey::Dismiss => {
                self.dismiss_ask(focus);
                true
            }
            AskKey::Accept => self.accept_ask(focus),
            // Without a proposal Ctrl+Enter runs the editor's SQL as usual.
            AskKey::Run => {
                let accepted = self.accept_ask(focus);
                if accepted && !self.busy {
                    self.events.push(SqlPanelEvent::Run(false));
                }
                accepted
            }
        }
    }

    /// Escape: cancels a request or drops its outcome, then closes the strip.
    pub fn dismiss_ask(&mut self, focus: &mut Option<WidgetId>) {
        if matches!(self.ask, AskState::Idle) {
            self.ask_open = false;
            self.focus(focus);
        } else {
            self.ask_revision += 1;
            self.ask = AskState::Idle;
            self.ask_target = None;
        }
    }

    pub fn focus(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.editor.id);
    }

    pub fn sql_to_run(&self, all: bool) -> String {
        let text = self.editor.text().to_owned();
        if all {
            return text;
        }
        let selection = self.editor.selected_range();
        if !selection.is_empty() {
            return text[selection].to_string();
        }
        current_statement(&text, self.editor.cursor())
            .map(|range| text[range].to_string())
            .unwrap_or_default()
    }

    /// Replaces the SQL as if the user had typed it.
    #[cfg(test)]
    pub fn edit(&mut self, text: &str) {
        self.editor.set_text(text);
        self.editor_changed();
    }

    fn editor_changed(&mut self) {
        self.events.push(SqlPanelEvent::Changed(self.editor.text().to_owned()));
    }

    fn open_find(&mut self, replace: bool, focus: &mut Option<WidgetId>) {
        let selected = self.editor.selected_text().to_owned();
        let find = self.find.get_or_insert_with(|| Find {
            query: Editor::single_line().with_placeholder("Find"),
            replacement: Editor::single_line().with_placeholder("Replace"),
            replace: false,
            case_sensitive: false,
        });
        find.replace |= replace;
        if !selected.is_empty() && !selected.contains('\n') {
            find.query.set_text(selected);
        }
        find.query.select_all();
        *focus = Some(find.query.id);
    }

    /// Byte ranges of the find query in the editor's text.
    fn matches(&self) -> Vec<Range<usize>> {
        let Some(find) = &self.find else {
            return Vec::new();
        };
        let query = find.query.text();
        if query.is_empty() {
            return Vec::new();
        }
        let text = self.editor.text();
        if find.case_sensitive {
            text.match_indices(query).map(|(start, found)| start..start + found.len()).collect()
        } else {
            let lower = text.to_lowercase();
            let query = query.to_lowercase();
            if lower.len() != text.len() {
                // Lowercasing changed byte lengths; fall back to an exact search.
                return text
                    .match_indices(find.query.text())
                    .map(|(start, found)| start..start + found.len())
                    .collect();
            }
            lower.match_indices(&query).map(|(start, found)| start..start + found.len()).collect()
        }
    }

    fn find_step(&mut self, backwards: bool) {
        let matches = self.matches();
        if matches.is_empty() {
            return;
        }
        let selection = self.editor.selected_range();
        let next = if backwards {
            matches
                .iter()
                .rev()
                .find(|range| range.end <= selection.start && *range != &selection)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|range| range.start >= selection.end && *range != &selection)
                .or_else(|| matches.first())
        };
        if let Some(range) = next {
            self.editor.set_selection(range.clone());
        }
    }

    fn replace_one(&mut self) {
        let Some(find) = &self.find else {
            return;
        };
        let replacement = find.replacement.text().to_owned();
        let selection = self.editor.selected_range();
        if self.matches().contains(&selection) {
            self.editor.replace(selection, &replacement);
            self.editor_changed();
        }
        self.find_step(false);
    }

    fn replace_all(&mut self) {
        let Some(find) = &self.find else {
            return;
        };
        let replacement = find.replacement.text().to_owned();
        let matches = self.matches();
        if matches.is_empty() {
            return;
        }
        let mut text = self.editor.text().to_owned();
        for range in matches.iter().rev() {
            text.replace_range(range.clone(), &replacement);
        }
        let all = 0..self.editor.text().len();
        self.editor.replace(all, &text);
        self.editor_changed();
    }

    pub fn close_find(&mut self, focus: &mut Option<WidgetId>) {
        self.find = None;
        self.focus(focus);
    }

    /// Keys while focus is in the panel. Returns whether the key was used.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        let key = event.key.as_str();
        let m = event.modifiers;
        if m.control && !m.shift && !m.alt && (key == "k" || key == "i") {
            self.open_ask(focus);
            return true;
        }
        if m.control && !m.shift && !m.alt && key == "f" {
            self.open_find(false, focus);
            return true;
        }
        if m.control && !m.shift && !m.alt && key == "h" {
            self.open_find(true, focus);
            return true;
        }
        if m.control && !m.shift && !m.alt && key == "j" {
            self.events.push(SqlPanelEvent::Close);
            return true;
        }
        let ask_key = match key {
            "enter" if m.control => Some(AskKey::Run),
            "escape" => Some(AskKey::Dismiss),
            "tab" if !m.modified() => Some(AskKey::Accept),
            _ => None,
        };
        if ask_key.is_some_and(|ask_key| self.ask_key(ask_key, focus)) {
            return true;
        }
        if key == "enter" && m.control {
            if !self.busy {
                self.events.push(SqlPanelEvent::Run(false));
            }
            return true;
        }
        if *focus == Some(self.ask_input.id) {
            let (handled, accepted) = self.mentions.key(event, &mut self.ask_input);
            if handled {
                if accepted {
                    self.mentions.refresh(&self.ask_input);
                }
                return true;
            }
            let outcome = self.ask_input.key(event);
            if outcome.handled {
                self.mentions.refresh(&self.ask_input);
            }
            if outcome.submit {
                self.submit_ask();
            }
            return outcome.handled;
        }
        if let Some(find) = &mut self.find
            && (*focus == Some(find.query.id) || *focus == Some(find.replacement.id))
        {
            let in_query = *focus == Some(find.query.id);
            match key {
                "escape" => {
                    self.close_find(focus);
                    return true;
                }
                "tab" if find.replace => {
                    *focus = Some(if in_query { find.replacement.id } else { find.query.id });
                    return true;
                }
                "enter" if !m.control => {
                    if in_query {
                        self.find_step(m.shift);
                    } else {
                        self.replace_one();
                    }
                    return true;
                }
                "f3" => {
                    self.find_step(m.shift);
                    return true;
                }
                _ => {}
            }
            let input = if in_query { &mut find.query } else { &mut find.replacement };
            return input.key(event).handled;
        }
        if *focus != Some(self.editor.id) {
            return false;
        }
        if key == "escape" {
            if !self.editor.close_completion() {
                self.events.push(SqlPanelEvent::GridFocus);
            }
            return true;
        }
        if key == "f3" && self.find.is_some() {
            self.find_step(m.shift);
            return true;
        }
        let outcome = self.editor.key(event);
        if outcome.changed {
            self.editor_changed();
        }
        outcome.handled
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus == Some(self.editor.id) {
            if self.editor.type_text(typed).changed {
                self.editor_changed();
            }
            return true;
        }
        if focus == Some(self.ask_input.id) {
            self.ask_input.type_text(typed);
            self.mentions.refresh(&self.ask_input);
            return true;
        }
        if let Some(find) = &mut self.find {
            if focus == Some(find.query.id) {
                find.query.type_text(typed);
                return true;
            }
            if focus == Some(find.replacement.id) {
                find.replacement.type_text(typed);
                return true;
            }
        }
        false
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Button(button) => match button {
                Button::SqlSave => self.events.push(SqlPanelEvent::Save),
                Button::SqlAutoSave => self.events.push(SqlPanelEvent::AutoSave(!self.auto_save)),
                Button::SqlAutoRun => self.events.push(SqlPanelEvent::AutoRun(!self.auto_run)),
                Button::SqlRun => self.events.push(SqlPanelEvent::Run(false)),
                Button::SqlRunAll => self.events.push(SqlPanelEvent::Run(true)),
                Button::SqlAsk => self.open_ask(focus),
                Button::SqlStop => self.events.push(SqlPanelEvent::Stop),
                Button::SqlClose => self.events.push(SqlPanelEvent::Close),
                Button::AskInsert => {
                    self.accept_ask(focus);
                }
                Button::AskRun => {
                    if self.accept_ask(focus) {
                        self.events.push(SqlPanelEvent::Run(false));
                    }
                }
                Button::AskDiscard => self.dismiss_ask(focus),
                Button::AskLog => {
                    self.show_log = !self.show_log;
                    self.log_scroll.offset = f32::MAX;
                }
                Button::AskGenerate => self.submit_ask(),
                Button::AskClose => {
                    self.dismiss_ask(focus);
                    self.dismiss_ask(focus);
                }
                Button::FindNext => self.find_step(false),
                Button::FindPrevious => self.find_step(true),
                Button::FindReplace => self.replace_one(),
                Button::FindReplaceAll => self.replace_all(),
                Button::FindClose => self.close_find(focus),
                Button::FindCase => {
                    if let Some(find) = &mut self.find {
                        find.case_sensitive = !find.case_sensitive;
                    }
                }
                _ => return false,
            },
            Hit::ColumnCompletion(id, index) if *id == self.ask_input.id => {
                self.mentions.accept(Some(*index), &mut self.ask_input);
                *focus = Some(*id);
            }
            Hit::Editor(id, part) => {
                let id = *id;
                if id == self.editor.id {
                    if self.editor.mouse_down(*part, pos, clicks, shift).changed {
                        self.editor_changed();
                    }
                } else if id == self.ask_input.id {
                    self.ask_input.mouse_down(*part, pos, clicks, shift);
                    self.mentions.refresh(&self.ask_input);
                } else if let Some(find) = &mut self.find
                    && (id == find.query.id || id == find.replacement.id)
                {
                    let input = if id == find.query.id { &mut find.query } else { &mut find.replacement };
                    input.mouse_down(*part, pos, clicks, shift);
                } else {
                    return false;
                }
                *focus = Some(id);
            }
            Hit::DialogBar => {
                let bar = rect(self.log_area.right() - scroll::BAR_WIDTH, self.log_area.y, scroll::BAR_WIDTH, self.log_area.h);
                self.log_scroll.press(pos.y - bar.y, bar.h, self.log_area.h, self.log_height);
            }
            _ => return false,
        }
        true
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        for input in [&mut self.editor, &mut self.ask_input] {
            if input.is_dragging() {
                input.mouse_drag(pos);
            }
        }
        if let Some(find) = &mut self.find {
            for input in [&mut find.query, &mut find.replacement] {
                if input.is_dragging() {
                    input.mouse_drag(pos);
                }
            }
        }
        if self.log_scroll.dragging() {
            let bar = rect(self.log_area.right() - scroll::BAR_WIDTH, self.log_area.y, scroll::BAR_WIDTH, self.log_area.h);
            self.log_scroll.drag(pos.y - bar.y, bar.h, self.log_area.h, self.log_height);
        }
    }

    pub fn mouse_up(&mut self) {
        self.editor.mouse_up();
        self.ask_input.mouse_up();
        self.log_scroll.release();
        if let Some(find) = &mut self.find {
            find.query.mouse_up();
            find.replacement.mouse_up();
        }
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, line: f32) -> bool {
        match hit {
            Hit::Editor(id, _) if *id == self.editor.id => self.editor.wheel(delta),
            Hit::DialogScroll | Hit::DialogBar => {
                self.log_scroll
                    .scroll_by(-delta.y * line * 2.0, self.log_area.h, self.log_height)
            }
            _ => false,
        }
    }

    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        let id = focus?;
        if id == self.editor.id {
            return self.editor.caret_rect();
        }
        if id == self.ask_input.id {
            return self.ask_input.caret_rect();
        }
        None
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        canvas.fill(r, colors::BACKGROUND);
        canvas.fill(rect(r.x, r.y, r.w, 1.0), colors::BORDER);
        let font = &fonts.ui;
        let line = text::line_height(font);
        let mut column = Column::new(rect(r.x, r.y + 1.0, r.w, r.h - 1.0), 0.0);
        let toolbar = column.next(button_height(font) + 8.0);
        let mut items = Row::new(toolbar.inset_xy(12.0, 4.0), 8.0);
        let label = if self.startup_script { "duckdb_startup.sql" } else { "SQL" };
        let label_rect = items.left(text::measure(label, font).0 + 4.0);
        canvas.text(label_rect, label, font, colors::FOREGROUND, Align::Left);
        if self.startup_script {
            let save = items.left(button_width("Save", font));
            button(canvas, save, "Save", font, ButtonStyle::Outline, true, Hit::Button(Button::SqlSave));
            canvas.hit_with(save, Hit::Button(Button::SqlSave), crate::ui::platform::Cursor::Arrow, Some("Save duckdb_startup.sql for the next database startup"));
        }
        // Auto-save and Auto-run are in the window's top row, with the
        // document's other settings.
        for (label, hit, enabled) in [
            ("Run · Ctrl+Enter", Button::SqlRun, !self.busy),
            ("Run All", Button::SqlRunAll, !self.busy),
            ("Ask AI · Ctrl+K", Button::SqlAsk, true),
            ("Stop", Button::SqlStop, self.busy),
        ] {
            let button_rect = items.left(button_width(label, font));
            button(canvas, button_rect, label, font, ButtonStyle::Outline, enabled, Hit::Button(hit));
        }
        let close = items.right(button_width("Close", font));
        button(canvas, close, "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::SqlClose));
        canvas.text(items.rest(), &self.status, &fonts.small, colors::FOREGROUND, Align::Left);
        if let Some(error) = self.error.clone() {
            let area = column.rest();
            let error_rect = rect(area.x + 12.0, area.y, area.w - 24.0, (line * 4.0).min(area.h / 3.0));
            canvas.push_clip(error_rect);
            let height = canvas.paragraph(error_rect, &error, font, colors::DANGER);
            canvas.pop_clip();
            column.skip(height.min(error_rect.h) + 4.0);
        }
        if self.ask_open {
            self.paint_ask(canvas, &mut column, fonts, focus);
        }
        if self.find.is_some() {
            self.paint_find(canvas, &mut column, fonts, focus);
        }
        let editor = column.rest();
        let id = self.editor.id;
        self.editor
            .paint(canvas, editor, &fonts.mono, focus == Some(id), |part| Hit::Editor(id, part));
    }

    fn paint_find(&mut self, canvas: &mut Canvas<Hit>, column: &mut Column, fonts: &Fonts, focus: Option<WidgetId>) {
        let small = &fonts.small;
        let height = Editor::line_box_height(small);
        let matches = self.matches();
        let Some(find) = &mut self.find else {
            return;
        };
        let rows = if find.replace { 2.0 } else { 1.0 };
        let area = column.next(height * rows + 4.0 * rows + 4.0);
        canvas.fill(area, colors::STRIPE);
        let mut items = Row::new(rect(area.x + 12.0, area.y + 4.0, area.w - 24.0, height), 6.0);
        let close = items.right(button_width("×", small));
        button(canvas, close, "×", small, ButtonStyle::Ghost, true, Hit::Button(Button::FindClose));
        for (label, hit) in [("Next", Button::FindNext), ("Previous", Button::FindPrevious)] {
            let r = items.right(button_width(label, small));
            button(canvas, r, label, small, ButtonStyle::Outline, true, Hit::Button(hit));
        }
        let case = items.right(button_width("Aa", small));
        button(
            canvas,
            case,
            "Aa",
            small,
            if find.case_sensitive { ButtonStyle::Selected } else { ButtonStyle::Outline },
            true,
            Hit::Button(Button::FindCase),
        );
        let count = format!("{} matches", matches.len());
        let count_rect = items.right(text::measure(&count, small).0 + 4.0);
        canvas.text(count_rect, &count, small, colors::MUTED, Align::Left);
        let query_rect = items.rest();
        let query_id = find.query.id;
        find.query
            .paint(canvas, query_rect.with_w(query_rect.w.min(420.0)), small, focus == Some(query_id), |part| Hit::Editor(query_id, part));
        if find.replace {
            let mut items = Row::new(rect(area.x + 12.0, area.y + 8.0 + height, area.w - 24.0, height), 6.0);
            let all = items.right(button_width("Replace all", small));
            button(canvas, all, "Replace all", small, ButtonStyle::Outline, true, Hit::Button(Button::FindReplaceAll));
            let one = items.right(button_width("Replace", small));
            button(canvas, one, "Replace", small, ButtonStyle::Outline, true, Hit::Button(Button::FindReplace));
            let replacement_rect = items.rest();
            let replacement_id = find.replacement.id;
            find.replacement.paint(
                canvas,
                replacement_rect.with_w(replacement_rect.w.min(420.0)),
                small,
                focus == Some(replacement_id),
                |part| Hit::Editor(replacement_id, part),
            );
        }
    }

    /// Which provider a request goes to, and a reminder when it may send data.
    fn provider_line(&self) -> Vec<(String, Color)> {
        let Some(provider) = &self.ask_provider else {
            return vec![(
                "No AI provider yet: add one with “AI providers” in the command palette".into(),
                colors::MUTED,
            )];
        };
        let mut parts = vec![(format!("{} · {}", provider.name, provider.model), colors::MUTED)];
        if provider.send_data {
            parts.push((
                "Data allowed: tables named with @ also send min, max, and frequent values".into(),
                Color::rgb(0xb7791f),
            ));
        } else {
            parts.push(("No data values sent".into(), colors::MUTED));
        }
        parts
    }

    fn paint_ask(&mut self, canvas: &mut Canvas<Hit>, column: &mut Column, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let small = &fonts.small;
        let line = text::line_height(small);
        let start = column.y;
        canvas.fill(rect(column.area.x, start, column.area.w, 1.0), colors::BORDER);
        let inner_x = column.area.x + 12.0;
        let inner_w = column.area.w - 24.0;
        let mut y = start + 8.0;
        let row = rect(inner_x, y, inner_w, Editor::line_box_height(font));
        let mut items = Row::new(row, 8.0);
        let label = items.left(text::measure("Ask AI", small).0 + 4.0);
        canvas.text(label, "Ask AI", small, colors::FOREGROUND, Align::Left);
        let working = matches!(self.ask, AskState::Working(_));
        let button_top = row.y + (row.h - button_height(font)) / 2.0;
        let close = items.right(button_width("Close", font));
        button(canvas, close.with_h(button_height(font)).offset(0.0, button_top - row.y), "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::AskClose));
        let generate = items.right(button_width("Generate · Enter", font));
        button(canvas, generate.with_h(button_height(font)).offset(0.0, button_top - row.y), "Generate · Enter", font, ButtonStyle::Outline, !working, Hit::Button(Button::AskGenerate));
        let log_label = if self.show_log { "Hide log" } else { "Show log" };
        let log = items.right(button_width(log_label, font));
        let log_rect = log.with_h(button_height(font)).offset(0.0, button_top - row.y);
        button(canvas, log_rect, log_label, font, ButtonStyle::Outline, true, Hit::Button(Button::AskLog));
        canvas.hit_with(log_rect, Hit::Button(Button::AskLog), crate::ui::platform::Cursor::Arrow, Some("Show each request sent to the AI and each reply"));
        let input = items.rest();
        let id = self.ask_input.id;
        self.ask_input.paint(canvas, input, font, focus == Some(id), |part| Hit::Editor(id, part));
        y = row.bottom() + 4.0;
        let mut x = inner_x;
        for (part, color) in self.provider_line() {
            let width = text::measure(&part, small).0;
            canvas.text(rect(x, y, (inner_x + inner_w - x).max(0.0), line), &part, small, color, Align::Left);
            x += width + 8.0;
        }
        y += line + 4.0;
        match &self.ask {
            AskState::Idle => {
                let text_value = "Type @ to name tables: only those are described, with row, null, and distinct counts. Without @, every table's columns, types, and comments are sent. With SQL selected, describe a change to it.";
                y += canvas.paragraph(rect(inner_x, y, inner_w, 200.0), text_value, small, colors::MUTED);
            }
            AskState::Working(status) => {
                canvas.text(rect(inner_x, y, inner_w, line), &format!("{status} · Esc cancels"), small, colors::MUTED, Align::Left);
                y += line;
            }
            AskState::Failed(error) => {
                y += canvas.paragraph(rect(inner_x, y, inner_w, 200.0), error, small, colors::DANGER);
            }
            AskState::Proposal(proposal) => {
                let mono = fonts.mono.clone();
                let layout = text::layout(&proposal.sql, &mono, TextOptions::default());
                let height = (layout.height + 8.0).min(260.0);
                let code = rect(inner_x, y, inner_w, height);
                canvas.fill_rounded(code, 4.0, colors::STRIPE);
                self.proposal_scroll.clamp(height - 8.0, layout.height);
                canvas.push_clip(code.inset(4.0));
                canvas.draw_layout(&layout, point(code.x + 8.0, code.y + 4.0 - self.proposal_scroll.offset), colors::FOREGROUND);
                canvas.pop_clip();
                y += height + 4.0;
                let (check, color) = match &proposal.check {
                    SqlCheck::Passed => ("✓ DuckDB accepts this SQL".to_owned(), colors::MUTED),
                    SqlCheck::Partial { checked, total } => (
                        format!("✓ DuckDB accepts the first {checked} of {total} statements; the rest use what they create"),
                        colors::MUTED,
                    ),
                    SqlCheck::Failed(error) => (format!("DuckDB still rejects it: {error}"), colors::DANGER),
                };
                canvas.text(rect(inner_x, y, inner_w, line), &check, small, color, Align::Left);
                y += line + 4.0;
                let mut buttons = Row::new(rect(inner_x, y, inner_w, button_height(font)), 8.0);
                for (label, hit, style, enabled) in [
                    ("Insert · Tab", Button::AskInsert, ButtonStyle::Primary, true),
                    ("Insert and run · Ctrl+Enter", Button::AskRun, ButtonStyle::Outline, !self.busy),
                    ("Discard · Esc", Button::AskDiscard, ButtonStyle::Outline, true),
                ] {
                    let r = buttons.left(button_width(label, font));
                    button(canvas, r, label, font, style, enabled, Hit::Button(hit));
                }
                y += button_height(font);
            }
        }
        if self.show_log {
            y += 4.0;
            let area = rect(inner_x, y, inner_w, 320.0_f32.min((column.area.bottom() - y - 80.0).max(60.0)));
            self.paint_log(canvas, area, fonts);
            y = area.bottom();
        }
        y += 8.0;
        canvas.fill(rect(column.area.x, y - 1.0, column.area.w, 1.0), colors::BORDER);
        column.skip(y - start);
    }

    fn paint_log(&mut self, canvas: &mut Canvas<Hit>, area: Rect, fonts: &Fonts) {
        canvas.stroke_rounded(area, 4.0, colors::BORDER, 1.0);
        let small = &fonts.small;
        let mono = fonts.mono.sized(small.size);
        let width = area.w - 16.0 - scroll::BAR_WIDTH;
        let mut entries: Vec<(String, Font, Color, bool)> = Vec::new();
        if !self.ask_endpoint.is_empty() {
            entries.push((format!("POST {}", self.ask_endpoint), small.clone(), colors::MUTED, false));
        }
        for event in &self.ask_log {
            match event {
                AiEvent::Sent(messages) => {
                    for message in messages {
                        entries.push((format!("→ {}", message.role), small.semibold(), colors::MUTED, false));
                        entries.push((message.content.clone(), mono.clone(), colors::FOREGROUND, true));
                    }
                }
                AiEvent::Received { text, elapsed } => {
                    entries.push((format!("← assistant · {:.1} s", elapsed.as_secs_f32()), small.semibold(), colors::MUTED, false));
                    entries.push((text.clone(), mono.clone(), colors::FOREGROUND, true));
                }
                AiEvent::Checked(check) => {
                    let (text_value, color) = match check {
                        SqlCheck::Passed => ("DuckDB: accepted".to_owned(), colors::MUTED),
                        SqlCheck::Partial { checked, total } => (format!("DuckDB: accepted the first {checked} of {total} statements"), colors::MUTED),
                        SqlCheck::Failed(error) => (format!("DuckDB: rejected · {error}"), colors::DANGER),
                    };
                    entries.push((text_value, small.semibold(), color, false));
                }
                AiEvent::Failed(error) => entries.push((format!("Failed · {error}"), small.semibold(), colors::DANGER, false)),
                AiEvent::Note(text_value) => entries.push((text_value.clone(), small.semibold(), colors::MUTED, false)),
            }
        }
        if self.ask_log.is_empty() {
            entries.push(("Nothing sent yet. Each request and reply shows here as it happens.".into(), small.clone(), colors::MUTED, false));
        }
        let layouts = entries
            .iter()
            .map(|(text_value, font, _, _)| text::layout(text_value, font, TextOptions::wrapped(width - 8.0)))
            .collect::<Vec<_>>();
        let content = layouts.iter().map(|layout| layout.height + 8.0).sum::<f32>() + 8.0;
        self.log_area = area;
        self.log_height = content;
        self.log_scroll.clamp(area.h, content);
        canvas.push_clip(area.inset(1.0));
        let mut y = area.y + 8.0 - self.log_scroll.offset;
        for ((_, _, color, code), layout) in entries.iter().zip(&layouts) {
            if *code {
                canvas.fill_rounded(rect(area.x + 8.0, y - 2.0, width, layout.height + 4.0), 3.0, colors::STRIPE);
            }
            canvas.draw_layout(layout, point(area.x + 12.0, y), *color);
            y += layout.height + 8.0;
        }
        canvas.pop_clip();
        canvas.hit(area, Hit::DialogScroll);
        let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
        scroll::vertical(canvas, bar, area.h, content, &self.log_scroll, Hit::DialogBar);
    }

    /// Completion menus, drawn above everything else.
    pub fn paint_overlay(&mut self, canvas: &mut Canvas<Hit>, fonts: &Fonts, focus: Option<WidgetId>) {
        let id = self.editor.id;
        self.editor
            .paint_completion(canvas, focus == Some(id), |part| Hit::Editor(id, part));
        if focus == Some(self.ask_input.id) {
            self.mentions
                .paint(canvas, self.ask_input.caret_rect(), &fonts.ui, self.ask_input.id);
        }
    }
}
