//! The suggest popup: completion items from the host's provider and the
//! built-in word provider, fuzzy-filtered by the word being typed.

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::Rect;
use crate::text::Pos;

/// What kind of thing a completion is (picks its icon color).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    #[default]
    Text,
    Function,
    Variable,
    Field,
    Class,
    Module,
    Constant,
}

impl CompletionKind {
    /// A one-letter icon.
    pub fn icon(self) -> &'static str {
        match self {
            CompletionKind::Keyword => "k",
            CompletionKind::Text => "a",
            CompletionKind::Function => "f",
            CompletionKind::Variable => "v",
            CompletionKind::Field => "c",
            CompletionKind::Class => "T",
            CompletionKind::Module => "m",
            CompletionKind::Constant => "#",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompletionItem {
    /// Shown in the list and matched against what is typed.
    pub label: String,
    /// Inserted on accept (`label` when `None`).
    pub insert_text: Option<String>,
    /// Shown dimmed after the label (a type, a table, ...).
    pub detail: Option<String>,
    pub kind: CompletionKind,
    /// The byte range the item replaces, when it is not the word before the
    /// caret (a path inside a string, a quoted name). Items with their own
    /// range are listed in the provider's order and asked for again as the
    /// user types.
    pub range: Option<std::ops::Range<usize>>,
    /// Open the list again after accepting (a folder, to list what is inside).
    pub retrigger: bool,
    /// Orders items that match equally well, and the whole list before
    /// anything is typed (lower first; items without one go last).
    pub sort_text: Option<String>,
}

impl CompletionItem {
    pub fn new(label: impl Into<String>, kind: CompletionKind) -> Self {
        CompletionItem {
            label: label.into(),
            kind,
            ..Default::default()
        }
    }
}

/// What a provider gets to decide on completions.
pub struct CompletionContext<'a> {
    /// The whole text.
    pub text: &'a str,
    /// Byte offset of the caret in `text`, and its position.
    pub offset: usize,
    pub pos: Pos,
    /// The word being typed, before the caret (possibly empty).
    pub prefix: &'a str,
    /// The character that opened the popup, if a trigger character did.
    pub trigger: Option<char>,
    pub language: Option<&'static str>,
}

/// Supplies completion items. Calls are synchronous; keep them fast.
pub trait CompletionProvider {
    fn complete(&self, cx: &CompletionContext) -> Vec<CompletionItem>;

    /// Characters that open the popup when typed (like `.`).
    fn trigger_characters(&self) -> Vec<char> {
        Vec::new()
    }

    /// Whether the editor adds the language's keywords and the text's words
    /// to these items. A provider that knows what fits the caret says no.
    fn extra_words(&self) -> bool {
        true
    }
}

/// One shown row: an index into `items` and the matched character positions
/// in its label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub item: usize,
    pub matches: Vec<u32>,
}

/// Row height, rows and width at the default 13 DIP font.
pub const ROW_H: f32 = 22.0;
pub const MAX_ROWS: usize = 12;
const WIDTH: f32 = 430.0;

/// How big the popup draws, from the editor's suggest options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SuggestStyle {
    pub row_h: f32,
    pub max_rows: usize,
    pub width: f32,
}

impl Default for SuggestStyle {
    fn default() -> Self {
        SuggestStyle {
            row_h: ROW_H,
            max_rows: MAX_ROWS,
            width: WIDTH,
        }
    }
}

impl SuggestStyle {
    /// Rows and width grow with the font, as at 13 DIPs they are 22 and 430.
    pub fn for_font(size: f32, rows: usize) -> Self {
        let k = size.clamp(6.0, 48.0) / 13.0;
        SuggestStyle {
            row_h: (ROW_H * k).round(),
            max_rows: rows.clamp(3, 40),
            width: (WIDTH * k).round(),
        }
    }
}

#[derive(Default)]
pub struct Suggest {
    pub open: bool,
    /// All items from the providers, before filtering.
    pub items: Vec<CompletionItem>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// First visible row.
    pub top: usize,
    /// Where the word being completed starts (byte offset).
    pub word_start: usize,
    /// The items replace the provider's own range from `word_start`.
    pub ranged: bool,
    pub style: SuggestStyle,
}

