//! Completions at the caret: what the parser says fits there, plus column
//! names inside `"…"` and the color picker after `#`.

use std::ops::Range;

use crate::catalog::{self, ArgValue, Form, FunctionKind, PropertyValue};
use crate::colors::{self, Color};
use crate::lexer::{Token, TokenKind, quote_column, tokenize};
use crate::parser::{Expect, expected};
use crate::value::{Column, ValueKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Operator,
    Action,
    Function,
    Column,
    Cell,
    Color,
    Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub label: String,
    pub insert: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
    pub swatch: Option<Color>,
    /// The byte range the completion replaces; `None` is the word before
    /// the caret.
    pub range: Option<Range<usize>>,
    /// Orders the list before anything is typed.
    pub sort: String,
    /// Open completion again after inserting (for what comes next).
    pub retrigger: bool,
    /// `insert` has `${1:…}` placeholders.
    pub snippet: bool,
    /// Accepting it should open a color picker for the `#…` at the caret.
    pub pick_color: bool,
}

impl Completion {
    fn new(label: impl Into<String>, kind: CompletionKind, sort: impl Into<String>) -> Self {
        let label = label.into();
        Completion {
            insert: label.clone(),
            label,
            detail: None,
            kind,
            swatch: None,
            range: None,
            sort: sort.into(),
            retrigger: false,
            snippet: false,
            pick_color: false,
        }
    }

    fn insert(mut self, insert: impl Into<String>) -> Self {
        self.insert = insert.into();
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn retrigger(mut self) -> Self {
        self.retrigger = true;
        self
    }

    fn snippet(mut self, snippet: impl Into<String>) -> Self {
        self.insert = snippet.into();
        self.snippet = true;
        self
    }
}

/// The token the caret is in or right after, when it's one being typed.
fn token_at(tokens: &[Token], offset: usize) -> Option<&Token> {
    tokens.iter().find(|t| match t.kind {
        TokenKind::Column | TokenKind::Str => t.span.start < offset && (offset < t.span.end || !t.terminated),
        TokenKind::Comment => t.span.start < offset && offset <= t.span.end,
        TokenKind::Ident | TokenKind::Number | TokenKind::Hex => t.span.start < offset && offset <= t.span.end,
        _ => false,
    })
}

/// The `#…` color the caret is in or right after, for a color picker to replace.
pub fn hex_at(text: &str, offset: usize) -> Option<Range<usize>> {
    let tokens = tokenize(text);
    token_at(&tokens, offset).filter(|t| t.kind == TokenKind::Hex).map(|t| t.span.clone())
}

pub fn complete(text: &str, offset: usize, columns: &[Column]) -> Vec<Completion> {
    let offset = offset.min(text.len());
    let tokens = tokenize(text);
    let current = token_at(&tokens, offset).cloned();
    if let Some(token) = &current {
        match token.kind {
            TokenKind::Column => {
                // `"col"."fi|`: a quoted field name.
                let before: Vec<Token> = tokens.iter().filter(|t| t.span.end <= token.span.start).cloned().collect();
                let field = expected(text, before).into_iter().find_map(|e| match e {
                    Expect::Field(path) => Some(path),
                    _ => None,
                });
                if let Some(path) = field {
                    let typed = text[token.span.start + 1..offset].to_lowercase();
                    let range = if token.terminated { token.span.clone() } else { token.span.start..offset };
                    return field_items(&path, columns)
                        .into_iter()
                        .filter(|item| item.label.to_lowercase().starts_with(&typed))
                        .map(|mut item| {
                            item.insert = quote_column(&item.label);
                            item.range = Some(range.clone());
                            item
                        })
                        .collect();
                }
                return quoted_columns(text, token, offset, columns);
            }
            TokenKind::Str | TokenKind::Comment => return Vec::new(),
            TokenKind::Hex => return picker(&text[token.span.start..offset], token.span.clone()),
            _ => {}
        }
    }
    let end = current.as_ref().map_or(offset, |t| t.span.start);
    let before: Vec<Token> = tokens.into_iter().filter(|t| t.span.end <= end).collect();
    let mut out: Vec<Completion> = Vec::new();
    for expect in expected(text, before) {
        for item in items(&expect, columns) {
            if !out.iter().any(|o| o.label == item.label) {
                out.push(item);
            }
        }
    }
    out.sort_by(|a, b| a.sort.cmp(&b.sort));
    out
}

fn items(expect: &Expect, columns: &[Column]) -> Vec<Completion> {
    match expect {
        Expect::Keyword(keyword) => vec![keyword_item(keyword)],
        Expect::Symbol(_) => Vec::new(),
        Expect::Expr => expression_items(columns),
        Expect::Operator => operator_items(),
        Expect::Action => action_items(),
        Expect::Value { property, index } => value_items(property, *index, columns),
        Expect::Arg { call, index, action: true } => visual_arg_items(call, *index, columns),
        Expect::Arg { call, index, action: false } => function_arg_items(call, *index, columns),
        Expect::Field(path) => field_items(path, columns),
        Expect::NamedValue { call, name } => match crate::check::named_arg_value(call, name) {
            Some(ArgValue::Expr) | None => expression_items(columns),
            Some(ArgValue::Column) => column_items(columns, true),
            Some(ArgValue::Color) => color_items(),
            Some(ArgValue::Fill) => words(catalog::FILLS),
            Some(ArgValue::Kind) => words(catalog::SPARK_KINDS),
        },
    }
}

fn keyword_item(keyword: &str) -> Completion {
    let doc = catalog::keyword(keyword).unwrap_or_default();
    let item = Completion::new(keyword, CompletionKind::Keyword, format!("1{keyword}")).detail(doc);
    match keyword {
        "null" | "true" | "false" => item,
        _ => item.insert(format!("{keyword} ")).retrigger(),
    }
}

pub(crate) fn kind_name(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Number => "number",
        ValueKind::Text => "text",
        ValueKind::Date => "date",
        ValueKind::Bool => "true/false",
        ValueKind::List => "list",
        ValueKind::Struct => "struct",
        ValueKind::Unknown => "column",
    }
}

