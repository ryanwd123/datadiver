//! What can be typed at the cursor.
//!
//! The statement under the cursor is parsed with a hole: the partial word is
//! replaced by a `CURSOR` token (or one is inserted), and every check the
//! parser makes at the hole is recorded. See `PLAN.md`.

use rowan::{TextRange, TextSize};

use crate::lexer::{self, Token};
use crate::scope::{self, Scope};
use crate::{Kw, ParseOptions, SyntaxKind, SyntaxNode, parser};

/// One thing the parser would accept at the cursor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Expected {
    Keyword(Kw),
    /// Keywords that only make sense together: `GROUP BY`, `IF NOT EXISTS`.
    Phrase(&'static [Kw]),
    Punct(SyntaxKind),
    DatabaseName,
    SchemaName {
        database: Option<Name>,
    },
    /// `main.|`, `db.main.|`
    TableRef {
        qualifier: Vec<Name>,
    },
    /// `o.|`, resolved through the scope
    ColumnRef {
        qualifier: Vec<Name>,
    },
    FunctionName {
        kind: FunctionKind,
        qualifier: Vec<Name>,
    },
    TypeName,
    CteName,
    NewName(NewNameKind),
    FilePath,
    SettingName,
    ExtensionName,
    PragmaName,
    /// An expression may start here.
    Expr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionKind {
    Scalar,
    Aggregate,
    Window,
    Table,
    Macro,
    Any,
}

/// What kind of name is being made up at the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NewNameKind {
    Table,
    View,
    Column,
    Alias,
    Cte,
    Macro,
    Type,
    Schema,
    Database,
    Window,
    Variable,
    Other,
}

/// An identifier as written: its text without quotes, and whether it was quoted.
///
/// DuckDB matches identifiers case-insensitively, quoted or not, so
/// comparisons fold case.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Name {
    pub text: String,
    pub quoted: bool,
}

impl Name {
    pub fn new(text: impl Into<String>) -> Name {
        Name {
            text: text.into(),
            quoted: false,
        }
    }

    /// The name a token spells: quotes removed and `""` unescaped.
    pub fn from_token(kind: SyntaxKind, text: &str) -> Name {
        match kind {
            SyntaxKind::QUOTED_IDENT => {
                let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or("");
                Name {
                    text: inner.replace("\"\"", "\""),
                    quoted: true,
                }
            }
            SyntaxKind::UNTERMINATED_QUOTED_IDENT => Name {
                text: text.get(1..).unwrap_or("").replace("\"\"", "\""),
                quoted: true,
            },
            SyntaxKind::CURSOR if text.starts_with('"') => Name::from_token(
                if text.len() > 1 && text.ends_with('"') {
                    SyntaxKind::QUOTED_IDENT
                } else {
                    SyntaxKind::UNTERMINATED_QUOTED_IDENT
                },
                text,
            ),
            _ => Name {
                text: text.to_string(),
                quoted: false,
            },
        }
    }

    /// Case-insensitive comparison, as DuckDB resolves names.
    pub fn matches(&self, other: &str) -> bool {
        eq_fold(&self.text, other)
    }
}

