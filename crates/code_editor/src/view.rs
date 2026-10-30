//! Editor geometry in DIPs: line metrics, scrolling, the gutter and the overlay
//! scrollbars. Font metrics come in as plain numbers, so none of this needs
//! Windows.

/// Width of the vertical scrollbar and height of the horizontal one.
pub const SCROLLBAR: f32 = 14.0;
const MIN_THUMB: f32 = 20.0;
/// Space between the line numbers and the text (Monaco's line decorations).
const GUTTER_GAP: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Region {
    Gutter,
    Text,
    VScroll,
    HScroll,
}

#[derive(Clone, Debug)]
pub struct View {
    /// Size of the editor in DIPs.
    pub width: f32,
    pub height: f32,
    pub dpi: f32,
    pub line_h: f32,
    /// Width of a space.
    pub char_w: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    /// Number of view lines.
    pub lines: usize,
    /// Widest line laid out so far (Monaco's scroll width grows the same way).
    pub content_w: f32,
    pub line_numbers: bool,
    /// Room for fold chevrons.
    pub folding: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            width: 0.0,
            height: 0.0,
            dpi: 96.0,
            line_h: 19.0,
            char_w: 8.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            lines: 1,
            content_w: 0.0,
            line_numbers: true,
            folding: false,
        }
    }
}

impl View {
    /// Width of the line number column.
    pub fn numbers_w(&self) -> f32 {
        if !self.line_numbers {
            return 0.0;
        }
        let digits = self.lines.max(1).ilog10() as usize + 1;
        (digits.max(4) as f32 * self.char_w).round() + 8.0
    }

    pub fn gutter_w(&self) -> f32 {
        let fold = if self.folding { self.char_w * 2.0 } else { 0.0 };
        (self.numbers_w() + fold + GUTTER_GAP * 0.5).round()
    }

    /// Left edge of the text at scroll position 0.
    pub fn text_x(&self) -> f32 {
        self.gutter_w() + (GUTTER_GAP * 0.5).round()
    }

    /// Width available to text.
    pub fn text_w(&self) -> f32 {
        (self.width - self.text_x() - SCROLLBAR).max(0.0)
    }

    pub fn first_line(&self) -> usize {
        ((self.scroll_y / self.line_h).floor().max(0.0) as usize).min(self.lines.saturating_sub(1))
    }

    /// View lines at least partly visible.
    pub fn visible(&self) -> std::ops::Range<usize> {
        let first = self.first_line();
        let last = ((self.scroll_y + self.height) / self.line_h).ceil() as usize;
        first..last.min(self.lines).max(first)
    }

    /// Top of view line `l` relative to the top of the editor.
    pub fn line_y(&self, l: usize) -> f32 {
        l as f32 * self.line_h - self.scroll_y
    }

    /// View line at `y` (relative to the top of the editor), clamped.
    pub fn line_at(&self, y: f32) -> usize {
        let l = ((y + self.scroll_y) / self.line_h).floor();
        (l.max(0.0) as usize).min(self.lines.saturating_sub(1))
    }

    /// Lines that fit in the viewport (for Page Up/Down).
    pub fn page_lines(&self) -> usize {
        ((self.height / self.line_h).floor() as usize).max(2) - 1
    }

    /// The last line can scroll up to the top, as with Monaco's
    /// `scrollBeyondLastLine`.
    pub fn max_scroll_y(&self) -> f32 {
        self.lines.saturating_sub(1) as f32 * self.line_h
    }

    pub fn max_scroll_x(&self) -> f32 {
        (self.content_w + self.char_w * 2.0 - self.text_w()).max(0.0)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll_y()).round();
        self.scroll_x = self.scroll_x.clamp(0.0, self.max_scroll_x()).round();
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> bool {
        let (x, y) = (self.scroll_x, self.scroll_y);
        self.scroll_x += dx;
        self.scroll_y += dy;
        self.clamp_scroll();
        (x, y) != (self.scroll_x, self.scroll_y)
    }

    /// Scrolls the least needed so view line `l` is visible, keeping `margin`
    /// lines around it; centers it when it is far away and `center_far`.
    pub fn reveal_line(&mut self, l: usize, margin: usize, center_far: bool) {
        let top = l as f32 * self.line_h;
        let m = (margin as f32 * self.line_h).min(((self.height - self.line_h) / 2.0).max(0.0));
        let far = top + self.line_h < self.scroll_y - self.height || top > self.scroll_y + 2.0 * self.height;
        if center_far && far {
            self.scroll_y = top - (self.height - self.line_h) / 2.0;
        } else if top - m < self.scroll_y {
            self.scroll_y = top - m;
        } else if top + self.line_h + m > self.scroll_y + self.height {
            self.scroll_y = top + self.line_h + m - self.height;
        }
        self.clamp_scroll();
    }

