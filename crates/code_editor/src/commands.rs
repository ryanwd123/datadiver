//! Pure editing operations: (text, selections) in, (transaction, selections) out.
//! Every command handles any number of cursors.

use crate::movement::{self, visual_col};
use crate::selection::{Selection, Selections};
use crate::text::{Buffer, Edit, Transaction, normalize_eol};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentOptions {
    pub tab_size: usize,
    pub insert_spaces: bool,
}

impl Default for IndentOptions {
    fn default() -> Self {
        IndentOptions {
            tab_size: 4,
            insert_spaces: true,
        }
    }
}

impl IndentOptions {
    /// One level of indentation.
    pub fn unit(&self) -> String {
        if self.insert_spaces {
            " ".repeat(self.tab_size)
        } else {
            "\t".into()
        }
    }
}

/// A command's result: the edits and the selections after them.
#[derive(Debug)]
pub struct Outcome {
    pub tx: Transaction,
    pub sels: Selections,
}

/// One replacement per selection, in selection order.
pub(crate) struct Replace {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// The new selection, relative to `start` in the new text.
    pub anchor: usize,
    pub head: usize,
}

impl Replace {
    /// Replace `start..end` with `text` and put the caret after it.
    pub fn caret_after(start: usize, end: usize, text: String) -> Self {
        let n = text.len();
        Replace {
            start,
            end,
            text,
            anchor: n,
            head: n,
        }
    }
}

/// Builds a transaction from one replacement per selection (sorted like the
/// selections), trimming overlaps, and places the resulting selections.
pub(crate) fn per_selection(
    sels: &Selections,
    mut f: impl FnMut(&Selection) -> Replace,
) -> Outcome {
    let mut edits = Vec::with_capacity(sels.len());
    let mut out = Vec::with_capacity(sels.len());
    let mut delta: isize = 0;
    let mut prev_end = 0;
    for s in sels.iter() {
        let mut r = f(s);
        if r.start < prev_end {
            r.start = prev_end;
            r.end = r.end.max(prev_end);
        }
        let new_start = (r.start as isize + delta) as usize;
        out.push(Selection::new(new_start + r.anchor, new_start + r.head));
        delta += r.text.len() as isize - (r.end - r.start) as isize;
        prev_end = r.end;
        if r.start != r.end || !r.text.is_empty() {
            edits.push(Edit::new(r.start, r.end, r.text));
        }
    }
    Outcome {
        tx: Transaction { edits },
        sels: Selections::new(out, sels.primary_index()),
    }
}

/// Replaces every selection with `text`.
pub fn insert(sels: &Selections, text: &str) -> Outcome {
    let text = normalize_eol(text);
    per_selection(sels, |s| Replace::caret_after(s.start(), s.end(), text.to_string()))
}

/// Pastes clipboard text. With as many lines as cursors, each cursor gets one
/// line; text copied as whole lines goes above the line of each empty cursor.
pub fn paste(buf: &Buffer, sels: &Selections, text: &str, whole_lines: bool) -> Outcome {
    let text = normalize_eol(text);
    let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(&text).split('\n').collect();
    if sels.len() > 1 && lines.len() == sels.len() {
        let mut i = 0;
        return per_selection(sels, |s| {
            i += 1;
            Replace::caret_after(s.start(), s.end(), lines[i - 1].to_string())
        });
    }
    if whole_lines && text.ends_with('\n') && sels.iter().all(|s| s.is_empty()) {
        return per_selection(sels, |s| {
            let start = buf.line_start(buf.line_of(s.head));
            let at = s.head - start + text.len();
            Replace {
                start,
                end: start,
                text: text.to_string(),
                anchor: at,
                head: at,
            }
        });
    }
    insert(sels, &text)
}

/// Backspace: deletes the selection, or the grapheme before the caret, or back
/// to the previous tab stop inside space indentation.
pub fn backspace(buf: &Buffer, sels: &Selections, opts: IndentOptions) -> Outcome {
    per_selection(sels, |s| {
        if !s.is_empty() {
            return Replace::caret_after(s.start(), s.end(), String::new());
        }
        let p = buf.pos(s.head);
        let line = buf.line(p.line);
        let before = &line[..p.col.min(line.len())];
        let start = if opts.insert_spaces && p.col > 0 && before.bytes().all(|b| b == b' ') {
            let stop = (p.col - 1) / opts.tab_size * opts.tab_size;
            s.head - (p.col - stop)
        } else {
            movement::prev_grapheme(buf, s.head)
        };
        Replace::caret_after(start, s.head, String::new())
    })
}

/// Delete: the selection, or the grapheme after the caret.
pub fn delete(buf: &Buffer, sels: &Selections) -> Outcome {
    delete_to(sels, |h| movement::next_grapheme(buf, h))
}

pub fn delete_word_left(buf: &Buffer, sels: &Selections) -> Outcome {
    delete_to(sels, |h| movement::delete_word_left_target(buf, h))
}

