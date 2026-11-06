//! Highlighting, hovers and signature help over the tokens.

use std::ops::Range;

use crate::catalog::{self, Form};
use crate::colors::Color;
use crate::lexer::{Token, TokenKind, tokenize, unquote};
use crate::complete::kind_name;
use crate::value::Column;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Keyword,
    /// `c`.
    Cell,
    Column,
    Function,
    /// `bg`, `bold`, `bar`: the formatting.
    Action,
    String,
    Number,
    Color,
    /// Icon names, line styles and other fixed words.
    Value,
    Operator,
    Comment,
    Error,
}

/// What each token is, and the color of each color literal or name.
pub fn highlight(text: &str) -> Vec<(Range<usize>, Class)> {
    let tokens = tokenize(text);
    let mut out = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if is_template(text, &tokens, i) {
            template_classes(text, t, &mut out);
        } else {
            out.push((t.span.clone(), classify(text, &tokens, i)));
        }
    }
    out
}

/// Whether token `i` is a tooltip's template: text right after `tooltip(`.
pub(crate) fn is_template(text: &str, tokens: &[Token], i: usize) -> bool {
    tokens[i].kind == TokenKind::Str
        && i >= 2
        && tokens[i - 1].kind == TokenKind::LParen
        && tokens[i - 2].text(text).eq_ignore_ascii_case("tooltip")
}

/// Where byte `i` of a template's unquoted text is in the rules.
pub(crate) fn template_offset(token: &Token, unquoted: &str, i: usize) -> usize {
    token.span.start + 1 + i + unquoted[..i.min(unquoted.len())].matches('\'').count()
}

/// A template's text as text, and its fields' values as rules.
fn template_classes(text: &str, token: &Token, out: &mut Vec<(Range<usize>, Class)>) {
    use crate::template::{Piece, split};
    let inner = unquote(token.text(text));
    let Ok(pieces) = split(&inner) else {
        out.push((token.span.clone(), Class::String));
        return;
    };
    let at = |i: usize| template_offset(token, &inner, i);
    // Text runs from `from` to each field, which colors its own parts.
    let mut from = token.span.start;
    for piece in pieces {
        let Piece::Field(field) = piece else { continue };
        out.push((from..at(field.span.start), Class::String));
        out.push((at(field.span.start)..at(field.span.start + 1), Class::Operator));
        let (value, offset) = &field.value;
        for (range, class) in highlight(value) {
            out.push((at(offset + range.start)..at(offset + range.end), class));
        }
        if let Some((format, offset)) = &field.format {
            out.push((at(offset - 1)..at(offset + format.len()), Class::Number));
        }
        if let Some((style, offset)) = &field.style {
            out.push((at(offset - 1)..at(*offset), Class::Operator));
            for word in style.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()) {
                let start = offset + (word.as_ptr() as usize - style.as_ptr() as usize);
                let class = if catalog::TOOLTIP_STYLES.contains(&word.to_ascii_lowercase().as_str()) {
                    Class::Action
                } else if Color::named(word).is_some() || Color::parse_hex(word).is_some() {
                    Class::Color
                } else {
                    Class::Error
                };
                out.push((at(start)..at(start + word.len()), class));
            }
        }
        out.push((at(field.span.end - 1)..at(field.span.end), Class::Operator));
        from = at(field.span.end);
    }
    out.push((from..token.span.end, Class::String));
}

fn next_kind(tokens: &[Token], i: usize) -> Option<TokenKind> {
    tokens[i + 1..].iter().find(|t| t.kind != TokenKind::Comment).map(|t| t.kind)
}

/// Whether the token comes right after a `.`: a struct field's name.
fn after_dot(tokens: &[Token], i: usize) -> bool {
    i > 0 && tokens[i - 1].kind == TokenKind::Dot
}

