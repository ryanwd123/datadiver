//! Writes a result frame to a file in the format its extension names.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use polars::prelude::*;

use super::expressions::{is_nested, polars_error, render_value, rendered};
use crate::domain::export::FileFormat;

/// Excel's row limit, including the header row.
const XLSX_ROWS: usize = 1_048_576;

pub fn write(frame: &mut DataFrame, path: &Path, format: FileFormat) -> Result<(), String> {
    match format {
        FileFormat::Parquet => {
            let file = File::create(path).map_err(|error| error.to_string())?;
            ParquetWriter::new(file)
                .finish(frame)
                .map(|_| ())
                .map_err(polars_error)
        }
        FileFormat::Xlsx => write_xlsx(frame, path),
        FileFormat::Csv | FileFormat::Tsv | FileFormat::Json | FileFormat::NdJson => {
            let file = File::create(path).map_err(|error| error.to_string())?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.ends_with(".gz") {
                let mut encoder =
                    flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::default());
                write_text(frame, &mut encoder, format)?;
                encoder.finish().map_err(|error| error.to_string())?;
            } else if name.ends_with(".zst") {
                let mut encoder = zstd::Encoder::new(BufWriter::new(file), 0)
                    .map_err(|error| error.to_string())?;
                write_text(frame, &mut encoder, format)?;
                encoder.finish().map_err(|error| error.to_string())?;
            } else {
                let mut writer = BufWriter::new(file);
                write_text(frame, &mut writer, format)?;
                writer.flush().map_err(|error| error.to_string())?;
            }
            Ok(())
        }
    }
}

fn write_text(frame: &mut DataFrame, writer: &mut impl Write, format: FileFormat) -> Result<(), String> {
    match format {
        FileFormat::Csv | FileFormat::Tsv => {
            let mut flat = flattened(frame)?;
            CsvWriter::new(writer)
                .include_header(true)
                .with_separator(if format == FileFormat::Tsv { b'\t' } else { b',' })
                .finish(&mut flat)
                .map_err(polars_error)
        }
        FileFormat::Json => JsonWriter::new(writer)
            .with_json_format(JsonFormat::Json)
            .finish(frame)
            .map_err(polars_error),
        FileFormat::NdJson => JsonWriter::new(writer)
            .with_json_format(JsonFormat::JsonLines)
            .finish(frame)
            .map_err(polars_error),
        FileFormat::Parquet | FileFormat::Xlsx => unreachable!("binary formats are not text"),
    }
}

/// `frame` with nested and duration columns as text, which CSV cannot hold otherwise.
fn flattened(frame: &DataFrame) -> Result<DataFrame, String> {
    let schema = frame.schema();
    if !schema
        .iter()
        .any(|(_, dtype)| is_nested(dtype) || matches!(dtype, DataType::Duration(_) | DataType::Binary))
    {
        return Ok(frame.clone());
    }
    frame
        .clone()
        .lazy()
        .select(
            schema
                .iter()
                .map(|(name, dtype)| {
                    if is_nested(dtype) || matches!(dtype, DataType::Duration(_) | DataType::Binary) {
                        rendered(col(name.clone()), dtype)
                    } else {
                        col(name.clone())
                    }
                })
                .collect::<Vec<_>>(),
        )
        .collect()
        .map_err(polars_error)
}

