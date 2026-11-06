//! How the open file is read (its query engine and options), and the
//! Settings list.

use super::*;
use crate::domain::table_preferences::{FileSettings, SlicerOptions, TablePreferencesPatch};
use crate::infrastructure::source_database::LoadMode;
use crate::infrastructure::store::Engine;
use crate::ui::font_picker::FontTarget;
use crate::ui::widgets::list::Item;

/// The rows of the Settings list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    DefaultEngine,
    FileEngine,
    DefaultCategorical,
    PreserveOrder,
    FileViews,
    GridFont,
    EditorFont,
    EditorOptions,
    Theme,
    Keymap,
    AiProviders,
    StartupSql,
    SettingsFolder,
}

impl DataFrameView {
    /// The file whose settings the window's per-file controls change.
    pub(crate) fn settings_file(&self) -> Option<PathBuf> {
        (!self.session.is_sql())
            .then(|| self.session.source_path.as_ref().map(PathBuf::from))
            .flatten()
    }

    pub(crate) fn open_engine_palette(&mut self, cx: &mut Cx<Self>) {
        if self.session.is_sql() && !matches!(self.session.identity, SessionIdentity::StartupSql) {
            let current = self.sql_engine();
            let items = Engine::ALL
                .iter()
                .map(|engine| {
                    let mut item = Item::new(super::sql_editor::sql_engine_label(*engine)).detail(match engine {
                        Engine::Polars => "Polars SQL; read files with read_csv('…') or read_parquet('…')",
                        Engine::DuckDb => "All of DuckDB's SQL, with the profile's tables and attached databases",
                    });
                    item.check = Some(*engine == current);
                    item
                })
                .collect();
            let name = self.session.working.as_ref().map(|d| d.name.clone()).unwrap_or_default();
            self.palette(&format!("Engine for {name}"), "Choose an engine", items, dialogs::PaletteKind::Engines, cx);
            return;
        }
        let Some(path) = self.settings_file() else {
            self.copy_status = Some("Files, SQL sources and SQL scripts choose an engine; database tables and the startup script run on DuckDB".into());
            cx.notify();
            return;
        };
        let current = self.session.source.engine;
        let items = Engine::ALL
            .iter()
            .map(|engine| {
                let mut item = Item::new(engine.label()).detail(match engine {
                    Engine::Polars => "Reads the file into memory quickly",
                    Engine::DuckDb => "Full DuckDB SQL in expressions and cards",
                });
                item.check = Some(*engine == current);
                item
            })
            .collect();
        let title = format!("Query engine for {}", source_file_name(&path));
        self.palette(&title, "Choose an engine", items, dialogs::PaletteKind::Engines, cx);
    }

    /// Saves `edit` to the open file's settings and reads the file again
    /// with them, keeping the view.
    fn change_file_settings(&mut self, edit: impl FnOnce(&mut FileSettings), cx: &mut Cx<Self>) {
        let Some(path) = self.settings_file() else {
            return;
        };
        let mut file = self.file_settings;
        edit(&mut file);
        if file == self.file_settings {
            return;
        }
        let patch = TablePreferencesPatch {
            file: Some(file),
            ..Default::default()
        };
        if let Err(error) = self.table_writer.edit(path.clone(), patch) {
            self.error = Some(format!("Cannot save the file settings: {error}"));
            cx.notify();
            return;
        }
        self.file_settings = file;
        // The reader sees the edit before it is written.
        self.load_source(path, false, LoadMode::Check, cx);
    }

    pub(crate) fn set_file_engine(&mut self, engine: Engine, cx: &mut Cx<Self>) {
        self.change_file_settings(|file| file.engine = Some(engine), cx);
    }

    /// Whether Polars reads the open file's text columns as categoricals:
    /// the file's own choice, or the default in settings.
    pub(crate) fn categorical(&self) -> bool {
        let default = self.repository.load_settings().map(|settings| settings.polars_categorical).unwrap_or_default();
        self.file_settings.categorical_or(default)
    }

    /// Switches categorical text for the open file, keeps the choice with
    /// the file, and reads it again that way.
    pub(crate) fn toggle_categorical(&mut self, cx: &mut Cx<Self>) {
        if self.settings_file().is_none() {
            self.copy_status = Some("Categorical text applies to files Polars reads".into());
            cx.notify();
            return;
        }
        let on = !self.categorical();
        self.change_file_settings(|file| file.categorical = Some(on), cx);
        if self.session.source.engine != Engine::Polars {
            self.copy_status = Some(format!(
                "Categorical text {} for this file; it applies when Polars reads it",
                if on { "on" } else { "off" }
            ));
            cx.notify();
        }
    }

