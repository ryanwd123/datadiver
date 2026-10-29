//! Group-by, pivot, and totals over a store, in the store's own engine.
//! `sql` builds DuckDB SQL, and `lazy` builds Polars lazy frames.

pub mod lazy;
pub mod sql;

use crate::domain::documents::{QueryConfig, SlicerDefinition, ValueFilter};
use crate::domain::sql::SqlTrace;
use crate::infrastructure::store::{Store, ViewQuery};

pub use sql::{EvaluationClock, TotalsRow, dimension_editor_expression, is_total_column};
use crate::domain::documents::Aggregation;
use crate::infrastructure::store::{ColumnSchema, Engine};

/// An aggregation as SQL for its editor, in `engine`'s dialect, so a
/// structured aggregation turned into a custom one runs where it came from.
pub fn editor_expression(
    aggregation: &Aggregation,
    schema: &[ColumnSchema],
    engine: Engine,
) -> Result<String, String> {
    match engine {
        Engine::DuckDb => sql::editor_expression(aggregation, schema),
        Engine::Polars => lazy::editor_expression(aggregation, schema),
    }
}

pub fn execute(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
) -> Result<Store, String> {
    execute_with_clock(source, config, slicers, &EvaluationClock::system())
}

pub fn execute_with_clock(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
) -> Result<Store, String> {
    execute_traced(source, config, slicers, clock, &mut SqlTrace::default())
}

pub fn execute_traced(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    trace: &mut SqlTrace,
) -> Result<Store, String> {
    execute_traced_limited(
        source,
        config,
        slicers,
        clock,
        crate::domain::documents::DEFAULT_QUERY_LIMIT,
        trace,
    )
}

