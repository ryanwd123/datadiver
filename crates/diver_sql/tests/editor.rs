//! Highlighting, diagnostics, signature help, hover, folding and selection ranges.

mod common;

use common::{catalog, split_cursor};
use diver_sql::{HighlightClass, Severity, parse};
use expect_test::{Expect, expect};

fn check_highlight(sql: &str, expect: Expect) {
    let p = parse(sql);
    let out: Vec<String> = diver_sql::highlight_with(&p, &catalog())
        .into_iter()
        .map(|(range, class)| format!("{:?} {class:?}", &sql[range]))
        .collect();
    expect.assert_eq(&out.join("\n"));
}

#[test]
fn highlight_select() {
    check_highlight(
        "WITH x AS (SELECT 1) SELECT o.amount, sum(o.amount) AS total, year FROM main.orders o JOIN x ON true -- c\nWHERE d = DATE '2024-01-01' AND p = $1",
        expect![[r#"
            "WITH" Keyword
            "x" Cte
            "AS" Keyword
            "SELECT" Keyword
            "1" Number
            "SELECT" Keyword
            "o" Alias
            "amount" Column
            "sum" Aggregate
            "o" Alias
            "amount" Column
            "AS" Keyword
            "total" Alias
            "year" Column
            "FROM" Keyword
            "main" Schema
            "orders" Table
            "o" Alias
            "JOIN" Keyword
            "x" Cte
            "ON" Keyword
            "true" Keyword
            "-- c" Comment
            "WHERE" Keyword
            "d" Column
            "=" Operator
            "DATE" Type
            "'2024-01-01'" String
            "AND" Keyword
            "p" Column
            "=" Operator
            "$1" Parameter"#]],
    );
}

#[test]
fn highlight_paths_and_ddl() {
    check_highlight(
        "CREATE TABLE s.t (a INTEGER, \"b c\" VARCHAR); SELECT * FROM 'x.parquet', read_csv('y.csv', header := true)",
        expect![[r#"
            "CREATE" Keyword
            "TABLE" Keyword
            "s" Schema
            "t" Table
            "a" Column
            "INTEGER" Type
            "\"b c\"" Column
            "VARCHAR" Type
            "SELECT" Keyword
            "*" Operator
            "FROM" Keyword
            "'x.parquet'" FilePath
            "read_csv" Function
            "'y.csv'" FilePath
            "header" Parameter
            ":=" Operator
            "true" Keyword"#]],
    );
}

#[test]
fn keywords_used_as_names_highlight_as_names() {
    let sql = "SELECT name, value, year FROM t";
    let classes = diver_sql::highlight(&parse(sql));
    let of = |word: &str| {
        classes
            .iter()
            .find(|(r, _)| &sql[*r] == word)
            .map(|(_, c)| *c)
    };
    assert_eq!(of("name"), Some(HighlightClass::Column));
    assert_eq!(of("year"), Some(HighlightClass::Column));
    assert_eq!(of("SELECT"), Some(HighlightClass::Keyword));
}

fn check_diagnostics(sql: &str, expect: Expect) {
    let p = parse(sql);
    let out: Vec<String> = diver_sql::diagnostics_with(&p, &catalog())
        .into_iter()
        .map(|d| {
            format!(
                "{:?} {:?} {:?}: {}",
                d.severity, d.range, &sql[d.range], d.message
            )
        })
        .collect();
    expect.assert_eq(&out.join("\n"));
}

#[test]
fn diagnostics() {
    check_diagnostics(
        "SELECT 'open",
        expect![[r#"Error 7..12 "'open": unterminated string"#]],
    );
    check_diagnostics(
        "SELECT a FROM WHERE; SELECT \"x",
        expect![[r#"
        Error 14..19 "WHERE": expected a table
        Error 19..20 ";": expected a condition
        Error 28..30 "\"x": unterminated quoted identifier"#]],
    );
    check_diagnostics(
        "SELECT * FROM orders o JOIN nosuch n ON true",
        expect![[r#"Warning 28..34 "nosuch": unknown table nosuch"#]],
    );
    check_diagnostics(
        "WITH c AS (SELECT 1) SELECT * FROM c, range(3), test_schema.schema_table",
        expect![[r#""#]],
    );
    check_diagnostics(
        "frobnicate everything",
        expect![[r#"Hint 0..10 "frobnicate": statement not checked: the parser doesn't know it"#]],
    );
    let clean = diver_sql::diagnostics(&parse("SELECT 1; /* fine */ SELECT 2"));
    assert!(clean.is_empty(), "{clean:?}");
    assert!(
        diver_sql::diagnostics(&parse("SELECT \u{1}"))
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
}

fn check_signature(sql: &str, expect: Expect) {
    let (text, offset) = split_cursor(sql);
    let help = diver_sql::signature_help_with(&text, offset, &catalog());
    let out = match help {
        Some(h) => format!(
            "{} arg {} {:?} {:?}",
            h.function
                .iter()
                .map(|n| n.text.as_str())
                .collect::<Vec<_>>()
                .join("."),
            h.active_argument,
            h.active_name,
            h.signatures
        ),
        None => "none".into(),
    };
    expect.assert_eq(&out);
}

#[test]
fn signature_help() {
    check_signature("SELECT sum(|", expect![[r#"sum arg 0 None ["sum(...)"]"#]]);
    check_signature(
        "SELECT sum(a, |b)",
        expect![[r#"sum arg 1 None ["sum(...)"]"#]],
    );
    check_signature(
        "SELECT max(sum(a), |) FROM t",
        expect![[r#"max arg 1 None ["max(...)"]"#]],
    );
    check_signature(
        "SELECT * FROM read_csv('x', header := |)",
        expect![[r#"read_csv arg 1 Some("header") ["read_csv(...)"]"#]],
    );
    check_signature("SELECT v.add(1, |)", expect!["v.add arg 1 None []"]);
    check_signature("SELECT sum(a)| FROM t", expect!["none"]);
    check_signature("SELECT a| FROM t", expect!["none"]);
}

fn check_hover(sql: &str, expect: Expect) {
    let (text, offset) = split_cursor(sql);
    let out = match diver_sql::hover(&text, offset, &catalog()) {
        Some(h) => format!(
            "{:?}\n{}",
            &text[h.range],
            h.markdown.lines().take(6).collect::<Vec<_>>().join("\n")
        ),
        None => "none".into(),
    };
    expect.assert_eq(&out);
}

#[test]
fn hover() {
    check_hover(
        "SEL|ECT 1",
        expect![[r#"
        "SELECT"
        **SELECT**: retrieves rows from the database.

        ```sql
        [WITH ...]
        SELECT [DISTINCT] select_list
        FROM tables"#]],
    );
    check_hover(
        "CREATE OR REPLACE VI|EW v AS SELECT 1",
        expect![[r#"
        "VIEW"
        **CREATE VIEW**: defines a view: a named query that runs each time the view is referenced.

        ```sql
        CREATE [OR REPLACE] [TEMP | TEMPORARY] VIEW [IF NOT EXISTS] name [(column_names)]
            AS query
        ```"#]],
    );
    check_hover(
        "SELECT o.amo|unt FROM orders o",
        expect![[r#"
        "amount"
        column `amount` `DOUBLE` of table `orders`"#]],
    );
    check_hover(
        "SELECT * FROM ord|ers",
        expect![[r#"
        "orders"
        table `orders`

        - `order_id` INTEGER
        - `customer_id` INTEGER
        - `amount` DOUBLE
        - `order date` DATE"#]],
    );
    check_hover(
        "WITH x AS (SELECT 1 AS one) SELECT * FROM x|",
        expect![[r#"
        "x"
        CTE `x`

        - `one`"#]],
    );
    check_hover(
        "SELECT * FROM orders o|",
        expect![[r#"
        "o"
        alias for table `orders`"#]],
    );
    check_hover("SELECT 1 FROM t ON CONFLICT DO UPD|ATE", expect!["none"]);
}

#[test]
fn folding_and_selection() {
    let sql = "WITH x AS (\n  SELECT 1\n)\nSELECT CASE\n  WHEN a THEN 1\nEND FROM x;\nSELECT 2;\n/* a\n b */";
    let p = parse(sql);
    let folds: Vec<&str> = diver_sql::folding_ranges(&p)
        .into_iter()
        .map(|r| &sql[r])
        .collect();
    expect![[r#"
        [
            "WITH x AS (\n  SELECT 1\n)\nSELECT CASE\n  WHEN a THEN 1\nEND FROM x",
            "x AS (\n  SELECT 1\n)",
            "CASE\n  WHEN a THEN 1\nEND",
            "/* a\n b */",
        ]
    "#]]
    .assert_debug_eq(&folds);
    let (text, offset) = split_cursor("SELECT a + b|c * 2 FROM t");
    let p = parse(&text);
    let ranges: Vec<&str> = diver_sql::selection_ranges(&p, offset)
        .into_iter()
        .map(|r| &text[r])
        .collect();
    expect![[r#"
        [
            "bc",
            "bc * 2",
            "a + bc * 2",
            "SELECT a + bc * 2",
            "SELECT a + bc * 2 FROM t",
        ]
    "#]]
    .assert_debug_eq(&ranges);
}
