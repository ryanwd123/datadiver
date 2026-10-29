//! Group-by, pivot, and totals over a store, in the store's own engine.
//! `sql` builds DuckDB SQL, and `lazy` builds Polars lazy frames.

pub mod lazy;
pub mod sql;

use crate::domain::documents::{QueryConfig, SlicerDefinition, ValueFilter};
use crate::domain::sql::SqlTrace;
use crate::infrastructure::store::{Store, ViewQuery};

pub use sql::{
    EvaluationClock, TotalsRow, dimension_editor_expression, editor_expression, is_total_column,
};

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
