//! Polars expressions for what the user types: SQL snippets, rendered values,
//! and typed literals for slicer selections.

use polars::prelude::*;

use crate::domain::documents::ScalarKey;

/// A Polars error as one message, without the query plan Polars appends.
pub fn polars_error(error: PolarsError) -> String {
    let text = error.to_string();
    let text = text
        .split("\n\nResolved plan until failure")
        .next()
        .unwrap_or_default();
    text.trim().to_owned()
}

pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// The SQL name of a Polars type, as the grid, Info, and type checks show it.
pub fn sql_type_name(dtype: &DataType) -> String {
    match dtype {
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int8 => "TINYINT".into(),
        DataType::Int16 => "SMALLINT".into(),
        DataType::Int32 => "INTEGER".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::Int128 => "HUGEINT".into(),
        DataType::UInt8 => "UTINYINT".into(),
        DataType::UInt16 => "USMALLINT".into(),
        DataType::UInt32 => "UINTEGER".into(),
        DataType::UInt64 => "UBIGINT".into(),
        DataType::UInt128 => "UHUGEINT".into(),
        DataType::Float16 | DataType::Float32 => "FLOAT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Decimal(precision, scale) => format!("DECIMAL({precision},{scale})"),
        DataType::String => "VARCHAR".into(),
        DataType::Categorical(..) | DataType::Enum(..) => "ENUM".into(),
        DataType::Binary | DataType::BinaryOffset => "BLOB".into(),
        DataType::Date => "DATE".into(),
        DataType::Datetime(unit, timezone) => match (unit, timezone) {
            (_, Some(_)) => "TIMESTAMP WITH TIME ZONE".into(),
            (TimeUnit::Milliseconds, None) => "TIMESTAMP_MS".into(),
            (TimeUnit::Microseconds, None) => "TIMESTAMP".into(),
            (TimeUnit::Nanoseconds, None) => "TIMESTAMP_NS".into(),
        },
        DataType::Duration(_) => "INTERVAL".into(),
        DataType::Time => "TIME".into(),
        DataType::List(inner) => format!("{}[]", sql_type_name(inner)),
        DataType::Array(inner, size) => format!("{}[{size}]", sql_type_name(inner)),
        DataType::Struct(fields) => format!(
            "STRUCT({})",
            fields
                .iter()
                .map(|field| format!(
                    "{} {}",
                    quote_identifier(field.name()),
                    sql_type_name(field.dtype())
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        DataType::Null => "NULL".into(),
        other => other.to_string().to_uppercase(),
    }
}

/// The Polars type for an SQL type name, for building tables in code.
pub fn data_type_from_sql(name: &str) -> Result<DataType, String> {
    let upper = name.trim().to_ascii_uppercase();
    if let Some(inner) = upper.strip_suffix("[]") {
        return Ok(DataType::List(Box::new(data_type_from_sql(inner)?)));
    }
    if let Some(arguments) = upper
        .strip_prefix("DECIMAL(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let (precision, scale) = arguments.split_once(',').unwrap_or((arguments, "0"));
        let number = |text: &str| {
            text.trim()
                .parse::<usize>()
                .map_err(|_| format!("invalid type {name}"))
        };
        return Ok(DataType::Decimal(number(precision)?, number(scale)?));
    }
    Ok(match upper.as_str() {
        "BOOLEAN" | "BOOL" => DataType::Boolean,
        "TINYINT" => DataType::Int8,
        "SMALLINT" => DataType::Int16,
        "INTEGER" | "INT" => DataType::Int32,
        "BIGINT" => DataType::Int64,
        "HUGEINT" => DataType::Int128,
        "UTINYINT" => DataType::UInt8,
        "USMALLINT" => DataType::UInt16,
        "UINTEGER" => DataType::UInt32,
        "UBIGINT" => DataType::UInt64,
        "UHUGEINT" => DataType::UInt128,
        "FLOAT" | "REAL" => DataType::Float32,
        "DOUBLE" => DataType::Float64,
        "DECIMAL" => DataType::Decimal(38, 0),
        "VARCHAR" | "TEXT" | "STRING" => DataType::String,
        "ENUM" => DataType::from_categories(Categories::global()),
        "BLOB" => DataType::Binary,
        "DATE" => DataType::Date,
        "TIMESTAMP" => DataType::Datetime(TimeUnit::Microseconds, None),
        "TIMESTAMP_MS" => DataType::Datetime(TimeUnit::Milliseconds, None),
        "TIMESTAMP_NS" => DataType::Datetime(TimeUnit::Nanoseconds, None),
        "TIMESTAMPTZ" | "TIMESTAMP WITH TIME ZONE" => {
            DataType::Datetime(TimeUnit::Microseconds, Some(TimeZone::UTC))
        }
        "INTERVAL" => DataType::Duration(TimeUnit::Microseconds),
        "TIME" => DataType::Time,
        _ => return Err(format!("unsupported type {name}")),
    })
}

/// Lists, arrays, and structs.
pub fn is_nested(dtype: &DataType) -> bool {
    matches!(
        dtype,
        DataType::List(_) | DataType::Array(..) | DataType::Struct(_)
    )
}

/// `value` as the grid shows it. Types Polars cannot cast to text, such as
/// lists and durations, are rendered value by value like [`render_value`].
pub fn rendered(value: Expr, dtype: &DataType) -> Expr {
    match dtype {
        DataType::String => value,
        DataType::Datetime(_, None) => value.dt().to_string("%Y-%m-%d %H:%M:%S%.f"),
        DataType::Datetime(_, Some(_)) => value.dt().to_string("%Y-%m-%d %H:%M:%S%.f%:z"),
        DataType::Boolean
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::Int128
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::UInt128
        | DataType::Float16
        | DataType::Float32
        | DataType::Float64
        | DataType::Decimal(..)
        | DataType::Categorical(..)
        | DataType::Enum(..)
        | DataType::Date => value.cast(DataType::String),
        _ => value.map(
            |column| {
                let rendered = column
                    .as_materialized_series()
                    .iter()
                    .map(|value| render_value(&value))
                    .collect::<StringChunked>()
                    .with_name(column.name().clone());
                Ok(rendered.into_column())
            },
            |_, field| Ok(Field::new(field.name().clone(), DataType::String)),
        ),
    }
}

/// Text inside a list or struct, quoted as DuckDB quotes it: only when it
/// is empty, reads as null, has spaces at an end, or holds a character that
/// would end the item or the list.
fn nested_text(text: &str) -> String {
    let quote = text.is_empty()
        || text.eq_ignore_ascii_case("null")
        || text.starts_with(char::is_whitespace)
        || text.ends_with(char::is_whitespace)
        || text.contains([':', '=', '"', ',', '\'', '(', ')', '[', ']', '{', '}']);
    if quote { format!("'{}'", text.replace('\'', "\\'")) } else { text.to_owned() }
}

/// An item of a list or struct as text: text quoted where needed, nulls as `NULL`.
fn render_item(value: &AnyValue) -> String {
    match value {
        AnyValue::String(text) => nested_text(text),
        AnyValue::StringOwned(text) => nested_text(text),
        value => render_value(value).unwrap_or_else(|| "NULL".into()),
    }
}

/// One value as text; `None` is null. Matches [`rendered`] for every type.
pub fn render_value(value: &AnyValue) -> Option<String> {
    let nested = |values: Series| {
        let items = values.iter().map(|item| render_item(&item)).collect::<Vec<_>>();
        format!("[{}]", items.join(", "))
    };
    Some(match value {
        AnyValue::Null => return None,
        AnyValue::String(text) => (*text).to_owned(),
        AnyValue::StringOwned(text) => text.to_string(),
        AnyValue::List(values) => nested(values.clone()),
        AnyValue::Array(values, _) => nested(values.clone()),
        AnyValue::Struct(..) | AnyValue::StructOwned(..) => {
            let fields = match value {
                AnyValue::Struct(_, _, fields) => fields.to_vec(),
                AnyValue::StructOwned(payload) => payload.1.clone(),
                _ => unreachable!(),
            };
            let values = value._iter_struct_av().collect::<Vec<_>>();
            let entries = fields
                .iter()
                .zip(values)
                .map(|(field, value)| {
                    format!("'{}': {}", field.name().replace('\'', "\\'"), render_item(&value))
                })
                .collect::<Vec<_>>();
            format!("{{{}}}", entries.join(", "))
        }
        AnyValue::Datetime(..) | AnyValue::DatetimeOwned(..) => {
            let Some(datetime) = datetime_value(value) else {
                return Some(value.to_string());
            };
            datetime
        }
        other => other.str_value().into_owned(),
    })
}

fn datetime_value(value: &AnyValue) -> Option<String> {
    let (value, unit, timezone) = match value {
        AnyValue::Datetime(value, unit, timezone) => (*value, *unit, timezone.is_some()),
        AnyValue::DatetimeOwned(value, unit, timezone) => (*value, *unit, timezone.is_some()),
        _ => return None,
    };
    let datetime = match unit {
        TimeUnit::Nanoseconds => chrono::DateTime::from_timestamp_nanos(value),
        TimeUnit::Microseconds => chrono::DateTime::from_timestamp_micros(value)?,
        TimeUnit::Milliseconds => chrono::DateTime::from_timestamp_millis(value)?,
    };
    Some(if timezone {
        datetime.format("%Y-%m-%d %H:%M:%S%.f%:z").to_string()
    } else {
        datetime.naive_utc().format("%Y-%m-%d %H:%M:%S%.f").to_string()
    })
}

/// DuckDB-style shorthands Polars SQL lacks, rewritten before parsing:
/// `year(x)` and the other date parts become `date_part('year', x)`.
const DATE_PARTS: [&str; 15] = [
    "year",
    "quarter",
    "month",
    "week",
    "day",
    "dayofweek",
    "dayofyear",
    "hour",
    "minute",
    "second",
    "millisecond",
    "microsecond",
    "nanosecond",
    "isoyear",
    "epoch",
];

/// `expression` reading each categorical column of `schema` as text, since
/// Polars' SQL string functions such as `upper` take only text.
pub fn text_for_categoricals(expression: Expr, schema: &[crate::infrastructure::store::ColumnSchema]) -> Expr {
    let categorical: Vec<&str> =
        schema.iter().filter(|column| column.sql_type == "ENUM").map(|column| column.name.as_str()).collect();
    if categorical.is_empty() {
        return expression;
    }
    expression.map_expr(|node| match node {
        Expr::Column(name) if categorical.contains(&name.as_str()) => col(name).cast(DataType::String),
        node => node,
    })
}

/// One SQL expression, such as `year("date") AS year` or `sum(sales)`, as a
/// Polars expression. An `AS` alias names the result.
pub fn parse_sql_expression(sql: &str) -> Result<Expr, String> {
    let rewritten = rewrite_shorthands(sql)?;
    polars::sql::sql_expr(&rewritten).map_err(polars_error)
}

fn rewrite_shorthands(sql: &str) -> Result<String, String> {
    let spans = identifier_spans(sql);
    let mut output = String::with_capacity(sql.len());
    let mut copied = 0;
    let mut index = 0;
    while index < spans.len() {
        let (start, end) = spans[index];
        let word = sql[start..end].to_ascii_lowercase();
        let after = sql[end..].trim_start();
        if !after.starts_with('(') {
            index += 1;
            continue;
        }
        let open = end + (sql[end..].len() - after.len());
        if DATE_PARTS.contains(&word.as_str()) {
            output.push_str(&sql[copied..start]);
            output.push_str(&format!("date_part('{word}', "));
            copied = open + 1;
        } else if word == "date_trunc" {
            let close = matching_paren(sql, open).ok_or("Unclosed bracket in SQL expression")?;
            let arguments = &sql[open + 1..close];
            let (unit, value) = arguments
                .split_once(',')
                .ok_or("date_trunc takes a unit and a value")?;
            let unit = unit
                .trim()
                .trim_matches('\'')
                .to_ascii_lowercase();
            let value = rewrite_shorthands(value.trim())?;
            let format = match unit.as_str() {
                "year" => "%Y-01-01 00:00:00",
                "month" => "%Y-%m-01 00:00:00",
                "day" => "%Y-%m-%d 00:00:00",
                "hour" => "%Y-%m-%d %H:00:00",
                "minute" => "%Y-%m-%d %H:%M:00",
                "second" => "%Y-%m-%d %H:%M:%S",
                _ => {
                    return Err(format!(
                        "date_trunc supports year, month, day, hour, minute, and second, not {unit}"
                    ));
                }
            };
            output.push_str(&sql[copied..start]);
            output.push_str(&format!(
                "strptime(strftime({value}, '{format}'), '%Y-%m-%d %H:%M:%S')"
            ));
            copied = close + 1;
            while index < spans.len() && spans[index].0 < copied {
                index += 1;
            }
            continue;
        }
        index += 1;
    }
    output.push_str(&sql[copied..]);
    Ok(output)
}

/// Byte ranges of the bare identifiers in `sql`, outside strings, quoted
/// identifiers, and comments.
fn identifier_spans(sql: &str) -> Vec<(usize, usize)> {
    let bytes = sql.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                    i += 1;
                }
                i += 2;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' || byte >= 128 => {
                let start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 128)
                {
                    i += 1;
                }
                spans.push((start, i));
            }
            _ => i += 1,
        }
    }
    spans
}

