//! The find/replace widget: its query, matches, status text and geometry.

use super::Rect;
use super::input::Input;
use crate::search::{self, Query};

/// Height of one row of the widget.
pub const ROW_H: f32 = 33.0;
const WIDTH: f32 = 440.0;
const BUTTON: f32 = 24.0;
const TOGGLE: f32 = 22.0;
const STATUS_W: f32 = 76.0;

#[derive(Default)]
pub struct Find {
    pub open: bool,
    /// The replace row is shown.
    pub replace: bool,
    pub find: Input,
    pub with: Input,
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Every match, as byte ranges, for `computed`.
    pub matches: Vec<(usize, usize)>,
    pub error: Option<String>,
    /// The text version and query the matches are for.
    computed: Option<(u64, Query)>,
    /// Where the caret was when the search started (incremental search looks
    /// from here).
    pub origin: usize,
}

/// Parts of the widget a click can land on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    ToggleReplace,
    FindInput,
    MatchCase,
    WholeWord,
    Regex,
    Prev,
    Next,
    Close,
    ReplaceInput,
    ReplaceOne,
    ReplaceAll,
    /// Elsewhere on the widget.
    Body,
}

/// Where everything is, for an editor `width` wide.
pub struct Layout {
    pub frame: Rect,
    pub chevron: Rect,
    pub find_input: Rect,
    /// Match case, whole word, regex.
    pub toggles: [Rect; 3],
    pub status: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub close: Rect,
    pub replace_input: Rect,
    pub replace_one: Rect,
    pub replace_all: Rect,
}

impl Find {
    pub fn query(&self) -> Query {
        Query {
            text: self.find.text.clone(),
            match_case: self.match_case,
            whole_word: self.whole_word,
            regex: self.regex,
        }
    }

    /// Recomputes the matches when the text or the query changed. Returns
    /// whether they were recomputed.
    pub fn update(&mut self, version: u64, hay: &str) -> bool {
        let q = self.query();
        if self.computed.as_ref().is_some_and(|(v, cq)| *v == version && *cq == q) {
            return false;
        }
        self.matches.clear();
        self.error = None;
        if !q.text.is_empty() {
            match q.compile() {
                Ok(re) => self.matches = search::find_all(hay, &re, q.whole_word),
                Err(e) => self.error = Some(e),
            }
        }
        self.computed = Some((version, q));
        true
    }

    /// Forgets the matches so the next `update` recomputes them.
    pub fn invalidate(&mut self) {
        self.computed = None;
    }

    /// The index of the match exactly covering `range`.
    pub fn index_of(&self, range: (usize, usize)) -> Option<usize> {
        self.matches.binary_search_by_key(&range.0, |m| m.0).ok().filter(|&i| self.matches[i] == range)
    }

    /// The first match starting at or after `off`, wrapping around.
    pub fn next_from(&self, off: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let i = self.matches.partition_point(|m| m.0 < off);
        Some(if i < self.matches.len() { i } else { 0 })
    }

    /// The last match ending at or before `off`, wrapping around.
    pub fn prev_before(&self, off: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let i = self.matches.partition_point(|m| m.1 <= off);
        Some(if i > 0 { i - 1 } else { self.matches.len() - 1 })
    }

    /// "3 of 12", "? of 12", "No results", or the regex error.
    pub fn status(&self, selection: (usize, usize)) -> String {
        if self.error.is_some() {
            return "Invalid regex".into();
        }
        if self.find.text.is_empty() {
            return "No results".into();
        }
        let n = self.matches.len();
        let more = if n >= search::MAX_MATCHES { "+" } else { "" };
        match (n, self.index_of(selection)) {
            (0, _) => "No results".into(),
            (_, Some(i)) => format!("{} of {n}{more}", i + 1),
            _ => format!("? of {n}{more}"),
        }
    }