    /// Scrolls horizontally so text x `x` (relative to the text start) is visible.
    pub fn reveal_x(&mut self, x: f32) {
        let w = self.text_w();
        let margin = (self.char_w * 4.0).min(w / 3.0);
        if x - margin < self.scroll_x {
            self.scroll_x = x - margin;
        } else if x + margin > self.scroll_x + w {
            self.scroll_x = x + margin - w;
        }
        self.content_w = self.content_w.max(x);
        self.clamp_scroll();
    }

    pub fn region(&self, x: f32, y: f32) -> Region {
        if x >= self.width - SCROLLBAR && self.vthumb().is_some() {
            Region::VScroll
        } else if y >= self.height - SCROLLBAR && x >= self.text_x() && self.hthumb().is_some() {
            Region::HScroll
        } else if x < self.gutter_w() {
            Region::Gutter
        } else {
            Region::Text
        }
    }

    /// (top, height) of the vertical scrollbar thumb, when there is anything to
    /// scroll.
    pub fn vthumb(&self) -> Option<(f32, f32)> {
        let max = self.max_scroll_y();
        if max <= 0.0 || self.height <= 0.0 {
            return None;
        }
        let total = max + self.height;
        let h = (self.height * self.height / total).max(MIN_THUMB).min(self.height);
        Some((self.scroll_y / max * (self.height - h), h))
    }

    /// (left, width) of the horizontal scrollbar thumb, relative to the text start.
    pub fn hthumb(&self) -> Option<(f32, f32)> {
        let max = self.max_scroll_x();
        let w = self.text_w();
        if max <= 0.0 || w <= 0.0 {
            return None;
        }
        let tw = (w * w / (max + w)).max(MIN_THUMB).min(w);
        Some((self.scroll_x / max * (w - tw), tw))
    }

    /// The scroll position for the vertical thumb's top at `top`.
    pub fn scroll_for_vthumb(&self, top: f32) -> f32 {
        match self.vthumb() {
            Some((_, h)) if self.height > h => top / (self.height - h) * self.max_scroll_y(),
            _ => 0.0,
        }
    }

    pub fn scroll_for_hthumb(&self, left: f32) -> f32 {
        match self.hthumb() {
            Some((_, w)) if self.text_w() > w => left / (self.text_w() - w) * self.max_scroll_x(),
            _ => 0.0,
        }
    }
}

/// Maps view lines (rows on screen) to model lines and the part of each line
/// they show. It is the identity until lines are folded away or wrapped.
#[derive(Clone, Debug, Default)]
pub struct LineMap {
    lines: usize,
    /// Columns (bytes) where each model line's continuation rows start; empty
    /// when the line takes one row. Empty when wrapping is off.
    wraps: Vec<Vec<u32>>,
    /// Folded model line ranges, inclusive, sorted and disjoint.
    hidden: Vec<(usize, usize)>,
    /// First row of each model line (one more entry for the end); empty while
    /// the map is the identity.
    starts: Vec<u32>,
}

impl LineMap {
    pub fn identity(&self) -> bool {
        self.starts.is_empty()
    }

    pub fn wrapping(&self) -> bool {
        !self.wraps.is_empty()
    }

    pub fn hidden(&self) -> &[(usize, usize)] {
        &self.hidden
    }

    /// Rebuilds the map for `lines` model lines with these wraps (`None`: no
    /// wrapping) and hidden ranges.
    pub fn set(&mut self, lines: usize, wraps: Option<Vec<Vec<u32>>>, hidden: Vec<(usize, usize)>) {
        self.lines = lines;
        self.wraps = wraps.unwrap_or_default();
        self.hidden = hidden;
        self.rebuild();
    }

    /// Replaces the wraps of `removed` model lines from `first` with `new`
    /// (after an edit), keeping the hidden ranges as they are.
    pub fn splice_wraps(&mut self, first: usize, removed: usize, new: Vec<Vec<u32>>, lines: usize) {
        if self.wrapping() {
            let end = (first + removed).min(self.wraps.len());
            self.wraps.splice(first.min(end)..end, new);
        }
        self.lines = lines;
        self.rebuild();
    }

