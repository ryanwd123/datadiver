use crate::domain::documents::{
    Aggregation, AggregationOp, ConditionOp, ConditionValue, Dimension, QueryConfig, ScalarKey,
};
use crate::domain::schema::{ColumnType, TimeUnit, from_duckdb};
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

pub fn data_type_from_duckdb(data_type: &str) -> ColumnType {
    from_duckdb(data_type)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PivotFieldKind {
    GroupBy,
    PivotBy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PivotEdit {
    Add(Dimension),
    Replace {
        original: Dimension,
        replacement: Dimension,
    },
    Remove(Dimension),
    Move {
        field: Dimension,
        direction: isize,
    },
}

pub fn apply(
    config: &mut QueryConfig,
    kind: PivotFieldKind,
    edit: PivotEdit,
) -> Result<bool, String> {
    match edit {
        PivotEdit::Add(field) => {
            field.validate()?;
            let other = match kind {
                PivotFieldKind::GroupBy => &config.pivot_by,
                PivotFieldKind::PivotBy => &config.group_by,
            };
            if other.contains(&field) {
                return Err(format!("{field} is already used in the other pivot pane"));
            }
            let fields = fields_mut(config, kind);
            if fields.contains(&field) {
                return Ok(false);
            }
            fields.push(field);
        }
        PivotEdit::Replace {
            original,
            replacement,
        } => {
            replacement.validate()?;
            let other = match kind {
                PivotFieldKind::GroupBy => &config.pivot_by,
                PivotFieldKind::PivotBy => &config.group_by,
            };
            if other.contains(&replacement) {
                return Err(format!(
                    "{replacement} is already used in the other pivot pane"
                ));
            }
            let fields = fields_mut(config, kind);
            let Some(index) = fields.iter().position(|field| field == &original) else {
                return Ok(false);
            };
            if original == replacement {
                return Ok(false);
            }
            if fields.contains(&replacement) {
                return Err("duplicate dimension".into());
            }
            fields[index] = replacement;
        }
        PivotEdit::Remove(field) => {
            let fields = fields_mut(config, kind);
            let Some(index) = fields.iter().position(|saved| saved == &field) else {
                return Ok(false);
            };
            fields.remove(index);
        }
        PivotEdit::Move { field, direction } => {
            let fields = fields_mut(config, kind);
            let Some(index) = fields.iter().position(|saved| saved == &field) else {
                return Ok(false);
            };
            let next = index as isize + direction;
            if next < 0 || next >= fields.len() as isize {
                return Ok(false);
            }
            fields.swap(index, next as usize);
        }
    }
    Ok(true)
}

fn fields_mut(config: &mut QueryConfig, kind: PivotFieldKind) -> &mut Vec<Dimension> {
    match kind {
        PivotFieldKind::GroupBy => &mut config.group_by,
        PivotFieldKind::PivotBy => &mut config.pivot_by,
    }
}

#[derive(Clone, Debug)]
pub enum AggregationEdit {
    Add(Aggregation),
    Replace {
        original: Aggregation,
        replacement: Aggregation,
    },
    Remove(Aggregation),
    Move {
        aggregation: Aggregation,
        direction: isize,
    },
}

pub const OPERATIONS: [AggregationOp; 15] = [
    AggregationOp::Sum,
    AggregationOp::Mean,
    AggregationOp::Median,
    AggregationOp::Minimum,
    AggregationOp::Maximum,
    AggregationOp::RowCount,
    AggregationOp::NonNullCount,
    AggregationOp::DistinctNonNullCount,
    AggregationOp::First,
    AggregationOp::Last,
    AggregationOp::StandardDeviation,
    AggregationOp::Variance,
    AggregationOp::Percentile,
    AggregationOp::ValuesList,
    AggregationOp::UniqueValuesList,
];

pub fn operation_label(operation: &AggregationOp) -> &'static str {
    match operation {
        AggregationOp::Sum => "Sum",
        AggregationOp::Mean => "Mean",
        AggregationOp::Median => "Median",
        AggregationOp::Minimum => "Minimum",
        AggregationOp::Maximum => "Maximum",
        AggregationOp::RowCount => "Row count",
        AggregationOp::NonNullCount => "Non-null count",
        AggregationOp::DistinctNonNullCount => "Distinct non-null count",
        AggregationOp::First => "First",
        AggregationOp::Last => "Last",
        AggregationOp::StandardDeviation => "Standard deviation",
        AggregationOp::Variance => "Variance",
        AggregationOp::Percentile => "Percentile",
        AggregationOp::ValuesList => "Values list",
        AggregationOp::UniqueValuesList => "Unique values list",
        AggregationOp::SqlExpression => "SQL",
    }
}

pub fn supported_operations(dtype: &ColumnType) -> Vec<AggregationOp> {
    OPERATIONS
        .iter()
        .filter(|operation| {
            if matches!(
                operation,
                AggregationOp::Sum
                    | AggregationOp::Mean
                    | AggregationOp::Median
                    | AggregationOp::StandardDeviation
                    | AggregationOp::Variance
                    | AggregationOp::Percentile
            ) {
                dtype.is_numeric()
            } else if matches!(operation, AggregationOp::Minimum | AggregationOp::Maximum) {
                !matches!(dtype, ColumnType::List | ColumnType::Struct)
            } else {
                true
            }
        })
        .cloned()
        .collect()
}

pub fn default_aggregation(source_column: String, dtype: &ColumnType) -> Aggregation {
    Aggregation {
        sql: None,
        source_column,
        operation: if dtype.is_numeric() {
            AggregationOp::Sum
        } else {
            AggregationOp::NonNullCount
        },
        name: None,
        conditions: Vec::new(),
        percentile: None,
        interpolation: None,
        ddof: None,
    }
}

pub fn apply_aggregation(config: &mut QueryConfig, edit: AggregationEdit) -> Result<bool, String> {
    let mut candidate = config.aggregations.clone();
    match edit {
        AggregationEdit::Add(aggregation) => candidate.push(aggregation),
        AggregationEdit::Replace {
            original,
            replacement,
        } => {
            let index = candidate
                .iter()
                .position(|saved| saved == &original)
                .ok_or("aggregation changed while editing")?;
            candidate[index] = replacement;
        }
        AggregationEdit::Remove(aggregation) => {
            let Some(index) = candidate.iter().position(|saved| saved == &aggregation) else {
                return Ok(false);
            };
            candidate.remove(index);
        }
        AggregationEdit::Move {
            aggregation,
            direction,
        } => {
            let Some(index) = candidate.iter().position(|saved| saved == &aggregation) else {
                return Ok(false);
            };
            let next = index as isize + direction;
            if next < 0 || next >= candidate.len() as isize {
                return Ok(false);
            }
            candidate.swap(index, next as usize);
        }
    }
    if candidate
        .iter()
        .enumerate()
        .any(|(index, item)| candidate[..index].contains(item))
    {
        return Err("duplicate aggregation".into());
    }
    if candidate == config.aggregations {
        return Ok(false);
    }
    config.aggregations = candidate;
    Ok(true)
}

pub fn condition_operation_label(operation: &ConditionOp) -> &'static str {
    match operation {
        ConditionOp::Equal => "Equals",
        ConditionOp::NotEqual => "Does not equal",
        ConditionOp::Greater => "Greater than",
        ConditionOp::GreaterOrEqual => "Greater than or equal",
        ConditionOp::Less => "Less than",
        ConditionOp::LessOrEqual => "Less than or equal",
        ConditionOp::Contains => "Contains",
        ConditionOp::StartsWith => "Starts with",
        ConditionOp::EndsWith => "Ends with",
        ConditionOp::In => "In list",
        ConditionOp::IsNull => "Is null",
        ConditionOp::IsNotNull => "Is not null",
    }
}

