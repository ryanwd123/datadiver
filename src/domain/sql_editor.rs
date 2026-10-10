use std::ops::Range;

use crate::infrastructure::duckdb_store::quote_identifier;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Word(String),
    Quoted(String),
    Literal,
    Comment,
    Symbol(char),
}

#[derive(Clone, Debug)]
pub struct SqlToken {
    pub kind: TokenKind,
    pub range: Range<usize>,
    pub scope: usize,
}

impl SqlToken {
    fn identifier(&self) -> Option<&str> {
        match &self.kind {
            TokenKind::Word(word) | TokenKind::Quoted(word) => Some(word),
            _ => None,
        }
    }

    pub fn keyword(&self, word: &str) -> bool {
        matches!(&self.kind, TokenKind::Word(value) if value.eq_ignore_ascii_case(word))
    }
}

/// Tolerates incomplete SQL so execution selection and completion share the same boundaries.
pub fn tokenize(text: &str) -> Vec<SqlToken> {
    let mut tokens = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut scopes = vec![0];
    let mut next_scope = 1;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let kind = if bytes[i..].starts_with(b"--") {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            TokenKind::Comment
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            TokenKind::Comment
        } else if matches!(bytes[i], b'\'' | b'"') {
            let quote = bytes[i];
            i += 1;
            let content = i;
            let mut content_end = bytes.len();
            while i < bytes.len() {
                if bytes[i] == quote {
                    if bytes.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    content_end = i;
                    i += 1;
                    break;
                }
                // DuckDB escape strings use E'...'.
                if quote == b'\''
                    && start > 0
                    && matches!(bytes[start - 1], b'e' | b'E')
                    && bytes[i] == b'\\'
                {
                    i = (i + 2).min(bytes.len());
                } else {
                    i += 1;
                }
            }
            if quote == b'"' {
                TokenKind::Quoted(text[content..content_end].replace("\"\"", "\""))
            } else {
                TokenKind::Literal
            }
        } else if bytes[i] == b'$' && dollar_delimiter(&text[i..]).is_some() {
            let delimiter = dollar_delimiter(&text[i..]).unwrap();
            i += delimiter.len();
            i = text[i..]
                .find(delimiter)
                .map_or(bytes.len(), |end| i + end + delimiter.len());
            TokenKind::Literal
        } else {
            let ch = text[i..].chars().next().unwrap();
            if ch.is_alphanumeric() || ch == '_' {
                i += ch.len_utf8();
                while i < bytes.len() {
                    let ch = text[i..].chars().next().unwrap();
                    if !(ch.is_alphanumeric() || ch == '_' || ch == '$') {
                        break;
                    }
                    i += ch.len_utf8();
                }
                TokenKind::Word(text[start..i].into())
            } else {
                i += ch.len_utf8();
                TokenKind::Symbol(ch)
            }
        };
        if kind == TokenKind::Symbol(')') && scopes.len() > 1 {
            scopes.pop();
        }
        let scope = *scopes.last().unwrap();
        tokens.push(SqlToken {
            kind: kind.clone(),
            range: start..i,
            scope,
        });
        if kind == TokenKind::Symbol('(') {
            scopes.push(next_scope);
            next_scope += 1;
        }
    }
    tokens
}

fn dollar_delimiter(text: &str) -> Option<&str> {
    let end = text[1..].find('$')? + 1;
    let tag = &text[1..end];
    if !tag.chars().all(|c| c.is_alphanumeric() || c == '_')
        || tag.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    Some(&text[..=end])
}

pub fn statements(text: &str) -> Vec<Range<usize>> {
    let tokens = tokenize(text);
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut has_sql = false;
    for token in tokens {
        if token.kind == TokenKind::Symbol(';') && token.scope == 0 {
            if has_sql {
                ranges.push(start..token.range.end);
            }
            start = token.range.end;
            has_sql = false;
        } else if token.kind != TokenKind::Comment {
            has_sql = true;
        }
    }
    if has_sql {
        ranges.push(start..text.len());
    }
    ranges
}

pub fn current_statement(text: &str, cursor: usize) -> Option<Range<usize>> {
    let cursor = cursor.min(text.len());
    statements(text)
        .into_iter()
        .find(|range| range.start <= cursor && cursor <= range.end)
}