pub fn delete_word_right(buf: &Buffer, sels: &Selections) -> Outcome {
    delete_to(sels, |h| movement::delete_word_right_target(buf, h))
}

/// Deletes each selection, or from each caret to `target(caret)`.
fn delete_to(sels: &Selections, target: impl Fn(usize) -> usize) -> Outcome {
    per_selection(sels, |s| {
        let (a, b) = if s.is_empty() {
            let t = target(s.head);
            (t.min(s.head), t.max(s.head))
        } else {
            (s.start(), s.end())
        };
        Replace::caret_after(a, b, String::new())
    })
}

/// Enter: a line break, with indentation from `indent_for(line_before_caret)`.
pub fn newline(
    buf: &Buffer,
    sels: &Selections,
    mut indent_for: impl FnMut(&Buffer, usize) -> String,
) -> Outcome {
    per_selection(sels, |s| {
        let text = format!("\n{}", indent_for(buf, s.start()));
        Replace::caret_after(s.start(), s.end(), text)
    })
}

/// The indentation the line holding `off` has before `off`.
pub fn current_indent(buf: &Buffer, off: usize) -> String {
    let p = buf.pos(off);
    let line = buf.line(p.line);
    let before = &line[..p.col.min(line.len())];
    let rest = before.trim_start_matches([' ', '\t']);
    before[..before.len() - rest.len()].to_string()
}

/// Tab: indents the lines of multi-line selections; otherwise inserts
/// indentation up to the next tab stop.
pub fn tab(buf: &Buffer, sels: &Selections, opts: IndentOptions) -> Outcome {
    if sels.iter().any(|s| buf.line_of(s.start()) != buf.line_of(s.end())) {
        return indent_lines(buf, sels, opts);
    }
    per_selection(sels, |s| {
        let p = buf.pos(s.start());
        let text = if opts.insert_spaces {
            let v = visual_col(&buf.line(p.line), p.col, opts.tab_size);
            " ".repeat(opts.tab_size - v % opts.tab_size)
        } else {
            "\t".into()
        };
        Replace::caret_after(s.start(), s.end(), text)
    })
}

/// The lines each selection covers, deduplicated and ascending. A non-empty
/// selection ending at the start of a line does not cover that line.
pub fn selected_lines(buf: &Buffer, sels: &Selections) -> Vec<usize> {
    let mut lines = Vec::new();
    for s in sels.iter() {
        let a = buf.line_of(s.start());
        let mut b = buf.line_of(s.end());
        if b > a && buf.pos(s.end()).col == 0 {
            b -= 1;
        }
        for l in a..=b {
            if lines.last() != Some(&l) {
                lines.push(l);
            }
        }
    }
    lines.dedup();
    lines
}

/// Maps selections through line-start insertions or deletions; a non-empty
/// selection starting at a line start keeps starting there.
fn map_line_edits(buf: &Buffer, sels: &Selections, tx: &Transaction) -> Selections {
    sels.map(|s| {
        if s.is_empty() {
            return Selection::cursor(tx.map(s.head, true));
        }
        let at_line_start = buf.pos(s.start()).col == 0;
        let a = tx.map(s.start(), !at_line_start);
        let b = tx.map(s.end(), buf.pos(s.end()).col != 0);
        s.with_range(a, b.max(a))
    })
}

/// Ctrl+]: one more level of indentation on every selected line (blank lines
/// are skipped unless only one line is selected).
pub fn indent_lines(buf: &Buffer, sels: &Selections, opts: IndentOptions) -> Outcome {
    let lines = selected_lines(buf, sels);
    let unit = opts.unit();
    let edits = lines
        .iter()
        .filter(|&&l| lines.len() == 1 || buf.line_len(l) > 0)
        .map(|&l| Edit::insert(buf.line_start(l), unit.clone()))
        .collect();
    let tx = Transaction { edits };
    let sels = map_line_edits(buf, sels, &tx);
    Outcome { tx, sels }
}

/// Ctrl+[ / Shift+Tab: removes one level of indentation from every selected line.
pub fn outdent_lines(buf: &Buffer, sels: &Selections, opts: IndentOptions) -> Outcome {
    let mut edits = Vec::new();
    for l in selected_lines(buf, sels) {
        let line = buf.line(l);
        let n = if line.starts_with('\t') {
            1
        } else {
            let spaces = line.len() - line.trim_start_matches(' ').len();
            let k = match spaces % opts.tab_size {
                0 => opts.tab_size,
                r => r,
            };
            let k = k.min(spaces);
            // Spaces before a tab go together with it.
            if k == spaces && line[spaces..].starts_with('\t') { spaces + 1 } else { k }
        };
        if n > 0 {
            let s = buf.line_start(l);
            edits.push(Edit::delete(s, s + n));
        }
    }
    let tx = Transaction { edits };
    let sels = sels.map_tx(&tx);
    Outcome { tx, sels }
}

