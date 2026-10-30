//! Caret motion over the text: graphemes, words (with Monaco's word separators),
//! smart Home, and vertical moves that keep a goal column.

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::text::{Buffer, floor_boundary};

/// Monaco's default `wordSeparators`.
pub const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Space,
    Separator,
    Word,
}

pub fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if WORD_SEPARATORS.contains(c) {
        CharClass::Separator
    } else {
        CharClass::Word
    }
}

/// The grapheme boundary before byte `col` of `line`.
pub fn prev_boundary(line: &str, col: usize) -> usize {
    let col = floor_boundary(line, col);
    let mut c = GraphemeCursor::new(col, line.len(), true);
    c.prev_boundary(line, 0).ok().flatten().unwrap_or(0)
}

/// The grapheme boundary after byte `col` of `line`.
pub fn next_boundary(line: &str, col: usize) -> usize {
    let col = floor_boundary(line, col);
    let mut c = GraphemeCursor::new(col, line.len(), true);
    c.next_boundary(line, 0).ok().flatten().unwrap_or(line.len())
}

pub fn prev_grapheme(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    if p.col == 0 {
        return off.saturating_sub(1);
    }
    let line = buf.line(p.line);
    buf.line_start(p.line) + prev_boundary(&line, p.col)
}

pub fn next_grapheme(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    if p.col >= line.len() {
        return (off + 1).min(buf.len());
    }
    buf.line_start(p.line) + next_boundary(&line, p.col)
}

/// A run of characters of one class: (start, end, class).
fn run_before(line: &str, col: usize) -> Option<(usize, usize, CharClass)> {
    let mut it = line[..col].char_indices().rev().skip_while(|&(_, c)| c.is_whitespace());
    let (i, c) = it.next()?;
    let k = class(c);
    let mut start = i;
    for (j, d) in it {
        if class(d) != k {
            break;
        }
        start = j;
    }
    Some((start, i + c.len_utf8(), k))
}

fn run_after(line: &str, col: usize) -> Option<(usize, usize, CharClass)> {
    let mut it = line[col..]
        .char_indices()
        .map(|(i, c)| (i + col, c))
        .skip_while(|&(_, c)| c.is_whitespace());
    let (i, c) = it.next()?;
    let k = class(c);
    let mut end = i + c.len_utf8();
    for (j, d) in it {
        if class(d) != k {
            break;
        }
        end = j + d.len_utf8();
    }
    Some((i, end, k))
}

fn class_at(line: &str, col: usize) -> Option<CharClass> {
    line[col..].chars().next().map(class)
}

/// Column of the start of the word before `col` (Ctrl+Left within a line).
pub fn word_start_left(line: &str, col: usize) -> usize {
    let col = floor_boundary(line, col);
    match run_before(line, col) {
        None => 0,
        // A lone separator glued to the following word (`foo.|bar`) is skipped.
        Some((s, e, CharClass::Separator))
            if e - s == 1 && class_at(line, e) == Some(CharClass::Word) =>
        {
            run_before(line, s).map_or(0, |r| r.0)
        }
        Some((s, _, _)) => s,
    }
}

/// Column of the end of the word after `col` (Ctrl+Right within a line).
pub fn word_end_right(line: &str, col: usize) -> usize {
    let col = floor_boundary(line, col);
    match run_after(line, col) {
        None => line.len(),
        // A lone separator glued to a following word (`|.foo`) is skipped.
        Some((s, e, CharClass::Separator))
            if e - s == 1 && class_at(line, e) == Some(CharClass::Word) =>
        {
            run_after(line, e).map_or(line.len(), |r| r.1)
        }
        Some((_, e, _)) => e,
    }
}

pub fn word_left(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    if p.col == 0 {
        return off.saturating_sub(1);
    }
    buf.line_start(p.line) + word_start_left(&buf.line(p.line), p.col)
}

pub fn word_right(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    if p.col >= line.len() {
        return (off + 1).min(buf.len());
    }
    buf.line_start(p.line) + word_end_right(&line, p.col)
}

