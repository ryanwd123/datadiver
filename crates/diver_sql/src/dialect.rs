//! Per-dialect tables: what the grammar accepts and which functions take paths.

/// The SQL dialect being parsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dialect {
    #[default]
    DuckDb,
    /// Polars SQL, as the app's Polars expression editors use it.
    Polars,
}

/// Table functions whose first argument is a file path or glob.
const PATH_FUNCTIONS: &[&str] = &[
    "read_parquet",
    "parquet_scan",
    "read_csv",
    "read_csv_auto",
    "sniff_csv",
    "read_json",
    "read_json_auto",
    "read_ndjson",
    "read_ndjson_auto",
    "read_json_objects",
    "read_ndjson_objects",
    "read_json_objects_auto",
    "read_text",
    "read_blob",
    "read_xlsx",
    "glob",
    "parquet_metadata",
    "parquet_schema",
    "parquet_file_metadata",
    "parquet_kv_metadata",
    "delta_scan",
    "iceberg_scan",
    "iceberg_metadata",
    "iceberg_snapshots",
    "st_read",
    "read_ipc",
    "scan_parquet",
    "scan_csv",
];

/// Whether the function `name` takes a path as its first argument.
pub fn takes_path(name: &str) -> bool {
    let name = name.trim_matches('"');
    PATH_FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(name))
}
