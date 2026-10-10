use std::collections::HashSet;

use crate::domain::documents::TableConfig;

pub fn resolved_order(available: &[String], config: &TableConfig) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut order = Vec::with_capacity(available.len());
    for name in config
        .column_order
        .iter()
        .map(String::as_str)
        .chain(available.iter().map(String::as_str))
    {
        if available.iter().any(|column| column == name) && seen.insert(name.to_string()) {
            order.push(name.to_string());
        }
    }
    order
}

/// The columns the grid shows: the resolved order without hidden columns.
pub fn visible_order(available: &[String], config: &TableConfig) -> Vec<String> {
    let mut order = resolved_order(available, config);
    order.retain(|name| !config.hidden.contains(name));
    order
}

/// Frozen columns the grid shows, for a config whose `column_order` is resolved.
pub fn visible_frozen_count(config: &TableConfig) -> usize {
    config
        .column_order
        .iter()
        .take(config.frozen_count)
        .filter(|name| !config.hidden.contains(name))
        .count()
}

/// How many of `columns` are frozen in a view that cannot reorder them: the
/// leading run that still matches `frozen` by name.
pub fn frozen_prefix(frozen: &[String], columns: &[String]) -> usize {
    frozen
        .iter()
        .zip(columns)
        .take_while(|(frozen, column)| frozen == column)
        .count()
}

/// Freezing without reordering: on a column right of the frozen ones, freezes
/// every column through it; on a frozen column, unfreezes it and those right
/// of it. Returns the new frozen column names.
pub fn toggle_frozen_prefix(columns: &[String], frozen: usize, active: usize) -> Vec<String> {
    let count = if active < frozen { active } else { active + 1 };
    columns[..count.min(columns.len())].to_vec()
}

/// Positions in `order` of the columns that are not hidden.
fn visible_positions(config: &TableConfig, order: &[String]) -> Vec<usize> {
    order
        .iter()
        .enumerate()
        .filter(|(_, name)| !config.hidden.contains(name))
        .map(|(index, _)| index)
        .collect()
}

/// Moves visible column `active` past its visible neighbour. `order` is the full
/// resolved order, so hidden columns keep their places. Returns the new visible index.
pub fn move_column(
    config: &mut TableConfig,
    order: &[String],
    active: usize,
    direction: isize,
) -> Option<usize> {
    let visible = visible_positions(config, order);
    let from = *visible.get(active)?;
    let destination = active.checked_add_signed(direction)?;
    let to = *visible.get(destination)?;
    let frozen = config.frozen_count.min(order.len());
    if (from < frozen) != (to < frozen) {
        return None;
    }
    let mut order = order.to_vec();
    order.swap(from, to);
    config.column_order = order;
    Some(destination)
}

/// Freezes or unfreezes visible column `active`; `order` is the full resolved order.
/// Returns the column's new visible index.
pub fn toggle_frozen(config: &mut TableConfig, order: &[String], active: usize) -> Option<usize> {
    let from = *visible_positions(config, order).get(active)?;
    let mut order = order.to_vec();
    let frozen = config.frozen_count.min(order.len());
    let name = order.remove(from);
    let destination = if from < frozen {
        config.frozen_count = frozen - 1;
        frozen - 1
    } else {
        config.frozen_count = frozen + 1;
        frozen
    };
    order.insert(destination, name.clone());
    config.column_order = order;
    visible_positions(config, &config.column_order)
        .iter()
        .position(|index| config.column_order[*index] == name)
}

/// Hides or shows `name`. Returns whether anything changed.
pub fn set_hidden(config: &mut TableConfig, name: &str, hidden: bool) -> bool {
    let present = config.hidden.iter().any(|column| column == name);
    if hidden == present {
        return false;
    }
    if hidden {
        config.hidden.push(name.to_owned());
    } else {
        config.hidden.retain(|column| column != name);
    }
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn frozen_prefixes_never_reorder() {
        use super::{frozen_prefix, toggle_frozen_prefix};
        let columns = ["region", "product", "2023", "2024"]
            .map(String::from)
            .to_vec();
        // On the second column: the first two freeze.
        let frozen = toggle_frozen_prefix(&columns, 0, 1);
        assert_eq!(frozen, ["region", "product"]);
        assert_eq!(frozen_prefix(&frozen, &columns), 2);
        // Again on it: it unfreezes, the first stays.
        assert_eq!(toggle_frozen_prefix(&columns, 2, 1), ["region"]);
        // On the first column everything unfreezes; further right freezes more.
        assert!(toggle_frozen_prefix(&columns, 2, 0).is_empty());
        assert_eq!(toggle_frozen_prefix(&columns, 2, 3).len(), 4);
        // After a rerun only the part still leading the result stays frozen.
        let rerun = ["region", "month", "product"].map(String::from).to_vec();
        assert_eq!(frozen_prefix(&frozen, &rerun), 1);
        assert_eq!(frozen_prefix(&frozen, &rerun[1..]), 0);
    }

    use super::*;

    #[test]
    fn saved_order_skips_missing_and_appends_new_columns() {
        let available = vec!["a".into(), "b".into(), "c".into()];
        let config = TableConfig {
            column_order: vec!["b".into(), "missing".into(), "b".into(), "a".into()],
            ..Default::default()
        };
        assert_eq!(resolved_order(&available, &config), vec!["b", "a", "c"]);
    }

    #[test]
    fn freeze_appends_and_unfreeze_precedes_scrollable_columns() {
        let mut config = TableConfig::default();
        let columns = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(toggle_frozen(&mut config, &columns, 2), Some(0));
        assert_eq!(config.column_order, vec!["c", "a", "b"]);
        assert_eq!(config.frozen_count, 1);
        let order = config.column_order.clone();
        assert_eq!(toggle_frozen(&mut config, &order, 0), Some(0));
        assert_eq!(config.frozen_count, 0);
    }

    #[test]
    fn movement_stays_inside_its_pane() {
        let mut config = TableConfig {
            frozen_count: 1,
            ..Default::default()
        };
        let columns = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(move_column(&mut config, &columns, 1, -1), None);
        assert_eq!(move_column(&mut config, &columns, 1, 1), Some(2));
        assert_eq!(config.column_order, vec!["a", "c", "b"]);
    }

    #[test]
    fn hidden_columns_keep_their_places_when_visible_columns_move() {
        let mut config = TableConfig {
            hidden: vec!["b".into()],
            ..Default::default()
        };
        let available: Vec<String> = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        assert_eq!(visible_order(&available, &config), vec!["a", "c", "d"]);
        let order = resolved_order(&available, &config);
        // Visible "a" swaps with its visible neighbour "c"; hidden "b" stays put.
        assert_eq!(move_column(&mut config, &order, 0, 1), Some(1));
        assert_eq!(config.column_order, vec!["c", "b", "a", "d"]);
        assert_eq!(move_column(&mut config, &order, 2, 1), None);
    }

    #[test]
    fn freezing_reports_visible_indices_and_counts() {
        let mut config = TableConfig {
            hidden: vec!["a".into()],
            ..Default::default()
        };
        let order: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        // Visible index 1 is "c".
        assert_eq!(toggle_frozen(&mut config, &order, 1), Some(0));
        assert_eq!(config.column_order, vec!["c", "a", "b"]);
        assert_eq!(visible_frozen_count(&config), 1);
        assert!(set_hidden(&mut config, "c", true));
        assert!(!set_hidden(&mut config, "c", true));
        assert_eq!(visible_frozen_count(&config), 0);
        assert!(set_hidden(&mut config, "a", false));
        assert_eq!(config.hidden, vec!["c"]);
    }
}