pub fn execute_traced_limited(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<Store, String> {
    execute_filtered(source, config, slicers, &[], clock, limit, trace)
}

/// Runs the query over source rows matching both the slicers and the value filters.
pub fn execute_filtered(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<Store, String> {
    execute_query(source, config, slicers, filters, clock, limit, None, trace)
}

/// The query's result rows, narrowed by a view over a previous result before the
/// limit applies: `store` is that previous result, giving the result schema.
pub type ResultView<'a> = (&'a Store, &'a ViewQuery);

/// Like [`execute_filtered`], but with `result_view` the result search and sort
/// run inside the query, so they see every result row instead of only the
/// first `limit`. The result is in the source's engine.
#[allow(clippy::too_many_arguments)]
pub fn execute_query(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    result_view: Option<ResultView<'_>>,
    trace: &mut SqlTrace,
) -> Result<Store, String> {
    match source {
        Store::DuckDb(source) => {
            let result_view = match result_view {
                Some((store, view)) => Some((store.duckdb()?, view)),
                None => None,
            };
            sql::execute_query(source, config, slicers, filters, clock, limit, result_view, trace)
                .map(Store::from)
        }
        Store::Polars(source) => {
            let result_view = match result_view {
                Some((store, view)) => Some((
                    store.as_polars().ok_or("the result is not a Polars store")?,
                    view,
                )),
                None => None,
            };
            lazy::execute_query(source, config, slicers, filters, clock, limit, result_view, trace)
                .map(Store::from)
        }
    }
}

/// The grand total row of a query with totals; see [`sql::execute_totals`].
pub fn execute_totals(
    source: &Store,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
) -> Result<TotalsRow, String> {
    match source {
        Store::DuckDb(source) => sql::execute_totals(source, config, slicers, filters, clock),
        Store::Polars(source) => lazy::execute_totals(source, config, slicers, filters, clock),
    }
}

pub fn validate_dimensions(source: &Store, config: &QueryConfig) -> Result<(), String> {
    match source {
        Store::DuckDb(source) => sql::validate_dimensions(source, config),
        Store::Polars(source) => lazy::validate_dimensions(source, config),
    }
}

#[cfg(test)]
mod parity_tests {
    //! The same views over the same file in both engines give the same rows.

    use super::*;
    use crate::domain::documents::{Aggregation, AggregationOp, Dimension, ScalarKey, SortKey};
    use crate::infrastructure::source_database::LoadMode;
    use crate::infrastructure::sources::{ReadOptions, Sources};
    use crate::infrastructure::store::Engine;

    fn aggregation(column: &str, operation: AggregationOp) -> Aggregation {
        Aggregation {
            sql: None,
            source_column: column.into(),
            operation,
            name: None,
            conditions: Vec::new(),
            percentile: None,
            interpolation: None,
            ddof: None,
        }
    }

    /// Every row of `store` as text, sorted, so engines that order groups
    /// differently still compare equal.
    fn rows(store: &Store) -> (Vec<String>, Vec<Vec<Option<String>>>) {
        let names = store.schema().iter().map(|column| column.name.clone()).collect();
        let ids = store.initial_row_ids().unwrap();
        let mut rows = store
            .fetch_row_ids(&ids)
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect::<Vec<_>>();
        rows.sort();
        (names, rows)
    }

    #[test]
    fn group_by_pivot_slicers_search_and_totals_match_across_engines() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("sales.csv");
        let mut text = String::from("region,product,amount,units\n");
        for n in 0..500 {
            let region = ["North", "South", "East", "West"][n % 4];
            let product = ["A", "B", "C"][n % 3];
            let units = if n % 7 == 0 { String::new() } else { (n % 5).to_string() };
            text.push_str(&format!("{region},{product},{},{units}\n", n * 3 % 101));
        }
        std::fs::write(&path, text).unwrap();
        let load = |engine| {
            sources
                .load(&path, ReadOptions { engine, ..ReadOptions::default() }, LoadMode::Check)
                .unwrap()
        };
        let duckdb = load(Engine::DuckDb);
        let polars = load(Engine::Polars);
        let slicer = SlicerDefinition {
            sql: false,
            condition: None,
            column: "region".into(),
            selected: vec![ScalarKey::String("North".into()), ScalarKey::String("East".into())],
        };
        let configs = [
            QueryConfig {
                group_by: vec![Dimension::Column("region".into())],
                aggregations: vec![
                    aggregation("amount", AggregationOp::Sum),
                    aggregation("amount", AggregationOp::RowCount),
                    aggregation("units", AggregationOp::NonNullCount),
                    aggregation("amount", AggregationOp::Minimum),
                    aggregation("amount", AggregationOp::Maximum),
                    aggregation("units", AggregationOp::DistinctNonNullCount),
                ],
                ..Default::default()
            },
            QueryConfig {
                group_by: vec![Dimension::Column("region".into()), Dimension::Column("product".into())],
                aggregations: vec![aggregation("units", AggregationOp::Sum)],
                result_sort: vec![SortKey {
                    column: "region".into(),
                    descending: true,
                }],
                ..Default::default()
            },
            QueryConfig {
                group_by: vec![Dimension::Column("region".into())],
                pivot_by: vec![Dimension::Column("product".into())],
                aggregations: vec![aggregation("amount", AggregationOp::Sum)],
                totals: true,
                ..Default::default()
            },
        ];
        let clock = EvaluationClock::system();
        for config in &configs {
            for slicers in [&[][..], std::slice::from_ref(&slicer)] {
                let run = |store: &Store| {
                    execute_filtered(store, config, slicers, &[], &clock, 1000, &mut SqlTrace::default()).unwrap()
                };
                assert_eq!(rows(&run(&duckdb)), rows(&run(&polars)), "{config:?} {slicers:?}");
                let totals = |store: &Store| execute_totals(store, config, slicers, &[], &clock).unwrap();
                assert_eq!(totals(&duckdb), totals(&polars), "totals of {config:?}");
            }
        }
        // Table views: search, sort, and slicer filters give the same rows in order.
        let view = ViewQuery {
            tokens: crate::domain::query::parse("north").unwrap(),
            sort: vec![SortKey {
                column: "amount".into(),
                descending: false,
            }],
            ..Default::default()
        };
        let ordered = |store: &Store| {
            let ids = store.query_row_ids(&view).unwrap();
            store
                .fetch_row_ids(&ids)
                .unwrap()
                .into_iter()
                .map(|row| row.values[2].clone())
                .collect::<Vec<_>>()
        };
        let amounts = ordered(&duckdb);
        assert_eq!(amounts.len(), 125);
        assert_eq!(amounts, ordered(&polars));
    }
}
