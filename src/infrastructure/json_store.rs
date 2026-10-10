use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

type HistoryLocks = std::collections::HashMap<PathBuf, Weak<Mutex<()>>>;
static HISTORY_LOCKS: OnceLock<Mutex<HistoryLocks>> = OnceLock::new();

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::application::keymap::{ACTIONS, DisabledDefault, KeymapFile, UserBinding};
use crate::application::theme::{THEME_SCHEMA_VERSION, ThemeSettings};
use crate::domain::documents::{DocumentDefinition, SCHEMA_VERSION};
use crate::domain::table_preferences::{TablePreferences, TablePreferencesPatch};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub use crate::domain::documents::DEFAULT_QUERY_LIMIT;

pub fn parse_query_limit(text: &str) -> Result<u64, String> {
    text.trim()
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= i64::MAX as u64)
        .ok_or_else(|| "Limit must be an integer between 1 and 9223372036854775807".into())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FavoriteTarget {
    Source(String),
    View { view_id: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub schema_version: u32,
    pub font_size: u8,
    pub font_family: Option<String>,
    pub editor_font_family: Option<String>,
    pub startup_sql_auto_save: bool,
    pub sql_editor_height: Option<f32>,
    pub database_tree_open: bool,
    pub database_tree_width: Option<f32>,
    pub column_formats: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, crate::domain::formatting::NumericFormat>,
    >,
    pub query_limit: u64,
    pub keybindings: std::collections::BTreeMap<String, String>,
    pub watched_folders: Vec<String>,
    pub favorites: [Option<FavoriteTarget>; 9],
    pub ai: crate::domain::ai::AiSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            font_size: 13,
            font_family: None,
            editor_font_family: None,
            startup_sql_auto_save: false,
            sql_editor_height: None,
            database_tree_open: false,
            database_tree_width: None,
            column_formats: Default::default(),
            query_limit: DEFAULT_QUERY_LIMIT,
            keybindings: Default::default(),
            watched_folders: Vec::new(),
            favorites: Default::default(),
            ai: Default::default(),
        }
    }
}

fn migrate_legacy_keybindings(settings: &Settings) -> KeymapFile {
    let mut migrated = KeymapFile::default();
    for (name, stroke) in &settings.keybindings {
        let Some(action) = crate::application::keymap::action_id(name) else {
            continue;
        };
        let Some(spec) = ACTIONS.iter().find(|spec| spec.id == action) else {
            continue;
        };
        if stroke.is_empty() {
            for default in spec.default_bindings {
                migrated.disabled_defaults.push(DisabledDefault {
                    action,
                    context: spec.context,
                    keystroke: (*default).to_owned(),
                });
            }
        } else if spec
            .default_bindings
            .iter()
            .all(|default| !default.eq_ignore_ascii_case(stroke))
            && let Ok(stroke) = crate::application::keymap::canonical_keystroke(stroke)
        {
            migrated.bindings.push(UserBinding {
                action,
                context: spec.context,
                keystroke: stroke,
            });
        }
    }
    migrated
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApplicationHistory {
    pub schema_version: u32,
    pub last_view_id: Option<String>,
    pub last_raw_source: Option<String>,
    pub recent_sources: Vec<String>,
    pub primary_window: Option<WindowGeometry>,
}

impl Default for ApplicationHistory {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            last_view_id: None,
            last_raw_source: None,
            recent_sources: Vec::new(),
            primary_window: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Clone, Debug)]
pub struct StorePaths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl StorePaths {
    pub fn for_home(home: &Path) -> Self {
        Self {
            config: home.to_path_buf(),
            data: home.to_path_buf(),
        }
    }

    pub fn operating_system() -> Self {
        #[cfg(windows)]
        {
            let data = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            Self::for_home(&data.join("datadiver"))
        }
        #[cfg(not(windows))]
        {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            Self::for_home(&home.join(".local/share/datadiver"))
        }
    }
}

#[derive(Clone, Debug)]
pub struct ViewRepository {
    pub paths: StorePaths,
}

#[derive(Clone, Debug)]
pub struct LoadIssue {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArchivedView {
    pub schema_version: u32,
    pub deleted_at_unix_ms: u128,
    pub original_name: String,
    pub original_kind: String,
    pub view: DocumentDefinition,
}

impl ViewRepository {
    pub fn new(paths: StorePaths) -> Self {
        Self { paths }
    }