impl Suggest {
    /// Filters and sorts the items by `prefix`: best fuzzy score first, then
    /// shorter labels, then alphabetically. An empty prefix keeps everything.
    pub fn filter(&mut self, prefix: &str) {
        if self.ranged {
            // The provider already filtered and ranked what fits its range.
            self.rows = (0..self.items.len()).map(|item| Row { item, matches: Vec::new() }).collect();
            self.selected = 0;
            self.top = 0;
            return;
        }
        let mut config = Config::DEFAULT;
        config.prefer_prefix = true;
        let mut matcher = Matcher::new(config);
        let atom = Atom::new(prefix, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy, false);
        let mut buf = Vec::new();
        let mut scored: Vec<(u16, Row)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                if prefix.is_empty() {
                    return Some((0, Row { item: i, matches: Vec::new() }));
                }
                let mut idx = Vec::new();
                let score = atom.indices(Utf32Str::new(&it.label, &mut buf), &mut matcher, &mut idx)?;
                idx.sort_unstable();
                idx.dedup();
                Some((score, Row { item: i, matches: idx }))
            })
            .collect();
        let items = &self.items;
        scored.sort_by(|(sa, a), (sb, b)| {
            let (ia, ib) = (&items[a.item], &items[b.item]);
            let (la, lb) = (&ia.label, &ib.label);
            let rank = |it: &CompletionItem| (it.sort_text.is_none(), it.sort_text.clone());
            sb.cmp(sa)
                .then_with(|| rank(ia).cmp(&rank(ib)))
                .then(la.len().cmp(&lb.len()))
                .then(la.to_lowercase().cmp(&lb.to_lowercase()))
        });
        self.rows = scored.into_iter().map(|(_, r)| r).collect();
        self.selected = 0;
        self.top = 0;
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.rows.get(self.selected).map(|r| &self.items[r.item])
    }

    /// Moves the selection by `n` rows (wrapping at the ends, as in Monaco).
    pub fn step(&mut self, n: isize) {
        let len = self.rows.len() as isize;
        if len == 0 {
            return;
        }
        let s = self.selected as isize + n;
        self.selected = if s < 0 {
            if self.selected == 0 { (len - 1) as usize } else { 0 }
        } else if s >= len {
            if self.selected as isize == len - 1 { 0 } else { (len - 1) as usize }
        } else {
            s as usize
        };
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + self.style.max_rows {
            self.top = self.selected + 1 - self.style.max_rows;
        }
    }

    /// The popup rectangle for a caret box at (x, y, line height), inside an
    /// editor of `width` x `height`: below the caret, or above when there is no
    /// room below.
    pub fn frame(&self, x: f32, y: f32, lh: f32, width: f32, height: f32) -> Rect {
        let n = self.rows.len().min(self.style.max_rows) as f32;
        let h = n * self.style.row_h + 2.0;
        let w = self.style.width.min(width - 8.0).max(120.0);
        let l = x.min(width - w - 4.0).max(0.0);
        let t = if y + lh + h <= height || y - h < 0.0 { y + lh } else { y - h };
        Rect::new(l, t, l + w, t + h)
    }

    /// The row at y inside `frame`.
    pub fn row_at(&self, frame: Rect, y: f32) -> Option<usize> {
        let i = ((y - frame.t - 1.0) / self.style.row_h).floor();
        let i = self.top + (i.max(0.0) as usize);
        (i < self.rows.len() && i < self.top + self.style.max_rows).then_some(i)
    }
}

/// Words of the text for the built-in provider: identifiers of two or more
/// characters, without duplicates, skipping the word at `skip` (the one being
/// typed). At most `limit` words.
pub fn buffer_words(text: &str, skip: std::ops::Range<usize>, limit: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
        let word_char = c.is_alphanumeric() || c == '_';
        match (start, word_char) {
            (None, true) => start = Some(i),
            (Some(s), false) => {
                let w = &text[s..i];
                if w.chars().count() >= 2
                    && !w.starts_with(|c: char| c.is_ascii_digit())
                    && !(s == skip.start && i == skip.end)
                    && seen.insert(w)
                {
                    out.push(w.to_string());
                    if out.len() >= limit {
                        break;
                    }
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(s: &Suggest) -> Vec<&str> {
        s.rows.iter().map(|r| s.items[r.item].label.as_str()).collect()
    }

    #[test]
    fn filters_fuzzily_and_sorts() {
        let mut s = Suggest {
            items: ["SELECT", "select_all", "SET", "DELETE", "seldom", "x"]
                .iter()
                .map(|l| CompletionItem::new(*l, CompletionKind::Keyword))
                .collect(),
            ..Default::default()
        };
        s.filter("sel");
        let l = labels(&s);
        assert_eq!(l.len(), 3);
        assert!(l[..2].contains(&"SELECT") && l[..2].contains(&"seldom"));
        assert_eq!(l[2], "select_all");
        assert_eq!(s.rows[0].matches, vec![0, 1, 2]);
        s.filter("st");
        assert!(labels(&s).contains(&"SET"));
        assert!(!labels(&s).contains(&"x"));
        s.filter("");
        assert_eq!(s.rows.len(), 6);
    }

    #[test]
    fn stepping_wraps_and_scrolls() {
        let mut s = Suggest {
            items: (0..20).map(|i| CompletionItem::new(format!("w{i:02}"), CompletionKind::Text)).collect(),
            ..Default::default()
        };
        s.filter("");
        s.step(-1);
        assert_eq!(s.selected, 19);
        assert_eq!(s.top, 8);
        s.step(1);
        assert_eq!((s.selected, s.top), (0, 0));
        s.step(15);
        assert_eq!((s.selected, s.top), (15, 4));
        s.step(100);
        assert_eq!(s.selected, 19);
    }

    #[test]
    fn collects_buffer_words() {
        let t = "foo bar_1 foo x 9lives fo";
        assert_eq!(buffer_words(t, 23..25, 100), vec!["foo", "bar_1"]);
        assert_eq!(buffer_words(t, 0..0, 1), vec!["foo"]);
    }

    #[test]
    fn frame_flips_above_near_the_bottom() {
        let mut s = Suggest {
            items: vec![CompletionItem::new("a", CompletionKind::Text); 3],
            ..Default::default()
        };
        s.filter("");
        let below = s.frame(100.0, 50.0, 19.0, 800.0, 600.0);
        assert_eq!(below.t, 69.0);
        let above = s.frame(100.0, 580.0, 19.0, 800.0, 600.0);
        assert!(above.b <= 580.0);
        assert_eq!(s.row_at(below, below.t + ROW_H * 1.5), Some(1));
    }
}
