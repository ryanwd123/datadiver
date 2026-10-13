use crate::application::navigation_history::NavigationEntry;
use crate::domain::documents::{
    DocumentDefinition, QueryConfig, RelationKind, SortKey, SqlConfig, TableConfig, ValueFilter,
    ViewKind,
};
use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
use crate::domain::sql::{SqlDiagnostics, SqlTrace};
use crate::domain::table_preferences::{TablePreferences, TablePreferencesPatch, valid_width};
use crate::infrastructure::json_store::ViewRepository;
use std::io;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionIdentity {
    RawSource,
    DraftTable,
    SavedTable(String),
    DraftQuery,
    SavedQuery(String),
    DraftSql,
    SavedSql(String),
    StartupSql,
}

#[derive(Clone)]
pub struct WindowSession {
    pub identity: SessionIdentity,
    pub source: FrameSnapshot,
    pub result: ResultSnapshot,
    pub source_path: Option<String>,
    pub working: Option<DocumentDefinition>,
    pub dirty: bool,
    pub search_text: String,
    pub sort: Vec<SortKey>,
    pub sql: SqlDiagnostics,
    pub direct_table: TablePreferences,
    queued_table: TablePreferences,
    table_reset_pending: bool,
    session_generation: u64,
    request_revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobToken {
    pub session_generation: u64,
    pub source_revision: u64,
    pub request_revision: u64,
}

impl WindowSession {
    pub(crate) fn navigation_entry(&self) -> Option<NavigationEntry> {
        if !self.is_sql()
            && self
                .working
                .as_ref()
                .is_none_or(|d| d.database_view.is_none())
            && self.source_path.as_deref().is_none_or(str::is_empty)
        {
            return None;
        }
        Some(self.view_state())
    }

    /// The current view configuration, as navigation history and undo keep it.
    pub(crate) fn view_state(&self) -> NavigationEntry {
        let mut definition = self.working.clone();
        if self.is_direct_table()
            && let Some(definition) = &mut definition
        {
            definition.view = ViewKind::Table(self.effective_table_config());
        }
        NavigationEntry {
            identity: self.identity.clone(),
            source_path: self.source_path.clone(),
            definition,
            dirty: self.dirty,
            search_text: self.search_text.clone(),
            sort: self.sort.clone(),
            direct_table: self.direct_table.clone(),
        }
    }

    pub(crate) fn restore_configuration(&mut self, entry: &NavigationEntry) {
        self.identity = entry.identity.clone();
        self.source_path = entry.source_path.clone();
        self.working = entry.definition.clone();
        self.dirty = entry.dirty;
        self.search_text = entry.search_text.clone();
        self.sort = entry.sort.clone();
        self.direct_table = entry.direct_table.clone();
        self.session_generation += 1;
        self.request_revision += 1;
    }

    /// Restores an undo state's configuration, keeping the current search,
    /// column widths and formats, view name, and actions.
    pub(crate) fn restore_view_state(&mut self, state: &NavigationEntry) {
        let search_text = self.search_text.clone();
        let widths = self.direct_table.widths.clone();
        let current = self.working.clone();
        self.restore_configuration(state);
        self.search_text = search_text;
        self.direct_table.widths = widths;
        if let (Some(current), Some(definition)) = (current, &mut self.working) {
            definition.name = current.name;
            definition.actions = current.actions;
            match (current.view, &mut definition.view) {
                (ViewKind::Table(current), ViewKind::Table(config)) => {
                    config.presentation = current.presentation;
                }
                (ViewKind::Sql(current), ViewKind::Sql(config)) => {
                    config.text = current.text;
                    config.auto_save = current.auto_save;
                    config.auto_run = current.auto_run;
                }
                _ => {}
            }
        }
    }

    pub fn raw(source: FrameSnapshot, source_path: Option<String>) -> Self {
        let result = ResultSnapshot::pending(&source);
        let preferences = TablePreferences::new(source_path.clone().unwrap_or_default());
        Self {
            identity: SessionIdentity::RawSource,
            source,
            result,
            source_path,
            working: None,
            dirty: false,
            search_text: String::new(),
            sort: Vec::new(),
            sql: SqlDiagnostics::default(),
            direct_table: preferences.clone(),
            queued_table: preferences,
            table_reset_pending: false,
            session_generation: 0,
            request_revision: 0,
        }
    }

    pub fn start_table_draft(&mut self, name: String) -> Result<(), String> {
        let path = self.source_path.clone().unwrap_or_default();
        let mut definition = self
            .working
            .clone()
            .unwrap_or_else(|| DocumentDefinition::draft_table(name.clone(), path));
        definition.name = name;
        let mut config = self.direct_table.config(&self.available_columns());
        config.sort = self.sort.clone();
        definition.view = ViewKind::Table(config);
        if self.working.is_none() {
            definition.slicers = self.direct_table.slicers();
        }
        self.working = Some(definition);
        self.identity = SessionIdentity::DraftTable;
        self.dirty = false;
        self.session_generation += 1;
        self.sql = SqlDiagnostics::default();
        Ok(())
    }

    pub fn is_sql(&self) -> bool {
        self.working
            .as_ref()
            .is_some_and(|d| matches!(d.view, ViewKind::Sql(_)))
    }

    pub fn start_sql_draft(&mut self) {
        let mut draft = DocumentDefinition::draft_table("SQL query".into(), String::new());
        draft.view = ViewKind::Sql(SqlConfig::default());
        self.working = Some(draft);
        self.identity = SessionIdentity::DraftSql;
        self.source_path = None;
        self.sort.clear();
        self.dirty = true;
        self.session_generation += 1;
        self.request_revision += 1;
        self.sql = SqlDiagnostics::default();
    }

