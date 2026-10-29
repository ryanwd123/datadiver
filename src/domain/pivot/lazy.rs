//! Group-by, pivot, and totals for Polars stores, built with the lazy API.

use chrono::{DateTime, Duration, NaiveDate};
use polars::prelude::*;

use crate::domain::documents::{
    Aggregation, AggregationOp, Condition, ConditionOp, ConditionValue, Dimension, Interpolation,
    QueryConfig, ScalarKey, SlicerDefinition, ValueFilter,
};
use crate::domain::schema::ColumnType;
use crate::domain::sql::SqlTrace;
use super::sql::EvaluationClock;
pub use super::sql::{TotalsRow, is_total_column};
use crate::infrastructure::polars::expressions::{
    display_name, parse_sql_expression, polars_error, quote_identifier, rendered,
    scalar_series,
};
use crate::infrastructure::polars::store::{PolarsStore, dimension_expression};
use crate::infrastructure::store::{ColumnFilter, ColumnSchema, ViewQuery, filters_mention_rowid};

pub fn execute(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
) -> Result<PolarsStore, String> {
    execute_with_clock(source, config, slicers, &EvaluationClock::system())
}

pub fn execute_with_clock(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
) -> Result<PolarsStore, String> {
    execute_traced(source, config, slicers, clock, &mut SqlTrace::default())
}

pub fn execute_traced(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    trace: &mut SqlTrace,
) -> Result<PolarsStore, String> {
    execute_traced_limited(
        source,
        config,
        slicers,
        clock,
        crate::domain::documents::DEFAULT_QUERY_LIMIT,
        trace,
    )
}