    pub fn set_hidden(&mut self, hidden: Vec<(usize, usize)>) {
        self.hidden = hidden;
        self.rebuild();
    }

    fn rebuild(&mut self) {
        if self.wraps.is_empty() && self.hidden.is_empty() {
            self.starts.clear();
            return;
        }
        let mut starts = Vec::with_capacity(self.lines + 1);
        let mut row = 0u32;
        let mut h = self.hidden.iter().peekable();
        for l in 0..self.lines {
            starts.push(row);
            while h.peek().is_some_and(|r| r.1 < l) {
                h.next();
            }
            let hidden = h.peek().is_some_and(|r| r.0 <= l && l <= r.1);
            if !hidden {
                row += 1 + self.wraps.get(l).map_or(0, |w| w.len() as u32);
            }
        }
        starts.push(row);
        self.starts = starts;
    }

    /// Number of rows.
    pub fn rows(&self, lines: usize) -> usize {
        if self.identity() { lines } else { *self.starts.last().unwrap_or(&0) as usize }
    }

    pub fn is_hidden(&self, line: usize) -> bool {
        let i = self.hidden.partition_point(|r| r.1 < line);
        self.hidden.get(i).is_some_and(|r| r.0 <= line)
    }

    /// Rows of model line `line` (0 when hidden).
    pub fn segments(&self, line: usize) -> usize {
        if self.identity() {
            return 1;
        }
        (self.starts[line + 1] - self.starts[line]) as usize
    }

    /// The row showing column `col` of model line `line` (a hidden line maps to
    /// the last row before it).
    pub fn row_of(&self, line: usize, col: usize) -> usize {
        if self.identity() {
            return line;
        }
        let line = line.min(self.lines.saturating_sub(1));
        let start = self.starts[line] as usize;
        if self.segments(line) == 0 {
            return start.saturating_sub(1);
        }
        let seg = self.wraps.get(line).map_or(0, |w| w.partition_point(|&b| b as usize <= col));
        start + seg
    }

    /// The model line and segment index a row shows.
    pub fn line_of_row(&self, row: usize) -> (usize, usize) {
        if self.identity() {
            return (row, 0);
        }
        let row = row.min(self.rows(self.lines).saturating_sub(1)) as u32;
        // The last line starting at or before the row that is not hidden.
        let l = self.starts.partition_point(|&s| s <= row).saturating_sub(1);
        let mut l = l.min(self.lines.saturating_sub(1));
        while l > 0 && self.segments(l) == 0 {
            l -= 1;
        }
        (l, (row - self.starts[l]) as usize)
    }

    /// The column range a segment of a line shows.
    pub fn segment(&self, line: usize, seg: usize, line_len: usize) -> (usize, usize) {
        let w = self.wraps.get(line).map(Vec::as_slice).unwrap_or(&[]);
        let a = if seg == 0 { 0 } else { w.get(seg - 1).map_or(line_len, |&b| b as usize) };
        let b = w.get(seg).map_or(line_len, |&b| b as usize);
        (a.min(line_len), b.min(line_len))
    }
}

