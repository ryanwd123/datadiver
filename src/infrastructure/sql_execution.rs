use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use duckdb::{
    Connection, InterruptHandle,
    arrow::datatypes::{DataType, TimeUnit},
};

use super::{
    duckdb_store::{DuckDbStore, is_nested_type, quote_identifier},
    path_completion::PathRoots,
    source_database::SourceDatabase,
};
use crate::domain::ai::{
    ALL_VALUES_LIMIT, ColumnProfile, RelationProfile, SchemaColumn, SchemaRelation, TOP_VALUES,
    relation_name,
};
use crate::domain::sql_editor::{
    Relation, SqlCatalog, is_filterable_query, may_mutate, returns_rows, statements,
};

/// An in-memory database attached to the profile for complete SQL results.
/// Attached databases are shared by every connection, so result stores read
/// it through their own connection.
const RESULTS: &str = "__dd_results";
/// Names result tables uniquely across every editor in the process.
static RESULT_NUMBER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct SqlExecution {
    connection: Connection,
    database: Arc<SourceDatabase>,
    pub interrupt: Arc<InterruptHandle>,
    pub stopped: Arc<AtomicBool>,
}

pub struct SqlRun {
    pub result: Option<DuckDbStore>,
    pub completed: usize,
    pub affected_rows: usize,
    pub truncated: bool,
    pub error: Option<String>,
    pub status: String,
    pub catalog: SqlCatalog,
    pub path_roots: PathRoots,
    /// The result holds every row of a query, so slicers and value filters
    /// can narrow it; otherwise it holds at most the row limit.
    pub complete: bool,
}

