//! The View options list behind the gear at the end of the search row:
//! the switches and the engine that apply to what is open.

use super::*;
use crate::infrastructure::store::Engine;
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
        let sql = match self.session.working.as_ref().map(|definition| &definition.view) {
            Some(ViewKind::Sql(config)) => Some(config.auto_run),
            _ => None,
        };
        if sql.is_some() || self.session.working.is_some() || self.session.source_path.as_deref().is_some_and(|path| !path.is_empty()) {
            let detail = if sql.is_some() {
                "Save the SQL 2 seconds after typing stops"
            } else {
                "Save changes to this view 2 seconds after making them"
            };
            rows.push((ViewOption::AutoSave, "Auto-save", detail.into(), Some(self.auto_save())));
        }
        // The startup script already runs whenever the database opens.
        if let Some(auto_run) = sql
            && !matches!(self.session.identity, SessionIdentity::StartupSql)
        {
            rows.push((ViewOption::AutoRun, "Auto-run", "Run the whole script each time it opens".into(), Some(auto_run)));
        }
        if sql.is_some() && !matches!(self.session.identity, SessionIdentity::StartupSql) {
            let engine = super::sql_editor::sql_engine_label(self.sql_engine());
            rows.push((ViewOption::Engine, "Engine", format!("{engine} · Enter switches"), None));
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
        let Some((option, _, _, check)) = self.view_options().get(index).cloned() else {
            return;
        };
        let on = check == Some(true);
        match option {
            ViewOption::AutoSave => self.set_auto_save(!on, cx),
            ViewOption::AutoRun => self.set_auto_run(!on, cx),
            ViewOption::Engine if self.session.is_sql() => {
                let engine = match self.sql_engine() {
                    Engine::Polars => Engine::DuckDb,
                    Engine::DuckDb => Engine::Polars,
                };
                self.set_sql_engine(engine, cx);
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