    pub fn set_sql_text(&mut self, text: String) {
        if let Some(ViewKind::Sql(sql)) = self.working.as_mut().map(|d| &mut d.view)
            && sql.text != text
        {
            sql.text = text;
            self.dirty = true;
        }
    }

    pub fn start_query_draft(&mut self, name: String) -> Result<(), String> {
        let path = self.source_path.clone().unwrap_or_default();
        let mut draft = DocumentDefinition::draft_table(name, path);
        draft.database_view = self.working.as_ref().and_then(|d| d.database_view.clone());
        draft.view = ViewKind::Query(Default::default());
        if let Some(definition) = &self.working {
            draft.slicers = definition.slicers.clone();
            draft.filters = definition.filters.clone();
        }
        self.working = Some(draft);
        self.identity = SessionIdentity::DraftQuery;
        self.dirty = true;
        self.session_generation += 1;
        self.sql = SqlDiagnostics::default();
        Ok(())
    }

    pub fn edit_definition(
        &mut self,
        edit: impl FnOnce(&mut DocumentDefinition),
    ) -> Result<(), String> {
        let direct = self.is_direct_table();
        let available = self.available_columns();
        let definition = self.working.as_mut().ok_or("no working view")?;
        let previous = definition.view.clone();
        let previous_name = definition.name.clone();
        let previous_actions = definition.actions.clone();
        edit(definition);
        if direct && let ViewKind::Table(config) = &definition.view {
            if previous != definition.view {
                self.direct_table.update_layout(config, &available);
            }
            // A plain file remembers column cards only; an SQL card lives in a saved view.
            self.direct_table.slicer_columns = definition
                .slicers
                .iter()
                .filter(|card| !card.sql)
                .map(|card| card.column.clone())
                .collect();
            self.direct_table.condition_slicers = definition
                .slicers
                .iter()
                .filter(|card| card.condition.is_some() && !card.sql)
                .map(|card| card.column.clone())
                .collect();
            self.dirty |=
                definition.name != previous_name || definition.actions != previous_actions;
        } else {
            self.dirty = true;
        }
        self.request_revision += 1;
        Ok(())
    }

    pub fn is_direct_table(&self) -> bool {
        matches!(
            self.identity,
            SessionIdentity::RawSource | SessionIdentity::DraftTable
        ) && !matches!(
            self.working.as_ref().map(|definition| &definition.view),
            Some(ViewKind::Query(_))
        )
    }

    fn available_columns(&self) -> Vec<String> {
        self.source
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect()
    }

    pub fn apply_direct_preferences(&mut self, preferences: TablePreferences) {
        self.direct_table = preferences.clone();
        self.queued_table = preferences;
        self.table_reset_pending = false;
        if self.is_direct_table() {
            let mut definition = self.working.take().unwrap_or_else(|| {
                DocumentDefinition::draft_table(
                    self.source_path
                        .as_deref()
                        .and_then(|path| std::path::Path::new(path).file_stem())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Table".into()),
                    self.source_path.clone().unwrap_or_default(),
                )
            });
            let mut config = self.direct_table.config(&self.available_columns());
            config.sort = self.sort.clone();
            definition.view = ViewKind::Table(config);
            definition.slicers = self.direct_table.slicers();
            self.working = Some(definition);
            self.dirty = false;
        }
    }

    pub fn effective_table_config(&self) -> TableConfig {
        if self.is_direct_table() {
            let mut config = self.direct_table.config(&self.available_columns());
            config.sort = self.sort.clone();
            config
        } else if let Some(ViewKind::Table(config)) =
            self.working.as_ref().map(|definition| &definition.view)
        {
            let mut resolved = config.clone();
            let available = self.available_columns();
            resolved.column_order = crate::domain::table::resolved_order(&available, config);
            resolved.frozen_count = if config.column_order.is_empty() {
                config.frozen_count.min(available.len())
            } else {
                config
                    .column_order
                    .iter()
                    .take(config.frozen_count)
                    .filter(|name| available.contains(name))
                    .count()
            };
            resolved
        } else {
            TableConfig::default()
        }
    }

    pub fn set_table_layout(&mut self, mut config: TableConfig) -> Result<(), String> {
        if !self.is_direct_table()
            && let Some(ViewKind::Table(previous)) =
                self.working.as_ref().map(|definition| &definition.view)
        {
            let available = self.available_columns();
            let missing_frozen = previous
                .column_order
                .iter()
                .take(previous.frozen_count)
                .filter(|name| !available.contains(name))
                .cloned()
                .collect::<Vec<_>>();
            let missing_other = previous
                .column_order
                .iter()
                .skip(previous.frozen_count)
                .filter(|name| !available.contains(name))
                .cloned()
                .collect::<Vec<_>>();
            config.column_order = config
                .column_order
                .iter()
                .take(config.frozen_count)
                .cloned()
                .chain(missing_frozen.iter().cloned())
                .chain(
                    config
                        .column_order
                        .iter()
                        .skip(config.frozen_count)
                        .cloned(),
                )
                .chain(missing_other)
                .collect();
            config.frozen_count += missing_frozen.len();
        }
        self.edit_definition(|definition| definition.view = ViewKind::Table(config))
    }

