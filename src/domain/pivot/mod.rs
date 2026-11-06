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
/// first `limit`. The result is in the source's engine, and its `cut_at` says
/// whether the query had more than `limit` rows.
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
    // One row past the limit shows whether the query goes on.
    let fetch = limit.saturating_add(1).min(i64::MAX as u64);
    let mut store = execute_engine_query(source, config, slicers, filters, clock, fetch, result_view, trace)?;
    if store.row_count() as u64 > limit {
        store.cut_to(limit as usize)?;
    }
    Ok(store)
}

#[allow(clippy::too_many_arguments)]
fn execute_engine_query(
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
    fn results_are_cut_only_when_the_query_has_more_rows_than_the_limit() {
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("regions.csv");
        std::fs::write(&path, "region,amount\nNorth,1\nSouth,2\nEast,3\nWest,4\nNorth,5\n").unwrap();
        let config = QueryConfig {
            group_by: vec![Dimension::Column("region".into())],
            aggregations: vec![aggregation("amount", AggregationOp::Sum)],
            ..Default::default()
        };
        let clock = EvaluationClock::system();
        for engine in Engine::ALL {
            let source = sources
                .load(&path, ReadOptions { engine, ..ReadOptions::default() }, LoadMode::Check)
                .unwrap();
            let run = |limit| execute_filtered(&source, &config, &[], &[], &clock, limit, &mut SqlTrace::default()).unwrap();
            let ordered = |store: &Store| {
                let ids = store.initial_row_ids().unwrap();
                store.fetch_row_ids(&ids).unwrap().into_iter().map(|row| row.values).collect::<Vec<_>>()
            };
            // Four groups fit a limit of four exactly.
            let whole = run(4);
            assert_eq!((whole.row_count(), whole.cut_at()), (4, None), "{engine:?}");
            let cut = run(3);
            assert_eq!((cut.row_count(), cut.cut_at()), (3, Some(3)), "{engine:?}");
            assert_eq!(ordered(&cut), ordered(&whole)[..3], "{engine:?}");
        }
    }

    /// Reading text as categoricals changes the column type only: grouping,
    /// pivots, text aggregations, conditions, searches, filters and sorts
    /// give the same rows as plain text.
    #[test]
    fn categorical_text_gives_the_same_rows_as_plain_text() {
        use crate::domain::documents::{Condition, ConditionOp, ConditionValue, ValueFilter};
        use crate::infrastructure::store::{ColumnFilter, ViewQuery};
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("orders.csv");
        let mut text = String::from("region,kind,city,amount\n");
        for n in 0..50 {
            let region = ["North", "South", "East"][n % 3];
            let kind = ["a", "b"][n % 2];
            let city = if n % 7 == 3 { String::new() } else { ["Oslo", "Lima", "Kyiv", "Bern"][(n * 31) % 4].into() };
            text.push_str(&format!("{region},{kind},{city},{}\n", (n * 13) % 17));
        }
        std::fs::write(&path, text).unwrap();
        let load = |categorical| {
            let options = ReadOptions { engine: Engine::Polars, categorical, ..ReadOptions::default() };
            sources.load(&path, options, LoadMode::Check).unwrap()
        };
        let (plain, categorical) = (load(false), load(true));
        assert_eq!(categorical.schema()[0].sql_type, "ENUM");
        assert_eq!(plain.schema()[0].sql_type, "VARCHAR");
        // A read of each kind is kept apart.
        assert_eq!(load(true).schema()[0].sql_type, "ENUM");

        let clock = EvaluationClock::system();
        let mut north = aggregation("amount", AggregationOp::Sum);
        north.name = Some("north".into());
        north.conditions = vec![Condition {
            column: "region".into(),
            operation: ConditionOp::Equal,
            values: vec![ConditionValue::Literal(ScalarKey::String("North".into()))],
        }];
        let aggregations = vec![
            aggregation("amount", AggregationOp::Sum),
            aggregation("city", AggregationOp::Minimum),
            aggregation("city", AggregationOp::Maximum),
            aggregation("city", AggregationOp::First),
            aggregation("city", AggregationOp::NonNullCount),
            aggregation("city", AggregationOp::DistinctNonNullCount),
            aggregation("city", AggregationOp::UniqueValuesList),
            north,
            Aggregation::sql_expression("max(upper(city))".into(), Some("upper".into())),
            Aggregation::sql_expression("sum(case when city = 'Oslo' then 1 else 0 end)".into(), Some("oslo".into())),
        ];
        let filters = [ValueFilter { target: Dimension::Column("kind".into()), values: vec![Some("a".into())], exclude: false }];
        let upper = [ValueFilter {
            target: Dimension::Expression { sql: "upper(region)".into() },
            values: vec![Some("NORTH".into())],
            exclude: true,
        }];
        let region = Dimension::Column("region".into());
        let city = Dimension::Expression { sql: "upper(city) AS city".into() };
        for (group_by, pivot_by, filters) in [
            (&region, vec![], &[][..]),
            (&region, vec![Dimension::Column("kind".into())], &[][..]),
            (&region, vec![], &filters[..]),
            (&city, vec![], &upper[..]),
            (&region, vec![city.clone()], &[][..]),
        ] {
            let config = QueryConfig {
                group_by: vec![group_by.clone()],
                pivot_by,
                aggregations: aggregations.clone(),
                ..Default::default()
            };
            let run = |store: &Store| execute_filtered(store, &config, &[], filters, &clock, 1000, &mut SqlTrace::default()).unwrap();
            assert_eq!(rows(&run(&plain)), rows(&run(&categorical)), "{config:?}");
        }

        let view = |search: &str, filters: Vec<ColumnFilter>, sort: &str| ViewQuery {
            tokens: crate::domain::query::parse(search).unwrap(),
            filters,
            sort: vec![SortKey { column: sort.into(), descending: false }],
            ..ViewQuery::default()
        };
        let card = |column: &str, values: Vec<ScalarKey>, condition: Option<&str>| ColumnFilter {
            column: column.into(),
            values,
            condition: condition.map(Into::into),
            sql: false,
        };
        for query in [
            view("nor", Vec::new(), "city"),
            view("region=South -lima", Vec::new(), "amount"),
            view("city:o", vec![card("region", vec![ScalarKey::String("East".into())], None)], "region"),
            view("", vec![card("city", vec![ScalarKey::Null], None)], "region"),
            view("", vec![card("city", Vec::new(), Some(":ky"))], "city"),
            view("", vec![card("city", Vec::new(), Some("bern..lima"))], "city"),
            view("", vec![ColumnFilter { sql: true, ..card("", Vec::new(), Some("city = 'Oslo' or upper(region) like 'E%'")) }], "city"),
            view("", vec![ColumnFilter { sql: true, ..card("", Vec::new(), Some("length(city) > 3 and city <> 'Kyiv'")) }], "city"),
        ] {
            let ids = |store: &Store| store.query_row_ids(&query).unwrap();
            assert_eq!(ids(&plain), ids(&categorical), "{query:?}");
        }
        let slicer = |store: &Store| store.slicer_values("city", &[], &filters).unwrap();
        assert_eq!(slicer(&plain), slicer(&categorical));
        let names = ["region".to_string(), "city".to_string()];
        assert_eq!(plain.distinct_values(&names, &[]).unwrap(), categorical.distinct_values(&names, &[]).unwrap());
        assert_eq!(plain.top_values("city", 3).unwrap(), categorical.top_values("city", 3).unwrap());
        assert_eq!(plain.column_bounds("city").unwrap(), categorical.column_bounds("city").unwrap());
        let values = ["Oslo".to_string(), "Bern".to_string()];
        // Counts come in no particular order.
        let counts = |store: &Store| {
            let mut counts = store.value_counts(&ViewQuery::default(), "city", &values).unwrap();
            counts.sort();
            counts
        };
        assert_eq!(counts(&plain), counts(&categorical));
    }

    #[test]
    fn unique_values_keep_first_seen_order_across_engines() {
        use crate::domain::documents::{Condition, ConditionOp, ConditionValue};
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("visits.csv");
        // Values repeat out of sorted order, with nulls, and Gold never has
        // a second-half row.
        let mut text = String::from("team,half,city\n");
        for n in 0..60 {
            let team = ["Red", "Blue", "Gold"][n % 3];
            let half = if team == "Gold" { "First" } else { ["First", "Second"][n % 2] };
            let city = if n % 7 == 3 { String::new() } else { ["Oslo", "Lima", "Kyiv", "Bern", "Doha"][(n * 7919) % 5].into() };
            text.push_str(&format!("{team},{half},{city}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let load = |engine| {
            sources
                .load(&path, ReadOptions { engine, ..ReadOptions::default() }, LoadMode::Check)
                .unwrap()
        };
        let (duckdb, polars) = (load(Engine::DuckDb), load(Engine::Polars));
        let mut cities = aggregation("city", AggregationOp::UniqueValuesList);
        cities.name = Some("cities".into());
        let mut second = cities.clone();
        second.name = Some("second half".into());
        second.conditions = vec![Condition {
            column: "half".into(),
            operation: ConditionOp::Equal,
            values: vec![ConditionValue::Literal(ScalarKey::String("Second".into()))],
        }];
        let clock = EvaluationClock::system();
        for pivot_by in [vec![], vec![Dimension::Column("half".into())]] {
            let config = QueryConfig {
                group_by: vec![Dimension::Column("team".into())],
                pivot_by,
                aggregations: vec![cities.clone(), second.clone()],
                ..Default::default()
            };
            let run = |store: &Store| execute_filtered(store, &config, &[], &[], &clock, 1000, &mut SqlTrace::default()).unwrap();
            let duckdb_rows = rows(&run(&duckdb));
            assert_eq!(duckdb_rows, rows(&run(&polars)), "{config:?}");
            if config.pivot_by.is_empty() {
                // Not sorted: first-seen order differs from it.
                let red = &duckdb_rows.1.iter().find(|row| row[0].as_deref() == Some("Red")).unwrap()[1];
                assert_ne!(red.as_deref(), Some("[Bern, Doha, Kyiv, Lima, Oslo]"));
                let custom = QueryConfig {
                    aggregations: config
                        .aggregations
                        .iter()
                        .map(|a| Aggregation::sql_expression(editor_expression(a, duckdb.schema(), Engine::DuckDb).unwrap(), None))
                        .collect(),
                    ..config.clone()
                };
                let edited = execute_filtered(&duckdb, &custom, &[], &[], &clock, 1000, &mut SqlTrace::default()).unwrap();
                assert_eq!(rows(&edited), duckdb_rows, "editor SQL");
            }
        }
    }

    #[test]
    fn percentiles_match_across_engines_for_every_interpolation() {
        use crate::domain::documents::{Condition, ConditionOp, ConditionValue, Interpolation};
        let home = tempfile::tempdir().unwrap();
        let sources = Sources::for_profile(home.path());
        let path = home.path().join("scores.csv");
        // Groups of different sizes, with nulls, so (n - 1) * q lands on and
        // between ranks.
        let mut text = String::from("team,half,score\n");
        for n in 0..47 {
            let team = ["Red", "Blue", "Gold"][n % 3];
            let half = ["First", "Second"][n % 2];
            let score = if n % 9 == 4 { String::new() } else { ((n * 37) % 23).to_string() };
            text.push_str(&format!("{team},{half},{score}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let load = |engine| {
            sources
                .load(&path, ReadOptions { engine, ..ReadOptions::default() }, LoadMode::Check)
                .unwrap()
        };
        let (duckdb, polars) = (load(Engine::DuckDb), load(Engine::Polars));
        let clock = EvaluationClock::system();
        for interpolation in [
            Interpolation::Linear,
            Interpolation::Lower,
            Interpolation::Higher,
            Interpolation::Nearest,
            Interpolation::Midpoint,
        ] {
            for percentile in [0.0, 10.0, 50.0, 95.0, 100.0] {
                let mut score = aggregation("score", AggregationOp::Percentile);
                score.percentile = Some(percentile);
                score.interpolation = Some(interpolation.clone());
                score.name = Some("all".into());
                // The same percentile again, over only the first half.
                let mut first_half = score.clone();
                first_half.name = Some("first half".into());
                first_half.conditions = vec![Condition {
                    column: "half".into(),
                    operation: ConditionOp::Equal,
                    values: vec![ConditionValue::Literal(ScalarKey::String("First".into()))],
                }];
                for pivot_by in [vec![], vec![Dimension::Column("half".into())]] {
                    let config = QueryConfig {
                        group_by: vec![Dimension::Column("team".into())],
                        pivot_by,
                        aggregations: vec![score.clone(), first_half.clone()],
                        totals: true,
                        ..Default::default()
                    };
                    let run = |store: &Store| {
                        execute_filtered(store, &config, &[], &[], &clock, 1000, &mut SqlTrace::default()).unwrap()
                    };
                    let case = format!("{interpolation:?} {percentile} pivot {}", !config.pivot_by.is_empty());
                    // Linear interpolation may round its last digit differently.
                    let same = |a: &[Option<String>], b: &[Option<String>]| {
                        a.len() == b.len()
                            && a.iter().zip(b).all(|(a, b)| match (a, b) {
                                (Some(a), Some(b)) => match (a.parse::<f64>(), b.parse::<f64>()) {
                                    (Ok(a), Ok(b)) => (a - b).abs() < 1e-9,
                                    _ => a == b,
                                },
                                _ => a == b,
                            })
                    };
                    let (names, left) = rows(&run(&duckdb));
                    let (other, right) = rows(&run(&polars));
                    assert_eq!(names, other, "{case}");
                    assert!(
                        left.len() == right.len() && left.iter().zip(&right).all(|(a, b)| same(a, b)),
                        "{case}\n{left:?}\n{right:?}"
                    );
                    if config.pivot_by.is_empty() {
                        // The SQL the aggregation editor shows gives the same values.
                        let custom = QueryConfig {
                            aggregations: config
                                .aggregations
                                .iter()
                                .map(|a| {
                                    let sql = editor_expression(a, duckdb.schema(), Engine::DuckDb).unwrap();
                                    Aggregation::sql_expression(sql, None)
                                })
                                .collect(),
                            ..config.clone()
                        };
                        let edited = execute_filtered(&duckdb, &custom, &[], &[], &clock, 1000, &mut SqlTrace::default());
                        assert_eq!(rows(&edited.unwrap()).1, left, "editor SQL {case}");
                    }
                    let totals = |store: &Store| {
                        let row = execute_totals(store, &config, &[], &[], &clock).unwrap();
                        row.into_iter().unzip::<_, _, Vec<_>, Vec<_>>()
                    };
                    let ((names, left), (other, right)) = (totals(&duckdb), totals(&polars));
                    assert_eq!(names, other, "totals {case}");
                    assert!(same(&left, &right), "totals {case}\n{left:?}\n{right:?}");
                }
            }
        }
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