    pub fn table_preferences_path(&self, source: &Path) -> PathBuf {
        let key = super::source_database::normalized_path(source)
            .to_string_lossy()
            .into_owned();
        // Fixed FNV-1a, rather than DefaultHasher, keeps filenames stable across releases.
        let hash = key
            .as_bytes()
            .iter()
            .fold(0xcbf29ce484222325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
        self.paths
            .data
            .join("tables")
            .join(format!("{hash:016x}.json"))
    }

    pub fn load_table_preferences(&self, source: &Path) -> io::Result<TablePreferences> {
        let key = super::source_database::normalized_path(source)
            .to_string_lossy()
            .into_owned();
        let preferences = match read_json::<TablePreferences>(&self.table_preferences_path(source))
        {
            Ok(preferences) => preferences,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(TablePreferences::new(key));
            }
            Err(error) => return Err(error),
        };
        preferences
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if preferences.source_path != key {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "table preferences source path does not match",
            ));
        }
        Ok(preferences)
    }

    pub fn update_table_preferences(
        &self,
        source: &Path,
        patch: &TablePreferencesPatch,
    ) -> io::Result<TablePreferences> {
        let mut preferences = self.load_table_preferences(source)?;
        patch.apply(&mut preferences);
        preferences
            .validate()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        atomic_json(&self.table_preferences_path(source), &preferences)?;
        Ok(preferences)
    }

    pub fn load_settings(&self) -> io::Result<Settings> {
        read_json_or_default(&self.paths.config.join("settings.json"))
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        atomic_json(&self.paths.config.join("settings.json"), settings)
    }

    pub fn startup_sql_path(&self) -> PathBuf {
        self.paths.config.join("startup.sql")
    }

    pub fn load_startup_sql(&self) -> io::Result<DocumentDefinition> {
        let text = match fs::read_to_string(self.startup_sql_path()) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                atomic_bytes(&self.startup_sql_path(), b"")?;
                String::new()
            }
            Err(error) => return Err(error),
        };
        let mut definition = DocumentDefinition::draft_table("startup.sql".into(), String::new());
        definition.view =
            crate::domain::documents::ViewKind::Sql(crate::domain::documents::SqlConfig {
                text,
                auto_save: self.load_settings()?.startup_sql_auto_save,
                ..Default::default()
            });
        Ok(definition)
    }

    pub fn save_startup_sql(&self, text: &str) -> io::Result<()> {
        atomic_bytes(&self.startup_sql_path(), text.as_bytes())
    }

    pub fn load_keymap(&self, settings: &Settings) -> io::Result<KeymapFile> {
        let path = self.paths.config.join("keymap.json");
        match read_json::<KeymapFile>(&path) {
            Ok(mut keymap) => {
                keymap.schema_version = crate::application::keymap::KEYMAP_SCHEMA_VERSION;
                Ok(keymap)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let keymap = migrate_legacy_keybindings(settings);
                if !keymap.bindings.is_empty() || !keymap.disabled_defaults.is_empty() {
                    self.save_keymap(&keymap)?;
                }
                Ok(keymap)
            }
            Err(error) => Err(error),
        }
    }

    pub fn save_keymap(&self, keymap: &KeymapFile) -> io::Result<()> {
        atomic_json(&self.paths.config.join("keymap.json"), keymap)
    }

    pub fn load_theme(&self) -> io::Result<ThemeSettings> {
        let theme: ThemeSettings = read_json_or_default(&self.paths.config.join("theme.json"))?;
        if theme.schema_version != THEME_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported theme schema version",
            ));
        }
        Ok(theme)
    }

    pub fn save_theme(&self, theme: &ThemeSettings) -> io::Result<()> {
        if theme.schema_version != THEME_SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Unsupported theme schema version",
            ));
        }
        atomic_json(&self.paths.config.join("theme.json"), theme)
    }

    pub fn load_history(&self) -> io::Result<ApplicationHistory> {
        let mut history: ApplicationHistory =
            read_json_or_default(&self.paths.data.join("state.json"))?;
        history
            .recent_sources
            .retain(|path| !path.trim().is_empty());
        Ok(history)
    }

    pub fn save_history(&self, history: &ApplicationHistory) -> io::Result<()> {
        atomic_json(&self.paths.data.join("state.json"), history)
    }

    /// Serialize read/modify/write operations, including geometry and background history.
    pub fn update_history(
        &self,
        update: impl FnOnce(&mut ApplicationHistory),
    ) -> io::Result<ApplicationHistory> {
        let key = super::source_database::normalized_path(&self.paths.data);
        let lock = {
            let mut locks = HISTORY_LOCKS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            let lock = locks
                .get(&key)
                .and_then(Weak::upgrade)
                .unwrap_or_else(|| Arc::new(Mutex::new(())));
            locks.insert(key, Arc::downgrade(&lock));
            lock
        };
        let _guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut history = self.load_history()?;
        update(&mut history);
        self.save_history(&history)?;
        Ok(history)
    }

    pub fn save_window_geometry(&self, geometry: &WindowGeometry) -> io::Result<()> {
        self.update_history(|history| history.primary_window = Some(geometry.clone()))?;
        Ok(())
    }

    pub fn load_views(&self) -> (Vec<DocumentDefinition>, Vec<LoadIssue>) {
        let mut views = Vec::new();
        let mut issues = Vec::new();
        let directory = self.paths.data.join("views");
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return (views, issues),
            Err(error) => {
                issues.push(LoadIssue {
                    path: directory,
                    message: error.to_string(),
                });
                return (views, issues);
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(LoadIssue {
                        path: directory.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match read_json::<DocumentDefinition>(&path).and_then(|view| {
                view.validate().map_err(io::Error::other)?;
                Ok(view)
            }) {
                Ok(view) => views.push(view),
                Err(error) => issues.push(LoadIssue {
                    path,
                    message: error.to_string(),
                }),
            }
        }
        views.sort_by_key(|view| view.name.to_lowercase());
        (views, issues)
    }

    pub fn save_view(&self, view: &mut DocumentDefinition) -> io::Result<PathBuf> {
        view.validate().map_err(io::Error::other)?;
        if view.source_path.is_empty()
            && view.database_view.is_none()
            && !matches!(view.view, crate::domain::documents::ViewKind::Sql(_))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a source file is required",
            ));
        }
        let mut candidate = view.clone();
        if candidate.id.is_empty() {
            candidate.id = new_id();
        }
        let directory = self.paths.data.join("views");
        let target = directory.join(format!(
            "{}-{}.json",
            safe_name(&candidate.name),
            candidate.id
        ));
        let old = self.find_by_id(&candidate.id)?;
        if target.exists() && old.as_ref() != Some(&target) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("view destination already exists: {}", target.display()),
            ));
        }
        atomic_json(&target, &candidate)?;
        if let Some(old) = old.filter(|old| old != &target)
            && let Err(error) = fs::remove_file(old)
        {
            let _ = fs::remove_file(&target);
            return Err(error);
        }
        view.id = candidate.id;
        Ok(target)
    }

    pub fn archive(&self, id: &str) -> io::Result<ArchivedView> {
        let path = self
            .find_by_id(id)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "view not found"))?;
        let view: DocumentDefinition = read_json(&path)?;
        view.validate().map_err(io::Error::other)?;
        let archive = ArchivedView {
            schema_version: SCHEMA_VERSION,
            deleted_at_unix_ms: unix_ms(),
            original_name: view.name.clone(),
            original_kind: match view.view {
                crate::domain::documents::ViewKind::Table(_) => "table",
                crate::domain::documents::ViewKind::Query(_) => "query",
                crate::domain::documents::ViewKind::Sql(_) => "sql",
            }
            .into(),
            view,
        };
        let target = self.paths.data.join("deleted").join(format!(
            "{}-{}-{}.json",
            archive.view.id,
            archive.deleted_at_unix_ms,
            new_id()
        ));
        atomic_json(&target, &archive)?;
        if let Err(error) = fs::remove_file(path) {
            let _ = fs::remove_file(target);
            return Err(error);
        }
        Ok(archive)
    }

    pub fn load_archives(&self) -> (Vec<(PathBuf, ArchivedView)>, Vec<LoadIssue>) {
        let directory = self.paths.data.join("deleted");
        let mut archives = Vec::new();
        let mut issues = Vec::new();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return (archives, issues),
            Err(error) => {
                issues.push(LoadIssue {
                    path: directory,
                    message: error.to_string(),
                });
                return (archives, issues);
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(LoadIssue {
                        path: directory.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match read_json::<ArchivedView>(&path) {
                Ok(archive)
                    if archive.schema_version == SCHEMA_VERSION
                        && archive.view.validate().is_ok() =>
                {
                    archives.push((path, archive))
                }
                Ok(_) => issues.push(LoadIssue {
                    path,
                    message: "unsupported or invalid archive".into(),
                }),
                Err(error) => issues.push(LoadIssue {
                    path,
                    message: error.to_string(),
                }),
            }
        }
        archives.sort_by_key(|archive| std::cmp::Reverse(archive.1.deleted_at_unix_ms));
        (archives, issues)
    }

    pub fn recover(&self, archive_path: &Path) -> io::Result<DocumentDefinition> {
        let deleted_dir = self.paths.data.join("deleted");
        if archive_path.parent() != Some(deleted_dir.as_path()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "archive is outside deleted views",
            ));
        }
        let archive: ArchivedView = read_json(archive_path)?;
        if archive.schema_version != SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported archive version",
            ));
        }
        archive.view.validate().map_err(io::Error::other)?;
        if self.find_by_id(&archive.view.id)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "view ID already exists",
            ));
        }
        let (existing, _) = self.load_views();
        let original_name = archive.view.name.clone();
        let mut view = archive.view;
        for suffix in 2.. {
            if !existing
                .iter()
                .any(|saved| saved.name.eq_ignore_ascii_case(&view.name))
            {
                break;
            }
            view.name = format!("{original_name} ({suffix})");
        }
        let target = self.save_view(&mut view)?;
        if let Err(error) = fs::remove_file(archive_path) {
            let _ = fs::remove_file(target);
            return Err(error);
        }
        Ok(view)
    }

    fn find_by_id(&self, id: &str) -> io::Result<Option<PathBuf>> {
        let directory = self.paths.data.join("views");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(view) = read_json::<DocumentDefinition>(&path)
                && view.id == id
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }
}

