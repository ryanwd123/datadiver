use crate::domain::documents::ScalarKey;
use duckdb::types::{TimeUnit, Value};

pub const MAX_UNIQUE_VALUES: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicerValue {
    pub key: ScalarKey,
    pub label: String,
    pub available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicerCard {
    pub column: String,
    pub values: Vec<SlicerValue>,
    pub selected: Vec<ScalarKey>,
    pub error: Option<String>,
    /// A condition or SQL card's text; `None` for a value card.
    pub condition: Option<String>,
    /// An SQL card, filtering by a predicate over the whole row.
    pub sql: bool,
    /// A condition card's column minimum and maximum, as hints.
    pub bounds: Option<(String, String)>,
}

impl SlicerCard {
    pub fn selectable(&self, index: usize) -> bool {
        self.values
            .get(index)
            .is_some_and(|value| value.available || self.selected.contains(&value.key))
    }

    pub fn highlighted(&self, index: usize) -> bool {
        self.values.get(index).is_some_and(|value| {
            self.selected.contains(&value.key) || (self.selected.is_empty() && value.available)
        })
    }
}

pub fn supported(dtype: &str) -> bool {
    let dtype = dtype.to_ascii_uppercase();
    !dtype.contains('[')
        && !matches!(
            crate::domain::schema::from_duckdb(&dtype),
            crate::domain::schema::ColumnType::List
                | crate::domain::schema::ColumnType::Struct
                | crate::domain::schema::ColumnType::Other
        )
        && !dtype.starts_with("TIME WITH")
}

pub fn compatible(key: &ScalarKey, dtype: &str) -> bool {
    use crate::domain::schema::ColumnType;
    match key {
        ScalarKey::Null => true,
        ScalarKey::Boolean(_) => matches!(
            crate::domain::schema::from_duckdb(dtype),
            ColumnType::Boolean
        ),
        ScalarKey::String(_) => matches!(
            crate::domain::schema::from_duckdb(dtype),
            ColumnType::String
        ),
        ScalarKey::Signed(_) => matches!(
            crate::domain::schema::from_duckdb(dtype),
            ColumnType::Int8
                | ColumnType::Int16
                | ColumnType::Int32
                | ColumnType::Int64
                | ColumnType::Int128
        ),
        ScalarKey::Unsigned(_) => matches!(
            crate::domain::schema::from_duckdb(dtype),
            ColumnType::UInt8
                | ColumnType::UInt16
                | ColumnType::UInt32
                | ColumnType::UInt64
                | ColumnType::UInt128
        ),
        ScalarKey::FloatBits(_) => matches!(dtype, "FLOAT" | "DOUBLE"),
        ScalarKey::Decimal(_) => dtype.starts_with("DECIMAL"),
        ScalarKey::Date(_) => matches!(crate::domain::schema::from_duckdb(dtype), ColumnType::Date),
        ScalarKey::Datetime { timezone, .. } => {
            dtype.starts_with("TIMESTAMP") && timezone.is_some() == dtype.contains("WITH TIME ZONE")
        }
        ScalarKey::Time(_) => dtype == "TIME",
        ScalarKey::Duration { .. } => dtype == "INTERVAL",
    }
}

pub fn scalar_key(value: Value, dtype: &str) -> Result<ScalarKey, String> {
    Ok(match value {
        Value::Null => ScalarKey::Null,
        Value::Boolean(value) => ScalarKey::Boolean(value),
        Value::Text(value) | Value::Enum(value) => ScalarKey::String(value),
        Value::TinyInt(value) => ScalarKey::Signed(value.to_string()),
        Value::SmallInt(value) => ScalarKey::Signed(value.to_string()),
        Value::Int(value) => ScalarKey::Signed(value.to_string()),
        Value::BigInt(value) => ScalarKey::Signed(value.to_string()),
        Value::HugeInt(value) => ScalarKey::Signed(value.to_string()),
        Value::UTinyInt(value) => ScalarKey::Unsigned(value.to_string()),
        Value::USmallInt(value) => ScalarKey::Unsigned(value.to_string()),
        Value::UInt(value) => ScalarKey::Unsigned(value.to_string()),
        Value::UBigInt(value) => ScalarKey::Unsigned(value.to_string()),
        Value::UHugeInt(value) => ScalarKey::Unsigned(value.to_string()),
        Value::Float(value) => ScalarKey::FloatBits(format!("f32:{}", value.to_bits())),
        Value::Double(value) => ScalarKey::FloatBits(format!("f64:{}", value.to_bits())),
        Value::Decimal(value) => ScalarKey::Decimal(value.to_string()),
        Value::Date32(value) => ScalarKey::Date(value.to_string()),
        Value::Timestamp(unit, value) => ScalarKey::Datetime {
            value: value.to_string(),
            unit: unit_name(unit).into(),
            timezone: dtype.contains("WITH TIME ZONE").then(|| "UTC".into()),
        },
        Value::Time64(unit, value) => ScalarKey::Time(
            match unit {
                TimeUnit::Second => value.checked_mul(1_000_000_000),
                TimeUnit::Millisecond => value.checked_mul(1_000_000),
                TimeUnit::Microsecond => value.checked_mul(1_000),
                TimeUnit::Nanosecond => Some(value),
            }
            .ok_or("slicer time is out of range")?
            .to_string(),
        ),
        Value::Interval {
            months,
            days,
            nanos,
        } => ScalarKey::Duration {
            value: format!("{months} months {days} days {} microseconds", nanos / 1_000),
            unit: "DuckDBInterval".into(),
        },
        _ => return Err(format!("unsupported slicer type: {dtype}")),
    })
}

fn unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Second => "Seconds",
        TimeUnit::Millisecond => "Milliseconds",
        TimeUnit::Microsecond => "Microseconds",
        TimeUnit::Nanosecond => "Nanoseconds",
    }
}

