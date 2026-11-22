//! Differential test of the query engines: the same random table, read by
//! DuckDB and by Polars, must give the same rows for the same random
//! searches, filters, sorts and groupings, and no view may lose or repeat a
//! row.

use proptest::prelude::*;

use crate::domain::documents::{
    Aggregation, AggregationOp, Dimension, QueryConfig, ScalarKey, SortKey, ValueFilter,
};
use crate::domain::pivot::{EvaluationClock, execute_filtered};
use crate::domain::sql::SqlTrace;
use crate::infrastructure::source_database::LoadMode;
use crate::infrastructure::sources::{ReadOptions, Sources};
use crate::infrastructure::store::{ColumnFilter, Engine, Store, ViewQuery};

/// Text values, chosen for case, accents, quoting, SQL wildcards and words
/// some readers treat as null.
const NAMES: &[&str] = &[
    "North", "north", "NORTH", "Nörth", "South East", "O'Brien", "a,b", "50%", "x_y", "日本",
    " pad ", "Straße", "İstanbul", "NA", "null", "\"quoted\"", "line\nbreak", "-",
];
const PRICES: &[&str] = &["0.5", "-1.25", "3", "1e3", "0", "-0.0", "123456.789", "2.5"];
const DAYS: &[&str] = &["2024-01-15", "2024-02-29", "2023-12-31", "1999-01-01", "2024-01-01"];

#[derive(Clone, Debug)]
struct Row {
    name: Option<&'static str>,
    amount: Option<i64>,
    price: Option<&'static str>,
    day: Option<&'static str>,
    flag: Option<bool>,
}

fn row() -> impl Strategy<Value = Row> {
    let amount = prop_oneof![
        3 => (-5i64..20).prop_map(Some),
        1 => Just(Some(i64::MAX)),
        1 => Just(Some(-1_000_000_000_000)),
        1 => Just(None),
    ];
    (
        proptest::option::weighted(0.85, proptest::sample::select(NAMES)),
        amount,
        proptest::option::weighted(0.85, proptest::sample::select(PRICES)),
        proptest::option::weighted(0.85, proptest::sample::select(DAYS)),
        proptest::option::weighted(0.85, any::<bool>()),
    )
        .prop_map(|(name, amount, price, day, flag)| Row { name, amount, price, day, flag })
}

const COLUMNS: &[&str] = &["id", "name", "amount", "price", "day", "flag"];

fn csv(rows: &[Row]) -> String {
    let mut text = COLUMNS.join(",");
    text.push('\n');
    for (index, row) in rows.iter().enumerate() {
        let name = row.name.map(|name| format!("\"{}\"", name.replace('"', "\"\""))).unwrap_or_default();
        let amount = row.amount.map(|n| n.to_string()).unwrap_or_default();
        let flag = row.flag.map(|flag| flag.to_string()).unwrap_or_default();
        text.push_str(&format!(
            "{},{name},{amount},{},{},{flag}\n",
            index + 1,
            row.price.unwrap_or_default(),
            row.day.unwrap_or_default(),
        ));
    }
    text
}

fn word() -> impl Strategy<Value = String> {
    proptest::sample::select(&[
        "nor", "NOR", "ö", "o'b", "50%", "%", "_", "x_y", "日", "ss", "straße", "ı", "i", "1", "2024",
        "-1", "0.5", "e3", "true", "a,b", "east", "na", "null", "\"", "\"south east\"", "'x'", "*",
    ][..])
    .prop_map(str::to_owned)
}

fn number() -> impl Strategy<Value = String> {
    proptest::sample::select(&["0", "1", "5", "-3", "0.5", "100", "1e3", "99999999999999999999", "-0", "2.5"][..])
        .prop_map(str::to_owned)
}

fn term() -> impl Strategy<Value = String> {
    let column = proptest::sample::select(&["id", "name", "amount", "price", "day", "flag", "Name", "AMOUNT", "missing"][..]);
    let operator = proptest::sample::select(&[":", "=", "!=", ">", "<", ">=", "<="][..]);
    let value = prop_oneof![
        word(),
        number(),
        proptest::sample::select(&["2024", "2024-01", "2024-01-15", "\"amount\"", "\"price\"", ""][..]).prop_map(str::to_owned),
    ];
    let comparison = proptest::sample::select(&[">", ">=", "<", "<=", "="][..]);
    prop_oneof![
        3 => word(),
        1 => word().prop_map(|word| format!("-{word}")),
        1 => (comparison, number()).prop_map(|(op, n)| format!("{op}{n}")),
        3 => (any::<bool>(), column, operator, value).prop_map(|(negate, column, op, value)| {
            format!("{}{column}{op}{value}", if negate { "-" } else { "" })
        }),
    ]
}

