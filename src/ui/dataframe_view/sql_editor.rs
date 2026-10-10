use super::*;
use crate::{
    domain::ai::{AiEvent, SqlRequest, generate_sql, mentioned_relations},
    infrastructure::{ai_client, sql_execution::SqlExecution},
    ui::sql_panel::{AskProvider, SqlAsk, SqlPanel, SqlPanelEvent},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

impl DataFrameView {
    pub(super) fn capture_sql_editor(
        &mut self,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.begin_sql_editor(window, cx);
        if target == "editor:sql" {
            return;
        }
        self.loading = true;
        let target = target.to_owned();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            loop {
                if this.update(cx, |view, _| view.sql_interrupt.is_some()).unwrap_or(true) { break; }
                if started.elapsed() > Duration::from_secs(20) { let _ = this.update(cx, |view, _| view.loading = false); return; }
                cx.background_executor().timer(Duration::from_millis(20)).await;
            }
            let _ = cx.update_window(handle, |_, window, cx| { let _ = this.update(cx, |view, cx| {
                let panel = view.sql_panel.clone().unwrap();
                let state = panel.read(cx).editor.read(cx).state().clone();
                let sql = "CREATE TEMP TABLE apples AS SELECT * FROM (VALUES ('red', 12), ('green', 8), ('gold', 15)) AS t(color, quantity);\nCREATE TEMP VIEW fruit AS SELECT * FROM apples;\nSELECT a.color, a.quantity FROM apples a ORDER BY a.quantity DESC;";
                let sql = if target == "editor:sql-error" { format!("{sql}\nSELECT a.missing FROM apples a;") } else { sql.into() };
                state.update(cx, |state, cx| state.set_value(sql.clone(), window, cx));
                view.session.set_sql_text(sql);
                view.run_sql(true, window, cx);
            }); });
            loop {
                if this.update(cx, |view, _| !view.sql_running && !view.busy).unwrap_or(true) { break; }
                cx.background_executor().timer(Duration::from_millis(20)).await;
            }
            let _ = cx.update_window(handle, |_, window, cx| { let _ = this.update(cx, |view, cx| {
                view.loading = false;
                if target == "editor:sql-ask-mention" {
                    // Captures run on a throwaway profile, so a sample provider is safe to save.
                    if let Ok(mut settings) = view.repository.load_settings() {
                        settings.ai.providers = vec![crate::domain::ai::AiProvider { name: "AI Factory".into(), model: "gpt-6-sole".into(), send_data: true, ..Default::default() }];
                        let _ = view.repository.save_settings(&settings);
                    }
                    let panel = view.sql_panel.clone().unwrap();
                    panel.update(cx, |panel, cx| {
                        panel.open_ask(window, cx);
                        panel.ask_input.update(cx, |input, cx| input.set_value("total quantity per color from @ap", window, cx));
                        panel.mentions.update(cx, |mentions, cx| mentions.refresh(cx));
                    });
                } else if target.starts_with("editor:sql-ask") {
                    use crate::domain::ai::{AiEvent, ChatMessage, SqlCheck};
                    let panel = view.sql_panel.clone().unwrap();
                    // Tall enough to show the proposal and log above the editor.
                    view.sql_split.update(cx, |split, cx| split.resize_panel(1, px(760.), window, cx));
                    panel.update(cx, |panel, cx| {
                        panel.ask_input.update(cx, |input, cx| input.set_value("total quantity per color, largest first", window, cx));
                        panel.open_ask(window, cx);
                        let sql = "SELECT color, sum(quantity) AS total_quantity\nFROM apples\nGROUP BY color\nORDER BY total_quantity DESC;";
                        panel.ask = crate::ui::sql_panel::AskState::Proposal(crate::domain::ai::SqlProposal { sql: sql.into(), check: SqlCheck::Passed });
                        if target == "editor:sql-ask-log" {
                            panel.show_log = true;
                            panel.ask_log = vec![
                                AiEvent::Sent(vec![
                                    ChatMessage::new("system", "You write SQL for DuckDB inside DataDiver…\n\nRelations (name: columns):\ntemp.apples: color VARCHAR, quantity INTEGER"),
                                    ChatMessage::new("user", "total quantity per color, largest first"),
                                ]),
                                AiEvent::Received { text: "```sql\nSELECT colour, sum(quantity) FROM apples GROUP BY colour\n```".into(), elapsed: Duration::from_millis(1840) },
                                AiEvent::Checked(SqlCheck::Failed("Binder Error: Referenced column \"colour\" not found".into())),
                                AiEvent::Sent(vec![ChatMessage::new("user", "DuckDB rejected that SQL:\nBinder Error: Referenced column \"colour\" not found\nFix it.")]),
                                AiEvent::Received { text: format!("```sql\n{sql}\n```"), elapsed: Duration::from_millis(1210) },
                                AiEvent::Checked(SqlCheck::Passed),
                            ];
                        }
                    });
                }
                if target == "editor:sql-completion" {
                    use gpui_kit::component::input::RopeExt;
                    let panel = view.sql_panel.clone().unwrap();
                    let state = panel.read(cx).editor.read(cx).state().clone();
                    state.update(cx, |state, cx| {
                        let text = "SELECT\n    a.\nFROM apples a";
                        let offset = text.find("a.").unwrap()+2;
                        state.set_value(text, window, cx);
                        state.set_selected_range(offset..offset, cx);
                        let items = crate::domain::sql_editor::completions(text, offset, &panel.read(cx).catalog.read().unwrap()).into_iter().map(|item| lsp_types::CompletionItem {
                            label: item.label, kind: Some(lsp_types::CompletionItemKind::FIELD),
                            text_edit: Some(lsp_types::CompletionTextEdit::Edit(lsp_types::TextEdit { range: lsp_types::Range { start: state.text().offset_to_position(item.range.start), end: state.text().offset_to_position(item.range.end) }, new_text: item.insert })),
                            ..Default::default()
                        }).collect();
                        state.present_completion_items(offset, "", items, cx);
                        state.focus(window, cx);
                    });
                }
                cx.notify();
            }); });
        }).detach();
    }

    pub(super) fn reset_sql_editor(&mut self) {
        self.sql_auto_save_task = None;
        self.stop_sql();
        self.sql_revision += 1;
        self.sql_panel = None;
        self.sql_panel_open = false;
        self.sql_running = false;
        self.sql_subscription = None;
        self.sql_service = Arc::new(Mutex::new(None));
        self.sql_interrupt = None;
        self.sql_stopped = None;
        self.sql_restore_sort = false;
        self.sql_catalog_refreshing = false;
        self.sql_result_complete = false;
    }

    pub(super) fn open_sql_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.is_sql() && self.session.dirty {
            let entity = cx.entity();
            window.open_dialog(cx, move |dialog, _, _| {
                let entity = entity.clone();
                dialog.title("Open SQL editor?").content(|content, _, _| content.child("The current view has unsaved changes. Opening SQL replaces the working document."))
                    .footer(DialogFooter::new().child(DialogAction::new().child(Button::new("open-sql").label("Open SQL editor"))).child(DialogClose::new().trigger(|button| button.label("Cancel"))))
                    .on_ok(move |_, window, cx| { entity.update(cx, |view, cx| view.begin_sql_editor(window, cx)); true })
            });
        } else {
            self.begin_sql_editor(window, cx);
        }
    }

    fn begin_sql_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session.is_sql() {
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
            self.slicer_sidebar
                .update(cx, |sidebar, cx| sidebar.set_cards(Vec::new(), cx));
            self.grid.update(cx, |grid, cx| {
                grid.set_frozen_count(0, cx);
                grid.set_sort(Vec::new(), cx);
                grid.set_keymap(self.keymap.clone(), KeyContext::QueryGrid);
            });
            self.commit_navigation_history();
        }
        self.ensure_sql_panel(window, cx);
        self.sql_panel_open = true;
        if let Some(panel) = &self.sql_panel {
            panel.update(cx, |panel, cx| panel.focus(window, cx));
        }
        cx.notify();
    }

    pub(super) fn ensure_sql_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sql_panel.is_some() {
            return;
        }
        let Some(ViewKind::Sql(config)) = self.session.working.as_ref().map(|d| &d.view) else {
            return;
        };
        let panel = cx.new(|cx| SqlPanel::new(config.text.clone(), window, cx));
        let startup_script = matches!(self.session.identity, SessionIdentity::StartupSql);
        let auto_run = config.auto_run && !startup_script;
        let handle = window.window_handle();
        panel.update(cx, |panel, _| {
            panel.startup_script = startup_script;
            panel.auto_save = config.auto_save;
            panel.auto_run = config.auto_run;
            panel.busy = true;
            panel.status = "Opening database…".into();
        });
        self.sql_subscription = Some(cx.subscribe_in(
            &panel,
            window,
            |view, _, event, window, cx| match event {
                SqlPanelEvent::Changed(text) => {
                    view.session.set_sql_text(text.clone());
                    view.schedule_sql_auto_save(cx);
                    cx.notify();
                }
                SqlPanelEvent::AutoRun(enabled) => {
                    if let Some(ViewKind::Sql(config)) =
                        view.session.working.as_mut().map(|d| &mut d.view)
                    {
                        config.auto_run = *enabled;
                        view.session.dirty = true;
                    }
                    if let Some(panel) = &view.sql_panel {
                        panel.update(cx, |panel, cx| {
                            panel.auto_run = *enabled;
                            cx.notify();
                        });
                    }
                    // Kept like Auto-save: a saved view stores it at once.
                    if matches!(view.session.identity, SessionIdentity::SavedSql(_)) {
                        view.persist_sql_view();
                    }
                    cx.notify();
                }
                SqlPanelEvent::AutoSave(enabled) => {
                    let was_dirty = view.session.dirty;
                    if let Some(ViewKind::Sql(config)) =
                        view.session.working.as_mut().map(|d| &mut d.view)
                    {
                        config.auto_save = *enabled;
                        view.session.dirty = true;
                    }
                    if let Some(panel) = &view.sql_panel {
                        panel.update(cx, |panel, cx| {
                            panel.auto_save = *enabled;
                            cx.notify();
                        });
                    }
                    view.sql_auto_save_task = None;
                    if matches!(view.session.identity, SessionIdentity::StartupSql) {
                        let result = view.repository.load_settings().and_then(|mut settings| {
                            settings.startup_sql_auto_save = *enabled;
                            view.repository.save_settings(&settings)
                        });
                        if let Err(error) = result {
                            view.error = Some(format!("Cannot save startup SQL settings: {error}"));
                        } else {
                            view.session.dirty = was_dirty;
                            if *enabled {
                                view.persist_sql_view();
                            }
                        }
                    } else if matches!(view.session.identity, SessionIdentity::SavedSql(_)) {
                        view.persist_sql_view();
                    } else if *enabled {
                        view.open_save_dialog(window, cx);
                    }
                    cx.notify();
                }
                SqlPanelEvent::Save => {
                    view.persist_sql_view();
                    cx.notify();
                }
                SqlPanelEvent::Run(all) => view.run_sql(*all, window, cx),
                SqlPanelEvent::Stop => view.stop_sql(),
                SqlPanelEvent::Close => {
                    view.sql_panel_open = false;
                    window.focus(&view.grid.read(cx).focus_handle(cx), cx);
                    cx.notify();
                }
                SqlPanelEvent::GridFocus => window.focus(&view.grid.read(cx).focus_handle(cx), cx),
                SqlPanelEvent::Ask(ask) => view.ask_sql(ask, cx),
                SqlPanelEvent::AskOpened => {
                    let provider = view.repository.load_settings().ok().and_then(|settings| {
                        settings.ai.active().map(|provider| AskProvider {
                            name: provider.name.clone(),
                            model: provider.model.clone(),
                            send_data: provider.send_data,
                        })
                    });
                    if let Some(panel) = &view.sql_panel {
                        panel.update(cx, |panel, cx| {
                            panel.ask_provider = provider;
                            cx.notify();
                        });
                    }
                }
            },
        ));
        self.sql_panel = Some(panel.clone());
        self.sql_panel_open = true;
        let service = self.sql_service.clone();
        let database = self.database.clone();
        let startup_database = database.clone();
        let revision = self.sql_revision;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut guard = service
                        .lock()
                        .map_err(|_| "SQL connection lock poisoned".to_string())?;
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
                })
                .await;
            let opened = result.is_ok();
            let _ = this.update(cx, |view, cx| {
                if revision != view.sql_revision {
                    return;
                }
                match result {
                    Ok((interrupt, stopped, catalog, path_roots, default_catalog)) => {
                        view.sql_interrupt = Some(interrupt);
                        view.sql_stopped = Some(stopped);
                        view.sql_catalog_revision = view.database.revision();
                        *panel.read(cx).catalog.write().unwrap() = catalog;
                        *panel.read(cx).path_roots.write().unwrap() = path_roots;
                        panel.update(cx, |panel, cx| {
                            panel.default_catalog = default_catalog;
                            panel.busy = false;
                            panel.status = "Ctrl+Enter · selection or current statement".into();
                            if startup_script {
                                panel.error = startup_database.startup_error();
                                panel.status =
                                    "Runs when the database opens · Ctrl+Enter runs now".into();
                            }
                            cx.notify();
                        });
                    }
                    Err(error) => panel.update(cx, |panel, cx| {
                        panel.busy = false;
                        panel.error = Some(error);
                        cx.notify();
                    }),
                }
                cx.notify();
            });
            if auto_run && opened {
                let _ = cx.update_window(handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        if revision == view.sql_revision {
                            view.run_sql(true, window, cx);
                        }
                    });
                });
            }
        })
        .detach();
    }

    /// Writes SQL for `ask` with the default AI provider on the background
    /// executor, checking it on this editor's connection.
    fn ask_sql(&mut self, ask: &SqlAsk, cx: &mut Context<Self>) {
        let Some(panel) = self.sql_panel.clone() else {
            return;
        };
        let revision = ask.revision;
        let provider = match self.repository.load_settings() {
            Ok(settings) => settings.ai.active().cloned().ok_or_else(|| {
                "No AI provider yet: add one with “AI providers” in the command palette (Ctrl+Shift+P)"
                    .to_owned()
            }),
            Err(error) => Err(format!("Cannot read settings: {error}")),
        };
        let provider = match provider {
            Ok(provider) => provider,
            Err(error) => {
                panel.update(cx, |panel, cx| panel.finish_ask(revision, Err(error), cx));
                return;
            }
        };
        panel.update(cx, |panel, cx| {
            let text = format!("Asking {} ({})…", provider.name, provider.model);
            let endpoint = format!("{} · model {}", provider.chat_url(), provider.model);
            panel.set_ask_working(revision, text, endpoint, cx)
        });
        let result_columns = self
            .session
            .source
            .schema
            .iter()
            .map(|column| (column.name.clone(), column.duckdb_type.clone()))
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
        let (log_copy, done_copy, log_panel) = (log.clone(), done.clone(), panel.clone());
        cx.spawn(async move |_, cx| {
            while !done_copy.load(Ordering::Acquire) {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let events = log_copy.lock().map(|log| log.clone()).unwrap_or_default();
                log_panel.update(cx, |panel, cx| panel.set_ask_log(revision, events, cx));
            }
        })
        .detach();
        cx.spawn(async move |_, cx| {
            let events = log.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let push = |event| {
                        if let Ok(mut log) = events.lock() {
                            log.push(event);
                        }
                    };
                    // Locked per call so a slow reply never holds up a SQL run.
                    // The editor's connection, since its temp tables live there.
                    let mut relations =
                        with_sql_service(&service, |service| service.ai_schema())
                            .inspect_err(|error| push(AiEvent::Failed(error.clone())))?;
                    for index in mentioned_relations(&request.prompt, &relations) {
                        let started = Instant::now();
                        let relation = &relations[index];
                        let profile = with_sql_service(&service, |service| {
                            service.profile_relation(relation, provider.send_data)
                        });
                        push(AiEvent::Note(match &profile {
                            Ok(profile) => format!(
                                "Profiled {} · {} rows in {:.1} s{}",
                                relation.name,
                                profile.rows,
                                started.elapsed().as_secs_f32(),
                                if provider.send_data {
                                    ", with data values"
                                } else {
                                    ", counts only"
                                }
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
                })
                .await;
            done.store(true, Ordering::Release);
            let events = log.lock().map(|log| log.clone()).unwrap_or_default();
            panel.update(cx, |panel, cx| {
                panel.set_ask_log(revision, events, cx);
                panel.finish_ask(revision, result, cx);
            });
        })
        .detach();
    }

    pub(super) fn stop_sql(&self) {
        if let Some(stopped) = &self.sql_stopped {
            stopped.store(true, Ordering::Release);
        }
        if let Some(interrupt) = &self.sql_interrupt {
            interrupt.interrupt();
        }
    }

    pub(super) fn persist_sql_view(&mut self) {
        if !self.session.dirty
            || !matches!(
                self.session.identity,
                SessionIdentity::SavedSql(_) | SessionIdentity::StartupSql
            )
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
                Err(error) => self.error = Some(format!("Cannot save startup.sql: {error}")),
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

    pub(super) fn flush_sql_auto_save(&mut self) {
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

    fn schedule_sql_auto_save(&mut self, cx: &mut Context<Self>) {
        self.sql_auto_save_task = None;
        if !matches!(
            self.session.identity,
            SessionIdentity::SavedSql(_) | SessionIdentity::StartupSql
        ) || !self
            .session
            .working
            .as_ref()
            .is_some_and(|d| matches!(&d.view, ViewKind::Sql(config) if config.auto_save))
        {
            return;
        }
        self.sql_auto_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2000))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.persist_sql_view();
                cx.notify();
            });
        }));
    }

    pub(super) fn refresh_sql_catalog(&mut self, cx: &mut Context<Self>) {
        if self.sql_running
            || self.sql_catalog_refreshing
            || self.sql_interrupt.is_none()
            || self.sql_catalog_revision == self.database.revision()
        {
            return;
        }
        let Some(panel) = self.sql_panel.clone() else {
            return;
        };
        self.sql_catalog_refreshing = true;
        let service = self.sql_service.clone();
        let revision = self.sql_revision;
        let catalog_revision = self.database.revision();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    service
                        .lock()
                        .map_err(|_| "SQL connection lock poisoned".to_string())?
                        .as_ref()
                        .ok_or_else(|| "SQL connection unavailable".to_string())?
                        .catalog()
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                // Clear before the staleness check: a stopped run bumps the revision
                // without resetting this flag, which would block every later refresh.
                view.sql_catalog_refreshing = false;
                if revision != view.sql_revision {
                    return;
                }
                if let Ok(catalog) = result {
                    *panel.read(cx).catalog.write().unwrap() = catalog;
                    view.sql_catalog_revision = catalog_revision;
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn run_sql(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.sql_running || !self.session.is_sql() {
            return;
        }
        let Some(panel) = self.sql_panel.clone() else {
            return;
        };
        if self.sql_interrupt.is_none() {
            panel.update(cx, |panel, cx| {
                panel.error = Some("Database connection is still opening; try Run again".into());
                cx.notify();
            });
            return;
        }
        let text = panel.read(cx).sql_to_run(all, cx);
        if text.trim().is_empty() {
            return;
        }
        self.sql_running = true;
        self.sql_stopped
            .as_ref()
            .unwrap()
            .store(false, Ordering::Release);
        panel.update(cx, |panel, cx| {
            panel.busy = true;
            panel.error = None;
            panel.status = "Running…".into();
            cx.notify();
        });
        let service = self.sql_service.clone();
        let revision = self.sql_revision;
        let limit = self.query_limit;
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move {
                    service
                        .lock()
                        .map_err(|_| "SQL connection lock poisoned".to_string())?
                        .as_ref()
                        .ok_or_else(|| "SQL connection unavailable".to_string())
                        .map(|service| service.run(&text, limit))
                })
                .await;
            let _ = cx.update_window(handle, |_, window, cx| {
                let _ = this.update(cx, |view, cx| {
                    if revision != view.sql_revision || !view.session.is_sql() {
                        return;
                    }
                    view.sql_running = false;
                    match outcome {
                        Ok(mut run) => {
                            view.sql_catalog_revision = view.database.revision();
                            *panel.read(cx).catalog.write().unwrap() = run.catalog;
                            *panel.read(cx).path_roots.write().unwrap() = run.path_roots;
                            if run.result.is_some() {
                                view.sql_result_complete = run.complete;
                                if !run.complete {
                                    run.status.push_str(
                                        " · slicers and = / - filters are off: the last statement is not a plain query",
                                    );
                                }
                            }
                            panel.update(cx, |panel, cx| {
                                panel.busy = false;
                                panel.status = run.status;
                                panel.error = run.error;
                                cx.notify();
                            });
                            if let Some(store) = run.result.take() {
                                let sort = if view.sql_restore_sort {
                                    view.session.sort.clone()
                                } else {
                                    Vec::new()
                                };
                                view.sql_restore_sort = false;
                                let source_revision = view.session.source.source_revision + 1;
                                view.session
                                    .replace_source(FrameSnapshot::new(store, source_revision));
                                view.session.sort =
                                    available_sort_keys(&sort, view.session.source.schema.as_ref());
                                if let Some(ViewKind::Sql(config)) =
                                    view.session.working.as_mut().map(|d| &mut d.view)
                                {
                                    config.result_sort = view.session.sort.clone();
                                }
                                view.query_cache = None;
                                view.table_query = None;
                                view.slicer_stage = None;
                                view.grid.update(cx, |grid, _| grid.reset_widths());
                                view.set_search_text(String::new(), window, cx);
                            }
                        }
                        Err(error) => panel.update(cx, |panel, cx| {
                            panel.busy = false;
                            panel.error = Some(error);
                            cx.notify();
                        }),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }
}

fn with_sql_service<T>(
    service: &Mutex<Option<SqlExecution>>,
    f: impl FnOnce(&SqlExecution) -> Result<T, String>,
) -> Result<T, String> {
    let guard = service
        .lock()
        .map_err(|_| "SQL connection lock poisoned".to_string())?;
    f(guard
        .as_ref()
        .ok_or_else(|| "SQL connection is still opening".to_string())?)
}
