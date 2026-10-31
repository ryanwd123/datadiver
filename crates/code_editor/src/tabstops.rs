//! Linked tab stops: groups of ranges selected together, so typing in one
//! edits them all. Snippets, renames and host rewrites start them. Tab and
//! Shift+Tab move between groups; Enter, Escape or moving out of the group
//! ends them.

use std::ops::Range;

use crate::decorations::{DecorationId, DecorationOptions};
use crate::editor::State;
use crate::selection::{Selection, Selections};

pub struct TabStops {
    /// The tracked ranges of each group, in tab order.
    pub groups: Vec<Vec<DecorationId>>,
    pub current: usize,
}

/// Parses a snippet: `$1`, `${1}` and `${1:default}` are tab stops (the
/// same number twice is one linked group), `$0` is where the caret ends,
/// `\$` is a dollar sign. Returns the text and the groups in tab order.
pub fn parse_snippet(snippet: &str) -> (String, Vec<Vec<Range<usize>>>) {
    // A stop without its own text takes the text its group has elsewhere.
    let mut defaults: Vec<(u32, String)> = Vec::new();
    for_each_part(snippet, |part| {
        if let Part::Stop(n, Some(text)) = part
            && !text.is_empty()
            && !defaults.iter().any(|(m, _)| *m == n)
        {
            defaults.push((n, text.to_string()));
        }
    });
    let mut text = String::new();
    let mut stops: Vec<(u32, Range<usize>)> = Vec::new();
    for_each_part(snippet, |part| match part {
        Part::Text(t) => text.push_str(t),
        Part::Stop(n, own) => {
            let start = text.len();
            let fill = own
                .filter(|t| !t.is_empty())
                .or_else(|| defaults.iter().find(|(m, _)| *m == n).map(|(_, t)| t.as_str()))
                .unwrap_or("");
            text.push_str(fill);
            stops.push((n, start..text.len()));
        }
    });
    let mut numbers: Vec<u32> = stops.iter().map(|(n, _)| *n).collect();
    numbers.sort_by_key(|n| (*n == 0, *n));
    numbers.dedup();
    let groups = numbers
        .iter()
        .map(|n| stops.iter().filter(|(m, _)| m == n).map(|(_, r)| r.clone()).collect())
        .collect();
    (text, groups)
}

enum Part<'a> {
    Text(&'a str),
    Stop(u32, Option<&'a str>),
}

fn for_each_part<'a>(snippet: &'a str, mut f: impl FnMut(Part<'a>)) {
    let mut at = 0;
    let mut text_start = 0;
    let bytes = snippet.as_bytes();
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if matches!(bytes.get(at + 1), Some(b'$' | b'}' | b'\\')) => {
                f(Part::Text(&snippet[text_start..at]));
                text_start = at + 1;
                at += 2;
            }
            b'$' => {
                let rest = &snippet[at + 1..];
                let stop = if let Some(body) = rest.strip_prefix('{') {
                    body.find('}').and_then(|close| {
                        let inner = &body[..close];
                        let (n, default) = match inner.split_once(':') {
                            Some((n, d)) => (n, Some(d)),
                            None => (inner, None),
                        };
                        n.parse::<u32>().ok().map(|n| (n, default, close + 3))
                    })
                } else {
                    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                    rest[..digits].parse::<u32>().ok().map(|n| (n, None, digits + 1))
                };
                match stop {
                    Some((n, default, used)) => {
                        f(Part::Text(&snippet[text_start..at]));
                        f(Part::Stop(n, default));
                        at += used;
                        text_start = at;
                    }
                    None => at += 1,
                }
            }
            _ => at += 1,
        }
    }
    f(Part::Text(&snippet[text_start..]));
}