/// A search: terms, sometimes in `|` groups.
fn search() -> impl Strategy<Value = String> {
    proptest::collection::vec((term(), any::<u8>()), 0..4).prop_map(|terms| {
        let mut text = String::new();
        for (index, (term, glue)) in terms.into_iter().enumerate() {
            if index > 0 {
                text.push_str(if glue % 5 == 0 { " | " } else { " " });
            }
            text.push_str(&term);
        }
        text
    })
}

fn column_filter() -> impl Strategy<Value = ColumnFilter> {
    let condition = (
        proptest::sample::select(&["name", "amount", "price", "day", "flag"][..]),
        proptest::sample::select(&[
            "north", ":or", ">2", "1..10", "-3..0", "2024-01..2024-02", "<=0.5", "=", "!north", "null",
            "  ", "2024", ">=2024-01-15", "true", "a,b", "\"north\"",
        ][..]),
    )
        .prop_map(|(column, text)| ColumnFilter {
            column: column.into(),
            values: Vec::new(),
            condition: Some(text.into()),
            sql: false,
        });
    let names = proptest::collection::vec(proptest::option::of(proptest::sample::select(NAMES)), 1..3).prop_map(|names| {
        ColumnFilter {
            column: "name".into(),
            values: names
                .into_iter()
                .map(|name| name.map_or(ScalarKey::Null, |name| ScalarKey::String(name.into())))
                .collect(),
            condition: None,
            sql: false,
        }
    });
    let amounts = proptest::collection::vec(proptest::option::of(-5i64..20), 1..3).prop_map(|amounts| ColumnFilter {
        column: "amount".into(),
        values: amounts
            .into_iter()
            .map(|amount| amount.map_or(ScalarKey::Null, |n| ScalarKey::Signed(n.to_string())))
            .collect(),
        condition: None,
        sql: false,
    });
    let sql = proptest::sample::select(&[
        "amount > 2",
        "name like 'N%'",
        "price is null",
        "lower(name) = 'north'",
        "amount between 1 and 5 or flag",
        "coalesce(amount, 0) % 2 = 0",
        "day >= DATE '2024-01-15'",
    ][..])
    .prop_map(|text| ColumnFilter { column: String::new(), values: Vec::new(), condition: Some(text.into()), sql: true });
    prop_oneof![3 => condition, 2 => names, 2 => amounts, 1 => sql]
}

fn value_filter() -> impl Strategy<Value = ValueFilter> {
    (
        proptest::sample::select(&["name", "flag", "day"][..]),
        proptest::collection::vec(
            proptest::option::of(proptest::sample::select(&["North", "true", "false", "2024-01-15", "NA", "日本"][..])),
            1..3,
        ),
        any::<bool>(),
    )
        .prop_map(|(column, values, exclude)| ValueFilter {
            target: Dimension::Column(column.into()),
            values: values.into_iter().map(|value| value.map(str::to_owned)).collect(),
            exclude,
        })
}

fn sort() -> impl Strategy<Value = Vec<SortKey>> {
    proptest::collection::vec(
        (proptest::sample::select(&COLUMNS[1..]), any::<bool>())
            .prop_map(|(column, descending)| SortKey { column: column.into(), descending, sql: false }),
        0..3,
    )
}

#[derive(Clone, Debug)]
struct Query {
    search: String,
    filters: Vec<ColumnFilter>,
    value_filters: Vec<ValueFilter>,
    sort: Vec<SortKey>,
    hidden: Vec<String>,
}

fn query() -> impl Strategy<Value = Query> {
    (
        search(),
        proptest::collection::vec(column_filter(), 0..3),
        proptest::collection::vec(value_filter(), 0..2),
        sort(),
        proptest::collection::vec(proptest::sample::select(&COLUMNS[1..]).prop_map(str::to_owned), 0..2),
    )
        .prop_map(|(search, filters, value_filters, sort, hidden)| Query { search, filters, value_filters, sort, hidden })
}

