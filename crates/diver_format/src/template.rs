//! Tooltip templates: text with `{value:format|style}` fields, like
//! Python's f-strings.
//!
//! ```text
//! tooltip('Target {"target":,.0f}\nGap    {c - "target":>8,.1f|red bold}')
//! ```
//!
//! A field's value is any rule expression. The format after `:` reads like
//! Python's (`>10`, `,.2f`, `.1%`, `+d`); anything it doesn't cover is an
//! Excel format (`YYYY-MM-DD`, `#,##0`), which can follow an alignment
//! and width, as in `>12mmm d, yyyy`. The style after `|` is a color and
//! `bold`, `italic` or `underline`. `{{` and `}}` are braces, `\n` a new
//! line, `\t` a tab and `\\` a backslash.

use std::ops::Range;

use crate::format::Format;
use crate::value::{self, Value};

/// A piece of template text, at a byte range of it.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// Text as shown, its escapes read.
    Text { text: String, span: Range<usize> },
    Field(RawField),
}

/// A `{…}` field before it's checked: each part's text, and where it
/// starts in the template.
#[derive(Clone, Debug, PartialEq)]
pub struct RawField {
    /// The whole field, braces included.
    pub span: Range<usize>,
    pub value: (String, usize),
    pub format: Option<(String, usize)>,
    pub style: Option<(String, usize)>,
}

/// Splits template text into text and fields.
pub fn split(text: &str) -> Result<Vec<Piece>, (Range<usize>, String)> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut literal_start = 0;
    let mut i = 0;
    let flush = |pieces: &mut Vec<Piece>, literal: &mut String, start: usize, end: usize| {
        if start < end {
            pieces.push(Piece::Text { text: std::mem::take(literal), span: start..end });
        }
    };
    while i < bytes.len() {
        match bytes[i] {
            b'{' if bytes.get(i + 1) == Some(&b'{') => {
                literal.push('{');
                i += 2;
            }
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                literal.push('}');
                i += 2;
            }
            b'}' => return Err((i..i + 1, "a `}` without its `{`: write `}}` for the brace itself".into())),
            b'\\' => {
                let (shown, len) = match bytes.get(i + 1) {
                    Some(b'n') => ('\n', 2),
                    Some(b't') => ('\t', 2),
                    Some(b'\\') => ('\\', 2),
                    _ => ('\\', 1),
                };
                literal.push(shown);
                i += len;
            }
            b'{' => {
                flush(&mut pieces, &mut literal, literal_start, i);
                let field = field(text, i)?;
                i = field.span.end;
                literal_start = i;
                pieces.push(Piece::Field(field));
            }
            _ => {
                let len = text[i..].chars().next().map_or(1, char::len_utf8);
                literal.push_str(&text[i..i + len]);
                i += len;
            }
        }
    }
    flush(&mut pieces, &mut literal, literal_start, i);
    Ok(pieces)
}

/// The field whose `{` is at `open`.
fn field(text: &str, open: usize) -> Result<RawField, (Range<usize>, String)> {
    let bytes = text.as_bytes();
    let part = |start: usize, end: usize| (text[start..end].to_string(), start);
    let mut value_end = None;
    let mut format_start = None;
    let mut format_end = None;
    let mut style_start = None;
    let mut depth = 0usize;
    let mut i = open + 1;
    while i < bytes.len() {
        let in_value = value_end.is_none();
        match bytes[i] {
            // Quoted names and text may hold any character, as may an Excel
            // format's quoted text.
            quote @ (b'"' | b'\'') if in_value || (quote == b'"' && style_start.is_none()) => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
            }
            b'(' | b'[' if in_value => depth += 1,
            b')' | b']' if in_value => depth = depth.saturating_sub(1),
            b'|' if in_value && bytes.get(i + 1) == Some(&b'|') => i += 1,
            b':' if in_value && depth == 0 => {
                value_end = Some(i);
                format_start = Some(i + 1);
            }
            b'|' if style_start.is_none() && depth == 0 => {
                if in_value {
                    value_end = Some(i);
                } else {
                    format_end = Some(i);
                }
                style_start = Some(i + 1);
            }
            b'}' if depth == 0 => {
                let value = part(open + 1, value_end.unwrap_or(i));
                if value.0.trim().is_empty() {
                    return Err((open..i + 1, "a field needs a value, like {c} or {\"name\"}".into()));
                }
                let format = format_start.map(|start| part(start, format_end.unwrap_or(i)));
                let style = style_start.map(|start| part(start, i));
                return Ok(RawField { span: open..i + 1, value, format, style });
            }
            _ => {}
        }
        i += 1;
    }
    Err((open..text.len(), "this `{` has no closing `}`: write `{{` for the brace itself".into()))
}

