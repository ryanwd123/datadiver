//! SQL sources and scripts that run on Polars SQL. Statements share one
//! context, so a table one creates is there for the next; files are read
//! with `read_csv('…')`, `read_parquet('…')` and the like.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use polars::prelude::{DataFrame, Engine, IdxSize, LazyFrame, PlanCallback};
use polars::sql::SQLContext;

use super::expressions::polars_error;
use super::store::PolarsStore;
use crate::domain::sql_editor::{returns_rows, statements};

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

/// The rows a SQL source's script returns, in a context of its own. Like a
/// DuckDB source, its last statement has to return them.
pub fn source_rows(text: &str) -> Result<PolarsStore, String> {
    let planned = plan(&mut SQLContext::new(), text);
    if planned.error.is_none() && !planned.last_returns_rows {
        return Err("The SQL source's last statement returns no rows".to_owned());
    }
    let run = planned.collect(u64::MAX, &Arc::new(AtomicBool::new(false)));
    if let Some(error) = run.error {
        return Err(error);
    }
    run.result.ok_or_else(|| "The SQL source's last statement returns no rows".to_owned())
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
        )
        .unwrap();
        assert_eq!(store.row_count(), 2);
        assert_eq!(store.schema()[1].name, "doubled");
        assert!(source_rows("CREATE TABLE empty AS SELECT 1 AS n").err().unwrap().contains("returns no rows"));
        assert!(source_rows("SELECT * FROM missing").err().unwrap().starts_with("Statement 1:"));
    }

    #[test]
    fn a_source_needs_its_last_statement_to_return_rows() {
        let error = source_rows("SELECT 1 AS n; CREATE TABLE later AS SELECT 2 AS n").err().unwrap();
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
    fn a_stopped_run_reads_nothing() {
        let stop = Arc::new(AtomicBool::new(true));
        let run = plan(&mut SQLContext::new(), "SELECT 1 AS n").collect(u64::MAX, &stop);
        assert_eq!(run.error.as_deref(), Some("Statement 1: Stopped"));
        assert!(run.result.is_none());
    }
}