impl Query {
    fn view(&self) -> Option<ViewQuery> {
        Some(ViewQuery {
            tokens: crate::domain::query::parse(&self.search).ok()?,
            filters: self.filters.clone(),
            value_filters: self.value_filters.clone(),
            sort: self.sort.clone(),
            hidden: self.hidden.clone(),
        })
    }
}

/// The value of `id` in each row `store` shows for `view`, in order.
fn ids(store: &Store, view: &ViewQuery) -> Result<Vec<usize>, String> {
    let row_ids = store.query_row_ids(view)?;
    let rows = store.fetch_row_ids(&row_ids)?;
    assert_eq!(rows.len(), row_ids.len(), "fetching {} rows gave {}", row_ids.len(), rows.len());
    Ok(rows
        .into_iter()
        .map(|row| row.values[0].as_deref().expect("id is never null").parse().expect("id is a number"))
        .collect())
}

fn load(sources: &Sources, path: &std::path::Path, engine: Engine) -> Store {
    sources
        .load(path, ReadOptions { engine, ..ReadOptions::default() }, LoadMode::Force)
        .unwrap_or_else(|error| panic!("{engine:?} could not read the table: {error}"))
}

/// Each row's sort key, from the generated rows rather than either engine's
/// rendering, so the engines' orders compare even where they render values
/// differently.
fn sort_keys(rows: &[Row], order: &[usize], sort: &[SortKey]) -> Vec<Vec<String>> {
    order
        .iter()
        .map(|id| {
            let row = &rows[id - 1];
            sort.iter()
                .map(|key| match key.column.as_str() {
                    "name" => format!("{:?}", row.name),
                    "amount" => format!("{:?}", row.amount),
                    "price" => format!("{:?}", row.price.map(|p| p.parse::<f64>().unwrap())),
                    "day" => format!("{:?}", row.day),
                    "flag" => format!("{:?}", row.flag),
                    other => unreachable!("{other}"),
                })
                .collect()
        })
        .collect()
}

fn sorted(mut ids: Vec<usize>) -> Vec<usize> {
    ids.sort_unstable();
    ids
}

fn assert_unique(ids: &[usize], all: usize, context: &str) {
    let mut seen = vec![false; all + 1];
    for &id in ids {
        assert!((1..=all).contains(&id), "{context}: id {id} is not a row of the table");
        assert!(!seen[id], "{context}: row {id} shown twice");
        seen[id] = true;
    }
}