fn classify(text: &str, tokens: &[Token], i: usize) -> Class {
    let t = &tokens[i];
    if after_dot(tokens, i) && matches!(t.kind, TokenKind::Ident | TokenKind::Column) {
        return Class::Column;
    }
    match t.kind {
        TokenKind::Comment => Class::Comment,
        TokenKind::Str => Class::String,
        TokenKind::Column => Class::Column,
        TokenKind::Number => Class::Number,
        TokenKind::Hex if Color::parse_hex(t.text(text)).is_some() => Class::Color,
        TokenKind::Hex | TokenKind::Unknown => Class::Error,
        TokenKind::Ident => {
            let word = t.text(text).to_ascii_lowercase();
            let next = next_kind(tokens, i);
            if word == "c" {
                Class::Cell
            } else if catalog::is_reserved(&word) {
                Class::Keyword
            } else if next == Some(TokenKind::LParen) {
                if catalog::function(&word).is_some() || word == "rgb" {
                    Class::Function
                } else {
                    Class::Action
                }
            } else if next == Some(TokenKind::Eq) && is_named_arg(&word) {
                Class::Keyword
            } else if catalog::action(&word).is_some() && !after_colon(tokens, i) {
                Class::Action
            } else if Color::named(&word).is_some() {
                Class::Color
            } else {
                Class::Value
            }
        }
        _ => Class::Operator,
    }
}

fn is_named_arg(word: &str) -> bool {
    catalog::ACTIONS.iter().any(|a| catalog::named_args(a.name).iter().any(|(n, ..)| *n == word))
}

/// Whether the token is among a property's values (after `name:`).
fn after_colon(tokens: &[Token], i: usize) -> bool {
    for t in tokens[..i].iter().rev() {
        match t.kind {
            TokenKind::Colon => return true,
            TokenKind::Comma | TokenKind::Semi | TokenKind::LParen | TokenKind::RParen => return false,
            _ => {}
        }
    }
    false
}

/// The color that each color token shows, for swatches next to it.
pub fn color_spans(text: &str) -> Vec<(Range<usize>, Color)> {
    let tokens = tokenize(text);
    tokens
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let color = match t.kind {
                TokenKind::Hex => Color::parse_hex(t.text(text))?,
                TokenKind::Ident if classify(text, &tokens, i) == Class::Color => Color::named(t.text(text))?,
                _ => return None,
            };
            Some((t.span.clone(), color))
        })
        .collect()
}

/// Help for the word at `offset`.
pub fn hover(text: &str, offset: usize, columns: &[Column]) -> Option<String> {
    let tokens = tokenize(text);
    let i = tokens.iter().position(|t| t.span.start <= offset && offset < t.span.end)?;
    let t = &tokens[i];
    let word = t.text(text);
    let lower = word.to_ascii_lowercase();
    if after_dot(&tokens, i) && matches!(t.kind, TokenKind::Ident | TokenKind::Column) {
        let name = if t.kind == TokenKind::Column { unquote(word) } else { word.to_string() };
        return Some(format!("Field `{name}`\nThe struct field of that name; blank when there's none."));
    }
    match t.kind {
        TokenKind::Column => {
            let name = unquote(word);
            Some(match columns.iter().find(|c| c.name.eq_ignore_ascii_case(&name)) {
                Some(column) => {
                    format!("Column \"{}\" ({})\nIts value in the same row.", column.name, kind_name(column.kind))
                }
                None => format!("No column \"{name}\"."),
            })
        }
        TokenKind::Hex => Color::parse_hex(word).map(|c| format!("Color {}", c.to_hex())),
        TokenKind::Ident => {
            if let Some(doc) = catalog::keyword(&lower) {
                return Some(format!("{lower}\n{doc}"));
            }
            let call = next_kind(&tokens, i) == Some(TokenKind::LParen);
            if let Some(f) = catalog::function(&lower).filter(|_| call) {
                return Some(format!("{}\n{}", f.signature, f.doc));
            }
            if let Some(a) = catalog::action(&lower).filter(|a| a.form != Form::Call || call || !after_colon(&tokens, i)) {
                return Some(format!("{}\n{}", a.signature, a.doc));
            }
            if let Some(color) = Color::named(&lower) {
                return Some(format!("{lower}: {}", color.to_hex()));
            }
            if let Some((name, _, doc)) = catalog::icon_set(&lower) {
                return Some(format!("Icon set {name}\n{doc}"));
            }
            if catalog::icon(&lower).is_some() {
                return Some(format!("Icon {lower}"));
            }
            None
        }
        _ => None,
    }
}

