//! The store a view reads its rows from, in either query engine, and the
//! types both engines share.

use std::path::Path;
use std::sync::Arc;

use super::duckdb_store::{DuckDbStore, ViewRowMap};
use super::polars::store::PolarsStore;
use crate::domain::{
    documents::{Dimension, ScalarKey, SortKey, ValueFilter},
    query::Token,
    sql::SqlTrace,
    statistics::ColumnAggregates,
};

/// The source row at each display position: DuckDB's rowid, or a Polars
/// frame's row number.
#[derive(Clone, Debug)]
pub enum RowIds {
    Listed(Arc<[i64]>),
    /// Every row in rowid order, where the rowids are exactly `0..n`, so a
    /// position is its rowid and no list is built.
    Natural(usize),
}

impl RowIds {
    pub fn len(&self) -> usize {
        match self {
            Self::Listed(ids) => ids.len(),
            Self::Natural(count) => *count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The rowids at positions `start..end`, clamped to the rows there are.
    pub fn range(&self, start: usize, end: usize) -> Vec<i64> {
        let end = end.min(self.len());
        match self {
            Self::Listed(ids) => ids.get(start..end).unwrap_or_default().to_vec(),
            Self::Natural(_) => (start as i64..end as i64).collect(),
        }
    }

    pub fn to_vec(&self) -> Vec<i64> {
        self.range(0, self.len())
    }

    /// Whether both are the same order without comparing every id.
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Listed(a), Self::Listed(b)) => Arc::ptr_eq(a, b),
            (Self::Natural(a), Self::Natural(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Clone)]
pub struct MaterializedViewRows {
    pub row_ids: RowIds,
    /// The order as a DuckDB table; natural order has none.
    pub row_map: Option<Arc<ViewRowMap>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSchema {
    /// The name shown to the user. A physical source column named `rowid` is
    /// preserved under this name while DuckDB's pseudocolumn remains internal.
    pub name: String,
    /// The SQL name of the type, such as `BIGINT` or `VARCHAR`, in DuckDB's
    /// spelling for both engines.
    pub sql_type: String,
    pub(crate) storage_name: String,
    /// The column's type in a Polars store; DuckDB stores leave it out.
    pub(crate) polars_type: Option<polars::prelude::DataType>,
}

impl ColumnSchema {
    pub(crate) fn polars(name: impl Into<String>, dtype: polars::prelude::DataType) -> Self {
        let name = name.into();
        Self {
            storage_name: name.clone(),
            sql_type: super::polars::expressions::sql_type_name(&dtype),
            name,
            polars_type: Some(dtype),
        }
    }

    /// The Polars type of a Polars store's column.
    pub(crate) fn dtype(&self) -> &polars::prelude::DataType {
        self.polars_type.as_ref().unwrap_or(&polars::prelude::DataType::Null)
    }
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedRow {
    pub rowid: i64,
    pub values: Vec<Option<String>>,
}


/// Which engine reads a source and runs its views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Polars,
    #[serde(rename = "duckdb")]
    DuckDb,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Polars, Engine::DuckDb];

    pub fn label(self) -> &'static str {
        match self {
            Self::Polars => "Polars",
            Self::DuckDb => "DuckDB",
        }
    }
}

/// Stops a running query. Each engine stops differently.
#[derive(Clone)]
pub enum Interrupt {
    DuckDb(Arc<duckdb::InterruptHandle>),
}

impl Interrupt {
    pub fn interrupt(&self) {
        match self {
            Self::DuckDb(handle) => handle.interrupt(),
        }
    }
}

/// A source's rows in one engine. Every view of the source, and every result
/// computed from it, stays in that engine.
pub enum Store {
    DuckDb(DuckDbStore),
    Polars(PolarsStore),
}

impl From<DuckDbStore> for Store {
    fn from(store: DuckDbStore) -> Self {
        Self::DuckDb(store)
    }
}

impl From<PolarsStore> for Store {
    fn from(store: PolarsStore) -> Self {
        Self::Polars(store)
    }
}

/// Calls the same method on whichever engine holds the rows.
macro_rules! dispatch {
    ($self:ident, $store:ident => $call:expr) => {
        match $self {
            Store::DuckDb($store) => $call,
            Store::Polars($store) => $call,
        }
    };
}

impl Store {
    pub fn engine(&self) -> Engine {
        match self {
            Self::DuckDb(_) => Engine::DuckDb,
            Self::Polars(_) => Engine::Polars,
        }
    }