/// Every row of a grouped result, sorted, with numbers rounded so the
/// engines' float formatting compares equal.
fn grouped_rows(store: &Store) -> Vec<Vec<Option<String>>> {
    let ids = store.initial_row_ids().unwrap();
    let mut rows = store
        .fetch_row_ids(&ids)
        .unwrap()
        .into_iter()
        .map(|row| {
            row.values
                .into_iter()
                .map(|value| {
                    value.map(|text| match text.parse::<f64>() {
                        // Adding zero turns -0 into 0, which compare equal anyway.
                        Ok(number) if text.chars().any(|c| c.is_ascii_digit()) => format!("{:.6}", number + 0.0),
                        _ => text,
                    })
                })
                .collect()
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows
}

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

fn check(rows: &[Row], queries: &[Query], group_by: &str) {
    let home = tempfile::tempdir().unwrap();
    let sources = Sources::for_profile(home.path());
    let path = home.path().join("table.csv");
    std::fs::write(&path, csv(rows)).unwrap();
    let duckdb = load(&sources, &path, Engine::DuckDb);
    let polars = load(&sources, &path, Engine::Polars);

    // The whole file is read, once.
    for (engine, store) in [(Engine::DuckDb, &duckdb), (Engine::Polars, &polars)] {
        assert_eq!(store.row_count(), rows.len(), "{engine:?} row count");
        let all = ids(store, &ViewQuery::default()).unwrap();
        assert_eq!(all, (1..=rows.len()).collect::<Vec<_>>(), "{engine:?} reads every row in file order");
    }
    let types = |store: &Store| store.schema().iter().map(|c| (c.name.clone(), c.sql_type.clone())).collect::<Vec<_>>();
    assert_eq!(types(&duckdb), types(&polars), "the engines read different column types");

    for query in queries {
        let Some(view) = query.view() else { continue };
        let context = format!("{query:?}");
        let (left, right) = (ids(&duckdb, &view), ids(&polars, &view));
        let (left, right) = match (left, right) {
            (Ok(left), Ok(right)) => (left, right),
            (Err(_), Err(_)) => continue,
            // Raw SQL meets each engine's own dialect, so one may refuse what
            // the other casts implicitly; the user sees the error.
            _ if query.filters.iter().any(|filter| filter.sql) => continue,
            (left, right) => panic!("{context}: only one engine failed\n  DuckDB: {left:?}\n  Polars: {right:?}"),
        };
        assert_unique(&left, rows.len(), &format!("DuckDB {context}"));
        assert_unique(&right, rows.len(), &format!("Polars {context}"));
        assert_eq!(sorted(left.clone()), sorted(right.clone()), "{context}: the engines keep different rows");
        if !query.sort.is_empty() {
            assert_eq!(
                sort_keys(rows, &left, &query.sort),
                sort_keys(rows, &right, &query.sort),
                "{context}: the engines sort differently"
            );
        }

        // A plain word and its negation split the rows between them.
        if let Some(word) = query.search.split_whitespace().next().filter(|w| !w.starts_with('-') && !w.contains(['>', '<', '=', ':', '|', '"']))
        {
            for store in [&duckdb, &polars] {
                let only = |search: String| ids(store, &ViewQuery { tokens: crate::domain::query::parse(&search).unwrap(), ..ViewQuery::default() });
                if let (Ok(with), Ok(without)) = (only(word.to_owned()), only(format!("-{word}"))) {
                    let mut both = with.clone();
                    both.extend(&without);
                    assert_eq!(sorted(both), (1..=rows.len()).collect::<Vec<_>>(), "{:?}: {word} and -{word} do not split the rows", store.engine());
                }
            }
        }

        // Another term only narrows a search without `|`.
        if !query.search.contains('|') && !query.search.is_empty() {
            let mut fewer = view.clone();
            fewer.tokens.pop();
            for store in [&duckdb, &polars] {
                if let (Ok(narrow), Ok(wide)) = (ids(store, &view), ids(store, &fewer)) {
                    let wide = sorted(wide);
                    assert!(
                        narrow.iter().all(|id| wide.binary_search(id).is_ok()),
                        "{:?} {context}: the last term added rows",
                        store.engine()
                    );
                }
            }
        }

        // Grouping the kept rows counts each exactly once.
        let config = QueryConfig {
            group_by: vec![Dimension::Column(group_by.into())],
            aggregations: vec![
                aggregation("id", AggregationOp::RowCount),
                aggregation("amount", AggregationOp::Sum),
                aggregation("amount", AggregationOp::Minimum),
                aggregation("price", AggregationOp::Maximum),
                aggregation("name", AggregationOp::DistinctNonNullCount),
                aggregation("price", AggregationOp::NonNullCount),
            ],
            ..Default::default()
        };
        let clock = EvaluationClock::system();
        let group = |store: &Store| {
            execute_filtered(store, &config, &[], &query.value_filters, &clock, 100_000, &mut SqlTrace::default())
        };
        match (group(&duckdb), group(&polars)) {
            (Ok(left), Ok(right)) => {
                let (left, right) = (grouped_rows(&left), grouped_rows(&right));
                assert_eq!(left, right, "{context}: grouping by {group_by} differs");
                // Row counts are the second column and cover every kept row.
                let kept = ids(&duckdb, &ViewQuery { value_filters: query.value_filters.clone(), ..ViewQuery::default() });
                if let Ok(kept) = kept {
                    let counted: f64 = left.iter().map(|row| row[1].as_deref().unwrap().parse::<f64>().unwrap()).sum();
                    assert_eq!(counted as usize, kept.len(), "{context}: group counts do not add up to the kept rows");
                }
            }
            (Err(_), Err(_)) => {}
            (left, right) => panic!(
                "{context}: grouping by {group_by} failed in one engine\n  DuckDB: {:?}\n  Polars: {:?}",
                left.err(),
                right.err()
            ),
        }
    }
}

proptest! {
    #![proptest_config(super::config(400))]

    #[test]
    #[ignore = "stress test: run with scripts/stress.ps1"]
    fn stress_engines_agree(
        rows in proptest::collection::vec(row(), 0..30),
        queries in proptest::collection::vec(query(), 1..6),
        group_by in proptest::sample::select(&["name", "flag", "day", "amount"][..]),
    ) {
        check(&rows, &queries, group_by);
    }
}
