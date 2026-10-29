use super::*;
use crate::domain::ai::{AiEvent, SqlRequest, generate_sql, mentioned_relations};
use crate::infrastructure::{ai_client, sql_execution::SqlExecution};
use crate::ui::sql_panel::{AskProvider, SqlAsk, SqlPanelEvent};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

impl DataFrameView {
    /// Opens the SQL editor for a screenshot, running sample SQL first for
    /// targets that show results.
    pub(crate) fn capture_sql_editor(&mut self, target: &str, cx: &mut Cx<Self>) {
        self.begin_sql_editor(cx);
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
            let text = "SELECT\n    a.\nFROM apples a";
            let offset = text.find("a.").unwrap() + 2;
            panel.editor.set_text(text);
            panel.editor.set_selection(offset..offset);
            let catalog = panel.catalog.read().map(|catalog| catalog.clone()).unwrap_or_default();
            let items = crate::domain::sql_editor::completions(text, offset, &catalog)
                .into_iter()
                .map(|item| crate::ui::widgets::editor::CompletionItem {
                    label: item.label,
                    detail: None,
                    insert: item.insert,
                    range: item.range,
                })
                .collect();
            panel.editor.present_completion(items);
            focus = Some(panel.editor.id);
        }
        self.set_focus(focus, cx);
        self.handle_sql_panel_events(cx);
        cx.notify();
    }

    pub(crate) fn reset_sql_editor(&mut self) {
        self.sql_auto_save_task = None;
        self.stop_sql();
        self.sql_revision += 1;
        self.sql_panel = None;
        self.sql_panel_open = false;
        self.sql_running = false;
        self.sql_service = Arc::new(Mutex::new(None));
        self.sql_interrupt = None;
        self.sql_stopped = None;
        self.sql_restore_sort = false;
        self.sql_catalog_refreshing = false;
        self.sql_result_complete = false;
    }

    pub(crate) fn open_sql_editor(&mut self, cx: &mut Cx<Self>) {
        if !self.session.is_sql() && self.session.dirty {
            self.confirm(
                "Open SQL editor?".into(),
                "The current view has unsaved changes. Opening SQL replaces the working document.".into(),
                "Open SQL editor".into(),
                false,
                dialogs::ConfirmAction::OpenSqlEditor,
                cx,
            );
        } else {
            self.begin_sql_editor(cx);
        }
    }

    /// Starts an empty SQL draft, replacing the working document even when it
    /// is already SQL.
    pub(crate) fn new_sql_query(&mut self, cx: &mut Cx<Self>) {
        let empty_sql = matches!(
            self.session.working.as_ref().map(|d| &d.view),
            Some(ViewKind::Sql(config)) if config.text.trim().is_empty()
        );
        if self.session.dirty && !empty_sql {
            self.confirm(
                "Start a new SQL query?".into(),
                "The current view has unsaved changes. A new query replaces the working document.".into(),
                "New SQL query".into(),
                false,
                dialogs::ConfirmAction::NewSqlQuery,
                cx,
            );
        } else {
            self.begin_new_sql_query(cx);
        }
    }

    pub(crate) fn begin_new_sql_query(&mut self, cx: &mut Cx<Self>) {
        self.start_sql_session(true, cx);
    }

    pub(crate) fn begin_sql_editor(&mut self, cx: &mut Cx<Self>) {
        self.start_sql_session(false, cx);
    }

    /// Opens the SQL editor, on a new draft unless the working document is
    /// already SQL and `fresh` is off.
    fn start_sql_session(&mut self, fresh: bool, cx: &mut Cx<Self>) {
        if fresh || !self.session.is_sql() {
            self.rollback_navigation(cx);
            self.autosave_table_preferences(cx);
            self.remember_navigation_current();
            self.reset_sql_editor();
            self.session.start_sql_draft();
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
        let mut panel = SqlPanel::new(config.text.clone());
        let startup_script = matches!(self.session.identity, SessionIdentity::StartupSql);
        let auto_run = config.auto_run && !startup_script;
        panel.startup_script = startup_script;
        panel.auto_save = config.auto_save;
        panel.auto_run = config.auto_run;
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
                let catalog = execution.catalog()?;
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
                        *catalog_handle.write().unwrap() = catalog;
                        *roots_handle.write().unwrap() = path_roots;
                        if let Some(panel) = &mut view.sql_panel {
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
                self.schedule_sql_auto_save(cx);
                cx.notify();
            }
            SqlPanelEvent::AutoRun(enabled) => {
                if let Some(ViewKind::Sql(config)) = self.session.working.as_mut().map(|d| &mut d.view) {
                    config.auto_run = enabled;
                    self.session.dirty = true;
                }
                if let Some(panel) = &mut self.sql_panel {
                    panel.auto_run = enabled;
                }
                // Kept like Auto-save: a saved view stores it at once.
                if matches!(self.session.identity, SessionIdentity::SavedSql(_)) {
                    self.persist_sql_view();
                }
                cx.notify();
            }
            SqlPanelEvent::AutoSave(enabled) => {
                let was_dirty = self.session.dirty;
                if let Some(ViewKind::Sql(config)) = self.session.working.as_mut().map(|d| &mut d.view) {
                    config.auto_save = enabled;
                    self.session.dirty = true;
                }
                if let Some(panel) = &mut self.sql_panel {
                    panel.auto_save = enabled;
                }
                self.sql_auto_save_task = None;
                if matches!(self.session.identity, SessionIdentity::StartupSql) {
                    let result = self.repository.load_settings().and_then(|mut settings| {
                        settings.startup_sql_auto_save = enabled;
                        self.repository.save_settings(&settings)
                    });
                    if let Err(error) = result {
                        self.error = Some(format!("Cannot save startup SQL settings: {error}"));
                    } else {
                        self.session.dirty = was_dirty;
                        if enabled {
                            self.persist_sql_view();
                        }
                    }
                } else if matches!(self.session.identity, SessionIdentity::SavedSql(_)) {
                    self.persist_sql_view();
                } else if enabled {
                    self.open_save_dialog(cx);
                }
                cx.notify();
            }
            SqlPanelEvent::Save => {
                self.persist_sql_view();
                cx.notify();
            }
            SqlPanelEvent::Run(all) => self.run_sql(all, cx),
            SqlPanelEvent::Stop => self.stop_sql(),
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
                let result = generate_sql(
                    &request,
                    |messages| ai_client::chat(&provider, messages),
                    |sql| with_sql_service(&service, |service| service.prepare_only(sql)),
                    push,
                );
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

    pub(crate) fn stop_sql(&self) {
        if let Some(stopped) = &self.sql_stopped {
            stopped.store(true, Ordering::Release);
        }
        if let Some(interrupt) = &self.sql_interrupt {
            interrupt.interrupt();
        }
    }

    pub(crate) fn persist_sql_view(&mut self) {
        if !self.session.dirty
            || !matches!(self.session.identity, SessionIdentity::SavedSql(_) | SessionIdentity::StartupSql)
        {
            return;
        }
        let Some(mut definition) = self.session.working.clone() else {
            return;
        };
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            let ViewKind::Sql(config) = &definition.view else {
                return;
            };
            match self.repository.save_startup_sql(&config.text) {
                Ok(()) => {
                    self.session.dirty = false;
                    self.error = None;
                }
                Err(error) => self.error = Some(format!("Cannot save {}: {error}", crate::infrastructure::source_database::STARTUP_SCRIPT)),
            }
            return;
        }
        match self.repository.save_view(&mut definition) {
            Ok(_) => {
                self.session.saved(definition);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("SQL auto-save failed: {error}")),
        }
    }

    pub(crate) fn flush_sql_auto_save(&mut self) {
        self.sql_auto_save_task = None;
        if self
            .session
            .working
            .as_ref()
            .is_some_and(|d| matches!(&d.view, ViewKind::Sql(config) if config.auto_save))
        {
            self.persist_sql_view();
        }
    }

    fn schedule_sql_auto_save(&mut self, cx: &mut Cx<Self>) {
        self.sql_auto_save_task = None;
        if !matches!(self.session.identity, SessionIdentity::SavedSql(_) | SessionIdentity::StartupSql)
            || !self
                .session
                .working
                .as_ref()
                .is_some_and(|d| matches!(&d.view, ViewKind::Sql(config) if config.auto_save))
        {
            return;
        }
        self.sql_auto_save_task = Some(cx.timer(Duration::from_millis(2000), |view, cx| {
            view.persist_sql_view();
            cx.notify();
        }));
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
                    .catalog()
            },
            move |view: &mut Self, cx, result| {
                // Clear before the staleness check: a stopped run bumps the revision
                // without resetting this flag, which would block every later refresh.
                view.sql_catalog_refreshing = false;
                if revision != view.sql_revision {
                    return;
                }
                if let Ok(catalog) = result {
                    *catalog_handle.write().unwrap() = catalog;
                    view.sql_catalog_revision = catalog_revision;
                }
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn run_sql(&mut self, all: bool, cx: &mut Cx<Self>) {
        if self.sql_running || !self.session.is_sql() {
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
                        *catalog_handle.write().unwrap() = run.catalog;
                        *roots_handle.write().unwrap() = run.path_roots;
                        if run.result.is_some() {
                            view.sql_result_complete = run.complete;
                            if !run.complete {
                                run.status.push_str(
                                    " · slicers and = / - filters are off: the last statement is not a plain query",
                                );
                            }
                        }
                        if let Some(panel) = &mut view.sql_panel {
                            panel.busy = false;
                            panel.status = run.status;
                            panel.error = run.error;
                        }
                        if let Some(store) = run.result.take() {
                            let sort = if view.sql_restore_sort {
                                view.session.sort.clone()
                            } else {
                                Vec::new()
                            };
                            view.sql_restore_sort = false;
                            let source_revision = view.session.source.source_revision + 1;
                            view.session.replace_source(FrameSnapshot::new(store, source_revision));
                            view.session.sort = available_sort_keys(&sort, view.session.source.schema.as_ref());
                            if let Some(ViewKind::Sql(config)) = view.session.working.as_mut().map(|d| &mut d.view) {
                                config.result_sort = view.session.sort.clone();
                            }
                            view.query_cache = None;
                            view.table_query = None;
                            view.slicer_stage = None;
                            view.grid.reset_widths();
                            view.set_search_text(String::new(), cx);
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
}

fn with_sql_service<T>(service: &Mutex<Option<SqlExecution>>, f: impl FnOnce(&SqlExecution) -> Result<T, String>) -> Result<T, String> {
    let guard = service
        .lock()
        .map_err(|_| "SQL connection lock poisoned".to_string())?;
    f(guard.as_ref().ok_or_else(|| "SQL connection is still opening".to_string())?)
}
