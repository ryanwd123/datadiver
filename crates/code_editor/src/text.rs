//! The text buffer: a rope with `\n` line breaks, file encodings and EOLs, and
//! conversions between byte offsets, line/column positions and UTF-16.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

use ropey::Rope;

/// Versions are unique across all buffers, so caches keyed by version never
/// mistake a newly loaded text for the previous one.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

/// A position in the text. `col` is a byte offset within the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }
}

/// The line break written when saving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

/// A file's bytes decoded into text with `\n` line breaks.
pub struct Decoded {
    pub text: String,
    pub eol: Eol,
    pub encoding: Encoding,
    /// Invalid UTF-8 or UTF-16 was replaced with U+FFFD.
    pub lossy: bool,
}

/// Decodes UTF-8 (with or without BOM) or BOM-marked UTF-16. The EOL is the
/// dominant one in the file; every line break becomes `\n`.
pub fn decode(bytes: &[u8]) -> Decoded {
    let (text, encoding, lossy) = if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        let (t, lossy) = utf8(b);
        (t, Encoding::Utf8Bom, lossy)
    } else if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let (t, lossy) = utf16(b, u16::from_le_bytes);
        (t, Encoding::Utf16Le, lossy)
    } else if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let (t, lossy) = utf16(b, u16::from_be_bytes);
        (t, Encoding::Utf16Be, lossy)
    } else {
        let (t, lossy) = utf8(bytes);
        (t, Encoding::Utf8, lossy)
    };
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    let eol = if crlf > lf { Eol::Crlf } else { Eol::Lf };
    Decoded {
        text: normalize_eol(&text).into_owned(),
        eol,
        encoding,
        lossy,
    }
}

fn utf8(b: &[u8]) -> (String, bool) {
    match std::str::from_utf8(b) {
        Ok(s) => (s.to_string(), false),
        Err(_) => (String::from_utf8_lossy(b).into_owned(), true),
    }
}

fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> (String, bool) {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| f(c)).collect();
    let odd = b.len() % 2 == 1;
    match String::from_utf16(&units) {
        Ok(s) => (s, odd),
        Err(_) => (String::from_utf16_lossy(&units), true),
    }
}

/// `s` with `\r\n` and lone `\r` turned into `\n`.
pub fn normalize_eol(s: &str) -> Cow<'_, str> {
    if s.contains('\r') {
        Cow::Owned(s.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(s)
    }
}

/// Encodes text for saving with the given EOL and encoding.
pub fn encode(rope: &Rope, eol: Eol, encoding: Encoding) -> Vec<u8> {
    let mut s = String::with_capacity(rope.len_bytes());
    for chunk in rope.chunks() {
        match eol {
            Eol::Lf => s.push_str(chunk),
            Eol::Crlf => s.push_str(&chunk.replace('\n', "\r\n")),
        }
    }
    match encoding {
        Encoding::Utf8 => s.into_bytes(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], s.as_bytes()].concat(),
        Encoding::Utf16Le => [0xFF, 0xFE]
            .into_iter()
            .chain(s.encode_utf16().flat_map(u16::to_le_bytes))
            .collect(),
        Encoding::Utf16Be => [0xFE, 0xFF]
            .into_iter()
            .chain(s.encode_utf16().flat_map(u16::to_be_bytes))
            .collect(),
    }
}

/// One replacement, in byte offsets of the text before any edit of its
/// transaction is applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn new(start: usize, end: usize, text: impl Into<String>) -> Self {
        Edit {
            start,
            end,
            text: text.into(),
        }
    }

    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Edit::new(at, at, text)
    }

    pub fn delete(start: usize, end: usize) -> Self {
        Edit::new(start, end, "")
    }
}

/// Edits applied together: sorted by start and non-overlapping, all in offsets of
/// the text before the transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Transaction {
    pub edits: Vec<Edit>,
}

impl Transaction {
    /// Sorts the edits and drops the overlapping part of any edit that overlaps an
    /// earlier one (two cursors deleting the same character).
    pub fn new(mut edits: Vec<Edit>) -> Self {
        edits.sort_by_key(|e| (e.start, e.end));
        let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
        for mut e in edits {
            if let Some(prev) = out.last_mut() {
                if e.start < prev.end {
                    e.start = prev.end;
                    e.end = e.end.max(prev.end);
                }
                // Two inserts at one point become one.
                if e.start == e.end && prev.start == prev.end && e.start == prev.start {
                    prev.text.push_str(&e.text);
                    continue;
                }
            }
            if e.start == e.end && e.text.is_empty() {
                continue;
            }
            out.push(e);
        }
        Transaction { edits: out }
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Maps an offset in the text before the transaction to the text after it. An
    /// offset at an insertion point moves after the inserted text when `after`.
    pub fn map(&self, off: usize, after: bool) -> usize {
        let mut delta: isize = 0;
        for e in &self.edits {
            let grow = e.text.len() as isize - (e.end - e.start) as isize;
            if off < e.start {
                break;
            }
            if off == e.start {
                if e.start == e.end && after {
                    delta += grow;
                }
                break;
            }
            if off < e.end {
                // Inside a replaced range: the end of the replacement.
                return (e.start as isize + delta) as usize + e.text.len();
            }
            delta += grow;
        }
        (off as isize + delta) as usize
    }
}

/// Where one applied edit was, in the text at the moment it was applied.
/// Edits of a transaction are applied last to first, so each change's offsets
/// are valid in sequence, and the start and old end are also offsets in the
/// text before the transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
    pub start_pos: Pos,
    pub old_end_pos: Pos,
    pub new_end_pos: Pos,
    /// The inserted text.
    pub text: String,
}

