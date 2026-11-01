//! Conditional formatting of a grid: a view's format sets compiled against
//! its columns, and the column statistics their rules read.
//!
//! Statistics cover the rows the view's slicers and filters leave, not a
//! search, so formatting stays put while searching. Numbers for a whole
//! column (ranges, averages) are read in one query; counts and ranks of a
//! value are looked up for the values on screen, over the same rows.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use diver_format::program::Need;
use diver_format::stats::Position;
use diver_format::{Aggregate, CellStyle, Column, Program, Row, Stats, Value, ValueKind};

use crate::domain::documents::ConditionalFormat;
use crate::domain::formatting::{numeric_type_name, string_type_name, temporal_type_name};
use crate::infrastructure::store::{CachedRow, ColumnSchema, Store, ViewQuery};

/// The rows statistics are taken over, and the columns of the store holding them.
#[derive(Clone)]
pub struct FormatScope {
    pub store: Arc<Mutex<Store>>,
    /// The view's filters, without its search or sort.
    pub view: ViewQuery,
    pub schema: Arc<[ColumnSchema]>,
}

impl FormatScope {
    /// Whether statistics taken over `self` hold for `other` too.
    pub fn same_rows(&self, other: &FormatScope) -> bool {
        Arc::ptr_eq(&self.store, &other.store) && self.view == other.view
    }
}

/// What kind of values a column of `sql_type` holds, for the rules.
pub fn value_kind(sql_type: &str) -> ValueKind {
    let upper = sql_type.to_ascii_uppercase();
    if upper.ends_with(']') || upper.starts_with("LIST") {
        ValueKind::List
    } else if numeric_type_name(&upper) {
        ValueKind::Number
    } else if temporal_type_name(&upper) {
        ValueKind::Date
    } else if upper.starts_with("BOOL") {
        ValueKind::Bool
    } else if string_type_name(&upper) {
        ValueKind::Text
    } else {
        ValueKind::Unknown
    }
}

/// The columns rules can name, from a store's schema.
pub fn rule_columns(schema: &[ColumnSchema]) -> Vec<Column> {
    schema.iter().map(|column| Column::new(column.name.clone(), value_kind(&column.sql_type))).collect()
}

/// Statistics known so far. Columns are indices into the store's schema.
#[derive(Default)]
struct KnownStats {
    aggregates: HashMap<(usize, String), Option<f64>>,
    /// By the value's key (`Value::key`).
    counts: HashMap<(usize, String), usize>,
    /// By the number's bits.
    positions: HashMap<(usize, u64), Position>,
}

fn aggregate_key(aggregate: Aggregate) -> String {
    format!("{aggregate:?}")
}

impl Stats for KnownStats {
    fn aggregate(&self, column: usize, aggregate: Aggregate) -> Option<f64> {
        self.aggregates.get(&(column, aggregate_key(aggregate))).copied().flatten()
    }

    fn frequency(&self, column: usize, value: &Value) -> Option<usize> {
        self.counts.get(&(column, value.key()?)).copied()
    }

    fn position(&self, column: usize, value: f64) -> Option<Position> {
        self.positions.get(&(column, value.to_bits())).copied()
    }
}

/// A grid row as the rules read it.
struct CachedValues<'a> {
    row: &'a CachedRow,
    kinds: &'a [ValueKind],
    index: usize,
}

impl Row for CachedValues<'_> {
    fn value(&self, column: usize) -> Value {
        let text = self.row.values.get(column).and_then(Option::as_deref);
        Value::from_text(text, self.kinds.get(column).copied().unwrap_or_default())
    }

    fn index(&self) -> usize {
        self.index
    }
}

/// Values to look up, by column (an index into the store's schema).
#[derive(Debug, Default, PartialEq)]
pub struct Lookups {
    /// Values as the grid shows them, to count.
    pub counts: Vec<(usize, Vec<String>)>,
    /// Numbers to rank.
    pub positions: Vec<(usize, Vec<f64>)>,
}

impl Lookups {
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty() && self.positions.is_empty()
    }

    /// Runs the lookups on a worker.
    pub fn run(self, scope: &FormatScope) -> Result<LookedUp, String> {
        let store = scope.store.lock().map_err(|_| "store lock poisoned".to_owned())?;
        let name = |column: usize| {
            scope.schema.get(column).map(|c| c.name.clone()).ok_or_else(|| "formatting column is gone".to_owned())
        };
        let mut found = LookedUp::default();
        for (column, values) in self.counts {
            let counted = store.value_counts(&scope.view, &name(column)?, &values)?;
            found.counts.push(Counted { column, asked: values, found: counted });
        }
        for (column, values) in self.positions {
            let positions = store.value_positions(&scope.view, &name(column)?, &values)?;
            found.positions.push((column, values.into_iter().zip(positions).collect()));
        }
        Ok(found)
    }
}

/// Lookup results: each column's values asked about, and what was found.
#[derive(Debug, Default)]
pub struct LookedUp {
    counts: Vec<Counted>,
    positions: Vec<(usize, Vec<(f64, Position)>)>,
}

