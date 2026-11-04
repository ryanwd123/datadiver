use std::ops::Range;
use std::sync::{Arc, RwLock};

use super::column_completion::{ColumnCompletion, ColumnSyntax};
use super::input_with_autocompletion;
use crate::domain::ai::{AiEvent, SqlCheck, SqlProposal, insertion, relation_name};
use crate::domain::sql_editor::{CompletionOptions, SqlCatalog, current_statement};
use crate::infrastructure::json_store::EditorSettings;
use crate::infrastructure::path_completion::PathRoots;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Align, Canvas, Color, Font, Point, Rect, TextLayout, TextOptions, point, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::hosted_editor::HostedEditor;
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors, scroll};

pub enum SqlPanelEvent {
    Changed(String),
    Save,
    Run(bool),
    Stop,
    Close,
    Ask(SqlAsk),
    /// The ask panel opened; the view says which provider it will use.
    AskOpened,
    /// The Helper button: open the SQL helper menu.
    Helper,
}

/// The provider a request will go to, shown in the ask panel.
#[derive(Clone)]
pub struct AskProvider {
    pub name: String,
    pub model: String,
    pub send_data: bool,
}

/// A request for SQL from the ask panel.
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
    /// Where the editor was last painted; the helper menu opens over it.
    pub editor_area: Rect,
    /// Whether the window is the active one; the caret blinks only then.
    pub window_active: bool,
    pub catalog: Arc<RwLock<SqlCatalog>>,
    pub path_roots: Arc<RwLock<PathRoots>>,
    /// The completion triggers and quoting from the editor options.
    completion_options: Arc<RwLock<CompletionOptions>>,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
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
    /// The ask panel scrolls when its content is taller than the panel.
    ask_scroll: scroll::Scroll,
    ask_area: Rect,
    ask_content: f32,
    /// Scroll a new proposal into view on the next paint.
    reveal_proposal: bool,
    events: Vec<SqlPanelEvent>,
}

impl SqlPanel {
    /// A panel whose editor paints into `host`'s frames (none in headless tests).
    pub fn new(text: String, host: Option<windows::Win32::Foundation::HWND>) -> Self {
        let catalog = Arc::new(RwLock::new(SqlCatalog::default()));
        let path_roots = Arc::new(RwLock::new(PathRoots::default()));
        let completion_options = Arc::new(RwLock::new(CompletionOptions::default()));
        let completions = input_with_autocompletion::sql_catalog(catalog.clone(), path_roots.clone(), completion_options.clone());
        let language = super::sql_language::SqlLanguage::new(catalog.clone());
        let mut editor = HostedEditor::sql(host, completions, language);
        editor.set_text(text);
        editor.set_selection(0..0);
        Self {
            editor,
            editor_area: Rect::default(),
            window_active: true,
            catalog,
            path_roots,
            completion_options,
            busy: false,
            status: "Ctrl+Enter · selection or current statement".into(),
            error: None,
            startup_script: false,
            ask_open: false,
            ask_input: ask_input(),
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
            ask_scroll: scroll::Scroll::default(),
            ask_area: Rect::default(),
            ask_content: 0.0,
            reveal_proposal: false,
            events: Vec::new(),
        }
    }

    /// Applies the editor options to the editor and its completions.
    pub fn set_editor_settings(&mut self, settings: &EditorSettings) {
        if let Ok(mut options) = self.completion_options.write() {
            *options = CompletionOptions {
                after_dot: settings.suggest_after_dot,
                after_quote: settings.suggest_after_quote,
                after_from: settings.suggest_after_from,
                quoting: if settings.always_quote_names {
                    diver_sql::Quoting::Always
                } else {
                    diver_sql::Quoting::WhenNeeded
                },
            };
        }
        self.editor.set_settings(settings);
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

    /// Shows the ask panel and focuses its prompt.
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
        self.reveal_proposal = matches!(self.ask, AskState::Proposal(_));
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
        text[self.sql_to_run_range(all)].to_string()
    }

