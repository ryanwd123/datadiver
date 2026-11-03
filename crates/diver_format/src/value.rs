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
    /// Structs like `{'a': 1, 'b': x}`, and maps like `{a=1, b=2}`.
    Struct,
    #[default]
    Unknown,
}

/// A column the rules can refer to. A struct column may list its fields,
/// and a list column its items, for completion after `.`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub kind: ValueKind,
    /// A struct's fields, each a column of its own.
    pub fields: Vec<Column>,
    /// What a list holds, named `""`.
    pub item: Option<Box<Column>>,
}

impl Column {
    pub fn new(name: impl Into<String>, kind: ValueKind) -> Self {
        Column { name: name.into(), kind, fields: Vec::new(), item: None }
    }

    pub fn with_fields(mut self, fields: Vec<Column>) -> Self {
        self.fields = fields;
        self
    }

    pub fn with_item(mut self, item: Column) -> Self {
        self.item = Some(Box::new(item));
        self
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
    /// Fields in order, by name.
    Struct(Vec<(String, Value)>),
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
            ValueKind::List | ValueKind::Struct => match nested::parse(text) {
                Some(value @ (Value::List(_) | Value::Struct(_))) => value,
                _ => Value::Text(text.into()),
            },
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
            Value::Null | Value::List(_) | Value::Struct(_) => None,
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
            Value::Struct(fields) => Cow::Owned(format!(
                "{{{}}}",
                fields.iter().map(|(name, v)| format!("'{name}': {}", v.text())).collect::<Vec<_>>().join(", ")
            )),
        }
    }

    /// Item `index` of a list, counting from 0; a negative index counts back
    /// from the end. A text index names a struct's field.
    pub fn index(&self, index: &Value) -> Value {
        match (self, index) {
            (Value::List(items), index) => {
                let Some(n) = index.number().filter(|n| n.fract() == 0.0) else {
                    return Value::Null;
                };
                let position = if n < 0.0 { items.len() as f64 + n } else { n };
                if position < 0.0 {
                    return Value::Null;
                }
                items.get(position as usize).cloned().unwrap_or_default()
            }
            (Value::Struct(_), Value::Text(name)) => self.field(name),
            _ => Value::Null,
        }
    }

    /// A struct's field, by name; case doesn't count when no field matches exactly.
    pub fn field(&self, name: &str) -> Value {
        let Value::Struct(fields) = self else {
            return Value::Null;
        };
        fields
            .iter()
            .find(|(field, _)| field == name)
            .or_else(|| fields.iter().find(|(field, _)| field.eq_ignore_ascii_case(name)))
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    }

    /// Whether a condition holding this value is met.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::Text(t) => !t.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Struct(fields) => !fields.is_empty(),
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
            Value::Struct(_) => Some(format!("s{}", self.text())),
        }
    }
}

/// Reads lists and structs as DuckDB and Polars write them as text:
/// `[1, 2, NULL]`, `[a, 'b, c', 'it\'s']`, `{'a': 1, 'b': [x]}`, and maps
/// as `{k=1, j=2}`. Text is quoted only when it has to be, so an unquoted
/// `NULL` is a null, and words and numbers stand as they are.
mod nested {
    use super::Value;

    pub fn parse(text: &str) -> Option<Value> {
        let mut reader = Reader { chars: text.trim().chars().collect(), pos: 0 };
        let value = reader.value()?;
        reader.skip_space();
        (reader.pos == reader.chars.len()).then_some(value)
    }

    struct Reader {
        chars: Vec<char>,
        pos: usize,
    }

    impl Reader {
        fn peek(&self) -> Option<char> {
            self.chars.get(self.pos).copied()
        }

        fn skip_space(&mut self) {
            while self.peek().is_some_and(char::is_whitespace) {
                self.pos += 1;
            }
        }

        fn eat(&mut self, ch: char) -> bool {
            self.skip_space();
            let found = self.peek() == Some(ch);
            if found {
                self.pos += 1;
            }
            found
        }

