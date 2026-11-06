use std::path::Path;
use std::sync::Arc;

use polars::prelude::*;

pub use super::expressions::quote_identifier;
use super::expressions::{
    data_type_from_sql, is_nested, polars_error, render_value, rendered, scalar_series,
    sql_type_name,
};
use crate::infrastructure::store::{
    CachedRow, ColumnFilter, ColumnSchema, ColumnSummary, MaterializedViewRows, RowIds, ViewQuery,
    filters_mention_rowid,
};
use crate::infrastructure::sources::FrameIdentity;
use duckdb::types::Value;
use crate::domain::{
    documents::{Dimension, ScalarKey, SortKey, ValueFilter},
    formatting::{list_element_type, numeric_type_name, string_type_name},
    query::{
        ColumnOperator, ColumnTerm, Comparison, Numeric, Token, parse_condition, parse_number,
        resolve_column,
    },
    sql::SqlTrace,
    statistics::{ColumnAggregates, NumericColumnStatistics},
};

/// Owns a source's rows and builds views over them. Methods run on the
/// background executor, never on the UI thread.
pub struct PolarsStore {
    /// Every row in memory, shared with other windows showing the same read;
    /// a row's id is its position.
    frame: Arc<DataFrame>,
    columns: Vec<ColumnSchema>,
    row_count: usize,
    identity: Option<FrameIdentity>,
    /// The row limit a file read or query result stopped at, leaving rows out.
    cut_at: Option<usize>,
    #[cfg(test)]
    width_sampling_calls: std::sync::atomic::AtomicUsize,
}

/// The name of the internal row number column, which the store adds to
/// frames it filters and sorts.
const ROW: &str = "__dd_row";

impl PolarsStore {
    /// Reads a supported file into memory. Desktop file access goes through
    /// `Sources::load`, which also caches the frame.
    pub fn open_source(path: &Path) -> Result<Self, String> {
        super::readers::read_frame(path).map(Self::from_frame)
    }

    pub fn empty() -> Result<Self, String> {
        Self::from_rows(&[("Row".into(), "INTEGER".into())], Vec::new())
    }

    pub fn synthetic(count: usize) -> Result<Self, String> {
        let rows = Series::new("Row".into(), (0..count as i64).collect::<Vec<_>>());
        Ok(Self::from_frame(
            DataFrame::new(count, vec![rows.into_column()]).map_err(polars_error)?,
        ))
    }

    /// A table of `columns` (name and SQL type) holding `rows`.
    pub fn from_rows(
        columns: &[(String, String)],
        rows: impl IntoIterator<Item = Vec<Value>>,
    ) -> Result<Self, String> {
        let rows = rows.into_iter().collect::<Vec<_>>();
        let mut series = Vec::with_capacity(columns.len());
        for (index, (name, sql_type)) in columns.iter().enumerate() {
            let dtype = data_type_from_sql(sql_type)?;
            let values = rows
                .iter()
                .map(|row| {
                    if row.len() != columns.len() {
                        return Err("row width does not match source schema".to_owned());
                    }
                    any_value(&row[index])
                })
                .collect::<Result<Vec<_>, _>>()?;
            series.push(
                Series::from_any_values_and_dtype(name.as_str().into(), &values, &dtype, true)
                    .map_err(polars_error)?
                    .into_column(),
            );
        }
        DataFrame::new(rows.len(), series)
            .map(Self::from_frame)
            .map_err(polars_error)
    }

    pub fn from_frame(frame: DataFrame) -> Self {
        Self::from_shared_frame(Arc::new(frame))
    }

    pub(crate) fn from_shared_frame(frame: Arc<DataFrame>) -> Self {
        let columns = schema_columns(frame.schema());
        Self {
            row_count: frame.height(),
            frame,
            columns,
            identity: None,
            cut_at: None,
            #[cfg(test)]
            width_sampling_calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(crate) fn with_identity(mut self, identity: FrameIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    pub(crate) fn with_cut_at(mut self, cut_at: Option<usize>) -> Self {
        self.cut_at = cut_at;
        self
    }

    pub fn cut_at(&self) -> Option<usize> {
        self.cut_at
    }

    /// Keeps the first `rows` rows of a query result and notes that the
    /// query had more.
    pub fn cut_to(&mut self, rows: usize) -> Result<(), String> {
        if self.row_count > rows {
            self.frame = Arc::new(self.frame.slice(0, rows));
            self.row_count = rows;
            self.cut_at = Some(rows);
        }
        Ok(())
    }

    /// Polars stores always hold every row, unlike DuckDB views read only
    /// through limited queries.
    pub fn is_database_view(&self) -> bool {
        false
    }

    pub fn persistent_identity(&self) -> Option<FrameIdentity> {
        self.identity.clone()
    }

    /// Every row as a lazy frame. With `rowid`, adds a `rowid` column numbering
    /// the rows from 0 for SQL that names it, unless the source has its own.
    pub fn query_frame(&self, rowid: bool) -> LazyFrame {
        let frame = self.frame.as_ref().clone().lazy();
        if rowid && self.column("rowid").is_none() {
            frame
                .with_row_index("rowid", None)
                .with_column(col("rowid").cast(DataType::Int64))
        } else {
            frame
        }
    }

    /// Runs `frame` on Polars' streaming engine, which spreads filters, group
    /// bys, and distinct values across threads and is several times faster on
    /// large sources than the in-memory engine `Engine::Auto` picks. Float sums
    /// and means can differ in their last digits from run to run.
    pub fn collect(&self, frame: LazyFrame) -> Result<DataFrame, String> {
        frame
            .collect_with_engine(Engine::Streaming)
            .map(|result| result.unwrap_single())
            .map_err(polars_error)
    }

    /// Collects `frame` into a new in-memory store, recording its plan under `stage`.
    pub fn materialize(
        &self,
        stage: &str,
        frame: LazyFrame,
        trace: &mut SqlTrace,
    ) -> Result<Self, String> {
        let plan = frame.describe_plan().unwrap_or_default();
        trace
            .execute(stage, &plan, &[], || self.collect(frame))
            .map(Self::from_frame)
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

    fn frame(&self) -> Result<&DataFrame, String> {
        Ok(self.frame.as_ref())
    }

    fn schema_column(&self, name: &str) -> Result<&ColumnSchema, String> {
        self.column(name)
            .ok_or_else(|| format!("column not found: {name}"))
    }

    /// The output name of a row-level dimension expression, checked against
    /// the source schema without reading rows.
    pub fn dimension_name(&self, sql: &str) -> Result<String, String> {
        crate::domain::aggregation_sql::validate_dimension(sql)?;
        let (expression, name) = dimension_expression(sql, &self.columns)?;
        let mut frame = self
            .query_frame(crate::domain::aggregation_sql::mentions_rowid(sql))
            .select([expression.alias(name.as_str())]);
        let schema = frame.collect_schema().map_err(polars_error)?;
        if schema.len() != 1 {
            return Err("Enter one result expression per dimension".into());
        }
        Ok(name)
    }

    /// The rows `view`'s search and filters leave.
    fn view_frame_of(&self, view: &ViewQuery) -> Result<LazyFrame, String> {
        let frame = self.query_frame(self.view_mentions_rowid(view));
        Ok(match self.view_predicate(view)? {
            Some(predicate) => frame.filter(predicate),
            None => frame,
        })
    }

    /// Whole-column numbers for conditional formatting, over the rows `view`
    /// leaves, in one pass. Values that aren't numbers count as blanks.
    pub fn column_aggregates(
        &self,
        view: &ViewQuery,
        requests: &[(String, diver_format::Aggregate)],
    ) -> Result<Vec<Option<f64>>, String> {
        use diver_format::Aggregate;
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let mut projection = Vec::new();
        for (index, (name, aggregate)) in requests.iter().enumerate() {
            let column = self.schema_column(name)?;
            let raw = col(name.as_str());
            let number = if is_nested(&column.dtype()) {
                lit(NULL).cast(DataType::Float64)
            } else {
                raw.clone().cast(DataType::Float64)
            };
            let value = match aggregate {
                Aggregate::Min => number.min(),
                Aggregate::Max => number.max(),
                Aggregate::Mean => number.mean(),
                Aggregate::Median => number.median(),
                Aggregate::Sum => number.sum(),
                Aggregate::Count => raw.count(),
                Aggregate::Distinct => raw.drop_nulls().n_unique(),
                Aggregate::Stdev => number.std(1),
                Aggregate::Percentile(p) => number.quantile(lit(p.clamp(0.0, 1.0)), QuantileMethod::Linear),
            };
            projection.push(value.cast(DataType::Float64).alias(format!("a{index}")));
        }
        let frame = self.collect(self.view_frame_of(view)?.select(projection))?;
        (0..requests.len())
            .map(|index| {
                Ok(frame
                    .column(&format!("a{index}"))
                    .map_err(polars_error)?
                    .get(0)
                    .map_err(polars_error)?
                    .extract::<f64>())
            })
            .collect()
    }

    /// How many of the rows `view` leaves hold each of `values`, which are
    /// written as the grid shows them. Values with no rows are left out.
    pub fn value_counts(&self, view: &ViewQuery, column: &str, values: &[String]) -> Result<Vec<(String, usize)>, String> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let schema = self.schema_column(column)?;
        let wanted = Series::new("wanted".into(), values);
        let frame = self
            .view_frame_of(view)?
            .select([rendered(col(column), &schema.dtype()).alias("value")])
            .filter(col("value").is_in(lit(wanted).implode(false), false))
            .group_by([col("value")])
            .agg([len().alias("n")]);
        let counted = self.collect(frame)?;
        let names = counted.column("value").map_err(polars_error)?.str().map_err(polars_error)?.clone();
        let counts = counted.column("n").map_err(polars_error)?.cast(&DataType::UInt64).map_err(polars_error)?;
        let counts = counts.u64().map_err(polars_error)?;
        Ok(names
            .iter()
            .zip(counts.iter())
            .filter_map(|(name, count)| Some((name?.to_owned(), count? as usize)))
            .collect())
    }

    /// Where each of `values` stands among the column's numbers in the rows
    /// `view` leaves, in one pass.
    pub fn value_positions(
        &self,
        view: &ViewQuery,
        column: &str,
        values: &[f64],
    ) -> Result<Vec<diver_format::stats::Position>, String> {
        if values.is_empty() {
            return Ok(Vec::new());
        }
        let schema = self.schema_column(column)?;
        let number = if is_nested(&schema.dtype()) {
            lit(NULL).cast(DataType::Float64)
        } else {
            col(column).cast(DataType::Float64)
        };
        let mut projection = Vec::new();
        for (index, value) in values.iter().enumerate() {
            let count = |predicate: Expr| predicate.cast(DataType::UInt64).sum().cast(DataType::Float64);
            projection.push(count(number.clone().gt(lit(*value))).alias(format!("above{index}")));
            projection.push(count(number.clone().lt(lit(*value))).alias(format!("below{index}")));
        }
        projection.push(number.count().cast(DataType::Float64).alias("count"));
        let frame = self.collect(self.view_frame_of(view)?.select(projection))?;
        let read = |name: &str| -> Result<usize, String> {
            Ok(frame
                .column(name)
                .map_err(polars_error)?
                .get(0)
                .map_err(polars_error)?
                .extract::<f64>()
                .unwrap_or(0.0) as usize)
        };
        let count = read("count")?;
        (0..values.len())
            .map(|index| {
                Ok(diver_format::stats::Position {
                    above: read(&format!("above{index}"))?,
                    below: read(&format!("below{index}"))?,
                    count,
                })
            })
            .collect()
    }

    /// Row, null, and exact distinct counts plus min and max for each column,
    /// over every row of this store.
    pub fn column_summaries(&self, columns: &[String]) -> Result<Vec<ColumnSummary>, String> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let mut projection = vec![len().alias("rows")];
        for (index, name) in columns.iter().enumerate() {
            let column = self.schema_column(name)?;
            let value = col(name.as_str());
            projection.push(value.clone().null_count().alias(format!("nulls{index}")));
            projection.push(value.clone().drop_nulls().n_unique().alias(format!("unique{index}")));
            // Nested values have no useful order to report.
            if is_nested(&column.dtype()) {
                projection.push(lit(NULL).cast(DataType::String).alias(format!("min{index}")));
                projection.push(lit(NULL).cast(DataType::String).alias(format!("max{index}")));
            } else {
                projection.push(rendered(value.clone().min(), &column.dtype()).alias(format!("min{index}")));
                projection.push(rendered(value.max(), &column.dtype()).alias(format!("max{index}")));
            }
        }
        let frame = self.collect(self.query_frame(false).select(projection))?;
        let total = count_at(&frame, "rows")?;
        columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                Ok(ColumnSummary {
                    name: name.clone(),
                    rows: total,
                    nulls: count_at(&frame, &format!("nulls{index}"))?,
                    unique: count_at(&frame, &format!("unique{index}"))?,
                    min: text_at(&frame, &format!("min{index}"), 0)?,
                    max: text_at(&frame, &format!("max{index}"), 0)?,
                })
            })
            .collect()
    }