fn write_xlsx(frame: &DataFrame, path: &Path) -> Result<(), String> {
    use rust_xlsxwriter::{Format, Workbook};
    if frame.height() >= XLSX_ROWS {
        return Err(format!(
            "Excel sheets hold at most {} rows; this result has {}. Export to CSV or Parquet instead",
            XLSX_ROWS - 1,
            frame.height()
        ));
    }
    let error = |error: rust_xlsxwriter::XlsxError| error.to_string();
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let date = Format::new().set_num_format("yyyy-mm-dd");
    let datetime = Format::new().set_num_format("yyyy-mm-dd hh:mm:ss");
    let epoch = chrono::NaiveDate::from_ymd_opt(1899, 12, 30).expect("Excel epoch");
    for (column_index, column) in frame.columns().iter().enumerate() {
        let column_index = column_index as u16;
        sheet
            .write_string(0, column_index, column.name().as_str())
            .map_err(error)?;
        for row in 0..frame.height() {
            let value = column.get(row).map_err(polars_error)?;
            let cell = row as u32 + 1;
            match &value {
                AnyValue::Null => {}
                AnyValue::Boolean(value) => {
                    sheet.write_boolean(cell, column_index, *value).map_err(error)?;
                }
                // Categorical values have no dtype of their own to ask about below.
                AnyValue::Categorical(..) | AnyValue::CategoricalOwned(..) | AnyValue::Enum(..) | AnyValue::EnumOwned(..) => {
                    sheet.write_string(cell, column_index, value.str_value().as_ref()).map_err(error)?;
                }
                AnyValue::Date(days) => {
                    let serial = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                        .expect("unix epoch")
                        .signed_duration_since(epoch)
                        .num_days()
                        + i64::from(*days);
                    sheet
                        .write_number_with_format(cell, column_index, serial as f64, &date)
                        .map_err(error)?;
                }
                AnyValue::Datetime(value, unit, _) | AnyValue::DatetimeOwned(value, unit, _) => {
                    let seconds = match unit {
                        TimeUnit::Nanoseconds => *value as f64 / 1e9,
                        TimeUnit::Microseconds => *value as f64 / 1e6,
                        TimeUnit::Milliseconds => *value as f64 / 1e3,
                    };
                    let unix_serial = 25_569.0;
                    sheet
                        .write_number_with_format(
                            cell,
                            column_index,
                            unix_serial + seconds / 86_400.0,
                            &datetime,
                        )
                        .map_err(error)?;
                }
                value if value.dtype().is_primitive_numeric() || matches!(value, AnyValue::Decimal(..)) => {
                    let number = value
                        .extract::<f64>()
                        .or_else(|| render_value(value).and_then(|text| text.parse().ok()))
                        .unwrap_or(f64::NAN);
                    if number.is_finite() {
                        sheet.write_number(cell, column_index, number).map_err(error)?;
                    } else if let Some(text) = render_value(value) {
                        sheet.write_string(cell, column_index, text).map_err(error)?;
                    }
                }
                value => {
                    if let Some(text) = render_value(value) {
                        sheet.write_string(cell, column_index, text).map_err(error)?;
                    }
                }
            }
        }
    }
    workbook.save(path).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_formats_compress_and_csv_flattens_lists() {
        let directory = tempfile::tempdir().unwrap();
        let mut frame = df!("a" => [1i64, 2], "b" => ["x", "y"]).unwrap();
        frame
            .with_column(
                Series::new(
                    "l".into(),
                    [Series::new("".into(), [1i32, 2]), Series::new("".into(), [3i32])],
                )
                .into_column(),
            )
            .unwrap();
        let csv = directory.path().join("out.csv");
        write(&mut frame, &csv, FileFormat::Csv).unwrap();
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap().replace("\r\n", "\n"),
            "a,b,l\n1,x,\"[1, 2]\"\n2,y,[3]\n"
        );
        let gz = directory.path().join("out.ndjson.gz");
        write(&mut frame, &gz, FileFormat::NdJson).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(
            &mut flate2::read::GzDecoder::new(File::open(&gz).unwrap()),
            &mut text,
        )
        .unwrap();
        assert!(text.starts_with("{\"a\":1,\"b\":\"x\",\"l\":[1,2]}"), "{text}");
        let xlsx = directory.path().join("out.xlsx");
        write(&mut frame, &xlsx, FileFormat::Xlsx).unwrap();
        let read = super::super::readers::read_frame(&xlsx).unwrap();
        assert_eq!(read.height(), 2);
        assert_eq!(read.column("b").unwrap().str().unwrap().get(1), Some("y"));
    }

    #[test]
    fn categorical_text_writes_like_plain_text() {
        let directory = tempfile::tempdir().unwrap();
        let plain = df!("a" => [1i64, 2, 3], "b" => [Some("x"), None, Some("x")]).unwrap();
        let categorical = super::super::readers::with_categorical_text(plain.clone()).unwrap();
        assert!(matches!(categorical.column("b").unwrap().dtype(), DataType::Categorical(..)));
        for (name, format) in [("out.csv", FileFormat::Csv), ("out.parquet", FileFormat::Parquet), ("out.xlsx", FileFormat::Xlsx)] {
            let written = |frame: &DataFrame, folder: &str| {
                let path = directory.path().join(folder);
                std::fs::create_dir_all(&path).unwrap();
                let path = path.join(name);
                write(&mut frame.clone(), &path, format).unwrap();
                let read = super::super::readers::read_frame(&path).unwrap();
                read.column("b").unwrap().cast(&DataType::String).unwrap().str().unwrap().iter().map(|v| v.map(str::to_owned)).collect::<Vec<_>>()
            };
            assert_eq!(written(&plain, "plain"), written(&categorical, "categorical"), "{name}");
        }
    }
}
