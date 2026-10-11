#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NumericFormat {
    pub precision: u8,
    pub grouping: bool,
}

pub fn numeric_type_name(dtype: &str) -> bool {
    let dtype = dtype.to_ascii_uppercase();
    [
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
        "FLOAT",
        "DOUBLE",
        "DECIMAL",
    ]
    .iter()
    .any(|prefix| dtype.starts_with(prefix))
}

pub fn string_type_name(dtype: &str) -> bool {
    let dtype = dtype.to_ascii_uppercase();
    ["VARCHAR", "CHAR", "ENUM"]
        .iter()
        .any(|prefix| dtype.starts_with(prefix))
}

pub fn temporal_type_name(dtype: &str) -> bool {
    let dtype = dtype.to_ascii_uppercase();
    ["DATE", "TIME", "TIMESTAMP", "INTERVAL"]
        .iter()
        .any(|prefix| dtype.starts_with(prefix))
}

pub fn default_numeric_format(dtype: &str) -> Option<NumericFormat> {
    let dtype = dtype.to_ascii_uppercase();
    numeric_type_name(&dtype).then_some(NumericFormat {
        precision: if dtype.starts_with("FLOAT") || dtype.starts_with("DOUBLE") {
            2
        } else if dtype.starts_with("DECIMAL") {
            dtype
                .split_once(',')
                .and_then(|(_, scale)| scale.trim_end_matches(')').trim().parse::<u8>().ok())
                .unwrap_or(2)
                .min(38)
        } else {
            0
        },
        grouping: true,
    })
}

pub fn display_cell(
    value: Option<&str>,
    dtype: &str,
    format: Option<NumericFormat>,
    limit: usize,
) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let dtype = dtype.to_ascii_uppercase();
    let text = if dtype.starts_with("DECIMAL")
        || [
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
        ]
        .iter()
        .any(|prefix| dtype.starts_with(prefix))
    {
        format_exact_number(value, format)
    } else if dtype.starts_with("FLOAT") || dtype.starts_with("DOUBLE") {
        value
            .parse::<f64>()
            .map(|number| format_float(number, format))
            .unwrap_or_else(|_| value.to_owned())
    } else {
        value.to_owned()
    };
    truncate(&text, limit)
}

fn format_exact_number(value: &str, numeric_format: Option<NumericFormat>) -> String {
    let Some(format) = numeric_format else {
        return value.into();
    };
    let (sign, magnitude) = if value.starts_with(['-', '+']) {
        value.split_at(1)
    } else {
        ("", value)
    };
    let (integer, fraction) = magnitude.split_once('.').unwrap_or((magnitude, ""));
    if integer.is_empty()
        || !integer.bytes().all(|ch| ch.is_ascii_digit())
        || !fraction.bytes().all(|ch| ch.is_ascii_digit())
    {
        return value.into();
    }
    let precision = usize::from(format.precision.min(38));
    let mut digits = integer
        .bytes()
        .chain(fraction.bytes().take(precision))
        .collect::<Vec<_>>();
    digits.resize(integer.len() + precision, b'0');
    if fraction
        .as_bytes()
        .get(precision)
        .is_some_and(|digit| *digit >= b'5')
    {
        let mut carry = true;
        for digit in digits.iter_mut().rev() {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            digits.insert(0, b'1');
        }
    }
    let text = String::from_utf8(digits).expect("decimal digits are ASCII");
    let (integer, fraction) = text.split_at(text.len() - precision);
    let integer = if format.grouping {
        group_integer(integer)
    } else {
        integer.into()
    };
    if precision == 0 {
        format!("{sign}{integer}")
    } else {
        format!("{sign}{integer}.{fraction}")
    }
}

fn format_float(value: f64, numeric_format: Option<NumericFormat>) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let Some(format) = numeric_format else {
        return value.to_string();
    };
    let text = format!("{value:.precision$}", precision = format.precision as usize);
    if !format.grouping {
        return text;
    }
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let grouped = group_integer(integer);
    if fraction.is_empty() {
        grouped
    } else {
        format!("{grouped}.{fraction}")
    }
}

fn group_integer(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + value.len() / 3);
    let (sign, digits) = value
        .strip_prefix('-')
        .map(|digits| (Some('-'), digits))
        .or_else(|| value.strip_prefix('+').map(|digits| (Some('+'), digits)))
        .unwrap_or((None, value));
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return value.into();
    }
    if let Some(sign) = sign {
        result.push(sign);
    }
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    result
}