pub fn supported_condition_operations(dtype: &ColumnType) -> Vec<ConditionOp> {
    let mut operations = vec![ConditionOp::Equal, ConditionOp::NotEqual];
    if dtype.is_numeric()
        || matches!(
            dtype,
            ColumnType::String
                | ColumnType::Date
                | ColumnType::Datetime(_, _)
                | ColumnType::Time
                | ColumnType::Duration(_)
        )
    {
        operations.extend([
            ConditionOp::Greater,
            ConditionOp::GreaterOrEqual,
            ConditionOp::Less,
            ConditionOp::LessOrEqual,
        ]);
    }
    if matches!(dtype, ColumnType::String) {
        operations.extend([
            ConditionOp::Contains,
            ConditionOp::StartsWith,
            ConditionOp::EndsWith,
        ]);
    }
    operations.extend([ConditionOp::In, ConditionOp::IsNull, ConditionOp::IsNotNull]);
    operations
}

pub fn condition_dtype_supported(dtype: &ColumnType) -> bool {
    matches!(
        dtype,
        ColumnType::Boolean
            | ColumnType::String
            | ColumnType::Date
            | ColumnType::Datetime(_, _)
            | ColumnType::Time
            | ColumnType::Duration(_)
            | ColumnType::Int8
            | ColumnType::Int16
            | ColumnType::Int32
            | ColumnType::Int64
            | ColumnType::Int128
            | ColumnType::UInt8
            | ColumnType::UInt16
            | ColumnType::UInt32
            | ColumnType::UInt64
            | ColumnType::UInt128
            | ColumnType::Float32
            | ColumnType::Float64
    )
}