pub fn execute_traced_limited(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<PolarsStore, String> {
    execute_filtered(source, config, slicers, &[], clock, limit, trace)
}

/// Runs the query over source rows matching both the slicers and the value filters.
pub fn execute_filtered(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<PolarsStore, String> {
    execute_query(source, config, slicers, filters, clock, limit, None, trace)
}

/// The query's result rows, narrowed by a view over a previous result before the
/// limit applies: `store` is that previous result, giving the result schema.
pub type ResultView<'a> = (&'a PolarsStore, &'a ViewQuery);

/// Like [`execute_filtered`], but with `result_view` the result search and sort
/// run inside the query, so they see every result row instead of only the
/// first `limit`.
#[allow(clippy::too_many_arguments)]
pub fn execute_query(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    result_view: Option<ResultView<'_>>,
    trace: &mut SqlTrace,
) -> Result<PolarsStore, String> {
    if limit == 0 || limit > i64::MAX as u64 {
        return Err("query limit must be between 1 and 9223372036854775807".into());
    }
    validate_columns(source.schema(), config)?;
    if !config.pivot_by.is_empty() && config.group_by.is_empty() {
        return Err("Add a group by to set up this pivot".into());
    }
    let slicer_filters = slicer_filters(slicers);
    if config.group_by.is_empty() && config.aggregations.is_empty() {
        if source.is_database_view() {
            let columns = source
                .schema()
                .iter()
                .map(|column| col(column.name.as_str()))
                .collect::<Vec<_>>();
            let frame = source
                .filtered_frame(&slicer_filters, filters)?
                .select(columns);
            let frame = limited(frame, limit, result_view)?;
            return source.materialize("Scanned file rows", frame, trace);
        }
        return Err("empty query configuration must use table mode".into());
    }
    let dimensions = bind_dimensions(source, config)?;
    let rowid = filters_mention_rowid(&slicer_filters, filters)
        || config
            .group_by
            .iter()
            .chain(&config.pivot_by)
            .any(dimension_mentions_rowid)
        || aggregations_read_rowid(config);
    let mut frame = source.query_frame(rowid);
    if let Some(predicate) = source.filter_predicate(&slicer_filters, filters)? {
        frame = frame.filter(predicate);
    }
    let measures = config
        .aggregations
        .iter()
        .map(|aggregation| measure(source.schema(), aggregation, clock))
        .collect::<Result<Vec<_>, _>>()?;
    let group_count = config.group_by.len();
    let (groups, pivots) = dimensions.split_at(group_count);
    let result = if pivots.is_empty() {
        grouped(frame, groups, &measures)
    } else {
        pivoted(source, frame, groups, pivots, &measures, config.totals)?
    };
    let result = limited(result, limit, result_view)?;
    source.materialize("Query materialization", result, trace)
}

/// One row per group, ordered by the groups, or one row in all without them.
fn grouped(frame: LazyFrame, groups: &[BoundDimension], measures: &[Measure]) -> LazyFrame {
    let keys = groups.iter().map(BoundDimension::key).collect::<Vec<_>>();
    let aggregates = measures
        .iter()
        .map(|measure| measure.expression.clone())
        .collect::<Vec<_>>();
    if keys.is_empty() {
        return frame.select(aggregates);
    }
    let frame = if aggregates.is_empty() {
        frame
            .select(keys)
            .unique_stable(None, UniqueKeepStrategy::Any)
    } else {
        frame.group_by(keys).agg(aggregates)
    };
    sort_groups(frame, groups)
}

fn sort_groups(frame: LazyFrame, groups: &[BoundDimension]) -> LazyFrame {
    if groups.is_empty() {
        return frame;
    }
    frame.sort_by_exprs(
        groups
            .iter()
            .map(|group| col(group.name.as_str()))
            .collect::<Vec<_>>(),
        SortMultipleOptions::default()
            .with_nulls_last(true)
            .with_maintain_order(true),
    )
}

/// The group rows with one column per pivot value and measure. Pivot values
/// are read first, so the columns are known; a column is named by its values
/// joined with `_`, then `_` and the measure name when there are several
/// measures or the measure is named.
fn pivoted(
    source: &PolarsStore,
    frame: LazyFrame,
    groups: &[BoundDimension],
    pivots: &[BoundDimension],
    measures: &[Measure],
    totals: bool,
) -> Result<LazyFrame, String> {
    let present = [Measure {
        expression: lit("").alias("__dd_present"),
        name: "__dd_present".into(),
        named: false,
    }];
    let cells = if measures.is_empty() { &present[..] } else { measures };
    let internal = |index: usize| format!("__dd_measure_{index}");
    let keys = groups
        .iter()
        .chain(pivots)
        .map(BoundDimension::key)
        .collect::<Vec<_>>();
    let aggregates = cells
        .iter()
        .enumerate()
        .map(|(index, measure)| measure.expression.clone().alias(internal(index)))
        .collect::<Vec<_>>();
    let mut cell_frame = source.collect(frame.clone().group_by(keys).agg(aggregates))?;
    let schema = cell_frame.schema().clone();
    let pivot_text = pivots
        .iter()
        .map(|pivot| {
            let dtype = schema.get(pivot.name.as_str()).cloned().unwrap_or(DataType::Null);
            rendered(col(pivot.name.as_str()), &dtype).fill_null(lit("NULL"))
        })
        .collect::<Vec<_>>();
    cell_frame = cell_frame
        .lazy()
        .with_column(concat_str(pivot_text, "_", false).alias("__dd_label"))
        .collect()
        .map_err(polars_error)?;
    let labels = cell_frame
        .clone()
        .lazy()
        .select(
            pivots
                .iter()
                .map(|pivot| col(pivot.name.as_str()))
                .chain([col("__dd_label")])
                .collect::<Vec<_>>(),
        )
        .unique(None, UniqueKeepStrategy::Any)
        .sort_by_exprs(
            pivots
                .iter()
                .map(|pivot| col(pivot.name.as_str()))
                .collect::<Vec<_>>(),
            SortMultipleOptions::default().with_nulls_last(true),
        )
        .select([col("__dd_label")])
        .collect()
        .map_err(polars_error)?;
    let labels = labels
        .column("__dd_label")
        .map_err(polars_error)?
        .str()
        .map_err(polars_error)?
        .iter()
        .map(|label| label.unwrap_or("NULL").to_owned())
        .collect::<Vec<_>>();
    let single = cells.len() == 1 && !cells[0].named;
    let mut names = groups
        .iter()
        .map(|group| group.name.clone())
        .collect::<Vec<_>>();
    let mut columns = Vec::new();
    for label in &labels {
        for (index, measure) in cells.iter().enumerate() {
            let name = if single {
                label.clone()
            } else {
                format!("{label}_{}", measure.name)
            };
            if names.iter().any(|taken| taken.eq_ignore_ascii_case(&name)) {
                return Err(format!(
                    "duplicate column name {name}: a pivot column matches another column; use AS to rename a dimension or measure"
                ));
            }
            names.push(name.clone());
            columns.push(
                col(internal(index))
                    .filter(col("__dd_label").eq(lit(label.clone())))
                    .first()
                    .alias(name),
            );
        }
    }
    let group_keys = groups
        .iter()
        .map(|group| col(group.name.as_str()))
        .collect::<Vec<_>>();
    let mut result = cell_frame.lazy().group_by(group_keys.clone()).agg(columns);
    // Per-row totals aggregate the source rows grouped without the pivot, so
    // means, medians, and distinct counts stay correct.
    if totals && !measures.is_empty() {
        let total_names = total_column_names(
            &measures
                .iter()
                .map(|measure| measure.name.clone())
                .collect::<Vec<_>>(),
        );
        for name in &total_names {
            if names.iter().any(|taken| taken.eq_ignore_ascii_case(name)) {
                return Err(format!("duplicate column name {name}"));
            }
        }
        let row_totals = frame
            .group_by(groups.iter().map(BoundDimension::key).collect::<Vec<_>>())
            .agg(
                measures
                    .iter()
                    .zip(&total_names)
                    .map(|(measure, name)| measure.expression.clone().alias(name.as_str()))
                    .collect::<Vec<_>>(),
            );
        result = result.join(
            row_totals,
            group_keys.clone(),
            group_keys,
            JoinArgs {
                nulls_equal: true,
                ..JoinArgs::new(JoinType::Left)
            },
        );
    }
    Ok(sort_groups(result, groups))
}

/// Whether SQL aggregations name `rowid`, which then numbers the source rows.
fn aggregations_read_rowid(config: &QueryConfig) -> bool {
    config.aggregations.iter().any(|aggregation| {
        aggregation
            .sql
            .as_deref()
            .is_some_and(crate::domain::aggregation_sql::mentions_rowid)
    })
}

fn dimension_mentions_rowid(dimension: &Dimension) -> bool {
    matches!(dimension, Dimension::Expression { sql }
        if crate::domain::aggregation_sql::mentions_rowid(sql))
}

/// Values by column name, rendered as text.

/// The grand total row of a query with totals: each total column's name and
/// its value over every source row the query reads. A plain group-by gives its
/// aggregate columns; a pivot also gives each pivot column and its `(Total)`
/// columns. Group columns have no total and are left out.
pub fn execute_totals(
    source: &PolarsStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
) -> Result<TotalsRow, String> {
    if config.group_by.is_empty() || config.aggregations.is_empty() {
        return Ok(Vec::new());
    }
    let grand = QueryConfig {
        aggregations: config.aggregations.clone(),
        ..Default::default()
    };
    let mut trace = SqlTrace::default();
    let mut totals = first_row(execute_filtered(
        source, &grand, slicers, filters, clock, 1, &mut trace,
    )?)?;
    if config.pivot_by.is_empty() {
        return Ok(totals);
    }
    let names = total_column_names(
        &totals
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    for ((name, _), total) in totals.iter_mut().zip(names) {
        *name = total;
    }
    // One constant group gives the pivot's columns summed over every group.
    let by_pivot = QueryConfig {
        group_by: vec![Dimension::Expression {
            sql: "'Total' AS __qp_total_row".into(),
        }],
        pivot_by: config.pivot_by.clone(),
        aggregations: config.aggregations.clone(),
        ..Default::default()
    };
    let mut row = first_row(execute_filtered(
        source, &by_pivot, slicers, filters, clock, 1, &mut trace,
    )?)?;
    if !row.is_empty() {
        row.remove(0);
    }
    row.extend(totals);
    Ok(row)
}

/// The first row's values by column name, rendered as text; `None` values when
/// there is no row.
fn first_row(store: PolarsStore) -> Result<TotalsRow, String> {
    let names = store
        .schema()
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let values = if store.row_count() > 0 {
        store
            .fetch_row_ids(&[0])?
            .into_iter()
            .next()
            .map(|row| row.values)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, values.get(index).cloned().flatten()))
        .collect())
}

const TOTAL_COLUMN: &str = "(Total)";

/// Names of a pivot's per-row total columns, from the aggregate output names.
fn total_column_names(aggregates: &[String]) -> Vec<String> {
    if aggregates.len() == 1 {
        vec![TOTAL_COLUMN.into()]
    } else {
        aggregates
            .iter()
            .map(|name| format!("{TOTAL_COLUMN}_{name}"))
            .collect()
    }
}

/// Keeps the first `limit` rows, applying `result_view` first when given.
fn limited(
    frame: LazyFrame,
    limit: u64,
    result_view: Option<ResultView<'_>>,
) -> Result<LazyFrame, String> {
    match result_view {
        Some((store, view)) => store.apply_view(view, frame, limit),
        None => Ok(frame.limit(limit.min(IdxSize::MAX as u64) as IdxSize)),
    }
}

struct BoundDimension {
    expression: Expr,
    name: String,
}

impl BoundDimension {
    fn key(&self) -> Expr {
        self.expression.clone().alias(self.name.as_str())
    }
}

fn bind_dimensions(source: &PolarsStore, config: &QueryConfig) -> Result<Vec<BoundDimension>, String> {
    let mut bound: Vec<BoundDimension> = Vec::new();
    for dimension in config.group_by.iter().chain(&config.pivot_by) {
        let entry = match dimension {
            Dimension::Column(name) => {
                let column = schema_column(source.schema(), name)?;
                BoundDimension {
                    expression: col(column.name.as_str()),
                    name: column.name.clone(),
                }
            }
            Dimension::Expression { sql } => {
                let name = source.dimension_name(sql)?;
                let (expression, _) = dimension_expression(sql)?;
                BoundDimension { expression, name }
            }
        };
        if bound
            .iter()
            .any(|previous| previous.name.eq_ignore_ascii_case(&entry.name))
        {
            return Err(format!(
                "duplicate dimension output name: {}. Use AS to choose a different name",
                entry.name
            ));
        }
        bound.push(entry);
    }
    Ok(bound)
}

pub fn validate_dimensions(source: &PolarsStore, config: &QueryConfig) -> Result<(), String> {
    bind_dimensions(source, config).map(|_| ())
}

pub fn dimension_editor_expression(
    dimension: &Dimension,
    schema: &[ColumnSchema],
) -> Result<String, String> {
    match dimension {
        Dimension::Column(name) => Ok(quote_identifier(&schema_column(schema, name)?.name)),
        Dimension::Expression { sql } => Ok(sql.clone()),
    }
}

fn schema_column<'a>(schema: &'a [ColumnSchema], name: &str) -> Result<&'a ColumnSchema, String> {
    schema
        .iter()
        .find(|column| column.name == name)
        .ok_or_else(|| format!("column {name} does not exist in the source schema"))
}