impl State {
    /// Starts linked tab stops over byte ranges of the text, selecting the
    /// first group.
    pub fn start_tab_stops(&mut self, groups: Vec<Vec<Range<usize>>>) {
        self.end_tab_stops();
        let groups: Vec<Vec<Range<usize>>> = groups.into_iter().filter(|g| !g.is_empty()).collect();
        if groups.is_empty() {
            return;
        }
        let len = self.doc.buffer.len();
        let ids = groups
            .iter()
            .map(|group| {
                let ranges = group
                    .iter()
                    .map(|r| (r.start.min(len), r.end.min(len), DecorationOptions::default()))
                    .collect();
                self.decorations.delta(&[], ranges)
            })
            .collect();
        self.tab_stops = Some(TabStops {
            groups: ids,
            current: 0,
        });
        self.select_stop(0);
    }

    fn stop_ranges(&self, group: usize) -> Vec<(usize, usize)> {
        let Some(stops) = &self.tab_stops else {
            return Vec::new();
        };
        stops.groups.get(group).map_or_else(Vec::new, |ids| {
            ids.iter()
                .filter_map(|id| self.decorations.get(*id))
                .map(|t| (t.start, t.end))
                .collect()
        })
    }

    fn select_stop(&mut self, group: usize) {
        let ranges = self.stop_ranges(group);
        if ranges.is_empty() {
            return self.end_tab_stops();
        }
        if let Some(stops) = &mut self.tab_stops {
            stops.current = group;
        }
        let sels = ranges.iter().map(|&(a, e)| Selection::new(a, e)).collect();
        self.set_sels(Selections::new(sels, 0));
        // The last group, when it is a caret, ends the stops there.
        let last = self.tab_stops.as_ref().is_some_and(|s| group + 1 == s.groups.len());
        if last && ranges.iter().all(|r| r.0 == r.1) {
            self.end_tab_stops();
        }
    }

    pub fn end_tab_stops(&mut self) {
        if let Some(stops) = self.tab_stops.take() {
            let ids: Vec<DecorationId> = stops.groups.into_iter().flatten().collect();
            self.decorations.delta(&ids, Vec::new());
            self.invalidate();
        }
    }

    /// Tab (`forward`) or Shift+Tab: the next or previous group. Past the
    /// last one the stops end with the caret where it is.
    pub fn next_stop(&mut self, forward: bool) {
        let Some(stops) = &self.tab_stops else { return };
        let (current, count) = (stops.current, stops.groups.len());
        if forward && current + 1 < count {
            self.select_stop(current + 1);
        } else if !forward && current > 0 {
            self.select_stop(current - 1);
        } else if forward {
            self.finish_stops();
        }
    }

    /// Enter or Escape: one caret at the end of the primary range.
    pub fn finish_stops(&mut self) {
        let head = self.doc.sels.primary().end();
        self.end_tab_stops();
        self.set_sels(Selections::single(Selection::cursor(head)));
    }

    /// Ends the stops once a caret leaves the current group's ranges.
    pub fn check_tab_stops(&mut self) {
        let Some(stops) = &self.tab_stops else { return };
        let mut ranges = self.stop_ranges(stops.current);
        ranges.sort_unstable();
        let mut sels: Vec<(usize, usize)> = self.doc.sels.iter().map(|s| (s.start(), s.end())).collect();
        sels.sort_unstable();
        // Each caret stays in its own range.
        let inside = sels.len() == ranges.len() && sels.iter().zip(&ranges).all(|(s, r)| s.0 >= r.0 && s.1 <= r.1);
        if !inside {
            self.end_tab_stops();
        }
    }

    /// The ranges of every group, for painting: (start, end, current group).
    pub fn tab_stop_marks(&self) -> Vec<(usize, usize, bool)> {
        let Some(stops) = &self.tab_stops else {
            return Vec::new();
        };
        (0..stops.groups.len())
            .flat_map(|g| {
                self.stop_ranges(g)
                    .into_iter()
                    .map(move |(a, e)| (a, e, g == stops.current))
            })
            .collect()
    }

    /// Replaces `range` with `text` as one undo step, then starts tab stops
    /// at `groups` (offsets in the new text).
    pub fn replace_with_stops(&mut self, range: Range<usize>, text: &str, groups: Vec<Vec<Range<usize>>>) {
        use crate::history::EditKind;
        use crate::text::{Edit, Transaction};
        if self.opts.read_only {
            return;
        }
        self.end_tab_stops();
        let b = &self.doc.buffer;
        let (start, end) = (b.floor_char(range.start.min(b.len())), b.floor_char(range.end.min(b.len())));
        let tx = Transaction::new(vec![Edit::new(start, end, text.to_string())]);
        let caret = start + text.len();
        let sels = Selections::single(Selection::cursor(caret));
        self.edit(crate::commands::Outcome { tx, sels }, EditKind::Other);
        if groups.is_empty() {
            return;
        }
        self.start_tab_stops(groups);
    }

