//! Excel-style display formats for `display(value, format)`: number formats
//! like `#,##0.00`, `0%` or `$#,##0;($#,##0)`, and date formats like
//! `YYYY-MM-DD` or `mmm d, yyyy h:mm AM/PM`.
//!
//! A format has up to four `;`-separated sections: positive numbers,
//! negative numbers (shown without their sign), zero, and text (`@`).
//! Text in double quotes, or after `\`, is shown as written, as are most
//! other characters. `[Red]`-style brackets are skipped.

use crate::value::{self, Value};

/// A parsed display format.
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    sections: Vec<Section>,
}

#[derive(Clone, Debug, PartialEq)]
enum Section {
    Number(NumberFormat),
    Date(Vec<DatePart>),
    /// A section with `@`, or only literals: shown with `@` replaced by the text.
    Text(Vec<Piece>),
}

#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Literal(String),
    Digit(char),
    Point,
    Comma,
    Percent,
    Exponent { plus: bool },
    Text,
}

#[derive(Clone, Debug, PartialEq)]
struct NumberFormat {
    pieces: Vec<Piece>,
    /// Placeholders before the point, after it, and in the exponent.
    integer: Vec<char>,
    decimals: Vec<char>,
    exponent: Vec<char>,
    thousands: bool,
    /// Divide by 1000 this many times, for commas after the last digit.
    scale: u32,
    percent: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum DatePart {
    Literal(String),
    Year(usize),
    Month(usize),
    Day(usize),
    Hour(usize),
    Minute(usize),
    Second(usize),
    /// `.000` after seconds.
    Fraction(usize),
    /// `AM/PM` (as cased) or `A/P`.
    AmPm { short: bool, lower: bool },
}

/// Formats the examples completion offers, with what each shows.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("#,##0", "1,235"),
    ("#,##0.00", "1,234.57"),
    ("0", "1235"),
    ("0.0", "1234.6"),
    ("0%", "12%"),
    ("0.0%", "12.3%"),
    ("$#,##0.00", "$1,234.57"),
    ("$#,##0;($#,##0)", "($1,235) when negative"),
    ("#,##0,\"K\"", "1K (thousands)"),
    ("#,##0.0,,\"M\"", "1.2M (millions)"),
    ("0.00E+00", "1.23E+03"),
    ("+0;-0;0", "+5, -5, 0"),
    ("YYYY-MM-DD", "2024-03-15"),
    ("MM/DD/YYYY", "03/15/2024"),
    ("DD/MM/YYYY", "15/03/2024"),
    ("mmm d, yyyy", "Mar 15, 2024"),
    ("mmmm yyyy", "March 2024"),
    ("dddd", "Friday"),
    ("ddd mmm d", "Fri Mar 15"),
    ("yyyy-mm-dd hh:mm", "2024-03-15 08:30"),
    ("h:mm AM/PM", "8:30 AM"),
    ("\"Total: \"@", "Total: text"),
];

impl Format {
    pub fn parse(text: &str) -> Result<Format, String> {
        let sections = split_sections(text)?;
        let sections = sections.iter().map(|s| section(s)).collect::<Result<Vec<_>, _>>()?;
        Ok(Format { sections })
    }

    /// `value` as the format shows it. Values the format can't show (text in
    /// a number format, a number in a date format) show as they are.
    pub fn apply(&self, value: &Value) -> String {
        if value.is_null() {
            return String::new();
        }
        let dates = self.sections.iter().any(|s| matches!(s, Section::Date(_)));
        if dates {
            if let Some(stamp) = Stamp::read(value)
                && let Some(Section::Date(parts)) = self.sections.iter().find(|s| matches!(s, Section::Date(_)))
            {
                return format_date(parts, &stamp);
            }
        } else if let Some(n) = number(value) {
            return self.number(n);
        }
        let text = value.text();
        match self.sections.iter().find(|s| matches!(s, Section::Text(_))) {
            Some(Section::Text(pieces)) => text_section(pieces, &text),
            _ => text.into_owned(),
        }
    }