pub fn parse_condition_values(
    text: &str,
    dtype: &ColumnType,
    operation: &ConditionOp,
) -> Result<Vec<ConditionValue>, String> {
    if matches!(operation, ConditionOp::IsNull | ConditionOp::IsNotNull) {
        return Ok(Vec::new());
    }
    if matches!(dtype, ColumnType::String) && !matches!(operation, ConditionOp::In) {
        return Ok(vec![ConditionValue::Literal(ScalarKey::String(
            text.into(),
        ))]);
    }
    let parts = if matches!(operation, ConditionOp::In) {
        text.split(',').collect::<Vec<_>>()
    } else {
        vec![text]
    };
    if parts.iter().any(|part| part.trim().is_empty()) {
        return Err("enter a value for the condition".into());
    }
    parts
        .into_iter()
        .map(|part| parse_condition_value(part.trim(), dtype))
        .collect()
}

pub fn format_condition_values(values: &[ConditionValue], dtype: &ColumnType) -> String {
    values
        .iter()
        .map(|value| format_condition_value(value, dtype))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_condition_value(value: &ConditionValue, dtype: &ColumnType) -> String {
    match value {
        ConditionValue::TodayOffsetDays(offset) => {
            format_relative("today", i64::from(*offset), 'd')
        }
        ConditionValue::NowOffsetSeconds(offset) => format_relative("now", *offset, 's'),
        ConditionValue::Literal(key) => match key {
            ScalarKey::Null => "null".into(),
            ScalarKey::Boolean(value) => value.to_string(),
            ScalarKey::String(value)
            | ScalarKey::Signed(value)
            | ScalarKey::Unsigned(value)
            | ScalarKey::Decimal(value) => value.clone(),
            ScalarKey::FloatBits(value) => {
                if let Some(bits) = value
                    .strip_prefix("f32:")
                    .and_then(|bits| bits.parse::<u32>().ok())
                {
                    f32::from_bits(bits).to_string()
                } else if let Some(bits) = value
                    .strip_prefix("f64:")
                    .and_then(|bits| bits.parse::<u64>().ok())
                {
                    f64::from_bits(bits).to_string()
                } else {
                    value.clone()
                }
            }
            ScalarKey::Date(value) => value
                .parse::<i64>()
                .ok()
                .and_then(|days| {
                    NaiveDate::from_ymd_opt(1970, 1, 1)?
                        .checked_add_signed(Duration::try_days(days)?)
                })
                .map(|day| day.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| value.clone()),
            ScalarKey::Datetime { value, .. } => {
                let unit = match dtype {
                    ColumnType::Datetime(unit, _) => *unit,
                    _ => TimeUnit::Milliseconds,
                };
                value
                    .parse::<i64>()
                    .ok()
                    .and_then(|value| {
                        let nanos = time_unit_nanos(unit);
                        let seconds = value.div_euclid(1_000_000_000 / nanos);
                        let remainder = value.rem_euclid(1_000_000_000 / nanos) * nanos;
                        chrono::DateTime::from_timestamp(seconds, remainder as u32)
                    })
                    .map(|datetime| datetime.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| value.clone())
            }
            ScalarKey::Time(value) => value
                .parse::<i64>()
                .ok()
                .and_then(|nanos| {
                    if nanos < 0 {
                        return None;
                    }
                    NaiveTime::from_num_seconds_from_midnight_opt(
                        (nanos / 1_000_000_000) as u32,
                        (nanos % 1_000_000_000) as u32,
                    )
                })
                .map(|time| time.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| value.clone()),
            ScalarKey::Duration { value, .. } => {
                let unit = match dtype {
                    ColumnType::Duration(unit) => *unit,
                    _ => TimeUnit::Milliseconds,
                };
                value
                    .parse::<i64>()
                    .ok()
                    .map(|value| {
                        (value as f64 / (1_000_000_000 / time_unit_nanos(unit)) as f64).to_string()
                    })
                    .unwrap_or_else(|| value.clone())
            }
        },
    }
}

fn format_relative(prefix: &str, offset: i64, unit: char) -> String {
    if offset == 0 {
        prefix.into()
    } else if offset > 0 {
        format!("{prefix}+{offset}{unit}")
    } else {
        format!("{prefix}{offset}{unit}")
    }
}

fn parse_condition_value(text: &str, dtype: &ColumnType) -> Result<ConditionValue, String> {
    if matches!(dtype, ColumnType::Date | ColumnType::Datetime(_, _)) {
        if let Some(suffix) = text.strip_prefix("today") {
            let (number, unit) = parse_relative_with_unit(suffix, 'd')?;
            if unit != 'd' {
                return Err("today offset must be in days".into());
            }
            return Ok(ConditionValue::TodayOffsetDays(
                i32::try_from(number).map_err(|_| "today offset is too large")?,
            ));
        }
        if matches!(dtype, ColumnType::Datetime(_, _))
            && let Some(suffix) = text.strip_prefix("now")
        {
            let (number, unit) = parse_relative_with_unit(suffix, 's')?;
            let scale = match unit {
                'd' => 86_400,
                'h' => 3_600,
                'm' => 60,
                's' => 1,
                _ => return Err("use d, h, m, or s after a relative now offset".into()),
            };
            return Ok(ConditionValue::NowOffsetSeconds(
                number
                    .checked_mul(scale)
                    .ok_or("relative offset is too large")?,
            ));
        }
    }
    let key = match dtype {
        ColumnType::Boolean => {
            ScalarKey::Boolean(text.parse::<bool>().map_err(|_| "enter true or false")?)
        }
        ColumnType::String => ScalarKey::String(text.into()),
        ColumnType::Int8
        | ColumnType::Int16
        | ColumnType::Int32
        | ColumnType::Int64
        | ColumnType::Int128 => {
            let number = text.parse::<i128>().map_err(|_| "enter a signed integer")?;
            let in_range = match dtype {
                ColumnType::Int8 => i8::try_from(number).is_ok(),
                ColumnType::Int16 => i16::try_from(number).is_ok(),
                ColumnType::Int32 => i32::try_from(number).is_ok(),
                ColumnType::Int64 => i64::try_from(number).is_ok(),
                _ => true,
            };
            if !in_range {
                return Err("integer is out of range for this column".into());
            }
            ScalarKey::Signed(number.to_string())
        }
        ColumnType::UInt8
        | ColumnType::UInt16
        | ColumnType::UInt32
        | ColumnType::UInt64
        | ColumnType::UInt128 => {
            let number = text
                .parse::<u128>()
                .map_err(|_| "enter a non-negative integer")?;
            let in_range = match dtype {
                ColumnType::UInt8 => u8::try_from(number).is_ok(),
                ColumnType::UInt16 => u16::try_from(number).is_ok(),
                ColumnType::UInt32 => u32::try_from(number).is_ok(),
                ColumnType::UInt64 => u64::try_from(number).is_ok(),
                _ => true,
            };
            if !in_range {
                return Err("integer is out of range for this column".into());
            }
            ScalarKey::Unsigned(number.to_string())
        }
        ColumnType::Float32 => {
            let value = text.parse::<f32>().map_err(|_| "enter a number")?;
            if !value.is_finite() {
                return Err("enter a finite number".into());
            }
            ScalarKey::FloatBits(format!("f32:{}", value.to_bits()))
        }
        ColumnType::Float64 => {
            let value = text.parse::<f64>().map_err(|_| "enter a number")?;
            if !value.is_finite() {
                return Err("enter a finite number".into());
            }
            ScalarKey::FloatBits(format!("f64:{}", value.to_bits()))
        }
        ColumnType::Date => {
            let day = NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map_err(|_| "enter an ISO date (YYYY-MM-DD)")?;
            let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
            ScalarKey::Date(day.signed_duration_since(epoch).num_days().to_string())
        }
        ColumnType::Datetime(unit, timezone) => {
            let datetime = parse_datetime(text)?;
            let nanos = datetime
                .and_utc()
                .timestamp_nanos_opt()
                .ok_or("datetime is out of range")?;
            ScalarKey::Datetime {
                value: (nanos / time_unit_nanos(*unit)).to_string(),
                unit: format!("{unit:?}"),
                timezone: timezone.as_ref().map(ToString::to_string),
            }
        }
        ColumnType::Time => {
            let time = NaiveTime::parse_from_str(text, "%H:%M:%S")
                .or_else(|_| NaiveTime::parse_from_str(text, "%H:%M"))
                .map_err(|_| "enter a time (HH:MM[:SS])")?;
            ScalarKey::Time(
                (i64::from(time.num_seconds_from_midnight()) * 1_000_000_000
                    + i64::from(time.nanosecond()))
                .to_string(),
            )
        }
        ColumnType::Duration(unit) => {
            let seconds = text
                .parse::<f64>()
                .map_err(|_| "enter duration in seconds")?;
            let units = seconds * (1_000_000_000 / time_unit_nanos(*unit)) as f64;
            if !units.is_finite() || units < i64::MIN as f64 || units > i64::MAX as f64 {
                return Err("duration is out of range".into());
            }
            ScalarKey::Duration {
                value: units.round().to_string(),
                unit: format!("{unit:?}"),
            }
        }
        _ => return Err("this column type cannot be used in a condition".into()),
    };
    Ok(ConditionValue::Literal(key))
}

fn parse_datetime(text: &str) -> Result<NaiveDateTime, String> {
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(value);
        }
    }
    let day = NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .map_err(|_| "enter an ISO date and optional time")?;
    day.and_hms_opt(0, 0, 0)
        .ok_or_else(|| "invalid datetime".into())
}

