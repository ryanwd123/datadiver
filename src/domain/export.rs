use serde_json::{Map, Number, Value};

use crate::domain::formatting::{default_numeric_format, display_cell};
use crate::domain::frame::ResultSnapshot;
use crate::domain::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyFormat {
    RawTsv,
    FormattedTsv,
    RawTsvHeaders,
    FormattedTsvHeaders,
    RawCsv,
    JsonRecords,
    JsonColumns,
    PythonList,
}

/// A file format DuckDB's `COPY ... TO` writes, chosen by the file extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFormat {
    Csv,
    Tsv,
    Parquet,
    /// One JSON array of records.
    Json,
    /// One JSON record per line.
    NdJson,
    /// Written by DuckDB's `excel` core extension, which DuckDB installs on first use.
    Xlsx,
}

pub const FILE_FORMAT_HELP: &str = "csv, tsv, parquet, json, ndjson, jsonl, or xlsx; \
     csv, tsv, json, and ndjson may end in .gz or .zst";

impl FileFormat {
    /// The format named by `path`'s extension. Text formats may add a `.gz` or
    /// `.zst` suffix, which DuckDB reads as the compression.
    pub fn from_path(path: &std::path::Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let (stem, compressed) = match name
            .strip_suffix(".gz")
            .or_else(|| name.strip_suffix(".zst"))
        {
            Some(stem) => (stem, true),
            None => (name.as_str(), false),
        };
        let format = match stem.rsplit_once('.').map(|(_, extension)| extension) {
            Some("csv") => Self::Csv,
            Some("tsv" | "tab") => Self::Tsv,
            Some("parquet") => Self::Parquet,
            Some("json") => Self::Json,
            Some("ndjson" | "jsonl") => Self::NdJson,
            Some("xlsx") => Self::Xlsx,
            _ => {
                return Err(format!(
                    "Cannot tell the export format from {name:?}; use {FILE_FORMAT_HELP}"
                ));
            }
        };
        if compressed && matches!(format, Self::Parquet | Self::Xlsx) {
            return Err(format!(
                "{name:?}: parquet and xlsx files cannot end in .gz or .zst"
            ));
        }
        Ok(format)
    }

    /// The DuckDB core extension that writes this format, if it is not built in.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Xlsx => Some("excel"),
            _ => None,
        }
    }

    /// The `COPY ... TO` options.
    pub fn copy_options(self) -> &'static str {
        match self {
            Self::Csv => "FORMAT CSV, HEADER true",
            Self::Tsv => "FORMAT CSV, HEADER true, DELIMITER '\t'",
            Self::Parquet => "FORMAT PARQUET",
            Self::Json => "FORMAT JSON, ARRAY true",
            Self::NdJson => "FORMAT JSON",
            Self::Xlsx => "FORMAT XLSX, HEADER true",
        }
    }
}

