use gpui_kit::component::ActiveTheme;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Root;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::input::{
    IndentInline, Input, InputEvent, InputState, MoveDown, OutdentInline,
};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::application::commands::{COMMANDS, Command as AppCommand, CommandId};
use crate::application::computation::{ComputationController, ScheduleReason};
use crate::application::history::HistoryWriter;
use crate::application::keymap::{
    ACTIONS as KEYMAP_ACTIONS, ActionId, KeyContext, KeymapFile, KeymapRuntime, canonical_keystroke,
};
use crate::application::navigation_history::{NavigationEntry, NavigationHistory};
use crate::application::pivot_editor::{self, AggregationEdit, PivotFieldKind};
use crate::application::session::{JobToken, SessionIdentity, WindowSession};
use crate::application::slicers::{SlicerDiscovery, SlicerStage};
use crate::application::state::ApplicationState;
use crate::application::table_preferences::TablePreferencesWriter;
use crate::application::theme::ThemeSettings;
use crate::application::undo::UndoHistory;
use crate::domain::actions;
use crate::domain::documents::{
    Aggregation, Dimension, DocumentDefinition, QueryConfig, SlicerDefinition, SortKey,
    ValueFilter, ViewAction, ViewKind,
};
use crate::domain::export::{self, CopyFormat};
use crate::domain::frame::{FrameSnapshot, ResultSnapshot};
use crate::domain::query;
use crate::domain::query::Token;
use crate::domain::selection::Selection;
use crate::domain::sql::SqlTrace;
use crate::domain::statistics::{self, SelectionStatistics};
use crate::domain::table;
use crate::infrastructure::catalog::scan_sources;
use crate::infrastructure::duckdb_store::{ColumnSchema, ViewQuery};
use crate::infrastructure::json_store::{
    FavoriteTarget, StorePaths, ViewRepository, WindowGeometry,
};
use crate::infrastructure::launcher;
use crate::infrastructure::readers::read_source;
use crate::infrastructure::source_database::{LoadMode, SourceDatabase, SourceLoad, SourceProbe};
use crate::ui::actions_editor::{ActionsEditor, ActionsEditorEvent};
use crate::ui::ai_settings::{AiSettingsEditor, AiSettingsEvent};
use crate::ui::aggregation_editor::{AggregationPanel, AggregationPanelEvent};
use crate::ui::cell_detail::{CellContent, CellDetailEvent, CellDetailPane};
use crate::ui::command_palette::{PALETTE_WIDTH, matches_terms, matching_commands};
use crate::ui::font_picker::{FontPreferences, FontRuntime, FontTarget};
use crate::ui::grid::{GridEvent, GridView};
use crate::ui::info::InformationPanel;
use crate::ui::keymap_editor::{KeymapEditor, KeymapEditorEvent, update_query};
use crate::ui::order_by_panel::{OrderByEdit, OrderByPanel};
use crate::ui::pivot_panel::{PivotPanel, PivotPanelEvent};
use crate::ui::query_limit::QueryLimitRuntime;
use crate::ui::slicer_sidebar::{
    SlicerChoice, SlicerEvent, SlicerKind, SlicerPicker, SlicerSidebar,
};
use crate::ui::space_menu::{ActionMenu, SpaceMenu};
use crate::ui::theme_editor::{ThemeEditor, ThemeRuntime};
use crate::ui::{status, window_placement};

gpui_kit::actions!(datadiver, [ArchiveSelectedView]);

#[cfg(all(test, feature = "snapshots"))]
mod action_tests;
mod database_tree;
#[cfg(all(test, feature = "snapshots"))]
mod filter_tests;
#[cfg(all(test, feature = "snapshots"))]
mod focus_tests;
#[cfg(all(test, feature = "snapshots"))]
mod navigation_tests;
mod performance;
mod search_completion;
mod sql_editor;
#[cfg(all(test, feature = "snapshots"))]
mod sql_tests;

pub struct DataFrameView {
    sql_panel: Option<Entity<crate::ui::sql_panel::SqlPanel>>,
    sql_panel_open: bool,
    sql_running: bool,
    sql_revision: u64,
    sql_auto_save_task: Option<Task<()>>,
    sql_service: std::sync::Arc<
        std::sync::Mutex<Option<crate::infrastructure::sql_execution::SqlExecution>>,
    >,
    sql_interrupt: Option<std::sync::Arc<duckdb::InterruptHandle>>,
    sql_stopped: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    sql_subscription: Option<Subscription>,
    sql_restore_sort: bool,
    sql_catalog_revision: u64,
    sql_catalog_refreshing: bool,
    /// The SQL result holds every row of its query, so it can be filtered.
    sql_result_complete: bool,
    sql_split: Entity<gpui_kit::component::resizable::ResizableState>,
    sql_editor_height: f32,
    database_tree: Entity<crate::ui::database_tree::DatabaseTree>,
    database_tree_open: bool,
    database_tree_split: Entity<gpui_kit::component::resizable::ResizableState>,
    database_tree_width: f32,
    session: WindowSession,
    computation: ComputationController<PipelineRequest>,
    grid: Entity<GridView>,
    fitted_query_config: Option<QueryConfig>,
    search_input: Entity<InputState>,
    search_completion: Entity<crate::ui::column_completion::ColumnCompletion>,
    completion_columns_key: (usize, usize),
    limit_input: Entity<InputState>,
    query_limit: u64,
    limit_error: Option<String>,
    sync_limit_input: bool,
    repository: ViewRepository,
    database: std::sync::Arc<SourceDatabase>,
    history_writer: std::sync::Arc<HistoryWriter>,
    table_writer: std::sync::Arc<TablePreferencesWriter>,
    source_modified: Option<String>,
    error: Option<String>,
    table_preferences_error: Option<String>,
    busy: bool,
    debouncing: bool,
    loading: bool,
    copy_revision: u64,
    copy_status: Option<String>,
    statistics_revision: u64,
    statistics: Option<SelectionStatistics>,
    statistics_error: Option<String>,
    statistics_pending: bool,
    statistics_running: bool,
    pending_statistics: Option<PendingStatistics>,
    selection_counts: Option<(usize, usize, usize)>,
    information_revision: u64,
    information_busy: bool,
    /// The open Information dialog's panel, if any.
    information_panel: Option<WeakEntity<InformationPanel>>,
    /// The grand total row of the last query with totals, and what it covers.
    totals: Option<(TotalsKey, crate::domain::pivot::TotalsRow)>,
    totals_revision: u64,
    /// Heat-map ranges and the query result they were measured on.
    heat_ranges: Option<(StoreHandle, HeatRanges)>,
    heat_revision: u64,
    query_cache: Option<QueryResultCache>,
    table_query: Option<(ViewQuery, SqlTrace)>,
    navigation_backup: Option<NavigationBackup>,
    navigation_history: NavigationHistory,
    undo_history: UndoHistory,
    navigation_target: Option<(usize, NavigationEntry)>,
    slicer_stage: Option<SlicerStage>,
    slicer_sidebar: Entity<SlicerSidebar>,
    /// The cell detail pane while it is open, with its subscription.
    cell_detail: Option<(Entity<CellDetailPane>, Subscription)>,
    slicer_picker: Option<Entity<SlicerPicker>>,
    /// Every column's values, wanted while the Add Slicer picker is open.
    slicer_discovery: Option<std::sync::Arc<SlicerDiscovery>>,
    file_groups: [Vec<FileEntry>; 5],
    file_loading: bool,
    file_revision: u64,
    space_menu_open: bool,
    action_menu_open: bool,
    keymap: KeymapFile,
    theme: ThemeSettings,
    font: FontPreferences,
    keymap_profile: PathBuf,
    keymap_editor: Option<Entity<KeymapEditor>>,
    aggregation_panel: Option<WeakEntity<AggregationPanel>>,
    dimension_panel: Option<WeakEntity<PivotPanel>>,
    restore_focus: Option<FocusHandle>,
    window_geometry: Option<WindowGeometry>,
    geometry_save: Option<Task<()>>,
    source_monitor: Option<Task<()>>,
    source_check_pending: bool,
    /// The probe from the last check that reused the source, keyed by path and
    /// source revision; an unchanged probe skips the DuckDB catalog queries.
    source_probe: Option<(String, u64, SourceProbe)>,
    source_refresh_error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

/// Identifies a query result without keeping it alive.
type StoreHandle =
    std::sync::Weak<std::sync::Mutex<crate::infrastructure::duckdb_store::DuckDbStore>>;
type HeatRanges = std::collections::HashMap<String, (f64, f64)>;

type ColumnFormats = std::collections::BTreeMap<
    String,
    std::collections::BTreeMap<String, crate::domain::formatting::NumericFormat>,
>;

#[derive(Default)]
struct ColumnFormatRuntime {
    profiles: std::collections::BTreeMap<PathBuf, ColumnFormats>,
}

impl Global for ColumnFormatRuntime {}

const COPY_FORMATS: &[(CopyFormat, &str)] = &[
    (CopyFormat::RawTsv, "Raw TSV"),
    (CopyFormat::FormattedTsv, "Formatted TSV"),
    (CopyFormat::RawTsvHeaders, "Raw TSV with headers"),
    (
        CopyFormat::FormattedTsvHeaders,
        "Formatted TSV with headers",
    ),
    (CopyFormat::RawCsv, "Raw CSV"),
    (CopyFormat::JsonRecords, "JSON records"),
    (CopyFormat::JsonColumns, "JSON columns"),
    (CopyFormat::PythonList, "Python list"),
];

#[derive(Clone)]
enum FileEntry {
    View(DocumentDefinition),
    Source(PathBuf),
    StartupSql,
}

/// Filters applied to a source table once a drill-down has opened it.
#[derive(Clone)]
struct DrillDown {
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
}

enum SourceTarget {
    File(PathBuf),
    DatabaseView(crate::domain::documents::DatabaseView),
}

impl FileEntry {
    fn is_saved_view(&self) -> bool {
        matches!(self, Self::View(view) if !view.id.is_empty())
    }
    fn label(&self) -> String {
        match self {
            Self::View(view) => view.name.clone(),
            Self::Source(path) => source_file_name(path),
            Self::StartupSql => "startup.sql".into(),
        }
    }

    fn source_parent(&self) -> Option<String> {
        match self {
            Self::Source(path) => Some(source_parent_folder(path)),
            Self::View(_) | Self::StartupSql => None,
        }
    }

