//! Completion for the editors that hold one SQL expression: group-by and
//! pivot dimensions, aggregations, and SQL condition cards. `diver_sql`
//! parses the text as an expression in the source's dialect, so the list
//! fits the cursor; the frame's columns are in scope without a FROM clause.

use std::ops::Range;

use diver_sql::{
    Catalog, ColumnInfo, Dialect, Entry, Expected, FunctionInfo, ItemKind, ParseOptions, TextSize,
};

use crate::domain::documents::Engine;
use crate::domain::query::rank_columns;
use crate::infrastructure::duckdb_store::quote_identifier;

/// The columns an expression works on, in preference order, and the
/// functions it may call.
#[derive(Clone, Debug, Default)]
pub struct ExpressionCatalog {
    pub columns: Vec<ColumnInfo>,
    pub functions: Vec<FunctionInfo>,
}

impl ExpressionCatalog {
    pub fn new(columns: impl IntoIterator<Item = (String, Option<String>)>) -> Self {
        Self {
            columns: columns
                .into_iter()
                .map(|(name, data_type)| ColumnInfo { name, data_type })
                .collect(),
            functions: Vec::new(),
        }
    }
}

impl Catalog for ExpressionCatalog {
    fn implicit_columns(&self) -> Vec<ColumnInfo> {
        self.columns.clone()
    }

    fn functions(&self) -> Vec<FunctionInfo> {
        self.functions.clone()
    }
}

/// The SQL dialect expressions over a source run in.
pub fn dialect(engine: Engine) -> Dialect {
    match engine {
        Engine::Polars => Dialect::Polars,
        Engine::DuckDb => Dialect::DuckDb,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionCompletion {
    pub label: String,
    /// A column's type, a function's signature.
    pub detail: Option<String>,
    pub insert: String,
    /// What the completion replaces: the word or quoted name at the cursor.
    pub range: Range<usize>,
}

/// What fits the cursor in `text`. An open `"` lists every column, ranked
/// like the search box (prefix, word start, contains, fuzzy); a bare word
/// lists the columns, functions and keywords it starts once something is
/// typed; strings and comments get nothing.
pub fn completions(
    text: &str,
    offset: usize,
    catalog: &ExpressionCatalog,
    dialect: Dialect,
) -> Vec<ExpressionCompletion> {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let options = ParseOptions {
        dialect,
        entry: Entry::Expression,
    };
    let ctx = diver_sql::complete_with_options(text, TextSize::new(offset as u32), options);
    if ctx.in_string.is_some() || ctx.in_comment {
        return Vec::new();
    }
    let range = usize::from(ctx.replace_range.start())..usize::from(ctx.replace_range.end());
    if ctx.quoted {
        // Right after a closed `"name"` the name is done.
        let word = &text[range.clone()];
        let closed = word.len() >= 2 && word.ends_with('"') && word.matches('"').count().is_multiple_of(2);
        if closed && range.end == offset {
            return Vec::new();
        }
        if !ctx.expected.iter().any(|e| matches!(e, Expected::ColumnRef { .. })) {
            return Vec::new();
        }
        let names = catalog.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>();
        return rank_columns(&names, &ctx.prefix)
            .into_iter()
            .filter_map(|name| catalog.columns.iter().find(|c| c.name == name))
            .map(|column| ExpressionCompletion {
                label: column.name.clone(),
                detail: column.data_type.clone(),
                insert: quote_identifier(&column.name),
                range: range.clone(),
            })
            .collect();
    }
    if ctx.prefix.is_empty() {
        return Vec::new();
    }
    let typed = ctx.prefix.to_lowercase();
    diver_sql::items(&ctx, catalog, &[])
        .into_iter()
        .filter(|item| item.kind != ItemKind::Operator)
        .filter(|item| item.label.trim_start_matches('"').to_lowercase().starts_with(&typed))
        .map(|item| ExpressionCompletion {
            label: item.label,
            detail: item.detail,
            insert: item.insert_text,
            range: range.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> ExpressionCatalog {
        let mut catalog = ExpressionCatalog::new(
            ["Order Total", "region", "a\"b", "total_units"]
                .into_iter()
                .map(|name| (name.to_string(), Some("BIGINT".to_string()))),
        );
        catalog.functions.push(FunctionInfo {
            name: "sum".into(),
            schema: None,
            kind: diver_sql::FunctionKind::Aggregate,
            signatures: vec!["sum(arg)".into()],
            description: None,
        });
        catalog
    }

    /// `(replaced text, labels)` with the cursor at `|`.
    fn complete(marked: &str, dialect: Dialect) -> (String, Vec<String>) {
        let at = marked.find('|').unwrap();
        let text = marked.replace('|', "");
        let items = completions(&text, at, &catalog(), dialect);
        let replaced = items.first().map(|i| text[i.range.clone()].to_string()).unwrap_or_default();
        (replaced, items.into_iter().map(|i| i.label).collect())
    }

    #[test]
    fn quotes_rank_every_column_and_replace_both_quotes() {
        for dialect in [Dialect::DuckDb, Dialect::Polars] {
            let (replaced, labels) = complete("sum(\"|\")", dialect);
            assert_eq!(replaced, "\"\"");
            assert_eq!(labels, ["Order Total", "region", "a\"b", "total_units"]);
            // A word start beats a later substring.
            assert_eq!(complete("sum(\"tot|\")", dialect).1, ["total_units", "Order Total"]);
        }
        let items = completions("\"a\"\"b", 5, &catalog(), Dialect::DuckDb);
        assert_eq!(items[0].insert, "\"a\"\"b\"");
        assert_eq!(complete("\"region\"|", Dialect::DuckDb).1, Vec::<String>::new());
        assert_eq!(complete("region = '\"No|", Dialect::DuckDb).1, Vec::<String>::new());
        assert_eq!(complete("\"a\" > 1 AND \"tot|", Dialect::DuckDb).1, ["total_units", "Order Total"]);
        // Inside a closed name the whole name is replaced, escaped quotes included.
        let (replaced, labels) = complete("max(\"a|\"\"b\") ", Dialect::DuckDb);
        assert_eq!(replaced, "\"a\"\"b\"");
        assert_eq!(labels[0], "a\"b");
    }

    #[test]
    fn words_complete_columns_functions_and_keywords_that_fit() {
        let (replaced, labels) = complete("su|", Dialect::DuckDb);
        assert_eq!(replaced, "su");
        assert_eq!(labels, ["sum"]);
        assert_eq!(complete("sum(re|)", Dialect::DuckDb).1, ["region"]);
        let (_, labels) = complete("CASE WHEN region = 1 TH|", Dialect::DuckDb);
        assert_eq!(labels, ["THEN"]);
        // Nothing before a word is typed, or in strings and comments.
        assert!(complete("sum(|", Dialect::DuckDb).1.is_empty());
        assert!(complete("region = 're|", Dialect::DuckDb).1.is_empty());
        assert!(complete("region -- re|", Dialect::DuckDb).1.is_empty());
    }

    #[test]
    fn polars_leaves_out_what_it_lacks() {
        assert_eq!(complete("sum(LAM|)", Dialect::DuckDb).1, ["LAMBDA"]);
        assert!(complete("sum(LAM|)", Dialect::Polars).1.is_empty());
    }
}