/// The fields after `path.`. Below `c`, every struct column's fields are
/// offered, since rules may format several columns.
fn field_items(path: &crate::parser::FieldPath, columns: &[Column]) -> Vec<Completion> {
    let bases: Vec<&Column> = match &path.column {
        Some(name) => columns
            .iter()
            .find(|c| c.name == *name)
            .or_else(|| columns.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
            .into_iter()
            .collect(),
        None => columns.iter().collect(),
    };
    let mut out: Vec<Completion> = Vec::new();
    for base in bases {
        let mut column = Some(base);
        for step in &path.steps {
            column = column.and_then(|c| match step {
                Some(name) => c
                    .fields
                    .iter()
                    .find(|f| f.name == *name)
                    .or_else(|| c.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))),
                None => c.item.as_deref(),
            });
        }
        for field in column.map(|c| c.fields.as_slice()).unwrap_or_default() {
            if out.iter().any(|o| o.label == field.name) {
                continue;
            }
            let plain = field.name.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                && field.name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
            let insert = if plain { field.name.clone() } else { quote_column(&field.name) };
            let sort = format!("0{:04}", out.len());
            out.push(
                Completion::new(field.name.clone(), CompletionKind::Column, sort)
                    .insert(insert)
                    .detail(kind_name(field.kind)),
            );
        }
    }
    out
}

fn column_items(columns: &[Column], cell: bool) -> Vec<Completion> {
    let mut out = Vec::new();
    if cell {
        out.push(Completion::new("c", CompletionKind::Cell, "0").detail("this cell"));
    }
    out.extend(columns.iter().enumerate().map(|(i, column)| {
        Completion::new(column.name.clone(), CompletionKind::Column, format!("2{i:04}"))
            .insert(quote_column(&column.name))
            .detail(kind_name(column.kind))
    }));
    out
}

fn expression_items(columns: &[Column]) -> Vec<Completion> {
    let mut out = column_items(columns, true);
    for (i, f) in catalog::FUNCTIONS.iter().enumerate() {
        let insert = match (f.min_args, f.max_args) {
            (0, 0) => format!("{}()", f.name),
            _ if f.kind == FunctionKind::Scalar => format!("{}(${{1}})", f.name),
            _ => format!("{}(${{1:c}})", f.name),
        };
        out.push(
            Completion::new(f.name, CompletionKind::Function, format!("3{i:03}"))
                .snippet(insert)
                .detail(f.signature),
        );
    }
    for keyword in ["not", "null", "true", "false"] {
        out.push(keyword_item(keyword));
    }
    out
}