pub fn returns_rows(sql: &str) -> bool {
    let tokens = tokenize(sql);
    let top = tokens
        .iter()
        .filter(|t| t.scope == 0 && t.kind != TokenKind::Comment)
        .collect::<Vec<_>>();
    if top.iter().any(|t| t.keyword("returning")) {
        return true;
    }
    let Some(first) = tokens
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Comment | TokenKind::Symbol('(')))
    else {
        return false;
    };
    if first.keyword("with") {
        return !top.iter().any(|t| {
            ["insert", "update", "delete", "merge"]
                .iter()
                .any(|w| t.keyword(w))
        });
    }
    [
        "select",
        "from",
        "pivot",
        "unpivot",
        "values",
        "show",
        "describe",
        "desc",
        "explain",
        "summarize",
        "pragma",
        "call",
    ]
    .iter()
    .any(|w| first.keyword(w))
}

pub fn may_mutate(sql: &str) -> bool {
    let tokens = tokenize(sql);
    !returns_rows(sql)
        || tokens.iter().any(|t| {
            t.scope == 0
                && ["insert", "update", "delete", "merge", "call", "pragma"]
                    .iter()
                    .any(|word| t.keyword(word))
        })
}

/// A query whose whole result can be kept and filtered: it returns rows,
/// writes nothing, and can be wrapped in `FROM (…)`, which EXPLAIN cannot.
pub fn is_filterable_query(sql: &str) -> bool {
    let explain = tokenize(sql)
        .iter()
        .find(|t| !matches!(t.kind, TokenKind::Comment | TokenKind::Symbol('(')))
        .is_some_and(|t| t.keyword("explain"));
    returns_rows(sql) && !may_mutate(sql) && !explain
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub catalog: String,
    pub schema: String,
    pub name: String,
    pub columns: Vec<String>,
    pub is_view: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SqlCompletion {
    pub label: String,
    pub insert: String,
    pub range: Range<usize>,
    pub is_column: bool,
}

fn reserved(word: &str) -> bool {
    [
        "select",
        "from",
        "join",
        "left",
        "right",
        "inner",
        "outer",
        "full",
        "cross",
        "natural",
        "on",
        "using",
        "where",
        "group",
        "order",
        "having",
        "qualify",
        "limit",
        "offset",
        "union",
        "except",
        "intersect",
        "pivot",
        "unpivot",
        "as",
        "set",
        "returning",
        "window",
    ]
    .iter()
    .any(|w| word.eq_ignore_ascii_case(w))
}

fn relation_matches(relation: &Relation, names: &[String]) -> bool {
    match names {
        [name] => relation.name.eq_ignore_ascii_case(name),
        [schema, name] => {
            relation.schema.eq_ignore_ascii_case(schema) && relation.name.eq_ignore_ascii_case(name)
        }
        [catalog, schema, name] => {
            relation.catalog.eq_ignore_ascii_case(catalog)
                && relation.schema.eq_ignore_ascii_case(schema)
                && relation.name.eq_ignore_ascii_case(name)
        }
        _ => false,
    }
}

pub fn completions(text: &str, offset: usize, relations: &[Relation]) -> Vec<SqlCompletion> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let Some(statement) = current_statement(text, offset) else {
        return Vec::new();
    };
    let mut tokens = tokenize(&text[statement.clone()])
        .into_iter()
        .map(|mut t| {
            t.range.start += statement.start;
            t.range.end += statement.start;
            t
        })
        .collect::<Vec<_>>();
    // Parentheses around expressions share the surrounding query block; subqueries do not.
    let mut query_scopes = std::collections::HashMap::from([(0, 0)]);
    let mut next_scope = 1;
    for (i, token) in tokens.iter().enumerate() {
        if token.kind == TokenKind::Symbol('(') {
            let query = tokens[i + 1..]
                .iter()
                .find(|t| t.kind != TokenKind::Comment)
                .is_some_and(|t| {
                    ["select", "with", "from", "values", "pivot", "unpivot"]
                        .iter()
                        .any(|word| t.keyword(word))
                });
            query_scopes.insert(
                next_scope,
                if query {
                    next_scope
                } else {
                    *query_scopes.get(&token.scope).unwrap_or(&0)
                },
            );
            next_scope += 1;
        }
    }
    for token in &mut tokens {
        token.scope = *query_scopes.get(&token.scope).unwrap_or(&0);
    }
    if tokens.iter().any(|t| {
        matches!(t.kind, TokenKind::Comment | TokenKind::Literal)
            && t.range.start < offset
            && offset <= t.range.end
    }) {
        return Vec::new();
    }
    let word = tokens
        .iter()
        .find(|t| t.identifier().is_some() && t.range.start < offset && offset <= t.range.end);
    let (range, prefix, before_offset) = if let Some(word) = word {
        let start = word.range.start + usize::from(matches!(word.kind, TokenKind::Quoted(_)));
        (
            word.range.clone(),
            text[start..offset]
                .trim_end_matches('"')
                .replace("\"\"", "\"")
                .to_lowercase(),
            word.range.start,
        )
    } else {
        (offset..offset, String::new(), offset)
    };
    let before = tokens
        .iter()
        .filter(|t| t.range.end <= before_offset && t.kind != TokenKind::Comment)
        .collect::<Vec<_>>();
    let mut names = Vec::new();
    let mut at = before.len();
    while at >= 2 && before[at - 1].kind == TokenKind::Symbol('.') {
        let Some(name) = before[at - 2].identifier() else {
            break;
        };
        names.insert(0, name.to_owned());
        at -= 2;
    }
    let scope = word
        .map(|t| t.scope)
        .or_else(|| {
            before.last().map(|t| {
                if t.kind == TokenKind::Symbol('(') {
                    tokens
                        .iter()
                        .find(|n| n.range.start >= t.range.end)
                        .map_or(t.scope, |n| n.scope)
                } else {
                    t.scope
                }
            })
        })
        .unwrap_or(0);
    if at > 0
        && ["from", "join", "pivot"]
            .iter()
            .any(|w| before[at - 1].keyword(w))
    {
        return relations
            .iter()
            .filter(|r| {
                r.name.to_lowercase().starts_with(&prefix)
                    && match names.as_slice() {
                        [] => true,
                        [schema] => r.schema.eq_ignore_ascii_case(schema),
                        [catalog, schema] => {
                            r.catalog.eq_ignore_ascii_case(catalog)
                                && r.schema.eq_ignore_ascii_case(schema)
                        }
                        _ => false,
                    }
            })
            .map(|r| SqlCompletion {
                label: format!(
                    "{}.{}{}",
                    r.schema,
                    r.name,
                    if r.is_view { " (view)" } else { "" }
                ),
                insert: if names.is_empty() {
                    format!(
                        "{}.{}",
                        quote_identifier(&r.schema),
                        quote_identifier(&r.name)
                    )
                } else {
                    quote_identifier(&r.name)
                },
                range: range.clone(),
                is_column: false,
            })
            .collect();
    }
    let [alias] = names.as_slice() else {
        return Vec::new();
    };
    let boundaries = tokens
        .iter()
        .filter(|t| {
            t.scope == scope
                && ["union", "except", "intersect"]
                    .iter()
                    .any(|word| t.keyword(word))
        })
        .collect::<Vec<_>>();
    let branch_start = boundaries
        .iter()
        .filter(|t| t.range.end <= offset)
        .map(|t| t.range.end)
        .max()
        .unwrap_or(statement.start);
    let branch_end = boundaries
        .iter()
        .filter(|t| t.range.start > offset)
        .map(|t| t.range.start)
        .min()
        .unwrap_or(statement.end);
    let relevant = tokens
        .iter()
        .filter(|t| {
            t.scope == scope
                && t.range.start >= branch_start
                && t.range.end <= branch_end
                && t.kind != TokenKind::Comment
        })
        .collect::<Vec<_>>();
    let mut matches = Vec::new();
    let mut bindings = 0;
    for (i, token) in relevant.iter().enumerate() {
        if !(token.keyword("from") || token.keyword("join")) {
            continue;
        }
        let mut j = i + 1;
        let mut table = Vec::new();
        if let Some(name) = relevant.get(j).and_then(|t| t.identifier()) {
            table.push(name.to_owned());
            j += 1;
        }
        while relevant
            .get(j)
            .is_some_and(|t| t.kind == TokenKind::Symbol('.'))
        {
            let Some(name) = relevant.get(j + 1).and_then(|t| t.identifier()) else {
                break;
            };
            table.push(name.to_owned());
            j += 2;
        }
        if relevant.get(j).is_some_and(|t| t.keyword("as")) {
            j += 1;
        }
        let Some(alias_token) = relevant.get(j) else {
            continue;
        };
        let Some(found_alias) = alias_token.identifier() else {
            continue;
        };
        if (matches!(alias_token.kind, TokenKind::Word(_)) && reserved(found_alias))
            || !found_alias.eq_ignore_ascii_case(alias)
        {
            continue;
        }
        bindings += 1;
        // CTE names shadow catalog objects. Their output columns are intentionally not inferred.
        let significant = tokens
            .iter()
            .filter(|t| t.kind != TokenKind::Comment)
            .collect::<Vec<_>>();
        let cte_shadow = table.len() == 1
            && significant.iter().enumerate().any(|(i, candidate)| {
                if candidate.range.start >= token.range.start
                    || !candidate
                        .identifier()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&table[0]))
                {
                    return false;
                }
                let mut j = i + 1;
                if significant
                    .get(j)
                    .is_some_and(|t| t.kind == TokenKind::Symbol('('))
                {
                    j += 1;
                    while significant
                        .get(j)
                        .is_some_and(|t| t.kind != TokenKind::Symbol(')'))
                    {
                        j += 1;
                    }
                    j += 1;
                }
                significant.get(j).is_some_and(|t| t.keyword("as"))
                    && significant
                        .get(j + 1)
                        .is_some_and(|t| t.kind == TokenKind::Symbol('('))
            });
        if !cte_shadow {
            matches.extend(relations.iter().filter(|r| relation_matches(r, &table)));
        }
    }
    if matches.len() != 1 || bindings != 1 {
        return Vec::new();
    }
    matches[0]
        .columns
        .iter()
        .filter(|c| c.to_lowercase().starts_with(&prefix))
        .map(|column| SqlCompletion {
            label: column.clone(),
            insert: quote_identifier(column),
            range: range.clone(),
            is_column: true,
        })
        .collect()
}

