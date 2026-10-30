//! Columns of the files SQL scripts read, from DESCRIBE on a connection of
//! their own. Each is kept with the file's modified time and size and
//! described again only when those change, so completion never waits on a file.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, SystemTime};

use diver_sql::ColumnInfo;
use duckdb::Connection;

use super::path_completion::{self, PathRoots};
use super::source_database::SourceDatabase;
use crate::domain::sql_editor::FileSource;

/// How long one DESCRIBE may run before it is stopped; sniffing a large CSV can be slow.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(10);

/// What a file (or a glob's folder) looked like when it was described.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

struct Entry {
    /// `None` for a source with no local path to watch, which is described once.
    stamp: Option<Stamp>,
    /// `None` when DESCRIBE failed; it is tried again once the file changes.
    columns: Option<Vec<ColumnInfo>>,
}

#[derive(Default)]
pub struct FileSchemas {
    entries: HashMap<String, Entry>,
}

impl FileSchemas {
    /// Every described source's columns, by [`FileSource::key`].
    pub fn columns(&self) -> HashMap<String, Vec<ColumnInfo>> {
        self.entries
            .iter()
            .filter_map(|(key, entry)| Some((key.clone(), entry.columns.clone()?)))
            .collect()
    }

    /// The sources not described since their file last changed, with the
    /// stamp each has now. A file that isn't there, like a path still being
    /// typed, waits until it is.
    fn stale(&self, sources: &[FileSource], roots: &PathRoots) -> Vec<(FileSource, Option<Stamp>)> {
        sources
            .iter()
            .filter_map(|source| {
                let stamp = stamp(source, roots).ok()?;
                let current = self.entries.get(&source.key).is_some_and(|entry| entry.stamp == stamp);
                (!current).then(|| (source.clone(), stamp))
            })
            .collect()
    }
}

/// What to watch for `source`: `Ok(None)` when it names no local path,
/// `Err` when the file (or a glob's folder) isn't there.
fn stamp(source: &FileSource, roots: &PathRoots) -> Result<Option<Stamp>, ()> {
    let Some(path) = &source.path else {
        return Ok(None);
    };
    // A glob is watched through the folder it starts in, which changes as
    // files are added or removed.
    let glob = path.find(['*', '?', '[', '{']);
    let watched = match glob {
        Some(at) => match path[..at].rfind(['/', '\\']) {
            Some(end) => &path[..=end],
            None => ".",
        },
        None => path.as_str(),
    };
    let resolved = path_completion::resolve(watched, roots).ok_or(())?;
    let metadata = fs::metadata(&resolved).map_err(|_| ())?;
    if glob.is_none() && !metadata.is_file() {
        return Err(());
    }
    Ok(Some(Stamp {
        path: resolved,
        modified: metadata.modified().ok(),
        len: if metadata.is_file() { metadata.len() } else { 0 },
    }))
}