fn operator_items() -> Vec<Completion> {
    let ops: &[(&str, &str, &str)] = &[
        (">", "> ", "greater than"),
        (">=", ">= ", "at least"),
        ("<", "< ", "less than"),
        ("<=", "<= ", "at most"),
        ("=", "= ", "equal to"),
        ("!=", "!= ", "not equal to"),
        ("is null", "is null", "blank"),
        ("is not null", "is not null", "not blank"),
        ("between", "between ${1:low} and ${2:high}", "within a range, ends included"),
        ("in", "in (${1})", "one of a list"),
        ("not in", "not in (${1})", "none of a list"),
        ("contains", "contains '${1}'", "has the text in it, ignoring case"),
        ("startswith", "startswith '${1}'", "starts with the text, ignoring case"),
        ("endswith", "endswith '${1}'", "ends with the text, ignoring case"),
        ("like", "like '${1:%}'", "matches a pattern: % any text, _ one character"),
    ];
    ops.iter()
        .enumerate()
        .map(|(i, (label, insert, detail))| {
            let item = Completion::new(*label, CompletionKind::Operator, format!("0{i:02}")).detail(*detail);
            if insert.contains('$') {
                item.snippet(*insert)
            } else if insert.ends_with(' ') {
                item.insert(*insert).retrigger()
            } else {
                item.insert(*insert)
            }
        })
        .collect()
}

fn action_items() -> Vec<Completion> {
    catalog::ACTIONS
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let item = Completion::new(a.name, CompletionKind::Action, format!("2{i:02}")).detail(a.signature);
            match a.form {
                Form::Flag => item,
                Form::Property(_) => item.insert(format!("{}: ", a.name)).retrigger(),
                Form::Call => item.snippet(format!("{}(${{1}})", a.name)).retrigger(),
            }
        })
        .collect()
}

fn color_items() -> Vec<Completion> {
    let mut out = vec![pick_item(None)];
    out.extend(colors::NAMED.iter().filter(|(name, _)| *name != "transparent").map(|(name, color)| {
        let mut item = Completion::new(*name, CompletionKind::Color, format!("5{name}")).detail(color.to_hex());
        item.swatch = Some(*color);
        item
    }));
    out
}

/// "Pick a color…", which opens the host's picker.
fn pick_item(range: Option<Range<usize>>) -> Completion {
    let mut item = Completion::new("Pick a color…", CompletionKind::Color, "4")
        .insert("#")
        .detail("opens a color picker");
    item.pick_color = true;
    item.range = range;
    item
}

fn words(words: &[&str]) -> Vec<Completion> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| Completion::new(*w, CompletionKind::Value, format!("1{i:02}")))
        .collect()
}

fn widths() -> Vec<Completion> {
    ["1px", "2px", "3px"]
        .iter()
        .enumerate()
        .map(|(i, w)| Completion::new(*w, CompletionKind::Value, format!("0{i}")).detail("width"))
        .collect()
}

fn value_items(property: &str, index: usize, columns: &[Column]) -> Vec<Completion> {
    let Some(def) = catalog::action(property) else { return Vec::new() };
    let kind = match def.form {
        Form::Property(kind) => kind,
        // `bar: green` is short for `bar(green)`.
        Form::Call => return visual_arg_items(def.name, index, columns),
        Form::Flag => return Vec::new(),
    };
    match (kind, index) {
        (PropertyValue::Paint, 0) => {
            let mut out = color_items();
            out.push(
                Completion::new("scale", CompletionKind::Function, "45")
                    .snippet("scale(${1})")
                    .retrigger()
                    .detail("shade by the value"),
            );
            out
        }
        (PropertyValue::Color, 0) => color_items(),
        (PropertyValue::Width, 0) => widths(),
        (PropertyValue::LineStyle, 0) => words(catalog::LINE_STYLES),
        (PropertyValue::Align, 0) => words(catalog::ALIGNMENTS),
        (PropertyValue::Border, _) => {
            let mut out = widths();
            out.extend(words(catalog::LINE_STYLES));
            out.extend(color_items());
            out
        }
        (PropertyValue::Icon, 0) => catalog::ICONS
            .iter()
            .enumerate()
            .map(|(i, (name, _, color))| {
                let mut item = Completion::new(*name, CompletionKind::Value, format!("1{i:02}")).detail("icon");
                item.swatch = Some(Color::rgb((color >> 16) as u8, (color >> 8) as u8, *color as u8));
                item
            })
            .collect(),
        (PropertyValue::Icon, 1) => color_items(),
        _ => Vec::new(),
    }
}

fn named_items(call: &str) -> Vec<Completion> {
    catalog::named_args(call)
        .iter()
        .enumerate()
        .map(|(i, (name, _, doc))| {
            Completion::new(format!("{name}="), CompletionKind::Keyword, format!("6{i}"))
                .detail(*doc)
                .retrigger()
        })
        .collect()
}