    /// Switches the default for files without their own choice, and reads
    /// the open file again when it follows the default.
    fn toggle_default_categorical(&mut self, cx: &mut Cx<Self>) {
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.polars_categorical = !settings.polars_categorical;
            self.repository.save_settings(&settings)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save settings: {error}"));
            return;
        }
        if let Some(path) = self.settings_file()
            && self.file_settings.categorical.is_none()
            && self.session.source.engine == Engine::Polars
        {
            self.load_source(path, false, LoadMode::Check, cx);
        }
    }

    /// Switches Preserve order for every file DuckDB reads, a setting of the
    /// profile since DuckDB applies it to the whole database, and reads the
    /// open file again when DuckDB read it.
    pub(crate) fn toggle_preserve_order(&mut self, cx: &mut Cx<Self>) {
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.preserve_order = !settings.preserve_order;
            self.repository.save_settings(&settings).map(|()| settings.preserve_order)
        });
        match result {
            Ok(on) => {
                let state = if on { "on" } else { "off" };
                self.copy_status = Some(format!("Preserve order {state} for files DuckDB reads"));
            }
            Err(error) => {
                self.error = Some(format!("Could not save settings: {error}"));
                cx.notify();
                return;
            }
        }
        if let Some(path) = self.settings_file()
            && self.session.source.engine == Engine::DuckDb
        {
            self.load_source(path, false, LoadMode::Check, cx);
        }
        cx.notify();
    }

    /// Switches the profile database's views of the watched files on or off,
    /// and makes or drops them.
    pub(crate) fn toggle_file_views(&mut self, cx: &mut Cx<Self>) {
        let result = self.repository.load_settings().and_then(|mut settings| {
            settings.file_views = !settings.file_views;
            self.repository.save_settings(&settings).map(|()| settings.file_views)
        });
        match result {
            Ok(on) => {
                self.copy_status = Some(if on { "Making views of the watched files…" } else { "Dropping the views of the watched files" }.into());
                self.sync_file_views(true, cx);
            }
            Err(error) => self.error = Some(format!("Could not save settings: {error}")),
        }
        cx.notify();
    }

    /// The file open now, read into memory or scanned in place.
    pub(crate) fn read_mode_file(&self) -> Option<PathBuf> {
        self.settings_file().or_else(|| self.query_file().map(PathBuf::from))
    }

    /// Switches the open file between reading it into memory and scanning it
    /// in place, keeps the choice with the file, and opens it again that way.
    pub(crate) fn toggle_scan(&mut self, cx: &mut Cx<Self>) {
        let Some(path) = self.read_mode_file() else {
            self.copy_status = Some("Scanning in place applies to files".into());
            cx.notify();
            return;
        };
        // A scanned file's view may not have read its settings, so start from the saved ones.
        let mut file = self.table_writer.load(&path).map(|preferences| preferences.file).unwrap_or_default();
        file.scan = self.query_file().is_none();
        let patch = TablePreferencesPatch {
            file: Some(file),
            ..Default::default()
        };
        if let Err(error) = self.table_writer.edit(path.clone(), patch) {
            self.error = Some(format!("Cannot save the file settings: {error}"));
            cx.notify();
            return;
        }
        self.navigate_to_file(FileEntry::Source(path), true, false, false, cx);
    }

    pub(crate) fn toggle_cross_filter(&mut self, cx: &mut Cx<Self>) {
        self.change_slicer_options(|options| options.cross_filter = !options.cross_filter, cx);
    }

    pub(crate) fn toggle_available_first(&mut self, cx: &mut Cx<Self>) {
        self.change_slicer_options(|options| options.available_first = !options.available_first, cx);
    }

    /// Slicer options change only how cards show their values, so the file is
    /// not read again. They are kept with the file, or for this window when
    /// the source is not a file.
    fn change_slicer_options(&mut self, edit: impl FnOnce(&mut SlicerOptions), cx: &mut Cx<Self>) {
        edit(&mut self.file_settings.slicers);
        if let Some(path) = self.settings_file() {
            let patch = TablePreferencesPatch {
                file: Some(self.file_settings),
                ..Default::default()
            };
            if let Err(error) = self.table_writer.edit(path, patch) {
                self.error = Some(format!("Cannot save the slicer options: {error}"));
            }
        }
        self.schedule_search(self.session.search_text.clone(), cx);
        cx.notify();
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Cx<Self>) {
        let items = self.settings_items();
        self.palette("Settings", "Find a setting", items, dialogs::PaletteKind::Settings, cx);
    }

    pub(crate) fn settings_list(&self) -> Vec<(SettingsItem, String, String)> {
        let settings = self.repository.load_settings().unwrap_or_default();
        let mut rows = vec![(
            SettingsItem::DefaultEngine,
            "Default query engine".to_owned(),
            format!("{} · Enter switches", settings.default_engine.label()),
        )];
        if let Some(path) = self.settings_file() {
            rows.push((
                SettingsItem::FileEngine,
                format!("Query engine for {}", source_file_name(&path)),
                self.session.source.engine.label().to_owned(),
            ));
        }
        rows.push((
            SettingsItem::DefaultCategorical,
            "Polars: categorical text by default".to_owned(),
            format!(
                "{} · Read text columns as categorical, which saves memory when values repeat; View options sets it per file · Enter switches",
                if settings.polars_categorical { "On" } else { "Off" }
            ),
        ));
        rows.push((
            SettingsItem::PreserveOrder,
            "DuckDB: preserve row order".to_owned(),
            format!(
                "{} · Keep rows in file order when DuckDB reads a file; off reads large files faster · Enter switches",
                if settings.preserve_order { "On" } else { "Off" }
            ),
        ));
        rows.push((
            SettingsItem::FileViews,
            "DuckDB: views of watched files".to_owned(),
            format!(
                "{} · A view of each watched file in the profile database's files schema, such as files.sales_csv · Enter switches",
                if settings.file_views { "On" } else { "Off" }
            ),
        ));
        let font = |family: &Option<String>| family.clone().unwrap_or_else(|| "Default".into());
        rows.extend([
            (SettingsItem::GridFont, "Grid font".to_owned(), format!("{} · {} pt", font(&self.font.family), self.font.size)),
            (SettingsItem::EditorFont, "Editor font".to_owned(), font(&self.font.editor_family)),
            (
                SettingsItem::EditorOptions,
                "Editor options".to_owned(),
                "Completion list, signature help, problems, wrapping, tabs".to_owned(),
            ),
            (SettingsItem::Theme, "Theme colors".to_owned(), String::new()),
            (SettingsItem::Keymap, "Keyboard shortcuts".to_owned(), "Keymap Editor".to_owned()),
            (SettingsItem::AiProviders, "AI providers".to_owned(), "Ask AI for SQL".to_owned()),
            (
                SettingsItem::StartupSql,
                "DuckDB startup script".to_owned(),
                crate::infrastructure::source_database::STARTUP_SCRIPT.to_owned(),
            ),
            (
                SettingsItem::SettingsFolder,
                "Open settings folder".to_owned(),
                self.repository.paths.config.display().to_string(),
            ),
        ]);
        rows
    }

    fn settings_items(&self) -> Vec<Item> {
        self.settings_list()
            .into_iter()
            .map(|(_, label, detail)| {
                let item = Item::new(label);
                if detail.is_empty() { item } else { item.detail(detail) }
            })
            .collect()
    }

    pub(crate) fn confirm_setting(&mut self, index: usize, cx: &mut Cx<Self>) {
        let Some((item, ..)) = self.settings_list().get(index).cloned() else {
            return;
        };
        if matches!(
            item,
            SettingsItem::DefaultEngine | SettingsItem::DefaultCategorical | SettingsItem::PreserveOrder | SettingsItem::FileViews
        ) {
            // Switching stays in the list, so it shows the new value.
            if item == SettingsItem::DefaultCategorical {
                self.toggle_default_categorical(cx);
            } else if item == SettingsItem::PreserveOrder {
                self.toggle_preserve_order(cx);
            } else if item == SettingsItem::FileViews {
                self.toggle_file_views(cx);
            } else {
                let result = self.repository.load_settings().and_then(|mut settings| {
                    settings.default_engine = match settings.default_engine {
                        Engine::Polars => Engine::DuckDb,
                        Engine::DuckDb => Engine::Polars,
                    };
                    self.repository.save_settings(&settings)
                });
                if let Err(error) = result {
                    self.error = Some(format!("Could not save settings: {error}"));
                }
            }
            let items = self.settings_items();
            if let Some(dialogs::Dialog::Palette(dialog)) = &mut self.dialog {
                dialog.palette.set_items(items);
                dialog.palette.list.select_item(Some(index));
            }
            cx.notify();
            return;
        }
        self.close_dialog(cx);
        match item {
            SettingsItem::DefaultEngine | SettingsItem::DefaultCategorical | SettingsItem::PreserveOrder | SettingsItem::FileViews => {}
            SettingsItem::FileEngine => self.open_engine_palette(cx),
            SettingsItem::GridFont => self.open_font_picker(FontTarget::Grid, cx),
            SettingsItem::EditorFont => self.open_font_picker(FontTarget::Editor, cx),
            SettingsItem::EditorOptions => self.open_editor_settings(None, cx),
            SettingsItem::Theme => self.open_theme_editor(cx),
            SettingsItem::Keymap => self.open_keymap_editor(cx),
            SettingsItem::AiProviders => self.open_ai_settings(cx),
            SettingsItem::StartupSql => self.dispatch_command(CommandId::StartupSql, cx),
            SettingsItem::SettingsFolder => self.dispatch_command(CommandId::OpenSettingsDirectory, cx),
        }
    }
}
