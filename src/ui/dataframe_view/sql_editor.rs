use super::*;
use crate::domain::ai::{AiEvent, SqlRequest, generate_sql, mentioned_relations};
use crate::domain::documents::Engine;
use crate::domain::sql_editor;
use crate::infrastructure::{ai_client, file_schemas, sql_execution::SqlExecution};
use crate::ui::sql_panel::{AskProvider, SqlAsk, SqlPanelEvent};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// The editor action a SQL editor command runs.
pub(crate) fn sql_editor_action(id: CommandId) -> Option<code_editor::Action> {
    use code_editor::Action;
    Some(match id {
        CommandId::SqlToggleLineComment => Action::ToggleLineComment,
        CommandId::SqlToggleBlockComment => Action::ToggleBlockComment,
        CommandId::SqlFind => Action::Find,
        CommandId::SqlReplace => Action::Replace,
        CommandId::SqlGoToLine => Action::GoToLine,
        CommandId::SqlShowCompletions => Action::TriggerSuggest,
        CommandId::SqlShowSignature => Action::TriggerParameterHints,
        CommandId::SqlAddCursorAbove => Action::AddCursorAbove,
        CommandId::SqlAddCursorBelow => Action::AddCursorBelow,
        CommandId::SqlAddNextOccurrence => Action::AddNextOccurrence,
        CommandId::SqlSelectAllOccurrences => Action::SelectAllOccurrences,
        CommandId::SqlMoveLinesUp => Action::MoveLinesUp,
        CommandId::SqlMoveLinesDown => Action::MoveLinesDown,
        CommandId::SqlCopyLinesUp => Action::CopyLinesUp,
        CommandId::SqlCopyLinesDown => Action::CopyLinesDown,
        CommandId::SqlDeleteLines => Action::DeleteLines,
        CommandId::SqlSelectLine => Action::SelectLine,
        CommandId::SqlExpandSelection => Action::ExpandSelection,
        CommandId::SqlShrinkSelection => Action::ShrinkSelection,
        CommandId::SqlJumpToBracket => Action::JumpToBracket,
        CommandId::SqlFold => Action::Fold,
        CommandId::SqlUnfold => Action::Unfold,
        CommandId::SqlFoldAll => Action::FoldAll,
        CommandId::SqlUnfoldAll => Action::UnfoldAll,
        CommandId::SqlToggleWordWrap => Action::ToggleWordWrap,
        CommandId::SqlInsertLineBelow => Action::InsertLineBelow,
        CommandId::SqlInsertLineAbove => Action::InsertLineAbove,
        _ => return None,
    })
}

impl DataFrameView {
    /// Opens the SQL editor for a screenshot, running sample SQL first for
    /// targets that show results.
    pub(crate) fn capture_sql_editor(&mut self, target: &str, cx: &mut Cx<Self>) {
        self.begin_new_sql(false, cx);
        if target == "editor:sql" {
            return;
        }
        self.loading = true;
        let target = target.to_owned();
        self.wait_for_sql_connection(target, Instant::now(), cx);
    }

    fn wait_for_sql_connection(&mut self, target: String, started: Instant, cx: &mut Cx<Self>) {
        if self.sql_interrupt.is_none() {
            if started.elapsed() > Duration::from_secs(20) {
                self.loading = false;
                return;
            }
            cx.timer(Duration::from_millis(20), move |view, cx| view.wait_for_sql_connection(target, started, cx))
                .detach();
            return;
        }
        let sql = "CREATE TEMP TABLE apples AS SELECT * FROM (VALUES ('red', 12), ('green', 8), ('gold', 15)) AS t(color, quantity);\nCREATE TEMP VIEW fruit AS SELECT * FROM apples;\nSELECT a.color, a.quantity FROM apples a ORDER BY a.quantity DESC;";
        let sql = if target == "editor:sql-error" {
            format!("{sql}\nSELECT a.missing FROM apples a;")
        } else {
            sql.into()
        };
        if let Some(panel) = &mut self.sql_panel {
            panel.editor.set_text(sql.clone());
        }
        self.session.set_sql_text(sql);
        self.run_sql(true, cx);
        self.wait_for_sql_run(target, cx);
    }

