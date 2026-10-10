//! Heat-map shading of a query's measure columns.

use crate::domain::documents::QueryConfig;
use crate::domain::formatting::numeric_type_name;
use crate::domain::pivot::is_total_column;

/// The result columns a heat map shades, in groups that share one color
/// scale. A group-by query scales each measure on its own. A pivot repeats
/// each aggregate once per pivot value, so those columns share a scale and
/// stay comparable. Group-by keys, text, and row-total columns (which would
/// set every scale) are not shaded.
pub fn scale_groups(config: &QueryConfig, columns: &[(String, String)]) -> Vec<Vec<String>> {
    let measures = columns
        .iter()
        .skip(config.group_by.len())
        .filter(|(name, _)| !is_total_column(name))
        .collect::<Vec<_>>();
    // DuckDB's PIVOT lists each pivot value's aggregates in order.
    let aggregates = config.aggregations.len().max(1);
    let count = if config.pivot_by.is_empty() {
        measures.len()
    } else {
        aggregates
    };
    let mut groups = vec![Vec::new(); count];
    for (index, (name, dtype)) in measures.into_iter().enumerate() {
        if numeric_type_name(dtype) {
            groups[index % count].push(name.clone());
        }
    }
    groups.retain(|group| !group.is_empty());
    groups
}

/// The range spanning every column range in a group.
pub fn combined(ranges: impl IntoIterator<Item = Option<(f64, f64)>>) -> Option<(f64, f64)> {
    ranges
        .into_iter()
        .flatten()
        .reduce(|(min, max), (low, high)| (min.min(low), max.max(high)))
}

/// Where `value` falls in its column's `(min, max)`, from 0 to 1. A column
/// whose values are all equal shades every cell halfway.
pub fn fraction(value: f64, (min, max): (f64, f64)) -> f32 {
    if !value.is_finite() || max <= min {
        return 0.5;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::{Aggregation, Dimension};

    fn columns(columns: &[(&str, &str)]) -> Vec<(String, String)> {
        columns
            .iter()
            .map(|(name, dtype)| (name.to_string(), dtype.to_string()))
            .collect()
    }

    #[test]
    fn group_by_measures_scale_alone_and_keys_text_and_totals_are_unshaded() {
        let config = QueryConfig {
            group_by: vec![Dimension::Column("region".into())],
            ..Default::default()
        };
        let columns = columns(&[
            ("region", "VARCHAR"),
            ("sales", "HUGEINT"),
            ("mean", "DOUBLE"),
            ("label", "VARCHAR"),
        ]);
        assert_eq!(scale_groups(&config, &columns), [["sales"], ["mean"]]);
    }

    #[test]
    fn pivot_columns_of_one_aggregate_share_a_scale() {
        let aggregation = |name: &str| Aggregation {
            name: Some(name.into()),
            ..Aggregation::sql_expression("count(*)".into(), None)
        };
        let mut config = QueryConfig {
            group_by: vec![Dimension::Column("region".into())],
            pivot_by: vec![Dimension::Column("kind".into())],
            aggregations: vec![aggregation("n")],
            ..Default::default()
        };
        let single = columns(&[
            ("region", "VARCHAR"),
            ("a", "BIGINT"),
            ("b", "BIGINT"),
            ("(Total)", "BIGINT"),
        ]);
        assert_eq!(scale_groups(&config, &single), [["a", "b"]]);
        config.aggregations.push(aggregation("mean"));
        let double = columns(&[
            ("region", "VARCHAR"),
            ("a_n", "BIGINT"),
            ("a_mean", "DOUBLE"),
            ("b_n", "BIGINT"),
            ("b_mean", "DOUBLE"),
            ("(Total)_n", "BIGINT"),
            ("(Total)_mean", "DOUBLE"),
        ]);
        assert_eq!(
            scale_groups(&config, &double),
            [["a_n", "b_n"], ["a_mean", "b_mean"]]
        );
    }

    #[test]
    fn combined_ranges_span_every_column_and_fraction_scales() {
        assert_eq!(
            combined([Some((1.0, 5.0)), None, Some((-2.0, 3.0))]),
            Some((-2.0, 5.0))
        );
        assert_eq!(combined([None]), None);
        assert_eq!(fraction(5.0, (0.0, 10.0)), 0.5);
        assert_eq!(fraction(-1.0, (0.0, 10.0)), 0.0);
        assert_eq!(fraction(10.0, (0.0, 10.0)), 1.0);
        assert_eq!(fraction(3.0, (3.0, 3.0)), 0.5);
    }
}
