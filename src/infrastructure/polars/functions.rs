//! The aggregate functions Polars SQL has, for the aggregation manager's
//! reference list. Polars does not list them with descriptions, so they are
//! kept here; a test runs every example. Also every function Polars SQL
//! has, for the SQL editor's completions.

use diver_sql::{FunctionInfo, FunctionKind};

use crate::infrastructure::sql_execution::AggregateFunction;

/// Name, description, signatures, and examples over a column `x` (and `y`).
const FUNCTIONS: &[(&str, &str, &[&str], &[&str])] = &[
    ("array_agg", "Collects the values, including nulls, into a list.", &["array_agg(x)"], &["array_agg(x)", "array_agg(x ORDER BY y)"]),
    ("avg", "The average (mean) of the values.", &["avg(x)"], &["avg(x)"]),
    ("corr", "The Pearson correlation coefficient of two columns.", &["corr(x, y)"], &["corr(x, y)"]),
    ("count", "The number of non-null values, or of rows with *.", &["count(x)", "count(*)", "count(DISTINCT x)"], &["count(x)", "count(*)", "count(DISTINCT x)"]),
    ("covar_pop", "The population covariance of two columns.", &["covar_pop(x, y)"], &["covar_pop(x, y)"]),
    ("covar_samp", "The sample covariance of two columns.", &["covar_samp(x, y)"], &["covar_samp(x, y)"]),
    ("first", "The first value.", &["first(x)"], &["first(x)"]),
    ("last", "The last value.", &["last(x)"], &["last(x)"]),
    ("max", "The largest value.", &["max(x)"], &["max(x)"]),
    ("median", "The median value.", &["median(x)"], &["median(x)"]),
    ("min", "The smallest value.", &["min(x)"], &["min(x)"]),
    (
        "quantile_cont",
        "The quantile q (0 to 1), interpolated between the two closest values.",
        &["quantile_cont(x, q)"],
        &["quantile_cont(x, 0.9)"],
    ),
    ("quantile_disc", "The quantile q (0 to 1), as one of the values.", &["quantile_disc(x, q)"], &["quantile_disc(x, 0.9)"]),
    ("stddev", "The sample standard deviation. Also stddev_samp.", &["stddev(x)"], &["stddev(x)"]),
    (
        "string_agg",
        "Joins text values with a separator, ',' by default. Also listagg and group_concat.",
        &["string_agg(x)", "string_agg(x, separator)"],
        &["string_agg(label, ', ')", "string_agg(DISTINCT label, ', ' ORDER BY label)"],
    ),
    ("sum", "The sum of the values; null when there are none.", &["sum(x)"], &["sum(x)"]),
    ("total", "The sum of the values; 0 when there are none.", &["total(x)"], &["total(x)"]),
    ("variance", "The sample variance. Also var and var_samp.", &["variance(x)"], &["variance(x)"]),
];

/// Aggregates the reference list leaves out: aliases and bitwise ones.
const OTHER_AGGREGATES: &[&str] = &[
    "bit_and", "bit_or", "bit_xor", "covar", "group_concat", "listagg", "stddev_samp", "stdev", "stdev_samp", "var",
    "var_samp",
];

const WINDOW_FUNCTIONS: &[&str] = &["dense_rank", "first_value", "lag", "last_value", "lead", "rank", "row_number"];

const TABLE_FUNCTIONS: &[&str] = &["read_csv", "read_ipc", "read_json", "read_parquet"];

/// Every function Polars SQL has, by its own list, for completion and
/// highlighting. The aggregates in the reference list bring their signatures
/// and descriptions.
pub fn sql_functions() -> Vec<FunctionInfo> {
    let mut names = polars::sql::keywords::all_functions();
    names.extend(TABLE_FUNCTIONS);
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let reference = FUNCTIONS.iter().find(|(known, ..)| *known == name);
            let kind = if reference.is_some() || OTHER_AGGREGATES.contains(&name) {
                FunctionKind::Aggregate
            } else if WINDOW_FUNCTIONS.contains(&name) {
                FunctionKind::Window
            } else if TABLE_FUNCTIONS.contains(&name) {
                FunctionKind::Table
            } else {
                FunctionKind::Scalar
            };
            FunctionInfo {
                name: name.to_owned(),
                schema: None,
                kind,
                signatures: reference.map_or_else(Vec::new, |(_, _, signatures, _)| {
                    signatures.iter().map(|text| (*text).to_owned()).collect()
                }),
                description: reference.map(|(_, description, ..)| (*description).to_owned()),
            }
        })
        .collect()
}

pub fn aggregate_functions() -> Vec<AggregateFunction> {
    FUNCTIONS
        .iter()
        .map(|(name, description, signatures, examples)| AggregateFunction {
            name: (*name).into(),
            description: (*description).into(),
            signatures: signatures.iter().map(|text| (*text).into()).collect(),
            examples: examples.iter().map(|text| (*text).into()).collect(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use polars::prelude::*;

    #[test]
    fn sql_functions_are_polars_own_with_kinds() {
        let functions = sql_functions();
        let kind = |name: &str| functions.iter().find(|f| f.name == name).map(|f| f.kind);
        assert_eq!(kind("sum"), Some(FunctionKind::Aggregate));
        assert_eq!(kind("var_samp"), Some(FunctionKind::Aggregate));
        assert_eq!(kind("row_number"), Some(FunctionKind::Window));
        assert_eq!(kind("read_parquet"), Some(FunctionKind::Table));
        assert_eq!(kind("upper"), Some(FunctionKind::Scalar));
        // DuckDB's own functions are not offered.
        for duckdb in ["list_transform", "regexp_extract_all", "read_csv_auto", "epoch_ms"] {
            assert_eq!(kind(duckdb), None, "{duckdb}");
        }
        assert!(functions.windows(2).all(|pair| pair[0].name < pair[1].name));
        let sum = functions.iter().find(|f| f.name == "sum").unwrap();
        assert_eq!(sum.signatures, ["sum(x)"]);
    }

    #[test]
    fn every_example_aggregates_in_polars() {
        let frame = df!(
            "x" => [1.0, 2.0, 4.0],
            "y" => [2.0, 1.0, 5.0],
            "label" => ["b", "a", "b"],
        )
        .unwrap();
        let functions = aggregate_functions();
        assert!(functions.windows(2).all(|pair| pair[0].name < pair[1].name));
        for function in functions {
            for example in &function.examples {
                let expression = super::super::expressions::parse_sql_expression(example)
                    .unwrap_or_else(|error| panic!("{example}: {error}"));
                let result = frame
                    .clone()
                    .lazy()
                    .select([expression.alias("value")])
                    .collect()
                    .unwrap_or_else(|error| panic!("{example}: {error}"));
                assert_eq!(result.height(), 1, "{example} aggregates to one row");
            }
        }
    }
}