/// The rope plus its file format and a version bumped on every change.
#[derive(Clone)]
pub struct Buffer {
    rope: Rope,
    pub eol: Eol,
    pub encoding: Encoding,
    pub version: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new("")
    }
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Buffer {
            rope: Rope::from_str(&normalize_eol(text)),
            eol: Eol::default(),
            encoding: Encoding::default(),
            version: next_version(),
        }
    }

    pub fn from_decoded(d: Decoded) -> Self {
        Buffer {
            rope: Rope::from_str(&d.text),
            eol: d.eol,
            encoding: d.encoding,
            version: next_version(),
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        encode(&self.rope, self.eol, self.encoding)
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `i` without its line break.
    pub fn line(&self, i: usize) -> Cow<'_, str> {
        let s: Cow<str> = self.rope.line(i).into();
        match s {
            Cow::Borrowed(b) => Cow::Borrowed(b.strip_suffix('\n').unwrap_or(b)),
            Cow::Owned(mut o) => {
                if o.ends_with('\n') {
                    o.pop();
                }
                Cow::Owned(o)
            }
        }
    }

    pub fn line_start(&self, i: usize) -> usize {
        self.rope.line_to_byte(i.min(self.line_count()))
    }

    /// Byte length of line `i` without its line break.
    pub fn line_len(&self, i: usize) -> usize {
        self.line_end(i) - self.line_start(i)
    }

    /// Offset of the end of line `i`, before its line break.
    pub fn line_end(&self, i: usize) -> usize {
        if i + 1 >= self.line_count() {
            self.len()
        } else {
            self.line_start(i + 1) - 1
        }
    }

    pub fn line_of(&self, off: usize) -> usize {
        self.rope.byte_to_line(off.min(self.len()))
    }

    pub fn pos(&self, off: usize) -> Pos {
        let off = off.min(self.len());
        let line = self.rope.byte_to_line(off);
        Pos::new(line, off - self.rope.line_to_byte(line))
    }

    /// The offset of `p`, clamped into the text and onto a char boundary.
    pub fn offset(&self, p: Pos) -> usize {
        let line = p.line.min(self.line_count() - 1);
        let start = self.line_start(line);
        let off = start + p.col.min(self.line_len(line));
        self.floor_char(off)
    }

    /// `off` moved back onto a char boundary.
    pub fn floor_char(&self, off: usize) -> usize {
        let off = off.min(self.len());
        self.rope.char_to_byte(self.rope.byte_to_char(off))
    }

    pub fn slice(&self, start: usize, end: usize) -> String {
        let (s, e) = (start.min(self.len()), end.min(self.len()));
        self.rope.byte_slice(s..e.max(s)).to_string()
    }

    pub fn char_at(&self, off: usize) -> Option<char> {
        (off < self.len()).then(|| self.rope.char(self.rope.byte_to_char(off)))
    }

    pub fn char_before(&self, off: usize) -> Option<char> {
        (off > 0 && off <= self.len()).then(|| self.rope.char(self.rope.byte_to_char(off) - 1))
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Replaces the whole text (a new version, not an edit).
    pub fn set_text(&mut self, text: &str) {
        self.rope = Rope::from_str(&normalize_eol(text));
        self.version = next_version();
    }

    /// Applies a transaction, last edit first. Returns the inverse transaction and
    /// the changes in the order they were applied.
    pub fn apply(&mut self, tx: &Transaction) -> (Transaction, Vec<Change>) {
        let mut inverse = Vec::with_capacity(tx.edits.len());
        let mut delta: isize = 0;
        for e in &tx.edits {
            let start = (e.start as isize + delta) as usize;
            inverse.push(Edit::new(start, start + e.text.len(), self.slice(e.start, e.end)));
            delta += e.text.len() as isize - (e.end - e.start) as isize;
        }
        let mut changes = Vec::with_capacity(tx.edits.len());
        for e in tx.edits.iter().rev() {
            changes.push(self.replace(e.start, e.end, &e.text));
        }
        if !tx.edits.is_empty() {
            self.version = next_version();
        }
        (Transaction { edits: inverse }, changes)
    }

    fn replace(&mut self, start: usize, end: usize, text: &str) -> Change {
        let start_pos = self.pos(start);
        let old_end_pos = self.pos(end);
        let cs = self.rope.byte_to_char(start);
        if end > start {
            let ce = self.rope.byte_to_char(end);
            self.rope.remove(cs..ce);
        }
        if !text.is_empty() {
            self.rope.insert(cs, text);
        }
        let new_end = start + text.len();
        Change {
            start,
            old_end: end,
            new_end,
            start_pos,
            old_end_pos,
            new_end_pos: self.pos(new_end),
            text: text.to_string(),
        }
    }
}

/// UTF-16 index in `line` of byte column `col`.
pub fn col_to_utf16(line: &str, col: usize) -> usize {
    let col = col.min(line.len());
    line[..floor_boundary(line, col)].encode_utf16().count()
}

/// Byte column in `line` of UTF-16 index `u`.
pub fn utf16_to_col(line: &str, u: usize) -> usize {
    let mut n = 0;
    for (i, c) in line.char_indices() {
        if n >= u {
            return i;
        }
        n += c.len_utf16();
    }
    line.len()
}

/// `i` moved back onto a char boundary of `s`.
pub fn floor_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_eols_and_boms() {
        let d = decode(b"a\r\nb\r\nc\n");
        assert_eq!(d.text, "a\nb\nc\n");
        assert_eq!(d.eol, Eol::Crlf);
        assert_eq!(d.encoding, Encoding::Utf8);
        let d = decode(b"\xEF\xBB\xBFx\ny\r");
        assert_eq!((d.text.as_str(), d.eol, d.encoding), ("x\ny\n", Eol::Lf, Encoding::Utf8Bom));
        let d = decode(&[0xFF, 0xFE, b'h', 0, b'\r', 0, b'\n', 0, b'i', 0]);
        assert_eq!((d.text.as_str(), d.encoding), ("h\ni", Encoding::Utf16Le));
        let d = decode(&[0xFE, 0xFF, 0, b'o', 0, b'k']);
        assert_eq!((d.text.as_str(), d.encoding), ("ok", Encoding::Utf16Be));
        let d = decode(b"bad \xFF byte");
        assert!(d.lossy);
        assert_eq!(d.text, "bad \u{FFFD} byte");
    }

    #[test]
    fn encode_round_trips() {
        for bytes in [
            &b"a\r\nb\r\n"[..],
            b"\xEF\xBB\xBFx\ny",
            &[0xFF, 0xFE, b'h', 0, b'\r', 0, b'\n', 0, b'i', 0],
            &[0xFE, 0xFF, 0, b'o', 0, b'k'],
        ] {
            let b = Buffer::from_decoded(decode(bytes));
            assert_eq!(b.to_bytes(), bytes);
        }
    }

    #[test]
    fn lines_and_positions() {
        let b = Buffer::new("ab\ncdé\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1), "cdé");
        assert_eq!(b.line(2), "");
        assert_eq!(b.line_len(1), 4);
        assert_eq!(b.line_end(0), 2);
        assert_eq!(b.pos(5), Pos::new(1, 2));
        assert_eq!(b.offset(Pos::new(1, 99)), 7);
        // Inside é snaps back to its start.
        assert_eq!(b.offset(Pos::new(1, 3)), 5);
        assert_eq!(b.offset(Pos::new(9, 0)), b.len());
        assert_eq!(b.char_at(5), Some('é'));
        assert_eq!(b.char_before(7), Some('é'));
    }

    #[test]
    fn apply_and_inverse() {
        let mut b = Buffer::new("hello world");
        let tx = Transaction::new(vec![Edit::new(6, 11, "there"), Edit::insert(0, ">> ")]);
        let v = b.version;
        let (inv, changes) = b.apply(&tx);
        assert_eq!(b.text(), ">> hello there");
        assert!(b.version > v);
        assert_eq!(changes[0].start, 6);
        assert_eq!(changes[1].new_end, 3);
        b.apply(&inv);
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn map_offsets() {
        let tx = Transaction::new(vec![Edit::insert(2, "xx"), Edit::new(5, 7, "")]);
        assert_eq!(tx.map(0, true), 0);
        assert_eq!(tx.map(2, true), 4);
        assert_eq!(tx.map(2, false), 2);
        assert_eq!(tx.map(6, true), 7);
        assert_eq!(tx.map(9, true), 9);
    }

    #[test]
    fn overlapping_edits_are_trimmed() {
        let tx = Transaction::new(vec![Edit::delete(1, 3), Edit::delete(2, 4)]);
        assert_eq!(tx.edits, vec![Edit::delete(1, 3), Edit::delete(3, 4)]);
    }

    #[test]
    fn utf16_conversions() {
        let s = "a😀b";
        assert_eq!(col_to_utf16(s, 5), 3);
        assert_eq!(utf16_to_col(s, 3), 5);
        assert_eq!(utf16_to_col(s, 9), s.len());
    }
}