    fn number(&self, n: f64) -> String {
        let numbers: Vec<&NumberFormat> = self
            .sections
            .iter()
            .filter_map(|s| match s {
                Section::Number(f) => Some(f),
                _ => None,
            })
            .collect();
        let literal = |i: usize| match self.sections.get(i) {
            Some(Section::Text(pieces)) => Some(text_section(pieces, &value::format_number(n))),
            _ => None,
        };
        match self.sections.len() {
            // A literal-only positive section, like "-", shows as is.
            _ if numbers.is_empty() => literal(0).unwrap_or_else(|| value::format_number(n)),
            1 => format_number(numbers[0], n, true),
            _ => {
                let index = if n > 0.0 {
                    0
                } else if n < 0.0 {
                    1
                } else if self.sections.len() >= 3 {
                    2
                } else {
                    0
                };
                // The negative section shows the value without its sign.
                match self.sections.get(index) {
                    Some(Section::Number(f)) => format_number(f, n.abs(), false),
                    Some(Section::Text(pieces)) => text_section(pieces, ""),
                    _ => format_number(numbers[0], n, true),
                }
            }
        }
    }
}

/// A number from a number, or from text that reads as one.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => Some(*n),
        Value::Text(t) => value::parse_number(t),
        _ => None,
    }
}

/// Splits at `;` outside quotes and brackets.
fn split_sections(text: &str) -> Result<Vec<String>, String> {
    let mut sections = vec![String::new()];
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        let current = sections.last_mut().unwrap();
        match ch {
            ';' => sections.push(String::new()),
            '"' => {
                current.push(ch);
                loop {
                    match chars.next() {
                        Some('"') => break current.push('"'),
                        Some(c) => current.push(c),
                        None => return Err("a quote in the format isn't closed".into()),
                    }
                }
            }
            '\\' => {
                current.push(ch);
                if let Some(c) = chars.next() {
                    current.push(c);
                }
            }
            _ => current.push(ch),
        }
    }
    if sections.len() > 4 {
        return Err("a format has at most four sections: positive;negative;zero;text".into());
    }
    Ok(sections)
}

/// Reads the literal at `chars`' head (`"…"`, `\x`, `_x`, `*x`, `[…]`), if any.
fn literal(ch: char, chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<Option<String>, String> {
    Ok(Some(match ch {
        '"' => {
            let mut text = String::new();
            loop {
                match chars.next() {
                    Some('"') => break text,
                    Some(c) => text.push(c),
                    None => return Err("a quote in the format isn't closed".into()),
                }
            }
        }
        '\\' => chars.next().map(String::from).unwrap_or_default(),
        // `_x` leaves room for x; `*x` fills with x, which needs a width.
        '_' => {
            chars.next();
            " ".into()
        }
        '*' => {
            chars.next();
            String::new()
        }
        '[' => {
            for c in chars.by_ref() {
                if c == ']' {
                    return Ok(Some(String::new()));
                }
            }
            return Err("a `[` in the format isn't closed".into());
        }
        _ => return Ok(None),
    }))
}

fn is_date_letter(ch: char) -> bool {
    matches!(ch.to_ascii_lowercase(), 'y' | 'm' | 'd' | 'h' | 's')
}

/// Whether the section has date letters outside literals.
fn has_date_letters(text: &str) -> Result<bool, String> {
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if literal(ch, &mut chars)?.is_some() {
            continue;
        }
        if is_date_letter(ch) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn section(text: &str) -> Result<Section, String> {
    if text.eq_ignore_ascii_case("general") {
        return Ok(Section::Text(vec![Piece::Text]));
    }
    if has_date_letters(text)? {
        return date_section(text).map(Section::Date);
    }
    let mut pieces = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(text) = literal(ch, &mut chars)? {
            pieces.push(Piece::Literal(text));
            continue;
        }
        pieces.push(match ch {
            '0' | '#' | '?' => Piece::Digit(ch),
            '.' => Piece::Point,
            ',' => Piece::Comma,
            '%' => Piece::Percent,
            '@' => Piece::Text,
            'E' | 'e' if matches!(chars.peek(), Some('+' | '-')) => {
                let plus = chars.next() == Some('+');
                Piece::Exponent { plus }
            }
            _ => Piece::Literal(ch.to_string()),
        });
    }
    Ok(match number_format(&pieces) {
        Some(format) if !pieces.contains(&Piece::Text) => Section::Number(format),
        _ => Section::Text(pieces),
    })
}

/// Works out a number section's placeholders; `None` when it has no digits.
fn number_format(pieces: &[Piece]) -> Option<NumberFormat> {
    let mut format = NumberFormat {
        pieces: pieces.to_vec(),
        integer: Vec::new(),
        decimals: Vec::new(),
        exponent: Vec::new(),
        thousands: false,
        scale: 0,
        percent: 0,
    };
    #[derive(PartialEq)]
    enum Part {
        Integer,
        Decimals,
        Exponent,
    }
    let mut part = Part::Integer;
    let last_digit = pieces.iter().rposition(|p| matches!(p, Piece::Digit(_)))?;
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Digit(d) => match part {
                Part::Integer => format.integer.push(*d),
                Part::Decimals => format.decimals.push(*d),
                Part::Exponent => format.exponent.push(*d),
            },
            Piece::Point if part == Part::Integer => part = Part::Decimals,
            Piece::Exponent { .. } => part = Part::Exponent,
            Piece::Comma if part == Part::Integer && i < last_digit && !format.integer.is_empty() => {
                format.thousands = true
            }
            // Commas right after the last digit scale by a thousand each.
            Piece::Comma if i > last_digit && pieces[last_digit + 1..i].iter().all(|p| *p == Piece::Comma) => {
                format.scale += 1
            }
            Piece::Percent => format.percent += 1,
            _ => {}
        }
    }
    Some(format)
}

