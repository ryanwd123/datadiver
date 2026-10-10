use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::UNIX_EPOCH;

use duckdb::{Connection, OptionalExt, params};

use super::duckdb_store::{DuckDbStore, quote_identifier};
use crate::domain::documents::{DatabaseView, RelationKind};

const DATABASE_VERSION: i32 = 2;
type Profiles = HashMap<PathBuf, Weak<SourceDatabase>>;
static PROFILES: OnceLock<Mutex<Profiles>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadMode {
    Check,
    Force,
}

#[derive(Clone)]
pub struct PersistentSourceIdentity {
    pub(crate) database: Arc<SourceDatabase>,
    pub(crate) table_sql: String,
    pub(crate) source_version: (i64, i64),
    pub(crate) database_revision: u64,
}

pub enum SourceLoad {
    Reused,
    Loaded(Box<DuckDbStore>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    modified_ns: i128,
    size: u64,
}

#[derive(Clone, Debug)]
struct CatalogEntry {
    file_id: i64,
    revision: i64,
    table_name: String,
    stamp: FileStamp,
    renamed_rowid: Option<String>,
}

struct DatabaseState {
    connection: Connection,
    startup_error: Option<String>,
}

/// One database instance per profile, with independent query connections.
/// The writer mutex protects imports and catalog changes, not UI queries.
pub struct SourceDatabase {
    path: PathBuf,
    state: Mutex<Option<DatabaseState>>,
    revision: std::sync::atomic::AtomicU64,
}

pub(crate) struct PersistentBacking {
    pub database: Arc<SourceDatabase>,
    // Identifies the current import; materialized query results have no source version.
    pub source_version: Option<(i64, i64)>,
}

impl SourceDatabase {
    /// Called after the application loop exits, when blocking no longer affects UI input.
    pub fn checkpoint_profiles() -> Result<(), String> {
        let databases = PROFILES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        for database in databases {
            database.checkpoint()?;
        }
        Ok(())
    }

    pub fn checkpoint(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "database writer lock poisoned")?;
        if let Some(state) = state.as_mut() {
            state
                .connection
                .execute_batch("CHECKPOINT")
                .map_err(|error| format!("database checkpoint failed: {error}"))?;
        }
        Ok(())
    }
    pub fn for_profile(config: &Path) -> Arc<Self> {
        let profile = normalized_path(config);
        let mut profiles = PROFILES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        profiles.retain(|_, database| database.strong_count() > 0);
        if let Some(database) = profiles.get(&profile).and_then(Weak::upgrade) {
            return database;
        }
        let database = Arc::new(Self {
            path: profile.join("datadiver.duckdb"),
            state: Mutex::new(None),
            revision: std::sync::atomic::AtomicU64::new(0),
        });
        profiles.insert(profile, Arc::downgrade(&database));
        database
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn invalidate(&self) {
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    pub fn connect(&self) -> Result<Connection, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "database writer lock poisoned")?;
        self.ensure_initialized(&mut state)?;
        state
            .as_ref()
            .unwrap()
            .connection
            .try_clone()
            .map_err(|e| e.to_string())
    }