    /// The `limit` most frequent values of `column` with their row counts, most
    /// frequent first; ties follow the column's own order. `None` is null.
    pub fn top_values(
        &self,
        column: &str,
        limit: usize,
    ) -> Result<Vec<(Option<String>, usize)>, String> {
        let schema = self.schema_column(column)?;
        let frame = self
            .query_frame(false)
            .group_by([col(column).alias("value")])
            .agg([len().alias("n")])
            .sort_by_exprs(
                [col("n"), col("value")],
                SortMultipleOptions::default()
                    .with_order_descending_multi([true, false])
                    .with_nulls_last(true),
            )
            .limit(limit as IdxSize)
            .select([rendered(col("value"), &schema.dtype()).alias("value"), col("n")]);
        let frame = self.collect(frame)?;
        (0..frame.height())
            .map(|row| Ok((text_at(&frame, "value", row)?, count_row(&frame, "n", row)?)))
            .collect()
    }

    pub fn unique_counts(&self, columns: &[String]) -> Result<Vec<(String, usize)>, String> {
        self.distinct_counts(columns, |value| value.drop_nulls().n_unique())
    }

    /// Distinct non-null values of each column among the first `rows` rows: a
    /// lower bound on its count that stops reading after those rows.
    pub fn leading_unique_counts(&self, columns: &[String], rows: usize) -> Result<Vec<usize>, String> {
        Ok(self
            .distinct_counts_of(self.query_frame(false).limit(rows as IdxSize), columns, |value| {
                value.drop_nulls().n_unique()
            })?
            .into_iter()
            .map(|(_, count)| count)
            .collect())
    }

    fn distinct_counts(
        &self,
        columns: &[String],
        count: impl Fn(Expr) -> Expr,
    ) -> Result<Vec<(String, usize)>, String> {
        self.distinct_counts_of(self.query_frame(false), columns, count)
    }

    fn distinct_counts_of(
        &self,
        frame: LazyFrame,
        columns: &[String],
        count: impl Fn(Expr) -> Expr,
    ) -> Result<Vec<(String, usize)>, String> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let projection = columns
            .iter()
            .enumerate()
            .map(|(index, name)| {
                self.schema_column(name)?;
                Ok(count(col(name.as_str())).alias(format!("n{index}")))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let frame = self.collect(frame.select(projection))?;
        columns
            .iter()
            .enumerate()
            .map(|(index, name)| Ok((name.clone(), count_at(&frame, &format!("n{index}"))?)))
            .collect()
    }

    /// Distinct rendered combinations of `names` among rows matching `filters`,
    /// in the columns' order.
    pub fn distinct_values(
        &self,
        names: &[String],
        filters: &[ColumnFilter],
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let columns = names
            .iter()
            .map(|name| self.schema_column(name))
            .collect::<Result<Vec<_>, _>>()?;
        let frame = self
            .filtered_frame(filters, &[])?
            .select(names.iter().map(|name| col(name.as_str())).collect::<Vec<_>>())
            .unique_stable(None, UniqueKeepStrategy::Any)
            .sort_by_exprs(
                names.iter().map(|name| col(name.as_str())).collect::<Vec<_>>(),
                SortMultipleOptions::default().with_nulls_last(true),
            )
            .select(
                columns
                    .iter()
                    .map(|column| rendered(col(column.name.as_str()), &column.dtype()))
                    .collect::<Vec<_>>(),
            );
        text_rows(&self.collect(frame)?)
    }

    /// The distinct values of `name` among rows matching the filters, with one
    /// value beyond the slicer limit to tell eligible columns from larger ones.
    pub fn slicer_values(
        &self,
        name: &str,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Vec<crate::domain::slicers::SlicerValue>, String> {
        let column = self
            .column(name)
            .ok_or_else(|| format!("slicer column not found: {name}"))?;
        let value = col(name);
        let order = if crate::domain::schema::from_duckdb(&column.sql_type)
            == crate::domain::schema::ColumnType::String
        {
            value.clone().cast(DataType::String)
        } else {
            value.clone()
        };
        let frame = self
            .filtered_frame(filters, value_filters)?
            .select([value.clone()])
            .unique(None, UniqueKeepStrategy::Any)
            .limit((crate::domain::slicers::MAX_UNIQUE_VALUES + 1) as IdxSize)
            .sort_by_exprs([order], SortMultipleOptions::default().with_nulls_last(true))
            .select([
                value.clone(),
                rendered(value, &column.dtype()).alias("__dd_label"),
            ]);
        let frame = self.collect(frame)?;
        let values = frame.column(name).map_err(polars_error)?;
        let mut output = Vec::with_capacity(frame.height());
        for row in 0..frame.height() {
            let value = values.get(row).map_err(polars_error)?;
            let key = super::expressions::scalar_key(&value, &column.dtype())?;
            let label = match &key {
                ScalarKey::Null => "(Blank)".into(),
                ScalarKey::String(value) if value.is_empty() => "(Empty)".into(),
                _ => text_at(&frame, "__dd_label", row)?.unwrap_or_default(),
            };
            output.push(crate::domain::slicers::SlicerValue {
                key,
                label,
                available: true,
            });
        }
        Ok(output)
    }

    /// The slicer values of each of `names`, or `None` for a column with more
    /// values than a slicer shows. The frame is in memory, so each column is
    /// read on its own.
    pub fn slicer_values_of(
        &self,
        names: &[String],
    ) -> Result<Vec<Option<Vec<crate::domain::slicers::SlicerValue>>>, String> {
        names
            .iter()
            .map(|name| {
                let values = self.slicer_values(name, &[], &[])?;
                Ok((values.len() <= crate::domain::slicers::MAX_UNIQUE_VALUES).then_some(values))
            })
            .collect()
    }

    pub fn initial_row_ids(&self) -> Result<Vec<i64>, String> {
        self.query_row_ids(&ViewQuery::default())
    }

    pub fn query_row_ids(&self, view: &ViewQuery) -> Result<Vec<i64>, String> {
        Ok(self.query_view_rows(view)?.row_ids.to_vec())
    }

    /// The source rows `view` shows, in display order. Display positions are
    /// zero-based; ties in the sort keep source order.
    pub fn query_view_rows(&self, view: &ViewQuery) -> Result<MaterializedViewRows, String> {
        self.query_view_rows_traced(view, &mut SqlTrace::default())
    }

    pub fn query_view_rows_traced(
        &self,
        view: &ViewQuery,
        trace: &mut SqlTrace,
    ) -> Result<MaterializedViewRows, String> {
        let predicate = self.view_predicate(view)?;
        let order = self.sort_columns(&view.sort)?;
        if predicate.is_none() && order.is_empty() {
            let count = self.row_count;
            return trace.execute("Result search and sort", "Every row in source order", &[], || {
                Ok(MaterializedViewRows {
                    row_ids: RowIds::Natural(count),
                    row_map: None,
                })
            });
        }
        let start = std::time::Instant::now();
        let mut frame = self
            .query_frame(self.view_mentions_rowid(view))
            .with_row_index(ROW, None);
        if let Some(predicate) = predicate {
            frame = frame.filter(predicate);
        }
        frame = self.sorted(frame, &order, &view.sort);
        let frame = frame.select([col(ROW)]);
        let plan = frame.describe_plan().unwrap_or_default();
        let rows = trace.execute("Result search and sort", &plan, &[], || self.collect(frame))?;
        crate::infrastructure::performance::record("row_map_creation", start);
        let _span = crate::infrastructure::performance::Span::new("row_id_extraction");
        let ids = rows
            .column(ROW)
            .map_err(polars_error)?
            .idx()
            .map_err(polars_error)?
            .into_no_null_iter()
            .map(i64::from)
            .collect::<Vec<_>>();
        Ok(MaterializedViewRows {
            row_ids: RowIds::Listed(ids.into()),
            row_map: None,
        })
    }

    fn view_mentions_rowid(&self, view: &ViewQuery) -> bool {
        filters_mention_rowid(&view.filters, &view.value_filters)
    }

    fn sort_columns<'a>(&'a self, sort: &[SortKey]) -> Result<Vec<&'a ColumnSchema>, String> {
        sort.iter()
            .map(|key| {
                self.column(&key.column)
                    .ok_or_else(|| format!("sort column not found: {}", key.column))
            })
            .collect()
    }