impl SqlExecution {
    pub fn new(database: Arc<SourceDatabase>) -> Result<Self, String> {
        let connection = database.connect()?;
        let interrupt = connection.interrupt_handle();
        connection
            .execute_batch(&format!("ATTACH IF NOT EXISTS ':memory:' AS {RESULTS}"))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            connection,
            database,
            interrupt,
            stopped: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn catalog(&self) -> Result<Vec<Relation>, String> {
        let sql = "SELECT database_name, schema_name, table_name, false, database_name = current_database() FROM duckdb_tables() WHERE NOT internal UNION ALL SELECT database_name, schema_name, view_name, true, database_name = current_database() FROM duckdb_views() WHERE NOT internal ORDER BY 1, 2, 3";
        let mut stmt = self.connection.prepare(sql).map_err(|e| e.to_string())?;
        let mut relations = stmt
            .query_map([], |row| {
                Ok(Relation {
                    catalog: row.get(0)?,
                    schema: row.get(1)?,
                    name: row.get(2)?,
                    is_view: row.get(3)?,
                    current_database: row.get(4)?,
                    columns: Vec::new(),
                    column_types: Vec::new(),
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        relations.retain(|r| !r.name.starts_with("__qp_") && !r.name.starts_with("__dd_"));
        let mut columns = self.connection.prepare("SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE NOT internal ORDER BY column_index").map_err(|e| e.to_string())?;
        let rows = columns
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (catalog, schema, table, column, data_type) = row.map_err(|e| e.to_string())?;
            if let Some(r) = relations
                .iter_mut()
                .find(|r| r.catalog == catalog && r.schema == schema && r.name == table)
            {
                r.columns.push(column);
                r.column_types.push(data_type);
            }
        }
        Ok(relations)
    }

    /// Everything the SQL editor completes and checks against: relations,
    /// databases and schemas, functions with their signatures, settings and
    /// extensions.
    pub fn sql_catalog(&self) -> Result<SqlCatalog, String> {
        let strings = |sql: &str| -> Result<Vec<String>, String> {
            let mut stmt = self.connection.prepare(sql).map_err(|e| e.to_string())?;
            stmt.query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())
        };
        let mut schemas = self
            .connection
            .prepare("SELECT database_name, schema_name FROM duckdb_schemas() WHERE schema_name NOT IN ('information_schema', 'pg_catalog') AND database_name NOT IN ('system', 'temp') ORDER BY 1, 2")
            .map_err(|e| e.to_string())?;
        let schemas = schemas
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut settings = self
            .connection
            .prepare("SELECT name, description FROM duckdb_settings() ORDER BY name")
            .map_err(|e| e.to_string())?;
        let settings = settings
            .query_map([], |row| {
                Ok(diver_sql::SettingInfo {
                    name: row.get(0)?,
                    description: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut variables = self
            .connection
            .prepare("SELECT name, type FROM duckdb_variables() ORDER BY name")
            .map_err(|e| e.to_string())?;
        let variables = variables
            .query_map([], |row| {
                Ok(diver_sql::VariableInfo {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut sequences = self
            .connection
            .prepare("SELECT database_name, schema_name, sequence_name FROM duckdb_sequences() ORDER BY 1, 2, 3")
            .map_err(|e| e.to_string())?;
        let sequences = sequences
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let hidden = |name: &String| name.starts_with("__dd_") || name.starts_with("__qp_");
        Ok(SqlCatalog {
            relations: self.catalog()?,
            current_database: self.default_catalog()?,
            databases: strings("SELECT database_name FROM duckdb_databases() WHERE NOT internal ORDER BY 1")?
                .into_iter()
                .filter(|name| !hidden(name))
                .collect(),
            schemas: schemas.into_iter().filter(|(d, _)| !hidden(d)).collect(),
            functions: self.functions()?,
            settings,
            variables,
            sequences: sequences.into_iter().filter(|(d, _, _)| !hidden(d)).collect(),
            extensions: strings("SELECT extension_name FROM duckdb_extensions() ORDER BY 1")?,
            files: Default::default(),
            table_functions: self.metadata_function_columns().unwrap_or_default(),
        })
    }

    /// The columns of the metadata table functions that take no arguments
    /// (`duckdb_columns()`, `pragma_version()`), as DESCRIBE binds them
    /// without running them.
    fn metadata_function_columns(&self) -> Result<HashMap<String, Vec<diver_sql::ColumnInfo>>, String> {
        let error = |e: duckdb::Error| e.to_string();
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT function_name FROM duckdb_functions() WHERE function_type = 'table' AND len(parameters) = 0 AND regexp_matches(function_name, '^(duckdb|pragma)_[a-z0-9_]+$')")
            .map_err(error)?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error)?;
        let mut out = HashMap::new();
        for name in names {
            let Ok(mut describe) = self.connection.prepare(&format!("DESCRIBE SELECT * FROM {name}()")) else {
                continue;
            };
            let columns = describe
                .query_map([], |row| {
                    Ok(diver_sql::ColumnInfo {
                        name: row.get(0)?,
                        data_type: row.get(1)?,
                    })
                })
                .and_then(|rows| rows.collect::<Result<Vec<_>, _>>());
            if let Ok(columns) = columns {
                out.insert(name, columns);
            }
        }
        Ok(out)
    }

    /// Functions and macros by name, with up to a dozen signatures each like
    /// `round(x DOUBLE, precision INTEGER) -> DOUBLE`. Operators are left out.
    fn functions(&self) -> Result<Vec<diver_sql::FunctionInfo>, String> {
        use diver_sql::FunctionKind;
        const MAX_SIGNATURES: usize = 12;
        let sql = "SELECT function_name, schema_name, function_type, array_to_string(parameters, chr(31)), array_to_string(parameter_types, chr(31)), varargs, return_type, description FROM duckdb_functions() WHERE function_type <> 'pragma' AND regexp_matches(function_name, '^[A-Za-z_][A-Za-z0-9_]*$') ORDER BY function_name, schema_name";
        let mut stmt = self.connection.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out: Vec<diver_sql::FunctionInfo> = Vec::new();
        for row in rows {
            let (name, schema, kind, names, types, varargs, returns, description) =
                row.map_err(|e| e.to_string())?;
            let split = |list: Option<String>| -> Vec<String> {
                list.filter(|l| !l.is_empty())
                    .map(|l| l.split('\u{1f}').map(str::to_string).collect())
                    .unwrap_or_default()
            };
            let (names, types) = (split(names), split(types));
            let arguments = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    // Built-ins name many arguments col0, col1, ...: the type says more.
                    let generic = name.strip_prefix("col").is_some_and(|n| n.parse::<u32>().is_ok());
                    match types.get(i).filter(|t| !t.is_empty() && t.as_str() != "ANY") {
                        Some(t) if generic => t.clone(),
                        Some(t) => format!("{name} {t}"),
                        None => name.clone(),
                    }
                })
                .chain(varargs.filter(|v| !v.is_empty()).map(|v| format!("{v}...")))
                .collect::<Vec<_>>();
            let returns = returns
                .filter(|r| !r.is_empty() && kind != "table" && kind != "table_macro")
                .map(|r| format!(" -> {r}"))
                .unwrap_or_default();
            let signature = format!("{name}({}){returns}", arguments.join(", "));
            let same = out
                .last()
                .is_some_and(|f| f.name == name && f.schema == schema);
            if !same {
                out.push(diver_sql::FunctionInfo {
                    name,
                    schema,
                    kind: match kind.as_str() {
                        "aggregate" => FunctionKind::Aggregate,
                        "window" => FunctionKind::Window,
                        "table" | "table_macro" => FunctionKind::Table,
                        "macro" => FunctionKind::Macro,
                        _ => FunctionKind::Scalar,
                    },
                    signatures: Vec::new(),
                    description: None,
                });
            }
            let function = out.last_mut().expect("pushed above");
            if function.signatures.len() < MAX_SIGNATURES && !function.signatures.contains(&signature) {
                function.signatures.push(signature);
            }
            if function.description.is_none() {
                function.description = description.filter(|d| !d.is_empty());
            }
        }
        Ok(out)
    }

    /// The database unqualified names resolve to.
    pub fn default_catalog(&self) -> Result<String, String> {
        self.connection
            .query_row("SELECT current_database()", [], |row| row.get(0))
            .map_err(|e| e.to_string())
    }

    /// The relations the AI may query, with column types and comments, as SQL
    /// names: the profile's own relations first.
    pub fn ai_schema(&self) -> Result<Vec<SchemaRelation>, String> {
        type Key = (String, String, String);
        let error = |e: duckdb::Error| e.to_string();
        let default_catalog = self.default_catalog()?;
        let catalog = self.catalog()?;
        let mut comments = std::collections::HashMap::<Key, String>::new();
        let mut statement = self
            .connection
            .prepare("SELECT database_name, schema_name, table_name, comment FROM duckdb_tables() WHERE comment IS NOT NULL UNION ALL SELECT database_name, schema_name, view_name, comment FROM duckdb_views() WHERE comment IS NOT NULL")
            .map_err(error)?;
        let rows = statement
            .query_map([], |row| Ok(((row.get(0)?, row.get(1)?, row.get(2)?), row.get(3)?)))
            .map_err(error)?;
        for row in rows {
            let (key, comment) = row.map_err(error)?;
            comments.insert(key, comment);
        }
        let mut statement = self
            .connection
            .prepare("SELECT database_name, schema_name, table_name, column_name, data_type, comment FROM duckdb_columns() WHERE NOT internal ORDER BY column_index")
            .map_err(error)?;
        let mut columns = std::collections::HashMap::<Key, Vec<SchemaColumn>>::new();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    (row.get(0)?, row.get(1)?, row.get(2)?),
                    SchemaColumn {
                        name: row.get(3)?,
                        kind: row.get(4)?,
                        comment: row.get(5)?,
                    },
                ))
            })
            .map_err(error)?;
        for row in rows {
            let (key, column) = row.map_err(error)?;
            columns.entry(key).or_default().push(column);
        }
        let mut relations = catalog
            .into_iter()
            .map(|r| {
                let key = (r.catalog.clone(), r.schema.clone(), r.name.clone());
                (
                    r.catalog != default_catalog,
                    SchemaRelation {
                        name: relation_name(&r.catalog, &r.schema, &r.name, &default_catalog),
                        is_view: r.is_view,
                        comment: comments.remove(&key),
                        columns: columns.remove(&key).unwrap_or_default(),
                        profile: None,
                    },
                )
            })
            .collect::<Vec<_>>();
        relations.sort_by_key(|(other, _)| *other);
        Ok(relations.into_iter().map(|(_, r)| r).collect())
    }

    /// Counts rows, nulls, and distinct values per column in one scan. With
    /// `send_data`, also min and max, every value of columns with few of them,
    /// and the most frequent values of other text columns.
    pub fn profile_relation(
        &self,
        relation: &SchemaRelation,
        send_data: bool,
    ) -> Result<RelationProfile, String> {
        let error = |e: duckdb::Error| e.to_string();
        let mut projection = vec!["count(*)".to_owned()];
        for column in &relation.columns {
            let id = quote_identifier(&column.name);
            projection.push(format!("count({id})"));
            projection.push(format!("approx_count_distinct({id})"));
            if send_data && !is_nested_type(&column.kind) {
                projection.push(format!("CAST(min({id}) AS VARCHAR)"));
                projection.push(format!("CAST(max({id}) AS VARCHAR)"));
            } else {
                projection.push("NULL".into());
                projection.push("NULL".into());
            }
        }
        let sql = format!("SELECT {} FROM {}", projection.join(", "), relation.name);
        let (rows, mut columns) = self
            .connection
            .query_row(&sql, [], |row| {
                let rows = row.get::<_, i64>(0)? as u64;
                let columns = (0..relation.columns.len())
                    .map(|index| {
                        let at = 1 + index * 4;
                        Ok(ColumnProfile {
                            nulls: rows - row.get::<_, i64>(at)? as u64,
                            unique: row.get::<_, i64>(at + 1)? as u64,
                            min: row.get(at + 2)?,
                            max: row.get(at + 3)?,
                            values: Vec::new(),
                            all_values: false,
                        })
                    })
                    .collect::<Result<Vec<_>, duckdb::Error>>()?;
                Ok((rows, columns))
            })
            .map_err(error)?;
        if send_data {
            for (column, profile) in relation.columns.iter().zip(&mut columns) {
                if is_nested_type(&column.kind) {
                    continue;
                }
                // The estimate can be a little off, so a column near the limit
                // is counted exactly by listing one more value than it allows.
                let few = profile.unique <= ALL_VALUES_LIMIT + ALL_VALUES_LIMIT / 5;
                let text = ["VARCHAR", "CHAR", "ENUM"]
                    .iter()
                    .any(|kind| column.kind.to_ascii_uppercase().starts_with(kind));
                if !few && !text {
                    continue;
                }
                let limit = if few {
                    ALL_VALUES_LIMIT as usize + 1
                } else {
                    TOP_VALUES
                };
                let id = quote_identifier(&column.name);
                let sql = format!(
                    "SELECT CAST({id} AS VARCHAR), count(*) AS n FROM {} WHERE {id} IS NOT NULL GROUP BY {id} ORDER BY n DESC, {id} LIMIT {limit}",
                    relation.name
                );
                let mut statement = self.connection.prepare(&sql).map_err(error)?;
                let mut values = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as u64)))
                    .map_err(error)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(error)?;
                if few && values.len() <= ALL_VALUES_LIMIT as usize {
                    profile.unique = values.len() as u64;
                    profile.all_values = true;
                } else {
                    values.truncate(TOP_VALUES);
                }
                profile.values = values;
            }
        }
        Ok(RelationProfile { rows, columns })
    }

    /// Prepares `sql` without running it, so binder errors such as a missing
    /// column surface.
    pub fn prepare_only(&self, sql: &str) -> Result<(), String> {
        self.connection
            .prepare(sql)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Read per connection: a `SET` run in this editor applies only here.
    pub fn path_roots(&self) -> PathRoots {
        let setting = |name: &str| {
            self.connection
                .query_row(
                    &format!("SELECT current_setting('{name}')::VARCHAR"),
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        PathRoots::new(&setting("file_search_path"), &setting("home_directory"))
    }

    /// Runs a SQL source's script on a connection of its own and keeps every
    /// row of its last statement, for the views built on the source. The
    /// rows are copied out, so the store outlives the connection and its
    /// temporary objects.
    pub fn source_rows(database: Arc<SourceDatabase>, text: &str) -> Result<DuckDbStore, String> {
        let execution = Self::new(database)?;
        let ranges = statements(text);
        let (last, setup) = ranges.split_last().ok_or("The SQL source has no statement")?;
        let mutated = ranges.iter().any(|range| may_mutate(&text[range.clone()]));
        let result = (|| {
            for (index, range) in setup.iter().enumerate() {
                execution
                    .execute(&text[range.clone()], 0)
                    .map_err(|error| format!("Statement {}: {error}", index + 1))?;
            }
            match execution.execute(&text[last.clone()], u64::MAX) {
                Ok((Some((store, _)), _)) => Ok(store),
                Ok((None, _)) => Err("The SQL source's last statement returns no rows".to_owned()),
                Err(error) => Err(format!("Statement {}: {error}", ranges.len())),
            }
        })();
        if mutated {
            execution.database.invalidate();
        }
        result
    }

    pub fn run(&self, text: &str, limit: u64) -> SqlRun {
        let started = Instant::now();
        let mut mutated = false;
        let mut run = SqlRun {
            result: None,
            completed: 0,
            affected_rows: 0,
            truncated: false,
            error: None,
            status: String::new(),
            catalog: SqlCatalog::default(),
            path_roots: PathRoots::default(),
            complete: false,
        };
        let ranges = statements(text);
        let last = ranges.len().saturating_sub(1);
        for (index, range) in ranges.into_iter().enumerate() {
            if self.stopped.load(Ordering::Acquire) {
                run.error = Some("Stopped".into());
                break;
            }
            let sql = &text[range];
            mutated |= may_mutate(sql);
            // Earlier statements are setup; only the last query keeps every row.
            let mut complete = false;
            let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if index == last && is_filterable_query(sql) {
                    match self.materialize(sql) {
                        Ok(Some(store)) => {
                            complete = true;
                            return Ok((Some((store, false)), 0));
                        }
                        Ok(None) => {}
                        // A query DuckDB cannot keep this way still shows its first rows.
                        Err(error) if materialize_only_failure(&error) => {}
                        // Anything else fails the plain run too, after running
                        // the query a second time.
                        Err(error) => return Err(error),
                    }
                }
                self.execute(sql, limit)
            }));
            let result = execution.unwrap_or_else(|panic| {
                Err(panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "SQL result conversion failed".into()))
            });
            match result {
                Ok((Some((store, truncated)), affected)) => {
                    run.affected_rows += affected;
                    run.result = Some(store);
                    run.truncated = truncated;
                    run.complete = complete;
                    run.completed += 1;
                }
                Ok((None, affected)) => {
                    run.completed += 1;
                    run.affected_rows += affected;
                }
                Err(error) => {
                    run.error = Some(format!("Statement {}: {error}", index + 1));
                    break;
                }
            }
        }
        if mutated {
            self.database.invalidate();
        }
        run.catalog = self.sql_catalog().unwrap_or_default();
        run.path_roots = self.path_roots();
        run.status = format!(
            "{} statement{} completed in {:.2}s{}",
            run.completed,
            if run.completed == 1 { "" } else { "s" },
            started.elapsed().as_secs_f64(),
            if run.truncated {
                " · result truncated at row limit"
            } else {
                ""
            }
        );
        if run.affected_rows > 0 {
            run.status
                .push_str(&format!(" · {} affected rows", run.affected_rows));
        }
        if run.completed == 0 && run.error.is_none() {
            run.status = "No SQL statement to run".into();
        }
        run
    }