/// Where Ctrl+Backspace deletes back to: a run of two or more spaces, or the
/// start of the previous word.
pub fn delete_word_left_target(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    if p.col == 0 {
        return off.saturating_sub(1);
    }
    let line = buf.line(p.line);
    let col = floor_boundary(&line, p.col);
    let ws = line[..col].len() - line[..col].trim_end().len();
    let start = buf.line_start(p.line);
    if ws >= 2 {
        start + col - ws
    } else {
        start + word_start_left(&line, col)
    }
}

/// Where Ctrl+Delete deletes forward to.
pub fn delete_word_right_target(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    if p.col >= line.len() {
        return (off + 1).min(buf.len());
    }
    let col = floor_boundary(&line, p.col);
    let ws = line[col..].len() - line[col..].trim_start().len();
    let start = buf.line_start(p.line);
    if ws >= 2 || col + ws == line.len() {
        start + col + ws
    } else {
        start + word_end_right(&line, col)
    }
}

/// The word (letters, digits, `_` and other non-separators) at or touching
/// `off`, as offsets.
pub fn word_at(buf: &Buffer, off: usize) -> Option<(usize, usize)> {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    let col = floor_boundary(&line, p.col);
    let is_word = |c: char| class(c) == CharClass::Word;
    let mut s = col;
    for (i, c) in line[..col].char_indices().rev() {
        if !is_word(c) {
            break;
        }
        s = i;
    }
    let mut e = col;
    for (i, c) in line[col..].char_indices() {
        if !is_word(c) {
            break;
        }
        e = col + i + c.len_utf8();
    }
    let start = buf.line_start(p.line);
    (e > s).then_some((start + s, start + e))
}

/// The run of one character class at `off`, for double-click: a word, a run of
/// separators or a run of spaces.
pub fn run_at(buf: &Buffer, off: usize) -> (usize, usize) {
    if let Some(w) = word_at(buf, off) {
        return w;
    }
    let p = buf.pos(off);
    let line = buf.line(p.line);
    let col = floor_boundary(&line, p.col);
    let Some(k) = class_at(&line, col) else {
        return (off, off);
    };
    let mut s = col;
    for (i, c) in line[..col].char_indices().rev() {
        if class(c) != k {
            break;
        }
        s = i;
    }
    let mut e = col;
    for (i, c) in line[col..].char_indices() {
        if class(c) != k {
            break;
        }
        e = col + i + c.len_utf8();
    }
    let start = buf.line_start(p.line);
    (start + s, start + e)
}

/// Smart Home: the first non-space character, or the line start when already
/// there.
pub fn home(buf: &Buffer, off: usize) -> usize {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    let indent = line.len() - line.trim_start().len();
    let start = buf.line_start(p.line);
    if p.col == indent { start } else { start + indent }
}

pub fn line_end(buf: &Buffer, off: usize) -> usize {
    buf.line_end(buf.line_of(off))
}

/// Visual column of byte `col`: graphemes count one, tabs go to the next stop.
pub fn visual_col(line: &str, col: usize, tab: usize) -> usize {
    let col = floor_boundary(line, col);
    let mut v = 0;
    for g in line[..col].graphemes(true) {
        v = if g == "\t" { (v / tab + 1) * tab } else { v + 1 };
    }
    v
}

/// The byte column nearest to visual column `v`.
pub fn col_at_visual(line: &str, v: usize, tab: usize) -> usize {
    let mut cur = 0;
    for (i, g) in line.grapheme_indices(true) {
        let next = if g == "\t" { (cur / tab + 1) * tab } else { cur + 1 };
        if next > v {
            // Round to the nearer side of the grapheme.
            return if v - cur > (next - cur) / 2 { i + g.len() } else { i };
        }
        cur = next;
    }
    line.len()
}