/// A field's format: Python's alignment, width and number format, or an
/// Excel format after the alignment and width.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldFormat {
    pub fill: char,
    /// `<`, `>`, `^` or `=` (after the sign).
    pub align: Option<char>,
    pub width: Option<usize>,
    /// `+`: a sign on positive numbers too.
    pub sign: bool,
    /// `,` or `_` between thousands.
    pub grouping: Option<char>,
    pub precision: Option<usize>,
    /// `f`, `e`, `E`, `%`, `d` or `s`.
    pub kind: Option<char>,
    pub excel: Option<Format>,
}

impl FieldFormat {
    pub fn parse(text: &str) -> Result<FieldFormat, String> {
        let chars: Vec<char> = text.chars().collect();
        let is_align = |c: char| matches!(c, '<' | '>' | '^' | '=');
        let mut format = FieldFormat {
            fill: ' ',
            align: None,
            width: None,
            sign: false,
            grouping: None,
            precision: None,
            kind: None,
            excel: None,
        };
        let mut i = 0;
        if chars.len() >= 2 && is_align(chars[1]) {
            format.fill = chars[0];
            format.align = Some(chars[1]);
            i = 2;
        } else if chars.first().copied().is_some_and(is_align) {
            format.align = chars.first().copied();
            i = 1;
        }
        let after_align = i;
        if let Some(python) = python(&chars[i..], &mut format) {
            return python.then_some(format).ok_or_else(|| "the format's width or precision is too large".into());
        }
        // Python's shape with a letter it doesn't have, like `,.2q`.
        if let Some((&kind, before)) = chars[i..].split_last()
            && kind.is_ascii_alphabetic()
            && !before.is_empty()
            && !before.iter().any(char::is_ascii_alphabetic)
            && python(before, &mut format.clone()).is_some()
        {
            return Err(format!("unknown format type `{kind}`: use f, e, %, d or s, like `,.2f`"));
        }
        // An Excel format, perhaps after a width.
        format.sign = false;
        format.grouping = None;
        format.precision = None;
        format.kind = None;
        i = after_align;
        format.width = width(&chars, &mut i);
        let rest: String = chars[i..].iter().collect();
        if rest.trim().is_empty() {
            return Err(format!("`{text}` isn't a format: try `>10`, `,.2f`, `.1%` or 'YYYY-MM-DD'"));
        }
        format.excel = Some(Format::parse(&rest)?);
        Ok(format)
    }

    pub fn has_width(&self) -> bool {
        self.width.is_some()
    }

    /// `value` as the format shows it, padded to the width.
    pub fn apply(&self, value: &Value) -> String {
        let number = value.number().filter(|_| !matches!(value, Value::Text(_)) || self.kind.is_some());
        let shown = match &self.excel {
            Some(excel) => excel.apply(value),
            None => self.python(value, number),
        };
        let numeric = matches!(value, Value::Number(_)) || (number.is_some() && self.kind.is_some_and(|k| k != 's'));
        self.pad(shown, numeric)
    }

