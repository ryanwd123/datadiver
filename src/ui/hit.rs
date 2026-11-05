//! What a click lands on, for every part of a DataDiver window.

use crate::ui::widgets::{WidgetId, color_picker, editor, list, tree};

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Editor(WidgetId, editor::Part),
    /// The hosted code editor (the SQL panel's).
    CodeEditor(WidgetId),
    ColorPicker(WidgetId, color_picker::Part),
    List(WidgetId, list::Part),
    Tree(WidgetId, tree::Part),
    Grid(GridHit),
    Button(Button),
    Splitter(Splitter),
    /// A column or table name in a `"` or `@` list under an input.
    ColumnCompletion(WidgetId, usize),
    Slicer(SlicerHit),
    /// The dimmed area around a dialog.
    Backdrop,
    /// A dialog or popup's own background, which keeps clicks from the backdrop.
    Surface,
    /// A scrollable dialog body.
    DialogScroll,
    DialogBar,
    /// A tab across the top of the help window.
    HelpTab(usize),
    /// A link on the open help page.
    HelpLink(usize),
    /// The SQL panel's Ask AI panel, which scrolls.
    AskScroll,
    AskBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridHit {
    Header(usize),
    Cell { row: usize, column: usize },
    /// The rows area outside any cell.
    Body,
    VerticalBar,
    HorizontalBar,
    Tooltip,
    TooltipBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Splitter {
    Sql,
    DatabaseTree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlicerHit {
    Value { column: String, index: usize },
    Values(String),
    Bar(String),
    Column(usize),
    ColumnBar(usize),
}

/// Every button in a window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Button {
    ClearSort,
    ClearFilters,
    RemoveValueFilter(usize),
    SpaceCommand(crate::application::commands::CommandId),
    /// A command listed in the SQL helper menu.
    SqlHelperCommand(crate::application::commands::CommandId),
    /// A grid shortcut listed in the Helper menu.
    GridKey(crate::application::keymap::ActionId),
    RunViewAction(usize),
    EditViewActions,
    CacheDropBrowse,
    CacheDropCancel,
    // The gear at the end of the search row.
    ViewOptions,
    // Status and error messages: the log's toolbar button, the toast, and
    // the log's Clear.
    NoticeLog,
    DismissNotice,
    ClearNotices,
    // Dialog chrome.
    DialogClose,
    DialogOk,
    DialogCancel,
    DialogExtra(usize),
    // SQL panel.
    SqlSave,
    SqlRun,
    SqlRunAll,
    SqlAsk,
    SqlHelper,
    SqlStop,
    SqlClose,
    AskInsert,
    AskRun,
    AskDiscard,
    AskLog,
    AskGenerate,
    AskClose,
    // Slicers.
    SlicerSearch(String),
    SlicerClear(String),
    SlicerRemove(String),
    // Database tree.
    TreeRefresh,
    TreeClose,
    // Cell detail.
    CellDetailCopy,
    CellDetailClose,
    /// Keep (`false`) or exclude (`true`) pasted values.
    PasteFilterMode(bool),
    // Group, pivot, and aggregation editors.
    PanelCustom,
    PanelRevert,
    PanelApply,
    PanelClose,
    PanelShowColumns,
    PanelShowFunctions,
    PanelLimitations,
    // Keymap editor.
    KeymapBack,
    KeymapResetAll,
    KeymapBind(usize),
    KeymapReset(usize),
    KeymapRemove(usize, usize),
    KeymapReplace,
    KeymapCancelReplace,
    // Theme editor.
    ThemeReset,
    ThemeSwatch(usize),
    // Actions editor.
    ActionRun(usize),
    ActionEdit(usize),
    ActionDelete(usize),
    ActionExample(usize),
    ActionCancelEdit,
    ActionRunForm,
    ActionSave,
    // Conditional formatting manager.
    FormatSet(usize),
    FormatNewSet,
    FormatDeleteSet,
    FormatColumn(usize),
    FormatPreset(usize),
    /// The presets drop-down's button.
    FormatPresetsToggle,
    /// The open presets list, behind its rows.
    FormatPresets,
    FormatPresetScroll,
    FormatProblem(usize),
    /// Shows or hides the colors panel.
    FormatColorsToggle,
    /// Puts the panel's color at the caret.
    FormatColorInsert,
    // AI providers.
    ProviderDefault(usize),
    ProviderEdit(usize),
    ProviderDelete(usize),
    HeaderRemove(usize),
    HeaderAdd,
    ProviderSendData,
    ProviderNew,
    ProviderTest,
    ProviderSave,
    // Information.
    CopySql,
    // Files palette footer.
    FilesFolders,
    FilesArchive,
    FilesRecover,
}