/// Indices of the values whose label contains `filter`, ignoring case. An empty
/// filter shows every value.
pub fn visible_values(card: &SlicerCard, filter: &str) -> Vec<usize> {
    let filter = filter.trim().to_lowercase();
    card.values
        .iter()
        .enumerate()
        .filter(|(_, value)| filter.is_empty() || value.label.to_lowercase().contains(&filter))
        .map(|(index, _)| index)
        .collect()
}

/// A drag always derives its range from the press-time selection. `start` and
/// `end` are value indices; the range covers the values between them in
/// `visible`, so values hidden by a card filter are skipped.
pub fn drag_selection(
    card: &SlicerCard,
    visible: &[usize],
    start: usize,
    end: usize,
    additive: bool,
    initial: &[ScalarKey],
) -> Vec<ScalarKey> {
    let mut selected = if additive {
        initial.to_vec()
    } else {
        Vec::new()
    };
    let (Some(start), Some(end)) = (
        visible.iter().position(|&index| index == start),
        visible.iter().position(|&index| index == end),
    ) else {
        return selected;
    };
    for value in visible[start.min(end)..=start.max(end)]
        .iter()
        .filter_map(|&index| card.values.get(index))
    {
        if (value.available || initial.contains(&value.key)) && !selected.contains(&value.key) {
            selected.push(value.key.clone());
        }
    }
    selected
}

/// Selects every visible available value, replacing the selection.
pub fn select_visible(card: &SlicerCard, visible: &[usize]) -> Vec<ScalarKey> {
    visible
        .iter()
        .filter_map(|&index| card.values.get(index))
        .filter(|value| value.available)
        .map(|value| value.key.clone())
        .collect()
}

pub fn click_selection(card: &SlicerCard, index: usize, toggle: bool) -> Vec<ScalarKey> {
    let Some(value) = card.values.get(index) else {
        return card.selected.clone();
    };
    if !card.selectable(index) {
        return card.selected.clone();
    }
    if toggle || !value.available {
        let mut selected = card.selected.clone();
        if let Some(index) = selected.iter().position(|key| key == &value.key) {
            selected.remove(index);
        } else {
            selected.push(value.key.clone());
        }
        selected
    } else {
        vec![value.key.clone()]
    }
}

pub fn label(key: &ScalarKey) -> String {
    match key {
        ScalarKey::Null => "(Blank)".into(),
        ScalarKey::String(value) if value.is_empty() => "(Empty)".into(),
        ScalarKey::String(value) => value.clone(),
        ScalarKey::Boolean(value) => value.to_string(),
        ScalarKey::Signed(value) | ScalarKey::Unsigned(value) | ScalarKey::Decimal(value) => {
            value.clone()
        }
        ScalarKey::FloatBits(bits) => float_value(bits)
            .map(|value| value.to_string())
            .unwrap_or_else(|| bits.clone()),
        ScalarKey::Date(value) => value
            .parse::<i64>()
            .ok()
            .and_then(|days| {
                chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?
                    .checked_add_signed(chrono::Duration::try_days(days)?)
            })
            .map(|date| date.to_string())
            .unwrap_or_else(|| value.clone()),
        ScalarKey::Time(value) => value
            .parse::<u64>()
            .ok()
            .and_then(|nanos| {
                chrono::NaiveTime::from_num_seconds_from_midnight_opt(
                    u32::try_from(nanos / 1_000_000_000).ok()?,
                    (nanos % 1_000_000_000) as u32,
                )
            })
            .map(|time| time.to_string())
            .unwrap_or_else(|| value.clone()),
        ScalarKey::Datetime { value, unit, .. } => value
            .parse::<i64>()
            .ok()
            .and_then(|value| match unit.as_str() {
                "Nanoseconds" => Some(chrono::DateTime::from_timestamp_nanos(value)),
                "Milliseconds" => chrono::DateTime::from_timestamp_millis(value),
                "Seconds" => chrono::DateTime::from_timestamp(value, 0),
                _ => chrono::DateTime::from_timestamp_micros(value),
            })
            .map(|time| time.naive_utc().to_string())
            .unwrap_or_else(|| value.clone()),
        ScalarKey::Duration { value, .. } => value.clone(),
    }
}

