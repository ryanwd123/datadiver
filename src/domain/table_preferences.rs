use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use super::documents::{Alignment, ColumnPresentation, SlicerDefinition, TableConfig};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TablePreferences {
    pub schema_version: u32,
    pub source_path: String,
    #[serde(default)]
    pub widths: BTreeMap<String, f32>,
    #[serde(default)]
    pub column_order: Vec<String>,
    #[serde(default)]
    pub frozen_columns: Vec<String>,
    #[serde(default)]
    pub slicer_columns: Vec<String>,
    /// The slicer columns shown as condition cards. Their text is not kept,
    /// just as value cards keep no selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub condition_slicers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_columns: Vec<String>,
}

impl TablePreferences {
    pub fn new(source_path: String) -> Self {
        Self {
            schema_version: 1,
            source_path,
            widths: BTreeMap::new(),
            column_order: Vec::new(),
            frozen_columns: Vec::new(),
            slicer_columns: Vec::new(),
            condition_slicers: Vec::new(),
            hidden_columns: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("unsupported table preferences version".into());
        }
        if self.widths.values().any(|width| !valid_width(*width)) {
            return Err("table width must be finite and between 40 and 1200".into());
        }
        for names in [
            &self.column_order,
            &self.frozen_columns,
            &self.slicer_columns,
            &self.hidden_columns,
        ] {
            let mut seen = HashSet::new();
            if names.iter().any(|name| !seen.insert(name)) {
                return Err("duplicate column in table preferences".into());
            }
        }
        Ok(())
    }

    pub fn config(&self, available: &[String]) -> TableConfig {
        let mut seen = HashSet::new();
        let order = self
            .frozen_columns
            .iter()
            .chain(&self.column_order)
            .chain(available)
            .filter(|name| available.contains(name) && seen.insert((*name).clone()))
            .cloned()
            .collect();
        TableConfig {
            column_order: order,
            frozen_count: self
                .frozen_columns
                .iter()
                .filter(|name| available.contains(name))
                .count(),
            presentation: self
                .widths
                .iter()
                .map(|(name, width)| {
                    (
                        name.clone(),
                        ColumnPresentation {
                            width: *width,
                            alignment: Alignment::Left,
                            format_override: None,
                        },
                    )
                })
                .collect(),
            hidden: self.hidden_columns.clone(),
            ..Default::default()
        }
    }

    pub fn slicers(&self) -> Vec<SlicerDefinition> {
        self.slicer_columns
            .iter()
            .map(|column| SlicerDefinition {
                sql: false,
                condition: self.condition_slicers.contains(column).then(String::new),
                column: column.clone(),
                selected: Vec::new(),
            })
            .collect()
    }