fn matching_paren(sql: &str, open: usize) -> Option<usize> {
    let bytes = sql.as_bytes();
    let mut depth = 0;
    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(open) {
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            },
        }
    }
    None
}

/// The header for an expression without an alias: its text without comments,
/// runs of whitespace, or identifier quotes, so `sum("Score")` is `sum(Score)`.
pub fn display_name(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut output = String::new();
    let mut i = 0;
    let mut space = false;
    let push = |output: &mut String, text: &str, space: &mut bool| {
        if *space && !output.is_empty() {
            output.push(' ');
        }
        *space = false;
        output.push_str(text);
    };
    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i = (i + 1).min(bytes.len());
                push(&mut output, &sql[start..i], &mut space);
            }
            b'"' => {
                let mut name = String::new();
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'"' {
                        if bytes.get(i + 1) == Some(&b'"') {
                            name.push('"');
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    let length = sql[i..].chars().next().map_or(1, char::len_utf8);
                    name.push_str(&sql[i..i + length]);
                    i += length;
                }
                i = (i + 1).min(bytes.len());
                push(&mut output, &name, &mut space);
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                space = true;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                space = true;
            }
            byte if byte.is_ascii_whitespace() => {
                space = true;
                i += 1;
            }
            _ => {
                let length = sql[i..].chars().next().map_or(1, char::len_utf8);
                push(&mut output, &sql[i..i + length], &mut space);
                i += length;
            }
        }
    }
    // DuckDB's name for it, so a view's columns are the same in both engines.
    if output.eq_ignore_ascii_case("count(*)") {
        return "count_star()".into();
    }
    output
}