/// Where a line wraps to fit `cols` visual columns: the byte column of each
/// continuation row, preferring breaks after spaces and separators.
pub fn wrap_points(line: &str, cols: usize, tab: usize) -> Vec<u32> {
    use unicode_segmentation::UnicodeSegmentation;
    let cols = cols.max(4);
    let mut out = Vec::new();
    let (mut v, mut seg_start, mut last_break) = (0usize, 0usize, None::<usize>);
    for (i, g) in line.grapheme_indices(true) {
        let w = if g == "\t" { tab - v % tab } else { 1 };
        if v + w > cols && i > seg_start {
            let at = match last_break {
                Some(b) if b > seg_start => b,
                _ => i,
            };
            out.push(at as u32);
            seg_start = at;
            // Restart counting from the break.
            v = crate::movement::visual_col(&line[at..], i - at, tab);
            last_break = None;
        }
        v += if g == "\t" { tab - v % tab } else { 1 };
        let c = g.chars().next().unwrap_or(' ');
        if c.is_whitespace() || (crate::movement::WORD_SEPARATORS.contains(c) && c != '_') {
            last_break = Some(i + g.len());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(lines: usize) -> View {
        View {
            width: 800.0,
            height: 190.0,
            line_h: 19.0,
            char_w: 8.0,
            lines,
            ..Default::default()
        }
    }

    #[test]
    fn visible_lines_and_hits() {
        let mut v = view(100);
        assert_eq!(v.visible(), 0..10);
        v.scroll_y = 19.0 * 5.0 + 3.0;
        assert_eq!(v.visible(), 5..16);
        assert_eq!(v.line_at(0.0), 5);
        assert_eq!(v.line_at(-1000.0), 0);
        assert_eq!(v.line_at(1e6), 99);
        assert_eq!(v.line_y(6), 19.0 - 3.0);
        assert_eq!(view(3).visible(), 0..3);
    }

    #[test]
    fn scrolling_is_clamped() {
        let mut v = view(100);
        v.scroll_by(0.0, -50.0);
        assert_eq!(v.scroll_y, 0.0);
        v.scroll_by(0.0, 1e9);
        assert_eq!(v.scroll_y, 99.0 * 19.0);
        assert!(!v.scroll_by(5.0, 0.0));
    }

    #[test]
    fn reveal_scrolls_minimally() {
        let mut v = view(100);
        v.reveal_line(12, 0, false);
        assert_eq!(v.scroll_y, 13.0 * 19.0 - 190.0);
        v.reveal_line(2, 0, false);
        assert_eq!(v.scroll_y, 38.0);
        v.reveal_line(90, 0, true);
        assert!((v.line_y(90) - (190.0 - 19.0) / 2.0).abs() < 1.0);
    }

    #[test]
    fn gutter_grows_with_digits() {
        let small = view(10).gutter_w();
        assert_eq!(small, view(9999).gutter_w());
        assert!(view(100_000).gutter_w() > small);
        let mut v = view(10);
        v.line_numbers = false;
        assert!(v.gutter_w() < small);
    }

    #[test]
    fn thumbs_and_regions() {
        let v = view(5);
        assert!(v.vthumb().is_some());
        let mut v = view(1);
        assert!(v.vthumb().is_none());
        assert!(v.hthumb().is_none());
        v.content_w = 5000.0;
        let (_, w) = v.hthumb().unwrap();
        assert!(w < v.text_w());
        assert_eq!(v.region(5.0, 50.0), Region::Gutter);
        assert_eq!(v.region(400.0, 50.0), Region::Text);
        assert_eq!(v.region(400.0, 185.0), Region::HScroll);
        let v = view(100);
        assert_eq!(v.region(795.0, 50.0), Region::VScroll);
        let (top, h) = v.vthumb().unwrap();
        assert_eq!(top, 0.0);
        assert!((v.scroll_for_vthumb(190.0 - h) - v.max_scroll_y()).abs() < 0.01);
    }
    #[test]
    fn line_map_identity_folds_and_wraps() {
        let mut m = LineMap::default();
        assert!(m.identity());
        assert_eq!((m.rows(10), m.row_of(3, 5), m.line_of_row(4)), (10, 3, (4, 0)));
        // Lines 2..=4 folded under line 1.
        m.set(10, None, vec![(2, 4)]);
        assert_eq!(m.rows(10), 7);
        assert_eq!(m.line_of_row(2), (5, 0));
        assert_eq!(m.row_of(5, 0), 2);
        assert_eq!(m.row_of(3, 0), 1);
        assert!(m.is_hidden(4) && !m.is_hidden(5));
        // Line 0 wraps at columns 10 and 20.
        let mut wraps = vec![Vec::new(); 10];
        wraps[0] = vec![10, 20];
        m.set(10, Some(wraps), vec![(2, 4)]);
        assert_eq!(m.rows(10), 9);
        assert_eq!(m.line_of_row(1), (0, 1));
        assert_eq!(m.line_of_row(3), (1, 0));
        assert_eq!(m.row_of(0, 15), 1);
        assert_eq!(m.row_of(0, 20), 2);
        assert_eq!(m.segment(0, 1, 25), (10, 20));
        assert_eq!(m.segment(0, 2, 25), (20, 25));
        assert_eq!(m.segments(0), 3);
        // An edit that adds a line after line 0.
        m.splice_wraps(1, 0, vec![vec![3]], 11);
        assert_eq!(m.rows(11), 11);
        assert_eq!(m.line_of_row(4), (1, 1));
    }

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(wrap_points("aaaa bbbb cccc", 9, 4), vec![5]);
        assert_eq!(wrap_points("aaaa bbbb cccc", 8, 4), vec![5, 10]);
        assert_eq!(wrap_points("abcdefghijkl", 5, 4), vec![5, 10]);
        assert_eq!(wrap_points("short", 20, 4), Vec::<u32>::new());
        assert_eq!(wrap_points("foo(bar, baz)", 6, 4), vec![4, 9]);
    }
}