/// One column's counts: the values asked about and those found, with their counts.
#[derive(Debug)]
struct Counted {
    column: usize,
    asked: Vec<String>,
    found: Vec<(String, usize)>,
}

/// A view's formatting, ready to draw.
pub struct GridFormatting {
    pub scope: FormatScope,
    kinds: Vec<ValueKind>,
    /// Each format's program and the columns it formats.
    programs: Vec<(Program, Vec<usize>)>,
    /// The statistics each formatted column's rules read.
    needs: HashMap<usize, Vec<(usize, Need)>>,
    stats: KnownStats,
    /// Lookups asked for and not back yet.
    pending: HashSet<(usize, String)>,
}

impl GridFormatting {
    /// `None` when no format applies to a column of `scope`.
    pub fn new(formats: &[ConditionalFormat], scope: FormatScope) -> Option<Self> {
        let columns = rule_columns(&scope.schema);
        let programs: Vec<(Program, Vec<usize>)> = formats
            .iter()
            .filter_map(|format| {
                let targets: Vec<usize> = format
                    .columns
                    .iter()
                    .filter_map(|name| columns.iter().position(|column| column.name == *name))
                    .collect();
                let program = diver_format::check(&format.rules, &columns).program;
                (!targets.is_empty() && !program.is_empty()).then_some((program, targets))
            })
            .collect();
        if programs.is_empty() {
            return None;
        }
        let mut needs: HashMap<usize, Vec<(usize, Need)>> = HashMap::new();
        for (program, targets) in &programs {
            for target in targets {
                let list = needs.entry(*target).or_default();
                for need in program.needs(*target) {
                    if !list.contains(&need) {
                        list.push(need);
                    }
                }
            }
        }
        Some(GridFormatting {
            kinds: columns.iter().map(|column| column.kind).collect(),
            scope,
            programs,
            needs,
            stats: KnownStats::default(),
            pending: HashSet::new(),
        })
    }

    /// Keeps the statistics already known, when they hold for `self`.
    pub fn keep_known(&mut self, previous: GridFormatting) {
        if self.scope.same_rows(&previous.scope) && self.kinds == previous.kinds {
            self.stats = previous.stats;
        }
    }

    /// Whether column `column` (of the store's schema) has formatting.
    pub fn formats(&self, column: usize) -> bool {
        self.needs.contains_key(&column)
    }

    /// The formatting of column `column` in `row`, the `index`th row shown.
    pub fn style(&self, column: usize, row: &CachedRow, index: usize) -> CellStyle {
        let mut style = CellStyle::default();
        let values = CachedValues { row, kinds: &self.kinds, index };
        for (program, targets) in &self.programs {
            if targets.contains(&column) {
                program.apply(column, &values, &self.stats, &mut style);
            }
        }
        style
    }

    /// The whole-column numbers the rules read and that aren't known yet.
    pub fn missing_aggregates(&self) -> Vec<(String, Aggregate)> {
        let mut out: Vec<(String, Aggregate)> = Vec::new();
        for (column, need) in self.needs.values().flatten() {
            let Need::Aggregate(aggregate) = need else { continue };
            if self.stats.aggregates.contains_key(&(*column, aggregate_key(*aggregate))) {
                continue;
            }
            let Some(schema) = self.scope.schema.get(*column) else { continue };
            let request = (schema.name.clone(), *aggregate);
            if !out.contains(&request) {
                out.push(request);
            }
        }
        out
    }

    /// Records whole-column numbers read for `requests`.
    pub fn set_aggregates(&mut self, requests: &[(String, Aggregate)], values: Vec<Option<f64>>) {
        for ((name, aggregate), value) in requests.iter().zip(values) {
            if let Some(column) = self.scope.schema.iter().position(|c| c.name == *name) {
                self.stats.aggregates.insert((column, aggregate_key(*aggregate)), value);
            }
        }
    }

    /// The counts and ranks `rows` need for `columns` (of the store's
    /// schema) that aren't known or asked for yet, marked as asked for.
    pub fn missing_lookups(&mut self, rows: &[CachedRow], columns: &[usize]) -> Lookups {
        let mut counts: Vec<(usize, Vec<String>)> = Vec::new();
        let mut positions: Vec<(usize, Vec<f64>)> = Vec::new();
        for target in columns {
            let Some(needs) = self.needs.get(target) else { continue };
            for (column, need) in needs {
                let kind = self.kinds.get(*column).copied().unwrap_or_default();
                for row in rows {
                    let Some(text) = row.values.get(*column).and_then(Option::as_deref) else { continue };
                    let value = Value::from_text(Some(text), kind);
                    match need {
                        Need::Frequency => {
                            let Some(key) = value.key() else { continue };
                            if self.stats.counts.contains_key(&(*column, key.clone()))
                                || !self.pending.insert((*column, format!("c{key}")))
                            {
                                continue;
                            }
                            push(&mut counts, *column, text.to_owned());
                        }
                        Need::Position => {
                            let Some(number) = value.number().filter(|n| n.is_finite()) else { continue };
                            if self.stats.positions.contains_key(&(*column, number.to_bits()))
                                || !self.pending.insert((*column, format!("p{}", number.to_bits())))
                            {
                                continue;
                            }
                            push(&mut positions, *column, number);
                        }
                        Need::Aggregate(_) => {}
                    }
                }
            }
        }
        Lookups { counts, positions }
    }

