use crate::domain::formatting::{numeric_type_name, string_type_name};
use crate::domain::frame::ResultSnapshot;
use crate::domain::selection::Selection;
use crate::infrastructure::duckdb_store::{DuckDbStore, RowIds};

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionStatistics {
    pub rows: usize,
    pub columns: usize,
    pub cells: usize,
    pub nulls: usize,
    /// Distinct non-null values of one column, or distinct rows across several.
    pub unique: usize,
    pub characters: Option<usize>,
    pub numeric_sum: Option<String>,
    pub mean: Option<f64>,
    pub single_numeric_column: Option<NumericColumnStatistics>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NumericColumnStatistics {
    pub mean: f64,
    pub median: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub sample_std_dev: Option<f64>,
}

pub(crate) struct ColumnAggregates {
    pub nulls: usize,
    pub characters: usize,
    pub numeric_count: usize,
    pub numeric_sum_as_f64: f64,
    pub integer_sum: Option<String>,
    pub integer_sum_overflow: bool,
    pub single_numeric: Option<NumericColumnStatistics>,
}

enum Sum {
    Empty,
    Integer(i128),
    Float(f64),
    Overflow,
}

pub fn calculate(
    result: &ResultSnapshot,
    selection: Selection,
) -> Result<SelectionStatistics, String> {
    let Some(selection) = selection.clamped(result.row_count(), result.column_count()) else {
        return Err("no cells selected".into());
    };
    let (row_range, column_range) = selection.rectangle();
    let start = *row_range.start();
    let end = row_range.end().saturating_add(1);
    let columns = column_range.clone().collect::<Vec<_>>();
    let selected_columns = columns
        .iter()
        .map(|&column| result.column_indices[column])
        .collect::<Vec<_>>();
    let selected_types = columns
        .iter()
        .map(|&column| result.dtypes[column].clone())
        .collect::<Vec<_>>();
    let (aggregates, unique) = {
        let store = result
            .store
            .lock()
            .map_err(|_| "DuckDB store lock poisoned".to_owned())?;
        let temporary_map;
        let row_map = match (&result.row_ids, result.row_map.as_deref()) {
            (RowIds::Natural(_), _) => None,
            (RowIds::Listed(_), Some(row_map)) => Some(row_map),
            (RowIds::Listed(ids), None) => {
                temporary_map = store.materialize_view_rows_from_ids(ids)?;
                temporary_map.row_map.as_deref()
            }
        };
        DuckDbStore::selection_column_aggregates(
            &store,
            row_map,
            start,
            end,
            &selected_columns,
            &selected_types,
        )?
    };

    let mut nulls = 0usize;
    let mut characters = 0usize;
    let mut sum = Sum::Empty;
    let mut float_sum = 0.0;
    let mut numeric_count = 0usize;
    for (column, aggregate) in columns.iter().zip(aggregates.iter()) {
        nulls = nulls.saturating_add(aggregate.nulls);
        characters = characters.saturating_add(aggregate.characters);
        if !numeric_type_name(&result.dtypes[*column]) {
            continue;
        }
        numeric_count = numeric_count.saturating_add(aggregate.numeric_count);
        float_sum += aggregate.numeric_sum_as_f64;
        let integer = is_integer_type(&result.dtypes[*column]);
        if aggregate.integer_sum_overflow {
            sum = Sum::Overflow;
        } else if integer {
            if let Some(value) = aggregate.integer_sum.as_ref() {
                let value = value
                    .parse::<i128>()
                    .map_err(|_| "DuckDB returned an invalid integer sum".to_owned())?;
                sum = match sum {
                    Sum::Empty => Sum::Integer(value),
                    Sum::Integer(previous) => previous
                        .checked_add(value)
                        .map(Sum::Integer)
                        .unwrap_or(Sum::Overflow),
                    Sum::Float(previous) => Sum::Float(previous + value as f64),
                    Sum::Overflow => Sum::Overflow,
                };
            }
        } else if aggregate.numeric_count > 0 {
            sum = match sum {
                Sum::Empty => Sum::Float(aggregate.numeric_sum_as_f64),
                Sum::Integer(previous) => {
                    Sum::Float(previous as f64 + aggregate.numeric_sum_as_f64)
                }
                Sum::Float(previous) => Sum::Float(previous + aggregate.numeric_sum_as_f64),
                Sum::Overflow => Sum::Overflow,
            };
        }
    }
    let numeric_sum = match sum {
        Sum::Empty => None,
        Sum::Integer(value) => Some(value.to_string()),
        Sum::Float(value) => Some(value.to_string()),
        Sum::Overflow => Some("Overflow".into()),
    };
    let mean = (numeric_count > 0)
        .then_some(float_sum / numeric_count as f64)
        .filter(|value| value.is_finite());
    let single_numeric_column = if columns.len() == 1 {
        aggregates
            .first()
            .and_then(|aggregate| aggregate.single_numeric.clone())
    } else {
        None
    };
    let row_count = end - start;
    let column_count = columns.len();
    Ok(SelectionStatistics {
        rows: row_count,
        columns: column_count,
        cells: row_count.saturating_mul(column_count),
        nulls,
        unique,
        characters: selected_types
            .iter()
            .any(|dtype| string_type_name(dtype))
            .then_some(characters),
        numeric_sum,
        mean,
        single_numeric_column,
    })
}

fn is_integer_type(dtype: &str) -> bool {
    let dtype = dtype.to_ascii_uppercase();
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
    .any(|prefix| dtype.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::frame::FrameSnapshot;
    use crate::domain::selection::Cell;
    use crate::infrastructure::duckdb_store::DuckDbStore;
    use duckdb::types::Value;

    fn result() -> ResultSnapshot {
        let store = DuckDbStore::from_rows(
            &[
                ("n".into(), "BIGINT".into()),
                ("s".into(), "VARCHAR".into()),
            ],
            [
                vec![Value::BigInt(0), Value::Text("😀".into())],
                vec![Value::Null, Value::Text("a".into())],
            ],
        )
        .unwrap();
        ResultSnapshot::from_source(&FrameSnapshot::new(store, 1), 1, 1)
    }

    #[test]
    fn zero_unicode_nulls_and_single_numeric_summary_are_retained() {
        let stats = calculate(
            &result(),
            Selection {
                anchor: Cell { row: 0, column: 0 },
                active: Cell { row: 1, column: 1 },
            },
        )
        .unwrap();
        assert_eq!(stats.numeric_sum.as_deref(), Some("0"));
        assert_eq!(stats.nulls, 1);
        assert_eq!(stats.characters, Some(2));
        assert_eq!(stats.unique, 2);
        let numeric_only = calculate(
            &result(),
            Selection {
                anchor: Cell { row: 0, column: 0 },
                active: Cell { row: 1, column: 0 },
            },
        )
        .unwrap();
        assert_eq!(numeric_only.characters, None);
        assert_eq!(numeric_only.unique, 1);
        assert_eq!(
            numeric_only.single_numeric_column.as_ref().unwrap().mean,
            0.0
        );
    }

    #[test]
    fn integer_sum_keeps_precision_and_reports_overflow() {
        let store = DuckDbStore::from_rows(
            &[("n".into(), "BIGINT".into())],
            [
                vec![Value::BigInt(9_007_199_254_740_993)],
                vec![Value::BigInt(2)],
            ],
        )
        .unwrap();
        let result = ResultSnapshot::from_source(&FrameSnapshot::new(store, 1), 1, 1);
        let stats = calculate(
            &result,
            Selection {
                anchor: Cell { row: 0, column: 0 },
                active: Cell { row: 1, column: 0 },
            },
        )
        .unwrap();
        assert_eq!(stats.numeric_sum.as_deref(), Some("9007199254740995"));
        assert_eq!(stats.unique, 2);
    }

    #[test]
    fn text_and_all_null_numeric_selections_have_no_numeric_summary() {
        let result = result();
        let text = calculate(
            &result,
            Selection {
                anchor: Cell { row: 0, column: 1 },
                active: Cell { row: 1, column: 1 },
            },
        )
        .unwrap();
        assert_eq!(text.characters, Some(2));
        assert_eq!(text.unique, 2);
        assert_eq!(text.numeric_sum, None);
        assert_eq!(text.mean, None);
        assert_eq!(text.single_numeric_column, None);
        let null = calculate(
            &result,
            Selection {
                anchor: Cell { row: 1, column: 0 },
                active: Cell { row: 1, column: 0 },
            },
        )
        .unwrap();
        assert_eq!(null.nulls, 1);
        assert_eq!(null.unique, 0);
        assert_eq!(null.characters, None);
        assert_eq!(null.numeric_sum, None);
        assert_eq!(null.mean, None);
        assert_eq!(null.single_numeric_column, None);
    }

    #[test]
    fn unique_counts_selected_display_rows_and_preserves_empty_strings() {
        use crate::domain::documents::SortKey;
        use crate::infrastructure::duckdb_store::ViewQuery;

        let store = DuckDbStore::from_rows(
            &[("text".into(), "VARCHAR".into())],
            ["b", "a", "a", ""]
                .map(|value| vec![Value::Text(value.into())])
                .into_iter()
                .chain([vec![Value::Null]]),
        )
        .unwrap();
        let source = FrameSnapshot::new(store, 1);
        let rows = source
            .store
            .lock()
            .unwrap()
            .query_view_rows(&ViewQuery {
                hidden: Vec::new(),
                value_filters: Vec::new(),
                tokens: vec![],
                filters: vec![],
                sort: vec![SortKey {
                    column: "text".into(),
                    descending: false,
                }],
            })
            .unwrap();
        let schema = source.store.lock().unwrap().schema().to_vec();
        let result = ResultSnapshot::from_view(&source, rows, schema, 1, 1);
        let selected = |first, last| Selection {
            anchor: Cell {
                row: first,
                column: 0,
            },
            active: Cell {
                row: last,
                column: 0,
            },
        };
        assert_eq!(calculate(&result, selected(0, 4)).unwrap().unique, 3);
        assert_eq!(calculate(&result, selected(1, 2)).unwrap().unique, 1);
        assert_eq!(calculate(&result, selected(0, 0)).unwrap().unique, 1);
        assert_eq!(calculate(&result, selected(4, 4)).unwrap().unique, 0);
    }

    #[test]
    fn unique_counts_large_integers_without_float_conversion() {
        let store = DuckDbStore::from_rows(
            &[("number".into(), "BIGINT".into())],
            [
                9_007_199_254_740_992,
                9_007_199_254_740_993,
                9_007_199_254_740_993,
            ]
            .map(|value| vec![Value::BigInt(value)]),
        )
        .unwrap();
        let result = ResultSnapshot::from_source(&FrameSnapshot::new(store, 1), 1, 1);
        let stats = calculate(
            &result,
            Selection {
                anchor: Cell { row: 0, column: 0 },
                active: Cell { row: 2, column: 0 },
            },
        )
        .unwrap();
        assert_eq!(stats.unique, 2);
    }

    #[test]
    fn unique_counts_distinct_rows_across_selected_columns() {
        let store = DuckDbStore::from_rows(
            &[
                ("key".into(), "VARCHAR".into()),
                ("part".into(), "BIGINT".into()),
            ],
            [
                vec![Value::Text("a".into()), Value::BigInt(1)],
                vec![Value::Text("a".into()), Value::BigInt(2)],
                vec![Value::Text("a".into()), Value::BigInt(1)],
                vec![Value::Text("b".into()), Value::Null],
            ],
        )
        .unwrap();
        let result = ResultSnapshot::from_source(&FrameSnapshot::new(store, 1), 1, 1);
        let selected = |last| Selection {
            anchor: Cell { row: 0, column: 0 },
            active: Cell {
                row: last,
                column: 1,
            },
        };
        // (a, 1) repeats in row 2; a row with a NULL part is still a distinct row.
        assert_eq!(calculate(&result, selected(1)).unwrap().unique, 2);
        assert_eq!(calculate(&result, selected(3)).unwrap().unique, 3);
    }
}
