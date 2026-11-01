use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::application::commands::CommandId;

pub const KEYMAP_SCHEMA_VERSION: u32 = 1;

#[derive(Default)]
pub struct KeymapRuntime {
    pub profiles: HashMap<PathBuf, KeymapFile>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyContext {
    Application,
    Window,
    Grid,
    TableGrid,
    #[serde(alias = "pivot_grid")]
    QueryGrid,
    Search,
    SpaceMenu,
    TableSpaceMenu,
    #[serde(alias = "pivot_space_menu")]
    QuerySpaceMenu,
    FilePicker,
    GroupBy,
    PivotBy,
    Aggregations,
    KeymapEditor,
    SqlEditor,
    SqlHelperMenu,
}

impl KeyContext {
    pub const ALL: [Self; 16] = [
        Self::Application,
        Self::Window,
        Self::Grid,
        Self::TableGrid,
        Self::QueryGrid,
        Self::Search,
        Self::SpaceMenu,
        Self::TableSpaceMenu,
        Self::QuerySpaceMenu,
        Self::FilePicker,
        Self::GroupBy,
        Self::PivotBy,
        Self::Aggregations,
        Self::KeymapEditor,
        Self::SqlEditor,
        Self::SqlHelperMenu,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Window => "window",
            Self::Grid => "grid",
            Self::TableGrid => "table_grid",
            Self::QueryGrid => "query_grid",
            Self::Search => "search",
            Self::SpaceMenu => "space_menu",
            Self::TableSpaceMenu => "table_space_menu",
            Self::QuerySpaceMenu => "query_space_menu",
            Self::FilePicker => "file_picker",
            Self::GroupBy => "group_by",
            Self::PivotBy => "pivot_by",
            Self::Aggregations => "aggregations",
            Self::KeymapEditor => "keymap_editor",
            Self::SqlEditor => "sql_editor",
            Self::SqlHelperMenu => "sql_helper_menu",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Application => "Application",
            Self::Window => "Window",
            Self::Grid => "Grid",
            Self::TableGrid => "Table Grid",
            Self::QueryGrid => "Query Grid",
            Self::Search => "Search",
            Self::SpaceMenu => "Helper Menu",
            Self::TableSpaceMenu => "Table Helper Menu",
            Self::QuerySpaceMenu => "Query Helper Menu",
            Self::FilePicker => "File Picker",
            Self::GroupBy => "Group By Panel",
            Self::PivotBy => "Pivot By Panel",
            Self::Aggregations => "Aggregations Panel",
            Self::KeymapEditor => "Keymap Editor",
            Self::SqlEditor => "SQL Editor",
            Self::SqlHelperMenu => "SQL Helper Menu",
        }
    }

    pub fn parent(self) -> Option<Self> {
        match self {
            Self::TableGrid | Self::QueryGrid => Some(Self::Grid),
            Self::Grid
            | Self::Search
            | Self::SpaceMenu
            | Self::FilePicker
            | Self::GroupBy
            | Self::PivotBy
            | Self::Aggregations
            | Self::KeymapEditor
            | Self::SqlEditor => Some(Self::Application),
            // Only its own keys work while the menu is open.
            Self::SqlHelperMenu => None,
            Self::TableSpaceMenu | Self::QuerySpaceMenu => Some(Self::SpaceMenu),
            Self::Application => Some(Self::Window),
            Self::Window => None,
        }
    }

    pub fn chain(self) -> Vec<Self> {
        let mut result: Vec<Self> = Vec::new();
        let mut current = Some(self);
        while let Some(context) = current {
            result.push(context);
            current = context.parent();
        }
        result
    }
}

