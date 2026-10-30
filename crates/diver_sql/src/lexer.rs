//! The lexer: text in, tokens out. Never fails, and the tokens cover every byte.
//!
//! Words all lex as `IDENT`; the parser decides which ones are keywords.

use rowan::TextSize;

use crate::SyntaxKind;
use crate::SyntaxKind as K;

/// A token: its kind and its length in bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: TextSize,
}

/// Lexes `text` into tokens, trivia included.
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        bytes: text.as_bytes(),
        pos: 0,
    };
    let mut tokens = Vec::with_capacity(text.len() / 3 + 1);
    while lexer.pos < lexer.bytes.len() {
        let start = lexer.pos;
        let kind = lexer.next_kind();
        debug_assert!(lexer.pos > start);
        tokens.push(Token {
            kind,
            len: TextSize::new((lexer.pos - start) as u32),
        });
    }
    tokens
}

struct Lexer<'a> {
    bytes: &'a [u8],
    pos: usize,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

impl Lexer<'_> {
    fn peek(&self, n: usize) -> u8 {
        self.bytes.get(self.pos + n).copied().unwrap_or(0)
    }

    fn rest(&self) -> &[u8] {
        &self.bytes[self.pos..]
    }

    fn eat_while(&mut self, f: impl Fn(u8) -> bool) {
        while self.pos < self.bytes.len() && f(self.bytes[self.pos]) {
            self.pos += 1;
        }
    }

    fn next_kind(&mut self) -> SyntaxKind {
        let b = self.peek(0);
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c => {
                self.eat_while(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c));
                K::WHITESPACE
            }
            b'-' if self.peek(1) == b'-' => {
                self.eat_while(|b| b != b'\n' && b != b'\r');
                K::LINE_COMMENT
            }
            b'/' if self.peek(1) == b'*' => self.block_comment(),
            b'\'' => {
                self.pos += 1;
                self.quoted(b'\'', false, K::STRING, K::UNTERMINATED_STRING)
            }
            b'"' => {
                self.pos += 1;
                self.quoted(b'"', false, K::QUOTED_IDENT, K::UNTERMINATED_QUOTED_IDENT)
            }
            b'e' | b'E' if self.peek(1) == b'\'' => {
                self.pos += 2;
                self.quoted(b'\'', true, K::ESCAPE_STRING, K::UNTERMINATED_STRING)
            }
            b'x' | b'X' | b'b' | b'B' if self.peek(1) == b'\'' => {
                self.pos += 2;
                self.quoted(b'\'', false, K::BLOB_STRING, K::UNTERMINATED_STRING)
            }
            b'0'..=b'9' => self.number(),
            b'.' if self.peek(1).is_ascii_digit() => self.number(),
            b'$' => self.dollar(),
            b'?' => {
                self.pos += 1;
                self.eat_while(|b| b.is_ascii_digit());
                K::PARAM
            }
            _ if is_ident_start(b) => {
                self.pos += 1;
                self.eat_while(is_ident_continue);
                K::IDENT
            }
            _ => self.punct(),
        }
    }

    fn block_comment(&mut self) -> SyntaxKind {
        self.pos += 2;
        let mut depth = 1;
        while self.pos < self.bytes.len() {
            if self.rest().starts_with(b"/*") {
                depth += 1;
                self.pos += 2;
            } else if self.rest().starts_with(b"*/") {
                depth -= 1;
                self.pos += 2;
                if depth == 0 {
                    return K::BLOCK_COMMENT;
                }
            } else {
                self.pos += 1;
            }
        }
        K::UNTERMINATED_BLOCK_COMMENT
    }

    /// The body of a quoted token, after its opening quote. A doubled quote
    /// stands for one; with `backslash`, so does `\` and the next byte.
    fn quoted(
        &mut self,
        quote: u8,
        backslash: bool,
        done: SyntaxKind,
        open: SyntaxKind,
    ) -> SyntaxKind {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if backslash && b == b'\\' {
                self.pos = (self.pos + 2).min(self.bytes.len());
            } else if b == quote {
                if self.peek(1) == quote {
                    self.pos += 2;
                } else {
                    self.pos += 1;
                    return done;
                }
            } else {
                self.pos += 1;
            }
        }
        open
    }

    fn number(&mut self) -> SyntaxKind {
        if self.peek(0) == b'0'
            && matches!(self.peek(1), b'x' | b'X')
            && self.peek(2).is_ascii_hexdigit()
        {
            self.pos += 2;
            self.eat_while(|b| b.is_ascii_hexdigit() || b == b'_');
            return K::INT_NUMBER;
        }
        if self.peek(0) == b'0'
            && matches!(self.peek(1), b'b' | b'B')
            && matches!(self.peek(2), b'0' | b'1')
        {
            self.pos += 2;
            self.eat_while(|b| b == b'0' || b == b'1' || b == b'_');
            return K::INT_NUMBER;
        }
        let digits = |b: u8| b.is_ascii_digit() || b == b'_';
        let mut float = false;
        self.eat_while(digits);
        // `1.5`, `1.`, `.5`, but not `1..2` or a method-style `1.foo`
        if self.peek(0) == b'.' && self.peek(1) != b'.' && !is_ident_start(self.peek(1)) {
            float = true;
            self.pos += 1;
            self.eat_while(digits);
        }
        if matches!(self.peek(0), b'e' | b'E') {
            let sign = usize::from(matches!(self.peek(1), b'+' | b'-'));
            if self.peek(1 + sign).is_ascii_digit() {
                float = true;
                self.pos += 1 + sign;
                self.eat_while(digits);
            }
        }
        if float {
            K::FLOAT_NUMBER
        } else {
            K::INT_NUMBER
        }
    }

    /// `$1`, `$name`, `$$…$$` or `$tag$…$tag$`.
    fn dollar(&mut self) -> SyntaxKind {
        let start = self.pos;
        self.pos += 1;
        if self.peek(0).is_ascii_digit() {
            self.eat_while(|b| b.is_ascii_digit());
            return K::PARAM;
        }
        let tag_start = self.pos;
        if is_ident_start(self.peek(0)) {
            self.eat_while(|b| is_ident_continue(b) && b != b'$');
        }
        if self.peek(0) != b'$' {
            return if self.pos > tag_start {
                K::PARAM
            } else {
                K::ERROR_TOKEN
            };
        }
        self.pos += 1;
        let delim = &self.bytes[start..self.pos];
        let body = &self.bytes[self.pos..];
        match body.windows(delim.len()).position(|w| w == delim) {
            Some(i) => {
                self.pos += i + delim.len();
                K::DOLLAR_STRING
            }
            None => {
                self.pos = self.bytes.len();
                K::UNTERMINATED_STRING
            }
        }
    }

    fn punct(&mut self) -> SyntaxKind {
        const OPS: &[(&[u8], SyntaxKind)] = &[
            (b"!~~*", K::NOT_TILDE2_STAR),
            (b"->>", K::LONG_ARROW),
            (b"!~~", K::NOT_TILDE2),
            (b"~~*", K::TILDE2_STAR),
            (b"!~*", K::NOT_TILDE_STAR),
            (b"::", K::DOUBLE_COLON),
            (b":=", K::COLON_EQ),
            (b"->", K::ARROW),
            (b"=>", K::FAT_ARROW),
            (b"**", K::STAR2),
            (b"//", K::SLASH2),
            (b"||", K::PIPE2),
            (b"&&", K::AMP2),
            (b"@>", K::AT_GT),
            (b"<@", K::LT_AT),
            (b"<<", K::SHL),
            (b">>", K::SHR),
            (b"<=", K::LTE),
            (b">=", K::GTE),
            (b"<>", K::NEQ),
            (b"!=", K::NEQ),
            (b"==", K::EQ2),
            (b"~~", K::TILDE2),
            (b"~*", K::TILDE_STAR),
            (b"!~", K::NOT_TILDE),
            (b"^@", K::CARET_AT),
            (b"(", K::L_PAREN),
            (b")", K::R_PAREN),
            (b"[", K::L_BRACK),
            (b"]", K::R_BRACK),
            (b"{", K::L_CURLY),
            (b"}", K::R_CURLY),
            (b",", K::COMMA),
            (b";", K::SEMICOLON),
            (b".", K::DOT),
            (b":", K::COLON),
            (b"+", K::PLUS),
            (b"-", K::MINUS),
            (b"*", K::STAR),
            (b"/", K::SLASH),
            (b"%", K::PERCENT),
            (b"^", K::CARET),
            (b"=", K::EQ),
            (b"<", K::LT),
            (b">", K::GT),
            (b"|", K::PIPE),
            (b"&", K::AMP),
            (b"~", K::TILDE),
            (b"!", K::BANG),
            (b"@", K::AT),
            (b"#", K::HASH),
        ];
        let rest = self.rest();
        for (text, kind) in OPS {
            if rest.starts_with(text) {
                self.pos += text.len();
                return *kind;
            }
        }
        // Anything else is one character of junk; keep whole UTF-8 sequences.
        self.pos += 1;
        self.eat_while(|b| (0x80..0xc0).contains(&b));
        K::ERROR_TOKEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex(text: &str) -> Vec<(SyntaxKind, &str)> {
        let mut pos = 0;
        tokenize(text)
            .into_iter()
            .map(|t| {
                let len = usize::from(t.len);
                let s = &text[pos..pos + len];
                pos += len;
                (t.kind, s)
            })
            .filter(|(k, _)| *k != K::WHITESPACE)
            .collect()
    }

    #[test]
    fn covers_every_byte() {
        for text in [
            "",
            "select",
            "'abc",
            "/* /* */",
            "$a$ x",
            "é ü",
            "\u{0}\u{1}",
            "1e",
            "0x",
            "$",
        ] {
            let total: u32 = tokenize(text).iter().map(|t| u32::from(t.len)).sum();
            assert_eq!(total as usize, text.len(), "{text:?}");
        }
    }

    #[test]
    fn kinds() {
        use SyntaxKind as K;
        let cases: &[(&str, SyntaxKind)] = &[
            ("'it''s'", K::STRING),
            ("E'a\\'b'", K::ESCAPE_STRING),
            ("x'0F'", K::BLOB_STRING),
            ("$$a;b$$", K::DOLLAR_STRING),
            ("$fn$ body $fn$", K::DOLLAR_STRING),
            ("$1", K::PARAM),
            ("$name", K::PARAM),
            ("?", K::PARAM),
            ("\"a\"\"b\"", K::QUOTED_IDENT),
            ("\"open", K::UNTERMINATED_QUOTED_IDENT),
            ("'open", K::UNTERMINATED_STRING),
            ("/* a /* b */ c */", K::BLOCK_COMMENT),
            ("/* a /* b */", K::UNTERMINATED_BLOCK_COMMENT),
            ("-- x", K::LINE_COMMENT),
            ("1_000", K::INT_NUMBER),
            ("0x1F", K::INT_NUMBER),
            ("0b101", K::INT_NUMBER),
            ("1.5", K::FLOAT_NUMBER),
            (".5", K::FLOAT_NUMBER),
            ("1e10", K::FLOAT_NUMBER),
            ("1.5e-3", K::FLOAT_NUMBER),
            ("café", K::IDENT),
            ("->>", K::LONG_ARROW),
            ("!~~*", K::NOT_TILDE2_STAR),
            ("::", K::DOUBLE_COLON),
        ];
        for (text, kind) in cases {
            assert_eq!(lex(text), vec![(*kind, *text)], "{text}");
        }
    }

    #[test]
    fn sequences() {
        assert_eq!(
            lex("a[1:2]"),
            vec![
                (K::IDENT, "a"),
                (K::L_BRACK, "["),
                (K::INT_NUMBER, "1"),
                (K::COLON, ":"),
                (K::INT_NUMBER, "2"),
                (K::R_BRACK, "]")
            ]
        );
        assert_eq!(
            lex("x::int"),
            vec![(K::IDENT, "x"), (K::DOUBLE_COLON, "::"), (K::IDENT, "int")]
        );
        assert_eq!(
            lex("1..2"),
            vec![(K::INT_NUMBER, "1"), (K::DOT, "."), (K::FLOAT_NUMBER, ".2")]
        );
        assert_eq!(lex("t.1"), vec![(K::IDENT, "t"), (K::FLOAT_NUMBER, ".1")]);
        assert_eq!(
            lex("a-->b"),
            vec![(K::IDENT, "a"), (K::LINE_COMMENT, "-->b")]
        );
    }
}
