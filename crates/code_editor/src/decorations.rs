//! Decorations: ranges the host attaches styles to (line backgrounds, inline
//! colors, squiggles, gutter markers, hover messages). They are tracked ranges
//! that move with edits, like Monaco's `deltaDecorations`.

use crate::text::Change;

/// What happens when text is typed exactly at a decoration's edge (Monaco's
/// `TrackedRangeStickiness`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stickiness {
    /// Text typed at either edge joins the range.
    #[default]
    AlwaysGrows,
    /// Text typed at either edge stays outside.
    NeverGrows,
    /// Only text typed at the start joins.
    GrowsBefore,
    /// Only text typed at the end joins.
    GrowsAfter,
}

/// The squiggle under a range, colored by severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// How a decoration looks. Colors are ARGB (`0xAARRGGBB`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecorationOptions {
    /// Background of the whole line(s), across the editor's width.
    pub line_background: Option<u32>,
    /// Background behind the range's text.
    pub background: Option<u32>,
    /// Color of the range's text (over syntax colors).
    pub foreground: Option<u32>,
    /// A wavy underline (for errors reported by the host).
    pub squiggle: Option<Severity>,
    /// A marker in the gutter on the range's first line.
    pub gutter_marker: Option<u32>,
    /// Shown when the mouse rests on the range.
    pub hover: Option<String>,
    pub stickiness: Stickiness,
}

/// Identifies a decoration for later removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DecorationId(pub u64);

#[derive(Clone, Debug)]
pub struct Tracked {
    pub id: DecorationId,
    /// Byte offsets.
    pub start: usize,
    pub end: usize,
    pub options: DecorationOptions,
}

#[derive(Default)]
pub struct Decorations {
    items: Vec<Tracked>,
    next: u64,
}

/// Maps a decoration edge through one change. `grows` says whether text
/// inserted exactly at the edge joins the range.
fn map_edge(off: usize, c: &Change, is_start: bool, grows: bool) -> usize {
    let insert = c.start == c.old_end;
    if off < c.start || (off == c.start && !insert) {
        return off;
    }
    if insert && off == c.start {
        // Joining at the start keeps the start before the text; joining at the
        // end moves the end after it.
        return if is_start == grows { off } else { c.new_end };
    }
    if off >= c.old_end {
        return off - c.old_end + c.new_end;
    }
    // Inside replaced text: to the start of the replacement.
    c.start
}

impl Decorations {
    /// Removes `old` and adds `new` (byte ranges), returning the new ids.
    pub fn delta(&mut self, old: &[DecorationId], new: Vec<(usize, usize, DecorationOptions)>) -> Vec<DecorationId> {
        self.items.retain(|t| !old.contains(&t.id));
        new.into_iter()
            .map(|(start, end, options)| {
                self.next += 1;
                let id = DecorationId(self.next);
                self.items.push(Tracked {
                    id,
                    start: start.min(end),
                    end: end.max(start),
                    options,
                });
                id
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, id: DecorationId) -> Option<&Tracked> {
        self.items.iter().find(|t| t.id == id)
    }

    /// Moves every decoration through applied changes.
    pub fn apply(&mut self, changes: &[Change]) {
        for t in &mut self.items {
            let (gs, ge) = match t.options.stickiness {
                Stickiness::AlwaysGrows => (true, true),
                Stickiness::NeverGrows => (false, false),
                Stickiness::GrowsBefore => (true, false),
                Stickiness::GrowsAfter => (false, true),
            };
            for c in changes {
                t.start = map_edge(t.start, c, true, gs);
                t.end = map_edge(t.end, c, false, ge).max(t.start);
            }
        }
    }

    /// Decorations touching `from..to`.
    pub fn in_range(&self, from: usize, to: usize) -> impl Iterator<Item = &Tracked> {
        self.items.iter().filter(move |t| t.start <= to && t.end >= from)
    }

    /// The hover message at `off`, if a decoration with one covers it.
    pub fn hover_at(&self, off: usize) -> Option<&str> {
        self.items
            .iter()
            .filter(|t| t.start <= off && off < t.end.max(t.start + 1))
            .find_map(|t| t.options.hover.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Buffer, Edit, Transaction};

    fn tracked(text: &str, range: (usize, usize), stick: Stickiness, edits: Vec<Edit>) -> (usize, usize) {
        let mut b = Buffer::new(text);
        let mut d = Decorations::default();
        let opts = DecorationOptions {
            stickiness: stick,
            ..Default::default()
        };
        let id = d.delta(&[], vec![(range.0, range.1, opts)])[0];
        let (_, changes) = b.apply(&Transaction::new(edits));
        d.apply(&changes);
        let t = d.get(id).unwrap();
        (t.start, t.end)
    }

    #[test]
    fn edges_follow_stickiness() {
        let at_start = || vec![Edit::insert(2, "xx")];
        let at_end = || vec![Edit::insert(5, "xx")];
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::AlwaysGrows, at_start()), (2, 7));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::AlwaysGrows, at_end()), (2, 7));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::NeverGrows, at_start()), (4, 7));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::NeverGrows, at_end()), (2, 5));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::GrowsBefore, at_end()), (2, 5));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::GrowsAfter, at_start()), (4, 7));
    }

    #[test]
    fn ranges_move_and_shrink() {
        // Edits before shift the range; deleting through it collapses it.
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::NeverGrows, vec![Edit::delete(0, 1)]), (1, 4));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::NeverGrows, vec![Edit::delete(1, 6)]), (1, 1));
        assert_eq!(tracked("abcdefg", (2, 5), Stickiness::NeverGrows, vec![Edit::new(3, 4, "XYZ")]), (2, 7));
        // Multi-edit transactions apply in sequence.
        assert_eq!(
            tracked("abcdefg", (2, 5), Stickiness::NeverGrows, vec![Edit::insert(0, "1"), Edit::insert(6, "2")]),
            (3, 6)
        );
    }

    #[test]
    fn delta_and_hover() {
        let mut d = Decorations::default();
        let hover = DecorationOptions {
            hover: Some("bad".into()),
            ..Default::default()
        };
        let ids = d.delta(&[], vec![(1, 3, hover.clone()), (5, 6, Default::default())]);
        assert_eq!(d.hover_at(2), Some("bad"));
        assert_eq!(d.hover_at(3), None);
        assert_eq!(d.in_range(4, 10).count(), 1);
        let ids2 = d.delta(&ids[..1], vec![]);
        assert!(ids2.is_empty());
        assert_eq!(d.hover_at(2), None);
    }
}
