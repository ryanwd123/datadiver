//! Dialogs: palettes, editors, confirmations, and their frames.

use super::*;
use crate::application::state::ApplicationState;
use crate::infrastructure::catalog::scan_sources;
use crate::infrastructure::json_store::FavoriteTarget;
use crate::ui::actions_editor::{ActionsEditor, ActionsEditorEvent};
use crate::ui::ai_settings::{AiSettingsEditor, AiSettingsEvent};
use crate::ui::aggregation_editor::AggregationPanel;
use crate::ui::command_palette::{PALETTE_WIDTH, matches_terms, matching_commands};
use crate::ui::font_picker::FontTarget;
use crate::ui::hit::Button;
use crate::ui::info::InformationPanel;
use crate::ui::keymap_editor::KeymapEditorEvent;
use crate::ui::order_by_panel::{OrderByEdit, OrderByPanel};
use crate::ui::pivot_panel::PivotPanel;
use crate::ui::platform::{Align, Canvas, KeyEvent, Modifiers, Point, point, rect, system, text};
use crate::ui::slicer_sidebar::SlicerPicker;
use crate::ui::theme_editor::ThemeEditor;
use crate::ui::widgets::list::{self, Item, Palette};
use crate::ui::widgets::{ButtonStyle, Row, button, button_height, button_width, colors, scroll};