    fn wait_for_sql_run(&mut self, target: String, cx: &mut Cx<Self>) {
        if self.sql_running || self.busy {
            cx.timer(Duration::from_millis(20), move |view, cx| view.wait_for_sql_run(target, cx))
                .detach();
            return;
        }
        self.loading = false;
        let mut focus = self.focus;
        if target == "editor:sql-ask-mention" {
            // Captures run on a throwaway profile, so a sample provider is safe to save.
            if let Ok(mut settings) = self.repository.load_settings() {
                settings.ai.providers = vec![crate::domain::ai::AiProvider {
                    name: "AI Factory".into(),
                    model: "gpt-6-sole".into(),
                    send_data: true,
                    ..Default::default()
                }];
                let _ = self.repository.save_settings(&settings);
            }
            if let Some(panel) = &mut self.sql_panel {
                panel.open_ask(&mut focus);
                panel.ask_input.set_text("total quantity per color from @ap");
                panel.mentions.refresh(&panel.ask_input);
            }
        } else if target.starts_with("editor:sql-ask") {
            use crate::domain::ai::{ChatMessage, SqlCheck};
            // Tall enough to show the proposal and log above the editor.
            self.sql_editor_height = 760.0;
            if let Some(panel) = &mut self.sql_panel {
                panel.ask_input.set_text("total quantity per color, largest first");
                panel.open_ask(&mut focus);
                let sql = "SELECT color, sum(quantity) AS total_quantity\nFROM apples\nGROUP BY color\nORDER BY total_quantity DESC;";
                panel.ask = crate::ui::sql_panel::AskState::Proposal(crate::domain::ai::SqlProposal {
                    sql: sql.into(),
                    check: SqlCheck::Passed,
                });
                if target == "editor:sql-ask-log" {
                    panel.show_log = true;
                    panel.ask_log = vec![
                        AiEvent::Sent(vec![
                            ChatMessage::new(
                                "system",
                                "You write SQL for DuckDB inside DataDiver…\n\nRelations (name: columns):\ntemp.apples: color VARCHAR, quantity INTEGER",
                            ),
                            ChatMessage::new("user", "total quantity per color, largest first"),
                        ]),
                        AiEvent::Received {
                            text: "```sql\nSELECT colour, sum(quantity) FROM apples GROUP BY colour\n```".into(),
                            elapsed: Duration::from_millis(1840),
                        },
                        AiEvent::Checked(SqlCheck::Failed("Binder Error: Referenced column \"colour\" not found".into())),
                        AiEvent::Sent(vec![ChatMessage::new(
                            "user",
                            "DuckDB rejected that SQL:\nBinder Error: Referenced column \"colour\" not found\nFix it.",
                        )]),
                        AiEvent::Received {
                            text: format!("```sql\n{sql}\n```"),
                            elapsed: Duration::from_millis(1210),
                        },
                        AiEvent::Checked(SqlCheck::Passed),
                    ];
                }
            }
        }
        if target == "editor:sql-completion"
            && let Some(panel) = &mut self.sql_panel
        {
            let text = "SELECT\n    a\nFROM apples a";
            let offset = text.find(" a").unwrap() + 2;
            panel.editor.set_text(text);
            panel.editor.set_selection(offset..offset);
            // Typing the `.` opens the column list, as it would for the user.
            panel.editor.type_text(".");
            focus = Some(panel.editor.id);
        }
        if target == "editor:sql-hints"
            && let Some(panel) = &mut self.sql_panel
        {
            // A statement missing its condition, and a call being typed.
            let text = "SELECT * FROM apples WHERE;
SELECT round(a.quantity
FROM apples a";
            let offset = text.find("quantity").unwrap() + "quantity".len();
            panel.editor.set_text(text);
            panel.editor.set_selection(offset..offset);
            panel.editor.type_text(",");
            panel.editor.diagnostics();
            focus = Some(panel.editor.id);
        }
        self.set_focus(focus, cx);
        self.handle_sql_panel_events(cx);
        cx.notify();
    }

    pub(crate) fn reset_sql_editor(&mut self) {
        self.auto_save_task = None;
        self.stop_sql();
        self.sql_revision += 1;
        self.sql_panel = None;
        self.sql_panel_open = false;
        self.sql_running = false;
        self.sql_service = Arc::new(Mutex::new(None));
        self.sql_polars = Arc::new(Mutex::new(polars::sql::SQLContext::new()));
        self.sql_interrupt = None;
        self.sql_stopped = None;
        self.sql_restore_sort = false;
        self.sql_catalog_refreshing = false;
        self.sql_result_complete = false;
        self.sql_result_truncated = false;
    }

    /// Shows the editor of the SQL source or script that is open, or of the
    /// SQL source whose rows the view reads. Anything else has no SQL to edit.
    pub(crate) fn open_sql_editor(&mut self, cx: &mut Cx<Self>) {
        if self.session.is_sql() {
            self.begin_sql_editor(cx);
            return;
        }
        if let Some(id) = self.session.working.as_ref().and_then(|d| d.sql_source.clone()) {
            if let Err(error) = self.open_sql_source_editor(&id, cx) {
                self.error = Some(format!("Cannot open the SQL source: {error}"));
            }
            cx.notify();
            return;
        }
        self.copy_status =
            Some("The SQL editor opens SQL sources and scripts; start one with New SQL source or New SQL script".into());
        cx.notify();
    }

    /// Navigates to the saved SQL source `id` with its editor shown.
    pub(crate) fn open_sql_source_editor(&mut self, id: &str, cx: &mut Cx<Self>) -> std::io::Result<()> {
        let source = self.repository.load_view(id)?;
        self.navigate_to_file(FileEntry::View(source), false, false, false, cx);
        // Set after navigating starts, which drops a command from an earlier location.
        self.after_open = Some(CommandId::SqlEditor);
        Ok(())
    }

    /// Starts an empty SQL source, or with `script` a SQL script, replacing
    /// the working document even when it is already SQL.
    pub(crate) fn new_sql(&mut self, script: bool, cx: &mut Cx<Self>) {
        let empty_sql = matches!(
            self.session.working.as_ref().map(|d| &d.view),
            Some(ViewKind::Sql(config)) if config.text.trim().is_empty()
        );
        let kind = if script { "SQL script" } else { "SQL source" };
        if self.session.dirty && !empty_sql {
            self.confirm(
                format!("Start a new {kind}?"),
                format!("The current view has unsaved changes. A new {kind} replaces the working document."),
                format!("New {kind}"),
                false,
                dialogs::ConfirmAction::NewSql { script },
                cx,
            );
        } else {
            self.begin_new_sql(script, cx);
        }
    }

    pub(crate) fn begin_new_sql(&mut self, script: bool, cx: &mut Cx<Self>) {
        self.start_sql_session(Some(script), cx);
    }

    pub(crate) fn begin_sql_editor(&mut self, cx: &mut Cx<Self>) {
        self.start_sql_session(None, cx);
    }

    /// Opens the SQL editor: on a new SQL source, or with `Some(true)` a new
    /// SQL script, unless `new` is `None` and the working document is already SQL.
    fn start_sql_session(&mut self, new: Option<bool>, cx: &mut Cx<Self>) {
        let new = new.or((!self.session.is_sql()).then_some(false));
        if let Some(script) = new {
            self.rollback_navigation(cx);
            self.autosave_table_preferences(cx);
            self.remember_navigation_current();
            self.reset_sql_editor();
            self.session.start_sql_draft(script);
            self.computation.invalidate(self.session.token());
            self.loading = false;
            self.debouncing = false;
            self.busy = self.computation.is_running();
            self.error = None;
            self.table_preferences_error = None;
            self.source_modified = None;
            self.query_cache = None;
            self.table_query = None;
            self.slicer_stage = None;
            self.slicer_sidebar.set_cards(Vec::new());
            self.grid.set_frozen_count(0);
            self.grid.set_sort(Vec::new());
            self.grid.set_keymap(self.keymap.clone(), KeyContext::QueryGrid);
            self.commit_navigation_history();
        }
        self.ensure_sql_panel(cx);
        self.sql_panel_open = true;
        if let Some(panel) = &self.sql_panel {
            let id = panel.editor.id;
            self.set_focus(Some(id), cx);
        }
        cx.notify();
    }

    /// Detaches attached databases and runs the startup script again, then
    /// gives this window's SQL editor a new connection, which drops its
    /// temporary tables and session settings.
    pub(crate) fn reset_database_connection(&mut self, cx: &mut Cx<Self>) {
        self.stop_sql();
        self.copy_status = Some("Resetting the database connection…".into());
        cx.notify();
        let database = self.database.clone();
        cx.spawn(
            move || database.reset_connection(),
            |view: &mut Self, cx, result| {
                view.copy_status = None;
                match result {
                    Ok(startup_error) => {
                        if view.sql_panel.is_some() {
                            view.reset_sql_editor();
                            view.ensure_sql_panel(cx);
                        }
                        if view.session.working.as_ref().is_some_and(|d| d.database_view.is_some()) {
                            view.reload(false, cx);
                        }
                        view.refresh_database_tree(cx);
                        match startup_error {
                            Some(error) => view.error = Some(error),
                            None => view.copy_status = Some("Database connection reset".into()),
                        }
                    }
                    Err(error) => view.error = Some(format!("Cannot reset the database connection: {error}")),
                }
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn ensure_sql_panel(&mut self, cx: &mut Cx<Self>) {
        if self.sql_panel.is_some() {
            return;
        }
        let Some(ViewKind::Sql(config)) = self.session.working.as_ref().map(|d| &d.view) else {
            return;
        };
        let mut panel = SqlPanel::new(config.text.clone(), cx.hwnd());
        panel.set_editor_settings(&self.editor_settings);
        let startup_script = matches!(self.session.identity, SessionIdentity::StartupSql);
        let auto_run = config.auto_run && !startup_script;
        panel.startup_script = startup_script;
        panel.busy = true;
        panel.status = "Opening database…".into();
        let catalog_handle = panel.catalog.clone();
        let roots_handle = panel.path_roots.clone();
        self.sql_panel = Some(panel);
        self.sql_panel_open = true;
        let service = self.sql_service.clone();
        let database = self.database.clone();
        let startup_database = database.clone();
        let revision = self.sql_revision;
        cx.spawn(
            move || {
                let mut guard = service.lock().map_err(|_| "SQL connection lock poisoned".to_string())?;
                let execution = SqlExecution::new(database)?;
                let catalog = execution.sql_catalog()?;
                let handles = (
                    execution.interrupt.clone(),
                    execution.stopped.clone(),
                    catalog,
                    execution.path_roots(),
                    execution.default_catalog()?,
                );
                *guard = Some(execution);
                Ok::<_, String>(handles)
            },
            move |view: &mut Self, cx, result| {
                if revision != view.sql_revision {
                    return;
                }
                let opened = result.is_ok();
                match result {
                    Ok((interrupt, stopped, catalog, path_roots, default_catalog)) => {
                        view.sql_interrupt = Some(interrupt);
                        view.sql_stopped = Some(stopped);
                        view.sql_catalog_revision = view.database.revision();
                        catalog_handle.write().unwrap().replace(catalog);
                        *roots_handle.write().unwrap() = path_roots;
                        view.refresh_file_schemas(cx);
                        if let Some(panel) = &mut view.sql_panel {
                            panel.editor.refresh_language();
                            panel.default_catalog = default_catalog;
                            panel.busy = false;
                            panel.status = "Ctrl+Enter · selection or current statement".into();
                            if startup_script {
                                panel.error = startup_database.startup_error();
                                panel.status = "Runs when the database opens · Ctrl+Enter runs now".into();
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(panel) = &mut view.sql_panel {
                            panel.busy = false;
                            panel.error = Some(error);
                        }
                    }
                }
                cx.notify();
                if auto_run && opened {
                    view.run_sql(true, cx);
                }
            },
        )
        .detach();
    }

    /// Handles what the SQL panel reported since the last call.
    pub(crate) fn handle_sql_panel_events(&mut self, cx: &mut Cx<Self>) {
        loop {
            let events = match &mut self.sql_panel {
                Some(panel) => panel.take_events(),
                None => return,
            };
            if events.is_empty() {
                return;
            }
            for event in events {
                self.handle_sql_panel_event(event, cx);
            }
        }
    }

    fn handle_sql_panel_event(&mut self, event: SqlPanelEvent, cx: &mut Cx<Self>) {
        match event {
            SqlPanelEvent::Changed(text) => {
                self.session.set_sql_text(text);
                self.schedule_auto_save(cx);
                self.schedule_file_schemas(cx);
                cx.notify();
            }
            SqlPanelEvent::Save => {
                self.persist_view();
                cx.notify();
            }
            SqlPanelEvent::Run(all) => self.run_sql(all, cx),
            SqlPanelEvent::Stop => {
                self.stop_sql();
                cx.notify();
            }
            SqlPanelEvent::Close => {
                self.sql_panel_open = false;
                self.focus_grid(cx);
                cx.notify();
            }
            SqlPanelEvent::GridFocus => self.focus_grid(cx),
            SqlPanelEvent::Ask(ask) => self.ask_sql(&ask, cx),
            SqlPanelEvent::AskOpened => {
                let provider = self.repository.load_settings().ok().and_then(|settings| {
                    settings.ai.active().map(|provider| AskProvider {
                        name: provider.name.clone(),
                        model: provider.model.clone(),
                        send_data: provider.send_data,
                    })
                });
                if let Some(panel) = &mut self.sql_panel {
                    panel.ask_provider = provider;
                }
                cx.notify();
            }
        }
    }

    /// Writes SQL for `ask` with the default AI provider on a worker,
    /// checking it on this editor's connection.
    fn ask_sql(&mut self, ask: &SqlAsk, cx: &mut Cx<Self>) {
        if self.sql_panel.is_none() {
            return;
        }
        let revision = ask.revision;
        let provider = match self.repository.load_settings() {
            Ok(settings) => settings.ai.active().cloned().ok_or_else(|| {
                "No AI provider yet: add one with “AI providers” in the command palette (Ctrl+Shift+P)".to_owned()
            }),
            Err(error) => Err(format!("Cannot read settings: {error}")),
        };
        let provider = match provider {
            Ok(provider) => provider,
            Err(error) => {
                if let Some(panel) = &mut self.sql_panel {
                    panel.finish_ask(revision, Err(error));
                }
                return;
            }
        };
        if let Some(panel) = &mut self.sql_panel {
            let text = format!("Asking {} ({})…", provider.name, provider.model);
            let endpoint = format!("{} · model {}", provider.chat_url(), provider.model);
            panel.set_ask_working(revision, text, endpoint);
        }
        let result_columns = self
            .session
            .source
            .schema
            .iter()
            .map(|column| (column.name.clone(), column.sql_type.clone()))
            .collect();
        let mut request = SqlRequest {
            prompt: ask.prompt.clone(),
            relations: Vec::new(),
            result_columns,
            editor_text: ask.editor_text.clone(),
            selection: ask.selection.clone(),
        };
        let service = self.sql_service.clone();
        let log = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        // Copies the log to the panel while the request runs, so it shows each
        // step as it happens.
        self.follow_ask_log(revision, log.clone(), done.clone(), cx);
        let events = log.clone();
        cx.spawn(
            move || {
                let push = |event| {
                    if let Ok(mut log) = events.lock() {
                        log.push(event);
                    }
                };
                // Run as one closure so `done` is set on every exit, early
                // returns included; otherwise the log poller never stops.
                let mut ask = || {
                    // Locked per call so a slow reply never holds up a SQL run.
                    // The editor's connection, since its temp tables live there.
                    let mut relations = with_sql_service(&service, |service| service.ai_schema())
                        .inspect_err(|error| push(AiEvent::Failed(error.clone())))?;
                    for index in mentioned_relations(&request.prompt, &relations) {
                        let started = Instant::now();
                        let relation = &relations[index];
                        let profile = with_sql_service(&service, |service| service.profile_relation(relation, provider.send_data));
                        push(AiEvent::Note(match &profile {
                            Ok(profile) => format!(
                                "Profiled {} · {} rows in {:.1} s{}",
                                relation.name,
                                profile.rows,
                                started.elapsed().as_secs_f32(),
                                if provider.send_data { ", with data values" } else { ", counts only" }
                            ),
                            Err(error) => format!("Could not profile {}: {error}", relation.name),
                        }));
                        relations[index].profile = profile.ok();
                    }
                    request.relations = relations;
                    generate_sql(
                        &request,
                        |messages| ai_client::chat(&provider, messages),
                        |sql| with_sql_service(&service, |service| service.prepare_only(sql)),
                        push,
                    )
                };
                let result = ask();
                done.store(true, Ordering::Release);
                result
            },
            move |view: &mut Self, cx, result| {
                let events = log.lock().map(|log| log.clone()).unwrap_or_default();
                if let Some(panel) = &mut view.sql_panel {
                    panel.set_ask_log(revision, events);
                    panel.finish_ask(revision, result);
                }
                cx.notify();
            },
        )
        .detach();
    }

    fn follow_ask_log(&mut self, revision: u64, log: Arc<Mutex<Vec<AiEvent>>>, done: Arc<AtomicBool>, cx: &mut Cx<Self>) {
        if done.load(Ordering::Acquire) {
            return;
        }
        cx.timer(Duration::from_millis(150), move |view, cx| {
            let events = log.lock().map(|log| log.clone()).unwrap_or_default();
            if let Some(panel) = &mut view.sql_panel
                && panel.set_ask_log(revision, events)
            {
                cx.notify();
            }
            view.follow_ask_log(revision, log, done, cx);
        })
        .detach();
    }

    /// Runs the whole script each time it opens. Kept like Auto-save: a
    /// saved script stores it at once.
    pub(crate) fn set_auto_run(&mut self, enabled: bool, cx: &mut Cx<Self>) {
        if let Some(ViewKind::Sql(config)) = self.session.working.as_mut().map(|d| &mut d.view) {
            config.auto_run = enabled;
            self.session.dirty = true;
        }
        if matches!(self.session.identity, SessionIdentity::SavedSql(_)) {
            self.persist_view();
        }
        cx.notify();
    }

    pub(crate) fn stop_sql(&mut self) {
        // A Polars query may not reach its next batch for a long while, so
        // the editor lets it go at once; what it returns is dropped.
        if let Some(stop) = self.sql_polars_stop.take() {
            stop.store(true, Ordering::Release);
            self.sql_running = false;
            if let Some(panel) = &mut self.sql_panel {
                panel.busy = false;
                panel.status = "Stopped".into();
            }
        }
        if let Some(stopped) = &self.sql_stopped {
            stopped.store(true, Ordering::Release);
        }
        if let Some(interrupt) = &self.sql_interrupt {
            interrupt.interrupt();
        }
    }

    pub(crate) fn refresh_sql_catalog(&mut self, cx: &mut Cx<Self>) {
        if self.sql_running
            || self.sql_catalog_refreshing
            || self.sql_interrupt.is_none()
            || self.sql_catalog_revision == self.database.revision()
        {
            return;
        }
        let Some(panel) = &self.sql_panel else {
            return;
        };
        let catalog_handle = panel.catalog.clone();
        self.sql_catalog_refreshing = true;
        let service = self.sql_service.clone();
        let revision = self.sql_revision;
        let catalog_revision = self.database.revision();
        cx.spawn(
            move || {
                service
                    .lock()
                    .map_err(|_| "SQL connection lock poisoned".to_string())?
                    .as_ref()
                    .ok_or_else(|| "SQL connection unavailable".to_string())?
                    .sql_catalog()
            },
            move |view: &mut Self, cx, result| {
                // Clear before the staleness check: a stopped run bumps the revision
                // without resetting this flag, which would block every later refresh.
                view.sql_catalog_refreshing = false;
                if revision != view.sql_revision {
                    return;
                }
                if let Ok(catalog) = result {
                    catalog_handle.write().unwrap().replace(catalog);
                    view.sql_catalog_revision = catalog_revision;
                    if let Some(panel) = &mut view.sql_panel {
                        panel.editor.refresh_language();
                    }
                }
                cx.notify();
            },
        )
        .detach();
    }

    fn schedule_file_schemas(&mut self, cx: &mut Cx<Self>) {
        self.sql_file_schema_task = Some(cx.timer(Duration::from_millis(300), |view, cx| {
            view.refresh_file_schemas(cx);
        }));
    }

    /// Describes, on a worker, the files the script reads that are new or
    /// changed since they were last described, so `alias.` completes their columns.
    pub(crate) fn refresh_file_schemas(&mut self, cx: &mut Cx<Self>) {
        self.sql_file_schema_task = None;
        if self.sql_describing {
            self.sql_describe_again = true;
            return;
        }
        let Some(panel) = &self.sql_panel else {
            return;
        };
        let sources = sql_editor::file_sources(&panel.editor.text());
        if sources.is_empty() {
            return;
        }
        let roots = panel.path_roots.read().map(|roots| roots.clone()).unwrap_or_default();
        let schemas = self.sql_file_schemas.clone();
        let database = self.database.clone();
        self.sql_describing = true;
        cx.spawn(
            move || file_schemas::refresh(&schemas, &sources, &roots, || file_schemas::connection(&database, &roots)),
            move |view: &mut Self, cx, result| {
                view.sql_describing = false;
                if let Ok((columns, described)) = result
                    && let Some(panel) = &mut view.sql_panel
                {
                    panel.catalog.write().unwrap().files = columns;
                    if described {
                        panel.editor.refresh_language();
                        panel.editor.refresh_suggest(view.focus == Some(panel.editor.id));
                    }
                    cx.notify();
                }
                if std::mem::take(&mut view.sql_describe_again) {
                    view.refresh_file_schemas(cx);
                }
            },
        )
        .detach();
    }

    /// Keys the keymap binds in the SQL editor: running, asking AI, and the
    /// editor's own actions, which follow the keymap so they can be rebound.
    /// A default the keymap unbinds does nothing rather than reach the editor.
    pub(crate) fn sql_editor_key(&mut self, event: &crate::ui::platform::KeyEvent, cx: &mut Cx<Self>) -> bool {
        use crate::application::keymap::{ACTIONS, ActionId, canonical_keystroke};
        let Ok(stroke) = canonical_keystroke(&event.stroke()) else {
            return false;
        };
        let in_editor = |id: ActionId| ACTIONS.iter().any(|s| s.id == id && s.context == KeyContext::SqlEditor);
        match self.keymap.binding_for(&stroke, KeyContext::SqlEditor) {
            Some(action) if in_editor(action) => {
                self.dispatch_keymap_action(action, cx);
                true
            }
            _ => ACTIONS
                .iter()
                .any(|s| s.context == KeyContext::SqlEditor && s.default_bindings.contains(&stroke.as_str())),
        }
    }

    /// A SQL editor command from a key or the command palette: focuses the
    /// editor and runs its action there.
    pub(crate) fn run_sql_editor_action(&mut self, action: code_editor::Action, cx: &mut Cx<Self>) {
        let Some(panel) = self.sql_panel.as_mut().filter(|_| self.sql_panel_open) else {
            return;
        };
        let id = panel.editor.id;
        panel.editor.run(action);
        self.set_focus(Some(id), cx);
        self.handle_sql_panel_events(cx);
    }

    pub(crate) fn run_sql(&mut self, all: bool, cx: &mut Cx<Self>) {
        if self.sql_running || !self.session.is_sql() {
            return;
        }
        if self.sql_engine() == Engine::Polars {
            self.run_polars_sql(all, cx);
            return;
        }
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        if self.sql_interrupt.is_none() {
            panel.error = Some("Database connection is still opening; try Run again".into());
            cx.notify();
            return;
        }
        let text = panel.sql_to_run(all);
        if text.trim().is_empty() {
            return;
        }
        let catalog_handle = panel.catalog.clone();
        let roots_handle = panel.path_roots.clone();
        self.sql_running = true;
        self.sql_stopped.as_ref().unwrap().store(false, Ordering::Release);
        panel.busy = true;
        panel.error = None;
        panel.status = "Running…".into();
        let service = self.sql_service.clone();
        let revision = self.sql_revision;
        let limit = self.query_limit;
        cx.spawn(
            move || {
                service
                    .lock()
                    .map_err(|_| "SQL connection lock poisoned".to_string())?
                    .as_ref()
                    .ok_or_else(|| "SQL connection unavailable".to_string())
                    .map(|service| service.run(&text, limit))
            },
            move |view: &mut Self, cx, outcome| {
                if revision != view.sql_revision || !view.session.is_sql() {
                    return;
                }
                view.sql_running = false;
                match outcome {
                    Ok(mut run) => {
                        view.sql_catalog_revision = view.database.revision();
                        catalog_handle.write().unwrap().replace(run.catalog);
                        *roots_handle.write().unwrap() = run.path_roots;
                        // The run may have written a file the script reads.
                        view.refresh_file_schemas(cx);
                        if run.result.is_some() {
                            view.sql_result_complete = run.complete;
                            view.sql_result_truncated = run.truncated;
                            if !run.complete {
                                run.status.push_str(
                                    " · slicers and = / - filters are off: the last statement is not a plain query",
                                );
                            }
                        }
                        if let Some(panel) = &mut view.sql_panel {
                            panel.editor.refresh_language();
                            panel.busy = false;
                            panel.status = run.status;
                            panel.error = run.error;
                        }
                        if let Some(store) = run.result.take() {
                            view.show_sql_result(store.into(), cx);
                        }
                    }
                    Err(error) => {
                        if let Some(panel) = &mut view.sql_panel {
                            panel.busy = false;
                            panel.error = Some(error);
                        }
                    }
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    /// The engine the SQL in the editor runs on; the startup script always
    /// runs on DuckDB.
    pub(crate) fn sql_engine(&self) -> Engine {
        match self.session.working.as_ref().map(|d| &d.view) {
            Some(ViewKind::Sql(config)) if !matches!(self.session.identity, SessionIdentity::StartupSql) => config.engine,
            _ => Engine::DuckDb,
        }
    }

    /// Switches the engine the SQL runs on. A saved source or script stores
    /// it at once, and its next run uses the new engine.
    pub(crate) fn set_sql_engine(&mut self, engine: Engine, cx: &mut Cx<Self>) {
        if matches!(self.session.identity, SessionIdentity::StartupSql) || self.sql_engine() == engine {
            return;
        }
        if let Some(ViewKind::Sql(config)) = self.session.working.as_mut().map(|d| &mut d.view) {
            config.engine = engine;
            self.session.dirty = true;
        }
        // Tables an earlier run made belong to the other engine.
        self.sql_polars = Arc::new(Mutex::new(polars::sql::SQLContext::new()));
        if matches!(self.session.identity, SessionIdentity::SavedSql(_)) {
            self.persist_view();
        }
        if let Some(panel) = &mut self.sql_panel {
            panel.status = format!("Runs on {} · Ctrl+Enter · selection or current statement", sql_engine_label(engine));
        }
        cx.notify();
    }

    /// Runs the selection, the current statement, or with `all` the whole
    /// script on Polars SQL. Statements share this editor's context, so
    /// tables a run creates are there for the next one.
    fn run_polars_sql(&mut self, all: bool, cx: &mut Cx<Self>) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let text = panel.sql_to_run(all);
        if text.trim().is_empty() {
            return;
        }
        self.sql_running = true;
        panel.busy = true;
        panel.error = None;
        panel.status = "Running on Polars SQL…".into();
        let context = self.sql_polars.clone();
        let revision = self.sql_revision;
        let limit = self.query_limit;
        let stop = Arc::new(AtomicBool::new(false));
        self.sql_polars_stop = Some(stop.clone());
        cx.spawn(
            {
                let stop = stop.clone();
                move || {
                    let started = Instant::now();
                    // Planning needs the context; reading the rows does not, so a
                    // stopped read never holds up the next run.
                    let planned = crate::infrastructure::polars::sql::plan(
                        &mut *context.lock().map_err(|_| "Polars SQL lock poisoned".to_string())?,
                        &text,
                    );
                    Ok::<_, String>((planned.collect(limit, &stop), started.elapsed()))
                }
            },
            move |view: &mut Self, cx, outcome| {
                // Stop already let the run go.
                if revision != view.sql_revision || !view.session.is_sql() || stop.load(Ordering::Acquire) {
                    return;
                }
                view.sql_polars_stop = None;
                view.sql_running = false;
                let (result, truncated, status, error) = match outcome {
                    Ok((run, elapsed)) => {
                        let status = format!(
                            "{} statement{} completed in {:.2}s on Polars SQL{}",
                            run.completed,
                            if run.completed == 1 { "" } else { "s" },
                            elapsed.as_secs_f64(),
                            if run.truncated {
                                " · result truncated at row limit"
                            } else {
                                ""
                            }
                        );
                        (run.result, run.truncated, status, run.error)
                    }
                    Err(error) => (None, false, String::new(), Some(error)),
                };
                if let Some(panel) = &mut view.sql_panel {
                    panel.busy = false;
                    if !status.is_empty() {
                        panel.status = status;
                    }
                    panel.error = error;
                }
                if let Some(store) = result {
                    // Polars keeps every row up to the limit, so slicers and
                    // filters see them all unless it cut the result.
                    view.sql_result_complete = !truncated;
                    view.sql_result_truncated = truncated;
                    view.show_sql_result(store.into(), cx);
                }
                cx.notify();
            },
        )
        .detach();
        cx.notify();
    }

    /// Shows a run's rows in the grid, keeping the sort when the view just opened.
    fn show_sql_result(&mut self, store: crate::infrastructure::store::Store, cx: &mut Cx<Self>) {
        let sort = if self.sql_restore_sort {
            self.session.sort.clone()
        } else {
            Vec::new()
        };
        self.sql_restore_sort = false;
        let source_revision = self.session.source.source_revision + 1;
        self.session.replace_source(FrameSnapshot::new(store, source_revision));
        self.session.sort = available_sort_keys(&sort, self.session.source.schema.as_ref());
        if let Some(ViewKind::Sql(config)) = self.session.working.as_mut().map(|d| &mut d.view) {
            config.result_sort = self.session.sort.clone();
        }
        self.query_cache = None;
        self.table_query = None;
        self.slicer_stage = None;
        self.grid.reset_widths();
        self.set_search_text(String::new(), cx);
    }
}

pub(crate) fn sql_engine_label(engine: Engine) -> &'static str {
    match engine {
        Engine::DuckDb => "DuckDB",
        Engine::Polars => "Polars SQL",
    }
}

fn with_sql_service<T>(service: &Mutex<Option<SqlExecution>>, f: impl FnOnce(&SqlExecution) -> Result<T, String>) -> Result<T, String> {
    let guard = service
        .lock()
        .map_err(|_| "SQL connection lock poisoned".to_string())?;
    f(guard.as_ref().ok_or_else(|| "SQL connection is still opening".to_string())?)
}
