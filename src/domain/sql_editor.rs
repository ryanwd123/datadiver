//! A SQL script as the SQL panel sees it: its statements and what they do,
//! file paths being typed, and the catalog `diver_sql` completes, highlights
//! and checks against.

use std::collections::HashMap;
use std::ops::Range;

use diver_sql::{
    Catalog, ColumnInfo, CompletionItem, Expected, FunctionInfo, ItemKind, RelationSource,
    SettingInfo, SyntaxKind, TableInfo, TableKind, TextSize,
};

/// A token with its byte range and parenthesis depth.
struct Token {
    kind: SyntaxKind,
    range: Range<usize>,
    depth: usize,
}

/// The script's tokens without whitespace and comments.
fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut depth = 0usize;
    for token in diver_sql::tokenize(text) {
        let range = at..at + usize::from(token.len);
        at = range.end;
        if token.kind.is_trivia() {
            continue;
        }
        if token.kind == SyntaxKind::R_PAREN {
            depth = depth.saturating_sub(1);
        }
        out.push(Token {
            kind: token.kind,
            range,
            depth,
        });
        if token.kind == SyntaxKind::L_PAREN {
            depth += 1;
        }
    }
    out
}

fn is_word(text: &str, token: &Token, word: &str) -> bool {
    token.kind == SyntaxKind::IDENT && text[token.range.clone()].eq_ignore_ascii_case(word)
}

