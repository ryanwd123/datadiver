use crate::domain::formatting::group_numeric_text;
use crate::domain::statistics::SelectionStatistics;

pub(super) fn dimensions(rows: usize, columns: usize) -> String {
    format!(
        "{}R×{}C",
        group_numeric_text(&rows.to_string()),
        group_numeric_text(&columns.to_string())
    )
}

/// Follows the dimensions in the status bar, so it starts with a separator.
pub(super) fn filtered_rows(shown: usize, total: usize) -> String {
    format!(
        "- {} of {} rows",
        group_numeric_text(&shown.to_string()),
        group_numeric_text(&total.to_string())
    )
}

/// Shown with the red limit input when a query result stopped at the limit.
pub(super) fn limited_rows(limit: u64) -> String {
    format!("Limited to {} rows", group_numeric_text(&limit.to_string()))
}

fn number(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let precision = if value.abs() > 100.0 {
        1
    } else if value.abs() > 1.0 {
        2
    } else {
        4
    };
    let rounded = format!("{value:.precision$}");
    // Rounding small negative values to zero should not leave a minus sign.
    let rounded = if rounded.parse::<f64>().ok() == Some(0.0) {
        rounded.trim_start_matches('-')
    } else {
        &rounded
    };
    Some(group_numeric_text(rounded))
}

/// True when every selected row holds a different value (or, across several
/// columns, a different combination), so the selection is a unique key.
pub(super) fn selection_is_distinct(stats: &SelectionStatistics) -> bool {
    stats.rows > 1 && stats.unique == stats.rows
}

pub(super) fn nulls_label(stats: &SelectionStatistics) -> String {
    format!("Nulls: {}", group_numeric_text(&stats.nulls.to_string()))
}

/// The unique count label; several columns count distinct rows.
pub(super) fn unique_label(stats: &SelectionStatistics) -> String {
    let name = if stats.columns > 1 {
        "Unique rows"
    } else {
        "Unique"
    };
    format!("{name}: {}", group_numeric_text(&stats.unique.to_string()))
}

/// The statistics after the null and unique counts.
pub(super) fn statistics(stats: &SelectionStatistics) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(characters) = stats.characters {
        labels.push(format!(
            "Characters: {}",
            group_numeric_text(&characters.to_string())
        ));
    }
    if let Some(sum) = &stats.numeric_sum {
        let formatted = if sum.parse::<i128>().is_ok() || sum == "Overflow" {
            Some(group_numeric_text(sum))
        } else {
            sum.parse::<f64>().ok().and_then(number)
        };
        if let Some(sum) = formatted {
            labels.push(format!("Sum: {sum}"));
        }
    }
    if let Some(mean) = stats.mean.and_then(number) {
        labels.push(format!("Mean: {mean}"));
    }
    if let Some(column) = &stats.single_numeric_column {
        for (label, value) in [
            ("Median", Some(column.median)),
            ("Min", Some(column.minimum)),
            ("Max", Some(column.maximum)),
            ("Std dev", column.sample_std_dev),
        ] {
            if let Some(value) = value.and_then(number) {
                labels.push(format!("{label}: {value}"));
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::statistics::NumericColumnStatistics;

    #[test]
    fn dimensions_use_grouped_counts() {
        assert_eq!(dimensions(1_234_567, 2_000), "1,234,567R×2,000C");
        assert_eq!(dimensions(0, 0), "0R×0C");
        assert_eq!(filtered_rows(1_234, 50_000), "- 1,234 of 50,000 rows");
        assert_eq!(limited_rows(1_000_000), "Limited to 1,000,000 rows");
    }

    #[test]
    fn numeric_precision_follows_absolute_magnitude() {
        for (value, expected) in [
            (1234.5678, "1,234.6"),
            (-1234.5678, "-1,234.6"),
            (12.34567, "12.35"),
            (-12.34567, "-12.35"),
            (0.123456, "0.1235"),
            (-0.000001, "0.0000"),
            (100.0, "100.00"),
            (1.0, "1.0000"),
            (0.0, "0.0000"),
        ] {
            assert_eq!(number(value).as_deref(), Some(expected));
        }
        assert_eq!(number(f64::NAN), None);
        assert_eq!(number(f64::INFINITY), None);
    }

    fn empty_stats() -> SelectionStatistics {
        SelectionStatistics {
            rows: 1,
            columns: 1,
            cells: 1,
            nulls: 0,
            unique: 0,
            characters: None,
            numeric_sum: None,
            mean: None,
            single_numeric_column: None,
        }
    }

    #[test]
    fn only_defined_statistics_are_shown_and_zero_is_preserved() {
        let mut stats = empty_stats();
        assert_eq!(nulls_label(&stats), "Nulls: 0");
        assert!(statistics(&stats).is_empty());
        stats.characters = Some(0);
        assert_eq!(statistics(&stats), ["Characters: 0"]);
        stats.characters = None;
        stats.numeric_sum = Some("9007199254740995".into());
        stats.mean = Some(0.0);
        stats.single_numeric_column = Some(NumericColumnStatistics {
            mean: 0.0,
            median: 0.0,
            minimum: -1000.123,
            maximum: 1000.123,
            sample_std_dev: None,
        });
        assert_eq!(
            statistics(&stats),
            [
                "Sum: 9,007,199,254,740,995",
                "Mean: 0.0000",
                "Median: 0.0000",
                "Min: -1,000.1",
                "Max: 1,000.1",
            ]
        );
        stats.single_numeric_column.as_mut().unwrap().sample_std_dev = Some(2.3456);
        assert_eq!(statistics(&stats).last().unwrap(), "Std dev: 2.35");
    }

    #[test]
    fn unique_counts_are_grouped_and_name_rows_for_several_columns() {
        let mut stats = empty_stats();
        stats.unique = 1234;
        assert_eq!(unique_label(&stats), "Unique: 1,234");
        stats.unique = 0;
        assert_eq!(unique_label(&stats), "Unique: 0");
        stats.columns = 2;
        assert_eq!(unique_label(&stats), "Unique rows: 0");
    }

    #[test]
    fn distinct_selection_needs_every_row_unique_and_more_than_one_row() {
        let mut stats = empty_stats();
        stats.unique = 1;
        assert!(!selection_is_distinct(&stats));
        stats.rows = 3;
        stats.unique = 3;
        assert!(selection_is_distinct(&stats));
        stats.unique = 2;
        assert!(!selection_is_distinct(&stats));
    }
}
