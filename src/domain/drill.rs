//! Maps a query-result cell back to the source rows it summarizes.

use crate::domain::documents::{QueryConfig, ValueFilter};

/// Filters selecting the source rows behind one query-result cell: the row's
/// group-by keys, plus the pivot keys when `column_name` is a pivoted measure.
/// `group_values` holds the row's group-by cells in order, and `pivot_keys` the
/// distinct rendered pivot-by combinations that produced the result's columns.
pub fn cell_filters(
    config: &QueryConfig,
    group_values: &[Option<String>],
    column_name: &str,
    on_group_column: bool,
    pivot_keys: &[Vec<Option<String>>],
) -> Result<Vec<ValueFilter>, String> {
    let mut filters = config
        .group_by
        .iter()
        .zip(group_values)
        .map(|(target, value)| ValueFilter {
            target: target.clone(),
            value: value.clone(),
            exclude: false,
        })
        .collect::<Vec<_>>();
    if on_group_column || config.pivot_by.is_empty() {
        return Ok(filters);
    }
    // A row total covers every pivot value, so only the group keys apply.
    if config.totals && crate::domain::pivot::is_total_column(column_name) {
        if pivot_column_keys(column_name, pivot_keys).is_ok() {
            return Err(format!(
                "Column {column_name} matches both a pivot value and the row total; \
                 drill down from a group-by cell instead"
            ));
        }
        return Ok(filters);
    }
    let keys = pivot_column_keys(column_name, pivot_keys)?;
    filters.extend(
        config
            .pivot_by
            .iter()
            .zip(keys)
            .map(|(target, value)| ValueFilter {
                target: target.clone(),
                value: value.clone(),
                exclude: false,
            }),
    );
    Ok(filters)
}

/// DuckDB names pivoted columns by joining the key values with `_`, followed by
/// `_` and the measure name when the measure is named or there are several.
fn pivot_column_keys<'a>(
    column_name: &str,
    pivot_keys: &'a [Vec<Option<String>>],
) -> Result<&'a [Option<String>], String> {
    let label = |keys: &[Option<String>]| {
        keys.iter()
            .map(|key| key.as_deref().unwrap_or("NULL"))
            .collect::<Vec<_>>()
            .join("_")
    };
    if let Some(keys) = pivot_keys.iter().find(|keys| label(keys) == column_name) {
        return Ok(keys);
    }
    let mut matches = pivot_keys.iter().filter(|keys| {
        column_name
            .strip_prefix(&label(keys))
            .is_some_and(|rest| rest.starts_with('_'))
    });
    match (matches.next(), matches.next()) {
        (Some(keys), None) => Ok(keys),
        (None, _) => Err(format!(
            "Cannot find the pivot values for column {column_name}"
        )),
        (Some(_), Some(_)) => Err(format!(
            "Column {column_name} matches several pivot values; drill down from a group-by cell instead"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::Dimension;

    fn keys(values: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
        values
            .iter()
            .map(|row| row.iter().map(|value| value.map(str::to_owned)).collect())
            .collect()
    }

    fn config() -> QueryConfig {
        QueryConfig {
            group_by: vec!["region".into(), Dimension::expression("year(day) AS y")],
            pivot_by: vec!["kind".into(), "size".into()],
            ..Default::default()
        }
    }

    #[test]
    fn group_cells_use_only_group_keys_and_measures_add_pivot_keys() {
        let config = config();
        let group_values = [Some("East".to_owned()), None];
        let pivots = keys(&[
            &[Some("a"), Some("S")],
            &[Some("a"), None],
            &[Some("a_b"), Some("S")],
        ]);
        let group_only = cell_filters(&config, &group_values, "region", true, &pivots).unwrap();
        assert_eq!(group_only.len(), 2);
        assert_eq!(
            group_only[1].target,
            Dimension::expression("year(day) AS y")
        );
        assert_eq!(group_only[1].value, None);

        let named = cell_filters(&config, &group_values, "a_NULL_total", false, &pivots).unwrap();
        assert_eq!(
            named[2..]
                .iter()
                .map(|filter| (filter.target.to_string(), filter.value.clone()))
                .collect::<Vec<_>>(),
            [("kind".into(), Some("a".into())), ("size".into(), None)]
        );
        // An exact name wins over a longer key whose prefix also matches.
        let exact = cell_filters(&config, &group_values, "a_b_S", false, &pivots).unwrap();
        assert_eq!(exact[2].value.as_deref(), Some("a_b"));
        assert!(cell_filters(&config, &group_values, "z_S", false, &pivots).is_err());
    }

    #[test]
    fn row_total_cells_use_only_group_keys() {
        let mut config = config();
        config.totals = true;
        let pivots = keys(&[&[Some("a"), Some("b")]]);
        let group_values = vec![Some("North".into()), None];
        let filters = cell_filters(&config, &group_values, "(Total)", false, &pivots).unwrap();
        assert_eq!(filters.len(), 2);
        let named = cell_filters(&config, &group_values, "(Total)_sum", false, &pivots).unwrap();
        assert_eq!(named.len(), 2);
        // A pivot value spelled like the total is ambiguous.
        let clash = keys(&[&[Some("(Total)"), Some("x")]]);
        assert!(cell_filters(&config, &group_values, "(Total)_x", false, &clash).is_err());
        // Without totals the name is an ordinary pivot column.
        config.totals = false;
        assert!(cell_filters(&config, &group_values, "(Total)", false, &pivots).is_err());
    }

    #[test]
    fn ambiguous_pivot_names_are_rejected() {
        let config = config();
        let pivots = keys(&[&[Some("a"), Some("b_c")], &[Some("a_b"), Some("c")]]);
        let error = cell_filters(&config, &[None, None], "a_b_c_sum", false, &pivots).unwrap_err();
        assert!(error.contains("several"));
        let unpivoted = QueryConfig {
            pivot_by: Vec::new(),
            ..config
        };
        assert_eq!(
            cell_filters(&unpivoted, &[None, None], "sum", false, &[])
                .unwrap()
                .len(),
            2
        );
    }
}
