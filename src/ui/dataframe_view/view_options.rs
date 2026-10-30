//! The View options list behind the gear at the end of the search row:
//! the switches and the engine that apply to what is open.

use super::*;
use crate::infrastructure::store::Engine;
use crate::ui::sql_panel::SqlPanelEvent;
use crate::ui::widgets::list::Item;

/// The rows of the View options list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewOption {
    AutoSave,
    AutoRun,
    Engine,
    PreserveOrder,
    CrossFilter,
    AvailableFirst,
}

impl DataFrameView {
    /// The options that apply to what is open: each with its label, detail,
    /// and state when it is a switch.
    pub(crate) fn view_options(&self) -> Vec<(ViewOption, &'static str, String, Option<bool>)> {
        let mut rows = Vec::new();
        if let Some(panel) = self.sql_panel.as_ref().filter(|_| self.session.is_sql()) {
            rows.push((ViewOption::AutoSave, "Auto-save", "Save the SQL 2 seconds after typing stops".into(), Some(panel.auto_save)));
            // The startup script already runs whenever the database opens.
            if !panel.startup_script {
                rows.push((ViewOption::AutoRun, "Auto-run", "Run the whole script each time it opens".into(), Some(panel.auto_run)));
            }
        }
        if self.settings_file().is_some() {
            // The file's setting changes before the file is read again.
            let engine = self.file_settings.engine.unwrap_or(self.session.source.engine);
            rows.push((ViewOption::Engine, "Query engine", format!("{} · Enter switches", engine.label()), None));
            if engine == Engine::DuckDb {
                rows.push((
                    ViewOption::PreserveOrder,
                    "Preserve order",
                    "Keep rows in file order when DuckDB reads the file; off reads large files faster".into(),
                    Some(self.file_settings.preserve_order),
                ));
            }
        }
        let slicers = self.file_settings.slicers;
        rows.push((
            ViewOption::CrossFilter,
            "Slicers: cross filter",
            "Gray out values the other cards and filters rule out; off is faster on big sources".into(),
            Some(slicers.cross_filter),
        ));
        rows.push((
            ViewOption::AvailableFirst,
            "Slicers: available values first",
            "List available values before grayed-out ones".into(),
            Some(slicers.available_first),
        ));
        rows
    }

    fn view_option_items(&self) -> Vec<Item> {
        self.view_options()
            .into_iter()
            .map(|(_, label, detail, check)| {
                let mut item = Item::new(label).detail(detail);
                item.check = check;
                item
            })
            .collect()
    }

    pub(crate) fn open_view_options(&mut self, cx: &mut Cx<Self>) {
        let items = self.view_option_items();
        self.palette("View options", "Find an option", items, dialogs::PaletteKind::ViewOptions, cx);
    }

    /// Switches the option at `index`, staying in the list so it shows the
    /// new value.
    pub(crate) fn confirm_view_option(&mut self, index: usize, cx: &mut Cx<Self>) {
        let Some((option, ..)) = self.view_options().get(index).cloned() else {
            return;
        };
        let panel = self.sql_panel.as_ref().map(|panel| (panel.auto_save, panel.auto_run));
        match option {
            ViewOption::AutoSave => {
                if let Some((auto_save, _)) = panel {
                    self.handle_sql_panel_event(SqlPanelEvent::AutoSave(!auto_save), cx);
                }
            }
            ViewOption::AutoRun => {
                if let Some((_, auto_run)) = panel {
                    self.handle_sql_panel_event(SqlPanelEvent::AutoRun(!auto_run), cx);
                }
            }
            ViewOption::Engine => {
                let engine = match self.file_settings.engine.unwrap_or(self.session.source.engine) {
                    Engine::Polars => Engine::DuckDb,
                    Engine::DuckDb => Engine::Polars,
                };
                self.set_file_engine(engine, cx);
            }
            ViewOption::PreserveOrder => self.toggle_preserve_order(cx),
            ViewOption::CrossFilter => self.toggle_cross_filter(cx),
            ViewOption::AvailableFirst => self.toggle_available_first(cx),
        }
        let items = self.view_option_items();
        // Auto-save on a draft asks for a name instead.
        if let Some(dialogs::Dialog::Palette(dialog)) = &mut self.dialog
            && matches!(dialog.kind, dialogs::PaletteKind::ViewOptions)
        {
            let selected = items.len().saturating_sub(1).min(index);
            dialog.palette.set_items(items);
            dialog.palette.list.select_item(Some(selected));
        }
        cx.notify();
    }
}
