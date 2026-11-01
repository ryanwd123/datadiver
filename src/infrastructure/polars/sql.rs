//! SQL sources and scripts that run on Polars SQL. Statements share one
//! context, so a table one creates is there for the next; files are read
//! with `read_csv('…')`, `read_parquet('…')` and the like.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use polars::prelude::{DataFrame, Engine, IdxSize, LazyFrame, PlanCallback};
use polars::sql::SQLContext;

use diver_sql::Dialect;

use super::expressions::{polars_error, sql_type_name};
use super::store::PolarsStore;
use crate::domain::sql_editor::{Relation, SqlCatalog, returns_rows, statements};
use crate::infrastructure::duckdb_store::quote_identifier;

/// What running some statements produced.
pub struct PolarsRun {
    /// The rows of the last statement that returns any, at most the limit.
    pub result: Option<PolarsStore>,
    pub completed: usize,
    /// The statement returned more rows than the limit.
    pub truncated: bool,
    pub error: Option<String>,
}

/// Statements a context has planned, before any rows are read. Polars
/// plans lazily, so most runtime errors only show up when collecting.
pub struct Planned {
    /// The last statement that returns rows, numbered from one.
    rows: Option<(usize, LazyFrame)>,
    /// Whether the script's very last statement returns rows.
    last_returns_rows: bool,
    completed: usize,
    error: Option<String>,
}

/// Plans each statement of `text` in `context`, stopping at the first error.
/// Only planning needs the context, so collecting can run without its lock.
pub fn plan(context: &mut SQLContext, text: &str) -> Planned {
    let mut planned = Planned {
        rows: None,
        last_returns_rows: false,
        completed: 0,
        error: None,
    };
    for (index, range) in statements(text).into_iter().enumerate() {
        let sql = &text[range];
        planned.last_returns_rows = returns_rows(sql);
        match context.execute(sql) {
            Ok(frame) => {
                if planned.last_returns_rows {
                    planned.rows = Some((index + 1, frame));
                }
                planned.completed += 1;
            }
            Err(error) => {
                planned.error = Some(format!("Statement {}: {}", index + 1, polars_error(error)));
                break;
            }
        }
    }
    planned
}

impl Planned {
    /// Reads at most `limit` rows of the last statement that returns any.
    /// Nothing is read after an error, and setting `stop` ends the read at
    /// its next batch.
    pub fn collect(self, limit: u64, stop: &Arc<AtomicBool>) -> PolarsRun {
        let mut run = PolarsRun {
            result: None,
            completed: self.completed,
            truncated: false,
            error: self.error,
        };
        if run.error.is_some() {
            return run;
        }
        if let Some((number, frame)) = self.rows {
            match collect(frame, limit, stop) {
                Ok((frame, truncated)) => {
                    run.result = Some(PolarsStore::from_frame(frame));
                    run.truncated = truncated;
                }
                Err(error) => run.error = Some(format!("Statement {number}: {error}")),
            }
        }
        run
    }
}

/// Collects `frame` on the streaming engine through a batch sink, which can
/// stop early: Polars has no other way to cancel a streaming query.
fn collect(frame: LazyFrame, limit: u64, stop: &Arc<AtomicBool>) -> Result<(DataFrame, bool), String> {
    // One row past the limit tells whether there were more.
    let mut frame = match IdxSize::try_from(limit.saturating_add(1)) {
        Ok(rows) => frame.limit(rows),
        Err(_) => frame,
    };
    let schema = frame.collect_schema().map_err(polars_error)?;
    let batches = Arc::new(Mutex::new(DataFrame::empty_with_schema(&schema)));
    let sink = {
        let batches = batches.clone();
        let stop = stop.clone();
        PlanCallback::new(move |batch: DataFrame| {
            let mut rows = batches.lock().unwrap();
            rows.vstack_mut_owned(batch)?;
            Ok(stop.load(Ordering::Acquire) || rows.height() as u64 > limit)
        })
    };
    frame
        .sink_batches(sink, true, None)
        .and_then(|frame| frame.collect_with_engine(Engine::Streaming))
        .map_err(polars_error)?;
    if stop.load(Ordering::Acquire) {
        return Err("Stopped".into());
    }
    let mut rows = std::mem::take(&mut *batches.lock().unwrap());
    let truncated = rows.height() as u64 > limit;
    if truncated {
        rows = rows.slice(0, limit as usize);
    }
    rows.rechunk_mut();
    Ok((rows, truncated))
}

