//! SQL sources and scripts that run on Polars SQL. Statements share one
//! context, so a table one creates is there for the next; files are read
//! with `read_csv('…')`, `read_parquet('…')` and the like.

use polars::prelude::Engine;
use polars::sql::SQLContext;

use super::expressions::polars_error;
use super::store::PolarsStore;
use crate::domain::sql_editor::{returns_rows, statements};

/// What running some statements produced.
pub struct PolarsRun {
    /// The rows of the last statement that returns any, each one of them.
    pub result: Option<PolarsStore>,
    pub completed: usize,
    pub error: Option<String>,
}

/// Runs each statement of `text` in `context`, stopping at the first error.
/// Only the last statement that returns rows is collected.
pub fn run(context: &mut SQLContext, text: &str) -> PolarsRun {
    let mut run = PolarsRun {
        result: None,
        completed: 0,
        error: None,
    };
    let mut last = None;
    for (index, range) in statements(text).into_iter().enumerate() {
        let sql = &text[range];
        match context.execute(sql) {
            Ok(frame) => {
                if returns_rows(sql) {
                    last = Some(frame);
                }
                run.completed += 1;
            }
            Err(error) => {
                run.error = Some(format!("Statement {}: {}", index + 1, polars_error(error)));
                break;
            }
        }
    }
    if let Some(frame) = last {
        match frame.collect_with_engine(Engine::Streaming).map(|result| result.unwrap_single()) {
            Ok(frame) => run.result = Some(PolarsStore::from_frame(frame)),
            Err(error) => run.error = Some(polars_error(error)),
        }
    }
    run
}

/// The rows a SQL source's script returns, in a context of its own.
pub fn source_rows(text: &str) -> Result<PolarsStore, String> {
    let run = run(&mut SQLContext::new(), text);
    if let Some(error) = run.error {
        return Err(error);
    }
    run.result.ok_or_else(|| "The SQL source's last statement returns no rows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