/// Copied text and whether it is whole lines (nothing was selected).
pub fn copy(buf: &Buffer, sels: &Selections) -> (String, bool) {
    if sels.iter().all(|s| s.is_empty()) {
        let mut lines: Vec<usize> = sels.iter().map(|s| buf.line_of(s.head)).collect();
        lines.dedup();
        let text = lines
            .iter()
            .map(|&l| format!("{}\n", buf.line(l)))
            .collect();
        return (text, true);
    }
    let parts: Vec<String> = sels
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| buf.slice(s.start(), s.end()))
        .collect();
    (parts.join("\n"), false)
}

/// Cut: like copy, then deletes what was copied (whole lines when nothing was
/// selected).
pub fn cut(buf: &Buffer, sels: &Selections) -> (String, bool, Outcome) {
    let (text, whole) = copy(buf, sels);
    if !whole {
        return (text, false, delete_to(sels, |h| h));
    }
    let mut lines: Vec<usize> = sels.iter().map(|s| buf.line_of(s.head)).collect();
    lines.dedup();
    let last = buf.line_count() - 1;
    let edits = lines
        .iter()
        .map(|&l| {
            if l < last {
                Edit::delete(buf.line_start(l), buf.line_start(l + 1))
            } else if l > 0 {
                Edit::delete(buf.line_end(l - 1), buf.len())
            } else {
                Edit::delete(0, buf.len())
            }
        })
        .collect();
    let tx = Transaction::new(edits);
    // Each caret keeps its column on the line that takes its line's place: the
    // next line that stays, or else the previous one.
    let sels = sels.map(|s| {
        let p = buf.pos(s.head);
        let stays = |n: &usize| !lines.contains(n);
        let to = (p.line + 1..=last)
            .find(stays)
            .or_else(|| (0..p.line).rev().find(stays));
        match to {
            Some(n) => Selection::cursor(
                tx.map(buf.line_start(n), false) + p.col.min(buf.line_len(n)),
            ),
            None => Selection::cursor(0),
        }
    });
    (text, true, Outcome { tx, sels })
}

/// What typing needs to know about the language around the carets.
pub struct TypeContext<'a> {
    /// Auto-closing pairs (open, close).
    pub pairs: &'a [(&'a str, &'a str)],
    /// Whether an offset is inside a string or comment.
    pub in_string_or_comment: &'a dyn Fn(usize) -> bool,
    /// Closers the editor inserted itself, as (open, close) offsets; only these
    /// are typed over.
    pub auto_closed: &'a [(usize, usize)],
}

/// Characters before which a bracket or quote auto-closes (Monaco's
/// `autoCloseBefore`), besides whitespace and the end of the line.
const CLOSE_BEFORE: &str = ";:.,=}])>";

/// Types `c` at every selection: types over an auto-inserted closer, wraps a
/// selection in a pair, or inserts a pair when an opener is typed in code.
/// Returns the outcome and the auto-closed pairs it created (in the new text).
pub fn type_char(buf: &Buffer, sels: &Selections, c: char, cx: &TypeContext) -> (Outcome, Vec<(usize, usize)>) {
    let mut cs = [0u8; 4];
    let typed: &str = c.encode_utf8(&mut cs);
    let pair = cx.pairs.iter().find(|p| p.0 == typed);
    let is_closer = cx.pairs.iter().any(|p| p.1 == typed);
    let types_over = |s: &Selection| {
        s.is_empty() && is_closer && buf.char_at(s.head) == Some(c) && cx.auto_closed.iter().any(|&(_, cl)| cl == s.head)
    };
    // Which selections get a pair inserted.
    let mut closes = Vec::new();
    let o = per_selection(sels, |s| {
        let h = s.head;
        let mut closed = false;
        let r = if types_over(s) {
            // Just move past the closer.
            Replace {
                start: h,
                end: h,
                text: String::new(),
                anchor: typed.len(),
                head: typed.len(),
            }
        } else if let Some(&(open, close)) = pair
            && !s.is_empty()
            && !(open == close && buf.line_of(s.start()) != buf.line_of(s.end()))
        {
            let inner = buf.slice(s.start(), s.end());
            let n = inner.len();
            let (a, hd) = if s.reversed() { (open.len() + n, open.len()) } else { (open.len(), open.len() + n) };
            Replace {
                start: s.start(),
                end: s.end(),
                text: format!("{open}{inner}{close}"),
                anchor: a,
                head: hd,
            }
        } else if let Some(&(open, close)) = pair
            && s.is_empty()
            && should_close(buf, h, open, close, cx)
        {
            closed = true;
            Replace {
                start: h,
                end: h,
                text: format!("{open}{close}"),
                anchor: open.len(),
                head: open.len(),
            }
        } else {
            Replace::caret_after(s.start(), s.end(), typed.to_string())
        };
        closes.push(closed);
        r
    });
    let pairs = o
        .sels
        .iter()
        .zip(closes)
        .filter(|(_, closed)| *closed)
        .map(|(s, _)| (s.head - typed.len(), s.head))
        .collect();
    (o, pairs)
}

