use gpui_kit::prelude::FluentBuilder;
use std::ops::Range;
use std::sync::{Arc, RwLock};

use gpui_kit::{
    component::{
        ActiveTheme, Disableable,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        input::{Editor, EditorState, Input, InputEvent, InputState},
    },
    *,
};

use super::column_completion::{ColumnCompletion, ColumnSyntax, with_column_completion};
use super::input_with_autocompletion::{InputWithAutocompletion, path_completions};
use crate::domain::ai::{AiEvent, SqlCheck, SqlProposal, insertion, relation_name};
use crate::domain::sql_editor::{Relation, current_statement};
use crate::infrastructure::path_completion::PathRoots;

gpui_kit::actions!(sql_panel, [RunSql, SqlEscape, AcceptAsk]);
struct SqlBindings;
impl Global for SqlBindings {}

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
    pub editor: Entity<InputWithAutocompletion>,
    pub catalog: Arc<RwLock<Vec<Relation>>>,
    pub path_roots: Arc<RwLock<PathRoots>>,
    pub busy: bool,
    pub status: String,
    pub error: Option<String>,
    pub auto_save: bool,
    pub auto_run: bool,
    pub startup_script: bool,
    accepting_completion: bool,
    text_len: usize,
    reopen_task: Option<Task<()>>,
    pub ask_open: bool,
    pub ask_input: Entity<InputState>,
    /// The `@` table list under the prompt.
    pub mentions: Entity<ColumnCompletion>,
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
    log_scroll: ScrollHandle,
    _subscription: Subscription,
    _ask_subscription: Subscription,
}

impl EventEmitter<SqlPanelEvent> for SqlPanel {}