fn format_number(format: &NumberFormat, n: f64, signed: bool) -> String {
    let mut n = n * 100f64.powi(format.percent as i32) / 1000f64.powi(format.scale as i32);
    let negative = n < 0.0;
    n = n.abs();
    let mut exponent = 0i32;
    let scientific = format.pieces.iter().any(|p| matches!(p, Piece::Exponent { .. }));
    if scientific && n != 0.0 {
        let whole = format.integer.len().max(1) as i32;
        exponent = n.log10().floor() as i32 - (whole - 1);
        n /= 10f64.powi(exponent);
        // Rounding can carry into another digit: 9.996 shows as 1.00E+01.
        let decimals = format.decimals.len() as i32;
        if round(n, decimals as usize) >= 10f64.powi(whole) {
            n /= 10.0;
            exponent += 1;
        }
    }
    let fixed = format!("{:.*}", format.decimals.len(), round(n, format.decimals.len()));
    let (whole, fraction) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let mut integer = whole.trim_start_matches('0').to_string();
    let zeros = format.integer.iter().filter(|d| **d != '#').count();
    while integer.len() < zeros {
        integer.insert(0, '0');
    }
    if format.thousands {
        integer = group(&integer);
    }
    // `?` placeholders pad with spaces where `0` would show zeros.
    let pad = format.integer.iter().take_while(|d| **d == '?').count();
    if pad > 0 {
        let leading = integer.chars().take_while(|c| *c == '0').count().min(pad);
        integer = " ".repeat(leading) + &integer[leading..];
    }
    let mut fraction: Vec<char> = fraction.chars().collect();
    for (i, d) in format.decimals.iter().enumerate().rev() {
        if fraction[i] != '0' || *d == '0' {
            break;
        }
        if *d == '#' {
            fraction.pop();
        } else {
            fraction[i] = ' ';
        }
    }
    let fraction: String = fraction.into_iter().collect();
    let all_zero = integer.trim().chars().all(|c| c == '0' || c == ',') && fraction.trim().chars().all(|c| c == '0');
    let mut out = String::new();
    let (mut integer_done, mut decimals_done) = (false, false);
    let mut part = 0;
    for piece in &format.pieces {
        match piece {
            Piece::Literal(text) => out.push_str(text),
            Piece::Percent => out.push('%'),
            Piece::Comma => {}
            Piece::Text => {}
            Piece::Point if part == 0 => {
                if !integer_done {
                    out.push_str(&integer);
                    integer_done = true;
                }
                out.push('.');
                part = 1;
            }
            Piece::Point => {}
            Piece::Exponent { plus } => {
                if !integer_done {
                    out.push_str(&integer);
                    integer_done = true;
                }
                out.push('E');
                if exponent < 0 {
                    out.push('-');
                } else if *plus {
                    out.push('+');
                }
                let digits = exponent.unsigned_abs().to_string();
                for _ in digits.len()..format.exponent.len() {
                    out.push('0');
                }
                out.push_str(&digits);
                part = 2;
            }
            Piece::Digit(_) => match part {
                0 if !integer_done => {
                    out.push_str(&integer);
                    integer_done = true;
                }
                1 if !decimals_done => {
                    out.push_str(&fraction);
                    decimals_done = true;
                }
                _ => {}
            },
        }
    }
    if signed && negative && !all_zero {
        out.insert(0, '-');
    }
    out
}