fn should_close(buf: &Buffer, at: usize, open: &str, close: &str, cx: &TypeContext) -> bool {
    if (cx.in_string_or_comment)(at) {
        return false;
    }
    let next_ok = buf.char_at(at).is_none_or(|n| n.is_whitespace() || CLOSE_BEFORE.contains(n));
    if !next_ok {
        return false;
    }
    // A quote right after a word (`don't`) or another quote stays single.
    if open == close {
        let prev = buf.char_before(at);
        return !prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || open.starts_with(p));
    }
    true
}

/// Backspace between an auto-inserted pair deletes both halves.
pub fn backspace_pairs(buf: &Buffer, sels: &Selections, opts: IndentOptions, auto_closed: &[(usize, usize)]) -> Outcome {
    let pair_at = |s: &Selection| {
        auto_closed
            .iter()
            .find(|&&(o, c)| s.is_empty() && c == s.head && o + buf.char_before(c).map_or(0, char::len_utf8) == c)
            .copied()
    };
    if !sels.iter().all(|s| pair_at(s).is_some()) {
        return backspace(buf, sels, opts);
    }
    per_selection(sels, |s| {
        let (o, c) = pair_at(s).unwrap();
        let close_len = buf.char_at(c).map_or(0, char::len_utf8);
        Replace::caret_after(o, c + close_len, String::new())
    })
}

/// Enter at every selection, with the indentation (and line layout) `enter`
/// decides for each caret offset.
pub fn newline_with(sels: &Selections, mut enter: impl FnMut(usize) -> crate::indent::Enter) -> Outcome {
    per_selection(sels, |s| {
        let e = enter(s.start());
        Replace {
            start: s.start(),
            end: s.end(),
            text: e.text,
            anchor: e.caret,
            head: e.caret,
        }
    })
}

/// Ctrl+/: comments out the selected lines with `token`, or uncomments them
/// when every non-blank one is commented.
pub fn toggle_line_comment(buf: &Buffer, sels: &Selections, token: &str) -> Outcome {
    let lines = selected_lines(buf, sels);
    let indent_of = |l: usize| {
        let line = buf.line(l);
        line.len() - line.trim_start_matches([' ', '\t']).len()
    };
    let blank = |l: usize| buf.line(l).trim().is_empty();
    let content: Vec<usize> = lines.iter().copied().filter(|&l| !blank(l)).collect();
    let targets = if content.is_empty() { lines.clone() } else { content };
    let all_commented = targets.iter().all(|&l| buf.line(l)[indent_of(l)..].starts_with(token));
    let mut edits = Vec::new();
    if all_commented {
        for &l in &targets {
            let line = buf.line(l);
            let i = indent_of(l);
            let n = token.len() + usize::from(line[i + token.len()..].starts_with(' '));
            let at = buf.line_start(l) + i;
            edits.push(Edit::delete(at, at + n));
        }
    } else {
        let col = targets.iter().map(|&l| indent_of(l)).min().unwrap_or(0);
        for &l in &targets {
            edits.push(Edit::insert(buf.line_start(l) + col, format!("{token} ")));
        }
    }
    let tx = Transaction { edits };
    let sels = map_line_edits(buf, sels, &tx);
    Outcome { tx, sels }
}

/// Shift+Alt+A: wraps each selection in a block comment, or unwraps it when it
/// is one. An empty selection gets an empty comment with the caret inside.
pub fn toggle_block_comment(buf: &Buffer, sels: &Selections, open: &str, close: &str) -> Outcome {
    per_selection(sels, |s| {
        let text = buf.slice(s.start(), s.end());
        let trimmed = text.trim();
        if trimmed.len() >= open.len() + close.len() && trimmed.starts_with(open) && trimmed.ends_with(close) {
            let lead = text.len() - text.trim_start().len();
            let inner = &trimmed[open.len()..trimmed.len() - close.len()];
            let inner = inner.strip_prefix(' ').unwrap_or(inner);
            let inner = inner.strip_suffix(' ').unwrap_or(inner);
            let start = s.start() + lead;
            return Replace {
                start,
                end: start + trimmed.len(),
                text: inner.to_string(),
                anchor: 0,
                head: inner.len(),
            };
        }
        let (a, h) = (open.len() + 1, open.len() + 1 + text.len());
        Replace {
            start: s.start(),
            end: s.end(),
            text: format!("{open} {text} {close}"),
            anchor: a,
            head: h,
        }
    })
}

/// Contiguous runs of selected lines: (first, last), ascending.
pub fn line_blocks(buf: &Buffer, sels: &Selections) -> Vec<(usize, usize)> {
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for l in selected_lines(buf, sels) {
        match blocks.last_mut() {
            Some(b) if b.1 + 1 >= l => b.1 = b.1.max(l),
            _ => blocks.push((l, l)),
        }
    }
    blocks
}