        fn value(&mut self) -> Option<Value> {
            self.skip_space();
            match self.peek()? {
                '[' => {
                    self.pos += 1;
                    let mut items = Vec::new();
                    if self.eat(']') {
                        return Some(Value::List(items));
                    }
                    loop {
                        items.push(self.value()?);
                        if self.eat(']') {
                            return Some(Value::List(items));
                        }
                        if !self.eat(',') {
                            return None;
                        }
                    }
                }
                '{' => {
                    self.pos += 1;
                    let mut fields = Vec::new();
                    if self.eat('}') {
                        return Some(Value::Struct(fields));
                    }
                    loop {
                        self.skip_space();
                        let name = if self.peek() == Some('\'') { self.quoted()? } else { self.word(&[':', '='])? };
                        if !self.eat(':') && !self.eat('=') {
                            return None;
                        }
                        fields.push((name, self.value()?));
                        if self.eat('}') {
                            return Some(Value::Struct(fields));
                        }
                        if !self.eat(',') {
                            return None;
                        }
                    }
                }
                '\'' => self.quoted().map(Value::Text),
                _ => {
                    let word = self.word(&[])?;
                    Some(if word.eq_ignore_ascii_case("null") {
                        Value::Null
                    } else if let Some(n) = word.parse::<f64>().ok().filter(|n| !n.is_nan()) {
                        Value::Number(n)
                    } else {
                        Value::Text(word)
                    })
                }
            }
        }

        /// `'…'`, with `\'` and `''` for a quote and `\\` for a backslash.
        fn quoted(&mut self) -> Option<String> {
            self.pos += 1;
            let mut out = String::new();
            loop {
                let ch = self.peek()?;
                self.pos += 1;
                match ch {
                    '\\' => {
                        out.push(self.peek()?);
                        self.pos += 1;
                    }
                    '\'' if self.peek() == Some('\'') => {
                        out.push('\'');
                        self.pos += 1;
                    }
                    '\'' => return Some(out),
                    ch => out.push(ch),
                }
            }
        }

        /// Unquoted text up to the next `,` or closing bracket at this
        /// level (or one of `stops`), with the spaces around it trimmed.
        fn word(&mut self, stops: &[char]) -> Option<String> {
            let start = self.pos;
            while let Some(ch) = self.peek() {
                if matches!(ch, ',' | ']' | '}') || stops.contains(&ch) {
                    break;
                }
                self.pos += 1;
            }
            let word: String = self.chars[start..self.pos].iter().collect();
            let word = word.trim();
            (!word.is_empty()).then(|| word.to_string())
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
    fn nested_text_reads_as_lists_and_structs() {
        let n = Value::Number;
        let t = |t: &str| Value::Text(t.into());
        let list = |text: &str| Value::from_text(Some(text), ValueKind::List);
        assert_eq!(
            list("[a, 'b, c', 'it\\'s', '', 'NULL', NULL, ' sp', '[x]']"),
            Value::List(vec![t("a"), t("b, c"), t("it's"), t(""), t("NULL"), Value::Null, t(" sp"), t("[x]")])
        );
        assert_eq!(list("[[1, 2], []]"), Value::List(vec![Value::List(vec![n(1.0), n(2.0)]), Value::List(vec![])]));
        let record = Value::from_text(Some("{'item1': 1.5, 'item 2': 'a, b', 'n': NULL, 'xs': [1, 2]}"), ValueKind::Struct);
        assert_eq!(record.field("item1"), n(1.5));
        assert_eq!(record.field("ITEM 2"), t("a, b"));
        assert_eq!(record.field("n"), Value::Null);
        assert_eq!(record.field("xs").index(&n(-1.0)), n(2.0));
        assert_eq!(record.field("missing"), Value::Null);
        assert_eq!(record.index(&t("item1")), n(1.5));
        let map = Value::from_text(Some("{k=1, j=two}"), ValueKind::Struct);
        assert_eq!(map.field("j"), t("two"));
        let items = list("[10, 20, 30]");
        assert_eq!(items.index(&n(0.0)), n(10.0));
        assert_eq!(items.index(&n(-3.0)), n(10.0));
        assert_eq!(items.index(&n(3.0)), Value::Null);
        assert_eq!(items.index(&n(-4.0)), Value::Null);
        assert_eq!(items.index(&n(0.5)), Value::Null);
        // Text that doesn't read as a list stays text.
        assert_eq!(list("[1, 2"), t("[1, 2"));
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
