use duckdb::types::Value;
use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SqlStatus {
    Succeeded,
    Cached,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct SqlStatement {
    pub stage: String,
    pub sql: String,
    pub parameters: Vec<Value>,
    pub status: SqlStatus,
}

#[derive(Clone, Debug, Default)]
pub struct SqlTrace {
    pub statements: Vec<SqlStatement>,
}

impl SqlTrace {
    pub fn execute<T>(
        &mut self,
        stage: &str,
        sql: &str,
        parameters: &[Value],
        execute: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let result = execute();
        self.statements.push(SqlStatement {
            stage: stage.into(),
            sql: sql.into(),
            parameters: parameters.to_vec(),
            status: match &result {
                Ok(_) => SqlStatus::Succeeded,
                Err(error) => SqlStatus::Failed(error.clone()),
            },
        });
        result
    }

    pub fn cached(&self) -> Self {
        let mut trace = self.clone();
        for statement in &mut trace.statements {
            statement.status = SqlStatus::Cached;
        }
        trace
    }

    pub fn append(&mut self, other: &Self) {
        self.statements.extend(other.statements.clone());
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        for statement in &self.statements {
            let status = match &statement.status {
                SqlStatus::Succeeded => "Succeeded",
                SqlStatus::Cached => "Cached — reused from an earlier computation",
                SqlStatus::Failed(_) => "Failed",
            };
            let _ = writeln!(text, "-- {} · {status}", statement.stage);
            let _ = writeln!(text, "{};", statement.sql.trim().trim_end_matches(';'));
            for (index, parameter) in statement.parameters.iter().enumerate() {
                // Debug preserves the DuckDB binding type, integer precision,
                // and escaped newlines so a value cannot become SQL in a copy.
                let _ = writeln!(text, "-- Parameter {}: {parameter:?}", index + 1);
            }
            if let SqlStatus::Failed(error) = &statement.status {
                for line in error.lines() {
                    let _ = writeln!(text, "-- Error: {line}");
                }
            }
            text.push('\n');
        }
        text
    }
}

#[derive(Clone, Debug)]
pub struct SqlFailure {
    pub trace: SqlTrace,
    pub error: String,
}

#[derive(Clone, Debug, Default)]
pub struct SqlDiagnostics {
    pub displayed: SqlTrace,
    pub failed: Option<SqlFailure>,
}

impl SqlDiagnostics {
    pub fn finish(&mut self, trace: SqlTrace, error: Option<String>) {
        match error {
            Some(error) => self.failed = Some(SqlFailure { trace, error }),
            None => {
                self.displayed = trace;
                self.failed = None;
            }
        }
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        if let Some(failed) = &self.failed {
            text.push_str("-- Latest failed attempt · grid retains the last successful result\n");
            for line in failed.error.lines() {
                let _ = writeln!(text, "-- {line}");
            }
            text.push_str(&failed.trace.text());
            text.push('\n');
        }
        if !self.displayed.statements.is_empty() {
            text.push_str("-- SQL producing the displayed result\n");
            text.push_str(&self.displayed.text());
        } else {
            text.push_str("-- No SQL has been published for this session yet.\n");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_keep_success_on_failure_and_copy_typed_ordered_values() {
        let mut trace = SqlTrace::default();
        trace
            .execute(
                "Input",
                "SELECT ?, ?",
                &[
                    Value::HugeInt(i128::MAX),
                    Value::Text("winter\nSELECT 'x'".into()),
                ],
                || Ok(()),
            )
            .unwrap();
        let mut diagnostics = SqlDiagnostics::default();
        diagnostics.finish(trace.clone(), None);
        let mut failed = SqlTrace::default();
        let _ = failed.execute::<()>("Pivot", "PIVOT missing ON sex", &[], || {
            Err("missing table\nsecond line".into())
        });
        diagnostics.finish(failed, Some("missing table".into()));
        let text = diagnostics.text();
        assert!(text.contains("Latest failed attempt"));
        assert!(text.contains("SQL producing the displayed result"));
        assert!(text.contains(&i128::MAX.to_string()));
        assert!(text.contains("-- Parameter 2: Text(\"winter\\nSELECT 'x'\")"));
        assert!(text.contains("-- Error: second line"));
        diagnostics.finish(trace.cached(), None);
        assert!(diagnostics.failed.is_none());
        assert!(diagnostics.text().contains("Cached"));
    }
}