/// Maps every selection through a line rearrangement: `line_map(line)` gives
/// a position's new line (its column stays), in `after` (the buffer with the
/// edit applied). A non-empty selection ending at the start of the line after
/// a block keeps ending right after that block.
fn remap_lines(
    buf: &Buffer,
    after: &Buffer,
    sels: &Selections,
    blocks: &[(usize, usize)],
    shift: isize,
    line_map: impl Fn(usize) -> usize,
) -> Selections {
    let place = |off: usize, is_end: bool, non_empty: bool| {
        let p = buf.pos(off);
        if is_end && non_empty && p.col == 0 && blocks.iter().any(|b| b.1 + 1 == p.line) {
            let line = (p.line as isize + shift).max(0) as usize;
            return if line >= after.line_count() { after.len() } else { after.line_start(line) };
        }
        let line = line_map(p.line);
        after.line_start(line) + p.col.min(after.line_len(line))
    };
    sels.map(|s| {
        let ne = !s.is_empty();
        let (a, b) = (place(s.start(), false, ne), place(s.end(), true, ne));
        s.with_range(a, b.max(a))
    })
}

/// Alt+Up/Down: moves the selected lines past the line above or below.
pub fn move_lines(buf: &Buffer, sels: &Selections, down: bool) -> Option<Outcome> {
    let blocks = line_blocks(buf, sels);
    let last = buf.line_count() - 1;
    if blocks.iter().any(|&(a, b)| if down { b >= last } else { a == 0 }) {
        return None;
    }
    let mut edits = Vec::new();
    for &(a, b) in &blocks {
        let (first, other) = if down { (a, b + 1) } else { (a - 1, a - 1) };
        let end_line = if down { b + 1 } else { b };
        let block = buf.slice(buf.line_start(a), buf.line_end(b));
        let o = buf.line(other).into_owned();
        let text = if down { format!("{o}\n{block}") } else { format!("{block}\n{o}") };
        edits.push(Edit::new(buf.line_start(first), buf.line_end(end_line), text));
    }
    let tx = Transaction { edits };
    let mut after = buf.clone();
    after.apply(&tx);
    let d: isize = if down { 1 } else { -1 };
    let sels = remap_lines(buf, &after, sels, &blocks, d, |l| {
        for &(a, b) in &blocks {
            if (a..=b).contains(&l) {
                return (l as isize + d) as usize;
            }
            if down && l == b + 1 {
                return a;
            }
            if !down && l + 1 == a {
                return b;
            }
        }
        l
    });
    Some(Outcome { tx, sels })
}

/// Shift+Alt+Up/Down: duplicates the selected lines; the selection goes to
/// the lower copy when copying down.
pub fn copy_lines(buf: &Buffer, sels: &Selections, down: bool) -> Outcome {
    let blocks = line_blocks(buf, sels);
    let edits: Vec<Edit> = blocks
        .iter()
        .map(|&(a, b)| {
            let block = buf.slice(buf.line_start(a), buf.line_end(b));
            Edit::insert(buf.line_start(a), format!("{block}\n"))
        })
        .collect();
    let tx = Transaction { edits };
    let mut after = buf.clone();
    after.apply(&tx);
    // Lines added above each line by the copies.
    let shift_of = |l: usize| -> usize {
        blocks
            .iter()
            .filter(|&&(a, b)| if down { a <= l } else { b < l })
            .map(|&(a, b)| b - a + 1)
            .sum()
    };
    let sels = remap_lines(buf, &after, sels, &blocks, 0, |l| l + shift_of(l));
    Outcome { tx, sels }
}

/// Ctrl+Shift+K: deletes the selected lines.
pub fn delete_lines(buf: &Buffer, sels: &Selections) -> Outcome {
    let blocks = line_blocks(buf, sels);
    let last = buf.line_count() - 1;
    let edits: Vec<Edit> = blocks
        .iter()
        .map(|&(a, b)| {
            if b < last {
                Edit::delete(buf.line_start(a), buf.line_start(b + 1))
            } else if a > 0 {
                Edit::delete(buf.line_end(a - 1), buf.len())
            } else {
                Edit::delete(0, buf.len())
            }
        })
        .collect();
    let tx = Transaction::new(edits);
    let mut after = buf.clone();
    after.apply(&tx);
    // Carets keep their column on the line that takes their block's place.
    let sels = sels.map(|s| {
        let p = buf.pos(s.head);
        let off = tx.map(buf.line_start(p.line), false);
        let line = after.line_of(off);
        Selection::cursor(after.line_start(line) + p.col.min(after.line_len(line)))
    });
    Outcome { tx, sels }
}

/// Ctrl+L: selects the lines of each selection; again extends by a line.
pub fn select_line(buf: &Buffer, sels: &Selections) -> Selections {
    sels.map(|s| {
        let (sp, ep) = (buf.pos(s.start()), buf.pos(s.end()));
        // A selection ending at a line start (not of whole lines) ends there.
        let partial_to_line_start = !s.is_empty() && ep.col == 0 && sp.col != 0;
        let last = if partial_to_line_start { ep.line } else { ep.line + 1 };
        let end = if last >= buf.line_count() { buf.len() } else { buf.line_start(last) };
        Selection::new(buf.line_start(sp.line), end)
    })
}