/// The call around `offset`: its signature, the argument being typed (as a
/// byte range in the signature) and its documentation.
pub fn signature(text: &str, offset: usize) -> Option<(String, Option<Range<usize>>, String)> {
    let tokens: Vec<Token> = tokenize(text).into_iter().filter(|t| t.span.end <= offset).collect();
    let mut depth = 0usize;
    let mut commas = 0usize;
    for (i, t) in tokens.iter().enumerate().rev() {
        match t.kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen if depth > 0 => depth -= 1,
            TokenKind::LParen => {
                let name = tokens.get(i.checked_sub(1)?)?.text(text).to_ascii_lowercase();
                let (signature, doc) = match catalog::function(&name) {
                    Some(f) => (f.signature, f.doc),
                    None => {
                        let a = catalog::action(&name).filter(|a| a.form == Form::Call)?;
                        (a.signature, a.doc)
                    }
                };
                let active = argument_ranges(signature).get(commas).cloned();
                return Some((signature.to_string(), active, doc.to_string()));
            }
            TokenKind::Comma if depth == 0 => commas += 1,
            TokenKind::Semi => return None,
            _ => {}
        }
    }
    None
}

fn argument_ranges(signature: &str) -> Vec<Range<usize>> {
    let Some(open) = signature.find('(') else { return Vec::new() };
    let close = signature.rfind(')').unwrap_or(signature.len());
    let mut out = Vec::new();
    let mut start = open + 1;
    for (i, ch) in signature[..close].char_indices().skip_while(|(i, _)| *i <= open) {
        if ch == ',' {
            out.push(start..i);
            start = i + 1;
        }
    }
    out.push(start..close);
    out.into_iter()
        .map(|r| {
            let lead = signature[r.clone()].len() - signature[r.clone()].trim_start().len();
            r.start + lead..r.end
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_classified_by_place() {
        let text = "if c > \"a\" then bg: red, bar(green, fill=solid), border: 1px dashed";
        let classes: Vec<(String, Class)> =
            highlight(text).into_iter().map(|(r, c)| (text[r].to_string(), c)).collect();
        let class = |word: &str| classes.iter().find(|(w, _)| w == word).unwrap().1;
        assert_eq!(class("if"), Class::Keyword);
        assert_eq!(class("c"), Class::Cell);
        assert_eq!(class("\"a\""), Class::Column);
        assert_eq!(class("bg"), Class::Action);
        assert_eq!(class("red"), Class::Color);
        assert_eq!(class("bar"), Class::Action);
        assert_eq!(class("fill"), Class::Keyword);
        assert_eq!(class("solid"), Class::Value);
        assert_eq!(class("dashed"), Class::Value);
        assert_eq!(color_spans("bg: red, text: #00f").len(), 2);
        // A field named like a color is still a field.
        let fields = highlight("if c.red[0] > 1 then bold");
        assert_eq!(fields[2..6].iter().map(|(_, c)| *c).collect::<Vec<_>>(), [Class::Operator, Class::Column, Class::Operator, Class::Number]);
        assert!(hover("if c.\"red\" > 1", 7, &[]).unwrap().starts_with("Field `red`"));
    }

    #[test]
    fn template_fields_are_classified_as_rules() {
        let text = r#"tooltip('it''s {"sales" * 2:,.2f|red bold} ok', bg=red)"#;
        let classes: Vec<(String, Class)> =
            highlight(text).into_iter().map(|(r, c)| (text[r].to_string(), c)).collect();
        let expected = [
            ("'it''s ", Class::String),
            ("{", Class::Operator),
            ("\"sales\"", Class::Column),
            ("2", Class::Number),
            (":,.2f", Class::Number),
            ("|", Class::Operator),
            ("red", Class::Color),
            ("bold", Class::Action),
            ("}", Class::Operator),
            (" ok'", Class::String),
            ("bg", Class::Keyword),
        ];
        for (word, class) in expected {
            assert!(classes.contains(&(word.to_string(), class)), "{word}: {classes:?}");
        }
        // Text elsewhere is still plain text.
        assert!(highlight("display('{c}')").iter().any(|(r, c)| *r == (8..13) && *c == Class::String));
    }

    #[test]
    fn hovers_and_signatures_explain() {
        let columns = [Column::new("sales", crate::ValueKind::Number)];
        assert!(hover("if duplicate(c) then bold", 5, &columns).unwrap().starts_with("duplicate(column)"));
        assert_eq!(hover("x \"sales\"", 3, &columns).unwrap(), "Column \"sales\" (number)\nIts value in the same row.");
        let (label, active, _) = signature("bar(green, ", 11).unwrap();
        assert_eq!(&label[active.unwrap()], "[<negative color>]");
        assert!(signature("bar(green); x", 13).is_none());
    }
}