fn visual_arg_items(call: &str, index: usize, columns: &[Column]) -> Vec<Completion> {
    let mut out = match (call, index) {
        ("scale", 0..=2) | ("bar", 0..=1) => color_items(),
        ("icons", 0) => catalog::ICON_SETS
            .iter()
            .enumerate()
            .map(|(i, (name, _, doc))| Completion::new(*name, CompletionKind::Value, format!("1{i}")).detail(*doc))
            .collect(),
        ("icons", 1) => vec![Completion::new("reverse", CompletionKind::Value, "1").detail("high values get the first icon")],
        ("sparkline", _) => column_items(columns, false),
        ("display", 0) => expression_items(columns),
        ("display", 1) => format_items(),
        _ => Vec::new(),
    };
    let named_from = match call {
        "scale" | "display" => 2,
        "sparkline" => 1,
        _ => 1,
    };
    if index >= named_from {
        out.extend(named_items(call));
    }
    out
}

/// Display formats, each in quotes, with what it shows.
fn format_items() -> Vec<Completion> {
    crate::format::EXAMPLES
        .iter()
        .enumerate()
        .map(|(i, (format, shows))| {
            Completion::new(format!("'{format}'"), CompletionKind::Value, format!("1{i:02}")).detail(*shows)
        })
        .collect()
}

fn function_arg_items(call: &str, index: usize, columns: &[Column]) -> Vec<Completion> {
    match catalog::function(call) {
        Some(f) if f.kind != FunctionKind::Scalar => {
            if index == 0 { column_items(columns, true) } else { Vec::new() }
        }
        _ => expression_items(columns),
    }
}

/// Column names for the `"…"` around the caret, best matches first.
fn quoted_columns(text: &str, token: &Token, offset: usize, columns: &[Column]) -> Vec<Completion> {
    let typed = text[token.span.start + 1..offset].to_lowercase();
    // An open quote replaces only through the word at the caret.
    let range = if token.terminated {
        token.span.clone()
    } else {
        let word = text[offset..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(text.len() - offset);
        token.span.start..offset + word
    };
    let mut matches: Vec<(usize, usize, &Column)> = columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let name = c.name.to_lowercase();
            let rank = if name.starts_with(&typed) {
                0
            } else if name.contains(&typed) {
                1
            } else if subsequence(&typed, &name) {
                2
            } else {
                return None;
            };
            Some((rank, i, c))
        })
        .collect();
    matches.sort_by_key(|(rank, i, _)| (*rank, *i));
    matches
        .into_iter()
        .map(|(rank, i, c)| {
            let mut item = Completion::new(c.name.clone(), CompletionKind::Column, format!("{rank}{i:04}"))
                .insert(quote_column(&c.name))
                .detail(kind_name(c.kind));
            item.range = Some(range.clone());
            item
        })
        .collect()
}

fn subsequence(needle: &str, haystack: &str) -> bool {
    let mut rest = haystack.chars();
    needle.chars().all(|c| rest.any(|h| h == c))
}