/// A typed literal for a slicer selection or a condition value, cast to `dtype`.
pub fn scalar_series(keys: &[&ScalarKey], dtype: &DataType) -> Result<Series, String> {
    let mut parts = Vec::with_capacity(keys.len());
    for key in keys {
        parts.push(scalar_value(key)?);
    }
    let natural = parts
        .iter()
        .map(|(_, natural)| natural.clone())
        .find(|natural| *natural != DataType::Null)
        .unwrap_or(DataType::Null);
    let values = parts.into_iter().map(|(value, _)| value).collect::<Vec<_>>();
    let series = Series::from_any_values_and_dtype("".into(), &values, &natural, false)
        .map_err(polars_error)?;
    let cast = series.strict_cast(dtype).map_err(|_| {
        format!(
            "{} cannot be compared with a {} column",
            values_label(keys),
            sql_type_name(dtype)
        )
    })?;
    if !matches!(dtype, DataType::Decimal(..)) {
        return Ok(cast);
    }
    // A decimal with more digits than the column holds would round into a
    // different value, so it matches no row and is left out.
    let exact = keys
        .iter()
        .zip(cast.iter())
        .map(|(key, value)| match key {
            ScalarKey::Decimal(text) | ScalarKey::String(text) => {
                trimmed_decimal(text) == trimmed_decimal(&value.str_value())
            }
            _ => true,
        })
        .collect::<BooleanChunked>();
    cast.filter(&exact).map_err(polars_error)
}