    fn python(&self, value: &Value, number: Option<f64>) -> String {
        let plain = self.precision.is_none() && self.grouping.is_none() && !self.sign;
        match (self.kind, number) {
            (Some('s'), _) | (None, None) => {
                let text = value.text();
                match self.precision {
                    Some(n) if self.kind == Some('s') => text.chars().take(n).collect(),
                    _ => text.into_owned(),
                }
            }
            (None, Some(_)) if plain => value.text().into_owned(),
            (_, None) => value.text().into_owned(),
            (kind, Some(n)) => {
                let body = match kind {
                    Some('d') => format!("{:.0}", n.abs()),
                    Some('%') => format!("{:.*}%", self.precision.unwrap_or(6), n.abs() * 100.0),
                    Some(e @ ('e' | 'E')) => exponent(n.abs(), self.precision.unwrap_or(6), e == 'E'),
                    Some(_) => format!("{:.*}", self.precision.unwrap_or(6), n.abs()),
                    // A precision without a kind counts decimals.
                    None => match self.precision {
                        Some(p) => format!("{:.*}", p, n.abs()),
                        None => value::format_number(n.abs()),
                    },
                };
                let body = match self.grouping {
                    Some(separator) if !matches!(kind, Some('e' | 'E')) => group(&body, separator),
                    _ => body,
                };
                let sign = if n < 0.0 && body.chars().any(|c| c.is_ascii_digit() && c != '0') {
                    "-"
                } else if self.sign {
                    "+"
                } else {
                    ""
                };
                format!("{sign}{body}")
            }
        }
    }

    fn pad(&self, text: String, numeric: bool) -> String {
        let Some(width) = self.width else { return text };
        let len = text.chars().count();
        if len >= width {
            return text;
        }
        let fill = |n: usize| std::iter::repeat_n(self.fill, n).collect::<String>();
        let gap = width - len;
        match self.align.unwrap_or(if numeric { '>' } else { '<' }) {
            '<' => text + &fill(gap),
            '^' => fill(gap / 2) + &text + &fill(gap - gap / 2),
            '=' if text.starts_with(['-', '+']) => {
                let (sign, rest) = text.split_at(1);
                format!("{sign}{}{rest}", fill(gap))
            }
            _ => fill(gap) + &text,
        }
    }
}

/// Reads `[sign][width][grouping][.precision][kind]` to the end: `None`
/// when it isn't that, `Some(false)` when a number is too large.
fn python(chars: &[char], format: &mut FieldFormat) -> Option<bool> {
    let mut i = 0;
    if chars.first() == Some(&'+') {
        format.sign = true;
        i += 1;
    }
    format.width = width(chars, &mut i);
    if let Some(&separator @ (',' | '_')) = chars.get(i) {
        format.grouping = Some(separator);
        i += 1;
    }
    if chars.get(i) == Some(&'.') {
        i += 1;
        let start = i;
        while chars.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        let digits: String = chars[start..i].iter().collect();
        match digits.parse::<usize>() {
            Ok(n) if n <= 20 => format.precision = Some(n),
            _ => return Some(false),
        }
    }
    if let Some(&kind @ ('f' | 'F' | 'e' | 'E' | '%' | 'd' | 's')) = chars.get(i) {
        format.kind = Some(kind);
        i += 1;
    }
    if i != chars.len() {
        return None;
    }
    Some(format.width.is_none_or(|w| w <= 200))
}

/// A width: digits that don't start with 0, which Excel formats do.
fn width(chars: &[char], i: &mut usize) -> Option<usize> {
    if !chars.get(*i).is_some_and(|c| matches!(c, '1'..='9')) {
        return None;
    }
    let start = *i;
    while chars.get(*i).is_some_and(char::is_ascii_digit) {
        *i += 1;
    }
    chars[start..*i].iter().collect::<String>().parse().ok()
}

/// `digits` with `separator` between each three of its whole part.
fn group(digits: &str, separator: char) -> String {
    let (whole, rest) = digits.split_at(digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len()));
    let mut out = String::new();
    for (n, c) in whole.chars().enumerate() {
        if n > 0 && (whole.len() - n).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(c);
    }
    out + rest
}

