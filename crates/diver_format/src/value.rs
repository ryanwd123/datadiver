//! Cell values as the rules see them, and the dates they understand.

use std::borrow::Cow;
use std::cmp::Ordering;

/// What kind of values a column holds, as far as the rules care.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueKind {
    Number,
    Text,
    /// ISO dates and timestamps, like `2024-03-15` or `2024-03-15 08:30:00`.
    Date,
    Bool,
    /// Lists like `[1, 2, 3]`, which a sparkline can draw.
    List,
    #[default]
    Unknown,
}

/// A column the rules can refer to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub kind: ValueKind,
}

impl Column {
    pub fn new(name: impl Into<String>, kind: ValueKind) -> Self {
        Column { name: name.into(), kind }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Value>),
}

impl Value {
    /// A value from a grid's text, read as `kind`. Numbers may have
    /// thousands separators; lists are `[a, b, …]`.
    pub fn from_text(text: Option<&str>, kind: ValueKind) -> Value {
        let Some(text) = text else { return Value::Null };
        match kind {
            ValueKind::Number => parse_number(text).map_or(Value::Text(text.into()), Value::Number),
            ValueKind::Bool => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "t" | "1" | "yes" => Value::Bool(true),
                "false" | "f" | "0" | "no" => Value::Bool(false),
                _ => Value::Text(text.into()),
            },
            ValueKind::List => {
                let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
                let items = inner
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|item| match parse_number(item) {
                        Some(n) => Value::Number(n),
                        None if item.eq_ignore_ascii_case("null") => Value::Null,
                        None => Value::Text(item.trim_matches(['\'', '"']).into()),
                    })
                    .collect();
                Value::List(items)
            }
            ValueKind::Text | ValueKind::Date | ValueKind::Unknown => Value::Text(text.into()),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The value as a number: text that reads as one counts.
    pub fn number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Text(t) => parse_number(t),
            Value::Null | Value::List(_) => None,
        }
    }

    pub fn text(&self) -> Cow<'_, str> {
        match self {
            Value::Null => Cow::Borrowed(""),
            Value::Bool(b) => Cow::Borrowed(if *b { "true" } else { "false" }),
            Value::Number(n) => Cow::Owned(format_number(*n)),
            Value::Text(t) => Cow::Borrowed(t),
            Value::List(items) => {
                Cow::Owned(format!("[{}]", items.iter().map(|v| v.text().into_owned()).collect::<Vec<_>>().join(", ")))
            }
        }
    }

    /// Whether a condition holding this value is met.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Text(t) => !t.is_empty(),
            Value::List(items) => !items.is_empty(),
        }
    }

    /// Compares like SQL: numbers as numbers (text that reads as a number
    /// too), other text as text. Blanks compare with nothing.
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        if self.is_null() || other.is_null() {
            return None;
        }
        if let (Value::Text(a), Value::Text(b)) = (self, other) {
            // Two texts are only numbers when both read as numbers.
            return match (parse_number(a), parse_number(b)) {
                (Some(x), Some(y)) => x.partial_cmp(&y),
                _ => Some(a.as_str().cmp(b.as_str())),
            };
        }
        match (self.number(), other.number()) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => Some(self.text().as_ref().cmp(other.text().as_ref())),
        }
    }

    /// A key that's equal for equal values, for counting duplicates.
    pub fn key(&self) -> Option<String> {
        match self {
            Value::Null => None,
            Value::Number(n) => Some(format!("n{}", if *n == 0.0 { 0.0 } else { *n })),
            Value::Bool(b) => Some(format!("b{b}")),
            Value::Text(t) => Some(format!("t{t}")),
            Value::List(_) => Some(format!("l{}", self.text())),
        }
    }
}

pub fn parse_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    text.parse::<f64>()
        .ok()
        .or_else(|| text.contains(',').then(|| text.replace(',', "").parse().ok()).flatten())
        .filter(|n: &f64| !n.is_nan())
}

pub fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// Days since 1970-01-01 of an ISO date (`YYYY-MM-DD`, anything after it
/// ignored).
pub fn date_days(text: &str) -> Option<i64> {
    let (y, m, d) = date_parts(text)?;
    Some(days_from_civil(y, m, d))
}

pub fn date_parts(text: &str) -> Option<(i64, u32, u32)> {
    let text = text.trim();
    let mut parts = text.get(..10)?.splitn(3, ['-', '/']);
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// Howard Hinnant's `days_from_civil`.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Today in the local time zone isn't known without the OS, so this is
/// today in UTC; hosts that care pass their own with [`crate::Row::today`].
pub fn today_utc() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    secs.div_euclid(86_400)
}

pub fn format_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reads_by_kind() {
        assert_eq!(Value::from_text(Some("1,234.5"), ValueKind::Number), Value::Number(1234.5));
        assert_eq!(Value::from_text(None, ValueKind::Number), Value::Null);
        assert_eq!(
            Value::from_text(Some("[1, 2, null]"), ValueKind::List),
            Value::List(vec![Value::Number(1.0), Value::Number(2.0), Value::Null])
        );
    }

    #[test]
    fn comparisons_follow_the_values() {
        let n = |n: f64| Value::Number(n);
        let t = |t: &str| Value::Text(t.into());
        assert_eq!(n(10.0).compare(&t("9")), Some(Ordering::Greater));
        assert_eq!(t("10").compare(&t("9")), Some(Ordering::Greater));
        assert_eq!(t("b").compare(&t("a")), Some(Ordering::Greater));
        assert_eq!(t("2024-02-01").compare(&t("2024-01-31")), Some(Ordering::Greater));
        assert_eq!(Value::Null.compare(&n(1.0)), None);
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(date_days("1970-01-02"), Some(1));
        assert_eq!(date_days("2024-03-15 08:30:00"), Some(19797));
        assert_eq!(format_date(19797), "2024-03-15");
        assert_eq!(date_days("not a date"), None);
    }
}