fn validate_columns(schema: &[ColumnSchema], config: &QueryConfig) -> Result<(), String> {
    for dimension in config.group_by.iter().chain(&config.pivot_by) {
        dimension.validate()?;
        if let Some(name) = dimension.column() {
            schema_column(schema, name)?;
        }
    }
    for aggregation in &config.aggregations {
        aggregation.validate_expression(!config.pivot_by.is_empty())?;
        if aggregation.sql.is_some() {
            continue;
        }
        schema_column(schema, &aggregation.source_column)?;
        for condition in &aggregation.conditions {
            schema_column(schema, &condition.column)?;
        }
    }
    Ok(())
}

fn slicer_filters(slicers: &[SlicerDefinition]) -> Vec<ColumnFilter> {
    slicers
        .iter()
        .map(|slicer| ColumnFilter {
            sql: slicer.sql,
            condition: slicer.condition.clone(),
            column: slicer.column.clone(),
            values: slicer.selected.clone(),
        })
        .collect()
}

/// An aggregation as an expression over a group's rows, with its output name.
struct Measure {
    expression: Expr,
    name: String,
    /// Named by the user, so a pivot names its columns `value_name` even
    /// when it is the only measure.
    named: bool,
}

fn measure(
    schema: &[ColumnSchema],
    aggregation: &Aggregation,
    clock: &EvaluationClock,
) -> Result<Measure, String> {
    let saved_name = aggregation.name.as_deref().filter(|name| !name.is_empty());
    let (expression, text) = match &aggregation.sql {
        Some(sql) => (parse_sql_expression(sql)?, sql.clone()),
        None => (
            structured_expression(schema, aggregation, clock)?,
            structured_sql(schema, aggregation, clock)?,
        ),
    };
    let (expression, alias) = match expression {
        Expr::Alias(inner, alias) => (inner.as_ref().clone(), Some(alias.to_string())),
        expression => (expression, None),
    };
    let named = saved_name.is_some() || alias.is_some();
    let name = saved_name
        .map(str::to_owned)
        .or(alias)
        .unwrap_or_else(|| display_name(&text));
    Ok(Measure {
        expression: expression.alias(name.as_str()),
        name,
        named,
    })
}

fn structured_expression(
    schema: &[ColumnSchema],
    aggregation: &Aggregation,
    clock: &EvaluationClock,
) -> Result<Expr, String> {
    let column = schema_column(schema, &aggregation.source_column)?;
    let condition = aggregation
        .conditions
        .iter()
        .map(|condition| condition_expression(schema, condition, clock))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .reduce(Expr::and);
    let value = match &condition {
        Some(condition) => col(column.name.as_str()).filter(condition.clone()),
        None => col(column.name.as_str()),
    };
    Ok(match aggregation.operation {
        // SQL sums are null, not zero, over no values.
        AggregationOp::Sum => when(value.clone().count().gt(lit(0)))
            .then(value.sum())
            .otherwise(lit(NULL)),
        AggregationOp::Mean => value.mean(),
        AggregationOp::Median => value.median(),
        AggregationOp::Minimum => value.min(),
        AggregationOp::Maximum => value.max(),
        AggregationOp::RowCount => match condition {
            Some(condition) => condition.fill_null(lit(false)).sum(),
            None => len(),
        },
        AggregationOp::NonNullCount => value.count(),
        AggregationOp::DistinctNonNullCount => value.drop_nulls().n_unique(),
        AggregationOp::First => value.first(),
        AggregationOp::Last => value.last(),
        AggregationOp::StandardDeviation => value.std(aggregation.ddof.unwrap_or(1)),
        AggregationOp::Variance => value.var(aggregation.ddof.unwrap_or(1)),
        AggregationOp::Percentile => {
            let percentile = aggregation.percentile.unwrap_or(50.0);
            if !(0.0..=100.0).contains(&percentile) {
                return Err("percentile must be between 0 and 100".into());
            }
            let method = match aggregation
                .interpolation
                .as_ref()
                .unwrap_or(&Interpolation::Linear)
            {
                Interpolation::Linear => QuantileMethod::Linear,
                Interpolation::Nearest => QuantileMethod::Nearest,
                Interpolation::Lower => QuantileMethod::Lower,
                Interpolation::Higher => QuantileMethod::Higher,
                Interpolation::Midpoint => QuantileMethod::Midpoint,
            };
            value.quantile(lit(percentile / 100.0), method)
        }
        AggregationOp::ValuesList => value.implode(true),
        AggregationOp::UniqueValuesList => value.drop_nulls().unique_stable().implode(true),
        AggregationOp::SqlExpression => return Err("SQL expression is missing".into()),
    })
}

/// The SQL the aggregation editor shows for a structured aggregation, which
/// also names its column.
pub fn editor_expression(
    aggregation: &Aggregation,
    schema: &[ColumnSchema],
) -> Result<String, String> {
    let sql = match &aggregation.sql {
        Some(sql) => sql.clone(),
        None => structured_sql(schema, aggregation, &EvaluationClock::system())?,
    };
    // Saved views may still have a separate name; expose it as SQL in the editor.
    Ok(
        match aggregation.name.as_deref().filter(|name| !name.is_empty()) {
            Some(name) => format!("{sql}\nAS {}", quote_identifier(name)),
            None => sql,
        },
    )
}

