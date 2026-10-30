//! A DataDiver window: the grid of the current table, query, or SQL result
//! with its search, slicers, panels, and dialogs.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::application::commands::{COMMANDS, Command as AppCommand, CommandId};
use crate::application::computation::{ComputationController, ScheduleReason};
use crate::application::history::HistoryWriter;
use crate::application::keymap::{KeyContext, KeymapFile};
use crate::application::navigation_history::{NavigationEntry, NavigationHistory};
use crate::application::pivot_editor::PivotFieldKind;
use crate::application::session::{JobToken, SessionIdentity, WindowSession};
use crate::application::slicers::{SlicerDiscovery, SlicerStage};
use crate::application::table_preferences::TablePreferencesWriter;
use crate::application::theme::ThemeSettings;
use crate::application::undo::UndoHistory;
use crate::domain::documents::{
    Aggregation, Dimension, DocumentDefinition, QueryConfig, SlicerDefinition, SortKey,
    ValueFilter, ViewKind,
};
use crate::domain::export::CopyFormat;
use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
use crate::domain::query::Token;
use crate::domain::selection::Selection;
use crate::domain::sql::SqlTrace;
use crate::domain::statistics::SelectionStatistics;
use crate::infrastructure::duckdb_store::{ColumnSchema, ViewQuery};
use crate::infrastructure::json_store::{StorePaths, ViewRepository, WindowGeometry};
use crate::infrastructure::source_database::SourceDatabase;
use crate::infrastructure::sources::SourceProbe;
use crate::ui::cell_detail::CellDetailPane;
use crate::ui::column_completion::{ColumnCompletion, ColumnSyntax};
use crate::ui::database_tree::DatabaseTree;
use crate::ui::font_picker::FontPreferences;
use crate::ui::grid::{GridEvent, GridHost, GridView};
use crate::ui::hit::Hit;
use crate::ui::keymap_editor::KeymapEditor;
use crate::ui::platform::{Cx, Task};
use crate::ui::slicer_sidebar::SlicerSidebar;
use crate::ui::sql_panel::SqlPanel;
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{Fonts, WidgetId};

#[cfg(test)]
mod action_tests;
mod auto_save;
mod commands;
mod database_tree;
mod dialogs;
mod editor_settings;
mod file_settings;
#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod focus_tests;
mod input;
mod navigation;
#[cfg(test)]
mod navigation_tests;
mod performance;
mod pipeline;
pub(crate) mod profiles;
mod render;
mod search_completion;
mod sql_editor;
#[cfg(test)]
mod sql_tests;
mod startup;
#[cfg(test)]
mod test_support;
mod view_options;
mod watched_files;

pub use dialogs::Dialog;
pub use startup::ViewerOptions;

pub(crate) const COPY_FORMATS: &[(CopyFormat, &str)] = &[
    (CopyFormat::RawTsv, "Raw TSV"),
    (CopyFormat::FormattedTsv, "Formatted TSV"),
    (CopyFormat::RawTsvHeaders, "Raw TSV with headers"),
    (CopyFormat::FormattedTsvHeaders, "Formatted TSV with headers"),
    (CopyFormat::RawCsv, "Raw CSV"),
    (CopyFormat::JsonRecords, "JSON records"),
    (CopyFormat::JsonColumns, "JSON columns"),
    (CopyFormat::PythonList, "Python list"),
];

#[derive(Clone, Debug)]
pub(crate) enum FileEntry {
    View(DocumentDefinition),
    Source(PathBuf),
    StartupSql,
}

/// Filters applied to a source table once a drill-down has opened it.
#[derive(Clone, Debug)]
pub(crate) struct DrillDown {
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
}

pub(crate) enum SourceTarget {
    File(PathBuf),
    DatabaseView(crate::domain::documents::DatabaseView),
    /// A saved SQL source, by id.
    SqlSource(String),
}

impl FileEntry {
    pub(crate) fn is_saved_view(&self) -> bool {
        matches!(self, Self::View(view) if !view.id.is_empty())
    }

    /// A saved view, or the rows of a saved SQL source, which archive the
    /// source.
    pub(crate) fn is_archivable(&self) -> bool {
        self.is_saved_view() || matches!(self, Self::View(view) if view.sql_source.is_some())
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::View(view) => view.name.clone(),
            Self::Source(path) => source_file_name(path),
            Self::StartupSql => crate::infrastructure::source_database::STARTUP_SCRIPT.into(),
        }
    }

    pub(crate) fn source_parent(&self) -> Option<String> {
        match self {
            Self::Source(path) => Some(source_parent_folder(path)),
            Self::View(_) | Self::StartupSql => None,
        }
    }

    pub(crate) fn search_text(&self) -> String {
        match self {
            Self::View(view) => format!(
                "{} {} {}",
                view.name,
                view.source_path,
                match &view.view {
                    _ if view.sql_source.is_some() => "sql source",
                    ViewKind::Table(_) => "table view",
                    ViewKind::Query(_) => "query view",
                    ViewKind::Sql(config) if config.script => "sql script",
                    ViewKind::Sql(_) => "sql source",
                }
            ),
            Self::Source(path) => format!("{} source", path.display()),
            Self::StartupSql => "duckdb_startup.sql startup SQL script settings".into(),
        }
    }
}