/// Rounds half away from zero at `decimals`, as Excel does.
fn round(n: f64, decimals: usize) -> f64 {
    let p = 10f64.powi(decimals as i32);
    if (n * p).abs() < 1e15 { (n * p).round() / p } else { n }
}

fn group(digits: &str) -> String {
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn text_section(pieces: &[Piece], text: &str) -> String {
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Literal(t) => out.push_str(t),
            Piece::Text => out.push_str(text),
            Piece::Digit(d) => out.push(*d),
            Piece::Point => out.push('.'),
            Piece::Comma => out.push(','),
            Piece::Percent => out.push('%'),
            Piece::Exponent { plus } => out.push_str(if *plus { "E+" } else { "E-" }),
        }
    }
    out
}

fn date_section(text: &str) -> Result<Vec<DatePart>, String> {
    let mut parts = Vec::new();
    let mut chars = text.chars().peekable();
    let push_literal = |parts: &mut Vec<DatePart>, text: &str| match parts.last_mut() {
        Some(DatePart::Literal(last)) => last.push_str(text),
        _ => parts.push(DatePart::Literal(text.into())),
    };
    while let Some(ch) = chars.next() {
        if let Some(text) = literal(ch, &mut chars)? {
            push_literal(&mut parts, &text);
            continue;
        }
        if is_date_letter(ch) {
            let letter = ch.to_ascii_lowercase();
            let mut count = 1;
            while chars.peek().is_some_and(|c| c.to_ascii_lowercase() == letter) {
                chars.next();
                count += 1;
            }
            parts.push(match letter {
                'y' => DatePart::Year(count),
                'm' => DatePart::Month(count),
                'd' => DatePart::Day(count),
                'h' => DatePart::Hour(count),
                _ => DatePart::Second(count),
            });
            continue;
        }
        if ch == '.' && matches!(parts.last(), Some(DatePart::Second(_))) && chars.peek() == Some(&'0') {
            let mut count = 0;
            while chars.peek() == Some(&'0') {
                chars.next();
                count += 1;
            }
            parts.push(DatePart::Fraction(count));
            continue;
        }
        if matches!(ch, 'a' | 'A') {
            let rest: String = chars.clone().take(4).collect();
            if rest.eq_ignore_ascii_case("m/pm") {
                chars.nth(3);
                parts.push(DatePart::AmPm { short: false, lower: ch == 'a' });
                continue;
            }
            if rest.get(..2).is_some_and(|r| r.eq_ignore_ascii_case("/p")) {
                chars.nth(1);
                parts.push(DatePart::AmPm { short: true, lower: ch == 'a' });
                continue;
            }
        }
        push_literal(&mut parts, &ch.to_string());
    }
    // `m` is minutes right after hours or right before seconds.
    let fields: Vec<usize> = (0..parts.len()).filter(|&i| !matches!(parts[i], DatePart::Literal(_))).collect();
    for (k, &i) in fields.iter().enumerate() {
        if let DatePart::Month(count) = parts[i]
            && count <= 2
        {
            let after_hour = k > 0 && matches!(parts[fields[k - 1]], DatePart::Hour(_));
            let before_second = fields.get(k + 1).is_some_and(|&j| matches!(parts[j], DatePart::Second(_)));
            if after_hour || before_second {
                parts[i] = DatePart::Minute(count);
            }
        }
    }
    Ok(parts)
}

