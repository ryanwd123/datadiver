//! How the open file is read (its query engine and options), and the
//! Settings list.

use super::*;
use crate::domain::table_preferences::{FileSettings, TablePreferencesPatch};
use crate::infrastructure::source_database::LoadMode;
use crate::infrastructure::store::Engine;
use crate::ui::font_picker::FontTarget;
use crate::ui::widgets::list::Item;

/// The rows of the Settings list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsItem {
    DefaultEngine,
    FileEngine,
    GridFont,
    EditorFont,
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
        let Some(path) = self.settings_file() else {
            self.copy_status = Some("Only files choose a query engine; SQL and database tables run on DuckDB".into());
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

    pub(crate) fn toggle_preserve_order(&mut self, cx: &mut Cx<Self>) {
        if self.session.source.engine != Engine::DuckDb {
            self.copy_status = Some("Preserve order applies when DuckDB reads the file".into());
            cx.notify();
            return;
        }
        self.change_file_settings(|file| file.preserve_order = !file.preserve_order, cx);
    }

    pub(crate) fn open_settings(&mut self, cx: &mut Cx<Self>) {
        let items = self.settings_items();
        self.palette("Settings", "Find a setting", items, dialogs::PaletteKind::Settings, cx);
    }

    fn settings_list(&self) -> Vec<(SettingsItem, String, String)> {
        let default_engine = self
            .repository
            .load_settings()
            .map(|settings| settings.default_engine)
            .unwrap_or_default();
        let mut rows = vec![(
            SettingsItem::DefaultEngine,
            "Default query engine".to_owned(),
            format!("{} · Enter switches", default_engine.label()),
        )];
        if let Some(path) = self.settings_file() {
            rows.push((
                SettingsItem::FileEngine,
                format!("Query engine for {}", source_file_name(&path)),
                self.session.source.engine.label().to_owned(),
            ));
        }
        let font = |family: &Option<String>| family.clone().unwrap_or_else(|| "Default".into());
        rows.extend([
            (SettingsItem::GridFont, "Grid font".to_owned(), format!("{} · {} pt", font(&self.font.family), self.font.size)),
            (SettingsItem::EditorFont, "Editor font".to_owned(), font(&self.font.editor_family)),
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
        if item == SettingsItem::DefaultEngine {
            // Switching stays in the list, so it shows the new value.
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
            SettingsItem::DefaultEngine => {}
            SettingsItem::FileEngine => self.open_engine_palette(cx),
            SettingsItem::GridFont => self.open_font_picker(FontTarget::Grid, cx),
            SettingsItem::EditorFont => self.open_font_picker(FontTarget::Editor, cx),
            SettingsItem::Theme => self.open_theme_editor(cx),
            SettingsItem::Keymap => self.open_keymap_editor(cx),
            SettingsItem::AiProviders => self.open_ai_settings(cx),
            SettingsItem::StartupSql => self.dispatch_command(CommandId::StartupSql, cx),
            SettingsItem::SettingsFolder => self.dispatch_command(CommandId::OpenSettingsDirectory, cx),
        }
    }
}