    pub fn as_duckdb(&self) -> Option<&DuckDbStore> {
        match self {
            Self::DuckDb(store) => Some(store),
            Self::Polars(_) => None,
        }
    }

    pub fn as_polars(&self) -> Option<&PolarsStore> {
        match self {
            Self::Polars(store) => Some(store),
            Self::DuckDb(_) => None,
        }
    }

    /// The DuckDB store, for SQL that only DuckDB runs.
    pub fn duckdb(&self) -> Result<&DuckDbStore, String> {
        self.as_duckdb()
            .ok_or_else(|| "this needs the DuckDB engine".to_owned())
    }

    pub fn schema(&self) -> &[ColumnSchema] {
        dispatch!(self, store => store.schema())
    }

    pub fn column(&self, name: &str) -> Option<&ColumnSchema> {
        dispatch!(self, store => store.column(name))
    }

    pub fn row_count(&self) -> usize {
        dispatch!(self, store => store.row_count())
    }

    pub fn is_database_view(&self) -> bool {
        dispatch!(self, store => store.is_database_view())
    }

    pub fn database_view_revision(&self) -> Option<u64> {
        match self {
            Self::DuckDb(store) => store.database_view_revision(),
            Self::Polars(_) => None,
        }
    }

    pub fn persistent_identity(&self) -> Option<super::source_database::PersistentSourceIdentity> {
        match self {
            Self::DuckDb(store) => store.persistent_identity(),
            Self::Polars(_) => None,
        }
    }

    /// Stops this store's running query. Polars cannot stop a query once it
    /// runs, so its stores have none.
    pub fn interrupt_handle(&self) -> Option<Interrupt> {
        match self {
            Self::DuckDb(store) => Some(Interrupt::DuckDb(store.interrupt_handle())),
            Self::Polars(_) => None,
        }
    }

    pub fn dimension_name(&self, sql: &str) -> Result<String, String> {
        dispatch!(self, store => store.dimension_name(sql))
    }

    pub fn numeric_ranges(&self, columns: &[String]) -> Result<Vec<Option<(f64, f64)>>, String> {
        dispatch!(self, store => store.numeric_ranges(columns))
    }

    pub fn column_summaries(&self, columns: &[String]) -> Result<Vec<ColumnSummary>, String> {
        dispatch!(self, store => store.column_summaries(columns))
    }

    pub fn top_values(&self, column: &str, limit: usize) -> Result<Vec<(Option<String>, usize)>, String> {
        dispatch!(self, store => store.top_values(column, limit))
    }

    pub fn unique_counts(&self, columns: &[String]) -> Result<Vec<(String, usize)>, String> {
        dispatch!(self, store => store.unique_counts(columns))
    }

    pub fn approximate_unique_counts(&self, columns: &[String]) -> Result<Vec<(String, usize)>, String> {
        dispatch!(self, store => store.approximate_unique_counts(columns))
    }

    pub fn distinct_values(
        &self,
        names: &[String],
        filters: &[ColumnFilter],
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        dispatch!(self, store => store.distinct_values(names, filters))
    }