pub(crate) fn eq_fold(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// Everything completion knows about the cursor position, before any catalog lookup.
#[derive(Clone, Debug, Default)]
pub struct CompletionContext {
    /// Deduplicated, in the order the parser recorded them.
    pub expected: Vec<Expected>,
    /// The partial word under the cursor (empty at a space).
    pub replace_range: TextRange,
    /// Its text up to the cursor, unquoted.
    pub prefix: String,
    /// Whether the partial word is a quoted identifier.
    pub quoted: bool,
    pub statement: TextRange,
    /// Visible relations and aliases at the cursor.
    pub scope: Scope,
    /// The string token the cursor is in.
    pub in_string: Option<TextRange>,
    pub in_comment: bool,
}

/// Where the cursor is, in token terms.
enum At {
    /// Inside the word at this index.
    Word(usize),
    /// Inside the string at this index.
    String(usize),
    Comment,
    /// Inside a token that takes no completion (a number, an operator).
    Other,
    /// Between tokens; the hole goes before this index, after splitting
    /// whitespace the cursor is inside.
    Between(usize),
}

fn locate(tokens: &[Token], starts: &[TextSize], offset: TextSize) -> At {
    for (i, t) in tokens.iter().enumerate() {
        let start = starts[i];
        let end = start + t.len;
        if offset <= start {
            return At::Between(i);
        }
        if offset > end {
            continue;
        }
        // start < offset <= end
        let inside = offset < end;
        let k = t.kind;
        match k {
            SyntaxKind::LINE_COMMENT => return At::Comment,
            SyntaxKind::BLOCK_COMMENT if inside => return At::Comment,
            SyntaxKind::UNTERMINATED_BLOCK_COMMENT => return At::Comment,
            SyntaxKind::UNTERMINATED_STRING => return At::String(i),
            _ if k.is_string() && inside => return At::String(i),
            SyntaxKind::IDENT
            | SyntaxKind::QUOTED_IDENT
            | SyntaxKind::UNTERMINATED_QUOTED_IDENT => {
                return At::Word(i);
            }
            SyntaxKind::WHITESPACE if inside => return At::Between(i),
            _ if inside => return At::Other,
            _ => {}
        }
    }
    At::Between(tokens.len())
}

/// The tokens of `text`; an unterminated string or quoted identifier the
/// cursor is in ends at the cursor, so the text after it still lexes as SQL
/// (`SELECT "my co| FROM t` keeps its FROM clause).
fn tokens_at_cursor(text: &str, offset: TextSize) -> Vec<Token> {
    let mut tokens = lexer::tokenize(text);
    let mut start = TextSize::new(0);
    for i in 0..tokens.len() {
        let t = tokens[i];
        let end = start + t.len;
        let open = matches!(
            t.kind,
            SyntaxKind::UNTERMINATED_STRING | SyntaxKind::UNTERMINATED_QUOTED_IDENT
        );
        if open && start < offset && offset < end {
            tokens.truncate(i);
            tokens.push(Token {
                kind: t.kind,
                len: offset - start,
            });
            tokens.extend(lexer::tokenize(&text[usize::from(offset)..]));
            break;
        }
        start = end;
    }
    tokens
}

/// The completion context at `offset` (a byte offset into `text`).
pub fn complete(text: &str, offset: TextSize) -> CompletionContext {
    complete_with_options(text, offset, ParseOptions::default())
}

pub fn complete_with_options(
    text: &str,
    offset: TextSize,
    options: ParseOptions,
) -> CompletionContext {
    let offset = offset.min(TextSize::of(text));
    let tokens = tokens_at_cursor(text, offset);
    let mut starts = Vec::with_capacity(tokens.len());
    let mut pos = TextSize::new(0);
    for t in &tokens {
        starts.push(pos);
        pos += t.len;
    }

    // The statement: between the `;` before the cursor and the one after it.
    let mut first = 0;
    let mut last = tokens.len();
    if options.entry == crate::Entry::Script {
        for (i, t) in tokens.iter().enumerate() {
            if t.kind != SyntaxKind::SEMICOLON {
                continue;
            }
            if starts[i] + t.len <= offset {
                first = i + 1;
            } else {
                last = i;
                break;
            }
        }
    }
    let stmt_start = starts.get(first).copied().unwrap_or(TextSize::of(text));
    let stmt_end = if last < tokens.len() {
        starts[last]
    } else {
        TextSize::of(text)
    };
    let mut ctx = CompletionContext {
        statement: TextRange::new(stmt_start, stmt_end),
        replace_range: TextRange::empty(offset),
        ..CompletionContext::default()
    };

    let at = locate(&tokens, &starts, offset);
    let mut stmt_tokens: Vec<Token> = tokens[first..last].to_vec();
    let rel = |i: usize| i - first;
    let (hole, string_hole) = match at {
        At::Comment => {
            ctx.in_comment = true;
            return ctx;
        }
        At::Other => return ctx,
        At::Word(i) => {
            let start = starts[i];
            let range = TextRange::at(start, tokens[i].len);
            let typed = &text[TextRange::new(start, offset)];
            ctx.replace_range = range;
            ctx.quoted = typed.starts_with('"');
            ctx.prefix = Name::from_token(
                if ctx.quoted {
                    SyntaxKind::UNTERMINATED_QUOTED_IDENT
                } else {
                    SyntaxKind::IDENT
                },
                typed,
            )
            .text;
            stmt_tokens[rel(i)].kind = SyntaxKind::CURSOR;
            (Some(rel(i)), None)
        }
        At::String(i) => {
            let range = TextRange::at(starts[i], tokens[i].len);
            ctx.in_string = Some(range);
            // the content typed so far, after the opening quote
            let token_text = &text[range];
            let open = match token_text.find('\'') {
                Some(q) if q <= 1 => q + 1,
                _ => token_text[1..].find('$').map_or(1, |d| d + 2),
            };
            let content_start = (starts[i] + TextSize::new(open as u32)).min(offset);
            ctx.replace_range = TextRange::new(content_start, offset);
            ctx.prefix = text[ctx.replace_range].to_string();
            (None, Some(rel(i)))
        }
        At::Between(i) => {
            let i = i.clamp(first, last);
            let mut at = rel(i);
            if i < last && tokens[i].kind == SyntaxKind::WHITESPACE && starts[i] < offset {
                // split the whitespace around the cursor
                let before = offset - starts[i];
                stmt_tokens[at].len = before;
                stmt_tokens.insert(
                    at + 1,
                    Token {
                        kind: SyntaxKind::WHITESPACE,
                        len: tokens[i].len - before,
                    },
                );
                at += 1;
            }
            stmt_tokens.insert(
                at,
                Token {
                    kind: SyntaxKind::CURSOR,
                    len: TextSize::new(0),
                },
            );
            (Some(at), None)
        }
    };

    let stmt_text = &text[TextRange::new(stmt_start, stmt_end)];
    let out = parser::parse_tokens(stmt_text, &stmt_tokens, options, hole, string_hole);
    ctx.expected = out.expected;
    let root = SyntaxNode::new_root(out.green);
    ctx.scope = scope::scope_at(&root, offset - stmt_start);
    ctx
}