/// Moves `lines` lines up (negative) or down from `off`, aiming for the visual
/// column `goal` (the column at `off` when none). Returns the new offset and
/// the goal to keep. Up from the first line goes to its start, down from the last
/// to its end, as in Monaco.
pub fn vertical(
    buf: &Buffer,
    off: usize,
    goal: Option<usize>,
    lines: isize,
    tab: usize,
) -> (usize, usize) {
    let p = buf.pos(off);
    let goal = goal.unwrap_or_else(|| visual_col(&buf.line(p.line), p.col, tab));
    let target = p.line as isize + lines;
    if target < 0 {
        return (0, goal);
    }
    let last = buf.line_count() - 1;
    if target as usize > last {
        return (buf.len(), goal);
    }
    let t = target as usize;
    let line = buf.line(t);
    (buf.line_start(t) + col_at_visual(&line, goal, tab), goal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes() {
        let b = Buffer::new("ae\u{301}x\n😀");
        assert_eq!(next_grapheme(&b, 1), 4);
        assert_eq!(prev_grapheme(&b, 4), 1);
        assert_eq!(next_grapheme(&b, 5), 6);
        assert_eq!(prev_grapheme(&b, 6), 5);
        assert_eq!(next_grapheme(&b, 6), 10);
        assert_eq!(next_grapheme(&b, 10), 10);
        assert_eq!(prev_grapheme(&b, 0), 0);
    }

    #[test]
    fn word_motion_in_line() {
        let l = "foo.bar  baz(x)";
        assert_eq!(word_end_right(l, 0), 3);
        assert_eq!(word_end_right(l, 3), 7);
        assert_eq!(word_end_right(l, 7), 12);
        assert_eq!(word_end_right(l, 12), 14);
        assert_eq!(word_start_left(l, 15), 14);
        assert_eq!(word_start_left(l, 12), 9);
        assert_eq!(word_start_left(l, 9), 4);
        assert_eq!(word_start_left(l, 4), 0);
        assert_eq!(word_start_left("  x", 1), 0);
    }

    #[test]
    fn word_motion_across_lines() {
        let b = Buffer::new("ab\ncd");
        assert_eq!(word_right(&b, 2), 3);
        assert_eq!(word_left(&b, 3), 2);
        assert_eq!(word_right(&b, 5), 5);
    }

    #[test]
    fn delete_word_targets() {
        let b = Buffer::new("foo bar    baz");
        assert_eq!(delete_word_left_target(&b, 7), 4);
        assert_eq!(delete_word_left_target(&b, 11), 7);
        assert_eq!(delete_word_left_target(&b, 4), 0);
        assert_eq!(delete_word_right_target(&b, 7), 11);
        assert_eq!(delete_word_right_target(&b, 0), 3);
    }

    #[test]
    fn words_and_runs() {
        let b = Buffer::new("x = foo_bar + 1");
        assert_eq!(word_at(&b, 6), Some((4, 11)));
        assert_eq!(word_at(&b, 11), Some((4, 11)));
        assert_eq!(word_at(&b, 2), None);
        assert_eq!(run_at(&b, 2), (2, 3));
    }

    #[test]
    fn smart_home() {
        let b = Buffer::new("    x");
        assert_eq!(home(&b, 5), 4);
        assert_eq!(home(&b, 4), 0);
        assert_eq!(home(&b, 0), 4);
    }

    #[test]
    fn visual_columns() {
        assert_eq!(visual_col("\tab", 2, 4), 5);
        assert_eq!(col_at_visual("\tab", 5, 4), 2);
        assert_eq!(col_at_visual("\tab", 2, 4), 0);
        assert_eq!(col_at_visual("ab", 9, 4), 2);
    }

    #[test]
    fn vertical_keeps_goal() {
        let b = Buffer::new("hello\nhi\nworld");
        let (o, g) = vertical(&b, 4, None, 1, 4);
        assert_eq!((o, g), (8, 4));
        let (o, _) = vertical(&b, o, Some(g), 1, 4);
        assert_eq!(o, 13);
        assert_eq!(vertical(&b, 3, None, -1, 4).0, 0);
        assert_eq!(vertical(&b, 10, None, 1, 4).0, b.len());
    }
}