/// The columns DESCRIBE lists, stopped after [`DESCRIBE_TIMEOUT`].
fn describe(connection: &Connection, sql: &str) -> Result<Vec<ColumnInfo>, String> {
    let interrupt = connection.interrupt_handle();
    let (done, finished) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        if finished.recv_timeout(DESCRIBE_TIMEOUT) == Err(mpsc::RecvTimeoutError::Timeout) {
            interrupt.interrupt();
        }
    });
    let columns = (|| {
        let mut statement = connection.prepare(sql)?;
        statement
            .query_map([], |row| {
                Ok(ColumnInfo {
                    name: row.get(0)?,
                    data_type: row.get(1).ok(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()
    })()
    .map_err(|error| error.to_string());
    drop(done);
    columns
}

/// A connection to `database` for DESCRIBE, apart from the SQL editor's so
/// a long query never holds it up, finding relative paths where the editor's does.
pub fn connection(database: &SourceDatabase, roots: &PathRoots) -> Result<Connection, String> {
    let connection = database.connect()?;
    if !roots.search.is_empty() {
        let search = roots
            .search
            .iter()
            .map(|path| path.display().to_string().replace('\'', "''"))
            .collect::<Vec<_>>()
            .join(",");
        // Best effort: without it relative paths resolve as DuckDB's default does.
        let _ = connection.execute_batch(&format!("SET SESSION file_search_path = '{search}'"));
    }
    Ok(connection)
}

/// Describes the sources of `sources` that are new or whose file changed,
/// on a connection `connect` opens only when one is. Returns every known
/// source's columns and whether any were described.
pub fn refresh(
    schemas: &Mutex<FileSchemas>,
    sources: &[FileSource],
    roots: &PathRoots,
    connect: impl FnOnce() -> Result<Connection, String>,
) -> Result<(HashMap<String, Vec<ColumnInfo>>, bool), String> {
    let lock = || schemas.lock().map_err(|_| "file schema lock poisoned".to_string());
    let stale = lock()?.stale(sources, roots);
    let described = !stale.is_empty();
    if described {
        let connection = connect()?;
        for (source, stamp) in stale {
            let columns = describe(&connection, &source.describe).ok();
            lock()?.entries.insert(source.key, Entry { stamp, columns });
        }
    }
    Ok((lock()?.columns(), described))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sql_editor::file_sources;

    /// A connection that finds relative paths in `dir`, as the SQL editor's
    /// does through `file_search_path`.
    fn connection_in(dir: &std::path::Path) -> Result<Connection, String> {
        let connection = Connection::open_in_memory().map_err(|e| e.to_string())?;
        connection
            .execute_batch(&format!("SET file_search_path = '{}'", dir.display()))
            .map_err(|e| e.to_string())?;
        Ok(connection)
    }

    fn names(columns: &HashMap<String, Vec<ColumnInfo>>, key: &str) -> Vec<String> {
        columns[key].iter().map(|c| format!("{} {}", c.name, c.data_type.as_deref().unwrap_or("?"))).collect()
    }

    #[test]
    fn describes_a_file_once_until_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let roots = PathRoots {
            home: None,
            search: vec![dir.path().to_path_buf()],
            working: None,
        };
        let writer = Connection::open_in_memory().unwrap();
        let file = dir.path().join("a.parquet").display().to_string().replace('\\', "/");
        writer
            .execute_batch(&format!("COPY (SELECT 1 AS id, 'x' AS name) TO '{file}'"))
            .unwrap();
        let sources = file_sources(
            "FROM 'a.parquet' a SELECT a.;\nSELECT * FROM read_csv('missing.csv') m, 'still/typ'",
        );
        assert_eq!(sources.len(), 3);
        let schemas = Mutex::new(FileSchemas::default());
        let connect = || connection_in(dir.path());

        let (columns, described) = refresh(&schemas, &sources, &roots, connect).unwrap();
        assert!(described);
        assert_eq!(names(&columns, "'a.parquet'"), ["id INTEGER", "name VARCHAR"]);
        // Files that aren't there are neither described nor kept.
        assert_eq!(columns.len(), 1);

        let unchanged = refresh(&schemas, &sources, &roots, || panic!("nothing changed")).unwrap();
        assert!(!unchanged.1);
        assert_eq!(unchanged.0, columns);

        writer
            .execute_batch(&format!("COPY (SELECT 1 AS id, 'x' AS name, 2.5 AS score) TO '{file}'"))
            .unwrap();
        let (columns, described) = refresh(&schemas, &sources, &roots, connect).unwrap();
        assert!(described);
        assert_eq!(names(&columns, "'a.parquet'"), ["id INTEGER", "name VARCHAR", "score DECIMAL(2,1)"]);
    }

    #[test]
    fn a_glob_is_described_again_when_its_folder_changes() {
        let dir = tempfile::tempdir().unwrap();
        let roots = PathRoots {
            home: None,
            search: vec![dir.path().to_path_buf()],
            working: None,
        };
        std::fs::create_dir(dir.path().join("parts")).unwrap();
        let writer = Connection::open_in_memory().unwrap();
        let part = |n: u32| dir.path().join(format!("parts/{n}.csv")).display().to_string().replace('\\', "/");
        writer.execute_batch(&format!("COPY (SELECT 1 AS id) TO '{}'", part(1))).unwrap();
        let sources = file_sources("SELECT * FROM read_csv('parts/*.csv', union_by_name = true)");
        let schemas = Mutex::new(FileSchemas::default());
        let connect = || connection_in(dir.path());
        let key = "read_csv('parts/*.csv', union_by_name = true)";

        let (columns, _) = refresh(&schemas, &sources, &roots, connect).unwrap();
        assert_eq!(names(&columns, key), ["id BIGINT"]);
        assert!(!refresh(&schemas, &sources, &roots, || panic!("nothing changed")).unwrap().1);

        // A second part, with another column, changes the folder.
        std::thread::sleep(Duration::from_millis(20));
        writer.execute_batch(&format!("COPY (SELECT 2 AS id, 'b' AS extra) TO '{}'", part(2))).unwrap();
        let (columns, described) = refresh(&schemas, &sources, &roots, connect).unwrap();
        assert!(described);
        assert_eq!(names(&columns, key), ["id BIGINT", "extra VARCHAR"]);
    }
}
