//! Scope at the cursor: which relations, aliases and CTEs are visible, and
//! the columns inferred for them.

mod common;

use common::{context, render};
use expect_test::{Expect, expect};

fn check(sql: &str, expect: Expect) {
    let ctx = context(sql);
    // only the scope lines
    let text: String = render(&ctx)
        .lines()
        .filter(|l| l.starts_with("scope") || l.starts_with("aliases") || l.starts_with("ctes"))
        .map(|l| format!("{l}\n"))
        .collect();
    expect.assert_eq(&text);
}

#[test]
fn correlated_subquery_sees_outer_block() {
    check(
        "SELECT * FROM orders o WHERE EXISTS (SELECT 1 FROM customers c WHERE c.id = o.|)",
        expect![[r#"
        scope: customers AS c, orders AS o @1
    "#]],
    );
}

#[test]
fn implicit_lateral_sees_earlier_items_only() {
    check(
        "SELECT * FROM a, (SELECT * FROM b WHERE b.x = |) s, c",
        expect![[r#"
        scope: b, a @1
    "#]],
    );
}

#[test]
fn join_condition_sees_both_sides() {
    check(
        "SELECT * FROM a JOIN b ON a.x = | JOIN c ON true",
        expect![[r#"
        scope: a, b
    "#]],
    );
}

#[test]
fn select_list_aliases() {
    check(
        "SELECT a AS x, b AS y, | , c AS z FROM t",
        expect![[r#"
        scope: t
        aliases: x, y
    "#]],
    );
    check(
        "SELECT a AS x FROM t WHERE |",
        expect![[r#"
        scope: t
        aliases: x
    "#]],
    );
    check(
        "SELECT a AS x FROM t ORDER BY |",
        expect![[r#"
        scope: t
        aliases: x
    "#]],
    );
}

#[test]
fn ctes_and_inference() {
    check(
        "WITH a AS (SELECT 1 AS one, x FROM t), b(k) AS (SELECT * FROM a) SELECT | FROM a, b",
        expect![[r#"
            scope: cte a [one, x], cte b [k, *]
            ctes: b, a
        "#]],
    );
    check(
        "WITH a AS (SELECT 1), b AS (SELECT * FROM |) SELECT 1",
        expect![[r#"
        ctes: a
    "#]],
    );
}

#[test]
fn dml_targets() {
    check(
        "INSERT INTO t (|",
        expect![[r#"
        scope: t
    "#]],
    );
    check(
        "UPDATE t SET a = | FROM u",
        expect![[r#"
        scope: t, u
    "#]],
    );
    check(
        "DELETE FROM t USING u WHERE |",
        expect![[r#"
        scope: t, u
    "#]],
    );
    check(
        "ALTER TABLE t ALTER |",
        expect![[r#"
        scope: t
    "#]],
    );
    check(
        "CREATE INDEX i ON t (|)",
        expect![[r#"
        scope: t
    "#]],
    );
    check(
        "INSERT INTO t SELECT | FROM u",
        expect![[r#"
        scope: u
    "#]],
    );
}

#[test]
fn values_and_files() {
    check(
        "SELECT | FROM (VALUES (1, 2)) v, 'data/x.parquet', read_csv('y.csv') r",
        expect![[r#"
        scope: Values AS v [col0, col1], 'data/x.parquet', read_csv() AS r
    "#]],
    );
}

#[test]
fn file_relations_lists_each_file_and_call_once() {
    let parse = diver_sql::parse(
        "FROM '~/data/a.parquet' a SELECT a.x;\n\
         SELECT * FROM read_csv('b.csv', sep = ';') AS b JOIN orders o ON true, 'c.json';\n\
         SELECT * FROM (FROM 'a.parquet') s, '~/data/a.parquet'",
    );
    let found: Vec<String> = diver_sql::file_relations(&parse)
        .iter()
        .map(|r| match r {
            diver_sql::RelationSource::File { path } => format!("file {path}"),
            diver_sql::RelationSource::TableFunction { call, args, .. } => format!("call {call} {args:?}"),
            other => format!("{other:?}"),
        })
        .collect();
    expect![[r#"
        [
            "file ~/data/a.parquet",
            "call read_csv('b.csv', sep = ';') [\"b.csv\"]",
            "file c.json",
            "file a.parquet",
        ]
    "#]]
    .assert_debug_eq(&found);
}