fn structured_sql(
    schema: &[ColumnSchema],
    aggregation: &Aggregation,
    clock: &EvaluationClock,
) -> Result<String, String> {
    let column = quote_identifier(&schema_column(schema, &aggregation.source_column)?.name);
    let sample = aggregation.ddof.unwrap_or(1) != 0;
    let expression = match aggregation.operation {
        AggregationOp::Sum => format!("sum({column})"),
        AggregationOp::Mean => format!("avg({column})"),
        AggregationOp::Median => format!("median({column})"),
        AggregationOp::Minimum => format!("min({column})"),
        AggregationOp::Maximum => format!("max({column})"),
        AggregationOp::RowCount => "count(*)".to_owned(),
        AggregationOp::NonNullCount => format!("count({column})"),
        AggregationOp::DistinctNonNullCount => format!("count(DISTINCT {column})"),
        AggregationOp::First => format!("first({column})"),
        AggregationOp::Last => format!("last({column})"),
        AggregationOp::StandardDeviation if sample => format!("stddev_samp({column})"),
        // Polars SQL has only the sample statistics, so scale them to the population.
        AggregationOp::StandardDeviation => format!(
            "stddev_samp({column}) * sqrt((CAST(count({column}) AS DOUBLE) - 1) / count({column}))"
        ),
        AggregationOp::Variance if sample => format!("var_samp({column})"),
        AggregationOp::Variance => format!(
            "var_samp({column}) * (CAST(count({column}) AS DOUBLE) - 1) / count({column})"
        ),
        AggregationOp::Percentile => {
            let percentile = aggregation.percentile.unwrap_or(50.0);
            if !(0.0..=100.0).contains(&percentile) {
                return Err("percentile must be between 0 and 100".into());
            }
            let function = match aggregation
                .interpolation
                .as_ref()
                .unwrap_or(&Interpolation::Linear)
            {
                Interpolation::Linear | Interpolation::Midpoint => "quantile_cont",
                Interpolation::Nearest | Interpolation::Lower | Interpolation::Higher => {
                    "quantile_disc"
                }
            };
            format!("{function}({column}, {})", percentile / 100.0)
        }
        AggregationOp::ValuesList => format!("array_agg({column})"),
        AggregationOp::UniqueValuesList => format!("array_unique(array_agg({column}))"),
        AggregationOp::SqlExpression => return Err("SQL expression is missing".into()),
    };
    let conditions = aggregation
        .conditions
        .iter()
        .map(|condition| condition_sql(schema, condition, clock))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(if conditions.is_empty() {
        expression
    } else {
        format!("{expression} FILTER (WHERE {})", conditions.join(" AND "))
    })
}

fn condition_expression(
    schema: &[ColumnSchema],
    condition: &Condition,
    clock: &EvaluationClock,
) -> Result<Expr, String> {
    let column_schema = schema_column(schema, &condition.column)?;
    let data_type = crate::domain::schema::from_duckdb(&column_schema.sql_type);
    let column = col(column_schema.name.as_str());
    match condition.operation {
        ConditionOp::IsNull => return Ok(column.is_null()),
        ConditionOp::IsNotNull => return Ok(column.is_not_null()),
        _ => {}
    }
    let values = condition
        .values
        .iter()
        .map(|value| resolve_condition_value(value, &data_type, clock))
        .collect::<Result<Vec<_>, _>>()?;
    if condition.operation == ConditionOp::In {
        if values.is_empty() {
            return Ok(lit(false));
        }
        let keys = values
            .iter()
            .filter(|key| !matches!(key, ScalarKey::Null))
            .collect::<Vec<_>>();
        let mut alternatives = Vec::new();
        if !keys.is_empty() {
            let series = scalar_series(&keys, &column_schema.dtype())?;
            alternatives.push(column.clone().is_in(lit(series).implode(false), false));
        }
        if values.iter().any(|key| matches!(key, ScalarKey::Null)) {
            alternatives.push(column.is_null());
        }
        return Ok(alternatives.into_iter().reduce(Expr::or).unwrap_or(lit(false)));
    }
    let value = values.first().ok_or("condition requires a value")?;
    if matches!(
        condition.operation,
        ConditionOp::Contains | ConditionOp::StartsWith | ConditionOp::EndsWith
    ) {
        let ScalarKey::String(text) = value else {
            return Ok(lit(false));
        };
        let text_column = rendered(column, &column_schema.dtype());
        return Ok(match condition.operation {
            ConditionOp::Contains => text_column.str().contains_literal(lit(text.clone())),
            ConditionOp::StartsWith => text_column.str().starts_with(lit(text.clone())),
            _ => text_column.str().ends_with(lit(text.clone())),
        });
    }
    if matches!(value, ScalarKey::Null) {
        return Ok(match condition.operation {
            ConditionOp::Equal => column.is_null(),
            ConditionOp::NotEqual => column.is_not_null(),
            _ => lit(false),
        });
    }
    let literal = scalar_literal(value, &column_schema.dtype())?;
    Ok(match condition.operation {
        ConditionOp::Equal => column.eq_missing(literal),
        ConditionOp::NotEqual => column.neq_missing(literal),
        ConditionOp::Greater => column.gt(literal),
        ConditionOp::GreaterOrEqual => column.gt_eq(literal),
        ConditionOp::Less => column.lt(literal),
        ConditionOp::LessOrEqual => column.lt_eq(literal),
        _ => unreachable!(),
    })
}

/// A single key as a literal of `dtype`.
fn scalar_literal(key: &ScalarKey, dtype: &DataType) -> Result<Expr, String> {
    let series = scalar_series(&[key], dtype)?;
    if series.is_empty() {
        return Err(format!(
            "{} has more decimal places than the column holds",
            crate::domain::slicers::label(key)
        ));
    }
    let value = series
        .get(0)
        .map_err(polars_error)?
        .into_static();
    Ok(lit(Scalar::new(dtype.clone(), value)))
}

