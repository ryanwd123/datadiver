//! Colors for SQL text, from the SQL editor's tokenizer.

use crate::domain::sql_editor::{TokenKind, tokenize};
use crate::ui::platform::Color;

pub const KEYWORD: Color = Color::rgb(0x0b57d0);
pub const STRING: Color = Color::rgb(0xa31515);
pub const NUMBER: Color = Color::rgb(0x098658);
pub const COMMENT: Color = Color::rgb(0x2f7d32);
pub const IDENTIFIER: Color = Color::rgb(0x1f2328);
pub const FUNCTION: Color = Color::rgb(0x795e26);

const KEYWORDS: &[&str] = &[
    "all", "alter", "analyze", "and", "any", "array", "as", "asc", "attach", "between", "by",
    "call", "case", "cast", "check", "checkpoint", "collate", "column", "commit", "constraint",
    "copy", "create", "cross", "cube", "current_date", "current_time", "current_timestamp",
    "database", "default", "delete", "desc", "describe", "detach", "distinct", "drop", "else",
    "end", "except", "exclude", "exists", "explain", "export", "extension", "false", "filter",
    "first", "following", "for", "foreign", "from", "full", "function", "glob", "group",
    "grouping", "having", "if", "ilike", "import", "in", "index", "inner", "insert", "install",
    "intersect", "interval", "into", "is", "isnull", "join", "key", "last", "lateral", "left",
    "like", "limit", "load", "macro", "materialized", "natural", "not", "notnull", "null",
    "nulls", "offset", "on", "or", "order", "outer", "over", "partition", "pivot", "pivot_longer",
    "pivot_wider", "pragma", "preceding", "primary", "qualify", "range", "recursive",
    "references", "rename", "replace", "returning", "right", "rollback", "rollup", "row", "rows",
    "sample", "schema", "select", "semi", "sequence", "set", "setof", "show", "similar", "some",
    "summarize", "table", "temp", "temporary", "then", "to", "transaction", "true", "try_cast",
    "type", "unbounded", "union", "unique", "unpivot", "update", "use", "using", "vacuum",
    "values", "view", "when", "where", "window", "with", "within", "begin", "anti", "asof",
    "positional", "struct", "map", "union_by_name", "by_name", "only",
];

fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|keyword| keyword.eq_ignore_ascii_case(word))
}

/// Colored byte ranges of `text`.
pub fn runs(text: &str) -> Vec<(usize, usize, Color)> {
    let tokens = tokenize(text);
    let mut runs = Vec::with_capacity(tokens.len());
    for (index, token) in tokens.iter().enumerate() {
        let range = token.range.clone();
        let color = match &token.kind {
            TokenKind::Comment => COMMENT,
            TokenKind::Literal => STRING,
            TokenKind::Quoted(_) => IDENTIFIER,
            TokenKind::Word(word) => {
                if word.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
                    NUMBER
                } else if is_keyword(word) {
                    KEYWORD
                } else if tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind == TokenKind::Symbol('('))
                {
                    FUNCTION
                } else {
                    continue;
                }
            }
            TokenKind::Symbol(_) => continue,
        };
        runs.push((range.start, range.end, color));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_keywords_strings_numbers_and_comments() {
        let text = "SELECT sum(x), 'a b', 42 FROM t -- note";
        let colored = runs(text)
            .into_iter()
            .map(|(start, end, color)| (&text[start..end], color))
            .collect::<Vec<_>>();
        assert!(colored.contains(&("SELECT", KEYWORD)));
        assert!(colored.contains(&("sum", FUNCTION)));
        assert!(colored.contains(&("'a b'", STRING)));
        assert!(colored.contains(&("42", NUMBER)));
        assert!(colored.contains(&("FROM", KEYWORD)));
        assert!(colored.contains(&("-- note", COMMENT)));
        assert!(!colored.iter().any(|(text, _)| *text == "t"));
    }
}
