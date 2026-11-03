//! Splits rule text into tokens. Nothing fails here: an unterminated quote or
//! a stray character becomes a token the parser reports.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    /// A number, possibly with a `px` or `%` unit attached.
    Number,
    /// `'text'`.
    Str,
    /// `"column name"`.
    Column,
    /// `#` and the letters and digits after it.
    Hex,
    LParen,
    RParen,
    /// `[` and `]`, around a list index.
    LBracket,
    RBracket,
    /// `.` before a struct field.
    Dot,
    Comma,
    Semi,
    Colon,
    /// `=` or `==`.
    Eq,
    /// `!=` or `<>`.
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    /// `--` to the end of the line.
    Comment,
    /// A character the language doesn't use.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
    /// For quotes: whether the closing quote is there.
    pub terminated: bool,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.span.clone()]
    }
}

/// The tokens of `text`, comments included.
pub fn tokenize(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let b = bytes[i];
        let mut terminated = true;
        let kind = match b {
            b if b.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                TokenKind::Comment
            }
            b'\'' | b'"' => {
                i += 1;
                terminated = false;
                // Names and text don't span lines, so an open quote ends
                // at the line's end instead of swallowing the rules below.
                while i < bytes.len() && bytes[i] != b'\n' {
                    if bytes[i] == b {
                        // A doubled quote is the quote itself.
                        if bytes.get(i + 1) == Some(&b) {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        terminated = true;
                        break;
                    }
                    i += 1;
                }
                if b == b'\'' { TokenKind::Str } else { TokenKind::Column }
            }
            b'#' => {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                TokenKind::Hex
            }
            b'0'..=b'9' => {
                number(bytes, &mut i);
                TokenKind::Number
            }
            b'.' if bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                number(bytes, &mut i);
                TokenKind::Number
            }
            b if b.is_ascii_alphabetic() || b == b'_' => {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                TokenKind::Ident
            }
            _ => {
                let two = bytes.get(i + 1).copied();
                let (kind, len) = match (b, two) {
                    (b'=', Some(b'=')) => (TokenKind::Eq, 2),
                    (b'!', Some(b'=')) | (b'<', Some(b'>')) => (TokenKind::Ne, 2),
                    (b'<', Some(b'=')) => (TokenKind::Le, 2),
                    (b'>', Some(b'=')) => (TokenKind::Ge, 2),
                    (b'=', _) => (TokenKind::Eq, 1),
                    (b'<', _) => (TokenKind::Lt, 1),
                    (b'>', _) => (TokenKind::Gt, 1),
                    (b'(', _) => (TokenKind::LParen, 1),
                    (b')', _) => (TokenKind::RParen, 1),
                    (b'[', _) => (TokenKind::LBracket, 1),
                    (b']', _) => (TokenKind::RBracket, 1),
                    (b'.', _) => (TokenKind::Dot, 1),
                    (b',', _) => (TokenKind::Comma, 1),
                    (b';', _) => (TokenKind::Semi, 1),
                    (b':', _) => (TokenKind::Colon, 1),
                    (b'+', _) => (TokenKind::Plus, 1),
                    (b'-', _) => (TokenKind::Minus, 1),
                    (b'*', _) => (TokenKind::Star, 1),
                    (b'/', _) => (TokenKind::Slash, 1),
                    (b'%', _) => (TokenKind::Percent, 1),
                    _ => {
                        let len = text[i..].chars().next().map_or(1, char::len_utf8);
                        (TokenKind::Unknown, len)
                    }
                };
                i += len;
                kind
            }
        };
        tokens.push(Token { kind, span: start..i, terminated });
    }
    tokens
}

/// Digits with an optional fraction, then a `px` or `%` unit written right
/// after them. `5 % 3` stays a remainder: a `%` followed by something that
/// could start an operand is the operator.
fn number(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i < bytes.len() && bytes[*i] == b'.' && bytes.get(*i + 1).is_some_and(u8::is_ascii_digit) {
        *i += 1;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    let rest = &bytes[*i..];
    let ident_after = |n: usize| rest.get(n).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case(b"px") && !ident_after(2) {
        *i += 2;
    } else if rest.first() == Some(&b'%') {
        let operand = rest[1..]
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'(' | b'"' | b'\'' | b'.' | b'_'));
        if !operand {
            *i += 1;
        }
    }
}

/// The text of a quoted token without its quotes, doubled quotes undone.
pub fn unquote(token: &str) -> String {
    let Some(quote) = token.chars().next() else {
        return String::new();
    };
    let inner = &token[1..];
    let inner = inner.strip_suffix(quote).unwrap_or(inner);
    let doubled = format!("{quote}{quote}");
    inner.replace(&doubled, &quote.to_string())
}

/// `name` as a column reference, quotes doubled.
pub fn quote_column(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(TokenKind, &str)> {
        tokenize(text).iter().map(|t| (t.kind, t.text(text))).collect()
    }

    #[test]
    fn rules_tokenize() {
        use TokenKind::*;
        assert_eq!(
            kinds("if c >= \"a \"\"b\"\"\" then bg:#fc0; -- note\nborder: 1px"),
            [
                (Ident, "if"),
                (Ident, "c"),
                (Ge, ">="),
                (Column, "\"a \"\"b\"\"\""),
                (Ident, "then"),
                (Ident, "bg"),
                (Colon, ":"),
                (Hex, "#fc0"),
                (Semi, ";"),
                (Comment, "-- note"),
                (Ident, "border"),
                (Colon, ":"),
                (Number, "1px"),
            ]
        );
    }

    #[test]
    fn indexes_and_fields_tokenize() {
        use TokenKind::*;
        assert_eq!(
            kinds("c[0].item \"col\".\"a b\" .5"),
            [
                (Ident, "c"),
                (LBracket, "["),
                (Number, "0"),
                (RBracket, "]"),
                (Dot, "."),
                (Ident, "item"),
                (Column, "\"col\""),
                (Dot, "."),
                (Column, "\"a b\""),
                (Number, ".5"),
            ]
        );
    }

    #[test]
    fn percent_is_a_unit_unless_an_operand_follows() {
        use TokenKind::*;
        assert_eq!(kinds("10%)"), [(Number, "10%"), (RParen, ")")]);
        assert_eq!(kinds("10 % 3"), [(Number, "10"), (Percent, "%"), (Number, "3")]);
        assert_eq!(kinds("10%3"), [(Number, "10"), (Percent, "%"), (Number, "3")]);
        assert_eq!(kinds("2pxx"), [(Number, "2"), (Ident, "pxx")]);
    }

    #[test]
    fn open_quotes_are_marked() {
        let tokens = tokenize("\"sal
bold");
        assert_eq!(tokens[0].span, 0..4);
        assert_eq!(tokens[0].kind, TokenKind::Column);
        assert!(!tokens[0].terminated);
        assert_eq!(unquote("\"a \"\"b\"\"\""), "a \"b\"");
        assert_eq!(unquote("\"open"), "open");
        assert_eq!(quote_column("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
