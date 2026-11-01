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
    tokens.iter().enumerate().map(|(i, t)| (t.span.clone(), classify(text, &tokens, i))).collect()
}

fn next_kind(tokens: &[Token], i: usize) -> Option<TokenKind> {
    tokens[i + 1..].iter().find(|t| t.kind != TokenKind::Comment).map(|t| t.kind)
}

fn classify(text: &str, tokens: &[Token], i: usize) -> Class {
    let t = &tokens[i];
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
