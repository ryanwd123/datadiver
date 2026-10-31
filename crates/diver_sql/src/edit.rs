//! Rewrites of a script: the new text, where to put the carets in it, and
//! the helpers rewrites share (indentation, keyword case, tokens near a
//! position).

use rowan::{TextRange, TextSize};

use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

/// A change to the whole text, with the ranges of the new text to select.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    pub text: String,
    /// Tab stops in order. Each is a group of ranges selected together, so
    /// typing in one edits them all; the first group is selected at once.
    pub stops: Vec<Vec<TextRange>>,
}

impl Rewrite {
    /// The smallest single replacement that turns `old` into the new text:
    /// the range in `old` and what replaces it.
    pub fn edit(&self, old: &str) -> (TextRange, String) {
        let new = self.text.as_str();
        let mut prefix = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let max_suffix = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
            suffix -= 1;
        }
        let range = TextRange::new(size(prefix), size(old.len() - suffix));
        (range, new[prefix..new.len() - suffix].to_string())
    }
}

/// How rewrites lay out the lines they add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditStyle {
    /// One level of indentation.
    pub indent: String,
}

impl Default for EditStyle {
    fn default() -> Self {
        EditStyle {
            indent: "    ".into(),
        }
    }
}

pub(crate) fn size(n: usize) -> TextSize {
    TextSize::new(n as u32)
}

pub(crate) fn span(range: TextRange) -> std::ops::Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

/// Text built from pieces, some marked as tab stops.
#[derive(Default)]
pub(crate) struct Piece {
    pub text: String,
    /// (tab stop group, range in `text`).
    marks: Vec<(usize, std::ops::Range<usize>)>,
}

impl Piece {
    pub fn new(text: impl Into<String>) -> Piece {
        Piece {
            text: text.into(),
            marks: Vec::new(),
        }
    }

    pub fn push(&mut self, s: &str) -> &mut Self {
        self.text.push_str(s);
        self
    }

    /// Appends `s` as part of tab stop `group`.
    pub fn mark(&mut self, group: usize, s: &str) -> &mut Self {
        let start = self.text.len();
        self.text.push_str(s);
        self.marks.push((group, start..self.text.len()));
        self
    }
}

/// Replacements of ranges of the old text that don't overlap.
#[derive(Default)]
pub(crate) struct Edits {
    items: Vec<(TextRange, Piece)>,
}

impl Edits {
    pub fn replace(&mut self, range: TextRange, piece: Piece) {
        self.items.push((range, piece));
    }

    pub fn insert(&mut self, at: TextSize, piece: Piece) {
        self.replace(TextRange::empty(at), piece);
    }

    pub fn apply(mut self, old: &str) -> Rewrite {
        self.items.sort_by_key(|(range, _)| (range.start(), range.end()));
        let mut text = String::with_capacity(old.len());
        let mut groups: Vec<Vec<TextRange>> = Vec::new();
        let mut at = 0;
        for (range, piece) in self.items {
            let start = usize::from(range.start()).max(at);
            text.push_str(&old[at..start]);
            let base = text.len();
            text.push_str(&piece.text);
            for (group, r) in piece.marks {
                if groups.len() <= group {
                    groups.resize(group + 1, Vec::new());
                }
                groups[group].push(TextRange::new(size(base + r.start), size(base + r.end)));
            }
            at = usize::from(range.end()).max(start);
        }
        text.push_str(&old[at..]);
        groups.retain(|g| !g.is_empty());
        Rewrite { text, stops: groups }
    }
}

/// The token a position is at: a word touching it, or else the token after it.
pub(crate) fn token_near(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    if offset > root.text_range().end() {
        return None;
    }
    match root.token_at_offset(offset) {
        rowan::TokenAtOffset::None => None,
        rowan::TokenAtOffset::Single(t) => Some(t),
        rowan::TokenAtOffset::Between(l, r) => {
            let word = |t: &SyntaxToken| t.kind().is_word() || t.kind() == SyntaxKind::QUOTED_IDENT;
            Some(if word(&r) {
                r
            } else if word(&l) || r.kind().is_trivia() {
                l
            } else {
                r
            })
        }
    }
}

/// The leading whitespace of the line `offset` is on.
pub(crate) fn line_indent(text: &str, offset: usize) -> &str {
    let start = line_start(text, offset);
    let rest = &text[start..];
    &rest[..rest.len() - rest.trim_start_matches([' ', '\t']).len()]
}

pub(crate) fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// Whether only whitespace comes before `offset` on its line.
pub(crate) fn starts_line(text: &str, offset: usize) -> bool {
    text[line_start(text, offset)..offset].trim().is_empty()
}

/// `snippet`, whose first line started at column `first_col` of its line,
/// moved so its least indented line starts with `indent`. The other lines
/// keep their indentation relative to it.
pub(crate) fn reindent(snippet: &str, first_col: usize, indent: &str) -> String {
    let leading = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let lines: Vec<&str> = snippet.split('\n').collect();
    let least = lines
        .iter()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading(l))
        .chain(std::iter::once(first_col))
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        out.push_str(indent);
        if i == 0 {
            out.extend(std::iter::repeat_n(' ', first_col - least));
            out.push_str(line);
        } else {
            out.push_str(&line[least.min(leading(line))..]);
        }
    }
    out
}

/// Whether keywords in `node` are written in upper case (by its first one).
pub(crate) fn upper_keywords(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind().is_keyword())
        .is_some_and(|t| t.text().chars().any(|c| c.is_ascii_uppercase()))
}

/// `word` in the case the script uses.
pub(crate) fn kw(upper: bool, word: &str) -> String {
    if upper { word.to_uppercase() } else { word.to_lowercase() }
}

/// The first name not in `taken`: `base`, then `base2`, `base3`, ...
pub(crate) fn unique_name(base: &str, taken: &[String]) -> String {
    let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if free(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base}{i}")).find(|n| free(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smallest_edit_keeps_common_ends() {
        let rewrite = Rewrite {
            text: "select a, b from t".into(),
            stops: Vec::new(),
        };
        let (range, text) = rewrite.edit("select a from t");
        assert_eq!(range, TextRange::new(size(8), size(8)));
        assert_eq!(text, ", b");
    }

    #[test]
    fn reindent_moves_lines_together() {
        let snippet = "select\n        a\n    from t";
        assert_eq!(reindent(snippet, 4, "  "), "  select\n      a\n  from t");
        assert_eq!(reindent("x\n\n  y", 0, "\t"), "\tx\n\n\t  y");
    }
}
