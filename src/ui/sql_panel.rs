use std::ops::Range;
use std::sync::{Arc, RwLock};

use super::column_completion::{ColumnCompletion, ColumnSyntax};
use super::input_with_autocompletion;
use crate::domain::ai::{AiEvent, SqlCheck, SqlProposal, insertion, relation_name};
use crate::domain::sql_editor::{SqlCatalog, current_statement};
use crate::infrastructure::path_completion::PathRoots;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Align, Canvas, Color, Font, Point, Rect, TextOptions, point, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::hosted_editor::HostedEditor;
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

pub struct SqlPanel {
    pub editor: HostedEditor,
    /// Whether the window is the active one; the caret blinks only then.
    pub window_active: bool,
    pub catalog: Arc<RwLock<SqlCatalog>>,
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
    events: Vec<SqlPanelEvent>,
}

impl SqlPanel {
    /// A panel whose editor paints into `host`'s frames (none in headless tests).
    pub fn new(text: String, host: Option<windows::Win32::Foundation::HWND>) -> Self {
        let catalog = Arc::new(RwLock::new(SqlCatalog::default()));
        let path_roots = Arc::new(RwLock::new(PathRoots::default()));
        let completions = input_with_autocompletion::sql_catalog(catalog.clone(), path_roots.clone());
        let language = super::sql_language::SqlLanguage::new(catalog.clone());
        let mut editor = HostedEditor::sql(host, completions, language);
        editor.set_text(text);
        editor.set_selection(0..0);
        Self {
            editor,
            window_active: true,
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
            events: Vec::new(),
        }
    }

    /// What happened since the last call. A change to the SQL comes first, so
    /// a run asked for with it sees the new text.
    pub fn take_events(&mut self) -> Vec<SqlPanelEvent> {
        if self.editor.take_changed() {
            self.events.insert(0, SqlPanelEvent::Changed(self.editor.text()));
        }
        std::mem::take(&mut self.events)
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        id == self.editor.id || id == self.ask_input.id
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
        let text = self.editor.text();
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
        let current = self.editor.text();
        let range = match self.ask_target.take() {
            Some(target) if target.text == current => target.range,
            _ => self.editor.selected_range(),
        };
        let (text, inner) = insertion(&current, range.clone(), &sql);
        self.editor.replace(range.clone(), &text);
        self.editor
            .set_selection(range.start + inner.start..range.start + inner.end);
        self.ask = AskState::Idle;
        self.ask_open = false;
        self.ask_input.set_text("");
        self.focus(focus);
        true
    }

    /// Tables as `@` mentions name them: the default database's first.
    fn mention_names(&self) -> Vec<String> {
        let catalog = self.catalog.read().map(|c| c.relations.clone()).unwrap_or_default();
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
        let text = self.editor.text();
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

    #[cfg(test)]
    fn editor_changed(&mut self) {
        self.events.push(SqlPanelEvent::Changed(self.editor.text()));
    }

    /// Keys while focus is in the panel. Returns whether the key was used.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        let key = event.key.as_str();
        let m = event.modifiers;
        if m.control && !m.shift && !m.alt && (key == "k" || key == "i") {
            self.open_ask(focus);
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
        if *focus != Some(self.editor.id) {
            return false;
        }
        // Escape closes the editor's popups and extra carets first, then
        // leaves for the grid.
        if self.editor.key(event) {
            return true;
        }
        if key == "escape" {
            self.events.push(SqlPanelEvent::GridFocus);
            return true;
        }
        false
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus == Some(self.editor.id) {
            self.editor.type_text(typed);
            return true;
        }
        if focus == Some(self.ask_input.id) {
            self.ask_input.type_text(typed);
            self.mentions.refresh(&self.ask_input);
            return true;
        }
        false
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: Modifiers, focus: &mut Option<WidgetId>) -> bool {
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
                _ => return false,
            },
            Hit::ColumnCompletion(id, index) if *id == self.ask_input.id => {
                self.mentions.accept(Some(*index), &mut self.ask_input);
                *focus = Some(*id);
            }
            Hit::CodeEditor(id) if *id == self.editor.id => {
                self.editor.mouse_down(pos, clicks, modifiers);
                *focus = Some(*id);
            }
            Hit::Editor(id, part) if *id == self.ask_input.id => {
                self.ask_input.mouse_down(*part, pos, clicks, modifiers.shift);
                self.mentions.refresh(&self.ask_input);
                *focus = Some(*id);
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
        if self.editor.is_dragging() {
            self.editor.mouse_move(pos, true);
        }
        if self.ask_input.is_dragging() {
            self.ask_input.mouse_drag(pos);
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
    }

    /// The pointer moved with no button down; the editor tracks hover.
    pub fn mouse_move(&mut self, pos: Point, hit: Option<&Hit>) {
        let over = hit == Some(&Hit::CodeEditor(self.editor.id));
        self.editor.mouse_move(pos, over);
    }

    pub fn mouse_leave(&mut self) {
        self.editor.mouse_leave();
    }

    /// A right click: the editor's context menu. Returns whether it was the editor's.
    pub fn context_menu(&mut self, hit: &Hit, pos: Point, focus: &mut Option<WidgetId>) -> bool {
        if *hit != Hit::CodeEditor(self.editor.id) {
            return false;
        }
        *focus = Some(self.editor.id);
        self.editor.context_menu(pos);
        true
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, modifiers: Modifiers, line: f32) -> bool {
        match hit {
            Hit::CodeEditor(id) if *id == self.editor.id => self.editor.wheel(delta, modifiers),
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
        let editor = column.rest();
        let id = self.editor.id;
        let focused = focus == Some(id) && self.window_active;
        self.editor.paint(canvas, editor, &fonts.mono, focused, Hit::CodeEditor(id));
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
        if focus == Some(self.ask_input.id) {
            self.mentions
                .paint(canvas, self.ask_input.caret_rect(), &fonts.ui, self.ask_input.id);
        }
    }
}
