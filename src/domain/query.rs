#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Contains(String),
    Excludes(String),
    /// `>N`, `<N`, `>=N`, or `<=N` against every numeric column.
    Compare(Comparison, Numeric),
    /// A term scoped to one column, such as `region:north` or `amount>=10`.
    Column(ColumnTerm),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Numeric {
    Signed(i128),
    Unsigned(u128),
    Float(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Equal,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
}

impl Comparison {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
        }
    }

    /// Whether the comparison holds for every non-null value when the literal
    /// lies entirely above (or below) the column type's range.
    pub fn holds_beyond_range(self, literal_above: bool) -> bool {
        match self {
            Self::Equal => false,
            Self::Less | Self::LessOrEqual => literal_above,
            Self::Greater | Self::GreaterOrEqual => !literal_above,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnOperator {
    /// `col:text` matches values containing the text.
    Contains,
    Compare(Comparison),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ColumnTerm {
    /// Column name as typed; resolved against the schema with [`resolve_column`].
    pub column: String,
    pub operator: ColumnOperator,
    /// Value as typed, with quotes removed.
    pub value: String,
    /// The value was wrapped in double quotes, so it may name another column.
    pub quoted: bool,
    /// `-col:text` and `col!=value` keep rows that do not match.
    pub negated: bool,
    /// The whole lowercased word, searched as plain text when no column matches,
    /// so input such as `10:30` or `https://...` still behaves like a word.
    pub word: String,
}

impl ColumnTerm {
    /// Index of the column a comparison is made against: `"small bags">"large bags"`.
    /// Only a double-quoted value that exactly names a column (ignoring case) counts;
    /// single quotes or an unknown name keep the value literal.
    pub fn compared_column<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Option<usize> {
        if !self.quoted || !matches!(self.operator, ColumnOperator::Compare(_)) {
            return None;
        }
        names
            .into_iter()
            .position(|name| name.to_lowercase() == self.value.to_lowercase())
    }

    /// The plain-text token used when the column name matches no column.
    pub fn fallback(&self) -> Token {
        if self.negated {
            Token::Excludes(self.word.clone())
        } else {
            Token::Contains(self.word.clone())
        }
    }
}

/// Index of the column named `wanted`: an exact case-insensitive match, otherwise
/// the only column whose name starts with it.
pub fn resolve_column<'a>(names: impl IntoIterator<Item = &'a str>, wanted: &str) -> Option<usize> {
    let wanted = wanted.to_lowercase();
    let names = names.into_iter().map(str::to_lowercase).collect::<Vec<_>>();
    if let Some(index) = names.iter().position(|name| *name == wanted) {
        return Some(index);
    }
    let mut prefixed = names
        .iter()
        .enumerate()
        .filter(|(_, name)| name.starts_with(&wanted));
    match (prefixed.next(), prefixed.next()) {
        (Some((index, _)), None) => Some(index),
        _ => None,
    }
}

pub fn parse(search: &str) -> Result<Vec<Token>, String> {
    words(search)
        .iter()
        .filter_map(|word| parse_word(word).transpose())
        .collect()
}

/// Split on whitespace outside double quotes. Quotes stay in the word so
/// operators inside them can be told apart from real ones.
fn words(search: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in search.chars() {
        if ch == '"' {
            quoted = !quoted;
        }
        if ch.is_whitespace() && !quoted {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn double_quoted(text: &str) -> bool {
    text.len() >= 2 && text.starts_with('"') && text.ends_with('"')
}

fn unquote(text: &str) -> String {
    text.replace('"', "")
}

/// Parse one word; `None` skips words that are still being typed, such as a lone
/// `-` or `region:`.
fn parse_word(word: &str) -> Result<Option<Token>, String> {
    if matches!(word, "-" | ">" | "<" | ">=" | "<=") {
        return Ok(None);
    }
    for (prefix, comparison) in [
        (">=", Comparison::GreaterOrEqual),
        ("<=", Comparison::LessOrEqual),
        (">", Comparison::Greater),
        ("<", Comparison::Less),
    ] {
        if let Some(rest) = word.strip_prefix(prefix) {
            return parse_number(rest).map(|number| Some(Token::Compare(comparison, number)));
        }
    }
    let (negated, body) = match word.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    if let Some(term) = column_term(body, negated) {
        return Ok((!term.value.is_empty()).then_some(Token::Column(term)));
    }
    let text = unquote(body).to_lowercase();
    Ok(Some(if negated {
        Token::Excludes(text)
    } else {
        Token::Contains(text)
    }))
}

/// Split `col<op>value` at the first operator outside quotes.
fn column_term(body: &str, negated: bool) -> Option<ColumnTerm> {
    let mut quoted = false;
    for (index, ch) in body.char_indices() {
        if ch == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted || index == 0 {
            continue;
        }
        let rest = &body[index..];
        let (operator, length, negates) = if rest.starts_with(">=") {
            (
                ColumnOperator::Compare(Comparison::GreaterOrEqual),
                2,
                false,
            )
        } else if rest.starts_with("<=") {
            (ColumnOperator::Compare(Comparison::LessOrEqual), 2, false)
        } else if rest.starts_with("!=") {
            (ColumnOperator::Compare(Comparison::Equal), 2, true)
        } else {
            match ch {
                ':' => (ColumnOperator::Contains, 1, false),
                '=' => (ColumnOperator::Compare(Comparison::Equal), 1, false),
                '>' => (ColumnOperator::Compare(Comparison::Greater), 1, false),
                '<' => (ColumnOperator::Compare(Comparison::Less), 1, false),
                _ => continue,
            }
        };
        let column = unquote(&body[..index]);
        if column.trim().is_empty() {
            return None;
        }
        return Some(ColumnTerm {
            column,
            operator,
            value: unquote(&rest[length..]),
            quoted: double_quoted(&rest[length..]),
            negated: negated != negates,
            word: unquote(body).to_lowercase(),
        });
    }
    None
}

/// Parses a condition card's text for `column` into terms that must all match.
///
/// Terms are the column-scoped search operators with the column left out:
/// `>=10`, `<100`, `=North`, `!=0`, `:smith` (contains), a bare word (contains),
/// `-word` (does not contain), and inclusive ranges `a..b`, `a..`, `..b`.
/// On text and date columns a value stands for everything it starts or covers,
/// so `..2022-10` includes `2022-10-31` and `..italy` includes `Italy (south)`.
/// Comparison values may be relative dates: `today`, `yesterday`, `tomorrow`,
/// or an offset from today such as `-30d`, `+2w`, `-6m`, `-1y`.
pub fn parse_condition(
    text: &str,
    column: &str,
    today: chrono::NaiveDate,
) -> Result<Vec<ColumnTerm>, String> {
    let trimmed = text.trim();
    let mut terms = Vec::new();
    for word in words(trimmed) {
        let term = |operator, value: &str, quoted, negated| ColumnTerm {
            column: column.to_owned(),
            operator,
            value: value.to_owned(),
            quoted,
            negated,
            word: word.to_lowercase(),
        };
        let compare = |comparison, value: &str, negated| -> Result<ColumnTerm, String> {
            let quoted = double_quoted(value);
            let value = unquote(value);
            if value.is_empty() {
                return Err(format!("{word} needs a value"));
            }
            let value = if quoted {
                value
            } else {
                relative_date(&value, today)?.unwrap_or(value)
            };
            Ok(term(
                ColumnOperator::Compare(comparison),
                &value,
                quoted,
                negated,
            ))
        };
        if let Some((low, high)) = split_range(&word) {
            if low.is_empty() && high.is_empty() {
                return Err(".. needs a start or an end".into());
            }
            if !low.is_empty() {
                terms.push(compare(Comparison::GreaterOrEqual, low, false)?);
            }
            if !high.is_empty() {
                terms.push(compare(Comparison::LessOrEqual, high, false)?);
            }
            continue;
        }
        let operators = [
            (">=", Some(Comparison::GreaterOrEqual), false),
            ("<=", Some(Comparison::LessOrEqual), false),
            ("!=", Some(Comparison::Equal), true),
            (">", Some(Comparison::Greater), false),
            ("<", Some(Comparison::Less), false),
            ("=", Some(Comparison::Equal), false),
            (":", None, false),
        ];
        if let Some((prefix, comparison, negated)) = operators
            .iter()
            .find(|(prefix, _, _)| word.starts_with(prefix))
        {
            let value = &word[prefix.len()..];
            terms.push(match comparison {
                Some(comparison) => compare(*comparison, value, *negated)?,
                None => term(ColumnOperator::Contains, &unquote(value), false, false),
            });
            continue;
        }
        match word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
            Some(rest) => terms.push(term(ColumnOperator::Contains, &unquote(rest), false, true)),
            None => terms.push(term(
                ColumnOperator::Contains,
                &unquote(&word),
                false,
                false,
            )),
        }
    }
    Ok(terms)
}

/// Splits `a..b` at the first `..` outside quotes.
fn split_range(word: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (index, ch) in word.char_indices() {
        if ch == '"' {
            quoted = !quoted;
        } else if !quoted && word[index..].starts_with("..") {
            return Some((&word[..index], &word[index + 2..]));
        }
    }
    None
}

/// Resolves `today`, `yesterday`, `tomorrow`, and offsets like `-30d`, `+2w`,
/// `-6m`, `-1y` to an ISO date. Other values return `None`.
fn relative_date(value: &str, today: chrono::NaiveDate) -> Result<Option<String>, String> {
    use chrono::{Days, Months};
    let lowered = value.to_ascii_lowercase();
    let day = match lowered.as_str() {
        "today" => Some(today),
        "yesterday" => today.checked_sub_days(Days::new(1)),
        "tomorrow" => today.checked_add_days(Days::new(1)),
        _ => {
            let Some(sign) = lowered.chars().next().filter(|ch| matches!(ch, '+' | '-')) else {
                return Ok(None);
            };
            let Some(unit) = lowered.chars().last().filter(|ch| "dwmy".contains(*ch)) else {
                return Ok(None);
            };
            let Ok(amount) = lowered[1..lowered.len() - 1].parse::<u32>() else {
                return Ok(None);
            };
            let forward = sign == '+';
            match unit {
                'd' | 'w' => {
                    let days = Days::new(u64::from(amount) * if unit == 'w' { 7 } else { 1 });
                    if forward {
                        today.checked_add_days(days)
                    } else {
                        today.checked_sub_days(days)
                    }
                }
                _ => {
                    let months =
                        Months::new(amount.saturating_mul(if unit == 'y' { 12 } else { 1 }));
                    if forward {
                        today.checked_add_months(months)
                    } else {
                        today.checked_sub_months(months)
                    }
                }
            }
        }
    };
    day.map(|day| Some(day.format("%Y-%m-%d").to_string()))
        .ok_or_else(|| format!("relative date {value} is out of range"))
}

/// The days a whole or partial date covers, as `[start, end)`: `2022` is the
/// year, `2022-10` the month, `2022-10-05` one day. Other text returns `None`.
pub fn date_period(value: &str) -> Option<(chrono::NaiveDate, chrono::NaiveDate)> {
    use chrono::{Days, Months, NaiveDate};
    let parts = value.split('-').collect::<Vec<_>>();
    let number = |text: &str, digits: std::ops::RangeInclusive<usize>| {
        (digits.contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    let year = i32::try_from(number(parts.first()?, 4..=4)?).ok()?;
    match parts[1..] {
        [] => {
            let start = NaiveDate::from_ymd_opt(year, 1, 1)?;
            Some((start, start.checked_add_months(Months::new(12))?))
        }
        [month] => {
            let start = NaiveDate::from_ymd_opt(year, number(month, 1..=2)?, 1)?;
            Some((start, start.checked_add_months(Months::new(1))?))
        }
        [month, day] => {
            let start = NaiveDate::from_ymd_opt(year, number(month, 1..=2)?, number(day, 1..=2)?)?;
            Some((start, start.checked_add_days(Days::new(1))?))
        }
        _ => None,
    }
}

/// Lowercase words to highlight in cells: `all` applies to every column, while
/// column-scoped terms apply only to the column they name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchHighlights {
    all: Vec<String>,
    scoped: Vec<ColumnTerm>,
}

impl SearchHighlights {
    pub fn is_empty(&self) -> bool {
        self.all.is_empty() && self.scoped.is_empty()
    }

    /// Terms to highlight for each of `names`, resolved the same way as search.
    pub fn for_columns(&self, names: &[&str]) -> Vec<Vec<String>> {
        let mut terms = vec![self.all.clone(); names.len()];
        for term in &self.scoped {
            match resolve_column(names.iter().copied(), &term.column) {
                Some(index) if let Some(other) = term.compared_column(names.iter().copied()) => {
                    terms[index].push(WHOLE_CELL.into());
                    terms[other].push(WHOLE_CELL.into());
                }
                Some(index) => {
                    if matches!(
                        term.operator,
                        ColumnOperator::Contains | ColumnOperator::Compare(Comparison::Equal)
                    ) {
                        terms[index].push(term.value.to_lowercase());
                    }
                }
                None => terms
                    .iter_mut()
                    .for_each(|terms| terms.push(term.word.clone())),
            }
        }
        terms
    }
}

/// Words that rows must contain, for highlighting matches in cells.
pub fn highlight_terms(search: &str) -> SearchHighlights {
    let mut highlights = SearchHighlights::default();
    for token in parse(search).unwrap_or_default() {
        match token {
            Token::Contains(word) if !word.is_empty() => highlights.all.push(word),
            Token::Column(term) if !term.negated => highlights.scoped.push(term),
            _ => {}
        }
    }
    highlights
}

/// A highlight term that marks the whole cell, used for both sides of a
/// column-to-column comparison.
pub const WHOLE_CELL: &str = "";

/// Byte ranges of `text` matching any term case-insensitively, sorted and merged.
pub fn highlight_ranges(text: &str, terms: &[String]) -> Vec<std::ops::Range<usize>> {
    if terms.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // Lowercasing can change byte lengths, so map each lowered byte back to the
    // original character that produced it.
    let mut lower = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    for (index, ch) in text.char_indices() {
        for lowered in ch.to_lowercase() {
            let start = lower.len();
            lower.push(lowered);
            origin.extend(std::iter::repeat_n(
                (index, index + ch.len_utf8()),
                lower.len() - start,
            ));
        }
    }
    let mut ranges = Vec::new();
    for term in terms {
        if term == WHOLE_CELL {
            ranges.push(0..text.len());
            continue;
        }
        let mut from = 0;
        while let Some(found) = lower[from..].find(term.as_str()) {
            let start = from + found;
            let end = start + term.len();
            ranges.push(origin[start].0..origin[end - 1].1);
            from = start + lower[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

pub fn parse_number(text: &str) -> Result<Numeric, String> {
    if !text.contains(['.', 'e', 'E']) {
        if let Ok(number) = text.parse::<i128>() {
            return Ok(Numeric::Signed(number));
        }
        if let Ok(number) = text.parse::<u128>() {
            return Ok(Numeric::Unsigned(number));
        }
    }
    let number = text
        .parse::<f64>()
        .map_err(|_| format!("invalid numeric comparison: {text}"))?;
    if !number.is_finite() {
        return Err(format!("nonfinite numeric comparison: {text}"));
    }
    Ok(Numeric::Float(number))
}

/// A column name being typed after an opening `"` in the search box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnNameQuery {
    pub typed: String,
    /// From the opening quote through the closing one, if typed; replaced on accept.
    pub range: std::ops::Range<usize>,
}

/// In SQL every `"` outside a `'…'` string opens an identifier.
pub fn sql_column_name_query(text: &str, cursor: usize) -> Option<ColumnNameQuery> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &text.as_bytes()[..cursor];
    let mut string = false;
    let mut identifier = None;
    let mut index = 0;
    while index < before.len() {
        match before[index] {
            b'\'' if identifier.is_none() => string = !string,
            b'"' if !string => match identifier {
                None => identifier = Some(index),
                // `""` inside a name is an escaped quote.
                Some(_) if before.get(index + 1) == Some(&b'"') => index += 1,
                Some(_) => identifier = None,
            },
            _ => {}
        }
        index += 1;
    }
    let quote = identifier?;
    // A doubled `""` inside the name is an escaped quote, not the end.
    let mut end = cursor;
    let rest = &text[cursor..];
    let mut chars = rest.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if ch == '"' {
            if chars.peek().is_some_and(|(_, next)| *next == '"') {
                chars.next();
                continue;
            }
            end = cursor + offset + 1;
            break;
        }
    }
    Some(ColumnNameQuery {
        typed: text[quote + 1..cursor].replace("\"\"", "\""),
        range: quote..end,
    })
}

/// Where a `"` starts a column name: at the start of a word, after `-`, or
/// after a comparison operator (`"a">"b"`).
pub fn column_name_query(text: &str, cursor: usize) -> Option<ColumnNameQuery> {
    let mut cursor = cursor.min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let before = &text[..cursor];
    if before.matches('"').count().is_multiple_of(2) {
        return None;
    }
    let quote = before.rfind('"')?;
    let opens_name = before[..quote]
        .chars()
        .next_back()
        .is_none_or(|ch| ch.is_whitespace() || matches!(ch, '-' | '=' | '<' | '>'));
    if !opens_name {
        return None;
    }
    let end = text[cursor..]
        .find('"')
        .map_or(cursor, |closing| cursor + closing + 1);
    Some(ColumnNameQuery {
        typed: before[quote + 1..].to_owned(),
        range: quote..end,
    })
}

/// Columns matching `typed`, best first: names starting with it, then names
/// with a word starting with it, then names containing it, then names holding
/// its characters in order. Ties keep the given order, so callers list visible
/// columns first.
pub fn rank_columns<'a>(names: &[&'a str], typed: &str) -> Vec<&'a str> {
    let typed = typed.to_lowercase();
    let score = |name: &str| {
        let name = name.to_lowercase();
        if name.starts_with(&typed) {
            Some(0)
        } else if name
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|word| word.starts_with(&typed))
        {
            Some(1)
        } else if name.contains(&typed) {
            Some(2)
        } else {
            let mut chars = name.chars();
            typed
                .chars()
                .all(|wanted| chars.any(|ch| ch == wanted))
                .then_some(3)
        }
    };
    let mut ranked = names
        .iter()
        .filter_map(|name| score(name).map(|score| (score, *name)))
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(score, _)| *score);
    ranked.into_iter().map(|(_, name)| name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejects_malformed_and_nonfinite_comparisons() {
        assert!(parse(">wat").is_err());
        assert!(parse("<NaN").is_err());
        assert_eq!(parse("- > < >= <= region:").unwrap(), vec![]);
        assert_eq!(
            parse(">=5 <=-1").unwrap(),
            [
                Token::Compare(Comparison::GreaterOrEqual, Numeric::Signed(5)),
                Token::Compare(Comparison::LessOrEqual, Numeric::Signed(-1)),
            ]
        );
    }

    fn term(
        column: &str,
        operator: ColumnOperator,
        value: &str,
        negated: bool,
        word: &str,
    ) -> Token {
        Token::Column(ColumnTerm {
            column: column.into(),
            operator,
            value: value.into(),
            quoted: false,
            negated,
            word: word.into(),
        })
    }

    #[test]
    fn sql_column_name_queries_open_on_any_quote_outside_strings() {
        let query = |text: &str| sql_column_name_query(text, text.len());
        assert_eq!(query("sum(\"Sal").unwrap().typed, "Sal");
        assert_eq!(query("a+\"").unwrap().range, 2..3);
        assert_eq!(query("region = '\"North"), None);
        assert_eq!(query("\"a\" > 1 AND \"b\" = 'x' OR \"c").unwrap().typed, "c");
        assert_eq!(query("\"done\""), None);
        assert_eq!(query("\"say \"\"hi").unwrap().typed, "say \"hi");
        // The cursor inside a closed name replaces it whole, escaped quotes included.
        let text = "max(\"say \"\"hi\"\"\") ";
        let inside = sql_column_name_query(text, 6).unwrap();
        assert_eq!(&text[inside.range], "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn column_name_queries_open_at_word_starts_and_after_operators() {
        let query = |text: &str| column_name_query(text, text.len());
        assert_eq!(
            query("north \"sal"),
            Some(ColumnNameQuery {
                typed: "sal".into(),
                range: 6..10,
            })
        );
        assert_eq!(query("\"small bags\">\"la").unwrap().typed, "la");
        assert_eq!(query("-\"").unwrap().typed, "");
        // A closed name is not a query, nor a quote inside a word.
        assert_eq!(query("\"sales\""), None);
        assert_eq!(query("abc\"d"), None);
        // The cursor inside a closed name replaces the whole quoted name.
        let text = "\"sales region\":north";
        assert_eq!(column_name_query(text, 4).unwrap().range, 0..14);
    }

    #[test]
    fn column_ranking_prefers_prefixes_then_words_then_fuzzy() {
        let names = [
            "Total Sales",
            "sales region",
            "units",
            "wholesale",
            "s_a_l_e",
        ];
        assert_eq!(
            rank_columns(&names, "sal"),
            ["sales region", "Total Sales", "wholesale", "s_a_l_e"]
        );
        assert_eq!(rank_columns(&names, ""), names);
        assert!(rank_columns(&names, "zz").is_empty());
    }
    #[test]
    fn double_quoted_values_name_columns_only_for_comparisons() {
        let names = ["Small Bags", "Large Bags", "region"];
        let column_term = |text: &str| match parse(text).unwrap().remove(0) {
            Token::Column(term) => term,
            token => panic!("{token:?}"),
        };
        let compared = |text: &str| column_term(text).compared_column(names);
        assert_eq!(compared("\"small bags\">\"large bags\""), Some(1));
        assert_eq!(compared("\"small bags\"!=\"LARGE BAGS\""), Some(1));
        // Unquoted, single-quoted, unknown, and `:` stay literal.
        assert_eq!(compared("region=region"), None);
        assert_eq!(compared("region='region'"), None);
        assert_eq!(compared("region=\"nowhere\""), None);
        assert_eq!(compared("region:\"region\""), None);
        let today = chrono::NaiveDate::from_ymd_opt(2024, 5, 1).unwrap();
        let card = parse_condition(">\"Large Bags\" <=\"today\"", "Small Bags", today).unwrap();
        assert_eq!(card[0].compared_column(names), Some(1));
        // A quoted value is never read as a relative date.
        assert_eq!(card[1].value, "today");
        let highlights = highlight_terms("\"small bags\">\"large bags\"");
        assert_eq!(
            highlights.for_columns(&names),
            [
                vec![WHOLE_CELL.to_owned()],
                vec![WHOLE_CELL.to_owned()],
                vec![]
            ]
        );
        assert_eq!(highlight_ranges("12", &[WHOLE_CELL.into()]), vec![0..2]);
    }

    #[test]
    fn parse_column_scoped_terms_and_quotes() {
        use ColumnOperator::{Compare, Contains};
        assert_eq!(
            parse("Region:North amount>=10 -city=Paris qty!=3 \"order date\"<2024-01-01").unwrap(),
            [
                term("Region", Contains, "North", false, "region:north"),
                term(
                    "amount",
                    Compare(Comparison::GreaterOrEqual),
                    "10",
                    false,
                    "amount>=10"
                ),
                term(
                    "city",
                    Compare(Comparison::Equal),
                    "Paris",
                    true,
                    "city=paris"
                ),
                term("qty", Compare(Comparison::Equal), "3", true, "qty!=3"),
                term(
                    "order date",
                    Compare(Comparison::Less),
                    "2024-01-01",
                    false,
                    "order date<2024-01-01"
                ),
            ]
        );
        // Quoted phrases are one word, and operators inside quotes are literal.
        assert_eq!(
            parse("\"John Smith\" -\"a:b\" name:\"o'brien jr\"").unwrap(),
            [
                Token::Contains("john smith".into()),
                Token::Excludes("a:b".into()),
                Token::Column(ColumnTerm {
                    quoted: true,
                    ..match term("name", Contains, "o'brien jr", false, "name:o'brien jr") {
                        Token::Column(term) => term,
                        _ => unreachable!(),
                    }
                }),
            ]
        );
        // A leading operator is not a column term.
        assert_eq!(
            parse(":x =y").unwrap(),
            [Token::Contains(":x".into()), Token::Contains("=y".into())]
        );
    }

    #[test]
    fn conditions_parse_ranges_operators_and_relative_dates() {
        use ColumnOperator::{Compare, Contains};
        let today = chrono::NaiveDate::from_ymd_opt(2026, 3, 31).unwrap();
        let terms = |text: &str| {
            parse_condition(text, "col", today)
                .unwrap()
                .into_iter()
                .map(|term| (term.operator, term.value, term.negated))
                .collect::<Vec<_>>()
        };
        let compare = |comparison, value: &str| (Compare(comparison), value.to_owned(), false);
        assert_eq!(
            terms("10..20 >=-5 !=0 north -south :\"a b\""),
            [
                compare(Comparison::GreaterOrEqual, "10"),
                compare(Comparison::LessOrEqual, "20"),
                compare(Comparison::GreaterOrEqual, "-5"),
                (Compare(Comparison::Equal), "0".to_owned(), true),
                (Contains, "north".to_owned(), false),
                (Contains, "south".to_owned(), true),
                (Contains, "a b".to_owned(), false),
            ]
        );
        assert_eq!(
            terms("-5..5 ..x"),
            [
                compare(Comparison::GreaterOrEqual, "-5"),
                compare(Comparison::LessOrEqual, "5"),
                compare(Comparison::LessOrEqual, "x"),
            ]
        );
        // Relative dates resolve in comparisons only; contains keeps the word.
        assert_eq!(
            terms("-30d..today >=-1m <+1y =yesterday :today"),
            [
                compare(Comparison::GreaterOrEqual, "2026-03-01"),
                compare(Comparison::LessOrEqual, "2026-03-31"),
                compare(Comparison::GreaterOrEqual, "2026-02-28"),
                compare(Comparison::Less, "2027-03-31"),
                compare(Comparison::Equal, "2026-03-30"),
                (Contains, "today".to_owned(), false),
            ]
        );
        assert_eq!(terms("  "), []);
        for bad in ["..", ">=", "!="] {
            assert!(parse_condition(bad, "col", today).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_column_prefers_exact_then_unique_prefix() {
        let names = ["Region", "Region Code", "Amount", "Name"];
        assert_eq!(resolve_column(names, "region"), Some(0));
        assert_eq!(resolve_column(names, "AM"), Some(2));
        assert_eq!(resolve_column(names, "reg"), None);
        assert_eq!(resolve_column(names, "missing"), None);
    }

    #[test]
    fn scoped_highlights_apply_to_their_column_or_fall_back_to_the_word() {
        let names = ["Region", "Amount", "Name"];
        let highlights = highlight_terms("x reg:North amount>5 name=Bob -name:al 10:30");
        assert_eq!(
            highlights.for_columns(&names),
            [
                vec!["x", "north", "10:30"],
                vec!["x", "10:30"],
                vec!["x", "bob", "10:30"],
            ]
        );
    }

    #[test]
    fn highlights_merge_overlaps_and_map_case_changes_to_original_bytes() {
        let names = ["Region", "Name"];
        assert_eq!(
            highlight_terms("North -south >5 an").for_columns(&names),
            [["north", "an"], ["north", "an"]]
        );
        assert!(highlight_terms(">wat").is_empty());
        let terms = vec!["an".to_owned(), "nn".to_owned()];
        // Adjacent and overlapping matches merge into one range.
        assert_eq!(highlight_ranges("Banana ANNA", &terms), [1..5, 7..10]);
        assert_eq!(highlight_ranges("xanx an", &terms), [1..3, 5..7]);
        // 'İ' lowercases to two characters; ranges must stay on original boundaries.
        let text = "İstanbul";
        let ranges = highlight_ranges(text, &["stan".to_owned()]);
        assert_eq!(&text[ranges[0].clone()], "stan");
        assert_eq!(highlight_ranges(text, &["i".to_owned()]), vec![0..2]);
        assert!(highlight_ranges("abc", &[]).is_empty());
    }

    #[test]
    fn date_periods_cover_years_months_and_days() {
        let date = |text: &str| chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap();
        assert_eq!(
            date_period("2022"),
            Some((date("2022-01-01"), date("2023-01-01")))
        );
        assert_eq!(
            date_period("2022-12"),
            Some((date("2022-12-01"), date("2023-01-01")))
        );
        assert_eq!(
            date_period("2024-2-29"),
            Some((date("2024-02-29"), date("2024-03-01")))
        );
        for bad in [
            "",
            "22",
            "2022-13",
            "2023-02-29",
            "2022-10-05-1",
            "2022-x",
            "north",
            "-5",
        ] {
            assert_eq!(date_period(bad), None, "{bad}");
        }
    }
}
