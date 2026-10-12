use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, Utc};
use duckdb::types::Value;

use crate::domain::documents::{
    Aggregation, AggregationOp, Condition, ConditionOp, ConditionValue, Dimension, Interpolation,
    QueryConfig, ScalarKey, SlicerDefinition, ValueFilter,
};
use crate::domain::schema::ColumnType;
use crate::domain::sql::SqlTrace;
use crate::infrastructure::duckdb_store::{
    ColumnFilter, ColumnSchema, DuckDbStore, QueryEnum, QueryInput, ViewQuery,
    filters_mention_rowid, quote_identifier,
};

static NEXT_QUERY_ENUM: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct EvaluationClock {
    pub today_local: NaiveDate,
    pub today_utc: NaiveDate,
    pub now_local: NaiveDateTime,
    pub now_utc: DateTime<Utc>,
}

impl EvaluationClock {
    pub fn system() -> Self {
        let local = Local::now();
        let utc = Utc::now();
        Self {
            today_local: local.date_naive(),
            today_utc: utc.date_naive(),
            now_local: local.naive_local(),
            now_utc: utc,
        }
    }
}

pub fn execute(
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
) -> Result<DuckDbStore, String> {
    execute_with_clock(source, config, slicers, &EvaluationClock::system())
}

pub fn execute_with_clock(
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
) -> Result<DuckDbStore, String> {
    execute_traced(source, config, slicers, clock, &mut SqlTrace::default())
}