/// The statements of a script: each runs from the end of the previous `;` to
/// its own `;` (included), so comments before a statement belong to it.
/// Statements of only comments are left out.
pub fn statements(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut has_sql = false;
    for token in tokens(text) {
        if token.kind == SyntaxKind::SEMICOLON && token.depth == 0 {
            if has_sql {
                ranges.push(start..token.range.end);
            }
            start = token.range.end;
            has_sql = false;
        } else {
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

/// The first word of a statement, past opening parentheses.
fn first_word(tokens: &[Token]) -> Option<&Token> {
    tokens.iter().find(|t| t.kind != SyntaxKind::L_PAREN)
}

pub fn returns_rows(sql: &str) -> bool {
    let tokens = tokens(sql);
    let top = tokens.iter().filter(|t| t.depth == 0).collect::<Vec<_>>();
    if top.iter().any(|t| is_word(sql, t, "returning")) {
        return true;
    }
    let Some(first) = first_word(&tokens) else {
        return false;
    };
    if is_word(sql, first, "with") {
        return !top.iter().any(|t| {
            ["insert", "update", "delete", "merge"]
                .iter()
                .any(|w| is_word(sql, t, w))
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
    .any(|w| is_word(sql, first, w))
}

pub fn may_mutate(sql: &str) -> bool {
    !returns_rows(sql)
        || tokens(sql).iter().any(|t| {
            t.depth == 0
                && ["insert", "update", "delete", "merge", "call", "pragma"]
                    .iter()
                    .any(|word| is_word(sql, t, word))
        })
}

/// A query whose whole result can be kept and filtered: it returns rows,
/// writes nothing, and can be wrapped in `FROM (…)`, which EXPLAIN cannot.
pub fn is_filterable_query(sql: &str) -> bool {
    let explain = first_word(&tokens(sql)).is_some_and(|t| is_word(sql, t, "explain"));
    returns_rows(sql) && !may_mutate(sql) && !explain
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relation {
    pub catalog: String,
    pub schema: String,
    pub name: String,
    pub columns: Vec<String>,
    /// The type of each of `columns`, when known.
    pub column_types: Vec<String>,
    pub is_view: bool,
    /// In `current_database()`, so a name without the database finds it.
    pub current_database: bool,
}

/// What the SQL connection knows, for completion, highlighting and checks.
#[derive(Clone, Debug, Default)]
pub struct SqlCatalog {
    pub relations: Vec<Relation>,
    /// `current_database()`.
    pub current_database: String,
    pub databases: Vec<String>,
    /// (database, schema)
    pub schemas: Vec<(String, String)>,
    pub functions: Vec<FunctionInfo>,
    pub settings: Vec<SettingInfo>,
    pub extensions: Vec<String>,
    /// Columns of the files and table functions scripts read, by
    /// [`FileSource::key`], as DESCRIBE told them.
    pub files: HashMap<String, Vec<ColumnInfo>>,
    /// Columns of the metadata table functions that take no arguments, by
    /// lowercase name: `duckdb_columns()`, and `duckdb_columns`, which DuckDB
    /// reads through a system view of the same name.
    pub table_functions: HashMap<String, Vec<ColumnInfo>>,
}

impl SqlCatalog {
    /// Takes `catalog`'s database objects, keeping the file columns, which
    /// are learned apart from the database.
    pub fn replace(&mut self, catalog: SqlCatalog) {
        let files = std::mem::take(&mut self.files);
        *self = SqlCatalog { files, ..catalog };
    }

    /// The database a name without one is in.
    fn database<'a>(&'a self, database: Option<&'a str>) -> &'a str {
        database.unwrap_or(&self.current_database)
    }

    /// Whether a name without a database finds relations of `database`: the
    /// current one, and `temp` for temporary tables.
    fn searched(&self, database: &str) -> bool {
        database.eq_ignore_ascii_case(&self.current_database) || database.eq_ignore_ascii_case("temp")
    }

    /// Relations of `database.schema`; without a database, those a name
    /// without one finds, and `main` without a schema.
    fn relations_in<'a, 'b>(
        &'a self,
        database: Option<&'b str>,
        schema: Option<&'b str>,
    ) -> impl Iterator<Item = &'a Relation> {
        let schema = schema.unwrap_or("main");
        self.relations.iter().filter(move |r| {
            let database_ok = match database {
                Some(d) => r.catalog.eq_ignore_ascii_case(d),
                None => self.searched(&r.catalog),
            };
            database_ok && r.schema.eq_ignore_ascii_case(schema)
        })
    }

    /// The relation a written name refers to, as DuckDB resolves it:
    /// `name`, `schema.name` or `database.name`, `database.schema.name`.
    pub fn resolve(&self, path: &[&str]) -> Option<&Relation> {
        let named = |r: &&Relation, name: &str| r.name.eq_ignore_ascii_case(name);
        match path {
            [name] => self.relations_in(None, None).find(|r| named(r, name)),
            [qualifier, name] => self
                .relations_in(None, Some(qualifier))
                .find(|r| named(r, name))
                .or_else(|| self.relations_in(Some(qualifier), None).find(|r| named(r, name))),
            [database, schema, name] => self
                .relations_in(Some(database), Some(schema))
                .find(|r| named(r, name)),
            _ => None,
        }
    }

    /// Relations a name without a qualifier does not reach, with the
    /// qualified name to write for each: another database's, or another
    /// schema's.
    pub fn qualified_elsewhere(&self) -> impl Iterator<Item = (&Relation, Vec<&str>)> {
        self.relations.iter().filter_map(|r| {
            let here = self.searched(&r.catalog);
            let main = r.schema.eq_ignore_ascii_case("main");
            match (here, main) {
                (true, true) => None,
                (true, false) => Some((r, vec![r.schema.as_str(), &r.name])),
                (false, true) => Some((r, vec![r.catalog.as_str(), &r.name])),
                (false, false) => Some((r, vec![r.catalog.as_str(), &r.schema, &r.name])),
            }
        })
    }
}

impl Catalog for SqlCatalog {
    fn databases(&self) -> Vec<String> {
        self.databases.clone()
    }

    fn schemas(&self, database: Option<&str>) -> Vec<String> {
        let database = self.database(database);
        self.schemas
            .iter()
            .filter(|(d, _)| d.eq_ignore_ascii_case(database))
            .map(|(_, s)| s.clone())
            .collect()
    }

    fn tables(&self, database: Option<&str>, schema: Option<&str>) -> Vec<TableInfo> {
        self.relations_in(database, schema)
            .map(|r| TableInfo {
                database: Some(r.catalog.clone()),
                schema: Some(r.schema.clone()),
                name: r.name.clone(),
                kind: if r.is_view {
                    TableKind::View
                } else {
                    TableKind::Table
                },
            })
            .collect()
    }

    fn columns(&self, relation: &RelationSource) -> Option<Vec<ColumnInfo>> {
        let path = match relation {
            RelationSource::Table { path } => path,
            RelationSource::TableFunction { name, args, .. } if args.is_empty() => {
                return self.table_function_columns(name);
            }
            _ => return self.files.get(&file_key(relation)?).cloned(),
        };
        let names = path.iter().map(|n| n.text.as_str()).collect::<Vec<_>>();
        let Some(relation) = self.resolve(&names) else {
            return self.table_function_columns(path);
        };
        Some(
            relation
                .columns
                .iter()
                .enumerate()
                .map(|(i, name)| ColumnInfo {
                    name: name.clone(),
                    data_type: relation.column_types.get(i).cloned(),
                })
                .collect(),
        )
    }

    fn functions(&self) -> Vec<FunctionInfo> {
        self.functions.clone()
    }

    fn settings(&self) -> Vec<SettingInfo> {
        self.settings.clone()
    }

    fn extensions(&self) -> Vec<String> {
        self.extensions.clone()
    }
}

impl SqlCatalog {
    /// The columns of a table function without arguments, called (`name()`)
    /// or named like a table (`name`, `main.name`, `system.main.name`).
    fn table_function_columns(&self, path: &[diver_sql::Name]) -> Option<Vec<ColumnInfo>> {
        let (name, qualifier) = path.split_last()?;
        let system = match qualifier {
            [] => true,
            [schema] => schema.matches("main") || schema.matches("system"),
            [database, schema] => database.matches("system") && schema.matches("main"),
            _ => false,
        };
        system.then(|| self.table_functions.get(&name.text.to_lowercase()).cloned())?
    }
}

/// A file or file-reading table function a script reads from, which the
/// app describes to learn its columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSource {
    /// The quoted path or the call as written; it names the source in
    /// [`SqlCatalog::files`].
    pub key: String,
    /// The statement that lists its columns.
    pub describe: String,
    /// The local file (or glob) it reads, when one is named, to tell when it changes.
    pub path: Option<String>,
}

/// Whether DuckDB would fetch `path` over the network, which is too slow
/// (and may cost money) to do in the background.
fn is_remote(path: &str) -> bool {
    path.contains("://")
}

/// The key `source`'s columns are kept under: the path for a file, the
/// call for a table function that reads files.
fn file_key(source: &RelationSource) -> Option<String> {
    match source {
        RelationSource::File { path } if !path.is_empty() && !is_remote(path) => Some(format!("'{path}'")),
        RelationSource::TableFunction { name, args, call }
            if name.last().is_some_and(|n| diver_sql::takes_path(&n.text))
                && !args.iter().any(|arg| is_remote(arg)) =>
        {
            Some(call.clone())
        }
        _ => None,
    }
}

/// The local files and file-reading table functions `text` reads from.
pub fn file_sources(text: &str) -> Vec<FileSource> {
    diver_sql::file_relations(&diver_sql::parse(text))
        .iter()
        .filter_map(|source| {
            let key = file_key(source)?;
            let (describe, path) = match source {
                RelationSource::File { path } => {
                    (format!("DESCRIBE SELECT * FROM '{}'", path.replace('\'', "''")), Some(path.clone()))
                }
                RelationSource::TableFunction { call, args, .. } => {
                    (format!("DESCRIBE SELECT * FROM {call}"), args.first().cloned())
                }
                _ => return None,
            };
            Some(FileSource { key, describe, path })
        })
        .collect()
}

/// Completion items at `offset`: `diver_sql`'s, plus the relations a bare
/// name doesn't reach, under the qualified name to write, where a table may go.
pub fn completions(text: &str, offset: usize, catalog: &SqlCatalog) -> Vec<CompletionItem> {
    let ctx = diver_sql::complete(text, TextSize::new(offset.min(text.len()) as u32));
    let objects = diver_sql::document_objects(&diver_sql::parse(text));
    let mut items = diver_sql::items(&ctx, catalog, &objects);
    let tables = ctx
        .expected
        .iter()
        .any(|e| matches!(e, Expected::TableRef { qualifier } if qualifier.is_empty()));
    if tables {
        for (relation, path) in catalog.qualified_elsewhere() {
            let insert = path
                .iter()
                .map(|part| diver_sql::quote_if_needed(part))
                .collect::<Vec<_>>()
                .join(".");
            items.push(CompletionItem {
                label: path.join("."),
                kind: if relation.is_view {
                    ItemKind::View
                } else {
                    ItemKind::Table
                },
                insert_text: insert,
                replace_range: ctx.replace_range,
                detail: Some(relation.catalog.clone()),
                documentation: None,
                // After the default schema's tables.
                sort_key: 360,
            });
        }
    }
    items
}

/// A file path being typed inside a `'…'` literal.
#[derive(Debug, PartialEq, Eq)]
pub struct PathQuery {
    /// The literal's text from the opening quote to the cursor, with `''` unescaped.
    pub typed: String,
    /// The last path segment in the editor text, which a completion replaces.
    pub range: Range<usize>,
}

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

/// Whether a string after `before` opens a list of files: `read_csv(['…`.
fn first_of_path_list(text: &str, before: &[Token]) -> bool {
    match before {
        [.., name, open, bracket]
            if bracket.kind == SyntaxKind::L_BRACK && open.kind == SyntaxKind::L_PAREN =>
        {
            name.kind == SyntaxKind::IDENT && diver_sql::takes_path(&text[name.range.clone()])
        }
        _ => false,
    }
}

/// A path that doesn't look like one completes only where SQL takes a file
/// (`FROM '…'`, `read_csv('…'`, `ATTACH '…'`), so ordinary string values like
/// `name = 'abc'` never open a file list.
pub fn path_query(text: &str, offset: usize) -> Option<PathQuery> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let all = tokens(text);
    let index = all.iter().position(|t| {
        matches!(t.kind, SyntaxKind::STRING | SyntaxKind::UNTERMINATED_STRING)
            && t.range.start < offset
            && offset <= t.range.end
    })?;
    let (kind, literal) = (all[index].kind, all[index].range.clone());
    // E'…' strings (backslash escapes) and $$…$$ are other kinds.
    if text.as_bytes()[literal.start] != b'\'' {
        return None;
    }
    let content = literal.start + 1;
    let closed = kind == SyntaxKind::STRING;
    let content_end = if closed { literal.end - 1 } else { literal.end };
    if offset > content_end {
        return None;
    }
    let raw = &text[content..offset];
    let typed = raw.replace("''", "'");
    if !explicit_path(&typed) && !first_of_path_list(text, &all[..index]) {
        let cx = diver_sql::complete(text, TextSize::new(offset as u32));
        if !cx.expected.contains(&Expected::FilePath) {
            return None;
        }
    }
    let start = content + raw.rfind(['/', '\\']).map_or(0, |at| at + 1);
    // An unterminated literal runs to the end of the line, so only replace what was typed.
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

    pub(crate) fn catalog() -> SqlCatalog {
        let relation = |catalog: &str, schema: &str, name: &str, columns: &[&str], view: bool| Relation {
            catalog: catalog.into(),
            schema: schema.into(),
            name: name.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            column_types: columns.iter().map(|_| "VARCHAR".to_string()).collect(),
            is_view: view,
            current_database: catalog == "db",
        };
        SqlCatalog {
            relations: vec![
                relation("db", "main", "apples", &["color", "café"], false),
                relation("db", "files", "sales", &["total"], true),
                relation("shop", "main", "orders", &["qty"], false),
            ],
            current_database: "db".into(),
            databases: vec!["db".into(), "shop".into()],
            schemas: vec![
                ("db".into(), "main".into()),
                ("db".into(), "files".into()),
                ("shop".into(), "main".into()),
            ],
            ..Default::default()
        }
    }

    fn labels(sql: &str) -> Vec<String> {
        let at = sql.find('|').unwrap();
        let text = sql.replace('|', "");
        completions(&text, at, &catalog())
            .into_iter()
            .filter(|i| i.kind != diver_sql::ItemKind::Keyword)
            .map(|i| i.label)
            .collect()
    }

    #[test]
    fn split_quoted_commented_and_dollar_sql() {
        let sql = "-- ;\nSELECT 'a;''b', $$;$$, $tag$;$tag$, \"a;b\"; /* a; /* ; */ */ SELECT 'café';; -- empty";
        let parts = statements(sql);
        assert_eq!(parts.len(), 2);
        assert!(sql[parts[0].clone()].contains("$tag$;$tag$"));
        assert!(sql[parts[0].clone()].starts_with("-- ;"));
        assert!(sql[parts[0].clone()].ends_with(';'));
        assert_eq!(current_statement(sql, parts[0].end), Some(parts[0].clone()));
        assert_eq!(current_statement(sql, sql.len()), None);
    }

    #[test]
    fn the_catalog_resolves_names_as_duckdb_does() {
        let catalog = catalog();
        assert_eq!(catalog.resolve(&["APPLES"]).unwrap().name, "apples");
        assert_eq!(catalog.resolve(&["files", "sales"]).unwrap().name, "sales");
        assert_eq!(catalog.resolve(&["shop", "orders"]).unwrap().name, "orders");
        assert_eq!(catalog.resolve(&["db", "files", "sales"]).unwrap().name, "sales");
        assert!(catalog.resolve(&["orders"]).is_none());
        assert!(catalog.resolve(&["sales"]).is_none());
        let elsewhere = catalog
            .qualified_elsewhere()
            .map(|(_, path)| path.join("."))
            .collect::<Vec<_>>();
        assert_eq!(elsewhere, ["files.sales", "shop.orders"]);
    }

    #[test]
    fn completion_reads_aliases_and_qualified_names_from_the_catalog() {
        // Names that need quotes are shown and inserted quoted.
        assert_eq!(labels("select a.| from apples a"), ["color", "\"café\""]);
        let sql = "SELECT A.c FROM main.apples AS a JOIN files.sales s ON s.|";
        assert_eq!(labels(sql), ["total"]);
        assert_eq!(labels("SELECT sum(a.|) FROM apples a"), ["color", "\"café\""]);
        assert_eq!(labels("select o.| from shop.orders o"), ["qty"]);
        // Tables of the default schema, then schemas and databases to qualify with.
        let tables = labels("select * from |");
        assert!(tables.contains(&"apples".to_string()), "{tables:?}");
        assert!(!tables.contains(&"orders".to_string()), "{tables:?}");
        assert!(tables.contains(&"shop.orders".to_string()), "{tables:?}");
        assert!(labels("from shop.|").contains(&"orders".to_string()));
        assert!(labels("from files.|").contains(&"sales".to_string()));
        // A CTE that shadows a table has its own columns.
        assert!(labels("WITH apples AS (select 1 AS one) select a.| from apples a").contains(&"one".to_string()));
    }

    #[test]
    fn unusual_names_resolve_through_the_catalog() {
        let mut catalog = catalog();
        catalog.schemas.push(("db".into(), "_sys_bic".into()));
        catalog.relations.push(Relation {
            catalog: "db".into(),
            schema: "_sys_bic".into(),
            name: "org.department.abc/myview_name".into(),
            columns: vec!["dept".into(), "Head Count".into()],
            is_view: true,
            current_database: true,
            ..Default::default()
        });
        let labels = |sql: &str| {
            let at = sql.find('|').unwrap();
            completions(&sql.replace('|', ""), at, &catalog)
                .into_iter()
                .map(|i| i.label)
                .collect::<Vec<_>>()
        };
        let sql = "from _sys_bic.\"org.department.abc/myview_name\" a\nselect a.|";
        assert_eq!(labels(sql), ["dept", "\"Head Count\""]);
        assert!(labels("from _sys_bic.|").contains(&"\"org.department.abc/myview_name\"".to_string()));
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
        assert_eq!(typed("FROM read_parquet(['b").as_deref(), Some("b"));
        assert_eq!(typed("FROM read_parquet(['a.parquet', 'b").as_deref(), None);
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
            "(SELECT 1) UNION (SELECT 2)",
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