    /// Keeps a query's whole result as a table in the shared results database,
    /// so filters see every row and DuckDB's types survive exactly. Returns
    /// `None` inside an explicit transaction: a failed write there would abort
    /// the user's transaction, and a transaction may write to one database only.
    fn materialize(&self, sql: &str) -> Result<Option<DuckDbStore>, String> {
        if !self.connection.is_autocommit() {
            return Ok(None);
        }
        let table = format!(
            "{RESULTS}.main.__dd_result_{}",
            RESULT_NUMBER.fetch_add(1, Ordering::Relaxed)
        );
        // The query goes on its own lines so a trailing comment cannot hide the `)`.
        let body = sql.trim_end().trim_end_matches(';');
        self.database
            .keeping_order(|| self.connection.execute_batch(&format!("CREATE TABLE {table} AS FROM (\n{body}\n)")))
            .map_err(|error| error.to_string())?;
        // The table lasts as long as a view shows it, even after this editor
        // runs again or closes.
        DuckDbStore::owning_table(self.database.clone(), self.database.connect()?, table).map(Some)
    }

    /// Runs one statement, keeping at most `limit` of its rows, while the
    /// database preserves insertion order, so its rows and writes keep theirs.
    fn execute(
        &self,
        sql: &str,
        limit: u64,
    ) -> Result<(Option<(DuckDbStore, bool)>, usize), String> {
        self.database.keeping_order(|| self.execute_in_order(sql, limit))
    }