    fn sorted(&self, frame: LazyFrame, columns: &[&ColumnSchema], sort: &[SortKey]) -> LazyFrame {
        if columns.is_empty() {
            return frame;
        }
        frame.sort_by_exprs(
            columns
                .iter()
                .map(|column| col(column.name.as_str()))
                .collect::<Vec<_>>(),
            SortMultipleOptions::default()
                .with_order_descending_multi(sort.iter().map(|key| key.descending))
                .with_nulls_last(true)
                .with_maintain_order(true),
        )
    }

    /// Applies `view`'s search, filters, and sort to `frame`, a query producing
    /// this store's columns, before keeping its first `limit` rows.
    pub fn apply_view(
        &self,
        view: &ViewQuery,
        frame: LazyFrame,
        limit: u64,
    ) -> Result<LazyFrame, String> {
        let mut frame = frame;
        if let Some(predicate) = self.view_predicate(view)? {
            frame = frame.filter(predicate);
        }
        let order = self.sort_columns(&view.sort)?;
        Ok(self
            .sorted(frame, &order, &view.sort)
            .limit(limit.min(IdxSize::MAX as u64) as IdxSize))
    }

    /// `view`'s search terms and filters as one predicate, or `None` when they
    /// keep every row.
    fn view_predicate(&self, view: &ViewQuery) -> Result<Option<Expr>, String> {
        let mut predicates = Vec::new();
        for token in &view.tokens {
            predicates.push(self.token_predicate(token, &view.hidden)?);
        }
        if let Some(filter) = self.filter_predicate(&view.filters, &view.value_filters)? {
            predicates.push(filter);
        }
        Ok(predicates.into_iter().reduce(Expr::and))
    }

    /// The condition one search token puts on a row.
    fn token_predicate(&self, token: &Token, hidden: &[String]) -> Result<Expr, String> {
        Ok(match token {
            Token::Contains(text) => self.search_predicate(text, hidden),
            Token::Excludes(text) => self.search_predicate(text, hidden).not(),
            Token::Compare(comparison, number) => {
                let columns = self
                    .columns
                    .iter()
                    .filter(|column| is_numeric_type(&column.sql_type) && !hidden.contains(&column.name))
                    .map(|column| numeric_predicate(column, number, *comparison).fill_null(lit(false)))
                    .collect::<Vec<_>>();
                any(columns)
            }
            Token::Column(term) => {
                let column = resolve_column(
                    self.columns.iter().map(|column| column.name.as_str()),
                    &term.column,
                )
                .map(|index| &self.columns[index]);
                let predicate = match (column, term.fallback()) {
                    (Some(column), _) => self.term_predicate(column, term)?,
                    (None, Token::Contains(text) | Token::Excludes(text)) => {
                        self.search_predicate(&text, hidden)
                    }
                    (None, _) => unreachable!("fallback is a text token"),
                };
                if term.negated { predicate.not() } else { predicate }
            }
            Token::Any(groups) => {
                let mut alternatives = Vec::new();
                for group in groups {
                    let all = group
                        .iter()
                        .map(|token| self.token_predicate(token, hidden))
                        .collect::<Result<Vec<_>, _>>()?;
                    alternatives.push(all.into_iter().reduce(Expr::and).unwrap_or(lit(true)));
                }
                alternatives.into_iter().reduce(Expr::or).unwrap_or(lit(true))
            }
        })
    }

