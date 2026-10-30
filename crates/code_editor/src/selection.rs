//! Multi-cursor selections in byte offsets: normalizing, merging and mapping
//! through edits. One cursor is just a list with one element.

use crate::text::Transaction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    /// Where the caret is.
    pub head: usize,
    /// Visual column kept while moving up and down.
    pub goal: Option<usize>,
}

impl Selection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Selection {
            anchor,
            head,
            goal: None,
        }
    }

    pub fn cursor(at: usize) -> Self {
        Selection::new(at, at)
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The head is before the anchor.
    pub fn reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// The same range from `start` to `end`, keeping this selection's direction.
    pub fn with_range(&self, start: usize, end: usize) -> Self {
        if self.reversed() {
            Selection::new(end, start)
        } else {
            Selection::new(start, end)
        }
    }

    pub fn map(&self, tx: &Transaction) -> Self {
        if self.is_empty() {
            return Selection::cursor(tx.map(self.head, true));
        }
        // The range does not grow over text inserted at its edges.
        let (s, e) = (tx.map(self.start(), true), tx.map(self.end(), false));
        self.with_range(s, e.max(s))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selections {
    ranges: Vec<Selection>,
    primary: usize,
}

impl Default for Selections {
    fn default() -> Self {
        Selections::single(Selection::cursor(0))
    }
}

impl Selections {
    pub fn single(s: Selection) -> Self {
        Selections {
            ranges: vec![s],
            primary: 0,
        }
    }

    /// Sorted and merged; `primary` indexes `ranges` before sorting.
    pub fn new(ranges: Vec<Selection>, primary: usize) -> Self {
        assert!(!ranges.is_empty());
        let mut s = Selections {
            primary: primary.min(ranges.len() - 1),
            ranges,
        };
        s.normalize();
        s
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Selection> {
        self.ranges.iter()
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.ranges
    }

    pub fn primary(&self) -> Selection {
        self.ranges[self.primary]
    }

    pub fn primary_index(&self) -> usize {
        self.primary
    }

    /// The most recently added cursor becomes primary.
    pub fn push(&mut self, s: Selection) {
        self.ranges.push(s);
        self.primary = self.ranges.len() - 1;
        self.normalize();
    }

    /// Keeps only the primary selection.
    pub fn collapse_to_primary(&mut self) {
        let p = self.primary();
        *self = Selections::single(p);
    }

    pub fn map(&self, f: impl FnMut(&Selection) -> Selection) -> Self {
        Selections::new(self.ranges.iter().map(f).collect(), self.primary)
    }

    pub fn map_tx(&self, tx: &Transaction) -> Self {
        self.map(|s| s.map(tx))
    }

    /// Sorts by start and merges selections that overlap, or that touch when one
    /// of them is empty.
    fn normalize(&mut self) {
        let primary = self.ranges[self.primary];
        let mut idx: Vec<usize> = (0..self.ranges.len()).collect();
        idx.sort_by_key(|&i| (self.ranges[i].start(), self.ranges[i].end()));
        let mut out: Vec<Selection> = Vec::with_capacity(self.ranges.len());
        let mut new_primary = 0;
        for i in idx {
            let s = self.ranges[i];
            if let Some(last) = out.last_mut() {
                let touch = s.start() == last.end() && (s.is_empty() || last.is_empty());
                if s.start() < last.end() || touch {
                    let (a, b) = (last.start(), last.end().max(s.end()));
                    *last = if i == self.primary { s.with_range(a, b) } else { last.with_range(a, b) };
                    if i == self.primary {
                        new_primary = out.len() - 1;
                    }
                    continue;
                }
            }
            if i == self.primary {
                new_primary = out.len();
            }
            out.push(s);
        }
        // A merge keeps the primary's goal column when it was the primary.
        if out[new_primary].start() == primary.start() && out[new_primary].end() == primary.end() {
            out[new_primary].goal = primary.goal;
        }
        self.ranges = out;
        self.primary = new_primary;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Edit;

    #[test]
    fn normalize_sorts_and_merges() {
        let s = Selections::new(
            vec![
                Selection::new(10, 12),
                Selection::cursor(3),
                Selection::new(11, 15),
                Selection::cursor(3),
            ],
            2,
        );
        assert_eq!(s.as_slice(), &[Selection::cursor(3), Selection::new(10, 15)]);
        assert_eq!(s.primary_index(), 1);
    }

    #[test]
    fn adjacent_ranges_stay_apart() {
        let s = Selections::new(vec![Selection::new(0, 2), Selection::new(2, 4)], 0);
        assert_eq!(s.len(), 2);
        let s = Selections::new(vec![Selection::new(0, 2), Selection::cursor(2)], 0);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn map_through_edits() {
        let tx = Transaction::new(vec![Edit::insert(0, "ab"), Edit::delete(5, 6)]);
        let s = Selections::new(vec![Selection::cursor(0), Selection::new(4, 8)], 0);
        let m = s.map_tx(&tx);
        assert_eq!(m.as_slice(), &[Selection::cursor(2), Selection::new(6, 9)]);
    }

    #[test]
    fn reversed_direction_survives() {
        let s = Selection::new(9, 4);
        assert!(s.with_range(1, 2).reversed());
        let tx = Transaction::new(vec![Edit::insert(0, "x")]);
        assert_eq!(s.map(&tx), Selection::new(10, 5));
    }
}
