//! Opening files and views in a window, history, undo, and drill-down.

use super::*;
use crate::application::state::ApplicationState;
use crate::infrastructure::json_store::FavoriteTarget;
use crate::infrastructure::readers::{read_options, read_source};
use crate::infrastructure::source_database::LoadMode;
use crate::infrastructure::sources::SourceLoad;
use crate::ui::platform::system;

impl DataFrameView {
    pub(crate) fn navigate_to_file(&mut self, entry: FileEntry, carry_search: bool, pivot_draft: bool, raw_source: bool, cx: &mut Cx<Self>) {
        self.navigate_to_location(entry, carry_search, pivot_draft, raw_source, None, None, cx);
    }

    pub(crate) fn remember_navigation_current(&mut self) {
        if let Some(entry) = self.session.navigation_entry() {
            self.navigation_history.remember_current(entry);
        }
    }

    /// Starts undo over from the current state, as at a new location.
    pub(crate) fn reset_undo_history(&mut self) {
        self.undo_history.reset(Some(self.session.view_state()));
    }

    /// Steps the view configuration back, or forward again with `redo`.
    pub(crate) fn undo_view_change(&mut self, redo: bool, cx: &mut Cx<Self>) {
        if self.loading {
            return;
        }
        // An edit still computing has not been recorded by a publication yet.
        self.undo_history.record(self.session.view_state());
        let state = if redo {
            self.undo_history.redo()
        } else {
            self.undo_history.undo()
        };
        let Some(state) = state else {
            self.copy_status = Some(if redo { "Nothing to redo" } else { "Nothing to undo" }.into());
            cx.notify();
            return;
        };
        self.session.restore_view_state(&state);
        self.query_cache = None;
        self.table_query = None;
        self.grid.set_sort(self.session.sort.clone());
        self.sync_frozen_count();
        self.autosave_table_preferences(cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    pub(crate) fn commit_navigation_history(&mut self) {
        self.reset_undo_history();
        if let Some(entry) = self.session.navigation_entry() {
            if let Some((index, _)) = self.navigation_target.take() {
                self.navigation_history.commit(index, entry);
            } else {
                self.navigation_history.push(entry);
            }
        }
    }

    pub(crate) fn navigate_history(&mut self, forward: bool, cx: &mut Cx<Self>) {
        let Some((index, target)) = self.navigation_history.target(forward) else {
            return;
        };
        self.rollback_navigation(cx);
        let entry = if matches!(target.identity, SessionIdentity::RawSource | SessionIdentity::DraftTable) {
            FileEntry::Source(PathBuf::from(target.source_path.as_ref().expect("source location")))
        } else if let Some(definition) = &target.definition {
            FileEntry::View(definition.clone())
        } else {
            FileEntry::Source(PathBuf::from(target.source_path.as_ref().expect("source location")))
        };
        self.navigate_to_location(entry, false, false, true, Some((index, target)), None, cx);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn navigate_to_location(
        &mut self,
        entry: FileEntry,
        carry_search: bool,
        pivot_draft: bool,
        raw_source: bool,
        target: Option<(usize, NavigationEntry)>,
        drill: Option<DrillDown>,
        cx: &mut Cx<Self>,
    ) {
        self.rollback_navigation(cx);
        self.flush_sql_auto_save();
        self.autosave_table_preferences(cx);
        self.remember_navigation_current();
        self.navigation_target = target;
        if self.sql_running {
            self.stop_sql();
            self.sql_revision += 1;
            self.sql_running = false;
            if let Some(panel) = &mut self.sql_panel {
                panel.busy = false;
                panel.status = "Stopped".into();
            }
        }
        let startup_script = matches!(entry, FileEntry::StartupSql);
        let sql_definition = match &entry {
            FileEntry::View(definition) if matches!(definition.view, ViewKind::Sql(_)) => Some(definition.clone()),
            _ => None,
        };
        if startup_script || sql_definition.is_some() {
            let repository = self.repository.clone();
            let database = self.database.clone();
            let token = self.session.next_request();
            self.computation.invalidate(token);
            self.loading = true;
            self.debouncing = false;
            self.error = None;
            cx.notify();
            cx.spawn(
                move || {
                    database.initialize()?;
                    let definition = if startup_script {
                        repository
                            .load_startup_sql()
                            .map_err(|error| format!("Cannot open {}: {error}", crate::infrastructure::source_database::STARTUP_SCRIPT))?
                    } else {
                        sql_definition.expect("SQL view destination")
                    };
                    crate::infrastructure::duckdb_store::DuckDbStore::empty().map(|s| (FrameSnapshot::new(s, 1), definition))
                },
                move |view: &mut Self, cx, source: Result<(FrameSnapshot, DocumentDefinition), String>| {
                    if !view.session.accepts(token) {
                        return;
                    }
                    view.loading = false;
                    match source.and_then(|(source, definition)| view.session.navigate_to_view(Ok(source), definition)) {
                        Ok(()) => {
                            view.reset_sql_editor();
                            if startup_script {
                                view.session.identity = SessionIdentity::StartupSql;
                            }
                            if let Some((_, target)) = &view.navigation_target {
                                view.session.restore_configuration(target);
                            }
                            view.computation.invalidate(view.session.token());
                            view.query_cache = None;
                            view.table_query = None;
                            view.slicer_stage = None;
                            view.source_modified = None;
                            view.sql_restore_sort = true;
                            view.session.result = ResultSnapshot::pending(&view.session.source);
                            view.grid.replace_result(view.session.result.clone());
                            view.grid.set_sort(view.session.sort.clone());
                            view.grid.set_keymap(view.keymap.clone(), KeyContext::QueryGrid);
                            view.grid.set_frozen_count(0);
                            view.slicer_sidebar.set_cards(Vec::new());
                            view.search_input.set_text(view.session.search_text.clone());
                            view.ensure_sql_panel(cx);
                            if startup_script {
                                if let Some(panel) = &view.sql_panel {
                                    let id = panel.editor.id;
                                    view.set_focus(Some(id), cx);
                                }
                            } else {
                                view.focus_grid(cx);
                            }
                            view.commit_navigation_history();
                            let _ = view.record_opened();
                        }
                        Err(error) => {
                            view.navigation_target = None;
                            view.error = Some(error);
                        }
                    }
                    view.drain_grid_events(cx);
                    cx.notify();
                },
            )
            .detach();
            return;
        }
        let path = match &entry {
            FileEntry::View(view) => PathBuf::from(&view.source_path),
            FileEntry::Source(path) => path.clone(),
            FileEntry::StartupSql => unreachable!("startup SQL uses SQL navigation"),
        };
        let database_view = match &entry {
            FileEntry::View(definition) => definition.database_view.clone(),
            FileEntry::Source(_) | FileEntry::StartupSql => None,
        };
        let search = if let Some((_, target)) = &self.navigation_target {
            target.search_text.clone()
        } else if carry_search {
            self.session.search_text.clone()
        } else {
            String::new()
        };
        let token = self.session.next_request();
        self.computation.invalidate(token);
        self.debouncing = false;
        self.busy = self.computation.is_running();
        self.loading = true;
        self.error = None;
        cx.notify();
        let database = self.database.clone();
        let sources = self.sources.clone();
        let repository = self.repository.clone();
        let existing = self.session.source.clone();
        let table_writer = self.table_writer.clone();
        let direct_destination = matches!(entry, FileEntry::Source(_));
        let read_path = path.clone();
        cx.spawn(
            move || {
                let _span = crate::infrastructure::performance::Span::new("source_check");
                let source = if let Some(view) = &database_view {
                    FrameSnapshot::new(database.load_view(view)?, existing.source_revision + 1)
                } else {
                    let options = read_options(&repository, &table_writer, &read_path);
                    match sources.load_reusing(&read_path, options, LoadMode::Check, existing.source_identity.as_ref())? {
                        SourceLoad::Reused => existing,
                        SourceLoad::Loaded(store) => FrameSnapshot::new(*store, existing.source_revision + 1),
                    }
                };
                let preferences = direct_destination.then(|| table_writer.load(&read_path));
                Ok::<_, String>((source, source_modified(Some(&read_path.to_string_lossy())), preferences))
            },
            move |view: &mut Self, cx, read| {
                if !view.session.accepts(token) {
                    return;
                }
                view.loading = false;
                let _transition = crate::infrastructure::performance::Span::new("session_transition");
                match read {
                    Ok((source, modified, preferences)) => {
                        let previous_preferences_error = view.table_preferences_error.clone();
                        view.table_preferences_error = None;
                        let previous = view.session.clone();
                        view.navigation_backup = Some(NavigationBackup {
                            session: previous,
                            source_modified: view.source_modified.clone(),
                            table_preferences_error: previous_preferences_error,
                            slicer_stage: view.slicer_stage.clone(),
                            query_cache: view.query_cache.clone(),
                            table_query: view.table_query.clone(),
                        });
                        let destination = match entry {
                            FileEntry::View(definition) => view.session.navigate_to_view(Ok(source), definition),
                            FileEntry::StartupSql => unreachable!("startup SQL uses SQL navigation"),
                            FileEntry::Source(_) => view
                                .session
                                .navigate_to_raw(Ok(source), Some(path.to_string_lossy().into_owned()))
                                .and_then(|()| {
                                    if let Some(preferences) = preferences {
                                        match preferences {
                                            Ok(preferences) => view.session.apply_direct_preferences(preferences),
                                            Err(error) => {
                                                view.table_preferences_error =
                                                    Some(format!("Cannot read table preferences: {error}"))
                                            }
                                        }
                                    }
                                    if raw_source {
                                        return Ok(());
                                    }
                                    let name = view.suggested_name();
                                    if pivot_draft {
                                        view.session.start_query_draft(name)
                                    } else {
                                        view.session.start_table_draft(name)
                                    }
                                }),
                        };
                        if let Err(error) = destination {
                            view.rollback_navigation(cx);
                            view.error = Some(error);
                            cx.notify();
                            return;
                        }
                        if let Some((_, target)) = &view.navigation_target {
                            view.session.restore_configuration(target);
                            view.query_cache = None;
                            view.table_query = None;
                        }
                        if let Some(drill) = drill {
                            view.apply_drill_down(drill);
                        }
                        view.source_modified = modified;
                        view.computation.invalidate(view.session.token());
                        view.session.search_text = search.clone();
                        view.search_input.set_text(search.clone());
                        view.grid.set_sort(view.session.sort.clone());
                        view.schedule_search(search, cx);
                        // The picker may have restored focus to the SQL editor we just removed.
                        view.focus_grid(cx);
                    }
                    Err(error) => {
                        view.rollback_navigation(cx);
                        view.error = Some(error);
                    }
                }
                view.drain_grid_events(cx);
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn apply_drill_down(&mut self, drill: DrillDown) {
        if self.session.working.is_none() {
            let _ = self.session.start_table_draft(self.suggested_name());
        }
        let result = self.session.edit_definition(|definition| {
            for slicer in drill.slicers {
                match definition.slicers.iter_mut().find(|card| card.column == slicer.column) {
                    Some(card) => card.selected = slicer.selected,
                    None => definition.slicers.push(slicer),
                }
            }
            definition.filters = drill.filters;
        });
        if let Err(error) = result {
            self.error = Some(error);
        }
    }

    pub(crate) fn reload(&mut self, reset: bool, cx: &mut Cx<Self>) {
        if self.session.is_sql() {
            return;
        }
        if let Some(view) = self.session.working.as_ref().and_then(|d| d.database_view.clone()) {
            self.load_database_view(view, reset, cx);
            return;
        }
        let Some(path) = self.session.source_path.as_ref().map(PathBuf::from) else {
            self.error = Some("Reload requires a source file".into());
            cx.notify();
            return;
        };
        self.load_source(path, reset, LoadMode::Force, cx);
    }

    pub(crate) fn rollback_navigation(&mut self, cx: &mut Cx<Self>) {
        self.navigation_target = None;
        if let Some(previous) = self.navigation_backup.take() {
            self.session.restore_navigation(previous.session);
            let token = self.session.token();
            // Restore widgets once the enclosing update has finished.
            cx.defer(move |view, cx| {
                if view.session.accepts(token) {
                    view.search_input.set_text(view.session.search_text.clone());
                    if view.session.is_sql() {
                        view.ensure_sql_panel(cx);
                    }
                    cx.notify();
                }
            });
            self.source_modified = previous.source_modified;
            self.table_preferences_error = previous.table_preferences_error;
            self.slicer_stage = previous.slicer_stage;
            self.query_cache = previous.query_cache;
            self.table_query = previous.table_query;
            self.computation.invalidate(self.session.token());
            self.debouncing = false;
            let cards = self
                .slicer_stage
                .as_ref()
                .map(|stage| stage.cards.clone())
                .unwrap_or_default();
            self.slicer_sidebar.set_cards(cards);
            self.update_slicer_picker(None);
            self.grid.set_sort(self.session.sort.clone());
            let context = if self.is_table() {
                KeyContext::TableGrid
            } else {
                KeyContext::QueryGrid
            };
            self.grid.set_keymap(self.keymap.clone(), context);
            self.sync_frozen_count();
            self.sync_grid_widths(cx);
        }
    }

    /// The source path of a file open in query mode.
    pub(crate) fn query_file(&self) -> Option<String> {
        self.session
            .working
            .as_ref()?
            .database_view
            .as_ref()?
            .file_path()
            .map(str::to_owned)
    }

    pub(crate) fn record_opened(&self) -> Result<(), String> {
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            return Ok(());
        }
        self.history_writer.opened(
            self.session.working.clone(),
            self.session.source_path.clone().or_else(|| self.query_file()),
        )
    }

    pub(crate) fn load_source(&mut self, path: PathBuf, reset: bool, mode: LoadMode, cx: &mut Cx<Self>) {
        self.load_data_source(SourceTarget::File(path), reset, mode, cx);
    }

    pub(crate) fn load_database_view(&mut self, view: crate::domain::documents::DatabaseView, reset: bool, cx: &mut Cx<Self>) {
        self.load_data_source(SourceTarget::DatabaseView(view), reset, LoadMode::Check, cx);
    }

    fn load_data_source(&mut self, target: SourceTarget, reset: bool, mode: LoadMode, cx: &mut Cx<Self>) {
        let token = self.session.next_request();
        self.computation.invalidate(token);
        self.debouncing = false;
        self.loading = true;
        self.error = None;
        cx.notify();
        let database = self.database.clone();
        let sources = self.sources.clone();
        let repository = self.repository.clone();
        let table_writer = self.table_writer.clone();
        cx.spawn(
            move || match target {
                SourceTarget::File(path) => {
                    let options = read_options(&repository, &table_writer, &path);
                    read_source(&path, &sources, options, mode)
                }
                SourceTarget::DatabaseView(view) => database.load_view(&view).map(Into::into),
            },
            move |view: &mut Self, cx, result| {
                if !view.session.accepts(token) {
                    return;
                }
                view.loading = false;
                match result {
                    Ok(frame) => {
                        let source = FrameSnapshot::new(frame, view.session.source.source_revision + 1);
                        view.replace_dataframe_with_reset(source, reset, cx);
                        if let Err(error) = view.record_opened() {
                            eprintln!("history update failed: {error}");
                        }
                        if reset {
                            view.search_input.set_text("");
                        }
                        view.schedule_search(view.session.search_text.clone(), cx);
                    }
                    Err(error) => view.error = Some(error),
                }
                cx.notify();
            },
        )
        .detach();
    }

    /// Opens a new window, empty or showing the source at `path`.
    pub(crate) fn open_blank_window(&mut self, path: Option<PathBuf>, cx: &mut Cx<Self>) {
        self.open_window_with(
            ViewerOptions {
                load_path: path,
                ..Default::default()
            },
            cx,
        );
    }

    /// Opens a new window on this window's profile with `options`.
    pub(crate) fn open_window_with(&mut self, options: ViewerOptions, cx: &mut Cx<Self>) {
        let repository = self.repository.clone();
        cx.spawn(
            crate::infrastructure::duckdb_store::DuckDbStore::empty,
            move |view: &mut Self, cx, result| match result {
                Ok(store) => {
                    let source_path = options
                        .load_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned());
                    Self::open_in(
                        cx,
                        FrameSnapshot::new(store, 1),
                        source_path,
                        ViewerOptions {
                            repository,
                            restore_window: false,
                            ..options
                        },
                    );
                }
                Err(error) => {
                    view.error = Some(format!("Cannot open new window: {error}"));
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// Opens dropped or pasted files: the first in this window, and each
    /// other one in a window of its own.
    pub(crate) fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Cx<Self>) {
        let (databases, paths): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .partition(|path| crate::infrastructure::source_database::database_file_kind(path).is_some());
        if !databases.is_empty() {
            self.attach_databases(databases, cx);
        }
        let mut paths = paths.into_iter();
        let Some(first) = paths.next() else {
            return;
        };
        if first.is_dir() {
            self.error = Some(format!("{} is a folder; use Add folder to list its files", first.display()));
            cx.notify();
            return;
        }
        let entry = FileEntry::Source(first);
        self.navigate_to_file(entry, false, false, true, cx);
        for path in paths.filter(|path| !path.is_dir()) {
            self.open_blank_window(Some(path), cx);
        }
    }

    pub(crate) fn open_native_file(&mut self, cx: &mut Cx<Self>) {
        let chosen = system::prompt_for_paths(
            cx.hwnd(),
            system::PathPrompt {
                directories: false,
                multiple: false,
                title: "Open source",
            },
        );
        match chosen {
            Ok(Some(paths)) => {
                if let Some(path) = paths.into_iter().next() {
                    let entry = FileEntry::Source(path);
                    self.navigate_to_file(entry, false, false, true, cx);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.error = Some(format!("Open file failed: {error}"));
                cx.notify();
            }
        }
    }

    /// Opens what the clipboard holds: copied files open as sources, and text
    /// (such as cells copied from a spreadsheet) is saved as a CSV or TSV file
    /// under the data folder and opened as a table.
    pub(crate) fn paste_table(&mut self, cx: &mut Cx<Self>) {
        let Some(content) = system::read_clipboard() else {
            self.error = Some("The clipboard is empty".into());
            cx.notify();
            return;
        };
        let text = match content {
            system::ClipboardContent::Paths(paths) => {
                self.open_paths(paths, cx);
                return;
            }
            system::ClipboardContent::Text(text) => text,
        };
        let directory = self.repository.paths.data.join("pasted");
        let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string();
        cx.spawn(
            move || crate::infrastructure::pasted_table::save(&directory, &text, &stamp),
            |view: &mut Self, cx, saved| match saved {
                Ok(path) => {
                    view.copy_status = Some(format!("Pasted table saved to {}", path.display()));
                    let entry = FileEntry::Source(path);
                    view.navigate_to_file(entry, false, false, true, cx);
                }
                Err(error) => {
                    view.error = Some(error);
                    cx.notify();
                }
            },
        )
        .detach();
    }

    pub(crate) fn favorite(&mut self, slot: usize, assign: bool, cx: &mut Cx<Self>) {
        let result = ApplicationState::load(self.repository.clone());
        match result {
            Ok(mut state) if assign => {
                let target = match &self.session.identity {
                    SessionIdentity::SavedTable(id) | SessionIdentity::SavedQuery(id) | SessionIdentity::SavedSql(id) => {
                        Some(FavoriteTarget::View { view_id: id.clone() })
                    }
                    SessionIdentity::DraftQuery if self.query_file().is_some() => self.query_file().map(FavoriteTarget::Source),
                    SessionIdentity::DraftQuery | SessionIdentity::DraftSql => {
                        self.error = Some("Save this view before assigning it as a favorite".into());
                        cx.notify();
                        return;
                    }
                    _ => self.session.source_path.clone().map(FavoriteTarget::Source),
                };
                let Some(target) = target else {
                    self.error = Some("A source file or saved view is required to assign a favorite".into());
                    cx.notify();
                    return;
                };
                match state.set_favorite(slot, Some(target)) {
                    Ok(()) => {
                        self.error = None;
                        self.copy_status = Some(format!("Favorite {} assigned", slot + 1));
                    }
                    Err(error) => self.error = Some(format!("Favorite failed: {error}")),
                }
            }
            Ok(state) => match state.settings.favorites[slot].as_ref() {
                Some(target) => {
                    let entry = match target {
                        FavoriteTarget::Source(path) => Some(FileEntry::Source(PathBuf::from(path))),
                        FavoriteTarget::View { view_id } => state
                            .views
                            .iter()
                            .find(|view| &view.id == view_id)
                            .cloned()
                            .map(FileEntry::View),
                    };
                    if let Some(entry) = entry {
                        self.navigate_to_file(entry, false, false, true, cx);
                    } else {
                        self.error = Some(format!("Favorite {} refers to a view that is no longer saved", slot + 1));
                    }
                }
                None => self.error = Some(format!("Favorite {} is empty", slot + 1)),
            },
            Err(error) => self.error = Some(format!("Favorite failed: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn archive_view(&mut self, definition: &DocumentDefinition, cx: &mut Cx<Self>) {
        match self.repository.archive(&definition.id) {
            Ok(_) => {
                if self
                    .session
                    .working
                    .as_ref()
                    .is_some_and(|working| working.id == definition.id)
                {
                    let source = self.session.source.clone();
                    let path = self.session.source_path.clone();
                    let destination = if let Some(database_view) = &definition.database_view {
                        self.session
                            .navigate_to_view(Ok(source), DocumentDefinition::draft_database_view(database_view.clone()))
                    } else {
                        self.session.navigate_to_raw(Ok(source), path)
                    };
                    if let Err(error) = destination {
                        self.error = Some(error);
                    } else {
                        if let Some(path) = &self.session.source_path {
                            match self.table_writer.load(std::path::Path::new(path)) {
                                Ok(preferences) => self.session.apply_direct_preferences(preferences),
                                Err(error) => {
                                    self.table_preferences_error = Some(format!("Cannot read table preferences: {error}"))
                                }
                            }
                        }
                        self.computation.invalidate(self.session.token());
                        self.search_input.set_text("");
                        self.grid.set_sort(Vec::new());
                        self.grid.replace_result(self.session.result.clone());
                        self.grid.set_frozen_count(0);
                        if let Err(error) = self.record_opened() {
                            eprintln!("history update failed: {error}");
                        }
                        self.schedule_search(String::new(), cx);
                    }
                }
                self.file_groups[0].retain(|entry| !matches!(entry, FileEntry::View(view) if view.id == definition.id));
                self.file_groups[1].retain(|entry| !matches!(entry, FileEntry::View(view) if view.id == definition.id));
            }
            Err(error) => self.error = Some(format!("Archive failed: {error}")),
        }
        self.drain_grid_events(cx);
        cx.notify();
    }

    /// Opens the source rows behind the active query cell, here or in a new window.
    pub(crate) fn drill_down(&mut self, new_window: bool, cx: &mut Cx<Self>) {
        match self.drill_down_target() {
            Ok((entry, drill)) if new_window => self.open_window_with(
                ViewerOptions {
                    open_entry: Some((entry, drill)),
                    ..Default::default()
                },
                cx,
            ),
            Ok((entry, drill)) => self.navigate_to_location(entry, false, false, true, None, drill, cx),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn drill_down_target(&self) -> Result<(FileEntry, Option<DrillDown>), String> {
        let definition = self
            .session
            .working
            .as_ref()
            .filter(|definition| matches!(definition.view, ViewKind::Query(_)))
            .ok_or("Drill down works on query results; press = to filter a table instead")?;
        let ViewKind::Query(config) = &definition.view else {
            unreachable!("checked above");
        };
        let (row, column, _) = self.active_cell_value()?;
        let result = &self.session.result;
        let group_count = config.group_by.len();
        let cached = match result.cached_rows(row, 1) {
            Some(rows) => rows,
            None => result.fetch_rows(row, 1)?,
        };
        let values = &cached
            .first()
            .ok_or("The active row is no longer available")?
            .values;
        let group_values = (0..group_count.min(result.column_count()))
            .map(|index| values.get(result.column_indices[index]).cloned().flatten())
            .collect::<Vec<_>>();
        let on_group_column = column < group_count;
        let pivot_keys = if on_group_column || config.pivot_by.is_empty() {
            Vec::new()
        } else {
            self.session
                .source
                .store
                .lock()
                .map_err(|_| "DuckDB store lock poisoned".to_owned())?
                .distinct_rendered(
                    &config.pivot_by,
                    &crate::application::slicers::filters(&definition.slicers),
                    &definition.filters,
                )?
        };
        let mut filters = definition.filters.clone();
        for filter in crate::domain::drill::cell_filters(
            config,
            &group_values,
            &result.columns[column].0,
            on_group_column,
            &pivot_keys,
        )? {
            if !filters.contains(&filter) {
                filters.push(filter);
            }
        }
        let slicers = definition
            .slicers
            .iter()
            .filter(|slicer| slicer.is_active())
            .cloned()
            .collect::<Vec<_>>();
        if let Some(database_view) = &definition.database_view {
            let mut target = DocumentDefinition::draft_database_view(database_view.clone());
            target.slicers = slicers;
            target.filters = filters;
            return Ok((FileEntry::View(target), None));
        }
        let path = self
            .session
            .source_path
            .clone()
            .filter(|path| !path.is_empty())
            .ok_or("Drill down needs a file or database view source")?;
        Ok((FileEntry::Source(PathBuf::from(path)), Some(DrillDown { slicers, filters })))
    }
}