    /// The rows matching `filters` and `value_filters`, with a `rowid` column
    /// when one of them names it.
    pub fn filtered_frame(
        &self,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<LazyFrame, String> {
        let frame = self.query_frame(filters_mention_rowid(filters, value_filters));
        Ok(match self.filter_predicate(filters, value_filters)? {
            Some(predicate) => frame.filter(predicate),
            None => frame,
        })
    }

    /// A column or row-level expression as the grid displays it.
    fn rendered_dimension(&self, target: &Dimension) -> Result<Expr, String> {
        match target {
            Dimension::Column(name) => {
                let column = self
                    .column(name)
                    .ok_or_else(|| format!("filter column not found: {name}"))?;
                Ok(rendered(col(name.as_str()), &column.dtype()))
            }
            Dimension::Expression { sql } => {
                let (expression, _) = dimension_expression(sql, &self.columns)?;
                let dtype = self.expression_type(
                    expression.clone(),
                    crate::domain::aggregation_sql::mentions_rowid(sql),
                )?;
                Ok(rendered(expression, &dtype))
            }
        }
    }

    /// The type `expression` produces over this store's rows.
    fn expression_type(&self, expression: Expr, rowid: bool) -> Result<DataType, String> {
        let schema = self
            .query_frame(rowid)
            .select([expression.alias("__dd_type")])
            .collect_schema()
            .map_err(polars_error)?;
        Ok(schema.get("__dd_type").cloned().unwrap_or(DataType::Null))
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
            .enumerate()
            .map(|(index, dimension)| {
                Ok(self.rendered_dimension(dimension)?.alias(format!("d{index}")))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let rowid = dimensions.iter().any(|dimension| {
            matches!(dimension, Dimension::Expression { sql }
                if crate::domain::aggregation_sql::mentions_rowid(sql))
        });
        let mut frame = self.filtered_frame(filters, value_filters)?;
        if rowid && !filters_mention_rowid(filters, value_filters) {
            frame = self.query_frame(true);
            if let Some(predicate) = self.filter_predicate(filters, value_filters)? {
                frame = frame.filter(predicate);
            }
        }
        let frame = frame
            .select(projection)
            .unique_stable(None, UniqueKeepStrategy::Any);
        text_rows(&self.collect(frame)?)
    }

    /// The predicate of value filters and slicer cards, or `None` when they
    /// keep every row.
    pub fn filter_predicate(
        &self,
        filters: &[ColumnFilter],
        value_filters: &[ValueFilter],
    ) -> Result<Option<Expr>, String> {
        let mut predicates = Vec::new();
        for filter in value_filters {
            let rendered = self.rendered_dimension(&filter.target)?;
            let keys: Vec<&str> = filter.values.iter().flatten().map(String::as_str).collect();
            let mut alternatives = Vec::new();
            if !keys.is_empty() {
                alternatives.push(rendered.clone().is_in(lit(Series::new("".into(), keys)).implode(false), false));
            }
            if filter.values.contains(&None) {
                alternatives.push(rendered.is_null());
            }
            // A NULL target matches no value, so excluding values keeps it.
            let matches = any(alternatives).fill_null(lit(false));
            predicates.push(if filter.exclude { matches.not() } else { matches });
        }
        for filter in filters {
            if let Some(text) = &filter.condition {
                // Blank text leaves the card inactive.
                if !text.trim().is_empty() {
                    predicates.push(if filter.sql {
                        sql_predicate(text, &self.columns)?
                    } else {
                        let column = self.filter_column(&filter.column)?;
                        self.condition_predicate(column, text)?
                    });
                }
                continue;
            }
            if filter.values.is_empty() {
                continue;
            }
            let column = self.filter_column(&filter.column)?;
            predicates.push(value_predicate(column, &filter.values)?);
        }
        Ok(predicates.into_iter().reduce(Expr::and))
    }

    /// A scoped term's predicate; a double-quoted value naming a column compares
    /// the two columns row by row.
    fn term_predicate(&self, column: &ColumnSchema, term: &ColumnTerm) -> Result<Expr, String> {
        match (
            term.operator,
            term.compared_column(self.columns.iter().map(|column| column.name.as_str())),
        ) {
            (ColumnOperator::Compare(comparison), Some(other)) => {
                Ok(column_comparison(column, &self.columns[other], comparison))
            }
            _ => column_predicate(column, term),
        }
    }

    fn filter_column(&self, name: &str) -> Result<&ColumnSchema, String> {
        self.column(name)
            .ok_or_else(|| format!("filter column not found: {name}"))
    }

    /// Predicate for a condition card on `column`.
    fn condition_predicate(&self, column: &ColumnSchema, text: &str) -> Result<Expr, String> {
        let today = chrono::Local::now().date_naive();
        let predicates = parse_condition(text, &column.name, today)?
            .iter()
            .map(|term| {
                let predicate = self.term_predicate(column, term)?;
                Ok(if term.negated { predicate.not() } else { predicate })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(predicates.into_iter().reduce(Expr::and).unwrap_or(lit(true)))
    }

    /// Checks that a condition card's text parses and binds, without running it.
    pub fn validate_condition(&self, column: &str, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let column = self.filter_column(column)?;
        let predicate = self.condition_predicate(column, text)?;
        self.validate_predicate(predicate, false)
    }

    /// Checks that an SQL card's predicate binds against the source, without
    /// running it.
    pub fn validate_sql(&self, text: &str) -> Result<(), String> {
        if text.trim().is_empty() {
            return Ok(());
        }
        let predicate = sql_predicate(text, &self.columns)?;
        self.validate_predicate(predicate, crate::domain::aggregation_sql::mentions_rowid(text))
    }

    fn validate_predicate(&self, predicate: Expr, rowid: bool) -> Result<(), String> {
        let schema = self
            .query_frame(rowid)
            .select([predicate.alias("__dd_predicate")])
            .collect_schema()
            .map_err(|error| {
                polars_error(error)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })?;
        match schema.get("__dd_predicate") {
            Some(DataType::Boolean | DataType::Null) => Ok(()),
            Some(other) => Err(format!(
                "The condition gives {} values; it must give true or false",
                sql_type_name(other)
            )),
            None => Ok(()),
        }
    }

    /// The smallest and largest non-null values of `column`, as displayed.
    pub fn column_bounds(&self, column: &str) -> Result<Option<(String, String)>, String> {
        let column = self.filter_column(column)?;
        if is_nested(&column.dtype()) {
            return Ok(None);
        }
        let value = col(column.name.as_str());
        let frame = self.collect(self.query_frame(false).select([
            rendered(value.clone().min(), &column.dtype()).alias("min"),
            rendered(value.max(), &column.dtype()).alias("max"),
        ]))?;
        Ok(text_at(&frame, "min", 0)?.zip(text_at(&frame, "max", 0)?))
    }

    fn search_predicate(&self, text: &str, hidden: &[String]) -> Expr {
        let needle = text.to_lowercase();
        let needs_lowercase = !needle.chars().all(|ch| ch.is_ascii() && !ch.is_ascii_alphabetic());
        let predicates = self
            .columns
            .iter()
            .filter(|column| !hidden.contains(&column.name))
            .filter_map(|column| {
                let value = rendered(col(column.name.as_str()), &column.dtype());
                let value = match search_rendering(&column.sql_type)? {
                    // Lowercasing only changes letters, so a needle without them,
                    // such as a year, matches the text as it is.
                    SearchRendering::Text if !needs_lowercase => value,
                    SearchRendering::Text => value.str().to_lowercase(),
                    // Converting every value to text dominates search on wide numeric
                    // tables, so skip columns whose rendering cannot contain the needle.
                    SearchRendering::Lowercase(renders) => {
                        if !needle.chars().all(renders) {
                            return None;
                        }
                        value
                    }
                };
                Some(
                    value
                        .str()
                        .contains_literal(lit(needle.clone()))
                        .fill_null(lit(false)),
                )
            })
            .collect::<Vec<_>>();
        any(predicates)
    }

    /// Fetch display positions `start..start + count` of the view whose rows
    /// are `row_ids`.
    pub fn fetch_view_page(
        &self,
        row_ids: &RowIds,
        start: usize,
        count: usize,
    ) -> Result<Vec<CachedRow>, String> {
        let end = start.saturating_add(count).min(row_ids.len());
        if start >= end {
            return Ok(Vec::new());
        }
        self.fetch_row_ids(&row_ids.range(start, end))
    }

    /// The rows `ids` in that order, rendered as text.
    pub fn fetch_row_ids(&self, ids: &[i64]) -> Result<Vec<CachedRow>, String> {
        let _span = crate::infrastructure::performance::Span::new("page_retrieval_conversion");
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let frame = self.frame()?;
        let positions = ids
            .iter()
            .filter(|id| (0..self.row_count as i64).contains(*id))
            .map(|id| *id as IdxSize)
            .collect::<Vec<_>>();
        let taken = frame
            .take(&IdxCa::from_vec("".into(), positions.clone()))
            .map_err(polars_error)?;
        let rendered_frame = self.render_frame(taken)?;
        let columns = rendered_frame.columns();
        Ok(positions
            .iter()
            .enumerate()
            .map(|(row, position)| CachedRow {
                rowid: i64::from(*position),
                values: columns
                    .iter()
                    .map(|column| column.str().ok().and_then(|text| text.get(row).map(str::to_owned)))
                    .collect(),
            })
            .collect())
    }

    /// Every column of `frame`, which has this store's schema, as text.
    fn render_frame(&self, frame: DataFrame) -> Result<DataFrame, String> {
        if self.columns.is_empty() {
            return Ok(frame);
        }
        frame
            .lazy()
            .select(
                self.columns
                    .iter()
                    .map(|column| rendered(col(column.name.as_str()), &column.dtype()))
                    .collect::<Vec<_>>(),
            )
            .collect()
            .map_err(polars_error)
    }

    /// The rows `row_ids` lists, keeping only `columns` (indices into this
    /// store's schema) in that order.
    pub fn view_frame(&self, row_ids: &RowIds, columns: &[usize]) -> Result<DataFrame, String> {
        let frame = self.frame()?;
        let names = columns
            .iter()
            .map(|index| {
                self.columns
                    .get(*index)
                    .map(|column| column.name.clone())
                    .ok_or_else(|| "export column is out of range".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let projected = frame.select(names).map_err(polars_error)?;
        match row_ids {
            RowIds::Natural(count) => Ok(projected.slice(0, *count)),
            RowIds::Listed(ids) => projected
                .take(&IdxCa::from_vec(
                    "".into(),
                    ids.iter().map(|id| *id as IdxSize).collect(),
                ))
                .map_err(polars_error),
        }
    }

    /// Copies the rows `row_ids` lists to `path`, keeping only `columns`
    /// (indices into this store's schema) in that order. Values keep their
    /// types; nothing is rendered. Returns the rows written.
    pub fn export_view(
        &self,
        row_ids: &RowIds,
        columns: &[usize],
        path: &Path,
        format: crate::domain::export::FileFormat,
    ) -> Result<usize, String> {
        if columns.is_empty() {
            return Err("There are no columns to export".into());
        }
        let mut frame = self.view_frame(row_ids, columns)?;
        super::export::write(&mut frame, path, format)?;
        Ok(frame.height())
    }

    /// Bounded width previews, separate from result pages and the display cache.
    /// `row_ids` is supplied for query results; tables sample their unfiltered source.
    pub fn width_candidates(
        &self,
        row_ids: Option<&RowIds>,
        columns: &[(String, Option<crate::domain::formatting::NumericFormat>)],
    ) -> Result<Vec<(String, Vec<String>)>, String> {
        use crate::domain::widths::{candidates, sample_positions};
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        #[cfg(test)]
        self.width_sampling_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _span = crate::infrastructure::performance::Span::new("width_sampling");
        let ids = match row_ids {
            Some(rows) => sample_positions(rows.len())
                .into_iter()
                .map(|position| match rows {
                    RowIds::Listed(ids) => ids[position],
                    RowIds::Natural(_) => position as i64,
                })
                .collect::<Vec<_>>(),
            None => sample_positions(self.row_count)
                .into_iter()
                .map(|position| position as i64)
                .collect(),
        };
        let frame = self.frame()?;
        let schema = columns
            .iter()
            .map(|(name, _)| {
                self.column(name)
                    .ok_or_else(|| format!("width column no longer exists: {name}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let taken = frame
            .select(schema.iter().map(|column| column.name.clone()))
            .map_err(polars_error)?
            .take(&IdxCa::from_vec(
                "".into(),
                ids.iter().map(|id| *id as IdxSize).collect(),
            ))
            .map_err(polars_error)?;
        let previews = taken
            .lazy()
            .select(
                schema
                    .iter()
                    .map(|column| {
                        rendered(col(column.name.as_str()), &column.dtype())
                            .str()
                            .slice(lit(0), lit(201))
                    })
                    .collect::<Vec<_>>(),
            )
            .collect()
            .map_err(polars_error)?;
        Ok(columns
            .iter()
            .zip(schema)
            .map(|((name, format), column)| {
                let values = previews
                    .column(&column.name)
                    .ok()
                    .and_then(|values| values.str().ok().cloned())
                    .map(|values| {
                        values
                            .iter()
                            .map(|value| value.map(str::to_owned))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (name.clone(), candidates(values, &column.sql_type, *format))
            })
            .collect())
    }

    #[cfg(test)]
    pub(crate) fn width_sampling_calls(&self) -> usize {
        self.width_sampling_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Aggregates over display positions `start..end` of the rows `row_ids` lists.
    pub(crate) fn selection_column_aggregates(
        &self,
        row_ids: &RowIds,
        start: usize,
        end: usize,
        column_indices: &[usize],
        data_types: &[String],
    ) -> Result<(Vec<ColumnAggregates>, usize), String> {
        if column_indices.len() != data_types.len() {
            return Err("selection statistics columns do not match the result schema".into());
        }
        let end = end.min(row_ids.len());
        let selected = RowIds::Listed(row_ids.range(start, end).into());
        let frame = self.view_frame(&selected, column_indices)?;
        let names = frame
            .get_column_names()
            .into_iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        // One column counts distinct non-null values; several count distinct row
        // tuples, so a selection of key columns shows whether those keys are unique.
        let unique = match names.as_slice() {
            [name] => frame
                .column(name)
                .map_err(polars_error)?
                .drop_nulls()
                .n_unique()
                .map_err(polars_error)?,
            _ => frame
                .unique_stable(None, UniqueKeepStrategy::Any, None)
                .map_err(polars_error)?
                .height(),
        };
        let mut result = Vec::with_capacity(names.len());
        for (name, data_type) in names.iter().zip(data_types) {
            let column = frame
                .column(name)
                .map_err(polars_error)?
                .as_materialized_series()
                .clone();
            let nulls = column.null_count();
            let characters = if string_type_name(data_type) {
                column
                    .cast(&DataType::String)
                    .map_err(polars_error)?
                    .str()
                    .map_err(polars_error)?
                    .iter()
                    .flatten()
                    .map(|text| text.chars().count())
                    .sum()
            } else {
                0
            };
            let numeric = numeric_type_name(data_type);
            let (numeric_count, numeric_sum_as_f64, finite) = if numeric {
                let floats = column.cast(&DataType::Float64).map_err(polars_error)?;
                let finite = floats
                    .f64()
                    .map_err(polars_error)?
                    .iter()
                    .flatten()
                    .filter(|value| value.is_finite())
                    .collect::<Vec<_>>();
                (finite.len(), finite.iter().sum(), finite)
            } else {
                (0, 0.0, Vec::new())
            };
            let (integer_sum, integer_sum_overflow) = if numeric && is_integer_type(data_type) {
                integer_sum(&column)?
            } else {
                (None, false)
            };
            let single_numeric = (numeric && names.len() == 1 && !finite.is_empty())
                .then(|| numeric_statistics(&finite));
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
}

/// The exact sum of an integer column as text, or overflow past 128 bits.
fn integer_sum(column: &Series) -> Result<(Option<String>, bool), String> {
    let mut sum: Option<i128> = None;
    for value in column.iter() {
        let value = match value {
            AnyValue::Null => continue,
            AnyValue::Int8(v) => v as i128,
            AnyValue::Int16(v) => v as i128,
            AnyValue::Int32(v) => v as i128,
            AnyValue::Int64(v) => v as i128,
            AnyValue::Int128(v) => v,
            AnyValue::UInt8(v) => v as i128,
            AnyValue::UInt16(v) => v as i128,
            AnyValue::UInt32(v) => v as i128,
            AnyValue::UInt64(v) => v as i128,
            AnyValue::UInt128(v) => match i128::try_from(v) {
                Ok(v) => v,
                Err(_) => return Ok((None, true)),
            },
            _ => continue,
        };
        sum = match sum.unwrap_or(0).checked_add(value) {
            Some(total) => Some(total),
            None => return Ok((None, true)),
        };
    }
    Ok((sum.map(|sum| sum.to_string()), false))
}

fn numeric_statistics(values: &[f64]) -> NumericColumnStatistics {
    let count = values.len() as f64;
    let mean = values.iter().sum::<f64>() / count;
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    let sample_std_dev = (values.len() > 1).then(|| {
        let squares = values.iter().map(|value| (value - mean).powi(2)).sum::<f64>();
        (squares / (count - 1.0)).sqrt()
    });
    NumericColumnStatistics {
        mean,
        median,
        minimum: sorted[0],
        maximum: sorted[sorted.len() - 1],
        sample_std_dev,
    }
}

fn schema_columns(schema: &Schema) -> Vec<ColumnSchema> {
    schema
        .iter()
        .map(|(name, dtype)| ColumnSchema::polars(name.as_str(), dtype.clone()))
        .collect()
}

/// A DuckDB value as Polars reads it, for tables built in code.
fn any_value(value: &Value) -> Result<AnyValue<'static>, String> {
    Ok(match value {
        Value::Null => AnyValue::Null,
        Value::Boolean(value) => AnyValue::Boolean(*value),
        Value::TinyInt(value) => AnyValue::Int8(*value),
        Value::SmallInt(value) => AnyValue::Int16(*value),
        Value::Int(value) => AnyValue::Int32(*value),
        Value::BigInt(value) => AnyValue::Int64(*value),
        Value::HugeInt(value) => AnyValue::Int128(*value),
        Value::UTinyInt(value) => AnyValue::UInt8(*value),
        Value::USmallInt(value) => AnyValue::UInt16(*value),
        Value::UInt(value) => AnyValue::UInt32(*value),
        Value::UBigInt(value) => AnyValue::UInt64(*value),
        Value::Float(value) => AnyValue::Float32(*value),
        Value::Double(value) => AnyValue::Float64(*value),
        Value::Text(value) => AnyValue::StringOwned(value.as_str().into()),
        other => return Err(format!("unsupported value for a Polars table: {other:?}")),
    })
}

/// A row-level dimension expression over `schema` and its output name: the
/// alias after `AS`, or the expression's own text.
pub fn dimension_expression(sql: &str, schema: &[ColumnSchema]) -> Result<(Expr, String), String> {
    let body = crate::domain::aggregation_sql::dimension_body(sql)?;
    let name = crate::domain::aggregation_sql::dimension_alias(sql)
        .unwrap_or_else(|| super::expressions::display_name(body));
    let expression = super::expressions::text_for_categoricals(super::expressions::parse_sql_expression(body)?, schema);
    // A dimension gives one value per row; aggregates and window functions
    // belong in the aggregations instead.
    if (&expression)
        .into_iter()
        .any(|node| matches!(node, Expr::Agg(_) | Expr::Len | Expr::Over { .. }))
    {
        return Err(
            "A dimension gives a value for each row; put aggregates and window functions in the aggregations".into(),
        );
    }
    Ok((expression, name))
}

/// An SQL card's predicate over `schema`. Rows where it is null do not match.
fn sql_predicate(text: &str, schema: &[ColumnSchema]) -> Result<Expr, String> {
    let predicate = super::expressions::parse_sql_expression(text)?;
    Ok(super::expressions::text_for_categoricals(predicate, schema).fill_null(lit(false)))
}

/// Rows whose value is one of `values`; `ScalarKey::Null` matches null.
fn value_predicate(column: &ColumnSchema, values: &[ScalarKey]) -> Result<Expr, String> {
    let value = col(column.name.as_str());
    let nulls = values.iter().any(|key| matches!(key, ScalarKey::Null));
    let keys = values
        .iter()
        .filter(|key| !matches!(key, ScalarKey::Null))
        .collect::<Vec<_>>();
    let mut alternatives = Vec::new();
    if !keys.is_empty() {
        let series = scalar_series(&keys, &column.dtype())?;
        alternatives.push(value.clone().is_in(lit(series).implode(false), false).fill_null(lit(false)));
    }
    if nulls {
        alternatives.push(value.is_null());
    }
    Ok(any(alternatives))
}

/// True when any of `predicates` is, and false when there are none.
fn any(predicates: Vec<Expr>) -> Expr {
    predicates.into_iter().reduce(Expr::or).unwrap_or(lit(false))
}

fn count_at(frame: &DataFrame, name: &str) -> Result<usize, String> {
    count_row(frame, name, 0)
}

fn count_row(frame: &DataFrame, name: &str, row: usize) -> Result<usize, String> {
    frame
        .column(name)
        .map_err(polars_error)?
        .get(row)
        .map_err(polars_error)?
        .extract::<u64>()
        .map(|count| count as usize)
        .ok_or_else(|| format!("missing count {name}"))
}

fn text_at(frame: &DataFrame, name: &str, row: usize) -> Result<Option<String>, String> {
    Ok(render_value(
        &frame
            .column(name)
            .map_err(polars_error)?
            .get(row)
            .map_err(polars_error)?,
    ))
}

fn text_rows(frame: &DataFrame) -> Result<Vec<Vec<Option<String>>>, String> {
    (0..frame.height())
        .map(|row| {
            frame
                .columns()
                .iter()
                .map(|column| Ok(render_value(&column.get(row).map_err(polars_error)?)))
                .collect()
        })
        .collect()
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

fn is_numeric_type(data_type: &str) -> bool {
    numeric_type_name(data_type)
}

fn is_searchable_type(data_type: &str) -> bool {
    // A list or array is searched through its text when its elements could be.
    if let Some(element) = list_element_type(data_type) {
        return is_searchable_type(element);
    }
    let kind = data_type.to_ascii_uppercase();
    is_numeric_type(&kind)
        || ["VARCHAR", "CHAR", "BOOLEAN", "UUID", "ENUM"]
            .iter()
            .any(|name| kind.starts_with(name))
}

enum SearchRendering {
    /// Free text that must be lowercased before matching.
    Text,
    /// Scalars rendered in lowercase using only the accepted characters.
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
        // Includes exponents (1e20) and the specials inf, -inf, and NaN.
        |ch| matches!(ch, '0'..='9' | '-' | '+' | '.' | 'e' | 'i' | 'n' | 'f' | 'a')
    } else if kind == "BOOLEAN" {
        |ch| "truefals".contains(ch)
    } else {
        return Some(SearchRendering::Text);
    };
    Some(SearchRendering::Lowercase(renders))
}

/// Predicate for a search term scoped to `column`. Text matching is
/// case-insensitive on the value as the grid renders it; numeric columns
/// compare numerically, and other types compare against the value cast to the
/// column type.
fn column_predicate(column: &ColumnSchema, term: &ColumnTerm) -> Result<Expr, String> {
    let value = col(column.name.as_str());
    let lowered = rendered(value.clone(), &column.dtype()).str().to_lowercase();
    let predicate = match term.operator {
        ColumnOperator::Contains => lowered
            .str()
            .contains_literal(lit(term.value.to_lowercase())),
        ColumnOperator::Compare(comparison) if is_numeric_type(&column.sql_type) => {
            let number = parse_number(&term.value)?;
            numeric_predicate(column, &number, comparison)
        }
        ColumnOperator::Compare(comparison) if string_type_name(&column.sql_type) => {
            text_predicate(lowered, comparison, &term.value.to_lowercase())
        }
        ColumnOperator::Compare(comparison) if is_date_type(&column.dtype()) => {
            match crate::domain::query::date_period(&term.value) {
                Some(period) => date_predicate(column, comparison, period),
                // Timestamps also accept a full date and time.
                None if matches!(column.dtype(), DataType::Datetime(..)) => {
                    let moment = parse_datetime(&term.value).ok_or_else(|| {
                        format!(
                            "{} is not a date or time; use YYYY-MM-DD or YYYY-MM-DD HH:MM:SS",
                            term.value
                        )
                    })?;
                    compare(local_time(column), comparison, datetime_literal(moment))
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
            lowered.eq(lit(term.value.to_lowercase()))
        }
        ColumnOperator::Compare(comparison) => compare(
            value,
            comparison,
            lit(term.value.clone()).cast(column.dtype().clone()),
        ),
    };
    // Null comparisons count as no match, so negated terms keep null rows.
    Ok(predicate.fill_null(lit(false)))
}

fn compare(left: Expr, comparison: Comparison, right: Expr) -> Expr {
    match comparison {
        Comparison::Equal => left.eq(right),
        Comparison::Greater => left.gt(right),
        Comparison::GreaterOrEqual => left.gt_eq(right),
        Comparison::Less => left.lt(right),
        Comparison::LessOrEqual => left.lt_eq(right),
    }
}

/// `left <op> right` for two columns. Text compares case-insensitively like
/// other search terms; otherwise the right side is cast to the left's type, and
/// values that cannot be cast never match.
fn column_comparison(left: &ColumnSchema, right: &ColumnSchema, comparison: Comparison) -> Expr {
    let (left_value, right_value) =
        if string_type_name(&left.sql_type) && string_type_name(&right.sql_type) {
            (
                col(left.name.as_str()).cast(DataType::String).str().to_lowercase(),
                col(right.name.as_str()).cast(DataType::String).str().to_lowercase(),
            )
        } else if left.dtype() == right.dtype()
            || (is_numeric_type(&left.sql_type) && is_numeric_type(&right.sql_type))
        {
            (col(left.name.as_str()), col(right.name.as_str()))
        } else {
            (
                col(left.name.as_str()),
                col(right.name.as_str()).cast(left.dtype().clone()),
            )
        };
    compare(left_value, comparison, right_value).fill_null(lit(false))
}

fn is_date_type(dtype: &DataType) -> bool {
    matches!(dtype, DataType::Date | DataType::Datetime(..))
}

/// Case-insensitive text comparison where a value stands for every text that
/// starts with it: `<=b` keeps `b…`, and `>b` skips it, so `china..italy`
/// includes `Italy (south)`.
fn text_predicate(rendered: Expr, comparison: Comparison, value: &str) -> Expr {
    let starts = || rendered.clone().str().starts_with(lit(value.to_owned()));
    match comparison {
        Comparison::LessOrEqual => rendered.clone().lt_eq(lit(value.to_owned())).or(starts()),
        Comparison::Greater => rendered.clone().gt(lit(value.to_owned())).and(starts().not()),
        comparison => compare(rendered.clone(), comparison, lit(value.to_owned())),
    }
}

/// A date or timestamp column as local wall-clock time, so a time zone column
/// compares against dates in its own zone.
fn local_time(column: &ColumnSchema) -> Expr {
    let value = col(column.name.as_str());
    match &column.dtype() {
        DataType::Datetime(_, Some(_)) => value
            .dt()
            .replace_time_zone(None, lit("raise"), NonExistent::Raise),
        _ => value,
    }
}

fn datetime_literal(moment: chrono::NaiveDateTime) -> Expr {
    lit(moment.and_utc().timestamp_micros())
        .cast(DataType::Datetime(TimeUnit::Microseconds, None))
}

fn parse_datetime(text: &str) -> Option<chrono::NaiveDateTime> {
    let text = text.trim().replace('T', " ");
    ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|format| chrono::NaiveDateTime::parse_from_str(&text, format).ok())
}

/// Date comparison against the days a partial date covers, so `=2022-10` is the
/// month and `<=2024-02-01` keeps every timestamp on that day.
fn date_predicate(
    column: &ColumnSchema,
    comparison: Comparison,
    (start, end): (chrono::NaiveDate, chrono::NaiveDate),
) -> Expr {
    let bound = |date: chrono::NaiveDate| match column.dtype() {
        DataType::Date => lit(date),
        _ => datetime_literal(date.and_hms_opt(0, 0, 0).expect("midnight")),
    };
    let value = local_time(column);
    match comparison {
        Comparison::Equal => value.clone().gt_eq(bound(start)).and(value.lt(bound(end))),
        Comparison::GreaterOrEqual => value.gt_eq(bound(start)),
        Comparison::Greater => value.gt_eq(bound(end)),
        Comparison::Less => value.lt(bound(start)),
        Comparison::LessOrEqual => value.lt(bound(end)),
    }
}

fn numeric_predicate(column: &ColumnSchema, number: &Numeric, comparison: Comparison) -> Expr {
    let value = col(column.name.as_str());
    let beyond_range = |literal_above: bool| {
        if comparison.holds_beyond_range(literal_above) {
            value.clone().is_not_null()
        } else {
            lit(false)
        }
    };
    let float = |number: f64| compare(value.clone().cast(DataType::Float64), comparison, lit(number));
    let signed = matches!(
        column.dtype(),
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 | DataType::Int128
    );
    let unsigned = matches!(
        column.dtype(),
        DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
    );
    let integer = |literal: Expr| {
        compare(
            value.clone().cast(DataType::Int128),
            comparison,
            literal.cast(DataType::Int128),
        )
    };
    if signed || unsigned {
        match number {
            Numeric::Signed(literal) if unsigned && *literal < 0 => beyond_range(false),
            Numeric::Signed(literal) => integer(lit(*literal)),
            Numeric::Unsigned(literal) => match i128::try_from(*literal) {
                Ok(literal) => integer(lit(literal)),
                Err(_) if signed => beyond_range(true),
                Err(_) => compare(
                    value.clone().cast(DataType::UInt128),
                    comparison,
                    lit(*literal).cast(DataType::UInt128),
                ),
            },
            Numeric::Float(literal) => float(*literal),
        }
    } else {
        float(match number {
            Numeric::Signed(value) => *value as f64,
            Numeric::Unsigned(value) => *value as f64,
            Numeric::Float(value) => *value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::SortKey;
    use crate::domain::query::parse;

    /// A store holding what Polars SQL `sql` returns.
    fn store(sql: &str) -> PolarsStore {
        let frame = polars::sql::SQLContext::new()
            .execute(sql)
            .and_then(LazyFrame::collect)
            .unwrap();
        PolarsStore::from_frame(frame)
    }

    fn search(store: &PolarsStore, text: &str) -> Result<Vec<i64>, String> {
        store.query_row_ids(&ViewQuery {
            tokens: parse(text)?,
            ..ViewQuery::default()
        })
    }

    fn condition(store: &PolarsStore, column: &str, text: &str, sql: bool) -> Result<Vec<i64>, String> {
        store.query_row_ids(&ViewQuery {
            filters: vec![ColumnFilter {
                sql,
                column: column.into(),
                values: vec![ScalarKey::String("ignored".into())],
                condition: Some(text.into()),
            }],
            ..ViewQuery::default()
        })
    }

    #[test]
    fn unsorted_tables_read_by_position_without_listing_rows() {
        let store = PolarsStore::synthetic(5).unwrap();
        let rows = store.query_view_rows(&ViewQuery::default()).unwrap();
        assert!(matches!(rows.row_ids, RowIds::Natural(5)));
        let page = store
            .fetch_view_page(&rows.row_ids, 1, 2)
            .unwrap()
            .into_iter()
            .map(|row| row.values[0].clone().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(page, ["1", "2"]);
        assert!(store.fetch_view_page(&rows.row_ids, 5, 2).unwrap().is_empty());
        let (aggregates, unique) = store
            .selection_column_aggregates(&rows.row_ids, 1, 4, &[0], &["BIGINT".into()])
            .unwrap();
        assert_eq!(unique, 3);
        assert_eq!(aggregates[0].integer_sum.as_deref(), Some("6"));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rows.csv");
        let written = store
            .export_view(&rows.row_ids, &[0], &path, crate::domain::export::FileFormat::Csv)
            .unwrap();
        assert_eq!(written, 5);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n"),
            "Row\n0\n1\n2\n3\n4\n"
        );
        let widths = store
            .width_candidates(Some(&rows.row_ids), &[("Row".into(), None)])
            .unwrap();
        assert!(widths[0].1.contains(&"4".to_owned()));
        let sorted = store
            .query_view_rows(&ViewQuery {
                sort: vec![SortKey {
                    column: "Row".into(),
                    descending: true,
                }],
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(sorted.row_ids.to_vec(), [4, 3, 2, 1, 0]);
    }

    #[test]
    fn width_previews_quote_names_and_cut_long_text() {
        let frame = df!(
            "a\"b" => [0i64, 2, 4],
            "long" => ["界".repeat(500), "x".into(), "y".into()],
        )
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let columns = vec![("a\"b".into(), None), ("long".into(), None)];
        let sampled = store.width_candidates(None, &columns).unwrap();
        assert_eq!(sampled[0].1, ["0", "2", "4"]);
        assert!(
            sampled[1]
                .1
                .iter()
                .any(|value| value.chars().count() == 201 && value.ends_with('…')),
            "{:?}",
            sampled[1].1
        );
        let subset = store
            .width_candidates(Some(&RowIds::Listed(Arc::from([2]))), &columns[..1])
            .unwrap();
        assert_eq!(subset[0].1, ["4"]);
        assert_eq!(store.width_sampling_calls(), 2);
        assert!(store.width_candidates(None, &[]).unwrap().is_empty());
        assert_eq!(store.width_sampling_calls(), 2);
    }

    #[test]
    fn formatting_statistics_cover_the_rows_the_filters_leave() {
        let store = PolarsStore::from_frame(
            df!(
                "region" => [Some("North"), Some("South"), Some("North"), None, Some("North")],
                "n" => [Some(10i64), None, Some(30), Some(5), Some(10)],
            )
            .unwrap(),
        );
        let north = ViewQuery {
            filters: vec![ColumnFilter {
                column: "region".into(),
                values: vec![crate::domain::documents::ScalarKey::String("North".into())],
                condition: None,
                sql: false,
            }],
            ..ViewQuery::default()
        };
        use diver_format::Aggregate;
        let requests = [Aggregate::Min, Aggregate::Max, Aggregate::Count, Aggregate::Distinct, Aggregate::Median]
            .map(|aggregate| ("n".to_string(), aggregate));
        assert_eq!(
            store.column_aggregates(&north, &requests).unwrap(),
            [Some(10.0), Some(30.0), Some(3.0), Some(2.0), Some(10.0)]
        );
        let mut counts = store.value_counts(&north, "n", &["10".into(), "30".into(), "5".into()]).unwrap();
        counts.sort();
        // 5 is only in a row the filter leaves out.
        assert_eq!(counts, [("10".to_string(), 2), ("30".to_string(), 1)]);
        assert_eq!(
            store.value_positions(&north, "n", &[10.0]).unwrap(),
            [diver_format::stats::Position { above: 1, below: 0, count: 3 }]
        );
        assert!(store.value_counts(&north, "n", &[]).unwrap().is_empty());
    }

    #[test]
    fn column_summaries_count_nulls_and_distinct_values_and_skip_nested_ranges() {
        let mut frame = df!(
            "region" => [Some("North"), Some("South"), Some("North"), None],
            "n" => [Some(10i64), None, Some(30), Some(5)],
        )
        .unwrap();
        frame
            .with_column(
                Series::new(
                    "list".into(),
                    [
                        Some(Series::new("".into(), [1i32])),
                        Some(Series::new("".into(), [2i32])),
                        None,
                        Some(Series::new("".into(), [1i32])),
                    ],
                )
                .into_column(),
            )
            .unwrap();
        let store = PolarsStore::from_frame(frame);
        let summaries = store
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
        assert_eq!((summaries[1].nulls, summaries[1].min.as_deref()), (1, Some("5")));
        assert_eq!((summaries[2].unique, summaries[2].min.as_ref()), (2, None));
        assert_eq!(
            store.top_values("region", 5).unwrap(),
            [(Some("North".into()), 2), (Some("South".into()), 1), (None, 1)]
        );
        assert_eq!(store.unique_counts(&["n".into()]).unwrap(), [("n".into(), 3)]);
    }

    #[test]
    fn search_is_case_insensitive_literal_and_sort_keeps_source_order_for_ties() {
        let store = store(
            "SELECT * FROM (VALUES (3, 'Smith', 'C++'), (1, 'smith', NULL), (2, 'Jones', 'C#'), (1, 'Brown', 'C++')) AS t(employee_id, name, lang)",
        );
        assert_eq!(search(&store, "smith").unwrap(), [0, 1]);
        assert_eq!(search(&store, "c++").unwrap(), [0, 3]);
        assert_eq!(search(&store, "missing").unwrap(), Vec::<i64>::new());
        assert_eq!(search(&store, "smith c++ | jones").unwrap(), [0, 2]);
        assert_eq!(search(&store, "brown | employee_id>2 | missing").unwrap(), [0, 3]);
        let sorted = store
            .query_row_ids(&ViewQuery {
                sort: vec![SortKey {
                    column: "employee_id".into(),
                    descending: false,
                }],
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(sorted, [1, 3, 2, 0]);
        let page = store.fetch_row_ids(&sorted[..2]).unwrap();
        assert_eq!(page.iter().map(|row| row.rowid).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(page[0].values[1].as_deref(), Some("smith"));
        let descending = store
            .query_row_ids(&ViewQuery {
                tokens: parse("smith").unwrap(),
                sort: vec![SortKey {
                    column: "employee_id".into(),
                    descending: true,
                }],
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(descending, [0, 1]);
    }

    #[test]
    fn letterless_search_matches_text_as_is_and_keeps_source_order() {
        let store = store(
            "SELECT * FROM (VALUES ('1998 Winter', NULL), ('ÉTÉ 1996', 'x'), ('Spring', 'A-1998')) AS t(games, code)",
        );
        assert_eq!(search(&store, "1998").unwrap(), [0, 2]);
        assert_eq!(search(&store, "1996").unwrap(), [1]);
        assert_eq!(search(&store, "été").unwrap(), [1]);
        assert_eq!(search(&store, "a-1998").unwrap(), [2]);
        // Enough rows for the engine to split the work, which must not reorder it.
        let many = PolarsStore::synthetic(1_000_000).unwrap();
        let ids = search(&many, "7").unwrap();
        assert_eq!(ids.len(), 468_559);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn search_skips_columns_whose_rendering_cannot_contain_the_text() {
        let store = store(
            "SELECT * FROM (VALUES (1, -12.5, true, 'Label A'), (-7, 0.5, false, 'x')) AS t(i, f, b, s)",
        );
        assert_eq!(search(&store, "7").unwrap(), [1]);
        // A leading minus excludes.
        assert_eq!(search(&store, "-7").unwrap(), [0]);
        assert_eq!(search(&store, "12.5").unwrap(), [0]);
        assert_eq!(search(&store, "true").unwrap(), [0]);
        assert_eq!(search(&store, "label a").unwrap(), [0]);
        // Hidden columns are skipped by plain search but reached by a column term.
        let hidden = store
            .query_row_ids(&ViewQuery {
                tokens: parse("label").unwrap(),
                hidden: vec!["s".into()],
                ..ViewQuery::default()
            })
            .unwrap();
        assert!(hidden.is_empty());
        let scoped = store
            .query_row_ids(&ViewQuery {
                tokens: parse("s:label").unwrap(),
                hidden: vec!["s".into()],
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(scoped, [0]);
    }

    #[test]
    fn column_scoped_search_matches_one_column_by_type() {
        let frame = df!(
            "Sales Region" => [Some("North"), Some("South"), None, Some("West")],
            "city" => ["Southfield", "Northgate", "Eastview", "O'Brien"],
            "qty" => [Some(5i64), Some(15), None, Some(25)],
            "order date" => [
                chrono::NaiveDate::from_ymd_opt(2024, 1, 10),
                chrono::NaiveDate::from_ymd_opt(2024, 3, 1),
                None,
                chrono::NaiveDate::from_ymd_opt(2023, 12, 31),
            ],
            "price" => [Some("1.50"), Some("10.00"), None, Some("2.25")],
        )
        .unwrap()
        .lazy()
        .with_column(col("price").cast(DataType::Decimal(4, 2)))
        .collect()
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let search = |text: &str| search(&store, text);
        // Scoped contains ignores other columns; prefixes and quoted names resolve.
        assert_eq!(search("\"sales region\":north").unwrap(), [0]);
        assert_eq!(search("sales:NORTH").unwrap(), [0]);
        assert_eq!(search("city:north").unwrap(), [1]);
        // Equality is case-insensitive text on text columns and numeric on numbers.
        assert_eq!(search("city=o'brien").unwrap(), [3]);
        assert_eq!(search("price=10").unwrap(), [1]);
        assert_eq!(search("qty>=15").unwrap(), [1, 3]);
        assert_eq!(search("qty<15 qty>0").unwrap(), [0]);
        // Dates compare against the days a partial date covers.
        assert_eq!(search("order>=2024-01-10").unwrap(), [0, 1]);
        assert_eq!(search("order<2024-01-01").unwrap(), [3]);
        // Negation keeps rows whose value is null.
        assert_eq!(search("-sales:north").unwrap(), [1, 2, 3]);
        assert_eq!(search("qty!=15").unwrap(), [0, 2, 3]);
        // Unknown columns search the whole word as text.
        assert_eq!(search("nope:north").unwrap(), Vec::<i64>::new());
        assert!(search("qty>abc").is_err());
    }

    #[test]
    fn double_quoted_column_names_compare_two_columns() {
        let frame = df!(
            "small bags" => [Some(5i64), Some(20), None],
            "large bags" => [10i64, 10, 3],
            "fruit" => ["apple", "pear", "kiwi"],
            "other" => ["APPLE", "fig", "kiwi"],
            "code" => ["7", "x", "20"],
        )
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let search = |text: &str| search(&store, text);
        assert_eq!(search("\"small bags\">\"large bags\"").unwrap(), [1]);
        assert_eq!(search("\"small bags\"<=\"large bags\"").unwrap(), [0]);
        assert_eq!(search("\"small bags\"!=\"large bags\"").unwrap(), [0, 1, 2]);
        assert_eq!(search("fruit=\"other\"").unwrap(), [0, 2]);
        assert_eq!(search("\"small bags\"<\"code\"").unwrap(), [0]);
        assert_eq!(search("fruit=\"pear\"").unwrap(), [1]);
        assert_eq!(condition(&store, "large bags", ">=\"small bags\"", false).unwrap(), [0]);
    }

    #[test]
    fn condition_and_sql_filters_cover_ranges_text_dates_and_report_errors() {
        let today = chrono::Local::now().date_naive();
        let frame = df!(
            "region" => [Some("North"), Some("South"), None, Some("Northwest")],
            "qty" => [Some(5i64), Some(15), None, Some(25)],
            "day" => [
                chrono::NaiveDate::from_ymd_opt(2024, 1, 10),
                chrono::NaiveDate::from_ymd_opt(2024, 3, 1),
                None,
                Some(today),
            ],
        )
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let check = |column: &str, text: &str| condition(&store, column, text, false);
        assert_eq!(check("qty", "10..25").unwrap(), [1, 3]);
        assert_eq!(check("qty", ">5 <25").unwrap(), [1]);
        assert_eq!(check("qty", "!=15").unwrap(), [0, 2, 3]);
        assert_eq!(check("region", "north -west").unwrap(), [0]);
        assert_eq!(check("region", "=SOUTH").unwrap(), [1]);
        assert_eq!(check("day", "2024-01-01..2024-02-01").unwrap(), [0]);
        assert_eq!(check("day", "-7d..today").unwrap(), [3]);
        assert_eq!(check("day", "2024-01..2024-02").unwrap(), [0]);
        assert_eq!(check("day", "=2024-03").unwrap(), [1]);
        assert_eq!(check("day", ">2024-01 <=2024").unwrap(), [1]);
        assert!(check("day", ">=north").is_err());
        // Text ranges ignore case and keep values starting with the upper bound.
        assert_eq!(check("region", "north..NORTH").unwrap(), [0, 3]);
        assert_eq!(check("region", "a..n").unwrap(), [0, 3]);
        assert_eq!(check("region", ">north").unwrap(), [1]);
        assert_eq!(check("region", "<north").unwrap(), Vec::<i64>::new());
        // Blank text does not filter, even though `values` is set.
        assert_eq!(check("qty", "  ").unwrap(), [0, 1, 2, 3]);
        assert!(check("qty", ">=abc").is_err());
        // An SQL filter sees every column; its own column is unused.
        assert_eq!(
            condition(&store, "", "qty % 10 = 5 AND region <> 'North'", true).unwrap(),
            [1, 3]
        );
        assert_eq!(condition(&store, "", "rowid < 2", true).unwrap(), [0, 1]);
        assert_eq!(condition(&store, "", " ", true).unwrap(), [0, 1, 2, 3]);

        assert!(store.validate_condition("qty", ">=10").is_ok());
        assert!(store.validate_condition("qty", "").is_ok());
        assert!(store.validate_condition("qty", ">=abc").is_err());
        assert!(store.validate_condition("missing", ">1").is_err());
        assert!(store.validate_sql("qty > 1 OR region = 'x'").is_ok());
        assert!(store.validate_sql("").is_ok());
        let error = store.validate_sql("missing_column > 1").unwrap_err();
        assert!(error.contains("missing_column"), "{error}");
        assert!(!error.contains('\n'));
        assert!(store.validate_sql("qty + 1").unwrap_err().contains("true or false"));

        assert_eq!(store.column_bounds("qty").unwrap(), Some(("5".to_owned(), "25".to_owned())));
        let empty = PolarsStore::from_frame(df!("n" => [None::<i64>]).unwrap());
        assert_eq!(empty.column_bounds("n").unwrap(), None);
    }

    #[test]
    fn numeric_comparisons_include_bounds_and_out_of_range_literals() {
        let frame = df!(
            "u" => [1u8, 2],
            "s" => [-5i8, 7],
            "big" => [0u64, u64::MAX],
        )
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let search = |text: &str| search(&store, text).unwrap();
        assert_eq!(search(">=7"), [1]);
        assert_eq!(search("<=1"), [0]);
        assert_eq!(search("u>=-1"), [0, 1]);
        assert_eq!(search("u=-1"), Vec::<i64>::new());
        assert_eq!(search("s<=340282366920938463463374607431768211456"), [0, 1]);
        assert_eq!(search("s=340282366920938463463374607431768211456"), Vec::<i64>::new());
        assert_eq!(search("big>9223372036854775807"), [1]);
        assert_eq!(search("big>18446744073709551615"), Vec::<i64>::new());
    }

    #[test]
    fn value_cards_bind_values_handle_null_and_can_return_zero_rows() {
        let store = PolarsStore::from_frame(
            df!("department" => [Some("A"), Some("B"), None], "n" => [1i64, 2, 3]).unwrap(),
        );
        let rows = |values: Vec<ScalarKey>, tokens: &str| {
            store
                .query_row_ids(&ViewQuery {
                    tokens: parse(tokens).unwrap(),
                    filters: vec![ColumnFilter {
                        sql: false,
                        condition: None,
                        column: "department".into(),
                        values,
                    }],
                    ..ViewQuery::default()
                })
                .unwrap()
        };
        assert_eq!(rows(vec![ScalarKey::String("B".into()), ScalarKey::Null], ">1"), [1, 2]);
        assert!(rows(vec![ScalarKey::String("absent' OR 1=1 --".into())], "").is_empty());
        let numbers = store
            .query_row_ids(&ViewQuery {
                filters: vec![ColumnFilter {
                    sql: false,
                    condition: None,
                    column: "n".into(),
                    values: vec![ScalarKey::Signed("3".into())],
                }],
                ..ViewQuery::default()
            })
            .unwrap();
        assert_eq!(numbers, [2]);
    }

    #[test]
    fn value_filters_match_displayed_text_exclude_keeps_nulls_and_accept_expressions() {
        let frame = df!(
            "department" => [Some("A"), Some("B"), None],
            "amount" => [Some(1.5f64), Some(2.0), None],
            "day" => [
                chrono::NaiveDate::from_ymd_opt(2024, 1, 31).unwrap(),
                chrono::NaiveDate::from_ymd_opt(2024, 2, 1).unwrap(),
                chrono::NaiveDate::from_ymd_opt(2024, 2, 15).unwrap(),
            ],
        )
        .unwrap();
        let store = PolarsStore::from_frame(frame);
        let ids = |value_filters: Vec<ValueFilter>| {
            store
                .query_row_ids(&ViewQuery {
                    value_filters,
                    ..Default::default()
                })
                .unwrap()
        };
        let filter = |target: Dimension, value: Option<&str>, exclude: bool| ValueFilter {
            target,
            values: vec![value.map(str::to_owned)],
            exclude,
        };
        // The displayed rendering of each cell is what the filter matches.
        let amount = store.fetch_row_ids(&[1]).unwrap()[0].values[1].clone();
        assert_eq!(amount.as_deref(), Some("2.0"));
        assert_eq!(ids(vec![filter("amount".into(), amount.as_deref(), false)]), [1]);
        assert_eq!(ids(vec![filter("department".into(), Some("A"), true)]), [1, 2]);
        assert_eq!(ids(vec![filter("department".into(), None, false)]), [2]);
        assert_eq!(ids(vec![filter("department".into(), None, true)]), [0, 1]);
        assert_eq!(
            ids(vec![
                filter("department".into(), Some("A"), true),
                filter("department".into(), Some("B"), true),
            ]),
            [2]
        );
        let many = |values: &[Option<&str>], exclude: bool| ValueFilter {
            target: "department".into(),
            values: values.iter().map(|value| value.map(str::to_owned)).collect(),
            exclude,
        };
        assert_eq!(ids(vec![many(&[Some("A"), Some("B")], false)]), [0, 1]);
        assert_eq!(ids(vec![many(&[Some("A"), None], false)]), [0, 2]);
        // Excluding values keeps NULL unless NULL is among them.
        assert_eq!(ids(vec![many(&[Some("A"), Some("B")], true)]), [2]);
        assert_eq!(ids(vec![many(&[Some("A"), None], true)]), [1]);
        assert_eq!(
            ids(vec![filter(
                Dimension::expression("month(day) AS month"),
                Some("2"),
                false,
            )]),
            [1, 2]
        );
        assert!(ids(vec![filter("department".into(), Some("A' OR 1=1 --"), false)]).is_empty());
        assert!(
            store
                .query_row_ids(&ViewQuery {
                    value_filters: vec![filter("missing".into(), Some("x"), false)],
                    ..Default::default()
                })
                .is_err()
        );
    }

    #[test]
    fn slicer_values_are_typed_sorted_and_narrowed_by_filters() {
        let store = PolarsStore::from_frame(
            df!("region" => [Some("West"), Some("East"), None, Some("East")], "n" => [3i64, 1, 2, 4]).unwrap(),
        );
        let values = store.slicer_values("region", &[], &[]).unwrap();
        assert_eq!(
            values.iter().map(|value| value.label.as_str()).collect::<Vec<_>>(),
            ["East", "West", "(Blank)"]
        );
        let narrowed = store
            .slicer_values(
                "region",
                &[ColumnFilter {
                    sql: true,
                    condition: Some("n > 2".into()),
                    column: String::new(),
                    values: Vec::new(),
                }],
                &[],
            )
            .unwrap();
        assert_eq!(
            narrowed.iter().map(|value| value.key.clone()).collect::<Vec<_>>(),
            [ScalarKey::String("East".into()), ScalarKey::String("West".into())]
        );
        let numbers = store.slicer_values("n", &[], &[]).unwrap();
        assert_eq!(numbers[0].key, ScalarKey::Signed("1".into()));
    }
}