/// A date and time read from a value.
struct Stamp {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    /// Digits after the seconds' point, as written.
    fraction: String,
}

impl Stamp {
    fn read(value: &Value) -> Option<Stamp> {
        let Value::Text(text) = value else { return None };
        let text = text.trim();
        let (year, month, day) = value::date_parts(text)?;
        let mut stamp = Stamp { year, month, day, hour: 0, minute: 0, second: 0, fraction: String::new() };
        let time = text.get(11..).unwrap_or("");
        let mut fields = time.splitn(3, ':');
        let mut field = || fields.next().and_then(|f| f.get(..2)).and_then(|f| f.parse::<u32>().ok());
        if let Some(hour) = field() {
            stamp.hour = hour;
            stamp.minute = field().unwrap_or(0);
            if let Some(seconds) = time.splitn(3, ':').nth(2) {
                stamp.second = seconds.get(..2).and_then(|s| s.parse().ok()).unwrap_or(0);
                if let Some(rest) = seconds.get(2..).and_then(|r| r.strip_prefix('.')) {
                    stamp.fraction = rest.chars().take_while(char::is_ascii_digit).collect();
                }
            }
        }
        Some(stamp)
    }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

fn format_date(parts: &[DatePart], stamp: &Stamp) -> String {
    let twelve_hour = parts.iter().any(|p| matches!(p, DatePart::AmPm { .. }));
    let pm = stamp.hour >= 12;
    let mut out = String::new();
    let padded = |n: u32, count: usize| if count >= 2 { format!("{n:02}") } else { n.to_string() };
    for part in parts {
        match *part {
            DatePart::Literal(ref text) => out.push_str(text),
            DatePart::Year(count) if count <= 2 => out.push_str(&format!("{:02}", stamp.year.rem_euclid(100))),
            DatePart::Year(_) => out.push_str(&format!("{:04}", stamp.year)),
            DatePart::Month(count) => {
                let name = MONTHS[stamp.month as usize - 1];
                match count {
                    1 | 2 => out.push_str(&padded(stamp.month, count)),
                    3 => out.push_str(&name[..3]),
                    4 => out.push_str(name),
                    _ => out.push_str(&name[..1]),
                }
            }
            DatePart::Day(count) if count <= 2 => out.push_str(&padded(stamp.day, count)),
            DatePart::Day(count) => {
                let days = value::days_from_civil(stamp.year, stamp.month, stamp.day);
                // 1970-01-01 was a Thursday.
                let name = WEEKDAYS[(days + 3).rem_euclid(7) as usize];
                out.push_str(if count == 3 { &name[..3] } else { name });
            }
            DatePart::Hour(count) => {
                let hour = if twelve_hour { (stamp.hour + 11) % 12 + 1 } else { stamp.hour };
                out.push_str(&padded(hour, count));
            }
            DatePart::Minute(count) => out.push_str(&padded(stamp.minute, count)),
            DatePart::Second(count) => out.push_str(&padded(stamp.second, count)),
            DatePart::Fraction(count) => {
                out.push('.');
                let digits: String = stamp.fraction.chars().chain(std::iter::repeat('0')).take(count).collect();
                out.push_str(&digits);
            }
            DatePart::AmPm { short, lower } => {
                let text = match (short, pm) {
                    (true, false) => "A",
                    (true, true) => "P",
                    (false, false) => "AM",
                    (false, true) => "PM",
                };
                out.push_str(&if lower { text.to_lowercase() } else { text.to_owned() });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(format: &str, value: Value) -> String {
        Format::parse(format).unwrap().apply(&value)
    }

    fn num(format: &str, n: f64) -> String {
        show(format, Value::Number(n))
    }

    #[test]
    fn number_formats_place_digits_separators_and_literals() {
        assert_eq!(num("#,##0", 1234.5), "1,235");
        assert_eq!(num("#,##0.00", 1234567.891), "1,234,567.89");
        assert_eq!(num("0", 0.4), "0");
        assert_eq!(num("0.0", -2.25), "-2.3");
        assert_eq!(num("#.##", 0.5), ".5");
        assert_eq!(num("0.0#", 1.0), "1.0");
        assert_eq!(num("0.0#", 1.234), "1.23");
        assert_eq!(num("000", 7.0), "007");
        assert_eq!(num("$#,##0.00", -1234.5), "-$1,234.50");
        assert_eq!(num("0 \"units\"", 3.0), "3 units");
        assert_eq!(num("\\#0", 3.0), "#3");
        assert_eq!(num("[Red]0.0", 3.0), "3.0");
        assert_eq!(num("General", 2.5), "2.5");
        // Rounding to nothing drops the minus sign.
        assert_eq!(num("0", -0.2), "0");
    }

    #[test]
    fn percents_scaling_and_exponents() {
        assert_eq!(num("0%", 0.123), "12%");
        assert_eq!(num("0.0%", 0.1234), "12.3%");
        assert_eq!(num("#,##0,\"K\"", 1234567.0), "1,235K");
        assert_eq!(num("0.0,,\"M\"", 1234567.0), "1.2M");
        assert_eq!(num("0.00E+00", 1234.0), "1.23E+03");
        assert_eq!(num("0.00E+00", 0.000123), "1.23E-04");
        assert_eq!(num("0.00E+00", 9.996), "1.00E+01");
        assert_eq!(num("0.0E-0", 1234.0), "1.2E3");
    }

    #[test]
    fn sections_pick_by_sign_and_text() {
        let format = "$#,##0;($#,##0);\"-\"";
        assert_eq!(num(format, 5.0), "$5");
        assert_eq!(num(format, -1234.0), "($1,234)");
        assert_eq!(num(format, 0.0), "-");
        assert_eq!(num("+0;-0;0", 5.0), "+5");
        assert_eq!(num("+0;-0;0", -5.0), "-5");
        assert_eq!(show("0.0;-0.0;0;\"[\"@\"]\"", Value::Text("abc".into())), "[abc]");
        assert_eq!(show("\"Total: \"@", Value::Text("east".into())), "Total: east");
        // Text in a number format shows as it is; numbers in text read as numbers.
        assert_eq!(show("0.00", Value::Text("n/a".into())), "n/a");
        assert_eq!(show("0.00", Value::Text("1,234.5".into())), "1234.50");
        assert_eq!(show("0.00", Value::Null), "");
        assert!(Format::parse("0;0;0;0;0").is_err());
        assert!(Format::parse("0 \"open").is_err());
    }

    #[test]
    fn date_formats_name_and_pad_parts() {
        let date = |format: &str, text: &str| show(format, Value::Text(text.into()));
        assert_eq!(date("YYYY-MM-DD", "2024-03-05"), "2024-03-05");
        assert_eq!(date("m/d/yy", "2024-03-05"), "3/5/24");
        assert_eq!(date("mmm d, yyyy", "2024-03-05"), "Mar 5, 2024");
        assert_eq!(date("mmmm yyyy", "2024-12-05"), "December 2024");
        assert_eq!(date("mmmmm", "2024-12-05"), "D");
        assert_eq!(date("dddd", "2024-03-15"), "Friday");
        assert_eq!(date("ddd", "1970-01-01"), "Thu");
        // `m` after hours or before seconds is minutes.
        assert_eq!(date("yyyy-mm-dd hh:mm", "2024-03-15 08:30:59"), "2024-03-15 08:30");
        assert_eq!(date("h:mm AM/PM", "2024-03-15 18:05:00"), "6:05 PM");
        assert_eq!(date("h:mm a/p", "2024-03-15 00:05:00"), "12:05 a");
        assert_eq!(date("mm:ss.000", "2024-03-15 18:05:07.25"), "05:07.250");
        assert_eq!(date("\"Week of\" mmm d", "2024-03-15"), "Week of Mar 15");
        // What isn't a date shows as it is.
        assert_eq!(date("yyyy", "soon"), "soon");
        assert_eq!(num("yyyy", 5.0), "5");
    }
}