pub fn export(
    result: &ResultSnapshot,
    selection: Selection,
    format: CopyFormat,
) -> Result<String, String> {
    let Some(selection) = selection.clamped(result.row_count(), result.column_count()) else {
        return Err("no cells selected".into());
    };
    let (rows, columns) = selection.rectangle();
    let row_indices = rows.collect::<Vec<_>>();
    let column_indices = columns.collect::<Vec<_>>();
    let cached = result.fetch_rows(
        *row_indices.first().ok_or("no rows selected")?,
        row_indices.len(),
    )?;
    let values = row_indices
        .iter()
        .map(|row| {
            let cached_row = cached
                .get(row - row_indices[0])
                .ok_or("selected row is not available")?;
            column_indices
                .iter()
                .map(|&column| {
                    let source_column = result.column_indices[column];
                    let value = cached_row.values.get(source_column).cloned().flatten();
                    Ok((
                        value,
                        result.dtypes[column].as_str(),
                        result.columns[column].0.as_str(),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut output = String::new();
    match format {
        CopyFormat::RawTsv
        | CopyFormat::FormattedTsv
        | CopyFormat::RawTsvHeaders
        | CopyFormat::FormattedTsvHeaders => {
            let formatted = matches!(
                format,
                CopyFormat::FormattedTsv | CopyFormat::FormattedTsvHeaders
            );
            let headers = matches!(
                format,
                CopyFormat::RawTsvHeaders | CopyFormat::FormattedTsvHeaders
            );
            if headers {
                for (index, column) in column_indices.iter().enumerate() {
                    if index > 0 {
                        output.push('\t');
                    }
                    output.push_str(&result.columns[*column].0);
                }
                output.push('\n');
            }
            for (row_index, row) in values.iter().enumerate() {
                for (column_index, (value, dtype, _)) in row.iter().enumerate() {
                    if column_index > 0 {
                        output.push('\t');
                    }
                    output.push_str(&tsv_value(value.as_deref(), dtype, formatted));
                }
                if values.len() > 1 || headers || row_index + 1 < values.len() {
                    output.push('\n');
                }
            }
        }
        CopyFormat::RawCsv => {
            for (row_index, row) in values.iter().enumerate() {
                for (column_index, (value, dtype, _)) in row.iter().enumerate() {
                    if column_index > 0 {
                        output.push(',');
                    }
                    csv_field(&mut output, &tsv_value(value.as_deref(), dtype, false));
                }
                if values.len() > 1 && row_index + 1 < values.len() {
                    output.push('\n');
                }
            }
        }
        CopyFormat::JsonRecords => {
            let records = values
                .iter()
                .map(|row| {
                    let mut record = Map::new();
                    for (value, dtype, name) in row {
                        record.insert(name.to_string(), json_value(value.as_deref(), dtype));
                    }
                    Value::Object(record)
                })
                .collect::<Vec<_>>();
            output = serde_json::to_string(&records).map_err(|error| error.to_string())?;
        }
        CopyFormat::JsonColumns => {
            let mut object = Map::new();
            for (column_index, &column) in column_indices.iter().enumerate() {
                let column_values = values
                    .iter()
                    .map(|row| json_value(row[column_index].0.as_deref(), row[column_index].1))
                    .collect::<Vec<_>>();
                object.insert(
                    result.columns[column].0.clone(),
                    Value::Array(column_values),
                );
            }
            output = serde_json::to_string(&object).map_err(|error| error.to_string())?;
        }
        CopyFormat::PythonList => {
            let one_dimensional = values.len() == 1 || column_indices.len() == 1;
            output.push('[');
            for (row_index, row) in values.iter().enumerate() {
                if row_index > 0 {
                    output.push_str(", ");
                }
                if !one_dimensional {
                    output.push('[');
                }
                for (column_index, (value, dtype, _)) in row.iter().enumerate() {
                    if column_index > 0 {
                        output.push_str(", ");
                    }
                    python_value(&mut output, value.as_deref(), dtype);
                }
                if !one_dimensional {
                    output.push(']');
                }
            }
            output.push(']');
        }
    }
    Ok(output)
}

fn tsv_value(value: Option<&str>, dtype: &str, formatted: bool) -> String {
    match value {
        None if formatted => String::new(),
        None => "None".into(),
        Some(value) if formatted => {
            display_cell(Some(value), dtype, default_numeric_format(dtype), 1_000)
        }
        Some(value) => value.to_owned(),
    }
}

fn csv_field(output: &mut String, field: &str) {
    if field.contains([',', '"', '\n', '\r']) {
        output.push('"');
        output.push_str(&field.replace('"', "\"\""));
        output.push('"');
    } else {
        output.push_str(field);
    }
}

fn json_value(value: Option<&str>, dtype: &str) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    let kind = dtype.to_ascii_uppercase();
    if kind.starts_with("BOOLEAN") {
        return value
            .parse::<bool>()
            .map(Value::Bool)
            .unwrap_or_else(|_| Value::String(value.to_owned()));
    }
    if [
        "TINYINT",
        "SMALLINT",
        "INTEGER",
        "BIGINT",
        "HUGEINT",
        "UTINYINT",
        "USMALLINT",
        "UINTEGER",
        "UBIGINT",
        "UHUGEINT",
        "DECIMAL",
    ]
    .iter()
    .any(|prefix| kind.starts_with(prefix))
    {
        return value
            .parse::<Number>()
            .map(Value::Number)
            .unwrap_or_else(|_| Value::String(value.to_owned()));
    }
    if kind.starts_with("FLOAT") || kind.starts_with("DOUBLE") {
        return value
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    Value::String(value.to_owned())
}

fn python_value(output: &mut String, value: Option<&str>, dtype: &str) {
    let Some(value) = value else {
        output.push_str("None");
        return;
    };
    let kind = dtype.to_ascii_uppercase();
    if kind.starts_with("BOOLEAN") {
        match value.to_ascii_lowercase().as_str() {
            "true" => output.push_str("True"),
            "false" => output.push_str("False"),
            _ => output.push_str("None"),
        }
    } else if [
        "TINYINT",
        "SMALLINT",
        "INTEGER",
        "BIGINT",
        "HUGEINT",
        "UTINYINT",
        "USMALLINT",
        "UINTEGER",
        "UBIGINT",
        "UHUGEINT",
        "DECIMAL",
    ]
    .iter()
    .any(|prefix| kind.starts_with(prefix))
    {
        output.push_str(value);
    } else if kind.starts_with("FLOAT") || kind.starts_with("DOUBLE") {
        match value.parse::<f64>() {
            Ok(number) if number.is_finite() => output.push_str(value),
            _ => output.push_str("None"),
        }
    } else {
        output.push('\'');
        for ch in value.chars() {
            match ch {
                '\\' => output.push_str("\\\\"),
                '\'' => output.push_str("\\'"),
                '\n' => output.push_str("\\n"),
                '\r' => output.push_str("\\r"),
                '\t' => output.push_str("\\t"),
                ch => output.push(ch),
            }
        }
        output.push('\'');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::frame::FrameSnapshot;
    use crate::domain::selection::Cell;
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use duckdb::types::Value as DuckValue;

    fn result() -> ResultSnapshot {
        let store = DuckDbStore::from_rows(
            &[
                ("Name".into(), "VARCHAR".into()),
                ("Count".into(), "BIGINT".into()),
            ],
            [
                vec![DuckValue::Text("A".into()), DuckValue::BigInt(1)],
                vec![DuckValue::Null, DuckValue::BigInt(2)],
            ],
        )
        .unwrap();
        let source = FrameSnapshot::new(store, 1);
        ResultSnapshot::from_source(&source, 1, 1)
    }

    #[test]
    fn file_formats_follow_extensions_and_compression_suffixes() {
        use std::path::Path;
        let format = |name: &str| FileFormat::from_path(Path::new(name));
        assert_eq!(format("out.CSV"), Ok(FileFormat::Csv));
        assert_eq!(format("dir.v2/out.csv.gz"), Ok(FileFormat::Csv));
        assert_eq!(format("out.tsv.zst"), Ok(FileFormat::Tsv));
        assert_eq!(format("out.parquet"), Ok(FileFormat::Parquet));
        assert_eq!(format("out.jsonl"), Ok(FileFormat::NdJson));
        assert_eq!(format("out.json"), Ok(FileFormat::Json));
        assert_eq!(format("out.xlsx"), Ok(FileFormat::Xlsx));
        assert!(format("out.parquet.gz").is_err());
        assert!(format("out").is_err());
        assert!(format("out.txt").is_err());
    }

    #[test]
    fn file_export_writes_display_rows_and_columns_in_order() {
        let source = FrameSnapshot::new(
            DuckDbStore::from_rows(
                &[
                    ("region".into(), "VARCHAR".into()),
                    ("n".into(), "BIGINT".into()),
                ],
                [
                    vec![DuckValue::Text("North".into()), DuckValue::BigInt(10)],
                    vec![DuckValue::Text("South".into()), DuckValue::BigInt(20)],
                    vec![DuckValue::Text("East".into()), DuckValue::BigInt(30)],
                ],
            )
            .unwrap(),
            1,
        );
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_view_rows(&crate::infrastructure::duckdb_store::ViewQuery {
                hidden: Vec::new(),
                tokens: crate::domain::query::parse("-south").unwrap(),
                filters: Vec::new(),
                value_filters: Vec::new(),
                sort: vec![crate::domain::documents::SortKey {
                    column: "n".into(),
                    descending: true,
                }],
            })
            .unwrap();
        let schema = source.schema.to_vec();
        // Shown as n, region: the display order, not the store's.
        let shown = vec![schema[1].clone(), schema[0].clone()];
        let result = ResultSnapshot::from_view(&source, rows, shown, 1, 1);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.csv");
        assert_eq!(result.export_to_file(&path, FileFormat::Csv), Ok(2));
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n"),
            "n,region\n30,East\n10,North\n"
        );
        // Parquet keeps types; read it back through DuckDB.
        let parquet = directory.path().join("out.parquet");
        assert_eq!(result.export_to_file(&parquet, FileFormat::Parquet), Ok(2));
        let connection = duckdb::Connection::open_in_memory().unwrap();
        let (first, kind): (i64, String) = connection
            .query_row(
                "SELECT n, typeof(n) FROM read_parquet(?) LIMIT 1",
                [parquet.to_string_lossy().as_ref()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((first, kind.as_str()), (30, "BIGINT"));
        let json = directory.path().join("out.json");
        assert_eq!(result.export_to_file(&json, FileFormat::Json), Ok(2));
        let records: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
        assert_eq!(records[0]["region"], "East");
    }

    /// Needs the network the first time, when DuckDB installs `excel`.
    #[test]
    #[ignore]
    fn xlsx_export_installs_and_loads_the_excel_extension() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("out.xlsx");
        assert_eq!(result().export_to_file(&path, FileFormat::Xlsx), Ok(2));
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    fn selection(rows: usize, columns: usize) -> Selection {
        Selection {
            anchor: Cell { row: 0, column: 0 },
            active: Cell {
                row: rows - 1,
                column: columns - 1,
            },
        }
    }

    #[test]
    fn tsv_nulls_and_trailing_newlines_follow_selection_shape() {
        let result = result();
        assert_eq!(
            export(&result, selection(1, 1), CopyFormat::RawTsv).unwrap(),
            "A"
        );
        assert_eq!(
            export(&result, selection(2, 1), CopyFormat::RawTsv).unwrap(),
            "A\nNone\n"
        );
        assert_eq!(
            export(&result, selection(2, 1), CopyFormat::FormattedTsv).unwrap(),
            "A\n\n"
        );
    }

    #[test]
    fn csv_json_and_python_preserve_supported_types() {
        let result = result();
        assert_eq!(
            export(&result, selection(1, 1), CopyFormat::RawCsv).unwrap(),
            "A"
        );
        assert_eq!(
            export(&result, selection(2, 2), CopyFormat::JsonRecords).unwrap(),
            "[{\"Name\":\"A\",\"Count\":1},{\"Name\":null,\"Count\":2}]"
        );
        assert_eq!(
            export(&result, selection(1, 2), CopyFormat::PythonList).unwrap(),
            "['A', 1]"
        );
    }
}
