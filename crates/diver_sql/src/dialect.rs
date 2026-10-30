//! Per-dialect tables: what the grammar accepts and which functions take paths.

use crate::complete::StringNameKind;

/// The SQL dialect being parsed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dialect {
    #[default]
    DuckDb,
    /// Polars SQL, as the app's Polars expression editors use it.
    Polars,
}

/// Syntax only some dialects have. The grammar parses all of it either way
/// and reports what the dialect lacks, so the tree stays useful.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// A statement other than a query, SHOW, CREATE TABLE, DROP TABLE,
    /// EXPLAIN, TRUNCATE or DELETE.
    Statement,
    Lambda,
    StructLiteral,
    MapLiteral,
    ListComprehension,
    Columns,
    PrefixAlias,
    PositionalRef,
    Factorial,
    ExportState,
    Glob,
    SimilarTo,
    Collate,
    AtTimeZone,
    Pivot,
    Sample,
    AsofJoin,
    PositionalJoin,
    Lateral,
    GroupingSets,
}

impl Feature {
    fn describe(self) -> &'static str {
        match self {
            Feature::Statement => "this statement",
            Feature::Lambda => "lambdas",
            Feature::StructLiteral => "struct literals",
            Feature::MapLiteral => "MAP literals",
            Feature::ListComprehension => "list comprehensions",
            Feature::Columns => "COLUMNS(...)",
            Feature::PrefixAlias => "prefix aliases (`name: expr`)",
            Feature::PositionalRef => "positional references (`#1`)",
            Feature::Factorial => "the `!` factorial",
            Feature::ExportState => "EXPORT_STATE",
            Feature::Glob => "GLOB",
            Feature::SimilarTo => "SIMILAR TO",
            Feature::Collate => "COLLATE",
            Feature::AtTimeZone => "AT TIME ZONE",
            Feature::Pivot => "PIVOT and UNPIVOT",
            Feature::Sample => "samples",
            Feature::AsofJoin => "ASOF joins",
            Feature::PositionalJoin => "POSITIONAL joins",
            Feature::Lateral => "LATERAL",
            Feature::GroupingSets => "GROUPING SETS, CUBE and ROLLUP",
        }
    }
}

impl Dialect {
    pub fn supports(self, feature: Feature) -> bool {
        match self {
            Dialect::DuckDb => true,
            // Checked against polars-sql 0.55 (sqlparser's GenericDialect).
            Dialect::Polars => {
                let _ = feature;
                false
            }
        }
    }

    /// The message for a feature the dialect lacks.
    pub(crate) fn unsupported(self, feature: Feature) -> String {
        let dialect = match self {
            Dialect::DuckDb => "DuckDB",
            Dialect::Polars => "Polars SQL",
        };
        format!("{dialect} doesn't support {}", feature.describe())
    }
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
    // Polars
    "read_ipc",
];

/// Whether the function `name` takes a path as its first argument.
pub fn takes_path(name: &str) -> bool {
    let name = name.trim_matches('"');
    PATH_FUNCTIONS.iter().any(|f| f.eq_ignore_ascii_case(name))
}

/// Functions whose first argument is a string naming a variable, sequence or setting.
const STRING_NAME_FUNCTIONS: &[(&str, StringNameKind)] = &[
    ("getvariable", StringNameKind::Variable),
    ("nextval", StringNameKind::Sequence),
    ("currval", StringNameKind::Sequence),
    ("current_setting", StringNameKind::Setting),
];

/// What the first argument of the function `name` names, when it is a string naming something.
pub fn string_name_arg(name: &str) -> Option<StringNameKind> {
    let name = name.trim_matches('"');
    STRING_NAME_FUNCTIONS
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(name))
        .map(|(_, kind)| *kind)
}
