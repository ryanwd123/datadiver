use std::path::Path;
use std::sync::Arc;

use crate::application::table_preferences::TablePreferencesWriter;
use crate::domain::table_preferences::FileSettings;
use crate::infrastructure::json_store::ViewRepository;
use crate::infrastructure::source_database::LoadMode;
use crate::infrastructure::sources::{ReadOptions, Sources};
use crate::infrastructure::store::Store;

pub fn read_source(
    path: &Path,
    sources: &Arc<Sources>,
    options: ReadOptions,
    mode: LoadMode,
) -> Result<Store, String> {
    sources.load(path, options, mode)
}

/// How `path` is read: its own file settings, or the default engine, and the
/// profile's Preserve order setting. Reads
/// the profile, so call it on a worker.
pub fn read_options(
    repository: &ViewRepository,
    table_writer: &TablePreferencesWriter,
    path: &Path,
) -> ReadOptions {
    file_read(repository, table_writer, path).0
}

/// Like [`read_options`], with the file's own settings they come from.
pub fn file_read(
    repository: &ViewRepository,
    table_writer: &TablePreferencesWriter,
    path: &Path,
) -> (ReadOptions, FileSettings) {
    let settings = repository.load_settings().unwrap_or_default();
    let file = table_writer
        .load(path)
        .map(|preferences| preferences.file)
        .unwrap_or_default();
    let options = ReadOptions {
        engine: file.engine_or(settings.default_engine),
        preserve_order: settings.preserve_order,
        row_limit: usize::try_from(settings.query_limit).unwrap_or(usize::MAX),
    };
    (options, file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use crate::infrastructure::store::Engine;

    #[test]
    fn gzip_csv_reads_in_both_engines_and_refreshes_changed_data() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(&home.path().join("profile"));
        let writer = duckdb::Connection::open_in_memory().unwrap();
        for name in ["report.csv.gz", "REPORT.CSV.GZ"] {
            let path = home.path().join(name);
            writer.execute("COPY (SELECT 'North, East' AS region, 10 AS sales UNION ALL SELECT 'South', 20) TO ? (FORMAT CSV, HEADER true, COMPRESSION GZIP)", duckdb::params![path.to_string_lossy().as_ref()]).unwrap();
            assert!(std::fs::read(&path).unwrap().starts_with(&[0x1f, 0x8b]));
            for engine in Engine::ALL {
                let options = ReadOptions {
                    engine,
                    ..ReadOptions::default()
                };
                let store = read_source(&path, &sources, options, LoadMode::Check).unwrap();
                let standalone = Store::from(DuckDbStore::open_source(&path).unwrap());
                for store in [&store, &standalone] {
                    assert_eq!(store.row_count(), 2);
                    let rows = store.fetch_row_ids(&[0, 1]).unwrap();
                    assert_eq!(rows[0].values, [Some("North, East".into()), Some("10".into())]);
                    assert_eq!(rows[1].values, [Some("South".into()), Some("20".into())]);
                }
            }
            writer.execute("COPY (SELECT 'Updated' AS region, 300 AS sales) TO ? (FORMAT CSV, HEADER true, COMPRESSION GZIP)", duckdb::params![path.to_string_lossy().as_ref()]).unwrap();
            for engine in Engine::ALL {
                let options = ReadOptions {
                    engine,
                    ..ReadOptions::default()
                };
                let refreshed = read_source(&path, &sources, options, LoadMode::Check).unwrap();
                assert_eq!(refreshed.row_count(), 1);
                assert_eq!(
                    refreshed.fetch_row_ids(&[0]).unwrap()[0].values,
                    [Some("Updated".into()), Some("300".into())]
                );
            }
            // Remove the first case variant before opening the other on a case-insensitive filesystem.
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn unsupported_spreadsheet_formats_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        for extension in ["xls", "xlsb", "ods"] {
            let path = Path::new("report").with_extension(extension);
            for engine in Engine::ALL {
                let options = ReadOptions {
                    engine,
                    ..ReadOptions::default()
                };
                let error = match read_source(&path, &sources, options, LoadMode::Check) {
                    Ok(_) => panic!("{extension} should not be supported"),
                    Err(error) => error,
                };
                assert!(error.contains("reads .xlsx files"));
            }
        }
    }

    #[test]
    fn file_settings_choose_the_engine_and_otherwise_the_default_does() {
        let home = tempfile::tempdir().unwrap();
        let repository = ViewRepository::new(crate::infrastructure::json_store::StorePaths::for_home(home.path()));
        let writer = TablePreferencesWriter::for_repository(&repository);
        let path = home.path().join("a.csv");
        let options = read_options(&repository, &writer, &path);
        assert_eq!(options.engine, Engine::Polars);
        assert!(options.preserve_order);
        let mut settings = repository.load_settings().unwrap();
        settings.default_engine = Engine::DuckDb;
        settings.preserve_order = false;
        repository.save_settings(&settings).unwrap();
        assert_eq!(read_options(&repository, &writer, &path).engine, Engine::DuckDb);
        writer
            .edit(
                path.clone(),
                crate::domain::table_preferences::TablePreferencesPatch {
                    file: Some(crate::domain::table_preferences::FileSettings {
                        engine: Some(Engine::Polars),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
        let options = read_options(&repository, &writer, &path);
        assert_eq!(options.engine, Engine::Polars);
        // Preserve order is the profile's, not the file's.
        assert!(!options.preserve_order);
    }
}
