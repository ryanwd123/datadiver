//! Shared test helpers: the cursor marker and a catalog shaped like Dabbler's test database.

#![allow(dead_code)]

use diver_sql::{
    CompletionContext, Expected, FunctionKind, MockCatalog, TextSize, VisibleRelation,
};

pub const CURSOR: char = '|';

/// Splits `sql|more` into the text and the cursor offset.
pub fn split_cursor(sql: &str) -> (String, TextSize) {
    assert_eq!(
        sql.matches(CURSOR).count(),
        1,
        "needs exactly one | marker: {sql}"
    );
    let pos = sql.find(CURSOR).unwrap();
    (sql.replacen(CURSOR, "", 1), TextSize::new(pos as u32))
}

pub fn context(sql: &str) -> CompletionContext {
    let (text, offset) = split_cursor(sql);
    diver_sql::complete(&text, offset)
}

/// The labels at the cursor, filtered by the typed prefix the way an editor would.
pub fn labels(sql: &str) -> Vec<String> {
    let (text, offset) = split_cursor(sql);
    let ctx = diver_sql::complete(&text, offset);
    let items = diver_sql::complete_with(&text, offset, &catalog());
    let prefix = ctx.prefix.to_lowercase();
    items
        .into_iter()
        .filter(|i| {
            i.label
                .trim_matches('"')
                .to_lowercase()
                .starts_with(&prefix)
        })
        .map(|i| i.label)
        .collect()
}

/// A short rendering of a context for snapshots.
pub fn render(ctx: &CompletionContext) -> String {
    let mut out = String::new();
    let expected: Vec<String> = ctx.expected.iter().map(render_expected).collect();
    out.push_str(&format!("expected: {}\n", expected.join(", ")));
    out.push_str(&format!(
        "replace: {:?} {:?}\n",
        ctx.replace_range, ctx.prefix
    ));
    if !ctx.scope.relations.is_empty() {
        let rels: Vec<String> = ctx.scope.relations.iter().map(render_relation).collect();
        out.push_str(&format!("scope: {}\n", rels.join(", ")));
    }
    if !ctx.scope.aliases.is_empty() {
        let names: Vec<&str> = ctx.scope.aliases.iter().map(|n| n.text.as_str()).collect();
        out.push_str(&format!("aliases: {}\n", names.join(", ")));
    }
    if !ctx.scope.ctes.is_empty() {
        let names: Vec<&str> = ctx.scope.ctes.iter().map(|n| n.text.as_str()).collect();
        out.push_str(&format!("ctes: {}\n", names.join(", ")));
    }
    if ctx.in_string.is_some() {
        out.push_str("in_string\n");
    }
    if ctx.in_comment {
        out.push_str("in_comment\n");
    }
    out
}

fn names(q: &[diver_sql::Name]) -> String {
    q.iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub fn render_expected(e: &Expected) -> String {
    match e {
        Expected::Keyword(kw) => kw.text().to_uppercase(),
        Expected::Phrase(kws) => kws
            .iter()
            .map(|k| k.text().to_uppercase())
            .collect::<Vec<_>>()
            .join(" "),
        Expected::Punct(k) => format!("{k:?}"),
        Expected::TableRef { qualifier } if qualifier.is_empty() => "TableRef".into(),
        Expected::TableRef { qualifier } => format!("TableRef({})", names(qualifier)),
        Expected::ColumnRef { qualifier } if qualifier.is_empty() => "ColumnRef".into(),
        Expected::ColumnRef { qualifier } => format!("ColumnRef({})", names(qualifier)),
        Expected::FunctionName { kind, qualifier } if qualifier.is_empty() => {
            format!("Function({kind:?})")
        }
        Expected::FunctionName { kind, qualifier } => {
            format!("Function({kind:?}, {})", names(qualifier))
        }
        Expected::SchemaName { database: None } => "SchemaName".into(),
        Expected::SchemaName { database: Some(d) } => format!("SchemaName({})", d.text),
        other => format!("{other:?}"),
    }
}

fn render_relation(r: &VisibleRelation) -> String {
    let source = match &r.source {
        diver_sql::RelationSource::Table { path } => names(path),
        diver_sql::RelationSource::File { path } => format!("'{path}'"),
        diver_sql::RelationSource::TableFunction { name, .. } => format!("{}()", names(name)),
        diver_sql::RelationSource::Cte { name } => format!("cte {}", name.text),
        other => format!("{other:?}"),
    };
    let alias = r
        .alias
        .as_ref()
        .map(|a| format!(" AS {}", a.text))
        .unwrap_or_default();
    let depth = if r.depth > 0 {
        format!(" @{}", r.depth)
    } else {
        String::new()
    };
    let columns = match &r.columns {
        diver_sql::Columns::Known(cols) => {
            let names: Vec<String> = cols
                .iter()
                .map(|c| match c {
                    diver_sql::ColumnSource::Named(n) => n.text.clone(),
                    diver_sql::ColumnSource::Typed { name, .. } => name.text.clone(),
                    diver_sql::ColumnSource::Star { .. } => "*".into(),
                    diver_sql::ColumnSource::Unnamed(t) => format!("<{t}>"),
                })
                .collect();
            format!(" [{}]", names.join(", "))
        }
        _ => String::new(),
    };
    format!("{source}{alias}{columns}{depth}")
}

const ORDERS: &[&str] = &[
    "order_id INTEGER",
    "customer_id INTEGER",
    "amount DOUBLE",
    "\"order date\" DATE",
];
const CUSTOMERS: &[&str] = &["customer_id INTEGER", "name VARCHAR", "region VARCHAR"];

pub fn catalog() -> MockCatalog {
    MockCatalog::new()
        .database("attached_db")
        .schema("memory", "test_schema")
        .schema("memory", "v")
        .schema("attached_db", "main")
        .schema("attached_db", "attached_schema")
        .table("orders", ORDERS)
        .table("customers", CUSTOMERS)
        .table("Order Lines", &["line_id", "qty"])
        .table_in(
            "memory",
            "test_schema",
            "schema_table",
            &["column1", "column2"],
        )
        .table_in(
            "attached_db",
            "attached_schema",
            "db_table",
            &["column1", "column2"],
        )
        .view_in("memory", "main", "test_view", ORDERS)
        .view_in("memory", "test_schema", "test_view2", ORDERS)
        .view_in(
            "memory",
            "test_schema",
            "org.category/dotted_view",
            CUSTOMERS,
        )
        .view_in("memory", "v", "test_view3", ORDERS)
        .function("addition", FunctionKind::Macro)
        .function_in("v", "add", FunctionKind::Macro)
        .table_function("my_table_macro", &["1"])
        .table_function("recent_orders", &["order_id", "total"])
        .function_in("v", "my_table_macro_in_v", FunctionKind::Table)
        .function("sum", FunctionKind::Aggregate)
        .function("substring", FunctionKind::Scalar)
        .function("count", FunctionKind::Aggregate)
        .function("max", FunctionKind::Aggregate)
        .function("row_number", FunctionKind::Window)
        .function("list_transform", FunctionKind::Scalar)
        .table_function("read_csv", &[])
        .table_function("read_csv_auto", &[])
        .table_function("read_parquet", &[])
        .table_function("range", &["range"])
        .setting("max_memory")
        .setting("threads")
        .variable("db_var", "INTEGER")
        .sequence("db_seq")
        .extension("httpfs")
        .extension("parquet")
        .path("data/", true)
        .path("data/orders.parquet", false)
}
