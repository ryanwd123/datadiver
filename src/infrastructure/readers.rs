use std::path::Path;
use std::sync::Arc;

use crate::infrastructure::duckdb_store::DuckDbStore;
use crate::infrastructure::source_database::{LoadMode, SourceDatabase, source_reader};

pub fn read_source(
    path: &Path,
    database: &Arc<SourceDatabase>,
    mode: LoadMode,
) -> Result<DuckDbStore, String> {
    source_reader(path)?;
    database.load(path, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_csv_imports_through_both_readers_and_refreshes_persistent_data() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(&home.path().join("profile"));
        let writer = duckdb::Connection::open_in_memory().unwrap();
        for name in ["report.csv.gz", "REPORT.CSV.GZ"] {
            let path = home.path().join(name);
            writer.execute("COPY (SELECT 'North, East' AS region, 10 AS sales UNION ALL SELECT 'South', 20) TO ? (FORMAT CSV, HEADER true, COMPRESSION GZIP)", duckdb::params![path.to_string_lossy().as_ref()]).unwrap();
            assert!(std::fs::read(&path).unwrap().starts_with(&[0x1f, 0x8b]));
            let persistent = read_source(&path, &database, LoadMode::Check).unwrap();
            let standalone = DuckDbStore::open_source(&path).unwrap();
            for store in [&persistent, &standalone] {
                assert_eq!(store.row_count(), 2);
                let rows = store.fetch_row_ids(&[0, 1]).unwrap();
                assert_eq!(
                    rows[0].values,
                    [Some("North, East".into()), Some("10".into())]
                );
                assert_eq!(rows[1].values, [Some("South".into()), Some("20".into())]);
            }
            let identity = persistent.persistent_identity().unwrap();
            assert!(matches!(
                database
                    .load_reusing(&path, LoadMode::Check, Some(&identity))
                    .unwrap(),
                crate::infrastructure::source_database::SourceLoad::Reused
            ));
            writer.execute("COPY (SELECT 'Updated' AS region, 300 AS sales) TO ? (FORMAT CSV, HEADER true, COMPRESSION GZIP)", duckdb::params![path.to_string_lossy().as_ref()]).unwrap();
            let refreshed = read_source(&path, &database, LoadMode::Check).unwrap();
            assert_eq!(refreshed.row_count(), 1);
            assert_eq!(
                refreshed.fetch_row_ids(&[0]).unwrap()[0].values,
                [Some("Updated".into()), Some("300".into())]
            );
            // Remove the first case variant before opening the other on a case-insensitive filesystem.
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn unsupported_spreadsheet_formats_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(home.path());
        for extension in ["xls", "xlsb", "ods"] {
            let path = Path::new("report").with_extension(extension);
            let error = match read_source(&path, &database, LoadMode::Check) {
                Ok(_) => panic!("{extension} should not be supported"),
                Err(error) => error,
            };
            assert!(error.contains("reads .xlsx files"));
        }
    }
}
