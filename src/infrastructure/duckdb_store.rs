use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::source_database::PersistentBacking;
use duckdb::{Connection, appender_params_from_iter, params, params_from_iter, types::Value};

use crate::domain::{
    documents::{Dimension, ScalarKey, SortKey, ValueFilter},
    formatting::{numeric_type_name, string_type_name},
    query::{
        ColumnOperator, ColumnTerm, Comparison, Numeric, Token, parse_condition, parse_number,
        resolve_column,
    },
    sql::SqlTrace,
    statistics::{ColumnAggregates, NumericColumnStatistics},
};

static NEXT_VIEW_ROW_MAP: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct MaterializedViewRows {
    pub row_ids: Arc<[i64]>,
    pub row_map: Arc<ViewRowMap>,
}

pub struct ViewRowMap {
    table_name: String,
    retired_tables: Arc<Mutex<Vec<String>>>,
}

impl ViewRowMap {
    pub(crate) fn table_name(&self) -> &str {
        &self.table_name
    }
}

impl Drop for ViewRowMap {
    fn drop(&mut self) {
        if let Ok(mut retired) = self.retired_tables.lock() {
            retired.push(self.table_name.clone());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSchema {
    /// The name shown to the user. A physical source column named `rowid` is
    /// preserved under this name while DuckDB's pseudocolumn remains internal.
    pub name: String,
    pub duckdb_type: String,
    pub(crate) storage_name: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewQuery {
    pub tokens: Vec<Token>,
    pub filters: Vec<ColumnFilter>,
    pub value_filters: Vec<ValueFilter>,
    pub sort: Vec<SortKey>,
    /// Columns plain search and numeric comparisons skip. A `col:` term still
    /// reaches them.
    pub hidden: Vec<String>,
}

/// A view's predicates, their parameters, and its `ORDER BY` terms.
type ViewClauses = (Vec<String>, Vec<Value>, Vec<String>);

/// A column profile over every row of a store, ignoring any view's filters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSummary {
    pub name: String,
    pub rows: usize,
    pub nulls: usize,
    /// Exact count of distinct non-null values.
    pub unique: usize,
    /// Rendered as text; `None` when every value is null or the type is nested.
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnFilter {
    pub column: String,
    /// Selected typed values. `ScalarKey::Null` selects SQL NULL. An empty
    /// selection means this filter does not restrict the view.
    pub values: Vec<ScalarKey>,
    /// A condition card's text; when set, it replaces `values`. Blank text
    /// does not restrict the view.
    pub condition: Option<String>,
    /// `condition` is a raw DuckDB predicate over the row; `column` is unused.
    pub sql: bool,
}

/// Whether any SQL card or expression filter names `rowid`.
pub(crate) fn filters_mention_rowid(
    filters: &[ColumnFilter],
    value_filters: &[ValueFilter],
) -> bool {
    use crate::domain::aggregation_sql::mentions_rowid;
    filters
        .iter()
        .any(|filter| filter.sql && filter.condition.as_deref().is_some_and(mentions_rowid))
        || value_filters.iter().any(
            |filter| matches!(&filter.target, Dimension::Expression { sql } if mentions_rowid(sql)),
        )
}

pub struct QueryInput<'a> {
    pub enums: &'a [QueryEnum],
}

pub struct QueryEnum {
    pub name: String,
    /// Bound discovery query returning one VARCHAR dimension label per row.
    pub sql: String,
    pub values: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedRow {
    pub rowid: i64,
    pub values: Vec<Option<String>>,
}

/// How a store opened from a database relation reads its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelationRows {
    /// A DuckDB table, scrolled and sorted through its own rowid.
    Table,
    /// A relation without DuckDB's rowid, read only through limited queries.
    Lazy,
}

/// Owns the authoritative DuckDB table and builds lightweight row-id views.
/// The connection is deliberately kept on its owner thread; callers should
/// schedule its methods on the existing background executor.
pub struct DuckDbStore {
    connection: Connection,
    table_sql: String,
    persistent: Option<PersistentBacking>,
    /// Set for stores opened from a database relation.
    relation: Option<RelationRows>,
    columns: Vec<ColumnSchema>,
    renamed_source_rowid: Option<String>,
    row_count: usize,
    retired_view_row_maps: Arc<Mutex<Vec<String>>>,
    #[cfg(test)]
    width_sampling_calls: std::sync::atomic::AtomicUsize,
}

impl DuckDbStore {
    /// Standalone in-memory importer for embedding and comparison benchmarks.
    /// Desktop file access uses `SourceDatabase::load` and persistent tables.
    pub fn open_source(path: &Path) -> Result<Self, String> {
        if super::source_database::is_gzip_csv(path) {
            return Self::open_delimited(path, b',');
        }
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match extension.as_str() {
            "parquet" | "pq" => Self::open_parquet(path),
            "csv" => Self::open_delimited(path, b','),
            "tsv" => Self::open_delimited(path, b'\t'),
            "xlsx" => Self::open_xlsx(path),
            _ => Err(format!(
                "unsupported DuckDB source type: {}",
                path.display()
            )),
        }
    }

    pub fn empty() -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "CREATE TEMP TABLE qp_data AS SELECT NULL::INTEGER AS \"Row\" WHERE FALSE",
            )
            .map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    pub fn synthetic(count: usize) -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .execute(
                "CREATE TEMP TABLE qp_data AS SELECT range::BIGINT AS \"Row\" FROM range(?)",
                params![count as i64],
            )
            .map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    pub fn from_rows(
        columns: &[(String, String)],
        rows: impl IntoIterator<Item = Vec<Value>>,
    ) -> Result<Self, String> {
        if columns.is_empty() {
            return Self::empty();
        }
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let definitions = columns
            .iter()
            .map(|(name, data_type)| format!("{} {data_type}", quote_identifier(name)))
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute_batch(&format!("CREATE TEMP TABLE qp_data ({definitions})"))
            .map_err(|error| error.to_string())?;
        {
            let mut appender = connection
                .appender("qp_data")
                .map_err(|error| error.to_string())?;
            for values in rows {
                if values.len() != columns.len() {
                    return Err("row width does not match source schema".into());
                }
                appender
                    .append_row(appender_params_from_iter(values.iter()))
                    .map_err(|error| error.to_string())?;
            }
            appender.flush().map_err(|error| error.to_string())?;
        }
        Self::from_connection(connection)
    }

    pub fn materialize_query(&self, sql: &str, values: &[Value]) -> Result<Self, String> {
        self.materialize_query_traced("Aggregation", sql, values, None, &mut SqlTrace::default())
    }

    pub fn materialize_query_traced(
        &self,
        stage: &str,
        sql: &str,
        values: &[Value],
        input: Option<QueryInput<'_>>,
        trace: &mut SqlTrace,
    ) -> Result<Self, String> {
        if let Some(backing) = &self.persistent
            && (backing.source_version.is_some() || self.relation.is_some())
        {
            let connection = self
                .connection
                .try_clone()
                .map_err(|error| error.to_string())?;
            Self::materialize_on(&connection, stage, sql, values, input, trace)?;
            let mut result = Self::from_named_connection(connection, "__qp_result".into())?;
            result.persistent = Some(PersistentBacking {
                database: backing.database.clone(),
                source_version: None,
            });
            return Ok(result);
        }
        Self::materialize_on(&self.connection, stage, sql, values, input, trace)?;
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join("query-result.parquet");
        let path_sql = path.to_string_lossy().replace('\'', "''");
        // The copy is independent, so release the source connection's duplicate.
        self.connection
            .execute_batch(&format!(
                "COPY __qp_result TO '{path_sql}' (FORMAT PARQUET); DROP TABLE __qp_result;"
            ))
            .map_err(|error| error.to_string())?;
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        connection
            .execute(
                "CREATE TEMP TABLE qp_data AS SELECT * FROM read_parquet(?)",
                params![path.to_string_lossy().into_owned()],
            )
            .map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    fn materialize_on(
        connection: &Connection,
        stage: &str,
        sql: &str,
        values: &[Value],
        input: Option<QueryInput<'_>>,
        trace: &mut SqlTrace,
    ) -> Result<(), String> {
        let result = (|| {
            if let Some(input) = &input {
                for enumeration in input.enums {
                    let labels = trace.execute(
                        "Pivot column discovery",
                        &enumeration.sql,
                        &enumeration.values,
                        || {
                            let mut statement = connection
                                .prepare(&enumeration.sql)
                                .map_err(|error| error.to_string())?;
                            let rows = statement
                                .query_map(params_from_iter(enumeration.values.iter()), |row| {
                                    row.get::<_, String>(0)
                                })
                                .map_err(|error| error.to_string())?;
                            rows.collect::<Result<Vec<_>, _>>()
                                .map_err(|error| error.to_string())
                        },
                    )?;
                    let definition = if labels.is_empty() {
                        "SELECT NULL::VARCHAR WHERE FALSE".to_owned()
                    } else {
                        labels
                            .iter()
                            .map(|label| format!("'{}'", label.replace('\'', "''")))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    // ENUM definitions cannot bind labels. These labels come from
                    // the bound discovery query and are encoded as SQL literals.
                    let sql = format!(
                        "CREATE TEMP TYPE {} AS ENUM ({definition})",
                        quote_identifier(&enumeration.name)
                    );
                    trace.execute("Pivot column setup", &sql, &[], || {
                        connection
                            .execute_batch(&sql)
                            .map_err(|error| error.to_string())
                    })?;
                }
            }
            let result_sql = format!("CREATE OR REPLACE TEMP TABLE __qp_result AS (\n{sql}\n)");
            trace.execute(stage, &result_sql, values, || {
                connection
                    .execute(&result_sql, params_from_iter(values.iter()))
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        })();
        if result.is_err() {
            // Failed pivot binding can leave the implicit transaction aborted.
            // Release it before the source is reused or temporary enums are dropped.
            let _ = connection.execute_batch("ROLLBACK");
        }
        if let Some(input) = input {
            let mut cleanup = Ok(());
            for enumeration in input.enums {
                let dropped = connection
                    .execute_batch(&format!(
                        "DROP TYPE IF EXISTS {}",
                        quote_identifier(&enumeration.name)
                    ))
                    .map_err(|error| error.to_string());
                cleanup = cleanup.and(dropped);
            }
            // Keep the original query error if both execution and cleanup fail.
            return result.and(cleanup);
        }
        result
    }

    pub fn open_parquet(path: &Path) -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let path = path.to_string_lossy().into_owned();
        connection
            .execute(
                "CREATE OR REPLACE TEMP TABLE qp_data AS SELECT * FROM read_parquet(?)",
                params![path],
            )
            .map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    /// Load the first worksheet of an XLSX file through DuckDB's `excel` core
    /// extension. DuckDB installs and loads that extension on first use.
    pub fn open_xlsx(path: &Path) -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let path = path.to_string_lossy().into_owned();
        connection
            .execute(
                "CREATE OR REPLACE TEMP TABLE qp_data AS SELECT * FROM read_xlsx(?, header = true)",
                params![path],
            )
            .map_err(|error| {
                format!(
                    "could not read XLSX file '{path}'; DuckDB automatically downloads its excel extension on first use, so the first read may require internet access: {error}"
                )
            })?;
        Self::from_connection(connection)
    }

    pub(crate) fn from_connection(connection: Connection) -> Result<Self, String> {
        Self::from_named_connection(connection, "qp_data".into())
    }

    pub(crate) fn from_named_connection(connection: Connection, table_sql: String) -> Result<Self, String> {
        let mut store = Self {
            connection,
            table_sql,
            persistent: None,
            relation: None,
            columns: Vec::new(),
            renamed_source_rowid: None,
            row_count: 0,
            retired_view_row_maps: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            width_sampling_calls: std::sync::atomic::AtomicUsize::new(0),
        };
        store.renamed_source_rowid = store.normalize_source_rowid_column()?;
        store.discover_schema()?;
        store.row_count = store
            .connection
            .query_row(
                &format!("SELECT count(*) FROM {}", store.table_sql),
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| error.to_string())? as usize;
        Ok(store)
    }

    pub(crate) fn from_persistent(
        connection: Connection,
        table_sql: String,
        renamed_source_rowid: Option<String>,
        row_count: usize,
        backing: PersistentBacking,
    ) -> Result<Self, String> {
        let mut store = Self {
            connection,
            table_sql,
            persistent: Some(backing),
            relation: None,
            columns: Vec::new(),
            renamed_source_rowid,
            row_count,
            retired_view_row_maps: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            width_sampling_calls: std::sync::atomic::AtomicUsize::new(0),
        };
        store.discover_schema()?;
        Ok(store)
    }

    pub fn table_sql(&self) -> &str {
        &self.table_sql
    }

    /// Stops the query running on this store's connection, if any.
    pub(crate) fn interrupt_handle(&self) -> Arc<duckdb::InterruptHandle> {
        self.connection.interrupt_handle()
    }

    pub(crate) fn from_database_view(
        connection: Connection,
        database: Arc<super::source_database::SourceDatabase>,
        view: &crate::domain::documents::DatabaseView,
    ) -> Result<Self, String> {
        let relation = view.relation_sql();
        if view.kind == crate::domain::documents::RelationKind::Table {
            let row_count = connection
                .query_row(&format!("SELECT count(*) FROM {relation}"), [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())? as usize;
            let mut store = Self::from_persistent(
                connection,
                relation,
                None,
                row_count,
                PersistentBacking {
                    database,
                    source_version: None,
                },
            )?;
            store.relation = Some(RelationRows::Table);
            return Ok(store);
        }
        let mut columns = {
            let mut statement = connection
                .prepare(&format!("DESCRIBE SELECT * FROM {relation}"))
                .map_err(|error| error.to_string())?;
            statement
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    Ok(ColumnSchema {
                        storage_name: name.clone(),
                        name,
                        duckdb_type: row.get(1)?,
                    })
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?
        };
        // Views lack DuckDB's pseudocolumn, so queries that read rowid number
        // the rows (see `query_relation`). A user column named rowid is renamed
        // so that number cannot collide with it.
        let table_sql = if let Some(index) = columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case("rowid"))
        {
            let mut name = format!("{} (source column)", columns[index].name);
            while columns
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(&name))
            {
                name.push('_');
            }
            columns[index].storage_name = name;
            let projection = columns
                .iter()
                .map(|column| {
                    format!(
                        "{} AS {}",
                        quote_identifier(&column.name),
                        quote_identifier(&column.storage_name)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("(SELECT {projection} FROM {relation})")
        } else {
            relation
        };
        Ok(Self {
            connection,
            table_sql,
            persistent: Some(PersistentBacking {
                database,
                source_version: None,
            }),
            relation: Some(RelationRows::Lazy),
            columns,
            renamed_source_rowid: None,
            // The relation is lazy: opening it binds its schema without scanning rows.
            row_count: 0,
            retired_view_row_maps: Arc::new(Mutex::new(Vec::new())),
            #[cfg(test)]
            width_sampling_calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Whether this store is a relation read only through limited queries.
    pub fn is_database_view(&self) -> bool {
        self.relation == Some(RelationRows::Lazy)
    }

    /// The FROM for a query over this store, adding `extra` projections. With
    /// `rowid`, the relation has a `rowid` column: DuckDB's own, or a row
    /// number for relations without one, which costs a window over every row,
    /// so callers ask for it only when the query reads rowid.
    pub fn query_relation(&self, rowid: bool, extra: &[String]) -> String {
        let numbered = rowid && self.is_database_view();
        let relation = if numbered {
            format!(
                "(SELECT *, row_number() OVER () - 1 AS rowid FROM {})",
                self.table_sql
            )
        } else {
            self.table_sql.clone()
        };
        let extra = extra
            .iter()
            .map(|sql| format!(", {sql}"))
            .collect::<String>();
        if extra.is_empty() {
            relation
        } else if rowid && !numbered {
            // `*` leaves out DuckDB's rowid pseudocolumn.
            format!("(SELECT *, rowid AS rowid{extra} FROM {relation})")
        } else {
            format!("(SELECT *{extra} FROM {relation})")
        }
    }

    pub fn database_view_revision(&self) -> Option<u64> {
        self.relation
            .map(|_| self.persistent.as_ref().unwrap().database.revision())
    }

    pub fn persistent_identity(&self) -> Option<super::source_database::PersistentSourceIdentity> {
        let backing = self
            .persistent
            .as_ref()
            .filter(|backing| backing.source_version.is_some())?;
        Some(super::source_database::PersistentSourceIdentity {
            database: backing.database.clone(),
            table_sql: self.table_sql.clone(),
            source_version: backing.source_version.unwrap(),
            database_revision: backing.database.revision(),
        })
    }

    /// Load delimited text through DuckDB's reader, keeping the table in this
    /// connection as a TEMP TABLE just like Parquet sources.
    pub fn open_delimited(path: &Path, delimiter: u8) -> Result<Self, String> {
        let connection = Connection::open_in_memory().map_err(|error| error.to_string())?;
        let compression = if super::source_database::is_gzip_csv(path) {
            "gzip"
        } else {
            "auto"
        };
        let path = path.to_string_lossy().into_owned();
        connection
            .execute(
                "CREATE OR REPLACE TEMP TABLE qp_data AS SELECT * FROM read_csv(?, delim = ?, header = true, auto_detect = true, compression = ?)",
                params![path, (delimiter as char).to_string(), compression],
            )
            .map_err(|error| error.to_string())?;
        Self::from_connection(connection)
    }

    fn normalize_source_rowid_column(&mut self) -> Result<Option<String>, String> {
        super::source_database::normalize_rowid(&self.connection, &self.table_sql)
    }

    fn discover_schema(&mut self) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare(&format!("DESCRIBE {}", self.table_sql))
            .map_err(|error| error.to_string())?;
        self.columns = statement
            .query_map([], |row| {
                let storage_name = row.get::<_, String>(0)?;
                let duckdb_type = row.get::<_, String>(1)?;
                let name = if self.renamed_source_rowid.as_deref() == Some(&storage_name) {
                    storage_name
                        .split(" (source column")
                        .next()
                        .unwrap_or("rowid")
                        .to_owned()
                } else {
                    storage_name.clone()
                };
                Ok(ColumnSchema {
                    name,
                    duckdb_type,
                    storage_name,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Bind a dimension in a scalar context without scanning source rows.
    /// The output column names of `sql`, found by running it with no rows.
    pub fn query_column_names(&self, sql: &str, values: &[Value]) -> Result<Vec<String>, String> {
        let mut statement = self
            .connection
            .prepare(&format!("SELECT * FROM (\n{sql}\n) LIMIT 0"))
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query(params_from_iter(values.iter()))
            .map_err(|error| error.to_string())?;
        drop(rows);
        Ok(statement.column_names())
    }

    pub fn dimension_name(&self, sql: &str) -> Result<String, String> {
        crate::domain::aggregation_sql::validate_dimension(sql)?;
        let body = crate::domain::aggregation_sql::dimension_body(sql)?;
        let query = format!(
            "DESCRIBE SELECT {sql}\nFROM {} WHERE ({body}\n) IS NULL",
            self.query_relation(true, &[])
        );
        let mut statement = self
            .connection
            .prepare(&query)
            .map_err(|error| error.to_string())?;
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if names.len() != 1 {
            return Err("Enter one result expression per dimension".into());
        }
        Ok(names[0].clone())
    }

    pub fn schema(&self) -> &[ColumnSchema] {
        &self.columns
    }

    pub fn column(&self, name: &str) -> Option<&ColumnSchema> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// Row, null, and exact distinct counts plus min and max for each column,
    /// over every row of this store, in one scan.
    /// Each column's smallest and largest value as numbers, or `None` when it
    /// has no numeric values.
    pub fn numeric_ranges(&self, columns: &[String]) -> Result<Vec<Option<(f64, f64)>>, String> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let mut projection = Vec::new();
        for name in columns {
            let column = self
                .column(name)
                .ok_or_else(|| format!("column not found: {name}"))?;
            let value = format!(
                "TRY_CAST({} AS DOUBLE)",
                quote_identifier(&column.storage_name)
            );
            projection.push(format!("min({value})"));
            projection.push(format!("max({value})"));
        }
        let sql = format!("SELECT {} FROM {}", projection.join(", "), self.table_sql);
        let error = |error: duckdb::Error| error.to_string();
        let mut statement = self.connection.prepare(&sql).map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let row = rows.next().map_err(error)?.ok_or("missing range row")?;
        (0..columns.len())
            .map(|index| {
                let min = row.get::<_, Option<f64>>(index * 2).map_err(error)?;
                let max = row.get::<_, Option<f64>>(index * 2 + 1).map_err(error)?;
                Ok(min.zip(max))
            })
            .collect()
    }

    pub fn column_summaries(&self, columns: &[String]) -> Result<Vec<ColumnSummary>, String> {
        self.drop_retired_view_row_maps()?;
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let mut projection = vec!["count(*)".to_owned()];
        for name in columns {
            let column = self
                .columns
                .iter()
                .find(|column| &column.name == name)
                .ok_or_else(|| format!("column not found: {name}"))?;
            let identifier = quote_identifier(&column.storage_name);
            projection.push(format!("count({identifier})"));
            projection.push(format!("count(DISTINCT {identifier})"));
            // Nested values have no useful order to report.
            if is_nested_type(&column.duckdb_type) {
                projection.push("NULL".into());
                projection.push("NULL".into());
            } else {
                projection.push(format!("CAST(min({identifier}) AS VARCHAR)"));
                projection.push(format!("CAST(max({identifier}) AS VARCHAR)"));
            }
        }
        let sql = format!("SELECT {} FROM {}", projection.join(", "), self.table_sql);
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        let row = rows
            .next()
            .map_err(|error| error.to_string())?
            .ok_or("missing summary row")?;
        let error = |error: duckdb::Error| error.to_string();
        let total = row.get::<_, i64>(0).map_err(error)? as usize;
        columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let at = 1 + index * 4;
                Ok(ColumnSummary {
                    name: name.clone(),
                    rows: total,
                    nulls: total - row.get::<_, i64>(at).map_err(error)? as usize,
                    unique: row.get::<_, i64>(at + 1).map_err(error)? as usize,
                    min: row.get::<_, Option<String>>(at + 2).map_err(error)?,
                    max: row.get::<_, Option<String>>(at + 3).map_err(error)?,
                })
            })
            .collect()
    }

    /// The `limit` most frequent values of `column` with their row counts, most
    /// frequent first; ties follow the column's own order. `None` is SQL NULL.
    pub fn top_values(
        &self,
        column: &str,
        limit: usize,
    ) -> Result<Vec<(Option<String>, usize)>, String> {
        let column = self
            .columns
            .iter()
            .find(|candidate| candidate.name == column)
            .ok_or_else(|| format!("column not found: {column}"))?;
        let identifier = quote_identifier(&column.storage_name);
        let sql = format!(
            "SELECT CAST({identifier} AS VARCHAR) AS value, count(*) AS n FROM {}              GROUP BY {identifier} ORDER BY n DESC, {identifier} NULLS LAST LIMIT {limit}",
            self.table_sql
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)? as usize,
                ))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn unique_counts(&self, columns: &[String]) -> Result<Vec<(String, usize)>, String> {
        self.distinct_counts(columns, "count(DISTINCT {})")
    }

    /// HyperLogLog estimates from a single scan, without exact per-value hash tables.
    pub fn approximate_unique_counts(
        &self,
        columns: &[String],
    ) -> Result<Vec<(String, usize)>, String> {
        self.distinct_counts(columns, "approx_count_distinct({})")
    }

    fn distinct_counts(
        &self,
        columns: &[String],
        aggregate: &str,
    ) -> Result<Vec<(String, usize)>, String> {
        self.drop_retired_view_row_maps()?;
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let sql_columns = columns
            .iter()
            .map(|name| {
                self.columns
                    .iter()
                    .find(|column| &column.name == name)
                    .map(|column| aggregate.replace("{}", &quote_identifier(&column.storage_name)))
                    .ok_or_else(|| format!("column not found: {name}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let sql = format!("SELECT {} FROM {}", sql_columns.join(", "), self.table_sql);
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        let row = rows
            .next()
            .map_err(|error| error.to_string())?
            .ok_or("missing count row")?;
        columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                row.get::<_, i64>(index)
                    .map(|count| (name.clone(), count as usize))
                    .map_err(|error| error.to_string())
            })
            .collect()
    }

    pub fn distinct_values(
        &self,
        names: &[String],
        filters: &[ColumnFilter],
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        self.drop_retired_view_row_maps()?;
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let columns = names
            .iter()
            .map(|name| {
                self.columns
                    .iter()
                    .find(|column| &column.name == name)
                    .ok_or_else(|| format!("column not found: {name}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let projection = columns
            .iter()
            .map(|column| quote_identifier(&column.storage_name))
            .collect::<Vec<_>>();
        let outer_projection = projection
            .iter()
            .map(|column| format!("CAST({column} AS VARCHAR)"))
            .collect::<Vec<_>>();
        let (where_sql, parameters) = self.build_filter_clause(filters, &[])?;
        let sql = format!(
            "SELECT {} FROM (SELECT DISTINCT {} FROM {}{} ORDER BY {}) AS __qp_distinct",
            outer_projection.join(", "),
            projection.join(", "),
            self.query_relation(filters_mention_rowid(filters, &[]), &[]),
            where_sql,
            projection.join(", ")
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                (0..names.len())
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<duckdb::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// Probe one value beyond the limit to distinguish eligible columns from larger ones.
    pub fn slicer_values(
        &self,
        name: &str,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Vec<crate::domain::slicers::SlicerValue>, String> {
        let column = self
            .columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| format!("slicer column not found: {name}"))?;
        let identifier = quote_identifier(&column.storage_name);
        let order = if crate::domain::schema::from_duckdb(&column.duckdb_type)
            == crate::domain::schema::ColumnType::String
        {
            format!("CAST({identifier} AS VARCHAR)")
        } else {
            identifier.clone()
        };
        let (where_sql, parameters) = self.build_filter_clause(filters, value_filters)?;
        let sql = format!(
            "SELECT {identifier}, CAST({identifier} AS VARCHAR) FROM \
             (SELECT DISTINCT {identifier} FROM {}{where_sql} LIMIT {}) \
             ORDER BY {order} ASC NULLS LAST",
            self.query_relation(filters_mention_rowid(filters, value_filters), &[]),
            crate::domain::slicers::MAX_UNIQUE_VALUES + 1
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let mut rows = statement
            .query(params_from_iter(parameters.iter()))
            .map_err(|error| error.to_string())?;
        let mut values = Vec::new();
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            let value = row.get::<_, Value>(0).map_err(|error| error.to_string())?;
            let key = crate::domain::slicers::scalar_key(value, &column.duckdb_type)?;
            let label = match &key {
                ScalarKey::Null => "(Blank)".into(),
                ScalarKey::String(value) if value.is_empty() => "(Empty)".into(),
                _ => row.get::<_, String>(1).map_err(|error| error.to_string())?,
            };
            values.push(crate::domain::slicers::SlicerValue {
                key,
                label,
                available: true,
            });
        }
        Ok(values)
    }

    pub fn initial_row_ids(&self) -> Result<Vec<i64>, String> {
        self.query_row_ids(&ViewQuery {
            hidden: Vec::new(),
            value_filters: Vec::new(),
            tokens: Vec::new(),
            filters: Vec::new(),
            sort: Vec::new(),
        })
    }

    pub fn query_row_ids(&self, view: &ViewQuery) -> Result<Vec<i64>, String> {
        self.drop_retired_view_row_maps()?;
        let (sql, values) = self.build_view_sql(view)?;
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        statement
            .query_map(params_from_iter(values.iter()), |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    /// Materialize both the ordered source-row mapping and the rendering
    /// vector from one query. Display positions are zero-based and independent
    /// of DuckDB's physical row IDs.
    pub fn query_view_rows(&self, view: &ViewQuery) -> Result<MaterializedViewRows, String> {
        self.query_view_rows_traced(view, &mut SqlTrace::default())
    }

    pub fn query_view_rows_traced(
        &self,
        view: &ViewQuery,
        trace: &mut SqlTrace,
    ) -> Result<MaterializedViewRows, String> {
        self.drop_retired_view_row_maps()?;
        let (row_sql, values) = self.build_view_sql(view)?;
        let (table_name, row_map) = self.allocate_view_row_map();
        let create_sql = format!(
            "CREATE TEMP TABLE {} AS\nSELECT\n    row_number() OVER () - 1 AS display_position,\n    ordered.source_row_id\nFROM (\n{row_sql}\n) AS ordered(source_row_id)",
            quote_identifier(&table_name)
        );
        let create_start = std::time::Instant::now();
        trace.execute("Result search and sort", &create_sql, &values, || {
            self.connection
                .execute(&create_sql, params_from_iter(values.iter()))
                .map(|_| ())
                .map_err(|error| error.to_string())
        })?;
        super::performance::record("row_map_creation", create_start);
        self.collect_view_rows(table_name, row_map)
    }

    /// Build a mapping for an already materialized row order. This is used
    /// only when recovering a snapshot that predates its mapping table.
    pub fn materialize_view_rows_from_ids(
        &self,
        row_ids: &[i64],
    ) -> Result<MaterializedViewRows, String> {
        self.drop_retired_view_row_maps()?;
        let (table_name, row_map) = self.allocate_view_row_map();
        self.connection
            .execute_batch(&format!(
                "CREATE TEMP TABLE {} (display_position BIGINT, source_row_id BIGINT)",
                quote_identifier(&table_name)
            ))
            .map_err(|error| error.to_string())?;
        if !row_ids.is_empty() {
            let mut appender = self
                .connection
                .appender(&table_name)
                .map_err(|error| error.to_string())?;
            for (position, row_id) in row_ids.iter().enumerate() {
                appender
                    .append_row(appender_params_from_iter(
                        [Value::BigInt(position as i64), Value::BigInt(*row_id)].iter(),
                    ))
                    .map_err(|error| error.to_string())?;
            }
            appender.flush().map_err(|error| error.to_string())?;
        }
        self.collect_view_rows(table_name, row_map)
    }

    fn allocate_view_row_map(&self) -> (String, Arc<ViewRowMap>) {
        let id = NEXT_VIEW_ROW_MAP.fetch_add(1, Ordering::Relaxed);
        let table_name = format!("__qp_view_rows_{id}");
        let row_map = Arc::new(ViewRowMap {
            table_name: table_name.clone(),
            retired_tables: self.retired_view_row_maps.clone(),
        });
        (table_name, row_map)
    }

    fn collect_view_rows(
        &self,
        table_name: String,
        row_map: Arc<ViewRowMap>,
    ) -> Result<MaterializedViewRows, String> {
        let _span = super::performance::Span::new("row_id_extraction");
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT source_row_id FROM {} ORDER BY display_position",
                quote_identifier(&table_name)
            ))
            .map_err(|error| error.to_string())?;
        let row_ids = statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(MaterializedViewRows {
            row_ids: row_ids.into(),
            row_map,
        })
    }

    fn drop_retired_view_row_maps(&self) -> Result<(), String> {
        let mut retired = self
            .retired_view_row_maps
            .lock()
            .map_err(|_| "view row map cleanup queue poisoned".to_owned())?;
        let table_names = std::mem::take(&mut *retired);
        drop(retired);
        for (index, table_name) in table_names.iter().enumerate() {
            if let Err(error) = self.connection.execute_batch(&format!(
                "DROP TABLE IF EXISTS {}",
                quote_identifier(table_name)
            )) {
                if let Ok(mut retired) = self.retired_view_row_maps.lock() {
                    retired.extend(table_names[index..].iter().cloned());
                }
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    fn build_view_sql(&self, view: &ViewQuery) -> Result<(String, Vec<Value>), String> {
        let (predicates, parameters, order) = self.view_clauses(view)?;
        let mut sql = format!("    SELECT rowid\n    FROM {}", self.table_sql);
        if !predicates.is_empty() {
            sql.push_str("\n    WHERE ");
            sql.push_str(&predicates.join("\n        AND "));
        }
        sql.push_str("\n    ORDER BY ");
        for term in &order {
            sql.push_str(term);
            sql.push_str(", ");
        }
        sql.push_str("rowid ASC");
        Ok((sql, parameters))
    }

    /// Applies `view`'s search, filters, and sort to the rows of `inner_sql`, a
    /// query producing this store's columns in order, before `LIMIT limit`.
    /// Returns the SQL and the parameters to bind after `inner_sql`'s own.
    pub fn wrap_query(
        &self,
        view: &ViewQuery,
        inner_sql: &str,
        limit: u64,
    ) -> Result<(String, Vec<Value>), String> {
        let (predicates, parameters, order) = self.view_clauses(view)?;
        let aliases = self
            .columns
            .iter()
            .map(|column| quote_identifier(&column.storage_name))
            .collect::<Vec<_>>()
            .join(", ");
        let projection = self
            .columns
            .iter()
            .map(|column| {
                format!(
                    "{} AS {}",
                    quote_identifier(&column.storage_name),
                    quote_identifier(&column.name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut sql =
            format!("SELECT {projection}\nFROM (\n{inner_sql}\n) AS __qp_query({aliases})");
        if !predicates.is_empty() {
            sql.push_str("\nWHERE ");
            sql.push_str(&predicates.join("\n    AND "));
        }
        if !order.is_empty() {
            sql.push_str("\nORDER BY ");
            sql.push_str(&order.join(", "));
        }
        sql.push_str(&format!("\nLIMIT {limit}"));
        Ok((sql, parameters))
    }

    /// `view`'s predicates, their parameters, and its sort terms over this
    /// store's columns.
    fn view_clauses(&self, view: &ViewQuery) -> Result<ViewClauses, String> {
        let mut predicates = Vec::new();
        let mut parameters = Vec::new();
        for token in &view.tokens {
            match token {
                Token::Contains(text) => {
                    let predicate = self.search_predicate(text, &view.hidden, &mut parameters);
                    predicates.push(predicate);
                }
                Token::Excludes(text) => {
                    let predicate = self.search_predicate(text, &view.hidden, &mut parameters);
                    predicates.push(format!("NOT ({predicate})"));
                }
                Token::Compare(comparison, number) => {
                    let predicates_for_columns = self
                        .columns
                        .iter()
                        .filter(|column| {
                            is_numeric_type(&column.duckdb_type)
                                && !view.hidden.contains(&column.name)
                        })
                        .map(|column| {
                            numeric_predicate(column, number, *comparison, &mut parameters)
                        })
                        .collect::<Vec<_>>();
                    predicates.push(if predicates_for_columns.is_empty() {
                        "FALSE".to_owned()
                    } else {
                        format!("({})", predicates_for_columns.join(" OR "))
                    });
                }
                Token::Column(term) => {
                    let column = resolve_column(
                        self.columns.iter().map(|column| column.name.as_str()),
                        &term.column,
                    )
                    .map(|index| &self.columns[index]);
                    let predicate = match (column, term.fallback()) {
                        (Some(column), _) => self.term_predicate(column, term, &mut parameters)?,
                        (None, Token::Contains(text) | Token::Excludes(text)) => {
                            self.search_predicate(&text, &view.hidden, &mut parameters)
                        }
                        (None, _) => unreachable!("fallback is a text token"),
                    };
                    predicates.push(if term.negated {
                        format!("NOT ({predicate})")
                    } else {
                        predicate
                    });
                }
            }
        }

        let (filter_sql, filter_values) =
            self.build_filter_clause(&view.filters, &view.value_filters)?;
        if !filter_sql.is_empty() {
            predicates.push(filter_sql.trim_start_matches(" WHERE ").to_owned());
            parameters.extend(filter_values);
        }

        let order = view
            .sort
            .iter()
            .map(|sort| {
                let column = self
                    .columns
                    .iter()
                    .find(|column| column.name == sort.column)
                    .ok_or_else(|| format!("sort column not found: {}", sort.column))?;
                Ok(format!(
                    "{} {} NULLS LAST",
                    quote_identifier(&column.storage_name),
                    if sort.descending { "DESC" } else { "ASC" }
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok((predicates, parameters, order))
    }

    /// SQL rendering a column or row-level expression as the grid displays it.
    fn rendered_dimension(&self, target: &Dimension) -> Result<String, String> {
        Ok(match target {
            Dimension::Column(name) => {
                let column = self
                    .columns
                    .iter()
                    .find(|column| &column.name == name)
                    .ok_or_else(|| format!("filter column not found: {name}"))?;
                format!(
                    "CAST({} AS VARCHAR)",
                    quote_identifier(&column.storage_name)
                )
            }
            Dimension::Expression { sql } => format!(
                "CAST(({}
) AS VARCHAR)",
                crate::domain::aggregation_sql::dimension_body(sql)?
            ),
        })
    }

    /// Distinct rendered combinations of `dimensions` among rows matching the filters.
    pub fn distinct_rendered(
        &self,
        dimensions: &[Dimension],
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        if dimensions.is_empty() {
            return Ok(Vec::new());
        }
        let projection = dimensions
            .iter()
            .map(|dimension| self.rendered_dimension(dimension))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let (where_sql, parameters) = self.build_filter_clause(filters, value_filters)?;
        let rowid = filters_mention_rowid(filters, value_filters)
            || dimensions.iter().any(|dimension| {
                matches!(dimension, Dimension::Expression { sql }
                    if crate::domain::aggregation_sql::mentions_rowid(sql))
            });
        let sql = format!(
            "SELECT DISTINCT {projection} FROM {}{where_sql}",
            self.query_relation(rowid, &[])
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                (0..dimensions.len())
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<duckdb::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    }

    pub fn build_filter_clause(
        &self,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<(String, Vec<Value>), String> {
        let mut predicates = Vec::new();
        let mut parameters = Vec::new();
        for filter in value_filters {
            let rendered = self.rendered_dimension(&filter.target)?;
            predicates.push(match (&filter.value, filter.exclude) {
                (None, false) => format!("{rendered} IS NULL"),
                (None, true) => format!("{rendered} IS NOT NULL"),
                (Some(value), exclude) => {
                    parameters.push(Value::Text(value.clone()));
                    let operator = if exclude { "IS DISTINCT FROM" } else { "=" };
                    format!("{rendered} {operator} ?")
                }
            });
        }
        for filter in filters {
            if let Some(text) = &filter.condition {
                // Blank text leaves the card inactive.
                if !text.trim().is_empty() {
                    predicates.push(if filter.sql {
                        format!("({text})")
                    } else {
                        let column = self.filter_column(&filter.column)?;
                        self.condition_predicate(column, text, &mut parameters)?
                    });
                }
                continue;
            }
            if filter.values.is_empty() {
                continue;
            }
            let column = self.filter_column(&filter.column)?;
            let mut alternatives = Vec::new();
            for value in &filter.values {
                match value {
                    ScalarKey::Null => alternatives.push(format!(
                        "{} IS NULL",
                        quote_identifier(&column.storage_name)
                    )),
                    value => {
                        let decimal_cast = if let ScalarKey::Decimal(value) = value {
                            Some(format!(
                                "DECIMAL(38, {})",
                                value
                                    .split_once('.')
                                    .map_or(0, |(_, fraction)| fraction.len())
                            ))
                        } else {
                            None
                        };
                        let (parameter, cast) = if let ScalarKey::Decimal(value) = value {
                            (
                                Value::Text(value.clone()),
                                decimal_cast.as_deref().unwrap_or("DECIMAL(38, 0)"),
                            )
                        } else {
                            scalar_parameter(value)?
                        };
                        parameters.push(parameter);
                        let left = quote_identifier(&column.storage_name);
                        alternatives.push(
                            if column.duckdb_type == "VARCHAR"
                                && matches!(value, ScalarKey::String(_))
                            {
                                format!("{left} IS NOT DISTINCT FROM ?")
                            } else {
                                format!("{left} IS NOT DISTINCT FROM CAST(? AS {cast})")
                            },
                        );
                    }
                }
            }
            predicates.push(format!("({})", alternatives.join(" OR ")));
        }
        let where_sql = if predicates.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", predicates.join(" AND "))
        };
        Ok((where_sql, parameters))
    }

    /// A scoped term's predicate; a double-quoted value naming a column compares
    /// the two columns row by row.
    fn term_predicate(
        &self,
        column: &ColumnSchema,
        term: &ColumnTerm,
        parameters: &mut Vec<Value>,
    ) -> Result<String, String> {
        match (
            term.operator,
            term.compared_column(self.columns.iter().map(|column| column.name.as_str())),
        ) {
            (ColumnOperator::Compare(comparison), Some(other)) => {
                Ok(column_comparison(column, &self.columns[other], comparison))
            }
            _ => column_predicate(column, term, parameters),
        }
    }

    fn filter_column(&self, name: &str) -> Result<&ColumnSchema, String> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .ok_or_else(|| format!("filter column not found: {name}"))
    }

    /// Predicate for a condition card on `column`.
    fn condition_predicate(
        &self,
        column: &ColumnSchema,
        text: &str,
        parameters: &mut Vec<Value>,
    ) -> Result<String, String> {
        let today = chrono::Local::now().date_naive();
        let predicates = parse_condition(text, &column.name, today)?
            .iter()
            .map(|term| {
                let predicate = self.term_predicate(column, term, parameters)?;
                Ok(if term.negated {
                    format!("NOT ({predicate})")
                } else {
                    predicate
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(format!("({})", predicates.join(" AND ")))
    }

    /// Checks that a condition card's text parses and binds, without running it.
    pub fn validate_condition(&self, column: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let column = self.filter_column(column)?;
        let mut parameters = Vec::new();
        let predicate = self.condition_predicate(column, text, &mut parameters)?;
        self.validate_predicate(&predicate)
    }

    /// Checks that an SQL card's predicate binds against the source, without
    /// running it.
    pub fn validate_sql(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        self.validate_predicate(&format!("({text})"))
    }

    fn validate_predicate(&self, predicate: &str) -> Result<(), String> {
        self.connection
            .prepare(&format!(
                "SELECT 1 FROM {} WHERE {predicate} LIMIT 0",
                self.query_relation(true, &[])
            ))
            .map(|_| ())
            .map_err(|error| {
                error
                    .to_string()
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
    }

    /// The smallest and largest non-null values of `column`, as displayed.
    pub fn column_bounds(&self, column: &str) -> Result<Option<(String, String)>, String> {
        let column = self.filter_column(column)?;
        let identifier = quote_identifier(&column.storage_name);
        self.connection
            .query_row(
                &format!(
                    "SELECT CAST(min({identifier}) AS VARCHAR), CAST(max({identifier}) AS VARCHAR) FROM {}",
                    self.table_sql
                ),
                [],
                |row| {
                    Ok(row
                        .get::<_, Option<String>>(0)?
                        .zip(row.get::<_, Option<String>>(1)?))
                },
            )
            .map_err(|error| error.to_string())
    }

    fn search_predicate(
        &self,
        text: &str,
        hidden: &[String],
        parameters: &mut Vec<Value>,
    ) -> String {
        let needle = text.to_lowercase();
        let predicates = self
            .columns
            .iter()
            .filter(|column| !hidden.contains(&column.name))
            .filter_map(|column| {
                let identifier = quote_identifier(&column.storage_name);
                let value = match search_rendering(&column.duckdb_type)? {
                    SearchRendering::Text => format!("lower(CAST({identifier} AS VARCHAR))"),
                    // Converting every value to text dominates search on wide numeric
                    // tables, so skip columns whose rendering cannot contain the needle.
                    SearchRendering::Lowercase(renders) => {
                        if !needle.chars().all(renders) {
                            return None;
                        }
                        format!("CAST({identifier} AS VARCHAR)")
                    }
                };
                parameters.push(Value::Text(needle.clone()));
                Some(format!("coalesce(contains({value}, ?), FALSE)"))
            })
            .collect::<Vec<_>>();
        if predicates.is_empty() {
            "FALSE".to_owned()
        } else {
            format!("({})", predicates.join(" OR "))
        }
    }

    /// Fetch one display page by resolving GUI positions through the view's
    /// ordered row IDs. Values are fetched in one query and reordered to match.
    pub fn fetch_page(
        &self,
        row_ids: &[i64],
        start: usize,
        count: usize,
    ) -> Result<Vec<CachedRow>, String> {
        self.fetch_view_page(None, row_ids, start, count)
    }

    /// Fetch display positions `start..start + count`. With the view's row map, rows
    /// are selected by position inside DuckDB: a sort scatters row IDs across the
    /// table, where a literal IN list is several times slower and long ranges
    /// (copies and exports) would otherwise need one query per page.
    pub fn fetch_view_page(
        &self,
        row_map: Option<&ViewRowMap>,
        row_ids: &[i64],
        start: usize,
        count: usize,
    ) -> Result<Vec<CachedRow>, String> {
        let end = start.saturating_add(count).min(row_ids.len());
        let ids = row_ids.get(start..end).unwrap_or_default();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let Some(row_map) = row_map else {
            let mut rows = Vec::with_capacity(ids.len());
            for chunk in ids.chunks(crate::domain::frame::PAGE_SIZE) {
                rows.extend(self.fetch_row_ids(chunk)?);
            }
            return Ok(rows);
        };
        let predicate = format!(
            "rowid IN (SELECT source_row_id FROM {} \
             WHERE display_position >= ? AND display_position < ?)",
            quote_identifier(row_map.table_name())
        );
        let bounds = [start, end]
            .map(|position| i64::try_from(position).map(Value::BigInt))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "display position is out of range".to_owned())?;
        self.fetch_rows_where(ids, &predicate, &bounds)
    }

    /// Copies the rows `row_map` orders (or `row_ids`, when there is no map) to
    /// `path`, keeping only `columns` (indices into this store's schema) in that
    /// order. Values keep their types; nothing is rendered. Returns the rows written.
    pub fn export_view(
        &self,
        row_map: Option<&ViewRowMap>,
        row_ids: &[i64],
        columns: &[usize],
        path: &Path,
        format: crate::domain::export::FileFormat,
    ) -> Result<usize, String> {
        self.drop_retired_view_row_maps()?;
        let built;
        let row_map = match row_map {
            Some(row_map) => row_map,
            None => {
                built = self.materialize_view_rows_from_ids(row_ids)?;
                &*built.row_map
            }
        };
        let projection = columns
            .iter()
            .map(|index| {
                let column = self
                    .columns
                    .get(*index)
                    .ok_or("export column is out of range")?;
                Ok(format!(
                    "__qp_source.{} AS {}",
                    quote_identifier(&column.storage_name),
                    quote_identifier(&column.name)
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if projection.is_empty() {
            return Err("There are no columns to export".into());
        }
        if let Some(extension) = format.extension() {
            // DuckDB does not autoload a copy format's extension. Installing
            // fetches it once into DuckDB's own extension directory.
            let load = format!("LOAD {extension}");
            if self.connection.execute_batch(&load).is_err() {
                self.connection
                    .execute_batch(&format!("INSTALL {extension}; {load}"))
                    .map_err(|error| {
                        format!("Cannot load DuckDB's {extension} extension: {error}")
                    })?;
            }
        }
        let sql = format!(
            "COPY (\n    SELECT {}\n    FROM {} AS __qp_source\n    JOIN {} AS __qp_rows \
             ON __qp_source.rowid = __qp_rows.source_row_id\n    \
             ORDER BY __qp_rows.display_position\n) TO ? ({})",
            projection.join(", "),
            self.table_sql,
            quote_identifier(row_map.table_name()),
            format.copy_options()
        );
        self.connection
            .execute(&sql, [path.to_string_lossy().as_ref()])
            .map_err(|error| error.to_string())
    }

    pub fn fetch_row_ids(&self, ids: &[i64]) -> Result<Vec<CachedRow>, String> {
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let values = ids.iter().copied().map(Value::BigInt).collect::<Vec<_>>();
        self.fetch_rows_where(ids, &format!("rowid IN ({placeholders})"), &values)
    }

    /// Rows matching `predicate`, returned in the order of `ids`.
    fn fetch_rows_where(
        &self,
        ids: &[i64],
        predicate: &str,
        parameters: &[Value],
    ) -> Result<Vec<CachedRow>, String> {
        let _span = super::performance::Span::new("page_retrieval_conversion");
        self.drop_retired_view_row_maps()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let selected = self
            .columns
            .iter()
            .map(|column| {
                format!(
                    "CAST({} AS VARCHAR)",
                    quote_identifier(&column.storage_name)
                )
            })
            .collect::<Vec<_>>();
        let projection = if selected.is_empty() {
            "rowid".to_owned()
        } else {
            format!("rowid, {}", selected.join(", "))
        };
        let sql = format!(
            "SELECT {projection} FROM {} WHERE {predicate}",
            self.table_sql
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let fetched = statement
            .query_map(params_from_iter(parameters.iter()), |row| {
                let rowid = row.get::<_, i64>(0)?;
                let mut values = Vec::with_capacity(self.columns.len());
                for index in 0..self.columns.len() {
                    let value = row.get::<_, Option<String>>(index + 1)?;
                    values.push(value);
                }
                Ok(CachedRow { rowid, values })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut indexed = fetched
            .into_iter()
            .map(|row| (row.rowid, row))
            .collect::<HashMap<_, _>>();
        Ok(ids.iter().filter_map(|id| indexed.remove(id)).collect())
    }

    /// Bounded width previews, separate from result pages and the display cache.
    /// `row_ids` is supplied for query results; tables sample their unfiltered source.
    pub fn width_candidates(
        &self,
        row_ids: Option<&[i64]>,
        columns: &[(String, Option<crate::domain::formatting::NumericFormat>)],
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        use crate::domain::widths::{candidates, sample_positions};
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        #[cfg(test)]
        self.width_sampling_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _span = super::performance::Span::new("width_sampling");
        let ids = if let Some(rows) = row_ids {
            sample_positions(rows.len())
                .into_iter()
                .map(|position| rows[position])
                .collect::<Vec<_>>()
        } else {
            let positions = sample_positions(self.row_count);
            if positions.is_empty() {
                Vec::new()
            } else {
                let placeholders = std::iter::repeat_n("?", positions.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT row_id FROM (SELECT rowid AS row_id, row_number() OVER (ORDER BY rowid) - 1 AS position FROM {}) WHERE position IN ({placeholders}) ORDER BY position",
                    self.table_sql
                );
                let parameters = positions
                    .iter()
                    .map(|position| *position as i64)
                    .collect::<Vec<_>>();
                let mut statement = self
                    .connection
                    .prepare(&sql)
                    .map_err(|error| error.to_string())?;
                statement
                    .query_map(params_from_iter(parameters.iter()), |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| error.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?
            }
        };
        let mut output = Vec::new();
        for chunk in columns.chunks(16) {
            let schema = chunk
                .iter()
                .map(|(name, _)| {
                    self.columns
                        .iter()
                        .find(|column| column.name == *name)
                        .ok_or_else(|| format!("width column no longer exists: {name}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let projection = schema
                .iter()
                .map(|column| {
                    format!(
                        "left(CAST({} AS VARCHAR), 201)",
                        quote_identifier(&column.storage_name)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let mut values = vec![Vec::new(); chunk.len()];
            for batch in ids.chunks(250) {
                let placeholders = std::iter::repeat_n("?", batch.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!(
                    "SELECT {projection} FROM {} WHERE rowid IN ({placeholders})",
                    self.table_sql
                );
                let mut statement = self
                    .connection
                    .prepare(&sql)
                    .map_err(|error| error.to_string())?;
                let rows = statement
                    .query_map(params_from_iter(batch.iter()), |row| {
                        (0..chunk.len())
                            .map(|index| row.get::<_, Option<String>>(index))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .map_err(|error| error.to_string())?;
                for row in rows {
                    for (index, value) in row
                        .map_err(|error| error.to_string())?
                        .into_iter()
                        .enumerate()
                    {
                        values[index].push(value);
                    }
                }
            }
            for ((column, format), (schema, values)) in
                chunk.iter().zip(schema.into_iter().zip(values))
            {
                output.push((
                    column.clone(),
                    candidates(values, &schema.duckdb_type, *format),
                ));
            }
        }
        Ok(output)
    }

    #[cfg(test)]
    pub(crate) fn width_sampling_calls(&self) -> usize {
        self.width_sampling_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn selection_column_aggregates(
        &self,
        row_map: &ViewRowMap,
        start: usize,
        end: usize,
        column_indices: &[usize],
        data_types: &[String],
    ) -> Result<(Vec<ColumnAggregates>, usize), String> {
        self.drop_retired_view_row_maps()?;
        if column_indices.len() != data_types.len() {
            return Err("selection statistics columns do not match the result schema".into());
        }
        let start = i64::try_from(start).map_err(|_| "selection start is out of range")?;
        let end = i64::try_from(end).map_err(|_| "selection end is out of range")?;
        let columns = column_indices
            .iter()
            .map(|&index| {
                self.columns
                    .get(index)
                    .map(|column| format!("d.{}", quote_identifier(&column.storage_name)))
                    .ok_or_else(|| format!("statistics column index is invalid: {index}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // One column counts distinct non-null values; several count distinct row tuples,
        // so a selection of key columns shows whether those keys are unique.
        let unique = match columns.as_slice() {
            [column] => format!("COUNT(DISTINCT {column})"),
            _ => format!("COUNT(DISTINCT row({}))", columns.join(", ")),
        };
        let mut projections = vec![format!("CAST({unique} AS VARCHAR)")];
        let mut integer_columns = Vec::new();
        for (index, (column, data_type)) in columns.iter().zip(data_types).enumerate() {
            let nulls = format!("COUNT(*) - COUNT({column})");
            let characters = if string_type_name(data_type) {
                format!("COALESCE(SUM(LENGTH(CAST({column} AS VARCHAR))), 0)")
            } else {
                "0".into()
            };
            let numeric = numeric_type_name(data_type);
            let finite = format!("isfinite(CAST({column} AS DOUBLE))");
            let numeric_count = if numeric {
                format!("COUNT(*) FILTER (WHERE {column} IS NOT NULL AND {finite})")
            } else {
                "0".into()
            };
            let numeric_sum = if numeric {
                format!("COALESCE(SUM(CASE WHEN {finite} THEN CAST({column} AS DOUBLE) END), 0.0)")
            } else {
                "0.0".into()
            };
            projections.extend([
                format!("CAST({nulls} AS VARCHAR)"),
                format!("CAST({characters} AS VARCHAR)"),
                format!("CAST({numeric_count} AS VARCHAR)"),
                format!("CAST({numeric_sum} AS VARCHAR)"),
            ]);
            if numeric && is_integer_type(data_type) {
                integer_columns.push((index, column.clone()));
            }
            if numeric && column_indices.len() == 1 {
                let value = format!("CAST({column} AS DOUBLE)");
                projections.extend([
                    format!("CAST(AVG({value}) FILTER (WHERE {finite}) AS VARCHAR)"),
                    format!("CAST(MEDIAN({value}) FILTER (WHERE {finite}) AS VARCHAR)"),
                    format!("CAST(MIN({value}) FILTER (WHERE {finite}) AS VARCHAR)"),
                    format!("CAST(MAX({value}) FILTER (WHERE {finite}) AS VARCHAR)"),
                    format!("CAST(STDDEV_SAMP({value}) FILTER (WHERE {finite}) AS VARCHAR)"),
                ]);
            }
        }
        let map_table = quote_identifier(row_map.table_name());
        let sql = format!(
            "SELECT {} FROM {map_table} AS v JOIN {} AS d ON d.rowid = v.source_row_id WHERE v.display_position >= ? AND v.display_position < ?",
            projections.join(", "),
            self.table_sql
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let raw = statement
            .query_row(params![start, end], |row| {
                (0..projections.len())
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<duckdb::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;

        let integer_sums = self.query_integer_sums(&map_table, start, end, &integer_columns)?;
        let unique = parse_count(raw[0].as_deref())?;
        let mut result = Vec::with_capacity(column_indices.len());
        let mut offset = 1;
        for (index, data_type) in data_types.iter().enumerate() {
            let nulls = parse_count(raw[offset].as_deref())?;
            let characters = parse_count(raw[offset + 1].as_deref())?;
            let numeric_count = parse_count(raw[offset + 2].as_deref())?;
            let numeric_sum_as_f64 = parse_float(raw[offset + 3].as_deref())?.unwrap_or(0.0);
            offset += 4;
            let (integer_sum, integer_sum_overflow) = integer_sums
                .iter()
                .find(|(column_index, _, _)| *column_index == index)
                .map(|(_, sum, overflow)| (sum.clone(), *overflow))
                .unwrap_or((None, false));
            let single_numeric = if numeric_type_name(data_type) && column_indices.len() == 1 {
                let mean = parse_float(raw[offset].as_deref())?;
                let median = parse_float(raw[offset + 1].as_deref())?;
                let minimum = parse_float(raw[offset + 2].as_deref())?;
                let maximum = parse_float(raw[offset + 3].as_deref())?;
                let sample_std_dev = parse_float(raw[offset + 4].as_deref())?;
                offset += 5;
                mean.zip(median).zip(minimum).zip(maximum).map(
                    |(((mean, median), minimum), maximum)| NumericColumnStatistics {
                        mean,
                        median,
                        minimum,
                        maximum,
                        sample_std_dev,
                    },
                )
            } else {
                None
            };
            result.push(ColumnAggregates {
                nulls,
                characters,
                numeric_count,
                numeric_sum_as_f64,
                integer_sum,
                integer_sum_overflow,
                single_numeric,
            });
        }
        Ok((result, unique))
    }

    fn query_integer_sums(
        &self,
        map_table: &str,
        start: i64,
        end: i64,
        columns: &[(usize, String)],
    ) -> Result<Vec<(usize, Option<String>, bool)>, String> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        match self.run_integer_sum_query(map_table, start, end, columns) {
            Ok(results) => Ok(columns
                .iter()
                .zip(results)
                .map(|((index, _), (sum, overflow))| (*index, sum, overflow))
                .collect()),
            Err(error) if error.to_ascii_lowercase().contains("overflow") => columns
                .iter()
                .map(|(index, column)| {
                    let result =
                        self.run_integer_sum_query(map_table, start, end, &[(0, column.clone())]);
                    match result {
                        Ok(mut result) => {
                            let (sum, overflow) = result.pop().unwrap_or((None, true));
                            Ok((*index, sum, overflow))
                        }
                        Err(single_error)
                            if single_error.to_ascii_lowercase().contains("overflow") =>
                        {
                            Ok((*index, None, true))
                        }
                        Err(single_error) => Err(single_error),
                    }
                })
                .collect(),
            Err(error) => Err(error),
        }
    }

    fn run_integer_sum_query(
        &self,
        map_table: &str,
        start: i64,
        end: i64,
        columns: &[(usize, String)],
    ) -> Result<Vec<(Option<String>, bool)>, String> {
        let projections = columns
            .iter()
            .flat_map(|(_, column)| {
                let cast = format!("TRY_CAST({column} AS HUGEINT)");
                [
                    format!("CAST(SUM({cast}) AS VARCHAR)"),
                    format!(
                        "CAST(COUNT(*) FILTER (WHERE {column} IS NOT NULL AND {cast} IS NULL) AS VARCHAR)"
                    ),
                ]
            })
            .collect::<Vec<_>>();
        let sql = format!(
            "SELECT {} FROM {map_table} AS v JOIN {} AS d ON d.rowid = v.source_row_id WHERE v.display_position >= ? AND v.display_position < ?",
            projections.join(", "),
            self.table_sql
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let raw = statement
            .query_row(params![start, end], |row| {
                (0..projections.len())
                    .map(|index| row.get::<_, Option<String>>(index))
                    .collect::<duckdb::Result<Vec<_>>>()
            })
            .map_err(|error| error.to_string())?;
        (0..columns.len())
            .map(|index| {
                Ok((
                    raw[index * 2].clone(),
                    parse_count(raw[index * 2 + 1].as_deref())? > 0,
                ))
            })
            .collect()
    }
}

pub fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn is_integer_type(data_type: &str) -> bool {
    let data_type = data_type.to_ascii_uppercase();
    [
        "TINYINT",
        "SMALLINT",
        "INTEGER",
        "BIGINT",
        "HUGEINT",
        "UTINYINT",
        "USMALLINT",
        "UINTEGER",
        "UBIGINT",
        "UHUGEINT",
    ]
    .iter()
    .any(|prefix| data_type.starts_with(prefix))
}

fn parse_count(value: Option<&str>) -> Result<usize, String> {
    value
        .ok_or_else(|| "DuckDB returned a null selection count".to_owned())?
        .parse::<u64>()
        .map_err(|error| error.to_string())
        .and_then(|count| {
            usize::try_from(count).map_err(|_| "selection count is out of range".to_owned())
        })
}

fn parse_float(value: Option<&str>) -> Result<Option<f64>, String> {
    value
        .map(|value| match value.to_ascii_lowercase().as_str() {
            "inf" | "+inf" | "infinity" | "+infinity" => Ok(f64::INFINITY),
            "-inf" | "-infinity" => Ok(f64::NEG_INFINITY),
            "nan" | "+nan" | "-nan" => Ok(f64::NAN),
            _ => value.parse::<f64>().map_err(|error| error.to_string()),
        })
        .transpose()
}

fn scalar_parameter(value: &ScalarKey) -> Result<(Value, &'static str), String> {
    Ok(match value {
        ScalarKey::Null => return Err("null cannot be bound as a scalar filter".into()),
        ScalarKey::Boolean(value) => (Value::Boolean(*value), "BOOLEAN"),
        ScalarKey::String(value) => (Value::Text(value.clone()), "VARCHAR"),
        ScalarKey::Decimal(_) => {
            return Err("decimal filters require a precision-preserving cast".into());
        }
        ScalarKey::Signed(value) => (
            Value::HugeInt(value.parse().map_err(|_| "invalid signed filter value")?),
            "HUGEINT",
        ),
        ScalarKey::Unsigned(value) => (
            Value::UHugeInt(value.parse().map_err(|_| "invalid unsigned filter value")?),
            "UHUGEINT",
        ),
        ScalarKey::FloatBits(bits) => {
            let value = if let Some(bits) = bits.strip_prefix("f32:") {
                f32::from_bits(bits.parse().map_err(|_| "invalid float filter value")?) as f64
            } else if let Some(bits) = bits.strip_prefix("f64:") {
                f64::from_bits(bits.parse().map_err(|_| "invalid float filter value")?)
            } else {
                return Err("invalid float filter value".into());
            };
            (Value::Double(value), "DOUBLE")
        }
        ScalarKey::Date(days) => {
            let days = days
                .parse::<i64>()
                .map_err(|_| "invalid date filter value")?;
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .and_then(|epoch| epoch.checked_add_signed(chrono::Duration::days(days)))
                .ok_or("date filter value is out of range")?;
            (Value::Text(date.format("%Y-%m-%d").to_string()), "DATE")
        }
        ScalarKey::Datetime {
            value,
            unit,
            timezone,
        } => {
            let value = value
                .parse::<i64>()
                .map_err(|_| "invalid datetime filter value")?;
            if unit == "Nanoseconds" && timezone.is_none() {
                return Ok((
                    Value::Text(
                        chrono::DateTime::from_timestamp_nanos(value)
                            .naive_utc()
                            .format("%Y-%m-%d %H:%M:%S%.9f")
                            .to_string(),
                    ),
                    "TIMESTAMP_NS",
                ));
            }
            let micros = match unit.as_str() {
                "Seconds" => value.saturating_mul(1_000_000),
                "Nanoseconds" => value / 1_000,
                "Milliseconds" => value.saturating_mul(1_000),
                _ => value,
            };
            let datetime = chrono::DateTime::from_timestamp_micros(micros)
                .ok_or("datetime filter value is out of range")?;
            (
                Value::Text(
                    datetime
                        .naive_utc()
                        .format(if timezone.is_some() {
                            "%Y-%m-%d %H:%M:%S%.6f+00:00"
                        } else {
                            "%Y-%m-%d %H:%M:%S%.6f"
                        })
                        .to_string(),
                ),
                if timezone.is_some() {
                    "TIMESTAMPTZ"
                } else {
                    "TIMESTAMP"
                },
            )
        }
        ScalarKey::Time(nanos) => {
            let nanos = nanos
                .parse::<i64>()
                .map_err(|_| "invalid time filter value")?;
            let seconds = nanos.div_euclid(1_000_000_000);
            let hour = seconds / 3_600;
            let minute = seconds / 60 % 60;
            let second = seconds % 60;
            (
                Value::Text(format!(
                    "{hour:02}:{minute:02}:{second:02}.{:09}",
                    nanos.rem_euclid(1_000_000_000)
                )),
                "TIME",
            )
        }
        ScalarKey::Duration { value, unit } => {
            if unit == "DuckDBInterval" {
                return Ok((Value::Text(value.clone()), "INTERVAL"));
            }
            let value = value
                .parse::<i64>()
                .map_err(|_| "invalid duration filter value")?;
            let micros = match unit.as_str() {
                "Nanoseconds" => value / 1_000,
                "Milliseconds" => value.saturating_mul(1_000),
                _ => value,
            };
            (Value::Text(format!("{micros} microseconds")), "INTERVAL")
        }
    })
}

fn is_numeric_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_uppercase();
    [
        "TINYINT",
        "SMALLINT",
        "INTEGER",
        "BIGINT",
        "HUGEINT",
        "UTINYINT",
        "USMALLINT",
        "UINTEGER",
        "UBIGINT",
        "UHUGEINT",
        "FLOAT",
        "DOUBLE",
        "DECIMAL",
    ]
    .iter()
    .any(|name| kind.starts_with(name))
}

fn is_searchable_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_uppercase();
    is_numeric_type(&kind)
        || ["VARCHAR", "CHAR", "BOOLEAN", "UUID", "ENUM"]
            .iter()
            .any(|name| kind.starts_with(name))
}

/// Lists, arrays, structs, maps, and unions.
pub(crate) fn is_nested_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_uppercase();
    kind.ends_with(']')
        || ["STRUCT", "MAP", "UNION"]
            .iter()
            .any(|name| kind.starts_with(name))
}

enum SearchRendering {
    /// Free text that must be lowercased before matching.
    Text,
    /// DuckDB renders these scalars in lowercase using only the accepted characters.
    Lowercase(fn(char) -> bool),
}

fn search_rendering(data_type: &str) -> Option<SearchRendering> {
    if !is_searchable_type(data_type) {
        return None;
    }
    let kind = data_type.to_ascii_uppercase();
    // Lists and arrays such as INTEGER[] render brackets and separators, so only
    // bare scalar type names qualify.
    let scalar = kind.chars().all(|ch| ch.is_ascii_alphabetic());
    let renders: fn(char) -> bool = if scalar && is_integer_type(&kind) {
        |ch| matches!(ch, '0'..='9' | '-')
    } else if kind.starts_with("DECIMAL(") && kind.ends_with(')') {
        |ch| matches!(ch, '0'..='9' | '-' | '.')
    } else if kind == "FLOAT" || kind == "DOUBLE" {
        // Includes exponents (1e+20) and the specials inf, -inf, and nan.
        |ch| {
            matches!(
                ch,
                '0'..='9' | '-' | '+' | '.' | 'e' | 'i' | 'n' | 'f' | 'a'
            )
        }
    } else if kind == "BOOLEAN" {
        |ch| "truefals".contains(ch)
    } else if kind == "UUID" {
        |ch| matches!(ch, '0'..='9' | 'a'..='f' | '-')
    } else {
        return Some(SearchRendering::Text);
    };
    Some(SearchRendering::Lowercase(renders))
}

/// Predicate for a search term scoped to `column`. Text matching is
/// case-insensitive on the value as DuckDB renders it; numeric columns compare
/// numerically, and other types compare against the value cast to the column type.
fn column_predicate(
    column: &ColumnSchema,
    term: &ColumnTerm,
    parameters: &mut Vec<Value>,
) -> Result<String, String> {
    let identifier = quote_identifier(&column.storage_name);
    let rendered = format!("lower(CAST({identifier} AS VARCHAR))");
    let predicate = match term.operator {
        ColumnOperator::Contains => {
            parameters.push(Value::Text(term.value.to_lowercase()));
            format!("contains({rendered}, ?)")
        }
        ColumnOperator::Compare(comparison) if is_numeric_type(&column.duckdb_type) => {
            let number = parse_number(&term.value)?;
            numeric_predicate(column, &number, comparison, parameters)
        }
        ColumnOperator::Compare(comparison) if string_type_name(&column.duckdb_type) => {
            text_predicate(
                &rendered,
                comparison,
                &term.value.to_lowercase(),
                parameters,
            )
        }
        ColumnOperator::Compare(comparison) if is_date_type(&column.duckdb_type) => {
            match crate::domain::query::date_period(&term.value) {
                Some(period) => date_predicate(&identifier, comparison, period, parameters),
                // Timestamps also accept a full date and time, cast by DuckDB.
                None if column
                    .duckdb_type
                    .to_ascii_uppercase()
                    .starts_with("TIMESTAMP") =>
                {
                    parameters.push(Value::Text(term.value.clone()));
                    format!(
                        "{identifier} {} TRY_CAST(? AS {})",
                        comparison.sql(),
                        column.duckdb_type
                    )
                }
                None => {
                    return Err(format!(
                        "{} is not a date; use YYYY, YYYY-MM, or YYYY-MM-DD",
                        term.value
                    ));
                }
            }
        }
        ColumnOperator::Compare(Comparison::Equal) => {
            parameters.push(Value::Text(term.value.to_lowercase()));
            format!("{rendered} = ?")
        }
        ColumnOperator::Compare(comparison) => {
            parameters.push(Value::Text(term.value.clone()));
            format!(
                "{identifier} {} TRY_CAST(? AS {})",
                comparison.sql(),
                column.duckdb_type
            )
        }
    };
    // NULL comparisons count as no match, so negated terms keep NULL rows.
    Ok(format!("coalesce({predicate}, FALSE)"))
}

/// `left <op> right` for two columns. Text compares case-insensitively like
/// other search terms; otherwise the right side is cast to the left's type, and
/// values that cannot be cast never match.
fn column_comparison(left: &ColumnSchema, right: &ColumnSchema, comparison: Comparison) -> String {
    let identifier = |column: &ColumnSchema| quote_identifier(&column.storage_name);
    let (left_sql, right_sql) =
        if string_type_name(&left.duckdb_type) && string_type_name(&right.duckdb_type) {
            (
                format!("lower({})", identifier(left)),
                format!("lower({})", identifier(right)),
            )
        } else if left.duckdb_type == right.duckdb_type
            || (is_numeric_type(&left.duckdb_type) && is_numeric_type(&right.duckdb_type))
        {
            (identifier(left), identifier(right))
        } else {
            (
                identifier(left),
                format!("TRY_CAST({} AS {})", identifier(right), left.duckdb_type),
            )
        };
    format!(
        "coalesce({left_sql} {} {right_sql}, FALSE)",
        comparison.sql()
    )
}

fn is_date_type(data_type: &str) -> bool {
    let kind = data_type.to_ascii_uppercase();
    kind.starts_with("DATE") || kind.starts_with("TIMESTAMP")
}

/// Case-insensitive text comparison where a value stands for every text that
/// starts with it: `<=b` keeps `b…`, and `>b` skips it, so `china..italy`
/// includes `Italy (south)`.
fn text_predicate(
    rendered: &str,
    comparison: Comparison,
    value: &str,
    parameters: &mut Vec<Value>,
) -> String {
    let operator = comparison.sql();
    match comparison {
        Comparison::LessOrEqual => {
            parameters.extend([Value::Text(value.into()), Value::Text(value.into())]);
            format!("({rendered} <= ? OR starts_with({rendered}, ?))")
        }
        Comparison::Greater => {
            parameters.extend([Value::Text(value.into()), Value::Text(value.into())]);
            format!("({rendered} > ? AND NOT starts_with({rendered}, ?))")
        }
        Comparison::Equal | Comparison::GreaterOrEqual | Comparison::Less => {
            parameters.push(Value::Text(value.into()));
            format!("{rendered} {operator} ?")
        }
    }
}

/// Date comparison against the days a partial date covers, so `=2022-10` is the
/// month and `<=2024-02-01` keeps every timestamp on that day.
fn date_predicate(
    identifier: &str,
    comparison: Comparison,
    (start, end): (chrono::NaiveDate, chrono::NaiveDate),
    parameters: &mut Vec<Value>,
) -> String {
    let day = |date: chrono::NaiveDate| Value::Text(date.format("%Y-%m-%d").to_string());
    let (bounds, sql) = match comparison {
        Comparison::Equal => (
            vec![day(start), day(end)],
            format!("({identifier} >= CAST(? AS DATE) AND {identifier} < CAST(? AS DATE))"),
        ),
        Comparison::GreaterOrEqual => {
            (vec![day(start)], format!("{identifier} >= CAST(? AS DATE)"))
        }
        Comparison::Greater => (vec![day(end)], format!("{identifier} >= CAST(? AS DATE)")),
        Comparison::Less => (vec![day(start)], format!("{identifier} < CAST(? AS DATE)")),
        Comparison::LessOrEqual => (vec![day(end)], format!("{identifier} < CAST(? AS DATE)")),
    };
    parameters.extend(bounds);
    sql
}

fn numeric_predicate(
    column: &ColumnSchema,
    number: &Numeric,
    comparison: Comparison,
    parameters: &mut Vec<Value>,
) -> String {
    let identifier = quote_identifier(&column.storage_name);
    let beyond_range = |literal_above: bool| {
        if comparison.holds_beyond_range(literal_above) {
            format!("{identifier} IS NOT NULL")
        } else {
            "FALSE".to_owned()
        }
    };
    let comparison = comparison.sql();
    let kind = column.duckdb_type.to_ascii_uppercase();
    let signed_integer = ["TINYINT", "SMALLINT", "INTEGER", "BIGINT", "HUGEINT"]
        .iter()
        .any(|name| kind.starts_with(name));
    let unsigned_integer = ["UTINYINT", "USMALLINT", "UINTEGER", "UBIGINT", "UHUGEINT"]
        .iter()
        .any(|name| kind.starts_with(name));

    if signed_integer {
        match number {
            Numeric::Signed(value) => {
                parameters.push(Value::HugeInt(*value));
                format!("{identifier} {comparison} CAST(? AS HUGEINT)")
            }
            Numeric::Unsigned(value) if *value <= i128::MAX as u128 => {
                parameters.push(Value::HugeInt(*value as i128));
                format!("{identifier} {comparison} CAST(? AS HUGEINT)")
            }
            Numeric::Unsigned(_) => beyond_range(true),
            Numeric::Float(value) => {
                parameters.push(Value::Double(*value));
                format!("CAST({identifier} AS DOUBLE) {comparison} ?")
            }
        }
    } else if unsigned_integer {
        match number {
            Numeric::Signed(value) if *value < 0 => beyond_range(false),
            Numeric::Signed(value) => {
                parameters.push(Value::UHugeInt(*value as u128));
                format!("{identifier} {comparison} CAST(? AS UHUGEINT)")
            }
            Numeric::Unsigned(value) => {
                parameters.push(Value::UHugeInt(*value));
                format!("{identifier} {comparison} CAST(? AS UHUGEINT)")
            }
            Numeric::Float(value) => {
                parameters.push(Value::Double(*value));
                format!("CAST({identifier} AS DOUBLE) {comparison} ?")
            }
        }
    } else {
        let value = match number {
            Numeric::Signed(value) => *value as f64,
            Numeric::Unsigned(value) => *value as f64,
            Numeric::Float(value) => *value,
        };
        parameters.push(Value::Double(value));
        format!("CAST({identifier} AS DOUBLE) {comparison} ?")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_previews_project_quote_and_sample_real_row_ids_without_pages() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE qp_data AS SELECT i AS \"a\"\"b\", repeat('界', 500) AS long, 'ignored' AS other FROM range(5) t(i); DELETE FROM qp_data WHERE \"a\"\"b\" IN (1, 3)").unwrap();
        let store = DuckDbStore::from_connection(connection).unwrap();
        let columns = vec![("a\"b".into(), None), ("long".into(), None)];
        let sampled = store.width_candidates(None, &columns).unwrap();
        assert_eq!(sampled.len(), 2);
        assert_eq!(sampled[0].1, ["0", "2", "4"]);
        assert_eq!(sampled[1].1.len(), 1);
        assert_eq!(sampled[1].1[0].chars().count(), 201);
        assert!(sampled[1].1[0].ends_with('…'));
        let subset = store.width_candidates(Some(&[4]), &columns[..1]).unwrap();
        assert_eq!(subset[0].1, ["4"]);
        assert_eq!(store.width_sampling_calls(), 2);
        assert!(store.width_candidates(None, &[]).unwrap().is_empty());
        assert_eq!(store.width_sampling_calls(), 2);
    }

    fn store(sql: &str) -> DuckDbStore {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(sql).unwrap();
        DuckDbStore::from_connection(connection).unwrap()
    }

    #[test]
    fn column_summaries_count_nulls_and_distinct_values_and_skip_nested_ranges() {
        let source = store(
            "CREATE TABLE qp_data AS SELECT * FROM (VALUES              ('North', 10, [1]), ('South', NULL, [2]), ('North', 30, NULL), (NULL, 5, [1]))              t(region, n, list)",
        );
        let summaries = source
            .column_summaries(&["region".into(), "n".into(), "list".into()])
            .unwrap();
        assert_eq!(
            summaries[0],
            ColumnSummary {
                name: "region".into(),
                rows: 4,
                nulls: 1,
                unique: 2,
                min: Some("North".into()),
                max: Some("South".into()),
            }
        );
        // Numeric min and max compare as numbers, not text.
        assert_eq!(summaries[1].min.as_deref(), Some("5"));
        assert_eq!(summaries[1].max.as_deref(), Some("30"));
        assert_eq!((summaries[2].nulls, summaries[2].unique), (1, 2));
        assert_eq!(summaries[2].min, None);
        assert!(source.column_summaries(&["missing".into()]).is_err());
    }

    #[test]
    fn top_values_rank_by_count_then_value_and_keep_nulls() {
        let source = store(
            "CREATE TABLE qp_data AS SELECT * FROM (VALUES              ('b'), ('a'), ('b'), (NULL), (NULL), ('c'), ('a'), ('b')) t(v)",
        );
        assert_eq!(
            source.top_values("v", 3).unwrap(),
            [(Some("b".into()), 3), (Some("a".into()), 2), (None, 2),]
        );
        // Numeric ties order as numbers, not text.
        let numbers = store("CREATE TABLE qp_data AS SELECT * FROM (VALUES (90), (610)) t(n)");
        assert_eq!(
            numbers.top_values("n", 5).unwrap(),
            [(Some("90".into()), 1), (Some("610".into()), 1)]
        );
    }

    #[test]
    fn dimension_metadata_cleans_up_after_success_and_failure() {
        let source = store("CREATE TEMP TABLE qp_data AS SELECT 'O''Brien' AS name");
        for sql in [
            "SELECT * FROM (PIVOT (SELECT * FROM qp_data WHERE name = ?) ON name IN __qp_test_enum USING count(*))",
            "SELECT missing FROM qp_data",
        ] {
            let mut trace = SqlTrace::default();
            let enums = [QueryEnum {
                name: "__qp_test_enum".into(),
                sql: "SELECT DISTINCT name FROM qp_data WHERE name = ?".into(),
                values: vec![Value::Text("O'Brien".into())],
            }];
            let values = if sql.contains("missing") {
                vec![]
            } else {
                vec![Value::Text("O'Brien".into())]
            };
            let result = source.materialize_query_traced(
                "Query materialization",
                sql,
                &values,
                Some(QueryInput { enums: &enums }),
                &mut trace,
            );
            assert_eq!(result.is_ok(), !sql.contains("missing"), "{}", trace.text());
            assert_eq!(trace.statements.len(), 3);
            assert!(
                matches!(&trace.statements[0].parameters[0], Value::Text(value) if value == "O'Brien")
            );
            let remaining: i64 = source
                .connection
                .query_row(
                    "SELECT count(*) FROM duckdb_types() WHERE type_name = '__qp_test_enum'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(remaining, 0);
        }
        let enums = [QueryEnum {
            name: "__qp_test_enum".into(),
            sql: "SELECT missing FROM qp_data".into(),
            values: vec![],
        }];
        let mut trace = SqlTrace::default();
        assert!(
            source
                .materialize_query_traced(
                    "Query materialization",
                    "SELECT 1",
                    &[],
                    Some(QueryInput { enums: &enums }),
                    &mut trace
                )
                .is_err()
        );
        assert_eq!(trace.statements.len(), 1);
        assert!(matches!(
            trace.statements[0].status,
            crate::domain::sql::SqlStatus::Failed(_)
        ));
    }

    #[test]
    #[ignore = "manual deterministic performance comparison"]
    fn filtered_pivot_performance_comparison() {
        use crate::domain::documents::{Aggregation, AggregationOp, QueryConfig, SlicerDefinition};
        use crate::domain::pivot::{EvaluationClock, execute_traced_limited};
        use std::time::Instant;
        for count in [10_000, 100_000, 1_000_000] {
            let extra = (0..20)
                .map(|index| format!(", repeat('payload_{index}', 4) AS extra_{index}"))
                .collect::<String>();
            let source = store(&format!(
                "CREATE TEMP TABLE qp_data AS SELECT (range / 1000)::BIGINT AS Year, CASE WHEN range % 2 = 0 THEN 'F' ELSE 'M' END AS Sex, range::VARCHAR AS Name, CASE WHEN range % 3 = 0 THEN 'Winter' ELSE 'Summer' END AS Season{extra} FROM range({count})"
            ));
            let config = QueryConfig {
                group_by: vec!["Year".into()],
                pivot_by: vec!["Sex".into()],
                aggregations: vec![Aggregation {
                    sql: None,
                    source_column: "Name".into(),
                    operation: AggregationOp::NonNullCount,
                    name: None,
                    conditions: vec![],
                    percentile: None,
                    interpolation: None,
                    ddof: None,
                }],
                result_sort: vec![],
                totals: false,
                heatmap: false,
            };
            let slicers = [SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("Winter".into())],
            }];
            let mut direct_times = Vec::new();
            let mut staged_times = Vec::new();
            for iteration in 0..4 {
                let start = Instant::now();
                let mut trace = SqlTrace::default();
                let direct = execute_traced_limited(
                    &source,
                    &config,
                    &slicers,
                    &EvaluationClock::system(),
                    100000,
                    &mut trace,
                )
                .unwrap();
                let direct_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                source.connection.execute("CREATE TEMP TABLE __qp_benchmark_input AS SELECT *, rowid AS __qp_pivot_rowid, Name AS __qp_pivot_measure_0 FROM qp_data WHERE CAST(Season AS VARCHAR) IS NOT DISTINCT FROM CAST(? AS VARCHAR)", params!["Winter"]).unwrap();
                let staged = source.materialize_query("SELECT * FROM (PIVOT __qp_benchmark_input ON Sex USING count(__qp_pivot_measure_0) AS \"Name (Count)\" GROUP BY Year) ORDER BY Year ASC NULLS LAST LIMIT 100000", &[]).unwrap();
                source
                    .connection
                    .execute_batch("DROP TABLE __qp_benchmark_input")
                    .unwrap();
                let staged_ms = start.elapsed().as_secs_f64() * 1000.;
                assert_eq!(direct.schema(), staged.schema());
                assert_eq!(
                    direct
                        .fetch_row_ids(&direct.initial_row_ids().unwrap())
                        .unwrap(),
                    staged
                        .fetch_row_ids(&staged.initial_row_ids().unwrap())
                        .unwrap()
                );
                if iteration > 0 {
                    direct_times.push(direct_ms);
                    staged_times.push(staged_ms);
                }
            }
            direct_times.sort_by(f64::total_cmp);
            staged_times.sort_by(f64::total_cmp);
            println!(
                "rows={count} columns=24 direct_median_ms={:.3} staged_median_ms={:.3}",
                direct_times[1], staged_times[1]
            );
        }
    }

    #[test]
    fn discovers_schema_initial_row_order_search_sort_and_page_mapping() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (3, 'Smith'), (1, 'smith'), (2, 'Jones')) t(employee_id, name)",
        );
        assert_eq!(
            store
                .schema()
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["employee_id", "name"]
        );
        assert_eq!(store.initial_row_ids().unwrap(), [0, 1, 2]);
        let searched = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Contains("smith".into())],
                filters: vec![],
                sort: vec![],
            })
            .unwrap();
        assert_eq!(searched, [0, 1]);
        let sorted = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                filters: vec![],
                sort: vec![SortKey {
                    column: "employee_id".into(),
                    descending: false,
                }],
            })
            .unwrap();
        assert_eq!(sorted, [1, 2, 0]);
        let page = store.fetch_page(&sorted, 0, 2).unwrap();
        assert_eq!(page.iter().map(|row| row.rowid).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(page[0].values[0].as_deref(), Some("1"));
    }

    #[test]
    fn search_is_case_insensitive_literal_and_handles_empty_and_null_results() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('C++'), (NULL), ('C#')) t(text)",
        );
        assert_eq!(
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![Token::Contains("c++".into())],
                    filters: vec![],
                    sort: vec![]
                })
                .unwrap(),
            [0]
        );
        assert_eq!(
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![Token::Contains("missing".into())],
                    filters: vec![],
                    sort: vec![]
                })
                .unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![],
                    filters: vec![],
                    sort: vec![]
                })
                .unwrap(),
            [0, 1, 2]
        );
    }

    #[test]
    fn row_map_pages_match_row_id_pages_for_scattered_sorted_views() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT range AS n, (range * 7919) % 1000 AS scattered, \
             'row ' || range::VARCHAR AS label FROM range(1000)",
        );
        let rows = store
            .query_view_rows(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Contains("row 1".into())],
                filters: vec![],
                sort: vec![SortKey {
                    column: "scattered".into(),
                    descending: true,
                }],
            })
            .unwrap();
        assert_eq!(rows.row_ids.len(), 111);
        for (start, count) in [(0, 250), (5, 40), (100, 250), (110, 1), (111, 5), (500, 3)] {
            assert_eq!(
                store
                    .fetch_view_page(Some(&rows.row_map), &rows.row_ids, start, count)
                    .unwrap(),
                store.fetch_page(&rows.row_ids, start, count).unwrap(),
                "range {start}+{count}"
            );
        }
    }

    #[test]
    fn search_skips_only_columns_whose_rendering_cannot_contain_the_text() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES \
             (1::INTEGER, -12.345::DECIMAL(10,3), 1e20::DOUBLE, true, '663c8ea9-6892-4828-ae44-9794c657755d'::UUID, [1, 2]::INTEGER[], 'Label A', 'inf'::FLOAT, [1.5, 2.25]::DECIMAL(9,2)[], 18446744073709551615::UBIGINT), \
             (-7, 0.5, 'nan'::DOUBLE, false, NULL, [3], 'x', 2.5, NULL, 0), \
             (NULL, NULL, '-inf'::DOUBLE, NULL, NULL, NULL, NULL, NULL, NULL, NULL)\
             ) t(i, d, f, b, u, l, s, fl, dl, ub)",
        );
        let unpruned = store
            .columns
            .iter()
            .filter(|column| is_searchable_type(&column.duckdb_type))
            .map(|column| {
                format!(
                    "coalesce(contains(lower(CAST({} AS VARCHAR)), lower($1)), FALSE)",
                    quote_identifier(&column.storage_name)
                )
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        for needle in [
            "label", "1e+20", "inf", "-inf", "nan", "-12.3", "true", "als", "[1, 2", "2.25]", "-",
            ".", "e", "a", "1", "8ea9-", "3e", "+", ", ", "x", "615", "ü",
        ] {
            let expected = store
                .connection
                .prepare(&format!(
                    "SELECT rowid FROM qp_data WHERE {unpruned} ORDER BY rowid"
                ))
                .unwrap()
                .query_map([needle], |row| row.get::<_, i64>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let actual = store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![Token::Contains(needle.into())],
                    filters: vec![],
                    sort: vec![],
                })
                .unwrap();
            assert_eq!(actual, expected, "search for {needle:?}");
        }
    }

    #[test]
    fn duplicate_sort_values_use_rowid_and_unusual_identifiers_are_quoted() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (2, 'a'), (1, 'b'), (1, 'c')) t(\"odd name\", \"quote\"\"name\")",
        );
        let ids = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                filters: vec![],
                sort: vec![SortKey {
                    column: "odd name".into(),
                    descending: false,
                }],
            })
            .unwrap();
        assert_eq!(ids, [1, 2, 0]);
    }

    #[test]
    fn source_rowid_column_is_kept_distinct_from_duckdb_rowid() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (99, 'a'), (88, 'b')) t(rowid, name)",
        );
        assert_eq!(store.initial_row_ids().unwrap(), [0, 1]);
        assert_eq!(store.schema()[0].name, "rowid");
        assert_eq!(
            store.fetch_page(&[1], 0, 1).unwrap()[0].values[0].as_deref(),
            Some("88")
        );
    }

    #[test]
    fn column_scoped_search_matches_one_column_by_type() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES \
             ('North', 'Southfield', 5, DATE '2024-01-10', 1.5::DECIMAL(4, 2)), \
             ('South', 'Northgate', 15, DATE '2024-03-01', 10::DECIMAL(4, 2)), \
             (NULL, 'Eastview', NULL, NULL, NULL), \
             ('West', 'O''Brien', 25, DATE '2023-12-31', 2.25::DECIMAL(4, 2))) \
             t(\"Sales Region\", city, qty, \"order date\", price)",
        );
        let search = |text: &str| {
            store.query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                tokens: crate::domain::query::parse(text).unwrap(),
                ..ViewQuery::default()
            })
        };
        // Scoped contains ignores other columns; prefixes and quoted names resolve.
        assert_eq!(search("\"sales region\":north").unwrap(), [0]);
        assert_eq!(search("sales:NORTH").unwrap(), [0]);
        assert_eq!(search("city:north").unwrap(), [1]);
        // Equality is case-insensitive text on text columns and numeric on numbers.
        assert_eq!(search("city=o'brien").unwrap(), [3]);
        assert_eq!(search("price=10").unwrap(), [1]);
        assert_eq!(search("qty>=15").unwrap(), [1, 3]);
        assert_eq!(search("qty<15 qty>0").unwrap(), [0]);
        // Dates compare against the value cast to the column type.
        assert_eq!(search("order>=2024-01-10").unwrap(), [0, 1]);
        assert_eq!(search("order<2024-01-01").unwrap(), [3]);
        // Negation keeps rows whose value is NULL.
        assert_eq!(search("-sales:north").unwrap(), [1, 2, 3]);
        assert_eq!(search("qty!=15").unwrap(), [0, 2, 3]);
        // Unknown columns search the whole word as text.
        assert_eq!(search("o'brien:x").unwrap(), Vec::<i64>::new());
        assert_eq!(search("nope:north").unwrap(), Vec::<i64>::new());
        assert!(search("qty>abc").is_err());
    }

    #[test]
    fn double_quoted_column_names_compare_two_columns() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES              (5, 10, 'apple', 'APPLE', '7'),              (20, 10, 'pear', 'fig', 'x'),              (NULL, 3, 'kiwi', 'kiwi', '20'))              t(\"small bags\", \"large bags\", fruit, other, code)",
        );
        let search = |text: &str| {
            store.query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                tokens: crate::domain::query::parse(text).unwrap(),
                ..ViewQuery::default()
            })
        };
        assert_eq!(search("\"small bags\">\"large bags\"").unwrap(), [1]);
        assert_eq!(search("\"small bags\"<=\"large bags\"").unwrap(), [0]);
        // Negation keeps rows where either side is NULL, as for literal values.
        assert_eq!(search("\"small bags\"!=\"large bags\"").unwrap(), [0, 1, 2]);
        // Text compares case-insensitively; mixed types cast the right side.
        assert_eq!(search("fruit=\"other\"").unwrap(), [0, 2]);
        assert_eq!(search("\"small bags\"<\"code\"").unwrap(), [0]);
        // A name that is not a column stays a literal.
        assert_eq!(search("fruit=\"pear\"").unwrap(), [1]);
        let condition = |column: &str, text: &str| {
            store.query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                filters: vec![ColumnFilter {
                    sql: false,
                    column: column.into(),
                    values: Vec::new(),
                    condition: Some(text.into()),
                }],
                ..ViewQuery::default()
            })
        };
        assert_eq!(condition("large bags", ">=\"small bags\"").unwrap(), [0]);
    }

    #[test]
    fn condition_and_sql_filters_cover_ranges_text_dates_and_report_errors() {
        let store = store(concat!(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ",
            "('North', 5, DATE '2024-01-10'), ",
            "('South', 15, DATE '2024-03-01'), ",
            "(NULL, NULL, NULL), ",
            "('Northwest', 25, current_date)) t(region, qty, day)",
        ));
        let rows = |column: &str, text: &str, sql: bool| {
            store.query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                filters: vec![ColumnFilter {
                    sql,
                    column: column.into(),
                    values: vec![ScalarKey::String("ignored".into())],
                    condition: Some(text.into()),
                }],
                ..ViewQuery::default()
            })
        };
        let condition = |column: &str, text: &str| rows(column, text, false);
        assert_eq!(condition("qty", "10..25").unwrap(), [1, 3]);
        assert_eq!(condition("qty", ">5 <25").unwrap(), [1]);
        assert_eq!(condition("qty", "!=15").unwrap(), [0, 2, 3]);
        assert_eq!(condition("region", "north -west").unwrap(), [0]);
        assert_eq!(condition("region", "=SOUTH").unwrap(), [1]);
        assert_eq!(condition("day", "2024-01-01..2024-02-01").unwrap(), [0]);
        assert_eq!(condition("day", "-7d..today").unwrap(), [3]);
        // Partial dates cover their whole month or year.
        assert_eq!(condition("day", "2024-01..2024-02").unwrap(), [0]);
        assert_eq!(condition("day", "=2024-03").unwrap(), [1]);
        assert_eq!(condition("day", ">2024-01 <=2024").unwrap(), [1]);
        assert!(condition("day", ">=north").is_err());
        // Text ranges ignore case and keep values starting with the upper bound.
        assert_eq!(condition("region", "north..NORTH").unwrap(), [0, 3]);
        assert_eq!(condition("region", "a..n").unwrap(), [0, 3]);
        assert_eq!(condition("region", ">north").unwrap(), [1]);
        assert_eq!(condition("region", "<north").unwrap(), Vec::<i64>::new());
        // Blank text does not filter, even though `values` is set.
        assert_eq!(condition("qty", "  ").unwrap(), [0, 1, 2, 3]);
        assert!(condition("qty", ">=abc").is_err());
        // An SQL filter sees every column; its own column is unused.
        assert_eq!(
            rows("", "qty % 10 = 5 AND region <> 'North'", true).unwrap(),
            [1, 3]
        );
        assert_eq!(rows("", " ", true).unwrap(), [0, 1, 2, 3]);

        assert!(store.validate_condition("qty", ">=10").is_ok());
        assert!(store.validate_condition("qty", "").is_ok());
        assert!(store.validate_condition("qty", ">=abc").is_err());
        assert!(store.validate_condition("missing", ">1").is_err());
        assert!(store.validate_sql("qty > 1 OR region = 'x'").is_ok());
        assert!(store.validate_sql("").is_ok());
        let error = store.validate_sql("missing_column > 1").unwrap_err();
        assert!(error.contains("missing_column"), "{error}");
        assert!(!error.contains('\n'));

        assert_eq!(
            store.column_bounds("qty").unwrap(),
            Some(("5".to_owned(), "25".to_owned()))
        );
        let empty = super::tests::store("CREATE TEMP TABLE qp_data AS SELECT NULL::INTEGER AS n");
        assert_eq!(empty.column_bounds("n").unwrap(), None);
    }

    #[test]
    fn any_numeric_comparisons_include_bounds_and_out_of_range_literals() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (1::UTINYINT, -5::TINYINT), (2, 7)) t(u, s)",
        );
        let search = |text: &str| {
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    tokens: crate::domain::query::parse(text).unwrap(),
                    ..ViewQuery::default()
                })
                .unwrap()
        };
        assert_eq!(search(">=7"), [1]);
        assert_eq!(search("<=1"), [0]);
        assert_eq!(search("u>=-1"), [0, 1]);
        assert_eq!(search("u=-1"), Vec::<i64>::new());
        assert_eq!(search("s<=340282366920938463463374607431768211456"), [0, 1]);
        assert_eq!(
            search("s=340282366920938463463374607431768211456"),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn column_filters_bind_values_handle_null_and_can_return_zero_rows() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('A', 1), ('B', 2), (NULL, 3)) t(department, n)",
        );
        let filtered = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Compare(Comparison::Greater, Numeric::Signed(1))],
                filters: vec![ColumnFilter {
                    sql: false,
                    condition: None,
                    column: "department".into(),
                    values: vec![ScalarKey::String("B".into()), ScalarKey::Null],
                }],
                sort: vec![],
            })
            .unwrap();
        assert_eq!(filtered, [1, 2]);
        let empty = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                filters: vec![ColumnFilter {
                    sql: false,
                    condition: None,
                    column: "department".into(),
                    values: vec![ScalarKey::String("absent' OR 1=1 --".into())],
                }],
                sort: vec![],
            })
            .unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn value_filters_match_displayed_text_exclude_keeps_nulls_and_accept_expressions() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('A', 1.5, DATE '2024-01-31'),              ('B', 2.0, DATE '2024-02-01'), (NULL, NULL, DATE '2024-02-15')) t(department, amount, day)",
        );
        let ids = |value_filters: Vec<ValueFilter>| {
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters,
                    ..Default::default()
                })
                .unwrap()
        };
        let filter = |target: Dimension, value: Option<&str>, exclude: bool| ValueFilter {
            target,
            value: value.map(str::to_owned),
            exclude,
        };
        // The displayed (VARCHAR) rendering of each cell is what the filter matches.
        let page = store.fetch_page(&[1], 0, 1).unwrap();
        let amount = page[0].values[1].clone();
        assert_eq!(
            ids(vec![filter("amount".into(), amount.as_deref(), false)]),
            [1]
        );
        assert_eq!(
            ids(vec![filter("department".into(), Some("A"), true)]),
            [1, 2]
        );
        assert_eq!(ids(vec![filter("department".into(), None, false)]), [2]);
        assert_eq!(ids(vec![filter("department".into(), None, true)]), [0, 1]);
        assert_eq!(
            ids(vec![
                filter("department".into(), Some("A"), true),
                filter("department".into(), Some("B"), true),
            ]),
            [2]
        );
        assert_eq!(
            ids(vec![filter(
                Dimension::expression("date_trunc('month', day)::DATE AS month"),
                Some("2024-02-01"),
                false,
            )]),
            [1, 2]
        );
        assert!(
            ids(vec![filter(
                "department".into(),
                Some("A' OR 1=1 --"),
                false
            )])
            .is_empty()
        );
        assert!(
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: vec![filter("missing".into(), Some("x"), false)],
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn search_and_sort_share_the_same_view_query() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('smith', 4), ('smith', 2), ('jones', 1)) t(name, priority)",
        );
        let ids = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Contains("smith".into())],
                filters: vec![],
                sort: vec![SortKey {
                    column: "priority".into(),
                    descending: true,
                }],
            })
            .unwrap();
        assert_eq!(ids, [0, 1]);
    }

    #[test]
    fn numeric_search_handles_signed_unsigned_boundaries() {
        let store = store(
            "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (-1::BIGINT, 0::UBIGINT), (0::BIGINT, 9223372036854775808::UBIGINT), (1::BIGINT, 18446744073709551615::UBIGINT)) t(signed, unsigned)",
        );
        let above_signed_max = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Compare(
                    Comparison::Greater,
                    Numeric::Signed(i64::MAX as i128),
                )],
                filters: vec![],
                sort: vec![],
            })
            .unwrap();
        assert_eq!(above_signed_max, [1, 2]);
        let above_u64_max = store
            .query_row_ids(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![Token::Compare(
                    Comparison::Greater,
                    Numeric::Unsigned(u64::MAX as u128),
                )],
                filters: vec![],
                sort: vec![],
            })
            .unwrap();
        assert!(above_u64_max.is_empty());
    }

    #[test]
    fn parquet_loads_into_a_temp_table_and_descending_sort_is_stable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.parquet");
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES (2, 'a'), (1, 'b'), (1, 'c')) t(n, label)").unwrap();
        let path_sql = path.to_string_lossy().replace('\'', "''");
        connection
            .execute_batch(&format!("COPY qp_data TO '{path_sql}' (FORMAT PARQUET)"))
            .unwrap();
        let store = DuckDbStore::open_parquet(&path).unwrap();
        assert_eq!(
            store
                .schema()
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["n", "label"]
        );
        assert_eq!(store.initial_row_ids().unwrap(), [0, 1, 2]);
        assert_eq!(
            store
                .query_row_ids(&ViewQuery {
                    hidden: Vec::new(),
                    value_filters: Vec::new(),
                    tokens: vec![],
                    filters: vec![],
                    sort: vec![SortKey {
                        column: "n".into(),
                        descending: true
                    }],
                })
                .unwrap(),
            [0, 1, 2]
        );
    }
}