    fn search_text(&self) -> String {
        match self {
            Self::View(view) => format!(
                "{} {} {}",
                view.name,
                view.source_path,
                if matches!(view.view, ViewKind::Table(_)) {
                    "table view"
                } else {
                    "query view"
                }
            ),
            Self::Source(path) => format!("{} source", path.display()),
            Self::StartupSql => "startup.sql startup SQL script settings".into(),
        }
    }
}

fn source_file_name(path: &std::path::Path) -> String {
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

fn short_source(path: &std::path::Path) -> String {
    let name = source_file_name(path);
    let parent = source_parent_folder(path);
    if parent == "Other" {
        name
    } else {
        format!("{name} · {parent}")
    }
}

fn file_command_item(entry: &FileEntry) -> CommandItem {
    CommandItem::new().label(entry.label())
}

fn local_archive_time(unix_ms: u128) -> String {
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
fn query_limit_reached(result_rows: usize, query_limit: u64) -> bool {
    result_rows as u64 >= query_limit
}

fn source_modified(path: Option<&str>) -> Option<String> {
    let path = path?;
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let local: chrono::DateTime<chrono::Local> = modified.into();
    Some(local.format("%Y-%m-%d %I:%M %p").to_string())
}

const ACTIVE_COMMANDS: &[CommandId] = &[
    CommandId::Back,
    CommandId::Forward,
    CommandId::SqlEditor,
    CommandId::StartupSql,
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

fn active_commands() -> Vec<&'static AppCommand> {
    COMMANDS
        .iter()
        .filter(|command| ACTIVE_COMMANDS.contains(&command.id))
        .collect()
}

fn available_sort_keys(sort: &[SortKey], schema: &[ColumnSchema]) -> Vec<SortKey> {
    sort.iter()
        .filter(|key| schema.iter().any(|column| column.name == key.column))
        .cloned()
        .collect()
}

fn key_with_modifiers(event: &KeyDownEvent) -> String {
    let mut parts = Vec::new();
    if event.keystroke.modifiers.control {
        parts.push("ctrl".to_owned());
    }
    if event.keystroke.modifiers.alt {
        parts.push("alt".to_owned());
    }
    if event.keystroke.modifiers.shift {
        parts.push("shift".to_owned());
    }
    if event.keystroke.modifiers.platform {
        parts.push("cmd".to_owned());
    }
    parts.push(if event.keystroke.key == " " {
        "space".to_owned()
    } else {
        event.keystroke.key.to_ascii_lowercase()
    });
    parts.join("-")
}

fn favorite_shortcut(
    keystroke: &Keystroke,
    map_key: impl Fn(Keystroke) -> KeybindingKeystroke,
) -> Option<(usize, bool)> {
    let modifiers = keystroke.modifiers;
    if !modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return None;
    }
    for slot in 0..9 {
        for assign in [false, true] {
            let target = Keystroke {
                key: (slot + 1).to_string(),
                modifiers: Modifiers {
                    control: true,
                    shift: assign,
                    ..Default::default()
                },
                ..Default::default()
            };
            if keystroke.key == target.key && keystroke.modifiers == target.modifiers {
                return Some((slot, assign));
            }
            // Windows folds Shift into the digit's produced character, such as Ctrl+!.
            if keystroke.should_match(&map_key(target)) {
                return Some((slot, assign));
            }
        }
    }
    None
}

#[cfg(test)]
mod favorite_shortcut_tests {
    use super::favorite_shortcut;
    use gpui_kit::{KeybindingKeystroke, Keystroke};

    #[test]
    fn maps_control_digit_shortcuts_to_favorite_slots() {
        for digit in 1..=9 {
            for (prefix, assign) in [("ctrl", false), ("ctrl-shift", true)] {
                assert_eq!(
                    favorite_shortcut(
                        &Keystroke::parse(&format!("{prefix}-{digit}")).unwrap(),
                        KeybindingKeystroke::from_keystroke
                    ),
                    Some((digit - 1, assign))
                );
            }
        }
        for stroke in ["ctrl-0", "ctrl-alt-1", "shift-1", "ctrl-cmd-1", "ctrl-fn-1"] {
            assert_eq!(
                favorite_shortcut(
                    &Keystroke::parse(stroke).unwrap(),
                    KeybindingKeystroke::from_keystroke
                ),
                None
            );
        }
    }

    #[test]
    fn maps_windows_shifted_digits_through_the_keyboard_layout() {
        for (slot, symbol) in ["!", "@", "#", "$", "%", "^", "&", "*", "("]
            .iter()
            .enumerate()
        {
            let event = Keystroke::parse(&format!("ctrl-{symbol}")).unwrap();
            assert_eq!(
                favorite_shortcut(&event, |mut target| {
                    if target.modifiers.shift {
                        target.key = ["!", "@", "#", "$", "%", "^", "&", "*", "("]
                            [target.key.parse::<usize>().unwrap() - 1]
                            .into();
                        target.modifiers.shift = false;
                    }
                    KeybindingKeystroke::from_keystroke(target)
                }),
                Some((slot, true))
            );
        }
    }
}

struct PipelineRequest {
    query_candidate: Option<QueryCandidate>,
    pending_setup: Option<ResultSnapshot>,
    query_limit: u64,
    source: FrameSnapshot,
    tokens: Vec<Token>,
    sort: Vec<SortKey>,
    definition: Option<DocumentDefinition>,
    query_cache: Option<(FrameSnapshot, SqlTrace)>,
    slicer_discovery: Option<std::sync::Arc<SlicerDiscovery>>,
    queued_at: Instant,
    reuse_table: Option<(ViewQuery, ResultSnapshot)>,
    table_trace: SqlTrace,
}

enum CandidateEditor {
    Aggregation {
        panel: WeakEntity<AggregationPanel>,
        preferred: Option<Aggregation>,
    },
    Dimension {
        panel: WeakEntity<PivotPanel>,
        kind: PivotFieldKind,
        preferred: Option<Dimension>,
    },
}

struct QueryCandidate {
    editor: CandidateEditor,
    window: AnyWindowHandle,
    revision: u64,
    config: QueryConfig,
}

impl QueryCandidate {
    fn is_current(&self, view: &DataFrameView, cx: &App) -> bool {
        match &self.editor {
            CandidateEditor::Aggregation { panel, .. } => {
                panel
                    .upgrade()
                    .is_some_and(|panel| panel.read(cx).can_finish(self.revision, cx))
                    && view
                        .aggregation_panel
                        .as_ref()
                        .is_some_and(|current| current.entity_id() == panel.entity_id())
            }
            CandidateEditor::Dimension { panel, .. } => {
                panel
                    .upgrade()
                    .is_some_and(|panel| panel.read(cx).can_finish(self.revision, cx))
                    && view
                        .dimension_panel
                        .as_ref()
                        .is_some_and(|current| current.entity_id() == panel.entity_id())
            }
        }
    }
    fn finish_error(&self, error: String, cx: &mut App) {
        match &self.editor {
            CandidateEditor::Aggregation { panel, .. } => {
                if let Some(panel) = panel.upgrade() {
                    panel.update(cx, |panel, cx| panel.finish_error(self.revision, error, cx));
                }
            }
            CandidateEditor::Dimension { panel, .. } => {
                if let Some(panel) = panel.upgrade() {
                    panel.update(cx, |panel, cx| panel.finish_error(self.revision, error, cx));
                }
            }
        }
    }
    fn finish_success(&self, window: &mut Window, cx: &mut App) {
        match &self.editor {
            CandidateEditor::Aggregation { panel, preferred } => {
                if let Some(panel) = panel.upgrade() {
                    panel.update(cx, |panel, cx| {
                        panel.finish_success(
                            self.config.aggregations.clone(),
                            preferred.clone(),
                            window,
                            cx,
                        )
                    });
                }
            }
            CandidateEditor::Dimension {
                panel,
                kind,
                preferred,
            } => {
                if let Some(panel) = panel.upgrade() {
                    let entries = match kind {
                        PivotFieldKind::GroupBy => self.config.group_by.clone(),
                        PivotFieldKind::PivotBy => self.config.pivot_by.clone(),
                    };
                    panel.update(cx, |panel, cx| {
                        panel.finish_success(entries, preferred.clone(), window, cx)
                    });
                }
            }
        }
    }
}

struct NavigationBackup {
    session: WindowSession,
    source_modified: Option<String>,
    table_preferences_error: Option<String>,
    slicer_stage: Option<SlicerStage>,
    query_cache: Option<QueryResultCache>,
    table_query: Option<(ViewQuery, SqlTrace)>,
    window_handle: AnyWindowHandle,
}

struct PendingStatistics {
    result: ResultSnapshot,
    selection: Selection,
    revision: u64,
    generation: u64,
    result_revision: u64,
    deadline: Instant,
}

/// Everything a grand total row depends on; search and sort do not change it.
#[derive(Clone, PartialEq)]
struct TotalsKey {
    source_revision: u64,
    store: usize,
    config: QueryConfig,
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
}

/// A query to rerun without the query limit, with the result's search and sort.
struct UnlimitedQuery {
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
struct QueryResultCache {
    query_limit: u64,
    source: FrameSnapshot,
    config: QueryConfig,
    slicers: Vec<SlicerDefinition>,
    filters: Vec<ValueFilter>,
    frame: FrameSnapshot,
    trace: SqlTrace,
}

impl QueryResultCache {
    fn matches(
        &self,
        source: &FrameSnapshot,
        definition: &DocumentDefinition,
        query_limit: u64,
    ) -> bool {
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

pub struct ViewerOptions {
    pub benchmark_ui: bool,
    pub width: f32,
    pub height: f32,
    pub restore_window: bool,
    pub capture: Option<PathBuf>,
    pub capture_delay: Duration,
    pub exit_after: Option<Duration>,
    pub search_text: String,
    pub slicers: Vec<String>,
    pub load_path: Option<PathBuf>,
    pub load_view: Option<DocumentDefinition>,
    pub repository: ViewRepository,
    pub show: Vec<String>,
    /// A location for a new window to open, such as a drill-down.
    open_entry: Option<(FileEntry, Option<DrillDown>)>,
}

impl Default for ViewerOptions {
    fn default() -> Self {
        Self {
            benchmark_ui: false,
            width: 1400.0,
            height: 900.0,
            restore_window: true,
            capture: None,
            capture_delay: Duration::from_millis(300),
            exit_after: None,
            search_text: String::new(),
            slicers: Vec::new(),
            load_path: None,
            load_view: None,
            repository: ViewRepository::new(StorePaths::operating_system()),
            show: Vec::new(),
            open_entry: None,
        }
    }
}

impl DataFrameView {
    pub fn new(
        frame: FrameSnapshot,
        source_path: Option<String>,
        initial_search: String,
        load_view: Option<DocumentDefinition>,
        repository: ViewRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
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
                Err(error) => {
                    table_load_error = Some(format!("Cannot read table preferences: {error}"))
                }
            }
        }
        if !initial_search.is_empty() {
            session.set_search_text(initial_search.clone());
        }
        let grid = cx.new(|cx| GridView::new(session.result.clone(), cx));
        let initial_frozen = match session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Table(_)) => {
                table::visible_frozen_count(&session.effective_table_config())
            }
            Some(ViewKind::Query(pivot)) => pivot.group_by.len(),
            // A SQL result arrives after it runs; publishing it sets the count.
            Some(ViewKind::Sql(_)) => 0,
            None => 0,
        };
        grid.update(cx, |grid, cx| grid.set_frozen_count(initial_frozen, cx));
        if !session.sort.is_empty() {
            let sort = session.sort.clone();
            grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
        }
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Search rows"));
        if !initial_search.is_empty() {
            search_input.update(cx, |input, cx| input.set_value(initial_search, window, cx));
        }
        let subscription =
            cx.subscribe(
                &search_input,
                |this, input, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        if text != this.session.search_text {
                            this.schedule_computation(text, ScheduleReason::TextEdit, cx);
                        }
                    }
                    _ => {}
                },
            );
        let (search_completion, search_completion_subscription) =
            Self::new_search_completion(&search_input, cx);
        let grid_subscription = cx.subscribe(&grid, |this, _, event: &GridEvent, cx| match event {
            GridEvent::NumericFormatChanged { column, format } => {
                this.save_column_format(column, *format, cx)
            }
            GridEvent::WidthChanged {
                owner,
                column,
                width,
            } => {
                if *owner == this.width_owner() {
                    this.session.set_column_width(column.clone(), *width);
                    this.autosave_table_preferences(cx);
                    cx.notify();
                }
            }
            GridEvent::WidthInferenceFailed { owner, error } => {
                if *owner == this.width_owner() {
                    this.error = Some(format!("Width inference failed: {error}"));
                    cx.notify();
                }
            }
            GridEvent::SortDirectionRequested(descending) => this.sort_active(*descending, cx),
            GridEvent::SortRequested(column) => {
                this.session.toggle_sort(column.clone());
                let sort = this.session.sort.clone();
                this.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
                this.schedule_search(this.session.search_text.clone(), cx);
            }
            GridEvent::ClearSortRequested(column) => {
                if this.session.clear_sort_column(column) {
                    let sort = this.session.sort.clone();
                    this.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
                    this.schedule_search(this.session.search_text.clone(), cx);
                }
            }
            GridEvent::CopyRequested { result, selection } => {
                this.schedule_copy(result.clone(), *selection, cx)
            }
            GridEvent::SelectionChanged { result, selection } => {
                this.schedule_statistics(result.clone(), *selection, cx);
                this.refresh_cell_detail(cx);
            }
            GridEvent::SelectionCleared => {
                this.refresh_cell_detail(cx);
                this.statistics_revision += 1;
                this.selection_counts = None;
                this.statistics = None;
                this.statistics_error = None;
                this.statistics_pending = false;
                this.pending_statistics = None;
                cx.notify();
            }
            GridEvent::MoveColumn(direction) => this.change_table_column(Some(*direction), cx),
            GridEvent::ToggleFrozen => this.change_table_column(None, cx),
        });
        let keymap_profile = repository.paths.config.clone();
        let settings = repository.load_settings().unwrap_or_default();
        if !cx.has_global::<ColumnFormatRuntime>() {
            cx.set_global(ColumnFormatRuntime::default());
        }
        cx.global_mut::<ColumnFormatRuntime>()
            .profiles
            .entry(keymap_profile.clone())
            .or_insert_with(|| settings.column_formats.clone());
        let format_profile = keymap_profile.clone();
        let format_observer = cx.observe_global::<ColumnFormatRuntime>(move |view, cx| {
            if view.keymap_profile == format_profile {
                view.sync_grid_formats(cx);
            }
        });
        if !cx.has_global::<QueryLimitRuntime>() {
            cx.set_global(QueryLimitRuntime::default());
        }
        let query_limit = *cx
            .global_mut::<QueryLimitRuntime>()
            .profiles
            .entry(keymap_profile.clone())
            .or_insert_with(|| {
                if settings.query_limit > 0 && settings.query_limit <= i64::MAX as u64 {
                    settings.query_limit
                } else {
                    crate::infrastructure::json_store::DEFAULT_QUERY_LIMIT
                }
            });
        let limit_input = cx.new(|cx| InputState::new(window, cx).placeholder("Limit"));
        limit_input.update(cx, |input, cx| {
            input.set_value(query_limit.to_string(), window, cx)
        });
        let limit_subscription = cx.subscribe_in(
            &limit_input,
            window,
            |view, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    view.commit_query_limit(window, cx)
                }
                InputEvent::Change => {
                    cx.notify();
                }
                _ => {}
            },
        );
        let limit_profile = keymap_profile.clone();
        let limit_observer = cx.observe_global::<QueryLimitRuntime>(move |view, cx| {
            if let Some(limit) = cx
                .global::<QueryLimitRuntime>()
                .profiles
                .get(&limit_profile)
                .copied()
                && limit != view.query_limit
            {
                view.query_limit = limit;
                view.sync_limit_input = true;
                view.limit_error = None;
                view.query_cache = None;
                if !view.is_table() && !view.session.is_sql() {
                    view.schedule_search(view.session.search_text.clone(), cx);
                }
                cx.notify();
            }
        });

        if !cx.has_global::<FontRuntime>() {
            cx.set_global(FontRuntime::default());
        }
        let font = cx
            .global_mut::<FontRuntime>()
            .profiles
            .entry(keymap_profile.clone())
            .or_insert_with(|| FontPreferences::from(&settings))
            .clone();
        crate::ui::font_picker::apply_editor_font(&font, cx);
        let font_profile = keymap_profile.clone();
        let font_observer = cx.observe_global::<FontRuntime>(move |view, cx| {
            if let Some(font) = cx.global::<FontRuntime>().profiles.get(&font_profile)
                && *font != view.font
            {
                view.font = font.clone();
                crate::ui::font_picker::apply_editor_font(&view.font, cx);
                cx.notify();
            }
        });
        let loaded_keymap = repository.load_keymap(&settings).unwrap_or_default();
        let (loaded_theme, theme_error) = match repository.load_theme() {
            Ok(theme) => (theme, None),
            Err(error) => (
                ThemeSettings::default(),
                Some(format!("Could not load theme.json: {error}")),
            ),
        };
        if !cx.has_global::<ThemeRuntime>() {
            cx.set_global(ThemeRuntime::default());
        }
        let theme = cx
            .global_mut::<ThemeRuntime>()
            .profiles
            .entry(keymap_profile.clone())
            .or_insert(loaded_theme)
            .clone();
        grid.update(cx, |grid, cx| grid.set_theme(theme.clone(), cx));
        let theme_profile = keymap_profile.clone();
        let theme_observer = cx.observe_global::<ThemeRuntime>(move |view, cx| {
            if let Some(theme) = cx.global::<ThemeRuntime>().profiles.get(&theme_profile)
                && *theme != view.theme
            {
                view.theme = theme.clone();
                let theme = view.theme.clone();
                view.grid.update(cx, |grid, cx| grid.set_theme(theme, cx));
                cx.notify();
            }
        });
        let keymap = if cx.has_global::<KeymapRuntime>() {
            cx.global_mut::<KeymapRuntime>()
                .profiles
                .entry(keymap_profile.clone())
                .or_insert(loaded_keymap)
                .clone()
        } else {
            cx.set_global(KeymapRuntime {
                profiles: [(keymap_profile.clone(), loaded_keymap.clone())].into(),
            });
            loaded_keymap
        };
        let grid_context = if matches!(
            session.working.as_ref().map(|definition| &definition.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        ) {
            KeyContext::QueryGrid
        } else {
            KeyContext::TableGrid
        };
        grid.update(cx, |grid, _| grid.set_keymap(keymap.clone(), grid_context));
        let observed_profile = keymap_profile.clone();
        let keymap_observer = cx.observe_global::<KeymapRuntime>(move |view, cx| {
            if let Some(keymap) = cx.global::<KeymapRuntime>().profiles.get(&observed_profile) {
                view.keymap = keymap.clone();
                let keymap = view.keymap.clone();
                let context = if view.is_table() {
                    KeyContext::TableGrid
                } else {
                    KeyContext::QueryGrid
                };
                view.grid
                    .update(cx, |grid, _| grid.set_keymap(keymap, context));
                if let Some(editor) = view.keymap_editor.as_ref() {
                    editor.update(cx, |editor, cx| {
                        editor.keymap = view.keymap.clone();
                        cx.notify();
                    });
                }
                cx.notify();
            }
        });
        let computation = ComputationController::new(session.token());
        let slicer_sidebar = cx.new(|_| SlicerSidebar::default());
        let slicer_subscription =
            cx.subscribe(&slicer_sidebar, |view, _, event: &SlicerEvent, cx| {
                let changed = view
                    .session
                    .working
                    .as_ref()
                    .is_some_and(|definition| match event {
                        SlicerEvent::Select { column, selected } => definition
                            .slicers
                            .iter()
                            .any(|card| &card.column == column && &card.selected != selected),
                        SlicerEvent::Clear(column) => definition
                            .slicers
                            .iter()
                            .any(|card| &card.column == column && card.is_active()),
                        SlicerEvent::Condition { column, text } => {
                            definition.slicers.iter().any(|card| {
                                &card.column == column && card.condition.as_ref() != Some(text)
                            })
                        }
                        SlicerEvent::Remove(column) => {
                            definition.slicers.iter().any(|card| &card.column == column)
                        }
                    });
                if !changed {
                    return;
                }
                let _ = view.session.edit_definition(|definition| match event {
                    SlicerEvent::Select { column, selected } => {
                        if let Some(card) = definition
                            .slicers
                            .iter_mut()
                            .find(|card| &card.column == column)
                        {
                            card.selected = selected.clone();
                        }
                    }
                    SlicerEvent::Clear(column) => {
                        if let Some(card) = definition
                            .slicers
                            .iter_mut()
                            .find(|card| &card.column == column)
                        {
                            card.clear();
                        }
                    }
                    SlicerEvent::Remove(column) => {
                        definition.slicers.retain(|card| &card.column != column)
                    }
                    SlicerEvent::Condition { column, text } => {
                        if let Some(card) = definition
                            .slicers
                            .iter_mut()
                            .find(|card| &card.column == column)
                        {
                            card.condition = Some(text.clone());
                        }
                    }
                });
                view.autosave_table_preferences(cx);
                view.schedule_search(view.session.search_text.clone(), cx);
            });
        let selection_counts = grid.read(cx).selection().map(|selection| {
            let (rows, columns) = selection.rectangle();
            let row_count = rows.count();
            let column_count = columns.count();
            (row_count, column_count, selection.cell_count())
        });
        let database = SourceDatabase::for_profile(&repository.paths.config);
        let initializing_database = database.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    initializing_database.initialize()?;
                    initializing_database.startup_error().map_or(Ok(()), Err)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |view, cx| {
                    view.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
        let sql_split = cx.new(|_| gpui_kit::component::resizable::ResizableState::default());
        let sql_split_subscription = cx.subscribe(
            &sql_split,
            |view, split, _: &gpui_kit::component::resizable::ResizablePanelEvent, cx| {
                if let Some(height) = split.read(cx).sizes().get(1) {
                    view.save_sql_editor_height(height.as_f32());
                }
            },
        );
        let (database_tree, database_tree_subscription) = Self::new_database_tree(window, cx);
        let database_tree_split =
            cx.new(|_| gpui_kit::component::resizable::ResizableState::default());
        let database_tree_split_subscription = cx.subscribe(
            &database_tree_split,
            |view, split, _: &gpui_kit::component::resizable::ResizablePanelEvent, cx| {
                if let Some(width) = split.read(cx).sizes().first() {
                    view.save_database_tree_width(width.as_f32());
                }
            },
        );
        let history_writer = HistoryWriter::for_repository(&repository);
        let mut view = Self {
            sql_panel: None,
            sql_panel_open: false,
            sql_running: false,
            sql_revision: 0,
            sql_auto_save_task: None,
            sql_service: std::sync::Arc::new(std::sync::Mutex::new(None)),
            sql_interrupt: None,
            sql_stopped: None,
            sql_subscription: None,
            sql_restore_sort: false,
            sql_catalog_revision: 0,
            sql_catalog_refreshing: false,
            sql_result_complete: false,
            sql_split,
            sql_editor_height: settings.sql_editor_height.unwrap_or(280.),
            database_tree,
            database_tree_open: settings.database_tree_open,
            database_tree_split,
            database_tree_width: settings.database_tree_width.unwrap_or(280.),
            session,
            computation,
            grid,
            fitted_query_config: None,
            search_input,
            search_completion,
            completion_columns_key: (0, 0),
            limit_input,
            query_limit,
            limit_error: None,
            sync_limit_input: false,
            repository,
            database,
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
            information_panel: None,
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
            slicer_sidebar,
            slicer_picker: None,
            slicer_discovery: None,
            file_groups: std::array::from_fn(|_| Vec::new()),
            file_loading: false,
            file_revision: 0,
            space_menu_open: false,
            action_menu_open: false,
            keymap,
            theme,
            font,
            keymap_profile,
            keymap_editor: None,
            aggregation_panel: None,
            dimension_panel: None,
            restore_focus: None,
            window_geometry: None,
            geometry_save: None,
            source_monitor: None,
            source_check_pending: false,
            source_probe: None,
            source_refresh_error: None,
            _subscriptions: vec![
                subscription,
                limit_subscription,
                limit_observer,
                grid_subscription,
                keymap_observer,
                theme_observer,
                font_observer,
                format_observer,
                slicer_subscription,
                sql_split_subscription,
                database_tree_subscription,
                search_completion_subscription,
                database_tree_split_subscription,
            ],
        };
        let grid_focus = view.grid.read(cx).focus_handle(cx);
        view.sync_grid_formats(cx);
        window.focus(&grid_focus, cx);
        view.register_window_cleanup(window, cx);
        if view.database_tree_open {
            view.refresh_database_tree(cx);
        }
        view.start_source_monitor(window, cx);
        if view.session.is_sql() {
            view.sql_restore_sort = true;
            view.ensure_sql_panel(window, cx);
        } else {
            view.schedule_search(view.session.search_text.clone(), cx);
        }
        view
    }

    fn start_source_monitor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.observe_window_activation(window, |view, _, cx| {
                view.check_source(cx);
            }));
        self.source_monitor = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |view, cx| view.check_source(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn check_source(&mut self, cx: &mut Context<Self>) {
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
        let Some(identity) = self.session.source.persistent_identity.clone() else {
            return;
        };
        let Some(path) = self.session.source_path.clone() else {
            return;
        };
        let database = self.database.clone();
        let source_revision = self.session.source.source_revision;
        let last_probe = self
            .source_probe
            .clone()
            .filter(|(probe_path, revision, _)| probe_path == &path && *revision == source_revision)
            .map(|(_, _, probe)| probe);
        self.source_check_pending = true;
        cx.spawn(async move |this, cx| {
            let read_path = PathBuf::from(&path);
            let (probe, result) = cx
                .background_executor()
                .spawn(async move {
                    // This runs every second, so skip the catalog queries
                    // while neither the file nor the database has changed.
                    let probe = database.probe(&read_path);
                    if probe.is_some() && probe == last_probe {
                        return (probe, Ok(SourceLoad::Reused));
                    }
                    let result =
                        database.load_reusing(&read_path, LoadMode::Check, Some(&identity));
                    (probe, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.source_check_pending = false;
                if view.loading
                    || view.session.source_path.as_ref() != Some(&path)
                    || view.session.source.source_revision != source_revision
                {
                    return;
                }
                view.source_probe = match &result {
                    Ok(SourceLoad::Reused) => {
                        probe.map(|probe| (path.clone(), source_revision, probe))
                    }
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
                            view.replace_dataframe(
                                FrameSnapshot::new(*store, source_revision + 1),
                                cx,
                            );
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
            });
        })
        .detach();
    }

    pub fn replace_dataframe(&mut self, frame: FrameSnapshot, cx: &mut Context<Self>) {
        self.replace_dataframe_with_reset(frame, false, cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn check_database_view(
        &mut self,
        database_view: crate::domain::documents::DatabaseView,
        cx: &mut Context<Self>,
    ) {
        self.source_check_pending = true;
        let database = self.database.clone();
        let source_revision = self.session.source.source_revision;
        cx.spawn(async move |this, cx| {
            let read_view = database_view.clone();
            let result = cx
                .background_executor()
                .spawn(async move { database.load_view(&read_view) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.source_check_pending = false;
                if view.loading
                    || view.session.source.source_revision != source_revision
                    || view
                        .session
                        .working
                        .as_ref()
                        .and_then(|d| d.database_view.as_ref())
                        != Some(&database_view)
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
            });
        })
        .detach();
    }

    fn replace_dataframe_with_reset(
        &mut self,
        frame: FrameSnapshot,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        self.session.reload_source(frame, reset);
        if reset {
            self.grid.update(cx, |grid, _| grid.reset_widths());
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
    pub fn selection(&self, cx: &App) -> Option<Selection> {
        self.grid.read(cx).selection()
    }
    pub fn search_text(&self) -> &str {
        &self.session.search_text
    }
    pub fn set_search_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the model and widget aligned before submitting the single request.
        self.session.search_text = text.clone();
        self.search_input
            .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        self.schedule_search(text, cx);
    }

    fn suggested_name(&self) -> String {
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

    fn sync_frozen_count(&mut self, cx: &mut Context<Self>) {
        let count = match self
            .session
            .working
            .as_ref()
            .map(|definition| &definition.view)
        {
            Some(ViewKind::Table(_)) => {
                table::visible_frozen_count(&self.session.effective_table_config())
            }
            Some(ViewKind::Query(pivot)) => pivot.group_by.len(),
            Some(ViewKind::Sql(config)) => {
                table::frozen_prefix(&config.frozen_columns, &self.result_column_names())
            }
            None => 0,
        };
        self.grid
            .update(cx, |grid, cx| grid.set_frozen_count(count, cx));
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

    fn sync_grid_widths(&mut self, cx: &mut Context<Self>) {
        self.sync_grid_formats(cx);
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
        self.grid.update(cx, |grid, cx| {
            if autofit {
                grid.reset_widths();
            }
            grid.configure_widths(owner, source, widths, cx)
        });
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

    fn sync_grid_formats(&mut self, cx: &mut Context<Self>) {
        let keys = self.column_format_keys();
        let profile = cx
            .global::<ColumnFormatRuntime>()
            .profiles
            .get(&self.keymap_profile);
        let mut formats = std::collections::BTreeMap::new();
        for key in keys.iter().rev() {
            if let Some(saved) = profile.and_then(|profile| profile.get(key)) {
                formats.extend(saved.clone());
            }
        }
        self.grid
            .update(cx, |grid, cx| grid.configure_formats(&formats, cx));
    }

    fn save_column_format(
        &mut self,
        column: &str,
        format: crate::domain::formatting::NumericFormat,
        cx: &mut Context<Self>,
    ) {
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
                cx.update_global(|runtime: &mut ColumnFormatRuntime, _| {
                    runtime.profiles.insert(profile, formats);
                });
            }
            Err(error) => self.error = Some(format!("Could not save column formatting: {error}")),
        }
        cx.notify();
    }

    fn autosave_table_preferences(&mut self, cx: &mut Context<Self>) {
        let Some((source, patch)) = self.session.take_table_patch() else {
            return;
        };
        let receiver = match self.table_writer.edit(PathBuf::from(&source), patch) {
            Ok(receiver) => receiver,
            Err(error) => {
                self.table_preferences_error =
                    Some(format!("Table preferences save failed: {error}"));
                cx.notify();
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let result = loop {
                match receiver.try_recv() {
                    Ok(result) => break result,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        break Err("Table preferences writer stopped".into());
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await
                    }
                }
            };
            let _ = this.update(cx, |view, cx| {
                if view.session.is_direct_table() && view.session.direct_table.source_path == source
                {
                    match result {
                        Err(error) => {
                            view.table_preferences_error =
                                Some(format!("Table preferences save failed: {error}"))
                        }
                        Ok(()) => view.table_preferences_error = None,
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn change_table_column(&mut self, direction: Option<isize>, cx: &mut Context<Self>) {
        if direction.is_none() && self.session.is_sql() {
            self.toggle_sql_frozen(cx);
            return;
        }
        if matches!(
            self.session
                .working
                .as_ref()
                .map(|definition| &definition.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        ) {
            return;
        }
        let Some(selection) = self.grid.read(cx).selection() else {
            return;
        };
        let mut config = self.session.effective_table_config();
        let order = config.column_order.clone();
        let destination = match direction {
            Some(direction) => {
                table::move_column(&mut config, &order, selection.active.column, direction)
            }
            None => table::toggle_frozen(&mut config, &order, selection.active.column),
        };
        if destination.is_none() {
            return;
        }
        if self.session.working.is_none() {
            let name = self.suggested_name();
            if let Err(error) = self.session.start_table_draft(name) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        let had_pending = self.busy || self.debouncing;
        if let Err(error) = self.session.set_table_layout(config.clone()) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.computation.invalidate(self.session.token());
        let ViewKind::Table(config) = &self
            .session
            .working
            .as_ref()
            .expect("table draft exists")
            .view
        else {
            return;
        };
        let available = self
            .session
            .source
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        let order = table::visible_order(&available, config);
        let result = self.session.result.reordered(
            &order,
            self.session.result.result_revision + 1,
            self.session.token().request_revision,
        );
        self.session.result = result.clone();
        self.grid
            .update(cx, |grid, cx| grid.replace_result(result, cx));
        self.sync_frozen_count(cx);
        self.autosave_table_preferences(cx);
        self.sync_grid_widths(cx);
        if had_pending {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        cx.notify();
    }

    fn open_save_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            self.persist_sql_view();
            cx.notify();
            return;
        }
        let suggested = self.suggested_name();
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("View name"));
        name_input.update(cx, |input, cx| input.set_value(suggested, window, cx));
        let name_focus = name_input.read(cx).focus_handle(cx);
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let content_input = name_input.clone();
            let save_input = name_input.clone();
            let save_view = view.clone();
            let content_view = view.clone();
            dialog
                .title("Save view")
                .content(move |content, _, cx| {
                    let error = content_view.read(cx).error.clone();
                    content
                        .child(Input::new(&content_input))
                        .when_some(error, |content, error| {
                            content.child(div().text_color(rgb(0xb33c3c)).child(error))
                        })
                })
                .footer(
                    DialogFooter::new()
                        .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                        .child(
                            DialogAction::new().child(Button::new("save-confirm").label("Save")),
                        ),
                )
                .on_ok(move |_, _, cx| {
                    let name = save_input.read(cx).value().trim().to_string();
                    save_view.update(cx, |view, cx| {
                        match view.session.save_as(name, &view.repository) {
                            Ok(()) => {
                                // Undo would otherwise restore draft states of a saved view.
                                view.reset_undo_history();
                                view.sync_grid_widths(cx);
                                if let Err(error) = view.record_opened() {
                                    eprintln!("history update failed: {error}");
                                }
                                view.error = None;
                                cx.notify();
                                true
                            }
                            Err(error) => {
                                view.error = Some(format!("Save failed: {error}"));
                                cx.notify();
                                false
                            }
                        }
                    })
                })
        });
        window.focus(&name_focus, cx);
    }

    fn open_information(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.information_revision += 1;
        let revision = self.information_revision;
        let result_revision = self.session.result.result_revision;
        let result = self.session.result.clone();
        let schema = result
            .columns
            .iter()
            .zip(result.dtypes.iter())
            .map(|(name, dtype)| (name.0.clone(), dtype.clone()))
            .collect::<Vec<_>>();
        let column_names = result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        self.information_busy = true;
        let sql = self.session.sql.text();
        let stored_rows = result.store_row_count();
        let scope = if self.is_table() {
            format!(
                "Column profiles cover all {stored_rows} source rows, ignoring search, slicers, and filters."
            )
        } else {
            format!("Column profiles cover all {stored_rows} result rows, ignoring search.")
        };
        let panel =
            cx.new(|cx| InformationPanel::new(schema, result.row_count(), scope, sql, window, cx));
        self.information_panel = Some(panel.downgrade());
        let dialog_panel = panel.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            let panel = dialog_panel.clone();
            dialog
                .title("Information")
                .w(px((window.bounds().size.width.as_f32()
                    * crate::ui::info::DIALOG_FRACTION)
                    .max(240.0)))
                .margin_top(px(window.bounds().size.height.as_f32()
                    * (1.0 - crate::ui::info::DIALOG_FRACTION)
                    / 2.0))
                .content(move |content, _, _| content.child(panel.clone()))
        });
        let store = result.store.clone();
        cx.spawn(async move |this, cx| {
            let summary_store = store.clone();
            let summary_names = column_names.clone();
            let summaries = cx
                .background_executor()
                .spawn(async move {
                    summary_store
                        .lock()
                        .map_err(|_| "DuckDB store lock poisoned".to_owned())?
                        .column_summaries(&summary_names)
                })
                .await;
            panel.update(cx, |panel, cx| panel.finish(summaries.clone(), cx));
            let current = |cx: &mut AsyncApp| {
                this.upgrade().is_some_and(|this| {
                    this.read_with(cx, |view, _| view.information_revision == revision)
                })
            };
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view.information_revision != revision
                        || view.session.result.result_revision != result_revision
                    {
                        return;
                    }
                    view.information_busy = false;
                    if let Err(error) = &summaries {
                        view.error = Some(format!("Information failed: {error}"));
                    }
                    cx.notify();
                });
            }
            if summaries.is_err() {
                return;
            }
            // One column per lock, so searching is not blocked behind every scan.
            for name in column_names {
                if !current(cx) {
                    return;
                }
                let column_store = store.clone();
                let column = name.clone();
                let values = cx
                    .background_executor()
                    .spawn(async move {
                        column_store
                            .lock()
                            .map_err(|_| "DuckDB store lock poisoned".to_owned())?
                            .top_values(&column, crate::ui::info::TOP_VALUE_COUNT)
                    })
                    .await;
                panel.update(cx, |panel, cx| panel.finish_top(&name, values, cx));
            }
        })
        .detach();
    }

    fn open_help(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sections = crate::ui::help::sections(&self.keymap);
        let scroll_handle = ScrollHandle::new();
        let font_profile = self.repository.paths.config.clone();
        let fallback_font = self.font.clone();
        window.open_dialog(cx, move |dialog, window, cx| {
            let font = cx
                .global::<FontRuntime>()
                .profiles
                .get(&font_profile)
                .unwrap_or(&fallback_font)
                .clone();
            let family = font
                .family
                .clone()
                .map(Into::into)
                .unwrap_or_else(|| cx.theme().font_family.clone());
            let layout =
                crate::ui::help::HelpLayout::for_viewport(window.viewport_size().width.as_f32());
            dialog
                .title(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(px(f32::from(font.size)))
                        .font_family(family)
                        .font_bold()
                        .child("Help · Keyboard shortcuts")
                        .child(
                            div().w_16().flex_shrink_0().child(
                                DialogClose::new().trigger(|button| {
                                    button.w_full().label("Close").small().ghost()
                                }),
                            ),
                        ),
                )
                .close_button(false)
                .w(px(layout.dialog_width))
                .p(px(crate::ui::help::DIALOG_PADDING))
                .content({
                    let sections = sections.clone();
                    let scroll_handle = scroll_handle.clone();
                    let font_profile = font_profile.clone();
                    let fallback_font = fallback_font.clone();
                    move |content, window, cx| {
                        content
                            .min_h_0()
                            .overflow_hidden()
                            .child(crate::ui::help::render_help(
                                &sections,
                                &scroll_handle,
                                cx.global::<FontRuntime>()
                                    .profiles
                                    .get(&font_profile)
                                    .unwrap_or(&fallback_font),
                                window,
                                cx,
                            ))
                    }
                })
        });
    }

    fn open_pivot_panel(
        &mut self,
        kind: PivotFieldKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PivotPanel> {
        let config = self.session.query_config();
        let (configured, blocked) = match kind {
            PivotFieldKind::GroupBy => (config.group_by, config.pivot_by),
            PivotFieldKind::PivotBy => (config.pivot_by, config.group_by),
        };
        let schema = self.session.source.schema.to_vec();
        let panel = cx.new(|cx| PivotPanel::new(schema, configured, kind, blocked, window, cx));
        self.dimension_panel = Some(panel.downgrade());
        self._subscriptions.push(cx.subscribe_in(
            &panel,
            window,
            move |this, panel, event: &PivotPanelEvent, window, cx| {
                let preferred = match &event.edit {
                    pivot_editor::PivotEdit::Add(field) => Some(field.clone()),
                    pivot_editor::PivotEdit::Replace { replacement, .. } => {
                        Some(replacement.clone())
                    }
                    pivot_editor::PivotEdit::Move { field, .. } => Some(field.clone()),
                    pivot_editor::PivotEdit::Remove(_) => None,
                };
                let mut config = this.session.query_config();
                let candidate = match pivot_editor::apply(&mut config, kind, event.edit.clone()) {
                    Err(error) => {
                        panel.update(cx, |panel, cx| {
                            panel.finish_error(event.revision, error, cx)
                        });
                        return;
                    }
                    Ok(changed) => {
                        let candidate = QueryCandidate {
                            editor: CandidateEditor::Dimension {
                                panel: panel.downgrade(),
                                kind,
                                preferred,
                            },
                            window: window.window_handle(),
                            revision: event.revision,
                            config,
                        };
                        if !changed {
                            candidate.finish_success(window, cx);
                            return;
                        }
                        candidate
                    }
                };
                this.submit_query_candidate(candidate, cx);
            },
        ));
        let dialog_panel = panel.clone();
        let title = match kind {
            PivotFieldKind::GroupBy => "Group By",
            PivotFieldKind::PivotBy => "Pivot By",
        };
        let width = (window.bounds().size.width.as_f32() - 48.).clamp(240., 1000.);
        window.open_dialog(cx, move |dialog, _, _| {
            let close_panel = dialog_panel.downgrade();
            let panel = dialog_panel.clone();
            dialog
                .title(title)
                .close_button(false)
                .w(px(width))
                .content(move |content, window, _| {
                    content.child(
                        div()
                            .max_h(px((window.bounds().size.height.as_f32() - 140.).max(100.)))
                            .overflow_y_scrollbar()
                            .child(panel.clone()),
                    )
                })
                .on_close(move |_, _, cx| {
                    if let Some(panel) = close_panel.upgrade() {
                        panel.update(cx, |panel, _| panel.close());
                    }
                })
        });
        panel.update(cx, |panel, cx| panel.focus_available(window, cx));
        panel
    }

    fn capture_dimension_editor(
        &mut self,
        target: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let kind = if target.starts_with("editor:group") {
            PivotFieldKind::GroupBy
        } else {
            PivotFieldKind::PivotBy
        };
        let error = target.ends_with("-error");
        if kind == PivotFieldKind::PivotBy
            && !error
            && self.session.query_config().group_by.is_empty()
        {
            let mut config = self.session.query_config();
            config.group_by = vec![Dimension::expression("floor(\"Row\" / 10) AS Batch")];
            if let Err(error) = self.session.set_query_config(config, self.suggested_name()) {
                self.error = Some(error);
                return;
            }
        }
        let panel = self.open_pivot_panel(kind, window, cx);
        panel.update(cx, |panel, cx| {
            panel.start_custom(window, cx);
            let sql = if error {
                "upper(\"missing column\") AS Label"
            } else if kind == PivotFieldKind::GroupBy {
                "floor(\"Row\" / 10) AS Batch"
            } else {
                "\"Row\" % 3 AS Kind"
            };
            panel
                .sql
                .read(cx)
                .state()
                .clone()
                .update(cx, |state, cx| state.set_value(sql, window, cx));
            panel.apply(cx);
        });
    }

    fn open_order_by_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let columns = self
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        let configured = self.session.sort.clone();
        let panel = cx.new(|cx| OrderByPanel::new(columns, configured, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &panel,
            window,
            move |this, panel, edit, window, cx| {
                let changed = match edit {
                    OrderByEdit::Add(column) => {
                        if this.session.sort.iter().any(|key| key.column == *column) {
                            false
                        } else {
                            this.session.set_sort_direction(column.clone(), false);
                            true
                        }
                    }
                    OrderByEdit::Remove(column) => this.session.clear_sort_column(column),
                    OrderByEdit::Move { column, direction } => {
                        this.session.move_sort_key(column, *direction)
                    }
                    OrderByEdit::ToggleDirection(column) => {
                        if let Some(descending) = this
                            .session
                            .sort
                            .iter()
                            .find(|key| key.column == *column)
                            .map(|key| !key.descending)
                        {
                            this.session.set_sort_direction(column.clone(), descending);
                            true
                        } else {
                            false
                        }
                    }
                    OrderByEdit::SetDirection { column, descending } => {
                        if this
                            .session
                            .sort
                            .iter()
                            .find(|key| key.column == *column)
                            .is_some_and(|key| key.descending != *descending)
                        {
                            this.session.set_sort_direction(column.clone(), *descending);
                            true
                        } else {
                            false
                        }
                    }
                };
                if !changed {
                    return;
                }
                let sort = this.session.sort.clone();
                this.grid
                    .update(cx, |grid, cx| grid.set_sort(sort.clone(), cx));
                panel.update(cx, |panel, cx| panel.set_configured(sort, window, cx));
                this.schedule_search(this.session.search_text.clone(), cx);
            },
        ));
        let focus_panel = panel.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let panel = panel.clone();
            dialog
                .title("Order By")
                .close_button(false)
                .w(px(760.))
                .content(move |content, _, _| content.child(panel.clone()))
        });
        focus_panel.update(cx, |panel, cx| panel.focus_available(window, cx));
    }

    fn open_aggregation_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<AggregationPanel>> {
        let config = self.session.query_config();
        let schema = self.session.source.schema.to_vec();
        let panel = cx.new(|cx| {
            AggregationPanel::new(
                schema,
                config.aggregations,
                !config.pivot_by.is_empty(),
                window,
                cx,
            )
        });
        self.aggregation_panel = Some(panel.downgrade());
        self._subscriptions.push(cx.subscribe_in(
            &panel,
            window,
            |this, panel, event: &AggregationPanelEvent, window, cx| {
                this.check_aggregation(panel, event, window, cx);
            },
        ));
        let dialog_panel = panel.clone();
        let width = (window.bounds().size.width.as_f32() - 48.).clamp(240., 1000.);
        window.open_dialog(cx, move |dialog, _, _| {
            let close_panel = dialog_panel.downgrade();
            let panel = dialog_panel.clone();
            dialog
                .title("Aggregations")
                .close_button(true)
                .w(px(width))
                .content(move |content, window, _| {
                    content.child(
                        div()
                            .max_h(px((window.bounds().size.height.as_f32() - 140.).max(100.)))
                            .overflow_y_scrollbar()
                            .child(panel.clone()),
                    )
                })
                .on_close(move |_, _, cx| {
                    if let Some(panel) = close_panel.upgrade() {
                        panel.update(cx, |panel, _| panel.close());
                    }
                })
        });
        panel.update(cx, |panel, cx| panel.focus_available(window, cx));
        Some(panel)
    }

    fn check_aggregation(
        &mut self,
        panel: &Entity<AggregationPanel>,
        event: &AggregationPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preferred = match &event.edit {
            AggregationEdit::Add(aggregation) => Some(aggregation.clone()),
            AggregationEdit::Replace { replacement, .. } => Some(replacement.clone()),
            AggregationEdit::Move { aggregation, .. } => Some(aggregation.clone()),
            AggregationEdit::Remove(_) => None,
        };
        let mut config = self.session.query_config();
        match pivot_editor::apply_aggregation(&mut config, event.edit.clone()) {
            Err(error) => {
                panel.update(cx, |panel, cx| {
                    panel.finish_error(event.revision, error, cx)
                });
                return;
            }
            Ok(false) => {
                panel.update(cx, |panel, cx| {
                    panel.finish_success(config.aggregations, preferred, window, cx)
                });
                return;
            }
            Ok(true) => {}
        }
        let candidate = QueryCandidate {
            editor: CandidateEditor::Aggregation {
                panel: panel.downgrade(),
                preferred,
            },
            window: window.window_handle(),
            revision: event.revision,
            config,
        };
        self.submit_query_candidate(candidate, cx);
    }

    fn submit_query_candidate(&mut self, candidate: QueryCandidate, cx: &mut Context<Self>) {
        let mut candidate_session = self.session.clone();
        if let Err(error) =
            candidate_session.set_query_config(candidate.config.clone(), self.suggested_name())
        {
            candidate.finish_error(error, cx);
            return;
        }
        let tokens = match query::parse(&self.session.search_text) {
            Ok(tokens) => tokens,
            Err(error) => {
                candidate.finish_error(error, cx);
                return;
            }
        };
        let pending_setup = (candidate.config.group_by.is_empty()
            && !candidate.config.pivot_by.is_empty())
        .then(|| self.session.result.clone());
        let token = self
            .session
            .set_search_text(self.session.search_text.clone());
        self.computation.invalidate(token);
        self.busy = true;
        self.debouncing = false;
        let request = PipelineRequest {
            query_candidate: Some(candidate),
            pending_setup,
            query_limit: self.query_limit,
            source: self.session.source.clone(),
            tokens,
            sort: self.session.sort.clone(),
            definition: candidate_session.computation_definition(),
            query_cache: None,
            slicer_discovery: self.slicer_discovery.clone(),
            queued_at: Instant::now(),
            reuse_table: None,
            table_trace: SqlTrace::default(),
        };
        self.submit_pipeline(token, request, cx);
        cx.notify();
    }

    /// Why slicers and value filters are off for this SQL result, if they are.
    fn sql_filters_unavailable(&self) -> Option<&'static str> {
        (self.session.is_sql() && !self.sql_result_complete).then_some(
            "Slicers and = / - filters need the script's last statement to be a query              (SELECT, WITH, FROM, VALUES, PIVOT, DESCRIBE), run outside a transaction",
        )
    }

    fn dispatch_command(&mut self, id: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.is_sql()
            && matches!(
                id,
                CommandId::Group | CommandId::Pivot | CommandId::Aggregation | CommandId::OrderBy
            )
        {
            return;
        }
        if id == CommandId::AddSlicer
            && let Some(reason) = self.sql_filters_unavailable()
        {
            self.error = Some(reason.into());
            cx.notify();
            return;
        }
        match id {
            CommandId::Back => self.navigate_history(false, window, cx),
            CommandId::Forward => self.navigate_history(true, window, cx),
            CommandId::SqlEditor => self.open_sql_editor(window, cx),
            CommandId::StartupSql => {
                if matches!(self.session.identity, SessionIdentity::StartupSql) {
                    self.open_sql_editor(window, cx);
                } else {
                    self.navigate_to_file(
                        FileEntry::StartupSql,
                        false,
                        false,
                        false,
                        window.window_handle(),
                        cx,
                    );
                }
            }
            CommandId::Files => self.open_file_palette(window, cx),
            CommandId::DatabaseTree => self.toggle_database_tree(window, cx),
            CommandId::Commands => self.open_command_palette(window, cx),
            CommandId::Keymap => self.open_keymap_editor(window, cx),
            CommandId::Theme => {
                self.open_theme_editor(window, cx);
            }
            CommandId::Fonts => self.open_font_picker(FontTarget::Grid, window, cx),
            CommandId::EditorFont => self.open_font_picker(FontTarget::Editor, window, cx),
            CommandId::IncreaseFontSize => self.change_font_size(1, cx),
            CommandId::DecreaseFontSize => self.change_font_size(-1, cx),
            CommandId::ClearFilters => self.clear_filters(window, cx),
            CommandId::Reload => self.reload(false, window, cx),
            CommandId::Open => self.open_native_file(window, cx),
            CommandId::PasteTable => self.paste_table(window, cx),
            CommandId::AddFolder => self.add_folder_with_picker(cx),
            CommandId::OpenSettingsDirectory => self.open_settings_directory(cx),
            CommandId::ReloadReset => self.reload(true, window, cx),
            CommandId::Save => self.open_save_dialog(window, cx),
            CommandId::NewWindow => self.open_blank_window(None, cx),
            CommandId::Copy => {
                if let Some(selection) = self.grid.read(cx).selection() {
                    self.schedule_copy(self.session.result.clone(), selection, cx);
                }
            }
            CommandId::CopyAs => self.open_copy_as(window, cx),
            CommandId::Search => {
                let focus = self.search_input.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
            CommandId::Help => self.open_help(window, cx),
            CommandId::Info => self.open_information(window, cx),
            CommandId::Group => {
                self.open_pivot_panel(PivotFieldKind::GroupBy, window, cx);
            }
            CommandId::Aggregation => {
                self.open_aggregation_panel(window, cx);
            }
            CommandId::Pivot => {
                self.open_pivot_panel(PivotFieldKind::PivotBy, window, cx);
            }
            CommandId::OrderBy => self.open_order_by_panel(window, cx),
            CommandId::AddSlicer => self.open_slicer_picker(window, cx),
            CommandId::Space => self.open_space_menu(window, cx),
            CommandId::SortAscending => self.sort_active(false, cx),
            CommandId::SortDescending => self.sort_active(true, cx),
            CommandId::ClearSort => self.clear_sort(cx),
            CommandId::CloseWindow => {
                self.stop_sql();
                self.autosave_table_preferences(cx);
                self.update_geometry(window);
                self.save_geometry();
                window.remove_window();
            }
            CommandId::ToggleFrozen => self.change_table_column(None, cx),
            CommandId::UnfreezeAll => self.unfreeze_all(cx),
            CommandId::Export => self.export_result(cx),
            CommandId::ToggleTotals => self.toggle_totals(cx),
            CommandId::ToggleHeatmap => self.toggle_heatmap(cx),
            CommandId::RecalculateWidths => self.recalculate_widths(cx),
            CommandId::HideColumns => self.hide_selected_columns(cx),
            CommandId::ChooseColumns => self.open_column_chooser(window, cx),
            CommandId::ShowAllColumns => {
                let hidden = self.session.effective_table_config().hidden;
                self.set_columns_hidden(&hidden, false, cx);
            }
            CommandId::FilterToValue => self.filter_active_value(false, cx),
            CommandId::ExcludeValue => self.filter_active_value(true, cx),
            CommandId::DrillDown => self.drill_down(false, window, cx),
            CommandId::DrillDownNewWindow => self.drill_down(true, window, cx),
            CommandId::CellDetail => self.toggle_cell_detail(window, cx),
            CommandId::JumpToColumn => self.open_column_jump(window, cx),
            CommandId::Undo => self.undo_view_change(false, cx),
            CommandId::Redo => self.undo_view_change(true, cx),
            CommandId::FocusGrid => {
                window.focus(&self.grid.read(cx).focus_handle(cx), cx);
            }
            CommandId::FocusSlicers => self.focus_next_slicer(window, cx),
            CommandId::RunAction => self.open_action_menu(window, cx),
            CommandId::EditActions => self.open_actions_editor(window, cx),
            CommandId::AiProviders => self.open_ai_settings(window, cx),
            CommandId::AskSql => {
                self.open_sql_editor(window, cx);
                if self.session.is_sql()
                    && let Some(panel) = self.sql_panel.clone()
                {
                    panel.update(cx, |panel, cx| panel.open_ask(window, cx));
                }
            }
        }
    }

    fn open_settings_directory(&mut self, cx: &mut Context<Self>) {
        let path = &self.repository.paths.config;
        match std::fs::create_dir_all(path) {
            Ok(()) => cx.open_with_system(path),
            Err(error) => {
                self.error = Some(format!(
                    "Cannot open settings directory {}: {error}",
                    path.display()
                ));
                cx.notify();
            }
        }
    }

    /// Opens a new window, empty or showing the source at `path`.
    fn open_blank_window(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        self.open_window_with(
            ViewerOptions {
                load_path: path,
                ..Default::default()
            },
            cx,
        );
    }

    /// Opens a new window on this window's profile with `options`.
    fn open_window_with(&mut self, options: ViewerOptions, cx: &mut Context<Self>) {
        let repository = self.repository.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { crate::infrastructure::duckdb_store::DuckDbStore::empty() })
                .await;
            let _ = this.update(cx, |view, cx| match result {
                Ok(store) => Self::open_with_error_target(
                    FrameSnapshot::new(store, 1),
                    options
                        .load_path
                        .as_ref()
                        .map(|path| path.to_string_lossy().into_owned()),
                    ViewerOptions {
                        repository,
                        restore_window: false,
                        ..options
                    },
                    Some(cx.weak_entity()),
                    cx,
                ),
                Err(error) => {
                    view.error = Some(format!("Cannot open new window: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn dispatch_keymap_action(
        &mut self,
        action: ActionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(command) = KEYMAP_ACTIONS
            .iter()
            .find(|spec| spec.id == action)
            .and_then(|spec| spec.command)
        {
            self.dispatch_command(command, window, cx);
            return;
        }
        match action {
            ActionId::ToggleThousandsGrouping
            | ActionId::IncreaseDecimalPrecision
            | ActionId::DecreaseDecimalPrecision => self
                .grid
                .update(cx, |grid, cx| grid.change_numeric_format(action, cx)),
            ActionId::MoveColumnLeft => self
                .grid
                .update(cx, |_, cx| cx.emit(GridEvent::MoveColumn(-1))),
            ActionId::MoveColumnRight => self
                .grid
                .update(cx, |_, cx| cx.emit(GridEvent::MoveColumn(1))),
            ActionId::RemoveActiveGroupColumn => self.remove_active_group_column(cx),
            ActionId::ToggleFrozen => self.change_table_column(None, cx),
            ActionId::UnfreezeAll => self.unfreeze_all(cx),
            _ => self
                .grid
                .update(cx, |grid, cx| grid.navigate_action(action, window, cx)),
        }
    }

    fn is_table(&self) -> bool {
        !matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        )
    }

    fn open_space_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu_open = true;
        let grid_focus = self.grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        cx.notify();
    }

    fn select_space_command(&mut self, id: CommandId, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu_open = false;
        self.dispatch_command(id, window, cx);
        cx.notify();
    }

    fn handle_space_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if matches!(key.as_str(), "space" | "escape") {
            self.space_menu_open = false;
            cx.notify();
        } else if let Ok(stroke) = canonical_keystroke(&key_with_modifiers(event)) {
            let context = if self.is_table() {
                KeyContext::TableSpaceMenu
            } else {
                KeyContext::QuerySpaceMenu
            };
            if let Some(action) = self.keymap.binding_for(&stroke, context)
                && let Some(command) = KEYMAP_ACTIONS
                    .iter()
                    .find(|spec| {
                        spec.id == action
                            && matches!(
                                spec.context,
                                KeyContext::SpaceMenu
                                    | KeyContext::TableSpaceMenu
                                    | KeyContext::QuerySpaceMenu
                            )
                    })
                    .and_then(|spec| spec.command)
            {
                self.select_space_command(command, window, cx);
            }
        }
        cx.stop_propagation();
    }

    fn sort_active(&mut self, descending: bool, cx: &mut Context<Self>) {
        let Some(selection) = self.grid.read(cx).selection() else {
            return;
        };
        let Some(column) = self.session.result.columns.get(selection.active.column) else {
            return;
        };
        self.session
            .set_sort_direction(column.0.clone(), descending);
        let sort = self.session.sort.clone();
        self.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn remove_active_group_column(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.grid.read(cx).selection() else {
            return;
        };
        let Some(ViewKind::Query(current)) = self.session.working.as_ref().map(|view| &view.view)
        else {
            return;
        };
        let Some(field) = current.group_by.get(selection.active.column).cloned() else {
            return;
        };
        let mut updated = current.clone();
        match pivot_editor::apply(
            &mut updated,
            PivotFieldKind::GroupBy,
            pivot_editor::PivotEdit::Remove(field),
        ) {
            Ok(true) => {
                if let Err(error) = self
                    .session
                    .edit_definition(|view| view.view = ViewKind::Query(updated))
                {
                    self.error = Some(error);
                    cx.notify();
                    return;
                }
                self.schedule_search(self.session.search_text.clone(), cx);
            }
            Ok(false) => {}
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    fn clear_sort(&mut self, cx: &mut Context<Self>) {
        if !self.session.clear_sort() {
            return;
        }
        self.grid
            .update(cx, |grid, cx| grid.set_sort(Vec::new(), cx));
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.clear_filters() {
            self.set_search_text(String::new(), window, cx);
        }
    }

    /// The active cell's position and its value as the grid renders it before formatting.
    fn active_cell_value(&self, cx: &App) -> Result<(usize, usize, Option<String>), String> {
        let selection = self
            .grid
            .read(cx)
            .selection()
            .ok_or("Select a cell first")?;
        let result = &self.session.result;
        let crate::domain::selection::Cell { row, column } = selection.active;
        if row >= result.row_count() || column >= result.column_count() {
            return Err("Select a cell first".into());
        }
        let rows = match result.cached_rows(row, 1) {
            Some(rows) => rows,
            None => result.fetch_rows(row, 1)?,
        };
        let value = rows
            .first()
            .and_then(|cached| cached.values.get(result.column_indices[column]))
            .ok_or("The active row is no longer available")?
            .clone();
        Ok((row, column, value))
    }

    fn toggle_cell_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cell_detail.take().is_none() {
            let pane = cx.new(|cx| CellDetailPane::new(window, cx));
            let subscription = cx.subscribe(&pane, |view, _, event, cx| match event {
                CellDetailEvent::Close => {
                    view.cell_detail = None;
                    cx.notify();
                }
            });
            self.cell_detail = Some((pane, subscription));
            self.refresh_cell_detail(cx);
        }
        cx.notify();
    }

    /// Shows the active cell in the cell detail pane, when it is open.
    fn refresh_cell_detail(&mut self, cx: &mut Context<Self>) {
        let Some((pane, _)) = &self.cell_detail else {
            return;
        };
        let content = match self.active_cell_value(cx) {
            Ok((row, column, value)) => {
                let result = &self.session.result;
                CellContent::Cell {
                    column: result.columns[column].0.clone(),
                    dtype: result.dtypes[column].clone(),
                    row,
                    value,
                }
            }
            Err(message) => CellContent::Message(message),
        };
        pane.update(cx, |pane, cx| pane.set_content(content, cx));
    }

    fn filter_active_value(&mut self, exclude: bool, cx: &mut Context<Self>) {
        let filter = self.active_cell_value(cx).and_then(|(_, column, value)| {
            let target = match self.session.working.as_ref().map(|view| &view.view) {
                Some(ViewKind::Sql(_)) => {
                    if let Some(reason) = self.sql_filters_unavailable() {
                        return Err(reason.into());
                    }
                    Dimension::Column(self.session.result.columns[column].0.clone())
                }
                Some(ViewKind::Query(config)) => {
                    config.group_by.get(column).cloned().ok_or_else(|| {
                        "Value filters apply to group-by columns; press Enter to drill down instead"
                            .to_owned()
                    })?
                }
                _ => Dimension::Column(self.session.result.columns[column].0.clone()),
            };
            Ok(ValueFilter {
                target,
                value,
                exclude,
            })
        });
        let filter = match filter {
            Ok(filter) => filter,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.ensure_working_view();
        match self.session.add_value_filter(filter) {
            Ok(true) => {
                self.error = None;
                self.schedule_search(self.session.search_text.clone(), cx);
            }
            Ok(false) => {}
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }

    /// A raw file has no view definition; start a table draft so filters and
    /// actions have somewhere to live.
    fn ensure_working_view(&mut self) {
        if self.session.working.is_none() {
            let _ = self.session.start_table_draft(self.suggested_name());
            if let Some(definition) = &mut self.session.working
                && let ViewKind::Table(table) = &mut definition.view
            {
                table.sort = self.session.sort.clone();
            }
        }
    }

    fn view_actions(&self) -> Vec<ViewAction> {
        self.session
            .working
            .as_ref()
            .map(|definition| definition.actions.clone())
            .unwrap_or_default()
    }

    fn open_action_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view_actions().is_empty() {
            self.open_actions_editor(window, cx);
            return;
        }
        self.space_menu_open = false;
        self.action_menu_open = true;
        let grid_focus = self.grid.read(cx).focus_handle(cx);
        window.focus(&grid_focus, cx);
        cx.notify();
    }

    fn handle_action_menu_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.to_ascii_lowercase();
        if matches!(key.as_str(), "space" | "escape") {
            self.action_menu_open = false;
            cx.notify();
        } else if let Some(typed) = event.keystroke.key_char.as_deref()
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && let Some(index) = self
                .view_actions()
                .iter()
                .position(|action| action.key == typed)
        {
            self.run_view_action(index, cx);
        }
        cx.stop_propagation();
    }

    /// Runs action `index` of the current view with the active row's values.
    fn run_view_action(&mut self, index: usize, cx: &mut Context<Self>) {
        self.action_menu_open = false;
        let Some(action) = self.view_actions().into_iter().nth(index) else {
            return;
        };
        self.error = self.launch_action(&action, cx).err();
        cx.notify();
    }

    fn active_row(&self, cx: &App) -> usize {
        self.grid
            .read(cx)
            .selection()
            .map_or(usize::MAX, |selection| selection.active.row)
    }

    /// Runs `action` with the active row's values.
    fn launch_action(&self, action: &ViewAction, cx: &mut App) -> Result<(), String> {
        let row = self.active_row(cx);
        actions::launch_for_row(&action.command, &self.session.result, row)
            .and_then(|launch| match launch {
                actions::Launch::Url(url) => {
                    cx.open_url(&url);
                    Ok(())
                }
                actions::Launch::Program { program, args } => {
                    launcher::spawn_program(&program, &args)
                }
            })
            .map_err(|error| format!("Action {} failed: {error}", action.description))
    }

    fn open_actions_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu_open = false;
        self.action_menu_open = false;
        let columns = self
            .session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        let actions = self.view_actions();
        let row = actions::row_values(&self.session.result, self.active_row(cx)).ok();
        let editor = cx.new(|cx| ActionsEditor::new(actions, columns, row, window, cx));
        self._subscriptions.push(cx.subscribe_in(
            &editor,
            window,
            |this, editor, event: &ActionsEditorEvent, _, cx| {
                let result = match event {
                    ActionsEditorEvent::Changed(actions) => {
                        this.ensure_working_view();
                        this.session
                            .edit_definition(|definition| definition.actions = actions.clone())
                    }
                    ActionsEditorEvent::Run(action) => this.launch_action(action, cx),
                };
                editor.update(cx, |editor, cx| editor.set_error(result.err(), cx));
                cx.notify();
            },
        ));
        let focus_editor = editor.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let editor = editor.clone();
            dialog
                .title("View actions")
                .w(px(720.))
                .content(move |content, _, _| content.child(editor.clone()))
        });
        focus_editor.update(cx, |editor, cx| editor.focus(window, cx));
    }

    fn open_ai_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.space_menu_open = false;
        self.action_menu_open = false;
        let settings = match self.repository.load_settings() {
            Ok(settings) => settings.ai,
            Err(error) => {
                self.error = Some(format!("Cannot read settings: {error}"));
                cx.notify();
                return;
            }
        };
        let editor = cx.new(|cx| AiSettingsEditor::new(settings, window, cx));
        self._subscriptions.push(cx.subscribe(
            &editor,
            |this, _, event: &AiSettingsEvent, cx| {
                let AiSettingsEvent::Changed(ai) = event;
                let result = this.repository.load_settings().and_then(|mut settings| {
                    settings.ai = ai.clone();
                    this.repository.save_settings(&settings)
                });
                if let Err(error) = result {
                    this.error = Some(format!("Cannot save AI providers: {error}"));
                }
                cx.notify();
            },
        ));
        let focus_editor = editor.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let editor = editor.clone();
            dialog
                .title("AI providers")
                .w(px(760.))
                .content(move |content, _, _| content.child(editor.clone()))
        });
        focus_editor.update(cx, |editor, cx| editor.focus(window, cx));
    }

    /// Opens the actions editor or menu for a screenshot, with sample actions
    /// when the view has none.
    fn capture_actions(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.view_actions().is_empty() {
            let column = self
                .session
                .result
                .columns
                .first()
                .map_or_else(|| "Column".to_owned(), |column| column.0.clone());
            self.ensure_working_view();
            let _ = self.session.edit_definition(|definition| {
                definition.actions = vec![
                    ViewAction {
                        key: "g".into(),
                        description: "Search the web".into(),
                        command: format!("https://www.google.com/search?q={{{column}}}"),
                    },
                    ViewAction {
                        key: "n".into(),
                        description: "Open in Notepad".into(),
                        command: format!("notepad \"C:/notes/{{{column}}}.txt\""),
                    },
                ]
            });
        }
        if target == "action-menu" {
            self.open_action_menu(window, cx);
        } else {
            self.open_actions_editor(window, cx);
        }
    }

    fn remove_value_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.session.remove_value_filter(index) {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        cx.notify();
    }

    /// Opens the source table filtered to the rows behind the active query cell.
    /// Opens the source rows behind the active query cell, here or in a new window.
    fn drill_down(&mut self, new_window: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.drill_down_target(cx) {
            Ok((entry, drill)) if new_window => self.open_window_with(
                ViewerOptions {
                    open_entry: Some((entry, drill)),
                    ..Default::default()
                },
                cx,
            ),
            Ok((entry, drill)) => self.navigate_to_location(
                entry,
                false,
                false,
                true,
                None,
                drill,
                window.window_handle(),
                cx,
            ),
            Err(error) => {
                self.error = Some(error);
                cx.notify();
            }
        }
    }

    fn drill_down_target(&self, cx: &App) -> Result<(FileEntry, Option<DrillDown>), String> {
        let definition = self
            .session
            .working
            .as_ref()
            .filter(|definition| matches!(definition.view, ViewKind::Query(_)))
            .ok_or("Drill down works on query results; press = to filter a table instead")?;
        let ViewKind::Query(config) = &definition.view else {
            unreachable!("checked above");
        };
        let (row, column, _) = self.active_cell_value(cx)?;
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
        Ok((
            FileEntry::Source(PathBuf::from(path)),
            Some(DrillDown { slicers, filters }),
        ))
    }

    fn open_column_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.session.result.clone();
        if result.column_count() == 0 {
            return;
        }
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = command_state.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let result = result.clone();
            dialog
                .title("Jump to column")
                .w(px(PALETTE_WIDTH))
                .margin_top(px(24.))
                .content(move |content, window, _| {
                    let view = view.clone();
                    content.child(
                        Command::new(&state)
                            .max_h(px(
                                (window.bounds().size.height.as_f32() - 200.).clamp(100., 440.)
                            ))
                            .items(result.columns.iter().zip(result.dtypes.iter()).map(
                                |(column, dtype)| {
                                    let name = column.0.clone();
                                    let dtype = crate::domain::formatting::type_label(dtype)
                                        .into_owned();
                                    CommandItem::new().label(name.clone()).child(move |_, _| {
                                        div()
                                            .w_full()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_3()
                                            .child(div().min_w_0().truncate().child(name.clone()))
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_sm()
                                                    .opacity(0.6)
                                                    .child(dtype.clone()),
                                            )
                                    })
                                },
                            ))
                            .placeholder("Find a column")
                            .on_confirm(move |index, window, cx| {
                                window.close_dialog(cx);
                                view.update(cx, |view, cx| {
                                    let grid_focus = view.grid.read(cx).focus_handle(cx);
                                    view.grid.update(cx, |grid, cx| {
                                        grid.jump_to_column(index.row, window, cx)
                                    });
                                    window.focus(&grid_focus, cx);
                                });
                            }),
                    )
                })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn change_font_size(&mut self, delta: i16, cx: &mut Context<Self>) {
        let mut font = self.font.clone();
        font.size = (i16::from(font.size) + delta).clamp(7, 40) as u8;
        self.save_font(font, cx);
    }

    fn save_sql_editor_height(&mut self, height: f32) {
        let height = height.round();
        if height == self.sql_editor_height {
            return;
        }
        self.sql_editor_height = height;
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.sql_editor_height = Some(height);
            self.repository.save_settings(&settings)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save SQL editor height: {error}"));
        }
    }

    fn save_font(&mut self, font: FontPreferences, cx: &mut Context<Self>) {
        if font == self.font {
            return;
        }
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.font_size = font.size;
            settings.font_family = font.family.clone();
            settings.editor_font_family = font.editor_family.clone();
            self.repository.save_settings(&settings)
        });
        match result {
            Ok(()) => {
                crate::ui::font_picker::apply_editor_font(&font, cx);
                self.font = font.clone();
                let profile = self.keymap_profile.clone();
                cx.update_global(|runtime: &mut FontRuntime, _| {
                    runtime.profiles.insert(profile, font);
                });
            }
            Err(error) => self.error = Some(format!("Could not save font settings: {error}")),
        }
        cx.notify();
    }

    fn commit_query_limit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let limit = match crate::infrastructure::json_store::parse_query_limit(
            &self.limit_input.read(cx).value(),
        ) {
            Ok(limit) => limit,
            Err(error) => {
                self.limit_error = Some(error);
                cx.notify();
                return;
            }
        };
        if limit == self.query_limit {
            self.limit_error = None;
            return;
        }
        let saved = self.repository.load_settings().and_then(|mut settings| {
            settings.query_limit = limit;
            self.repository.save_settings(&settings)
        });
        if let Err(error) = saved {
            self.limit_error = Some(format!("Could not save query limit: {error}"));
            cx.notify();
            return;
        }
        self.query_limit = limit;
        self.limit_error = None;
        self.query_cache = None;
        self.limit_input.update(cx, |input, cx| {
            input.set_value(limit.to_string(), window, cx)
        });
        let profile = self.keymap_profile.clone();
        cx.update_global(|runtime: &mut QueryLimitRuntime, _| {
            runtime.profiles.insert(profile, limit);
        });
        if !self.is_table() && !self.session.is_sql() {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        cx.notify();
    }

    fn open_font_picker(
        &mut self,
        target: FontTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = state.clone();
        let mut families = cx.text_system().all_font_names();
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup();
        let (title, default_family) = match target {
            FontTarget::Grid => ("Fonts", cx.theme().font_family.clone()),
            FontTarget::Editor => {
                // Code reads best in a monospaced font, so those come first.
                families.sort_by_key(|family| !crate::ui::font_picker::looks_monospaced(family));
                (
                    "Editor font",
                    crate::ui::font_picker::default_editor_family(cx),
                )
            }
        };
        let mut fonts = vec![(None, format!("Default · {default_family}"))];
        fonts.extend(
            families
                .into_iter()
                .map(|family| (Some(family.clone()), family)),
        );
        let ui_family = cx.theme().font_family.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = state.clone();
            let fonts = fonts.clone();
            let view = view.clone();
            let default_family = default_family.clone();
            let ui_family = ui_family.clone();
            dialog
                .title(title)
                .w(px(PALETTE_WIDTH))
                .content(move |content, _, _| {
                    let confirm_fonts = fonts.clone();
                    let view = view.clone();
                    content.child(
                        Command::new(&state)
                            .placeholder("Find a font")
                            .items(fonts.iter().map(|(family, label)| {
                                let label = label.clone();
                                let label_family = ui_family.clone();
                                let family: SharedString = family
                                    .clone()
                                    .map(Into::into)
                                    .unwrap_or_else(|| default_family.clone());
                                let preview = format!("123 The quick brown fox — {label}");
                                CommandItem::new().label(label.clone()).child(move |_, _| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .w(px(180.))
                                                .flex_shrink_0()
                                                .font_family(label_family.clone())
                                                .child(label.clone()),
                                        )
                                        .child(
                                            div()
                                                .min_w_0()
                                                .font_family(family.clone())
                                                .child(preview.clone()),
                                        )
                                })
                            }))
                            .on_confirm(move |index, window, cx| {
                                if let Some((family, _)) = confirm_fonts.get(index.row) {
                                    window.close_dialog(cx);
                                    view.update(cx, |view, cx| {
                                        let mut font = view.font.clone();
                                        match target {
                                            FontTarget::Grid => font.family = family.clone(),
                                            FontTarget::Editor => {
                                                font.editor_family = family.clone()
                                            }
                                        }
                                        view.save_font(font, cx);
                                    });
                                }
                            }),
                    )
                })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Asks for a file and writes the whole displayed result to it, in the
    /// format its extension names.
    fn export_result(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            self.error = Some("Wait for the result to finish before exporting".into());
            cx.notify();
            return;
        }
        let result = self.session.result.clone();
        let rerun = self.unlimited_query_rerun();
        let directory = self
            .session
            .source_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).parent())
            .filter(|directory| directory.is_dir())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!(
            "{}.csv",
            self.suggested_name()
                .replace(['<', '>', ':', '"', '/', '\\', '|', '?', '*'], "_")
        );
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        let generation = self.session.token().session_generation;
        cx.spawn(async move |this, cx| {
            let report = |message: String, cx: &mut AsyncApp| {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                        view.copy_status = Some(message);
                        cx.notify();
                    });
                }
            };
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => return,
                failure => {
                    report(format!("Export failed: {failure:?}"), cx);
                    return;
                }
            };
            let format = match export::FileFormat::from_path(&path) {
                Ok(format) => format,
                Err(error) => {
                    report(format!("Export failed: {error}"), cx);
                    return;
                }
            };
            report("Exporting…".into(), cx);
            let target = path.clone();
            let written = cx
                .background_executor()
                .spawn(async move {
                    let result = match rerun {
                        Some(rerun) => rerun.run()?,
                        None => result,
                    };
                    result.export_to_file(&target, format)
                })
                .await;
            let current = this.upgrade().is_some_and(|this| {
                this.read_with(cx, |view, _| {
                    view.session.token().session_generation == generation
                })
            });
            if current {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                report(
                    match written {
                        Ok(rows) => format!(
                            "Exported {rows} {} to {name}",
                            if rows == 1 { "row" } else { "rows" }
                        ),
                        Err(error) => format!("Export failed: {error}"),
                    },
                    cx,
                );
            }
        })
        .detach();
    }

    /// Forgets every column width, including ones set by hand, and fits each
    /// column to its rows, header, and totals again.
    fn recalculate_widths(&mut self, cx: &mut Context<Self>) {
        let owner = self.width_owner();
        let source = self.is_table().then(|| self.session.source.clone());
        self.grid.update(cx, |grid, cx| {
            grid.reset_widths();
            grid.configure_widths(owner, source, Default::default(), cx)
        });
    }

    fn toggle_totals(&mut self, cx: &mut Context<Self>) {
        let has_groups = matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(config)) if !config.group_by.is_empty()
        );
        if !has_groups {
            self.error = Some("Totals apply to queries with a group-by".into());
            cx.notify();
            return;
        }
        if let Err(error) = self.session.edit_definition(|definition| {
            if let ViewKind::Query(config) = &mut definition.view {
                config.totals = !config.totals;
            }
        }) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn toggle_heatmap(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.session.working.as_ref().map(|view| &view.view),
            Some(ViewKind::Query(config)) if !config.group_by.is_empty()
        ) {
            self.error = Some("The heat map applies to queries with a group-by".into());
            cx.notify();
            return;
        }
        if let Err(error) = self.session.edit_definition(|definition| {
            if let ViewKind::Query(config) = &mut definition.view {
                config.heatmap = !config.heatmap;
            }
        }) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    /// Shades a query's measure columns when its heat map is on. Each
    /// column's range covers the whole query result, not only the rows a
    /// search leaves, so colors stay put while searching.
    fn refresh_heatmap(&mut self, cx: &mut Context<Self>) {
        self.heat_revision += 1;
        let config = match self.session.working.as_ref().map(|view| &view.view) {
            Some(ViewKind::Query(config)) if config.heatmap => Some(config.clone()),
            _ => None,
        };
        let frame = self.query_cache.as_ref().map(|cache| cache.frame.clone());
        let (Some(config), Some(frame)) = (config, frame) else {
            self.heat_ranges = None;
            self.grid
                .update(cx, |grid, cx| grid.set_heat_ranges(Default::default(), cx));
            return;
        };
        let store = std::sync::Arc::downgrade(&frame.store);
        if let Some((cached, ranges)) = &self.heat_ranges
            && cached.ptr_eq(&store)
        {
            let ranges = ranges.clone();
            self.grid
                .update(cx, |grid, cx| grid.set_heat_ranges(ranges, cx));
            return;
        }
        let columns = frame
            .columns
            .iter()
            .zip(frame.dtypes.iter())
            .map(|(column, dtype)| (column.0.clone(), dtype.clone()))
            .collect::<Vec<_>>();
        let groups = crate::domain::heatmap::scale_groups(&config, &columns);
        let revision = self.heat_revision;
        cx.spawn(async move |this, cx| {
            let ranges = cx
                .background_executor()
                .spawn(async move {
                    let store = frame
                        .store
                        .lock()
                        .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                    let mut ranges = std::collections::HashMap::new();
                    for group in groups {
                        let range = crate::domain::heatmap::combined(store.numeric_ranges(&group)?);
                        if let Some(range) = range {
                            ranges.extend(group.into_iter().map(|name| (name, range)));
                        }
                    }
                    Ok::<_, String>(ranges)
                })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view.heat_revision != revision {
                        return;
                    }
                    match ranges {
                        Ok(ranges) => {
                            view.heat_ranges = Some((store, ranges.clone()));
                            view.grid
                                .update(cx, |grid, cx| grid.set_heat_ranges(ranges, cx));
                        }
                        Err(error) => view.error = Some(format!("Heat map failed: {error}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows the grand total row under a query with totals, computing it from
    /// the source on a worker when the query, slicers, or filters changed.
    fn refresh_totals(&mut self, cx: &mut Context<Self>) {
        let key = match self.session.working.as_ref() {
            Some(definition) => match &definition.view {
                ViewKind::Query(config)
                    if config.totals
                        && !config.group_by.is_empty()
                        && !config.aggregations.is_empty() =>
                {
                    Some(TotalsKey {
                        source_revision: self.session.source.source_revision,
                        store: std::sync::Arc::as_ptr(&self.session.source.store) as usize,
                        config: QueryConfig {
                            result_sort: Vec::new(),
                            ..config.clone()
                        },
                        slicers: self
                            .slicer_stage
                            .as_ref()
                            .map(|stage| stage.effective())
                            .unwrap_or_else(|| definition.slicers.clone()),
                        filters: definition.filters.clone(),
                    })
                }
                _ => None,
            },
            None => None,
        };
        self.totals_revision += 1;
        let Some(key) = key else {
            self.totals = None;
            self.grid.update(cx, |grid, cx| grid.set_footer(None, cx));
            return;
        };
        if self
            .totals
            .as_ref()
            .is_some_and(|(cached, _)| cached == &key)
        {
            self.apply_totals(cx);
            return;
        }
        let revision = self.totals_revision;
        let source = self.session.source.clone();
        let job = key.clone();
        cx.spawn(async move |this, cx| {
            let totals = cx
                .background_executor()
                .spawn(async move {
                    let store = source
                        .store
                        .lock()
                        .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                    crate::domain::pivot::execute_totals(
                        &store,
                        &job.config,
                        &job.slicers,
                        &job.filters,
                        &crate::domain::pivot::EvaluationClock::system(),
                    )
                })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view.totals_revision != revision {
                        return;
                    }
                    match totals {
                        Ok(totals) => {
                            view.totals = Some((key, totals));
                            view.apply_totals(cx);
                        }
                        Err(error) => view.error = Some(format!("Totals failed: {error}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn apply_totals(&mut self, cx: &mut Context<Self>) {
        let Some((_, values)) = &self.totals else {
            return;
        };
        let narrowed = !self.session.search_text.trim().is_empty()
            || self.query_cache.as_ref().is_some_and(|cache| {
                query_limit_reached(cache.frame.row_count(), self.query_limit)
            });
        let footer = crate::ui::grid::GridFooter {
            // Totals cover every group, including any search or the limit hides.
            label: if narrowed {
                "Total (all groups)".into()
            } else {
                "Total".into()
            },
            values: values.clone(),
        };
        self.grid
            .update(cx, |grid, cx| grid.set_footer(Some(footer), cx));
    }

    /// A query result that stopped at the query limit is missing rows, so an
    /// export reruns the query without the limit.
    fn unlimited_query_rerun(&self) -> Option<UnlimitedQuery> {
        let definition = self.session.working.as_ref()?;
        let ViewKind::Query(config) = &definition.view else {
            return None;
        };
        let cache = self.query_cache.as_ref()?;
        if !query_limit_reached(cache.frame.row_count(), self.query_limit) {
            return None;
        }
        let tokens = query::parse(&self.session.search_text).ok()?;
        Some(UnlimitedQuery {
            source: self.session.source.clone(),
            config: config.clone(),
            slicers: self
                .slicer_stage
                .as_ref()
                .map(|stage| stage.effective())
                .unwrap_or_else(|| definition.slicers.clone()),
            filters: definition.filters.clone(),
            tokens,
            sort: self.session.sort.clone(),
        })
    }

    fn hide_selected_columns(&mut self, cx: &mut Context<Self>) {
        let result = &self.session.result;
        let Some(selection) = self
            .grid
            .read(cx)
            .selection()
            .and_then(|selection| selection.clamped(1, result.column_count()))
        else {
            return;
        };
        let names = selection
            .rectangle()
            .1
            .map(|column| result.columns[column].0.clone())
            .collect::<Vec<_>>();
        self.set_columns_hidden(&names, true, cx);
    }

    /// Hides or shows table columns. Hidden columns leave the grid and plain
    /// search but keep their place in the column order.
    fn set_columns_hidden(&mut self, names: &[String], hidden: bool, cx: &mut Context<Self>) {
        if !self.is_table() {
            self.error = Some("Hiding columns applies to tables, not query or SQL results".into());
            cx.notify();
            return;
        }
        let mut config = self.session.effective_table_config();
        let mut changed = false;
        for name in names {
            changed |= table::set_hidden(&mut config, name, hidden);
        }
        if !changed {
            return;
        }
        if config
            .column_order
            .iter()
            .all(|name| config.hidden.contains(name))
        {
            self.error = Some("At least one column must stay visible".into());
            cx.notify();
            return;
        }
        if self.session.working.is_none() {
            let name = self.suggested_name();
            if let Err(error) = self.session.start_table_draft(name) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        if let Err(error) = self.session.set_table_layout(config) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.autosave_table_preferences(cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn open_column_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_table() {
            self.error = Some("Hiding columns applies to tables, not query or SQL results".into());
            cx.notify();
            return;
        }
        let source = self.session.source.clone();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = command_state.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let source = source.clone();
            dialog
                .title("Show or hide columns")
                .w(px(PALETTE_WIDTH))
                .margin_top(px(24.))
                .content(move |content, window, cx| {
                    let config = view.read(cx).session.effective_table_config();
                    let order = config.column_order.clone();
                    let confirm_order = order.clone();
                    let view = view.clone();
                    content.child(
                        Command::new(&state)
                            .max_h(px(
                                (window.bounds().size.height.as_f32() - 200.).clamp(100., 440.)
                            ))
                            .items(order.into_iter().map(|name| {
                                let hidden = config.hidden.contains(&name);
                                let dtype = source
                                    .columns
                                    .iter()
                                    .position(|column| column.0 == name)
                                    .map(|index| {
                                        crate::domain::formatting::type_label(&source.dtypes[index])
                                            .into_owned()
                                    })
                                    .unwrap_or_default();
                                CommandItem::new().label(name.clone()).child(move |_, _| {
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .gap_3()
                                        .child(div().flex_shrink_0().w(px(16.)).child(if hidden {
                                            ""
                                        } else {
                                            "✓"
                                        }))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .when(hidden, |this| this.opacity(0.5))
                                                .child(name.clone()),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_sm()
                                                .opacity(0.6)
                                                .child(dtype.clone()),
                                        )
                                })
                            }))
                            .placeholder("Find a column; Enter shows or hides it")
                            .on_confirm(move |index, window, cx| {
                                let Some(name) = confirm_order.get(index.row).cloned() else {
                                    return;
                                };
                                view.update(cx, |view, cx| {
                                    let hidden = view
                                        .session
                                        .effective_table_config()
                                        .hidden
                                        .contains(&name);
                                    view.set_columns_hidden(&[name], !hidden, cx);
                                });
                                // The list stays open; redraw its check marks.
                                window.refresh();
                            }),
                    )
                })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// SQL sets the column order, so freezing only moves the boundary: through
    /// the active column, or back to just left of a frozen one.
    fn toggle_sql_frozen(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.grid.read(cx).selection() else {
            return;
        };
        let Some(ViewKind::Sql(config)) = self.session.working.as_ref().map(|view| &view.view)
        else {
            return;
        };
        let columns = self.result_column_names();
        let frozen = table::frozen_prefix(&config.frozen_columns, &columns);
        let names = table::toggle_frozen_prefix(&columns, frozen, selection.active.column);
        self.set_sql_frozen(names, cx);
    }

    fn set_sql_frozen(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let result = self.session.edit_definition(|definition| {
            if let ViewKind::Sql(config) = &mut definition.view {
                config.frozen_columns = names;
            }
        });
        if let Err(error) = result {
            self.error = Some(error);
        } else {
            self.undo_history.record(self.session.view_state());
            self.sync_frozen_count(cx);
            self.flush_sql_auto_save();
        }
        cx.notify();
    }

    fn result_column_names(&self) -> Vec<String> {
        self.session
            .result
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect()
    }

    fn unfreeze_all(&mut self, cx: &mut Context<Self>) {
        if let Some(ViewKind::Sql(config)) = self.session.working.as_ref().map(|view| &view.view) {
            if !config.frozen_columns.is_empty() {
                self.set_sql_frozen(Vec::new(), cx);
            }
            return;
        }
        let Some(ViewKind::Table(_)) = self.session.working.as_ref().map(|view| &view.view) else {
            return;
        };
        if self.session.effective_table_config().frozen_count == 0
            && (!self.session.is_direct_table()
                || self.session.direct_table.frozen_columns.is_empty())
        {
            return;
        }
        if let Err(error) = self.session.edit_definition(|view| {
            if let ViewKind::Table(table) = &mut view.view {
                table.frozen_count = 0;
            }
        }) {
            self.error = Some(error);
        } else {
            if self.session.is_direct_table() {
                self.session.direct_table.frozen_columns.clear();
            }
            self.sync_frozen_count(cx);
            self.autosave_table_preferences(cx);
        }
        cx.notify();
    }

    fn focus_next_slicer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_slicers = self
            .session
            .working
            .as_ref()
            .is_some_and(|definition| !definition.slicers.is_empty());
        if !has_slicers
            || !self
                .slicer_sidebar
                .update(cx, |sidebar, cx| sidebar.focus_next_input(window, cx))
        {
            self.copy_status = Some("No slicers to focus. Use Add slicer to add one".into());
            cx.notify();
        }
    }

    fn focus_grid_from_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        let search_focus = self.search_input.read(cx).focus_handle(cx);
        if window.focused(cx).as_ref() == Some(&search_focus) {
            let grid_focus = self.grid.read(cx).focus_handle(cx);
            window.focus(&grid_focus, cx);
            cx.stop_propagation();
        }
    }

    fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = command_state.clone();
        let commands = active_commands()
            .into_iter()
            .filter(|c| {
                !self.session.is_sql()
                    || !matches!(
                        c.id,
                        CommandId::Group
                            | CommandId::Pivot
                            | CommandId::Aggregation
                            | CommandId::OrderBy
                    ) && (c.id != CommandId::AddSlicer || self.sql_result_complete)
            })
            .collect::<Vec<_>>();
        let keymap = self.keymap.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let commands = commands.clone();
            let keymap = keymap.clone();
            dialog
                .title("Commands")
                .w(px(PALETTE_WIDTH))
                .margin_top(px(24.))
                .content(move |content, window, cx| {
                    let view = view.clone();
                    let query = state.read(cx).query(cx);
                    let commands = matching_commands(&commands, &keymap, &query);
                    content.child(
                        Command::new(&state)
                            .max_h(px(
                                (window.bounds().size.height.as_f32() - 200.).clamp(100., 440.)
                            ))
                            .filterable(false)
                            .on_query(|_, window, _| window.refresh())
                            .items(commands.iter().map(|(command, label)| {
                                CommandItem::new()
                                    .label(label.clone())
                                    .keywords([command.key])
                            }))
                            .placeholder("Find a command")
                            .on_confirm(move |index, window, cx| {
                                if let Some((command, _)) = commands.get(index.row) {
                                    window.close_dialog(cx);
                                    view.update(cx, |view, cx| {
                                        view.dispatch_command(command.id, window, cx)
                                    });
                                }
                            }),
                    )
                })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn open_theme_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ThemeEditor> {
        let form = cx.new(|cx| ThemeEditor::new(self.theme.clone(), window, cx));
        let dialog_form = form.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let content_form = dialog_form.clone();
            let reset_form = dialog_form.clone();
            let save_form = dialog_form.clone();
            let save_view = view.clone();
            dialog
                .title("Theme Editor")
                .w(px(600.))
                .content(move |content, window, _| {
                    let height = (window.bounds().size.height.as_f32() - 210.0).clamp(100.0, 650.0);
                    content.child(
                        div()
                            .max_h(px(height))
                            .overflow_y_scrollbar()
                            .child(content_form.clone()),
                    )
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("theme-reset")
                                .label("Reset to defaults")
                                .on_click(move |_, window, cx| {
                                    reset_form.update(cx, |form, cx| form.reset(window, cx));
                                }),
                        )
                        .child(DialogClose::new().trigger(|button| button.label("Cancel")))
                        .child(DialogAction::new().child(Button::new("theme-save").label("Save"))),
                )
                .on_ok(move |_, _, cx| {
                    let theme = match save_form.read(cx).build(cx) {
                        Ok(theme) => theme,
                        Err(error) => {
                            save_form.update(cx, |form, cx| form.set_error(error, cx));
                            return false;
                        }
                    };
                    let result = save_view.update(cx, |view, cx| view.save_theme(theme, cx));
                    match result {
                        Ok(()) => true,
                        Err(error) => {
                            save_form.update(cx, |form, cx| {
                                form.set_error(format!("Could not save theme.json: {error}"), cx)
                            });
                            false
                        }
                    }
                })
        });
        form.update(cx, |form, cx| form.focus_first(window, cx));
        form
    }

    fn save_theme(&mut self, theme: ThemeSettings, cx: &mut Context<Self>) -> std::io::Result<()> {
        self.repository.save_theme(&theme)?;
        self.theme = theme.clone();
        self.grid
            .update(cx, |grid, cx| grid.set_theme(theme.clone(), cx));
        let profile = self.repository.paths.config.clone();
        cx.update_global(|runtime: &mut ThemeRuntime, _| {
            runtime.profiles.insert(profile.clone(), theme);
            *runtime.revisions.entry(profile).or_default() += 1;
        });
        cx.notify();
        Ok(())
    }

    fn open_keymap_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.keymap_editor.is_some() {
            return;
        }
        let editor = cx.new(|cx| KeymapEditor::new(self.keymap.clone(), window, cx));
        let search_focus = editor.read(cx).search.read(cx).focus_handle(cx);
        window.focus(&search_focus, cx);
        let event_subscription =
            cx.subscribe(
                &editor,
                |view, _, event: &KeymapEditorEvent, cx| match event {
                    KeymapEditorEvent::Changed(keymap) => {
                        match view.repository.save_keymap(keymap) {
                            Ok(()) => {
                                view.keymap = keymap.clone();
                                let profile = view.keymap_profile.clone();
                                let keymap = keymap.clone();
                                cx.update_global(|runtime: &mut KeymapRuntime, _| {
                                    runtime.profiles.insert(profile, keymap);
                                });
                            }
                            Err(error) => {
                                view.error = Some(format!("Could not save keymap.json: {error}"));
                            }
                        }
                        cx.notify();
                    }
                    KeymapEditorEvent::Back => {
                        view.keymap_editor = None;
                        view.restore_focus = Some(view.grid.read(cx).focus_handle(cx));
                        cx.notify();
                    }
                },
            );
        let search = editor.read(cx).search.clone();
        let editor_for_search = editor.clone();
        let query_subscription = cx.subscribe(&search, move |_, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                update_query(&editor_for_search, input.read(cx).value().to_string(), cx);
            }
        });
        self._subscriptions.push(event_subscription);
        self._subscriptions.push(query_subscription);
        self.restore_focus = None;
        self.keymap_editor = Some(editor);
        cx.notify();
    }

    fn open_file_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_revision += 1;
        let revision = self.file_revision;
        self.file_loading = true;
        self.file_groups = std::array::from_fn(|_| Vec::new());
        let repository = self.repository.clone();
        let database = self.database.clone();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = command_state.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            dialog
                .title("Files and views")
                .w(px(PALETTE_WIDTH))
                .margin_top(px(24.))
                .content(move |content, window, cx| {
                    let app = view.read(cx);
                    let query = state.read(cx).query(cx).to_lowercase();
                    let terms = query.split_whitespace().collect::<Vec<_>>();
                    let groups = app.file_groups.each_ref().map(|group| {
                        group
                            .iter()
                            .filter(|entry| matches_terms(&entry.search_text(), &terms))
                            .cloned()
                            .collect::<Vec<_>>()
                    });
                    let loading = app.file_loading;
                    let mut palette_groups = vec![groups[0].clone(), groups[1].clone()];
                    let mut sources_by_parent = std::collections::BTreeMap::new();
                    for entry in &groups[2] {
                        let Some(folder) = entry.source_parent() else {
                            continue;
                        };
                        let key = folder.to_lowercase();
                        let source_group = sources_by_parent
                            .entry(key)
                            .or_insert_with(|| (folder, Vec::new()));
                        source_group.1.push(entry.clone());
                    }
                    let mut command = Command::new(&state)
                        .max_h(px(
                            (window.bounds().size.height.as_f32() - 200.).clamp(100., 440.)
                        ))
                        .filterable(false)
                        .on_query(|_, window, _| window.refresh())
                        .group(
                            CommandGroup::new()
                                .label("Table views")
                                .items(groups[0].iter().map(file_command_item)),
                        )
                        .group(
                            CommandGroup::new()
                                .label("Query views")
                                .items(groups[1].iter().map(file_command_item)),
                        );
                    if !groups[3].is_empty() {
                        palette_groups.push(groups[3].clone());
                        command = command.group(
                            CommandGroup::new()
                                .label("SQL views")
                                .items(groups[3].iter().map(file_command_item)),
                        );
                    }
                    if !groups[4].is_empty() {
                        palette_groups.push(groups[4].clone());
                        command = command.group(
                            CommandGroup::new()
                                .label("Database views")
                                .items(groups[4].iter().map(file_command_item)),
                        );
                    }
                    for (_, (folder, entries)) in sources_by_parent {
                        palette_groups.push(entries.clone());
                        command = command.group(
                            CommandGroup::new()
                                .label(format!("Sources · {folder}"))
                                .items(entries.iter().map(file_command_item)),
                        );
                    }
                    let action_groups = palette_groups.clone();
                    let footer_groups = palette_groups.clone();
                    let confirm_groups = palette_groups;
                    let view = view.clone();
                    content.child(
                        div()
                            .capture_action(
                                |_: &gpui_kit::component::input::DeleteToNextWordEnd,
                                 window,
                                 cx| {
                                    window.dispatch_action(Box::new(ArchiveSelectedView), cx);
                                    cx.stop_propagation();
                                },
                            )
                            .on_action({
                                let state = state.clone();
                                let view = view.clone();
                                move |_: &ArchiveSelectedView, window, cx| {
                                    let selected = state.read(cx).selected_index();
                                    let entry = selected.and_then(|index| {
                                        action_groups
                                            .get(index.section)
                                            .and_then(|group| group.get(index.row))
                                            .cloned()
                                    });
                                    if let Some(FileEntry::View(definition)) =
                                        entry.filter(FileEntry::is_saved_view)
                                    {
                                        window.close_dialog(cx);
                                        view.update(cx, |view, cx| {
                                            view.archive_view(&definition, window, cx)
                                        });
                                    }
                                }
                            })
                            .child(
                                command
                                    .placeholder("Find a file or view")
                                    .empty(move |_, _, _| {
                                        div().child(if loading {
                                            "Scanning files…"
                                        } else {
                                            "No files or views"
                                        })
                                    })
                                    .footer({
                                        let view = view.clone();
                                        let state = state.clone();
                                        move |command, _, _| {
                                            let archive_enabled = command
                                                .selected_index()
                                                .and_then(|index| {
                                                    footer_groups
                                                        .get(index.section)
                                                        .and_then(|group| group.get(index.row))
                                                })
                                                .is_some_and(|entry| entry.is_saved_view());
                                            let folder_view = view.clone();
                                            let archive_view = view.clone();
                                            let archive_state = state.clone();
                                            let recovery_view = view.clone();
                                            let archive_groups = footer_groups.clone();
                                            div()
                                                .flex()
                                                .flex_wrap()
                                                .gap_2()
                                                .child(
                                                    Button::new("manage-folders")
                                                        .label("Folders")
                                                        .on_click(move |_, window, cx| {
                                                            window.close_dialog(cx);
                                                            folder_view.update(cx, |view, cx| {
                                                                view.open_folder_palette(window, cx)
                                                            });
                                                        }),
                                                )
                                                .child(
                                                    Button::new("archive-view")
                                                        .label("Archive · Ctrl+Delete")
                                                        .disabled(!archive_enabled)
                                                        .on_click(move |_, window, cx| {
                                                            let selected = archive_state
                                                                .read(cx)
                                                                .selected_index();
                                                            window.close_dialog(cx);
                                                            archive_view.update(cx, |view, cx| {
                                                                let entry = selected
                                                                    .and_then(|index| {
                                                                        archive_groups
                                                                            .get(index.section)
                                                                            .and_then(|group| {
                                                                                group.get(index.row)
                                                                            })
                                                                    })
                                                                    .cloned();
                                                                if let Some(FileEntry::View(
                                                                    definition,
                                                                )) = entry.filter(
                                                                    FileEntry::is_saved_view,
                                                                ) {
                                                                    view.archive_view(
                                                                        &definition,
                                                                        window,
                                                                        cx,
                                                                    );
                                                                }
                                                            });
                                                        }),
                                                )
                                                .child(
                                                    Button::new("recover-view")
                                                        .label("Recover")
                                                        .on_click(move |_, window, cx| {
                                                            window.close_dialog(cx);
                                                            recovery_view.update(cx, |view, cx| {
                                                                view.open_recovery_palette(
                                                                    window, cx,
                                                                )
                                                            });
                                                        }),
                                                )
                                        }
                                    })
                                    .on_confirm(move |index, window, cx| {
                                        if let Some(entry) = confirm_groups
                                            .get(index.section)
                                            .and_then(|group| group.get(index.row))
                                            .cloned()
                                        {
                                            let carry_search = window.modifiers().shift;
                                            let pivot_draft = window.modifiers().control;
                                            window.close_dialog(cx);
                                            let handle = window.window_handle();
                                            view.update(cx, |view, cx| {
                                                view.navigate_to_file(
                                                    entry,
                                                    carry_search,
                                                    pivot_draft,
                                                    false,
                                                    handle,
                                                    cx,
                                                )
                                            });
                                        }
                                    }),
                            ),
                    )
                })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
        cx.spawn(async move |this, cx| {
            let entries = cx
                .background_executor()
                .spawn(async move {
                    let state =
                        ApplicationState::load(repository).map_err(|error| error.to_string())?;
                    let mut groups: [Vec<FileEntry>; 5] = std::array::from_fn(|_| Vec::new());
                    for view in state.views {
                        match view.view {
                            ViewKind::Table(_) => groups[0].push(FileEntry::View(view)),
                            ViewKind::Query(_) => groups[1].push(FileEntry::View(view)),
                            ViewKind::Sql(_) => groups[3].push(FileEntry::View(view)),
                        }
                    }
                    let folders = state
                        .settings
                        .watched_folders
                        .iter()
                        .map(PathBuf::from)
                        .collect::<Vec<_>>();
                    let mut sources = scan_sources(&folders);
                    sources.extend(state.history.recent_sources.into_iter().map(PathBuf::from));
                    let mut seen = std::collections::HashSet::new();
                    sources.retain(|path| seen.insert(path.to_string_lossy().to_lowercase()));
                    sources.sort_by_key(|path| path.to_string_lossy().to_lowercase());
                    groups[2] = sources.into_iter().map(FileEntry::Source).collect();
                    groups[4] = database
                        .views()?
                        .into_iter()
                        .map(DocumentDefinition::draft_database_view)
                        .map(FileEntry::View)
                        .collect();
                    Ok::<_, String>(groups)
                })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view.file_revision != revision {
                        return;
                    }
                    view.file_loading = false;
                    match entries {
                        Ok(groups) => view.file_groups = groups,
                        Err(error) => view.error = Some(format!("File scan failed: {error}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn archive_view(
        &mut self,
        definition: &DocumentDefinition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                        self.session.navigate_to_view(
                            Ok(source),
                            DocumentDefinition::draft_database_view(database_view.clone()),
                        )
                    } else {
                        self.session.navigate_to_raw(Ok(source), path)
                    };
                    if let Err(error) = destination {
                        self.error = Some(error);
                    } else {
                        if let Some(path) = &self.session.source_path {
                            match self.table_writer.load(std::path::Path::new(path)) {
                                Ok(preferences) => {
                                    self.session.apply_direct_preferences(preferences)
                                }
                                Err(error) => {
                                    self.table_preferences_error =
                                        Some(format!("Cannot read table preferences: {error}"))
                                }
                            }
                        }
                        self.computation.invalidate(self.session.token());
                        self.search_input
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        self.grid.update(cx, |grid, cx| {
                            grid.set_sort(Vec::new(), cx);
                            grid.replace_result(self.session.result.clone(), cx);
                            grid.set_frozen_count(0, cx);
                        });
                        if let Err(error) = self.record_opened() {
                            eprintln!("history update failed: {error}");
                        }
                        self.schedule_search(String::new(), cx);
                    }
                }
                self.file_groups[0].retain(
                    |entry| !matches!(entry, FileEntry::View(view) if view.id == definition.id),
                );
                self.file_groups[1].retain(
                    |entry| !matches!(entry, FileEntry::View(view) if view.id == definition.id),
                );
            }
            Err(error) => self.error = Some(format!("Archive failed: {error}")),
        }
        cx.notify();
    }

    fn open_recovery_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (archives, issues) = self.repository.load_archives();
        if let Some(issue) = issues.first() {
            self.error = Some(format!(
                "Cannot read archive {}: {}",
                issue.path.display(),
                issue.message
            ));
        }
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let archives = archives.clone();
            dialog
                .title("Recover deleted views")
                .content(move |content, _, _| {
                    let view = view.clone();
                    let archives = archives.clone();
                    content.child(
                        Command::new(&state)
                            .items(archives.iter().map(|(_, archive)| {
                                CommandItem::new().label(format!(
                                    "{} · {} · {}",
                                    archive.original_name,
                                    archive.original_kind,
                                    local_archive_time(archive.deleted_at_unix_ms)
                                ))
                            }))
                            .placeholder("Find a deleted view")
                            .empty(|_, _, _| div().child("No deleted views"))
                            .on_confirm(move |index, window, cx| {
                                if let Some((path, _)) = archives.get(index.row) {
                                    window.close_dialog(cx);
                                    view.update(cx, |view, cx| {
                                        match view.repository.recover(path) {
                                            Ok(_) => view.error = None,
                                            Err(error) => {
                                                view.error =
                                                    Some(format!("Recovery failed: {error}"))
                                            }
                                        }
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                })
        });
    }

    fn open_folder_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = match ApplicationState::load(self.repository.clone()) {
            Ok(state) => state,
            Err(error) => {
                self.error = Some(format!("Cannot load folders: {error}"));
                cx.notify();
                return;
            }
        };
        let folders = state.settings.watched_folders;
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let folders = folders.clone();
            dialog
                .title("Watched folders")
                .content(move |content, _, _| {
                    let view = view.clone();
                    let folders = folders.clone();
                    content.child(
                        Command::new(&state)
                            .items(
                                std::iter::once(CommandItem::new().label("Add folder…")).chain(
                                    folders.iter().map(|folder| {
                                        CommandItem::new().label(format!("Remove · {folder}"))
                                    }),
                                ),
                            )
                            .placeholder("Find a watched folder")
                            .on_confirm(move |index, window, cx| {
                                window.close_dialog(cx);
                                if index.row == 0 {
                                    view.update(cx, |view, cx| view.add_folder_with_picker(cx));
                                } else if let Some(folder) = folders.get(index.row - 1) {
                                    view.update(cx, |view, cx| view.remove_folder(folder, cx));
                                }
                            }),
                    )
                })
        });
    }

    fn add_folder_with_picker(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add watched folder".into()),
        });
        cx.spawn(async move |this, cx| match chosen.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.first()
                    && let Some(this) = this.upgrade()
                {
                    this.update(cx, |view, cx| {
                        let result = ApplicationState::load(view.repository.clone())
                            .and_then(|mut state| state.add_watched_folder(path));
                        view.error = result
                            .err()
                            .map(|error| format!("Add folder failed: {error}"));
                        cx.notify();
                    });
                }
            }
            Ok(Ok(None)) => {}
            result => {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                        view.error = Some(format!("Folder picker failed: {result:?}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    fn remove_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        let result = ApplicationState::load(self.repository.clone())
            .and_then(|mut state| state.remove_watched_folder(folder));
        self.error = result
            .err()
            .map(|error| format!("Remove folder failed: {error}"));
        cx.notify();
    }

    fn favorite(&mut self, slot: usize, assign: bool, window: &mut Window, cx: &mut Context<Self>) {
        let result = ApplicationState::load(self.repository.clone());
        match result {
            Ok(mut state) if assign => {
                let target = match &self.session.identity {
                    SessionIdentity::SavedTable(id)
                    | SessionIdentity::SavedQuery(id)
                    | SessionIdentity::SavedSql(id) => Some(FavoriteTarget::View {
                        view_id: id.clone(),
                    }),
                    SessionIdentity::DraftQuery | SessionIdentity::DraftSql => {
                        self.error =
                            Some("Save this view before assigning it as a favorite".into());
                        cx.notify();
                        return;
                    }
                    _ => self.session.source_path.clone().map(FavoriteTarget::Source),
                };
                let Some(target) = target else {
                    self.error =
                        Some("A source file or saved view is required to assign a favorite".into());
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
                        FavoriteTarget::Source(path) => {
                            Some(FileEntry::Source(PathBuf::from(path)))
                        }
                        FavoriteTarget::View { view_id } => state
                            .views
                            .iter()
                            .find(|view| &view.id == view_id)
                            .cloned()
                            .map(FileEntry::View),
                    };
                    if let Some(entry) = entry {
                        self.navigate_to_file(
                            entry,
                            false,
                            false,
                            true,
                            window.window_handle(),
                            cx,
                        );
                    } else {
                        self.error = Some(format!(
                            "Favorite {} refers to a view that is no longer saved",
                            slot + 1
                        ));
                    }
                }
                None => self.error = Some(format!("Favorite {} is empty", slot + 1)),
            },
            Err(error) => self.error = Some(format!("Favorite failed: {error}")),
        }
        cx.notify();
    }

    fn open_favorites_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (favorites, views) = match ApplicationState::load(self.repository.clone()) {
            Ok(state) => (state.settings.favorites, state.views),
            Err(error) => {
                self.error = Some(format!("Cannot load favorites: {error}"));
                cx.notify();
                return;
            }
        };
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let favorites = favorites.clone();
            let views = views.clone();
            dialog.title("Favorites").content(move |content, _, _| {
                let view = view.clone();
                content.child(
                    Command::new(&state)
                        .items(favorites.iter().enumerate().map(|(index, target)| {
                            let (label, keyword) = match target {
                                Some(FavoriteTarget::Source(path)) => {
                                    (short_source(&PathBuf::from(path)), path.clone())
                                }
                                Some(FavoriteTarget::View { view_id }) => views
                                    .iter()
                                    .find(|view| &view.id == view_id)
                                    .map(|view| (view.name.clone(), view.source_path.clone()))
                                    .unwrap_or_else(|| {
                                        ("Missing saved view".into(), view_id.clone())
                                    }),
                                None => ("Empty".into(), String::new()),
                            };
                            let mut item =
                                CommandItem::new().label(format!("{} · {label}", index + 1));
                            item = item.keywords([keyword]);
                            item
                        }))
                        .placeholder("Find a favorite")
                        .max_h(px(400.))
                        .footer(|_, _, _| {
                            div().child("Ctrl+Shift+1…9 assigns the current source or saved view")
                        })
                        .on_confirm(move |index, window, cx| {
                            if index.row < 9 {
                                window.close_dialog(cx);
                                view.update(cx, |view, cx| {
                                    view.favorite(index.row, false, window, cx)
                                });
                            }
                        }),
                )
            })
        });
    }

    fn open_native_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open source".into()),
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| match chosen.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next()
                    && let Some(this) = this.upgrade()
                {
                    this.update(cx, |view, cx| {
                        view.navigate_to_file(
                            FileEntry::Source(path),
                            false,
                            false,
                            true,
                            handle,
                            cx,
                        )
                    });
                }
            }
            Ok(Ok(None)) => {}
            result => {
                if let Some(this) = this.upgrade() {
                    this.update(cx, |view, cx| {
                        view.error = Some(format!("Open file failed: {result:?}"));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    /// Opens what the clipboard holds: copied files open as sources, and text
    /// (such as cells copied from a spreadsheet) is saved as a CSV or TSV file
    /// under the data folder and opened as a table.
    fn paste_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            self.error = Some("The clipboard is empty".into());
            cx.notify();
            return;
        };
        let paths = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => Some(paths.paths().to_vec()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            self.open_paths(paths, window, cx);
            return;
        }
        let Some(text) = item.text() else {
            self.error = Some("The clipboard has no text to open as a table".into());
            cx.notify();
            return;
        };
        let directory = self.repository.paths.data.join("pasted");
        let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string();
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    crate::infrastructure::pasted_table::save(&directory, &text, &stamp)
                })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| match saved {
                    Ok(path) => {
                        view.copy_status =
                            Some(format!("Pasted table saved to {}", path.display()));
                        view.navigate_to_file(
                            FileEntry::Source(path),
                            false,
                            false,
                            true,
                            handle,
                            cx,
                        );
                    }
                    Err(error) => {
                        view.error = Some(error);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    /// Opens dropped or pasted files: the first in this window, and each
    /// other one in a window of its own.
    fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let (databases, paths): (Vec<_>, Vec<_>) = paths.into_iter().partition(|path| {
            crate::infrastructure::source_database::database_file_kind(path).is_some()
        });
        if !databases.is_empty() {
            self.attach_databases(databases, window, cx);
        }
        let mut paths = paths.into_iter();
        let Some(first) = paths.next() else {
            return;
        };
        if first.is_dir() {
            self.error = Some(format!(
                "{} is a folder; use Add folder to list its files",
                first.display()
            ));
            cx.notify();
            return;
        }
        self.navigate_to_file(
            FileEntry::Source(first),
            false,
            false,
            true,
            window.window_handle(),
            cx,
        );
        for path in paths.filter(|path| !path.is_dir()) {
            self.open_blank_window(Some(path), cx);
        }
    }

    fn navigate_to_file(
        &mut self,
        entry: FileEntry,
        carry_search: bool,
        pivot_draft: bool,
        raw_source: bool,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.navigate_to_location(
            entry,
            carry_search,
            pivot_draft,
            raw_source,
            None,
            None,
            window_handle,
            cx,
        );
    }

    fn remember_navigation_current(&mut self) {
        if let Some(entry) = self.session.navigation_entry() {
            self.navigation_history.remember_current(entry);
        }
    }

    /// Starts undo over from the current state, as at a new location.
    fn reset_undo_history(&mut self) {
        self.undo_history.reset(Some(self.session.view_state()));
    }

    /// Steps the view configuration back, or forward again with `redo`.
    fn undo_view_change(&mut self, redo: bool, cx: &mut Context<Self>) {
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
            self.copy_status = Some(
                if redo {
                    "Nothing to redo"
                } else {
                    "Nothing to undo"
                }
                .into(),
            );
            cx.notify();
            return;
        };
        self.session.restore_view_state(&state);
        self.query_cache = None;
        self.table_query = None;
        let sort = self.session.sort.clone();
        self.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
        self.sync_frozen_count(cx);
        self.autosave_table_preferences(cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn commit_navigation_history(&mut self) {
        self.reset_undo_history();
        if let Some(entry) = self.session.navigation_entry() {
            if let Some((index, _)) = self.navigation_target.take() {
                self.navigation_history.commit(index, entry);
            } else {
                self.navigation_history.push(entry);
            }
        }
    }

    fn navigate_history(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((index, target)) = self.navigation_history.target(forward) else {
            return;
        };
        self.rollback_navigation(cx);
        let entry = if matches!(
            target.identity,
            SessionIdentity::RawSource | SessionIdentity::DraftTable
        ) {
            FileEntry::Source(PathBuf::from(
                target.source_path.as_ref().expect("source location"),
            ))
        } else if let Some(definition) = &target.definition {
            FileEntry::View(definition.clone())
        } else {
            FileEntry::Source(PathBuf::from(
                target.source_path.as_ref().expect("source location"),
            ))
        };
        self.navigate_to_location(
            entry,
            false,
            false,
            true,
            Some((index, target)),
            None,
            window.window_handle(),
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn navigate_to_location(
        &mut self,
        entry: FileEntry,
        carry_search: bool,
        pivot_draft: bool,
        raw_source: bool,
        target: Option<(usize, NavigationEntry)>,
        drill: Option<DrillDown>,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
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
            if let Some(panel) = &self.sql_panel {
                panel.update(cx, |panel, cx| {
                    panel.busy = false;
                    panel.status = "Stopped".into();
                    cx.notify();
                });
            }
        }
        let startup_script = matches!(entry, FileEntry::StartupSql);
        let sql_definition = match &entry {
            FileEntry::View(definition) if matches!(definition.view, ViewKind::Sql(_)) => {
                Some(definition.clone())
            }
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
            cx.spawn(async move |this, cx| {
                let source = cx
                    .background_executor()
                    .spawn(async move {
                        database.initialize()?;
                        let definition = if startup_script {
                            repository
                                .load_startup_sql()
                                .map_err(|error| format!("Cannot open startup.sql: {error}"))?
                        } else {
                            sql_definition.expect("SQL view destination")
                        };
                        crate::infrastructure::duckdb_store::DuckDbStore::empty()
                            .map(|s| (FrameSnapshot::new(s, 1), definition))
                    })
                    .await;
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        if !view.session.accepts(token) {
                            return;
                        }
                        view.loading = false;
                        match source.and_then(|(source, definition)| {
                            view.session.navigate_to_view(Ok(source), definition)
                        }) {
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
                                view.grid.update(cx, |grid, cx| {
                                    grid.replace_result(view.session.result.clone(), cx);
                                    grid.set_sort(view.session.sort.clone(), cx);
                                    grid.set_keymap(view.keymap.clone(), KeyContext::QueryGrid);
                                });
                                view.grid
                                    .update(cx, |grid, cx| grid.set_frozen_count(0, cx));
                                view.slicer_sidebar
                                    .update(cx, |sidebar, cx| sidebar.set_cards(Vec::new(), cx));
                                let search = view.session.search_text.clone();
                                view.search_input
                                    .update(cx, |input, cx| input.set_value(search, window, cx));
                                view.ensure_sql_panel(window, cx);
                                if startup_script {
                                    if let Some(panel) = &view.sql_panel {
                                        panel.update(cx, |panel, cx| panel.focus(window, cx));
                                    }
                                } else {
                                    window.focus(&view.grid.read(cx).focus_handle(cx), cx);
                                }
                                view.commit_navigation_history();
                                let _ = view.record_opened();
                            }
                            Err(error) => {
                                view.navigation_target = None;
                                view.error = Some(error);
                            }
                        }
                        cx.notify();
                    });
                });
            })
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
        let existing = self.session.source.clone();
        let table_writer = self.table_writer.clone();
        let direct_destination = matches!(entry, FileEntry::Source(_));
        cx.spawn(async move |this, cx| {
            let read_path = path.clone();
            let read = cx
                .background_executor()
                .spawn(async move {
                    let _span = crate::infrastructure::performance::Span::new("source_check");
                    let source = if let Some(view) = &database_view {
                        FrameSnapshot::new(database.load_view(view)?, existing.source_revision + 1)
                    } else { match database.load_reusing(
                        &read_path,
                        LoadMode::Check,
                        existing.persistent_identity.as_ref(),
                    )? {
                        SourceLoad::Reused => existing,
                        SourceLoad::Loaded(store) => {
                            FrameSnapshot::new(*store, existing.source_revision + 1)
                        }
                    }};
                    let preferences = direct_destination.then(|| table_writer.load(&read_path));
                    Ok::<_, String>((source, source_modified(Some(&read_path.to_string_lossy())), preferences))
                })
                .await;
            if let Some(this) = this.upgrade() {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    this.update(cx, |view, cx| {
                        if !view.session.accepts(token) {
                            return;
                        }
                        view.loading = false;
                        let _transition =
                            crate::infrastructure::performance::Span::new("session_transition");
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
                                    window_handle,
                                });
                                let destination = match entry {
                                    FileEntry::View(definition) => {
                                        view.session.navigate_to_view(Ok(source), definition)
                                    }
                                    FileEntry::StartupSql => unreachable!("startup SQL uses SQL navigation"),
                                    FileEntry::Source(_) => view
                                        .session
                                        .navigate_to_raw(
                                            Ok(source),
                                            Some(path.to_string_lossy().into_owned()),
                                        )
                                        .and_then(|()| {
                                            if let Some(preferences) = preferences {
                                                match preferences {
                                                    Ok(preferences) => view.session.apply_direct_preferences(preferences),
                                                    Err(error) => view.table_preferences_error = Some(format!("Cannot read table preferences: {error}")),
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
                                view.search_input.update(cx, |input, cx| {
                                    input.set_value(search.clone(), window, cx)
                                });
                                let sort = view.session.sort.clone();
                                view.grid.update(cx, |grid, cx| grid.set_sort(sort, cx));
                                view.schedule_search(search, cx);
                                // The picker may have restored focus to the SQL editor we just removed.
                                window.focus(&view.grid.read(cx).focus_handle(cx), cx);
                            }
                            Err(error) => {
                                view.rollback_navigation(cx);
                                view.error = Some(error);
                            }
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn apply_drill_down(&mut self, drill: DrillDown) {
        if self.session.working.is_none() {
            let _ = self.session.start_table_draft(self.suggested_name());
        }
        let result = self.session.edit_definition(|definition| {
            for slicer in drill.slicers {
                match definition
                    .slicers
                    .iter_mut()
                    .find(|card| card.column == slicer.column)
                {
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

    fn add_slicer(&mut self, column: String, cx: &mut Context<Self>) {
        self.add_slicer_card(
            SlicerDefinition {
                sql: false,
                condition: None,
                column,
                selected: Vec::new(),
            },
            cx,
        );
    }

    /// Adds a condition card, which filters `column` by typed text instead of values.
    fn add_condition_slicer(&mut self, column: String, cx: &mut Context<Self>) {
        self.add_slicer_card(
            SlicerDefinition {
                sql: false,
                condition: Some(String::new()),
                column,
                selected: Vec::new(),
            },
            cx,
        );
    }

    /// Adds `card` unless one for the same column, or another SQL card, exists.
    fn add_slicer_card(&mut self, card: SlicerDefinition, cx: &mut Context<Self>) {
        if self.session.working.as_ref().is_some_and(|definition| {
            definition
                .slicers
                .iter()
                .any(|other| other.sql == card.sql && other.column == card.column)
        }) {
            return;
        }
        self.ensure_working_view();
        let _ = self
            .session
            .edit_definition(|definition| definition.slicers.push(card));
        self.autosave_table_preferences(cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    /// Lists the SQL card and condition cards at once, since they need only the
    /// schema; value cards follow once discovery finds few enough values.
    fn update_slicer_picker(&self, error: Option<String>, cx: &mut Context<Self>) {
        let Some(picker) = &self.slicer_picker else {
            return;
        };
        let stage = self
            .slicer_stage
            .as_ref()
            .filter(|stage| stage.matches_source(&self.session.source));
        // Only a catalog of every column lists all the value cards.
        let discovered = stage.filter(|stage| stage.catalog.examined.is_none());
        let definitions = self
            .session
            .working
            .as_ref()
            .map(|definition| definition.slicers.as_slice())
            .unwrap_or_default();
        let free = |column: &String| {
            !definitions
                .iter()
                .any(|card| !card.sql && &card.column == column)
        };
        // The SQL card comes first. Value cards need few enough distinct values;
        // any column takes a condition.
        let sql = (!definitions.iter().any(|card| card.sql)).then(|| SlicerChoice {
            column: String::new(),
            kind: SlicerKind::Sql,
        });
        let columns = sql
            .into_iter()
            .chain(
                discovered
                    .iter()
                    .flat_map(|stage| stage.catalog.columns.iter())
                    .map(|(column, _, _)| column)
                    .filter(|column| free(column))
                    .map(|column| SlicerChoice {
                        column: column.clone(),
                        kind: SlicerKind::Values,
                    }),
            )
            .chain(
                stage
                    .map_or(&self.session.source, |stage| &stage.source)
                    .schema
                    .iter()
                    .filter(|column| free(&column.name))
                    .map(|column| SlicerChoice {
                        column: column.name.clone(),
                        kind: SlicerKind::Condition,
                    }),
            )
            .collect();
        let loading = discovered.is_none() && error.is_none();
        picker.update(cx, |picker, cx| picker.set_columns(columns, loading, error, cx));
    }

    /// Stops discovering every column's values; value cards still discover
    /// their own columns.
    fn close_slicer_picker(&mut self) {
        self.slicer_picker = None;
        if let Some(discovery) = self.slicer_discovery.take() {
            discovery.cancel();
        }
    }

    fn open_slicer_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.new(|cx| SlicerPicker::new(window, cx));
        self._subscriptions
            .push(cx.subscribe(&picker, |view, _, choice: &SlicerChoice, cx| {
                match choice.kind {
                    SlicerKind::Values => view.add_slicer(choice.column.clone(), cx),
                    SlicerKind::Condition => view.add_condition_slicer(choice.column.clone(), cx),
                    SlicerKind::Sql => view.add_slicer_card(SlicerDefinition::sql_filter(), cx),
                }
                view.close_slicer_picker();
            }));
        self.close_slicer_picker();
        self.slicer_picker = Some(picker.clone());
        self.slicer_discovery = Some(std::sync::Arc::default());
        self.update_slicer_picker(None, cx);
        if picker.read(cx).loading {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let picker = picker.clone();
            let view = view.clone();
            dialog
                .title("Add slicer")
                .content(move |content, _, _| content.child(picker.clone()))
                .on_close(move |_, _, cx| {
                    let _ = view.update(cx, |view, _| view.close_slicer_picker());
                })
        });
        if let Some(picker) = &self.slicer_picker {
            picker.update(cx, |picker, cx| picker.focus(window, cx));
        }
    }

    fn open_copy_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selection) = self.grid.read(cx).selection() else {
            return;
        };
        let result = self.session.result.clone();
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        let focus_state = command_state.clone();
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let state = command_state.clone();
            let view = view.clone();
            let result = result.clone();
            dialog.title("Copy As").content(move |content, _, _| {
                let view = view.clone();
                let result = result.clone();
                content.child(
                    Command::new(&state)
                        .items(
                            COPY_FORMATS
                                .iter()
                                .map(|(_, label)| CommandItem::new().label(*label)),
                        )
                        .placeholder("Choose a format")
                        .on_confirm(move |index, window, cx| {
                            if let Some((format, _)) = COPY_FORMATS.get(index.row) {
                                view.update(cx, |view, cx| {
                                    view.schedule_copy_format(
                                        result.clone(),
                                        selection,
                                        *format,
                                        cx,
                                    )
                                });
                                window.close_dialog(cx);
                            }
                        }),
                )
            })
        });
        focus_state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn reload(&mut self, reset: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.session.is_sql() {
            return;
        }
        if let Some(view) = self
            .session
            .working
            .as_ref()
            .and_then(|d| d.database_view.clone())
        {
            self.load_database_view(view, reset, window.window_handle(), cx);
            return;
        }
        let Some(path) = self.session.source_path.as_ref().map(PathBuf::from) else {
            self.error = Some("Reload requires a source file".into());
            cx.notify();
            return;
        };
        self.load_source(path, reset, LoadMode::Force, window.window_handle(), cx);
    }

    fn rollback_navigation(&mut self, cx: &mut Context<Self>) {
        self.navigation_target = None;
        if let Some(previous) = self.navigation_backup.take() {
            self.session.restore_navigation(previous.session);
            let window_handle = previous.window_handle;
            let token = self.session.token();
            // Defer widget restoration until any enclosing window update has finished.
            cx.spawn(async move |this, cx| {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    let _ = this.update(cx, |view, cx| {
                        if view.session.accepts(token) {
                            let search = view.session.search_text.clone();
                            view.search_input
                                .update(cx, |input, cx| input.set_value(search, window, cx));
                            if view.session.is_sql() {
                                view.ensure_sql_panel(window, cx);
                            }
                        }
                    });
                });
            })
            .detach();
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
            self.slicer_sidebar
                .update(cx, |sidebar, cx| sidebar.set_cards(cards, cx));
            self.update_slicer_picker(None, cx);
            let sort = self.session.sort.clone();
            self.grid.update(cx, |grid, cx| {
                grid.set_sort(sort, cx);
                grid.set_keymap(
                    self.keymap.clone(),
                    if self.is_table() {
                        KeyContext::TableGrid
                    } else {
                        KeyContext::QueryGrid
                    },
                );
            });
            self.sync_frozen_count(cx);
            self.sync_grid_widths(cx);
        }
    }

    fn record_opened(&self) -> Result<(), String> {
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            return Ok(());
        }
        self.history_writer.opened(
            self.session.working.clone(),
            self.session.source_path.clone(),
        )
    }

    fn schedule_search(&mut self, text: String, cx: &mut Context<Self>) {
        self.schedule_computation(text, ScheduleReason::Immediate, cx);
    }

    fn schedule_computation(
        &mut self,
        text: String,
        reason: ScheduleReason,
        cx: &mut Context<Self>,
    ) {
        if self.session.normalize_empty_query() {
            self.query_cache = None;
            self.sync_frozen_count(cx);
        }
        self.grid.update(cx, |grid, _| {
            grid.set_keymap(
                self.keymap.clone(),
                if self.is_table() {
                    KeyContext::TableGrid
                } else {
                    KeyContext::QueryGrid
                },
            )
        });
        let token = self.session.set_search_text(text.clone());
        self.computation.invalidate(token);
        self.debouncing = reason.debounce().is_some();
        self.busy = true;
        self.error = None;
        if self.session.working.as_ref().is_some_and(|view| matches!(&view.view, ViewKind::Query(config) if !config.pivot_by.is_empty() && config.group_by.is_empty())) {
            self.debouncing = false;
            self.busy = self.computation.is_running();
            cx.notify();
            return;
        }
        let tokens = match query::parse(&text) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.rollback_navigation(cx);
                self.session.publish_sql(
                    self.session.token(),
                    SqlTrace::default(),
                    Some(error.clone()),
                );
                self.error = Some(error);
                self.debouncing = false;
                self.busy = self.computation.is_running();
                cx.notify();
                return;
            }
        };
        let source = self.session.source.clone();
        let definition = self.session.computation_definition();
        let query_cache = match (definition.as_ref(), self.query_cache.as_ref()) {
            (Some(definition), Some(cache))
                if cache.matches(&source, definition, self.query_limit) =>
            {
                Some((cache.frame.clone(), cache.trace.clone()))
            }
            _ => None,
        };
        let request = PipelineRequest {
            query_candidate: None,
            pending_setup: None,
            query_limit: self.query_limit,
            source,
            tokens,
            sort: self.session.sort.clone(),
            definition,
            query_cache,
            slicer_discovery: self.slicer_discovery.clone(),
            queued_at: Instant::now(),
            reuse_table: self
                .table_query
                .clone()
                .map(|(query, _)| (query, self.session.result.clone())),
            table_trace: self
                .table_query
                .as_ref()
                .map(|(_, trace)| trace.clone())
                .unwrap_or_default(),
        };
        cx.notify();
        if let Some(delay) = reason.debounce() {
            cx.spawn(async move |this, cx| {
                let start = Instant::now();
                cx.background_executor().timer(delay).await;
                crate::infrastructure::performance::record("debounce_wait", start);
                let _ = this.update(cx, |view, cx| view.submit_pipeline(token, request, cx));
            })
            .detach();
        } else {
            self.submit_pipeline(token, request, cx);
        }
    }

    fn submit_pipeline(
        &mut self,
        token: JobToken,
        mut request: PipelineRequest,
        cx: &mut Context<Self>,
    ) {
        if !self.session.accepts(token) {
            return;
        }
        self.debouncing = false;
        request.queued_at = Instant::now();
        if let Some((token, request)) = self.computation.enqueue(token, request) {
            self.start_pipeline(token, request, cx);
        }
    }

    fn start_pipeline(
        &mut self,
        token: JobToken,
        mut request: PipelineRequest,
        cx: &mut Context<Self>,
    ) {
        let slicer_cache = self.slicer_stage.clone();
        let query_candidate = request.query_candidate.take();
        let pending_setup = request.pending_setup.is_some();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut trace = SqlTrace::default();
                    let mut query_trace = None;
                    crate::infrastructure::performance::record(
                        "worker_queue_wait",
                        request.queued_at,
                    );
                    let _pipeline = crate::infrastructure::performance::Span::new("pipeline");
                    let definitions = request
                        .definition
                        .as_ref()
                        .map(|definition| definition.slicers.as_slice())
                        .unwrap_or_default();
                    if let Some(snapshot) = request.pending_setup {
                        let result = (|| {
                            let store = request
                                .source
                                .store
                                .lock()
                                .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                            if let Some(definition) = &request.definition
                                && let ViewKind::Query(config) = &definition.view
                            {
                                crate::domain::pivot::validate_dimensions(&store, config)?;
                            }
                            Ok((snapshot, None, None, None))
                        })();
                        return (result, slicer_cache, trace, query_trace);
                    }
                    let slicer_stage = if request.slicer_discovery.is_some()
                        || !definitions.is_empty()
                        || slicer_cache
                            .as_ref()
                            .is_some_and(|stage| stage.matches_source(&request.source))
                    {
                        let value_filters = request
                            .definition
                            .as_ref()
                            .map(|definition| definition.filters.as_slice())
                            .unwrap_or_default();
                        match SlicerStage::prepare(
                            &request.source,
                            definitions,
                            value_filters,
                            slicer_cache.as_ref(),
                            request.slicer_discovery.as_deref(),
                        ) {
                            Ok(stage) => Some(stage),
                            Err(error) => return (Err(error), None, trace, query_trace),
                        }
                    } else {
                        None
                    };
                    let result = (|| {
                        if let Some(stage) = &slicer_stage {
                            stage.validate()?;
                        }
                        let effective_slicers = slicer_stage
                            .as_ref()
                            .map(|stage| stage.effective())
                            .unwrap_or_else(|| definitions.to_vec());
                        if let Some(definition) = &request.definition
                            && let ViewKind::Query(config) = &definition.view
                        {
                            let pivot_source =
                                if let Some((cached, cached_trace)) = request.query_cache.clone() {
                                    let cached_trace = cached_trace.cached();
                                    trace.append(&cached_trace);
                                    query_trace = Some(cached_trace);
                                    cached
                                } else {
                                    let source_store = request
                                        .source
                                        .store
                                        .lock()
                                        .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                                    let pivot_store = crate::domain::pivot::execute_filtered(
                                        &source_store,
                                        config,
                                        &effective_slicers,
                                        &definition.filters,
                                        &crate::domain::pivot::EvaluationClock::system(),
                                        request.query_limit,
                                        &mut trace,
                                    )?;
                                    query_trace = Some(trace.clone());
                                    drop(source_store);
                                    FrameSnapshot::new(pivot_store, request.source.source_revision)
                                };
                            let pivot_store = pivot_source
                                .store
                                .lock()
                                .map_err(|_| "DuckDB query store lock poisoned".to_owned())?;
                            let schema = pivot_store.schema().to_vec();
                            let sort = available_sort_keys(&request.sort, &schema);
                            let view = ViewQuery {
                                hidden: Vec::new(),
                                value_filters: Vec::new(),
                                tokens: request.tokens,
                                filters: Vec::new(),
                                sort: sort.clone(),
                            };
                            // A result that stopped at the limit is missing rows, so
                            // search and sort rerun inside the query to see all of them.
                            // The unnarrowed result stays cached for the next request.
                            let narrowed = if query_limit_reached(
                                pivot_store.row_count(),
                                request.query_limit,
                            ) && (!view.tokens.is_empty()
                                || !view.sort.is_empty())
                            {
                                let source_store = request
                                    .source
                                    .store
                                    .lock()
                                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                                let store = crate::domain::pivot::execute_query(
                                    &source_store,
                                    config,
                                    &effective_slicers,
                                    &definition.filters,
                                    &crate::domain::pivot::EvaluationClock::system(),
                                    request.query_limit,
                                    Some((&pivot_store, &view)),
                                    &mut trace,
                                )?;
                                Some(FrameSnapshot::new(store, request.source.source_revision))
                            } else {
                                None
                            };
                            drop(pivot_store);
                            let shown = narrowed.as_ref().unwrap_or(&pivot_source);
                            let rows = shown
                                .store
                                .lock()
                                .map_err(|_| "DuckDB query store lock poisoned".to_owned())?
                                .query_view_rows_traced(&view, &mut trace)?;
                            let snapshot = ResultSnapshot::from_view(
                                shown,
                                rows,
                                schema,
                                0,
                                token.request_revision,
                            );
                            snapshot.prefetch_page(0)?;
                            return Ok::<_, String>((
                                snapshot,
                                Some(pivot_source),
                                Some(sort),
                                None,
                            ));
                        }

                        let store = request
                            .source
                            .store
                            .lock()
                            .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
                        let filters = crate::application::slicers::filters(&effective_slicers);
                        let all_columns = store.schema();
                        let sort = available_sort_keys(&request.sort, all_columns);
                        let hidden = match request
                            .definition
                            .as_ref()
                            .map(|definition| &definition.view)
                        {
                            // Hiding only changes rows while searching, so an
                            // unsearched view can reuse its rows.
                            Some(ViewKind::Table(config)) if !request.tokens.is_empty() => {
                                config.hidden.clone()
                            }
                            _ => Vec::new(),
                        };
                        let effective_query = ViewQuery {
                            hidden,
                            value_filters: request
                                .definition
                                .as_ref()
                                .map(|definition| definition.filters.clone())
                                .unwrap_or_default(),
                            tokens: request.tokens,
                            filters: filters
                                .into_iter()
                                .filter(|filter| {
                                    !filter.values.is_empty()
                                        || filter
                                            .condition
                                            .as_ref()
                                            .is_some_and(|text| !text.trim().is_empty())
                                })
                                .collect(),
                            sort: sort.clone(),
                        };
                        let reused = request.reuse_table.filter(|(query, result)| {
                            query == &effective_query
                                && result.source_revision == request.source.source_revision
                                && std::sync::Arc::ptr_eq(&result.store, &request.source.store)
                        });
                        let rows = if reused.is_none() {
                            Some(store.query_view_rows_traced(&effective_query, &mut trace)?)
                        } else {
                            None
                        };
                        if reused.is_some() {
                            trace.append(&request.table_trace.cached());
                        }
                        let schema = match request
                            .definition
                            .as_ref()
                            .map(|definition| &definition.view)
                        {
                            Some(ViewKind::Table(config)) => {
                                let available = all_columns
                                    .iter()
                                    .map(|column| column.name.clone())
                                    .collect::<Vec<_>>();
                                let order = table::visible_order(&available, config);
                                order
                                    .iter()
                                    .filter_map(|name| {
                                        all_columns
                                            .iter()
                                            .find(|column| &column.name == name)
                                            .cloned()
                                    })
                                    .collect()
                            }
                            _ => all_columns.to_vec(),
                        };
                        drop(store);
                        let snapshot = if let Some((_, previous)) = reused {
                            previous.reproject(&request.source, &schema, token.request_revision)
                        } else {
                            ResultSnapshot::from_view(
                                &request.source,
                                rows.expect("queried rows"),
                                schema,
                                0,
                                token.request_revision,
                            )
                        };
                        snapshot.prefetch_page(0)?;
                        Ok::<_, String>((snapshot, None, Some(sort), Some(effective_query)))
                    })();
                    (result, slicer_stage, trace, query_trace)
                })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    let next = view.computation.finish(token);
                    // Reuse discovery even when newer input invalidated this publication.
                    if let Some(stage) = &result.1
                        && stage.matches_source(&view.session.source)
                    {
                        view.slicer_stage = Some(stage.clone());
                    }
                    let candidate_valid = query_candidate
                        .as_ref()
                        .is_none_or(|candidate| candidate.is_current(view, cx));
                    if let Some(candidate) = &query_candidate
                        && (!candidate_valid || !view.session.accepts(token))
                    {
                        candidate.finish_error(
                            "The expression or view changed while checking. Apply again.".into(),
                            cx,
                        );
                    }
                    if view.session.accepts(token)
                        && view.computation.may_publish(token)
                        && candidate_valid
                    {
                        let (mut result, slicer_stage, trace, query_trace) = result;
                        if let Some(candidate) = &query_candidate
                            && result.is_ok()
                            && let Err(error) = view
                                .session
                                .set_query_config(candidate.config.clone(), view.suggested_name())
                        {
                            result = Err(error);
                        }
                        let publication_token = view.session.token();
                        if !pending_setup || result.is_err() {
                            view.session.publish_sql(
                                publication_token,
                                trace,
                                result.as_ref().err().cloned(),
                            );
                        }
                        let cards = slicer_stage
                            .as_ref()
                            .map(|stage| stage.cards.clone())
                            .unwrap_or_default();
                        view.slicer_stage = slicer_stage;
                        view.slicer_sidebar
                            .update(cx, |sidebar, cx| sidebar.set_cards(cards, cx));
                        view.update_slicer_picker(result.as_ref().err().cloned(), cx);
                        match result {
                            Ok((mut snapshot, pivot_frame, effective_sort, table_query)) => {
                                if pending_setup {
                                    if let Some(candidate) = &query_candidate {
                                        let _ = cx
                                            .update_window(candidate.window, |_, window, cx| {
                                                candidate.finish_success(window, cx)
                                            });
                                    }
                                } else {
                                    snapshot.generation = publication_token.request_revision;
                                    let _publication =
                                        crate::infrastructure::performance::Span::new(
                                            "result_publication",
                                        );
                                    snapshot.result_revision =
                                        view.session.result.result_revision + 1;
                                    if crate::infrastructure::performance::enabled() {
                                        crate::infrastructure::performance::event(
                                            serde_json::json!({
                                                "metric": "published_generation",
                                                "session_generation": token.session_generation,
                                                "source_revision": token.source_revision,
                                                "request_revision": token.request_revision,
                                                "result_revision": snapshot.result_revision,
                                                "rows": snapshot.row_count(),
                                            }),
                                        );
                                    }
                                    if let Some(sort) = effective_sort
                                        && sort != view.session.sort
                                    {
                                        view.session.sort = sort.clone();
                                        if let Some(definition) = &mut view.session.working {
                                            match &mut definition.view {
                                                ViewKind::Table(config) => {
                                                    config.sort = sort.clone()
                                                }
                                                ViewKind::Sql(config) => {
                                                    config.result_sort = sort.clone()
                                                }
                                                ViewKind::Query(config) => {
                                                    config.result_sort = sort.clone()
                                                }
                                            }
                                        }
                                        view.session.dirty = true;
                                    }
                                    view.query_cache = match (&view.session.working, pivot_frame) {
                                        (Some(definition), Some(frame)) => {
                                            if let ViewKind::Query(config) = &definition.view {
                                                Some(QueryResultCache {
                                                    query_limit: view.query_limit,
                                                    source: view.session.source.clone(),
                                                    config: config.clone(),
                                                    slicers: definition.slicers.clone(),
                                                    filters: definition.filters.clone(),
                                                    frame,
                                                    trace: query_trace.unwrap_or_default(),
                                                })
                                            } else {
                                                None
                                            }
                                        }
                                        _ => None,
                                    };
                                    view.table_query = table_query
                                        .map(|query| (query, view.session.sql.displayed.clone()));
                                    view.session.result = snapshot.clone();
                                    view.information_revision += 1;
                                    view.information_busy = false;
                                    let sort = view.session.sort.clone();
                                    let terms = query::highlight_terms(&view.session.search_text);
                                    view.grid.update(cx, |grid, cx| {
                                        grid.set_sort(sort, cx);
                                        grid.set_highlights(terms, cx);
                                        grid.replace_result(snapshot, cx);
                                        if view.navigation_backup.is_some() {
                                            grid.reset_navigation_position(cx);
                                        }
                                    });
                                    view.sync_frozen_count(cx);
                                    view.sync_grid_widths(cx);
                                    view.refresh_totals(cx);
                                    view.refresh_heatmap(cx);
                                    view.refresh_cell_detail(cx);
                                    let keymap = view.keymap.clone();
                                    let context = if view.is_table() {
                                        KeyContext::TableGrid
                                    } else {
                                        KeyContext::QueryGrid
                                    };
                                    view.grid
                                        .update(cx, |grid, _| grid.set_keymap(keymap, context));
                                    view.error = None;
                                    if let Some(candidate) = &query_candidate {
                                        let _ = cx
                                            .update_window(candidate.window, |_, window, cx| {
                                                candidate.finish_success(window, cx)
                                            });
                                    }
                                    if view.navigation_backup.take().is_some() {
                                        view.reset_sql_editor();
                                        view.commit_navigation_history();
                                        if let Err(error) = view.record_opened() {
                                            eprintln!("history update failed: {error}");
                                        }
                                    } else {
                                        view.remember_navigation_current();
                                        view.undo_history.record(view.session.view_state());
                                    }
                                }
                            }
                            Err(error) => {
                                if let Some(candidate) = &query_candidate {
                                    candidate.finish_error(error.clone(), cx);
                                }
                                if query_candidate.is_none() {
                                    view.rollback_navigation(cx);
                                    view.error = Some(error.to_string());
                                }
                            }
                        }
                    }
                    if let Some((next_token, request)) = next {
                        view.start_pipeline(next_token, request, cx);
                    }
                    view.busy = view.computation.is_running() || view.debouncing;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn schedule_copy(
        &mut self,
        result: ResultSnapshot,
        selection: Selection,
        cx: &mut Context<Self>,
    ) {
        self.schedule_copy_format(result, selection, CopyFormat::RawTsv, cx);
    }

    fn schedule_copy_format(
        &mut self,
        result: ResultSnapshot,
        selection: Selection,
        format: CopyFormat,
        cx: &mut Context<Self>,
    ) {
        self.copy_revision += 1;
        let revision = self.copy_revision;
        let generation = self.session.token().session_generation;
        self.copy_status = Some("Copying…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move { export::export(&result, selection, format) })
                .await;
            if let Some(this) = this.upgrade() {
                this.update(cx, |view, cx| {
                    if view.copy_revision != revision
                        || view.session.token().session_generation != generation
                    {
                        return;
                    }
                    match text {
                        Ok(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            view.copy_status = Some("Copied".into());
                        }
                        Err(error) => view.copy_status = Some(format!("Copy failed: {error}")),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn schedule_statistics(
        &mut self,
        result: ResultSnapshot,
        selection: Selection,
        cx: &mut Context<Self>,
    ) {
        self.statistics_revision += 1;
        let revision = self.statistics_revision;
        let generation = self.session.token().session_generation;
        let result_revision = result.result_revision;
        let (rows, columns) = selection.rectangle();
        let row_count = rows.count();
        let column_count = columns.count();
        self.selection_counts = Some((row_count, column_count, selection.cell_count()));
        self.statistics = None;
        self.statistics_error = None;
        self.statistics_pending = true;
        self.pending_statistics = Some(PendingStatistics {
            result,
            selection,
            revision,
            generation,
            result_revision,
            deadline: Instant::now() + Duration::from_millis(50),
        });
        cx.notify();
        if self.statistics_running {
            return;
        }
        self.statistics_running = true;
        cx.spawn(async move |this, cx| {
            loop {
                let Some(this) = this.upgrade() else {
                    return;
                };
                let deadline = this.update(cx, |view, _| {
                    view.pending_statistics
                        .as_ref()
                        .map(|pending| pending.deadline)
                        .or_else(|| {
                            view.statistics_running = false;
                            None
                        })
                });
                let Some(deadline) = deadline else {
                    return;
                };
                if let Some(delay) = deadline.checked_duration_since(Instant::now()) {
                    cx.background_executor().timer(delay).await;
                    continue;
                }
                let Some(pending) = this.update(cx, |view, _| view.pending_statistics.take())
                else {
                    continue;
                };
                let revision = pending.revision;
                let generation = pending.generation;
                let result_revision = pending.result_revision;
                let stats = cx
                    .background_executor()
                    .spawn(async move { statistics::calculate(&pending.result, pending.selection) })
                    .await;
                this.update(cx, |view, cx| {
                    if view.statistics_revision == revision
                        && view.session.token().session_generation == generation
                        && view.session.result.result_revision == result_revision
                    {
                        view.statistics_pending = false;
                        match stats {
                            Ok(stats) => {
                                view.statistics = Some(stats);
                                view.statistics_error = None;
                            }
                            Err(error) => view.statistics_error = Some(error),
                        }
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn load_source(
        &mut self,
        path: PathBuf,
        reset: bool,
        mode: LoadMode,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.load_data_source(SourceTarget::File(path), reset, mode, window_handle, cx);
    }

    fn load_database_view(
        &mut self,
        view: crate::domain::documents::DatabaseView,
        reset: bool,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.load_data_source(
            SourceTarget::DatabaseView(view),
            reset,
            LoadMode::Check,
            window_handle,
            cx,
        );
    }

    fn load_data_source(
        &mut self,
        target: SourceTarget,
        reset: bool,
        mode: LoadMode,
        window_handle: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        let token = self.session.next_request();
        self.computation.invalidate(token);
        self.debouncing = false;
        self.loading = true;
        self.error = None;
        cx.notify();
        let database = self.database.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match target {
                        SourceTarget::File(path) => read_source(&path, &database, mode),
                        SourceTarget::DatabaseView(view) => database.load_view(&view),
                    }
                })
                .await;
            if let Some(this) = this.upgrade() {
                let _ = cx.update_window(window_handle, |_, window, cx| {
                    this.update(cx, |view, cx| {
                        if !view.session.accepts(token) {
                            return;
                        }
                        view.loading = false;
                        match result {
                            Ok(frame) => {
                                let source = FrameSnapshot::new(
                                    frame,
                                    view.session.source.source_revision + 1,
                                );
                                view.replace_dataframe_with_reset(source, reset, cx);
                                if let Err(error) = view.record_opened() {
                                    eprintln!("history update failed: {error}");
                                }
                                if reset {
                                    view.search_input.update(cx, |input, cx| {
                                        input.set_value(String::new(), window, cx)
                                    });
                                }
                                view.schedule_search(view.session.search_text.clone(), cx);
                            }
                            Err(error) => view.error = Some(error),
                        }
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    fn save_geometry(&self) {
        if let Some(geometry) = &self.window_geometry
            && let Err(error) = self.repository.save_window_geometry(geometry)
        {
            eprintln!("cannot save window placement: {error}");
        }
    }

    fn update_geometry(&mut self, window: &Window) {
        if self.window_geometry.is_some() {
            self.window_geometry = Some(window_placement::geometry(window.window_bounds()));
        }
    }

    fn remember_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_geometry = Some(window_placement::geometry(window.window_bounds()));
        self.save_geometry();
        self._subscriptions
            .push(cx.observe_window_bounds(window, |view, window, cx| {
                let geometry = window_placement::geometry(window.window_bounds());
                if view.window_geometry.as_ref() == Some(&geometry) {
                    return;
                }
                view.window_geometry = Some(geometry);
                view.geometry_save = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    let _ = this.update(cx, |view, _| view.save_geometry());
                }));
            }));
    }

    fn register_window_cleanup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions.push(cx.on_release(|view, _| {
            view.flush_sql_auto_save();
            if let Some((source, patch)) = view.session.take_table_patch()
                && let Err(error) = view.table_writer.edit(PathBuf::from(source), patch)
            {
                eprintln!("Cannot queue table preferences: {error}");
            }
            view.save_geometry();
        }));
        self._subscriptions.push(cx.on_app_quit(|view, cx| {
            view.flush_sql_auto_save();
            view.autosave_table_preferences(cx);
            view.save_geometry();
            async {}
        }));
        let view = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let _ = view.update(cx, |view, cx| {
                view.flush_sql_auto_save();
                view.autosave_table_preferences(cx);
                view.update_geometry(window);
                view.save_geometry();
            });
            true
        });
    }

    pub fn open(
        frame: FrameSnapshot,
        source_path: Option<String>,
        viewer_options: ViewerOptions,
        cx: &mut App,
    ) {
        Self::open_with_error_target(frame, source_path, viewer_options, None, cx);
    }

    fn open_with_error_target(
        frame: FrameSnapshot,
        source_path: Option<String>,
        viewer_options: ViewerOptions,
        error_target: Option<WeakEntity<Self>>,
        cx: &mut App,
    ) {
        let primary = cx.windows().is_empty();
        let mut bounds = WindowBounds::Windowed(Bounds {
            origin: point(px(100.), px(100.)),
            size: size(px(viewer_options.width), px(viewer_options.height)),
        });
        if primary && viewer_options.restore_window && viewer_options.capture.is_none() {
            match viewer_options.repository.load_history() {
                Ok(history) => {
                    let displays = cx
                        .displays()
                        .iter()
                        .map(|display| display.visible_bounds())
                        .collect::<Vec<_>>();
                    if let Some(saved) = history
                        .primary_window
                        .as_ref()
                        .and_then(|saved| window_placement::restored_bounds(saved, &displays))
                    {
                        bounds = saved;
                    }
                }
                Err(error) => eprintln!("cannot restore window placement: {error}"),
            }
        }
        let options = WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some("DataDiver".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.spawn(async move |cx| {
            let mut viewer = None;
            let result = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| {
                    Self::new(
                        frame,
                        source_path,
                        viewer_options.search_text.clone(),
                        viewer_options.load_view.clone(),
                        viewer_options.repository.clone(),
                        window,
                        cx,
                    )
                });
                if primary && viewer_options.capture.is_none() {
                    view.update(cx, |view, cx| view.remember_window(window, cx));
                }
                for column in &viewer_options.slicers {
                    view.update(cx, |view, cx| view.add_slicer(column.clone(), cx));
                }
                if let Some(database_view) = viewer_options
                    .load_view
                    .as_ref()
                    .and_then(|d| d.database_view.clone())
                {
                    view.update(cx, |view, cx| {
                        view.load_database_view(database_view, false, window.window_handle(), cx)
                    });
                } else if let Some((entry, drill)) = viewer_options.open_entry.clone() {
                    view.update(cx, |view, cx| {
                        view.navigate_to_location(
                            entry,
                            false,
                            false,
                            true,
                            None,
                            drill,
                            window.window_handle(),
                            cx,
                        )
                    });
                } else if let Some(path) = viewer_options.load_path.clone() {
                    view.update(cx, |view, cx| {
                        view.load_source(path, false, LoadMode::Check, window.window_handle(), cx)
                    });
                } else if !viewer_options.search_text.is_empty() {
                    view.update(cx, |view, cx| {
                        view.schedule_search(viewer_options.search_text.clone(), cx)
                    });
                }
                viewer = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            });
            let handle = match result {
                Ok(handle) => handle,
                Err(error) => {
                    let message = format!("Cannot open DataDiver window: {error}");
                    eprintln!("{message}");
                    if let Some(view) = error_target {
                        let _ = view.update(cx, |view, cx| {
                            view.error = Some(message);
                            cx.notify();
                        });
                    }
                    return;
                }
            };
            let _ = handle.update(cx, |_, window, _| window.activate_window());
            if viewer_options.benchmark_ui {
                if let Err(error) =
                    performance::run(viewer.as_ref().expect("viewer created"), *handle, cx).await
                {
                    eprintln!("UI benchmark failed: {error}");
                }
                cx.update(|cx| cx.quit());
                return;
            }
            if !viewer_options.show.is_empty() || viewer_options.capture.is_some() {
                let viewer = viewer.as_ref().expect("viewer created");
                let start = std::time::Instant::now();
                loop {
                    let ready = viewer.update(cx, |view, cx| {
                        !view.loading
                            && !view.busy
                            && !view.sql_running
                            && view.grid.read(cx).widths_ready()
                    });
                    if ready {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(20) {
                        eprintln!("capture timed out waiting for the requested data");
                        cx.update(|cx| cx.quit());
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
                for target in &viewer_options.show {
                    if let Err(error) = cx.update_window(*handle, |_, window, cx| {
                        viewer.update(cx, |view, cx| match target.as_str() {
                            "editor:startup-sql" => view.navigate_to_file(
                                FileEntry::StartupSql,
                                false,
                                false,
                                false,
                                window.window_handle(),
                                cx,
                            ),
                            "editor:sql"
                            | "editor:sql-results"
                            | "editor:sql-completion"
                            | "editor:sql-error"
                            | "editor:sql-ask"
                            | "editor:sql-ask-log"
                            | "editor:sql-ask-mention" => view.capture_sql_editor(target, window, cx),
                            "info" => view.open_information(window, cx),
                            "help" => view.open_help(window, cx),
                            "keymap" => view.open_keymap_editor(window, cx),
                            "actions" | "action-menu" => view.capture_actions(target, window, cx),
                            "ai-providers" => view.open_ai_settings(window, cx),
                            "database-tree" => view.toggle_database_tree(window, cx),
                            "search-columns" => {
                                window.focus(&view.search_input.read(cx).focus_handle(cx), cx);
                                view.set_search_text("\"".into(), window, cx);
                                view.sync_completion_columns(cx);
                                view.search_completion
                                    .update(cx, |completion, cx| completion.refresh(cx));
                            }
                            "slicer-search" => view.slicer_sidebar.update(cx, |sidebar, cx| {
                                sidebar.open_first_search("a", window, cx)
                            }),
                            "theme" | "theme-editor" => {
                                view.open_theme_editor(window, cx);
                            }
                            "space" => view.open_space_menu(window, cx),
                            "cell-detail" => view.toggle_cell_detail(window, cx),
                            "save" => view.open_save_dialog(window, cx),
                            "palette:copy-as" => view.open_copy_as(window, cx),
                            "palette:commands" => view.open_command_palette(window, cx),
                            "palette:fonts" => view.open_font_picker(FontTarget::Grid, window, cx),
                            "palette:editor-fonts" => {
                                view.open_font_picker(FontTarget::Editor, window, cx)
                            }
                            "palette:slicers" => view.open_slicer_picker(window, cx),
                            "palette:files" => view.open_file_palette(window, cx),
                            "palette:folders" => view.open_folder_palette(window, cx),
                            "palette:recovered" => view.open_recovery_palette(window, cx),
                            "palette:favorites" => view.open_favorites_palette(window, cx),
                            "palette:columns" => view.open_column_jump(window, cx),
                            "palette:column-chooser" => view.open_column_chooser(window, cx),
                            "editor:group" | "editor:group-error" | "editor:pivot"
                            | "editor:pivot-error" => {
                                view.capture_dimension_editor(target, window, cx)
                            }
                            "panel:group" => {
                                view.open_pivot_panel(PivotFieldKind::GroupBy, window, cx);
                            }
                            "panel:aggregation" | "aggregation-menu" => {
                                view.open_aggregation_panel(window, cx);
                            }
                            "panel:aggregation-functions" => {
                                if let Some(panel) = view.open_aggregation_panel(window, cx) {
                                    panel.update(cx, |panel, cx| {
                                        panel.show_functions(true, window, cx)
                                    });
                                }
                            }
                            "editor:aggregation" | "form:aggregation" | "form:condition" => {
                                if let Some(panel) = view.open_aggregation_panel(window, cx) {
                                    panel.update(cx, |panel, cx| panel.start_custom(window, cx));
                                }
                            }
                            "editor:aggregation-completion" | "editor:group-completion" => {
                                use gpui_kit::EntityInputHandler as _;
                                let sql = if target == "editor:group-completion" {
                                    let panel =
                                        view.open_pivot_panel(PivotFieldKind::GroupBy, window, cx);
                                    panel.update(cx, |panel, cx| panel.start_custom(window, cx));
                                    Some(panel.read(cx).sql.clone())
                                } else {
                                    view.open_aggregation_panel(window, cx).map(|panel| {
                                        panel.update(cx, |panel, cx| panel.start_custom(window, cx));
                                        panel.read(cx).sql.clone()
                                    })
                                };
                                if let Some(sql) = sql {
                                    let state = sql.read(cx).state().clone();
                                    state.update(cx, |state, cx| {
                                        state.set_value("sum(", window, cx);
                                        state.set_selected_range(4..4, cx);
                                        state.focus(window, cx);
                                        // Typed like a keystroke, so completion triggers.
                                        state.replace_text_in_range(None, "\"", window, cx);
                                    });
                                }
                            }
                            "editor:aggregation-error" => {
                                if let Some(panel) = view.open_aggregation_panel(window, cx) {
                                    panel.update(cx, |panel, cx| {
                                        panel.start_custom(window, cx);
                                        panel.sql.read(cx).state().clone().update(
                                            cx,
                                            |state, cx| {
                                                state.set_value(
                                                    "sum(\"missing column\")",
                                                    window,
                                                    cx,
                                                )
                                            },
                                        );
                                        panel.apply(cx);
                                    });
                                }
                            }
                            "panel:pivot" => {
                                view.open_pivot_panel(PivotFieldKind::PivotBy, window, cx);
                            }
                            "panel:order-by" => view.open_order_by_panel(window, cx),
                            _ => {}
                        });
                    }) {
                        eprintln!("cannot show {target}: {error}");
                    }
                }
                if viewer_options.show.iter().any(|target| {
                    target == "info"
                        || target == "palette:files"
                        || target == "palette:slicers"
                        || target == "editor:aggregation-error"
                }) {
                    loop {
                        let busy = viewer.update(cx, |view, _| {
                            view.information_busy || view.file_loading || view.busy
                        });
                        if !busy || start.elapsed() > Duration::from_secs(20) {
                            break;
                        }
                        cx.background_executor()
                            .timer(Duration::from_millis(50))
                            .await;
                    }
                }
            }
            if let Some(path) = viewer_options.capture {
                let started = Instant::now();
                loop {
                    if viewer.as_ref().is_none_or(|viewer| {
                        viewer.update(cx, |view, cx| {
                            !view.loading
                                && !view.sql_running
                                && !view.busy
                                && view.sql_panel.as_ref().is_none_or(|p| !p.read(cx).busy)
                                && view.grid.read(cx).widths_ready()
                        })
                    }) {
                        break;
                    }
                    if started.elapsed() > Duration::from_secs(20) {
                        eprintln!("capture timed out waiting for the SQL editor");
                        cx.update(|cx| cx.quit());
                        return;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(20))
                        .await;
                }
                #[cfg(feature = "snapshots")]
                {
                    cx.background_executor()
                        .timer(viewer_options.capture_delay)
                        .await;
                    match handle.update(cx, |_, window, _| window.render_to_image()) {
                        Ok(Ok(image)) => {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            if let Err(error) = image.save(&path) {
                                eprintln!("capture failed: {error}");
                            }
                        }
                        Ok(Err(error)) | Err(error) => eprintln!("capture failed: {error}"),
                    }
                    if viewer_options.exit_after.is_none() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                        cx.update(|cx| cx.quit());
                    }
                }
                #[cfg(not(feature = "snapshots"))]
                {
                    eprintln!(
                        "--capture requires --features snapshots: {}",
                        path.display()
                    );
                }
            }
            if let Some(delay) = viewer_options.exit_after {
                cx.background_executor().timer(delay).await;
                cx.update(|cx| cx.quit());
            }
            let _ = handle;
        })
        .detach();
    }
}

impl Render for DataFrameView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(px(f32::from(self.font.size)));
        self.sync_completion_columns(cx);
        if self.sync_limit_input {
            self.sync_limit_input = false;
            self.limit_input.update(cx, |input, cx| {
                input.set_value(self.query_limit.to_string(), window, cx)
            });
        }
        if let Some(focus) = self.restore_focus.take() {
            window.focus(&focus, cx);
        }
        let row_count = self.session.result.row_count();
        // Judge the limit by the query's own rows, before search narrowed them,
        // so the warning stays while searching a truncated result.
        let query_rows = self
            .query_cache
            .as_ref()
            .map_or(row_count, |cache| cache.frame.row_count());
        let limit_reached = !self.busy
            && !self.loading
            && self.error.is_none()
            && !self.is_table()
            && query_limit_reached(query_rows, self.query_limit);
        let limit_warning_color = rgb(0xff0000);
        let distinct_color = hsla(142.0 / 360.0, 0.7, 0.42, 1.0);
        let column_count = self.session.result.column_count();
        // Row total before search and filters narrowed it, shown as "N of M rows".
        let unfiltered_rows = if self.loading || self.session.is_sql() {
            None
        } else if self.is_table() {
            self.session
                .has_filters()
                .then(|| self.session.source.row_count())
        } else if self.session.search_text.trim().is_empty() {
            None
        } else {
            self.query_cache
                .as_ref()
                .map(|cache| cache.frame.row_count())
        };
        let has_sort = !self.session.sort.is_empty();
        let pivot_needs_setup = self.session.working.as_ref().is_some_and(|view| {
            matches!(&view.view, ViewKind::Query(config) if !config.pivot_by.is_empty() && config.group_by.is_empty())
        });
        let title = self
            .session
            .working
            .as_ref()
            .map(|view| view.name.clone())
            .or_else(|| {
                self.session.source_path.as_deref().map(|path| {
                    PathBuf::from(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.into())
                })
            })
            .unwrap_or_else(|| "DataDiver".into());
        let modified = self.source_modified.clone();
        let show_modified = window.viewport_size().width >= px(1000.);
        let dirty = self.session.dirty;
        let dialog_layer = Root::render_dialog_layer(window, cx);
        let dataframe = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().tokens.background)
            .text_color(cx.theme().tokens.foreground)
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                style.bg(cx.theme().tokens.accent.opacity(0.25))
            })
            .on_drop(cx.listener(|view, paths: &ExternalPaths, window, cx| {
                view.open_paths(paths.paths().to_vec(), window, cx)
            }))
            .capture_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if view.keymap_editor.is_some() || window.has_active_dialog(cx) {
                    return;
                }
                if view.action_menu_open {
                    view.handle_action_menu_key(event, cx);
                } else if view.space_menu_open {
                    view.handle_space_key(event, window, cx);
                }
            }))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if view.keymap_editor.is_some() {
                    return;
                }
                let key = event.keystroke.key.to_ascii_lowercase();
                if let Some(panel) = &view.sql_panel
                    && view.sql_panel_open
                    && panel
                        .read(cx)
                        .editor
                        .read(cx)
                        .state()
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                {
                    if event.keystroke.modifiers.control
                        && let Ok(stroke) = canonical_keystroke(&key_with_modifiers(event))
                        && let Some(action) =
                            view.keymap.binding_for(&stroke, KeyContext::Application)
                    {
                        view.dispatch_keymap_action(action, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                let limit_focus = view.limit_input.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&limit_focus) {
                    return;
                }
                if key == "tab"
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.alt
                {
                    let search_focus = view.search_input.read(cx).focus_handle(cx);
                    let grid_focus = view.grid.read(cx).focus_handle(cx);
                    let focused = window.focused(cx);
                    if focused.as_ref() == Some(&search_focus) {
                        return;
                    }
                    let next = if focused.as_ref() == Some(&grid_focus) {
                        if event.keystroke.modifiers.shift && !view.is_table() {
                            &limit_focus
                        } else {
                            &search_focus
                        }
                    } else if event.keystroke.modifiers.shift {
                        &grid_focus
                    } else {
                        &search_focus
                    };
                    window.focus(next, cx);
                    cx.stop_propagation();
                    return;
                }
                let stroke = match canonical_keystroke(&key_with_modifiers(event)) {
                    Ok(stroke) => stroke,
                    Err(_) => return,
                };
                if window.has_active_dialog(cx) {
                    if view.keymap.binding_for(&stroke, KeyContext::Window)
                        == Some(ActionId::CloseWindow)
                    {
                        view.dispatch_command(CommandId::CloseWindow, window, cx);
                        cx.stop_propagation();
                    }
                    return;
                }
                if let Some((slot, assign)) = favorite_shortcut(&event.keystroke, |target| {
                    KeybindingKeystroke::new_with_mapper(
                        target,
                        true,
                        cx.keyboard_mapper().as_ref(),
                    )
                }) {
                    view.favorite(slot, assign, window, cx);
                    cx.stop_propagation();
                    return;
                }
                let grid_focus = view.grid.read(cx).focus_handle(cx);
                let search_focus = view.search_input.read(cx).focus_handle(cx);
                let focused = window.focused(cx);
                if key == "escape" && focused.as_ref() == Some(&search_focus) {
                    // Escape first closes the column list, then clears the search.
                    let closed = view
                        .search_completion
                        .update(cx, |completion, cx| completion.close(cx));
                    if !closed && !view.search_input.read(cx).value().is_empty()
                    {
                        view.set_search_text(String::new(), window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let context = if focused.as_ref() == Some(&grid_focus) {
                    if view.is_table() {
                        KeyContext::TableGrid
                    } else {
                        KeyContext::QueryGrid
                    }
                } else if focused.as_ref() == Some(&search_focus) {
                    KeyContext::Search
                } else {
                    KeyContext::Application
                };
                if let Some(action) = view.keymap.binding_for(&stroke, context) {
                    view.dispatch_keymap_action(action, window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .child(
                        crate::ui::column_completion::with_column_completion(
                            div().w(px(320.)).min_w(px(128.)),
                            &self.search_completion,
                            window,
                            cx,
                        )
                        .on_action(cx.listener(|view, _: &MoveDown, window, cx| {
                                view.focus_grid_from_search(window, cx)
                            }))
                            .on_action(cx.listener(|view, _: &IndentInline, window, cx| {
                                if view.is_table() {
                                    view.focus_grid_from_search(window, cx);
                                } else {
                                    window.focus(&view.limit_input.read(cx).focus_handle(cx), cx);
                                    cx.stop_propagation();
                                }
                            }))
                            .on_action(cx.listener(|view, _: &OutdentInline, window, cx| {
                                view.focus_grid_from_search(window, cx)
                            }))
                        .child(Input::new(&self.search_input)),
                    )
                    .when(!self.is_table(), |header| {
                        header.child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .when(limit_reached, |label| {
                                            label.text_color(limit_warning_color)
                                        })
                                        .child("Limit"),
                                )
                                .child(
                                    div()
                                        .w(px(128.))
                                        .flex_shrink_0()
                                        .capture_key_down(cx.listener(
                                            |view, event: &KeyDownEvent, window, cx| {
                                                if event
                                                    .keystroke
                                                    .key
                                                    .eq_ignore_ascii_case("escape")
                                                {
                                                    view.limit_error = None;
                                                    view.limit_input.update(cx, |input, cx| {
                                                        input.set_value(
                                                            view.query_limit.to_string(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                    cx.stop_propagation();
                                                    cx.notify();
                                                }
                                            },
                                        ))
                                        .on_action(cx.listener(
                                            |view, _: &IndentInline, window, cx| {
                                                window.focus(
                                                    &view.grid.read(cx).focus_handle(cx),
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        ))
                                        .on_action(cx.listener(
                                            |view, _: &OutdentInline, window, cx| {
                                                window.focus(
                                                    &view.search_input.read(cx).focus_handle(cx),
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        ))
                                        .child(
                                            Input::new(&self.limit_input).id("query-limit").when(
                                                limit_reached,
                                                |input| {
                                                    input
                                                        .focus_bordered(false)
                                                        .border_3()
                                                        .border_color(limit_warning_color)
                                                },
                                            ),
                                        ),
                                ),
                        )
                    })
                    .when(has_sort, |header| {
                        header.child(
                            Button::new("clear-sort")
                                .label("Clear sort")
                                .on_click(cx.listener(|view, _, _, cx| view.clear_sort(cx))),
                        )
                    })
                    .when(self.session.has_filters(), |header| {
                        header.child(
                            Button::new("clear-filters")
                                .label("Clear filters")
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.clear_filters(window, cx)
                                })),
                        )
                    })
                    .when(self.loading || self.busy, |header| {
                        header.child(div().pl_2().child(if self.loading {
                            "Loading…"
                        } else {
                            "Searching…"
                        }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .child(div().min_w_0().truncate().child(title))
                            .when(dirty, |header| {
                                header.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(rgb(0x9a6b00))
                                        .child("● Unsaved"),
                                )
                            })
                            .when_some(modified.filter(|_| show_modified), |header, modified| {
                                header.child(div().flex_shrink_0().child(modified))
                            }),
                    ),
            )
            .when_some(self.error.as_ref(), |view, error| {
                view.child(div().px_3().text_color(rgb(0xb33c3c)).child(error.clone()))
            })
            .when_some(self.table_preferences_error.as_ref(), |view, error| {
                view.child(div().px_3().text_color(rgb(0xb33c3c)).child(error.clone()))
            })
            .when_some(self.limit_error.as_ref(), |view, error| {
                view.child(div().px_3().text_color(rgb(0xb33c3c)).child(error.clone()))
            })
            .when(!self.session.value_filters().is_empty(), |view| {
                view.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .child(div().text_sm().opacity(0.7).child("Filters"))
                        .children(self.session.value_filters().iter().enumerate().map(
                            |(index, filter)| {
                                Button::new(("value-filter", index))
                                    .small()
                                    .label(format!("{}  ✕", filter.label()))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.remove_value_filter(index, cx)
                                    }))
                            },
                        )),
                )
            })
            .when(pivot_needs_setup, |view| {
                view.child(
                    div()
                        .px_3()
                        .py_2()
                        .child("Add a group by to set up this pivot"),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(self.with_database_tree({
                        let body = div()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .when(
                                self.session
                                    .working
                                    .as_ref()
                                    .is_some_and(|definition| !definition.slicers.is_empty()),
                                |body| body.child(self.slicer_sidebar.clone()),
                            )
                            .child(div().flex_1().min_w_0().child(self.grid.clone()))
                            .when_some(self.cell_detail.as_ref(), |body, (pane, _)| {
                                let width =
                                    (window.viewport_size().width.as_f32() * 0.3).clamp(280., 560.);
                                body.child(div().w(px(width)).flex_shrink_0().child(pane.clone()))
                            });
                        if self.sql_panel_open
                            && let Some(panel) = &self.sql_panel
                        {
                            gpui_kit::component::resizable::v_resizable("sql-split")
                                .with_state(&self.sql_split)
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size_range(px(120.)..px(10000.))
                                        .child(body),
                                )
                                .child(
                                    gpui_kit::component::resizable::resizable_panel()
                                        .size(px(self.sql_editor_height))
                                        .size_range(px(140.)..px(10000.))
                                        .child(panel.clone()),
                                )
                                .into_any_element()
                        } else {
                            body.into_any_element()
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .justify_between()
                    .gap_3()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .gap_2()
                            .child(status::dimensions(row_count, column_count))
                            .when_some(unfiltered_rows, |status, total| {
                                status.child(status::filtered_rows(row_count, total))
                            })
                            .when(limit_reached, |status| {
                                let message = status::limited_rows(self.query_limit);
                                let warning = div()
                                    .id("query-limit-reached")
                                    .text_color(limit_warning_color)
                                    .aria_label(message.clone())
                                    .child(message);
                                #[cfg(feature = "snapshots")]
                                let warning = gpui_kit::TestSupportExt::test_support(warning);
                                status.child(warning)
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .justify_end()
                            .gap_x_2()
                            .when_some(self.copy_status.as_ref(), |status, copy| {
                                status.child(div().child(copy.clone()))
                            })
                            .when_some(self.selection_counts, |status, (rows, columns, cells)| {
                                status.child(div().child(format!(
                                    "Selected: {} · {} cells",
                                    status::dimensions(rows, columns),
                                    crate::domain::formatting::group_numeric_text(
                                        &cells.to_string()
                                    )
                                )))
                            })
                            .when(self.statistics_pending, |status| {
                                status.child(div().child("Calculating statistics…"))
                            })
                            .when_some(self.statistics.as_ref(), |status, stats| {
                                let unique = div()
                                    .id("selection-unique")
                                    .child(status::unique_label(stats));
                                // A distinct selection is a quick check that the selected
                                // columns form a unique key.
                                let unique = if status::selection_is_distinct(stats) {
                                    unique
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(distinct_color)
                                        .tooltip(|window, cx| {
                                            Tooltip::new("Every selected row is distinct")
                                                .build(window, cx)
                                        })
                                } else {
                                    unique
                                };
                                status
                                    .child(div().child(status::nulls_label(stats)))
                                    .child(unique)
                                    .children(
                                        status::statistics(stats)
                                            .into_iter()
                                            .map(|label| div().child(label)),
                                    )
                            })
                            .when_some(self.statistics_error.as_ref(), |status, error| {
                                status.child(div().child(format!("Statistics failed: {error}")))
                            }),
                    ),
            )
            .when(self.space_menu_open, |view| {
                let entity = cx.entity();
                view.child(
                    SpaceMenu::new(self.is_table(), has_sort, self.keymap.clone())
                        .sql_mode(self.session.is_sql(), self.sql_result_complete)
                        .on_select(move |id, window, cx| {
                            entity.update(cx, |view, cx| view.select_space_command(id, window, cx));
                        }),
                )
            })
            .when(self.action_menu_open, |view| {
                let entity = cx.entity();
                let edit_entity = entity.clone();
                let actions = self
                    .view_actions()
                    .into_iter()
                    .map(|action| (action.key, action.description))
                    .collect();
                view.child(ActionMenu::new(
                    actions,
                    move |index, _, cx| {
                        entity.update(cx, |view, cx| view.run_view_action(index, cx));
                    },
                    move |window, cx| {
                        edit_entity.update(cx, |view, cx| view.open_actions_editor(window, cx));
                    },
                ))
            })
            .children(dialog_layer);
        div()
            .size_full()
            .text_size(px(f32::from(self.font.size)))
            .font_family(
                self.font
                    .family
                    .clone()
                    .map(Into::into)
                    .unwrap_or_else(|| cx.theme().font_family.clone()),
            )
            .when_some(self.keymap_editor.clone(), |surface, editor| {
                surface.child(editor)
            })
            .when(self.keymap_editor.is_none(), |surface| {
                surface.child(dataframe)
            })
    }
}