    /// Records lookups that came back.
    pub fn absorb(&mut self, found: LookedUp) {
        for Counted { column, asked, found: counted } in found.counts {
            let kind = self.kinds.get(column).copied().unwrap_or_default();
            let key = |text: &str| Value::from_text(Some(text), kind).key();
            for text in &asked {
                if let Some(key) = key(text) {
                    self.pending.remove(&(column, format!("c{key}")));
                    // A value no row in scope holds counts as none.
                    self.stats.counts.entry((column, key)).or_insert(0);
                }
            }
            for (text, count) in counted {
                if let Some(key) = key(&text) {
                    self.stats.counts.insert((column, key), count);
                }
            }
        }
        for (column, positions) in found.positions {
            for (number, position) in positions {
                self.pending.remove(&(column, format!("p{}", number.to_bits())));
                self.stats.positions.insert((column, number.to_bits()), position);
            }
        }
    }

    /// Forgets lookups asked for, after they failed, so they're asked again.
    pub fn forget_pending(&mut self) {
        self.pending.clear();
    }
}

fn push<T: PartialEq>(groups: &mut Vec<(usize, Vec<T>)>, column: usize, value: T) {
    match groups.iter_mut().find(|(c, _)| *c == column) {
        Some((_, values)) => {
            if !values.contains(&value) {
                values.push(value);
            }
        }
        None => groups.push((column, vec![value])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::duckdb_store::DuckDbStore;

    fn scope() -> FormatScope {
        let connection = duckdb::Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE qp_data AS SELECT * FROM (VALUES \
                 ('a', 10), ('b', 20), ('a', 30), ('c', NULL)) t(name, n)",
            )
            .unwrap();
        let store = Store::from(DuckDbStore::from_connection(connection).unwrap());
        let schema: Arc<[ColumnSchema]> = store.schema().to_vec().into();
        FormatScope { store: Arc::new(Mutex::new(store)), view: ViewQuery::default(), schema }
    }

    fn row(name: &str, n: Option<&str>) -> CachedRow {
        CachedRow { rowid: 0, values: vec![Some(name.into()), n.map(Into::into)] }
    }

    fn format(rules: &str, columns: &[&str]) -> ConditionalFormat {
        ConditionalFormat { rules: rules.into(), columns: columns.iter().map(|c| c.to_string()).collect() }
    }

    #[test]
    fn duplicates_and_scales_fill_in_as_statistics_arrive() {
        let formats = [
            format("if duplicate(c) then bold", &["name"]),
            format("scale(black, white)", &["n"]),
            format("if c > \"nope\" then italic", &["n"]),
        ];
        let mut formatting = GridFormatting::new(&formats, scope()).unwrap();
        assert!(formatting.formats(0) && formatting.formats(1));
        let rows = [row("a", Some("10")), row("b", Some("20"))];
        // Nothing is known yet, so nothing is drawn.
        assert!(formatting.style(0, &rows[0], 0).is_plain());
        assert!(formatting.style(1, &rows[0], 0).background.is_none());

        let requests = formatting.missing_aggregates();
        let values = formatting.scope.store.lock().unwrap().column_aggregates(&formatting.scope.view, &requests).unwrap();
        formatting.set_aggregates(&requests, values);
        assert!(formatting.missing_aggregates().is_empty());
        assert_eq!(formatting.style(1, &rows[0], 0).background, Some(diver_format::Color::rgb(0, 0, 0)));

        let lookups = formatting.missing_lookups(&rows, &[0, 1]);
        assert_eq!(lookups.counts, [(0, vec!["a".to_string(), "b".to_string()])]);
        // Asked for once, not again while pending.
        assert!(formatting.missing_lookups(&rows, &[0]).is_empty());
        let scope = formatting.scope.clone();
        formatting.absorb(lookups.run(&scope).unwrap());
        assert!(formatting.style(0, &rows[0], 0).bold);
        assert!(!formatting.style(0, &rows[1], 1).bold);
        assert!(formatting.missing_lookups(&rows, &[0]).is_empty());
    }

    #[test]
    fn formats_without_columns_or_rules_draw_nothing() {
        assert!(GridFormatting::new(&[format("bold", &[])], scope()).is_none());
        assert!(GridFormatting::new(&[format("bolf", &["n"])], scope()).is_none());
        assert!(GridFormatting::new(&[], scope()).is_none());
    }

    #[test]
    fn kinds_follow_sql_types() {
        assert_eq!(value_kind("DECIMAL(10,2)"), ValueKind::Number);
        assert_eq!(value_kind("VARCHAR"), ValueKind::Text);
        assert_eq!(value_kind("TIMESTAMP"), ValueKind::Date);
        assert_eq!(value_kind("INTEGER[]"), ValueKind::List);
        assert_eq!(value_kind("BOOLEAN"), ValueKind::Bool);
    }
}