fn float_value(bits: &str) -> Option<f64> {
    if let Some(bits) = bits.strip_prefix("f32:") {
        return bits
            .parse::<u32>()
            .ok()
            .map(|bits| f32::from_bits(bits) as f64);
    }
    bits.strip_prefix("f64:")?
        .parse::<u64>()
        .ok()
        .map(f64::from_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_empty_and_literal_blank_labels_remain_distinct() {
        assert_eq!(label(&ScalarKey::Null), "(Blank)");
        assert_eq!(label(&ScalarKey::String(String::new())), "(Empty)");
        assert_eq!(label(&ScalarKey::String("(Blank)".into())), "(Blank)");
    }

    #[test]
    fn slicer_click_and_drag_keep_baseline_and_skip_disabled_values() {
        let mut card = SlicerCard {
            sql: false,
            condition: None,
            bounds: None,
            column: "value".into(),
            values: (0..5)
                .map(|value| SlicerValue {
                    key: ScalarKey::Signed(value.to_string()),
                    label: value.to_string(),
                    available: value != 2,
                })
                .collect(),
            selected: vec![],
            error: None,
        };
        assert!(card.highlighted(0));
        assert!(!card.highlighted(2));
        card.selected = click_selection(&card, 4, false);
        assert_eq!(card.selected, [ScalarKey::Signed("4".into())]);
        assert_eq!(click_selection(&card, 2, false), card.selected);
        assert!(click_selection(&card, 4, true).is_empty());
        let initial = card.selected.clone();
        let all = visible_values(&card, "");
        assert_eq!(all, [0, 1, 2, 3, 4]);
        assert_eq!(
            drag_selection(&card, &all, 0, 3, false, &initial),
            ["0", "1", "3"].map(|value| ScalarKey::Signed(value.into()))
        );
        assert_eq!(
            drag_selection(&card, &all, 3, 0, true, &initial),
            ["4", "0", "1", "3"].map(|value| ScalarKey::Signed(value.into()))
        );
        // Shrinking the swept range removes prior swept values, keeping the baseline.
        assert_eq!(
            drag_selection(&card, &all, 0, 1, true, &initial),
            ["4", "0", "1"].map(|value| ScalarKey::Signed(value.into()))
        );
    }

    #[test]
    fn filtered_cards_drag_and_select_only_visible_values() {
        let card = SlicerCard {
            sql: false,
            condition: None,
            bounds: None,
            column: "city".into(),
            values: ["Austin", "Boston", "Dallas", "Houston", "Paris"]
                .iter()
                .map(|city| SlicerValue {
                    key: ScalarKey::String((*city).into()),
                    label: (*city).into(),
                    available: *city != "Houston",
                })
                .collect(),
            selected: vec![],
            error: None,
        };
        let keys = |names: &[&str]| {
            names
                .iter()
                .map(|name| ScalarKey::String((*name).into()))
                .collect::<Vec<_>>()
        };
        let visible = visible_values(&card, " TON ");
        assert_eq!(visible, [1, 3]);
        assert!(visible_values(&card, "zz").is_empty());
        // Dragging from Austin's neighbour Boston to Houston skips hidden Dallas.
        assert_eq!(
            drag_selection(&card, &visible, 1, 3, false, &[]),
            keys(&["Boston"])
        );
        let visible = visible_values(&card, "s");
        assert_eq!(
            drag_selection(&card, &visible, 0, 4, false, &[]),
            keys(&["Austin", "Boston", "Dallas", "Paris"])
        );
        // A drag ending on a hidden value selects nothing new.
        assert!(drag_selection(&card, &[0, 1], 0, 4, false, &[]).is_empty());
        assert_eq!(select_visible(&card, &[1, 3]), keys(&["Boston"]));
    }
}