    /// Inserts a snippet in place of `range` and starts its tab stops.
    pub fn insert_snippet(&mut self, range: Range<usize>, snippet: &str) {
        let indent = {
            let b = &self.doc.buffer;
            let line = b.line(b.line_of(range.start.min(b.len())));
            line[..line.len() - line.trim_start_matches([' ', '\t']).len()].to_string()
        };
        let (text, groups) = parse_snippet(snippet);
        // Lines after the first start at the line's indentation.
        let mut shifted = String::with_capacity(text.len());
        let mut map = Vec::with_capacity(text.len() + 1);
        for c in text.chars() {
            for _ in 0..c.len_utf8() {
                map.push(shifted.len());
            }
            shifted.push(c);
            if c == '\n' {
                shifted.push_str(&indent);
            }
        }
        map.push(shifted.len());
        let start = range.start;
        let groups = groups
            .into_iter()
            .map(|g| g.into_iter().map(|r| start + map[r.start]..start + map[r.end]).collect())
            .collect();
        self.replace_with_stops(range, &shifted, groups);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_parse_stops_and_mirrors() {
        let (text, groups) = parse_snippet("select ${1:*} from ${2:t} where $0");
        assert_eq!(text, "select * from t where ");
        assert_eq!(groups, vec![vec![7..8], vec![14..15], vec![22..22]]);
        let (text, groups) = parse_snippet("${1:name} as (\n  $0\n)\nselect * from $1");
        assert_eq!(text, "name as (\n  \n)\nselect * from name");
        assert_eq!(groups, vec![vec![0..4, 29..33], vec![12..12]]);
        assert_eq!(parse_snippet("cost \\$5").0, "cost $5");
    }
}

#[cfg(test)]
mod state_tests {
    use crate::editor::tests::state;
    use crate::keymap::{Action, Chord};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_LEFT, VK_TAB};

    fn selected(st: &crate::editor::State) -> Vec<(usize, usize)> {
        st.doc.sels.iter().map(|s| (s.start(), s.end())).collect()
    }

    #[test]
    fn linked_stops_edit_together_and_tab_moves_on() {
        let mut st = state(None, "x |");
        st.insert_snippet(2..2, "${1:a} + $1 = ${2:b}$0");
        assert_eq!(st.doc.buffer.text(), "x a + a = b");
        assert_eq!(selected(&st), vec![(2, 3), (6, 7)]);
        for c in "zz".chars() {
            st.type_char(c);
        }
        assert_eq!(st.doc.buffer.text(), "x zz + zz = b");
        st.key(Chord::new(VK_TAB.0, false, false, false));
        assert_eq!(selected(&st), vec![(12, 13)]);
        // Tab onto the final caret ends the stops there.
        st.key(Chord::new(VK_TAB.0, false, false, false));
        assert!(st.tab_stops.is_none());
        assert_eq!(selected(&st), vec![(13, 13)]);
    }

    #[test]
    fn leaving_the_group_or_escape_ends_the_stops() {
        let mut st = state(None, "|");
        st.replace_with_stops(0..0, "ab ab", vec![vec![0..2, 3..5]]);
        st.run(Action::Move(crate::keymap::Motion::Left, false));
        st.key(Chord::new(VK_LEFT.0, false, false, false));
        assert!(st.tab_stops.is_none(), "a caret before the stop ends it");

        let mut st = state(None, "|");
        st.replace_with_stops(0..0, "ab ab", vec![vec![0..2, 3..5]]);
        st.key(Chord::new(VK_ESCAPE.0, false, false, false));
        assert!(st.tab_stops.is_none());
        assert_eq!(selected(&st), vec![(2, 2)]);
    }
}