    pub fn initialize(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "database writer lock poisoned")?;
        self.ensure_initialized(&mut state)
    }

    pub fn startup_error(&self) -> Option<String> {
        self.state.lock().ok()?.as_ref()?.startup_error.clone()
    }

    pub fn views(&self) -> Result<Vec<DatabaseView>, String> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT schema_name, view_name FROM duckdb_views() \
             WHERE NOT internal AND NOT temporary AND database_name = current_database() \
             ORDER BY lower(schema_name), lower(view_name)",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| {
                Ok(DatabaseView::view(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// Tables and views in the profile database and every attached database,
    /// grouped by database then schema, including `temp` and the imported
    /// sources in `files`. DataDiver's own catalog tables are left out.
    pub fn relations(&self) -> Result<Vec<DatabaseView>, String> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "FROM ( \
                     SELECT database_name, schema_name, table_name, false FROM duckdb_tables() \
                     WHERE NOT internal \
                     UNION ALL \
                     SELECT database_name, schema_name, view_name, true FROM duckdb_views() \
                     WHERE NOT internal \
                 ) t(catalog, schema, name, is_view) \
                 SELECT catalog, catalog = current_database(), schema, name, is_view \
                 ORDER BY 2 DESC, lower(catalog), lower(schema), lower(name)",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(rows
            .into_iter()
            .filter(|(_, _, _, name, _)| !name.starts_with("__qp_") && !name.starts_with("__dd_"))
            .map(|(catalog, current, schema, name, is_view)| DatabaseView {
                catalog: (!current).then_some(catalog),
                schema,
                name,
                kind: if is_view {
                    RelationKind::View
                } else {
                    RelationKind::Table
                },
            })
            .collect())
    }

    /// DuckDB's `duckdb_*()` metadata table functions that take no arguments.
    pub fn metadata_functions(&self) -> Result<Vec<DatabaseView>, String> {
        let connection = self.connect()?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT function_name FROM duckdb_functions() \
                 WHERE function_type = 'table' AND starts_with(function_name, 'duckdb_') \
                 AND len(parameters) = 0 ORDER BY 1",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| {
                row.get::<_, String>(0).map(DatabaseView::function)
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    fn relation_exists(&self, view: &DatabaseView) -> Result<bool, String> {
        if view.kind == RelationKind::Function {
            return Ok(self.metadata_functions()?.contains(view));
        }
        Ok(self.relations()?.contains(view) || self.views()?.contains(view))
    }

    pub fn load_view(self: &Arc<Self>, view: &DatabaseView) -> Result<DuckDbStore, String> {
        if !self.relation_exists(view)? {
            return Err(format!("Database view {} no longer exists", view.label()));
        }
        DuckDbStore::from_database_view(self.connect()?, self.clone(), view)
    }

    fn ensure_initialized(&self, state: &mut Option<DatabaseState>) -> Result<(), String> {
        if state.is_some() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create settings folder: {error}"))?;
        }
        let connection = Connection::open(&self.path).map_err(|error| format!(
            "cannot open database {}: {error}. If another DataDiver process is using this profile, close it first",
            self.path.display()
        ))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS main.__qp_storage_version (version INTEGER NOT NULL);",
            )
            .map_err(|error| error.to_string())?;
        let version = connection
            .query_row("SELECT version FROM main.__qp_storage_version", [], |row| {
                row.get::<_, i32>(0)
            })
            .optional()
            .map_err(|error| error.to_string())?;
        if version.is_some_and(|version| version != DATABASE_VERSION) {
            return Err(format!(
                "unsupported DataDiver database version: {}",
                version.unwrap()
            ));
        }
        connection.execute_batch(
            "BEGIN TRANSACTION;
             INSERT INTO main.__qp_storage_version SELECT 2 WHERE NOT EXISTS (SELECT 1 FROM main.__qp_storage_version);
             CREATE SCHEMA IF NOT EXISTS files;
             CREATE SEQUENCE IF NOT EXISTS main.__qp_file_id_seq;
             CREATE TABLE IF NOT EXISTS main.__qp_file_catalog (
                 path_key VARCHAR PRIMARY KEY, source_path VARCHAR NOT NULL,
                 file_id BIGINT NOT NULL, revision BIGINT NOT NULL, table_name VARCHAR NOT NULL UNIQUE,
                 modified_ns HUGEINT NOT NULL, size UBIGINT NOT NULL,
                 created_at TIMESTAMP NOT NULL, updated_at TIMESTAMP NOT NULL,
                 row_count UBIGINT NOT NULL, renamed_rowid VARCHAR
             );
             COMMIT;"
        ).map_err(|error| error.to_string())?;
        let script = self.path.parent().unwrap().join("startup.sql");
        let startup_error = run_startup_script(&connection, &script).err();
        let startup_error = startup_error.or_else(|| share_path_settings(&connection).err());
        let initialized = DatabaseState {
            connection,
            startup_error,
        };
        sweep_missing(&initialized.connection)?;
        *state = Some(initialized);
        Ok(())
    }

    pub fn load(self: &Arc<Self>, path: &Path, mode: LoadMode) -> Result<DuckDbStore, String> {
        self.load_observed(path, mode, || {})
    }

    /// Checks freshness before retaining an existing query connection and its caches.
    pub fn load_reusing(
        self: &Arc<Self>,
        path: &Path,
        mode: LoadMode,
        existing: Option<&PersistentSourceIdentity>,
    ) -> Result<SourceLoad, String> {
        self.load_internal(path, mode, existing, || {})
    }

    fn load_observed(
        self: &Arc<Self>,
        path: &Path,
        mode: LoadMode,
        after_import: impl FnMut(),
    ) -> Result<DuckDbStore, String> {
        match self.load_internal(path, mode, None, after_import)? {
            SourceLoad::Loaded(store) => Ok(*store),
            SourceLoad::Reused => unreachable!("no existing source was supplied"),
        }
    }

    fn load_internal(
        self: &Arc<Self>,
        path: &Path,
        mode: LoadMode,
        existing: Option<&PersistentSourceIdentity>,
        mut after_import: impl FnMut(),
    ) -> Result<SourceLoad, String> {
        let mut state_guard = self
            .state
            .lock()
            .map_err(|_| "database writer lock poisoned")?;
        self.ensure_initialized(&mut state_guard)?;
        let state = state_guard.as_mut().unwrap();
        let source_path = absolute_source_path(path);
        let key = path_key(&source_path);
        let mut stamp = match file_stamp(&source_path) {
            Ok(stamp) => stamp,
            Err(error) => {
                if error.kind() == std::io::ErrorKind::NotFound {
                    remove_source(&state.connection, &key)?;
                }
                return Err(format!("cannot access source {}: {error}", path.display()));
            }
        };
        let reader = source_reader(&source_path)?;
        let table_name = source_table_name(&source_path)?;
        let duplicate = state.connection.query_row(
            "SELECT path_key, source_path FROM main.__qp_file_catalog WHERE table_name = ? AND path_key <> ?",
            params![table_name, key], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).optional().map_err(|error| error.to_string())?;
        if let Some((duplicate_key, duplicate)) = duplicate {
            if fs::metadata(&duplicate)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                remove_source(&state.connection, &duplicate_key)?;
            } else {
                return Err(format!(
                    "duplicate file name: {} and {duplicate} both map to files.{table_name}; rename one of the files",
                    source_path.display()
                ));
            }
        }
        let previous = catalog_entry(&state.connection, &key)?;
        if mode == LoadMode::Check
            && let Some(entry) = &previous
            && entry.stamp == stamp
            && table_exists(&state.connection, &entry.table_name)?
        {
            let table_sql = format!("files.{}", quote_identifier(&entry.table_name));
            if existing.is_some_and(|identity| {
                Arc::ptr_eq(&identity.database, self)
                    && identity.table_sql == table_sql
                    && identity.source_version == (entry.file_id, entry.revision)
                    && identity.database_revision == self.revision()
            }) {
                return Ok(SourceLoad::Reused);
            }
            return self
                .store_for_entry(state, entry)
                .map(|store| SourceLoad::Loaded(Box::new(store)));
        }

        for attempt in 0..2 {
            let transaction = state
                .connection
                .transaction()
                .map_err(|error| error.to_string())?;
            let file_id = if let Some(entry) = &previous {
                entry.file_id
            } else {
                transaction
                    .query_row("SELECT nextval('main.__qp_file_id_seq')", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| error.to_string())?
            };
            let revision = previous.as_ref().map_or(1, |entry| entry.revision + 1);
            let table_name = table_name.clone();
            let table_sql = format!("files.{}", quote_identifier(&table_name));
            transaction
                .execute_batch(&format!("DROP TABLE IF EXISTS {table_sql}"))
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    &format!("CREATE TABLE {table_sql} AS SELECT * FROM {reader}"),
                    params![source_path.to_string_lossy().as_ref()],
                )
                .map_err(|error| {
                    let hint = if reader.starts_with("read_xlsx") {
                        "; the first XLSX import may require internet access to install DuckDB's excel extension"
                    } else { "" };
                    format!("could not import {}: {error}{hint}", source_path.display())
                })?;
            let renamed_rowid = normalize_rowid(&transaction, &table_sql)?;
            let row_count = transaction
                .query_row(&format!("SELECT count(*) FROM {table_sql}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())? as usize;
            after_import();
            let after = match file_stamp(&source_path) {
                Ok(stamp) => stamp,
                Err(error) => {
                    transaction.rollback().map_err(|error| error.to_string())?;
                    if error.kind() == std::io::ErrorKind::NotFound {
                        remove_source(&state.connection, &key)?;
                    }
                    return Err(format!("cannot verify source after importing: {error}"));
                }
            };
            if after != stamp {
                transaction.rollback().map_err(|error| error.to_string())?;
                if attempt == 1 {
                    return Err("source changed repeatedly while importing; try again when the file is stable".into());
                }
                stamp = after;
                continue;
            }
            transaction.execute(
                "INSERT INTO main.__qp_file_catalog (path_key, source_path, file_id, revision, table_name, modified_ns, size, created_at, updated_at, row_count, renamed_rowid)
                 VALUES (?, ?, ?, ?, ?, ?, ?, current_timestamp, current_timestamp, ?, ?)
                 ON CONFLICT (path_key) DO UPDATE SET
                 source_path = excluded.source_path, revision = excluded.revision, table_name = excluded.table_name,
                 modified_ns = excluded.modified_ns, size = excluded.size, updated_at = excluded.updated_at,
                 row_count = excluded.row_count, renamed_rowid = excluded.renamed_rowid",
                params![key, source_path.to_string_lossy().as_ref(), file_id, revision, table_name, stamp.modified_ns, stamp.size, row_count as u64, renamed_rowid]
            ).map_err(|error| error.to_string())?;
            transaction.commit().map_err(|error| error.to_string())?;
            self.invalidate();
            let entry = CatalogEntry {
                file_id,
                revision,
                table_name,
                stamp,
                renamed_rowid,
            };
            let store = self.store_for_entry(state, &entry)?;
            return Ok(SourceLoad::Loaded(Box::new(store)));
        }
        unreachable!()
    }

    fn store_for_entry(
        self: &Arc<Self>,
        state: &mut DatabaseState,
        entry: &CatalogEntry,
    ) -> Result<DuckDbStore, String> {
        let connection = state
            .connection
            .try_clone()
            .map_err(|error| error.to_string())?;
        let table_sql = format!("files.{}", quote_identifier(&entry.table_name));
        let row_count = connection
            .query_row(&format!("SELECT count(*) FROM {table_sql}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| e.to_string())? as usize;
        DuckDbStore::from_persistent(
            connection,
            format!("files.{}", quote_identifier(&entry.table_name)),
            entry.renamed_rowid.clone(),
            row_count,
            PersistentBacking {
                database: self.clone(),
                source_version: Some((entry.file_id, entry.revision)),
            },
        )
    }

    pub fn cleanup(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "database writer lock poisoned")?;
        if let Some(state) = state.as_mut() {
            sweep_missing(&state.connection)?;
        }
        Ok(())
    }
}