    pub fn update_layout(&mut self, config: &TableConfig, available: &[String]) {
        // Missing identities must survive a temporary schema change.
        let missing_order = self
            .column_order
            .iter()
            .filter(|name| !available.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        let missing_frozen = self
            .frozen_columns
            .iter()
            .filter(|name| !available.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        let displayed = super::table::resolved_order(available, config);
        self.frozen_columns = displayed
            .iter()
            .take(config.frozen_count)
            .cloned()
            .chain(missing_frozen)
            .collect();
        self.column_order = displayed.into_iter().chain(missing_order).collect();
        self.hidden_columns = config.hidden.clone();
        for (name, presentation) in &config.presentation {
            if valid_width(presentation.width) {
                self.widths.insert(name.clone(), presentation.width);
            }
        }
    }
}

pub fn valid_width(width: f32) -> bool {
    width.is_finite() && (40.0..=1200.0).contains(&width)
}

#[derive(Clone, Debug, Default)]
pub struct TablePreferencesPatch {
    pub reset: bool,
    pub widths: BTreeMap<String, Option<f32>>,
    pub column_order: Option<Vec<String>>,
    pub frozen_columns: Option<Vec<String>>,
    pub slicer_columns: Option<Vec<String>>,
    pub condition_slicers: Option<Vec<String>>,
    pub hidden_columns: Option<Vec<String>>,
}

impl TablePreferencesPatch {
    pub fn between(old: &TablePreferences, new: &TablePreferences) -> Self {
        let mut widths = BTreeMap::new();
        for name in old.widths.keys().chain(new.widths.keys()) {
            if old.widths.get(name) != new.widths.get(name) {
                widths.insert(name.clone(), new.widths.get(name).copied());
            }
        }
        Self {
            reset: false,
            widths,
            column_order: (old.column_order != new.column_order).then(|| new.column_order.clone()),
            frozen_columns: (old.frozen_columns != new.frozen_columns)
                .then(|| new.frozen_columns.clone()),
            slicer_columns: (old.slicer_columns != new.slicer_columns)
                .then(|| new.slicer_columns.clone()),
            condition_slicers: (old.condition_slicers != new.condition_slicers)
                .then(|| new.condition_slicers.clone()),
            hidden_columns: (old.hidden_columns != new.hidden_columns)
                .then(|| new.hidden_columns.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.reset
            && self.widths.is_empty()
            && self.column_order.is_none()
            && self.frozen_columns.is_none()
            && self.slicer_columns.is_none()
            && self.condition_slicers.is_none()
            && self.hidden_columns.is_none()
    }

    pub fn merge(&mut self, newer: Self) {
        if newer.reset {
            *self = newer;
            return;
        }
        self.widths.extend(newer.widths);
        if newer.column_order.is_some() {
            self.column_order = newer.column_order;
        }
        if newer.frozen_columns.is_some() {
            self.frozen_columns = newer.frozen_columns;
        }
        if newer.slicer_columns.is_some() {
            self.slicer_columns = newer.slicer_columns;
        }
        if newer.condition_slicers.is_some() {
            self.condition_slicers = newer.condition_slicers;
        }
        if newer.hidden_columns.is_some() {
            self.hidden_columns = newer.hidden_columns;
        }
    }

    pub fn apply(&self, preferences: &mut TablePreferences) {
        if self.reset {
            *preferences = TablePreferences::new(preferences.source_path.clone());
        }
        for (name, width) in &self.widths {
            if let Some(width) = width {
                preferences.widths.insert(name.clone(), *width);
            } else {
                preferences.widths.remove(name);
            }
        }
        if let Some(order) = &self.column_order {
            preferences.column_order = order.clone();
        }
        if let Some(frozen) = &self.frozen_columns {
            preferences.frozen_columns = frozen.clone();
        }
        if let Some(slicers) = &self.slicer_columns {
            preferences.slicer_columns = slicers.clone();
        }
        if let Some(conditions) = &self.condition_slicers {
            preferences.condition_slicers = conditions.clone();
        }
        if let Some(hidden) = &self.hidden_columns {
            preferences.hidden_columns = hidden.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_changes_keep_missing_identities_without_freezing_a_replacement() {
        let mut preferences = TablePreferences::new("source".into());
        preferences.column_order = vec!["b".into(), "a".into()];
        preferences.frozen_columns = vec!["b".into()];
        let available = vec!["a".into(), "new".into()];
        let mut config = preferences.config(&available);
        assert_eq!(config.frozen_count, 0);
        config.column_order.swap(0, 1);
        preferences.update_layout(&config, &available);
        assert!(preferences.column_order.contains(&"b".into()));
        let restored = preferences.config(&["a".into(), "b".into(), "new".into()]);
        assert_eq!(restored.column_order, ["b", "new", "a"]);
        assert_eq!(restored.frozen_count, 1);
    }

    #[test]
    fn hidden_columns_round_trip_through_layout_and_patches() {
        let original = TablePreferences::new("source".into());
        let available = vec!["a".into(), "b".into()];
        let mut config = original.config(&available);
        config.hidden.push("b".into());
        let mut updated = original.clone();
        updated.update_layout(&config, &available);
        assert_eq!(updated.hidden_columns, ["b"]);
        let mut patched = original.clone();
        TablePreferencesPatch::between(&original, &updated).apply(&mut patched);
        assert_eq!(patched.config(&available).hidden, ["b"]);
    }

    #[test]
    fn patches_merge_independent_columns_and_latest_same_field() {
        let original = TablePreferences::new("source".into());
        let mut first = original.clone();
        first.widths.insert("a".into(), 100.);
        let mut second = original.clone();
        second.widths.insert("b".into(), 200.);
        let mut patch = TablePreferencesPatch::between(&original, &first);
        patch.merge(TablePreferencesPatch::between(&original, &second));
        patch.apply(&mut first);
        assert_eq!(first.widths.len(), 2);
        second.widths.insert("a".into(), 300.);
        patch.merge(TablePreferencesPatch::between(&original, &second));
        patch.apply(&mut first);
        assert_eq!(first.widths["a"], 300.);
        assert!(!serde_json::to_string(&first).unwrap().contains("selected"));
    }

    #[test]
    fn reset_clears_other_windows_fields_and_later_edits_are_preserved() {
        let mut preferences = TablePreferences::new("source".into());
        preferences.widths.insert("other-window".into(), 200.);
        preferences.frozen_columns.push("other-window".into());
        let mut patch = TablePreferencesPatch {
            widths: [("old".into(), Some(300.))].into(),
            ..Default::default()
        };
        patch.merge(TablePreferencesPatch {
            reset: true,
            ..Default::default()
        });
        patch.merge(TablePreferencesPatch {
            widths: [("new".into(), Some(100.))].into(),
            ..Default::default()
        });
        patch.apply(&mut preferences);
        assert_eq!(preferences.widths, [("new".into(), 100.)].into());
        assert!(preferences.frozen_columns.is_empty());
    }
}
