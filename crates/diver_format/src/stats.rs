//! Whole-column numbers: what `avg(c)`, `duplicate(c)`, `rank(c)` and the
//! color scales need. Hosts answer them from wherever the column lives;
//! [`TableStats`] answers them from columns in memory.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aggregate {
    Min,
    Max,
    Mean,
    Median,
    Sum,
    /// Values that aren't blank.
    Count,
    Distinct,
    /// Sample standard deviation.
    Stdev,
    /// From 0 to 1, interpolating between values.
    Percentile(f64),
}

/// Where a number stands among a column's numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub above: usize,
    pub below: usize,
    pub count: usize,
}

/// Column statistics. Columns are indexes into the column list the rules
/// were compiled with.
pub trait Stats {
    fn aggregate(&self, column: usize, aggregate: Aggregate) -> Option<f64>;
    /// How many of the column's cells hold `value`; `None` when it isn't
    /// known (yet), which leaves tests on it blank.
    fn frequency(&self, column: usize, value: &Value) -> Option<usize>;
    /// Where `value` stands among the column's numbers.
    fn position(&self, column: usize, value: f64) -> Option<Position>;
}

/// No statistics: every whole-column test is blank.
pub struct NoStats;

impl Stats for NoStats {
    fn aggregate(&self, _: usize, _: Aggregate) -> Option<f64> {
        None
    }

    fn frequency(&self, _: usize, _: &Value) -> Option<usize> {
        None
    }

    fn position(&self, _: usize, _: f64) -> Option<Position> {
        None
    }
}

/// Statistics of columns held in memory, worked out per column on first use.
pub struct TableStats<'a> {
    columns: &'a [Vec<Value>],
    cache: RefCell<HashMap<usize, std::rc::Rc<Summary>>>,
}

struct Summary {
    /// The column's numbers, ascending.
    sorted: Vec<f64>,
    counts: HashMap<String, usize>,
    non_null: usize,
}

impl<'a> TableStats<'a> {
    pub fn new(columns: &'a [Vec<Value>]) -> Self {
        TableStats { columns, cache: RefCell::default() }
    }

    fn summary(&self, column: usize) -> Option<std::rc::Rc<Summary>> {
        if let Some(summary) = self.cache.borrow().get(&column) {
            return Some(summary.clone());
        }
        let values = self.columns.get(column)?;
        let mut sorted: Vec<f64> = values.iter().filter_map(Value::number).filter(|n| n.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        let mut counts = HashMap::new();
        for key in values.iter().filter_map(Value::key) {
            *counts.entry(key).or_insert(0) += 1;
        }
        let non_null = values.iter().filter(|v| !v.is_null()).count();
        let summary = std::rc::Rc::new(Summary { sorted, counts, non_null });
        self.cache.borrow_mut().insert(column, summary.clone());
        Some(summary)
    }
}

impl Stats for TableStats<'_> {
    fn aggregate(&self, column: usize, aggregate: Aggregate) -> Option<f64> {
        let s = self.summary(column)?;
        let n = s.sorted.len();
        let sum = || s.sorted.iter().sum::<f64>();
        match aggregate {
            Aggregate::Count => Some(s.non_null as f64),
            Aggregate::Distinct => Some(s.counts.len() as f64),
            _ if n == 0 => None,
            Aggregate::Min => s.sorted.first().copied(),
            Aggregate::Max => s.sorted.last().copied(),
            Aggregate::Sum => Some(sum()),
            Aggregate::Mean => Some(sum() / n as f64),
            Aggregate::Median => Some(percentile(&s.sorted, 0.5)),
            Aggregate::Percentile(p) => Some(percentile(&s.sorted, p)),
            Aggregate::Stdev => {
                if n < 2 {
                    return None;
                }
                let mean = sum() / n as f64;
                let squares: f64 = s.sorted.iter().map(|x| (x - mean).powi(2)).sum();
                Some((squares / (n - 1) as f64).sqrt())
            }
        }
    }

    fn frequency(&self, column: usize, value: &Value) -> Option<usize> {
        let s = self.summary(column)?;
        Some(value.key().and_then(|key| s.counts.get(&key).copied()).unwrap_or(0))
    }

    fn position(&self, column: usize, value: f64) -> Option<Position> {
        let s = self.summary(column)?;
        let below = s.sorted.partition_point(|x| *x < value);
        let not_above = s.sorted.partition_point(|x| *x <= value);
        Some(Position { above: s.sorted.len() - not_above, below, count: s.sorted.len() })
    }
}

/// Linear interpolation between the closest ranks, like `quantile_cont`.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    let rank = p * (sorted.len() - 1) as f64;
    let low = rank.floor() as usize;
    let high = rank.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_summarize() {
        let columns = vec![vec![
            Value::Number(3.0),
            Value::Number(1.0),
            Value::Null,
            Value::Number(3.0),
            Value::Number(5.0),
        ]];
        let stats = TableStats::new(&columns);
        assert_eq!(stats.aggregate(0, Aggregate::Min), Some(1.0));
        assert_eq!(stats.aggregate(0, Aggregate::Mean), Some(3.0));
        assert_eq!(stats.aggregate(0, Aggregate::Median), Some(3.0));
        assert_eq!(stats.aggregate(0, Aggregate::Percentile(0.25)), Some(2.5));
        assert_eq!(stats.aggregate(0, Aggregate::Count), Some(4.0));
        assert_eq!(stats.aggregate(0, Aggregate::Distinct), Some(3.0));
        assert_eq!(stats.frequency(0, &Value::Number(3.0)), Some(2));
        assert_eq!(stats.frequency(0, &Value::Null), Some(0));
        assert_eq!(stats.position(0, 3.0), Some(Position { above: 1, below: 1, count: 4 }));
    }
}