fn new_id() -> String {
    format!(
        "{:x}-{:x}-{:x}",
        unix_ms(),
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn safe_name(name: &str) -> String {
    let mut safe = name
        .trim()
        .trim_end_matches(['.', ' '])
        .chars()
        .map(|ch| {
            if "<>:\"/\\|?*".contains(ch) || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        safe.push_str("view");
    }
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        safe.insert(0, '_');
    }
    safe
}

fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(io::Error::other)
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> io::Result<T> {
    match read_json(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error),
    }
}

fn atomic_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    atomic_bytes(path, &bytes)
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing parent"))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_keeps_settings_history_views_and_database_together() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        repository.save_settings(&Settings::default()).unwrap();
        repository.save_keymap(&KeymapFile::default()).unwrap();
        repository.save_theme(&ThemeSettings::default()).unwrap();
        repository
            .save_history(&ApplicationHistory::default())
            .unwrap();
        let mut view = DocumentDefinition::draft_table("Sales".into(), "source.csv".into());
        let view_path = repository.save_view(&mut view).unwrap();
        for name in ["settings.json", "keymap.json", "theme.json", "state.json"] {
            assert!(home.path().join(name).is_file(), "missing {name}");
        }
        assert_eq!(view_path.parent().unwrap(), home.path().join("views"));
        let database =
            super::super::source_database::SourceDatabase::for_profile(&repository.paths.config);
        assert_eq!(
            database.path(),
            super::super::source_database::normalized_path(home.path()).join("datadiver.duckdb")
        );
        assert!(!home.path().join("config").exists());
        assert!(!home.path().join("data").exists());
    }

    #[test]
    fn default_profile_uses_one_datadiver_directory() {
        let paths = StorePaths::operating_system();
        #[cfg(windows)]
        let expected = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("datadiver");
        #[cfg(not(windows))]
        let expected = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(".local/share/datadiver");
        assert_eq!(paths.config, expected);
        assert_eq!(paths.data, expected);
    }

    #[test]
    fn table_preferences_round_trip_and_reject_mismatched_or_unsupported_files() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let source = home.path().join("Source.csv");
        let path = repository.table_preferences_path(&source);
        let patch = TablePreferencesPatch {
            reset: false,
            widths: [("amount".into(), Some(180.5))].into(),
            column_order: Some(vec!["amount".into(), "region".into()]),
            frozen_columns: Some(vec!["amount".into()]),
            slicer_columns: Some(vec!["region".into()]),
            condition_slicers: None,
            hidden_columns: None,
        };
        let preferences = repository
            .update_table_preferences(&source, &patch)
            .unwrap();
        assert_eq!(
            repository.load_table_preferences(&source).unwrap(),
            preferences
        );
        let json = fs::read_to_string(&path).unwrap();
        assert!(json.ends_with('\n'));
        assert!(!json.contains("selected") && !json.contains("sort"));
        for damaged in [
            TablePreferences {
                source_path: "other".into(),
                ..preferences.clone()
            },
            TablePreferences {
                schema_version: 99,
                ..preferences.clone()
            },
        ] {
            let json = serde_json::to_string(&damaged).unwrap();
            fs::write(&path, &json).unwrap();
            assert!(repository.load_table_preferences(&source).is_err());
            assert!(
                repository
                    .update_table_preferences(&source, &patch)
                    .is_err()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), json);
        }
    }

    #[test]
    fn failed_table_write_preserves_previous_file() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let source = home.path().join("source.csv");
        let patch = TablePreferencesPatch {
            widths: [("a".into(), Some(120.))].into(),
            ..Default::default()
        };
        repository
            .update_table_preferences(&source, &patch)
            .unwrap();
        let path = repository.table_preferences_path(&source);
        let previous = fs::read_to_string(&path).unwrap();
        let invalid = TablePreferencesPatch {
            widths: [("a".into(), Some(f32::INFINITY))].into(),
            ..Default::default()
        };
        assert!(
            repository
                .update_table_preferences(&source, &invalid)
                .is_err()
        );
        assert_eq!(fs::read_to_string(path).unwrap(), previous);
    }

    #[test]
    fn query_limit_defaults_validates_and_persists_without_losing_settings() {
        let old: Settings = serde_json::from_str("{\"font_size\":19}").unwrap();
        assert_eq!(old.query_limit, DEFAULT_QUERY_LIMIT);
        for input in ["", "0", "-1", "1.5", "NaN", "9223372036854775808"] {
            assert!(parse_query_limit(input).is_err());
        }
        assert_eq!(parse_query_limit(" 100000 ").unwrap(), DEFAULT_QUERY_LIMIT);
        assert_eq!(
            parse_query_limit("9223372036854775807").unwrap(),
            i64::MAX as u64
        );
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let settings = Settings {
            query_limit: 42,
            ..old
        };
        repository.save_settings(&settings).unwrap();
        assert_eq!(repository.load_settings().unwrap(), settings);
    }

    #[test]
    fn themes_round_trip_in_the_config_folder_and_missing_fields_use_defaults() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let path = repository.paths.config.join("theme.json");
        assert_eq!(repository.load_theme().unwrap(), ThemeSettings::default());
        assert!(!path.exists());
        let theme = ThemeSettings {
            selection_background: crate::application::theme::ThemeColor::parse("#12345680")
                .unwrap(),
            ..Default::default()
        };
        repository.save_theme(&theme).unwrap();
        assert_eq!(repository.load_theme().unwrap(), theme);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("#12345680"));
        assert!(text.ends_with('\n'));
        fs::write(
            &path,
            r##"{"schema_version":1,"header_background":"#AABBCC"}"##,
        )
        .unwrap();
        let loaded = repository.load_theme().unwrap();
        assert_eq!(
            loaded.header_background,
            crate::application::theme::ThemeColor::rgb(0xaabbcc)
        );
        assert_eq!(loaded.frozen_background, theme.frozen_background);
    }

    #[test]
    fn invalid_themes_report_errors_without_replacing_the_file() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        repository.save_theme(&ThemeSettings::default()).unwrap();
        let path = repository.paths.config.join("theme.json");
        for text in [
            r#"{"schema_version":2}"#,
            r##"{"header_background":"#bad"}"##,
            "broken json",
        ] {
            fs::write(&path, text).unwrap();
            assert!(repository.load_theme().is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
        }
        repository.save_theme(&ThemeSettings::default()).unwrap();
        let previous = fs::read(&path).unwrap();
        assert!(
            repository
                .save_theme(&ThemeSettings {
                    schema_version: 2,
                    ..Default::default()
                })
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), previous);
    }

    #[test]
    fn window_geometry_persists_without_replacing_session_history() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let history = ApplicationHistory {
            last_raw_source: Some("data.csv".into()),
            recent_sources: vec!["data.csv".into()],
            ..Default::default()
        };
        repository.save_history(&history).unwrap();
        let geometry = WindowGeometry {
            x: -1200.0,
            y: 80.0,
            width: 900.0,
            height: 600.0,
            maximized: true,
        };
        repository.save_window_geometry(&geometry).unwrap();
        let saved = repository.load_history().unwrap();
        assert_eq!(saved.primary_window, Some(geometry));
        assert_eq!(saved.last_raw_source, history.last_raw_source);
        assert_eq!(saved.recent_sources, history.recent_sources);
        let old: WindowGeometry =
            serde_json::from_str(r#"{"x":100,"y":100,"width":900,"height":600}"#).unwrap();
        assert!(!old.maximized);
    }

    #[test]
    fn json_round_trip_and_rename_keep_id() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut view = DocumentDefinition::draft_table("Original".into(), "C:\\source.csv".into());
        let first = repository.save_view(&mut view).unwrap();
        let id = view.id.clone();
        view.name = "Renamed".into();
        let second = repository.save_view(&mut view).unwrap();
        assert_ne!(first, second);
        assert!(!first.exists());
        assert_eq!(view.id, id);
        assert_eq!(repository.load_views().0, vec![view]);
    }

    #[test]
    fn keymap_customizations_round_trip_in_keymap_json() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut keymap = KeymapFile::default();
        keymap
            .add(
                crate::application::keymap::ActionId::Files,
                crate::application::keymap::KeyContext::Application,
                "ctrl-shift-f".into(),
                false,
            )
            .unwrap();

        repository.save_keymap(&keymap).unwrap();

        let path = repository.paths.config.join("keymap.json");
        assert!(path.exists());
        assert_eq!(
            repository.load_keymap(&Settings::default()).unwrap(),
            keymap
        );
    }

    #[test]
    fn failed_write_preserves_draft_and_previous_file() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut view = DocumentDefinition::draft_table("Good".into(), "C:\\source.csv".into());
        let path = repository.save_view(&mut view).unwrap();
        let original = fs::read(&path).unwrap();
        view.name.clear();
        assert!(repository.save_view(&mut view).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[test]
    fn malformed_destination_does_not_overwrite_or_remove_previous_view() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut view = DocumentDefinition::draft_table("Good".into(), "C:\\source.csv".into());
        let original_path = repository.save_view(&mut view).unwrap();
        let original = fs::read(&original_path).unwrap();
        view.name = "Renamed".into();
        let target = repository
            .paths
            .data
            .join("views")
            .join(format!("Renamed-{}.json", view.id));
        fs::write(&target, "{").unwrap();

        let error = repository.save_view(&mut view).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).unwrap(), b"{");
        assert_eq!(fs::read(&original_path).unwrap(), original);
        assert_eq!(repository.load_views().0[0].name, "Good");
    }

    #[test]
    fn malformed_views_do_not_hide_good_views() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut view = DocumentDefinition::draft_table("Good".into(), "C:\\source.csv".into());
        repository.save_view(&mut view).unwrap();
        fs::write(repository.paths.data.join("views/broken.json"), "{").unwrap();
        let (views, issues) = repository.load_views();
        assert_eq!(views.len(), 1);
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn recovery_resolves_name_collision_without_overwrite() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(StorePaths::for_home(home.path()));
        let mut first = DocumentDefinition::draft_table("Sales".into(), "C:\\source.csv".into());
        repository.save_view(&mut first).unwrap();
        repository.archive(&first.id).unwrap();
        let mut second = DocumentDefinition::draft_table("Sales".into(), "C:\\source.csv".into());
        repository.save_view(&mut second).unwrap();
        let (archives, issues) = repository.load_archives();
        assert!(issues.is_empty());
        let recovered = repository.recover(&archives[0].0).unwrap();
        assert_eq!(recovered.id, first.id);
        assert_eq!(recovered.name, "Sales (2)");
        assert_eq!(repository.load_views().0.len(), 2);
    }
}