fn run_startup_script(connection: &Connection, path: &Path) -> Result<(), String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Cannot read {}: {error}", path.display())),
    };
    for (index, range) in crate::domain::sql_editor::statements(&text)
        .into_iter()
        .enumerate()
    {
        if let Err(error) = connection.execute_batch(&text[range]) {
            // A failed startup transaction must not leave the shared writer inside a transaction.
            let _ = connection.execute_batch("ROLLBACK");
            return Err(format!(
                "{} · Statement {}: {error}",
                path.display(),
                index + 1
            ));
        }
    }
    Ok(())
}

/// `file_search_path` and `home_directory` are per connection in DuckDB, so a
/// plain `SET` in startup.sql would reach only this writer. Making them global
/// lets every query connection, including the SQL editor's, resolve the same paths.
fn share_path_settings(connection: &Connection) -> Result<(), String> {
    for name in ["file_search_path", "home_directory"] {
        let value = connection
            .query_row(
                &format!("SELECT current_setting('{name}')::VARCHAR"),
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        if !value.is_empty() {
            connection
                .execute_batch(&format!(
                    "SET GLOBAL {name} = '{}'",
                    value.replace('\'', "''")
                ))
                .map_err(|error| format!("startup.sql · SET GLOBAL {name}: {error}"))?;
        }
    }
    Ok(())
}

fn source_table_name(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid source file name: {}", path.display()))?;
    Ok(name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect())
}

fn remove_source(connection: &Connection, key: &str) -> Result<(), String> {
    let Some(entry) = catalog_entry(connection, key)? else {
        return Ok(());
    };
    connection
        .execute_batch("BEGIN TRANSACTION")
        .map_err(|error| error.to_string())?;
    let result = (|| {
        connection
            .execute_batch(&format!(
                "DROP TABLE IF EXISTS files.{}",
                quote_identifier(&entry.table_name)
            ))
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM main.__qp_file_catalog WHERE path_key = ?",
                params![key],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute_batch("COMMIT")
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = connection.execute_batch("ROLLBACK");
    }
    result
}

fn sweep_missing(connection: &Connection) -> Result<(), String> {
    let entries = {
        let mut statement = connection
            .prepare("SELECT path_key, source_path FROM main.__qp_file_catalog")
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    for (key, path) in entries {
        if fs::metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) {
            remove_source(connection, &key)?;
        }
    }
    Ok(())
}

fn catalog_entry(connection: &Connection, key: &str) -> Result<Option<CatalogEntry>, String> {
    connection.query_row("SELECT file_id, revision, table_name, modified_ns, size, renamed_rowid FROM main.__qp_file_catalog WHERE path_key = ?", params![key], |row| Ok(CatalogEntry {
        file_id: row.get(0)?, revision: row.get(1)?, table_name: row.get(2)?,
        stamp: FileStamp { modified_ns: row.get(3)?, size: row.get(4)? },
        renamed_rowid: row.get(5)?,
    })).optional().map_err(|error| error.to_string())
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, String> {
    connection.query_row("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'files' AND table_name = ?)", params![name], |row| row.get(0)).map_err(|error| error.to_string())
}

pub(crate) fn source_reader(path: &Path) -> Result<&'static str, String> {
    if is_gzip_csv(path) {
        return Ok(
            "read_csv(?, delim = ',', header = true, auto_detect = true, compression = 'gzip')",
        );
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "parquet" | "pq" => Ok("read_parquet(?)"),
        "csv" => Ok("read_csv(?, delim = ',', header = true, auto_detect = true)"),
        "tsv" => Ok("read_csv(?, delim = '\t', header = true, auto_detect = true)"),
        "xlsx" => Ok("read_xlsx(?, header = true)"),
        "xls" | "xlsb" | "ods" => {
            Err("unsupported spreadsheet format (DataDiver reads .xlsx files)".into())
        }
        _ => Err(format!("unsupported source type: {}", path.display())),
    }
}

pub(crate) fn is_gzip_csv(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".csv.gz"))
}