pub(crate) fn source_file_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy())
        .map(|name| name.into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn source_parent_folder(path: &std::path::Path) -> String {
    path.parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy())
        .map(|name| name.into_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| {
            path.parent()
                .map(|parent| parent.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Other".into())
}

pub(crate) fn short_source(path: &std::path::Path) -> String {
    let name = source_file_name(path);
    let parent = source_parent_folder(path);
    if parent == "Other" {
        name
    } else {
        format!("{name} · {parent}")
    }
}

pub(crate) fn local_archive_time(unix_ms: u128) -> String {
    i64::try_from(unix_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown date".into())
}

/// Whether a query result stopped at the row limit and may be missing rows.
pub(crate) fn query_limit_reached(result_rows: usize, query_limit: u64) -> bool {
    result_rows as u64 >= query_limit
}

pub(crate) fn source_modified(path: Option<&str>) -> Option<String> {
    let path = path?;
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let local: chrono::DateTime<chrono::Local> = modified.into();
    Some(local.format("%Y-%m-%d %I:%M %p").to_string())
}

const ACTIVE_COMMANDS: &[CommandId] = &[
    CommandId::RunSql,
    CommandId::RunAllSql,
    CommandId::SqlToggleLineComment,
    CommandId::SqlToggleBlockComment,
    CommandId::SqlFind,
    CommandId::SqlReplace,
    CommandId::SqlGoToLine,
    CommandId::SqlShowCompletions,
    CommandId::SqlShowSignature,
    CommandId::SqlAddCursorAbove,
    CommandId::SqlAddCursorBelow,
    CommandId::SqlAddNextOccurrence,
    CommandId::SqlSelectAllOccurrences,
    CommandId::SqlMoveLinesUp,
    CommandId::SqlMoveLinesDown,
    CommandId::SqlCopyLinesUp,
    CommandId::SqlCopyLinesDown,
    CommandId::SqlDeleteLines,
    CommandId::SqlSelectLine,
    CommandId::SqlExpandSelection,
    CommandId::SqlShrinkSelection,
    CommandId::SqlJumpToBracket,
    CommandId::SqlFold,
    CommandId::SqlUnfold,
    CommandId::SqlFoldAll,
    CommandId::SqlUnfoldAll,
    CommandId::SqlToggleWordWrap,
    CommandId::SqlInsertLineBelow,
    CommandId::SqlInsertLineAbove,
    CommandId::Back,
    CommandId::Forward,
    CommandId::SqlEditor,
    CommandId::NewSqlSource,
    CommandId::NewSqlScript,
    CommandId::ArchiveSql,
    CommandId::DeleteSql,
    CommandId::RecoverViews,
    CommandId::Settings,
    CommandId::ViewOptions,
    CommandId::QueryEngine,
    CommandId::TogglePreserveOrder,
    CommandId::ToggleCrossFilter,
    CommandId::ToggleAvailableFirst,
    CommandId::StartupSql,
    CommandId::ResetDatabase,
    CommandId::Files,
    CommandId::DatabaseTree,
    CommandId::Commands,
    CommandId::Keymap,
    CommandId::Theme,
    CommandId::Fonts,
    CommandId::EditorFont,
    CommandId::IncreaseFontSize,
    CommandId::DecreaseFontSize,
    CommandId::ClearFilters,
    CommandId::Open,
    CommandId::PasteTable,
    CommandId::AddFolder,
    CommandId::AddWatchedFiles,
    CommandId::CacheFiles,
    CommandId::OpenSettingsDirectory,
    CommandId::Reload,
    CommandId::ReloadReset,
    CommandId::Save,
    CommandId::NewWindow,
    CommandId::Copy,
    CommandId::CopyAs,
    CommandId::Search,
    CommandId::Help,
    CommandId::Info,
    CommandId::Group,
    CommandId::Aggregation,
    CommandId::Pivot,
    CommandId::OrderBy,
    CommandId::AddSlicer,
    CommandId::FilterToValue,
    CommandId::ExcludeValue,
    CommandId::DrillDown,
    CommandId::DrillDownNewWindow,
    CommandId::CellDetail,
    CommandId::JumpToColumn,
    CommandId::Undo,
    CommandId::Redo,
    CommandId::FocusGrid,
    CommandId::FocusSlicers,
    CommandId::Export,
    CommandId::ToggleTotals,
    CommandId::ToggleHeatmap,
    CommandId::RecalculateWidths,
    CommandId::HideColumns,
    CommandId::ChooseColumns,
    CommandId::ShowAllColumns,
    CommandId::RunAction,
    CommandId::EditActions,
    CommandId::AiProviders,
    CommandId::AskSql,
];

pub(crate) fn active_commands() -> Vec<&'static AppCommand> {
    COMMANDS
        .iter()
        .filter(|command| ACTIVE_COMMANDS.contains(&command.id))
        .collect()
}

pub(crate) fn available_sort_keys(sort: &[SortKey], schema: &[ColumnSchema]) -> Vec<SortKey> {
    sort.iter()
        .filter(|key| schema.iter().any(|column| column.name == key.column))
        .cloned()
        .collect()
}

/// The favorite slot Ctrl+1–9 opens, and whether Shift assigns it.
pub(crate) fn favorite_shortcut(event: &crate::ui::platform::KeyEvent) -> Option<(usize, bool)> {
    let modifiers = event.modifiers;
    if !modifiers.control || modifiers.alt || modifiers.platform {
        return None;
    }
    let digit = event.key.parse::<usize>().ok().filter(|digit| (1..=9).contains(digit))?;
    Some((digit - 1, modifiers.shift))
}

#[cfg(test)]
mod favorite_shortcut_tests {
    use super::favorite_shortcut;
    use crate::ui::platform::KeyEvent;

    #[test]
    fn maps_control_digit_shortcuts_to_favorite_slots() {
        for digit in 1..=9 {
            for (prefix, assign) in [("ctrl", false), ("ctrl-shift", true)] {
                assert_eq!(
                    favorite_shortcut(&KeyEvent::parse(&format!("{prefix}-{digit}")).unwrap()),
                    Some((digit - 1, assign))
                );
            }
        }
        for stroke in ["ctrl-0", "ctrl-alt-1", "shift-1", "ctrl-cmd-1"] {
            assert_eq!(favorite_shortcut(&KeyEvent::parse(stroke).unwrap()), None);
        }
    }
}

pub(crate) struct PipelineRequest {
    query_candidate: Option<QueryCandidate>,
    pending_setup: Option<ResultSnapshot>,
    query_limit: u64,
    source: FrameSnapshot,
    tokens: Vec<Token>,
    sort: Vec<SortKey>,
    definition: Option<DocumentDefinition>,
    query_cache: Option<(FrameSnapshot, SqlTrace)>,
    slicer_discovery: Option<std::sync::Arc<SlicerDiscovery>>,
    slicer_options: crate::domain::table_preferences::SlicerOptions,
    queued_at: Instant,
    reuse_table: Option<(ViewQuery, ResultSnapshot)>,
    table_trace: SqlTrace,
}

#[derive(Clone)]
pub(crate) enum CandidateEditor {
    Aggregation {
        panel: WidgetId,
        preferred: Option<Aggregation>,
    },
    Dimension {
        panel: WidgetId,
        kind: PivotFieldKind,
        preferred: Option<Dimension>,
    },
}

/// An edit from the group, pivot, or aggregation editor, checked by running
/// it before the view keeps it.
#[derive(Clone)]
pub(crate) struct QueryCandidate {
    editor: CandidateEditor,
    revision: u64,
    config: QueryConfig,
}

pub(crate) struct NavigationBackup {
    session: WindowSession,
    source_modified: Option<String>,
    table_preferences_error: Option<String>,
    slicer_stage: Option<SlicerStage>,
    query_cache: Option<QueryResultCache>,
    table_query: Option<(ViewQuery, SqlTrace)>,
}

pub(crate) struct PendingStatistics {
    result: ResultSnapshot,
    selection: Selection,
    revision: u64,
    generation: u64,
    result_revision: u64,
    deadline: Instant,
}

/// Everything a grand total row depends on; search and sort do not change it.
#[derive(Clone, PartialEq)]
pub(crate) struct TotalsKey {
    source_revision: u64,
    store: usize,
    config: QueryConfig,
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
}

/// A query to rerun without the query limit, with the result's search and sort.
pub(crate) struct UnlimitedQuery {
    source: FrameSnapshot,
    config: QueryConfig,
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
    tokens: Vec<Token>,
    sort: Vec<SortKey>,
}

impl UnlimitedQuery {
    fn run(self) -> Result<ResultSnapshot, String> {
        let source = self
            .source
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
        let store = crate::domain::pivot::execute_filtered(
            &source,
            &self.config,
            &self.slicers,
            &self.filters,
            &crate::domain::pivot::EvaluationClock::system(),
            i64::MAX as u64,
            &mut SqlTrace::default(),
        )?;
        drop(source);
        let frame = FrameSnapshot::new(store, self.source.source_revision);
        let schema = frame.schema.to_vec();
        let rows = frame
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?
            .query_view_rows(&ViewQuery {
                hidden: Vec::new(),
                tokens: self.tokens,
                filters: Vec::new(),
                value_filters: Vec::new(),
                sort: available_sort_keys(&self.sort, &schema),
            })?;
        Ok(ResultSnapshot::from_view(&frame, rows, schema, 0, 0))
    }
}

#[derive(Clone)]
pub(crate) struct QueryResultCache {
    query_limit: u64,
    source: FrameSnapshot,
    config: QueryConfig,
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
    frame: FrameSnapshot,
    trace: SqlTrace,
}

impl QueryResultCache {
    fn matches(&self, source: &FrameSnapshot, definition: &DocumentDefinition, query_limit: u64) -> bool {
        let ViewKind::Query(config) = &definition.view else {
            return false;
        };
        self.source.source_revision == source.source_revision
            && std::sync::Arc::ptr_eq(&self.source.store, &source.store)
            && self.config.group_by == config.group_by
            && self.config.pivot_by == config.pivot_by
            && self.config.aggregations == config.aggregations
            && self.config.totals == config.totals
            && self.slicers == definition.slicers
            && self.filters == definition.filters
            && self.query_limit == query_limit
    }
}

/// A splitter being dragged: where the press started and the size then.
#[derive(Clone, Copy)]
pub(crate) struct SplitDrag {
    splitter: crate::ui::hit::Splitter,
    start: f32,
    size: f32,
}

/// Identifies a query result without keeping it alive.
type StoreHandle = std::sync::Weak<std::sync::Mutex<crate::infrastructure::store::Store>>;
type HeatRanges = std::collections::HashMap<String, (f64, f64)>;

pub struct DataFrameView {
    pub(crate) sql_panel: Option<SqlPanel>,
    pub(crate) sql_panel_open: bool,
    pub(crate) sql_running: bool,
    sql_revision: u64,
    auto_save_task: Option<Task>,
    /// A command to run once the location being opened is shown.
    pub(crate) after_open: Option<CommandId>,
    sql_service: std::sync::Arc<std::sync::Mutex<Option<crate::infrastructure::sql_execution::SqlExecution>>>,
    /// Where SQL on the Polars engine runs; tables it creates stay for the next run.
    sql_polars: std::sync::Arc<std::sync::Mutex<polars::sql::SQLContext>>,
    sql_interrupt: Option<std::sync::Arc<duckdb::InterruptHandle>>,
    sql_stopped: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    sql_restore_sort: bool,
    sql_catalog_revision: u64,
    sql_catalog_refreshing: bool,
    /// Columns of the files SQL reads, kept across editors until a file changes.
    sql_file_schemas: std::sync::Arc<std::sync::Mutex<crate::infrastructure::file_schemas::FileSchemas>>,
    /// Describes the script's files once typing pauses.
    sql_file_schema_task: Option<Task>,
    sql_describing: bool,
    /// The script changed while its files were being described.
    sql_describe_again: bool,
    /// The SQL result holds every row of its query, so it can be filtered.
    pub(crate) sql_result_complete: bool,
    pub(crate) sql_editor_height: f32,
    pub(crate) database_tree: DatabaseTree,
    pub(crate) database_tree_open: bool,
    pub(crate) database_tree_width: f32,
    pub(crate) session: WindowSession,
    computation: ComputationController<PipelineRequest>,
    pub(crate) grid: GridView,
    fitted_query_config: Option<QueryConfig>,
    pub(crate) search_input: Editor,
    pub(crate) search_completion: ColumnCompletion,
    completion_columns_key: (usize, usize),
    pub(crate) limit_input: Editor,
    pub(crate) query_limit: u64,
    pub(crate) limit_error: Option<String>,
    pub(crate) repository: ViewRepository,
    pub(crate) database: std::sync::Arc<SourceDatabase>,
    pub(crate) sources: std::sync::Arc<crate::infrastructure::sources::Sources>,
    /// The open file's own settings, as last read with it.
    pub(crate) file_settings: crate::domain::table_preferences::FileSettings,
    history_writer: std::sync::Arc<HistoryWriter>,
    table_writer: std::sync::Arc<TablePreferencesWriter>,
    pub(crate) source_modified: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) table_preferences_error: Option<String>,
    pub(crate) busy: bool,
    debouncing: bool,
    pub(crate) loading: bool,
    copy_revision: u64,
    pub(crate) copy_status: Option<String>,
    statistics_revision: u64,
    pub(crate) statistics: Option<SelectionStatistics>,
    pub(crate) statistics_error: Option<String>,
    pub(crate) statistics_pending: bool,
    statistics_running: bool,
    pending_statistics: Option<PendingStatistics>,
    pub(crate) selection_counts: Option<(usize, usize, usize)>,
    information_revision: u64,
    pub(crate) information_busy: bool,
    /// The grand total row of the last query with totals, and what it covers.
    totals: Option<(TotalsKey, crate::domain::pivot::TotalsRow)>,
    totals_revision: u64,
    /// Heat-map ranges and the query result they were measured on.
    heat_ranges: Option<(StoreHandle, HeatRanges)>,
    heat_revision: u64,
    pub(crate) query_cache: Option<QueryResultCache>,
    table_query: Option<(ViewQuery, SqlTrace)>,
    navigation_backup: Option<NavigationBackup>,
    pub(crate) navigation_history: NavigationHistory,
    undo_history: UndoHistory,
    navigation_target: Option<(usize, NavigationEntry)>,
    slicer_stage: Option<SlicerStage>,
    pub(crate) slicer_sidebar: SlicerSidebar,
    /// The cell detail pane while it is open.
    pub(crate) cell_detail: Option<CellDetailPane>,
    /// Every column's values, wanted while the Add Slicer picker is open.
    slicer_discovery: Option<std::sync::Arc<SlicerDiscovery>>,
    pub(crate) file_groups: [Vec<FileEntry>; 6],
    pub(crate) file_loading: bool,
    file_revision: u64,
    pub(crate) space_menu_open: bool,
    pub(crate) action_menu_open: bool,
    /// The overlay that adds dropped files to the cache instead of opening them.
    pub(crate) cache_drop_open: bool,
    pub(crate) keymap: KeymapFile,
    pub(crate) theme: ThemeSettings,
    pub(crate) font: FontPreferences,
    /// How the SQL editor looks and behaves, shared by the profile's windows.
    pub(crate) editor_settings: crate::infrastructure::json_store::EditorSettings,
    pub(crate) fonts: Fonts,
    pub(crate) keymap_profile: PathBuf,
    pub(crate) keymap_editor: Option<KeymapEditor>,
    pub(crate) dialog: Option<Dialog>,
    /// The focused control; `None` leaves keys to the window.
    pub(crate) focus: Option<WidgetId>,
    window_geometry: Option<WindowGeometry>,
    geometry_save: Option<Task>,
    source_monitor: Option<Task>,
    source_check_pending: bool,
    /// Whether this window has the keyboard; the tree checks for changes
    /// only then, and once more when the window comes back.
    pub(crate) window_active: bool,
    pub(crate) tree_check_pending: bool,
    /// The probe from the last check that reused the source, keyed by path and
    /// source revision; an unchanged probe skips the DuckDB catalog queries.
    source_probe: Option<(String, u64, SourceProbe)>,
    source_refresh_error: Option<String>,
    /// The shared settings revision this window last caught up with.
    profiles_seen: u64,
    /// The control that took the last mouse press, for its drags.
    pub(crate) capture: Option<Hit>,
    pub(crate) split_drag: Option<SplitDrag>,
    pub(crate) benchmark: bool,
    /// Frames painted, for scripts that wait for one.
    pub(crate) frames: std::rc::Rc<std::cell::Cell<u64>>,
}

impl GridHost for DataFrameView {
    fn grid_mut(&mut self) -> &mut GridView {
        &mut self.grid
    }
}

impl DataFrameView {
    pub fn new(
        frame: FrameSnapshot,
        source_path: Option<String>,
        initial_search: String,
        load_view: Option<DocumentDefinition>,
        repository: ViewRepository,
        cx: &mut Cx<Self>,
    ) -> Self {
        let source_modified = source_modified(source_path.as_deref());
        let mut session = WindowSession::raw(frame, source_path);
        if let Some(definition) = load_view {
            session.sort = match &definition.view {
                ViewKind::Table(table) => table.sort.clone(),
                ViewKind::Query(pivot) => pivot.result_sort.clone(),
                ViewKind::Sql(sql) => sql.result_sort.clone(),
            };
            session.saved(definition);
            if session.is_sql() {
                session.source_path = None;
            }
        }
        let table_writer = TablePreferencesWriter::for_repository(&repository);
        let mut table_load_error = None;
        if session.is_direct_table()
            && let Some(path) = &session.source_path
        {
            match table_writer.load(std::path::Path::new(path)) {
                Ok(preferences) => session.apply_direct_preferences(preferences),
                Err(error) => table_load_error = Some(format!("Cannot read table preferences: {error}")),
            }
        }
        if !initial_search.is_empty() {
            session.set_search_text(initial_search.clone());
        }
        let mut grid = GridView::new(session.result.clone());
        let initial_frozen = match session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(_)) => {
                crate::domain::table::visible_frozen_count(&session.effective_table_config())
            }
            Some(ViewKind::Query(pivot)) => pivot.group_by.len(),
            // A SQL result arrives after it runs; publishing it sets the count.
            Some(ViewKind::Sql(_)) => 0,
            None => 0,
        };
        grid.set_frozen_count(initial_frozen);
        if !session.sort.is_empty() {
            grid.set_sort(session.sort.clone());
        }
        let mut search_input = Editor::single_line().with_placeholder("Search rows");
        if !initial_search.is_empty() {
            search_input.set_text(initial_search);
        }
        let keymap_profile = repository.paths.config.clone();
        let settings = repository.load_settings().unwrap_or_default();
        profiles::with(|profiles| {
            profiles
                .column_formats
                .entry(keymap_profile.clone())
                .or_insert_with(|| settings.column_formats.clone());
        });
        let query_limit = profiles::with(|profiles| {
            *profiles
                .query_limits
                .entry(keymap_profile.clone())
                .or_insert_with(|| {
                    if settings.query_limit > 0 && settings.query_limit <= i64::MAX as u64 {
                        settings.query_limit
                    } else {
                        crate::infrastructure::json_store::DEFAULT_QUERY_LIMIT
                    }
                })
        });
        let mut limit_input = Editor::single_line().with_placeholder("Limit");
        limit_input.set_text(query_limit.to_string());
        let font = profiles::with(|profiles| {
            profiles
                .fonts
                .entry(keymap_profile.clone())
                .or_insert_with(|| FontPreferences::from(&settings))
                .clone()
        });
        let editor_settings = profiles::with(|profiles| {
            profiles
                .editor_settings
                .entry(keymap_profile.clone())
                .or_insert_with(|| settings.editor.clone())
                .clone()
        });
        let loaded_keymap = repository.load_keymap(&settings).unwrap_or_default();
        let (loaded_theme, theme_error) = match repository.load_theme() {
            Ok(theme) => (theme, None),
            Err(error) => (
                ThemeSettings::default(),
                Some(format!("Could not load theme.json: {error}")),
            ),
        };
        let theme = profiles::with(|profiles| {
            profiles
                .themes
                .entry(keymap_profile.clone())
                .or_insert(loaded_theme)
                .clone()
        });
        grid.set_theme(theme.clone());
        let keymap = profiles::with(|profiles| {
            profiles
                .keymaps
                .entry(keymap_profile.clone())
                .or_insert(loaded_keymap)
                .clone()
        });
        let grid_context = if matches!(
            session.working.as_ref().map(|definition| &definition.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        ) {
            KeyContext::QueryGrid
        } else {
            KeyContext::TableGrid
        };
        grid.set_keymap(keymap.clone(), grid_context);
        let computation = ComputationController::new(session.token());
        let selection_counts = grid.selection().map(|selection| {
            let (rows, columns) = selection.rectangle();
            (rows.count(), columns.count(), selection.cell_count())
        });
        let sources = crate::infrastructure::sources::Sources::for_profile(&repository.paths.config);
        let database = sources.database().clone();
        let initializing_database = database.clone();
        cx.spawn(
            move || {
                initializing_database.initialize()?;
                initializing_database.startup_error().map_or(Ok(()), Err)
            },
            |view: &mut Self, cx, result: Result<(), String>| {
                if let Err(error) = result {
                    view.error = Some(error);
                    cx.notify();
                }
            },
        )
        .detach();
        let history_writer = HistoryWriter::for_repository(&repository);
        let fonts = font.fonts();
        let grid_id = grid.id;
        let mut view = Self {
            sql_panel: None,
            sql_panel_open: false,
            sql_running: false,
            sql_revision: 0,
            auto_save_task: None,
            after_open: None,
            sql_service: std::sync::Arc::new(std::sync::Mutex::new(None)),
            sql_polars: std::sync::Arc::new(std::sync::Mutex::new(polars::sql::SQLContext::new())),
            sql_interrupt: None,
            sql_stopped: None,
            sql_restore_sort: false,
            sql_catalog_revision: 0,
            sql_catalog_refreshing: false,
            sql_file_schemas: Default::default(),
            sql_file_schema_task: None,
            sql_describing: false,
            sql_describe_again: false,
            sql_result_complete: false,
            sql_editor_height: settings.sql_editor_height.unwrap_or(280.),
            database_tree: DatabaseTree::new(),
            database_tree_open: settings.database_tree_open,
            database_tree_width: settings.database_tree_width.unwrap_or(280.),
            session,
            computation,
            grid,
            fitted_query_config: None,
            search_input,
            search_completion: ColumnCompletion::new(ColumnSyntax::Search),
            completion_columns_key: (0, 0),
            limit_input,
            query_limit,
            limit_error: None,
            repository,
            database,
            sources,
            file_settings: Default::default(),
            history_writer,
            table_writer,
            source_modified,
            error: theme_error,
            table_preferences_error: table_load_error,
            busy: false,
            debouncing: false,
            loading: false,
            copy_revision: 0,
            copy_status: None,
            statistics_revision: 0,
            statistics: None,
            statistics_error: None,
            statistics_pending: false,
            statistics_running: false,
            pending_statistics: None,
            selection_counts,
            information_revision: 0,
            information_busy: false,
            totals: None,
            totals_revision: 0,
            heat_ranges: None,
            heat_revision: 0,
            query_cache: None,
            table_query: None,
            navigation_backup: None,
            navigation_history: NavigationHistory::default(),
            undo_history: UndoHistory::default(),
            cell_detail: None,
            navigation_target: None,
            slicer_stage: None,
            slicer_sidebar: SlicerSidebar::default(),
            slicer_discovery: None,
            file_groups: std::array::from_fn(|_| Vec::new()),
            file_loading: false,
            file_revision: 0,
            space_menu_open: false,
            action_menu_open: false,
            cache_drop_open: false,
            keymap,
            theme,
            font,
            fonts,
            keymap_profile,
            keymap_editor: None,
            dialog: None,
            focus: Some(grid_id),
            window_geometry: None,
            geometry_save: None,
            source_monitor: None,
            source_check_pending: false,
            window_active: true,
            tree_check_pending: false,
            source_probe: None,
            source_refresh_error: None,
            profiles_seen: profiles::revision(),
            editor_settings,
            capture: None,
            split_drag: None,
            benchmark: false,
            frames: Default::default(),
        };
        view.sync_grid_formats();
        if view.database_tree_open {
            view.refresh_database_tree(cx);
        }
        view.start_source_monitor(cx);
        if view.session.is_sql() {
            view.sql_restore_sort = true;
            view.ensure_sql_panel(cx);
        } else {
            view.schedule_search(view.session.search_text.clone(), cx);
        }
        view.drain_grid_events(cx);
        view
    }

    fn start_source_monitor(&mut self, cx: &mut Cx<Self>) {
        self.source_monitor = Some(cx.timer(Duration::from_secs(1), |view, cx| {
            view.check_source(cx);
            if view.window_active {
                view.check_database_tree(cx);
            }
            view.start_source_monitor(cx);
        }));
    }

    pub(crate) fn check_source(&mut self, cx: &mut Cx<Self>) {
        use crate::infrastructure::source_database::LoadMode;
        use crate::infrastructure::sources::SourceLoad;
        if self.session.is_sql() {
            self.refresh_sql_catalog(cx);
            return;
        }
        if self.loading || self.navigation_backup.is_some() || self.source_check_pending {
            return;
        }
        if let Some(database_view) = self
            .session
            .working
            .as_ref()
            .and_then(|d| d.database_view.clone())
        {
            if let Some(path) = database_view.file_path() {
                // A file read in place reloads when it changes on disk.
                let revision = self.session.source.source_revision;
                let probe = self.sources.probe(std::path::Path::new(path));
                let changed = match (&self.source_probe, &probe) {
                    (Some((seen, seen_revision, last)), probe) if seen == path && *seen_revision == revision => {
                        !probe.as_ref().is_some_and(|probe| probe.same_file(last))
                    }
                    _ => false,
                };
                self.source_probe = probe.map(|probe| (path.to_owned(), revision, probe));
                if changed {
                    self.check_database_view(database_view, cx);
                }
                return;
            }
            if self
                .session
                .source
                .database_view_revision
                .is_some_and(|revision| revision != self.database.revision())
            {
                self.check_database_view(database_view, cx);
            }
            return;
        }
        let Some(identity) = self.session.source.source_identity.clone() else {
            return;
        };
        let Some(path) = self.session.source_path.clone() else {
            return;
        };
        let sources = self.sources.clone();
        let repository = self.repository.clone();
        let table_writer = self.table_writer.clone();
        let source_revision = self.session.source.source_revision;
        let last_probe = self
            .source_probe
            .clone()
            .filter(|(probe_path, revision, _)| probe_path == &path && *revision == source_revision)
            .map(|(_, _, probe)| probe);
        self.source_check_pending = true;
        let read_path = PathBuf::from(&path);
        cx.spawn(
            move || {
                // This runs every second, so skip reading the settings
                // while neither the file nor a read of it has changed.
                let probe = sources.probe(&read_path);
                if probe.is_some() && probe == last_probe {
                    return (probe, Ok(SourceLoad::Reused));
                }
                let options = crate::infrastructure::readers::read_options(&repository, &table_writer, &read_path);
                (probe, sources.load_reusing(&read_path, options, LoadMode::Check, Some(&identity)))
            },
            move |view: &mut Self, cx, (probe, result)| {
                view.source_check_pending = false;
                if view.loading
                    || view.session.source_path.as_ref() != Some(&path)
                    || view.session.source.source_revision != source_revision
                {
                    return;
                }
                view.source_probe = match &result {
                    Ok(SourceLoad::Reused) => probe.map(|probe| (path.clone(), source_revision, probe)),
                    _ => None,
                };
                match result {
                    Ok(source) => {
                        if let Some(error) = view.source_refresh_error.take()
                            && view.error.as_ref() == Some(&error)
                        {
                            view.error = None;
                            cx.notify();
                        }
                        if let SourceLoad::Loaded(store) = source {
                            view.replace_dataframe(FrameSnapshot::new(*store, source_revision + 1), cx);
                        }
                    }
                    Err(error) => {
                        let error = format!("Cannot refresh source: {error}");
                        if view.source_refresh_error.as_ref() != Some(&error) {
                            view.source_refresh_error = Some(error.clone());
                            view.error = Some(error);
                            cx.notify();
                        }
                    }
                }
            },
        )
        .detach();
    }

    pub fn replace_dataframe(&mut self, frame: FrameSnapshot, cx: &mut Cx<Self>) {
        self.replace_dataframe_with_reset(frame, false, cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn check_database_view(&mut self, database_view: crate::domain::documents::DatabaseView, cx: &mut Cx<Self>) {
        self.source_check_pending = true;
        let database = self.database.clone();
        let source_revision = self.session.source.source_revision;
        let read_view = database_view.clone();
        cx.spawn(
            move || database.load_view(&read_view),
            move |view: &mut Self, cx, result| {
                view.source_check_pending = false;
                if view.loading
                    || view.session.source.source_revision != source_revision
                    || view.session.working.as_ref().and_then(|d| d.database_view.as_ref()) != Some(&database_view)
                {
                    return;
                }
                match result {
                    Ok(store) => {
                        view.source_refresh_error = None;
                        view.replace_dataframe(FrameSnapshot::new(store, source_revision + 1), cx);
                    }
                    Err(error) => {
                        let error = format!("Cannot refresh database view: {error}");
                        view.source_refresh_error = Some(error.clone());
                        view.error = Some(error);
                        cx.notify();
                    }
                }
            },
        )
        .detach();
    }

    fn replace_dataframe_with_reset(&mut self, frame: FrameSnapshot, reset: bool, cx: &mut Cx<Self>) {
        self.session.reload_source(frame, reset);
        if reset {
            self.grid.reset_widths();
            self.autosave_table_preferences(cx);
        }
        self.source_modified = source_modified(self.session.source_path.as_deref());
        self.computation.invalidate(self.session.token());
        self.debouncing = false;
        self.busy = self.computation.is_running();
        self.information_revision += 1;
        self.information_busy = false;
        cx.notify();
    }

    pub fn displayed_result(&self) -> &ResultSnapshot {
        &self.session.result
    }

    pub fn selection(&self) -> Option<Selection> {
        self.grid.selection()
    }

    pub fn search_text(&self) -> &str {
        &self.session.search_text
    }

    pub fn set_search_text(&mut self, text: String, cx: &mut Cx<Self>) {
        // Keep the model and widget aligned before submitting the single request.
        self.session.search_text = text.clone();
        self.search_input.set_text(text.clone());
        self.schedule_search(text, cx);
    }

    pub(crate) fn suggested_name(&self) -> String {
        self.session
            .working
            .as_ref()
            .map(|view| view.name.clone())
            .or_else(|| {
                self.session
                    .source_path
                    .as_ref()
                    .and_then(|path| std::path::Path::new(path).file_stem())
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "Untitled".into())
    }

    pub(crate) fn sync_frozen_count(&mut self) {
        let count = match self.session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(_)) => {
                crate::domain::table::visible_frozen_count(&self.session.effective_table_config())
            }
            Some(ViewKind::Query(pivot)) => pivot.group_by.len(),
            Some(ViewKind::Sql(config)) => {
                crate::domain::table::frozen_prefix(&config.frozen_columns, &self.result_column_names())
            }
            None => 0,
        };
        self.grid.set_frozen_count(count);
    }

    fn width_owner(&self) -> String {
        let token = self.session.token();
        format!(
            "{}:{:?}:{}",
            token.session_generation,
            self.session.identity,
            self.is_table()
        )
    }

    pub(crate) fn sync_grid_widths(&mut self, cx: &mut Cx<Self>) {
        self.sync_grid_formats();
        let mut config = self.session.query_config();
        config.result_sort.clear();
        let autofit = self.navigation_backup.is_none()
            && self
                .fitted_query_config
                .as_ref()
                .is_some_and(|previous| previous != &config);
        self.fitted_query_config = Some(config);
        let owner = self.width_owner();
        let source = self.is_table().then(|| self.session.source.clone());
        let widths = if autofit {
            Default::default()
        } else {
            self.session.table_widths()
        };
        if autofit {
            self.grid.reset_widths();
        }
        self.grid.configure_widths(owner, source, widths, cx);
    }

    fn column_format_keys(&self) -> Vec<String> {
        let kind = if self.session.is_sql() {
            "sql"
        } else if self.is_table() {
            "table"
        } else {
            "query"
        };
        let source = self
            .session
            .working
            .as_ref()
            .and_then(|definition| definition.database_view.as_ref())
            .map(|view| {
                serde_json::to_string(&(view.schema.as_str(), view.name.as_str()))
                    .expect("database view names serialize")
            })
            .unwrap_or_else(|| self.session.source_path.clone().unwrap_or_default());
        let mut keys = Vec::new();
        if let Some(definition) = &self.session.working
            && !definition.id.is_empty()
        {
            keys.push(format!("view:{}", definition.id));
        }
        keys.push(format!("{kind}:{source}"));
        keys
    }

    pub(crate) fn sync_grid_formats(&mut self) {
        let keys = self.column_format_keys();
        let formats = profiles::with(|profiles| {
            let profile = profiles.column_formats.get(&self.keymap_profile);
            let mut formats = std::collections::BTreeMap::new();
            for key in keys.iter().rev() {
                if let Some(saved) = profile.and_then(|profile| profile.get(key)) {
                    formats.extend(saved.clone());
                }
            }
            formats
        });
        self.grid.configure_formats(&formats);
    }

    fn save_column_format(&mut self, column: &str, format: crate::domain::formatting::NumericFormat, cx: &mut Cx<Self>) {
        let key = self.column_format_keys().remove(0);
        let saved = self.repository.load_settings().and_then(|mut settings| {
            settings
                .column_formats
                .entry(key)
                .or_default()
                .insert(column.to_owned(), format);
            self.repository.save_settings(&settings)?;
            Ok(settings.column_formats)
        });
        match saved {
            Ok(formats) => {
                let profile = self.keymap_profile.clone();
                profiles::changed(|profiles| {
                    profiles.column_formats.insert(profile, formats);
                });
                cx.notify_all();
            }
            Err(error) => self.error = Some(format!("Could not save column formatting: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn autosave_table_preferences(&mut self, cx: &mut Cx<Self>) {
        let Some((source, patch)) = self.session.take_table_patch() else {
            return;
        };
        let receiver = match self.table_writer.edit(PathBuf::from(&source), patch) {
            Ok(receiver) => receiver,
            Err(error) => {
                self.table_preferences_error = Some(format!("Table preferences save failed: {error}"));
                cx.notify();
                return;
            }
        };
        cx.spawn(
            move || match receiver.recv() {
                Ok(result) => result,
                Err(_) => Err("Table preferences writer stopped".into()),
            },
            move |view: &mut Self, cx, result: Result<(), String>| {
                if view.session.is_direct_table() && view.session.direct_table.source_path == source {
                    match result {
                        Err(error) => view.table_preferences_error = Some(format!("Table preferences save failed: {error}")),
                        Ok(()) => view.table_preferences_error = None,
                    }
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// Handles what the grid reported since the last call.
    pub(crate) fn drain_grid_events(&mut self, cx: &mut Cx<Self>) {
        loop {
            let events = self.grid.take_events();
            if events.is_empty() {
                return;
            }
            for event in events {
                self.handle_grid_event(event, cx);
            }
        }
    }

    fn handle_grid_event(&mut self, event: GridEvent, cx: &mut Cx<Self>) {
        match event {
            GridEvent::NumericFormatChanged { column, format } => self.save_column_format(&column, format, cx),
            GridEvent::WidthChanged { owner, column, width } => {
                if owner == self.width_owner() {
                    self.session.set_column_width(column, width);
                    self.autosave_table_preferences(cx);
                    self.schedule_auto_save(cx);
                    cx.notify();
                }
            }
            GridEvent::WidthInferenceFailed { owner, error } => {
                if owner == self.width_owner() {
                    self.error = Some(format!("Width inference failed: {error}"));
                    cx.notify();
                }
            }
            GridEvent::SortDirectionRequested(descending) => self.sort_active(descending, cx),
            GridEvent::SortRequested(column) => {
                self.session.toggle_sort(column);
                self.grid.set_sort(self.session.sort.clone());
                self.schedule_search(self.session.search_text.clone(), cx);
            }
            GridEvent::ClearSortRequested(column) => {
                if self.session.clear_sort_column(&column) {
                    self.grid.set_sort(self.session.sort.clone());
                    self.schedule_search(self.session.search_text.clone(), cx);
                }
            }
            GridEvent::CopyRequested { result, selection } => self.schedule_copy(result, selection, cx),
            GridEvent::SelectionChanged { result, selection, by_user } => {
                // A status message is about what happened before; a new
                // selection's statistics replace it.
                if by_user {
                    self.copy_status = None;
                }
                self.schedule_statistics(result, selection, cx);
                self.refresh_cell_detail();
            }
            GridEvent::SelectionCleared => {
                self.refresh_cell_detail();
                self.statistics_revision += 1;
                self.selection_counts = None;
                self.statistics = None;
                self.statistics_error = None;
                self.statistics_pending = false;
                self.pending_statistics = None;
                cx.notify();
            }
            GridEvent::MoveColumn(direction) => self.change_table_column(Some(direction), cx),
            GridEvent::ToggleFrozen => self.change_table_column(None, cx),
        }
    }

    /// Catches up with settings another window of this profile changed.
    pub(crate) fn sync_profiles(&mut self, cx: &mut Cx<Self>) {
        let revision = profiles::revision();
        if revision == self.profiles_seen {
            return;
        }
        self.profiles_seen = revision;
        let profile = self.keymap_profile.clone();
        let (keymap, theme, font, limit, editor) = profiles::with(|profiles| {
            (
                profiles.keymaps.get(&profile).cloned(),
                profiles.themes.get(&profile).cloned(),
                profiles.fonts.get(&profile).cloned(),
                profiles.query_limits.get(&profile).copied(),
                profiles.editor_settings.get(&profile).cloned(),
            )
        });
        if let Some(editor) = editor
            && editor != self.editor_settings
        {
            self.set_editor_settings(editor);
        }
        if let Some(keymap) = keymap
            && keymap != self.keymap
        {
            self.keymap = keymap.clone();
            let context = if self.is_table() {
                KeyContext::TableGrid
            } else {
                KeyContext::QueryGrid
            };
            self.grid.set_keymap(keymap, context);
            if let Some(editor) = &mut self.keymap_editor {
                editor.keymap = self.keymap.clone();
            }
        }
        if let Some(theme) = theme
            && theme != self.theme
        {
            self.theme = theme.clone();
            self.grid.set_theme(theme);
        }
        if let Some(font) = font
            && font != self.font
        {
            self.fonts = font.fonts();
            self.font = font;
        }
        if let Some(limit) = limit
            && limit != self.query_limit
        {
            self.query_limit = limit;
            self.limit_input.set_text(limit.to_string());
            self.limit_error = None;
            self.query_cache = None;
            if !self.is_table() && !self.session.is_sql() {
                self.schedule_search(self.session.search_text.clone(), cx);
            }
        }
        self.sync_grid_formats();
        cx.notify();
    }

    pub(crate) fn is_table(&self) -> bool {
        !matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        )
    }

    pub(crate) fn result_column_names(&self) -> Vec<String> {
        self.session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect()
    }

    /// Moves focus to `id`, letting the control that had it react.
    pub(crate) fn set_focus(&mut self, id: Option<WidgetId>, cx: &mut Cx<Self>) {
        let before = self.focus;
        self.focus = id;
        self.focus_changed(before, cx);
    }

    pub(crate) fn focus_grid(&mut self, cx: &mut Cx<Self>) {
        let id = self.grid.id;
        self.set_focus(Some(id), cx);
    }

    /// Focus moved from `before`: inputs that commit on blur do so.
    pub(crate) fn focus_changed(&mut self, before: Option<WidgetId>, cx: &mut Cx<Self>) {
        if before == self.focus {
            return;
        }
        let Some(before) = before else {
            return;
        };
        if before == self.limit_input.id {
            self.commit_query_limit(cx);
        }
        if before == self.search_input.id {
            self.search_completion.close();
        }
        if self.slicer_sidebar.contains(before) {
            self.slicer_sidebar.blur(before);
            self.drain_slicer_events(cx);
        }
        cx.notify();
    }
}