    pub fn slicer_values(
        &self,
        name: &str,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Vec<crate::domain::slicers::SlicerValue>, String> {
        dispatch!(self, store => store.slicer_values(name, filters, value_filters))
    }

    pub fn initial_row_ids(&self) -> Result<Vec<i64>, String> {
        dispatch!(self, store => store.initial_row_ids())
    }

    pub fn query_row_ids(&self, view: &ViewQuery) -> Result<Vec<i64>, String> {
        dispatch!(self, store => store.query_row_ids(view))
    }

    pub fn query_view_rows(&self, view: &ViewQuery) -> Result<MaterializedViewRows, String> {
        dispatch!(self, store => store.query_view_rows(view))
    }

    pub fn query_view_rows_traced(
        &self,
        view: &ViewQuery,
        trace: &mut SqlTrace,
    ) -> Result<MaterializedViewRows, String> {
        dispatch!(self, store => store.query_view_rows_traced(view, trace))
    }

    pub fn distinct_rendered(
        &self,
        dimensions: &[Dimension],
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        dispatch!(self, store => store.distinct_rendered(dimensions, filters, value_filters))
    }

    pub fn validate_condition(&self, column: &str, text: &str) -> Result<(), String> {
        dispatch!(self, store => store.validate_condition(column, text))
    }

    pub fn validate_sql(&self, text: &str) -> Result<(), String> {
        dispatch!(self, store => store.validate_sql(text))
    }

    pub fn column_bounds(&self, column: &str) -> Result<Option<(String, String)>, String> {
        dispatch!(self, store => store.column_bounds(column))
    }

    pub fn fetch_view_page(
        &self,
        row_map: Option<&ViewRowMap>,
        row_ids: &RowIds,
        start: usize,
        count: usize,
    ) -> Result<Vec<CachedRow>, String> {
        match self {
            Self::DuckDb(store) => store.fetch_view_page(row_map, row_ids, start, count),
            Self::Polars(store) => store.fetch_view_page(row_ids, start, count),
        }
    }

    pub fn fetch_row_ids(&self, ids: &[i64]) -> Result<Vec<CachedRow>, String> {
        dispatch!(self, store => store.fetch_row_ids(ids))
    }

    pub fn export_view(
        &self,
        row_map: Option<&ViewRowMap>,
        row_ids: &RowIds,
        columns: &[usize],
        path: &Path,
        format: crate::domain::export::FileFormat,
    ) -> Result<usize, String> {
        match self {
            Self::DuckDb(store) => store.export_view(row_map, row_ids, columns, path, format),
            Self::Polars(store) => store.export_view(row_ids, columns, path, format),
        }
    }

    pub fn width_candidates(
        &self,
        row_ids: Option<&RowIds>,
        columns: &[(String, Option<crate::domain::formatting::NumericFormat>)],
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        dispatch!(self, store => store.width_candidates(row_ids, columns))
    }

    #[cfg(test)]
    pub(crate) fn width_sampling_calls(&self) -> usize {
        dispatch!(self, store => store.width_sampling_calls())
    }

    /// Aggregates over display positions `start..end` of `row_ids`.
    pub(crate) fn selection_column_aggregates(
        &self,
        row_map: Option<&ViewRowMap>,
        row_ids: &RowIds,
        start: usize,
        end: usize,
        column_indices: &[usize],
        data_types: &[String],
    ) -> Result<(Vec<ColumnAggregates>, usize), String> {
        match self {
            Self::DuckDb(store) => {
                let temporary_map;
                let row_map = match (row_ids, row_map) {
                    (RowIds::Natural(_), _) => None,
                    (RowIds::Listed(_), Some(row_map)) => Some(row_map),
                    (RowIds::Listed(ids), None) => {
                        temporary_map = store.materialize_view_rows_from_ids(ids)?;
                        temporary_map.row_map.as_deref()
                    }
                };
                store.selection_column_aggregates(row_map, start, end, column_indices, data_types)
            }
            Self::Polars(store) => {
                store.selection_column_aggregates(row_ids, start, end, column_indices, data_types)
            }
        }
    }
}