/// Python's `e` format: `1.500000e+03`.
fn exponent(n: f64, precision: usize, upper: bool) -> String {
    let text = format!("{n:.precision$e}");
    let (mantissa, power) = text.split_once('e').unwrap_or((&text, "0"));
    let power: i32 = power.parse().unwrap_or(0);
    let e = if upper { 'E' } else { 'e' };
    format!("{mantissa}{e}{}{:02}", if power < 0 { '-' } else { '+' }, power.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(format: &str, value: Value) -> String {
        FieldFormat::parse(format).unwrap().apply(&value)
    }

    #[test]
    fn python_formats_pad_and_show_numbers() {
        let n = Value::Number(-1234.5678);
        assert_eq!(show(",.2f", n.clone()), "-1,234.57");
        assert_eq!(show(">12,.1f", n.clone()), "    -1,234.6");
        assert_eq!(show("*^9d", Value::Number(42.0)), "***42****");
        assert_eq!(show("+.1%", Value::Number(0.256)), "+25.6%");
        assert_eq!(show(".2e", Value::Number(1500.0)), "1.50e+03");
        assert_eq!(show("_d", Value::Number(1234567.0)), "1_234_567");
        assert_eq!(show(".3", Value::Number(2.0)), "2.000");
        assert_eq!(show("8", Value::Number(5.0)), "       5");
        assert_eq!(show("8", Value::Text("ab".into())), "ab      ");
        assert_eq!(show("=+8.1f", Value::Number(3.0)), "+    3.0");
        assert_eq!(show(".3s", Value::Text("abcdef".into())), "abc");
        // Text in a number format shows as it is.
        assert_eq!(show(",.2f", Value::Text("n/a".into())), "n/a");
        assert_eq!(show(">6", Value::Null), "      ");
    }

    #[test]
    fn excel_formats_follow_an_alignment_and_width() {
        let date = Value::Text("2024-03-15".into());
        assert_eq!(show("YYYY-MM-DD", date.clone()), "2024-03-15");
        assert_eq!(show(">12mmm d, yyyy", date), "Mar 15, 2024");
        assert_eq!(show("#,##0.00", Value::Number(1234.5)), "1,234.50");
        // Excel's own formats that look like Python's stay Excel's.
        assert_eq!(show("0.0%", Value::Number(0.256)), "25.6%");
        assert_eq!(show("0.00", Value::Number(2.0)), "2.00");
        assert_eq!(show("dd", Value::Text("2024-03-05".into())), "05");
        assert!(FieldFormat::parse(",.2q").unwrap_err().contains("`q`"));
        assert_eq!(show("<10#,##0", Value::Number(1234.0)), "1,234     ");
        assert!(FieldFormat::parse(">10").unwrap().excel.is_none());
        assert!(FieldFormat::parse(">10 0 \"x").is_err());
    }

    #[test]
    fn templates_split_into_text_and_fields() {
        let pieces = split(r#"Due {c:>10YYYY-MM-DD|red bold}\n{{x}} {"a:b" || c}{f(x, 'a}')}"#).unwrap();
        let texts: Vec<&str> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Due ", "\n{x} "]);
        let fields: Vec<&RawField> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Field(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(fields[0].value.0, "c");
        assert_eq!(fields[0].format.as_ref().unwrap().0, ">10YYYY-MM-DD");
        assert_eq!(fields[0].style.as_ref().unwrap().0, "red bold");
        assert_eq!(fields[1].value.0, r#""a:b" || c"#);
        assert!(fields[1].format.is_none() && fields[1].style.is_none());
        assert_eq!(fields[2].value.0, "f(x, 'a}')");
        assert_eq!(split("{c|green}").unwrap().len(), 1);
        assert!(split("a } b").unwrap_err().1.contains("without its"));
        assert!(split("a {c").unwrap_err().1.contains("no closing"));
        assert!(split("a {} b").unwrap_err().1.contains("needs a value"));
    }
}