fn trimmed_decimal(text: &str) -> String {
    let text = text.trim();
    match text.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_owned()
            } else {
                format!("{whole}.{fraction}")
            }
        }
        None => text.to_owned(),
    }
}

fn values_label(keys: &[&ScalarKey]) -> String {
    keys.iter()
        .map(|key| crate::domain::slicers::label(key))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A key as a value in its own type, and that type.
fn scalar_value(key: &ScalarKey) -> Result<(AnyValue<'static>, DataType), String> {
    let invalid = |kind: &str| format!("invalid {kind} filter value");
    Ok(match key {
        ScalarKey::Null => (AnyValue::Null, DataType::Null),
        ScalarKey::Boolean(value) => (AnyValue::Boolean(*value), DataType::Boolean),
        ScalarKey::String(value) | ScalarKey::Decimal(value) => (
            AnyValue::StringOwned(value.as_str().into()),
            DataType::String,
        ),
        ScalarKey::Signed(value) => {
            let value = value.parse::<i128>().map_err(|_| invalid("signed"))?;
            match i64::try_from(value) {
                Ok(value) => (AnyValue::Int64(value), DataType::Int64),
                Err(_) => (AnyValue::Int128(value), DataType::Int128),
            }
        }
        ScalarKey::Unsigned(value) => {
            let value = value.parse::<u128>().map_err(|_| invalid("unsigned"))?;
            match u64::try_from(value) {
                Ok(value) => (AnyValue::UInt64(value), DataType::UInt64),
                Err(_) => (AnyValue::UInt128(value), DataType::UInt128),
            }
        }
        ScalarKey::FloatBits(bits) => {
            if let Some(bits) = bits.strip_prefix("f32:") {
                let bits = bits.parse::<u32>().map_err(|_| invalid("float"))?;
                (AnyValue::Float32(f32::from_bits(bits)), DataType::Float32)
            } else if let Some(bits) = bits.strip_prefix("f64:") {
                let bits = bits.parse::<u64>().map_err(|_| invalid("float"))?;
                (AnyValue::Float64(f64::from_bits(bits)), DataType::Float64)
            } else {
                return Err(invalid("float"));
            }
        }
        ScalarKey::Date(days) => (
            AnyValue::Date(days.parse().map_err(|_| invalid("date"))?),
            DataType::Date,
        ),
        ScalarKey::Datetime {
            value,
            unit,
            timezone,
        } => {
            let unit = time_unit(unit);
            let value = value.parse().map_err(|_| invalid("datetime"))?;
            let timezone = timezone
                .as_deref()
                .map(|zone| TimeZone::opt_try_new(Some(zone)))
                .transpose()
                .map_err(polars_error)?
                .flatten();
            (
                AnyValue::DatetimeOwned(value, unit, timezone.clone().map(Arc::new)),
                DataType::Datetime(unit, timezone),
            )
        }
        ScalarKey::Time(nanos) => (
            AnyValue::Time(nanos.parse().map_err(|_| invalid("time"))?),
            DataType::Time,
        ),
        ScalarKey::Duration { value, unit } => {
            let unit = time_unit(unit);
            (
                AnyValue::Duration(value.parse().map_err(|_| invalid("duration"))?, unit),
                DataType::Duration(unit),
            )
        }
    })
}

pub fn time_unit(name: &str) -> TimeUnit {
    match name {
        "Nanoseconds" => TimeUnit::Nanoseconds,
        "Milliseconds" => TimeUnit::Milliseconds,
        _ => TimeUnit::Microseconds,
    }
}

pub fn time_unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Nanoseconds => "Nanoseconds",
        TimeUnit::Microseconds => "Microseconds",
        TimeUnit::Milliseconds => "Milliseconds",
    }
}