pub(crate) fn normalize_rowid(
    connection: &Connection,
    table_sql: &str,
) -> Result<Option<String>, String> {
    let names = {
        let mut statement = connection
            .prepare(&format!("DESCRIBE {table_sql}"))
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    if !names.iter().any(|name| name.eq_ignore_ascii_case("rowid")) {
        return Ok(None);
    }
    let original = names
        .iter()
        .find(|name| name.eq_ignore_ascii_case("rowid"))
        .unwrap();
    let mut replacement = format!("{original} (source column)");
    let mut suffix = 2;
    while names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&replacement))
    {
        replacement = format!("{original} (source column {suffix})");
        suffix += 1;
    }
    connection
        .execute_batch(&format!(
            "ALTER TABLE {table_sql} RENAME COLUMN {} TO {}",
            quote_identifier(original),
            quote_identifier(&replacement)
        ))
        .map_err(|error| error.to_string())?;
    Ok(Some(replacement))
}

fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("source is not a regular file"));
    }
    let modified = metadata.modified()?;
    let modified_ns = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    Ok(FileStamp {
        modified_ns,
        size: metadata.len(),
    })
}

fn absolute_source_path(path: &Path) -> PathBuf {
    let absolute = path
        .canonicalize()
        .unwrap_or_else(|_| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    #[cfg(windows)]
    {
        PathBuf::from(normalized.to_string_lossy().trim_start_matches(r"\\?\"))
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

pub(crate) fn normalized_path(path: &Path) -> PathBuf {
    PathBuf::from(path_key(&absolute_source_path(path)))
}

fn path_key(path: &Path) -> String {
    #[cfg(windows)]
    {
        path.to_string_lossy().replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_sql_runs_once_per_database_instance_and_attachments_reach_cloned_connections() {
        let home = tempfile::tempdir().unwrap();
        let attached_path = home.path().join("attached.duckdb");
        let attached = Connection::open(&attached_path).unwrap();
        attached
            .execute_batch("CREATE TABLE sample AS SELECT 'hello; world' AS value")
            .unwrap();
        drop(attached);
        fs::write(home.path().join("startup.sql"), format!(
            "-- A semicolon in a string is part of one statement.\nCREATE TABLE IF NOT EXISTS main.startup_runs(n INTEGER);\nINSERT INTO main.startup_runs VALUES (1);\nATTACH '{}' AS attached (READ_ONLY);",
            attached_path.to_string_lossy().replace('\'', "''")
        )).unwrap();
        for expected_runs in [1, 2] {
            let database = SourceDatabase::for_profile(home.path());
            database.initialize().unwrap();
            let shared = SourceDatabase::for_profile(home.path());
            assert!(Arc::ptr_eq(&database, &shared));
            shared.initialize().unwrap();
            for connection in [database.connect().unwrap(), shared.connect().unwrap()] {
                assert_eq!(
                    connection
                        .query_row("SELECT count(*) FROM main.startup_runs", [], |row| row
                            .get::<_, i64>(0))
                        .unwrap(),
                    expected_runs
                );
                assert_eq!(
                    connection
                        .query_row("SELECT value FROM attached.main.sample", [], |row| row
                            .get::<_, String>(
                            0
                        ))
                        .unwrap(),
                    "hello; world"
                );
            }
            assert!(database.startup_error().is_none());
        }
    }

    #[test]
    fn relations_list_tables_views_and_attached_databases_and_all_open() {
        let home = tempfile::tempdir().unwrap();
        let attached_path = home.path().join("shop.duckdb");
        let attached = Connection::open(&attached_path).unwrap();
        attached
            .execute_batch("CREATE TABLE orders AS SELECT * FROM range(3) t(id)")
            .unwrap();
        drop(attached);
        fs::write(
            home.path().join("startup.sql"),
            format!(
                "ATTACH '{}' AS shop (READ_ONLY);\nCREATE TABLE IF NOT EXISTS main.notes AS SELECT 'a' AS note;\nCREATE OR REPLACE VIEW main.note_view AS SELECT * FROM main.notes;
CREATE TABLE IF NOT EXISTS files.imported AS SELECT 1 AS n;
CREATE TEMP TABLE scratch AS SELECT 1 AS n;",
                attached_path.to_string_lossy().replace('\'', "''")
            ),
        )
        .unwrap();
        let database = SourceDatabase::for_profile(home.path());
        database.initialize().unwrap();
        assert!(database.startup_error().is_none());
        let relations = database.relations().unwrap();
        let orders = DatabaseView {
            catalog: Some("shop".into()),
            kind: RelationKind::Table,
            ..DatabaseView::view("main", "orders")
        };
        let notes = DatabaseView {
            kind: RelationKind::Table,
            ..DatabaseView::view("main", "notes")
        };
        // The profile database comes first, with imported sources in `files`;
        // DataDiver's own tables stay hidden. Temp objects belong to the
        // connection that made them, so startup's temp table is not listed.
        assert_eq!(
            relations,
            [
                DatabaseView {
                    kind: RelationKind::Table,
                    ..DatabaseView::view("files", "imported")
                },
                DatabaseView::view("main", "note_view"),
                notes.clone(),
                orders.clone()
            ]
        );
        assert_eq!(orders.label(), "shop.main.orders");
        let rows = |view: &DatabaseView| {
            let source = database.load_view(view).unwrap();
            crate::domain::pivot::execute_traced_limited(
                &source,
                &crate::domain::documents::QueryConfig::default(),
                &[],
                &crate::domain::pivot::EvaluationClock::system(),
                1000,
                &mut crate::domain::sql::SqlTrace::default(),
            )
            .unwrap()
            .row_count()
        };
        assert_eq!(rows(&orders), 3);
        assert_eq!(rows(&notes), 1);
        let functions = database.metadata_functions().unwrap();
        let tables = DatabaseView::function("duckdb_tables");
        assert!(functions.contains(&tables));
        assert_eq!(tables.label(), "duckdb_tables()");
        assert!(rows(&tables) >= 2);
        assert!(
            database
                .load_view(&DatabaseView::function("duckdb_nope"))
                .is_err()
        );
    }

    #[test]
    fn startup_path_settings_reach_the_sql_editor_connection() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join("it's data");
        fs::create_dir(&data).unwrap();
        fs::write(data.join("fruit.csv"), "name\napple\n").unwrap();
        fs::write(
            home.path().join("startup.sql"),
            format!(
                "SET file_search_path = '{}';",
                data.to_string_lossy().replace('\'', "''")
            ),
        )
        .unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let editor =
            crate::infrastructure::sql_execution::SqlExecution::new(database.clone()).unwrap();
        assert!(database.startup_error().is_none());
        assert_eq!(editor.path_roots().search, std::slice::from_ref(&data));
        let run = editor.run("SELECT name FROM 'fruit.csv'", 10);
        assert!(run.error.is_none(), "{:?}", run.error);
        // A SET in the editor stays on its connection and is picked up after the run.
        let run = editor.run("SET file_search_path = ''", 10);
        assert!(run.path_roots.search.is_empty());
        assert_eq!(
            SourceDatabase::for_profile(home.path())
                .connect()
                .unwrap()
                .query_row("SELECT current_setting('file_search_path')", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            data.to_string_lossy()
        );
    }

    #[test]
    fn failed_startup_sql_leaves_database_usable_and_reports_the_statement() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("startup.sql"), "CREATE TABLE startup_runs AS SELECT 1 AS n;\nBEGIN;\nINSERT INTO missing_table VALUES (1);\nCREATE TABLE should_not_run(n INT);").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        database.initialize().unwrap();
        let error = database.startup_error().unwrap();
        assert!(error.contains("startup.sql · Statement 3:"), "{error}");
        let connection = database.connect().unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM main.startup_runs", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'should_not_run'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        connection
            .execute_batch("CREATE TABLE recovery(n INT)")
            .unwrap();
        database.initialize().unwrap();
    }

    #[test]
    fn user_database_views_are_lazy_limited_and_keep_source_rowid_columns() {
        use crate::domain::documents::{Aggregation, QueryConfig, ScalarKey, SlicerDefinition};
        use crate::domain::pivot::{EvaluationClock, execute_traced_limited};
        use crate::domain::sql::SqlTrace;
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let connection = database.connect().unwrap();
        connection.execute_batch(
            "CREATE SCHEMA \"User schema\"; \
             CREATE VIEW \"User schema\".\"Books' \"\"quoted\" AS \
             SELECT range + 10000 AS rowid, range + 100 AS __qp_rowid, range % 2 AS category FROM range(1000); \
             CREATE TEMP VIEW session_only AS SELECT 1 AS n;"
        ).unwrap();
        let view = DatabaseView::view("User schema", "Books' \"quoted");
        assert_eq!(database.views().unwrap(), [view.clone()]);
        let source = database.load_view(&view).unwrap();
        assert!(source.is_database_view());
        assert_eq!(
            source
                .schema()
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["rowid", "__qp_rowid", "category"]
        );
        let mut trace = SqlTrace::default();
        let result = execute_traced_limited(
            &source,
            &QueryConfig::default(),
            &[],
            &EvaluationClock::system(),
            3,
            &mut trace,
        )
        .unwrap();
        assert_eq!(result.row_count(), 3);
        assert!(!result.is_database_view());
        let rows = result.fetch_row_ids(&[0, 1, 2]).unwrap();
        assert_eq!(
            rows[2].values,
            [Some("10002".into()), Some("102".into()), Some("0".into())]
        );
        assert_eq!(result.schema().len(), 3);
        let query = QueryConfig {
            aggregations: vec![Aggregation::sql_expression(
                "count(*)".into(),
                Some("Rows".into()),
            )],
            ..Default::default()
        };
        let count = execute_traced_limited(
            &source,
            &query,
            &[],
            &EvaluationClock::system(),
            3,
            &mut SqlTrace::default(),
        )
        .unwrap();
        assert_eq!(
            count.fetch_row_ids(&[0]).unwrap()[0].values,
            [Some("1000".into())]
        );
        let filtered = execute_traced_limited(
            &source,
            &QueryConfig::default(),
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "category".into(),
                selected: vec![ScalarKey::Signed("1".into())],
            }],
            &EvaluationClock::system(),
            2,
            &mut SqlTrace::default(),
        )
        .unwrap();
        assert_eq!(
            filtered.fetch_row_ids(&[0, 1]).unwrap()[0].values[0],
            Some("10001".into())
        );
        let pivot = QueryConfig {
            group_by: vec![crate::domain::documents::Dimension::expression(
                "__qp_rowid % 2 AS parity",
            )],
            pivot_by: vec!["category".into()],
            aggregations: query.aggregations.clone(),
            ..Default::default()
        };
        let pivoted = execute_traced_limited(
            &source,
            &pivot,
            &[],
            &EvaluationClock::system(),
            3,
            &mut SqlTrace::default(),
        )
        .unwrap();
        assert_eq!(pivoted.row_count(), 2);
        assert_eq!(pivoted.schema().len(), 3);
        connection
            .execute_batch("DROP VIEW \"User schema\".\"Books' \"\"quoted\"")
            .unwrap();
        assert!(database.views().unwrap().is_empty());
        assert!(
            database
                .load_view(&view)
                .err()
                .unwrap()
                .contains("no longer exists")
        );
        connection.execute_batch("CREATE VIEW main.lazy AS SELECT error('must not evaluate during opening') AS value").unwrap();
        let lazy = database
            .load_view(&DatabaseView::view("main", "lazy"))
            .unwrap();
        assert!(
            execute_traced_limited(
                &lazy,
                &QueryConfig::default(),
                &[],
                &EvaluationClock::system(),
                3,
                &mut SqlTrace::default()
            )
            .is_err()
        );
    }
    use std::time::SystemTime;

    #[test]
    fn checked_reuse_requires_the_same_profile_and_current_table_revision() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("source.csv");
        fs::write(&path, "n\n1\n2\n").unwrap();
        let database = SourceDatabase::for_profile(&home.path().join("profile"));
        let first = database.load(&path, LoadMode::Check).unwrap();
        let identity = first.persistent_identity().unwrap();
        assert!(matches!(
            database
                .load_reusing(&path, LoadMode::Check, Some(&identity))
                .unwrap(),
            SourceLoad::Reused
        ));
        let other = SourceDatabase::for_profile(&home.path().join("other"));
        assert!(matches!(
            other
                .load_reusing(&path, LoadMode::Check, Some(&identity))
                .unwrap(),
            SourceLoad::Loaded(_)
        ));
        let forced = match database
            .load_reusing(&path, LoadMode::Force, Some(&identity))
            .unwrap()
        {
            SourceLoad::Loaded(store) => store,
            _ => panic!("force must create a new revision"),
        };
        // A refresh in another window must also prevent reuse of this old snapshot.
        assert!(matches!(
            database
                .load_reusing(&path, LoadMode::Check, Some(&identity))
                .unwrap(),
            SourceLoad::Loaded(_)
        ));
        let current = forced.persistent_identity().unwrap();
        fs::write(&path, "n\n300\n").unwrap();
        assert!(matches!(
            database
                .load_reusing(&path, LoadMode::Check, Some(&current))
                .unwrap(),
            SourceLoad::Loaded(_)
        ));
        fs::remove_file(&path).unwrap();
        assert!(
            database
                .load_reusing(&path, LoadMode::Check, Some(&current))
                .is_err()
        );
        assert!(first.fetch_row_ids(&[0]).is_err());
        fs::write(&path, "n\n400\n").unwrap();
        assert!(matches!(
            database
                .load_reusing(&path, LoadMode::Check, Some(&identity))
                .unwrap(),
            SourceLoad::Loaded(_)
        ));
    }

    #[test]
    fn import_timestamps_only_change_when_a_revision_is_written() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "n\n1\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        let timestamps = || {
            let guard = database.state.lock().unwrap();
            guard.as_ref().unwrap().connection.query_row("SELECT CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR) FROM main.__qp_file_catalog", [], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).unwrap()
        };
        let first = timestamps();
        let _cached = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(timestamps(), first);
        let _forced = database.load(&source, LoadMode::Force).unwrap();
        let updated = timestamps();
        assert_eq!(first.0, updated.0);
        assert_ne!(first.1, updated.1);
        assert_eq!(old.row_count(), 1);
        database.checkpoint().unwrap();
        assert!(fs::metadata(database.path()).unwrap().len() > 12_288);
    }

    #[test]
    fn changes_during_import_retry_once_and_never_publish_an_unverified_revision() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "n\n1\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        let mut imports = 0;
        let updated = database
            .load_observed(&source, LoadMode::Force, || {
                imports += 1;
                if imports == 1 {
                    fs::write(&source, "n\n200\n300\n").unwrap();
                }
            })
            .unwrap();
        assert_eq!(imports, 2);
        assert_eq!(updated.row_count(), 2);
        let mut changes = 0;
        let error = database
            .load_observed(&source, LoadMode::Force, || {
                changes += 1;
                fs::write(&source, format!("n\n{}\n", "1\n".repeat(changes))).unwrap();
            })
            .err()
            .unwrap();
        assert!(error.contains("changed repeatedly"));
        assert_eq!(
            contents(&old),
            vec![vec![Some("200".into())], vec![Some("300".into())]]
        );
        assert_eq!(updated.row_count(), 2);
        let guard = database.state.lock().unwrap();
        assert_eq!(
            catalog_entry(&guard.as_ref().unwrap().connection, &path_key(&source))
                .unwrap()
                .unwrap()
                .revision,
            2
        );
    }

    #[test]
    fn startup_removes_missing_sources_and_keeps_other_schemas() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        database.initialize().unwrap();
        {
            let guard = database.state.lock().unwrap();
            let connection = &guard.as_ref().unwrap().connection;
            connection.execute_batch("CREATE TABLE files.abandoned (n INTEGER); CREATE SCHEMA future; CREATE TABLE future.keep (n INTEGER)").unwrap();
            connection.execute("INSERT INTO main.__qp_file_catalog VALUES (?, ?, 900, 1, 'abandoned', 0, 0, current_timestamp, current_timestamp, 0, NULL)", params![path_key(&home.path().join("missing.csv")), home.path().join("missing.csv").to_string_lossy().as_ref()]).unwrap();
        }
        drop(database);
        let reopened = SourceDatabase::for_profile(home.path());
        reopened.initialize().unwrap();
        let guard = reopened.state.lock().unwrap();
        let connection = &guard.as_ref().unwrap().connection;
        assert!(!table_exists(connection, "abandoned").unwrap());
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM future.keep", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn invalid_source_access_keeps_the_cache_and_profiles_share_one_owner() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "n\n1\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let alias = SourceDatabase::for_profile(&home.path().join("unused/.."));
        assert!(Arc::ptr_eq(&database, &alias));
        let old = database.load(&source, LoadMode::Check).unwrap();
        fs::remove_file(&source).unwrap();
        fs::create_dir(&source).unwrap();
        assert!(database.load(&source, LoadMode::Check).is_err());
        assert_eq!(contents(&old).len(), 1);
        let guard = database.state.lock().unwrap();
        assert!(
            catalog_entry(&guard.as_ref().unwrap().connection, &path_key(&source))
                .unwrap()
                .is_some()
        );
    }

    fn contents(store: &DuckDbStore) -> Vec<Vec<Option<String>>> {
        store
            .fetch_row_ids(&store.initial_row_ids().unwrap())
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect()
    }

    #[test]
    fn persists_reuses_and_replaces_one_named_table() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "rowid,n,label\n99,1,a\n88,1,a\n77,2,b\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        let first_table = old.table_sql().to_owned();
        assert_eq!(first_table, "files.\"data_csv\"");
        let cached = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(cached.table_sql(), first_table);
        assert_eq!(contents(&old), contents(&cached));
        assert_eq!(old.schema()[0].name, "rowid");
        fs::write(&source, "rowid,n,label,extra\n6,4,new,z\n").unwrap();
        let updated = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(updated.table_sql(), first_table);
        assert_eq!(updated.row_count(), 1);
        assert_eq!(updated.schema().len(), 4);
        assert_eq!(contents(&old).len(), 1);
        assert_eq!(
            old.fetch_row_ids(&[0]).unwrap()[0].values[1],
            Some("4".into())
        );
        let forced = database.load(&source, LoadMode::Force).unwrap();
        assert_eq!(forced.table_sql(), updated.table_sql());
        let guard = database.state.lock().unwrap();
        assert_eq!(
            guard
                .as_ref()
                .unwrap()
                .connection
                .query_row(
                    "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'files'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        drop(guard);
        let latest_table = forced.table_sql().to_owned();
        drop((old, cached, updated, forced));
        database.cleanup().unwrap();
        drop(database);
        let reopened = SourceDatabase::for_profile(home.path());
        assert_eq!(
            reopened.load(&source, LoadMode::Check).unwrap().table_sql(),
            latest_table
        );
    }

    #[test]
    fn missing_sources_remove_the_cached_table_immediately() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "n\n1\n2\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        assert!(
            database
                .load_observed(&source, LoadMode::Force, || fs::remove_file(&source)
                    .unwrap())
                .is_err()
        );
        assert!(database.load(&source, LoadMode::Check).is_err());
        assert!(old.initial_row_ids().is_err());
        drop(old);
        database.cleanup().unwrap();
        let guard = database.state.lock().unwrap();
        let state = guard.as_ref().unwrap();
        assert_eq!(
            state
                .connection
                .query_row(
                    "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'files'",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn failed_refresh_preserves_catalog_and_duplicate_filenames_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let source = home.path().join("same.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute(
                "COPY (SELECT 1 AS n) TO ? (FORMAT PARQUET)",
                params![source.to_string_lossy().as_ref()],
            )
            .unwrap();
        let old = database.load(&source, LoadMode::Check).unwrap();
        fs::write(&source, "invalid parquet").unwrap();
        assert!(database.load(&source, LoadMode::Force).is_err());
        assert_eq!(contents(&old), vec![vec![Some("1".into())]]);
        let guard = database.state.lock().unwrap();
        assert_eq!(
            catalog_entry(&guard.as_ref().unwrap().connection, &path_key(&source))
                .unwrap()
                .unwrap()
                .revision,
            1
        );
        drop(guard);
        for directory in ["a", "b"] {
            fs::create_dir(home.path().join(directory)).unwrap();
            fs::write(home.path().join(directory).join("same.csv"), "n\n3\n").unwrap();
        }
        let a = database
            .load(&home.path().join("a/same.csv"), LoadMode::Check)
            .unwrap();
        let error = database
            .load(&home.path().join("b/same.csv"), LoadMode::Check)
            .err()
            .unwrap();
        assert!(error.contains("duplicate file name"));
        assert!(error.contains("files.same_csv"));
        assert_eq!(contents(&a), vec![vec![Some("3".into())]]);
    }

    #[test]
    fn normalized_file_names_are_sql_safe_and_collisions_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(&home.path().join("profile"));
        for (index, name) in ["Sales.csv", "sales.csv"].iter().enumerate() {
            let directory = home.path().join(index.to_string());
            fs::create_dir(&directory).unwrap();
            let source = directory.join(name);
            fs::write(&source, "n\n1\n").unwrap();
            if index == 0 {
                assert_eq!(
                    database.load(&source, LoadMode::Check).unwrap().table_sql(),
                    "files.\"sales_csv\""
                );
            } else {
                assert!(
                    database
                        .load(&source, LoadMode::Check)
                        .err()
                        .unwrap()
                        .contains("duplicate file name")
                );
            }
        }
        let source = home.path().join("sales-csv.csv");
        fs::write(&source, "n\n1\n").unwrap();
        let first = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(first.table_sql(), "files.\"sales_csv_csv\"");
        let collision = home.path().join("sales_csv.csv");
        fs::write(&collision, "n\n2\n").unwrap();
        assert!(
            database
                .load(&collision, LoadMode::Check)
                .err()
                .unwrap()
                .contains("duplicate file name")
        );
        assert_eq!(contents(&first), vec![vec![Some("1".into())]]);
        fs::remove_file(&source).unwrap();
        assert_eq!(
            contents(&database.load(&collision, LoadMode::Check).unwrap()),
            vec![vec![Some("2".into())]]
        );
    }

    #[test]
    fn metadata_timestamp_changes_are_checked_even_when_size_matches() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.csv");
        fs::write(&source, "n\n1\n").unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        fs::write(&source, "n\n2\n").unwrap();
        let modified = SystemTime::now() + std::time::Duration::from_secs(2);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let updated = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(contents(&updated), vec![vec![Some("2".into())]]);
        assert_eq!(old.table_sql(), updated.table_sql());
    }

    #[test]
    fn size_changes_invalidate_even_when_the_modification_time_is_preserved() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("data.tsv");
        fs::write(&source, "n\tlabel\n1\ta\n").unwrap();
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let old = database.load(&source, LoadMode::Check).unwrap();
        fs::write(&source, "n\tlabel\n1\ta\n2\tb\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let updated = database.load(&source, LoadMode::Check).unwrap();
        assert_eq!(old.table_sql(), updated.table_sql());
        assert_eq!(updated.row_count(), 2);
    }

    #[test]
    fn persistent_queries_pivots_and_statistics_use_the_correct_source_table() {
        use crate::domain::documents::{Aggregation, AggregationOp, QueryConfig};
        use crate::domain::frame::ResultSnapshot;
        use crate::domain::selection::{Cell, Selection};
        use crate::domain::{pivot, statistics};
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let mut results = Vec::new();
        for (name, amount) in [("first", 2), ("second", 9)] {
            let source = home.path().join(format!("{name}.csv"));
            fs::write(
                &source,
                format!("ROWID,n,label\n99,{amount},a\n99,{amount},a\n88,,b\n"),
            )
            .unwrap();
            let store = database.load(&source, LoadMode::Check).unwrap();
            assert_eq!(store.schema()[0].name, "ROWID");
            assert_eq!(
                store.unique_counts(&["n".into()]).unwrap(),
                vec![("n".into(), 1)]
            );
            let config = QueryConfig {
                group_by: vec!["label".into()],
                aggregations: vec![Aggregation {
                    sql: None,
                    source_column: "n".into(),
                    operation: AggregationOp::Sum,
                    name: None,
                    conditions: vec![],
                    percentile: None,
                    interpolation: None,
                    ddof: None,
                }],
                ..Default::default()
            };
            let pivot_store = pivot::execute(&store, &config, &[]).unwrap();
            assert_eq!(contents(&pivot_store)[0][1], Some((2 * amount).to_string()));
            results.push(ResultSnapshot::from_store(pivot_store, 1, 1, 1));
        }
        for (result, expected) in results.into_iter().zip([4, 18]) {
            let stats = statistics::calculate(
                &result,
                Selection {
                    anchor: Cell { row: 0, column: 1 },
                    active: Cell { row: 1, column: 1 },
                },
            )
            .unwrap();
            assert_eq!(stats.numeric_sum, Some(expected.to_string()));
            assert_eq!(stats.nulls, 1);
        }
    }

    #[test]
    fn persistent_dimension_pivots_switch_slicers_without_affecting_retained_results() {
        use crate::domain::documents::{
            Aggregation, AggregationOp, QueryConfig, ScalarKey, SlicerDefinition,
        };
        use crate::domain::{pivot, sql::SqlTrace};
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        let path = home.path().join("athletes.csv");
        fs::write(
            &path,
            "Year,Sex,Name,Season\n2000,F,A,Summer\n2000,M,B,Winter\n2004,F,C,Winter\n",
        )
        .unwrap();
        let store = database.load(&path, LoadMode::Check).unwrap();
        let config = QueryConfig {
            group_by: vec!["Year".into()],
            pivot_by: vec!["Sex".into()],
            aggregations: vec![Aggregation {
                sql: None,
                source_column: "Name".into(),
                operation: AggregationOp::NonNullCount,
                name: None,
                conditions: vec![],
                percentile: None,
                interpolation: None,
                ddof: None,
            }],
            result_sort: vec![],
            totals: false,
            heatmap: false,
        };
        let mut retained = Vec::new();
        for (season, count) in [("Winter", 2), ("Summer", 1), ("Winter", 2)] {
            let mut trace = SqlTrace::default();
            let result = pivot::execute_traced(
                &store,
                &config,
                &[SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "Season".into(),
                    selected: vec![ScalarKey::String(season.into())],
                }],
                &pivot::EvaluationClock::system(),
                &mut trace,
            )
            .unwrap();
            assert_eq!(result.row_count(), count);
            assert_eq!(trace.statements.len(), 3);
            retained.push((result, count));
        }
        for (result, count) in retained {
            assert_eq!(contents(&result).len(), count);
        }
        let mut percentile = config.clone();
        percentile.aggregations[0].source_column = "Year".into();
        percentile.aggregations[0].operation = AggregationOp::Percentile;
        percentile.aggregations[0].percentile = Some(50.0);
        let result = pivot::execute(
            &store,
            &percentile,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("Winter".into())],
            }],
        )
        .unwrap();
        assert_eq!(result.row_count(), 2);
    }
}