fn time_unit_nanos(unit: TimeUnit) -> i64 {
    match unit {
        TimeUnit::Nanoseconds => 1,
        TimeUnit::Microseconds => 1_000,
        TimeUnit::Milliseconds => 1_000_000,
    }
}

fn parse_relative_with_unit(suffix: &str, default_unit: char) -> Result<(i64, char), String> {
    if suffix.is_empty() {
        return Ok((0, default_unit));
    }
    let (number, unit) = match suffix.chars().last() {
        Some(unit @ ('d' | 'h' | 'm' | 's')) => (&suffix[..suffix.len() - 1], unit),
        _ => (suffix, default_unit),
    };
    if !number.starts_with(['+', '-']) {
        return Err("relative offset needs a + or - sign".into());
    }
    Ok((
        number
            .parse::<i64>()
            .map_err(|_| "invalid relative offset")?,
        unit,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_reorders_and_removes_by_identity() {
        let mut config = QueryConfig::default();
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Add("region".into())
            )
            .unwrap()
        );
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Add("year".into())
            )
            .unwrap()
        );
        assert!(
            !apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Add("region".into())
            )
            .unwrap()
        );
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Move {
                    field: "year".into(),
                    direction: -1
                }
            )
            .unwrap()
        );
        assert_eq!(
            config.group_by,
            [Dimension::from("year"), Dimension::from("region")]
        );
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Remove("region".into())
            )
            .unwrap()
        );
        assert_eq!(config.group_by, [Dimension::from("year")]);
    }

    #[test]
    fn rejects_group_dimension_overlap_without_changing_definition() {
        let mut config = QueryConfig::default();
        apply(
            &mut config,
            PivotFieldKind::PivotBy,
            PivotEdit::Add("year".into()),
        )
        .unwrap();
        let before = config.clone();
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Add("year".into())
            )
            .is_err()
        );
        assert_eq!(config, before);
    }

    #[test]
    fn aggregation_edits_keep_distinct_measures_and_reject_duplicates() {
        let mut config = QueryConfig::default();
        let sum = default_aggregation("amount".into(), &ColumnType::Int64);
        let mut count = sum.clone();
        count.operation = AggregationOp::RowCount;
        assert!(apply_aggregation(&mut config, AggregationEdit::Add(sum.clone())).unwrap());
        assert!(apply_aggregation(&mut config, AggregationEdit::Add(count.clone())).unwrap());
        assert_eq!(config.aggregations.len(), 2);
        assert!(apply_aggregation(&mut config, AggregationEdit::Add(sum.clone())).is_err());
        assert_eq!(config.aggregations, [sum.clone(), count.clone()]);
        assert!(
            apply_aggregation(
                &mut config,
                AggregationEdit::Move {
                    aggregation: count.clone(),
                    direction: -1
                }
            )
            .unwrap()
        );
        assert_eq!(config.aggregations, [count.clone(), sum.clone()]);
        assert!(apply_aggregation(&mut config, AggregationEdit::Remove(sum)).unwrap());
        assert_eq!(config.aggregations, [count]);
    }

    #[test]
    fn operation_choices_follow_source_dtype() {
        assert_eq!(
            default_aggregation("n".into(), &ColumnType::Int64).operation,
            AggregationOp::Sum
        );
        assert_eq!(
            default_aggregation("name".into(), &ColumnType::String).operation,
            AggregationOp::NonNullCount
        );
        assert!(supported_operations(&ColumnType::Int64).contains(&AggregationOp::Percentile));
        assert!(!supported_operations(&ColumnType::String).contains(&AggregationOp::Percentile));
    }

    #[test]
    fn condition_values_keep_types_and_reject_invalid_offsets() {
        assert_eq!(
            parse_condition_values("2025-01-02", &ColumnType::Date, &ConditionOp::Equal).unwrap(),
            vec![ConditionValue::Literal(ScalarKey::Date("20090".into()))]
        );
        assert_eq!(
            parse_condition_values("today+2d", &ColumnType::Date, &ConditionOp::Equal).unwrap(),
            vec![ConditionValue::TodayOffsetDays(2)]
        );
        assert_eq!(
            format_condition_values(
                &parse_condition_values("2025-01-02", &ColumnType::Date, &ConditionOp::Equal)
                    .unwrap(),
                &ColumnType::Date,
            ),
            "2025-01-02"
        );
        assert_eq!(
            parse_condition_values(
                "now-3h",
                &ColumnType::Datetime(TimeUnit::Milliseconds, None),
                &ConditionOp::Greater
            )
            .unwrap(),
            vec![ConditionValue::NowOffsetSeconds(-10_800)]
        );
        assert!(
            parse_condition_values("today+3h", &ColumnType::Date, &ConditionOp::Equal).is_err()
        );
        assert!(
            parse_condition_values(
                "now+999999999999999999999d",
                &ColumnType::Datetime(TimeUnit::Milliseconds, None),
                &ConditionOp::Equal
            )
            .is_err()
        );
        assert!(parse_condition_values("2,,3", &ColumnType::Int64, &ConditionOp::In).is_err());
        assert!(parse_condition_values("128", &ColumnType::Int8, &ConditionOp::Equal).is_err());
        assert_eq!(
            parse_condition_values("", &ColumnType::String, &ConditionOp::Equal).unwrap(),
            vec![ConditionValue::Literal(ScalarKey::String(String::new()))]
        );
    }

    #[test]
    fn condition_operator_choices_match_dtype() {
        assert!(
            supported_condition_operations(&ColumnType::String).contains(&ConditionOp::Contains)
        );
        assert!(
            !supported_condition_operations(&ColumnType::Boolean).contains(&ConditionOp::Greater)
        );
        assert!(supported_condition_operations(&ColumnType::Date).contains(&ConditionOp::Less));
    }
}