/// What the SQL editor completes from on Polars SQL: the tables earlier runs
/// made in `context`, with their columns, and Polars SQL's functions.
pub fn catalog(context: &mut SQLContext) -> SqlCatalog {
    const DATABASE: &str = "polars";
    let mut tables = context.get_tables();
    tables.sort();
    let relations = tables
        .into_iter()
        .map(|name| {
            let schema: Vec<(String, String)> = context
                .execute(&format!("SELECT * FROM {}", quote_identifier(&name)))
                .and_then(|mut frame| frame.collect_schema())
                .map(|schema| schema.iter().map(|(column, dtype)| (column.to_string(), sql_type_name(dtype))).collect())
                .unwrap_or_default();
            let (columns, column_types) = schema.into_iter().unzip();
            Relation {
                catalog: DATABASE.into(),
                schema: "main".into(),
                name,
                columns,
                column_types,
                is_view: false,
                current_database: true,
            }
        })
        .collect();
    SqlCatalog {
        relations,
        current_database: DATABASE.into(),
        functions: super::functions::sql_functions(),
        dialect: Dialect::Polars,
        ..SqlCatalog::default()
    }
}

/// The rows a SQL source's script returns, in a context of its own. Like a
/// DuckDB source, its last statement has to return them, and it keeps at
/// most `limit` of them.
pub fn source_rows(text: &str, limit: u64) -> Result<PolarsStore, String> {
    let planned = plan(&mut SQLContext::new(), text);
    if planned.error.is_none() && !planned.last_returns_rows {
        return Err("The SQL source's last statement returns no rows".to_owned());
    }
    let run = planned.collect(limit, &Arc::new(AtomicBool::new(false)));
    if let Some(error) = run.error {
        return Err(error);
    }
    let cut_at = run.truncated.then(|| usize::try_from(limit).unwrap_or(usize::MAX));
    run.result
        .map(|store| store.with_cut_at(cut_at))
        .ok_or_else(|| "The SQL source's last statement returns no rows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(context: &mut SQLContext, text: &str, limit: u64) -> PolarsRun {
        plan(context, text).collect(limit, &Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn later_statements_see_tables_earlier_ones_create() {
        let store = source_rows(
            "CREATE TABLE fruit AS SELECT * FROM (VALUES ('red', 12), ('gold', 15)) AS t(color, quantity);\n\
             SELECT color, quantity * 2 AS doubled FROM fruit ORDER BY quantity",
            u64::MAX,
        )
        .unwrap();
        assert_eq!((store.row_count(), store.cut_at()), (2, None));
        assert_eq!(store.schema()[1].name, "doubled");
        assert!(source_rows("CREATE TABLE empty AS SELECT 1 AS n", u64::MAX).err().unwrap().contains("returns no rows"));
        assert!(source_rows("SELECT * FROM missing", u64::MAX).err().unwrap().starts_with("Statement 1:"));
        let cut = source_rows("SELECT * FROM (VALUES (3), (2), (1)) AS t(n) ORDER BY n", 2).unwrap();
        assert_eq!((cut.row_count(), cut.cut_at()), (2, Some(2)));
        let whole = source_rows("SELECT * FROM (VALUES (3), (2)) AS t(n)", 2).unwrap();
        assert_eq!((whole.row_count(), whole.cut_at()), (2, None));
    }

    #[test]
    fn a_source_needs_its_last_statement_to_return_rows() {
        let error = source_rows("SELECT 1 AS n; CREATE TABLE later AS SELECT 2 AS n", u64::MAX).err().unwrap();
        assert!(error.contains("last statement returns no rows"), "{error}");
        // A script in the editor still shows the earlier rows.
        let run = run(&mut SQLContext::new(), "SELECT 1 AS n; CREATE TABLE later AS SELECT 2 AS n", u64::MAX);
        assert!(run.error.is_none(), "{:?}", run.error);
        assert_eq!(run.result.unwrap().row_count(), 1);
    }

    #[test]
    fn runtime_errors_name_the_statement_and_keep_an_earlier_error() {
        // The cast fails only when the rows are read.
        let text = "SELECT 1 AS n;\nSELECT CAST('x' AS INTEGER) AS n";
        let run = run(&mut SQLContext::new(), text, u64::MAX);
        let error = run.error.unwrap();
        assert!(error.starts_with("Statement 2:"), "{error}");
        assert!(run.result.is_none());
        // A planning error stops the run before anything is read.
        let run = super::tests::run(&mut SQLContext::new(), "SELECT CAST('x' AS INTEGER) AS n;\nSELECT * FROM missing", u64::MAX);
        let error = run.error.unwrap();
        assert!(error.starts_with("Statement 2:"), "{error}");
        assert_eq!(run.completed, 1);
    }

    #[test]
    fn a_run_keeps_at_most_the_limit() {
        let text = "SELECT * FROM (VALUES (1), (2), (3), (4)) AS t(n)";
        let run = run(&mut SQLContext::new(), text, 3);
        assert!(run.truncated);
        assert_eq!(run.result.unwrap().row_count(), 3);
        let run = super::tests::run(&mut SQLContext::new(), text, 4);
        assert!(!run.truncated);
        assert_eq!(run.result.unwrap().row_count(), 4);
        let run = super::tests::run(&mut SQLContext::new(), "SELECT * FROM (VALUES (1)) AS t(n) WHERE n > 5", 4);
        assert_eq!(run.result.unwrap().row_count(), 0);
    }

    #[test]
    fn the_catalog_has_the_tables_runs_made_and_polars_functions() {
        let mut context = SQLContext::new();
        assert!(catalog(&mut context).relations.is_empty());
        let made = run(&mut context, "CREATE TABLE fruit AS SELECT * FROM (VALUES ('red', 12)) AS t(color, quantity)", 10);
        assert!(made.error.is_none(), "{:?}", made.error);
        let catalog = catalog(&mut context);
        assert_eq!(catalog.dialect, Dialect::Polars);
        let fruit = catalog.resolve(&["fruit"]).unwrap();
        assert_eq!(fruit.columns, ["color", "quantity"]);
        assert_eq!(fruit.column_types.len(), 2);
        // Polars completes its own names, not DuckDB's.
        let labels = |text: &str| {
            crate::domain::sql_editor::completions(text, text.len(), &catalog)
                .into_iter()
                .map(|item| item.label)
                .collect::<Vec<_>>()
        };
        let from = labels("SELECT * FROM ");
        assert!(from.iter().any(|l| l == "fruit") && from.iter().any(|l| l == "read_parquet"), "{from:?}");
        assert!(!from.iter().any(|l| l == "read_csv_auto"), "{from:?}");
        let cast = labels("SELECT CAST(quantity AS ");
        assert!(cast.iter().any(|l| l == "DATETIME") && !cast.iter().any(|l| l == "STRUCT"), "{cast:?}");
        let start = labels("");
        assert!(!start.iter().any(|l| l == "PIVOT" || l == "ATTACH"), "{start:?}");
    }

    #[test]
    fn a_stopped_run_reads_nothing() {
        let stop = Arc::new(AtomicBool::new(true));
        let run = plan(&mut SQLContext::new(), "SELECT 1 AS n").collect(u64::MAX, &stop);
        assert_eq!(run.error.as_deref(), Some("Statement 1: Stopped"));
        assert!(run.result.is_none());
    }
}