    pub fn computation_definition(&self) -> Option<DocumentDefinition> {
        let mut definition = self.working.clone();
        // A rerun SQL result may lack columns that cards and filters name.
        if let Some(definition) = &mut definition
            && matches!(definition.view, ViewKind::Sql(_))
        {
            definition.slicers.retain(|card| {
                card.sql
                    || self
                        .source
                        .columns
                        .iter()
                        .any(|column| column.0 == card.column)
            });
            definition.filters.retain(|filter| {
                filter.target.column().is_none_or(|name| {
                    self.source.columns.iter().any(|column| column.0 == name)
                })
            });
        }
        if let Some(definition) = &mut definition
            && matches!(definition.view, ViewKind::Table(_))
        {
            definition.view = ViewKind::Table(self.effective_table_config());
            if self.is_direct_table() {
                definition.slicers.retain(|card| {
                    card.sql
                        || self
                            .source
                            .columns
                            .iter()
                            .any(|column| column.0 == card.column)
                });
                definition.filters.retain(|filter| {
                    filter.target.column().is_none_or(|name| {
                        self.source.columns.iter().any(|column| column.0 == name)
                    })
                });
            }
        }
        definition
    }

    pub fn table_widths(&self) -> std::collections::BTreeMap<String, f32> {
        if self.is_direct_table() {
            return self.direct_table.widths.clone();
        }
        match self.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(config)) => config
                .presentation
                .iter()
                .filter(|(_, presentation)| valid_width(presentation.width))
                .map(|(name, presentation)| (name.clone(), presentation.width))
                .collect(),
            _ => Default::default(),
        }
    }

    pub fn set_column_width(&mut self, column: String, width: f32) {
        if !valid_width(width) {
            return;
        }
        if self.is_direct_table() {
            self.direct_table.widths.insert(column.clone(), width);
        }
        if let Some(definition) = &mut self.working
            && let ViewKind::Table(config) = &mut definition.view
        {
            config
                .presentation
                .entry(column)
                .and_modify(|presentation| presentation.width = width)
                .or_insert(crate::domain::documents::ColumnPresentation {
                    width,
                    alignment: crate::domain::documents::Alignment::Left,
                    format_override: None,
                });
            if !matches!(
                self.identity,
                SessionIdentity::RawSource | SessionIdentity::DraftTable
            ) {
                self.dirty = true;
            }
        }
    }

    pub fn take_table_patch(&mut self) -> Option<(String, TablePreferencesPatch)> {
        if !self.is_direct_table() || self.direct_table.source_path.is_empty() {
            return None;
        }
        let mut patch = TablePreferencesPatch::between(&self.queued_table, &self.direct_table);
        if std::mem::take(&mut self.table_reset_pending) {
            patch.reset = true;
            patch.widths.extend(
                self.direct_table
                    .widths
                    .iter()
                    .map(|(name, width)| (name.clone(), Some(*width))),
            );
        }
        self.queued_table = self.direct_table.clone();
        (!patch.is_empty()).then(|| (self.direct_table.source_path.clone(), patch))
    }

    pub fn query_config(&self) -> QueryConfig {
        match self.working.as_ref().map(|view| &view.view) {
            Some(ViewKind::Query(config)) => config.clone(),
            _ => QueryConfig::default(),
        }
    }

    pub fn set_query_config(&mut self, config: QueryConfig, name: String) -> Result<(), String> {
        if self.working.is_none() {
            self.start_table_draft(name)?;
        }
        self.edit_definition(|view| view.view = ViewKind::Query(config))?;
        let id = self
            .working
            .as_ref()
            .expect("working document exists")
            .id
            .clone();
        self.identity = if id.is_empty() {
            SessionIdentity::DraftQuery
        } else {
            SessionIdentity::SavedQuery(id)
        };
        self.normalize_empty_query();
        Ok(())
    }

    pub fn normalize_empty_query(&mut self) -> bool {
        let Some(definition) = &mut self.working else {
            return false;
        };
        // Relations without DuckDB's rowid can only be read through a query.
        if definition
            .database_view
            .as_ref()
            .is_some_and(|view| view.kind != RelationKind::Table)
        {
            return false;
        }
        if !matches!(&definition.view, ViewKind::Query(config) if config.group_by.is_empty() && config.pivot_by.is_empty() && config.aggregations.is_empty())
        {
            return false;
        }
        self.sort.retain(|key| {
            self.source
                .columns
                .iter()
                .any(|column| column.0 == key.column)
        });
        definition.view = ViewKind::Table(TableConfig {
            sort: self.sort.clone(),
            ..Default::default()
        });
        self.identity = if definition.id.is_empty() {
            SessionIdentity::DraftTable
        } else {
            SessionIdentity::SavedTable(definition.id.clone())
        };
        self.dirty = true;
        self.request_revision += 1;
        true
    }

    pub fn set_search_text(&mut self, search_text: String) -> JobToken {
        self.search_text = search_text;
        self.next_request()
    }

    pub fn toggle_sort(&mut self, column: String) -> JobToken {
        let descending = self
            .sort
            .iter()
            .find(|key| key.column == column)
            .is_some_and(|key| !key.descending);
        self.set_sort_direction(column, descending)
    }

    pub fn set_sort_direction(&mut self, column: String, descending: bool) -> JobToken {
        if let Some(key) = self.sort.iter_mut().find(|key| key.column == column) {
            key.descending = descending;
        } else {
            self.sort.push(SortKey { column, descending });
        }
        let direct = self.is_direct_table();
        if let Some(definition) = &mut self.working {
            match &mut definition.view {
                ViewKind::Table(table) => table.sort = self.sort.clone(),
                ViewKind::Query(pivot) => pivot.result_sort = self.sort.clone(),
                ViewKind::Sql(sql) => sql.result_sort = self.sort.clone(),
            }
            if !direct {
                self.dirty = true;
            }
        }
        self.next_request()
    }

    pub fn has_filters(&self) -> bool {
        !self.search_text.trim().is_empty()
            || self.working.as_ref().is_some_and(|definition| {
                !definition.filters.is_empty()
                    || definition.slicers.iter().any(|card| card.is_active())
            })
    }

    pub fn value_filters(&self) -> &[ValueFilter] {
        self.working
            .as_ref()
            .map_or(&[], |definition| definition.filters.as_slice())
    }

    /// Adds a value filter unless an identical one is already applied.
    pub fn add_value_filter(&mut self, filter: ValueFilter) -> Result<bool, String> {
        if self.value_filters().contains(&filter) {
            return Ok(false);
        }
        self.edit_definition(|definition| definition.filters.push(filter))?;
        Ok(true)
    }

    pub fn remove_value_filter(&mut self, index: usize) -> bool {
        if index >= self.value_filters().len() {
            return false;
        }
        self.edit_definition(|definition| {
            definition.filters.remove(index);
        })
        .is_ok()
    }

    pub fn clear_filters(&mut self) -> bool {
        if !self.has_filters() {
            return false;
        }
        self.search_text.clear();
        let direct = self.is_direct_table();
        if let Some(definition) = &mut self.working {
            let mut changed = !definition.filters.is_empty();
            definition.filters.clear();
            for card in &mut definition.slicers {
                changed |= card.is_active();
                card.clear();
            }
            self.dirty |= changed && !direct;
        }
        true
    }

    pub fn clear_sort(&mut self) -> bool {
        if self.sort.is_empty() {
            return false;
        }
        self.sort.clear();
        let direct = self.is_direct_table();
        if let Some(definition) = &mut self.working {
            match &mut definition.view {
                ViewKind::Table(table) => table.sort.clear(),
                ViewKind::Query(pivot) => pivot.result_sort.clear(),
                ViewKind::Sql(sql) => sql.result_sort.clear(),
            }
            if !direct {
                self.dirty = true;
            }
        }
        true
    }

    pub fn clear_sort_column(&mut self, column: &str) -> bool {
        let original_len = self.sort.len();
        self.sort.retain(|key| key.column != column);
        if self.sort.len() == original_len {
            return false;
        }
        let direct = self.is_direct_table();
        if let Some(definition) = &mut self.working {
            match &mut definition.view {
                ViewKind::Table(table) => table.sort = self.sort.clone(),
                ViewKind::Query(pivot) => pivot.result_sort = self.sort.clone(),
                ViewKind::Sql(sql) => sql.result_sort = self.sort.clone(),
            }
            if !direct {
                self.dirty = true;
            }
        }
        true
    }

    pub fn move_sort_key(&mut self, column: &str, direction: i32) -> bool {
        let Some(index) = self.sort.iter().position(|key| key.column == column) else {
            return false;
        };
        let target = if direction < 0 {
            index.checked_sub(1)
        } else if direction > 0 {
            (index + 1 < self.sort.len()).then_some(index + 1)
        } else {
            None
        };
        let Some(target) = target else {
            return false;
        };
        self.sort.swap(index, target);
        let direct = self.is_direct_table();
        if let Some(definition) = &mut self.working {
            match &mut definition.view {
                ViewKind::Table(table) => table.sort = self.sort.clone(),
                ViewKind::Query(pivot) => pivot.result_sort = self.sort.clone(),
                ViewKind::Sql(sql) => sql.result_sort = self.sort.clone(),
            }
            if !direct {
                self.dirty = true;
            }
        }
        self.next_request();
        true
    }

    pub fn next_request(&mut self) -> JobToken {
        self.request_revision += 1;
        self.token()
    }

    pub fn token(&self) -> JobToken {
        JobToken {
            session_generation: self.session_generation,
            source_revision: self.source.source_revision,
            request_revision: self.request_revision,
        }
    }

    pub fn accepts(&self, token: JobToken) -> bool {
        self.token() == token
    }

    pub fn publish_sql(&mut self, token: JobToken, trace: SqlTrace, error: Option<String>) {
        if self.accepts(token) {
            self.sql.finish(trace, error);
        }
    }

    pub fn restore_navigation(&mut self, mut previous: Self) {
        previous.session_generation = self.session_generation + 1;
        previous.request_revision = self.request_revision + 1;
        *self = previous;
    }

    pub fn replace_source(&mut self, source: FrameSnapshot) {
        self.source = source;
        self.session_generation += 1;
        self.request_revision += 1;
        self.sql = SqlDiagnostics::default();
    }

    pub fn reload_source(&mut self, source: FrameSnapshot, reset: bool) {
        self.replace_source(source);
        if reset {
            self.search_text.clear();
            self.sort.clear();
            if self.is_direct_table() {
                self.table_reset_pending = true;
                self.direct_table = TablePreferences::new(self.direct_table.source_path.clone());
            }
            if let Some(definition) = &mut self.working {
                definition.slicers.clear();
                definition.filters.clear();
                match &mut definition.view {
                    ViewKind::Table(table) => *table = Default::default(),
                    ViewKind::Query(pivot) => pivot.result_sort.clear(),
                    ViewKind::Sql(sql) => sql.result_sort.clear(),
                }
                self.dirty = !matches!(
                    self.identity,
                    SessionIdentity::RawSource | SessionIdentity::DraftTable
                );
            }
        } else if matches!(self.identity, SessionIdentity::RawSource) {
            self.sort.clear();
        }
    }

    pub fn navigate_to_raw(
        &mut self,
        source: Result<FrameSnapshot, String>,
        path: Option<String>,
    ) -> Result<(), String> {
        let source = source?;
        self.replace_source(source);
        self.source_path = path;
        self.identity = SessionIdentity::RawSource;
        self.direct_table = TablePreferences::new(self.source_path.clone().unwrap_or_default());
        self.queued_table = self.direct_table.clone();
        self.table_reset_pending = false;
        self.working = None;
        self.dirty = false;
        self.search_text.clear();
        self.sort.clear();
        Ok(())
    }

    pub fn navigate_to_view(
        &mut self,
        source: Result<FrameSnapshot, String>,
        definition: DocumentDefinition,
    ) -> Result<(), String> {
        definition.validate()?;
        let source = source?;
        let path = (!definition.source_path.is_empty()).then(|| definition.source_path.clone());
        self.navigate_to_raw(Ok(source), path)?;
        self.sort = match &definition.view {
            ViewKind::Table(table) => table.sort.clone(),
            ViewKind::Query(pivot) => pivot.result_sort.clone(),
            ViewKind::Sql(sql) => sql.result_sort.clone(),
        };
        self.saved(definition);
        if let Some(definition) = self
            .working
            .as_ref()
            .filter(|d| d.id.is_empty() && d.database_view.is_some())
        {
            self.identity = if matches!(definition.view, ViewKind::Table(_)) {
                SessionIdentity::DraftTable
            } else {
                SessionIdentity::DraftQuery
            };
        }
        Ok(())
    }

    pub fn save_as(&mut self, name: String, repository: &ViewRepository) -> io::Result<()> {
        if self.is_direct_table() {
            let mut config = self.effective_table_config();
            config.sort = self.sort.clone();
            if let Some(definition) = &mut self.working {
                definition.view = ViewKind::Table(config);
            } else {
                let mut definition = DocumentDefinition::draft_table(
                    name.clone(),
                    self.source_path.clone().unwrap_or_default(),
                );
                definition.view = ViewKind::Table(config);
                definition.slicers = self.direct_table.slicers();
                self.working = Some(definition);
            }
        }
        if self.working.is_none() {
            self.working = Some(DocumentDefinition::draft_table(
                name.clone(),
                self.source_path.clone().unwrap_or_default(),
            ));
            self.identity = SessionIdentity::DraftTable;
            self.dirty = true;
        }
        let definition = self.working.as_mut().expect("draft created");
        definition.name = name;
        self.dirty = true;
        let mut candidate = definition.clone();
        repository.save_view(&mut candidate)?;
        self.saved(candidate);
        Ok(())
    }

    pub fn rename_working(&mut self, name: String) -> Result<(), String> {
        let definition = self.working.as_mut().ok_or("no working view")?;
        if name.trim().is_empty() {
            return Err("view name is empty".into());
        }
        definition.name = name;
        self.dirty = true;
        Ok(())
    }

    pub fn saved(&mut self, definition: DocumentDefinition) {
        self.identity = match definition.view {
            ViewKind::Table(_) => SessionIdentity::SavedTable(definition.id.clone()),
            ViewKind::Query(_) => SessionIdentity::SavedQuery(definition.id.clone()),
            ViewKind::Sql(_) => SessionIdentity::SavedSql(definition.id.clone()),
        };
        self.working = Some(definition);
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_configuration_keeps_no_database_or_result_handles() {
        use crate::infrastructure::duckdb_store::DuckDbStore;
        let source = FrameSnapshot::new(DuckDbStore::synthetic(3).unwrap(), 1);
        let store = std::sync::Arc::downgrade(&source.store);
        let mut session = WindowSession::raw(source, Some("data.csv".into()));
        session.start_table_draft("Draft".into()).unwrap();
        session.rename_working("Unsaved".into()).unwrap();
        session.set_search_text("search".into());
        let entry = session.navigation_entry().unwrap();
        drop(session);
        assert!(store.upgrade().is_none());

        let fresh = FrameSnapshot::new(DuckDbStore::synthetic(3).unwrap(), 9);
        let mut restored = WindowSession::raw(fresh, Some("data.csv".into()));
        let old_token = restored.token();
        restored.restore_configuration(&entry);
        assert_eq!(restored.navigation_entry().unwrap(), entry);
        assert!(!restored.accepts(old_token));
        assert_eq!(restored.source.source_revision, 9);
    }

    #[test]
    fn blank_and_synthetic_sources_are_not_history_locations() {
        use crate::infrastructure::duckdb_store::DuckDbStore;
        let source = FrameSnapshot::new(DuckDbStore::synthetic(3).unwrap(), 1);
        let mut session = WindowSession::raw(source, None);
        assert!(session.navigation_entry().is_none());
        session.start_query_draft("Synthetic pivot".into()).unwrap();
        assert!(session.navigation_entry().is_none());
        session.start_sql_draft();
        assert!(session.navigation_entry().is_some());
    }

    #[test]
    fn direct_layout_autosaves_but_named_views_remain_independent() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let source = FrameSnapshot::new(DuckDbStore::synthetic(5).unwrap(), 1);
        let path = home
            .path()
            .join("source.csv")
            .to_string_lossy()
            .into_owned();
        let mut session = WindowSession::raw(source.clone(), Some(path.clone()));
        session.apply_direct_preferences(
            repository
                .load_table_preferences(std::path::Path::new(&path))
                .unwrap(),
        );
        session.set_column_width("Row".into(), 135.);
        session
            .edit_definition(|definition| {
                if let ViewKind::Table(config) = &mut definition.view {
                    config.column_order = vec!["Row".into()];
                    config.frozen_count = 1;
                }
                definition
                    .slicers
                    .push(crate::domain::documents::SlicerDefinition {
                        sql: false,
                        condition: None,
                        column: "Row".into(),
                        selected: vec![crate::domain::documents::ScalarKey::Signed("1".into())],
                    });
            })
            .unwrap();
        session.set_sort_direction("Row".into(), true);
        assert!(!session.dirty);
        let (source_path, patch) = session.take_table_patch().unwrap();
        repository
            .update_table_preferences(std::path::Path::new(&source_path), &patch)
            .unwrap();
        assert!(session.take_table_patch().is_none());
        session.save_as("Named".into(), &repository).unwrap();
        let first = session.working.clone().unwrap();
        assert_eq!(session.table_widths()["Row"], 135.);
        session.set_column_width("Row".into(), 245.);
        assert!(session.dirty);
        assert!(session.take_table_patch().is_none());
        assert_eq!(repository.load_views().0[0], first);
        session.save_as("Named".into(), &repository).unwrap();
        let mut reopened = WindowSession::raw(source.clone(), Some(path.clone()));
        reopened.apply_direct_preferences(
            repository
                .load_table_preferences(std::path::Path::new(&path))
                .unwrap(),
        );
        assert_eq!(reopened.table_widths()["Row"], 135.);
        assert_eq!(reopened.effective_table_config().frozen_count, 1);
        assert!(reopened.sort.is_empty());
        assert!(
            reopened.working.as_ref().unwrap().slicers[0]
                .selected
                .is_empty()
        );
        reopened
            .navigate_to_view(Ok(source), repository.load_views().0.remove(0))
            .unwrap();
        assert_eq!(reopened.table_widths()["Row"], 245.);
        assert!(reopened.sort[0].descending);
        assert!(
            !reopened.working.as_ref().unwrap().slicers[0]
                .selected
                .is_empty()
        );
    }

    #[test]
    fn reload_preserves_widths_and_reset_queues_removals() {
        let source = FrameSnapshot::new(DuckDbStore::synthetic(1).unwrap(), 1);
        let mut session = WindowSession::raw(source.clone(), Some("source.csv".into()));
        let mut preferences = TablePreferences::new("source.csv".into());
        preferences.widths.insert("Row".into(), 150.);
        preferences.widths.insert("missing".into(), 180.);
        session.apply_direct_preferences(preferences);
        session.reload_source(source.clone(), false);
        assert_eq!(session.table_widths()["Row"], 150.);
        assert!(session.take_table_patch().is_none());
        session.reload_source(source, true);
        assert!(session.table_widths().is_empty());
        let (_, patch) = session.take_table_patch().unwrap();
        assert_eq!(patch.widths["Row"], None);
        assert_eq!(patch.widths["missing"], None);
        assert!(!session.dirty);
    }

    #[test]
    fn named_layout_edits_preserve_missing_frozen_columns() {
        let source = FrameSnapshot::new(DuckDbStore::synthetic(1).unwrap(), 1);
        let mut session = WindowSession::raw(source, Some("source.csv".into()));
        let mut definition = DocumentDefinition::draft_table("Named".into(), "source.csv".into());
        definition.id = "named".into();
        definition.view = ViewKind::Table(TableConfig {
            column_order: vec!["missing".into(), "Row".into()],
            frozen_count: 1,
            ..Default::default()
        });
        session.saved(definition);
        let mut config = session.effective_table_config();
        assert_eq!(config.frozen_count, 0);
        crate::domain::table::toggle_frozen(&mut config, &["Row".into()], 0).unwrap();
        session.set_table_layout(config).unwrap();
        assert_eq!(session.effective_table_config().frozen_count, 1);
        let ViewKind::Table(saved) = &session.working.as_ref().unwrap().view else {
            panic!()
        };
        assert_eq!(saved.column_order, ["Row", "missing"]);
        assert_eq!(saved.frozen_count, 2);
    }

    #[test]
    fn empty_query_returns_to_table_preserving_document_and_slicers() {
        use crate::domain::documents::{ScalarKey, SlicerDefinition, ViewAction};
        let source = FrameSnapshot::new(DuckDbStore::synthetic(3).unwrap(), 1);
        let mut session = WindowSession::raw(source, Some("source.csv".into()));
        session.start_query_draft("Name".into()).unwrap();
        let definition = session.working.as_mut().unwrap();
        definition.id = "saved-id".into();
        definition.slicers = vec![SlicerDefinition {
            sql: false,
            condition: None,
            column: "Row".into(),
            selected: vec![ScalarKey::Signed("1".into())],
        }];
        definition.actions = vec![ViewAction {
            key: "x".into(),
            description: "Action".into(),
            command: "echo {Row}".into(),
        }];
        session.search_text = "1".into();
        session.sort = vec![
            SortKey {
                column: "Row".into(),
                descending: true,
            },
            SortKey {
                column: "gone".into(),
                descending: false,
            },
        ];
        let previous = session.working.clone().unwrap();
        assert!(session.normalize_empty_query());
        let definition = session.working.as_ref().unwrap();
        assert_eq!(definition.id, previous.id);
        assert_eq!(definition.name, previous.name);
        assert_eq!(definition.source_path, previous.source_path);
        assert_eq!(definition.slicers, previous.slicers);
        assert_eq!(definition.actions, previous.actions);
        assert_eq!(session.search_text, "1");
        assert_eq!(session.sort.len(), 1);
        assert_eq!(
            session.identity,
            SessionIdentity::SavedTable("saved-id".into())
        );
        assert!(
            matches!(&definition.view, ViewKind::Table(table) if table.sort == session.sort && table.presentation.is_empty())
        );
        assert!(session.dirty);
        session
            .set_query_config(
                QueryConfig {
                    group_by: vec!["Row".into()],
                    ..Default::default()
                },
                "unused".into(),
            )
            .unwrap();
        assert_eq!(
            session.identity,
            SessionIdentity::SavedQuery("saved-id".into())
        );
        assert_eq!(session.working.as_ref().unwrap().slicers, previous.slicers);
    }
    use crate::infrastructure::duckdb_store::DuckDbStore;

    #[test]
    fn sql_publication_rejects_obsolete_work_and_clears_on_session_change() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source.clone(), None);
        let mut trace = SqlTrace::default();
        trace.execute("Result", "SELECT 1", &[], || Ok(())).unwrap();
        let old = session.token();
        session.publish_sql(old, trace.clone(), None);
        session.next_request();
        session.publish_sql(old, SqlTrace::default(), Some("obsolete error".into()));
        assert!(session.sql.failed.is_none());
        assert_eq!(session.sql.displayed.statements.len(), 1);
        session.publish_sql(
            session.token(),
            SqlTrace::default(),
            Some("new error".into()),
        );
        assert_eq!(session.sql.displayed.statements.len(), 1);
        assert!(session.sql.failed.is_some());
        session.replace_source(source);
        assert!(session.sql.displayed.statements.is_empty());
        assert!(session.sql.failed.is_none());
        session.publish_sql(session.token(), trace, None);
        session.start_query_draft("Pivot".into()).unwrap();
        assert!(session.sql.displayed.statements.is_empty());
    }

    #[test]
    fn clear_filters_keeps_cards_sort_and_pivot_configuration() {
        use crate::domain::documents::{ScalarKey, SlicerDefinition};
        for pivot in [false, true] {
            let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
            let mut session = WindowSession::raw(source, None);
            if pivot {
                session.start_query_draft("Pivot".into()).unwrap();
            } else {
                session.start_table_draft("Table".into()).unwrap();
            }
            session.working.as_mut().unwrap().slicers = vec![
                SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "region".into(),
                    selected: vec![ScalarKey::String("North".into())],
                },
                SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "n".into(),
                    selected: vec![],
                },
            ];
            session.set_sort_direction("n".into(), true);
            let original_view = session.working.as_ref().unwrap().view.clone();
            session.search_text = "north".into();
            session.dirty = false;
            assert!(session.has_filters());
            assert!(session.clear_filters());
            assert!(!session.has_filters());
            assert_eq!(session.dirty, pivot);
            assert_eq!(session.working.as_ref().unwrap().view, original_view);
            assert_eq!(session.working.as_ref().unwrap().slicers.len(), 2);
            assert!(
                session
                    .working
                    .as_ref()
                    .unwrap()
                    .slicers
                    .iter()
                    .all(|card| card.selected.is_empty())
            );
            session.dirty = false;
            assert!(!session.clear_filters());
            assert!(!session.dirty);
            session.search_text = "search only".into();
            assert!(session.clear_filters());
            assert!(!session.dirty);
        }
    }

    #[test]
    fn value_filters_dedupe_follow_query_drafts_and_clear_with_filters() {
        let source = FrameSnapshot::new(DuckDbStore::synthetic(3).unwrap(), 1);
        let column = source.columns[0].0.clone();
        let mut session = WindowSession::raw(source, Some("data.csv".into()));
        assert!(
            session
                .add_value_filter(ValueFilter {
                    target: column.as_str().into(),
                    value: None,
                    exclude: false,
                })
                .is_err()
        );
        session.start_table_draft("Table".into()).unwrap();
        let filter = ValueFilter {
            target: column.as_str().into(),
            value: Some("x".into()),
            exclude: true,
        };
        assert!(session.add_value_filter(filter.clone()).unwrap());
        assert!(!session.add_value_filter(filter.clone()).unwrap());
        assert!(session.has_filters());
        let missing = ValueFilter {
            target: "missing".into(),
            value: None,
            exclude: false,
        };
        session.add_value_filter(missing).unwrap();
        assert_eq!(
            session.computation_definition().unwrap().filters,
            std::slice::from_ref(&filter)
        );
        assert!(session.remove_value_filter(1));
        assert!(!session.remove_value_filter(1));
        session.start_query_draft("Pivot".into()).unwrap();
        assert_eq!(session.value_filters(), [filter]);
        session.dirty = false;
        assert!(session.clear_filters());
        assert!(session.dirty);
        assert!(session.value_filters().is_empty());
        assert!(!session.has_filters());
    }

    #[test]
    fn navigation_is_query_free_retains_display_and_never_resets_request_ids() {
        let source = FrameSnapshot::new(DuckDbStore::synthetic(10).unwrap(), 3);
        let displayed = ResultSnapshot::from_source(&source, 9, 12);
        let _store_guard = source.store.lock().unwrap();
        // Holding the store lock proves neither construction nor navigation accesses SQL.
        let mut session = WindowSession::raw(source.clone(), Some("data.csv".into()));
        session.result = displayed.clone();
        let old = session.next_request();
        let definition = DocumentDefinition::draft_table("Next".into(), "data.csv".into());
        session
            .navigate_to_view(Ok(source.clone()), definition)
            .unwrap();
        assert!(session.result.row_ids.same(&displayed.row_ids));
        assert!(session.token().request_revision > old.request_revision);
        assert!(!session.accepts(old));
        let changed = session.token();
        session
            .navigate_to_raw(Ok(source.clone()), Some("data.csv".into()))
            .unwrap();
        assert!(!session.accepts(changed));
        assert_eq!(session.result.result_revision, 9);
    }

    #[test]
    fn edits_are_dirty_but_search_is_not() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, Some("C:\\data.csv".into()));
        session.start_table_draft("Data".into()).unwrap();
        session.dirty = false;
        session.set_search_text("abc".into());
        assert!(!session.dirty);
        session
            .edit_definition(|definition| definition.name = "Changed".into())
            .unwrap();
        assert!(session.dirty);
    }

    #[test]
    fn explicit_sort_direction_preserves_priority() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, None);
        session.start_table_draft("Data".into()).unwrap();
        session.set_sort_direction("region".into(), false);
        session.set_sort_direction("sales".into(), true);
        session.set_sort_direction("region".into(), true);
        assert_eq!(session.sort[0].column, "region");
        assert!(session.sort[0].descending);
        assert_eq!(session.sort[1].column, "sales");
        assert!(session.sort[1].descending);
        assert_eq!(
            session.working.as_ref().and_then(|view| match &view.view {
                ViewKind::Table(table) => Some(&table.sort),
                _ => None,
            }),
            Some(&session.sort)
        );
    }

    #[test]
    fn obsolete_tokens_are_rejected() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, None);
        let first = session.next_request();
        session.set_search_text("newer".into());
        assert!(!session.accepts(first));
    }

    #[test]
    fn failed_navigation_preserves_unsaved_work() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, Some("C:\\first.csv".into()));
        session.start_table_draft("First".into()).unwrap();
        session.rename_working("Edited".into()).unwrap();
        assert!(
            session
                .navigate_to_raw(Err("missing".into()), Some("C:\\second.csv".into()))
                .is_err()
        );
        assert_eq!(session.working.as_ref().unwrap().name, "Edited");
        assert!(session.dirty);
    }

    #[test]
    fn failed_save_keeps_draft_and_successful_save_clears_dirty() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, None);
        session.start_query_draft("Pivot".into()).unwrap();
        assert!(session.save_as("Pivot".into(), &repository).is_err());
        assert!(session.dirty);
        assert!(matches!(session.identity, SessionIdentity::DraftQuery));
        session.source_path = Some("C:\\source.csv".into());
        session.working.as_mut().unwrap().source_path = "C:\\source.csv".into();
        session.save_as("Pivot".into(), &repository).unwrap();
        assert!(!session.dirty);
        assert!(matches!(session.identity, SessionIdentity::SavedQuery(_)));
    }

    #[test]
    fn reload_reset_keeps_pivot_configuration() {
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source, Some("C:\\source.csv".into()));
        session.start_query_draft("Pivot".into()).unwrap();
        session
            .edit_definition(|definition| {
                if let ViewKind::Query(pivot) = &mut definition.view {
                    pivot.group_by.push("Region".into());
                    pivot.result_sort.push(SortKey {
                        column: "Total".into(),
                        descending: true,
                    });
                }
            })
            .unwrap();
        session.set_search_text("east".into());
        session.reload_source(FrameSnapshot::new(DuckDbStore::empty().unwrap(), 2), true);
        let ViewKind::Query(pivot) = &session.working.as_ref().unwrap().view else {
            panic!("expected pivot");
        };
        assert_eq!(
            pivot.group_by,
            vec![crate::domain::documents::Dimension::from("Region")]
        );
        assert!(pivot.result_sort.is_empty());
        assert!(session.search_text.is_empty());
        assert!(session.dirty);
    }

    #[test]
    fn slicer_order_and_typed_selections_save_reopen_and_reset_explicitly() {
        use crate::domain::documents::{ScalarKey, SlicerDefinition};
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(
            crate::infrastructure::json_store::StorePaths::for_home(home.path()),
        );
        let source = FrameSnapshot::new(DuckDbStore::empty().unwrap(), 1);
        let mut session = WindowSession::raw(source.clone(), Some("C:\\source.csv".into()));
        session.start_table_draft("Sliced".into()).unwrap();
        let cards = vec![
            SlicerDefinition {
                sql: false,
                condition: None,
                column: "n".into(),
                selected: vec![ScalarKey::Unsigned(u128::MAX.to_string())],
            },
            SlicerDefinition {
                sql: false,
                condition: None,
                column: "text".into(),
                selected: vec![ScalarKey::Null, ScalarKey::String(String::new())],
            },
        ];
        session
            .edit_definition(|definition| definition.slicers = cards.clone())
            .unwrap();
        assert!(repository.load_views().0.is_empty());
        session.save_as("Sliced".into(), &repository).unwrap();
        let definition = repository.load_views().0.remove(0);
        assert_eq!(definition.slicers, cards);
        session
            .navigate_to_view(Ok(source.clone()), definition)
            .unwrap();
        assert!(!session.dirty);
        session.start_query_draft("Pivot".into()).unwrap();
        assert_eq!(session.working.as_ref().unwrap().slicers, cards);
        session.reload_source(source.clone(), false);
        assert_eq!(session.working.as_ref().unwrap().slicers, cards);
        session.reload_source(source, true);
        assert!(session.working.as_ref().unwrap().slicers.is_empty());
        assert!(session.dirty);
        assert_eq!(repository.load_views().0[0].slicers, cards);
    }
}