/// A slicer value's key, exact for its column type `dtype`.
pub fn scalar_key(value: &AnyValue, dtype: &DataType) -> Result<ScalarKey, String> {
    Ok(match value {
        AnyValue::Null => ScalarKey::Null,
        AnyValue::Boolean(value) => ScalarKey::Boolean(*value),
        AnyValue::String(value) => ScalarKey::String((*value).to_owned()),
        AnyValue::StringOwned(value) => ScalarKey::String(value.to_string()),
        AnyValue::Categorical(..) | AnyValue::CategoricalOwned(..) | AnyValue::Enum(..) | AnyValue::EnumOwned(..) => {
            ScalarKey::String(value.str_value().into_owned())
        }
        AnyValue::Int8(value) => ScalarKey::Signed(value.to_string()),
        AnyValue::Int16(value) => ScalarKey::Signed(value.to_string()),
        AnyValue::Int32(value) => ScalarKey::Signed(value.to_string()),
        AnyValue::Int64(value) => ScalarKey::Signed(value.to_string()),
        AnyValue::Int128(value) => ScalarKey::Signed(value.to_string()),
        AnyValue::UInt8(value) => ScalarKey::Unsigned(value.to_string()),
        AnyValue::UInt16(value) => ScalarKey::Unsigned(value.to_string()),
        AnyValue::UInt32(value) => ScalarKey::Unsigned(value.to_string()),
        AnyValue::UInt64(value) => ScalarKey::Unsigned(value.to_string()),
        AnyValue::UInt128(value) => ScalarKey::Unsigned(value.to_string()),
        AnyValue::Float32(value) => ScalarKey::FloatBits(format!("f32:{}", value.to_bits())),
        AnyValue::Float64(value) => ScalarKey::FloatBits(format!("f64:{}", value.to_bits())),
        AnyValue::Decimal(..) => ScalarKey::Decimal(value.str_value().into_owned()),
        AnyValue::Date(days) => ScalarKey::Date(days.to_string()),
        AnyValue::Datetime(value, unit, timezone) => ScalarKey::Datetime {
            value: value.to_string(),
            unit: time_unit_name(*unit).into(),
            timezone: timezone.map(|zone| zone.to_string()),
        },
        AnyValue::DatetimeOwned(value, unit, timezone) => ScalarKey::Datetime {
            value: value.to_string(),
            unit: time_unit_name(*unit).into(),
            timezone: timezone.as_ref().map(|zone| zone.to_string()),
        },
        AnyValue::Time(nanos) => ScalarKey::Time(nanos.to_string()),
        AnyValue::Duration(value, unit) => ScalarKey::Duration {
            value: value.to_string(),
            unit: time_unit_name(*unit).into(),
        },
        _ => return Err(format!("unsupported slicer type: {}", sql_type_name(dtype))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_part_shorthands_and_truncation_rewrite_outside_strings() {
        assert_eq!(
            rewrite_shorthands("year(\"date\") AS year").unwrap(),
            "date_part('year', \"date\") AS year"
        );
        assert_eq!(
            rewrite_shorthands("'year(' || \"year\"(x)").unwrap(),
            "'year(' || \"year\"(x)"
        );
        assert_eq!(
            rewrite_shorthands("date_trunc('month', day) AS m").unwrap(),
            "strptime(strftime(day, '%Y-%m-01 00:00:00'), '%Y-%m-%d %H:%M:%S') AS m"
        );
        assert!(rewrite_shorthands("date_trunc('week', day)").is_err());
        let frame = df!("day" => [chrono::NaiveDate::from_ymd_opt(2024, 3, 9).unwrap()]).unwrap();
        let result = frame
            .lazy()
            .select([
                parse_sql_expression("year(day) AS y").unwrap(),
                parse_sql_expression("date_trunc('month', day) AS m").unwrap(),
            ])
            .collect()
            .unwrap();
        assert_eq!(result.column("y").unwrap().get(0).unwrap(), AnyValue::Int32(2024));
        assert_eq!(
            render_value(&result.column("m").unwrap().get(0).unwrap()).as_deref(),
            Some("2024-03-01 00:00:00")
        );
    }

    #[test]
    fn display_names_drop_quotes_comments_and_spacing() {
        assert_eq!(display_name("sum(\"Score\")"), "sum(Score)");
        assert_eq!(display_name("count(*) -- rows\n"), "count_star()");
        assert_eq!(
            display_name("sum(x)  FILTER (WHERE \"a b\" = 'it''s')"),
            "sum(x) FILTER (WHERE a b = 'it''s')"
        );
    }

    #[test]
    fn rendering_matches_value_by_value_and_vectorized() {
        let frame = df!(
            "f" => [1.5f64, 10.0],
            "b" => [true, false],
            "d" => [chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap(), chrono::NaiveDate::from_ymd_opt(1999, 12, 31).unwrap()],
        )
        .unwrap();
        let mut frame = frame;
        let times = Series::new("t".into(), [1_700_000_000_000_000i64, 1_700_000_000_123_456])
            .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
            .unwrap();
        let lists = Series::new(
            "l".into(),
            [Series::new("".into(), [1i32, 2]), Series::new("".into(), [3i32])],
        );
        frame.with_column(times.into_column()).unwrap();
        frame.with_column(lists.into_column()).unwrap();
        let schema = frame.schema().clone();
        let rendered_frame = frame
            .clone()
            .lazy()
            .select(
                schema
                    .iter()
                    .map(|(name, dtype)| rendered(col(name.clone()), dtype))
                    .collect::<Vec<_>>(),
            )
            .collect()
            .unwrap();
        for column in frame.columns() {
            for row in 0..frame.height() {
                let one = render_value(&column.get(row).unwrap());
                let vectorized = rendered_frame
                    .column(column.name())
                    .unwrap()
                    .get(row)
                    .unwrap()
                    .str_value()
                    .into_owned();
                assert_eq!(one.as_deref(), Some(vectorized.as_str()), "{}", column.name());
            }
        }
        assert_eq!(
            rendered_frame.column("t").unwrap().get(0).unwrap().str_value(),
            "2023-11-14 22:13:20"
        );
        assert_eq!(
            rendered_frame.column("l").unwrap().get(0).unwrap().str_value(),
            "[1, 2]"
        );
    }

    /// Text inside lists and structs is quoted as DuckDB quotes it.
    #[test]
    fn nested_text_is_quoted_like_duckdb() {
        let texts = ["a", "b, c", "it's", "", "NULL", " sp", "a b", "a:b", "[x]"];
        let list = Series::new("".into(), [Series::new("".into(), texts)]);
        assert_eq!(
            render_value(&list.get(0).unwrap()).as_deref(),
            Some("[a, 'b, c', 'it\\'s', '', 'NULL', ' sp', a b, 'a:b', '[x]']")
        );
        let duckdb = duckdb::Connection::open_in_memory().unwrap();
        let expected: String = duckdb
            .query_row(
                "SELECT CAST(['a', 'b, c', 'it''s', '', 'NULL', ' sp', 'a b', 'a:b', '[x]'] AS VARCHAR)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(render_value(&list.get(0).unwrap()), Some(expected));
    }
}