fn condition_sql(
    schema: &[ColumnSchema],
    condition: &Condition,
    clock: &EvaluationClock,
) -> Result<String, String> {
    let column_schema = schema_column(schema, &condition.column)?;
    let data_type = crate::domain::schema::from_duckdb(&column_schema.sql_type);
    let column = quote_identifier(&column_schema.name);
    match condition.operation {
        ConditionOp::IsNull => return Ok(format!("{column} IS NULL")),
        ConditionOp::IsNotNull => return Ok(format!("{column} IS NOT NULL")),
        _ => {}
    }
    let values = condition
        .values
        .iter()
        .map(|value| resolve_condition_value(value, &data_type, clock))
        .collect::<Result<Vec<_>, _>>()?;
    if condition.operation == ConditionOp::In {
        if values.is_empty() {
            return Ok("FALSE".into());
        }
        let mut alternatives = Vec::new();
        let literals = values
            .iter()
            .filter(|key| !matches!(key, ScalarKey::Null))
            .map(literal_sql)
            .collect::<Vec<_>>();
        if !literals.is_empty() {
            alternatives.push(format!("{column} IN ({})", literals.join(", ")));
        }
        if values.iter().any(|key| matches!(key, ScalarKey::Null)) {
            alternatives.push(format!("{column} IS NULL"));
        }
        return Ok(if alternatives.len() == 1 {
            alternatives.remove(0)
        } else {
            format!("({})", alternatives.join(" OR "))
        });
    }
    let value = values.first().ok_or("condition requires a value")?;
    if matches!(
        condition.operation,
        ConditionOp::Contains | ConditionOp::StartsWith | ConditionOp::EndsWith
    ) {
        let ScalarKey::String(text) = value else {
            return Ok("FALSE".into());
        };
        let text = literal_sql(&ScalarKey::String(text.clone()));
        let value = format!("CAST({column} AS VARCHAR)");
        return Ok(match condition.operation {
            ConditionOp::Contains => format!("strpos({value}, {text}) > 0"),
            ConditionOp::StartsWith => format!("starts_with({value}, {text})"),
            _ => format!("ends_with({value}, {text})"),
        });
    }
    if matches!(value, ScalarKey::Null) {
        return Ok(match condition.operation {
            ConditionOp::Equal => format!("{column} IS NULL"),
            ConditionOp::NotEqual => format!("{column} IS NOT NULL"),
            _ => "FALSE".into(),
        });
    }
    let operator = match condition.operation {
        ConditionOp::Equal => "=",
        ConditionOp::NotEqual => "<>",
        ConditionOp::Greater => ">",
        ConditionOp::GreaterOrEqual => ">=",
        ConditionOp::Less => "<",
        ConditionOp::LessOrEqual => "<=",
        _ => unreachable!(),
    };
    Ok(format!("{column} {operator} {}", literal_sql(value)))
}

fn literal_sql(key: &ScalarKey) -> String {
    let quoted = |text: &str| format!("'{}'", text.replace('\'', "''"));
    match key {
        ScalarKey::Null => "NULL".into(),
        ScalarKey::Boolean(value) => value.to_string(),
        ScalarKey::String(value) => quoted(value),
        ScalarKey::Signed(value) | ScalarKey::Unsigned(value) | ScalarKey::Decimal(value) => {
            value.clone()
        }
        ScalarKey::FloatBits(_) => crate::domain::slicers::label(key),
        ScalarKey::Date(value) => format!("CAST({} AS DATE)", quoted(&date_from_days(value))),
        ScalarKey::Datetime { value, unit, .. } => format!(
            "CAST({} AS TIMESTAMP)",
            quoted(&datetime_from_units(value, unit))
        ),
        ScalarKey::Time(value) => format!("CAST({} AS TIME)", quoted(&time_from_nanos(value))),
        ScalarKey::Duration { .. } => quoted(&crate::domain::slicers::label(key)),
    }
}

fn resolve_condition_value(
    value: &ConditionValue,
    data_type: &ColumnType,
    clock: &EvaluationClock,
) -> Result<ScalarKey, String> {
    match value {
        ConditionValue::Literal(key) => Ok(key.clone()),
        ConditionValue::TodayOffsetDays(offset) => {
            let day = clock
                .today_local
                .checked_add_signed(Duration::days(i64::from(*offset)))
                .ok_or("relative date is out of range")?;
            let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("unix epoch");
            match data_type {
                ColumnType::Date => Ok(ScalarKey::Date(
                    day.signed_duration_since(epoch).num_days().to_string(),
                )),
                ColumnType::Datetime(_, _) => Ok(ScalarKey::Datetime {
                    value: day
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp_micros()
                        .to_string(),
                    unit: "Microseconds".into(),
                    timezone: None,
                }),
                _ => Err("today is only valid for date or datetime columns".into()),
            }
        }
        ConditionValue::NowOffsetSeconds(offset) => {
            if !matches!(data_type, ColumnType::Datetime(_, _)) {
                return Err("now is only valid for datetime columns".into());
            }
            let timestamp = clock
                .now_local
                .and_utc()
                .timestamp_micros()
                .checked_add(offset * 1_000_000)
                .ok_or("relative time is out of range")?;
            Ok(ScalarKey::Datetime {
                value: timestamp.to_string(),
                unit: "Microseconds".into(),
                timezone: None,
            })
        }
    }
}

fn date_from_days(value: &str) -> String {
    let Ok(days) = value.parse::<i64>() else {
        return value.to_owned();
    };
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|epoch| epoch.checked_add_signed(Duration::days(days)))
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| value.to_owned())
}

fn datetime_from_units(value: &str, unit: &str) -> String {
    let Ok(value) = value.parse::<i64>() else {
        return value.to_owned();
    };
    let micros = match unit {
        "Nanoseconds" => value / 1_000,
        "Milliseconds" => value.saturating_mul(1_000),
        "Seconds" => value.saturating_mul(1_000_000),
        _ => value,
    };
    DateTime::from_timestamp_micros(micros)
        .map(|datetime| {
            datetime
                .naive_utc()
                .format("%Y-%m-%d %H:%M:%S%.f")
                .to_string()
        })
        .unwrap_or_default()
}