    fn execute_in_order(
        &self,
        sql: &str,
        limit: u64,
    ) -> Result<(Option<(DuckDbStore, bool)>, usize), String> {
        let mut statement = self.connection.prepare(sql).map_err(|e| e.to_string())?;
        if !returns_rows(sql) {
            let affected = statement.execute([]).map_err(|e| e.to_string())?;
            return Ok((None, affected));
        }
        let mut batches = statement.stream_arrow([]).map_err(|e| e.to_string())?;
        let schema = batches.get_schema();
        if schema.fields().is_empty() {
            for _ in batches {}
            return Ok((None, 0));
        }
        let mut retained = Vec::new();
        let mut count = 0;
        let mut truncated = false;
        for batch in &mut batches {
            let available = limit.saturating_sub(count) as usize;
            let keep = available.min(batch.num_rows());
            if keep < batch.num_rows() {
                truncated = true;
            }
            if keep > 0 {
                retained.push(batch.slice(0, keep));
                count += keep as u64;
            }
            // Drain the statement even after the display cap: RETURNING must complete all writes.
            if self.stopped.load(Ordering::Acquire) {
                return Err("Stopped".into());
            }
        }
        drop(batches);
        let output = Connection::open_in_memory().map_err(|e| e.to_string())?;
        let mut names = std::collections::HashSet::new();
        let mut result_names = Vec::new();
        let definitions = schema
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let mut name = field.name().clone();
                let mut suffix = 1;
                while !names.insert(name.to_lowercase()) {
                    name = format!("{}_{suffix}", field.name());
                    suffix += 1;
                }
                result_names.push(name.clone());
                Ok(format!(
                    "{} {}",
                    quote_identifier(&name),
                    result_sql_type(&statement.column_logical_type(index), field.data_type())?
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        output
            .execute_batch(&format!(
                "CREATE TABLE qp_data ({})",
                definitions.join(", ")
            ))
            .map_err(|e| e.to_string())?;
        let huge_columns = (0..schema.fields().len())
            .filter(|index| {
                matches!(
                    statement.column_logical_type(*index).id(),
                    duckdb::core::LogicalTypeId::Hugeint | duckdb::core::LogicalTypeId::UHugeint
                )
            })
            .collect::<Vec<_>>();
        if !huge_columns.is_empty() {
            // Arrow represents HUGEINT as decimal(38), which cannot validate its full 39-digit range.
            // Transfer the other columns in batches and bind the wide integers without decimal conversion.
            use duckdb::arrow::{
                array::{Array, ArrayRef, Int32Array},
                datatypes::{Field, Schema},
                record_batch::RecordBatch,
                util::display::array_value_to_string,
            };
            let mut transfer_name = "__dd_transfer".to_string();
            while names.contains(&transfer_name.to_lowercase()) {
                transfer_name.push('_');
            }
            output
                .execute_batch(&format!(
                    "ALTER TABLE qp_data ADD COLUMN {} INTEGER",
                    quote_identifier(&transfer_name)
                ))
                .map_err(|e| e.to_string())?;
            let other = (0..schema.fields().len())
                .filter(|i| !huge_columns.contains(i))
                .collect::<Vec<_>>();
            let mut fields = other
                .iter()
                .map(|i| schema.field(*i).clone())
                .collect::<Vec<_>>();
            fields.push(Field::new(&transfer_name, DataType::Int32, false));
            let transfer_schema = Arc::new(Schema::new(fields));
            let mut transfer_names = other
                .iter()
                .map(|i| result_names[*i].as_str())
                .collect::<Vec<_>>();
            transfer_names.push(&transfer_name);
            let mut position = 0_i64;
            for batch in retained {
                let mut arrays = other
                    .iter()
                    .map(|i| batch.column(*i).clone())
                    .collect::<Vec<ArrayRef>>();
                arrays.push(Arc::new(Int32Array::from(vec![0; batch.num_rows()])));
                let transfer = RecordBatch::try_new(transfer_schema.clone(), arrays)
                    .map_err(|e| e.to_string())?;
                {
                    let mut appender = output
                        .appender_with_columns("qp_data", &transfer_names)
                        .map_err(|e| e.to_string())?;
                    appender
                        .append_record_batch(transfer)
                        .map_err(|e| e.to_string())?;
                    appender.flush().map_err(|e| e.to_string())?;
                }
                for index in &huge_columns {
                    let mut update = output
                        .prepare(&format!(
                            "UPDATE qp_data SET {} = ? WHERE rowid = ?",
                            quote_identifier(&result_names[*index])
                        ))
                        .map_err(|e| e.to_string())?;
                    for row in 0..batch.num_rows() {
                        let array = batch.column(*index);
                        let value = if array.is_null(row) {
                            duckdb::types::Value::Null
                        } else {
                            let text = array_value_to_string(array.as_ref(), row)
                                .map_err(|e| e.to_string())?;
                            if statement.column_logical_type(*index).id()
                                == duckdb::core::LogicalTypeId::Hugeint
                            {
                                duckdb::types::Value::HugeInt(
                                    text.parse::<i128>().map_err(|e| e.to_string())?,
                                )
                            } else {
                                duckdb::types::Value::UHugeInt(
                                    text.parse::<u128>().map_err(|e| e.to_string())?,
                                )
                            }
                        };
                        update
                            .execute(duckdb::params![value, position + row as i64])
                            .map_err(|e| e.to_string())?;
                    }
                }
                position += batch.num_rows() as i64;
            }
            output
                .execute_batch(&format!(
                    "ALTER TABLE qp_data DROP COLUMN {}",
                    quote_identifier(&transfer_name)
                ))
                .map_err(|e| e.to_string())?;
            return DuckDbStore::from_connection(output).map(|store| (Some((store, truncated)), 0));
        }
        {
            let mut appender = output.appender("qp_data").map_err(|e| e.to_string())?;
            for batch in retained {
                appender
                    .append_record_batch(batch)
                    .map_err(|e| e.to_string())?;
            }
            appender.flush().map_err(|e| e.to_string())?;
        }
        DuckDbStore::from_connection(output).map(|store| (Some((store, truncated)), 0))
    }
}

/// Whether keeping a query as a table failed for the keeping alone, so running
/// it plainly can still work: the query doesn't parse wrapped in
/// `CREATE TABLE … AS FROM (…)`, or the results database couldn't take it.
/// Other errors, such as a conversion error or an interrupt, come from the query
/// itself, and running it again would only repeat them.
fn materialize_only_failure(error: &str) -> bool {
    error.starts_with("Parser Error") || error.contains(RESULTS)
}

fn result_sql_type(
    logical: &duckdb::core::LogicalTypeHandle,
    arrow: &DataType,
) -> Result<String, String> {
    use duckdb::core::LogicalTypeId;
    Ok(match logical.id() {
        LogicalTypeId::Uuid => "UUID".into(),
        LogicalTypeId::Hugeint => "HUGEINT".into(),
        LogicalTypeId::UHugeint => "UHUGEINT".into(),
        LogicalTypeId::TimeTZ => "TIMETZ".into(),
        LogicalTypeId::TimeNs => "TIME_NS".into(),
        LogicalTypeId::Bignum => "BIGNUM".into(),
        LogicalTypeId::Bit => "BIT".into(),
        LogicalTypeId::Decimal => format!(
            "DECIMAL({},{})",
            logical.decimal_width(),
            logical.decimal_scale()
        ),
        LogicalTypeId::Struct => {
            let DataType::Struct(fields) = arrow else {
                return sql_type(arrow);
            };
            format!(
                "STRUCT({})",
                fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| Ok(format!(
                        "{} {}",
                        quote_identifier(f.name()),
                        result_sql_type(&logical.child(i), f.data_type())?
                    )))
                    .collect::<Result<Vec<_>, String>>()?
                    .join(", ")
            )
        }
        LogicalTypeId::List | LogicalTypeId::Array => match arrow {
            DataType::List(field) | DataType::LargeList(field) => format!(
                "{}[]",
                result_sql_type(&logical.child(0), field.data_type())?
            ),
            DataType::FixedSizeList(field, size) => format!(
                "{}[{size}]",
                result_sql_type(&logical.child(0), field.data_type())?
            ),
            _ => sql_type(arrow)?,
        },
        LogicalTypeId::Map => match arrow {
            DataType::Map(field, _) => match field.data_type() {
                DataType::Struct(fields) if fields.len() == 2 => format!(
                    "MAP({}, {})",
                    result_sql_type(&logical.child(0), fields[0].data_type())?,
                    result_sql_type(&logical.child(1), fields[1].data_type())?
                ),
                _ => sql_type(arrow)?,
            },
            _ => sql_type(arrow)?,
        },
        _ if logical.get_alias().as_deref() == Some("JSON") => "JSON".into(),
        _ => sql_type(arrow)?,
    })
}

fn sql_type(data_type: &DataType) -> Result<String, String> {
    Ok(match data_type {
        DataType::Null => "VARCHAR".into(),
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int8 => "TINYINT".into(),
        DataType::Int16 => "SMALLINT".into(),
        DataType::Int32 => "INTEGER".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::UInt8 => "UTINYINT".into(),
        DataType::UInt16 => "USMALLINT".into(),
        DataType::UInt32 => "UINTEGER".into(),
        DataType::UInt64 => "UBIGINT".into(),
        DataType::Float16 | DataType::Float32 => "FLOAT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => "VARCHAR".into(),
        DataType::Binary
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::FixedSizeBinary(_) => "BLOB".into(),
        DataType::Date32 | DataType::Date64 => "DATE".into(),
        DataType::Time32(_) | DataType::Time64(_) => "TIME".into(),
        DataType::Timestamp(unit, tz) => {
            if tz.is_some() {
                "TIMESTAMPTZ".into()
            } else {
                match unit {
                    TimeUnit::Second => "TIMESTAMP_S",
                    TimeUnit::Millisecond => "TIMESTAMP_MS",
                    TimeUnit::Microsecond => "TIMESTAMP",
                    TimeUnit::Nanosecond => "TIMESTAMP_NS",
                }
                .into()
            }
        }
        DataType::Decimal128(precision, scale) => format!("DECIMAL({precision},{scale})"),
        DataType::Interval(_) => "INTERVAL".into(),
        DataType::List(field) | DataType::LargeList(field) => {
            format!("{}[]", sql_type(field.data_type())?)
        }
        DataType::FixedSizeList(field, size) => format!("{}[{size}]", sql_type(field.data_type())?),
        DataType::Struct(fields) => format!(
            "STRUCT({})",
            fields
                .iter()
                .map(|f| Ok(format!(
                    "{} {}",
                    quote_identifier(f.name()),
                    sql_type(f.data_type())?
                )))
                .collect::<Result<Vec<_>, String>>()?
                .join(", ")
        ),
        DataType::Dictionary(_, value) => sql_type(value)?,
        DataType::Map(field, _) => match field.data_type() {
            DataType::Struct(fields) if fields.len() == 2 => format!(
                "MAP({}, {})",
                sql_type(fields[0].data_type())?,
                sql_type(fields[1].data_type())?
            ),
            _ => return Err("Unsupported map result schema".into()),
        },
        other => return Err(format!("Unsupported SQL result type: {other}")),
    })
}

/// The rows a SQL source returns, run by the engine it chose.
pub fn sql_source_rows(
    database: Arc<SourceDatabase>,
    config: &crate::domain::documents::SqlConfig,
) -> Result<super::store::Store, String> {
    use crate::domain::documents::Engine;
    match config.engine {
        Engine::DuckDb => SqlExecution::source_rows(database, &config.text).map(Into::into),
        Engine::Polars => super::polars::sql::source_rows(&config.text).map(Into::into),
    }
}

/// A DuckDB aggregate function, for the aggregation manager's reference list.
#[derive(Clone, Debug, PartialEq)]
pub struct AggregateFunction {
    pub name: String,
    pub description: String,
    /// One `name(parameters)` per overload.
    pub signatures: Vec<String>,
    pub examples: Vec<String>,
}

/// Every aggregate function DuckDB has built in, read once from
/// `duckdb_functions()` on a private in-memory connection so it never waits on
/// a running query.
pub fn aggregate_functions() -> Result<Vec<AggregateFunction>, String> {
    static FUNCTIONS: std::sync::OnceLock<Result<Vec<AggregateFunction>, String>> =
        std::sync::OnceLock::new();
    FUNCTIONS.get_or_init(read_aggregate_functions).clone()
}

fn read_aggregate_functions() -> Result<Vec<AggregateFunction>, String> {
    let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
    let sql = concat!(
        "SELECT function_name, coalesce(max(description), ''), ",
        "list_sort(list_distinct(list(",
        "function_name || '(' || array_to_string(parameters, ', ') || ')'))), ",
        "list_sort(list_distinct(flatten(list(coalesce(examples, []))))) ",
        "FROM duckdb_functions() ",
        "WHERE function_type = 'aggregate' AND NOT starts_with(function_name, '__') ",
        "GROUP BY function_name ORDER BY lower(function_name)"
    );
    let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
    let texts = |value: duckdb::types::Value| match value {
        duckdb::types::Value::List(items) => items
            .into_iter()
            .filter_map(|item| match item {
                duckdb::types::Value::Text(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    statement
        .query_map([], |row| {
            Ok(AggregateFunction {
                name: row.get(0)?,
                description: row.get(1)?,
                signatures: texts(row.get(2)?),
                examples: texts(row.get(3)?),
            })
        })
        .and_then(Iterator::collect)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service() -> (tempfile::TempDir, SqlExecution) {
        let dir = tempfile::tempdir().unwrap();
        let service = SqlExecution::new(SourceDatabase::for_profile(dir.path())).unwrap();
        (dir, service)
    }

    #[test]
    fn sql_source_rows_keep_every_row_after_their_connection_closes() {
        let dir = tempfile::tempdir().unwrap();
        let database = SourceDatabase::for_profile(dir.path());
        let text = "CREATE TEMP TABLE numbers AS SELECT range AS n FROM range(250000);\nSELECT n, n % 7 AS bucket FROM numbers";
        let store = SqlExecution::source_rows(database.clone(), text).unwrap();
        assert_eq!(store.row_count(), 250_000);
        assert_eq!(store.schema()[1].name, "bucket");
        let error = SqlExecution::source_rows(database.clone(), "CREATE TEMP TABLE t AS SELECT 1").err().unwrap();
        assert!(error.contains("returns no rows"), "{error}");
        let error = SqlExecution::source_rows(database, "SELECT 1;\nSELECT * FROM missing").err().unwrap();
        assert!(error.starts_with("Statement 2:"), "{error}");
    }

    #[test]
    fn ai_schema_lists_types_and_comments_and_prepares_without_running() {
        let (_dir, service) = service();
        let run = service.run(
            "CREATE TABLE sales AS SELECT 'North' AS region, 12.5::DOUBLE AS \"Unit Price\";
             COMMENT ON TABLE sales IS 'Monthly sales';
             COMMENT ON COLUMN sales.region IS 'Sales region';
             CREATE TEMP VIEW recent AS SELECT * FROM sales",
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        let schema = service.ai_schema().unwrap();
        let sales = schema.iter().find(|r| r.name == "sales").unwrap();
        assert_eq!(sales.comment.as_deref(), Some("Monthly sales"));
        assert_eq!(
            sales.columns,
            [
                SchemaColumn {
                    name: "region".into(),
                    kind: "VARCHAR".into(),
                    comment: Some("Sales region".into()),
                },
                SchemaColumn {
                    name: "Unit Price".into(),
                    kind: "DOUBLE".into(),
                    comment: None,
                },
            ]
        );
        assert!(sales.profile.is_none());
        let recent = schema.iter().find(|r| r.name == "temp.recent").unwrap();
        assert!(recent.is_view);
        // The profile's relations come before temp ones.
        assert!(
            schema.iter().position(|r| r.name == "sales")
                < schema.iter().position(|r| r.name == "temp.recent")
        );
        assert!(!schema.iter().any(|r| r.name.contains("__dd_")));

        assert!(service.prepare_only("SELECT region FROM sales").is_ok());
        let error = service
            .prepare_only("SELECT missing FROM sales")
            .unwrap_err();
        assert!(error.contains("missing"), "{error}");
        // Preparing a write does not run it.
        service.prepare_only("DELETE FROM sales").unwrap();
        let count = service.run("SELECT count(*) AS n FROM sales", 10);
        assert_eq!(count.result.unwrap().row_count(), 1);
    }

    #[test]
    fn profiles_count_always_and_send_values_only_when_allowed() {
        let (_dir, service) = service();
        // 3 regions (all listed), 40 customers (top 5 only), 100 amounts
        // (min and max only), and a list column (counts only).
        let run = service.run(
            "CREATE TABLE orders AS SELECT
                 ['North', 'South', 'East'][1 + range % 3] AS region,
                 'customer ' || (range % 40) AS customer,
                 CASE WHEN range % 10 = 0 THEN NULL ELSE range END AS amount,
                 [range] AS tags
             FROM range(100)",
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        let orders = service
            .ai_schema()
            .unwrap()
            .into_iter()
            .find(|r| r.name == "orders")
            .unwrap();

        let counts = service.profile_relation(&orders, false).unwrap();
        assert_eq!(counts.rows, 100);
        let region = &counts.columns[0];
        assert_eq!((region.nulls, region.min.as_ref()), (0, None));
        assert!(region.values.is_empty());
        assert_eq!(counts.columns[2].nulls, 10);

        let data = service.profile_relation(&orders, true).unwrap();
        let [region, customer, amount, tags] = &data.columns[..] else {
            panic!()
        };
        assert!(region.all_values);
        assert_eq!(region.unique, 3);
        assert_eq!(region.values[0], (Some("North".into()), 34));
        assert_eq!(region.values.len(), 3);
        assert!(!customer.all_values);
        assert_eq!(customer.values.len(), TOP_VALUES);
        assert!(amount.values.is_empty());
        assert_eq!(
            (amount.min.as_deref(), amount.max.as_deref()),
            (Some("1"), Some("99"))
        );
        assert!(tags.min.is_none() && tags.values.is_empty());
    }
    #[test]
    fn a_final_query_keeps_every_row_with_exact_types() {
        let (_dir, service) = service();
        let rows = |run: &SqlRun| run.result.as_ref().unwrap().row_count();
        // Setup statements run first; the last query keeps all 500 rows past the limit.
        let run = service.run(
            "CREATE TEMP TABLE t AS SELECT range AS n FROM range(500);
             SELECT n, n AS n, 170141183460469231731687303715884105727::HUGEINT AS big FROM t -- done",
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        assert!(run.complete && !run.truncated);
        assert_eq!(rows(&run), 500);
        let store = run.result.as_ref().unwrap();
        let schema = store.schema();
        assert_eq!(
            schema.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["n", "n_1", "big"]
        );
        assert_eq!(schema[2].sql_type, "HUGEINT");
        // Writes, EXPLAIN, and statements inside a transaction keep the row limit.
        for sql in [
            "INSERT INTO t SELECT 1 RETURNING n",
            "EXPLAIN SELECT 1",
            "BEGIN; SELECT * FROM t",
        ] {
            let run = service.run(sql, 10);
            assert!(run.error.is_none(), "{sql}: {:?}", run.error);
            assert!(!run.complete, "{sql}");
        }
        assert!(service.run("ROLLBACK", 10).error.is_none());
        // A result's table lasts exactly as long as its store, even after the
        // editor runs again or closes.
        let database = service.database.clone();
        let tables = || {
            database
                .connect()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM duckdb_tables() WHERE database_name = '__dd_results' \
                     AND table_name LIKE '__dd_result_%'",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        let before = tables();
        for _ in 0..3 {
            assert!(service.run("SELECT 1", 10).complete);
        }
        assert_eq!(tables(), before);
        let shown = service.run("SELECT 42 AS n", 10).result.unwrap();
        drop(service);
        assert_eq!(tables(), before + 1);
        assert_eq!(shown.fetch_row_ids(&[0]).unwrap()[0].values, [Some("42".into())]);
        drop(shown);
        assert_eq!(tables(), before);
    }

    #[test]
    fn scripts_run_once_and_keep_last_result_after_failure() {
        let (_dir, service) = service();
        let run = service.run("CREATE TABLE apples(color VARCHAR); INSERT INTO apples VALUES ('red'); SELECT * FROM apples; SELECT missing", 100);
        assert_eq!(run.completed, 3);
        assert!(run.error.unwrap().contains("Statement 4"));
        assert_eq!(run.result.unwrap().row_count(), 1);
        let count = service
            .connection
            .query_row("SELECT count(*) FROM apples", [], |r| r.get::<_, i64>(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn returning_writes_all_rows_and_caps_only_display() {
        let (_dir, service) = service();
        service.run("CREATE TABLE apples(id BIGINT)", 2);
        let run = service.run("INSERT INTO apples SELECT * FROM range(100) RETURNING *", 2);
        assert!(run.error.is_none(), "{:?}", run.error);
        assert!(run.truncated);
        assert_eq!(run.result.unwrap().row_count(), 2);
        assert_eq!(
            service
                .connection
                .query_row("SELECT count(*) FROM apples", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            100
        );
    }
    #[test]
    fn temporary_objects_transactions_and_catalog_survive_runs() {
        let (_dir, service) = service();
        assert!(service.run("CREATE TEMP TABLE apples(color VARCHAR); CREATE VIEW fruit AS SELECT * FROM apples; BEGIN; INSERT INTO apples VALUES ('red')", 100).error.is_none());
        assert_eq!(
            service
                .run("SELECT * FROM apples", 100)
                .result
                .unwrap()
                .row_count(),
            1
        );
        assert!(service.run("ROLLBACK", 100).error.is_none());
        assert_eq!(
            service
                .run("SELECT * FROM apples", 100)
                .result
                .unwrap()
                .row_count(),
            0
        );
        let catalog = service.catalog().unwrap();
        assert!(catalog.iter().any(|r| r.name == "fruit" && r.is_view));
        assert!(!catalog.iter().any(|r| r.name.starts_with("__qp_")));
    }
    #[test]
    fn metadata_functions_complete_their_columns() {
        use crate::domain::sql_editor::completions;
        let (_dir, service) = service();
        let catalog = service.sql_catalog().unwrap();
        for sql in [
            "from duckdb_columns d select d.",
            "from duckdb_columns() d select d.",
            "from main.duckdb_tables t select t.",
            "select v. from pragma_version() v",
        ] {
            let at = sql.find(". ").map_or(sql.len(), |i| i + 1);
            let labels = completions(sql, at, &catalog).into_iter().map(|c| c.label).collect::<Vec<_>>();
            assert!(labels.iter().any(|l| l == "database_name" || l == "library_version"), "{sql}: {labels:?}");
        }
    }

    #[test]
    fn variables_sequences_and_settings_complete_in_their_functions() {
        use crate::domain::sql_editor::completions;
        let (_dir, service) = service();
        let run = service.run(
            "SET VARIABLE qq = 12; CREATE SEQUENCE ser START WITH 1; CREATE SCHEMA other; CREATE SEQUENCE other.ids",
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        let catalog = service.sql_catalog().unwrap();
        let inserts = |sql: &str| {
            completions(sql, sql.len(), &catalog)
                .into_iter()
                .map(|c| (c.label, c.insert_text))
                .collect::<Vec<_>>()
        };
        let variables = inserts("SELECT getvariable(");
        assert!(variables.contains(&("qq".into(), "'qq'".into())), "{variables:?}");
        let sequences = inserts("SELECT nextval('");
        assert!(sequences.contains(&("ser".into(), "ser".into())), "{sequences:?}");
        assert!(sequences.contains(&("other.ids".into(), "other.ids".into())), "{sequences:?}");
        assert!(inserts("SELECT current_setting(").iter().any(|(label, _)| label == "threads"));
    }

    #[test]
    fn attached_database_completions_name_the_database() {
        use crate::domain::sql_editor::completions;
        let (_dir, service) = service();
        let run = service.run(
            "ATTACH ':memory:' AS shop; CREATE TABLE shop.orders(qty INTEGER); CREATE TABLE main.orders(n INTEGER)",
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        let inserts = |service: &SqlExecution| {
            let catalog = service.sql_catalog().unwrap();
            let mut inserts = completions("SELECT * FROM orders", 20, &catalog)
                .into_iter()
                .filter(|c| c.label.contains("orders"))
                .map(|c| c.insert_text)
                .collect::<Vec<_>>();
            inserts.sort();
            inserts
        };
        assert_eq!(inserts(&service), ["orders", "shop.orders"]);
        let qty = service.run("SELECT * FROM shop.orders", 10);
        assert!(qty.error.is_none(), "{:?}", qty.error);
        assert_eq!(qty.result.unwrap().schema()[0].name, "qty");
        // After USE, the profile's tables are the ones that need the database.
        let profile = service.default_catalog().unwrap();
        assert!(service.run("USE shop", 10).error.is_none());
        let inserts = inserts(&service);
        assert!(inserts.contains(&"orders".to_owned()));
        assert!(inserts.contains(&format!("{}.orders", diver_sql::quote_if_needed(&profile))), "{inserts:?}");
    }
    #[test]
    fn database_catalog_completes_files_tables_and_view_alias_columns() {
        use crate::domain::sql_editor::completions;
        let (_dir, service) = service();
        let run = service.run("CREATE TABLE files.amazon_book_parquet(asin VARCHAR, ISBN10 VARCHAR); CREATE VIEW main.book_view AS SELECT * FROM files.amazon_book_parquet", 10);
        assert!(run.error.is_none(), "{:?}", run.error);
        let columns = service.connection.query_row("SELECT count(*) FROM duckdb_columns() WHERE database_name = 'files' AND table_name = 'amazon_book_parquet' AND NOT internal", [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(columns, 2);
        let catalog = service.sql_catalog().unwrap();
        let table = catalog
            .relations
            .iter()
            .find(|r| r.catalog == "files" && r.name == "amazon_book_parquet")
            .unwrap();
        assert_eq!(table.columns, ["asin", "ISBN10"]);
        assert_eq!(table.column_types, ["VARCHAR", "VARCHAR"]);
        let suggestions = completions("SELECT * FROM amazon", 20, &catalog)
            .into_iter()
            .filter(|c| c.label.contains("amazon"))
            .collect::<Vec<_>>();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].insert_text, "files.amazon_book_parquet");
        let run = service.run(&format!("SELECT * FROM {}", suggestions[0].insert_text), 10);
        assert!(run.error.is_none(), "{:?}", run.error);
        for relation in ["files.amazon_book_parquet", "main.book_view"] {
            let text = format!("SELECT a.\nFROM {relation} AS a");
            let suggestions = completions(&text, "SELECT a.".len(), &catalog);
            assert_eq!(
                suggestions
                    .iter()
                    .map(|s| s.label.as_str())
                    .collect::<Vec<_>>(),
                ["asin", "ISBN10"]
            );
            // Mixed case needs no quotes: DuckDB matches it ignoring case.
            let run = service.run(&format!("SELECT a.{} FROM {relation} AS a", suggestions[1].insert_text), 10);
            assert!(run.error.is_none(), "{:?}", run.error);
        }
        // Functions come with their signatures, and macros the script made too.
        let round = catalog.functions.iter().find(|f| f.name == "round").unwrap();
        assert!(round.signatures.iter().any(|s| s.starts_with("round(") && s.contains(" -> ")), "{:?}", round.signatures);
        assert!(catalog.settings.iter().any(|s| s.name == "threads"));
        assert!(
            service
                .run(
                    "ALTER TABLE files.amazon_book_parquet ADD COLUMN title VARCHAR",
                    10
                )
                .catalog
                .relations
                .iter()
                .any(
                    |r| r.name == "amazon_book_parquet" && r.columns == ["asin", "ISBN10", "title"]
                )
        );
    }

    #[test]
    fn a_failing_query_runs_once() {
        let (_dir, service) = service();
        let run = service.run("CREATE SEQUENCE calls; SELECT (nextval('calls') || 'x')::INT AS n", 100);
        assert!(run.error.as_deref().is_some_and(|e| e.starts_with("Statement 2: Conversion Error")), "{:?}", run.error);
        let run = service.run("SELECT currval('calls') AS n", 100);
        let rows = run.result.unwrap().fetch_row_ids(&[0]).unwrap();
        assert_eq!(rows[0].values, [Some("1".into())]);
    }

    #[test]
    fn typed_and_empty_results() {
        let (_dir, service) = service();
        for sql in [
            "SELECT 1.23::DECIMAL(10,2) AS amount, DATE '2026-10-04' AS day, NULL::VARCHAR AS name",
            "SELECT 1 AS a, 2 AS a, 3 AS rowid",
            "SELECT 1 AS x WHERE false",
            "DESCRIBE SELECT 1 AS x",
            "EXPLAIN SELECT 1",
            "SELECT UUID '550e8400-e29b-41d4-a716-446655440000' AS id",
            "SELECT 170141183460469231731687303715884105727::HUGEINT AS big",
            "SELECT [1,2] AS list, {'x': 10} AS item, MAP(['a'],[1]) AS mapping",
        ] {
            let run = service.run(sql, 100);
            assert!(run.error.is_none(), "{sql}: {:?}", run.error);
            assert!(run.result.is_some());
        }
    }

    #[test]
    fn interrupt_stops_running_query_and_connection_can_be_reused() {
        let (_dir, service) = service();
        let interrupt = service.interrupt.clone();
        let started = Arc::new(std::sync::Barrier::new(2));
        let worker_started = started.clone();
        let worker = std::thread::spawn(move || {
            worker_started.wait();
            let run = service.run("SELECT sum(sin(i)) FROM range(10000000000) t(i)", 10);
            (service, run)
        });
        started.wait();
        std::thread::sleep(std::time::Duration::from_millis(40));
        interrupt.interrupt();
        let (service, run) = worker.join().unwrap();
        assert!(run.error.is_some());
        assert!(service.run("SELECT 1", 10).error.is_none());
    }

    #[test]
    fn database_writes_refresh_import_identity_and_completion() {
        let (dir, service) = service();
        let file = dir.path().join("fruit.csv");
        std::fs::write(&file, "id\n1\n").unwrap();
        let original = service
            .database
            .load(&file, super::super::source_database::LoadMode::Check, true)
            .unwrap();
        let identity = original.persistent_identity().unwrap();
        let table = original.table_sql();
        let run = service.run(
            &format!(
                "INSERT INTO {table} VALUES (2); CREATE VIEW fruit_view AS SELECT * FROM {table}"
            ),
            10,
        );
        assert!(run.error.is_none(), "{:?}", run.error);
        assert!(
            run.catalog
                .relations
                .iter()
                .any(|r| r.name == "fruit_view" && r.is_view && r.columns == ["id"])
        );
        match service
            .database
            .load_reusing(
                &file,
                super::super::source_database::LoadMode::Check, true, None,
                Some(&identity),
            )
            .unwrap()
        {
            super::super::source_database::SourceLoad::Loaded(store) => {
                assert_eq!(store.row_count(), 2)
            }
            _ => panic!("mutated source was incorrectly reused"),
        }
        drop(identity);
        drop(original);
        drop(service);
        // SQL writes change the table in memory, never the file, so a new
        // read once nothing holds the table sees the file again.
        let reopened = SourceDatabase::for_profile(dir.path());
        assert_eq!(
            reopened
                .load(&file, super::super::source_database::LoadMode::Check, true)
                .unwrap()
                .row_count(),
            1
        );
    }

    #[test]
    fn aggregate_functions_list_builtins_with_descriptions_and_signatures() {
        let functions = super::aggregate_functions().unwrap();
        let sum = functions
            .iter()
            .find(|function| function.name == "sum")
            .unwrap();
        assert!(!sum.description.is_empty());
        assert!(
            sum.signatures
                .iter()
                .all(|signature| signature.starts_with("sum("))
        );
        assert!(functions.iter().any(|function| function.name == "median"));
        assert!(
            functions
                .iter()
                .all(|function| !function.name.starts_with("__"))
        );
        assert!(
            functions
                .windows(2)
                .all(|pair| pair[0].name.to_lowercase() <= pair[1].name.to_lowercase())
        );
    }
}
