//! Commands from the keymap, the palette, and the Space menu.

use super::*;
use crate::application::keymap::{ACTIONS as KEYMAP_ACTIONS, ActionId, canonical_keystroke};
use crate::domain::actions;
use crate::domain::documents::ViewAction;
use crate::infrastructure::launcher;
use crate::ui::cell_detail::CellContent;
use crate::ui::platform::KeyEvent;
use crate::ui::platform::system;
use crate::ui::slicer_sidebar::{SlicerChoice, SlicerEvent, SlicerKind};

impl DataFrameView {
    /// Why slicers and value filters are off for this SQL result, if they are.
    pub(crate) fn sql_filters_unavailable(&self) -> Option<&'static str> {
        (self.session.is_sql() && !self.sql_result_complete).then_some(
            "Slicers and = / - filters need the script's last statement to be a query              (SELECT, WITH, FROM, VALUES, PIVOT, DESCRIBE), run outside a transaction",
        )
    }

    pub(crate) fn dispatch_command(&mut self, id: CommandId, cx: &mut Cx<Self>) {
        if self.session.is_sql()
            && matches!(id, CommandId::Group | CommandId::Pivot | CommandId::Aggregation | CommandId::OrderBy)
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
        if let Some(action) = sql_editor::sql_editor_action(id) {
            self.run_sql_editor_action(action, cx);
            cx.notify();
            return;
        }
        match id {
            CommandId::Back => self.navigate_history(false, cx),
            CommandId::Forward => self.navigate_history(true, cx),
            CommandId::SqlEditor => self.open_sql_editor(cx),
            CommandId::NewSqlQuery => self.new_sql_query(cx),
            CommandId::Settings => self.open_settings(cx),
            CommandId::ViewOptions => self.open_view_options(cx),
            CommandId::QueryEngine => self.open_engine_palette(cx),
            CommandId::TogglePreserveOrder => self.toggle_preserve_order(cx),
            CommandId::ToggleCrossFilter => self.toggle_cross_filter(cx),
            CommandId::ToggleAvailableFirst => self.toggle_available_first(cx),
            CommandId::ResetDatabase => self.reset_database_connection(cx),
            CommandId::StartupSql => {
                if matches!(self.session.identity, SessionIdentity::StartupSql) {
                    self.open_sql_editor(cx);
                } else {
                    self.navigate_to_file(FileEntry::StartupSql, false, false, false, cx);
                }
            }
            CommandId::Files => self.open_file_palette(cx),
            CommandId::DatabaseTree => self.toggle_database_tree(cx),
            CommandId::Commands => self.open_command_palette(cx),
            CommandId::Keymap => self.open_keymap_editor(cx),
            CommandId::Theme => self.open_theme_editor(cx),
            CommandId::Fonts => self.open_font_picker(crate::ui::font_picker::FontTarget::Grid, cx),
            CommandId::EditorFont => self.open_font_picker(crate::ui::font_picker::FontTarget::Editor, cx),
            CommandId::IncreaseFontSize => self.change_font_size(1, cx),
            CommandId::DecreaseFontSize => self.change_font_size(-1, cx),
            CommandId::ClearFilters => self.clear_filters(cx),
            CommandId::Reload => self.reload(false, cx),
            CommandId::Open => self.open_native_file(cx),
            CommandId::PasteTable => self.paste_table(cx),
            CommandId::AddFolder => self.add_folder_with_picker(cx),
            CommandId::AddWatchedFiles => self.add_watched_files_with_picker(cx),
            CommandId::CacheFiles => self.set_cache_drop_open(!self.cache_drop_open, cx),
            CommandId::OpenSettingsDirectory => self.open_settings_directory(cx),
            CommandId::ReloadReset => self.reload(true, cx),
            CommandId::Save => self.open_save_dialog(cx),
            CommandId::NewWindow => self.open_blank_window(None, cx),
            CommandId::Copy => {
                if let Some(selection) = self.grid.selection() {
                    self.schedule_copy(self.session.result.clone(), selection, cx);
                }
            }
            CommandId::CopyAs => self.open_copy_as(cx),
            CommandId::Search => {
                let id = self.search_input.id;
                self.set_focus(Some(id), cx);
            }
            CommandId::Help => self.open_help(cx),
            CommandId::Info => self.open_information(cx),
            CommandId::Group => self.open_pivot_panel(PivotFieldKind::GroupBy, cx),
            CommandId::Aggregation => self.open_aggregation_panel(cx),
            CommandId::Pivot => self.open_pivot_panel(PivotFieldKind::PivotBy, cx),
            CommandId::OrderBy => self.open_order_by_panel(cx),
            CommandId::AddSlicer => self.open_slicer_picker(cx),
            CommandId::Space => self.open_space_menu(cx),
            CommandId::SortAscending => self.sort_active(false, cx),
            CommandId::SortDescending => self.sort_active(true, cx),
            CommandId::ClearSort => self.clear_sort(cx),
            CommandId::CloseWindow => {
                self.prepare_close(cx);
                cx.close_window();
            }
            CommandId::ToggleFrozen => self.change_table_column(None, cx),
            CommandId::UnfreezeAll => self.unfreeze_all(cx),
            CommandId::Export => self.export_result(cx),
            CommandId::ToggleTotals => self.toggle_totals(cx),
            CommandId::ToggleHeatmap => self.toggle_heatmap(cx),
            CommandId::RecalculateWidths => self.recalculate_widths(cx),
            CommandId::HideColumns => self.hide_selected_columns(cx),
            CommandId::ChooseColumns => self.open_column_chooser(cx),
            CommandId::ShowAllColumns => {
                let hidden = self.session.effective_table_config().hidden;
                self.set_columns_hidden(&hidden, false, cx);
            }
            CommandId::FilterToValue => self.filter_active_value(false, cx),
            CommandId::ExcludeValue => self.filter_active_value(true, cx),
            CommandId::DrillDown => self.drill_down(false, cx),
            CommandId::DrillDownNewWindow => self.drill_down(true, cx),
            CommandId::CellDetail => self.toggle_cell_detail(cx),
            CommandId::JumpToColumn => self.open_column_jump(cx),
            CommandId::Undo => self.undo_view_change(false, cx),
            CommandId::Redo => self.undo_view_change(true, cx),
            CommandId::FocusGrid => self.focus_grid(cx),
            CommandId::FocusSlicers => self.focus_next_slicer(cx),
            CommandId::RunAction => self.open_action_menu(cx),
            CommandId::EditActions => self.open_actions_editor(cx),
            CommandId::AiProviders => self.open_ai_settings(cx),
            CommandId::RunSql => self.run_sql(false, cx),
            CommandId::RunAllSql => self.run_sql(true, cx),
            CommandId::SqlToggleLineComment
            | CommandId::SqlToggleBlockComment
            | CommandId::SqlFind
            | CommandId::SqlReplace
            | CommandId::SqlGoToLine
            | CommandId::SqlShowCompletions
            | CommandId::SqlShowSignature
            | CommandId::SqlAddCursorAbove
            | CommandId::SqlAddCursorBelow
            | CommandId::SqlAddNextOccurrence
            | CommandId::SqlSelectAllOccurrences
            | CommandId::SqlMoveLinesUp
            | CommandId::SqlMoveLinesDown
            | CommandId::SqlCopyLinesUp
            | CommandId::SqlCopyLinesDown
            | CommandId::SqlDeleteLines
            | CommandId::SqlSelectLine
            | CommandId::SqlExpandSelection
            | CommandId::SqlShrinkSelection
            | CommandId::SqlJumpToBracket
            | CommandId::SqlFold
            | CommandId::SqlUnfold
            | CommandId::SqlFoldAll
            | CommandId::SqlUnfoldAll
            | CommandId::SqlToggleWordWrap
            | CommandId::SqlInsertLineBelow
            | CommandId::SqlInsertLineAbove => {}
            CommandId::AskSql => {
                self.open_sql_editor(cx);
                if self.session.is_sql()
                    && let Some(panel) = &mut self.sql_panel
                {
                    let mut focus = self.focus;
                    panel.open_ask(&mut focus);
                    self.set_focus(focus, cx);
                    self.handle_sql_panel_events(cx);
                }
            }
        }
        self.drain_grid_events(cx);
        cx.notify();
    }

    fn open_settings_directory(&mut self, cx: &mut Cx<Self>) {
        let path = &self.repository.paths.config;
        match std::fs::create_dir_all(path) {
            Ok(()) => system::open_path(path),
            Err(error) => {
                self.error = Some(format!("Cannot open settings directory {}: {error}", path.display()));
                cx.notify();
            }
        }
    }

    pub(crate) fn dispatch_keymap_action(&mut self, action: ActionId, cx: &mut Cx<Self>) {
        if let Some(command) = KEYMAP_ACTIONS
            .iter()
            .find(|spec| spec.id == action)
            .and_then(|spec| spec.command)
        {
            self.dispatch_command(command, cx);
            return;
        }
        let height = cx.viewport().1;
        match action {
            ActionId::ToggleThousandsGrouping | ActionId::IncreaseDecimalPrecision | ActionId::DecreaseDecimalPrecision => {
                self.grid.change_numeric_format(action)
            }
            ActionId::MoveColumnLeft => self.change_table_column(Some(-1), cx),
            ActionId::MoveColumnRight => self.change_table_column(Some(1), cx),
            ActionId::RemoveActiveGroupColumn => self.remove_active_group_column(cx),
            ActionId::ToggleFrozen => self.change_table_column(None, cx),
            ActionId::UnfreezeAll => self.unfreeze_all(cx),
            _ => self.grid.navigate_action(action, height),
        }
        self.drain_grid_events(cx);
        cx.notify();
    }

    fn open_space_menu(&mut self, cx: &mut Cx<Self>) {
        self.space_menu_open = true;
        self.focus_grid(cx);
        cx.notify();
    }

    pub(crate) fn select_space_command(&mut self, id: CommandId, cx: &mut Cx<Self>) {
        self.space_menu_open = false;
        self.dispatch_command(id, cx);
        cx.notify();
    }

    pub(crate) fn handle_space_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) {
        let key = event.key.to_ascii_lowercase();
        if matches!(key.as_str(), "space" | "escape") {
            self.space_menu_open = false;
            cx.notify();
        } else if let Ok(stroke) = canonical_keystroke(&event.stroke()) {
            let context = if self.is_table() {
                KeyContext::TableSpaceMenu
            } else {
                KeyContext::QuerySpaceMenu
            };
            let produced = event
                .typed()
                .filter(|typed| !typed.chars().all(char::is_alphanumeric) && event.modifiers.shift)
                .and_then(|typed| self.keymap.binding_for(typed, context));
            if let Some(action) = self.keymap.binding_for(&stroke, context).or(produced)
                && let Some(command) = KEYMAP_ACTIONS
                    .iter()
                    .find(|spec| {
                        spec.id == action
                            && matches!(
                                spec.context,
                                KeyContext::SpaceMenu | KeyContext::TableSpaceMenu | KeyContext::QuerySpaceMenu
                            )
                    })
                    .and_then(|spec| spec.command)
            {
                self.select_space_command(command, cx);
            }
        }
    }

    pub(crate) fn sort_active(&mut self, descending: bool, cx: &mut Cx<Self>) {
        let Some(selection) = self.grid.selection() else {
            return;
        };
        let Some(column) = self.session.result.columns.get(selection.active.column) else {
            return;
        };
        self.session.set_sort_direction(column.0.clone(), descending);
        self.grid.set_sort(self.session.sort.clone());
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    fn remove_active_group_column(&mut self, cx: &mut Cx<Self>) {
        let Some(selection) = self.grid.selection() else {
            return;
        };
        let Some(ViewKind::Query(current)) = self.session.working.as_ref().map(|view| &view.view) else {
            return;
        };
        let Some(field) = current.group_by.get(selection.active.column).cloned() else {
            return;
        };
        let mut updated = current.clone();
        match crate::application::pivot_editor::apply(
            &mut updated,
            PivotFieldKind::GroupBy,
            crate::application::pivot_editor::PivotEdit::Remove(field),
        ) {
            Ok(true) => {
                if let Err(error) = self.session.edit_definition(|view| view.view = ViewKind::Query(updated)) {
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

    pub(crate) fn clear_sort(&mut self, cx: &mut Cx<Self>) {
        if !self.session.clear_sort() {
            return;
        }
        self.grid.set_sort(Vec::new());
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    pub(crate) fn clear_filters(&mut self, cx: &mut Cx<Self>) {
        if self.session.clear_filters() {
            self.set_search_text(String::new(), cx);
        }
    }

    /// The active cell's position and its value as the grid renders it before formatting.
    pub(crate) fn active_cell_value(&self) -> Result<(usize, usize, Option<String>), String> {
        let selection = self.grid.selection().ok_or("Select a cell first")?;
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

    pub(crate) fn toggle_cell_detail(&mut self, cx: &mut Cx<Self>) {
        if self.cell_detail.take().is_none() {
            self.cell_detail = Some(CellDetailPane::new());
            self.refresh_cell_detail();
        } else if self.focus.is_some_and(|focus| !self.owns_focus(focus)) {
            self.focus_grid(cx);
        }
        cx.notify();
    }

    /// Shows the active cell in the cell detail pane, when it is open.
    pub(crate) fn refresh_cell_detail(&mut self) {
        if self.cell_detail.is_none() {
            return;
        }
        let content = match self.active_cell_value() {
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
        if let Some(pane) = &mut self.cell_detail {
            pane.set_content(content);
        }
    }

    pub(crate) fn filter_active_value(&mut self, exclude: bool, cx: &mut Cx<Self>) {
        let filter = self.active_cell_value().and_then(|(_, column, value)| {
            let target = match self.session.working.as_ref().map(|view| &view.view) {
                Some(ViewKind::Sql(_)) => {
                    if let Some(reason) = self.sql_filters_unavailable() {
                        return Err(reason.into());
                    }
                    Dimension::Column(self.session.result.columns[column].0.clone())
                }
                Some(ViewKind::Query(config)) => config.group_by.get(column).cloned().ok_or_else(|| {
                    "Value filters apply to group-by columns; press Enter to drill down instead".to_owned()
                })?,
                _ => Dimension::Column(self.session.result.columns[column].0.clone()),
            };
            Ok(ValueFilter { target, value, exclude })
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
    pub(crate) fn ensure_working_view(&mut self) {
        if self.session.working.is_none() {
            let _ = self.session.start_table_draft(self.suggested_name());
            if let Some(definition) = &mut self.session.working
                && let ViewKind::Table(table) = &mut definition.view
            {
                table.sort = self.session.sort.clone();
            }
        }
    }

    pub(crate) fn view_actions(&self) -> Vec<ViewAction> {
        self.session
            .working
            .as_ref()
            .map(|definition| definition.actions.clone())
            .unwrap_or_default()
    }

    pub(crate) fn open_action_menu(&mut self, cx: &mut Cx<Self>) {
        if self.view_actions().is_empty() {
            self.open_actions_editor(cx);
            return;
        }
        self.space_menu_open = false;
        self.action_menu_open = true;
        self.focus_grid(cx);
        cx.notify();
    }

    pub(crate) fn handle_action_menu_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) {
        let key = event.key.to_ascii_lowercase();
        if matches!(key.as_str(), "space" | "escape") {
            self.action_menu_open = false;
            cx.notify();
        } else if let Some(typed) = event.key_char.as_deref()
            && !event.modifiers.control
            && !event.modifiers.alt
            && let Some(index) = self.view_actions().iter().position(|action| action.key == typed)
        {
            self.run_view_action(index, cx);
        }
    }

    /// Runs action `index` of the current view with the active row's values.
    pub(crate) fn run_view_action(&mut self, index: usize, cx: &mut Cx<Self>) {
        self.action_menu_open = false;
        let Some(action) = self.view_actions().into_iter().nth(index) else {
            return;
        };
        self.error = self.launch_action(&action).err();
        cx.notify();
    }

    pub(crate) fn active_row(&self) -> usize {
        self.grid
            .selection()
            .map_or(usize::MAX, |selection| selection.active.row)
    }

    /// Runs `action` with the active row's values.
    pub(crate) fn launch_action(&self, action: &ViewAction) -> Result<(), String> {
        let row = self.active_row();
        actions::launch_for_row(&action.command, &self.session.result, row)
            .and_then(|launch| match launch {
                actions::Launch::Url(url) => {
                    system::open_url(&url);
                    Ok(())
                }
                actions::Launch::Program { program, args } => launcher::spawn_program(&program, &args),
            })
            .map_err(|error| format!("Action {} failed: {error}", action.description))
    }

    /// Opens the actions editor or menu for a screenshot, with sample actions
    /// when the view has none.
    pub(crate) fn capture_actions(&mut self, target: &str, cx: &mut Cx<Self>) {
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
            self.open_action_menu(cx);
        } else {
            self.open_actions_editor(cx);
        }
    }

    pub(crate) fn remove_value_filter(&mut self, index: usize, cx: &mut Cx<Self>) {
        if self.session.remove_value_filter(index) {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        cx.notify();
    }

    pub(crate) fn change_font_size(&mut self, delta: i16, cx: &mut Cx<Self>) {
        let mut font = self.font.clone();
        font.size = (i16::from(font.size) + delta).clamp(7, 40) as u8;
        self.save_font(font, cx);
    }

    pub(crate) fn save_sql_editor_height(&mut self, height: f32) {
        let height = height.round();
        self.sql_editor_height = height;
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.sql_editor_height = Some(height);
            self.repository.save_settings(&settings)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save SQL editor height: {error}"));
        }
    }

    pub(crate) fn save_font(&mut self, font: FontPreferences, cx: &mut Cx<Self>) {
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
                self.fonts = font.fonts();
                self.font = font.clone();
                let profile = self.keymap_profile.clone();
                profiles::changed(|profiles| {
                    profiles.fonts.insert(profile, font);
                });
                cx.notify_all();
            }
            Err(error) => self.error = Some(format!("Could not save font settings: {error}")),
        }
        cx.notify();
    }

    pub(crate) fn commit_query_limit(&mut self, cx: &mut Cx<Self>) {
        let limit = match crate::infrastructure::json_store::parse_query_limit(self.limit_input.text()) {
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
        self.limit_input.set_text(limit.to_string());
        let profile = self.keymap_profile.clone();
        profiles::changed(|profiles| {
            profiles.query_limits.insert(profile, limit);
        });
        self.profiles_seen = profiles::revision();
        cx.notify_all();
        if !self.is_table() && !self.session.is_sql() {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        cx.notify();
    }

    /// Asks for a file and writes the whole displayed result to it, in the
    /// format its extension names.
    pub(crate) fn export_result(&mut self, cx: &mut Cx<Self>) {
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
        let path = match system::prompt_for_new_path(cx.hwnd(), &directory, &suggested) {
            Ok(Some(path)) => path,
            Ok(None) => return,
            Err(error) => {
                self.copy_status = Some(format!("Export failed: {error}"));
                cx.notify();
                return;
            }
        };
        let format = match crate::domain::export::FileFormat::from_path(&path) {
            Ok(format) => format,
            Err(error) => {
                self.copy_status = Some(format!("Export failed: {error}"));
                cx.notify();
                return;
            }
        };
        self.copy_status = Some("Exporting…".into());
        cx.notify();
        let generation = self.session.token().session_generation;
        let target = path.clone();
        cx.spawn(
            move || {
                let result = match rerun {
                    Some(rerun) => rerun.run()?,
                    None => result,
                };
                result.export_to_file(&target, format)
            },
            move |view: &mut Self, cx, written: Result<usize, String>| {
                if view.session.token().session_generation != generation {
                    return;
                }
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                view.copy_status = Some(match written {
                    Ok(rows) => format!("Exported {rows} {} to {name}", if rows == 1 { "row" } else { "rows" }),
                    Err(error) => format!("Export failed: {error}"),
                });
                cx.notify();
            },
        )
        .detach();
    }

    pub(crate) fn hide_selected_columns(&mut self, cx: &mut Cx<Self>) {
        let result = &self.session.result;
        let Some(selection) = self
            .grid
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
    pub(crate) fn set_columns_hidden(&mut self, names: &[String], hidden: bool, cx: &mut Cx<Self>) {
        if !self.is_table() {
            self.error = Some("Hiding columns applies to tables, not query or SQL results".into());
            cx.notify();
            return;
        }
        let mut config = self.session.effective_table_config();
        let mut changed = false;
        for name in names {
            changed |= crate::domain::table::set_hidden(&mut config, name, hidden);
        }
        if !changed {
            return;
        }
        if config.column_order.iter().all(|name| config.hidden.contains(name)) {
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

    /// SQL sets the column order, so freezing only moves the boundary: through
    /// the active column, or back to just left of a frozen one.
    fn toggle_sql_frozen(&mut self, cx: &mut Cx<Self>) {
        let Some(selection) = self.grid.selection() else {
            return;
        };
        let Some(ViewKind::Sql(config)) = self.session.working.as_ref().map(|view| &view.view) else {
            return;
        };
        let columns = self.result_column_names();
        let frozen = crate::domain::table::frozen_prefix(&config.frozen_columns, &columns);
        let names = crate::domain::table::toggle_frozen_prefix(&columns, frozen, selection.active.column);
        self.set_sql_frozen(names, cx);
    }

    fn set_sql_frozen(&mut self, names: Vec<String>, cx: &mut Cx<Self>) {
        let result = self.session.edit_definition(|definition| {
            if let ViewKind::Sql(config) = &mut definition.view {
                config.frozen_columns = names;
            }
        });
        if let Err(error) = result {
            self.error = Some(error);
        } else {
            self.undo_history.record(self.session.view_state());
            self.sync_frozen_count();
            self.flush_auto_save();
        }
        cx.notify();
    }

    pub(crate) fn unfreeze_all(&mut self, cx: &mut Cx<Self>) {
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
            && (!self.session.is_direct_table() || self.session.direct_table.frozen_columns.is_empty())
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
            self.sync_frozen_count();
            self.autosave_table_preferences(cx);
        }
        cx.notify();
    }

    pub(crate) fn focus_next_slicer(&mut self, cx: &mut Cx<Self>) {
        let has_slicers = self
            .session
            .working
            .as_ref()
            .is_some_and(|definition| !definition.slicers.is_empty());
        let mut focus = self.focus;
        if !has_slicers || !self.slicer_sidebar.focus_next_input(&mut focus) {
            self.copy_status = Some("No slicers to focus. Use Add slicer to add one".into());
            cx.notify();
            return;
        }
        self.focus = focus;
        self.drain_slicer_events(cx);
        cx.notify();
    }

    pub(crate) fn change_table_column(&mut self, direction: Option<isize>, cx: &mut Cx<Self>) {
        if direction.is_none() && self.session.is_sql() {
            self.toggle_sql_frozen(cx);
            return;
        }
        if matches!(
            self.session.working.as_ref().map(|definition| &definition.view),
            Some(ViewKind::Query(_) | ViewKind::Sql(_))
        ) {
            return;
        }
        let Some(selection) = self.grid.selection() else {
            return;
        };
        let mut config = self.session.effective_table_config();
        let order = config.column_order.clone();
        let destination = match direction {
            Some(direction) => crate::domain::table::move_column(&mut config, &order, selection.active.column, direction),
            None => crate::domain::table::toggle_frozen(&mut config, &order, selection.active.column),
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
        let ViewKind::Table(config) = &self.session.working.as_ref().expect("table draft exists").view else {
            return;
        };
        let available = self
            .session
            .source
            .columns
            .iter()
            .map(|column| column.0.clone())
            .collect::<Vec<_>>();
        let order = crate::domain::table::visible_order(&available, config);
        let result = self.session.result.reordered(
            &order,
            self.session.result.result_revision + 1,
            self.session.token().request_revision,
        );
        self.session.result = result.clone();
        self.grid.replace_result(result);
        self.sync_frozen_count();
        self.autosave_table_preferences(cx);
        self.sync_grid_widths(cx);
        if had_pending {
            self.schedule_search(self.session.search_text.clone(), cx);
        }
        self.drain_grid_events(cx);
        cx.notify();
    }

    pub(crate) fn add_slicer(&mut self, column: String, cx: &mut Cx<Self>) {
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
    pub(crate) fn add_condition_slicer(&mut self, column: String, cx: &mut Cx<Self>) {
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
    pub(crate) fn add_slicer_card(&mut self, card: SlicerDefinition, cx: &mut Cx<Self>) {
        if self.session.working.as_ref().is_some_and(|definition| {
            definition
                .slicers
                .iter()
                .any(|other| other.sql == card.sql && other.column == card.column)
        }) {
            return;
        }
        self.ensure_working_view();
        let _ = self.session.edit_definition(|definition| definition.slicers.push(card));
        self.autosave_table_preferences(cx);
        self.schedule_search(self.session.search_text.clone(), cx);
    }

    /// Lists the SQL card and condition cards at once, since they need only the
    /// schema; value cards follow once discovery finds few enough values.
    pub(crate) fn update_slicer_picker(&mut self, error: Option<String>) {
        let Some(Dialog::SlicerPicker(_)) = &self.dialog else {
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
        let free = |column: &String| !definitions.iter().any(|card| !card.sql && &card.column == column);
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
        if let Some(Dialog::SlicerPicker(picker)) = &mut self.dialog {
            picker.set_columns(columns, loading, error);
        }
    }

    /// Stops discovering every column's values; value cards still discover
    /// their own columns.
    pub(crate) fn close_slicer_picker(&mut self) {
        if let Some(discovery) = self.slicer_discovery.take() {
            discovery.cancel();
        }
    }

    pub(crate) fn choose_slicer(&mut self, choice: SlicerChoice, cx: &mut Cx<Self>) {
        match choice.kind {
            SlicerKind::Values => self.add_slicer(choice.column, cx),
            SlicerKind::Condition => self.add_condition_slicer(choice.column, cx),
            SlicerKind::Sql => self.add_slicer_card(SlicerDefinition::sql_filter(), cx),
        }
        self.close_slicer_picker();
    }

    /// Applies what the slicer cards reported since the last call.
    pub(crate) fn drain_slicer_events(&mut self, cx: &mut Cx<Self>) {
        for event in self.slicer_sidebar.take_events() {
            let changed = self.session.working.as_ref().is_some_and(|definition| match &event {
                SlicerEvent::Select { column, selected } => definition
                    .slicers
                    .iter()
                    .any(|card| &card.column == column && &card.selected != selected),
                SlicerEvent::Clear(column) => definition
                    .slicers
                    .iter()
                    .any(|card| &card.column == column && card.is_active()),
                SlicerEvent::Condition { column, text } => definition
                    .slicers
                    .iter()
                    .any(|card| &card.column == column && card.condition.as_ref() != Some(text)),
                SlicerEvent::Remove(column) => definition.slicers.iter().any(|card| &card.column == column),
            });
            if !changed {
                continue;
            }
            let _ = self.session.edit_definition(|definition| match &event {
                SlicerEvent::Select { column, selected } => {
                    if let Some(card) = definition.slicers.iter_mut().find(|card| &card.column == column) {
                        card.selected = selected.clone();
                    }
                }
                SlicerEvent::Clear(column) => {
                    if let Some(card) = definition.slicers.iter_mut().find(|card| &card.column == column) {
                        card.clear();
                    }
                }
                SlicerEvent::Remove(column) => definition.slicers.retain(|card| &card.column != column),
                SlicerEvent::Condition { column, text } => {
                    if let Some(card) = definition.slicers.iter_mut().find(|card| &card.column == column) {
                        card.condition = Some(text.clone());
                    }
                }
            });
            self.autosave_table_preferences(cx);
            self.schedule_search(self.session.search_text.clone(), cx);
        }
    }

    pub(crate) fn save_theme(&mut self, theme: ThemeSettings, cx: &mut Cx<Self>) -> std::io::Result<()> {
        self.repository.save_theme(&theme)?;
        self.theme = theme.clone();
        self.grid.set_theme(theme.clone());
        let profile = self.repository.paths.config.clone();
        profiles::changed(|profiles| {
            profiles.themes.insert(profile, theme);
        });
        cx.notify_all();
        cx.notify();
        Ok(())
    }

    /// Saves what needs saving before the window closes.
    pub(crate) fn prepare_close(&mut self, cx: &mut Cx<Self>) {
        self.stop_sql();
        self.flush_auto_save();
        self.autosave_table_preferences(cx);
        self.update_geometry(cx);
        self.save_geometry();
    }
}