fn time_from_nanos(value: &str) -> String {
    let Ok(nanos) = value.parse::<i64>() else {
        return value.to_owned();
    };
    let seconds = nanos.div_euclid(1_000_000_000);
    let hour = seconds / 3_600;
    let minute = seconds / 60 % 60;
    let second = seconds % 60;
    format!("{hour:02}:{minute:02}:{second:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::SortKey;
    use crate::domain::query::Token;
    use duckdb::types::Value;

    fn athletes() -> PolarsStore {
        let row = |year: i64, sex: &str, name: Option<&str>, season: Option<&str>, score: i64| {
            vec![
                Value::BigInt(year),
                Value::Text(sex.into()),
                name.map_or(Value::Null, |name| Value::Text(name.into())),
                season.map_or(Value::Null, |season| Value::Text(season.into())),
                Value::BigInt(score),
            ]
        };
        PolarsStore::from_rows(
            &[
                ("Year".into(), "BIGINT".into()),
                ("Sex".into(), "VARCHAR".into()),
                ("Name".into(), "VARCHAR".into()),
                ("Season".into(), "VARCHAR".into()),
                ("Score".into(), "BIGINT".into()),
            ],
            [
                row(2000, "F", Some("Alice"), Some("Summer"), 10),
                row(2000, "M", Some("Bob"), Some("Summer"), 20),
                row(2000, "M", Some("Chris"), Some("Winter"), 30),
                row(2004, "F", Some("Dana"), Some("Winter"), 40),
                row(2004, "F", None, Some("Winter"), 50),
                row(2008, "M", Some("Evan"), None, 60),
            ],
        )
        .unwrap()
    }

    fn structured(column: &str, operation: AggregationOp) -> Aggregation {
        Aggregation {
            sql: None,
            source_column: column.into(),
            operation,
            name: None,
            conditions: vec![],
            percentile: None,
            interpolation: None,
            ddof: None,
        }
    }

    fn winter() -> Condition {
        Condition {
            column: "Season".into(),
            operation: ConditionOp::Equal,
            values: vec![ConditionValue::Literal(ScalarKey::String("Winter".into()))],
        }
    }

    fn rows(store: &PolarsStore) -> Vec<Vec<Option<String>>> {
        store
            .fetch_row_ids(&store.initial_row_ids().unwrap())
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect()
    }

    fn names(store: &PolarsStore) -> Vec<String> {
        store.schema().iter().map(|column| column.name.clone()).collect()
    }

    fn text(values: &[&str]) -> Vec<Option<String>> {
        values
            .iter()
            .map(|value| (!value.is_empty()).then(|| value.to_string()))
            .collect()
    }

    fn slicer(column: &str, value: &str) -> SlicerDefinition {
        SlicerDefinition {
            sql: false,
            condition: None,
            column: column.into(),
            selected: vec![ScalarKey::String(value.into())],
        }
    }

    #[test]
    fn query_branches_group_pivot_and_aggregate_source_rows() {
        let source = athletes();
        let sum = structured("Score", AggregationOp::Sum);
        let filters = [slicer("Season", "Winter")];
        for (group_by, pivot_by, aggregations, expected) in [
            (vec![], vec![], vec![sum.clone()], vec![text(&["120"])]),
            (vec!["Year".into()], vec![], vec![], vec![text(&["2000"]), text(&["2004"])]),
            (
                vec!["Year".into()],
                vec![],
                vec![sum.clone()],
                vec![text(&["2000", "30"]), text(&["2004", "90"])],
            ),
            (
                vec!["Year".into()],
                vec!["Sex".into()],
                vec![],
                vec![vec![Some("2000".into()), None, Some("".into())], vec![Some("2004".into()), Some("".into()), None]],
            ),
            (
                vec!["Year".into()],
                vec!["Sex".into()],
                vec![sum.clone()],
                vec![text(&["2000", "", "30"]), text(&["2004", "90", ""])],
            ),
        ] {
            let pivot = !pivot_by.is_empty();
            let config = QueryConfig {
                group_by,
                pivot_by,
                aggregations,
                ..Default::default()
            };
            let result = execute(&source, &config, &filters).unwrap();
            assert_eq!(rows(&result), expected, "{config:?}");
            if pivot {
                assert_eq!(&names(&result)[1..], ["F", "M"]);
            }
        }
        // An absent selection keeps no rows: sums are null and counts zero.
        let config = QueryConfig {
            aggregations: vec![sum, structured("Score", AggregationOp::RowCount)],
            ..Default::default()
        };
        assert_eq!(
            rows(&execute(&source, &config, &[slicer("Season", "absent")]).unwrap()),
            [vec![None, Some("0".into())]]
        );
    }

    #[test]
    fn invalid_configurations_and_limits_are_rejected_before_running() {
        let source = athletes();
        for config in [
            QueryConfig::default(),
            QueryConfig {
                pivot_by: vec!["Sex".into()],
                ..Default::default()
            },
            QueryConfig {
                group_by: vec!["missing".into()],
                ..Default::default()
            },
        ] {
            let mut trace = SqlTrace::default();
            assert!(execute_traced(&source, &config, &[], &EvaluationClock::system(), &mut trace).is_err());
            assert!(trace.statements.is_empty());
        }
        let config = QueryConfig {
            group_by: vec!["Year".into()],
            ..Default::default()
        };
        for limit in [0, i64::MAX as u64 + 1] {
            assert!(
                execute_traced_limited(&source, &config, &[], &EvaluationClock::system(), limit, &mut SqlTrace::default())
                    .is_err()
            );
        }
    }

    #[test]
    fn result_views_search_and_sort_every_result_row_before_the_limit() {
        let source = athletes();
        let mut pivot = QueryConfig {
            group_by: vec!["Year".into()],
            pivot_by: vec!["Sex".into()],
            aggregations: vec![structured("Name", AggregationOp::NonNullCount)],
            ..Default::default()
        };
        for config in [
            QueryConfig {
                group_by: vec!["Year".into()],
                ..Default::default()
            },
            pivot.clone(),
        ] {
            let clock = EvaluationClock::system();
            let limited = execute_traced_limited(&source, &config, &[], &clock, 2, &mut SqlTrace::default()).unwrap();
            let run = |view: &ViewQuery| {
                let result = execute_query(&source, &config, &[], &[], &clock, 2, Some((&limited, view)), &mut SqlTrace::default())
                    .unwrap();
                assert_eq!(result.schema(), limited.schema());
                rows(&result).into_iter().map(|row| row[0].clone().unwrap()).collect::<Vec<_>>()
            };
            assert_eq!(rows(&limited).len(), 2);
            let search = ViewQuery {
                tokens: vec![Token::Contains("2008".into())],
                ..ViewQuery::default()
            };
            assert_eq!(run(&search), ["2008"]);
            let sort = ViewQuery {
                sort: vec![SortKey {
                    column: "Year".into(),
                    descending: true,
                }],
                ..ViewQuery::default()
            };
            assert_eq!(run(&sort), ["2008", "2004"]);
        }
        pivot.totals = true;
        assert!(names(&execute(&source, &pivot, &[]).unwrap()).contains(&"(Total)".to_owned()));
    }

    #[test]
    fn every_aggregation_runs_grouped_and_pivoted_with_conditions() {
        let source = athletes();
        for (operation, expected) in [
            (AggregationOp::Sum, "60"),
            (AggregationOp::Mean, "20.0"),
            (AggregationOp::Median, "20.0"),
            (AggregationOp::Minimum, "10"),
            (AggregationOp::Maximum, "30"),
            (AggregationOp::RowCount, "3"),
            (AggregationOp::NonNullCount, "3"),
            (AggregationOp::DistinctNonNullCount, "3"),
            (AggregationOp::First, "10"),
            (AggregationOp::Last, "30"),
            (AggregationOp::StandardDeviation, "10.0"),
            (AggregationOp::Variance, "100.0"),
            (AggregationOp::Percentile, "20.0"),
            (AggregationOp::ValuesList, "[10, 20, 30]"),
            (AggregationOp::UniqueValuesList, "[10, 20, 30]"),
        ] {
            let mut config = QueryConfig {
                group_by: vec!["Year".into()],
                aggregations: vec![structured("Score", operation.clone())],
                ..Default::default()
            };
            let result = execute(&source, &config, &[]).unwrap();
            assert_eq!(rows(&result)[0][1].as_deref(), Some(expected), "{operation:?}");
            config.pivot_by = vec!["Sex".into()];
            let result = execute(&source, &config, &[]).unwrap();
            assert_eq!((result.row_count(), result.schema().len()), (3, 3), "{operation:?}");
            config.aggregations[0].conditions = vec![winter()];
            for pivot_by in [vec![], vec!["Sex".into()]] {
                config.pivot_by = pivot_by;
                assert_eq!(execute(&source, &config, &[]).unwrap().row_count(), 3, "{operation:?}");
            }
        }
    }

    #[test]
    fn conditional_row_counts_filter_only_their_own_measure() {
        let source = athletes();
        let mut config = QueryConfig {
            group_by: vec!["Year".into()],
            aggregations: vec![Aggregation {
                conditions: vec![winter()],
                ..structured("Name", AggregationOp::RowCount)
            }],
            ..Default::default()
        };
        assert_eq!(rows(&execute(&source, &config, &[]).unwrap())[0][1], Some("1".into()));
        config.pivot_by = vec!["Sex".into()];
        let result = execute(&source, &config, &[]).unwrap();
        // 2000 has a summer woman and a winter man: the cells are 0 and 1.
        assert_eq!(&rows(&result)[0][1..], &[Some("0".into()), Some("1".into())]);
    }

    #[test]
    fn first_last_and_percentiles_follow_source_order_and_interpolation() {
        let source = athletes();
        let config = |aggregation: Aggregation| QueryConfig {
            group_by: vec!["Sex".into()],
            aggregations: vec![aggregation],
            ..Default::default()
        };
        let value = |aggregation: Aggregation| rows(&execute(&source, &config(aggregation), &[]).unwrap())[0][1].clone();
        // Women in source order score 10, 40, 50; the first winter one is 40.
        assert_eq!(value(structured("Score", AggregationOp::First)).as_deref(), Some("10"));
        assert_eq!(
            value(Aggregation {
                conditions: vec![winter()],
                ..structured("Score", AggregationOp::First)
            })
            .as_deref(),
            Some("40")
        );
        assert_eq!(value(structured("Score", AggregationOp::Last)).as_deref(), Some("50"));
        let percentile = |interpolation: Interpolation| Aggregation {
            percentile: Some(75.0),
            interpolation: Some(interpolation),
            ..structured("Score", AggregationOp::Percentile)
        };
        assert_eq!(value(percentile(Interpolation::Linear)).as_deref(), Some("45.0"));
        assert_eq!(value(percentile(Interpolation::Lower)).as_deref(), Some("40.0"));
        assert_eq!(value(percentile(Interpolation::Higher)).as_deref(), Some("50.0"));
        let population = Aggregation {
            ddof: Some(0),
            ..structured("Score", AggregationOp::Variance)
        };
        let sample = value(structured("Score", AggregationOp::Variance)).unwrap().parse::<f64>().unwrap();
        let population = value(population).unwrap().parse::<f64>().unwrap();
        assert!((population - sample * 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn editor_sql_computes_what_the_structured_aggregation_does() {
        let source = athletes();
        let mut cases = [
            AggregationOp::Sum,
            AggregationOp::Mean,
            AggregationOp::Median,
            AggregationOp::Minimum,
            AggregationOp::Maximum,
            AggregationOp::RowCount,
            AggregationOp::NonNullCount,
            AggregationOp::DistinctNonNullCount,
            AggregationOp::First,
            AggregationOp::Last,
            AggregationOp::StandardDeviation,
            AggregationOp::Variance,
            AggregationOp::Percentile,
            AggregationOp::ValuesList,
        ]
        .into_iter()
        .map(|operation| structured("Score", operation))
        .collect::<Vec<_>>();
        cases.push(Aggregation {
            ddof: Some(0),
            ..structured("Score", AggregationOp::StandardDeviation)
        });
        cases.push(Aggregation {
            conditions: vec![winter()],
            ..structured("Score", AggregationOp::Sum)
        });
        cases.push(Aggregation {
            conditions: vec![Condition {
                column: "Name".into(),
                operation: ConditionOp::In,
                values: vec![
                    ConditionValue::Literal(ScalarKey::String("Alice".into())),
                    ConditionValue::Literal(ScalarKey::Null),
                ],
            }],
            ..structured("Score", AggregationOp::Sum)
        });
        for aggregation in cases {
            let sql = editor_expression(&aggregation, source.schema()).unwrap();
            let run = |aggregation: Aggregation| {
                let config = QueryConfig {
                    group_by: vec!["Sex".into()],
                    aggregations: vec![aggregation],
                    ..Default::default()
                };
                let result = execute(&source, &config, &[]).unwrap();
                rows(&result)
                    .into_iter()
                    .map(|row| row[1].as_ref().and_then(|value| value.parse::<f64>().ok().map(|value| format!("{value:.6}")).or(Some(value.clone()))))
                    .collect::<Vec<_>>()
            };
            let structured_rows = run(aggregation.clone());
            let sql_rows = run(Aggregation::sql_expression(sql.clone(), None));
            assert_eq!(structured_rows, sql_rows, "{sql}");
            // The header is the SQL without identifier quotes.
            let config = QueryConfig {
                aggregations: vec![aggregation],
                ..Default::default()
            };
            assert_eq!(names(&execute(&source, &config, &[]).unwrap()), [display_name(&sql)]);
        }
    }

    #[test]
    fn headers_use_aliases_or_the_expression_text() {
        let source = athletes();
        for (expressions, group_headers, pivot_headers) in [
            (vec!["count(*)"], vec!["Year", "count_star()"], vec!["Year", "F", "M"]),
            (vec!["count(*) AS Rows"], vec!["Year", "Rows"], vec!["Year", "F_Rows", "M_Rows"]),
            (
                vec!["sum(\"Score\") AS \"Total score\"", "count(*)"],
                vec!["Year", "Total score", "count_star()"],
                vec!["Year", "F_Total score", "F_count_star()", "M_Total score", "M_count_star()"],
            ),
            (
                vec!["sum(\"Score\") total", "count(*) -- rows\n"],
                vec!["Year", "total", "count_star()"],
                vec!["Year", "F_total", "F_count_star()", "M_total", "M_count_star()"],
            ),
        ] {
            for (pivot, expected) in [(false, group_headers), (true, pivot_headers)] {
                let config = QueryConfig {
                    group_by: vec!["Year".into()],
                    pivot_by: if pivot { vec!["Sex".into()] } else { vec![] },
                    aggregations: expressions
                        .iter()
                        .map(|sql| Aggregation::sql_expression((*sql).into(), None))
                        .collect(),
                    ..Default::default()
                };
                assert_eq!(names(&execute(&source, &config, &[]).unwrap()), expected, "{expressions:?}");
            }
        }
        // A saved name is an alias, shown in the editor as SQL.
        let named = Aggregation {
            name: Some("Points".into()),
            ..structured("Score", AggregationOp::Sum)
        };
        assert_eq!(editor_expression(&named, source.schema()).unwrap(), "sum(\"Score\")\nAS \"Points\"");
        let config = QueryConfig {
            aggregations: vec![named],
            ..Default::default()
        };
        assert_eq!(names(&execute(&source, &config, &[]).unwrap()), ["Points"]);
    }

    #[test]
    fn pivot_totals_aggregate_source_rows_per_group_and_overall() {
        let source = athletes();
        let config = QueryConfig {
            group_by: vec!["Season".into()],
            pivot_by: vec!["Sex".into()],
            aggregations: vec![structured("Score", AggregationOp::Mean)],
            totals: true,
            ..Default::default()
        };
        let clock = EvaluationClock::system();
        let result = execute_with_clock(&source, &config, &[], &clock).unwrap();
        assert_eq!(names(&result), ["Season", "F", "M", "(Total)"]);
        // Winter's total is the mean of its three scores (40), not of the two
        // pivot cells' means; the null season still joins its total.
        assert_eq!(
            rows(&result),
            [
                text(&["Summer", "10.0", "20.0", "15.0"]),
                text(&["Winter", "45.0", "30.0", "40.0"]),
                text(&["", "", "60.0", "60.0"]),
            ]
        );
        let totals = execute_totals(&source, &config, &[], &[], &clock).unwrap();
        let total = |name: &str| {
            totals
                .iter()
                .find(|(column, _)| column == name)
                .and_then(|(_, value)| value.clone())
                .unwrap()
                .parse::<f64>()
                .unwrap()
        };
        assert_eq!(totals.len(), 3);
        assert!((total("F") - 100.0 / 3.0).abs() < 1e-9);
        assert!((total("M") - 110.0 / 3.0).abs() < 1e-9);
        assert_eq!(total("(Total)"), 35.0);
        let winter = [slicer("Season", "Winter")];
        let result = execute_with_clock(&source, &config, &winter, &clock).unwrap();
        assert_eq!(rows(&result), [text(&["Winter", "45.0", "30.0", "40.0"])]);
        assert_eq!(
            execute_totals(&source, &config, &winter, &[], &clock).unwrap(),
            [
                ("F".into(), Some("45.0".into())),
                ("M".into(), Some("30.0".into())),
                ("(Total)".into(), Some("40.0".into())),
            ]
        );
        let mut two = config.clone();
        two.aggregations.push(Aggregation::sql_expression("count(*)".into(), Some("rows".into())));
        let columns = names(&execute(&source, &two, &[]).unwrap());
        assert!(columns.contains(&"(Total)_rows".to_owned()), "{columns:?}");
        let totals = execute_totals(&source, &two, &[], &[], &clock).unwrap();
        assert!(totals.iter().all(|(name, _)| columns.contains(name)), "{totals:?}");
        assert!(totals.contains(&("(Total)_rows".into(), Some("6".into()))));
        let no_aggregates = QueryConfig {
            group_by: vec!["Year".into()],
            totals: true,
            ..Default::default()
        };
        assert!(execute_totals(&source, &no_aggregates, &[], &[], &clock).unwrap().is_empty());
    }

    #[test]
    fn null_pivot_values_get_a_column_and_drill_down_finds_them() {
        let source = athletes();
        let config = QueryConfig {
            group_by: vec!["Sex".into()],
            pivot_by: vec!["Season".into()],
            aggregations: vec![structured("Score", AggregationOp::RowCount)],
            ..Default::default()
        };
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(names(&result), ["Sex", "Summer", "Winter", "NULL"]);
        assert_eq!(rows(&result)[1], text(&["M", "1", "1", "1"]));
        let keys = source
            .distinct_rendered(&config.pivot_by, &[], &[])
            .unwrap();
        let filters = crate::domain::drill::cell_filters(&config, &[Some("M".into())], "NULL", false, &keys).unwrap();
        assert_eq!(filters[1].value, None);
        let view = ViewQuery {
            value_filters: filters,
            ..ViewQuery::default()
        };
        assert_eq!(source.query_row_ids(&view).unwrap(), [5]);
    }

    #[test]
    fn expression_dimensions_group_by_date_parts_and_keep_nulls_last() {
        let frame = polars::df!(
            "day" => [
                NaiveDate::from_ymd_opt(2024, 1, 5),
                NaiveDate::from_ymd_opt(2024, 1, 20),
                None,
                NaiveDate::from_ymd_opt(2023, 7, 1),
            ],
            "amount" => [1i64, 2, 3, 4],
        )
        .unwrap();
        let source = PolarsStore::from_frame(frame);
        let config = QueryConfig {
            group_by: vec![Dimension::expression("year(day) AS year"), Dimension::expression("month(\"day\")")],
            aggregations: vec![Aggregation::sql_expression("sum(amount)".into(), None)],
            ..Default::default()
        };
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(names(&result), ["year", "month(day)", "sum(amount)"]);
        assert_eq!(rows(&result), [text(&["2023", "7", "4"]), text(&["2024", "1", "3"]), text(&["", "", "3"])]);
        let months = QueryConfig {
            group_by: vec![Dimension::expression("date_trunc('month', day) AS month")],
            pivot_by: vec![Dimension::expression("amount > 1 AS big")],
            aggregations: vec![Aggregation::sql_expression("count(*)".into(), None)],
            ..Default::default()
        };
        let result = execute(&source, &months, &[]).unwrap();
        assert_eq!(names(&result), ["month", "false", "true"]);
        assert_eq!(rows(&result)[1], text(&["2024-01-01 00:00:00", "1", "1"]));
        // A value filter on an expression narrows the source rows before grouping.
        let filters = [ValueFilter {
            target: Dimension::expression("year(day) AS year"),
            value: Some("2024".into()),
            exclude: false,
        }];
        let result = execute_filtered(&source, &config, &[], &filters, &EvaluationClock::system(), 100, &mut SqlTrace::default())
            .unwrap();
        assert_eq!(rows(&result), [text(&["2024", "1", "3"])]);
    }

    #[test]
    fn dimension_binding_rejects_aggregates_missing_columns_and_conflicting_names() {
        let source = athletes();
        let check = |group_by: Vec<Dimension>| {
            validate_dimensions(
                &source,
                &QueryConfig {
                    group_by,
                    ..Default::default()
                },
            )
        };
        assert!(check(vec![Dimension::expression("upper(\"Name\") AS Code")]).is_ok());
        assert!(check(vec![Dimension::expression("sum(\"Score\") AS Bad")]).unwrap_err().contains("each row"));
        assert!(check(vec![Dimension::expression("upper(missing)")]).unwrap_err().contains("missing"));
        let error = check(vec!["Year".into(), Dimension::expression("\"Score\" AS year")]).unwrap_err();
        assert!(error.contains("duplicate dimension output name"), "{error}");
        assert!(check(vec![Dimension::expression("rowid % 2 AS half")]).is_ok());
        assert_eq!(
            dimension_editor_expression(&Dimension::from("Year"), source.schema()).unwrap(),
            "\"Year\""
        );
    }
}