pub fn execute_traced(
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    trace: &mut SqlTrace,
) -> Result<DuckDbStore, String> {
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
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<DuckDbStore, String> {
    execute_filtered(source, config, slicers, &[], clock, limit, trace)
}

/// Runs the query over source rows matching both the slicers and the value filters.
pub fn execute_filtered(
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    trace: &mut SqlTrace,
) -> Result<DuckDbStore, String> {
    execute_query(source, config, slicers, filters, clock, limit, None, trace)
}

/// The query's result rows, narrowed by a view over a previous result before the
/// limit applies: `store` is that previous result, giving the result schema.
pub type ResultView<'a> = (&'a DuckDbStore, &'a ViewQuery);

/// Like [`execute_filtered`], but with `result_view` the result search and sort
/// run inside the query, so they see every result row instead of only the
/// first `limit`.
#[allow(clippy::too_many_arguments)]
pub fn execute_query(
    source: &DuckDbStore,
    config: &QueryConfig,
    slicers: &[SlicerDefinition],
    filters: &[ValueFilter],
    clock: &EvaluationClock,
    limit: u64,
    result_view: Option<ResultView<'_>>,
    trace: &mut SqlTrace,
) -> Result<DuckDbStore, String> {
    if limit == 0 || limit > i64::MAX as u64 {
        return Err("query limit must be between 1 and 9223372036854775807".into());
    }
    validate_columns(source.schema(), config)?;
    if !config.pivot_by.is_empty() && config.group_by.is_empty() {
        return Err("Add a group by to set up this pivot".into());
    }
    if config.group_by.is_empty() && config.aggregations.is_empty() {
        if source.is_database_view() {
            let slicer_filters = slicer_filters(slicers);
            let (where_sql, values) = source.build_filter_clause(&slicer_filters, filters)?;
            let projection = source
                .schema()
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
            let mut values = values;
            let sql = limited(
                format!(
                    "SELECT {projection}\nFROM {}{where_sql}",
                    source.query_relation(filters_mention_rowid(&slicer_filters, filters), &[])
                ),
                limit,
                result_view,
                &mut values,
            )?;
            return source.materialize_query_traced(
                "Database view materialization",
                &sql,
                &values,
                None,
                trace,
            );
        }
        return Err("empty query configuration must use table mode".into());
    }
    let slicer_filters = slicer_filters(slicers);
    let (where_sql, where_values) = source.build_filter_clause(&slicer_filters, filters)?;
    let dimensions = bind_dimensions(source, config)?;
    let group_count = config.group_by.len();
    let group_by = dimensions[..group_count]
        .iter()
        .map(|dimension| dimension.reference.clone())
        .collect::<Vec<_>>();
    let pivot_by = dimensions[group_count..]
        .iter()
        .map(|dimension| dimension.reference.clone())
        .collect::<Vec<_>>();
    let derived = dimensions
        .iter()
        .filter_map(|dimension| dimension.projection.clone())
        .collect::<Vec<_>>();
    // Pivot discovery only filters rows, so it reads rowid only when they do.
    let filters_read_rowid = filters_mention_rowid(&slicer_filters, filters)
        || config
            .group_by
            .iter()
            .chain(&config.pivot_by)
            .any(dimension_mentions_rowid);
    let from_sql = source.query_relation(
        filters_read_rowid || aggregations_read_rowid(config),
        &derived,
    );
    let order = if group_by.is_empty() {
        String::new()
    } else {
        format!(
            "\nORDER BY {}",
            dimensions[..group_count]
                .iter()
                .map(|dimension| format!("{} ASC NULLS LAST", quote_identifier(&dimension.name)))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut expressions = Vec::new();
    let mut parameters = Vec::new();
    if pivot_by.is_empty() {
        let (sql, mut parameters) = grouped_select(
            source,
            config,
            &dimensions,
            &from_sql,
            &where_sql,
            &where_values,
            clock,
        )?;
        let sql = limited(format!("{sql}{order}"), limit, result_view, &mut parameters)?;
        return source.materialize_query_traced(
            "Query materialization",
            &sql,
            &parameters,
            None,
            trace,
        );
    }

    let ordered = config.aggregations.iter().any(is_ordered);
    let internal_name = |base: String| {
        let mut name = base;
        while source
            .schema()
            .iter()
            .any(|column| column.storage_name == name)
        {
            name.push('_');
        }
        quote_identifier(&name)
    };
    let row_id = internal_name("__qp_query_rowid".into());
    let mut projection = Vec::new();
    if ordered {
        projection.push(format!("rowid AS {row_id}"));
    }
    let mut using_parameters = Vec::new();
    for (index, aggregation) in config.aggregations.iter().enumerate() {
        if let Some(sql) = &aggregation.sql {
            expressions.push(format!("{}\n", named_expression(sql.clone(), aggregation)));
            continue;
        }
        let column = quote_identifier(
            &schema_column(source.schema(), &aggregation.source_column)?.storage_name,
        );
        let parameter_start = parameters.len();
        let condition = aggregation
            .conditions
            .iter()
            .map(|condition| {
                condition_expression(source.schema(), condition, clock, &mut parameters)
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" AND ");
        let mut match_order = None;
        let measure = if condition.is_empty() {
            column
        } else {
            if matches!(
                aggregation.operation,
                AggregationOp::First | AggregationOp::Last
            ) {
                let matched = internal_name(format!("__qp_query_match_{index}"));
                projection.push(format!(
                    "CASE WHEN ({condition}) THEN TRUE ELSE FALSE END AS {matched}"
                ));
                parameters.extend(parameters[parameter_start..].to_vec());
                match_order = Some(matched);
            }
            let measure = internal_name(format!("__qp_query_measure_{index}"));
            let value = if aggregation.operation == AggregationOp::RowCount {
                "1".to_owned()
            } else {
                column
            };
            projection.push(format!(
                "CASE WHEN ({condition}) THEN {value} ELSE NULL END AS {measure}"
            ));
            measure
        };
        let expression =
            if aggregation.operation == AggregationOp::RowCount && !condition.is_empty() {
                format!("count({measure})")
            } else {
                aggregate_expression(
                    aggregation,
                    &measure,
                    &row_id,
                    match_order.as_deref(),
                    &mut using_parameters,
                )?
            };
        expressions.push(named_expression(expression, aggregation));
    }
    if expressions.is_empty() {
        expressions.push("first('')".into());
    }
    let extra = if projection.is_empty() {
        String::new()
    } else {
        format!(",\n        {}", projection.join(",\n        "))
    };
    let input_sql = format!(
        "SELECT\n        *{extra}\n    FROM {}{}",
        from_sql,
        where_sql.replacen(" WHERE ", "\n    WHERE ", 1)
    );
    parameters.extend(where_values.clone());
    parameters.extend(using_parameters);
    // Per-row totals come from the same rows grouped without the pivot.
    let totals = if config.totals && !config.aggregations.is_empty() {
        let (sql, values) = grouped_select(
            source,
            config,
            &dimensions,
            &from_sql,
            &where_sql,
            &where_values,
            clock,
        )?;
        let names = source.query_column_names(&sql, &values)?;
        let names = total_column_names(&names[group_count..]);
        Some((sql, values, names))
    } else {
        None
    };
    // Fix the schema before binding the final CTAS so DuckDB does not replay
    // parameterized input during dynamic-column preprocessing.
    let identity = NEXT_QUERY_ENUM.fetch_add(1, Ordering::Relaxed);
    // A result view binds its own search parameters around the pivot.
    let enums = if parameters.is_empty() && totals.is_none() && result_view.is_none() {
        Vec::new()
    } else {
        let discovery_sql = source.query_relation(filters_read_rowid, &derived);
        pivot_by
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let nonnull = if where_sql.is_empty() {
                    format!(" WHERE {column} IS NOT NULL")
                } else {
                    format!("{where_sql} AND {column} IS NOT NULL")
                };
                QueryEnum {
                    name: format!("__qp_query_enum_{identity}_{index}"),
                    sql: format!(
                        "SELECT DISTINCT CAST({column} AS VARCHAR)\nFROM {}{nonnull}\nORDER BY 1",
                        discovery_sql
                    ),
                    values: where_values.clone(),
                }
            })
            .collect::<Vec<_>>()
    };
    let on = pivot_by
        .iter()
        .enumerate()
        .map(|(index, column)| {
            enums.get(index).map_or_else(
                || column.clone(),
                |enumeration| format!("{column} IN {}", quote_identifier(&enumeration.name)),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let renamed = dimensions[..group_count]
        .iter()
        .filter(|dimension| dimension.projection.is_some())
        .map(|dimension| {
            format!(
                "{} AS {}",
                dimension.reference,
                quote_identifier(&dimension.name)
            )
        })
        .collect::<Vec<_>>();
    let rename = if renamed.is_empty() {
        String::new()
    } else {
        format!(" RENAME ({})", renamed.join(", "))
    };
    let pivot_sql = format!(
        "SELECT *{rename}\nFROM (\n    PIVOT (\n    {input_sql}\n    )\n    ON {on}\n    USING\n        {}\n    GROUP BY {}\n)",
        expressions.join(",\n        "),
        group_by.join(", ")
    );
    let sql = match totals {
        None => format!("{pivot_sql}{order}"),
        Some((totals_sql, values, names)) => {
            // Positional aliases keep the joined group keys from clashing with
            // the pivot's, so ORDER BY names stay unambiguous.
            let aliases = (0..group_count + names.len())
                .map(|index| format!("__qp_total_{index}"))
                .collect::<Vec<_>>();
            let on = dimensions[..group_count]
                .iter()
                .zip(&aliases)
                .map(|(dimension, alias)| {
                    format!(
                        "__qp_pivot.{} IS NOT DISTINCT FROM __qp_totals.{alias}",
                        quote_identifier(&dimension.name)
                    )
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let columns = names
                .iter()
                .zip(&aliases[group_count..])
                .map(|(name, alias)| format!("__qp_totals.{alias} AS {}", quote_identifier(name)))
                .collect::<Vec<_>>()
                .join(", ");
            parameters.extend(values);
            format!(
                "SELECT __qp_pivot.*, {columns}\nFROM (\n{pivot_sql}\n) AS __qp_pivot\nLEFT JOIN (\n{totals_sql}\n) AS __qp_totals({})\nON {on}{order}",
                aliases.join(", ")
            )
        }
    };
    let sql = limited(sql, limit, result_view, &mut parameters)?;
    source.materialize_query_traced(
        "Query materialization",
        &sql,
        &parameters,
        if enums.is_empty() {
            None
        } else {
            Some(QueryInput { enums: &enums })
        },
        trace,
    )
}

/// Aggregations that keep source row order: they read rowid.
fn is_ordered(aggregation: &Aggregation) -> bool {
    matches!(
        aggregation.operation,
        AggregationOp::First
            | AggregationOp::Last
            | AggregationOp::ValuesList
            | AggregationOp::UniqueValuesList
    )
}

/// Whether the aggregations read rowid, which relations without DuckDB's own
/// rowid supply by numbering every row.
fn aggregations_read_rowid(config: &QueryConfig) -> bool {
    config.aggregations.iter().any(|aggregation| {
        aggregation.sql.as_deref().map_or_else(
            || is_ordered(aggregation),
            crate::domain::aggregation_sql::mentions_rowid,
        )
    })
}

fn dimension_mentions_rowid(dimension: &Dimension) -> bool {
    matches!(dimension, Dimension::Expression { sql }
        if crate::domain::aggregation_sql::mentions_rowid(sql))
}

/// Values by column name, rendered as text.
pub type TotalsRow = Vec<(String, Option<String>)>;

/// The grand total row of a query with totals: each total column's name and
/// its value over every source row the query reads. A plain group-by gives its
/// aggregate columns; a pivot also gives each pivot column and its `(Total)`
/// columns. Group columns have no total and are left out.
pub fn execute_totals(
    source: &DuckDbStore,
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
fn first_row(store: DuckDbStore) -> Result<TotalsRow, String> {
    let names = store
        .schema()
        .iter()
        .map(|column| column.name.clone())
        .collect::<Vec<_>>();
    let ids = store.initial_row_ids()?;
    let values = match ids.first() {
        Some(id) => store
            .fetch_row_ids(&[*id])?
            .into_iter()
            .next()
            .map(|row| row.values)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, values.get(index).cloned().flatten()))
        .collect())
}

/// The group-by query without pivot columns or `ORDER BY`, and its parameters:
/// one row per group, or one row in all when there is no group-by.
fn grouped_select(
    source: &DuckDbStore,
    config: &QueryConfig,
    dimensions: &[BoundDimension],
    from_sql: &str,
    where_sql: &str,
    where_values: &[Value],
    clock: &EvaluationClock,
) -> Result<(String, Vec<Value>), String> {
    let group_count = config.group_by.len();
    let mut expressions = Vec::new();
    let mut parameters = Vec::new();
    for aggregation in &config.aggregations {
        if let Some(sql) = &aggregation.sql {
            expressions.push(format!("{}\n", named_expression(sql.clone(), aggregation)));
            continue;
        }
        let column = quote_identifier(
            &schema_column(source.schema(), &aggregation.source_column)?.storage_name,
        );
        let expression =
            aggregate_expression(aggregation, &column, "rowid", None, &mut parameters)?;
        let conditions = aggregation
            .conditions
            .iter()
            .map(|condition| {
                condition_expression(source.schema(), condition, clock, &mut parameters)
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(" AND ");
        let filter = if conditions.is_empty() {
            String::new()
        } else {
            format!(" FILTER (WHERE {conditions})")
        };
        let expression = if aggregation.operation == AggregationOp::UniqueValuesList {
            format!("list_distinct(list({column} ORDER BY rowid){filter})")
        } else {
            format!("{expression}{filter}")
        };
        expressions.push(named_expression(expression, aggregation));
    }
    let mut projection = dimensions[..group_count]
        .iter()
        .map(|dimension| {
            format!(
                "{} AS {}",
                dimension.reference,
                quote_identifier(&dimension.name)
            )
        })
        .collect::<Vec<_>>();
    projection.extend(expressions);
    let distinct = if group_count == 0 { "" } else { " DISTINCT" };
    let group_clause = if group_count == 0 || config.aggregations.is_empty() {
        ""
    } else {
        "\nGROUP BY ALL"
    };
    let sql = format!(
        "SELECT{distinct}\n    {}\nFROM {}{}{group_clause}",
        projection.join(",\n    "),
        from_sql,
        where_sql.replacen(" WHERE ", "\nWHERE ", 1)
    );
    parameters.extend(where_values.iter().cloned());
    Ok((sql, parameters))
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

/// Whether `name` is one of a pivot's per-row `(Total)` columns.
pub fn is_total_column(name: &str) -> bool {
    name.strip_prefix(TOTAL_COLUMN)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
}

/// Appends the row limit, applying `result_view` first when given.
fn limited(
    sql: String,
    limit: u64,
    result_view: Option<ResultView<'_>>,
    parameters: &mut Vec<Value>,
) -> Result<String, String> {
    let Some((store, view)) = result_view else {
        return Ok(format!("{sql}\nLIMIT {limit}"));
    };
    let (sql, values) = store.wrap_query(view, &sql, limit)?;
    parameters.extend(values);
    Ok(sql)
}

struct BoundDimension {
    reference: String,
    name: String,
    projection: Option<String>,
}

fn bind_dimensions(
    source: &DuckDbStore,
    config: &QueryConfig,
) -> Result<Vec<BoundDimension>, String> {
    let mut bound: Vec<BoundDimension> = Vec::new();
    let mut used = source
        .schema()
        .iter()
        .map(|column| column.storage_name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    for (index, dimension) in config.group_by.iter().chain(&config.pivot_by).enumerate() {
        let entry = match dimension {
            Dimension::Column(name) => {
                let column = schema_column(source.schema(), name)?;
                BoundDimension {
                    reference: quote_identifier(&column.storage_name),
                    name: column.storage_name.clone(),
                    projection: None,
                }
            }
            Dimension::Expression { sql } => {
                let name = source.dimension_name(sql)?;
                let body = crate::domain::aggregation_sql::dimension_body(sql)?;
                let mut internal = format!("__qp_dimension_{index}");
                while used.contains(&internal.to_ascii_lowercase()) {
                    internal.push('_');
                }
                used.push(internal.to_ascii_lowercase());
                let reference = quote_identifier(&internal);
                BoundDimension {
                    projection: Some(format!("({body}\n) AS {reference}")),
                    reference,
                    name,
                }
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

pub fn validate_dimensions(source: &DuckDbStore, config: &QueryConfig) -> Result<(), String> {
    bind_dimensions(source, config).map(|_| ())
}

pub fn dimension_editor_expression(
    dimension: &Dimension,
    schema: &[ColumnSchema],
) -> Result<String, String> {
    match dimension {
        Dimension::Column(name) => Ok(quote_identifier(&schema_column(schema, name)?.storage_name)),
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

pub fn editor_expression(
    aggregation: &Aggregation,
    schema: &[ColumnSchema],
) -> Result<String, String> {
    if let Some(sql) = &aggregation.sql {
        return Ok(named_expression(sql.clone(), aggregation));
    }
    let column = quote_identifier(&schema_column(schema, &aggregation.source_column)?.storage_name);
    let mut parameters = Vec::new();
    let expression = aggregate_expression(aggregation, &column, "rowid", None, &mut parameters)?;
    let conditions = aggregation
        .conditions
        .iter()
        .map(|condition| {
            condition_expression(
                schema,
                condition,
                &EvaluationClock::system(),
                &mut parameters,
            )
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(" AND ");
    let filter = if conditions.is_empty() {
        String::new()
    } else {
        format!(" FILTER (WHERE {conditions})")
    };
    let expression = if aggregation.operation == AggregationOp::UniqueValuesList {
        format!("list_distinct(list({column} ORDER BY rowid){filter})")
    } else {
        format!("{expression}{filter}")
    };
    inline_parameters(&expression, &parameters)
        .map(|expression| named_expression(expression, aggregation))
}

fn inline_parameters(sql: &str, parameters: &[Value]) -> Result<String, String> {
    let mut output = String::new();
    let mut values = parameters.iter();
    let mut quote = None;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if let Some(delimiter) = quote {
            output.push(ch);
            if ch == delimiter {
                if chars.peek() == Some(&delimiter) {
                    output.push(chars.next().unwrap());
                } else {
                    quote = None;
                }
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            output.push(ch);
        } else if ch == '?' {
            let value = values.next().ok_or("missing SQL parameter")?;
            output.push_str(&match value {
                Value::Text(value) => format!("'{}'", value.replace('\'', "''")),
                Value::HugeInt(value) => value.to_string(),
                Value::BigInt(value) => value.to_string(),
                Value::Int(value) => value.to_string(),
                Value::Double(value) if value.is_finite() => value.to_string(),
                Value::Double(value) => format!("'{}'::DOUBLE", value),
                Value::Boolean(value) => value.to_string(),
                _ => return Err("unsupported SQL parameter in aggregation".into()),
            });
        } else {
            output.push(ch);
        }
    }
    Ok(output)
}

fn aggregate_expression(
    aggregation: &Aggregation,
    source: &str,
    row_id: &str,
    match_order: Option<&str>,
    parameters: &mut Vec<Value>,
) -> Result<String, String> {
    let operation = match aggregation.operation {
        AggregationOp::Sum => format!("sum({source})"),
        AggregationOp::Mean => format!("avg({source})"),
        AggregationOp::Median => format!("median({source})"),
        AggregationOp::Minimum => format!("min({source})"),
        AggregationOp::Maximum => format!("max({source})"),
        AggregationOp::RowCount => "count(*)".to_owned(),
        AggregationOp::NonNullCount => format!("count({source})"),
        AggregationOp::DistinctNonNullCount => format!("count(DISTINCT {source})"),
        AggregationOp::First => format!(
            "first({source} ORDER BY {}{row_id})",
            match_order.map_or(String::new(), |column| format!("{column} DESC, "))
        ),
        AggregationOp::Last => format!(
            "last({source} ORDER BY {}{row_id})",
            match_order.map_or(String::new(), |column| format!("{column} ASC, "))
        ),
        AggregationOp::StandardDeviation => {
            if aggregation.ddof.unwrap_or(1) == 0 {
                format!("stddev_pop({source})")
            } else {
                format!("stddev_samp({source})")
            }
        }
        AggregationOp::Variance => {
            if aggregation.ddof.unwrap_or(1) == 0 {
                format!("var_pop({source})")
            } else {
                format!("var_samp({source})")
            }
        }
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
                Interpolation::Nearest | Interpolation::Lower | Interpolation::Higher => {
                    "quantile_disc"
                }
                Interpolation::Linear | Interpolation::Midpoint => "quantile_cont",
            };
            parameters.push(Value::Double(percentile / 100.0));
            format!("{function}({source}, CAST(? AS DOUBLE))")
        }
        AggregationOp::ValuesList => format!("list({source} ORDER BY {row_id})"),
        AggregationOp::UniqueValuesList => {
            format!("list_distinct(list({source} ORDER BY {row_id}))")
        }
        AggregationOp::SqlExpression => return Err("SQL expression is missing".into()),
    };
    Ok(operation)
}

fn condition_expression(
    schema: &[ColumnSchema],
    condition: &Condition,
    clock: &EvaluationClock,
    parameters: &mut Vec<Value>,
) -> Result<String, String> {
    let column_schema = schema_column(schema, &condition.column)?;
    let data_type = crate::domain::schema::from_duckdb(&column_schema.duckdb_type);
    let column = quote_identifier(&column_schema.storage_name);
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
    if condition.operation == ConditionOp::In && values.is_empty() {
        return Ok("FALSE".into());
    }
    if condition.operation == ConditionOp::In {
        let alternatives = values
            .iter()
            .map(|value| value_expression(&column, &data_type, value, parameters))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(format!("({})", alternatives.join(" OR ")));
    }
    let value = values.first().ok_or("condition requires a value")?;
    if matches!(
        condition.operation,
        ConditionOp::Contains | ConditionOp::StartsWith | ConditionOp::EndsWith
    ) {
        let function = match condition.operation {
            ConditionOp::Contains => "contains",
            ConditionOp::StartsWith => "starts_with",
            ConditionOp::EndsWith => "ends_with",
            _ => unreachable!(),
        };
        return Ok(string_predicate(&column, function, parameters, value));
    }
    if matches!(value, ScalarKey::Null) {
        return Ok(match condition.operation {
            ConditionOp::Equal => format!("{column} IS NULL"),
            ConditionOp::NotEqual => format!("{column} IS NOT NULL"),
            _ => "FALSE".into(),
        });
    }
    let (parameter, cast) = scalar_parameter(value)?;
    parameters.push(parameter);
    let operator = match condition.operation {
        ConditionOp::Equal => "IS NOT DISTINCT FROM",
        ConditionOp::NotEqual => "IS DISTINCT FROM",
        ConditionOp::Greater => ">",
        ConditionOp::GreaterOrEqual => ">=",
        ConditionOp::Less => "<",
        ConditionOp::LessOrEqual => "<=",
        _ => unreachable!(),
    };
    let left = column;
    Ok(
        if column_schema.duckdb_type == "VARCHAR" && matches!(value, ScalarKey::String(_)) {
            format!("{left} {operator} ?")
        } else {
            format!("{left} {operator} CAST(? AS {cast})")
        },
    )
}

fn value_expression(
    column: &str,
    data_type: &ColumnType,
    value: &ScalarKey,
    parameters: &mut Vec<Value>,
) -> Result<String, String> {
    if matches!(value, ScalarKey::Null) {
        return Ok(format!("{column} IS NOT DISTINCT FROM NULL"));
    }
    let (parameter, cast) = scalar_parameter(value)?;
    parameters.push(parameter);
    Ok(match data_type {
        ColumnType::String => format!("{column} = ?"),
        _ => format!("{column} IS NOT DISTINCT FROM CAST(? AS {cast})"),
    })
}

fn scalar_parameter(value: &ScalarKey) -> Result<(Value, String), String> {
    Ok(match value {
        ScalarKey::Boolean(value) => (Value::Boolean(*value), "BOOLEAN".to_owned()),
        ScalarKey::String(value) => (Value::Text(value.clone()), "VARCHAR".to_owned()),
        ScalarKey::Decimal(value) => (
            Value::Text(value.clone()),
            format!(
                "DECIMAL(38, {})",
                value
                    .split_once('.')
                    .map_or(0, |(_, fraction)| fraction.len())
            ),
        ),
        ScalarKey::Signed(value) => (
            Value::HugeInt(value.parse().unwrap_or_default()),
            "HUGEINT".into(),
        ),
        ScalarKey::Unsigned(value) => (
            Value::UHugeInt(value.parse().unwrap_or_default()),
            "UHUGEINT".into(),
        ),
        ScalarKey::FloatBits(bits) => {
            let value = bits
                .strip_prefix("f32:")
                .and_then(|bits| bits.parse::<u32>().ok().map(f32::from_bits).map(f64::from))
                .or_else(|| {
                    bits.strip_prefix("f64:")
                        .and_then(|bits| bits.parse::<u64>().ok().map(f64::from_bits))
                })
                .unwrap_or_default();
            (Value::Double(value), "DOUBLE".into())
        }
        ScalarKey::Date(value) => (Value::Text(date_from_days(value)), "DATE".into()),
        ScalarKey::Datetime { value, unit, .. } => (
            Value::Text(datetime_from_units(value, unit)),
            "TIMESTAMP".into(),
        ),
        ScalarKey::Time(value) => (Value::Text(time_from_nanos(value)), "TIME".into()),
        ScalarKey::Duration { value, .. } => (Value::Text(value.clone()), "INTERVAL".into()),
        ScalarKey::Null => return Err("null is not a scalar parameter".into()),
    })
}

fn string_predicate(
    column: &str,
    function: &str,
    parameters: &mut Vec<Value>,
    value: &ScalarKey,
) -> String {
    let ScalarKey::String(value) = value else {
        return "FALSE".into();
    };
    parameters.push(Value::Text(value.clone()));
    format!("{function}(CAST({column} AS VARCHAR), ?)")
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
        _ => value,
    };
    DateTime::from_timestamp_micros(micros)
        .map(|datetime| {
            datetime
                .naive_utc()
                .format("%Y-%m-%d %H:%M:%S%.6f")
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

fn named_expression(expression: String, aggregation: &Aggregation) -> String {
    // Saved views may still have a separate name; expose it as SQL in the editor.
    if let Some(name) = aggregation.name.as_deref().filter(|name| !name.is_empty()) {
        format!("{expression}\nAS {}", quote_identifier(name))
    } else {
        expression
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_branches_use_select_or_pivot_and_limit_the_result() {
        let source = athletes();
        let measure = Aggregation {
            sql: None,
            source_column: "Score".into(),
            operation: AggregationOp::Sum,
            name: None,
            conditions: vec![],
            percentile: None,
            interpolation: None,
            ddof: None,
        };
        let filters = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "Season".into(),
            selected: vec![ScalarKey::String("Winter".into())],
        }];
        for (group_by, pivot_by, aggregations, pivot, expected) in [
            (
                vec![],
                vec![],
                vec![measure.clone()],
                false,
                vec![vec![Some("120.0".into())]],
            ),
            (
                vec!["Year".into()],
                vec![],
                vec![],
                false,
                vec![vec![Some("2000".into())]],
            ),
            (
                vec!["Year".into()],
                vec![],
                vec![measure.clone()],
                false,
                vec![vec![Some("2000".into()), Some("30.0".into())]],
            ),
            (
                vec!["Year".into()],
                vec!["Sex".into()],
                vec![],
                true,
                vec![vec![Some("2000".into()), None, Some("".into())]],
            ),
            (
                vec!["Year".into()],
                vec!["Sex".into()],
                vec![measure.clone()],
                true,
                vec![vec![Some("2000".into()), None, Some("30.0".into())]],
            ),
        ] {
            let config = QueryConfig {
                group_by,
                pivot_by,
                aggregations,
                ..Default::default()
            };
            let mut trace = SqlTrace::default();
            let result = execute_traced_limited(
                &source,
                &config,
                &filters,
                &EvaluationClock::system(),
                1,
                &mut trace,
            )
            .unwrap_or_else(|error| panic!("{error}\n{}", trace.text()));
            assert_eq!(rows(&result), expected);
            let sql = &trace.statements.last().unwrap().sql;
            assert_eq!(sql.contains("PIVOT"), pivot);
            assert!(sql.contains("LIMIT 1"));
            assert!(!trace.text().contains("CREATE TEMP TABLE"));
            assert!(!trace.text().contains("CAST(\"Season\" AS VARCHAR)"));
            assert!(!trace.text().contains("__qp_query_measure"));
        }
        let mut config = QueryConfig {
            aggregations: vec![measure],
            ..Default::default()
        };
        config.aggregations.push(Aggregation {
            operation: AggregationOp::RowCount,
            ..config.aggregations[0].clone()
        });
        let result = execute(&source, &config, &filters).unwrap();
        assert_eq!(
            rows(&result),
            vec![vec![Some("120.0".into()), Some("3".into())]]
        );
        let empty = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "Season".into(),
            selected: vec![ScalarKey::String("absent".into())],
        }];
        assert_eq!(
            rows(&execute(&source, &config, &empty).unwrap()),
            vec![vec![None, Some("0".into())]]
        );
    }

    #[test]
    fn pivot_without_groups_and_empty_queries_execute_no_sql() {
        let source = athletes();
        for config in [
            QueryConfig::default(),
            QueryConfig {
                pivot_by: vec!["Sex".into()],
                ..Default::default()
            },
            QueryConfig {
                group_by: vec![],
                ..athlete_config()
            },
        ] {
            let mut trace = SqlTrace::default();
            assert!(
                execute_traced(
                    &source,
                    &config,
                    &[],
                    &EvaluationClock::system(),
                    &mut trace
                )
                .is_err()
            );
            assert!(trace.statements.is_empty());
        }
    }

    #[test]
    fn search_and_sort_see_only_the_limited_query_result() {
        let source = athletes();
        let config = QueryConfig {
            group_by: vec!["Year".into()],
            ..Default::default()
        };
        let mut trace = SqlTrace::default();
        let result = execute_traced_limited(
            &source,
            &config,
            &[],
            &EvaluationClock::system(),
            2,
            &mut trace,
        )
        .unwrap();
        assert_eq!(
            rows(&result),
            vec![vec![Some("2000".into())], vec![Some("2004".into())]]
        );
        let ids = result
            .query_row_ids(&crate::infrastructure::duckdb_store::ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![crate::domain::query::Token::Contains("2008".into())],
                filters: vec![],
                sort: vec![],
            })
            .unwrap();
        assert!(ids.is_empty());
        for limit in [0, i64::MAX as u64 + 1] {
            assert!(
                execute_traced_limited(
                    &source,
                    &config,
                    &[],
                    &EvaluationClock::system(),
                    limit,
                    &mut SqlTrace::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn result_views_search_and_sort_every_result_row_before_the_limit() {
        use crate::domain::documents::SortKey;
        use crate::domain::query::Token;
        let source = athletes();
        for config in [
            QueryConfig {
                group_by: vec!["Year".into()],
                ..Default::default()
            },
            athlete_config(),
        ] {
            let run = |view: Option<&ViewQuery>| {
                let limited = execute_traced_limited(
                    &source,
                    &config,
                    &[],
                    &EvaluationClock::system(),
                    2,
                    &mut SqlTrace::default(),
                )
                .unwrap();
                let Some(view) = view else {
                    return rows(&limited);
                };
                let result = execute_query(
                    &source,
                    &config,
                    &[],
                    &[],
                    &EvaluationClock::system(),
                    2,
                    Some((&limited, view)),
                    &mut SqlTrace::default(),
                )
                .unwrap();
                assert_eq!(result.schema(), limited.schema());
                rows(&result)
            };
            let years = |rows: Vec<Vec<Option<String>>>| {
                rows.into_iter()
                    .map(|row| row[0].clone().unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(years(run(None)), ["2000", "2004"]);
            let search = ViewQuery {
                hidden: Vec::new(),
                tokens: vec![Token::Contains("2008".into())],
                ..ViewQuery::default()
            };
            assert_eq!(years(run(Some(&search))), ["2008"]);
            let sort = ViewQuery {
                hidden: Vec::new(),
                sort: vec![SortKey {
                    column: "Year".into(),
                    descending: true,
                }],
                ..ViewQuery::default()
            };
            assert_eq!(years(run(Some(&sort))), ["2008", "2004"]);
        }
    }

    #[test]
    fn conditional_row_counts_filter_only_their_own_measure() {
        let source = athletes();
        let mut config = athlete_config();
        config.aggregations[0].operation = AggregationOp::RowCount;
        config.aggregations[0].conditions = vec![Condition {
            column: "Season".into(),
            operation: ConditionOp::Equal,
            values: vec![ConditionValue::Literal(ScalarKey::String("Winter".into()))],
        }];
        for pivot_by in [vec![], vec!["Sex".into()]] {
            config.pivot_by = pivot_by;
            let result = execute(&source, &config, &[]).unwrap();
            assert_eq!(result.row_count(), 3);
            let result_rows = rows(&result);
            if config.pivot_by.is_empty() {
                assert_eq!(result_rows[0][1], Some("1".into()));
            } else {
                assert_eq!(&result_rows[0][1..], &[Some("0".into()), Some("1".into())]);
            }
        }
    }

    #[test]
    fn every_aggregation_runs_in_select_and_pivot_queries() {
        let source = athletes();
        for (operation, expected) in [
            (AggregationOp::Sum, "60.0"),
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
            (AggregationOp::ValuesList, ""),
            (AggregationOp::UniqueValuesList, ""),
        ] {
            let mut config = athlete_config();
            config.aggregations[0].source_column = "Score".into();
            config.aggregations[0].operation = operation;
            config.pivot_by.clear();
            let result = execute(&source, &config, &[]).unwrap();
            let values = rows(&result);
            if expected.is_empty() {
                let list = values[0][1].as_ref().unwrap();
                assert!(list.contains("10") && list.contains("20") && list.contains("30"));
            } else {
                assert_eq!(values[0][1].as_deref(), Some(expected));
            }
            config.pivot_by = vec!["Sex".into()];
            let result = execute(&source, &config, &[]).unwrap();
            assert_eq!(result.row_count(), 3);
            assert_eq!(result.schema().len(), 3);
            config.aggregations[0].conditions = vec![Condition {
                column: "Season".into(),
                operation: ConditionOp::Equal,
                values: vec![ConditionValue::Literal(ScalarKey::String("Winter".into()))],
            }];
            for pivot_by in [vec![], vec!["Sex".into()]] {
                config.pivot_by = pivot_by;
                assert_eq!(execute(&source, &config, &[]).unwrap().row_count(), 3);
            }
        }
    }
    use crate::infrastructure::duckdb_store::DuckDbStore;

    fn source() -> DuckDbStore {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES ('West', 'A', 10), ('West', 'B', 20), ('East', 'A', 5)) t(region, kind, amount)").unwrap();
        DuckDbStore::from_connection(connection).unwrap()
    }

    #[test]
    fn tables_order_by_their_own_rowid_without_numbering_rows() {
        let source = source();
        for group_by in [
            vec![Dimension::from("region")],
            vec![Dimension::expression("lower(region) AS area")],
        ] {
            let config = QueryConfig {
                group_by,
                aggregations: vec![Aggregation {
                    sql: None,
                    source_column: "kind".into(),
                    operation: AggregationOp::Last,
                    ..Aggregation::sql_expression(String::new(), Some("kind".into()))
                }],
                ..Default::default()
            };
            let mut trace = SqlTrace::default();
            let result = execute_traced(
                &source,
                &config,
                &[],
                &EvaluationClock::system(),
                &mut trace,
            )
            .unwrap();
            assert_eq!(rows(&result)[1][1].as_deref(), Some("B"));
            assert!(!trace.text().contains("row_number"), "{}", trace.text());
        }
    }

    fn athletes() -> DuckDbStore {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TEMP TABLE qp_data AS SELECT * FROM (VALUES
            (2000, 'F', 'Alice', 'Summer', 10),
            (2000, 'M', 'Bob', 'Summer', 20),
            (2000, 'M', 'Chris', 'Winter', 30),
            (2004, 'F', 'Dana', 'Winter', 40),
            (2004, 'F', NULL, 'Winter', 50),
            (2008, 'M', 'Evan', NULL, 60)) t(Year, Sex, Name, Season, Score)",
            )
            .unwrap();
        DuckDbStore::from_connection(connection).unwrap()
    }

    fn athlete_config() -> QueryConfig {
        QueryConfig {
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
        }
    }

    fn rows(store: &DuckDbStore) -> Vec<Vec<Option<String>>> {
        store
            .fetch_row_ids(&store.initial_row_ids().unwrap())
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect()
    }

    fn mean_score_by(group: &str, pivot: Option<&str>) -> QueryConfig {
        QueryConfig {
            group_by: vec![group.into()],
            pivot_by: pivot.into_iter().map(Into::into).collect(),
            aggregations: vec![Aggregation {
                operation: AggregationOp::Mean,
                source_column: "Score".into(),
                ..athlete_config().aggregations[0].clone()
            }],
            totals: true,
            ..Default::default()
        }
    }

    fn names(store: &DuckDbStore) -> Vec<String> {
        store
            .schema()
            .iter()
            .map(|column| column.name.clone())
            .collect()
    }

    #[test]
    fn pivot_totals_aggregate_source_rows_per_group_and_overall() {
        let source = athletes();
        let config = mean_score_by("Season", Some("Sex"));
        let clock = EvaluationClock::system();
        let result = execute_with_clock(&source, &config, &[], &clock).unwrap();
        assert_eq!(names(&result), ["Season", "F", "M", "(Total)"]);
        // Winter's total is the mean of its three scores (40), not of the two
        // pivot cells' means; the NULL season still joins its total.
        let text = |values: &[&str]| {
            values
                .iter()
                .map(|value| (!value.is_empty()).then(|| value.to_string()))
                .collect::<Vec<_>>()
        };
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
        };
        assert_eq!(totals.len(), 3);
        assert_eq!(total("F").unwrap().parse::<f64>().unwrap(), 100.0 / 3.0);
        assert_eq!(total("M").unwrap().parse::<f64>().unwrap(), 110.0 / 3.0);
        assert_eq!(total("(Total)").as_deref(), Some("35.0"));

        // A slicer narrows the per-row and grand totals alike.
        let winter = vec![SlicerDefinition {
            column: "Season".into(),
            selected: vec![ScalarKey::String("Winter".into())],
            condition: None,
            sql: false,
        }];
        let result = execute_with_clock(&source, &config, &winter, &clock).unwrap();
        assert_eq!(rows(&result), [text(&["Winter", "45.0", "30.0", "40.0"])]);
        let totals = execute_totals(&source, &config, &winter, &[], &clock).unwrap();
        assert_eq!(
            totals,
            [
                ("F".into(), Some("45.0".into())),
                ("M".into(), Some("30.0".into())),
                ("(Total)".into(), Some("40.0".into())),
            ]
        );
    }

    #[test]
    fn group_totals_name_the_result_columns_and_skip_without_aggregates() {
        let source = athletes();
        let clock = EvaluationClock::system();
        let config = mean_score_by("Year", None);
        let result = execute_with_clock(&source, &config, &[], &clock).unwrap();
        // No pivot, so no per-row total column.
        assert_eq!(names(&result).len(), 2);
        let totals = execute_totals(&source, &config, &[], &[], &clock).unwrap();
        assert_eq!(totals, [(names(&result)[1].clone(), Some("35.0".into()))]);

        let mut two = mean_score_by("Year", Some("Sex"));
        two.aggregations.push(Aggregation::sql_expression(
            "count(*)".into(),
            Some("rows".into()),
        ));
        let result = execute_with_clock(&source, &two, &[], &clock).unwrap();
        let columns = names(&result);
        assert!(columns.contains(&"(Total)_rows".to_owned()), "{columns:?}");
        let totals = execute_totals(&source, &two, &[], &[], &clock).unwrap();
        assert!(
            totals.iter().all(|(name, _)| columns.contains(name)),
            "{totals:?}"
        );
        assert!(totals.contains(&("(Total)_rows".into(), Some("6".into()))));

        let no_aggregates = QueryConfig {
            group_by: vec!["Year".into()],
            totals: true,
            ..Default::default()
        };
        assert!(
            execute_totals(&source, &no_aggregates, &[], &[], &clock)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn year_sex_count_survives_season_switching_multi_selection_and_clear() {
        let source = athletes();
        for (selected, expected) in [
            (
                vec![ScalarKey::String("Summer".into())],
                vec![vec![
                    Some("2000".into()),
                    Some("1".into()),
                    Some("1".into()),
                ]],
            ),
            (
                vec![ScalarKey::String("Winter".into())],
                vec![
                    vec![Some("2000".into()), Some("0".into()), Some("1".into())],
                    vec![Some("2004".into()), Some("1".into()), Some("0".into())],
                ],
            ),
            (
                vec![
                    ScalarKey::String("Summer".into()),
                    ScalarKey::String("Winter".into()),
                ],
                vec![
                    vec![Some("2000".into()), Some("1".into()), Some("2".into())],
                    vec![Some("2004".into()), Some("1".into()), Some("0".into())],
                ],
            ),
        ] {
            let mut trace = SqlTrace::default();
            let result = execute_traced(
                &source,
                &athlete_config(),
                &[SlicerDefinition {
                    sql: false,
                    condition: None,
                    column: "Season".into(),
                    selected,
                }],
                &EvaluationClock::system(),
                &mut trace,
            )
            .unwrap();
            assert_eq!(
                result
                    .schema()
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>(),
                ["Year", "F", "M"]
            );
            assert_eq!(rows(&result), expected);
            assert_eq!(trace.statements.len(), 3);
            assert!(!trace.statements[0].parameters.is_empty());
            assert!(trace.statements[1].parameters.is_empty());
            assert!(!trace.statements[2].parameters.is_empty());
            assert!(!trace.text().contains("CREATE TEMP TABLE"));
        }
        let mut trace = SqlTrace::default();
        let result = execute_traced(
            &source,
            &athlete_config(),
            &[],
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap();
        assert_eq!(result.row_count(), 3);
        assert_eq!(trace.statements.len(), 1);
        let nulls = execute(
            &source,
            &athlete_config(),
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::Null],
            }],
        )
        .unwrap();
        assert_eq!(
            rows(&nulls),
            vec![vec![Some("2008".into()), Some("1".into())]]
        );
        let empty = execute(
            &source,
            &athlete_config(),
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("missing".into())],
            }],
        )
        .unwrap();
        assert_eq!(empty.row_count(), 0);
    }

    #[test]
    fn direct_pivot_conditions_keep_bound_percentile_and_original_order() {
        let source = athletes();
        let mut config = athlete_config();
        config.aggregations[0].conditions = vec![Condition {
            column: "Season".into(),
            operation: ConditionOp::Equal,
            values: vec![ConditionValue::Literal(ScalarKey::String("Winter".into()))],
        }];
        config.aggregations[0].source_column = "Score".into();
        config.aggregations[0].operation = AggregationOp::Percentile;
        config.aggregations[0].percentile = Some(50.0);
        config.aggregations[0].interpolation = Some(Interpolation::Linear);
        let mut trace = SqlTrace::default();
        let result = execute_traced(
            &source,
            &config,
            &[],
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap_or_else(|error| panic!("{error}\n{}", trace.text()));
        assert_eq!(rows(&result)[1][1], Some("45.0".into()));
        assert!(
            trace
                .statements
                .last()
                .unwrap()
                .parameters
                .iter()
                .any(|v| matches!(v, Value::Text(v) if v == "Winter"))
        );
        assert!(
            trace
                .statements
                .last()
                .unwrap()
                .parameters
                .iter()
                .any(|v| matches!(v, Value::Double(v) if *v == 0.5))
        );
        let empty = execute(
            &source,
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("missing".into())],
            }],
        )
        .unwrap();
        assert_eq!(empty.row_count(), 0);
        config.pivot_by.push("Season".into());
        let mut trace = SqlTrace::default();
        let result = execute_traced(
            &source,
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("Winter".into())],
            }],
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap();
        assert_eq!(result.row_count(), 2);
        assert_eq!(trace.statements[0].parameters.len(), 1);
        assert_eq!(trace.statements.len(), 5);
        config.pivot_by.pop();
        for (operation, expected) in [(AggregationOp::First, "40"), (AggregationOp::Last, "50")] {
            config.aggregations[0].operation = operation;
            let result = execute(&source, &config, &[]).unwrap();
            assert_eq!(rows(&result)[1][1], Some(expected.into()));
        }
        config.aggregations[0].operation = AggregationOp::ValuesList;
        let result = execute(&source, &config, &[]).unwrap();
        let values = rows(&result)[1][1].clone().unwrap();
        assert!(values.find("40").unwrap() < values.find("50").unwrap());
        config.pivot_by.push("Season".into());
        config.aggregations[0].conditions.clear();
        let result = execute(
            &source,
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("Winter".into())],
            }],
        )
        .unwrap();
        assert_eq!(result.row_count(), 2);
        assert!(
            result
                .schema()
                .iter()
                .any(|column| column.name.starts_with("F_Winter"))
        );
    }

    #[test]
    fn direct_pivot_quotes_identifiers_and_binds_literal_values() {
        let source = DuckDbStore::from_rows(
            &[
                ("Year".into(), "BIGINT".into()),
                ("Sex \" code".into(), "VARCHAR".into()),
                ("Name".into(), "VARCHAR".into()),
                ("Season's name".into(), "VARCHAR".into()),
            ],
            [vec![
                Value::BigInt(2000),
                Value::Text("F".into()),
                Value::Text("O'Brien".into()),
                Value::Text("Winter'; DROP TABLE qp_data; --".into()),
            ]],
        )
        .unwrap();
        let mut config = athlete_config();
        config.pivot_by = vec!["Sex \" code".into()];
        let result = execute(
            &source,
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season's name".into(),
                selected: vec![ScalarKey::String("Winter'; DROP TABLE qp_data; --".into())],
            }],
        )
        .unwrap();
        assert_eq!(
            rows(&result),
            vec![vec![Some("2000".into()), Some("1".into())]]
        );
        assert_eq!(source.initial_row_ids().unwrap().len(), 1);
    }

    #[test]
    fn grouped_and_multidimensional_aggregates_run_in_duckdb() {
        let result = execute(
            &source(),
            &QueryConfig {
                group_by: vec!["region".into()],
                pivot_by: vec!["kind".into()],
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
                result_sort: vec![],
                totals: false,
                heatmap: false,
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.row_count(), 2);
        assert_eq!(result.schema().len(), 3);
    }

    #[test]
    fn conditions_and_slicers_are_bound_and_group_membership_is_retained() {
        let mut config = QueryConfig {
            group_by: vec!["region".into()],
            pivot_by: vec![],
            aggregations: vec![Aggregation {
                sql: None,
                source_column: "amount".into(),
                operation: AggregationOp::Sum,
                name: None,
                conditions: vec![Condition {
                    column: "kind".into(),
                    operation: ConditionOp::Equal,
                    values: vec![ConditionValue::Literal(ScalarKey::String("A".into()))],
                }],
                percentile: None,
                interpolation: None,
                ddof: None,
            }],
            result_sort: vec![],
            totals: false,
            heatmap: false,
        };
        let result = execute(&source(), &config, &[]).unwrap();
        assert_eq!(result.row_count(), 2);
        config.aggregations[0].operation = AggregationOp::RowCount;
        let result = execute(
            &source(),
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "region".into(),
                selected: vec![ScalarKey::String("East".into())],
            }],
        )
        .unwrap();
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn percentile_value_is_bound_to_the_aggregate_query() {
        let result = execute(
            &source(),
            &QueryConfig {
                group_by: vec!["region".into()],
                pivot_by: vec![],
                aggregations: vec![Aggregation {
                    sql: None,
                    source_column: "amount".into(),
                    operation: AggregationOp::Percentile,
                    name: None,
                    conditions: vec![],
                    percentile: Some(50.0),
                    interpolation: Some(Interpolation::Linear),
                    ddof: None,
                }],
                result_sort: vec![],
                totals: false,
                heatmap: false,
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.row_count(), 2);
    }

    #[test]
    fn sql_aggregates_support_count_order_filter_arithmetic_and_slicers() {
        let mut config = QueryConfig {
            group_by: vec!["Year".into()],
            aggregations: [
                ("count(*)", "Rows"),
                ("first(\"Name\" ORDER BY \"Score\" DESC)", "Latest"),
                (
                    "sum(\"Score\") FILTER (WHERE \"Season\" = 'Summer')",
                    "Summer",
                ),
                ("sum(\"Score\") / count(*)", "Average"),
            ]
            .into_iter()
            .map(|(sql, name)| Aggregation::sql_expression(sql.into(), Some(name.into())))
            .collect(),
            ..Default::default()
        };
        let result = execute(&athletes(), &config, &[]).unwrap();
        assert_eq!(
            rows(&result)[0],
            vec![
                Some("2000".into()),
                Some("3".into()),
                Some("Chris".into()),
                Some("30.0".into()),
                Some("20.0".into())
            ]
        );
        config.group_by.clear();
        let result = execute(
            &athletes(),
            &config,
            &[SlicerDefinition {
                sql: false,
                condition: None,
                column: "Season".into(),
                selected: vec![ScalarKey::String("Summer".into())],
            }],
        )
        .unwrap();
        assert_eq!(
            rows(&result),
            vec![vec![
                Some("2".into()),
                Some("Bob".into()),
                Some("30.0".into()),
                Some("15.0".into())
            ]]
        );
    }

    #[test]
    fn sql_pivots_keep_the_direct_pipeline_and_report_limitations() {
        let source = athletes();
        let mut config = QueryConfig {
            group_by: vec!["Year".into()],
            pivot_by: vec!["Sex".into()],
            aggregations: vec![Aggregation::sql_expression(
                "count(*)".into(),
                Some("Rows".into()),
            )],
            ..Default::default()
        };
        let mut trace = SqlTrace::default();
        let result = execute_traced(
            &source,
            &config,
            &[],
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap();
        assert_eq!(
            rows(&result)[0],
            vec![Some("2000".into()), Some("1".into()), Some("2".into())]
        );
        assert!(trace.text().contains("PIVOT"));
        config.aggregations[0].sql = Some("count(*) + 1".into());
        assert!(execute(&source, &config, &[]).is_ok());
        config.aggregations[0].sql = Some("sum(sum(\"Score\"))".into());
        assert!(execute(&source, &config, &[]).is_err());
        config.aggregations[0].sql = Some("count(*) FILTER (WHERE true)".into());
        assert!(
            execute(&source, &config, &[])
                .err()
                .unwrap()
                .contains("DuckDB ignores FILTER")
        );
        config.aggregations[0].sql =
            Some("first(\"Name\" ORDER BY \"Score\" DESC) -- FILTER (WHERE true)".into());
        execute(&source, &config, &[]).unwrap();
        config.aggregations[0].sql = Some("sum(\"missing\")".into());
        assert!(execute(&source, &config, &[]).is_err());
    }

    #[test]
    fn aggregation_headers_follow_duckdb_with_or_without_sql_aliases() {
        let source = athletes();
        let headers = |store: &DuckDbStore| {
            store
                .schema()
                .iter()
                .map(|column| column.name.clone())
                .collect::<Vec<_>>()
        };
        for expressions in [
            vec!["count(*)"],
            vec!["count(*) AS Rows"],
            vec!["sum(\"Score\") AS \"Total score\"", "count(*)"],
            vec!["count(*) -- comment\n", "count(*)"],
            vec!["sum(\"Score\") total", "count(*) AS total"],
        ] {
            for pivot in [false, true] {
                let config = QueryConfig {
                    group_by: vec!["Year".into()],
                    pivot_by: if pivot { vec!["Sex".into()] } else { vec![] },
                    aggregations: expressions
                        .iter()
                        .map(|sql| Aggregation::sql_expression((*sql).into(), None))
                        .collect(),
                    ..Default::default()
                };
                let projection = expressions.join("\n, ");
                let sql = if pivot {
                    format!(
                        "PIVOT {} ON \"Sex\" USING {projection}\n GROUP BY \"Year\"",
                        source.table_sql()
                    )
                } else {
                    format!(
                        "SELECT \"Year\", {projection}\n FROM {} GROUP BY ALL",
                        source.table_sql()
                    )
                };
                let expected = source.materialize_query(&sql, &[]).unwrap();
                let result = execute(&source, &config, &[]).unwrap();
                assert_eq!(headers(&result), headers(&expected), "{sql}");
            }
        }
        let aggregation = crate::application::pivot_editor::default_aggregation(
            "Score".into(),
            &ColumnType::Int32,
        );
        let result = execute(
            &source,
            &QueryConfig {
                aggregations: vec![aggregation],
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        let expected = source
            .materialize_query(
                &format!("SELECT sum(\"Score\") FROM {}", source.table_sql()),
                &[],
            )
            .unwrap();
        assert_eq!(headers(&result), headers(&expected));
    }

    #[test]
    fn saved_aggregation_names_are_editable_sql_aliases() {
        let source = athletes();
        let name = "Total \"score\"";
        for mut aggregation in [
            crate::application::pivot_editor::default_aggregation(
                "Score".into(),
                &ColumnType::Int32,
            ),
            Aggregation::sql_expression("sum(\"Score\") -- trailing comment".into(), None),
        ] {
            aggregation.name = Some(name.into());
            let sql = editor_expression(&aggregation, source.schema()).unwrap();
            assert!(sql.ends_with("AS \"Total \"\"score\"\"\""));
            let result = execute(
                &source,
                &QueryConfig {
                    aggregations: vec![Aggregation::sql_expression(sql, None)],
                    ..Default::default()
                },
                &[],
            )
            .unwrap();
            assert_eq!(result.schema()[0].name, name);
        }
    }

    #[test]
    fn generated_editor_sql_quotes_identifiers_and_preserves_literal_parameters() {
        let source = DuckDbStore::from_rows(
            &[("a?b\"c".into(), "INTEGER".into())],
            [vec![Value::Int(10)]],
        )
        .unwrap();
        let mut aggregation = crate::application::pivot_editor::default_aggregation(
            "a?b\"c".into(),
            &ColumnType::Int32,
        );
        aggregation.operation = AggregationOp::Percentile;
        aggregation.percentile = Some(75.);
        let sql = editor_expression(&aggregation, source.schema()).unwrap();
        assert_eq!(sql, "quantile_cont(\"a?b\"\"c\", CAST(0.75 AS DOUBLE))");
        let result = execute(
            &source,
            &QueryConfig {
                aggregations: vec![Aggregation::sql_expression(sql, None)],
                ..Default::default()
            },
            &[],
        )
        .unwrap();
        assert_eq!(result.row_count(), 1);
    }
}

#[cfg(test)]
mod dimension_tests {
    use super::*;
    use crate::application::pivot_editor::default_aggregation;

    fn source() -> DuckDbStore {
        DuckDbStore::from_rows(
            &[
                ("region".into(), "VARCHAR".into()),
                ("kind".into(), "VARCHAR".into()),
                ("sales".into(), "INTEGER".into()),
                ("season".into(), "VARCHAR".into()),
                ("__qp_dimension_0".into(), "INTEGER".into()),
                ("a \" quote".into(), "VARCHAR".into()),
            ],
            [
                vec![
                    Value::Text("a".into()),
                    Value::Text("x".into()),
                    Value::Int(10),
                    Value::Text("yes".into()),
                    Value::Int(1),
                    Value::Text("q".into()),
                ],
                vec![
                    Value::Text("A".into()),
                    Value::Text("X".into()),
                    Value::Int(20),
                    Value::Text("yes".into()),
                    Value::Int(2),
                    Value::Text("q".into()),
                ],
                vec![
                    Value::Text("b".into()),
                    Value::Text("y".into()),
                    Value::Int(30),
                    Value::Text("no".into()),
                    Value::Int(3),
                    Value::Text("r".into()),
                ],
                vec![
                    Value::Null,
                    Value::Text("x".into()),
                    Value::Int(40),
                    Value::Text("yes".into()),
                    Value::Int(4),
                    Value::Null,
                ],
            ],
        )
        .unwrap()
    }
    fn rows(store: &DuckDbStore) -> Vec<Vec<Option<String>>> {
        store
            .fetch_row_ids(&store.initial_row_ids().unwrap())
            .unwrap()
            .into_iter()
            .map(|row| row.values)
            .collect()
    }
    fn strings(values: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
        values
            .iter()
            .map(|row| row.iter().map(|value| value.map(str::to_owned)).collect())
            .collect()
    }
    fn config() -> QueryConfig {
        QueryConfig {
            group_by: vec![Dimension::expression("upper(region) AS Code")],
            aggregations: vec![Aggregation::sql_expression(
                "sum(sales) AS Revenue".into(),
                None,
            )],
            ..Default::default()
        }
    }

    #[test]
    fn expression_grouping_preserves_names_null_order_and_source_measures() {
        let source = source();
        let mut config = config();
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(
            result
                .schema()
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["Code", "Revenue"]
        );
        assert_eq!(
            rows(&result),
            strings(&[
                &[Some("A"), Some("30.0")],
                &[Some("B"), Some("30.0")],
                &[None, Some("40.0")]
            ])
        );
        config.group_by = vec![Dimension::expression("upper(region) AS sales")];
        assert_eq!(
            rows(&execute(&source, &config, &[]).unwrap()),
            rows(&result)
        );
        config.group_by = vec![Dimension::expression("upper(region)")];
        config.aggregations.clear();
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(result.schema()[0].name, "upper(region)");
        assert_eq!(
            rows(&result),
            strings(&[&[Some("A")], &[Some("B")], &[None]])
        );
        config.group_by = vec![Dimension::expression("upper(\"a \"\" quote\") AS quoted")];
        assert_eq!(
            rows(&execute(&source, &config, &[]).unwrap()),
            strings(&[&[Some("Q")], &[Some("R")], &[None]])
        );
    }

    #[test]
    fn expressions_pivot_with_bound_slicers_and_multiple_dimensions() {
        let source = source();
        let mut config = config();
        config.pivot_by = vec![Dimension::expression("upper(kind) AS KindCode")];
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(
            result
                .schema()
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            ["Code", "X_Revenue", "Y_Revenue"]
        );
        assert_eq!(
            rows(&result),
            strings(&[
                &[Some("A"), Some("30.0"), None],
                &[Some("B"), None, Some("30.0")],
                &[None, Some("40.0"), None]
            ])
        );
        let slicers = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "season".into(),
            selected: vec![ScalarKey::String("yes".into())],
        }];
        let mut trace = SqlTrace::default();
        let filtered = execute_traced(
            &source,
            &config,
            &slicers,
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap();
        assert_eq!(filtered.schema().len(), 2);
        assert_eq!(
            rows(&filtered),
            strings(&[&[Some("A"), Some("30.0")], &[None, Some("40.0")]])
        );
        assert!(
            trace
                .statements
                .iter()
                .any(|statement| statement.stage == "Pivot column discovery"
                    && !statement.parameters.is_empty())
        );
        config
            .pivot_by
            .push(Dimension::expression("lower(season) AS SeasonCode"));
        assert_eq!(
            rows(&execute(&source, &config, &slicers).unwrap()),
            rows(&filtered)
        );
        config
            .group_by
            .push(Dimension::expression("length(region) AS Size"));
        assert_eq!(
            execute(&source, &config, &slicers).unwrap().schema()[1].name,
            "Size"
        );
        let empty = [SlicerDefinition {
            sql: false,
            condition: None,
            column: "season".into(),
            selected: vec![ScalarKey::String("absent".into())],
        }];
        assert_eq!(execute(&source, &config, &empty).unwrap().row_count(), 0);
    }

    #[test]
    fn value_filters_restrict_source_rows_before_grouping() {
        let source = source();
        let config = config();
        let filters = [
            ValueFilter {
                target: Dimension::expression("upper(region) AS Code"),
                value: None,
                exclude: true,
            },
            ValueFilter {
                target: "kind".into(),
                value: Some("y".into()),
                exclude: true,
            },
        ];
        let result = execute_filtered(
            &source,
            &config,
            &[],
            &filters,
            &EvaluationClock::system(),
            100,
            &mut SqlTrace::default(),
        )
        .unwrap();
        assert_eq!(rows(&result), strings(&[&[Some("A"), Some("30.0")]]));
    }

    #[test]
    fn derived_pivot_keeps_ordered_and_parameterized_structured_aggregations() {
        let source = source();
        let mut config = config();
        config.pivot_by = vec![Dimension::expression("upper(kind)")];
        let mut first = default_aggregation("sales".into(), &ColumnType::Int32);
        first.operation = AggregationOp::First;
        first.name = Some("FirstSale".into());
        config.aggregations = vec![first.clone()];
        assert_eq!(
            rows(&execute(&source, &config, &[]).unwrap())[0],
            vec![Some("A".into()), Some("10".into()), None]
        );
        first.operation = AggregationOp::Percentile;
        first.percentile = Some(50.);
        config.aggregations = vec![first];
        let mut trace = SqlTrace::default();
        let result = execute_traced(
            &source,
            &config,
            &[],
            &EvaluationClock::system(),
            &mut trace,
        )
        .unwrap();
        assert_eq!(rows(&result)[0][1].as_deref(), Some("15.0"));
        assert!(
            trace
                .statements
                .iter()
                .any(|statement| statement.stage == "Pivot column discovery")
        );
        let result = execute_traced_limited(
            &source,
            &config,
            &[],
            &EvaluationClock::system(),
            1,
            &mut SqlTrace::default(),
        )
        .unwrap();
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn date_expression_groups_by_month() {
        let source = DuckDbStore::from_rows(
            &[("day".into(), "VARCHAR".into())],
            [
                vec![Value::Text("2026-01-01".into())],
                vec![Value::Text("2026-01-25".into())],
                vec![Value::Text("2026-02-01".into())],
            ],
        )
        .unwrap();
        let config = QueryConfig {
            group_by: vec![Dimension::expression(
                "date_trunc('month', CAST(day AS DATE)) AS month",
            )],
            aggregations: vec![Aggregation::sql_expression("count(*) AS Rows".into(), None)],
            ..Default::default()
        };
        let result = execute(&source, &config, &[]).unwrap();
        assert_eq!(result.schema()[0].name, "month");
        let values = rows(&result);
        assert!(values[0][0].as_ref().unwrap().starts_with("2026-01-01"));
        assert_eq!(values[0][1].as_deref(), Some("2"));
        assert!(values[1][0].as_ref().unwrap().starts_with("2026-02-01"));
        assert_eq!(values[1][1].as_deref(), Some("1"));
    }

    #[test]
    fn dimension_binding_rejects_invalid_non_scalar_and_conflicting_names() {
        let source = source();
        for sql in [
            "",
            "sum(sales)",
            "count(*) AS total",
            "row_number() OVER ()",
            "upper(missing)",
            "region, kind",
            "region; SELECT 1",
            "?",
            "region AS",
            "*",
            "region FROM qp_data",
            "upper(region",
        ] {
            let config = QueryConfig {
                group_by: vec![Dimension::expression(sql)],
                ..Default::default()
            };
            assert!(
                validate_dimensions(&source, &config).is_err(),
                "accepted {sql}"
            );
        }
        for other in [
            Dimension::expression("lower(kind) AS code"),
            Dimension::expression("lower(kind) AS \"CODE\""),
        ] {
            let mut config = config();
            config.pivot_by = vec![other];
            assert!(
                validate_dimensions(&source, &config)
                    .unwrap_err()
                    .contains("duplicate dimension output name")
            );
        }
        let mut config = config();
        config
            .group_by
            .push(Dimension::expression("Code || 'x' AS Another"));
        assert!(validate_dimensions(&source, &config).is_err());
        assert!(
            source
                .dimension_name("upper(region) /* AS false */ AS \"real AS name\" -- trailing")
                .is_ok()
        );
        assert!(
            source
                .dimension_name("CAST(sales AS VARCHAR) AS TextSale")
                .is_ok()
        );
        assert!(
            source
                .dimension_name("CASE WHEN kind = 'AS' THEN $tag$AS$tag$ ELSE 'other' END AS label")
                .is_ok()
        );
    }
}