impl SqlPanel {
    pub fn new(text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<SqlBindings>() {
            cx.bind_keys([
                KeyBinding::new("ctrl-enter", RunSql, Some("SqlPanel")),
                KeyBinding::new("ctrl-enter", RunSql, Some("SqlPanel > Input")),
                KeyBinding::new("escape", SqlEscape, Some("SqlPanel")),
                KeyBinding::new("escape", SqlEscape, Some("SqlPanel > Input")),
                // The prompt's own Tab binding would win over a key listener.
                KeyBinding::new("tab", AcceptAsk, Some("SqlAsk > Input")),
                KeyBinding::new("escape", SqlEscape, Some("SqlAsk > Input")),
                KeyBinding::new("ctrl-enter", RunSql, Some("SqlAsk > Input")),
            ]);
            cx.set_global(SqlBindings);
        }
        let catalog = Arc::new(RwLock::new(Vec::new()));
        let path_roots = Arc::new(RwLock::new(PathRoots::default()));
        let editor = cx.new(|cx| {
            InputWithAutocompletion::sql_catalog(catalog.clone(), path_roots.clone(), window, cx)
        });
        let state = editor.read(cx).state().clone();
        let text_len = text.len();
        state.update(cx, |state, cx| state.set_value(text, window, cx));
        let subscription = cx.subscribe(&state, |panel, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = state.read(cx).value().to_string();
                let grew = text.len() > panel.text_len + 1;
                panel.text_len = text.len();
                if grew {
                    panel.reopen_after_folder(&state, cx);
                }
                cx.emit(SqlPanelEvent::Changed(text));
            }
        });
        let ask_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Describe the query you want, then Enter")
        });
        let ask_subscription = cx.subscribe(&ask_input, |panel, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = event {
                panel.submit_ask(cx);
            }
        });
        let mentions =
            cx.new(|cx| ColumnCompletion::new(ask_input.clone(), ColumnSyntax::Mention, cx));
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
            accepting_completion: false,
            text_len,
            reopen_task: None,
            ask_open: false,
            ask_input,
            mentions,
            ask_provider: None,
            default_catalog: String::new(),
            ask: AskState::Idle,
            ask_target: None,
            ask_revision: 0,
            ask_log: Vec::new(),
            ask_endpoint: String::new(),
            show_log: false,
            log_scroll: ScrollHandle::new(),
            _subscription: subscription,
            _ask_subscription: ask_subscription,
        }
    }

    /// Shows the ask strip and focuses its prompt.
    pub fn open_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_open = true;
        let names = self.mention_names();
        self.mentions
            .update(cx, |mentions, _| mentions.set_columns(names));
        cx.emit(SqlPanelEvent::AskOpened);
        let focus = self.ask_input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn submit_ask(&mut self, cx: &mut Context<Self>) {
        let prompt = self.ask_input.read(cx).value().trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        let state = self.editor.read(cx).state().read(cx);
        let text = state.value().to_string();
        let range = state.selected_range();
        let selection = (!range.is_empty()).then(|| text[range.clone()].to_owned());
        self.ask_target = Some(AskTarget {
            range,
            text: text.clone(),
        });
        self.ask_revision += 1;
        self.ask = AskState::Working("Writing SQL…".into());
        self.ask_log.clear();
        self.ask_endpoint.clear();
        cx.emit(SqlPanelEvent::Ask(SqlAsk {
            prompt,
            selection,
            editor_text: text,
            revision: self.ask_revision,
        }));
        cx.notify();
    }

    pub fn set_ask_working(
        &mut self,
        revision: u64,
        text: String,
        endpoint: String,
        cx: &mut Context<Self>,
    ) {
        if revision == self.ask_revision && matches!(self.ask, AskState::Working(_)) {
            self.ask = AskState::Working(text);
            self.ask_endpoint = endpoint;
            cx.notify();
        }
    }

    /// Replaces the log of request `revision` with `log`, following new steps.
    pub fn set_ask_log(&mut self, revision: u64, log: Vec<AiEvent>, cx: &mut Context<Self>) {
        if revision != self.ask_revision || log.len() == self.ask_log.len() {
            return;
        }
        self.ask_log = log;
        self.log_scroll.scroll_to_bottom();
        cx.notify();
    }

    fn render_log(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.tokens.border;
        let code_bg = theme.tokens.table_even;
        let mono = theme.mono_font_family.clone();
        let error_color: Hsla = rgb(0xb33c3c).into();
        let heading = |text: String, color: Hsla| {
            div()
                .pt_1()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(text)
        };
        let body = |text: String| {
            div()
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(code_bg)
                .font_family(mono.clone())
                .text_sm()
                .child(text)
        };
        let mut entries: Vec<AnyElement> = Vec::new();
        if !self.ask_endpoint.is_empty() {
            entries.push(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(format!("POST {}", self.ask_endpoint))
                    .into_any_element(),
            );
        }
        for event in &self.ask_log {
            match event {
                AiEvent::Sent(messages) => {
                    for message in messages {
                        entries
                            .push(heading(format!("→ {}", message.role), muted).into_any_element());
                        entries.push(body(message.content.clone()).into_any_element());
                    }
                }
                AiEvent::Received { text, elapsed } => {
                    entries.push(
                        heading(
                            format!("← assistant · {:.1} s", elapsed.as_secs_f32()),
                            muted,
                        )
                        .into_any_element(),
                    );
                    entries.push(body(text.clone()).into_any_element());
                }
                AiEvent::Checked(check) => {
                    let (text, color) = match check {
                        SqlCheck::Passed => ("DuckDB: accepted".to_owned(), muted),
                        SqlCheck::Partial { checked, total } => (
                            format!("DuckDB: accepted the first {checked} of {total} statements"),
                            muted,
                        ),
                        SqlCheck::Failed(error) => {
                            (format!("DuckDB: rejected · {error}"), error_color)
                        }
                    };
                    entries.push(heading(text, color).into_any_element());
                }
                AiEvent::Failed(error) => {
                    entries
                        .push(heading(format!("Failed · {error}"), error_color).into_any_element());
                }
                AiEvent::Note(text) => {
                    entries.push(heading(text.clone(), muted).into_any_element());
                }
            }
        }
        if self.ask_log.is_empty() {
            entries.push(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Nothing sent yet. Each request and reply shows here as it happens.")
                    .into_any_element(),
            );
        }
        div()
            .id("sql-ask-log")
            .track_scroll(&self.log_scroll)
            .max_h(px(320.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(border)
            .rounded_sm()
            .children(entries)
    }

    /// Shows the outcome of request `revision`, unless it was cancelled or replaced.
    pub fn finish_ask(
        &mut self,
        revision: u64,
        result: Result<SqlProposal, String>,
        cx: &mut Context<Self>,
    ) {
        if revision != self.ask_revision || !matches!(self.ask, AskState::Working(_)) {
            return;
        }
        self.ask = match result {
            Ok(proposal) => AskState::Proposal(proposal),
            Err(error) => AskState::Failed(error),
        };
        cx.notify();
    }

    /// Puts the proposal in the editor, selected, so Ctrl+Enter runs just it.
    pub fn accept_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let AskState::Proposal(proposal) = &self.ask else {
            return false;
        };
        let sql = proposal.sql.clone();
        let state = self.editor.read(cx).state().clone();
        let current = state.read(cx).value().to_string();
        let range = match self.ask_target.take() {
            Some(target) if target.text == current => target.range,
            _ => state.read(cx).selected_range(),
        };
        let (text, inner) = insertion(&current, range.clone(), &sql);
        state.update(cx, |state, cx| {
            state.set_selected_range(range.clone(), cx);
            state.replace(text, window, cx);
            state.set_selected_range(range.start + inner.start..range.start + inner.end, cx);
        });
        self.ask = AskState::Idle;
        self.ask_open = false;
        self.ask_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.focus(window, cx);
        cx.notify();
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
    fn ask_key(&mut self, key: AskKey, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.ask_input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        // The open `@` list takes Tab and Escape first.
        if self.mentions.read(cx).is_open() {
            match key {
                AskKey::Accept => {
                    return self
                        .mentions
                        .update(cx, |mentions, cx| mentions.accept(None, window, cx));
                }
                AskKey::Dismiss => {
                    return self.mentions.update(cx, |mentions, cx| mentions.close(cx));
                }
                AskKey::Run => {}
            }
        }
        match key {
            AskKey::Dismiss => {
                self.dismiss_ask(window, cx);
                true
            }
            AskKey::Accept => self.accept_ask(window, cx),
            // Without a proposal Ctrl+Enter runs the editor's SQL as usual.
            AskKey::Run => {
                let accepted = self.accept_ask(window, cx);
                if accepted && !self.busy {
                    cx.emit(SqlPanelEvent::Run(false));
                }
                accepted
            }
        }
    }

    /// Escape: cancels a request or drops its outcome, then closes the strip.
    pub fn dismiss_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.ask, AskState::Idle) {
            self.ask_open = false;
            self.focus(window, cx);
        } else {
            self.ask_revision += 1;
            self.ask = AskState::Idle;
            self.ask_target = None;
        }
        cx.notify();
    }

    /// Which provider a request goes to, and a reminder when it may send data.
    fn render_provider(&self, muted: Hsla) -> Div {
        let row = div().flex().flex_wrap().gap_x_2().text_sm();
        let Some(provider) = &self.ask_provider else {
            return row
                .text_color(muted)
                .child("No AI provider yet: add one with “AI providers” in the command palette");
        };
        let row = row.child(
            div()
                .text_color(muted)
                .child(format!("{} · {}", provider.name, provider.model)),
        );
        if provider.send_data {
            row.child(
                div().text_color(rgb(0xb7791f)).child(
                    "Data allowed: tables named with @ also send min, max, and frequent values",
                ),
            )
        } else {
            row.child(div().text_color(muted).child("No data values sent"))
        }
    }

    fn render_ask(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.tokens.border;
        let code_bg = theme.tokens.table_even;
        let mono = theme.mono_font_family.clone();
        let error_color: Hsla = rgb(0xb33c3c).into();
        let working = matches!(self.ask, AskState::Working(_));
        let status = match &self.ask {
            AskState::Idle => div().text_sm().text_color(muted).child(
                "Type @ to name tables: only those are described, with row, null, and distinct \
                 counts. Without @, every table's columns, types, and comments are sent. With \
                 SQL selected, describe a change to it.",
            ),
            AskState::Working(text) => div()
                .text_sm()
                .text_color(muted)
                .child(format!("{text} · Esc cancels")),
            AskState::Failed(error) => div().text_sm().text_color(error_color).child(error.clone()),
            AskState::Proposal(proposal) => {
                let (check, color) = match &proposal.check {
                    SqlCheck::Passed => ("✓ DuckDB accepts this SQL".to_owned(), muted),
                    SqlCheck::Partial { checked, total } => (
                        format!(
                            "✓ DuckDB accepts the first {checked} of {total} statements; the rest use what they create"
                        ),
                        muted,
                    ),
                    SqlCheck::Failed(error) => {
                        (format!("DuckDB still rejects it: {error}"), error_color)
                    }
                };
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("sql-ask-proposal")
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(code_bg)
                            .font_family(mono)
                            .child(proposal.sql.clone()),
                    )
                    .child(div().text_sm().text_color(color).child(check))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("sql-ask-insert")
                                    .primary()
                                    .label("Insert · Tab")
                                    .on_click(cx.listener(|panel, _, window, cx| {
                                        panel.accept_ask(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("sql-ask-run")
                                    .label("Insert and run · Ctrl+Enter")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|panel, _, window, cx| {
                                        if panel.accept_ask(window, cx) {
                                            cx.emit(SqlPanelEvent::Run(false));
                                        }
                                    })),
                            )
                            .child(
                                Button::new("sql-ask-discard")
                                    .label("Discard · Esc")
                                    .on_click(cx.listener(|panel, _, window, cx| {
                                        panel.dismiss_ask(window, cx)
                                    })),
                            ),
                    )
            }
        };
        div()
            .key_context("SqlAsk")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .px_3()
            .py_2()
            .border_t_1()
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_shrink_0().text_sm().child("Ask AI"))
                    .child(with_column_completion(
                        div().flex_1().min_w_0().child(Input::new(&self.ask_input)),
                        &self.mentions,
                        window,
                        cx,
                    ))
                    .child(
                        Button::new("sql-ask-log-toggle")
                            .label(if self.show_log {
                                "Hide log"
                            } else {
                                "Show log"
                            })
                            .tooltip("Show each request sent to the AI and each reply")
                            .on_click(cx.listener(|panel, _, _, cx| {
                                panel.show_log = !panel.show_log;
                                panel.log_scroll.scroll_to_bottom();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("sql-ask-generate")
                            .label("Generate · Enter")
                            .disabled(working)
                            .on_click(cx.listener(|panel, _, _, cx| panel.submit_ask(cx))),
                    )
                    .child(
                        Button::new("sql-ask-close")
                            .label("Close")
                            .on_click(cx.listener(|panel, _, window, cx| {
                                panel.dismiss_ask(window, cx);
                                panel.dismiss_ask(window, cx);
                            })),
                    ),
            )
            .child(self.render_provider(muted))
            .child(status)
            .when(self.show_log, |strip| strip.child(self.render_log(cx)))
    }

    /// Accepting a folder inserts `name/`, and the editor suppresses completion
    /// while it inserts, so the next level is listed by hand. Typing one `/`
    /// goes through the normal trigger instead.
    fn reopen_after_folder(&mut self, state: &Entity<EditorState>, cx: &mut Context<Self>) {
        let editor = state.read(cx);
        let cursor = editor.cursor();
        if !editor.value()[..cursor].ends_with(['/', '\\']) {
            return;
        }
        let Some(task) = path_completions(editor.text(), cursor, &self.path_roots, cx) else {
            return;
        };
        let state = state.clone();
        self.reopen_task = Some(cx.spawn(async move |_, cx| {
            let items = task.await;
            state.update(cx, |state, cx| {
                if state.cursor() == cursor {
                    state.present_completion_items(cursor, "", items, cx);
                }
            });
        }));
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.editor.read(cx).state().read(cx).focus_handle(cx), cx);
    }

    pub fn sql_to_run(&self, all: bool, cx: &App) -> String {
        let state = self.editor.read(cx).state().read(cx);
        let text = state.value().to_string();
        if all {
            return text;
        }
        let selection = state.selected_range();
        if !selection.is_empty() {
            return text[selection].to_string();
        }
        current_statement(&text, state.cursor())
            .map(|range| text[range].to_string())
            .unwrap_or_default()
    }
}