/// A command palette row, with its shortcuts on the right.
fn command_item((command, label, shortcut): &(&'static crate::application::commands::Command, String, String)) -> Item {
    let item = Item::new(label.clone()).keywords(command.key);
    if shortcut.is_empty() { item } else { item.detail(shortcut.clone()) }
}

/// What a confirmation does once accepted.
#[derive(Clone, Debug)]
pub enum ConfirmAction {
    /// A new SQL script, or else a new SQL source.
    NewSql { script: bool },
    DropRelation(crate::domain::documents::DatabaseView),
    RemoveWatchedFolder(PathBuf),
    ArchiveView(Box<DocumentDefinition>),
    /// Deletes a saved view for good.
    DeleteView(Box<DocumentDefinition>),
    /// Deletes an archived view for good.
    DeleteArchive(PathBuf),
    /// Exports a result whose file was read only up to the row limit.
    ExportCut,
}

pub struct ConfirmDialog {
    title: String,
    message: String,
    action_label: String,
    danger: bool,
    action: ConfirmAction,
}

#[cfg(test)]
impl ConfirmDialog {
    pub(crate) fn text(&self) -> (&str, &str, bool) {
        (&self.title, &self.message, self.danger)
    }
}

/// The lists a palette dialog can show, with what choosing an item does.
pub enum PaletteKind {
    Commands(Vec<&'static AppCommand>, Vec<&'static AppCommand>),
    /// Each section's entries, in the palette's section order.
    Files(Vec<Vec<FileEntry>>),
    Folders(Vec<String>),
    Recovery(Vec<PathBuf>),
    Favorites,
    CopyAs(ResultSnapshot, Selection),
    ColumnJump,
    ColumnChooser(Vec<String>),
    Fonts(FontTarget, Vec<Option<String>>),
    Engines,
    Settings,
    EditorSettings,
    EditorChoice(super::editor_settings::EditorOption, Vec<Option<u8>>),
    ViewOptions,
    /// Where each statement and CTE of the SQL editor is.
    SqlOutline(Vec<std::ops::Range<usize>>),
    /// Past SQL runs, newest first.
    SqlHistory(Vec<String>),
}

pub struct PaletteDialog {
    pub(crate) title: String,
    pub(crate) palette: Palette,
    pub(crate) kind: PaletteKind,
    /// A line under the list, such as the favorites hint.
    footer: Option<String>,
}

pub enum Dialog {
    Save { name: Editor },
    Confirm(ConfirmDialog),
    Information(InformationPanel),
    Pivot(PivotPanel),
    OrderBy(OrderByPanel),
    Aggregation(AggregationPanel),
    Palette(PaletteDialog),
    SlicerPicker(SlicerPicker),
    Theme(ThemeEditor),
    Actions(ActionsEditor),
    AiSettings(AiSettingsEditor),
    PasteFilter(super::paste_filter::PasteFilterDialog),
    /// A condition on one column of a query's result.
    ResultFilter {
        column: String,
        input: Editor,
        error: Option<String>,
    },
}

/// What the result filter dialog says under its input.
const RESULT_FILTER_HINT: &str =
    "Keeps groups whose value matches, after grouping: >100 · >=10 <100 · 10..20 · =5 · !=0 · Empty removes the filter";

impl Dialog {
    fn title(&self) -> String {
        match self {
            Dialog::Save { .. } => "Save view".into(),
            Dialog::ResultFilter { column, .. } => format!("Filter {column}"),
            Dialog::Confirm(confirm) => confirm.title.clone(),
            Dialog::Information(_) => "Information".into(),
            Dialog::Pivot(_) => String::new(),
            Dialog::OrderBy(_) => "Order By".into(),
            Dialog::Aggregation(_) => "Aggregations".into(),
            Dialog::Palette(palette) => palette.title.clone(),
            Dialog::SlicerPicker(_) => "Add slicer".into(),
            Dialog::Theme(_) => "Theme Editor".into(),
            Dialog::Actions(_) => "View actions".into(),
            Dialog::AiSettings(_) => "AI providers".into(),
            Dialog::PasteFilter(dialog) => {
                if dialog.exclude { "Exclude pasted values".into() } else { "Filter to pasted values".into() }
            }
        }
    }

    /// Buttons along the bottom, right to left.
    fn footer(&self) -> Vec<(&'static str, Button, ButtonStyle)> {
        match self {
            Dialog::Save { .. } => vec![
                ("Save", Button::DialogOk, ButtonStyle::Primary),
                ("Cancel", Button::DialogCancel, ButtonStyle::Outline),
            ],
            Dialog::ResultFilter { .. } => vec![
                ("Apply", Button::DialogOk, ButtonStyle::Primary),
                ("Cancel", Button::DialogCancel, ButtonStyle::Outline),
            ],
            Dialog::Confirm(confirm) => vec![
                (
                    // The label lives in the dialog; drawn specially below.
                    "",
                    Button::DialogOk,
                    if confirm.danger { ButtonStyle::Danger } else { ButtonStyle::Primary },
                ),
                ("Cancel", Button::DialogCancel, ButtonStyle::Outline),
            ],
            Dialog::PasteFilter(_) => vec![
                ("Apply", Button::DialogOk, ButtonStyle::Primary),
                ("Cancel", Button::DialogCancel, ButtonStyle::Outline),
            ],
            Dialog::Theme(_) => vec![
                ("Save", Button::DialogOk, ButtonStyle::Primary),
                ("Cancel", Button::DialogCancel, ButtonStyle::Outline),
                ("Reset to defaults", Button::ThemeReset, ButtonStyle::Outline),
            ],
            _ => Vec::new(),
        }
    }
}

impl DataFrameView {
    pub(crate) fn open_dialog(&mut self, dialog: Dialog, focus: Option<WidgetId>, cx: &mut Cx<Self>) {
        self.close_dialog(cx);
        let title = dialog.window_title();
        self.dialog = Some(dialog);
        match title {
            Some(title) => self.open_panel_window(title, cx),
            None => self.panel_owner = None,
        }
        self.set_focus(focus, cx);
        cx.notify();
    }

    /// Closes the open dialog, letting it clean up.
    pub(crate) fn close_dialog(&mut self, cx: &mut Cx<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        self.close_panel_window(cx);
        match dialog {
            Dialog::Pivot(mut panel) => panel.close(),
            Dialog::Aggregation(mut panel) => panel.close(),
            Dialog::SlicerPicker(_) => self.close_slicer_picker(),
            _ => {}
        }
        if self.focus.is_none_or(|focus| !self.owns_focus(focus)) {
            self.focus_grid(cx);
        }
        cx.notify();
    }

    /// Whether a control outside dialogs holds `id`.
    pub(crate) fn owns_focus(&self, id: WidgetId) -> bool {
        id == self.grid.id
            || id == self.search_input.id
            || id == self.limit_input.id
            || self.sql_panel.as_ref().is_some_and(|panel| self.sql_panel_open && panel.contains(id))
            || (self.database_tree_open && self.database_tree.contains(id))
            || self.slicer_sidebar.contains(id)
            || self.cell_detail.as_ref().is_some_and(|pane| pane.editor.id == id)
    }

    pub(crate) fn open_save_dialog(&mut self, cx: &mut Cx<Self>) {
        if matches!(self.session.identity, SessionIdentity::StartupSql) {
            self.persist_view();
            cx.notify();
            return;
        }
        let mut name = Editor::single_line().with_placeholder("View name");
        name.set_text(self.suggested_name());
        name.select_all();
        let id = name.id;
        self.open_dialog(Dialog::Save { name }, Some(id), cx);
    }

    /// Asks for a condition on the aggregation column under the cursor.
    pub(crate) fn open_result_filter(&mut self, cx: &mut Cx<Self>) {
        let Some((_, column)) = self.active_result_column() else {
            self.copy_status = Some("Put the cursor on an aggregation column of a grouped view to filter it".into());
            cx.notify();
            return;
        };
        let mut input = Editor::single_line().with_placeholder("Condition, such as >100");
        if let Some(filter) = self.session.result_filters().iter().find(|filter| filter.column == column) {
            input.set_text(filter.condition.clone());
            input.select_all();
        }
        let id = input.id;
        self.open_dialog(Dialog::ResultFilter { column, input, error: None }, Some(id), cx);
    }

    /// Applies the result filter dialog; a condition that does not read
    /// stays in the dialog with its error.
    fn apply_result_filter_dialog(&mut self, cx: &mut Cx<Self>) -> bool {
        let Some(Dialog::ResultFilter { column, input, .. }) = &self.dialog else {
            return false;
        };
        let (column, condition) = (column.clone(), input.text().to_owned());
        match self.set_result_filter(&column, &condition, cx) {
            Ok(()) => true,
            Err(message) => {
                if let Some(Dialog::ResultFilter { error, .. }) = &mut self.dialog {
                    *error = Some(message);
                }
                cx.notify();
                false
            }
        }
    }

    fn save_from_dialog(&mut self, cx: &mut Cx<Self>) -> bool {
        let Some(Dialog::Save { name }) = &self.dialog else {
            return false;
        };
        let name = name.text().trim().to_string();
        match self.session.save_as(name, &self.repository) {
            Ok(()) => {
                // Undo would otherwise restore draft states of a saved view.
                self.reset_undo_history();
                self.sync_grid_widths(cx);
                if let Err(error) = self.record_opened() {
                    eprintln!("history update failed: {error}");
                }
                self.error = None;
                cx.notify();
                true
            }
            Err(error) => {
                self.error = Some(format!("Save failed: {error}"));
                cx.notify();
                false
            }
        }
    }

    pub(crate) fn confirm(&mut self, title: String, message: String, action_label: String, danger: bool, action: ConfirmAction, cx: &mut Cx<Self>) {
        self.open_dialog(
            Dialog::Confirm(ConfirmDialog {
                title,
                message,
                action_label,
                danger,
                action,
            }),
            None,
            cx,
        );
    }

    fn run_confirmed(&mut self, action: ConfirmAction, cx: &mut Cx<Self>) {
        match action {
            ConfirmAction::NewSql { script } => self.begin_new_sql(script, cx),
            ConfirmAction::ExportCut => self.export_rows(cx),
            ConfirmAction::DropRelation(relation) => {
                let database = self.database.clone();
                self.run_tree_delete(move || database.drop_relation(&relation), cx);
            }
            ConfirmAction::RemoveWatchedFolder(root) => self.remove_watched_folder(root, cx),
            ConfirmAction::ArchiveView(definition) => self.remove_view(&definition, false, cx),
            ConfirmAction::DeleteView(definition) => self.remove_view(&definition, true, cx),
            ConfirmAction::DeleteArchive(path) => {
                match self.repository.delete_archive(&path) {
                    Ok(()) => self.copy_status = Some("Deleted the archived view".into()),
                    Err(error) => self.error = Some(format!("Delete failed: {error}")),
                }
                self.open_recovery_palette(cx);
            }
        }
    }

    pub(crate) fn open_information(&mut self, cx: &mut Cx<Self>) {
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
        let column_names = result.columns.iter().map(|column| column.0.clone()).collect::<Vec<_>>();
        self.information_busy = true;
        let sql = self.session.sql.text();
        let stored_rows = result.store_row_count();
        let scope = if self.is_table() {
            format!("Column profiles cover all {stored_rows} source rows, ignoring search, slicers, and filters.")
        } else {
            format!("Column profiles cover all {stored_rows} result rows, ignoring search.")
        };
        let panel = InformationPanel::new(schema, result.row_count(), scope, sql);
        self.open_dialog(Dialog::Information(panel), None, cx);
        let store = result.store.clone();
        let summary_store = store.clone();
        let summary_names = column_names.clone();
        cx.spawn(
            move || {
                summary_store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?
                    .column_summaries(&summary_names)
            },
            move |view: &mut Self, cx, summaries| {
                if let Some(Dialog::Information(panel)) = &mut view.dialog {
                    panel.finish(summaries.clone());
                }
                if view.information_revision == revision && view.session.result.result_revision == result_revision {
                    view.information_busy = false;
                    if let Err(error) = &summaries {
                        view.error = Some(format!("Information failed: {error}"));
                    }
                }
                cx.notify();
                if summaries.is_ok() {
                    view.profile_top_values(revision, store, column_names, 0, cx);
                }
            },
        )
        .detach();
    }

    /// Reads each column's most frequent values, one column per lock, so
    /// searching is not blocked behind every scan.
    fn profile_top_values(&mut self, revision: u64, store: std::sync::Arc<std::sync::Mutex<crate::infrastructure::store::Store>>, names: Vec<String>, index: usize, cx: &mut Cx<Self>) {
        if self.information_revision != revision || index >= names.len() {
            return;
        }
        let column = names[index].clone();
        let column_store = store.clone();
        let read = column.clone();
        cx.spawn(
            move || {
                column_store
                    .lock()
                    .map_err(|_| "DuckDB store lock poisoned".to_owned())?
                    .top_values(&read, crate::ui::info::TOP_VALUE_COUNT)
            },
            move |view: &mut Self, cx, values| {
                if let Some(Dialog::Information(panel)) = &mut view.dialog {
                    panel.finish_top(&column, values);
                }
                cx.notify();
                view.profile_top_values(revision, store, names, index + 1, cx);
            },
        )
        .detach();
    }

    pub(crate) fn open_pivot_panel(&mut self, kind: PivotFieldKind, cx: &mut Cx<Self>) {
        let config = self.session.query_config();
        let (configured, blocked) = match kind {
            PivotFieldKind::GroupBy => (config.group_by, config.pivot_by),
            PivotFieldKind::PivotBy => (config.pivot_by, config.group_by),
        };
        let schema = self.session.source.schema.to_vec();
        let panel = PivotPanel::new(schema, configured, kind, blocked, self.session.source.engine);
        let mut focus = None;
        panel.focus_available(&mut focus);
        self.open_dialog(Dialog::Pivot(panel), focus, cx);
    }

    /// Opens the group or pivot editor with a sample expression applied, for
    /// screenshots.
    pub(crate) fn capture_dimension_editor(&mut self, target: &str, cx: &mut Cx<Self>) {
        let kind = if target.starts_with("editor:group") {
            PivotFieldKind::GroupBy
        } else {
            PivotFieldKind::PivotBy
        };
        let error = target.ends_with("-error");
        if kind == PivotFieldKind::PivotBy && !error && self.session.query_config().group_by.is_empty() {
            let mut config = self.session.query_config();
            config.group_by = vec![Dimension::expression("floor(\"Row\" / 10) AS Batch")];
            if let Err(error) = self.session.set_query_config(config, self.suggested_name()) {
                self.error = Some(error);
                return;
            }
        }
        self.open_pivot_panel(kind, cx);
        let mut focus = self.focus;
        if let Some(Dialog::Pivot(panel)) = &mut self.dialog {
            panel.start_custom(&mut focus);
            let sql = if error {
                "upper(\"missing column\") AS Label"
            } else if kind == PivotFieldKind::GroupBy {
                "floor(\"Row\" / 10) AS Batch"
            } else {
                "\"Row\" % 3 AS Kind"
            };
            panel.sql.set_text(sql);
            panel.apply();
        }
        self.set_focus(focus, cx);
        self.handle_panel_events(cx);
    }

    pub(crate) fn open_order_by_panel(&mut self, cx: &mut Cx<Self>) {
        let columns = self.result_column_names();
        let panel = OrderByPanel::new(columns, self.session.sort.clone());
        let mut focus = None;
        panel.focus_available(&mut focus);
        self.open_dialog(Dialog::OrderBy(panel), focus, cx);
    }

    fn apply_order_by(&mut self, edit: OrderByEdit, cx: &mut Cx<Self>) {
        let changed = match &edit {
            OrderByEdit::Add(column) => {
                if self.session.sort.iter().any(|key| key.column == *column) {
                    false
                } else {
                    self.session.set_sort_direction(column.clone(), false);
                    true
                }
            }
            OrderByEdit::Remove(column) => self.session.clear_sort_column(column),
            OrderByEdit::Move { column, direction } => self.session.move_sort_key(column, *direction),
            OrderByEdit::ToggleDirection(column) => {
                if let Some(descending) = self
                    .session
                    .sort
                    .iter()
                    .find(|key| key.column == *column)
                    .map(|key| !key.descending)
                {
                    self.session.set_sort_direction(column.clone(), descending);
                    true
                } else {
                    false
                }
            }
            OrderByEdit::SetDirection { column, descending } => {
                if self
                    .session
                    .sort
                    .iter()
                    .find(|key| key.column == *column)
                    .is_some_and(|key| key.descending != *descending)
                {
                    self.session.set_sort_direction(column.clone(), *descending);
                    true
                } else {
                    false
                }
            }
        };
        if !changed {
            return;
        }
        let sort = self.session.sort.clone();
        self.grid.set_sort(sort.clone());
        if let Some(Dialog::OrderBy(panel)) = &mut self.dialog {
            panel.set_configured(sort);
        }
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    pub(crate) fn open_aggregation_panel(&mut self, cx: &mut Cx<Self>) {
        let config = self.session.query_config();
        let schema = self.session.source.schema.to_vec();
        let engine = self.session.source.engine;
        let panel = AggregationPanel::new(schema, config.aggregations, !config.pivot_by.is_empty(), engine);
        let id = panel.id;
        let mut focus = None;
        panel.focus_available(&mut focus);
        self.open_dialog(Dialog::Aggregation(panel), focus, cx);
        let functions = move || match engine {
            crate::infrastructure::store::Engine::DuckDb => crate::infrastructure::sql_execution::aggregate_functions(),
            crate::infrastructure::store::Engine::Polars => Ok(crate::infrastructure::polars::functions::aggregate_functions()),
        };
        cx.spawn(functions, move |view: &mut Self, cx, result| {
            if let Some(Dialog::Aggregation(panel)) = &mut view.dialog
                && panel.id == id
            {
                panel.set_functions(result);
                cx.notify();
            }
        })
        .detach();
    }

    /// Handles what the open group, pivot, or aggregation editor asked for.
    pub(crate) fn handle_panel_events(&mut self, cx: &mut Cx<Self>) {
        loop {
            match &mut self.dialog {
                Some(Dialog::Pivot(panel)) => {
                    let Some(event) = panel.take_event() else {
                        return;
                    };
                    let (id, kind) = (panel.id, panel.kind());
                    self.check_dimension(kind, id, event, cx);
                }
                Some(Dialog::Aggregation(panel)) => {
                    let Some(event) = panel.take_event() else {
                        return;
                    };
                    let id = panel.id;
                    self.check_aggregation(id, event, cx);
                }
                _ => return,
            }
        }
    }

    pub(crate) fn palette(&mut self, title: &str, placeholder: &str, items: Vec<Item>, kind: PaletteKind, cx: &mut Cx<Self>) {
        let mut palette = Palette::new(placeholder);
        palette.set_items(items);
        let id = palette.query.id;
        self.open_dialog(
            Dialog::Palette(PaletteDialog {
                title: title.into(),
                palette,
                kind,
                footer: None,
            }),
            Some(id),
            cx,
        );
    }

    pub(crate) fn open_column_jump(&mut self, cx: &mut Cx<Self>) {
        let result = self.session.result.clone();
        if result.column_count() == 0 {
            return;
        }
        let items = result
            .columns
            .iter()
            .zip(result.dtypes.iter())
            .map(|(column, dtype)| Item::new(column.0.clone()).detail(crate::domain::formatting::type_label(dtype).into_owned()))
            .collect();
        self.palette("Jump to column", "Find a column", items, PaletteKind::ColumnJump, cx);
    }

    pub(crate) fn open_font_picker(&mut self, target: FontTarget, cx: &mut Cx<Self>) {
        let mut families = text::font_families();
        families.sort_by_key(|family| family.to_lowercase());
        families.dedup();
        let title = match target {
            FontTarget::Grid => "Fonts",
            FontTarget::Editor => {
                // Code reads best in a monospaced font, so those come first.
                families.sort_by_key(|family| !crate::ui::font_picker::looks_monospaced(family));
                "Editor font"
            }
        };
        let default_family = crate::ui::font_picker::default_family(target);
        let mut fonts = vec![None];
        let mut items = vec![{
            let mut item = Item::new(format!("Default · {default_family}"));
            item.preview_family = Some(default_family.into());
            item
        }];
        for family in families {
            let mut item = Item::new(family.clone());
            item.preview_family = Some(family.clone());
            items.push(item);
            fonts.push(Some(family));
        }
        self.palette(title, "Find a font", items, PaletteKind::Fonts(target, fonts), cx);
    }

    pub(crate) fn open_column_chooser(&mut self, cx: &mut Cx<Self>) {
        if !self.is_table() {
            self.error = Some("Hiding columns applies to tables, not query or SQL results".into());
            cx.notify();
            return;
        }
        let order = self.session.effective_table_config().column_order;
        let items = self.column_chooser_items(&order);
        let mut palette_title = "Show or hide columns";
        let _ = &mut palette_title;
        self.palette(palette_title, "Find a column; Enter shows or hides it", items, PaletteKind::ColumnChooser(order), cx);
    }

    fn column_chooser_items(&self, order: &[String]) -> Vec<Item> {
        let config = self.session.effective_table_config();
        let source = &self.session.source;
        order
            .iter()
            .map(|name| {
                let hidden = config.hidden.contains(name);
                let dtype = source
                    .columns
                    .iter()
                    .position(|column| &column.0 == name)
                    .map(|index| crate::domain::formatting::type_label(&source.dtypes[index]).into_owned())
                    .unwrap_or_default();
                let mut item = Item::new(name.clone()).detail(dtype);
                item.check = Some(!hidden);
                item.muted = hidden;
                item
            })
            .collect()
    }

    pub(crate) fn open_command_palette(&mut self, cx: &mut Cx<Self>) {
        let commands = active_commands()
            .into_iter()
            .filter(|c| {
                (!self.session.is_sql()
                    || match c.id {
                        // A SQL source opens its rows to build these on.
                        CommandId::Group | CommandId::Pivot | CommandId::Aggregation | CommandId::OrderBy => self.sql_source_session(),
                        CommandId::AddSlicer => self.sql_result_complete,
                        _ => true,
                    })
                    // The SQL editor's own commands, while it is open.
                    && (self.sql_panel_open && self.sql_panel.is_some()
                        || super::sql_editor::sql_editor_action(c.id).is_none()
                            && !matches!(c.id, CommandId::RunSql | CommandId::RunAllSql))
            })
            .collect::<Vec<_>>();
        let shown = matching_commands(&commands, &self.keymap, "");
        let items = shown.iter().map(command_item).collect();
        let listed = shown.iter().map(|(command, ..)| *command).collect();
        self.palette("Commands", "Find a command", items, PaletteKind::Commands(commands, listed), cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.palette.filterable = false;
        }
    }

    /// The command palette filters itself, since its labels include shortcuts.
    fn refresh_command_palette(&mut self) {
        let keymap = self.keymap.clone();
        let Some(Dialog::Palette(dialog)) = &mut self.dialog else {
            return;
        };
        let PaletteKind::Commands(commands, listed) = &mut dialog.kind else {
            return;
        };
        let shown = matching_commands(commands, &keymap, dialog.palette.query_text());
        *listed = shown.iter().map(|(command, ..)| *command).collect();
        dialog.palette.set_items(shown.iter().map(command_item).collect());
        dialog.palette.list.select_item(Some(0));
    }

    pub(crate) fn open_theme_editor(&mut self, cx: &mut Cx<Self>) {
        let form = ThemeEditor::new(self.theme.clone());
        let mut focus = None;
        form.focus_first(&mut focus);
        self.open_dialog(Dialog::Theme(form), focus, cx);
    }

    fn save_theme_dialog(&mut self, cx: &mut Cx<Self>) -> bool {
        let Some(Dialog::Theme(form)) = &mut self.dialog else {
            return false;
        };
        let theme = match form.build() {
            Ok(theme) => theme,
            Err(error) => {
                form.set_error(error);
                return false;
            }
        };
        match self.save_theme(theme, cx) {
            Ok(()) => true,
            Err(error) => {
                if let Some(Dialog::Theme(form)) = &mut self.dialog {
                    form.set_error(format!("Could not save theme.json: {error}"));
                }
                false
            }
        }
    }

    pub(crate) fn open_keymap_editor(&mut self, cx: &mut Cx<Self>) {
        if self.keymap_editor.is_some() {
            return;
        }
        let editor = KeymapEditor::new(self.keymap.clone());
        let id = editor.search.id;
        self.keymap_editor = Some(editor);
        self.set_focus(Some(id), cx);
        cx.notify();
    }

    pub(crate) fn handle_keymap_editor_events(&mut self, cx: &mut Cx<Self>) {
        let Some(editor) = &mut self.keymap_editor else {
            return;
        };
        for event in editor.take_events() {
            match event {
                KeymapEditorEvent::Changed(keymap) => match self.repository.save_keymap(&keymap) {
                    Ok(()) => {
                        self.keymap = keymap.clone();
                        let profile = self.keymap_profile.clone();
                        profiles::changed(|profiles| {
                            profiles.keymaps.insert(profile, keymap);
                        });
                        cx.notify_all();
                    }
                    Err(error) => self.error = Some(format!("Could not save keymap.json: {error}")),
                },
                KeymapEditorEvent::Back => {
                    self.keymap_editor = None;
                    self.focus_grid(cx);
                    return;
                }
            }
        }
    }

    pub(crate) fn open_file_palette(&mut self, cx: &mut Cx<Self>) {
        self.file_revision += 1;
        let revision = self.file_revision;
        self.file_loading = true;
        self.file_groups = std::array::from_fn(|_| Vec::new());
        let repository = self.repository.clone();
        let database = self.database.clone();
        self.palette("Files and views", "Find a file or view", Vec::new(), PaletteKind::Files(Vec::new()), cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.palette.filterable = false;
            dialog.palette.list.empty_text = "Scanning files…".into();
        }
        cx.spawn(
            move || {
                let state = ApplicationState::load(repository).map_err(|error| error.to_string())?;
                let mut groups: [Vec<FileEntry>; 6] = std::array::from_fn(|_| Vec::new());
                for view in state.views {
                    match view.view {
                        ViewKind::Table(_) => groups[0].push(FileEntry::View(view)),
                        ViewKind::Query(_) => groups[1].push(FileEntry::View(view)),
                        // A SQL source opens as its rows, like the other sources.
                        ViewKind::Sql(_) if view.is_sql_script() => groups[5].push(FileEntry::View(view)),
                        ViewKind::Sql(_) => groups[3].push(FileEntry::View(DocumentDefinition::draft_sql_source(&view))),
                    }
                }
                let folders = state.settings.watched_folders.iter().map(PathBuf::from).collect::<Vec<_>>();
                let mut sources = scan_sources(&folders);
                sources.extend(state.settings.watched_files.iter().map(PathBuf::from));
                sources.extend(state.history.recent_sources.into_iter().map(PathBuf::from));
                let mut seen = std::collections::HashSet::new();
                sources.retain(|path| seen.insert(path.to_string_lossy().to_lowercase()));
                sources.sort_by_key(|path| path.to_string_lossy().to_lowercase());
                groups[2] = sources.into_iter().map(FileEntry::Source).collect();
                let file_views = state.settings.picker_file_views;
                groups[4] = database
                    .views()?
                    .into_iter()
                    .filter(|view| file_views || view.schema != crate::infrastructure::source_database::FILE_VIEWS_SCHEMA)
                    .map(DocumentDefinition::draft_database_view)
                    .map(FileEntry::View)
                    .collect();
                let hidden = state.settings.hidden_files.iter().map(|path| path.to_lowercase()).collect();
                Ok::<_, String>((groups, hidden))
            },
            move |view: &mut Self, cx, entries| {
                if view.file_revision != revision {
                    return;
                }
                view.file_loading = false;
                match entries {
                    Ok((groups, hidden)) => {
                        view.file_groups = groups;
                        view.file_hidden = hidden;
                    }
                    Err(error) => view.error = Some(format!("File scan failed: {error}")),
                }
                view.refresh_file_palette();
                cx.notify();
            },
        )
        .detach();
    }

    /// Groups the scanned files and views under the palette's query.
    fn refresh_file_palette(&mut self) {
        let loading = self.file_loading;
        let Some(Dialog::Palette(dialog)) = &mut self.dialog else {
            return;
        };
        let PaletteKind::Files(sections) = &mut dialog.kind else {
            return;
        };
        let query = dialog.palette.query_text().to_lowercase();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        let (hidden, show_hidden) = (&self.file_hidden, self.file_show_hidden);
        let is_hidden = |entry: &FileEntry| entry.hidden_key().is_some_and(|key| hidden.contains(&key));
        let groups = self.file_groups.each_ref().map(|group| {
            group
                .iter()
                .filter(|entry| show_hidden || !is_hidden(entry))
                .filter(|entry| matches_terms(&entry.search_text(), &terms))
                .cloned()
                .collect::<Vec<_>>()
        });
        let mut titles = vec!["Table views".to_owned(), "Query views".to_owned()];
        let mut new_sections = vec![groups[0].clone(), groups[1].clone()];
        if !groups[3].is_empty() {
            titles.push("SQL sources".into());
            new_sections.push(groups[3].clone());
        }
        if !groups[5].is_empty() {
            titles.push("SQL scripts".into());
            new_sections.push(groups[5].clone());
        }
        if !groups[4].is_empty() {
            titles.push("Database views".into());
            new_sections.push(groups[4].clone());
        }
        let mut sources_by_parent = std::collections::BTreeMap::new();
        for entry in &groups[2] {
            let Some(folder) = entry.source_parent() else {
                continue;
            };
            sources_by_parent
                .entry(folder.to_lowercase())
                .or_insert_with(|| (folder, Vec::new()))
                .1
                .push(entry.clone());
        }
        for (_, (folder, entries)) in sources_by_parent {
            titles.push(format!("Sources · {folder}"));
            new_sections.push(entries);
        }
        let items = new_sections
            .iter()
            .enumerate()
            .flat_map(|(section, entries)| {
                entries.iter().map(move |entry| {
                    let mut item = Item::new(entry.label()).section(section);
                    if is_hidden(entry) {
                        item.muted = true;
                        item.detail = Some("Hidden".into());
                    }
                    item
                })
            })
            .collect();
        *sections = new_sections;
        dialog.palette.list.set_sections(titles);
        dialog.palette.set_items(items);
        dialog.palette.list.empty_text = if loading { "Scanning files…" } else { "No files or views" }.into();
    }

    /// Hides the selected file from the file picker, or shows it again.
    fn toggle_selected_file_hidden(&mut self, cx: &mut Cx<Self>) {
        let Some(FileEntry::Source(path)) = self.selected_file_entry() else {
            return;
        };
        let key = path.to_string_lossy().to_lowercase();
        let hide = !self.file_hidden.contains(&key);
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.hidden_files.retain(|hidden| hidden.to_lowercase() != key);
            if hide {
                settings.hidden_files.push(path.to_string_lossy().into_owned());
            }
            self.repository.save_settings(&settings)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save settings: {error}"));
            cx.notify();
            return;
        }
        let selected = match &self.dialog {
            Some(Dialog::Palette(dialog)) => dialog.palette.selected_item(),
            _ => None,
        };
        if hide {
            self.file_hidden.insert(key);
        } else {
            self.file_hidden.remove(&key);
        }
        self.refresh_file_palette();
        // A file that left the list hands the highlight to the one after it.
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog
            && let Some(selected) = selected
            && let Some(last) = dialog.palette.list.visible_count().checked_sub(1)
        {
            dialog.palette.list.select_item(Some(selected.min(last)));
        }
        cx.notify();
    }

    /// Lists hidden files in the file picker, dimmed, or leaves them out.
    fn toggle_show_hidden_files(&mut self, cx: &mut Cx<Self>) {
        self.file_show_hidden = !self.file_show_hidden;
        self.refresh_file_palette();
        cx.notify();
    }

    fn selected_file_entry(&self) -> Option<FileEntry> {
        let Some(Dialog::Palette(dialog)) = &self.dialog else {
            return None;
        };
        let PaletteKind::Files(sections) = &dialog.kind else {
            return None;
        };
        let index = dialog.palette.selected_item()?;
        sections.iter().flatten().nth(index).cloned()
    }

    /// Archives the selected saved view, or the SQL source whose rows the
    /// selected entry shows.
    fn archive_selected_view(&mut self, cx: &mut Cx<Self>) {
        let Some(FileEntry::View(definition)) = self.selected_file_entry().filter(FileEntry::is_archivable) else {
            return;
        };
        let definition = match &definition.sql_source {
            Some(id) if definition.id.is_empty() => match self.repository.load_view(id) {
                Ok(source) => source,
                Err(error) => {
                    self.error = Some(format!("Cannot find the SQL source: {error}"));
                    cx.notify();
                    return;
                }
            },
            _ => definition,
        };
        self.close_dialog(cx);
        self.request_archive(&definition, cx);
    }

    /// Asks before deleting the selected archived view for good.
    fn delete_selected_archive(&mut self, cx: &mut Cx<Self>) {
        let Some(Dialog::Palette(dialog)) = &self.dialog else {
            return;
        };
        let PaletteKind::Recovery(paths) = &dialog.kind else {
            return;
        };
        let Some(path) = dialog.palette.selected_item().and_then(|index| paths.get(index)).cloned() else {
            return;
        };
        let name = self
            .repository
            .load_archives()
            .0
            .into_iter()
            .find(|(archive_path, _)| *archive_path == path)
            .map_or_else(|| "this view".to_owned(), |(_, archive)| archive.original_name);
        self.close_dialog(cx);
        self.confirm(
            format!("Delete {name} permanently?"),
            "This removes it from the archive and cannot be undone.".into(),
            "Delete".into(),
            true,
            ConfirmAction::DeleteArchive(path),
            cx,
        );
    }

    pub(crate) fn open_recovery_palette(&mut self, cx: &mut Cx<Self>) {
        let (archives, issues) = self.repository.load_archives();
        if let Some(issue) = issues.first() {
            self.error = Some(format!("Cannot read archive {}: {}", issue.path.display(), issue.message));
        }
        let items = archives
            .iter()
            .map(|(_, archive)| {
                Item::new(format!(
                    "{} · {} · {}",
                    archive.original_name,
                    archive.original_kind,
                    local_archive_time(archive.deleted_at_unix_ms)
                ))
            })
            .collect();
        let paths = archives.into_iter().map(|(path, _)| path).collect();
        self.palette("Recover archived views", "Find an archived view or SQL source", items, PaletteKind::Recovery(paths), cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.palette.list.empty_text = "No archived views".into();
            dialog.footer = Some("Enter recovers · Shift+Delete deletes permanently".into());
        }
    }

    pub(crate) fn open_folder_palette(&mut self, cx: &mut Cx<Self>) {
        let state = match ApplicationState::load(self.repository.clone()) {
            Ok(state) => state,
            Err(error) => {
                self.error = Some(format!("Cannot load folders: {error}"));
                cx.notify();
                return;
            }
        };
        let folders = state.settings.watched_folders;
        let items = std::iter::once(Item::new("Add folder…"))
            .chain(folders.iter().map(|folder| Item::new(format!("Remove · {folder}"))))
            .collect();
        self.palette("Watched folders", "Find a watched folder", items, PaletteKind::Folders(folders), cx);
    }

    pub(crate) fn add_folder_with_picker(&mut self, cx: &mut Cx<Self>) {
        let chosen = system::prompt_for_paths(
            cx.hwnd(),
            system::PathPrompt {
                directories: true,
                multiple: false,
                title: "Add watched folder",
            },
        );
        match chosen {
            Ok(Some(paths)) => {
                if let Some(path) = paths.first() {
                    let result = ApplicationState::load(self.repository.clone()).and_then(|mut state| state.add_watched_folder(path));
                    self.error = result.err().map(|error| format!("Add folder failed: {error}"));
                    self.sync_file_views(false, cx);
                }
            }
            Ok(None) => {}
            Err(error) => self.error = Some(format!("Folder picker failed: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn remove_folder(&mut self, folder: &str, cx: &mut Cx<Self>) {
        let result = ApplicationState::load(self.repository.clone()).and_then(|mut state| state.remove_watched_folder(folder));
        self.error = result.err().map(|error| format!("Remove folder failed: {error}"));
        self.sync_file_views(false, cx);
        cx.notify();
    }

    pub(crate) fn open_favorites_palette(&mut self, cx: &mut Cx<Self>) {
        let (favorites, views) = match ApplicationState::load(self.repository.clone()) {
            Ok(state) => (state.settings.favorites, state.views),
            Err(error) => {
                self.error = Some(format!("Cannot load favorites: {error}"));
                cx.notify();
                return;
            }
        };
        let items = favorites
            .iter()
            .enumerate()
            .map(|(index, target)| {
                let (label, keyword) = match target {
                    Some(FavoriteTarget::Source(path)) => (short_source(&PathBuf::from(path)), path.clone()),
                    Some(FavoriteTarget::View { view_id }) => views
                        .iter()
                        .find(|view| &view.id == view_id)
                        .map(|view| (view.name.clone(), view.source_path.clone()))
                        .unwrap_or_else(|| ("Missing saved view".into(), view_id.clone())),
                    None => ("Empty".into(), String::new()),
                };
                Item::new(format!("{} · {label}", index + 1)).keywords(keyword)
            })
            .collect();
        self.palette("Favorites", "Find a favorite", items, PaletteKind::Favorites, cx);
        if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
            dialog.footer = Some("Ctrl+Shift+1…9 assigns the current source or saved view".into());
        }
    }

    pub(crate) fn open_slicer_picker(&mut self, cx: &mut Cx<Self>) {
        self.close_slicer_picker();
        let picker = SlicerPicker::new();
        let id = picker.command.query.id;
        self.open_dialog(Dialog::SlicerPicker(picker), Some(id), cx);
        self.slicer_discovery = Some(std::sync::Arc::default());
        self.update_slicer_picker(None);
        if matches!(&self.dialog, Some(Dialog::SlicerPicker(picker)) if picker.loading) {
            self.schedule_search(self.session.search_text.clone(), cx);
            self.poll_slicer_progress(cx);
        }
    }

    /// Shows how many columns discovery has checked while the picker waits for it.
    fn poll_slicer_progress(&mut self, cx: &mut Cx<Self>) {
        let progress = self.slicer_discovery.as_ref().and_then(|discovery| discovery.progress());
        let Some(Dialog::SlicerPicker(picker)) = &mut self.dialog else {
            return;
        };
        if !picker.loading {
            return;
        }
        if picker.progress != progress {
            picker.progress = progress;
            cx.notify();
        }
        cx.timer(std::time::Duration::from_millis(100), |view, cx| view.poll_slicer_progress(cx))
            .detach();
    }

    pub(crate) fn open_copy_as(&mut self, cx: &mut Cx<Self>) {
        let Some(selection) = self.grid.selection() else {
            return;
        };
        let result = self.session.result.clone();
        let items = COPY_FORMATS.iter().map(|(_, label)| Item::new(*label)).collect();
        self.palette("Copy As", "Choose a format", items, PaletteKind::CopyAs(result, selection), cx);
    }

    pub(crate) fn open_actions_editor(&mut self, cx: &mut Cx<Self>) {
        self.space_menu_open = false;
        self.command_menu_open = false;
        self.action_menu_open = false;
        let columns = self.result_column_names();
        let actions = self.view_actions();
        let row = crate::domain::actions::row_values(&self.session.result, self.active_row()).ok();
        let editor = ActionsEditor::new(actions, columns, row);
        let mut focus = None;
        editor.focus(&mut focus);
        self.open_dialog(Dialog::Actions(editor), focus, cx);
    }

    fn handle_actions_events(&mut self, cx: &mut Cx<Self>) {
        let Some(Dialog::Actions(editor)) = &mut self.dialog else {
            return;
        };
        for event in editor.take_events() {
            let result = match event {
                ActionsEditorEvent::Changed(actions) => {
                    self.ensure_working_view();
                    self.session.edit_definition(|definition| definition.actions = actions.clone())
                }
                ActionsEditorEvent::Run(action) => self.launch_action(&action),
            };
            if let Some(Dialog::Actions(editor)) = &mut self.dialog {
                editor.set_error(result.err());
            }
        }
        cx.notify();
    }

    pub(crate) fn open_ai_settings(&mut self, cx: &mut Cx<Self>) {
        self.space_menu_open = false;
        self.command_menu_open = false;
        self.action_menu_open = false;
        let settings = match self.repository.load_settings() {
            Ok(settings) => settings.ai,
            Err(error) => {
                self.error = Some(format!("Cannot read settings: {error}"));
                cx.notify();
                return;
            }
        };
        let editor = AiSettingsEditor::new(settings);
        let mut focus = None;
        editor.focus(&mut focus);
        self.open_dialog(Dialog::AiSettings(editor), focus, cx);
    }

    fn handle_ai_settings_events(&mut self, test: Option<crate::domain::ai::AiProvider>, cx: &mut Cx<Self>) {
        let Some(Dialog::AiSettings(editor)) = &mut self.dialog else {
            return;
        };
        for event in editor.take_events() {
            let AiSettingsEvent::Changed(ai) = event;
            let result = self.repository.load_settings().and_then(|mut settings| {
                settings.ai = ai.clone();
                self.repository.save_settings(&settings)
            });
            if let Err(error) = result {
                self.error = Some(format!("Cannot save AI providers: {error}"));
            }
        }
        if let Some(provider) = test {
            cx.spawn(
                move || crate::infrastructure::ai_client::test_connection(&provider),
                |view: &mut Self, cx, result| {
                    if let Some(Dialog::AiSettings(editor)) = &mut view.dialog {
                        editor.finish_test(result);
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        cx.notify();
    }

    /// An item chosen in a palette dialog.
    pub(crate) fn confirm_palette(&mut self, index: usize, modifiers: Modifiers, cx: &mut Cx<Self>) {
        let Some(Dialog::Palette(dialog)) = &self.dialog else {
            return;
        };
        match &dialog.kind {
            PaletteKind::Commands(_, listed) => {
                if let Some(command) = listed.get(index).copied() {
                    self.close_dialog(cx);
                    self.dispatch_command(command.id, cx);
                }
            }
            PaletteKind::Files(sections) => {
                if let Some(entry) = sections.iter().flatten().nth(index).cloned() {
                    self.close_dialog(cx);
                    self.navigate_to_file(entry, modifiers.shift, modifiers.control, false, cx);
                }
            }
            PaletteKind::Folders(folders) => {
                let folder = index.checked_sub(1).and_then(|index| folders.get(index).cloned());
                self.close_dialog(cx);
                match (index, folder) {
                    (0, _) => self.add_folder_with_picker(cx),
                    (_, Some(folder)) => self.remove_folder(&folder, cx),
                    _ => {}
                }
            }
            PaletteKind::Recovery(paths) => {
                if let Some(path) = paths.get(index).cloned() {
                    self.close_dialog(cx);
                    match self.repository.recover(&path) {
                        Ok(_) => self.error = None,
                        Err(error) => self.error = Some(format!("Recovery failed: {error}")),
                    }
                    cx.notify();
                }
            }
            PaletteKind::Favorites => {
                if index < 9 {
                    self.close_dialog(cx);
                    self.favorite(index, false, cx);
                }
            }
            PaletteKind::CopyAs(result, selection) => {
                if let Some((format, _)) = COPY_FORMATS.get(index) {
                    let (result, selection, format) = (result.clone(), *selection, *format);
                    self.schedule_copy_format(result, selection, format, cx);
                    self.close_dialog(cx);
                }
            }
            PaletteKind::ColumnJump => {
                self.close_dialog(cx);
                self.grid.jump_to_column(index);
                self.focus_grid(cx);
                self.drain_grid_events(cx);
            }
            PaletteKind::ColumnChooser(order) => {
                let order = order.clone();
                let Some(name) = order.get(index).cloned() else {
                    return;
                };
                let hidden = self.session.effective_table_config().hidden.contains(&name);
                self.set_columns_hidden(&[name], !hidden, cx);
                // The list stays open; redraw its check marks.
                let items = self.column_chooser_items(&order);
                if let Some(Dialog::Palette(dialog)) = &mut self.dialog {
                    let selected = dialog.palette.selected_item();
                    dialog.palette.set_items(items);
                    dialog.palette.list.select_item(selected);
                }
            }
            PaletteKind::Engines => {
                if let Some(engine) = crate::infrastructure::store::Engine::ALL.get(index).copied() {
                    self.close_dialog(cx);
                    if self.session.is_sql() {
                        self.set_sql_engine(engine, cx);
                    } else {
                        self.set_file_engine(engine, cx);
                    }
                }
            }
            PaletteKind::SqlOutline(ranges) => {
                if let Some(range) = ranges.get(index).cloned() {
                    self.close_dialog(cx);
                    self.go_to_sql_range(range, cx);
                }
            }
            PaletteKind::SqlHistory(runs) => {
                if let Some(sql) = runs.get(index).cloned() {
                    self.close_dialog(cx);
                    self.insert_sql_from_history(&sql, cx);
                }
            }
            PaletteKind::Settings => self.confirm_setting(index, cx),
            PaletteKind::EditorSettings => self.confirm_editor_setting(index, cx),
            PaletteKind::ViewOptions => self.confirm_view_option(index, cx),
            PaletteKind::EditorChoice(option, values) => {
                if let Some(value) = values.get(index).copied() {
                    let option = *option;
                    self.choose_editor_setting(option, value, cx);
                }
            }
            PaletteKind::Fonts(target, fonts) => {
                if let Some(family) = fonts.get(index).cloned() {
                    let target = *target;
                    self.close_dialog(cx);
                    let mut font = self.font.clone();
                    match target {
                        FontTarget::Grid => font.family = family,
                        FontTarget::Editor => font.editor_family = family,
                    }
                    self.save_font(font, cx);
                }
            }
        }
        cx.notify();
    }

    /// Keys while a dialog is open. Returns whether the dialog used the key.
    pub(crate) fn dialog_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let mut focus = self.focus;
        let handled = match &mut self.dialog {
            None => return false,
            Some(Dialog::Save { name }) => {
                let outcome = name.key(event);
                if outcome.submit {
                    if self.save_from_dialog(cx) {
                        self.close_dialog(cx);
                    }
                    return true;
                }
                outcome.handled
            }
            Some(Dialog::ResultFilter { input, error, .. }) => {
                let outcome = input.key(event);
                if outcome.submit {
                    if self.apply_result_filter_dialog(cx) {
                        self.close_dialog(cx);
                    }
                    return true;
                }
                if outcome.changed {
                    *error = None;
                }
                outcome.handled
            }
            Some(Dialog::PasteFilter(_)) => return self.paste_filter_key(event, cx),
            Some(Dialog::Confirm(confirm)) => {
                if event.is("enter") {
                    let action = confirm.action.clone();
                    self.close_dialog(cx);
                    self.run_confirmed(action, cx);
                    return true;
                }
                false
            }
            Some(Dialog::Information(panel)) => {
                if focus == Some(panel.editor.id) {
                    panel.editor.key(event).handled
                } else {
                    false
                }
            }
            Some(Dialog::Pivot(panel)) => {
                let result = panel.key(event, &mut focus);
                if result.close {
                    self.focus = focus;
                    self.close_dialog(cx);
                    return true;
                }
                let handled = result.handled;
                if let Some(event) = result.event {
                    let (id, kind) = (panel.id, panel.kind());
                    self.focus = focus;
                    self.check_dimension(kind, id, event, cx);
                    self.handle_panel_events(cx);
                    return true;
                }
                handled
            }
            Some(Dialog::Aggregation(panel)) => {
                let result = panel.key(event, &mut focus);
                let handled = result.handled;
                if let Some(event) = result.event {
                    let id = panel.id;
                    self.focus = focus;
                    self.check_aggregation(id, event, cx);
                    self.handle_panel_events(cx);
                    return true;
                }
                handled
            }
            Some(Dialog::OrderBy(panel)) => {
                let (handled, edit) = panel.key(event, &mut focus);
                self.focus = focus;
                if let Some(edit) = edit {
                    self.apply_order_by(edit, cx);
                }
                return handled;
            }
            Some(Dialog::Palette(dialog)) => {
                if event.key == "delete" && event.modifiers.control && matches!(dialog.kind, PaletteKind::Files(_)) {
                    self.archive_selected_view(cx);
                    return true;
                }
                if event.key == "h" && event.modifiers.control && !event.modifiers.alt && matches!(dialog.kind, PaletteKind::Files(_)) {
                    if event.modifiers.shift {
                        self.toggle_show_hidden_files(cx);
                    } else {
                        self.toggle_selected_file_hidden(cx);
                    }
                    return true;
                }
                if event.key == "delete" && event.modifiers.shift && matches!(dialog.kind, PaletteKind::Recovery(_)) {
                    self.delete_selected_archive(cx);
                    return true;
                }
                let outcome = dialog.palette.key(event);
                if let Some(index) = outcome.confirmed {
                    self.confirm_palette(index, event.modifiers, cx);
                    return true;
                }
                if outcome.query_changed {
                    self.palette_query_changed();
                }
                outcome.handled
            }
            Some(Dialog::SlicerPicker(picker)) => {
                let outcome = picker.command.key(event);
                if let Some(index) = outcome.confirmed {
                    if let Some(choice) = picker.choice(index) {
                        self.choose_slicer(choice, cx);
                        self.close_dialog(cx);
                    }
                    return true;
                }
                outcome.handled
            }
            Some(Dialog::Theme(form)) => {
                let (handled, submit) = form.key(event, &mut focus);
                self.focus = focus;
                if submit && self.save_theme_dialog(cx) {
                    self.close_dialog(cx);
                }
                return handled;
            }
            Some(Dialog::Actions(editor)) => {
                let handled = editor.key(event, &mut focus);
                self.focus = focus;
                self.handle_actions_events(cx);
                return handled;
            }
            Some(Dialog::AiSettings(editor)) => {
                let handled = editor.key(event, &mut focus);
                self.focus = focus;
                self.handle_ai_settings_events(None, cx);
                return handled;
            }
        };
        self.focus = focus;
        handled
    }

    fn palette_query_changed(&mut self) {
        let Some(Dialog::Palette(dialog)) = &self.dialog else {
            return;
        };
        match dialog.kind {
            PaletteKind::Commands(..) => self.refresh_command_palette(),
            PaletteKind::Files(_) => self.refresh_file_palette(),
            _ => {}
        }
    }

    pub(crate) fn dialog_text(&mut self, typed: &str, cx: &mut Cx<Self>) -> bool {
        let focus = self.focus;
        let used = match &mut self.dialog {
            None => return false,
            Some(Dialog::Save { name }) => focus == Some(name.id) && name.type_text(typed).handled,
            Some(Dialog::ResultFilter { input, error, .. }) => {
                *error = None;
                focus == Some(input.id) && input.type_text(typed).handled
            }
            Some(Dialog::Pivot(panel)) => panel.type_text(typed, focus),
            Some(Dialog::Aggregation(panel)) => panel.type_text(typed, focus),
            Some(Dialog::OrderBy(panel)) => panel.type_text(typed, focus),
            Some(Dialog::Palette(dialog)) => {
                if focus != Some(dialog.palette.query.id) {
                    return false;
                }
                if dialog.palette.type_text(typed).query_changed {
                    self.palette_query_changed();
                }
                true
            }
            Some(Dialog::SlicerPicker(picker)) => {
                focus == Some(picker.command.query.id) && {
                    picker.command.type_text(typed);
                    true
                }
            }
            Some(Dialog::Theme(form)) => form.type_text(typed, focus),
            Some(Dialog::Actions(editor)) => editor.type_text(typed, focus),
            Some(Dialog::AiSettings(editor)) => editor.type_text(typed, focus),
            Some(Dialog::Information(_) | Dialog::Confirm(_) | Dialog::PasteFilter(_)) => false,
        };
        cx.notify();
        used
    }

    /// A press inside the open dialog.
    pub(crate) fn dialog_mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: Modifiers, cx: &mut Cx<Self>) {
        let mut focus = self.focus;
        match hit {
            Hit::Button(Button::DialogClose | Button::DialogCancel) => {
                self.close_dialog(cx);
                return;
            }
            Hit::Button(Button::DialogOk) => {
                match &self.dialog {
                    Some(Dialog::Save { .. }) => {
                        if self.save_from_dialog(cx) {
                            self.close_dialog(cx);
                        }
                    }
                    Some(Dialog::ResultFilter { .. }) => {
                        if self.apply_result_filter_dialog(cx) {
                            self.close_dialog(cx);
                        }
                    }
                    Some(Dialog::Confirm(confirm)) => {
                        let action = confirm.action.clone();
                        self.close_dialog(cx);
                        self.run_confirmed(action, cx);
                    }
                    Some(Dialog::Theme(_)) => {
                        if self.save_theme_dialog(cx) {
                            self.close_dialog(cx);
                        }
                    }
                    Some(Dialog::PasteFilter(_)) => self.apply_paste_filter(cx),
                    _ => {}
                }
                return;
            }
            Hit::Button(Button::PasteFilterMode(exclude)) => {
                if let Some(Dialog::PasteFilter(dialog)) = &mut self.dialog {
                    dialog.exclude = *exclude;
                }
            }
            Hit::Button(Button::ThemeReset) => {
                if let Some(Dialog::Theme(form)) = &mut self.dialog {
                    form.reset();
                }
            }
            Hit::Button(Button::ThemeSwatch(index)) => {
                if let Some(Dialog::Theme(form)) = &mut self.dialog {
                    form.toggle_picker(*index);
                }
            }
            Hit::Button(Button::FilesFolders) => {
                self.close_dialog(cx);
                self.open_folder_palette(cx);
                return;
            }
            Hit::Button(Button::FilesArchive) => {
                self.archive_selected_view(cx);
                return;
            }
            Hit::Button(Button::FilesHide) => {
                self.toggle_selected_file_hidden(cx);
                return;
            }
            Hit::Button(Button::FilesShowHidden) => {
                self.toggle_show_hidden_files(cx);
                return;
            }
            Hit::Button(Button::FilesRecover) => {
                self.close_dialog(cx);
                self.open_recovery_palette(cx);
                return;
            }
            _ => match &mut self.dialog {
                Some(Dialog::Save { name } | Dialog::ResultFilter { input: name, .. }) => {
                    if let Hit::Editor(id, part) = hit
                        && *id == name.id
                    {
                        name.mouse_down(*part, pos, clicks, modifiers.shift);
                        focus = Some(*id);
                    }
                }
                Some(Dialog::Information(panel)) => {
                    panel.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus);
                }
                Some(Dialog::Pivot(panel)) => {
                    let result = panel.mouse_down(hit, pos, clicks, modifiers, &mut focus);
                    self.focus = focus;
                    if result.close {
                        self.close_dialog(cx);
                        return;
                    }
                    if let Some(event) = result.event {
                        let (id, kind) = (panel.id, panel.kind());
                        self.check_dimension(kind, id, event, cx);
                        self.handle_panel_events(cx);
                    }
                    cx.notify();
                    return;
                }
                Some(Dialog::Aggregation(panel)) => {
                    let result = panel.mouse_down(hit, pos, clicks, modifiers, &mut focus);
                    self.focus = focus;
                    if let Some(event) = result.event {
                        let id = panel.id;
                        self.check_aggregation(id, event, cx);
                        self.handle_panel_events(cx);
                    }
                    cx.notify();
                    return;
                }
                Some(Dialog::OrderBy(panel)) => {
                    if let Some(edit) = panel.mouse_down(hit, pos, clicks, &mut focus) {
                        self.focus = focus;
                        self.apply_order_by(edit, cx);
                    }
                }
                Some(Dialog::Palette(dialog)) => {
                    let part = match hit {
                        Hit::Editor(id, part) if *id == dialog.palette.query.id => Some(list::Part::Query(*part)),
                        Hit::List(id, part) if *id == dialog.palette.list.id => Some(*part),
                        _ => None,
                    };
                    if let Some(part) = part {
                        let outcome = dialog.palette.mouse_down(part, pos, clicks, modifiers.shift);
                        focus = Some(dialog.palette.query.id);
                        if let Some(index) = outcome.confirmed {
                            self.focus = focus;
                            self.confirm_palette(index, modifiers, cx);
                            return;
                        }
                    }
                }
                Some(Dialog::SlicerPicker(picker)) => {
                    let part = match hit {
                        Hit::Editor(id, part) if *id == picker.command.query.id => Some(list::Part::Query(*part)),
                        Hit::List(id, part) if *id == picker.command.list.id => Some(*part),
                        _ => None,
                    };
                    if let Some(part) = part {
                        let outcome = picker.command.mouse_down(part, pos, clicks, modifiers.shift);
                        focus = Some(picker.command.query.id);
                        if let Some(choice) = outcome.confirmed.and_then(|index| picker.choice(index)) {
                            self.focus = focus;
                            self.choose_slicer(choice, cx);
                            self.close_dialog(cx);
                            return;
                        }
                    }
                }
                Some(Dialog::Theme(form)) => {
                    form.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus);
                }
                Some(Dialog::Actions(editor)) => {
                    editor.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus);
                    self.focus = focus;
                    self.handle_actions_events(cx);
                    return;
                }
                Some(Dialog::AiSettings(editor)) => {
                    let (_, test) = editor.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus);
                    self.focus = focus;
                    self.handle_ai_settings_events(test, cx);
                    return;
                }
                Some(Dialog::Confirm(_) | Dialog::PasteFilter(_)) | None => {}
            },
        }
        let before = self.focus;
        self.focus = focus;
        self.focus_changed(before, cx);
        cx.notify();
    }

    pub(crate) fn dialog_mouse_drag(&mut self, pos: Point) {
        match &mut self.dialog {
            Some(Dialog::Save { name } | Dialog::ResultFilter { input: name, .. }) => name.mouse_drag(pos),
            Some(Dialog::Information(panel)) => {
                if panel.editor.is_dragging() {
                    panel.editor.mouse_drag(pos)
                }
            }
            Some(Dialog::Pivot(panel)) => panel.mouse_drag(pos),
            Some(Dialog::Aggregation(panel)) => panel.mouse_drag(pos),
            Some(Dialog::OrderBy(panel)) => panel.mouse_drag(pos),
            Some(Dialog::Palette(dialog)) => dialog.palette.mouse_drag(pos),
            Some(Dialog::SlicerPicker(picker)) => picker.command.mouse_drag(pos),
            Some(Dialog::Theme(form)) => form.mouse_drag(pos),
            Some(Dialog::Actions(editor)) => editor.mouse_drag(pos),
            Some(Dialog::AiSettings(editor)) => editor.mouse_drag(pos),
            Some(Dialog::Confirm(_) | Dialog::PasteFilter(_)) | None => {}
        }
    }

    pub(crate) fn dialog_mouse_up(&mut self) {
        match &mut self.dialog {
            Some(Dialog::Save { name } | Dialog::ResultFilter { input: name, .. }) => name.mouse_up(),
            Some(Dialog::Information(panel)) => panel.editor.mouse_up(),
            Some(Dialog::Pivot(panel)) => panel.mouse_up(),
            Some(Dialog::Aggregation(panel)) => panel.mouse_up(),
            Some(Dialog::OrderBy(panel)) => panel.mouse_up(),
            Some(Dialog::Palette(dialog)) => dialog.palette.mouse_up(),
            Some(Dialog::SlicerPicker(picker)) => picker.command.mouse_up(),
            Some(Dialog::Theme(form)) => form.mouse_up(),
            Some(Dialog::Actions(editor)) => editor.mouse_up(),
            Some(Dialog::AiSettings(editor)) => editor.mouse_up(),
            Some(Dialog::Confirm(_) | Dialog::PasteFilter(_)) | None => {}
        }
    }

    pub(crate) fn dialog_wheel(&mut self, hit: &Hit, delta: Point, modifiers: Modifiers) -> bool {
        let shift = modifiers.shift;
        let line = text::line_height(&self.fonts.ui);
        match &mut self.dialog {
            Some(Dialog::Information(panel)) => panel.wheel(hit, delta, shift, line),
            Some(Dialog::Pivot(panel)) => panel.wheel(hit, delta, modifiers),
            Some(Dialog::Aggregation(panel)) => panel.wheel(hit, delta, modifiers),
            Some(Dialog::OrderBy(panel)) => panel.wheel(hit, delta),
            Some(Dialog::Palette(dialog)) => dialog.palette.list.wheel(delta),
            Some(Dialog::SlicerPicker(picker)) => picker.command.list.wheel(delta),
            _ => false,
        }
    }

    /// Draws the open dialog over the window.
    pub(crate) fn paint_dialog(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        // A manager in a window of its own leaves the grid's window clear.
        if !self.modal_dialog() {
            return;
        }
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let fonts = self.fonts.clone();
        let focus = self.focus;
        let (window_width, window_height) = (canvas.width, canvas.height);
        canvas.fill(canvas.bounds(), colors::OVERLAY);
        canvas.hit(canvas.bounds(), Hit::Backdrop);
        let font = &fonts.ui;
        let line = text::line_height(font);
        let padding = 16.0;
        let (width, top) = match dialog {
            Dialog::Information(_) => (
                (window_width * crate::ui::info::DIALOG_FRACTION).max(240.0),
                window_height * (1.0 - crate::ui::info::DIALOG_FRACTION) / 2.0,
            ),
            // These show in a window of their own.
            Dialog::Pivot(_) | Dialog::Aggregation(_) | Dialog::OrderBy(_) => return,
            Dialog::Palette(_) => (PALETTE_WIDTH, 24.0),
            Dialog::Theme(_) => (600.0, 48.0),
            Dialog::Actions(_) => (720.0, 48.0),
            Dialog::AiSettings(_) => (760.0, 48.0),
            Dialog::SlicerPicker(_) | Dialog::Save { .. } | Dialog::Confirm(_) => (520.0, 96.0),
            Dialog::ResultFilter { .. } => (640.0, 96.0),
            Dialog::PasteFilter(_) => (640.0, 96.0),
        };
        let width = width.min(window_width - 16.0).max(200.0);
        let content_width = width - 2.0 * padding;
        let title = dialog.title();
        let title_height = if title.is_empty() { 0.0 } else { line + 16.0 };
        let footer = dialog.footer();
        let footer_height = if footer.is_empty() { 0.0 } else { button_height(font) + 16.0 };
        let max_content = (window_height - top - 24.0 - title_height - footer_height - 2.0 * padding).max(80.0);
        let content_height = match dialog {
            Dialog::Save { .. } => {
                Editor::line_box_height(font) + if self.error.is_some() { line + 8.0 } else { 0.0 }
            }
            Dialog::ResultFilter { error, .. } => {
                let note = error.as_deref().unwrap_or(RESULT_FILTER_HINT);
                Editor::line_box_height(font)
                    + 8.0
                    + text::layout(note, &fonts.small, crate::ui::platform::TextOptions::wrapped(content_width)).height
            }
            Dialog::Confirm(confirm) => {
                text::layout(&confirm.message, font, crate::ui::platform::TextOptions::wrapped(content_width)).height
            }
            Dialog::Information(_) => window_height * crate::ui::info::DIALOG_FRACTION - 110.0,
            Dialog::Pivot(_) | Dialog::Aggregation(_) | Dialog::OrderBy(_) => 0.0,
            Dialog::Palette(dialog) => {
                let list_max = (window_height - 200.0).clamp(100.0, 440.0);
                dialog.palette.height(font, list_max) + if dialog.footer.is_some() || matches!(dialog.kind, PaletteKind::Files(_)) { button_height(font) + 8.0 } else { 0.0 }
            }
            Dialog::SlicerPicker(picker) => {
                picker.command.height(font, 360.0) + if picker.message().is_some() { line + 16.0 } else { 0.0 }
            }
            Dialog::Theme(form) => form.height(&fonts).min((window_height - 210.0).clamp(100.0, 650.0)),
            Dialog::Actions(editor) => editor.height(&fonts, content_width),
            Dialog::AiSettings(editor) => editor.height(&fonts, content_width),
            Dialog::PasteFilter(dialog) => dialog.height(&fonts),
        }
        .min(max_content);
        let height = title_height + content_height + footer_height + 2.0 * padding;
        let x = ((window_width - width) / 2.0).max(8.0);
        let frame = rect(x, top, width, height);
        canvas.shadow(frame, 8.0);
        canvas.fill_rounded(frame, 8.0, colors::POPOVER);
        canvas.stroke_rounded(frame, 8.0, colors::BORDER, 1.0);
        canvas.hit(frame, Hit::Surface);
        if !title.is_empty() {
            let title_rect = rect(x + padding, top + padding, content_width, line + 4.0);
            let mut items = Row::new(title_rect, 8.0);
            let close_label = "×";
            let close = items.right(button_width(close_label, &fonts.small));
            button(
                canvas,
                rect(close.x, close.y - 2.0, close.w, button_height(&fonts.small)),
                close_label,
                &fonts.small,
                ButtonStyle::Ghost,
                true,
                Hit::Button(Button::DialogClose),
            );
            canvas.text(items.rest(), &title, &fonts.large, colors::FOREGROUND, Align::Left);
        }
        let content = rect(x + padding, top + padding + title_height, content_width, content_height);
        match dialog {
            Dialog::Save { name } => {
                let id = name.id;
                name.paint(canvas, content.with_h(Editor::line_box_height(font)), font, focus == Some(id), |part| Hit::Editor(id, part));
                if let Some(error) = &self.error {
                    canvas.text(
                        rect(content.x, content.y + Editor::line_box_height(font) + 8.0, content.w, line),
                        error,
                        font,
                        colors::DANGER,
                        Align::Left,
                    );
                }
            }
            Dialog::ResultFilter { input, error, .. } => {
                let id = input.id;
                let box_height = Editor::line_box_height(font);
                input.paint(canvas, content.with_h(box_height), font, focus == Some(id), |part| Hit::Editor(id, part));
                let note = rect(content.x, content.y + box_height + 8.0, content.w, content.h - box_height - 8.0);
                let (text, color) = match error {
                    Some(error) => (error.as_str(), colors::DANGER),
                    None => (RESULT_FILTER_HINT, colors::MUTED),
                };
                canvas.paragraph(note, text, &fonts.small, color);
            }
            Dialog::Confirm(confirm) => {
                canvas.paragraph(content, &confirm.message, font, colors::FOREGROUND);
            }
            Dialog::Information(panel) => panel.paint(canvas, content, &fonts, focus),
            Dialog::Pivot(_) | Dialog::Aggregation(_) | Dialog::OrderBy(_) => {}
            Dialog::Palette(dialog) => {
                let footer_room = if dialog.footer.is_some() || matches!(dialog.kind, PaletteKind::Files(_)) {
                    button_height(font) + 8.0
                } else {
                    0.0
                };
                let list_area = rect(content.x, content.y, content.w, content.h - footer_room);
                let (query_id, list_id) = (dialog.palette.query.id, dialog.palette.list.id);
                dialog.palette.paint(canvas, list_area, font, focus == Some(query_id), |part| match part {
                    list::Part::Query(part) => Hit::Editor(query_id, part),
                    part => Hit::List(list_id, part),
                });
                let footer_rect = rect(content.x, content.bottom() - button_height(font), content.w, button_height(font));
                if let Some(text_value) = &dialog.footer {
                    canvas.text(footer_rect, text_value, &fonts.small, colors::MUTED, Align::Left);
                }
                if let PaletteKind::Files(sections) = &dialog.kind {
                    let selected = dialog.palette.selected_item().and_then(|index| sections.iter().flatten().nth(index));
                    let archive_enabled = selected.is_some_and(FileEntry::is_archivable);
                    let hidden_key = selected.and_then(FileEntry::hidden_key);
                    let hide_label = if hidden_key.as_ref().is_some_and(|key| self.file_hidden.contains(key)) {
                        "Unhide · Ctrl+H"
                    } else {
                        "Hide · Ctrl+H"
                    };
                    let show_label = if self.file_show_hidden {
                        "Leave out hidden · Ctrl+Shift+H".to_owned()
                    } else {
                        format!("Show hidden ({}) · Ctrl+Shift+H", self.file_hidden.len())
                    };
                    let mut buttons = Row::new(footer_rect, 8.0);
                    for (label, hit, enabled) in [
                        ("Folders", Button::FilesFolders, true),
                        ("Archive · Ctrl+Delete", Button::FilesArchive, archive_enabled),
                        ("Recover", Button::FilesRecover, true),
                        (hide_label, Button::FilesHide, hidden_key.is_some()),
                        (show_label.as_str(), Button::FilesShowHidden, self.file_show_hidden || !self.file_hidden.is_empty()),
                    ] {
                        let r = buttons.left(button_width(label, font));
                        button(canvas, r, label, font, ButtonStyle::Outline, enabled, Hit::Button(hit));
                    }
                }
            }
            Dialog::SlicerPicker(picker) => {
                let message = picker.message();
                let list_area = rect(content.x, content.y, content.w, content.h - if message.is_some() { line + 16.0 } else { 0.0 });
                let (query_id, list_id) = (picker.command.query.id, picker.command.list.id);
                picker.command.paint(canvas, list_area, font, focus == Some(query_id), |part| match part {
                    list::Part::Query(part) => Hit::Editor(query_id, part),
                    part => Hit::List(list_id, part),
                });
                if let Some(message) = message {
                    canvas.text(rect(content.x + 8.0, list_area.bottom() + 8.0, content.w - 16.0, line), &message, font, colors::FOREGROUND, Align::Left);
                }
            }
            Dialog::Theme(form) => {
                canvas.push_clip(content);
                form.paint(canvas, content, &fonts, focus);
                canvas.pop_clip();
            }
            Dialog::Actions(editor) => {
                canvas.push_clip(content);
                editor.paint(canvas, content, &fonts, focus);
                canvas.pop_clip();
            }
            Dialog::AiSettings(editor) => {
                canvas.push_clip(content);
                editor.paint(canvas, content, &fonts, focus);
                canvas.pop_clip();
            }
            Dialog::PasteFilter(dialog) => {
                let matches = match &dialog.target {
                    super::paste_filter::PasteTarget::Slicer(column) => self.slicer_sidebar.pasted_matches(column, &dialog.values),
                    super::paste_filter::PasteTarget::Column { .. } => None,
                };
                dialog.paint(canvas, content, &fonts, matches);
            }
        }
        if !footer.is_empty() {
            let footer_rect = rect(x + padding, frame.bottom() - padding - button_height(font), content_width, button_height(font));
            let mut buttons = Row::new(footer_rect, 8.0);
            for (label, hit, style) in footer {
                let label = if label.is_empty() {
                    match dialog {
                        Dialog::Confirm(confirm) => confirm.action_label.clone(),
                        _ => String::new(),
                    }
                } else {
                    label.to_owned()
                };
                let r = buttons.right(button_width(&label, font));
                button(canvas, r, &label, font, style, true, Hit::Button(hit));
            }
        }
        // Completion menus of the dialog's editors go above it.
        match dialog {
            Dialog::Theme(form) => form.paint_picker(canvas, &fonts),
            _ => {}
        }
        let _ = (scroll::BAR_WIDTH, point(0.0, 0.0), cx.window_id());
    }
}