/// Ctrl+Shift+Enter: a new line above each caret's line, with its indentation.
pub fn insert_line_above(buf: &Buffer, sels: &Selections) -> Outcome {
    per_selection(sels, |s| {
        let line = buf.line_of(s.head);
        let start = buf.line_start(line);
        let indent = current_indent(buf, buf.line_end(line));
        let n = indent.len();
        Replace {
            start,
            end: start,
            text: format!("{indent}\n"),
            anchor: n,
            head: n,
        }
    })
}

/// Ctrl+Enter: a new line below each caret's line; `enter` gives the line
/// break and indentation for the end of a line.
pub fn insert_line_below(buf: &Buffer, sels: &Selections, enter: impl Fn(usize) -> String) -> Outcome {
    per_selection(sels, |s| {
        let end = buf.line_end(buf.line_of(s.head));
        Replace::caret_after(end, end, enter(end))
    })
}

/// Byte ranges of `needle` in `hay` from `from` on, wrapping around, as
/// whole words when `whole_word`.
pub fn occurrences<'a>(
    hay: &'a str,
    needle: &'a str,
    whole_word: bool,
) -> impl Iterator<Item = (usize, usize)> + 'a {
    let is_word = |c: Option<char>| c.is_some_and(|c| crate::movement::class(c) == crate::movement::CharClass::Word);
    hay.match_indices(needle).map(|(i, m)| (i, i + m.len())).filter(move |&(a, b)| {
        !whole_word || (!is_word(hay[..a].chars().next_back()) && !is_word(hay[b..].chars().next()))
    })
}

