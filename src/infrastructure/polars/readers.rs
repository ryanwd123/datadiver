//! Reads source files into memory with Polars.

use std::path::Path;

use polars::prelude::*;

use super::expressions::polars_error;

/// Rows sampled to infer CSV column types. A later value that does not fit
/// fails the read instead of being dropped, and an in-memory read then
/// retries with every row.
const INFER_ROWS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Delimited(u8),
    Parquet,
    Xlsx,
}

pub fn file_kind(path: &Path) -> Result<FileKind, String> {
    if is_gzip_csv(path) {
        return Ok(FileKind::Delimited(b','));
    }
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "parquet" | "pq" => Ok(FileKind::Parquet),
        "csv" => Ok(FileKind::Delimited(b',')),
        "tsv" => Ok(FileKind::Delimited(b'\t')),
        "xlsx" => Ok(FileKind::Xlsx),
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

/// Reads every row of `path` into memory.
/// `frame` with its text columns as categoricals, which hold each distinct
/// value once: less memory, and faster grouping, when values repeat.
pub fn with_categorical_text(frame: DataFrame) -> Result<DataFrame, String> {
    let text: Vec<PlSmallStr> = frame
        .columns()
        .iter()
        .filter(|column| column.dtype() == &DataType::String)
        .map(|column| column.name().clone())
        .collect();
    if text.is_empty() {
        return Ok(frame);
    }
    let categorical = DataType::from_categories(Categories::global());
    frame
        .lazy()
        .with_columns(text.into_iter().map(|name| col(name).cast(categorical.clone())).collect::<Vec<_>>())
        .collect()
        .map_err(polars_error)
}

pub fn read_frame(path: &Path) -> Result<DataFrame, String> {
    read_frame_limited(path, None).map(|(frame, _)| frame)
}

/// Reads the rows of `path` into memory, at most `limit` of them, and says
/// whether the file had more.
pub fn read_frame_limited(path: &Path, limit: Option<usize>) -> Result<(DataFrame, bool), String> {
    // One row past the limit shows whether the file goes on.
    let rows = limit.map(|limit| limit.saturating_add(1));
    let read = || -> Result<DataFrame, String> {
        match file_kind(path)? {
            FileKind::Delimited(separator) => read_delimited(path, separator, Some(INFER_ROWS), rows)
                .or_else(|_| read_delimited(path, separator, None, rows)),
            FileKind::Parquet => {
                let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
                ParquetReader::new(file)
                    .with_slice(rows.map(|rows| (0, rows)))
                    .finish()
                    .map_err(polars_error)
            }
            FileKind::Xlsx => read_xlsx(path, rows),
        }
    };
    let frame = read().map_err(|error| format!("could not read {}: {error}", path.display()))?;
    match limit {
        Some(limit) if frame.height() > limit => Ok((frame.head(Some(limit)), true)),
        _ => Ok((frame, false)),
    }
}

fn read_delimited(path: &Path, separator: u8, infer: Option<usize>, rows: Option<usize>) -> Result<DataFrame, String> {
    CsvReadOptions::default()
        .with_has_header(true)
        .with_n_rows(rows)
        .with_infer_schema_length(infer)
        .map_parse_options(|options| {
            options
                .with_separator(separator)
                .with_try_parse_dates(true)
        })
        .try_into_reader_with_file_path(Some(path.to_path_buf()))
        .and_then(|reader| reader.finish())
        .map_err(polars_error)
}

/// The first worksheet, with its first row as the header. A column whose
/// cells are all numbers, booleans, or dates keeps that type; any other
/// column is text.
fn read_xlsx(path: &Path, limit: Option<usize>) -> Result<DataFrame, String> {
    use calamine::{Data, Reader, open_workbook_auto};
    let mut workbook = open_workbook_auto(path).map_err(|error| error.to_string())?;
    let sheet = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or("the workbook has no worksheets")?;
    let range = workbook
        .worksheet_range(&sheet)
        .map_err(|error| error.to_string())?;
    let mut rows = range.rows();
    let header = rows.next().map(<[Data]>::to_vec).unwrap_or_default();
    let body = rows.take(limit.unwrap_or(usize::MAX)).collect::<Vec<_>>();
    let mut names: Vec<String> = Vec::new();
    for (index, cell) in header.iter().enumerate() {
        let base = match cell {
            Data::Empty => format!("column{}", index + 1),
            cell => cell.to_string(),
        };
        let mut name = base.clone();
        let mut suffix = 1;
        while names.iter().any(|taken| taken.eq_ignore_ascii_case(&name)) {
            name = format!("{base}_{suffix}");
            suffix += 1;
        }
        names.push(name);
    }
    let mut columns = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let cells = body
            .iter()
            .map(|row| row.get(index).unwrap_or(&Data::Empty))
            .collect::<Vec<_>>();
        columns.push(xlsx_column(name, &cells)?.into_column());
    }
    DataFrame::new(body.len(), columns).map_err(polars_error)
}