    /// Where the SQL Run runs is in the text: all of it, the selection, or
    /// the statement at the caret.
    pub fn sql_to_run_range(&self, all: bool) -> Range<usize> {
        let text = self.editor.text();
        if all {
            return 0..text.len();
        }
        let selection = self.editor.selected_range();
        if !selection.is_empty() {
            return selection;
        }
        current_statement(&text, self.editor.cursor()).unwrap_or(0..0)
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
        let in_ask = *focus == Some(self.ask_input.id);
        if in_ask && m.control && !m.shift && !m.alt && (key == "k" || key == "i") {
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
        if in_ask && key == "enter" && m.control {
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
            // The prompt is multi-line: Enter sends it, Shift+Enter starts a
            // new line, and Tab never types a tab into it.
            if key == "enter" && !m.control && !m.alt {
                if m.shift {
                    self.ask_input.replace_selection("\n");
                    self.mentions.refresh(&self.ask_input);
                } else {
                    self.submit_ask();
                }
                return true;
            }
            if key == "tab" {
                return false;
            }
            let outcome = self.ask_input.key(event);
            if outcome.handled {
                self.mentions.refresh(&self.ask_input);
            }
            return outcome.handled;
        }
        if *focus != Some(self.editor.id) {
            return false;
        }
        // Escape closes the editor's popups and extra carets, and otherwise
        // stays in the editor.
        self.editor.key(event) || key == "escape"
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
                Button::SqlRun => self.events.push(SqlPanelEvent::Run(false)),
                Button::SqlRunAll => self.events.push(SqlPanelEvent::Run(true)),
                Button::SqlAsk => self.open_ask(focus),
                Button::SqlHelper => self.events.push(SqlPanelEvent::Helper),
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
            Hit::AskBar => {
                let bar = self.ask_bar();
                self.ask_scroll.press(pos.y - bar.y, bar.h, self.ask_area.h, self.ask_content);
            }
            Hit::AskScroll => {}
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
        if self.ask_scroll.dragging() {
            let bar = self.ask_bar();
            self.ask_scroll.drag(pos.y - bar.y, bar.h, self.ask_area.h, self.ask_content);
        }
    }

    pub fn mouse_up(&mut self) {
        self.editor.mouse_up();
        self.ask_input.mouse_up();
        self.log_scroll.release();
        self.ask_scroll.release();
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
            Hit::Editor(id, _) if *id == self.ask_input.id => self.ask_input.wheel(delta),
            Hit::DialogScroll | Hit::DialogBar => {
                self.log_scroll
                    .scroll_by(-delta.y * line * 2.0, self.log_area.h, self.log_height)
            }
            Hit::AskScroll | Hit::AskBar => {
                self.ask_scroll
                    .scroll_by(-delta.y * line * 2.0, self.ask_area.h, self.ask_content)
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
            ("Helper · Ctrl+K", Button::SqlHelper, true),
            ("Ask AI · Ctrl+I", Button::SqlAsk, true),
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
        let mut editor = column.rest();
        if self.ask_open {
            // The ask panel sits to the right of the editor, which keeps at
            // least half the width.
            let width = (editor.w * 0.36).clamp(340.0, 560.0).min(editor.w * 0.5);
            let (ask, rest) = editor.split_right(width);
            self.paint_ask(canvas, ask, fonts, focus);
            editor = rest;
        }
        self.editor_area = editor;
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

    /// The ask panel, beside the editor: a fixed header over a body that
    /// scrolls. The body is measured first so a new proposal can scroll into
    /// view in the same frame.
    fn paint_ask(&mut self, canvas: &mut Canvas<Hit>, area: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let small = &fonts.small;
        let gap = 6.0;
        let inner_w = (area.w - 24.0 - scroll::BAR_WIDTH).max(40.0);
        let wrapped = |value: &str, font: &Font| text::layout(value, font, TextOptions::wrapped(inner_w));

        let header_h = button_height(font);
        let (head, body) = area.split_top(8.0 + header_h + 8.0);

        // Measure, in content coordinates from the body's top.
        let input_h = (text::line_height(font) * 4.0 + 12.0).round();
        let mut y = 0.0;
        let input_y = y;
        y += input_h + gap;
        let actions_y = y;
        y += button_height(font) + gap;
        let mut texts: Vec<(f32, std::rc::Rc<TextLayout>, Color)> = Vec::new();
        for (part, color) in self.provider_line() {
            let layout = wrapped(&part, small);
            let height = layout.height;
            texts.push((y, layout, color));
            y += height;
        }
        y += gap;
        let mut code = None;
        let mut buttons = Vec::new();
        let mut proposal_span = None;
        match &self.ask {
            AskState::Idle => {}
            AskState::Working(status) => {
                let layout = wrapped(&format!("{status} · Esc cancels"), small);
                y += layout.height;
                texts.push((y - layout.height, layout, colors::MUTED));
            }
            AskState::Failed(error) => {
                let layout = wrapped(error, small);
                y += layout.height;
                texts.push((y - layout.height, layout, colors::DANGER));
            }
            AskState::Proposal(proposal) => {
                let start = y;
                let layout = text::layout(&proposal.sql, &fonts.mono, TextOptions::wrapped((inner_w - 16.0).max(40.0)));
                let height = layout.height + 8.0;
                code = Some((y, height, layout));
                y += height + 4.0;
                let (check, color) = match &proposal.check {
                    SqlCheck::Passed => ("✓ DuckDB accepts this SQL".to_owned(), colors::MUTED),
                    SqlCheck::Partial { checked, total } => (
                        format!("✓ DuckDB accepts the first {checked} of {total} statements; the rest use what they create"),
                        colors::MUTED,
                    ),
                    SqlCheck::Failed(error) => (format!("DuckDB still rejects it: {error}"), colors::DANGER),
                };
                let layout = wrapped(&check, small);
                let height = layout.height;
                texts.push((y, layout, color));
                y += height + 4.0;
                // The buttons wrap onto another line when the panel is narrow.
                let mut x = 0.0;
                for (label, hit, style, enabled) in [
                    ("Insert · Tab", Button::AskInsert, ButtonStyle::Primary, true),
                    ("Insert and run · Ctrl+Enter", Button::AskRun, ButtonStyle::Outline, !self.busy),
                    ("Discard · Esc", Button::AskDiscard, ButtonStyle::Outline, true),
                ] {
                    let width = button_width(label, font);
                    if x > 0.0 && x + width > inner_w {
                        x = 0.0;
                        y += button_height(font) + gap;
                    }
                    buttons.push((rect(x, y, width, button_height(font)), label, hit, style, enabled));
                    x += width + 8.0;
                }
                y += button_height(font);
                proposal_span = Some((start, y - start));
            }
        }
        let log = self.show_log.then(|| {
            y += 8.0;
            let top = y;
            let height = (body.h - y - 8.0).max(160.0);
            y += height;
            (top, height)
        });
        let content = y + 8.0;
        if self.reveal_proposal
            && let Some((start, size)) = proposal_span
        {
            self.reveal_proposal = false;
            self.ask_scroll.reveal(start, size + 8.0, body.h);
        }
        self.ask_scroll.clamp(body.h, content);
        self.ask_area = body;
        self.ask_content = content;

        // Paint.
        canvas.fill(area, colors::BACKGROUND);
        canvas.fill(rect(area.x, area.y, 1.0, area.h), colors::BORDER);
        canvas.hit(head, Hit::AskScroll);
        let x = area.x + 12.0;
        let mut items = Row::new(rect(x, head.y + 8.0, inner_w, header_h), 8.0);
        let title = font.semibold();
        let label = items.left(text::measure("Ask AI", &title).0 + 4.0);
        canvas.text(label, "Ask AI", &title, colors::FOREGROUND, Align::Left);
        let close = items.right(button_width("Close", font));
        button(canvas, close, "Close", font, ButtonStyle::Outline, true, Hit::Button(Button::AskClose));
        let log_label = if self.show_log { "Hide log" } else { "Show log" };
        let log_button = items.right(button_width(log_label, font));
        button(canvas, log_button, log_label, font, ButtonStyle::Outline, true, Hit::Button(Button::AskLog));
        canvas.hit_with(log_button, Hit::Button(Button::AskLog), crate::ui::platform::Cursor::Arrow, Some("Show each request sent to the AI and each reply"));

        canvas.push_clip(body);
        canvas.hit(body, Hit::AskScroll);
        let top = body.y - self.ask_scroll.offset;
        let at = |offset: f32, w: f32, h: f32| rect(x, top + offset, w, h);

        let id = self.ask_input.id;
        self.ask_input
            .paint(canvas, at(input_y, inner_w, input_h), font, focus == Some(id), |part| Hit::Editor(id, part));

        let working = matches!(self.ask, AskState::Working(_));
        let mut actions = Row::new(at(actions_y, inner_w, button_height(font)), 8.0);
        let generate = actions.left(button_width("Generate · Enter", font));
        button(canvas, generate, "Generate · Enter", font, ButtonStyle::Outline, !working, Hit::Button(Button::AskGenerate));
        canvas.text(actions.rest(), "Shift+Enter for a new line", small, colors::MUTED, Align::Left);

        for (offset, layout, color) in &texts {
            canvas.draw_layout(layout, point(x, top + offset), *color);
        }
        if let Some((offset, height, layout)) = &code {
            let block = at(*offset, inner_w, *height);
            canvas.fill_rounded(block, 4.0, colors::STRIPE);
            canvas.draw_layout(layout, point(block.x + 8.0, block.y + 4.0), colors::FOREGROUND);
        }
        for (r, label, hit, style, enabled) in buttons {
            button(canvas, r.offset(x, top), label, font, style, enabled, Hit::Button(hit));
        }
        if let Some((offset, height)) = log {
            self.paint_log(canvas, at(offset, inner_w, height), fonts);
        }
        canvas.pop_clip();
        scroll::vertical(canvas, self.ask_bar(), body.h, content, &self.ask_scroll, Hit::AskBar);
    }

    /// The ask panel's scroll bar, along its right edge.
    fn ask_bar(&self) -> Rect {
        let area = self.ask_area;
        rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h)
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
        self.editor.paint_popups(canvas, Hit::CodeEditor(id));
        if focus == Some(self.ask_input.id) {
            self.mentions
                .paint(canvas, self.ask_input.caret_rect(), &fonts.ui, self.ask_input.id);
        }
    }
}

/// The ask prompt: a few wrapped lines, framed like an input.
fn ask_input() -> Editor {
    let mut editor = Editor::multi_line().with_placeholder("Describe the query you want");
    editor.soft_wrap = true;
    editor.framed = true;
    editor
}
