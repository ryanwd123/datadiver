//! The Polars dialect and the expression entry, against a corpus of Polars
//! SQL checked with Polars 0.55 (`tests/polars_corpus`).

mod common;

use diver_sql::{Dialect, Entry, MockCatalog, ParseOptions, TextSize, parse_with};

const POLARS_EXPR: ParseOptions = ParseOptions {
    dialect: Dialect::Polars,
    entry: Entry::Expression,
};
const POLARS_SCRIPT: ParseOptions = ParseOptions {
    dialect: Dialect::Polars,
    entry: Entry::Script,
};

fn corpus(name: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/polars_corpus")
        .join(name);
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn errors(text: &str, options: ParseOptions) -> Vec<String> {
    let p = parse_with(text, options);
    assert_eq!(p.syntax().to_string(), text);
    p.errors().iter().map(|e| e.message.clone()).collect()
}

#[test]
fn polars_expressions_parse_cleanly() {
    for line in corpus("expressions.sql") {
        let errs = errors(&line, POLARS_EXPR);
        assert!(errs.is_empty(), "{line}: {errs:?}");
        let root = parse_with(&line, POLARS_EXPR).syntax();
        assert_eq!(root.kind(), diver_sql::SyntaxKind::EXPRESSION_ROOT);
    }
}

#[test]
fn polars_queries_parse_cleanly() {
    for line in corpus("queries.sql") {
        let errs = errors(&line, POLARS_SCRIPT);
        assert!(errs.is_empty(), "{line}: {errs:?}");
    }
}

#[test]
fn duckdb_only_syntax_is_reported() {
    for text in [
        "list_transform(l, x -> x + 1)",
        "lambda x: x",
        "{'a': 1}",
        "MAP {'k': 1}",
        "[x FOR x IN l]",
        "COLUMNS('a')",
        "a GLOB '*.csv'",
        "a SIMILAR TO 'x'",
        "ts AT TIME ZONE 'UTC'",
        "10!",
    ] {
        let errs = errors(text, POLARS_EXPR);
        assert!(
            errs.iter()
                .any(|e| e.contains("Polars SQL doesn't support")),
            "{text}: {errs:?}"
        );
        assert!(
            errors(
                text,
                ParseOptions {
                    dialect: Dialect::DuckDb,
                    entry: Entry::Expression
                }
            )
            .is_empty(),
            "{text}"
        );
    }
    for text in [
        "INSTALL httpfs",
        "CREATE VIEW v AS SELECT 1",
        "DROP SCHEMA s",
        "PIVOT t ON a USING sum(b)",
        "SELECT * FROM t PIVOT (sum(x) FOR y IN ('a'))",
        "SELECT * FROM a ASOF JOIN b USING (k)",
        "SELECT * FROM a POSITIONAL JOIN b",
        "SELECT * FROM t USING SAMPLE 10%",
        "SELECT * FROM a, LATERAL (SELECT 1) l",
        "SELECT a, sum(b) FROM t GROUP BY CUBE (a)",
        "SELECT x: 1",
    ] {
        let errs = errors(text, POLARS_SCRIPT);
        assert!(
            errs.iter()
                .any(|e| e.contains("Polars SQL doesn't support")),
            "{text}: {errs:?}"
        );
        assert!(errors(text, ParseOptions::default()).is_empty(), "{text}");
    }
}

fn labels(sql: &str, options: ParseOptions, catalog: &MockCatalog) -> Vec<String> {
    let (text, offset) = common::split_cursor(sql);
    let ctx = diver_sql::complete_with_options(&text, offset, options);
    diver_sql::items(&ctx, catalog, &[])
        .into_iter()
        .map(|i| i.label)
        .collect()
}

#[test]
fn expression_completion() {
    let catalog = MockCatalog::new()
        .implicit_columns(&["amount", "region", "Order Date"])
        .function("sum", diver_sql::FunctionKind::Aggregate);
    let got = labels("sum(|)", POLARS_EXPR, &catalog);
    for want in ["amount", "region", "\"Order Date\"", "sum", "CASE"] {
        assert!(got.iter().any(|g| g == want), "missing {want}: {got:?}");
    }
    assert!(
        !got.iter().any(|g| g == "LAMBDA" || g == "COLUMNS"),
        "{got:?}"
    );
    let got = labels("amount |", POLARS_EXPR, &catalog);
    assert!(
        got.iter().any(|g| g == "AS") && got.iter().any(|g| g == "BETWEEN"),
        "{got:?}"
    );
    // DuckDB expressions keep their extras
    let got = labels(
        "sum(|)",
        ParseOptions {
            dialect: Dialect::DuckDb,
            entry: Entry::Expression,
        },
        &catalog,
    );
    assert!(got.iter().any(|g| g == "LAMBDA"), "{got:?}");
}

#[test]
fn polars_completion_offers_only_polars_syntax() {
    let catalog = MockCatalog::new();
    let got = labels("|", POLARS_SCRIPT, &catalog);
    assert!(
        got.iter().any(|g| g == "SELECT") && got.iter().any(|g| g == "SHOW"),
        "{got:?}"
    );
    assert!(
        !got.iter()
            .any(|g| g == "INSTALL" || g == "PIVOT" || g == "ATTACH"),
        "{got:?}"
    );
    let got = labels("CREATE |", POLARS_SCRIPT, &catalog);
    assert!(
        got.iter().any(|g| g == "TABLE") && !got.iter().any(|g| g == "VIEW" || g == "MACRO"),
        "{got:?}"
    );
    let got = labels("SELECT * FROM t |", POLARS_SCRIPT, &catalog);
    assert!(
        got.iter().any(|g| g == "JOIN") && got.iter().any(|g| g == "WHERE"),
        "{got:?}"
    );
    assert!(
        !got.iter()
            .any(|g| g == "ASOF" || g == "POSITIONAL" || g == "USING SAMPLE"),
        "{got:?}"
    );
    let ctx = diver_sql::complete_with_options("SELECT 1;", TextSize::new(9), POLARS_SCRIPT);
    assert!(!ctx.expected.is_empty());
}

#[test]
fn types_are_the_dialects_own() {
    let catalog = MockCatalog::new().implicit_columns(&["amount"]);
    let got = labels("CAST(amount AS |)", POLARS_EXPR, &catalog);
    for want in ["DATETIME", "STRING", "BIGINT"] {
        assert!(got.iter().any(|g| g == want), "missing {want}: {got:?}");
    }
    assert!(
        !got.iter().any(|g| g == "STRUCT" || g == "MAP" || g == "JSON"),
        "{got:?}"
    );
    let duckdb = ParseOptions {
        dialect: Dialect::DuckDb,
        entry: Entry::Expression,
    };
    let got = labels("CAST(amount AS |)", duckdb, &catalog);
    assert!(got.iter().any(|g| g == "STRUCT") && !got.iter().any(|g| g == "DATETIME"), "{got:?}");
}