/// A file path being typed inside a `'…'` literal.
#[derive(Debug, PartialEq, Eq)]
pub struct PathQuery {
    /// The literal's text from the opening quote to the cursor, with `''` unescaped.
    pub typed: String,
    /// The last path segment in the editor text, which a completion replaces.
    pub range: Range<usize>,
}

/// Table functions whose first argument is a file path or glob.
const FILE_FUNCTIONS: &[&str] = &[
    "parquet_scan",
    "parquet_metadata",
    "parquet_schema",
    "parquet_file_metadata",
    "parquet_kv_metadata",
    "glob",
    "sniff_csv",
    "delta_scan",
    "iceberg_scan",
];

fn explicit_path(typed: &str) -> bool {
    let bytes = typed.as_bytes();
    ["./", ".\\", "../", "..\\", "~/", "~\\", "/", "\\"]
        .iter()
        .any(|prefix| typed.starts_with(prefix))
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// A bare relative path completes only where SQL expects a file, so ordinary
/// string values like `name = 'abc'` never open a file list.
fn file_position(tokens: &[SqlToken], literal: usize) -> bool {
    let mut before = tokens[..literal]
        .iter()
        .rev()
        .filter(|t| t.kind != TokenKind::Comment);
    let Some(previous) = before.next() else {
        return false;
    };
    if ["from", "join", "attach"]
        .iter()
        .any(|w| previous.keyword(w))
    {
        return true;
    }
    let mut previous = Some(previous);
    if previous.is_some_and(|t| t.kind == TokenKind::Symbol('[')) {
        previous = before.next();
    }
    if !previous.is_some_and(|t| t.kind == TokenKind::Symbol('(')) {
        return false;
    }
    before.next().is_some_and(|t| match &t.kind {
        TokenKind::Word(name) => {
            let name = name.to_ascii_lowercase();
            name.starts_with("read_") || FILE_FUNCTIONS.contains(&name.as_str())
        }
        _ => false,
    })
}

pub fn path_query(text: &str, offset: usize) -> Option<PathQuery> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let tokens = tokenize(text);
    let index = tokens.iter().position(|t| {
        t.kind == TokenKind::Literal && t.range.start < offset && offset <= t.range.end
    })?;
    let literal = &tokens[index];
    let bytes = text.as_bytes();
    if bytes[literal.range.start] != b'\'' {
        return None;
    }
    // E'…' strings use backslash escapes, so their text is not a plain path.
    if index > 0
        && tokens[index - 1].range.end == literal.range.start
        && tokens[index - 1].keyword("e")
    {
        return None;
    }
    let content = literal.range.start + 1;
    let closed = literal.range.end > content
        && bytes[literal.range.end - 1] == b'\''
        && !text[content..literal.range.end - 1]
            .replace("''", "")
            .contains('\'');
    let content_end = if closed {
        literal.range.end - 1
    } else {
        literal.range.end
    };
    if offset > content_end {
        return None;
    }
    let raw = &text[content..offset];
    let typed = raw.replace("''", "'");
    if !explicit_path(&typed) && !file_position(&tokens, index) {
        return None;
    }
    let start = content + raw.rfind(['/', '\\']).map_or(0, |at| at + 1);
    // An unterminated literal runs to the end of the text, so only replace what was typed.
    let end = if closed {
        text[offset..content_end]
            .find(['/', '\\'])
            .map_or(content_end, |at| offset + at)
    } else {
        offset
    };
    Some(PathQuery {
        typed,
        range: start..end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Vec<Relation> {
        vec![
            Relation {
                catalog: "db".into(),
                schema: "main".into(),
                name: "apples".into(),
                columns: vec!["color".into(), "café".into()],
                is_view: false,
            },
            Relation {
                catalog: "db".into(),
                schema: "files".into(),
                name: "sales".into(),
                columns: vec!["total".into()],
                is_view: true,
            },
        ]
    }
    #[test]
    fn split_quoted_commented_and_dollar_sql() {
        let sql = "-- ;\nSELECT 'a;''b', $$;$$, $tag$;$tag$, \"a;b\"; /* a; /* ; */ */ SELECT 'café';; -- empty";
        let parts = statements(sql);
        assert_eq!(parts.len(), 2);
        assert!(sql[parts[0].clone()].contains("$tag$;$tag$"));
        assert_eq!(current_statement(sql, parts[0].end), Some(parts[0].clone()));
        assert_eq!(current_statement(sql, sql.len()), None);
    }
    #[test]
    fn alias_completion_reads_below_cursor_and_filters() {
        let sql = "select a. from apples a";
        assert_eq!(
            completions(sql, 9, &catalog())
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["color", "café"]
        );
        let sql = "SELECT A.c FROM main.apples AS a JOIN files.sales s ON s.";
        assert_eq!(completions(sql, 10, &catalog()).len(), 2);
        assert_eq!(completions(sql, sql.len(), &catalog())[0].label, "total");
        let sql = "SELECT sum(a.) FROM apples a";
        assert_eq!(
            completions(sql, sql.find("a.").unwrap() + 2, &catalog()).len(),
            2
        );
    }
    #[test]
    fn relation_triggers_and_qualification() {
        for sql in ["select * FrOm ", "select * from apples a JOIN ", "PIVOT "] {
            assert_eq!(completions(sql, sql.len(), &catalog()).len(), 2);
        }
        let sql = "from files.sa";
        assert_eq!(
            completions(sql, sql.len(), &catalog())[0].insert,
            "\"sales\""
        );
    }
    #[test]
    fn no_unqualified_or_unresolved_or_cross_statement_columns() {
        for sql in [
            "select col",
            "select x. from apples a",
            "select a.; select a.",
            "select 'a.",
            "-- a.",
            "select * from apples a /* a.",
            "WITH apples AS (select 1) select a. from apples a",
            "select x. from (select 1) x",
        ] {
            assert!(completions(sql, sql.len(), &catalog()).is_empty(), "{sql}");
        }
        let sql = "select (select a. from files.sales s) from apples a";
        assert!(completions(sql, sql.find("a.").unwrap() + 2, &catalog()).is_empty());
        let sql = "WITH apples(x) AS (select 1) SELECT a. FROM apples a";
        assert!(completions(sql, sql.find("a.").unwrap() + 2, &catalog()).is_empty());
    }
    #[test]
    fn duplicate_aliases_are_ambiguous() {
        let sql = "select a. from apples a join files.sales a on true";
        assert!(completions(sql, 9, &catalog()).is_empty());
        let sql = "select a. from apples a join unknown_table a on true";
        assert!(completions(sql, 9, &catalog()).is_empty());
        let sql = "select a. from apples a union all select s.total from files.sales s";
        assert_eq!(completions(sql, 9, &catalog()).len(), 2);
        let sql = "select a.color from apples a union all select a. from files.sales s";
        assert!(completions(sql, sql.rfind("a.").unwrap() + 2, &catalog()).is_empty());
    }
    #[test]
    fn path_queries_need_a_path_prefix_or_a_file_position() {
        let typed = |sql: &str| path_query(sql, sql.len()).map(|q| q.typed);
        assert_eq!(
            typed("SELECT * FROM '~/data/ab").as_deref(),
            Some("~/data/ab")
        );
        assert_eq!(typed(r"select 'C:\Users\").as_deref(), Some(r"C:\Users\"));
        assert_eq!(typed("select x = '../a").as_deref(), Some("../a"));
        assert_eq!(typed("FROM 'sales/q1").as_deref(), Some("sales/q1"));
        assert_eq!(typed("select * from read_csv('sa").as_deref(), Some("sa"));
        assert_eq!(typed("FROM read_parquet(['a.parquet', 'b").as_deref(), None);
        assert_eq!(typed("FROM read_parquet(['b").as_deref(), Some("b"));
        assert_eq!(typed("FROM glob('").as_deref(), Some(""));
        assert_eq!(typed("ATTACH 'db/x").as_deref(), Some("db/x"));
        assert_eq!(typed("select * from t where name = 'abc"), None);
        assert_eq!(typed("select E'./a"), None);
        assert_eq!(typed("select $$./a"), None);
        assert_eq!(typed("-- './a"), None);
    }
    #[test]
    fn path_query_replaces_the_last_segment_only() {
        let sql = "FROM './data/sal' AS s";
        let at = sql.find("sal").unwrap() + 2;
        let query = path_query(sql, at).unwrap();
        assert_eq!(query.typed, "./data/sa");
        assert_eq!(&sql[query.range], "sal");
        let sql = "FROM './da/x.csv'";
        let query = path_query(sql, sql.find("da").unwrap() + 1).unwrap();
        assert_eq!(&sql[query.range], "da");
        // After the closing quote the cursor is outside the literal.
        assert!(path_query(sql, sql.len()).is_none());
        let sql = "FROM './it''s/a";
        let query = path_query(sql, sql.len()).unwrap();
        assert_eq!(query.typed, "./it's/a");
        assert_eq!(&sql[query.range], "a");
        let sql = "FROM './x\nSELECT 1";
        let query = path_query(sql, "FROM './x".len()).unwrap();
        assert_eq!(&sql[query.range], "x");
    }
    #[test]
    fn filterable_queries_read_without_writing() {
        for sql in [
            "SELECT 1",
            "-- note\nWITH x AS (SELECT 1) SELECT * FROM x",
            "FROM t",
            "VALUES (1)",
            "PIVOT t ON a USING sum(b)",
            "DESCRIBE t",
            "SUMMARIZE t",
            "SHOW TABLES",
        ] {
            assert!(is_filterable_query(sql), "{sql}");
        }
        for sql in [
            "INSERT INTO t VALUES (1) RETURNING *",
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
            "CREATE TABLE t AS SELECT 1",
            "EXPLAIN SELECT 1",
            "CALL duckdb_tables()",
            "PRAGMA table_info('t')",
        ] {
            assert!(!is_filterable_query(sql), "{sql}");
        }
    }

    #[test]
    fn row_returning_statements() {
        assert!(returns_rows("WITH x AS (select 1) SELECT * FROM x"));
        assert!(!returns_rows(
            "WITH x AS (select 1) INSERT INTO t SELECT * FROM x"
        ));
        assert!(returns_rows("INSERT INTO t VALUES (1) RETURNING *"));
        assert!(!returns_rows("CREATE VIEW v AS SELECT 1"));
    }
}