impl Render for SqlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.editor.read(cx).state().clone();
        div()
            .id("sql-panel")
            .key_context("SqlPanel")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .capture_action(cx.listener(
                |panel, action: &gpui_kit::component::input::Enter, _, cx| {
                    panel.accepting_completion = !action.secondary
                        && panel
                            .editor
                            .read(cx)
                            .state()
                            .read(cx)
                            .completion_menu_state()
                            .open;
                    cx.propagate();
                },
            ))
            .on_action(
                cx.listener(|panel, action: &gpui_kit::component::input::Enter, _, cx| {
                    // Kit accepts the item asynchronously but propagates Enter; suppress the fallback newline.
                    if !action.secondary && panel.accepting_completion {
                        panel.accepting_completion = false;
                        cx.stop_propagation();
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(cx.listener(|panel, _: &RunSql, window, cx| {
                if !panel.ask_key(AskKey::Run, window, cx) && !panel.busy {
                    cx.emit(SqlPanelEvent::Run(false));
                }
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|panel, _: &AcceptAsk, window, cx| {
                if !panel.ask_key(AskKey::Accept, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|panel, _: &SqlEscape, window, cx| {
                cx.stop_propagation();
                if panel.ask_key(AskKey::Dismiss, window, cx) {
                    return;
                }
                let state = panel.editor.read(cx).state().clone();
                if state.read(cx).completion_menu_state().open {
                    state.update(cx, |s, cx| s.dismiss_completion_overlay(cx));
                } else {
                    cx.emit(SqlPanelEvent::GridFocus);
                }
                cx.stop_propagation();
            }))
            .bg(cx.theme().tokens.background)
            .capture_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.to_ascii_lowercase();
                let modifiers = event.keystroke.modifiers;
                if modifiers.control && !modifiers.shift && (key == "k" || key == "i") {
                    panel.open_ask(window, cx);
                    cx.stop_propagation();
                    return;
                }
                let ask_key = match key.as_str() {
                    "enter" if modifiers.control => Some(AskKey::Run),
                    "escape" => Some(AskKey::Dismiss),
                    _ => None,
                };
                if ask_key.is_some_and(|ask_key| panel.ask_key(ask_key, window, cx)) {
                    cx.stop_propagation();
                    return;
                }
                if key == "enter" && event.keystroke.modifiers.control {
                    if !panel.busy {
                        cx.emit(SqlPanelEvent::Run(false));
                    }
                    cx.stop_propagation();
                } else if key == "escape" {
                    let state = panel.editor.read(cx).state().clone();
                    if state.read(cx).completion_menu_state().open {
                        state.update(cx, |state, cx| state.dismiss_completion_overlay(cx));
                    } else {
                        cx.emit(SqlPanelEvent::GridFocus);
                    }
                    cx.stop_propagation();
                } else if key == "j" && event.keystroke.modifiers.control {
                    cx.emit(SqlPanelEvent::Close);
                    cx.stop_propagation();
                }
                let _ = window;
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .flex_shrink_0()
                    .child(if self.startup_script {
                        "startup.sql"
                    } else {
                        "SQL"
                    })
                    .when(self.startup_script, |toolbar| {
                        toolbar.child(
                            Button::new("sql-save-script")
                                .label("Save")
                                .tooltip("Save startup.sql for the next database startup")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(SqlPanelEvent::Save))),
                        )
                    })
                    .child(
                        Checkbox::new("sql-auto-save")
                            .label("Auto-save")
                            .checked(self.auto_save)
                            .tooltip("Save SQL text after 2 seconds without typing")
                            .on_change(cx.listener(|_, checked: &bool, _, cx| {
                                cx.emit(SqlPanelEvent::AutoSave(*checked));
                            })),
                    )
                    // startup.sql already runs whenever the database opens.
                    .when(!self.startup_script, |toolbar| {
                        toolbar.child(
                            Checkbox::new("sql-auto-run")
                                .label("Auto-run")
                                .checked(self.auto_run)
                                .tooltip("Run the whole script each time this view opens")
                                .on_change(cx.listener(|_, checked: &bool, _, cx| {
                                    cx.emit(SqlPanelEvent::AutoRun(*checked));
                                })),
                        )
                    })
                    .child(
                        Button::new("sql-run")
                            .label("Run · Ctrl+Enter")
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(SqlPanelEvent::Run(false))),
                            ),
                    )
                    .child(
                        Button::new("sql-run-all")
                            .label("Run All")
                            .disabled(self.busy)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SqlPanelEvent::Run(true)))),
                    )
                    .child(
                        Button::new("sql-ask")
                            .label("Ask AI · Ctrl+K")
                            .tooltip("Describe the query you want and get SQL")
                            .on_click(
                                cx.listener(|panel, _, window, cx| panel.open_ask(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("sql-stop")
                            .label("Stop")
                            .disabled(!self.busy)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SqlPanelEvent::Stop))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .truncate()
                            .child(self.status.clone()),
                    )
                    .child(
                        Button::new("sql-close")
                            .label("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SqlPanelEvent::Close))),
                    ),
            )
            .when_some(self.error.as_ref(), |panel, error| {
                panel.child(div().px_3().text_color(rgb(0xb33c3c)).child(error.clone()))
            })
            .when(self.ask_open, |panel| {
                panel.child(self.render_ask(window, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&state).h_full().text_size(window.rem_size())),
            )
    }
}