    pub fn layout(&self, width: f32) -> Layout {
        let w = WIDTH.min(width - 20.0).max(240.0);
        let x = width - w - crate::view::SCROLLBAR - 4.0;
        let h = if self.replace { ROW_H * 2.0 } else { ROW_H };
        let frame = Rect::new(x, 0.0, x + w, h + 4.0);
        let row = |i: f32| (4.0 + ROW_H * i, ROW_H * (i + 1.0) - 3.0);
        let (t, b) = row(0.0);
        let chevron = Rect::new(x + 2.0, 4.0, x + 18.0, frame.b - 4.0);
        let close = Rect::new(x + w - 4.0 - BUTTON, t + 1.0, x + w - 4.0, b - 1.0);
        let next = Rect::new(close.l - BUTTON, close.t, close.l, close.b);
        let prev = Rect::new(next.l - BUTTON, close.t, next.l, close.b);
        let status = Rect::new(prev.l - STATUS_W, t, prev.l - 2.0, b);
        let find_input = Rect::new(x + 22.0, t, status.l - 4.0, b);
        let tog = |i: f32| {
            let r = find_input.r - 2.0 - TOGGLE * (3.0 - i);
            Rect::new(r, t + 3.0, r + TOGGLE - 2.0, b - 3.0)
        };
        let (t2, b2) = row(1.0);
        let replace_input = Rect::new(find_input.l, t2, find_input.r, b2);
        let replace_one = Rect::new(replace_input.r + 4.0, t2 + 1.0, replace_input.r + 4.0 + 64.0, b2 - 1.0);
        let replace_all = Rect::new(replace_one.r + 2.0, t2 + 1.0, replace_one.r + 2.0 + 36.0, b2 - 1.0);
        Layout {
            frame,
            chevron,
            find_input,
            toggles: [tog(0.0), tog(1.0), tog(2.0)],
            status,
            prev,
            next,
            close,
            replace_input,
            replace_one,
            replace_all,
        }
    }

    pub fn hit(&self, width: f32, x: f32, y: f32) -> Option<Hit> {
        if !self.open {
            return None;
        }
        let l = self.layout(width);
        if !l.frame.contains(x, y) {
            return None;
        }
        let parts = [
            (l.toggles[0], Hit::MatchCase),
            (l.toggles[1], Hit::WholeWord),
            (l.toggles[2], Hit::Regex),
            (l.chevron, Hit::ToggleReplace),
            (l.find_input, Hit::FindInput),
            (l.prev, Hit::Prev),
            (l.next, Hit::Next),
            (l.close, Hit::Close),
        ];
        let rows = [
            (l.replace_input, Hit::ReplaceInput),
            (l.replace_one, Hit::ReplaceOne),
            (l.replace_all, Hit::ReplaceAll),
        ];
        let replace_parts = if self.replace { &rows[..] } else { &[] };
        Some(
            parts
                .iter()
                .chain(replace_parts)
                .find(|(r, _)| r.contains(x, y))
                .map_or(Hit::Body, |p| p.1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_navigation_and_status() {
        let mut f = Find::default();
        f.find.set("ab");
        assert!(f.update(1, "ab xab ab"));
        assert!(!f.update(1, "ab xab ab"));
        assert_eq!(f.matches, vec![(0, 2), (4, 6), (7, 9)]);
        assert_eq!(f.status((4, 6)), "2 of 3");
        assert_eq!(f.status((5, 5)), "? of 3");
        assert_eq!(f.next_from(3), Some(1));
        assert_eq!(f.next_from(8), Some(0));
        assert_eq!(f.prev_before(4), Some(0));
        assert_eq!(f.prev_before(0), Some(2));
        f.whole_word = true;
        assert!(f.update(1, "ab xab ab"));
        assert_eq!(f.matches.len(), 2);
        f.regex = true;
        f.find.set("(");
        f.update(1, "x");
        assert_eq!(f.status((0, 0)), "Invalid regex");
    }

    #[test]
    fn hit_testing() {
        let mut f = Find {
            open: true,
            ..Default::default()
        };
        let l = f.layout(800.0);
        let c = |r: Rect| ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        let (x, y) = c(l.close);
        assert_eq!(f.hit(800.0, x, y), Some(Hit::Close));
        let (x, y) = c(l.toggles[2]);
        assert_eq!(f.hit(800.0, x, y), Some(Hit::Regex));
        let (x, y) = c(l.replace_one);
        assert_eq!(f.hit(800.0, x, y), None);
        f.replace = true;
        assert_eq!(f.hit(800.0, x, y), Some(Hit::ReplaceOne));
        assert_eq!(f.hit(800.0, 5.0, 5.0), None);
    }
}