#[cfg(test)]
mod dimension_tests {
    use super::*;

    #[test]
    fn expression_edits_replace_in_place_reorder_and_remove() {
        let mut config = QueryConfig {
            group_by: vec!["region".into(), "year".into()],
            ..Default::default()
        };
        let expression = Dimension::expression("upper(region) AS code");
        apply(
            &mut config,
            PivotFieldKind::GroupBy,
            PivotEdit::Replace {
                original: "region".into(),
                replacement: expression.clone(),
            },
        )
        .unwrap();
        assert_eq!(config.group_by, [expression.clone(), "year".into()]);
        let before = config.clone();
        assert!(
            apply(
                &mut config,
                PivotFieldKind::GroupBy,
                PivotEdit::Replace {
                    original: "year".into(),
                    replacement: expression.clone()
                }
            )
            .is_err()
        );
        assert_eq!(config, before);
        assert!(
            apply(
                &mut config,
                PivotFieldKind::PivotBy,
                PivotEdit::Add(expression.clone())
            )
            .is_err()
        );
        apply(
            &mut config,
            PivotFieldKind::GroupBy,
            PivotEdit::Move {
                field: expression.clone(),
                direction: 1,
            },
        )
        .unwrap();
        assert_eq!(config.group_by, ["year".into(), expression.clone()]);
        apply(
            &mut config,
            PivotFieldKind::GroupBy,
            PivotEdit::Remove(expression),
        )
        .unwrap();
        assert_eq!(config.group_by, [Dimension::from("year")]);
    }
}