pub fn select_all(buf: &Buffer) -> Selections {
    Selections::single(Selection::new(0, buf.len()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Text with `|` marking cursors and `[`...`]` marking selections (anchor at
    /// `[`), parsed into a buffer and selections.
    pub fn parse(marked: &str) -> (Buffer, Selections) {
        let mut text = String::new();
        let mut sels = Vec::new();
        let mut open = None;
        for c in marked.chars() {
            match c {
                '|' => sels.push(Selection::cursor(text.len())),
                '[' => open = Some(text.len()),
                ']' => sels.push(Selection::new(open.take().unwrap(), text.len())),
                _ => text.push(c),
            }
        }
        (Buffer::new(&text), Selections::new(sels, 0))
    }

    /// The inverse of `parse`.
    pub fn show(buf: &Buffer, sels: &Selections) -> String {
        let text = buf.text();
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in sels.iter() {
            if s.is_empty() {
                marks.push((s.head, '|'));
            } else {
                marks.push((s.start(), '['));
                marks.push((s.end(), ']'));
            }
        }
        marks.sort_by_key(|m| (m.0, m.1 != ']'));
        let mut out = String::new();
        let mut last = 0;
        for (at, c) in marks {
            out.push_str(&text[last..at]);
            out.push(c);
            last = at;
        }
        out.push_str(&text[last..]);
        out
    }

    /// Applies a command to marked text and returns the marked result; undoing
    /// must restore the original text.
    pub fn run(marked: &str, cmd: impl Fn(&Buffer, &Selections) -> Outcome) -> String {
        let (mut buf, sels) = parse(marked);
        let original = buf.text();
        let c = cmd(&buf, &sels);
        let (inverse, _) = buf.apply(&c.tx);
        let out = show(&buf, &c.sels);
        buf.apply(&inverse);
        assert_eq!(buf.text(), original, "undo did not round-trip");
        out
    }

    const OPTS: IndentOptions = IndentOptions {
        tab_size: 4,
        insert_spaces: true,
    };

    #[test]
    fn insert_text() {
        assert_eq!(run("a|b", |_, s| insert(s, "xy")), "axy|b");
        assert_eq!(run("[ab]c", |_, s| insert(s, "Z")), "Z|c");
        assert_eq!(run("a|b|c", |_, s| insert(s, "-")), "a-|b-|c");
        assert_eq!(run("|x\n|y", |_, s| insert(s, "\r\n")), "\n|x\n\n|y");
    }

    #[test]
    fn paste_distributes_lines() {
        assert_eq!(run("|a\n|b", |b, s| paste(b, s, "1\n2", false)), "1|a\n2|b");
        assert_eq!(run("|a\n|b", |b, s| paste(b, s, "1\n2\n3", false)), "1\n2\n3|a\n1\n2\n3|b");
        assert_eq!(run("a|b\nc", |b, s| paste(b, s, "line\n", true)), "line\na|b\nc");
        assert_eq!(run("a[b]c", |b, s| paste(b, s, "line\n", true)), "aline\n|c");
    }

    #[test]
    fn backspace_cases() {
        assert_eq!(run("ab|c", |b, s| backspace(b, s, OPTS)), "a|c");
        assert_eq!(run("|abc", |b, s| backspace(b, s, OPTS)), "|abc");
        assert_eq!(run("a\n|b", |b, s| backspace(b, s, OPTS)), "a|b");
        assert_eq!(run("x😀|", |b, s| backspace(b, s, OPTS)), "x|");
        assert_eq!(run("e\u{301}|", |b, s| backspace(b, s, OPTS)), "|");
        assert_eq!(run("      |x", |b, s| backspace(b, s, OPTS)), "    |x");
        assert_eq!(run("    |x", |b, s| backspace(b, s, OPTS)), "|x");
        assert_eq!(run("a|b|c", |b, s| backspace(b, s, OPTS)), "|c");
        assert_eq!(run("[ab]|c", |b, s| backspace(b, s, OPTS)), "|c");
    }

    #[test]
    fn delete_cases() {
        assert_eq!(run("a|bc", delete), "a|c");
        assert_eq!(run("a|\nb", delete), "a|b");
        assert_eq!(run("ab|", delete), "ab|");
        assert_eq!(run("|ab|cd", delete), "|b|d");
        assert_eq!(run("foo bar|", delete_word_left), "foo |");
        assert_eq!(run("|foo bar", delete_word_right), "| bar");
        assert_eq!(run("a.b|  c.d|", delete_word_left), "a.|  c.|");
    }

    #[test]
    fn newline_keeps_indent() {
        assert_eq!(run("    ab|cd", |b, s| newline(b, s, current_indent)), "    ab\n    |cd");
        assert_eq!(run("  |  x", |b, s| newline(b, s, current_indent)), "  \n  |  x");
        assert_eq!(run("a|\nb|", |b, s| newline(b, s, current_indent)), "a\n|\nb\n|");
    }

    #[test]
    fn tab_and_indent() {
        assert_eq!(run("ab|", |b, s| tab(b, s, OPTS)), "ab  |");
        let tabs = IndentOptions { insert_spaces: false, ..OPTS };
        assert_eq!(run("|x", |b, s| tab(b, s, tabs)), "\t|x");
        assert_eq!(run("[a\nb]", |b, s| tab(b, s, OPTS)), "[    a\n    b]");
        assert_eq!(run("[a\n\nb]", |b, s| tab(b, s, OPTS)), "[    a\n\n    b]");
        assert_eq!(run("a|", |b, s| indent_lines(b, s, OPTS)), "    a|");
        assert_eq!(run("[a\n]b", |b, s| indent_lines(b, s, OPTS)), "[    a\n]b");
    }

    #[test]
    fn outdent() {
        assert_eq!(run("      a|", |b, s| outdent_lines(b, s, OPTS)), "    a|");
        assert_eq!(run("    a|", |b, s| outdent_lines(b, s, OPTS)), "a|");
        assert_eq!(run("\t\ta|", |b, s| outdent_lines(b, s, OPTS)), "\ta|");
        assert_eq!(run("  \ta|", |b, s| outdent_lines(b, s, OPTS)), "a|");
        assert_eq!(run("a|", |b, s| outdent_lines(b, s, OPTS)), "a|");
        assert_eq!(run("[    a\n    b]", |b, s| outdent_lines(b, s, OPTS)), "[a\nb]");
    }

    fn cx<'a>(pairs: &'a [(&'a str, &'a str)], auto: &'a [(usize, usize)]) -> TypeContext<'a> {
        TypeContext {
            pairs,
            in_string_or_comment: &|_| false,
            auto_closed: auto,
        }
    }

    const PAIRS: &[(&str, &str)] = &[("(", ")"), ("[", "]"), ("\"", "\""), ("'", "'")];

    fn typed(marked: &str, c: char, auto: &[(usize, usize)]) -> (String, Vec<(usize, usize)>) {
        let (b, s) = parse(marked);
        let (o, pairs) = type_char(&b, &s, c, &cx(PAIRS, auto));
        let mut b2 = b.clone();
        b2.apply(&o.tx);
        (show(&b2, &o.sels), pairs)
    }

    #[test]
    fn auto_closing() {
        assert_eq!(typed("f|", '(', &[]), ("f(|)".into(), vec![(1, 2)]));
        assert_eq!(typed("|x", '(', &[]).0, "(|x");
        assert_eq!(typed("|)", '(', &[]).0, "(|))");
        assert_eq!(typed("a = |", '"', &[]).0, "a = \"|\"");
        assert_eq!(typed("don|", '\'', &[]).0, "don'|");
        assert_eq!(typed("[ab]", '(', &[]).0, "([ab])");
        assert_eq!(typed("a|\nb|", '[', &[]), ("a[|]\nb[|]".into(), vec![(1, 2), (5, 6)]));
        // Type over an auto-inserted closer, but not one typed by hand.
        assert_eq!(typed("f(|)", ')', &[(1, 2)]).0, "f()|");
        assert_eq!(typed("f(|)", ')', &[]).0, "f()|)");
        // Not in strings or comments.
        let (b, s) = parse("# x|");
        let c = TypeContext {
            pairs: PAIRS,
            in_string_or_comment: &|_| true,
            auto_closed: &[],
        };
        let (o, _) = type_char(&b, &s, '(', &c);
        let mut b2 = b.clone();
        b2.apply(&o.tx);
        assert_eq!(show(&b2, &o.sels), "# x(|");
    }

    #[test]
    fn backspace_deletes_auto_pairs() {
        assert_eq!(run("f(|)", |b, s| backspace_pairs(b, s, OPTS, &[(1, 2)])), "f|");
        assert_eq!(run("f(|)", |b, s| backspace_pairs(b, s, OPTS, &[])), "f|)");
    }

    #[test]
    fn line_comments() {
        let t = |m: &str| run(m, |b, s| toggle_line_comment(b, s, "#"));
        assert_eq!(t("[  a\n    b\n]c"), "[  # a\n  #   b\n]c");
        assert_eq!(t("  # a|\n  #   b"), "  a|\n  #   b");
        assert_eq!(t("[  # a\n  #   b]"), "[  a\n    b]");
        assert_eq!(t("[a\n\nb]"), "[# a\n\n# b]");
        assert_eq!(t("|"), "# |");
    }

    #[test]
    fn block_comments() {
        let t = |m: &str| run(m, |b, s| toggle_block_comment(b, s, "/*", "*/"));
        assert_eq!(t("a [b] c"), "a /* [b] */ c");
        assert_eq!(t("a [/* b */] c"), "a [b] c");
        assert_eq!(t("x|"), "x/* | */");
    }

    #[test]
    fn move_lines_up_and_down() {
        let mv = |m: &str, down: bool| run(m, |b, s| move_lines(b, s, down).unwrap());
        assert_eq!(mv("a\nb|\nc", true), "a\nc\nb|");
        assert_eq!(mv("a\nb|\nc", false), "b|\na\nc");
        assert_eq!(mv("a\n[b\nc]\nd", true), "a\nd\n[b\nc]");
        assert_eq!(mv("a\n[b\n]c\nd", true), "a\nc\n[b\n]d");
        assert_eq!(mv("a|\nb\nc|\nd\ne", true), "b\na|\nd\nc|\ne");
        let (b, s) = parse("a\nb|");
        assert!(move_lines(&b, &s, true).is_none());
    }

    #[test]
    fn copy_and_delete_lines() {
        assert_eq!(run("a|b\nc", |b, s| copy_lines(b, s, true)), "ab\na|b\nc");
        assert_eq!(run("a|b\nc", |b, s| copy_lines(b, s, false)), "a|b\nab\nc");
        assert_eq!(run("x\n[a\nb]", |b, s| copy_lines(b, s, true)), "x\na\nb\n[a\nb]");
        assert_eq!(run("a\nb|c\nd", delete_lines), "a\nd|");
        assert_eq!(run("a\nbc|", delete_lines), "a|");
        assert_eq!(run("a|\nb|\nc", delete_lines), "c|");
    }

    #[test]
    fn line_selection_and_insertion() {
        let (b, s) = parse("a\nb|c\nd");
        let s1 = select_line(&b, &s);
        assert_eq!(show(&b, &s1), "a\n[bc\n]d");
        assert_eq!(show(&b, &select_line(&b, &s1)), "a\n[bc\nd]");
        assert_eq!(run("  a|b", insert_line_above), "  |\n  ab");
        assert_eq!(
            run("  a|b", |b, s| insert_line_below(b, s, |off| format!("\n{}", current_indent(b, off)))),
            "  ab\n  |"
        );
    }

    #[test]
    fn finds_occurrences() {
        let v: Vec<_> = occurrences("foo food foo_x foo", "foo", true).collect();
        assert_eq!(v, vec![(0, 3), (15, 18)]);
        assert_eq!(occurrences("aaa", "a", false).count(), 3);
    }

    #[test]
    fn copy_and_cut() {
        let (b, s) = parse("a[bc]d\nef[g]");
        assert_eq!(copy(&b, &s), ("bc\ng".to_string(), false));
        let (b, s) = parse("a|b\nc|d\n");
        assert_eq!(copy(&b, &s), ("ab\ncd\n".to_string(), true));
        let (b, s) = parse("x|y|z\n");
        assert_eq!(copy(&b, &s), ("xyz\n".to_string(), true));

        assert_eq!(run("one\nt|wo\nthree", |b, s| cut(b, s).2), "one\nt|hree");
        assert_eq!(run("one\ntwo|", |b, s| cut(b, s).2), "one|");
        assert_eq!(run("only|", |b, s| cut(b, s).2), "|");
        assert_eq!(run("a[b]c", |b, s| cut(b, s).2), "a|c");
    }
}