fn xlsx_column(name: &str, cells: &[&calamine::Data]) -> Result<Series, String> {
    use calamine::{Data, DataType as _};
    let present = cells
        .iter()
        .filter(|cell| !matches!(cell, Data::Empty))
        .collect::<Vec<_>>();
    let name = PlSmallStr::from(name);
    if !present.is_empty() && present.iter().all(|cell| matches!(cell, Data::Int(_))) {
        let values = cells
            .iter()
            .map(|cell| match cell {
                Data::Int(value) => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        return Ok(Series::new(name, values));
    }
    if !present.is_empty()
        && present
            .iter()
            .all(|cell| matches!(cell, Data::Int(_) | Data::Float(_)))
    {
        let values = cells
            .iter()
            .map(|cell| match cell {
                Data::Int(value) => Some(*value as f64),
                Data::Float(value) => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        // Whole numbers stored as floats, as Excel usually does, read as integers.
        if values
            .iter()
            .flatten()
            .all(|value| value.fract() == 0.0 && value.abs() < 9e15)
        {
            return Ok(Series::new(
                name,
                values
                    .into_iter()
                    .map(|value| value.map(|value| value as i64))
                    .collect::<Vec<_>>(),
            ));
        }
        return Ok(Series::new(name, values));
    }
    if !present.is_empty() && present.iter().all(|cell| matches!(cell, Data::Bool(_))) {
        let values = cells
            .iter()
            .map(|cell| match cell {
                Data::Bool(value) => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        return Ok(Series::new(name, values));
    }
    if !present.is_empty()
        && present
            .iter()
            .all(|cell| matches!(cell, Data::DateTime(_) | Data::DateTimeIso(_)))
    {
        let values = cells
            .iter()
            .map(|cell| cell.as_datetime())
            .collect::<Vec<_>>();
        let dates_only = values
            .iter()
            .flatten()
            .all(|moment| moment.time() == chrono::NaiveTime::MIN);
        if dates_only {
            let values = values
                .into_iter()
                .map(|moment| moment.map(|moment| moment.date()))
                .collect::<Vec<_>>();
            return Ok(Series::new(name, values));
        }
        return Ok(Series::new(name, values));
    }
    let values = cells
        .iter()
        .map(|cell| match cell {
            Data::Empty => None,
            Data::DateTime(_) => cell.as_datetime().map(|moment| moment.to_string()),
            cell => Some(cell.to_string()),
        })
        .collect::<Vec<_>>();
    Ok(Series::new(name, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_gzip(path: &Path, text: &str) {
        let mut encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(path).unwrap(),
            flate2::Compression::default(),
        );
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap();
    }

    #[test]
    fn gzip_csv_reads_in_memory() {
        let home = tempfile::tempdir().unwrap();
        for name in ["report.csv.gz", "REPORT.CSV.GZ"] {
            let path = home.path().join(name);
            write_gzip(&path, "region,sales
\"North, East\",10
South,20
");
            assert!(std::fs::read(&path).unwrap().starts_with(&[0x1f, 0x8b]));
            let store = super::super::store::PolarsStore::open_source(&path).unwrap();
            assert_eq!(store.row_count(), 2);
            let rows = store.fetch_row_ids(&[0, 1]).unwrap();
            assert_eq!(rows[0].values, [Some("North, East".into()), Some("10".into())]);
            assert_eq!(rows[1].values, [Some("South".into()), Some("20".into())]);
            // Remove the first case variant before opening the other on a case-insensitive filesystem.
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn late_values_that_break_the_sampled_type_are_read_as_text() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("late.csv");
        let mut text = String::from("code\n");
        for index in 0..INFER_ROWS + 5 {
            text.push_str(&format!("{index}\n"));
        }
        text.push_str("A-7\n");
        std::fs::write(&path, text).unwrap();
        let frame = read_frame(&path).unwrap();
        assert_eq!(frame.height(), INFER_ROWS + 6);
        assert_eq!(frame.column("code").unwrap().dtype(), &DataType::String);
    }

    #[test]
    fn unsupported_spreadsheet_formats_are_rejected() {
        for extension in ["xls", "xlsb", "ods"] {
            let path = Path::new("report").with_extension(extension);
            let error = match file_kind(&path) {
                Ok(_) => panic!("{extension} should not be supported"),
                Err(error) => error,
            };
            assert!(error.contains("reads .xlsx files"));
        }
    }
}