pub fn group_numeric_text(value: &str) -> String {
    let exponent_start = value.find(['e', 'E']).unwrap_or(value.len());
    let (mantissa, exponent) = value.split_at(exponent_start);
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.trim_start_matches(['-', '+']).is_empty()
        || !integer
            .trim_start_matches(['-', '+'])
            .chars()
            .all(|ch| ch.is_ascii_digit())
        || !fraction.chars().all(|ch| ch.is_ascii_digit())
    {
        return value.into();
    }
    format!(
        "{}{}{}",
        group_integer(integer),
        if fraction.is_empty() { "" } else { "." },
        fraction
    ) + exponent
}

/// A type name for display. DuckDB spells out every member of an enum, even a
/// named one, so `ENUM('a', 'b', ...)` shows as `ENUM`, also inside lists and
/// structs.
pub fn type_label(dtype: &str) -> std::borrow::Cow<'_, str> {
    if !dtype.contains("ENUM(") {
        return dtype.into();
    }
    let mut label = String::with_capacity(dtype.len());
    let mut rest = dtype;
    let mut quote = None;
    while let Some(ch) = rest.chars().next() {
        let at_word_start = !label
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_alphanumeric() || previous == '_');
        if quote.is_none() && at_word_start && rest.starts_with("ENUM(") {
            label.push_str("ENUM");
            rest = skip_parenthesized(&rest["ENUM".len()..]);
            continue;
        }
        match (quote, ch) {
            (None, '\'' | '"') => quote = Some(ch),
            (Some(open), _) if open == ch => quote = None,
            _ => {}
        }
        label.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    label.into()
}

/// The text after the parenthesized group `text` starts with; quoted members
/// may contain parentheses.
fn skip_parenthesized(text: &str) -> &str {
    let mut depth = 0;
    let mut quote = None;
    for (offset, ch) in text.char_indices() {
        match (quote, ch) {
            (Some(open), _) if open == ch => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => {
                depth -= 1;
                if depth == 0 {
                    return &text[offset + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

pub fn truncate(value: &str, limit: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(limit).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_type_labels_hide_their_members() {
        assert_eq!(type_label("VARCHAR"), "VARCHAR");
        assert_eq!(type_label("ENUM('a', 'b')"), "ENUM");
        assert_eq!(type_label("ENUM('x)', 'it''s (')[]"), "ENUM[]");
        assert_eq!(
            type_label("STRUCT(\"ENUM(\" VARCHAR, kind ENUM('a'), n INTEGER)"),
            "STRUCT(\"ENUM(\" VARCHAR, kind ENUM, n INTEGER)"
        );
        assert_eq!(type_label("MAP(VARCHAR, ENUM('p', 'q'))"), "MAP(VARCHAR, ENUM)");
    }

    #[test]
    fn precision_changes_round_exact_decimals_and_pad_integers() {
        let grouped = Some(NumericFormat {
            precision: 2,
            grouping: true,
        });
        assert_eq!(
            display_cell(
                Some("999999999999999999999999999999.995"),
                "DECIMAL(38,3)",
                grouped,
                200
            ),
            "1,000,000,000,000,000,000,000,000,000,000.00"
        );
        assert_eq!(
            display_cell(Some("-1234.567"), "DECIMAL(10,3)", grouped, 200),
            "-1,234.57"
        );
        assert_eq!(
            display_cell(Some("18446744073709551615"), "UBIGINT", grouped, 200),
            "18,446,744,073,709,551,615.00"
        );
        let ungrouped = Some(NumericFormat {
            precision: 0,
            grouping: false,
        });
        assert_eq!(
            display_cell(Some("-1234.5"), "DECIMAL(10,1)", ungrouped, 200),
            "-1235"
        );
        assert_eq!(
            display_cell(Some("1234.567"), "DOUBLE", grouped, 200),
            "1,234.57"
        );
        assert_eq!(display_cell(None, "DOUBLE", grouped, 200), "");
        assert_eq!(default_numeric_format("INTEGER").unwrap().precision, 0);
        assert_eq!(
            default_numeric_format("DECIMAL(38, 12)").unwrap().precision,
            12
        );
    }

    #[test]
    fn integers_are_grouped_without_precision_loss() {
        let format = NumericFormat {
            precision: 0,
            grouping: true,
        };
        assert_eq!(
            display_cell(Some("18446744073709551615"), "UBIGINT", Some(format), 200),
            "18,446,744,073,709,551,615"
        );
    }

    #[test]
    fn strings_are_unquoted_and_unicode_truncation_is_safe() {
        assert_eq!(display_cell(Some("a,b"), "VARCHAR", None, 200), "a,b");
        assert_eq!(truncate(&"😀".repeat(201), 200).chars().count(), 201);
    }

    #[test]
    fn zero_samples_are_not_missing() {
        assert_eq!(default_numeric_format("DOUBLE").unwrap().precision, 2);
    }
}