pub fn supported_contexts(context: KeyContext) -> Vec<KeyContext> {
    match context {
        KeyContext::Grid => vec![
            KeyContext::Grid,
            KeyContext::TableGrid,
            KeyContext::QueryGrid,
        ],
        KeyContext::SpaceMenu => vec![
            KeyContext::SpaceMenu,
            KeyContext::TableSpaceMenu,
            KeyContext::QuerySpaceMenu,
        ],
        _ => vec![context],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionId {
    Back,
    Forward,
    SqlEditor,
    Files,
    Commands,
    Settings,
    Keymap,
    Theme,
    Fonts,
    EditorFont,
    IncreaseFontSize,
    DecreaseFontSize,
    Open,
    PasteTable,
    CacheFiles,
    Reload,
    ReloadReset,
    Save,
    NewWindow,
    CloseWindow,
    Copy,
    CopyAs,
    Search,
    Help,
    Info,
    Space,
    Group,
    Aggregation,
    Pivot,
    OrderBy,
    RemoveActiveGroupColumn,
    AddSlicer,
    SortAscending,
    SortDescending,
    ClearSort,
    ClearFilters,
    ToggleFrozen,
    UnfreezeAll,
    HideColumns,
    ChooseColumns,
    ShowAllColumns,
    Export,
    ToggleTotals,
    RecalculateWidths,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    ExtendLeft,
    ExtendRight,
    ExtendUp,
    ExtendDown,
    JumpLeft,
    JumpRight,
    JumpUp,
    JumpDown,
    ExtendToLeft,
    ExtendToRight,
    ExtendToUp,
    ExtendToDown,
    FirstColumn,
    LastColumn,
    FirstCell,
    LastCell,
    PreviousPage,
    NextPage,
    ExtendPreviousPage,
    ExtendNextPage,
    MoveColumnLeft,
    MoveColumnRight,
    IncreaseColumnWidth,
    DecreaseColumnWidth,
    ToggleThousandsGrouping,
    IncreaseDecimalPrecision,
    DecreaseDecimalPrecision,
    SpaceCopy,
    SpaceCopyAs,
    SpaceSave,
    SpaceSearch,
    SpaceAddSlicer,
    SpaceGroup,
    SpaceAggregation,
    SpacePivot,
    SpaceSortAscending,
    SpaceSortDescending,
    SpaceClearSort,
    SpaceToggleFreeze,
    SpaceUnfreezeAll,
    SpaceHideColumns,
    SpaceChooseColumns,
    SpaceExport,
    SpaceToggleTotals,
    SpaceRunAction,
    SpaceReload,
    FilterToValue,
    ExcludeValue,
    DrillDown,
    DrillDownNewWindow,
    CellDetail,
    DatabaseTree,
    JumpToColumn,
    Undo,
    Redo,
    FocusGrid,
    FocusSlicers,
    AskSql,
    RunSql,
    RunAllSql,
    SqlToggleLineComment,
    SqlToggleBlockComment,
    SqlFind,
    SqlReplace,
    SqlGoToLine,
    SqlShowCompletions,
    SqlShowSignature,
    SqlAddCursorAbove,
    SqlAddCursorBelow,
    SqlAddNextOccurrence,
    SqlSelectAllOccurrences,
    SqlMoveLinesUp,
    SqlMoveLinesDown,
    SqlCopyLinesUp,
    SqlCopyLinesDown,
    SqlDeleteLines,
    SqlSelectLine,
    SqlExpandSelection,
    SqlShrinkSelection,
    SqlJumpToBracket,
    SqlFold,
    SqlUnfold,
    SqlFoldAll,
    SqlUnfoldAll,
    SqlToggleWordWrap,
    SqlInsertLineBelow,
    SqlInsertLineAbove,
    SqlHelperMenu,
    SqlGoToDefinition,
    SqlFindReferences,
    SqlRename,
    SqlHelperAskAi,
    SqlHelperExtractCte,
    SqlHelperRunQuery,
    SqlHelperFormat,
    SqlHelperInlineCte,
    SqlHelperWrapQuery,
    SqlHelperExpandStar,
    SqlHelperCollapseStar,
    SqlHelperCountRows,
    SqlHelperPeek,
    SqlHelperDescribe,
    SqlHelperExplain,
    SqlHelperExplainAnalyze,
    SqlHelperGroupBy,
    SqlHelperJoinOn,
    SqlHelperToggleLimit,
    SqlHelperPasteInList,
    SqlHelperPasteValues,
    SqlHelperOutline,
    SqlHelperHistory,
    SqlHelperKeywordCase,
    SqlHelperCopyOneLine,
    SqlHelperPipelineCounts,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    Navigation,
    Selection,
    DataExploration,
    Columns,
    CopyExport,
    FilesViews,
    Application,
    SpaceMenu,
    SqlEditor,
    SqlHelperMenu,
}

impl ActionCategory {
    pub const ALL: [Self; 10] = [
        Self::Navigation,
        Self::Selection,
        Self::DataExploration,
        Self::Columns,
        Self::CopyExport,
        Self::FilesViews,
        Self::Application,
        Self::SpaceMenu,
        Self::SqlEditor,
        Self::SqlHelperMenu,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Selection => "Selection",
            Self::DataExploration => "Data exploration",
            Self::Columns => "Columns",
            Self::CopyExport => "Copy & export",
            Self::FilesViews => "Files & views",
            Self::Application => "Application",
            Self::SpaceMenu => "Helper menu",
            Self::SqlEditor => "SQL editor",
            Self::SqlHelperMenu => "SQL helper menu: Ctrl+K, then",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActionSpec {
    pub id: ActionId,
    pub name: &'static str,
    pub category: ActionCategory,
    pub context: KeyContext,
    pub command: Option<CommandId>,
    pub default_bindings: &'static [&'static str],
}

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: ActionId::AskSql,
        category: ActionCategory::SqlEditor,
        name: "Ask AI for SQL",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::AskSql),
        default_bindings: &["ctrl-i"],
    },
    ActionSpec {
        id: ActionId::RunSql,
        category: ActionCategory::SqlEditor,
        name: "Run SQL statement",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::RunSql),
        default_bindings: &["ctrl-enter"],
    },
    ActionSpec {
        id: ActionId::RunAllSql,
        category: ActionCategory::SqlEditor,
        name: "Run SQL script",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::RunAllSql),
        default_bindings: &["ctrl-shift-enter"],
    },
    ActionSpec {
        id: ActionId::SqlToggleLineComment,
        category: ActionCategory::SqlEditor,
        name: "Toggle line comment",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlToggleLineComment),
        default_bindings: &["ctrl-/"],
    },
    ActionSpec {
        id: ActionId::SqlToggleBlockComment,
        category: ActionCategory::SqlEditor,
        name: "Toggle block comment",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlToggleBlockComment),
        default_bindings: &["alt-shift-a"],
    },
    ActionSpec {
        id: ActionId::SqlFind,
        category: ActionCategory::SqlEditor,
        name: "Find",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlFind),
        default_bindings: &["ctrl-f"],
    },
    ActionSpec {
        id: ActionId::SqlReplace,
        category: ActionCategory::SqlEditor,
        name: "Replace",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlReplace),
        default_bindings: &["ctrl-h"],
    },
    ActionSpec {
        id: ActionId::SqlGoToLine,
        category: ActionCategory::SqlEditor,
        name: "Go to line",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlGoToLine),
        default_bindings: &["ctrl-g"],
    },
    ActionSpec {
        id: ActionId::SqlShowCompletions,
        category: ActionCategory::SqlEditor,
        name: "Show completions",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlShowCompletions),
        default_bindings: &["ctrl-space"],
    },
    ActionSpec {
        id: ActionId::SqlShowSignature,
        category: ActionCategory::SqlEditor,
        name: "Show signature help",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlShowSignature),
        default_bindings: &["ctrl-shift-space"],
    },
    ActionSpec {
        id: ActionId::SqlAddCursorAbove,
        category: ActionCategory::SqlEditor,
        name: "Add cursor above",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlAddCursorAbove),
        default_bindings: &["ctrl-alt-up"],
    },
    ActionSpec {
        id: ActionId::SqlAddCursorBelow,
        category: ActionCategory::SqlEditor,
        name: "Add cursor below",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlAddCursorBelow),
        default_bindings: &["ctrl-alt-down"],
    },
    ActionSpec {
        id: ActionId::SqlAddNextOccurrence,
        category: ActionCategory::SqlEditor,
        name: "Add next occurrence to selection",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlAddNextOccurrence),
        default_bindings: &["ctrl-d"],
    },
    ActionSpec {
        id: ActionId::SqlSelectAllOccurrences,
        category: ActionCategory::SqlEditor,
        name: "Select all occurrences",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlSelectAllOccurrences),
        default_bindings: &["ctrl-shift-l", "ctrl-f2"],
    },
    ActionSpec {
        id: ActionId::SqlMoveLinesUp,
        category: ActionCategory::SqlEditor,
        name: "Move lines up",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlMoveLinesUp),
        default_bindings: &["alt-up"],
    },
    ActionSpec {
        id: ActionId::SqlMoveLinesDown,
        category: ActionCategory::SqlEditor,
        name: "Move lines down",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlMoveLinesDown),
        default_bindings: &["alt-down"],
    },
    ActionSpec {
        id: ActionId::SqlCopyLinesUp,
        category: ActionCategory::SqlEditor,
        name: "Copy lines up",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlCopyLinesUp),
        default_bindings: &["alt-shift-up"],
    },
    ActionSpec {
        id: ActionId::SqlCopyLinesDown,
        category: ActionCategory::SqlEditor,
        name: "Copy lines down",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlCopyLinesDown),
        default_bindings: &["alt-shift-down"],
    },
    ActionSpec {
        id: ActionId::SqlDeleteLines,
        category: ActionCategory::SqlEditor,
        name: "Delete lines",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlDeleteLines),
        default_bindings: &["ctrl-shift-k"],
    },
    ActionSpec {
        id: ActionId::SqlSelectLine,
        category: ActionCategory::SqlEditor,
        name: "Select line",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlSelectLine),
        default_bindings: &["ctrl-l"],
    },
    ActionSpec {
        id: ActionId::SqlExpandSelection,
        category: ActionCategory::SqlEditor,
        name: "Expand selection",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlExpandSelection),
        default_bindings: &["alt-shift-right"],
    },
    ActionSpec {
        id: ActionId::SqlShrinkSelection,
        category: ActionCategory::SqlEditor,
        name: "Shrink selection",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlShrinkSelection),
        default_bindings: &["alt-shift-left"],
    },
    ActionSpec {
        id: ActionId::SqlJumpToBracket,
        category: ActionCategory::SqlEditor,
        name: "Go to matching bracket",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlJumpToBracket),
        default_bindings: &["ctrl-shift-\\"],
    },
    ActionSpec {
        id: ActionId::SqlFold,
        category: ActionCategory::SqlEditor,
        name: "Fold",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlFold),
        default_bindings: &["ctrl-shift-["],
    },
    ActionSpec {
        id: ActionId::SqlUnfold,
        category: ActionCategory::SqlEditor,
        name: "Unfold",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlUnfold),
        default_bindings: &["ctrl-shift-]"],
    },
    ActionSpec {
        id: ActionId::SqlFoldAll,
        category: ActionCategory::SqlEditor,
        name: "Fold all",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlFoldAll),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::SqlUnfoldAll,
        category: ActionCategory::SqlEditor,
        name: "Unfold all",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlUnfoldAll),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::SqlToggleWordWrap,
        category: ActionCategory::SqlEditor,
        name: "Toggle word wrap",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlToggleWordWrap),
        default_bindings: &["alt-z"],
    },
    ActionSpec {
        id: ActionId::SqlInsertLineBelow,
        category: ActionCategory::SqlEditor,
        name: "Insert line below",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlInsertLineBelow),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::SqlInsertLineAbove,
        category: ActionCategory::SqlEditor,
        name: "Insert line above",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlInsertLineAbove),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::SqlHelperMenu,
        category: ActionCategory::SqlEditor,
        name: "SQL helper menu",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlHelperMenu),
        default_bindings: &["ctrl-k"],
    },
    ActionSpec {
        id: ActionId::SqlGoToDefinition,
        category: ActionCategory::SqlEditor,
        name: "Go to definition",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlGoToDefinition),
        default_bindings: &["f12"],
    },
    ActionSpec {
        id: ActionId::SqlFindReferences,
        category: ActionCategory::SqlEditor,
        name: "Find uses",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlFindReferences),
        default_bindings: &["shift-f12"],
    },
    ActionSpec {
        id: ActionId::SqlRename,
        category: ActionCategory::SqlEditor,
        name: "Rename",
        context: KeyContext::SqlEditor,
        command: Some(CommandId::SqlRename),
        default_bindings: &["f2"],
    },
    ActionSpec {
        id: ActionId::SqlHelperAskAi,
        category: ActionCategory::SqlHelperMenu,
        name: "Ask AI for SQL",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::AskSql),
        default_bindings: &["a"],
    },
    ActionSpec {
        id: ActionId::SqlHelperExtractCte,
        category: ActionCategory::SqlHelperMenu,
        name: "Move query to a CTE",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlExtractCte),
        default_bindings: &["c"],
    },
    ActionSpec {
        id: ActionId::SqlHelperRunQuery,
        category: ActionCategory::SqlHelperMenu,
        name: "Run the query at the caret",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlRunQuery),
        default_bindings: &["t"],
    },
    ActionSpec {
        id: ActionId::SqlHelperFormat,
        category: ActionCategory::SqlHelperMenu,
        name: "Format SQL",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlFormat),
        default_bindings: &["f"],
    },
    ActionSpec {
        id: ActionId::SqlHelperInlineCte,
        category: ActionCategory::SqlHelperMenu,
        name: "Inline CTE",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlInlineCte),
        default_bindings: &["i"],
    },
    ActionSpec {
        id: ActionId::SqlHelperWrapQuery,
        category: ActionCategory::SqlHelperMenu,
        name: "Wrap in SELECT * FROM (…)",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlWrapQuery),
        default_bindings: &["w"],
    },
    ActionSpec {
        id: ActionId::SqlHelperExpandStar,
        category: ActionCategory::SqlHelperMenu,
        name: "Expand * into columns",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlExpandStar),
        default_bindings: &["e"],
    },
    ActionSpec {
        id: ActionId::SqlHelperCollapseStar,
        category: ActionCategory::SqlHelperMenu,
        name: "Collapse columns into *",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlCollapseStar),
        default_bindings: &["shift-e"],
    },
    ActionSpec {
        id: ActionId::SqlHelperCountRows,
        category: ActionCategory::SqlHelperMenu,
        name: "Count rows",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlCountRows),
        default_bindings: &["n"],
    },
    ActionSpec {
        id: ActionId::SqlHelperPeek,
        category: ActionCategory::SqlHelperMenu,
        name: "Peek at table or CTE",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlPeek),
        default_bindings: &["p"],
    },
    ActionSpec {
        id: ActionId::SqlHelperDescribe,
        category: ActionCategory::SqlHelperMenu,
        name: "Describe table or CTE",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlDescribe),
        default_bindings: &["d"],
    },
    ActionSpec {
        id: ActionId::SqlHelperExplain,
        category: ActionCategory::SqlHelperMenu,
        name: "Explain",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlExplain),
        default_bindings: &["x"],
    },
    ActionSpec {
        id: ActionId::SqlHelperExplainAnalyze,
        category: ActionCategory::SqlHelperMenu,
        name: "Explain analyze",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlExplainAnalyze),
        default_bindings: &["shift-x"],
    },
    ActionSpec {
        id: ActionId::SqlHelperGroupBy,
        category: ActionCategory::SqlHelperMenu,
        name: "Add or switch GROUP BY",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlGroupBy),
        default_bindings: &["g"],
    },
    ActionSpec {
        id: ActionId::SqlHelperJoinOn,
        category: ActionCategory::SqlHelperMenu,
        name: "Fill in JOIN … ON",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlJoinOn),
        default_bindings: &["j"],
    },
    ActionSpec {
        id: ActionId::SqlHelperToggleLimit,
        category: ActionCategory::SqlHelperMenu,
        name: "Add or remove LIMIT",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlToggleLimit),
        default_bindings: &["l"],
    },
    ActionSpec {
        id: ActionId::SqlHelperPasteInList,
        category: ActionCategory::SqlHelperMenu,
        name: "Paste as IN list",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlPasteInList),
        default_bindings: &["v"],
    },
    ActionSpec {
        id: ActionId::SqlHelperPasteValues,
        category: ActionCategory::SqlHelperMenu,
        name: "Paste as VALUES table",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlPasteValues),
        default_bindings: &["shift-v"],
    },
    ActionSpec {
        id: ActionId::SqlHelperOutline,
        category: ActionCategory::SqlHelperMenu,
        name: "Go to CTE or statement",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlOutline),
        default_bindings: &["o"],
    },
    ActionSpec {
        id: ActionId::SqlHelperHistory,
        category: ActionCategory::SqlHelperMenu,
        name: "Query history",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlHistory),
        default_bindings: &["h"],
    },
    ActionSpec {
        id: ActionId::SqlHelperKeywordCase,
        category: ActionCategory::SqlHelperMenu,
        name: "Change keyword case",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlKeywordCase),
        default_bindings: &["u"],
    },
    ActionSpec {
        id: ActionId::SqlHelperCopyOneLine,
        category: ActionCategory::SqlHelperMenu,
        name: "Copy as one-line string",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlCopyOneLine),
        default_bindings: &["y"],
    },
    ActionSpec {
        id: ActionId::SqlHelperPipelineCounts,
        category: ActionCategory::SqlHelperMenu,
        name: "Row counts of each CTE",
        context: KeyContext::SqlHelperMenu,
        command: Some(CommandId::SqlPipelineCounts),
        default_bindings: &["r"],
    },
    ActionSpec {
        id: ActionId::Back,
        category: ActionCategory::FilesViews,
        name: "Back",
        context: KeyContext::Grid,
        command: Some(CommandId::Back),
        default_bindings: &["u"],
    },
    ActionSpec {
        id: ActionId::Forward,
        category: ActionCategory::FilesViews,
        name: "Forward",
        context: KeyContext::Grid,
        command: Some(CommandId::Forward),
        default_bindings: &["r"],
    },
    ActionSpec {
        id: ActionId::SqlEditor,
        category: ActionCategory::DataExploration,
        name: "Open SQL editor",
        context: KeyContext::Application,
        command: Some(CommandId::SqlEditor),
        default_bindings: &["ctrl-j"],
    },
    ActionSpec {
        id: ActionId::IncreaseColumnWidth,
        category: ActionCategory::Columns,
        name: "Increase column width",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &[")"],
    },
    ActionSpec {
        id: ActionId::ToggleThousandsGrouping,
        category: ActionCategory::Columns,
        name: "Toggle thousands grouping",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &[","],
    },
    ActionSpec {
        id: ActionId::IncreaseDecimalPrecision,
        category: ActionCategory::Columns,
        name: "Increase decimal precision",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &[">"],
    },
    ActionSpec {
        id: ActionId::DecreaseDecimalPrecision,
        category: ActionCategory::Columns,
        name: "Decrease decimal precision",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["<"],
    },
    ActionSpec {
        id: ActionId::DecreaseColumnWidth,
        category: ActionCategory::Columns,
        name: "Decrease column width",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["("],
    },
    ActionSpec {
        id: ActionId::Fonts,
        category: ActionCategory::Application,
        name: "Font picker",
        context: KeyContext::Application,
        command: Some(CommandId::Fonts),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::EditorFont,
        category: ActionCategory::Application,
        name: "Choose editor font",
        context: KeyContext::Application,
        command: Some(CommandId::EditorFont),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::IncreaseFontSize,
        category: ActionCategory::Application,
        name: "Increase font size",
        context: KeyContext::Application,
        command: Some(CommandId::IncreaseFontSize),
        default_bindings: &["ctrl-="],
    },
    ActionSpec {
        id: ActionId::DecreaseFontSize,
        category: ActionCategory::Application,
        name: "Decrease font size",
        context: KeyContext::Application,
        command: Some(CommandId::DecreaseFontSize),
        default_bindings: &["ctrl--"],
    },
    ActionSpec {
        id: ActionId::ClearFilters,
        category: ActionCategory::DataExploration,
        name: "Clear filters",
        context: KeyContext::Application,
        command: Some(CommandId::ClearFilters),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::Files,
        category: ActionCategory::FilesViews,
        name: "Files and views",
        context: KeyContext::Application,
        command: Some(CommandId::Files),
        default_bindings: &["ctrl-p"],
    },
    ActionSpec {
        id: ActionId::DatabaseTree,
        category: ActionCategory::FilesViews,
        name: "Show or hide database tree",
        context: KeyContext::Application,
        command: Some(CommandId::DatabaseTree),
        default_bindings: &["ctrl-b"],
    },
    ActionSpec {
        id: ActionId::Commands,
        category: ActionCategory::Application,
        name: "Command palette",
        context: KeyContext::Application,
        command: Some(CommandId::Commands),
        default_bindings: &["ctrl-shift-p"],
    },
    ActionSpec {
        id: ActionId::Settings,
        category: ActionCategory::Application,
        name: "Settings",
        context: KeyContext::Application,
        command: Some(CommandId::Settings),
        default_bindings: &["ctrl-,"],
    },
    ActionSpec {
        id: ActionId::Keymap,
        category: ActionCategory::Application,
        name: "Keymap Editor",
        context: KeyContext::Application,
        command: Some(CommandId::Keymap),
        default_bindings: &["ctrl-shift-k"],
    },
    ActionSpec {
        id: ActionId::Theme,
        category: ActionCategory::Application,
        name: "Theme Editor",
        context: KeyContext::Application,
        command: Some(CommandId::Theme),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::Open,
        category: ActionCategory::FilesViews,
        name: "Open file",
        context: KeyContext::Application,
        command: Some(CommandId::Open),
        default_bindings: &["ctrl-o"],
    },
    ActionSpec {
        id: ActionId::PasteTable,
        category: ActionCategory::FilesViews,
        name: "Open table from clipboard",
        context: KeyContext::Grid,
        command: Some(CommandId::PasteTable),
        default_bindings: &["ctrl-v"],
    },
    ActionSpec {
        id: ActionId::CacheFiles,
        category: ActionCategory::FilesViews,
        name: "Drop files to watch",
        context: KeyContext::Grid,
        command: Some(CommandId::CacheFiles),
        default_bindings: &["n"],
    },
    ActionSpec {
        id: ActionId::Reload,
        category: ActionCategory::FilesViews,
        name: "Reload",
        context: KeyContext::Application,
        command: Some(CommandId::Reload),
        default_bindings: &["ctrl-r"],
    },
    ActionSpec {
        id: ActionId::ReloadReset,
        category: ActionCategory::FilesViews,
        name: "Reload and reset",
        context: KeyContext::Application,
        command: Some(CommandId::ReloadReset),
        default_bindings: &["ctrl-shift-r"],
    },
    ActionSpec {
        id: ActionId::Save,
        category: ActionCategory::FilesViews,
        name: "Save view",
        context: KeyContext::Application,
        command: Some(CommandId::Save),
        default_bindings: &["ctrl-s", "f12"],
    },
    ActionSpec {
        id: ActionId::NewWindow,
        category: ActionCategory::Application,
        name: "New window",
        context: KeyContext::Application,
        command: Some(CommandId::NewWindow),
        default_bindings: &["ctrl-n"],
    },
    ActionSpec {
        id: ActionId::CloseWindow,
        category: ActionCategory::Application,
        name: "Close window",
        context: KeyContext::Window,
        command: Some(CommandId::CloseWindow),
        default_bindings: &["ctrl-q"],
    },
    ActionSpec {
        id: ActionId::Copy,
        category: ActionCategory::CopyExport,
        name: "Copy selection",
        context: KeyContext::Grid,
        command: Some(CommandId::Copy),
        default_bindings: &["ctrl-c", "c"],
    },
    ActionSpec {
        id: ActionId::CopyAs,
        category: ActionCategory::CopyExport,
        name: "Copy as…",
        context: KeyContext::Grid,
        command: Some(CommandId::CopyAs),
        default_bindings: &["ctrl-shift-c"],
    },
    ActionSpec {
        id: ActionId::Search,
        category: ActionCategory::DataExploration,
        name: "Focus search",
        context: KeyContext::Grid,
        command: Some(CommandId::Search),
        default_bindings: &["/"],
    },
    // ctrl-f also works from inputs; Help shows it on the row above.
    ActionSpec {
        id: ActionId::Search,
        category: ActionCategory::DataExploration,
        name: "Focus search",
        context: KeyContext::Application,
        command: Some(CommandId::Search),
        default_bindings: &["ctrl-f"],
    },
    ActionSpec {
        id: ActionId::Help,
        category: ActionCategory::Application,
        name: "Help",
        context: KeyContext::Application,
        command: Some(CommandId::Help),
        default_bindings: &["f1"],
    },
    ActionSpec {
        id: ActionId::Info,
        category: ActionCategory::DataExploration,
        name: "Information",
        context: KeyContext::Grid,
        command: Some(CommandId::Info),
        default_bindings: &["i"],
    },
    ActionSpec {
        id: ActionId::Space,
        category: ActionCategory::Application,
        name: "Open Helper menu",
        context: KeyContext::Grid,
        command: Some(CommandId::Space),
        default_bindings: &["space", "ctrl-k"],
    },
    ActionSpec {
        id: ActionId::Group,
        category: ActionCategory::DataExploration,
        name: "Group By panel",
        context: KeyContext::Grid,
        command: Some(CommandId::Group),
        default_bindings: &["g"],
    },
    ActionSpec {
        id: ActionId::AddSlicer,
        category: ActionCategory::DataExploration,
        name: "Add Slicer",
        context: KeyContext::Grid,
        command: Some(CommandId::AddSlicer),
        default_bindings: &["z"],
    },
    ActionSpec {
        id: ActionId::Aggregation,
        category: ActionCategory::DataExploration,
        name: "Aggregations panel",
        context: KeyContext::Grid,
        command: Some(CommandId::Aggregation),
        default_bindings: &["a"],
    },
    ActionSpec {
        id: ActionId::Pivot,
        category: ActionCategory::DataExploration,
        name: "Pivot By panel",
        context: KeyContext::Grid,
        command: Some(CommandId::Pivot),
        default_bindings: &["p"],
    },
    ActionSpec {
        id: ActionId::OrderBy,
        category: ActionCategory::DataExploration,
        name: "Order By panel",
        context: KeyContext::Grid,
        command: Some(CommandId::OrderBy),
        default_bindings: &["o"],
    },
    ActionSpec {
        id: ActionId::Undo,
        category: ActionCategory::DataExploration,
        name: "Undo view change",
        context: KeyContext::Application,
        command: Some(CommandId::Undo),
        default_bindings: &["ctrl-z"],
    },
    ActionSpec {
        id: ActionId::Redo,
        category: ActionCategory::DataExploration,
        name: "Redo view change",
        context: KeyContext::Application,
        command: Some(CommandId::Redo),
        default_bindings: &["ctrl-y", "ctrl-shift-z"],
    },
    ActionSpec {
        id: ActionId::FilterToValue,
        category: ActionCategory::DataExploration,
        name: "Filter to selected values",
        context: KeyContext::Grid,
        command: Some(CommandId::FilterToValue),
        default_bindings: &["="],
    },
    ActionSpec {
        id: ActionId::ExcludeValue,
        category: ActionCategory::DataExploration,
        name: "Exclude selected values",
        context: KeyContext::Grid,
        command: Some(CommandId::ExcludeValue),
        default_bindings: &["-"],
    },
    ActionSpec {
        id: ActionId::DrillDown,
        category: ActionCategory::DataExploration,
        name: "Drill down to source rows",
        context: KeyContext::QueryGrid,
        command: Some(CommandId::DrillDown),
        default_bindings: &["enter"],
    },
    ActionSpec {
        id: ActionId::DrillDownNewWindow,
        category: ActionCategory::DataExploration,
        name: "Drill down in new window",
        context: KeyContext::QueryGrid,
        command: Some(CommandId::DrillDownNewWindow),
        default_bindings: &["shift-enter"],
    },
    ActionSpec {
        id: ActionId::CellDetail,
        category: ActionCategory::DataExploration,
        name: "Show or hide cell detail",
        context: KeyContext::Grid,
        command: Some(CommandId::CellDetail),
        default_bindings: &["d"],
    },
    ActionSpec {
        id: ActionId::RemoveActiveGroupColumn,
        category: ActionCategory::DataExploration,
        name: "Remove active group-by column",
        context: KeyContext::QueryGrid,
        command: None,
        default_bindings: &["delete"],
    },
    ActionSpec {
        id: ActionId::AddSlicer,
        category: ActionCategory::DataExploration,
        name: "Add slicer",
        context: KeyContext::Grid,
        command: Some(CommandId::AddSlicer),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::SortAscending,
        category: ActionCategory::DataExploration,
        name: "Sort ascending",
        context: KeyContext::Grid,
        command: Some(CommandId::SortAscending),
        default_bindings: &["["],
    },
    ActionSpec {
        id: ActionId::SortDescending,
        category: ActionCategory::DataExploration,
        name: "Sort descending",
        context: KeyContext::Grid,
        command: Some(CommandId::SortDescending),
        default_bindings: &["]"],
    },
    ActionSpec {
        id: ActionId::ClearSort,
        category: ActionCategory::DataExploration,
        name: "Clear sort",
        context: KeyContext::TableGrid,
        command: Some(CommandId::ClearSort),
        default_bindings: &["shift-x"],
    },
    ActionSpec {
        id: ActionId::ToggleFrozen,
        category: ActionCategory::Columns,
        name: "Freeze or unfreeze column",
        context: KeyContext::TableGrid,
        command: Some(CommandId::ToggleFrozen),
        default_bindings: &["f"],
    },
    ActionSpec {
        id: ActionId::UnfreezeAll,
        category: ActionCategory::Columns,
        name: "Unfreeze all columns",
        context: KeyContext::TableGrid,
        command: Some(CommandId::UnfreezeAll),
        default_bindings: &["shift-f"],
    },
    // SQL views freeze without reordering; pivot views freeze their group-by.
    ActionSpec {
        id: ActionId::ToggleFrozen,
        category: ActionCategory::Columns,
        name: "Freeze or unfreeze column",
        context: KeyContext::QueryGrid,
        command: Some(CommandId::ToggleFrozen),
        default_bindings: &["f"],
    },
    ActionSpec {
        id: ActionId::UnfreezeAll,
        category: ActionCategory::Columns,
        name: "Unfreeze all columns",
        context: KeyContext::QueryGrid,
        command: Some(CommandId::UnfreezeAll),
        default_bindings: &["shift-f"],
    },
    ActionSpec {
        id: ActionId::HideColumns,
        category: ActionCategory::Columns,
        name: "Hide selected columns",
        context: KeyContext::TableGrid,
        command: Some(CommandId::HideColumns),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::ChooseColumns,
        category: ActionCategory::Columns,
        name: "Show or hide columns",
        context: KeyContext::TableGrid,
        command: Some(CommandId::ChooseColumns),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::Export,
        category: ActionCategory::DataExploration,
        name: "Export result to file",
        context: KeyContext::Grid,
        command: Some(CommandId::Export),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::ToggleTotals,
        category: ActionCategory::DataExploration,
        name: "Show or hide totals",
        context: KeyContext::QueryGrid,
        command: Some(CommandId::ToggleTotals),
        default_bindings: &["t"],
    },
    ActionSpec {
        id: ActionId::RecalculateWidths,
        category: ActionCategory::Columns,
        name: "Recalculate column widths",
        context: KeyContext::Grid,
        command: Some(CommandId::RecalculateWidths),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::ShowAllColumns,
        category: ActionCategory::Columns,
        name: "Unhide all columns",
        context: KeyContext::TableGrid,
        command: Some(CommandId::ShowAllColumns),
        default_bindings: &[],
    },
    ActionSpec {
        id: ActionId::MoveLeft,
        category: ActionCategory::Navigation,
        name: "Move cell left",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["left"],
    },
    ActionSpec {
        id: ActionId::MoveRight,
        category: ActionCategory::Navigation,
        name: "Move cell right",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["right"],
    },
    ActionSpec {
        id: ActionId::MoveUp,
        category: ActionCategory::Navigation,
        name: "Move cell up",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["up"],
    },
    ActionSpec {
        id: ActionId::MoveDown,
        category: ActionCategory::Navigation,
        name: "Move cell down",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["down"],
    },
    ActionSpec {
        id: ActionId::ExtendLeft,
        category: ActionCategory::Selection,
        name: "Extend selection left",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-left"],
    },
    ActionSpec {
        id: ActionId::ExtendRight,
        category: ActionCategory::Selection,
        name: "Extend selection right",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-right"],
    },
    ActionSpec {
        id: ActionId::ExtendUp,
        category: ActionCategory::Selection,
        name: "Extend selection up",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-up"],
    },
    ActionSpec {
        id: ActionId::ExtendDown,
        category: ActionCategory::Selection,
        name: "Extend selection down",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-down"],
    },
    ActionSpec {
        id: ActionId::JumpLeft,
        category: ActionCategory::Navigation,
        name: "Move to first column",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-left"],
    },
    ActionSpec {
        id: ActionId::JumpRight,
        category: ActionCategory::Navigation,
        name: "Move to last column",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-right"],
    },
    ActionSpec {
        id: ActionId::JumpUp,
        category: ActionCategory::Navigation,
        name: "Move to first row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-up"],
    },
    ActionSpec {
        id: ActionId::JumpDown,
        category: ActionCategory::Navigation,
        name: "Move to last row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-down"],
    },
    ActionSpec {
        id: ActionId::ExtendToLeft,
        category: ActionCategory::Selection,
        name: "Extend to first column",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-shift-left"],
    },
    ActionSpec {
        id: ActionId::ExtendToRight,
        category: ActionCategory::Selection,
        name: "Extend to last column",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-shift-right"],
    },
    ActionSpec {
        id: ActionId::ExtendToUp,
        category: ActionCategory::Selection,
        name: "Extend to first row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-shift-up"],
    },
    ActionSpec {
        id: ActionId::ExtendToDown,
        category: ActionCategory::Selection,
        name: "Extend to last row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-shift-down"],
    },
    ActionSpec {
        id: ActionId::FirstColumn,
        category: ActionCategory::Navigation,
        name: "First column in row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["home"],
    },
    ActionSpec {
        id: ActionId::LastColumn,
        category: ActionCategory::Navigation,
        name: "Last column in row",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["end"],
    },
    ActionSpec {
        id: ActionId::FirstCell,
        category: ActionCategory::Navigation,
        name: "First cell",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-home"],
    },
    ActionSpec {
        id: ActionId::LastCell,
        category: ActionCategory::Navigation,
        name: "Last cell",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["ctrl-end"],
    },
    ActionSpec {
        id: ActionId::JumpToColumn,
        category: ActionCategory::Navigation,
        name: "Jump to column",
        context: KeyContext::Application,
        command: Some(CommandId::JumpToColumn),
        default_bindings: &["ctrl-g"],
    },
    ActionSpec {
        id: ActionId::FocusGrid,
        category: ActionCategory::Navigation,
        name: "Focus data grid",
        context: KeyContext::Application,
        command: Some(CommandId::FocusGrid),
        default_bindings: &["ctrl-d"],
    },
    ActionSpec {
        id: ActionId::FocusSlicers,
        category: ActionCategory::Navigation,
        name: "Focus next slicer input",
        context: KeyContext::Application,
        command: Some(CommandId::FocusSlicers),
        default_bindings: &["ctrl-l"],
    },
    ActionSpec {
        id: ActionId::PreviousPage,
        category: ActionCategory::Navigation,
        name: "Previous page",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["pageup"],
    },
    ActionSpec {
        id: ActionId::NextPage,
        category: ActionCategory::Navigation,
        name: "Next page",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["pagedown"],
    },
    ActionSpec {
        id: ActionId::ExtendPreviousPage,
        category: ActionCategory::Selection,
        name: "Extend selection up one page",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-pageup"],
    },
    ActionSpec {
        id: ActionId::ExtendNextPage,
        category: ActionCategory::Selection,
        name: "Extend selection down one page",
        context: KeyContext::Grid,
        command: None,
        default_bindings: &["shift-pagedown"],
    },
    ActionSpec {
        id: ActionId::MoveColumnLeft,
        category: ActionCategory::Columns,
        name: "Move column left",
        context: KeyContext::TableGrid,
        command: None,
        default_bindings: &["alt-left"],
    },
    ActionSpec {
        id: ActionId::MoveColumnRight,
        category: ActionCategory::Columns,
        name: "Move column right",
        context: KeyContext::TableGrid,
        command: None,
        default_bindings: &["alt-right"],
    },
    ActionSpec {
        id: ActionId::SpaceCopy,
        category: ActionCategory::SpaceMenu,
        name: "Copy",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Copy),
        default_bindings: &["c"],
    },
    ActionSpec {
        id: ActionId::SpaceCopyAs,
        category: ActionCategory::SpaceMenu,
        name: "Copy as…",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::CopyAs),
        default_bindings: &["shift-c"],
    },
    ActionSpec {
        id: ActionId::SpaceSave,
        category: ActionCategory::SpaceMenu,
        name: "Save",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Save),
        default_bindings: &["s"],
    },
    ActionSpec {
        id: ActionId::SpaceSearch,
        category: ActionCategory::SpaceMenu,
        name: "Search",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Search),
        default_bindings: &["/"],
    },
    ActionSpec {
        id: ActionId::SpaceGroup,
        category: ActionCategory::SpaceMenu,
        name: "Group By",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Group),
        default_bindings: &["g"],
    },
    ActionSpec {
        id: ActionId::SpaceAddSlicer,
        category: ActionCategory::SpaceMenu,
        name: "Add slicer",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::AddSlicer),
        default_bindings: &["z"],
    },
    ActionSpec {
        id: ActionId::SpaceAggregation,
        category: ActionCategory::SpaceMenu,
        name: "Aggregations",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Aggregation),
        default_bindings: &["a"],
    },
    ActionSpec {
        id: ActionId::SpacePivot,
        category: ActionCategory::SpaceMenu,
        name: "Pivot By",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Pivot),
        default_bindings: &["p"],
    },
    ActionSpec {
        id: ActionId::SpaceSortAscending,
        category: ActionCategory::SpaceMenu,
        name: "Sort ascending",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::SortAscending),
        default_bindings: &["["],
    },
    ActionSpec {
        id: ActionId::SpaceSortDescending,
        category: ActionCategory::SpaceMenu,
        name: "Sort descending",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::SortDescending),
        default_bindings: &["]"],
    },
    ActionSpec {
        id: ActionId::SpaceClearSort,
        category: ActionCategory::SpaceMenu,
        name: "Clear sort",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::ClearSort),
        default_bindings: &["shift-x"],
    },
    ActionSpec {
        id: ActionId::SpaceToggleFreeze,
        category: ActionCategory::SpaceMenu,
        name: "Freeze or unfreeze column",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::ToggleFrozen),
        default_bindings: &["f"],
    },
    ActionSpec {
        id: ActionId::SpaceUnfreezeAll,
        category: ActionCategory::SpaceMenu,
        name: "Unfreeze all columns",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::UnfreezeAll),
        default_bindings: &["shift-f"],
    },
    ActionSpec {
        id: ActionId::SpaceToggleFreeze,
        category: ActionCategory::SpaceMenu,
        name: "Freeze or unfreeze column",
        context: KeyContext::QuerySpaceMenu,
        command: Some(CommandId::ToggleFrozen),
        default_bindings: &["f"],
    },
    ActionSpec {
        id: ActionId::SpaceUnfreezeAll,
        category: ActionCategory::SpaceMenu,
        name: "Unfreeze all columns",
        context: KeyContext::QuerySpaceMenu,
        command: Some(CommandId::UnfreezeAll),
        default_bindings: &["shift-f"],
    },
    ActionSpec {
        id: ActionId::SpaceExport,
        category: ActionCategory::SpaceMenu,
        name: "Export result to file",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::Export),
        default_bindings: &["e"],
    },
    ActionSpec {
        id: ActionId::SpaceToggleTotals,
        category: ActionCategory::SpaceMenu,
        name: "Show or hide totals",
        context: KeyContext::QuerySpaceMenu,
        command: Some(CommandId::ToggleTotals),
        default_bindings: &["t"],
    },
    ActionSpec {
        id: ActionId::SpaceHideColumns,
        category: ActionCategory::SpaceMenu,
        name: "Hide selected columns",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::HideColumns),
        default_bindings: &["h"],
    },
    ActionSpec {
        id: ActionId::SpaceChooseColumns,
        category: ActionCategory::SpaceMenu,
        name: "Show or hide columns",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::ChooseColumns),
        default_bindings: &["shift-h"],
    },
    ActionSpec {
        id: ActionId::SpaceRunAction,
        category: ActionCategory::SpaceMenu,
        name: "Run view action",
        context: KeyContext::SpaceMenu,
        command: Some(CommandId::RunAction),
        default_bindings: &["x"],
    },
    ActionSpec {
        id: ActionId::SpaceReload,
        category: ActionCategory::SpaceMenu,
        name: "Reload",
        context: KeyContext::TableSpaceMenu,
        command: Some(CommandId::Reload),
        default_bindings: &["r"],
    },
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBinding {
    pub action: ActionId,
    pub context: KeyContext,
    pub keystroke: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DisabledDefault {
    pub action: ActionId,
    pub context: KeyContext,
    pub keystroke: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeymapFile {
    pub schema_version: u32,
    pub bindings: Vec<UserBinding>,
    pub disabled_defaults: Vec<DisabledDefault>,
}

impl Default for KeymapFile {
    fn default() -> Self {
        Self {
            schema_version: KEYMAP_SCHEMA_VERSION,
            bindings: Vec::new(),
            disabled_defaults: Vec::new(),
        }
    }
}

impl KeymapFile {
    pub fn binding_for(&self, stroke: &str, context: KeyContext) -> Option<ActionId> {
        let chain = context.chain();
        for scope in &chain {
            if let Some(binding) = self.bindings.iter().rev().find(|binding| {
                binding.context == *scope && binding.keystroke.eq_ignore_ascii_case(stroke)
            }) {
                return Some(binding.action);
            }
            if self.disabled_defaults.iter().any(|disabled| {
                disabled.context == *scope && disabled.keystroke.eq_ignore_ascii_case(stroke)
            }) {
                return None;
            }
            if let Some(binding) = ACTIONS.iter().rev().find(|action| {
                action.context == *scope
                    && action
                        .default_bindings
                        .iter()
                        .any(|key| key.eq_ignore_ascii_case(stroke))
                    && !self.disabled_defaults.iter().any(|disabled| {
                        disabled.action == action.id
                            && disabled.context == *scope
                            && disabled.keystroke.eq_ignore_ascii_case(stroke)
                    })
            }) {
                return Some(binding.id);
            }
        }
        None
    }

    pub fn direct_binding_for(&self, stroke: &str, context: KeyContext) -> Option<ActionId> {
        self.bindings
            .iter()
            .rev()
            .find(|binding| {
                binding.context == context && binding.keystroke.eq_ignore_ascii_case(stroke)
            })
            .map(|binding| binding.action)
            .or_else(|| {
                ACTIONS
                    .iter()
                    .rev()
                    .find(|action| {
                        action.context == context
                            && action
                                .default_bindings
                                .iter()
                                .any(|key| key.eq_ignore_ascii_case(stroke))
                            && !self.disabled_defaults.iter().any(|disabled| {
                                disabled.action == action.id
                                    && disabled.context == context
                                    && disabled.keystroke.eq_ignore_ascii_case(stroke)
                            })
                    })
                    .map(|action| action.id)
            })
    }

    pub fn bindings_for(&self, action: ActionId, context: KeyContext) -> Vec<(String, bool)> {
        let mut result: Vec<(String, bool)> = Vec::new();
        for scope in context.chain().into_iter().rev() {
            for disabled in self
                .disabled_defaults
                .iter()
                .filter(|disabled| disabled.action == action && disabled.context == scope)
            {
                result.retain(|(stroke, _)| !stroke.eq_ignore_ascii_case(&disabled.keystroke));
            }
            for binding in ACTIONS
                .iter()
                .filter(|spec| spec.id == action && spec.context == scope)
                .flat_map(|spec| spec.default_bindings.iter().copied())
                .filter(|stroke| {
                    !self.disabled_defaults.iter().any(|disabled| {
                        disabled.action == action
                            && disabled.context == scope
                            && disabled.keystroke.eq_ignore_ascii_case(stroke)
                    })
                })
            {
                result.push((binding.to_owned(), false));
            }
            result.extend(
                self.bindings
                    .iter()
                    .filter(|binding| binding.action == action && binding.context == scope)
                    .map(|binding| (binding.keystroke.clone(), true)),
            );
        }
        result
    }

    pub fn add(
        &mut self,
        action: ActionId,
        context: KeyContext,
        keystroke: String,
        replace: bool,
    ) -> Result<Option<ActionId>, String> {
        let keystroke = canonical_keystroke(&keystroke)?;
        if protected_keystroke(&keystroke, context) {
            return Err(format!(
                "{keystroke} is reserved for text editing in {}",
                context.label()
            ));
        }
        if let Some(conflict) = self
            .direct_binding_for(&keystroke, context)
            .filter(|conflict| *conflict != action)
        {
            if !replace {
                return Err(format!(
                    "{keystroke} is already assigned to {} in {}",
                    action_name(conflict),
                    context.label()
                ));
            }
            self.remove_keystroke(&keystroke, context);
            if let Some(spec) = ACTIONS.iter().find(|spec| {
                spec.id == conflict
                    && spec.context == context
                    && spec
                        .default_bindings
                        .iter()
                        .any(|key| key.eq_ignore_ascii_case(&keystroke))
            }) {
                self.disabled_defaults.push(DisabledDefault {
                    action: spec.id,
                    context,
                    keystroke: keystroke.clone(),
                });
            }
            self.bindings.retain(|binding| {
                !(binding.context == context
                    && binding.action == conflict
                    && binding.keystroke.eq_ignore_ascii_case(&keystroke))
            });
        }
        self.bindings.retain(|binding| {
            !(binding.context == context
                && binding.action == action
                && binding.keystroke.eq_ignore_ascii_case(&keystroke))
        });
        self.bindings.push(UserBinding {
            action,
            context,
            keystroke,
        });
        Ok(None)
    }

    pub fn remove(&mut self, action: ActionId, context: KeyContext, keystroke: &str) {
        let was_user_binding = self.bindings.iter().any(|binding| {
            binding.action == action
                && binding.context == context
                && binding.keystroke.eq_ignore_ascii_case(keystroke)
        });
        let is_local_default = ACTIONS.iter().any(|spec| {
            spec.id == action
                && spec.context == context
                && spec
                    .default_bindings
                    .iter()
                    .any(|stroke| stroke.eq_ignore_ascii_case(keystroke))
        });
        let inherited = self
            .bindings_for(action, context)
            .iter()
            .any(|(stroke, _)| stroke.eq_ignore_ascii_case(keystroke));
        self.bindings.retain(|binding| {
            !(binding.action == action
                && binding.context == context
                && binding.keystroke.eq_ignore_ascii_case(keystroke))
        });
        if inherited
            && (!was_user_binding || is_local_default)
            && !self.disabled_defaults.iter().any(|disabled| {
                disabled.action == action
                    && disabled.context == context
                    && disabled.keystroke.eq_ignore_ascii_case(keystroke)
            })
        {
            self.disabled_defaults.push(DisabledDefault {
                action,
                context,
                keystroke: keystroke.to_owned(),
            });
        }
    }

    pub fn reset(&mut self, action: ActionId, context: KeyContext) {
        self.bindings
            .retain(|binding| !(binding.action == action && binding.context == context));
        self.disabled_defaults
            .retain(|disabled| !(disabled.action == action && disabled.context == context));
    }

    pub fn remove_keystroke(&mut self, stroke: &str, context: KeyContext) {
        self.bindings.retain(|binding| {
            !(binding.context == context && binding.keystroke.eq_ignore_ascii_case(stroke))
        });
    }
}

pub fn action_name(id: ActionId) -> &'static str {
    ACTIONS
        .iter()
        .find(|spec| spec.id == id)
        .map_or("Unknown action", |spec| spec.name)
}

pub fn action_id(key: &str) -> Option<ActionId> {
    match key {
        "back" => Some(ActionId::Back),
        "forward" => Some(ActionId::Forward),
        "files" => Some(ActionId::Files),
        "commands" => Some(ActionId::Commands),
        "keymap" => Some(ActionId::Keymap),
        "theme" => Some(ActionId::Theme),
        "open" => Some(ActionId::Open),
        "reload" => Some(ActionId::Reload),
        "reload_reset" => Some(ActionId::ReloadReset),
        "save" => Some(ActionId::Save),
        "new_window" => Some(ActionId::NewWindow),
        "close_window" => Some(ActionId::CloseWindow),
        "copy" => Some(ActionId::Copy),
        "copy_as" => Some(ActionId::CopyAs),
        "search" => Some(ActionId::Search),
        "help" => Some(ActionId::Help),
        "info" => Some(ActionId::Info),
        "space" => Some(ActionId::Space),
        "group" => Some(ActionId::Group),
        "aggregation" => Some(ActionId::Aggregation),
        "pivot" => Some(ActionId::Pivot),
        "order_by" => Some(ActionId::OrderBy),
        "remove_active_group_column" => Some(ActionId::RemoveActiveGroupColumn),
        "increase_column_width" => Some(ActionId::IncreaseColumnWidth),
        "decrease_column_width" => Some(ActionId::DecreaseColumnWidth),
        "toggle_thousands_grouping" => Some(ActionId::ToggleThousandsGrouping),
        "increase_decimal_precision" => Some(ActionId::IncreaseDecimalPrecision),
        "decrease_decimal_precision" => Some(ActionId::DecreaseDecimalPrecision),
        _ => None,
    }
}

pub fn canonical_keystroke(source: &str) -> Result<String, String> {
    let (modifiers, key) = crate::domain::keystroke::parse_keystroke(source)?;
    Ok(crate::domain::keystroke::stroke(modifiers, &key))
}

fn protected_keystroke(stroke: &str, context: KeyContext) -> bool {
    let is_text = matches!(
        context,
        KeyContext::Application
            | KeyContext::Search
            | KeyContext::FilePicker
            | KeyContext::GroupBy
            | KeyContext::PivotBy
            | KeyContext::Aggregations
            | KeyContext::KeymapEditor
    );
    is_text
        && !stroke.contains("ctrl-")
        && !stroke.contains("alt-")
        && !stroke.contains("cmd-")
        && (stroke.len() == 1
            || matches!(
                stroke,
                "space" | "tab" | "backspace" | "delete" | "enter" | "escape"
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_format_shortcuts_only_apply_to_grids() {
        let map = KeymapFile::default();
        for (stroke, action) in [
            (",", ActionId::ToggleThousandsGrouping),
            (">", ActionId::IncreaseDecimalPrecision),
            ("<", ActionId::DecreaseDecimalPrecision),
        ] {
            for context in [
                KeyContext::Grid,
                KeyContext::TableGrid,
                KeyContext::QueryGrid,
            ] {
                assert_eq!(map.binding_for(stroke, context), Some(action));
            }
            for context in [
                KeyContext::Application,
                KeyContext::Search,
                KeyContext::FilePicker,
                KeyContext::Aggregations,
            ] {
                assert_eq!(map.binding_for(stroke, context), None);
            }
        }
    }

    #[test]
    fn helper_menu_reloads_tables_with_r() {
        let map = KeymapFile::default();
        assert_eq!(map.binding_for("r", KeyContext::TableSpaceMenu), Some(ActionId::SpaceReload));
        assert_eq!(map.binding_for("r", KeyContext::QuerySpaceMenu), None);
    }

    #[test]
    fn back_and_forward_defaults_only_apply_to_grids() {
        let map = KeymapFile::default();
        for context in [KeyContext::Grid, KeyContext::TableGrid, KeyContext::QueryGrid] {
            assert_eq!(map.binding_for("u", context), Some(ActionId::Back));
            assert_eq!(map.binding_for("r", context), Some(ActionId::Forward));
        }
        for context in [KeyContext::Search, KeyContext::Application, KeyContext::FilePicker,
            KeyContext::GroupBy, KeyContext::PivotBy, KeyContext::Aggregations, KeyContext::KeymapEditor] {
            assert_eq!(map.binding_for("u", context), None);
            assert_eq!(map.binding_for("r", context), None);
        }
        assert_eq!(map.binding_for("ctrl-r", KeyContext::Grid), Some(ActionId::Reload));
        assert_eq!(action_id("back"), Some(ActionId::Back));
        assert_eq!(action_id("forward"), Some(ActionId::Forward));
    }

    #[test]
    fn totals_and_helper_menu_keys_work_on_grids() {
        let map = KeymapFile::default();
        assert_eq!(map.binding_for("t", KeyContext::QueryGrid), Some(ActionId::ToggleTotals));
        assert_eq!(map.binding_for("t", KeyContext::TableGrid), None);
        assert_eq!(map.binding_for("t", KeyContext::Search), None);
        for context in [KeyContext::TableGrid, KeyContext::QueryGrid] {
            for stroke in ["space", "ctrl-k"] {
                assert_eq!(map.binding_for(stroke, context), Some(ActionId::Space));
            }
        }
        assert_eq!(map.binding_for("ctrl-k", KeyContext::SqlEditor), Some(ActionId::SqlHelperMenu));
    }

    #[test]
    fn copy_save_and_freeze_defaults_respect_contexts() {
        let map = KeymapFile::default();
        for context in [KeyContext::TableGrid, KeyContext::QueryGrid] {
            for stroke in ["ctrl-c", "c"] {
                assert_eq!(map.binding_for(stroke, context), Some(ActionId::Copy));
            }
            for stroke in ["ctrl-s", "f12"] {
                assert_eq!(map.binding_for(stroke, context), Some(ActionId::Save));
            }
        }
        assert_eq!(
            map.binding_for("f", KeyContext::TableGrid),
            Some(ActionId::ToggleFrozen)
        );
        assert_eq!(map.binding_for("alt-f", KeyContext::TableGrid), None);
        assert_eq!(
            map.binding_for("f", KeyContext::TableSpaceMenu),
            Some(ActionId::SpaceToggleFreeze)
        );
        assert_eq!(
            map.binding_for("f", KeyContext::QueryGrid),
            Some(ActionId::ToggleFrozen)
        );
        for stroke in ["c", "f"] {
            assert_eq!(map.binding_for(stroke, KeyContext::Search), None);
        }
        assert_eq!(
            map.binding_for("ctrl-s", KeyContext::Search),
            Some(ActionId::Save)
        );
    }

    #[test]
    fn additional_defaults_can_be_listed_removed_replaced_and_reset() {
        let mut map = KeymapFile::default();
        assert_eq!(
            map.bindings_for(ActionId::Copy, KeyContext::Grid),
            vec![("ctrl-c".into(), false), ("c".into(), false)]
        );
        assert_eq!(
            map.bindings_for(ActionId::Save, KeyContext::Application),
            vec![("ctrl-s".into(), false), ("f12".into(), false)]
        );
        map.remove(ActionId::Copy, KeyContext::Grid, "c");
        assert_eq!(map.binding_for("c", KeyContext::TableGrid), None);
        assert_eq!(
            map.binding_for("ctrl-c", KeyContext::TableGrid),
            Some(ActionId::Copy)
        );
        map.reset(ActionId::Copy, KeyContext::Grid);
        assert_eq!(
            map.binding_for("c", KeyContext::TableGrid),
            Some(ActionId::Copy)
        );
        assert!(
            map.add(ActionId::Info, KeyContext::Grid, "c".into(), false)
                .is_err()
        );
        map.add(ActionId::Info, KeyContext::Grid, "c".into(), true)
            .unwrap();
        map.remove(ActionId::Info, KeyContext::Grid, "c");
        assert_eq!(map.binding_for("c", KeyContext::Grid), None);
        assert_eq!(
            map.bindings_for(ActionId::Copy, KeyContext::Grid),
            vec![("ctrl-c".into(), false)]
        );
    }

    #[test]
    fn legacy_mode_contexts_keep_custom_bindings() {
        for (old, context, new) in [
            ("pivot_grid", KeyContext::QueryGrid, "query_grid"),
            (
                "pivot_space_menu",
                KeyContext::QuerySpaceMenu,
                "query_space_menu",
            ),
        ] {
            assert_eq!(
                serde_json::from_str::<KeyContext>(&format!("\"{old}\"")).unwrap(),
                context
            );
            assert_eq!(
                serde_json::to_string(&context).unwrap(),
                format!("\"{new}\"")
            );
        }
    }

    #[test]
    fn font_shortcuts_resolve_in_grid_and_search() {
        let map = KeymapFile::default();
        for context in [
            KeyContext::TableGrid,
            KeyContext::QueryGrid,
            KeyContext::Search,
        ] {
            assert_eq!(
                map.binding_for("ctrl-=", context),
                Some(ActionId::IncreaseFontSize)
            );
            assert_eq!(
                map.binding_for("ctrl--", context),
                Some(ActionId::DecreaseFontSize)
            );
        }
    }

    #[test]
    fn specific_grid_context_overrides_general_grid_binding() {
        let mut map = KeymapFile::default();
        map.add(ActionId::Info, KeyContext::Grid, "i".into(), false)
            .unwrap();
        map.add(ActionId::Help, KeyContext::TableGrid, "i".into(), false)
            .unwrap();
        assert_eq!(
            map.binding_for("i", KeyContext::TableGrid),
            Some(ActionId::Help)
        );
        assert_eq!(
            map.binding_for("i", KeyContext::QueryGrid),
            Some(ActionId::Info)
        );
    }

    #[test]
    fn conflicts_require_explicit_replacement_and_replace_keeps_default_unbound() {
        let mut map = KeymapFile::default();
        assert!(
            map.add(ActionId::Info, KeyContext::Grid, "g".into(), false)
                .is_err()
        );
        map.add(ActionId::Info, KeyContext::Grid, "g".into(), true)
            .unwrap();
        assert_eq!(map.binding_for("g", KeyContext::Grid), Some(ActionId::Info));
        assert_eq!(map.binding_for("i", KeyContext::Grid), Some(ActionId::Info));
        assert_eq!(
            map.bindings_for(ActionId::Info, KeyContext::Grid),
            vec![("i".into(), false), ("g".into(), true)]
        );
    }

    #[test]
    fn reset_and_remove_keep_default_semantics_and_protect_text_input() {
        let mut map = KeymapFile::default();
        map.remove(ActionId::Files, KeyContext::Application, "ctrl-p");
        assert_eq!(map.binding_for("ctrl-p", KeyContext::Application), None);
        map.reset(ActionId::Files, KeyContext::Application);
        assert_eq!(
            map.binding_for("ctrl-p", KeyContext::Application),
            Some(ActionId::Files)
        );
        assert!(
            map.add(ActionId::Help, KeyContext::Search, "g".into(), false)
                .is_err()
        );
    }

    #[test]
    fn removing_inherited_default_masks_only_the_selected_context() {
        let mut map = KeymapFile::default();
        map.remove(ActionId::Info, KeyContext::TableGrid, "i");
        assert_eq!(map.binding_for("i", KeyContext::TableGrid), None);
        assert_eq!(
            map.binding_for("i", KeyContext::QueryGrid),
            Some(ActionId::Info)
        );
        map.reset(ActionId::Info, KeyContext::TableGrid);
        assert_eq!(
            map.binding_for("i", KeyContext::TableGrid),
            Some(ActionId::Info)
        );
    }

    #[test]
    fn removing_custom_context_override_reveals_parent_binding() {
        let mut map = KeymapFile::default();
        map.add(ActionId::Help, KeyContext::TableGrid, "i".into(), true)
            .unwrap();
        assert_eq!(
            map.binding_for("i", KeyContext::TableGrid),
            Some(ActionId::Help)
        );
        map.remove(ActionId::Help, KeyContext::TableGrid, "i");
        assert_eq!(
            map.binding_for("i", KeyContext::TableGrid),
            Some(ActionId::Info)
        );
    }
}
