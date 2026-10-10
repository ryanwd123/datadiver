use std::path::{Path, PathBuf};
use std::time::Instant;

use datadiver::domain::documents::{Aggregation, AggregationOp, QueryConfig, SortKey};
use datadiver::domain::frame::ResultSnapshot;
use datadiver::domain::query::Token;
use datadiver::domain::selection::{Cell, Selection};
use datadiver::domain::{pivot, statistics};
use datadiver::infrastructure::duckdb_store::{DuckDbStore, ViewQuery};
use datadiver::infrastructure::source_database::{LoadMode, SourceDatabase};
use duckdb::Connection;
use serde_json::json;

fn generate(directory: &Path) {
    std::fs::create_dir_all(directory).unwrap();
    let connection = Connection::open_in_memory().unwrap();
    for (name, count, width) in [
        ("10k", 10_000, 5),
        ("100k", 100_000, 5),
        ("1m", 1_000_000, 5),
        ("wide", 100_000, 100),
    ] {
        let extra = (5..width)
            .map(|i| format!(", (range % {})::DOUBLE AS col_{i}", i + 1))
            .collect::<String>();
        connection.execute_batch(&format!("CREATE OR REPLACE TABLE fixture AS SELECT range::BIGINT AS id, 'region_' || (range % 10)::VARCHAR AS region, (range % 10000)::DOUBLE / 10 AS amount, 'label_' || (range % 100)::VARCHAR AS label, CASE WHEN range % 7 = 0 THEN NULL ELSE range::BIGINT END AS nullable{extra} FROM range({count})")).unwrap();
        for (extension, options) in [("csv", "FORMAT CSV, HEADER"), ("parquet", "FORMAT PARQUET")] {
            let path = directory.join(format!("{name}.{extension}"));
            let path_sql = path.to_string_lossy().replace('\'', "''");
            connection
                .execute_batch(&format!("COPY fixture TO '{path_sql}' ({options})"))
                .unwrap();
        }
    }
}

fn measure<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    println!(
        "{}",
        json!({"metric": name, "ms": start.elapsed().as_secs_f64() * 1000., "memory": memory()})
    );
    result
}

#[cfg(windows)]
fn memory() -> serde_json::Value {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    let mut counters = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    if ok == 0 {
        return serde_json::Value::Null;
    }
    json!({"working_set": counters.working_set, "private_bytes": counters.private, "peak_working_set": counters.peak_working_set})
}

#[cfg(not(windows))]
fn memory() -> serde_json::Value {
    serde_json::Value::Null
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args[0] == "generate" {
        generate(Path::new(&args[1]));
        return;
    }
    let path = PathBuf::from(&args[1]);
    println!("{}", json!({"metric": "before", "memory": memory()}));
    let database = (args[0] != "memory").then(|| SourceDatabase::for_profile(Path::new(&args[2])));
    let store = measure("load", || match &database {
        Some(database) => database
            .load(
                &path,
                if args[0] == "force" {
                    LoadMode::Force
                } else {
                    LoadMode::Check
                },
            )
            .unwrap(),
        None => DuckDbStore::open_source(&path).unwrap(),
    });
    if let Some(database) = &database {
        let reused = measure("cached_same_process", || {
            database.load(&path, LoadMode::Check).unwrap()
        });
        assert_eq!(reused.table_sql(), store.table_sql());
    }
    for (name, tokens, sort) in [
        ("all_rows", vec![], vec![]),
        ("search", vec![Token::Contains("label_42".into())], vec![]),
        (
            "sort",
            vec![],
            vec![SortKey {
                column: "amount".into(),
                descending: true,
            }],
        ),
    ] {
        let rows = measure(name, || {
            store
                .query_view_rows(&ViewQuery {
                    hidden: Vec::new(),
                    tokens,
                    filters: vec![],
                    value_filters: vec![],
                    sort,
                })
                .unwrap()
        });
        measure("page", || store.fetch_page(&rows.row_ids, 0, 250).unwrap());
    }
    measure("pivot", || {
        pivot::execute(
            &store,
            &QueryConfig {
                group_by: vec!["region".into()],
                aggregations: vec![Aggregation {
                    sql: None,
                    source_column: "amount".into(),
                    operation: AggregationOp::Sum,
                    name: None,
                    conditions: vec![],
                    percentile: None,
                    interpolation: None,
                    ddof: None,
                }],
                ..Default::default()
            },
            &[],
        )
        .unwrap()
    });
    let result = ResultSnapshot::from_store(store, 1, 1, 1);
    measure("statistics", || {
        statistics::calculate(
            &result,
            Selection {
                anchor: Cell { row: 0, column: 2 },
                active: Cell {
                    row: result.row_count() - 1,
                    column: 2,
                },
            },
        )
        .unwrap()
    });
    drop(result);
    if let Some(database) = database {
        database.checkpoint().unwrap();
    }
}
