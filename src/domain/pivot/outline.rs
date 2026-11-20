//! Which rows an outline shows, independent of the engine that runs it.
//!
//! An outline of fields `k1..kn` is the union of one query per level: level
//! `L` groups by `k1..kL`, so every subtotal aggregates source rows and stays
//! correct for means, medians and distinct counts. A row at level `L` shows
//! when none of its ancestors is collapsed and it is a leaf (`L == n`), a
//! collapsed item, or an expanded item whose level shows a row of its own.

use crate::domain::documents::{Outline, OutlinePath, QueryConfig};

/// The hidden result column holding a row's level: 1 for the first field.
pub const LEVEL: &str = "__dd_level";
/// The hidden result column that is true on an expanded item's own row.
pub const OPEN: &str = "__dd_open";

/// Whether a row's item at `level` is collapsed: its key path (the row's
/// first `level` keys) is one of `paths`, or, with `negate`, is not.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSet {
    pub level: usize,
    pub paths: Vec<OutlinePath>,
    pub negate: bool,
}

/// One level's query and the rows it keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub level: usize,
    /// Each ancestor's collapsed test; any that holds hides the row.
    pub ancestors: Vec<PathSet>,
    /// The row's own item's collapsed test, below the last level.
    pub own: Option<PathSet>,
    /// Expanded items at this level keep a row of their own.
    pub headers: bool,
    /// That row shows the item's subtotals; without, its measures are blank.
    pub subtotals: bool,
    /// No row of this level shows; it is queried for its columns only, so
    /// every key keeps its type when the outline stops above the leaves.
    pub empty: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutlinePlan {
    keys: Vec<String>,
    parts: Vec<Part>,
}

impl OutlinePlan {
    /// The plan for `config`'s outline, with `keys` the result names of its
    /// shown group-by fields, or `None` when it shows the flat result.
    pub fn new(config: &QueryConfig, keys: Vec<String>) -> Option<Self> {
        let outline = config.active_outline()?;
        let fields = config.shown_group_by().len();
        debug_assert_eq!(keys.len(), fields);
        let set = |level: usize| PathSet {
            level,
            paths: outline
                .toggled
                .iter()
                .filter(|path| path.len() == level)
                .cloned()
                .collect(),
            negate: starts_collapsed(outline, level),
        };
        let reach = reach(outline, fields);
        let mut parts = (1..=reach)
            .filter(|&level| {
                level == fields
                    || outline.shows_header(level)
                    || starts_collapsed(outline, level)
                    || outline.toggled.iter().any(|path| path.len() == level)
            })
            .map(|level| Part {
                level,
                ancestors: (1..level).map(set).filter(|set| set.negate || !set.paths.is_empty()).collect(),
                own: (level < fields).then(|| set(level)),
                headers: outline.shows_header(level),
                subtotals: level == fields || outline.shows_subtotals(level),
                empty: false,
            })
            .collect::<Vec<_>>();
        if reach < fields {
            parts.push(Part {
                level: fields,
                ancestors: Vec::new(),
                own: None,
                headers: false,
                subtotals: true,
                empty: true,
            });
        }
        Some(Self { keys, parts })
    }

    /// The result names of the group-by fields, from the first.
    pub fn keys(&self) -> Vec<String> {
        self.keys.clone()
    }

    pub fn fields(&self) -> usize {
        self.keys.len()
    }

    /// The levels to query, from the first.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }
}

fn starts_collapsed(outline: &Outline, level: usize) -> bool {
    outline.level.is_some_and(|start| level >= start)
}

/// The deepest level any row can show at: below "expand to level N", only
/// the children of items expanded by hand.
fn reach(outline: &Outline, fields: usize) -> usize {
    match outline.level {
        None => fields,
        Some(start) => outline
            .toggled
            .iter()
            .filter(|path| path.len() >= start)
            .map(|path| path.len() + 1)
            .max()
            .unwrap_or(start)
            .max(start)
            .min(fields),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::documents::{Dimension, OutlineLayout};

    fn config(outline: Outline) -> QueryConfig {
        QueryConfig {
            group_by: ["a", "b", "c"].map(Dimension::from).to_vec(),
            outline: Some(outline),
            ..Default::default()
        }
    }

    fn keys() -> Vec<String> {
        ["a", "b", "c"].map(String::from).to_vec()
    }

    fn levels(plan: &OutlinePlan) -> Vec<usize> {
        plan.parts().iter().filter(|part| !part.empty).map(|part| part.level).collect()
    }

    #[test]
    fn only_levels_that_can_show_rows_are_queried() {
        // Tabular without subtotals, nothing collapsed: only the leaves.
        let bare = Outline {
            layout: OutlineLayout::Tabular,
            hidden_subtotals: vec![1, 2],
            ..Default::default()
        };
        let plan = OutlinePlan::new(&config(bare.clone()), keys()).unwrap();
        assert_eq!(levels(&plan), [3]);
        // A collapsed item needs its level, and hides its children.
        let collapsed = Outline {
            toggled: vec![vec![Some("x".into())]],
            ..bare.clone()
        };
        let plan = OutlinePlan::new(&config(collapsed), keys()).unwrap();
        assert_eq!(levels(&plan), [1, 3]);
        assert_eq!(plan.parts()[1].ancestors.len(), 1);
        assert!(!plan.parts()[1].ancestors[0].negate);
        // By default every expanded item shows a row with its subtotals.
        let plan = OutlinePlan::new(&config(Outline::default()), keys()).unwrap();
        assert_eq!(levels(&plan), [1, 2, 3]);
        assert!(plan.parts().iter().all(|part| part.subtotals));
        // Other layouts keep the rows of items whose subtotals are hidden.
        let outline = Outline {
            layout: OutlineLayout::Outline,
            hidden_subtotals: vec![1],
            ..Default::default()
        };
        let plan = OutlinePlan::new(&config(outline), keys()).unwrap();
        assert_eq!(levels(&plan), [1, 2, 3]);
        assert!(!plan.parts()[0].subtotals && plan.parts()[1].subtotals);
        // Expanding to level 1 stops there, unless an item is expanded by hand.
        let top = Outline {
            level: Some(1),
            ..bare
        };
        assert_eq!(levels(&OutlinePlan::new(&config(top.clone()), keys()).unwrap()), [1]);
        let mut opened = top;
        opened.toggled = vec![vec![Some("x".into())]];
        let plan = OutlinePlan::new(&config(opened), keys()).unwrap();
        assert_eq!(levels(&plan), [1, 2]);
        assert!(plan.parts()[1].ancestors[0].negate);
    }

    #[test]
    fn one_shown_field_has_no_outline() {
        let mut one = config(Outline::default());
        one.depth = Some(1);
        assert!(OutlinePlan::new(&one, vec!["a".into()]).is_none());
        one.depth = Some(2);
        assert!(OutlinePlan::new(&one, keys()[..2].to_vec()).is_some());
    }
}