/// The `#` picker: "Pick a color…", then palette colors whose hex starts
/// with what's typed.
fn picker(typed: &str, range: Range<usize>) -> Vec<Completion> {
    let digits = typed.trim_start_matches('#').to_ascii_lowercase();
    let mut out = vec![pick_item(Some(range.clone()))];
    if let Some(color) = Color::parse_hex(&digits).filter(|_| digits.len() >= 3) {
        let mut item = Completion::new(format!("#{digits}"), CompletionKind::Color, "0").detail("as typed");
        item.swatch = Some(color);
        item.range = Some(range.clone());
        out.push(item);
    }
    let palette = colors::PALETTE.iter().chain(colors::NAMED.iter());
    for (i, (name, color)) in palette.enumerate() {
        let hex = color.to_hex();
        if color.a == 0 || !hex[1..].starts_with(&digits) || out.iter().any(|o| o.label == hex) {
            continue;
        }
        let mut item = Completion::new(hex, CompletionKind::Color, format!("1{i:03}")).detail(*name);
        item.swatch = Some(*color);
        item.range = Some(range.clone());
        out.push(item);
        if out.len() > 60 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<Column> {
        vec![Column::new("sales", ValueKind::Number), Column::new("region", ValueKind::Text)]
    }

    fn labels(text: &str) -> Vec<String> {
        let offset = text.find('|').expect("caret");
        let text = text.replace('|', "");
        complete(&text, offset, &columns()).into_iter().map(|c| c.label).collect()
    }

    #[test]
    fn display_completes_values_then_formats() {
        assert!(labels("display(|").contains(&"c".into()));
        let formats = labels("display(c, |");
        assert!(formats.contains(&"'#,##0.00'".into()));
        assert!(formats.contains(&"'YYYY-MM-DD'".into()));
        assert!(labels("display(c, '0', |").contains(&"blank=".into()));
    }

    #[test]
    fn rules_start_with_if_or_formatting() {
        let l = labels("|");
        assert!(l.contains(&"if".into()));
        assert!(l.contains(&"bg".into()));
        assert!(l.contains(&"bar".into()));
        assert!(!l.contains(&"c".into()));
    }

    #[test]
    fn conditions_offer_the_cell_columns_and_functions() {
        let l = labels("if |");
        assert_eq!(l[0], "c");
        assert!(l.contains(&"sales".into()));
        assert!(l.contains(&"duplicate".into()));
        assert!(!l.contains(&"bg".into()));
        let partial = labels("if dup|");
        assert!(partial.contains(&"duplicate".into()));
    }

    #[test]
    fn after_a_value_come_operators_and_then() {
        let l = labels("if c |");
        assert!(l.contains(&">".into()));
        assert!(l.contains(&"is null".into()));
        assert!(l.contains(&"then".into()));
        assert!(l.contains(&"and".into()));
        assert!(!l.contains(&"sales".into()));
    }

    #[test]
    fn after_then_comes_formatting_and_after_bg_colors() {
        assert!(labels("if c > 1 then |").contains(&"bold".into()));
        let colors = labels("if c > 1 then bg: |");
        assert!(colors.contains(&"red".into()));
        assert!(colors.contains(&"scale".into()));
        assert_eq!(colors[0], "Pick a color…");
        let more = labels("if c > 1 then bold, |");
        assert!(more.contains(&"italic".into()));
        let next = labels("if c > 1 then bold |");
        assert!(next.contains(&"else".into()));
        assert!(!next.contains(&"bg".into()));
    }

    #[test]
    fn calls_complete_their_arguments() {
        assert!(labels("icons(|").contains(&"arrows".into()));
        assert!(labels("bar(green, |").contains(&"fill=".into()));
        assert_eq!(labels("bar(green, fill=|"), ["solid", "gradient"]);
        assert_eq!(labels("if rank(|"), ["c", "sales", "region"]);
        assert_eq!(labels("sparkline(\"sales\", |")[..2], ["sales", "region"]);
    }

    #[test]
    fn quotes_complete_columns_over_their_range() {
        let items = complete("if c > \"reg\"", 10, &columns());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].insert, "\"region\"");
        assert_eq!(items[0].range, Some(7..12));
        assert!(complete("if c = 'reg", 11, &columns()).is_empty());
    }

    #[test]
    fn dots_complete_struct_fields() {
        let place = Column::new("place", ValueKind::Struct)
            .with_fields(vec![Column::new("city", ValueKind::Text), Column::new("zip code", ValueKind::Number)]);
        let columns = vec![
            Column::new("info", ValueKind::Struct)
                .with_fields(vec![Column::new("kind", ValueKind::Text), place.clone()]),
            Column::new("stops", ValueKind::List).with_item(Column::new("", ValueKind::Struct).with_fields(vec![place])),
            Column::new("sales", ValueKind::Number),
        ];
        let at = |text: &str| {
            let offset = text.find('|').expect("caret");
            complete(&text.replace('|', ""), offset, &columns)
        };
        let labels = |text: &str| at(text).into_iter().map(|c| c.label).collect::<Vec<_>>();
        assert_eq!(labels("if \"info\".|"), ["kind", "place"]);
        assert_eq!(labels("if \"info\".place.|"), ["city", "zip code"]);
        assert_eq!(labels("if \"stops\"[0].|"), ["place"]);
        assert_eq!(labels("if \"stops\"[0].place.ci|"), ["city", "zip code"]);
        // `c` could be any of the formatted columns.
        assert_eq!(labels("if c.|"), ["kind", "place"]);
        assert!(labels("if \"sales\".|").is_empty());
        let zip = at("if \"info\".place.z|").into_iter().find(|c| c.label == "zip code").unwrap();
        assert_eq!(zip.insert, "\"zip code\"");
        // A quoted field replaces the quotes.
        let quoted = at("if \"info\".place.\"zi|");
        assert_eq!(quoted.len(), 1);
        assert_eq!((quoted[0].insert.as_str(), quoted[0].range.clone()), ("\"zip code\"", Some(16..19)));
    }

    #[test]
    fn hashes_open_the_picker() {
        let items = complete("bg: #ff", 7, &columns());
        assert!(items[0].pick_color);
        assert!(items.iter().skip(1).all(|i| i.label.starts_with("#ff")));
        assert_eq!(items[1].range, Some(4..7));
        assert_eq!(hex_at("bg: #ff, bold", 6), Some(4..7));
    }
}
