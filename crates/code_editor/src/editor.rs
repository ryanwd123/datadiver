//! The editor's state and behavior: document, view, options and language;
//! actions, typing and mouse handling. It does not own a window, so a child
//! window drives it, or a host that paints it into its own render target.

use std::cell::RefCell;
use std::rc::Rc;

use windows::Win32::Foundation::HWND;
use windows::core::Result;

use crate::commands::{self, IndentOptions, Outcome};
use crate::doc::Document;
use crate::history::EditKind;
use crate::keymap::{Action, Binding, Chord, Keymap, Motion};
use crate::lang::{self, Lang};
use crate::movement;
use crate::platform;
use crate::render::{LineLayout, Renderer};
use crate::selection::{Selection, Selections};
use crate::indent;
use crate::search;
use crate::widgets::find::{self, Find};
use crate::widgets::goto::{self, GoTo};
use crate::widgets::input::{Edited, Input};
use crate::widgets::suggest::{self, CompletionContext, CompletionItem, CompletionKind, CompletionProvider, Suggest};
use crate::widgets::{Focus, Rect};
use crate::syntax::{self, Context, HighlightCache, Span, Syntax};
use crate::text::{self, Buffer, Change};
use crate::decorations::Decorations;
use crate::theme::{Paint, Palette, Theme};
use crate::view::{LineMap, Region, View};
use crate::{ContentChange, Event, Options};

thread_local! {
    /// Text the last whole-line copy put on the clipboard, so pasting it inserts
    /// whole lines (in any editor on the thread).
    static LINE_COPY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Posted to the editor window when a background parse finishes: wparam is the
/// parse generation, lparam a boxed `tree_sitter::Tree`.
pub(crate) const WM_APP_PARSED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x102;

/// Above this size a reparse waits for a pause in typing instead of running
/// on every keystroke (an incremental reparse of a 100k-line file takes ~17 ms);
/// meanwhile the edited tree colors the text.
const SYNC_PARSE_BYTES: usize = 512 * 1024;

const MIN_ZOOM: f32 = 0.5;
const MAX_ZOOM: f32 = 3.0;

/// What the window should do after the state changed.
#[derive(Default)]
pub(crate) struct Effects {
    pub invalidate: bool,
    /// Show the caret and restart its blink timer.
    pub reset_blink: bool,
    /// Move the IME composition window.
    pub ime: bool,
    /// Reparse soon (a big file's reparse waits for a pause in typing).
    pub reparse: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Unit {
    Char,
    Word,
    Line,
}

pub(crate) enum Drag {
    /// Selecting text; `anchor` is the word or line first clicked.
    Text { unit: Unit, anchor: (usize, usize) },
    Gutter { anchor: (usize, usize) },
    /// Shift+Alt+drag: a column selection from (line, visual column).
    Column { line: usize, vcol: usize },
    VThumb { grab: f32 },
    HThumb { grab: f32 },
}

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Context menu items.
pub(crate) const MENU_CUT: u32 = 1;
pub(crate) const MENU_COPY: u32 = 2;
pub(crate) const MENU_PASTE: u32 = 3;
pub(crate) const MENU_SELECT_ALL: u32 = 4;

/// The indent guides of one line: `levels` guides, one of which may be active.
pub(crate) struct Guide {
    pub line: usize,
    pub levels: usize,
    pub active: Option<usize>,
}

pub(crate) enum KeyResult {
    Handled,
    Unhandled,
    ContextMenu,
}

pub(crate) struct State {
    /// The window that receives input and owns the clipboard.
    pub hwnd: HWND,
    /// The editor's own window (message-only when hosted), for posted results.
    pub own_hwnd: HWND,
    pub doc: Document,
    pub opts: Options,
    pub view: View,
    pub render: Renderer,
    pub theme: Theme,
    pub lang: Option<&'static Lang>,
    pub syntax: Option<Syntax>,
    /// A language whose `Syntax` is made on first use (its query compiles on
    /// a worker thread meanwhile).
    syntax_pending: Option<&'static Lang>,
    /// Highlight spans of the lines last painted.
    pub hl: HighlightCache,
    /// Identifies the current background parse, so a stale result is dropped.
    parse_gen: usize,
    pub keymap: Keymap,
    pub focused: bool,
    pub caret_on: bool,
    pub zoom: f32,
    pub events: Vec<Event>,
    pub fx: Effects,
    pub drag: Option<Drag>,
    /// Last mouse position in DIPs.
    pub mouse: (f32, f32),
    pub hover: Option<Region>,
    high_surrogate: Option<u16>,
    notified_sels: Selections,
    /// Closers the editor inserted while typing, as (open, close) offsets; they
    /// can be typed over or deleted with their opener.
    pub auto_closed: Vec<(usize, usize)>,
    /// The whole text as one string, by version (for searching).
    snapshot: Option<(u64, Rc<str>)>,
    /// Ctrl+D started from an empty caret, so it matches whole words.
    whole_word_next: bool,
    /// Selections before each smart-select expansion, for shrinking.
    select_stack: Vec<Selections>,
    pub find: Find,
    pub goto: Option<GoTo>,
    pub decorations: Decorations,
    /// Rows on screen for model lines: folds and word wrap.
    pub rows: LineMap,
    /// Folded regions as (header line start, last folded line start) offsets.
    pub folds: Vec<(usize, usize)>,
    /// The mouse is over the gutter (fold chevrons show).
    pub gutter_hover: bool,
    pub suggest: Suggest,
    pub provider: Option<Rc<dyn CompletionProvider>>,
    /// Start of a word that had no completions: longer prefixes of it cannot
    /// match either, so typing on in it does not ask again.
    suggest_dead: Option<usize>,
    /// A hover message shown at (x, y).
    pub tooltip: Option<(f32, f32, String)>,
    /// Where typing goes: the text or one of the overlay inputs.
    pub focus: Focus,
}

impl State {
    pub fn new(hwnd: HWND, opts: Options, theme: Theme) -> Result<Self> {
        let mut view = View {
            line_numbers: opts.line_numbers,
            ..Default::default()
        };
        view.folding = opts.folding;
        Ok(State {
            hwnd,
            own_hwnd: hwnd,
            doc: Document::default(),
            render: Renderer::new(Palette::new(theme))?,
            theme,
            opts,
            view,
            lang: None,
            syntax: None,
            syntax_pending: None,
            hl: HighlightCache::default(),
            parse_gen: 0,
            keymap: Keymap::default(),
            focused: false,
            caret_on: false,
            zoom: 1.0,
            events: Vec::new(),
            fx: Effects::default(),
            drag: None,
            mouse: (0.0, 0.0),
            hover: None,
            high_surrogate: None,
            notified_sels: Selections::default(),
            auto_closed: Vec::new(),
            snapshot: None,
            whole_word_next: false,
            select_stack: Vec::new(),
            find: Find::default(),
            goto: None,
            decorations: Decorations::default(),
            rows: LineMap::default(),
            folds: Vec::new(),
            gutter_hover: false,
            suggest: Suggest::default(),
            provider: None,
            suggest_dead: None,
            tooltip: None,
            focus: Focus::Text,
        })
    }

    pub fn invalidate(&mut self) {
        self.fx.invalidate = true;
    }

    fn indent_opts(&self) -> IndentOptions {
        IndentOptions {
            tab_size: self.opts.tab_size.max(1),
            insert_spaces: self.opts.insert_spaces,
        }
    }

    // ---- Fonts, size, DPI ----

    /// Builds fonts when they are missing (first paint, or after a font, zoom,
    /// tab size or DPI change).
    pub fn ensure_fonts(&mut self) {
        if self.render.fonts.is_some() {
            return;
        }
        let size = self.opts.font_size * self.zoom;
        if let Ok(f) = self.render.set_font(
            self.opts.font_family.as_deref(),
            size,
            self.opts.tab_size.max(1),
            self.view.dpi,
        ) {
            let line = self.view.first_line();
            self.view.line_h = f.line_h;
            self.view.char_w = f.char_w;
            self.view.content_w = 0.0;
            self.view.scroll_y = line as f32 * f.line_h;
            if self.opts.word_wrap {
                self.sync_rows();
            }
            self.view.clamp_scroll();
        }
    }

    pub fn reset_fonts(&mut self) {
        self.render.fonts = None;
        self.ensure_fonts();
        self.invalidate();
    }

    /// The window was resized to `w` x `h` device pixels.
    pub fn resize(&mut self, w: u32, h: u32) {
        let k = self.view.dpi / 96.0;
        let width = w as f32 / k;
        let rewrap = self.opts.word_wrap && (width - self.view.width).abs() > 0.5;
        self.view.width = width;
        self.view.height = h as f32 / k;
        if rewrap && self.render.fonts.is_some() {
            self.sync_rows();
        }
        self.view.clamp_scroll();
        self.invalidate();
    }

    pub fn set_dpi(&mut self, dpi: f32) {
        if dpi != self.view.dpi {
            let (w, h) = (self.view.width * self.view.dpi / 96.0, self.view.height * self.view.dpi / 96.0);
            self.view.dpi = dpi;
            self.resize(w.round() as u32, h.round() as u32);
            self.reset_fonts();
        }
    }

    pub fn set_zoom(&mut self, z: f32) {
        let z = z.clamp(MIN_ZOOM, MAX_ZOOM);
        if (z - self.zoom).abs() > 0.001 {
            self.zoom = z;
            self.reset_fonts();
            self.reveal(false);
        }
    }

    // ---- Layout queries ----

    /// Makes the pending `Syntax`, if any.
    fn ensure_syntax(&mut self) {
        let Some(lang) = self.syntax_pending.take() else { return };
        match Syntax::new(lang) {
            Ok(s) => self.syntax = Some(s),
            Err(e) => self.events.push(Event::Warning(format!("Highlighting is off: {e}"))),
        }
    }

    /// Parses if needed and computes highlight spans for `lines` (model lines).
    pub fn ensure_highlight(&mut self, lines: std::ops::Range<usize>) {
        self.ensure_syntax();
        let version = self.doc.buffer.version;
        let Some(syn) = &mut self.syntax else {
            self.hl = HighlightCache::default();
            return;
        };
        if self.hl.covers(version, &lines) {
            return;
        }
        let rope = self.doc.buffer.rope();
        if rope.len_bytes() <= SYNC_PARSE_BYTES {
            syn.parse(rope);
        } else if syn.is_dirty() {
            self.fx.reparse = true;
        }
        let b = &self.doc.buffer;
        let starts: Vec<usize> = (lines.start..=lines.end).map(|l| b.line_start(l)).collect();
        self.hl = HighlightCache {
            version,
            first: lines.start,
            lines: syn.highlight(b.rope(), &starts),
        };
    }

    /// Colored ranges of model line `line`.
    pub fn line_spans(&mut self, line: usize) -> Vec<Span> {
        self.ensure_syntax();
        let version = self.doc.buffer.version;
        if let Some(s) = self.hl.get(version, line) {
            return s.to_vec();
        }
        self.parse_now();
        let Some(syn) = &self.syntax else {
            return Vec::new();
        };
        let b = &self.doc.buffer;
        syn.highlight(b.rope(), &[b.line_start(line), b.line_start(line + 1)])
            .pop()
            .unwrap_or_default()
    }

    /// Text colors of a line: syntax colors with decoration colors over them.
    pub fn line_paints(&mut self, line: usize) -> Vec<(usize, usize, Paint)> {
        let spans = self.line_spans(line);
        let b = &self.doc.buffer;
        let (ls, le) = (b.line_start(line), b.line_end(line));
        let fg: Vec<(usize, usize, u32)> = self
            .decorations
            .in_range(ls, le)
            .filter_map(|t| Some((t.start.max(ls) - ls, t.end.min(le) - ls, t.options.foreground?)))
            .filter(|f| f.0 < f.1)
            .collect();
        if fg.is_empty() {
            return spans.into_iter().map(|(a, e, ink)| (a, e, Paint::Ink(ink))).collect();
        }
        let len = (le - ls).min(crate::render::MAX_RENDER_BYTES);
        let mut by: Vec<Option<Paint>> = vec![None; len];
        for (a, e, ink) in spans {
            by[a.min(len)..e.min(len)].fill(Some(Paint::Ink(ink)));
        }
        for (a, e, c) in fg {
            by[a.min(len)..e.min(len)].fill(Some(Paint::Argb(c)));
        }
        let mut out = Vec::new();
        let mut i = 0;
        while i < len {
            let mut j = i + 1;
            while j < len && by[j] == by[i] {
                j += 1;
            }
            if let Some(p) = by[i] {
                out.push((i, j, p));
            }
            i = j;
        }
        out
    }

    pub fn layout_of(&mut self, line: usize) -> Rc<LineLayout> {
        self.ensure_fonts();
        let spans = self.line_paints(line);
        let text = self.doc.buffer.line(line);
        let l = self.render.layout(&text, &spans);
        if l.width > self.view.content_w {
            self.view.content_w = l.width;
        }
        l
    }

    /// The layout of columns `a..b` of a line (one row of a wrapped line).
    pub fn seg_layout(&mut self, line: usize, a: usize, b: usize) -> Rc<LineLayout> {
        let len = self.doc.buffer.line_len(line);
        if a == 0 && b >= len {
            return self.layout_of(line);
        }
        self.ensure_fonts();
        let spans: Vec<(usize, usize, Paint)> = self
            .line_paints(line)
            .into_iter()
            .filter(|s| s.1 > a && s.0 < b)
            .map(|(s, e, p)| (s.max(a) - a, e.min(b) - a, p))
            .collect();
        let text = self.doc.buffer.line(line);
        self.render.layout(&text[a..b], &spans)
    }

    /// The row showing offset `off`.
    pub fn row_of(&self, off: usize) -> usize {
        let p = self.doc.buffer.pos(off);
        self.rows.row_of(p.line, p.col)
    }

    /// The model line and column range a row shows.
    pub fn row_span(&self, row: usize) -> (usize, usize, usize) {
        let (line, seg) = self.rows.line_of_row(row);
        let (a, b) = self.rows.segment(line, seg, self.doc.buffer.line_len(line));
        (line, a, b)
    }

    /// x of offset `off` relative to the start of the text (scroll not applied).
    pub fn x_of(&mut self, off: usize) -> f32 {
        let p = self.doc.buffer.pos(off);
        let (_, a, b) = self.row_span(self.rows.row_of(p.line, p.col));
        let l = self.seg_layout(p.line, a, b);
        l.x_of(p.col - a.min(p.col), self.view.char_w)
    }

    /// The offset at editor point (x, y) in DIPs.
    pub fn hit(&mut self, x: f32, y: f32) -> usize {
        let (line, a, b) = self.row_span(self.view.line_at(y));
        let l = self.seg_layout(line, a, b);
        let tx = x - self.view.text_x() + self.view.scroll_x;
        let col = if tx > l.width {
            // Past a truncated line's laid-out text, count spaces.
            let extra = ((tx - l.width) / self.view.char_w).round() as usize;
            a + (l.text.len() + extra).min(b - a)
        } else {
            a + l.col_at(tx.max(0.0))
        };
        self.doc.buffer.offset(text::Pos::new(line, col.min(b)))
    }

    /// Caret position in the hosting window's client DIPs: (x, y, height).
    pub fn caret_point(&mut self) -> (f32, f32, f32) {
        let head = self.doc.sels.primary().head;
        let x = self.x_of(head) + self.view.text_x() - self.view.scroll_x;
        (x, self.view.line_y(self.row_of(head)), self.view.line_h)
    }

    // ---- Folding and word wrap ----

    /// Rebuilds the rows from scratch: wrap points for every line (when
    /// wrapping) and the folded lines.
    pub fn sync_rows(&mut self) {
        let n = self.doc.buffer.line_count();
        let wraps = self.opts.word_wrap.then(|| self.wraps_for(0..n));
        let hidden = self.hidden_lines();
        self.rows.set(n, wraps, hidden);
        self.view.lines = self.rows.rows(n);
        self.view.clamp_scroll();
        self.invalidate();
    }

    /// Columns that fit in the text area, for wrapping.
    fn wrap_cols(&self) -> usize {
        ((self.view.text_w() - self.view.char_w) / self.view.char_w).floor().max(10.0) as usize
    }

    fn wraps_for(&self, lines: std::ops::Range<usize>) -> Vec<Vec<u32>> {
        let (cols, tab) = (self.wrap_cols(), self.opts.tab_size.max(1));
        let b = &self.doc.buffer;
        lines.map(|l| crate::view::wrap_points(&b.line(l), cols, tab)).collect()
    }

    /// Model lines hidden by folds, merged.
    fn hidden_lines(&self) -> Vec<(usize, usize)> {
        let b = &self.doc.buffer;
        let mut v: Vec<(usize, usize)> = self
            .folds
            .iter()
            .map(|&(h, e)| (b.line_of(h) + 1, b.line_of(e)))
            .filter(|r| r.1 >= r.0)
            .collect();
        v.sort_unstable();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for r in v {
            match out.last_mut() {
                Some(l) if r.0 <= l.1 + 1 => l.1 = l.1.max(r.1),
                _ => out.push(r),
            }
        }
        out
    }

    /// Updates rows after an edit: wrap points of the changed lines, folds
    /// moved with the text (dropped when the edit reached their hidden lines).
    fn rows_after_edit(&mut self, changes: &[Change]) {
        let b = &self.doc.buffer;
        let old_hidden = self.rows.hidden().to_vec();
        self.folds = self
            .folds
            .iter()
            .filter_map(|&(h, e)| {
                let hl = map_offset(h, changes)?;
                let el = map_offset(e, changes)?;
                Some((hl, el))
            })
            .filter(|&(h, e)| {
                // Drop folds whose hidden lines an edit touched.
                let (hl, el) = (b.line_of(h), b.line_of(e));
                el > hl && !changes.iter().any(|c| {
                    let l = b.line_of(c.start.min(b.len()));
                    l > hl && l <= el
                })
            })
            .collect();
        let n = b.line_count();
        let hidden = self.hidden_lines();
        if self.opts.word_wrap && changes.len() == 1 {
            let c = &changes[0];
            let first = c.start_pos.line;
            let removed = c.old_end_pos.line - first + 1;
            let added = c.new_end_pos.line - first + 1;
            let wraps = self.wraps_for(first..first + added);
            self.rows.splice_wraps(first, removed, wraps, n);
            if hidden != old_hidden {
                self.rows.set_hidden(hidden);
            }
            self.view.lines = self.rows.rows(n);
        } else if self.opts.word_wrap || !self.folds.is_empty() || !old_hidden.is_empty() {
            self.sync_rows();
        } else {
            self.view.lines = n;
        }
    }

    /// Fold regions with headers in `first..=last`: from the grammar, or by
    /// indentation without one.
    pub fn fold_regions(&mut self, first: usize, last: usize) -> Vec<(usize, usize)> {
        self.parse_now();
        let b = &self.doc.buffer;
        match &self.syntax {
            Some(s) if s.is_current() => s.fold_ranges(b.rope(), first, last),
            _ => crate::indent::fold_ranges(b, first, last, self.opts.tab_size.max(1)),
        }
    }

    pub fn is_folded(&self, header: usize) -> bool {
        let b = &self.doc.buffer;
        self.folds.iter().any(|&(h, _)| b.line_of(h) == header)
    }

    /// Folds `region` (header line, last line); carets inside move to the
    /// header's end.
    pub fn fold(&mut self, region: (usize, usize)) {
        let b = &self.doc.buffer;
        if self.is_folded(region.0) {
            return;
        }
        self.folds.push((b.line_start(region.0), b.line_start(region.1)));
        let end = b.line_end(region.0);
        let sels = self.doc.sels.map(|s| {
            let l = b.line_of(s.head);
            if l > region.0 && l <= region.1 { Selection::cursor(end) } else { *s }
        });
        self.doc.set_selections(sels);
        self.sync_rows();
        self.reveal(false);
    }

    /// Unfolds folds whose header is `line` or that hide `line`.
    pub fn unfold_at(&mut self, line: usize) -> bool {
        let b = &self.doc.buffer;
        let n = self.folds.len();
        self.folds.retain(|&(h, e)| {
            let (hl, el) = (b.line_of(h), b.line_of(e));
            !(hl == line || (line > hl && line <= el))
        });
        let changed = self.folds.len() != n;
        if changed {
            self.sync_rows();
        }
        changed
    }

    /// Unfolds whatever hides a caret (after a jump into folded lines).
    fn unfold_carets(&mut self) {
        if self.folds.is_empty() {
            return;
        }
        let lines: Vec<usize> = self.doc.sels.iter().map(|s| self.doc.buffer.line_of(s.head)).collect();
        for l in lines {
            if self.rows.is_hidden(l) {
                self.unfold_at(l);
            }
        }
    }

    /// Ctrl+Shift+[: folds the innermost region holding the caret.
    fn fold_at_caret(&mut self) {
        let line = self.doc.buffer.line_of(self.doc.sels.primary().head);
        let regions = self.fold_regions(line.saturating_sub(2000), line);
        let region = regions
            .iter()
            .rev()
            .find(|r| r.0 <= line && line <= r.1 && !self.is_folded(r.0))
            .copied();
        if let Some(r) = region {
            self.fold(r);
        }
    }

    /// The fold chevron toggled for `line`.
    pub fn toggle_fold(&mut self, line: usize) {
        if !self.unfold_at(line)
            && let Some(r) = self.fold_regions(line, line).first().copied()
        {
            self.fold(r);
        }
    }

    pub fn fold_all(&mut self) {
        let n = self.doc.buffer.line_count();
        let regions = self.fold_regions(0, n);
        let b = &self.doc.buffer;
        self.folds = regions.iter().map(|&(h, e)| (b.line_start(h), b.line_start(e))).collect();
        let sels = self.doc.sels.map(|s| {
            let l = b.line_of(s.head);
            match regions.iter().find(|r| l > r.0 && l <= r.1) {
                Some(r) => Selection::cursor(b.line_end(r.0)),
                None => *s,
            }
        });
        self.doc.set_selections(sels);
        self.sync_rows();
        // Nested folds inside an outer one keep only the outer.
        self.reveal(false);
    }

    pub fn unfold_all(&mut self) {
        if !self.folds.is_empty() {
            self.folds.clear();
            self.sync_rows();
            self.reveal(false);
        }
    }

    pub fn toggle_wrap(&mut self) {
        self.opts.word_wrap = !self.opts.word_wrap;
        self.view.scroll_x = 0.0;
        self.view.content_w = 0.0;
        self.sync_rows();
        self.reveal(false);
    }

    // ---- Selections and scrolling ----

    /// Scrolls so the primary caret is visible.
    pub fn reveal(&mut self, center_far: bool) {
        if self.view.height <= 0.0 {
            return;
        }
        self.ensure_fonts();
        let head = self.doc.sels.primary().head;
        let row = self.row_of(head);
        self.view.lines = self.rows.rows(self.doc.buffer.line_count());
        self.view.reveal_line(row, 0, center_far);
        let x = self.x_of(head);
        self.view.reveal_x(x);
        self.invalidate();
    }

    fn caret_moved(&mut self) {
        self.fx.reset_blink = true;
        self.fx.ime = true;
        self.caret_on = true;
        self.invalidate();
    }

    pub fn set_sels(&mut self, sels: Selections) {
        self.select_stack.clear();
        self.close_suggest();
        self.doc.set_selections(sels);
        self.unfold_carets();
        self.prune_auto_closed();
        self.reveal(false);
        self.caret_moved();
    }

    /// Forgets auto-inserted closers no caret is in front of any more.
    fn prune_auto_closed(&mut self) {
        let b = &self.doc.buffer;
        let sels = &self.doc.sels;
        self.auto_closed.retain(|&(o, c)| {
            c < b.len() && b.line_of(o) == b.line_of(c) && sels.iter().any(|s| s.head > o && s.head <= c)
        });
    }

    /// Queues change notifications for the host.
    pub fn notify(&mut self) {
        if self.doc.sels != self.notified_sels {
            self.notified_sels = self.doc.sels.clone();
            self.events.push(Event::SelectionChanged);
        }
    }

    // ---- Editing ----

    /// Applies a command's outcome (ignored when read-only).
    pub fn edit(&mut self, o: Outcome, kind: EditKind) {
        if self.opts.read_only {
            return;
        }
        let changes = self.doc.edit(o, kind);
        self.after_edit(changes);
    }

    pub fn after_edit(&mut self, changes: Vec<Change>) {
        self.select_stack.clear();
        self.whole_word_next = false;
        if let Some(syn) = &mut self.syntax {
            for c in &changes {
                syn.edit(c);
            }
        }
        self.decorations.apply(&changes);
        self.tooltip = None;
        self.auto_closed = self
            .auto_closed
            .iter()
            .filter_map(|&(o, c)| Some((map_offset(o, &changes)?, map_offset(c, &changes)?)))
            .collect();
        self.prune_auto_closed();
        if !changes.is_empty() {
            self.rows_after_edit(&changes);
            self.unfold_carets();
            self.events.push(Event::ContentChanged {
                version: self.doc.buffer.version,
                changes: changes
                    .into_iter()
                    .map(|c| ContentChange {
                        start: c.start_pos,
                        end: c.old_end_pos,
                        text: c.text,
                    })
                    .collect(),
            });
        }
        if self.suggest.open {
            self.update_suggest();
        }
        self.reveal(false);
        self.caret_moved();
    }

    pub fn type_text(&mut self, s: &str) {
        let o = commands::insert(&self.doc.sels, s);
        self.edit(o, EditKind::Typing);
    }

    /// Brings the syntax tree up to date with the text.
    pub fn parse_now(&mut self) {
        self.ensure_syntax();
        if let Some(syn) = &mut self.syntax {
            let rope = self.doc.buffer.rope();
            if rope.len_bytes() <= SYNC_PARSE_BYTES {
                syn.parse(rope);
            } else if syn.is_dirty() {
                self.fx.reparse = true;
            }
        }
    }

    /// The deferred reparse of a big file (typing paused).
    pub fn parse_deferred(&mut self) {
        if let Some(syn) = &mut self.syntax
            && syn.is_dirty()
        {
            syn.parse(self.doc.buffer.rope());
            self.hl = HighlightCache::default();
            self.invalidate();
        }
    }

    pub fn context_at(&self, off: usize) -> Context {
        self.syntax.as_ref().map_or(Context::Code, |s| s.context_at(off))
    }

    /// Types one character with auto-closing pairs, surround and type-over.
    pub fn type_char(&mut self, c: char) {
        let pairs: &[(&str, &str)] = match self.lang {
            Some(l) if self.opts.auto_closing => l.auto_close,
            _ => &[],
        };
        if pairs.is_empty() {
            return self.type_text(c.encode_utf8(&mut [0; 4]));
        }
        self.parse_now();
        let in_text = |off: usize| self.context_at(off) != Context::Code;
        let cx = commands::TypeContext {
            pairs,
            in_string_or_comment: &in_text,
            auto_closed: &self.auto_closed,
        };
        let (o, new_pairs) = commands::type_char(&self.doc.buffer, &self.doc.sels, c, &cx);
        if self.opts.read_only {
            return;
        }
        self.edit(o, EditKind::Typing);
        self.auto_closed.extend(new_pairs);
        self.typed_for_suggest(c);
    }

    // ---- Completion ----

    /// Where the word before `off` starts.
    fn word_start(&self, off: usize) -> usize {
        let b = &self.doc.buffer;
        let p = b.pos(off);
        let line = b.line(p.line);
        let before = &line[..p.col.min(line.len())];
        let n: usize = before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum();
        off - n
    }

    /// After a character was typed: refilters an open popup, or opens one for
    /// a word character or a trigger character (in code, not strings).
    fn typed_for_suggest(&mut self, c: char) {
        if self.suggest.open {
            return self.update_suggest();
        }
        if !self.opts.quick_suggestions || self.doc.sels.len() > 1 {
            return;
        }
        let head = self.doc.sels.primary().head;
        let triggers = self.provider.as_ref().map(|p| p.trigger_characters()).unwrap_or_default();
        if triggers.contains(&c) {
            return self.open_suggest(Some(c), false);
        }
        if (c.is_alphanumeric() || c == '_')
            && self.suggest_dead != Some(self.word_start(head))
            && self.context_at(head) == Context::Code
        {
            self.open_suggest(None, false);
            if !self.suggest.open {
                self.suggest_dead = Some(self.word_start(head));
            }
        }
    }

    /// Opens the popup at the primary caret; `explicit` (Ctrl+Space) also
    /// opens it with nothing typed.
    pub fn open_suggest(&mut self, trigger: Option<char>, explicit: bool) {
        /// Words are collected this far around the caret, so big files stay fast.
        const WORD_WINDOW: usize = 50_000;
        let head = self.doc.sels.primary().head;
        let start = self.word_start(head);
        let prefix = self.doc.buffer.slice(start, head);
        if prefix.is_empty() && trigger.is_none() && !explicit {
            return;
        }
        let mut items: Vec<CompletionItem> = Vec::new();
        if let Some(p) = self.provider.clone() {
            let text = self.snapshot();
            let cx = CompletionContext {
                text: &text,
                offset: head,
                pos: self.doc.buffer.pos(head),
                prefix: &prefix,
                trigger,
                language: self.lang.map(|l| l.id),
            };
            items = p.complete(&cx);
        }
        self.suggest_dead = None;
        if trigger.is_none() {
            if let Some(l) = self.lang {
                items.extend(l.keywords.iter().map(|k| CompletionItem::new(*k, CompletionKind::Keyword)));
            }
            let b = &self.doc.buffer;
            let from = b.floor_char(start.saturating_sub(WORD_WINDOW));
            let to = b.floor_char(head + WORD_WINDOW);
            let near = b.slice(from, to);
            let words = suggest::buffer_words(&near, start - from..head - from, 5_000);
            items.extend(words.into_iter().map(|w| CompletionItem::new(w, CompletionKind::Text)));
        }
        let mut seen = std::collections::HashSet::new();
        items.retain(|it| seen.insert(it.label.clone()));
        self.suggest.items = items;
        self.suggest.word_start = start;
        self.suggest.open = true;
        self.update_suggest();
    }

    /// Refilters by the word typed so far; closes when the caret left it or
    /// nothing matches.
    fn update_suggest(&mut self) {
        let head = self.doc.sels.primary().head;
        let start = self.suggest.word_start;
        if head < start || self.word_start(head) != start || self.doc.buffer.line_of(head) != self.doc.buffer.line_of(start) {
            return self.close_suggest();
        }
        let prefix = self.doc.buffer.slice(start, head);
        self.suggest.filter(&prefix);
        if self.suggest.rows.is_empty() {
            return self.close_suggest();
        }
        self.invalidate();
    }

    pub fn close_suggest(&mut self) {
        if self.suggest.open {
            self.suggest = Suggest::default();
            self.invalidate();
        }
    }

    /// Inserts the selected item in place of the typed word, at every caret.
    pub fn accept_suggest(&mut self) {
        let Some(item) = self.suggest.selected_item().cloned() else {
            return self.close_suggest();
        };
        let head = self.doc.sels.primary().head;
        let n = head - self.suggest.word_start;
        let prefix = self.doc.buffer.slice(self.suggest.word_start, head);
        let insert = item.insert_text.unwrap_or(item.label);
        self.close_suggest();
        let b = &self.doc.buffer;
        let o = commands::per_selection(&self.doc.sels, |s| {
            let h = s.head;
            let start = if h >= n && b.slice(h - n, h) == prefix { h - n } else { h };
            commands::Replace::caret_after(start.min(s.start()), s.end(), insert.clone())
        });
        self.edit(o, EditKind::Other);
    }

    /// Keys while the popup is open. `None` lets the key through.
    fn suggest_key(&mut self, c: Chord) -> Option<KeyResult> {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        if c.ctrl || c.alt {
            return None;
        }
        match VIRTUAL_KEY(c.key) {
            VK_UP if !c.shift => self.suggest.step(-1),
            VK_DOWN if !c.shift => self.suggest.step(1),
            VK_PRIOR if !c.shift => self.suggest.step(-(suggest::MAX_ROWS as isize)),
            VK_NEXT if !c.shift => self.suggest.step(suggest::MAX_ROWS as isize),
            VK_RETURN | VK_TAB if !c.shift => self.accept_suggest(),
            VK_ESCAPE => self.close_suggest(),
            _ => return None,
        }
        self.invalidate();
        Some(KeyResult::Handled)
    }

    /// The popup's rectangle, at the start of the word being completed.
    pub fn suggest_frame(&mut self) -> Option<Rect> {
        if !self.suggest.open {
            return None;
        }
        let start = self.suggest.word_start;
        let x = self.x_of(start) + self.view.text_x() - self.view.scroll_x - 26.0;
        let y = self.view.line_y(self.row_of(start));
        Some(self.suggest.frame(x, y, self.view.line_h, self.view.width, self.view.height))
    }

    /// The offset where a trailing line comment starts before `off`, if any.
    fn comment_start(&self, off: usize) -> Option<usize> {
        let token = self.lang?.line_comment?;
        let b = &self.doc.buffer;
        let p = b.pos(off);
        let start = b.line_start(p.line);
        let line = b.line(p.line);
        line[..p.col.min(line.len())]
            .match_indices(token)
            .map(|(i, _)| start + i)
            .find(|&at| self.context_at(at + token.len()) == Context::Comment)
    }

    fn newline(&mut self) {
        self.parse_now();
        let io = self.indent_opts();
        let lang = self.lang;
        let buf = &self.doc.buffer;
        let o = commands::newline_with(&self.doc.sels, |off| {
            indent::enter(buf, off, lang, io, |at| self.comment_start(at))
        });
        self.edit(o, EditKind::Other);
    }

    /// The bracket next to the primary caret (before it first) and its match.
    pub fn bracket_pair(&mut self) -> Option<(usize, usize)> {
        let brackets: &[(char, char)] = match self.lang {
            Some(l) => l.brackets,
            None => &[('(', ')'), ('[', ']'), ('{', '}')],
        };
        self.parse_now();
        let b = &self.doc.buffer;
        let head = self.doc.sels.primary().head;
        let is_bracket = |c: char| brackets.iter().any(|&(o, cl)| o == c || cl == c);
        let mut candidates = Vec::new();
        if let Some(c) = b.char_before(head).filter(|&c| is_bracket(c)) {
            candidates.push(head - c.len_utf8());
        }
        if b.char_at(head).is_some_and(is_bracket) {
            candidates.push(head);
        }
        for at in candidates {
            let m = match &self.syntax {
                Some(syn) if syn.is_current() => syn.matching_bracket(at, brackets),
                Some(_) => None,
                None => syntax::scan_bracket(b.rope(), at, brackets, 100_000),
            };
            if let Some(m) = m {
                return Some((at, m));
            }
        }
        None
    }

    /// Ctrl+Shift+\: to the matching bracket, or to the closer of the
    /// enclosing pair.
    fn jump_to_bracket(&mut self) {
        let head = self.doc.sels.primary().head;
        let target = if let Some((_, m)) = self.bracket_pair() {
            m
        } else {
            let brackets = self.lang.map_or(&[('(', ')'), ('[', ']'), ('{', '}')][..], |l| l.brackets);
            match self.syntax.as_ref().and_then(|s| s.enclosing_brackets(head, brackets)) {
                Some((_, close)) => close,
                None => return,
            }
        };
        self.set_sels(Selections::single(Selection::cursor(target)));
    }

    // ---- Find, replace and go to line ----

    /// The active overlay input.
    fn active_input(&mut self) -> Option<&mut Input> {
        match self.focus {
            Focus::Text => None,
            Focus::Find => Some(&mut self.find.find),
            Focus::Replace => Some(&mut self.find.with),
            Focus::GoTo => self.goto.as_mut().map(|g| &mut g.input),
        }
    }

    fn widget_insert(&mut self, s: &str) {
        if let Some(i) = self.active_input() {
            i.insert(s);
            self.input_changed();
        }
    }

    /// The active input's text changed.
    fn input_changed(&mut self) {
        match self.focus {
            Focus::Find => self.find_changed(),
            Focus::GoTo => self.goto_preview(),
            _ => {}
        }
        self.caret_moved();
    }

    /// Keys while an overlay input has focus. `None` lets the editor's keymap
    /// handle the key (find next, close, ...).
    fn widget_key(&mut self, c: Chord) -> Option<KeyResult> {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let k = VIRTUAL_KEY(c.key);
        let plain = !c.ctrl && !c.alt;
        let in_find = matches!(self.focus, Focus::Find | Focus::Replace);
        match k {
            VK_RETURN if self.focus == Focus::GoTo => self.goto_close(true),
            VK_RETURN if in_find && c.ctrl && c.alt => self.replace_all(),
            VK_RETURN if self.focus == Focus::Replace && plain && !c.shift => self.replace_one(),
            VK_RETURN if in_find && plain => self.find_next(!c.shift),
            VK_ESCAPE => return None,
            VK_TAB if in_find && plain => {
                self.focus = if self.focus == Focus::Find && self.find.replace && !c.shift {
                    Focus::Replace
                } else {
                    Focus::Find
                };
                self.caret_moved();
            }
            _ if in_find && c.alt && !c.ctrl && matches!(c.key, 0x43 | 0x57 | 0x52) => {
                match c.key as u8 {
                    b'C' => self.find.match_case ^= true,
                    b'W' => self.find.whole_word ^= true,
                    _ => self.find.regex ^= true,
                }
                self.find_changed();
                self.caret_moved();
            }
            _ if c.ctrl && !c.alt && matches!(c.key, 0x43 | 0x58 | 0x56) => {
                let hwnd = self.hwnd;
                let input = self.active_input()?;
                match c.key as u8 {
                    b'C' => platform::set_clipboard(hwnd, input.selected()),
                    b'X' => {
                        if let Some(t) = input.cut() {
                            platform::set_clipboard(hwnd, &t);
                        }
                    }
                    _ => {
                        if let Some(t) = platform::get_clipboard(hwnd) {
                            input.insert(&t);
                        }
                    }
                }
                self.input_changed();
            }
            _ => {
                let edited = self.active_input()?.key(c);
                match edited {
                    Edited::Text => self.input_changed(),
                    Edited::Moved => self.caret_moved(),
                    // Keys bound to find and go-to actions reach the keymap;
                    // others stay with the input.
                    Edited::No => match self.keymap.lookup(c) {
                        Some(Binding::Action(
                            Action::Find
                            | Action::Replace
                            | Action::FindNext
                            | Action::FindPrevious
                            | Action::GoToLine
                            | Action::ZoomIn
                            | Action::ZoomOut
                            | Action::ZoomReset,
                        ))
                        | Some(Binding::Command(_)) => return None,
                        _ => {}
                    },
                }
            }
        }
        Some(KeyResult::Handled)
    }

    /// Ctrl+F / Ctrl+H: opens the find widget, seeded from the selection or the
    /// word at the caret, with focus in the find (or replace) input.
    pub fn open_find(&mut self, replace: bool) {
        let p = self.doc.sels.primary();
        let b = &self.doc.buffer;
        let seed = if p.is_empty() {
            movement::word_at(b, p.head).map(|(a, e)| b.slice(a, e))
        } else {
            Some(b.slice(p.start(), p.end())).filter(|t| !t.contains('\n'))
        };
        match seed {
            Some(t) => self.find.find.set(&t),
            None => self.find.find.select_all(),
        }
        self.find.replace = replace || (self.find.open && self.find.replace);
        self.find.open = true;
        self.find.origin = p.start();
        self.find.invalidate();
        self.goto = None;
        self.focus = if replace && !self.find.find.text.is_empty() { Focus::Replace } else { Focus::Find };
        if self.focus == Focus::Replace {
            self.find.with.select_all();
        }
        self.update_find();
        self.caret_moved();
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.focus = Focus::Text;
        self.caret_moved();
    }

    /// Recomputes the matches if the text or query changed.
    pub fn update_find(&mut self) {
        if self.find.open {
            let text = self.snapshot();
            self.find.update(self.doc.buffer.version, &text);
        }
    }

    /// The query changed: select the first match from where the search began.
    fn find_changed(&mut self) {
        self.update_find();
        if let Some(i) = self.find.next_from(self.find.origin) {
            self.select_match(i);
        }
    }

    fn select_match(&mut self, i: usize) {
        let (a, e) = self.find.matches[i];
        self.doc.set_selections(Selections::single(Selection::new(a, e)));
        self.select_stack.clear();
        self.reveal(true);
        self.caret_moved();
    }

    /// Enter / F3 (and Shift+): the next or previous match. Opens the widget
    /// (without moving focus) when it is closed.
    pub fn find_next(&mut self, forward: bool) {
        if !self.find.open {
            let focus = self.focus;
            self.open_find(false);
            self.focus = focus;
        }
        self.update_find();
        let p = self.doc.sels.primary();
        let i = if forward { self.find.next_from(p.end()) } else { self.find.prev_before(p.start()) };
        if let Some(i) = i {
            self.find.origin = self.find.matches[i].0;
            self.select_match(i);
        }
    }

    /// Enter in the replace input: replaces the current match and moves on.
    pub fn replace_one(&mut self) {
        self.update_find();
        let p = self.doc.sels.primary();
        let Some(i) = self.find.index_of((p.start(), p.end())) else {
            return self.find_next(true);
        };
        let q = self.find.query();
        let Ok(re) = q.compile() else { return };
        let text = self.snapshot();
        let m = self.find.matches[i];
        let with = search::replacement(&text, &re, m, &self.find.with.text, q.regex);
        let end = m.0 + with.len();
        let o = commands::Outcome {
            tx: crate::text::Transaction { edits: vec![crate::text::Edit::new(m.0, m.1, with)] },
            sels: Selections::single(Selection::cursor(end)),
        };
        self.edit(o, EditKind::Other);
        self.update_find();
        if let Some(j) = self.find.next_from(end) {
            self.select_match(j);
        }
    }

    /// Ctrl+Alt+Enter: replaces every match as one undo step.
    pub fn replace_all(&mut self) {
        self.update_find();
        let q = self.find.query();
        let Ok(re) = q.compile() else { return };
        if self.find.matches.is_empty() {
            return;
        }
        let text = self.snapshot();
        let edits = search::replace_all(&text, &re, &self.find.matches, &self.find.with.text, q.regex);
        let tx = crate::text::Transaction { edits };
        let sels = self.doc.sels.map_tx(&tx);
        self.edit(commands::Outcome { tx, sels }, EditKind::Other);
        self.update_find();
    }

    /// Ctrl+G: opens go to line.
    pub fn open_goto(&mut self) {
        self.goto = Some(GoTo {
            input: Input::default(),
            scroll: (self.view.scroll_x, self.view.scroll_y),
            sels: self.doc.sels.clone(),
        });
        self.focus = Focus::GoTo;
        self.caret_moved();
    }

    fn goto_target(&self) -> Option<(usize, Option<usize>)> {
        goto::parse(&self.goto.as_ref()?.input.text, self.doc.buffer.line_count())
    }

    /// Scrolls to the line being typed.
    fn goto_preview(&mut self) {
        if let Some((line, _)) = self.goto_target() {
            self.ensure_fonts();
            let row = self.rows.row_of(line, 0);
            self.view.reveal_line(row, 0, true);
            self.invalidate();
        }
    }

    /// Enter (`accept`) moves the caret to the line; Esc goes back.
    pub fn goto_close(&mut self, accept: bool) {
        let target = self.goto_target();
        let Some(g) = self.goto.take() else { return };
        self.focus = Focus::Text;
        match target {
            Some((line, col)) if accept => {
                let b = &self.doc.buffer;
                let off = match col {
                    Some(c) => b.offset(crate::text::Pos::new(line, c)),
                    None => b.line_start(line) + movement::home(b, b.line_start(line)) - b.line_start(line),
                };
                self.doc.set_selections(Selections::single(Selection::cursor(off)));
                self.unfold_carets();
                self.view.reveal_line(self.rows.row_of(line, 0), 0, true);
                self.reveal(true);
            }
            _ => {
                (self.view.scroll_x, self.view.scroll_y) = g.scroll;
                self.doc.set_selections(g.sels);
            }
        }
        self.caret_moved();
    }

    /// A click on an overlay. Returns false when (x, y) is not on one.
    fn widget_click(&mut self, x: f32, y: f32) -> bool {
        let w = self.view.width;
        if self.goto.is_some() {
            if GoTo::frame(w).contains(x, y) {
                let r = GoTo::input_rect(w);
                self.focus = Focus::GoTo;
                let i = &mut self.goto.as_mut().unwrap().input;
                let col = self.render.ui_col_at(&i.text, x - r.l - 4.0 + i.scroll);
                (i.caret, i.anchor) = (col, col);
                self.caret_moved();
                return true;
            }
            self.goto_close(false);
        }
        let Some(hit) = self.find.hit(w, x, y) else {
            if self.focus != Focus::Text {
                self.focus = Focus::Text;
                self.caret_moved();
            }
            return false;
        };
        let l = self.find.layout(w);
        let place = |st: &mut State, focus: Focus, r: Rect| {
            st.focus = focus;
            let i = if focus == Focus::Find { &mut st.find.find } else { &mut st.find.with };
            let col = st.render.ui_col_at(&i.text, x - r.l - 4.0 + i.scroll);
            (i.caret, i.anchor) = (col, col);
        };
        match hit {
            find::Hit::ToggleReplace => self.find.replace ^= true,
            find::Hit::FindInput => place(self, Focus::Find, l.find_input),
            find::Hit::ReplaceInput => place(self, Focus::Replace, l.replace_input),
            find::Hit::MatchCase | find::Hit::WholeWord | find::Hit::Regex => {
                match hit {
                    find::Hit::MatchCase => self.find.match_case ^= true,
                    find::Hit::WholeWord => self.find.whole_word ^= true,
                    _ => self.find.regex ^= true,
                }
                self.find_changed();
            }
            find::Hit::Prev => self.find_next(false),
            find::Hit::Next => self.find_next(true),
            find::Hit::Close => self.close_find(),
            find::Hit::ReplaceOne => self.replace_one(),
            find::Hit::ReplaceAll => self.replace_all(),
            find::Hit::Body => {}
        }
        if self.focus == Focus::Replace && !self.find.replace {
            self.focus = Focus::Find;
        }
        self.caret_moved();
        true
    }

    /// The mouse rested at (x, y): shows a decoration's hover message there.
    pub fn hover(&mut self, x: f32, y: f32) {
        if self.view.region(x, y) != Region::Text || self.over_widget(x, y) {
            return;
        }
        let off = self.hit(x, y);
        // A hit past the end of a line is not over its last character.
        let at_end = off == self.doc.buffer.line_end(self.doc.buffer.line_of(off))
            && x - self.view.text_x() + self.view.scroll_x > self.x_of(off) + self.view.char_w;
        let msg = if at_end { None } else { self.decorations.hover_at(off).map(str::to_string) };
        if let Some(m) = msg {
            self.tooltip = Some((x, y, m));
            self.invalidate();
        }
    }

    pub fn hide_tooltip(&mut self) {
        if self.tooltip.take().is_some() {
            self.invalidate();
        }
    }

    /// Whether (x, y) is over an overlay (for the mouse cursor).
    pub fn over_widget(&self, x: f32, y: f32) -> bool {
        (self.goto.is_some() && GoTo::frame(self.view.width).contains(x, y)) || self.find.hit(self.view.width, x, y).is_some()
    }

    /// The whole text as one string, kept until the next edit.
    pub fn snapshot(&mut self) -> Rc<str> {
        let v = self.doc.buffer.version;
        match &self.snapshot {
            Some((sv, t)) if *sv == v => t.clone(),
            _ => {
                let t: Rc<str> = self.doc.buffer.text().into();
                self.snapshot = Some((v, t.clone()));
                t
            }
        }
    }

    /// Ctrl+Alt+Up/Down: a caret one line above or below every selection.
    fn add_cursors(&mut self, dir: isize) {
        let b = &self.doc.buffer;
        let tab = self.opts.tab_size.max(1);
        let mut v = self.doc.sels.as_slice().to_vec();
        let mut added = None;
        for s in self.doc.sels.iter() {
            let line = b.line_of(s.head);
            if (dir < 0 && line == 0) || (dir > 0 && line + 1 >= b.line_count()) {
                continue;
            }
            let (off, goal) = movement::vertical(b, s.head, s.goal, dir, tab);
            let mut c = Selection::cursor(off);
            c.goal = Some(goal);
            v.push(c);
            added = Some(v.len() - 1);
        }
        if let Some(p) = added {
            // The new caret furthest in the direction of travel is primary.
            let p = if dir < 0 {
                (0..v.len()).min_by_key(|&i| v[i].head).unwrap_or(p)
            } else {
                (0..v.len()).max_by_key(|&i| v[i].head).unwrap_or(p)
            };
            self.set_sels(Selections::new(v, p));
        }
    }

    /// The text Ctrl+D and Ctrl+Shift+L look for: the primary selection, or the
    /// word at each empty caret (which then become selected).
    fn occurrence_needle(&mut self) -> Option<String> {
        let prim = self.doc.sels.primary();
        if prim.is_empty() {
            let b = &self.doc.buffer;
            let sels = self.doc.sels.map(|s| match movement::word_at(b, s.head) {
                Some((a, e)) if s.is_empty() => Selection::new(a, e),
                _ => *s,
            });
            let prim = sels.primary();
            if prim.is_empty() {
                return None;
            }
            self.whole_word_next = true;
            self.set_sels(sels);
            return None;
        }
        Some(self.doc.buffer.slice(prim.start(), prim.end()))
    }

    /// Ctrl+D: selects the word at the caret, then adds the next occurrence of
    /// the selection.
    fn add_next_occurrence(&mut self) {
        let Some(needle) = self.occurrence_needle() else { return };
        let text = self.snapshot();
        let from = self.doc.sels.primary().end();
        let taken = |a: usize, e: usize| self.doc.sels.iter().any(|s| s.start() < e && a < s.end());
        let next = commands::occurrences(&text, &needle, self.whole_word_next)
            .filter(|&(a, _)| a >= from)
            .chain(commands::occurrences(&text, &needle, self.whole_word_next).filter(|&(a, _)| a < from))
            .find(|&(a, e)| !taken(a, e));
        if let Some((a, e)) = next {
            let mut sels = self.doc.sels.clone();
            sels.push(Selection::new(a, e));
            self.set_sels(sels);
        }
    }

    /// Ctrl+Shift+L: selects every occurrence of the selection or word.
    fn select_all_occurrences(&mut self) {
        let needle = match self.occurrence_needle() {
            Some(n) => n,
            None if !self.doc.sels.primary().is_empty() => {
                let p = self.doc.sels.primary();
                self.doc.buffer.slice(p.start(), p.end())
            }
            None => return,
        };
        let text = self.snapshot();
        let prim = self.doc.sels.primary().start();
        let v: Vec<Selection> = commands::occurrences(&text, &needle, self.whole_word_next)
            .map(|(a, e)| Selection::new(a, e))
            .collect();
        if v.is_empty() {
            return;
        }
        let p = v.iter().position(|s| s.start() == prim).unwrap_or(0);
        self.set_sels(Selections::new(v, p));
    }

    /// Shift+Alt+Right: grows each selection to the word, then to enclosing
    /// syntax nodes (or the line and the whole text without a grammar).
    fn expand_selection(&mut self) {
        self.parse_now();
        let b = &self.doc.buffer;
        let syn = self.syntax.as_ref().filter(|s| s.is_current());
        let new = self.doc.sels.map(|s| {
            let (a, e) = (s.start(), s.end());
            if s.is_empty()
                && let Some((wa, we)) = movement::word_at(b, a)
            {
                return Selection::new(wa, we);
            }
            let grown = match syn {
                Some(syn) => syn.expand(a, e),
                None => {
                    let l0 = b.line_start(b.line_of(a));
                    let l1 = b.line_end(b.line_of(e));
                    if (l0, l1) != (a, e) { Some((l0, l1)) } else { Some((0, b.len())) }
                }
            };
            let (na, ne) = grown.unwrap_or((a, e));
            Selection::new(na, ne)
        });
        if new != self.doc.sels {
            self.select_stack.push(self.doc.sels.clone());
            self.doc.set_selections(new);
            self.reveal(false);
            self.caret_moved();
        }
    }

    /// Ranges to mark as other occurrences of the selection or the word at the
    /// caret, within `lines`.
    pub fn occurrence_marks(&mut self, lines: std::ops::Range<usize>) -> Vec<(usize, usize)> {
        if !self.opts.occurrences_highlight || self.doc.sels.len() > 1 {
            return Vec::new();
        }
        let b = &self.doc.buffer;
        let p = self.doc.sels.primary();
        let (needle, whole) = if p.is_empty() {
            match movement::word_at(b, p.head) {
                Some((a, e)) => (b.slice(a, e), true),
                None => return Vec::new(),
            }
        } else {
            let t = b.slice(p.start(), p.end());
            if t.contains('\n') || t.trim().is_empty() || t.len() > 200 {
                return Vec::new();
            }
            (t, false)
        };
        let (from, to) = (b.line_start(lines.start), b.line_start(lines.end));
        let hay = b.slice(from, to);
        let mut marks: Vec<(usize, usize)> = commands::occurrences(&hay, &needle, whole)
            .map(|(a, e)| (from + a, from + e))
            .collect();
        // A selection alone (or a word with no other occurrence) is not marked.
        if !p.is_empty() {
            marks.retain(|&(a, _)| a != p.start());
        } else if marks.len() < 2 {
            marks.clear();
        }
        marks
    }

    /// Indent level of a line (leading whitespace in tab stops), or `None` when
    /// it is blank.
    fn indent_level(&self, line: usize) -> Option<usize> {
        let text = self.doc.buffer.line(line);
        if text.trim().is_empty() {
            return None;
        }
        let lead = text.len() - text.trim_start_matches([' ', '\t']).len();
        let tab = self.opts.tab_size.max(1);
        Some(movement::visual_col(&text, lead, tab).div_ceil(tab))
    }

    /// Indent level of a line, with blank lines taking the smaller level of the
    /// nearest non-blank lines around them (searching at most `reach` lines).
    fn guide_level(&self, line: usize, reach: usize) -> usize {
        if let Some(l) = self.indent_level(line) {
            return l;
        }
        let n = self.doc.buffer.line_count();
        let up = (line.saturating_sub(reach)..line).rev().find_map(|l| self.indent_level(l)).unwrap_or(0);
        let down = (line + 1..(line + reach).min(n)).find_map(|l| self.indent_level(l)).unwrap_or(0);
        up.min(down)
    }

    /// Indent guides for the visible lines; the guide of the block holding the
    /// primary caret is active.
    pub fn indent_guides(&self, vis: std::ops::Range<usize>) -> Vec<Guide> {
        const REACH: usize = 200;
        let caret = self.doc.buffer.line_of(self.doc.sels.primary().head);
        let levels: Vec<usize> = vis.clone().map(|l| self.guide_level(l, REACH)).collect();
        let level_of = |l: usize| {
            if vis.contains(&l) { levels[l - vis.start] } else { self.guide_level(l, REACH) }
        };
        let cl = level_of(caret);
        // The active block: lines around the caret at its level or deeper.
        let active = (cl > 0).then(|| {
            let lo = vis.start.min(caret).saturating_sub(REACH);
            let hi = (vis.end.max(caret) + REACH).min(self.doc.buffer.line_count());
            let first = (lo..caret).rev().take_while(|&l| level_of(l) >= cl).last().unwrap_or(caret);
            let last = (caret + 1..hi).take_while(|&l| level_of(l) >= cl).last().unwrap_or(caret);
            (first, last)
        });
        vis.clone()
            .zip(levels.iter())
            .filter(|(_, lv)| **lv > 0)
            .map(|(line, &levels)| Guide {
                line,
                levels,
                active: active.filter(|&(a, b)| line >= a && line <= b).map(|_| cl - 1),
            })
            .collect()
    }

    /// Detects tabs or spaces and the tab size from the text (when enabled).
    fn detect_indentation(&mut self) {
        if !self.opts.detect_indentation {
            return;
        }
        if let Some(io) = indent::detect(&self.doc.buffer)
            && (io.tab_size != self.opts.tab_size || io.insert_spaces != self.opts.insert_spaces)
        {
            let fonts = io.tab_size != self.opts.tab_size;
            self.opts.tab_size = io.tab_size;
            self.opts.insert_spaces = io.insert_spaces;
            if fonts {
                self.reset_fonts();
            }
        }
    }

    /// A WM_CHAR code unit (surrogate pairs arrive as two).
    pub fn on_char(&mut self, u: u16) {
        if (0xD800..0xDC00).contains(&u) {
            self.high_surrogate = Some(u);
            return;
        }
        let units: Vec<u16> = match self.high_surrogate.take() {
            Some(h) if (0xDC00..0xE000).contains(&u) => vec![h, u],
            _ => vec![u],
        };
        let s = String::from_utf16_lossy(&units);
        if self.focus != Focus::Text {
            if s.chars().all(|c| !c.is_control()) {
                self.widget_insert(&s);
            }
            return;
        }
        // Control characters come from keys the keymap already handled.
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => self.type_char(c),
            _ if s.chars().all(|c| !c.is_control()) => self.type_text(&s),
            _ => {}
        }
    }

    pub fn key(&mut self, chord: Chord) -> KeyResult {
        if self.suggest.open
            && self.focus == Focus::Text
            && let Some(r) = self.suggest_key(chord)
        {
            return r;
        }
        if self.focus != Focus::Text
            && let Some(r) = self.widget_key(chord)
        {
            return r;
        }
        match self.keymap.lookup(chord) {
            Some(Binding::Action(Action::ContextMenu)) => KeyResult::ContextMenu,
            Some(Binding::Action(a)) => {
                self.run(a);
                KeyResult::Handled
            }
            Some(Binding::Command(id)) => {
                self.events.push(Event::Command(id));
                KeyResult::Handled
            }
            Some(Binding::Nothing) => KeyResult::Handled,
            None => KeyResult::Unhandled,
        }
    }

    pub fn run(&mut self, a: Action) {
        let buf = &self.doc.buffer;
        let sels = &self.doc.sels;
        let io = self.indent_opts();
        match a {
            Action::Move(m, extend) => self.move_carets(m, extend),
            Action::ScrollLineUp => self.scroll_lines(-1.0),
            Action::ScrollLineDown => self.scroll_lines(1.0),
            Action::ScrollPageUp => self.scroll_lines(-(self.view.page_lines() as f32)),
            Action::ScrollPageDown => self.scroll_lines(self.view.page_lines() as f32),
            Action::Backspace => {
                let o = commands::backspace_pairs(buf, sels, io, &self.auto_closed);
                self.edit(o, EditKind::Delete);
                if self.suggest.open {
                    self.update_suggest();
                }
            }
            Action::TriggerSuggest => self.open_suggest(None, true),
            Action::Fold => self.fold_at_caret(),
            Action::Unfold => {
                let line = buf.line_of(sels.primary().head);
                self.unfold_at(line);
                self.invalidate();
            }
            Action::FoldAll => self.fold_all(),
            Action::UnfoldAll => self.unfold_all(),
            Action::ToggleWordWrap => self.toggle_wrap(),
            Action::Delete => {
                let o = commands::delete(buf, sels);
                self.edit(o, EditKind::Delete);
            }
            Action::DeleteWordLeft => {
                let o = commands::delete_word_left(buf, sels);
                self.edit(o, EditKind::Other);
            }
            Action::DeleteWordRight => {
                let o = commands::delete_word_right(buf, sels);
                self.edit(o, EditKind::Other);
            }
            Action::Newline => self.newline(),
            Action::Tab => {
                let o = commands::tab(buf, sels, io);
                self.edit(o, EditKind::Other);
            }
            Action::Outdent | Action::OutdentLines => {
                let o = commands::outdent_lines(buf, sels, io);
                self.edit(o, EditKind::Other);
            }
            Action::IndentLines => {
                let o = commands::indent_lines(buf, sels, io);
                self.edit(o, EditKind::Other);
            }
            Action::Undo => {
                if !self.opts.read_only
                    && let Some(ch) = self.doc.undo()
                {
                    self.after_edit(ch);
                }
            }
            Action::Redo => {
                if !self.opts.read_only
                    && let Some(ch) = self.doc.redo()
                {
                    self.after_edit(ch);
                }
            }
            Action::Copy => self.copy(),
            Action::Cut => self.cut(),
            Action::Paste => self.paste(),
            Action::SelectAll => {
                let s = commands::select_all(buf);
                self.doc.set_selections(s);
                self.caret_moved();
            }
            Action::ZoomIn => self.set_zoom(self.zoom + 0.1),
            Action::ZoomOut => self.set_zoom(self.zoom - 0.1),
            Action::ZoomReset => self.set_zoom(1.0),
            Action::Find => self.open_find(false),
            Action::Replace => self.open_find(true),
            Action::FindNext => self.find_next(true),
            Action::FindPrevious => self.find_next(false),
            Action::GoToLine => self.open_goto(),
            Action::Cancel if self.goto.is_some() => self.goto_close(false),
            Action::Cancel if self.find.open => self.close_find(),
            Action::Cancel if self.doc.sels.len() > 1 => {
                let mut s = self.doc.sels.clone();
                s.collapse_to_primary();
                self.set_sels(s);
            }
            Action::Cancel if !self.doc.sels.primary().is_empty() => {
                let h = self.doc.sels.primary().head;
                self.set_sels(Selections::single(Selection::cursor(h)));
            }
            Action::ToggleLineComment => {
                if let Some(t) = self.lang.and_then(|l| l.line_comment) {
                    let o = commands::toggle_line_comment(buf, sels, t);
                    self.edit(o, EditKind::Other);
                }
            }
            Action::ToggleBlockComment => {
                if let Some((open, close)) = self.lang.and_then(|l| l.block_comment) {
                    let o = commands::toggle_block_comment(buf, sels, open, close);
                    self.edit(o, EditKind::Other);
                }
            }
            Action::JumpToBracket => self.jump_to_bracket(),
            Action::AddCursorAbove => self.add_cursors(-1),
            Action::AddCursorBelow => self.add_cursors(1),
            Action::AddNextOccurrence => self.add_next_occurrence(),
            Action::SelectAllOccurrences => self.select_all_occurrences(),
            Action::MoveLinesUp | Action::MoveLinesDown => {
                if let Some(o) = commands::move_lines(buf, sels, a == Action::MoveLinesDown) {
                    self.edit(o, EditKind::Other);
                }
            }
            Action::CopyLinesUp | Action::CopyLinesDown => {
                let o = commands::copy_lines(buf, sels, a == Action::CopyLinesDown);
                self.edit(o, EditKind::Other);
            }
            Action::DeleteLines => {
                let o = commands::delete_lines(buf, sels);
                self.edit(o, EditKind::Other);
            }
            Action::SelectLine => {
                let s = commands::select_line(buf, sels);
                self.set_sels(s);
            }
            Action::InsertLineAbove => {
                let o = commands::insert_line_above(buf, sels);
                self.edit(o, EditKind::Other);
            }
            Action::InsertLineBelow => {
                self.parse_now();
                let (lang, buf) = (self.lang, &self.doc.buffer);
                let o = commands::insert_line_below(buf, &self.doc.sels, |off| {
                    indent::enter(buf, off, lang, io, |at| self.comment_start(at)).text
                });
                self.edit(o, EditKind::Other);
            }
            Action::ExpandSelection => self.expand_selection(),
            Action::ShrinkSelection => {
                if let Some(prev) = self.select_stack.pop() {
                    self.doc.set_selections(prev);
                    self.reveal(false);
                    self.caret_moved();
                }
            }
            // Later phases.
            _ => {}
        }
    }

    fn scroll_lines(&mut self, n: f32) {
        if self.view.scroll_by(0.0, n * self.view.line_h) {
            self.invalidate();
        }
    }

    /// Moves `n` rows from `off` keeping the visual column `goal` within rows
    /// (for folded or wrapped text). Returns the offset and goal.
    fn vertical_rows(&mut self, off: usize, goal: Option<usize>, n: isize) -> (usize, usize) {
        let tab = self.opts.tab_size.max(1);
        let b = &self.doc.buffer;
        let row = self.row_of(off);
        let (line, a, _) = self.row_span(row);
        let goal = goal.unwrap_or_else(|| {
            let text = b.line(line);
            movement::visual_col(&text[a..], b.pos(off).col - a, tab)
        });
        let target = row as isize + n;
        let rows = self.view.lines as isize;
        if target < 0 {
            return (0, goal);
        }
        if target >= rows {
            return (b.len(), goal);
        }
        let (tl, ta, tb) = self.row_span(target as usize);
        let text = b.line(tl);
        let col = ta + movement::col_at_visual(&text[ta..tb], goal, tab);
        (b.line_start(tl) + col, goal)
    }

    pub fn move_carets(&mut self, m: Motion, extend: bool) {
        self.whole_word_next = false;
        if !self.rows.identity() && matches!(m, Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown) {
            let page = self.view.page_lines() as isize;
            let n = match m {
                Motion::Up => -1,
                Motion::Down => 1,
                Motion::PageUp => -page,
                _ => page,
            };
            let sels: Vec<Selection> = self.doc.sels.as_slice().to_vec();
            let moved: Vec<Selection> = sels
                .iter()
                .map(|s| {
                    let from = if extend || s.is_empty() {
                        s.head
                    } else if n < 0 {
                        s.start()
                    } else {
                        s.end()
                    };
                    let (head, goal) = self.vertical_rows(from, s.goal, n);
                    let mut x = if extend { Selection::new(s.anchor, head) } else { Selection::cursor(head) };
                    x.goal = Some(goal);
                    x
                })
                .collect();
            if n.abs() > 1 {
                self.scroll_lines(n as f32);
            }
            let p = self.doc.sels.primary_index();
            self.set_sels(Selections::new(moved, p));
            return;
        }
        let buf = &self.doc.buffer;
        let tab = self.opts.tab_size.max(1);
        let page = self.view.page_lines() as isize;
        let new = self.doc.sels.map(|s| {
            if !extend && !s.is_empty() {
                match m {
                    Motion::Left => return Selection::cursor(s.start()),
                    Motion::Right => return Selection::cursor(s.end()),
                    _ => {}
                }
            }
            let from = match m {
                _ if extend || s.is_empty() => s.head,
                Motion::Up | Motion::PageUp => s.start(),
                Motion::Down | Motion::PageDown => s.end(),
                _ => s.head,
            };
            let vertical = |n: isize| {
                let (o, g) = movement::vertical(buf, from, s.goal, n, tab);
                (o, Some(g))
            };
            let (head, goal) = match m {
                Motion::Left => (movement::prev_grapheme(buf, from), None),
                Motion::Right => (movement::next_grapheme(buf, from), None),
                Motion::Up => vertical(-1),
                Motion::Down => vertical(1),
                Motion::PageUp => vertical(-page),
                Motion::PageDown => vertical(page),
                Motion::WordLeft => (movement::word_left(buf, from), None),
                Motion::WordRight => (movement::word_right(buf, from), None),
                Motion::Home => (movement::home(buf, from), None),
                Motion::End => (movement::line_end(buf, from), None),
                Motion::DocStart => (0, None),
                Motion::DocEnd => (buf.len(), None),
            };
            let mut n = if extend {
                Selection::new(s.anchor, head)
            } else {
                Selection::cursor(head)
            };
            n.goal = goal;
            n
        });
        match m {
            Motion::PageUp => self.scroll_lines(-(page as f32)),
            Motion::PageDown => self.scroll_lines(page as f32),
            _ => {}
        }
        self.set_sels(new);
    }

    // ---- Clipboard ----

    pub fn copy(&mut self) {
        let (text, whole) = commands::copy(&self.doc.buffer, &self.doc.sels);
        platform::set_clipboard(self.hwnd, &text);
        LINE_COPY.with(|c| *c.borrow_mut() = whole.then_some(text));
    }

    pub fn cut(&mut self) {
        if self.opts.read_only {
            return self.copy();
        }
        let (text, whole, o) = commands::cut(&self.doc.buffer, &self.doc.sels);
        platform::set_clipboard(self.hwnd, &text);
        LINE_COPY.with(|c| *c.borrow_mut() = whole.then_some(text));
        self.edit(o, EditKind::Other);
    }

    pub fn paste(&mut self) {
        let Some(text) = platform::get_clipboard(self.hwnd) else {
            return;
        };
        let text = text::normalize_eol(&text).into_owned();
        let whole = LINE_COPY.with(|c| c.borrow().as_deref() == Some(text.as_str()));
        let o = commands::paste(&self.doc.buffer, &self.doc.sels, &text, whole);
        self.edit(o, EditKind::Other);
    }

    // ---- Mouse ----

    pub fn mouse_down(&mut self, x: f32, y: f32, mods: Mods, clicks: u32) {
        self.whole_word_next = false;
        if let Some(f) = self.suggest_frame() {
            if f.contains(x, y) {
                if let Some(i) = self.suggest.row_at(f, y) {
                    self.suggest.selected = i;
                    if clicks >= 2 {
                        self.accept_suggest();
                    }
                    self.invalidate();
                }
                return;
            }
            self.close_suggest();
        }
        if self.widget_click(x, y) {
            return;
        }
        self.mouse = (x, y);
        match self.view.region(x, y) {
            Region::VScroll => {
                let (top, h) = self.view.vthumb().unwrap_or((0.0, 0.0));
                let grab = if y >= top && y < top + h { y - top } else { h / 2.0 };
                self.drag = Some(Drag::VThumb { grab });
                self.drag_to(x, y);
            }
            Region::HScroll => {
                let (left, w) = self.view.hthumb().unwrap_or((0.0, 0.0));
                let tx = x - self.view.text_x();
                let grab = if tx >= left && tx < left + w { tx - left } else { w / 2.0 };
                self.drag = Some(Drag::HThumb { grab });
                self.drag_to(x, y);
            }
            Region::Gutter => {
                let (line, seg) = self.rows.line_of_row(self.view.line_at(y));
                if self.opts.folding && seg == 0 && x >= self.view.numbers_w() {
                    self.toggle_fold(line);
                    self.reveal(false);
                    return;
                }
                let r = self.line_range(line);
                let sel = if mods.shift {
                    let a = self.doc.sels.primary().anchor;
                    let r0 = self.line_range(self.doc.buffer.line_of(a));
                    span(r0, r)
                } else {
                    Selection::new(r.0, r.1)
                };
                self.drag = Some(Drag::Gutter { anchor: r });
                self.set_sels(Selections::single(sel));
            }
            Region::Text => {
                let off = self.hit(x, y);
                let unit = match clicks {
                    1 => Unit::Char,
                    2 => Unit::Word,
                    _ => Unit::Line,
                };
                let r = self.unit_range(off, unit);
                let mut sels = self.doc.sels.clone();
                if mods.shift && mods.alt {
                    let line = self.doc.buffer.line_of(off);
                    let vcol = self.vcol_at(x);
                    self.drag = Some(Drag::Column { line, vcol });
                    self.set_sels(Selections::single(Selection::cursor(off)));
                    return;
                }
                if mods.shift && unit == Unit::Char {
                    let a = sels.primary().anchor;
                    let new = Selection::new(a, off);
                    sels = replace_primary(&sels, new);
                    self.drag = Some(Drag::Text {
                        unit,
                        anchor: (a, a),
                    });
                } else if mods.alt && unit == Unit::Char {
                    sels.push(Selection::cursor(off));
                    self.drag = Some(Drag::Text { unit, anchor: r });
                } else if mods.alt {
                    sels = replace_primary(&sels, Selection::new(r.0, r.1));
                    self.drag = Some(Drag::Text { unit, anchor: r });
                } else {
                    sels = Selections::single(Selection::new(r.0, r.1));
                    self.drag = Some(Drag::Text { unit, anchor: r });
                }
                self.set_sels(sels);
            }
        }
    }

    /// (start, end) of the unit at `off`: a caret, a word, or a line with its
    /// line break.
    fn unit_range(&self, off: usize, unit: Unit) -> (usize, usize) {
        match unit {
            Unit::Char => (off, off),
            Unit::Word => movement::run_at(&self.doc.buffer, off),
            Unit::Line => self.line_range(self.doc.buffer.line_of(off)),
        }
    }

    fn line_range(&self, line: usize) -> (usize, usize) {
        let b = &self.doc.buffer;
        let end = if line + 1 < b.line_count() { b.line_start(line + 1) } else { b.len() };
        (b.line_start(line), end)
    }

    /// Mouse moved with the button down.
    pub fn drag_to(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        match self.drag {
            Some(Drag::VThumb { grab }) => {
                let s = self.view.scroll_for_vthumb(y - grab);
                self.view.scroll_y = s;
                self.view.clamp_scroll();
                self.invalidate();
            }
            Some(Drag::HThumb { grab }) => {
                let s = self.view.scroll_for_hthumb(x - self.view.text_x() - grab);
                self.view.scroll_x = s;
                self.view.clamp_scroll();
                self.invalidate();
            }
            Some(Drag::Text { unit, anchor }) => {
                let off = self.hit(x, y.clamp(0.0, self.view.height - 1.0));
                let r = self.unit_range(off, unit);
                let sel = span(anchor, r);
                let sels = replace_primary(&self.doc.sels, sel);
                self.set_sels(sels);
            }
            Some(Drag::Column { line, vcol }) => {
                let cur = self.rows.line_of_row(self.view.line_at(y.clamp(0.0, self.view.height - 1.0))).0;
                let cv = self.vcol_at(x);
                let tab = self.opts.tab_size.max(1);
                let b = &self.doc.buffer;
                let lines: Vec<usize> = if cur >= line { (line..=cur).collect() } else { (cur..=line).rev().collect() };
                let v: Vec<Selection> = lines
                    .iter()
                    .map(|&l| {
                        let text = b.line(l);
                        let s = b.line_start(l);
                        Selection::new(
                            s + movement::col_at_visual(&text, vcol, tab),
                            s + movement::col_at_visual(&text, cv, tab),
                        )
                    })
                    .collect();
                let n = v.len() - 1;
                self.set_sels(Selections::new(v, n));
            }
            Some(Drag::Gutter { anchor }) => {
                let line = self.rows.line_of_row(self.view.line_at(y.clamp(0.0, self.view.height - 1.0))).0;
                let sel = span(anchor, self.line_range(line));
                self.set_sels(Selections::single(sel));
            }
            None => {}
        }
    }

    /// The visual column nearest to editor x (for column selection).
    fn vcol_at(&self, x: f32) -> usize {
        let tx = x - self.view.text_x() + self.view.scroll_x;
        (tx / self.view.char_w).round().max(0.0) as usize
    }

    /// Whether a selection drag is outside the text area, so it should scroll.
    pub fn autoscroll_needed(&self) -> bool {
        let (x, y) = self.mouse;
        let v = &self.view;
        matches!(self.drag, Some(Drag::Text { .. } | Drag::Gutter { .. } | Drag::Column { .. }))
            && (y < 0.0
                || y > v.height
                || (x < v.text_x() && v.scroll_x > 0.0)
                || x > v.width - crate::view::SCROLLBAR)
    }

    /// While dragging a selection outside the editor, scrolls toward the mouse.
    /// Returns whether to keep auto-scrolling.
    pub fn autoscroll(&mut self) -> bool {
        if !matches!(self.drag, Some(Drag::Text { .. } | Drag::Gutter { .. } | Drag::Column { .. })) {
            return false;
        }
        let (x, y) = self.mouse;
        let v = &self.view;
        let dy = if y < 0.0 {
            y.max(-v.line_h * 5.0)
        } else if y > v.height {
            (y - v.height).min(v.line_h * 5.0)
        } else {
            0.0
        };
        let tx = x - v.text_x();
        let dx = if tx < 0.0 && v.scroll_x > 0.0 {
            tx.max(-v.char_w * 4.0)
        } else if x > v.width - crate::view::SCROLLBAR {
            (x - (v.width - crate::view::SCROLLBAR)).min(v.char_w * 4.0)
        } else {
            0.0
        };
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        self.view.scroll_by(dx, dy);
        self.drag_to(x, y);
        true
    }

    pub fn mouse_up(&mut self) {
        self.drag = None;
    }

    /// Mouse moved without a button: tracks the scrollbar hover.
    pub fn hover_at(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        let in_gutter = self.view.region(x, y) == Region::Gutter;
        if in_gutter != self.gutter_hover {
            self.gutter_hover = in_gutter;
            self.invalidate();
        }
        let r = Some(self.view.region(x, y)).filter(|r| matches!(r, Region::VScroll | Region::HScroll));
        if r != self.hover {
            self.hover = r;
            self.invalidate();
        }
    }

    pub fn mouse_leave(&mut self) {
        if std::mem::take(&mut self.gutter_hover) {
            self.invalidate();
        }
        if self.hover.take().is_some() {
            self.invalidate();
        }
    }

    /// Wheel notches (positive: away from the user / right).
    pub fn wheel(&mut self, notches: f32, horizontal: bool, mods: Mods) {
        let (mx, my) = self.mouse;
        if !horizontal
            && let Some(f) = self.suggest_frame()
            && f.contains(mx, my)
        {
            let n = self.suggest.rows.len().saturating_sub(suggest::MAX_ROWS);
            let top = self.suggest.top as f32 - notches * 3.0;
            self.suggest.top = (top.max(0.0) as usize).min(n);
            self.invalidate();
            return;
        }
        if mods.ctrl && !horizontal {
            if self.opts.mouse_wheel_zoom {
                self.set_zoom(self.zoom + 0.1 * notches.signum());
            }
            return;
        }
        let lines = platform::wheel_lines().max(1) as f32;
        let moved = if horizontal || mods.shift {
            let d = if horizontal { notches } else { -notches };
            self.view.scroll_by(d * self.view.char_w * lines * 2.0, 0.0)
        } else {
            self.view.scroll_by(0.0, -notches * lines * self.view.line_h)
        };
        if moved {
            self.invalidate();
        }
    }

    /// A right click: moves the caret there unless it is inside a selection.
    pub fn right_down(&mut self, x: f32, y: f32) {
        if self.view.region(x, y) != Region::Text {
            return;
        }
        let off = self.hit(x, y);
        if !self.doc.sels.iter().any(|s| s.start() <= off && off <= s.end() && !s.is_empty()) {
            self.set_sels(Selections::single(Selection::cursor(off)));
        }
    }

    pub fn menu_items(&self) -> Vec<platform::MenuItem<'static>> {
        // Cut and copy take the whole line when nothing is selected, as in Monaco.
        let ro = self.opts.read_only;
        vec![
            platform::MenuItem { id: MENU_CUT, label: "Cut\tCtrl+X", enabled: !ro },
            platform::MenuItem { id: MENU_COPY, label: "Copy\tCtrl+C", enabled: true },
            platform::MenuItem {
                id: MENU_PASTE,
                label: "Paste\tCtrl+V",
                enabled: !ro && platform::clipboard_has_text(),
            },
            platform::MenuItem { id: 0, label: "", enabled: true },
            platform::MenuItem { id: MENU_SELECT_ALL, label: "Select All\tCtrl+A", enabled: true },
        ]
    }

    pub fn menu_command(&mut self, id: u32) {
        match id {
            MENU_CUT => self.run(Action::Cut),
            MENU_COPY => self.run(Action::Copy),
            MENU_PASTE => self.run(Action::Paste),
            MENU_SELECT_ALL => self.run(Action::SelectAll),
            _ => {}
        }
    }

    // ---- Focus and caret ----

    pub fn set_focus(&mut self, focused: bool) {
        if !focused {
            self.close_suggest();
        }
        if self.focused != focused {
            self.focused = focused;
            self.caret_on = focused;
            self.fx.reset_blink = focused;
            self.fx.ime = focused;
            self.events.push(Event::Focus(focused));
            self.invalidate();
        }
    }

    pub fn blink(&mut self) {
        self.caret_on = !self.caret_on || !self.opts.cursor_blinking;
        self.invalidate();
    }

    // ---- Text and language ----

    /// Replaces the document (new history, caret at the start).
    pub fn load(&mut self, buffer: Buffer) {
        self.doc.reset(buffer);
        self.folds.clear();
        self.rows = LineMap::default();
        self.view.lines = self.doc.buffer.line_count();
        self.view.scroll_x = 0.0;
        self.view.scroll_y = 0.0;
        self.view.content_w = 0.0;
        self.events.push(Event::ContentChanged {
            version: self.doc.buffer.version,
            changes: Vec::new(),
        });
        self.auto_closed.clear();
        self.decorations.clear();
        self.detect_indentation();
        self.start_syntax();
        if self.opts.word_wrap && self.render.fonts.is_some() {
            self.sync_rows();
        }
        self.caret_moved();
    }

    pub fn set_lang(&mut self, lang: Option<&'static Lang>) {
        if self.lang.map(|l| l.id) != lang.map(|l| l.id) {
            self.lang = lang;
            self.start_syntax();
            self.events.push(Event::LanguageChanged(lang.map(|l| l.id)));
            self.invalidate();
        }
    }

    /// Sets up parsing for the current language and text: lazily on the next
    /// paint, or on a worker thread for big files.
    fn start_syntax(&mut self) {
        self.hl = HighlightCache::default();
        self.parse_gen += 1;
        self.syntax = None;
        self.syntax_pending = None;
        let Some(lang) = self.lang else { return };
        syntax::prefetch(lang);
        if self.doc.buffer.len() <= syntax::BACKGROUND_PARSE_BYTES {
            self.syntax_pending = Some(lang);
            return;
        }
        let mut syn = match Syntax::new(lang) {
            Ok(s) => s,
            Err(e) => {
                self.events.push(Event::Warning(format!("Highlighting is off: {e}")));
                return;
            }
        };
        let (hwnd, generation) = (self.own_hwnd.0 as isize, self.parse_gen);
        syn.start_background(self.doc.buffer.rope().clone(), move |tree| {
            let boxed = Box::into_raw(Box::new(tree));
            unsafe {
                let h = HWND(hwnd as _);
                if windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(h),
                    WM_APP_PARSED,
                    windows::Win32::Foundation::WPARAM(generation),
                    windows::Win32::Foundation::LPARAM(boxed as isize),
                )
                .is_err()
                {
                    drop(Box::from_raw(boxed));
                }
            }
        });
        self.syntax = Some(syn);
    }

    /// A background parse finished.
    pub fn parsed(&mut self, generation: usize, tree: tree_sitter::Tree) {
        if generation == self.parse_gen
            && let Some(syn) = &mut self.syntax
        {
            syn.finish_background(tree);
            self.hl = HighlightCache::default();
            self.invalidate();
        }
    }

    pub fn set_lang_for_file(&mut self, name: &str) {
        self.set_lang(lang::detect(name));
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.render.set_palette(Palette::new(theme));
        self.invalidate();
    }

    pub fn set_options(&mut self, o: Options) {
        let fonts = o.font_family != self.opts.font_family
            || o.font_size != self.opts.font_size
            || o.tab_size != self.opts.tab_size;
        let wrap = o.word_wrap != self.opts.word_wrap;
        if !o.folding {
            self.folds.clear();
        }
        self.view.line_numbers = o.line_numbers;
        self.view.folding = o.folding;
        self.opts = o;
        if fonts {
            self.reset_fonts();
        }
        if wrap || fonts {
            self.view.scroll_x = 0.0;
            self.sync_rows();
        }
        self.invalidate();
    }
}

/// `off` mapped through applied changes (in application order); `None` when
/// its text was replaced.
fn map_offset(mut off: usize, changes: &[Change]) -> Option<usize> {
    for c in changes {
        if off >= c.old_end {
            off = off - c.old_end + c.new_end;
        } else if off > c.start {
            return None;
        }
    }
    Some(off)
}

/// The selection from `anchor` (a unit range first clicked) to the unit `r`.
fn span(anchor: (usize, usize), r: (usize, usize)) -> Selection {
    if r.0 < anchor.0 {
        Selection::new(anchor.1, r.0)
    } else {
        Selection::new(anchor.0, r.1.max(anchor.1))
    }
}

fn replace_primary(sels: &Selections, s: Selection) -> Selections {
    let i = sels.primary_index();
    let mut v = sels.as_slice().to_vec();
    v[i] = s;
    Selections::new(v, i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::{parse, show};

    /// A state holding marked text in `lang` (no window).
    pub(crate) fn state(lang: Option<&str>, marked: &str) -> State {
        let mut st = State::new(HWND::default(), Options::default(), Theme::Dark).unwrap();
        let (buf, sels) = parse(marked);
        st.load(buf);
        st.set_lang(lang.and_then(lang::by_id));
        st.doc.sels = sels;
        st
    }

    pub(crate) fn shown(st: &State) -> String {
        show(&st.doc.buffer, &st.doc.sels)
    }

    fn type_str(st: &mut State, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                st.run(Action::Newline);
            } else {
                st.type_char(c);
            }
        }
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_typing_session() {
        let mut st = state(Some("python"), "|");
        type_str(&mut st, "def f(x):\nreturn [x, \"a\"]\n");
        assert_eq!(st.doc.buffer.text(), "def f(x):\n    return [x, \"a\"]\n");
        assert_eq!(shown(&st), "def f(x):\n    return [x, \"a\"]\n|");
        // Backspace right after typing an opener removes the pair.
        let mut st = state(Some("python"), "x = |");
        type_str(&mut st, "(");
        st.run(Action::Backspace);
        assert_eq!(shown(&st), "x = |");
        // No auto-closing inside strings and comments.
        let mut st = state(Some("python"), "s = 'ab|c'  # (|");
        type_str(&mut st, "(");
        assert_eq!(shown(&st), "s = 'ab(|c'  # ((|");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_enter_between_brackets_and_comments() {
        let mut st = state(Some("python"), "foo(|)");
        st.run(Action::Newline);
        assert_eq!(shown(&st), "foo(\n    |\n)");
        let mut st = state(Some("python"), "if x:  # note|");
        st.run(Action::Newline);
        assert_eq!(shown(&st), "if x:  # note\n    |");
        let mut st = state(Some("python"), "s = 'a:'|");
        st.run(Action::Newline);
        assert_eq!(shown(&st), "s = 'a:'\n|");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn comment_toggle_and_bracket_jump() {
        let mut st = state(Some("python"), "[a = 1\nb = 2]");
        st.run(Action::ToggleLineComment);
        assert_eq!(st.doc.buffer.text(), "# a = 1\n# b = 2");
        st.run(Action::ToggleLineComment);
        assert_eq!(st.doc.buffer.text(), "a = 1\nb = 2");

        let mut st = state(Some("python"), "f(a|, \"(\", g(b))");
        assert_eq!(st.bracket_pair(), None);
        st.run(Action::JumpToBracket);
        assert_eq!(shown(&st), "f(a, \"(\", g(b)|)");
        // Next to a bracket (the one before the caret first): to its match.
        st.run(Action::JumpToBracket);
        assert_eq!(shown(&st), "f(a, \"(\", g|(b))");
        let mut st = state(Some("python"), "f(|a)");
        st.run(Action::JumpToBracket);
        assert_eq!(shown(&st), "f(a|)");
    }

    #[cfg(feature = "lang-sql")]
    #[test]
    fn sql_block_comments_and_parens() {
        let mut st = state(Some("sql"), "SELECT [a] FROM t");
        st.run(Action::ToggleBlockComment);
        assert_eq!(st.doc.buffer.text(), "SELECT /* a */ FROM t");
        let mut st = state(Some("sql"), "WHERE x IN |");
        type_str(&mut st, "(1, 'a')");
        assert_eq!(shown(&st), "WHERE x IN (1, 'a')|");
    }

    #[test]
    fn plain_text_has_no_auto_closing_but_matches_brackets() {
        let mut st = state(None, "f|");
        type_str(&mut st, "(");
        assert_eq!(shown(&st), "f(|");
        let mut st = state(None, "a(b{c}|)");
        assert_eq!(st.bracket_pair(), Some((5, 3)));
        let mut st = state(None, "|a(b{c})");
        assert_eq!(st.bracket_pair(), None);
        let mut st = state(None, "a|(b{c})");
        assert_eq!(st.bracket_pair(), Some((1, 6)));
    }

    #[test]
    fn indentation_is_detected_on_load() {
        let st = state(None, "a:\n  b\n  c:\n    d\n|");
        assert_eq!((st.opts.tab_size, st.opts.insert_spaces), (2, true));
        let st = state(None, "a\n\tb\n|");
        assert!(!st.opts.insert_spaces);
    }

    #[test]
    fn indent_guides_follow_blocks() {
        let st = state(None, "a:\n    b\n\n    c|\nd\n");
        let g = st.indent_guides(0..5);
        let lines: Vec<(usize, usize, Option<usize>)> = g.iter().map(|g| (g.line, g.levels, g.active)).collect();
        assert_eq!(lines, vec![(1, 1, Some(0)), (2, 1, Some(0)), (3, 1, Some(0))]);
    }
}

#[cfg(test)]
mod multi_cursor_tests {
    use super::tests::{shown, state};
    use super::*;

    #[test]
    fn ctrl_d_adds_whole_word_occurrences() {
        let mut st = state(None, "foo bar fo|o foo_x foo");
        st.run(Action::AddNextOccurrence);
        assert_eq!(shown(&st), "foo bar [foo] foo_x foo");
        st.run(Action::AddNextOccurrence);
        assert_eq!(shown(&st), "foo bar [foo] foo_x [foo]");
        st.run(Action::AddNextOccurrence);
        assert_eq!(shown(&st), "[foo] bar [foo] foo_x [foo]");
        // Typing replaces every occurrence.
        st.type_text("baz");
        assert_eq!(shown(&st), "baz| bar baz| foo_x baz|");
        st.run(Action::Cancel);
        assert_eq!(st.doc.sels.len(), 1);
    }

    #[test]
    fn select_all_occurrences_of_a_selection() {
        let mut st = state(None, "[ab] abc xab");
        st.run(Action::SelectAllOccurrences);
        assert_eq!(shown(&st), "[ab] [ab]c x[ab]");
    }

    #[test]
    fn cursors_above_and_below_keep_the_column() {
        let mut st = state(None, "abcd\nx\nabcd|");
        st.run(Action::AddCursorAbove);
        st.run(Action::AddCursorAbove);
        assert_eq!(shown(&st), "abcd|\nx|\nabcd|");
        st.type_text("!");
        assert_eq!(shown(&st), "abcd!|\nx!|\nabcd!|");
    }

    #[test]
    fn column_selection_by_mouse() {
        let mut st = state(None, "|abcdef\nab\nabcdef");
        st.view.width = 800.0;
        st.view.height = 400.0;
        st.ensure_fonts();
        let (x0, cw, lh) = (st.view.text_x(), st.view.char_w, st.view.line_h);
        let alt_shift = Mods { shift: true, alt: true, ctrl: false };
        st.mouse_down(x0 + cw, lh / 2.0, alt_shift, 1);
        st.drag_to(x0 + 4.0 * cw, 2.5 * lh);
        assert_eq!(shown(&st), "a[bcd]ef\na[b]\na[bcd]ef");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn smart_select_grows_and_shrinks() {
        let mut st = state(Some("python"), "x = foo(a|b, c)\n");
        st.run(Action::ExpandSelection);
        assert_eq!(shown(&st), "x = foo([ab], c)\n");
        st.run(Action::ExpandSelection);
        assert_eq!(shown(&st), "x = foo[(ab, c)]\n");
        st.run(Action::ExpandSelection);
        assert_eq!(shown(&st), "x = [foo(ab, c)]\n");
        st.run(Action::ShrinkSelection);
        st.run(Action::ShrinkSelection);
        assert_eq!(shown(&st), "x = foo([ab], c)\n");
    }

    #[test]
    fn occurrence_marks_for_word_and_selection() {
        let mut st = state(None, "ab x a|b\nab");
        assert_eq!(st.occurrence_marks(0..2), vec![(0, 2), (5, 7), (8, 10)]);
        let mut st = state(None, "[ab] xab");
        assert_eq!(st.occurrence_marks(0..1), vec![(4, 6)]);
        let mut st = state(None, "lonely|");
        assert!(st.occurrence_marks(0..1).is_empty());
    }

    #[test]
    fn line_actions_through_the_keymap() {
        let mut st = state(None, "a\nb|\nc");
        st.run(Action::MoveLinesUp);
        assert_eq!(shown(&st), "b|\na\nc");
        st.run(Action::CopyLinesDown);
        assert_eq!(shown(&st), "b\nb|\na\nc");
        st.run(Action::DeleteLines);
        assert_eq!(shown(&st), "b\na|\nc");
        st.run(Action::InsertLineAbove);
        assert_eq!(shown(&st), "b\n|\na\nc");
        st.run(Action::Undo);
        st.run(Action::SelectLine);
        assert_eq!(shown(&st), "b\n[a\n]c");
    }
}

#[cfg(test)]
mod find_tests {
    use super::tests::{shown, state};
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    fn key(st: &mut State, vk: VIRTUAL_KEY, ctrl: bool, shift: bool, alt: bool) {
        st.key(Chord::new(vk.0, ctrl, shift, alt));
    }

    fn type_in(st: &mut State, s: &str) {
        for u in s.encode_utf16() {
            st.on_char(u);
        }
    }

    #[test]
    fn find_is_seeded_and_steps_through_matches() {
        let mut st = state(None, "one tw|o one two\ntwo");
        st.run(Action::Find);
        assert!(st.find.open);
        assert_eq!(st.focus, Focus::Find);
        assert_eq!(st.find.find.text, "two");
        assert_eq!(st.find.matches.len(), 3);
        // Enter looks from the caret, which is inside the first match.
        key(&mut st, VK_RETURN, false, false, false);
        assert_eq!(shown(&st), "one two one [two]\ntwo");
        key(&mut st, VK_RETURN, false, false, false);
        assert_eq!(shown(&st), "one two one two\n[two]");
        key(&mut st, VK_F3, false, false, false);
        assert_eq!(shown(&st), "one [two] one two\ntwo");
        key(&mut st, VK_RETURN, false, true, false);
        assert_eq!(shown(&st), "one two one two\n[two]");
        assert_eq!(st.find.status((16, 19)), "3 of 3");
        // Typing a new query searches from where the search began.
        st.find.find.select_all();
        type_in(&mut st, "one");
        assert_eq!(st.find.matches, vec![(0, 3), (8, 11)]);
        // Esc closes and returns to the text.
        key(&mut st, VK_ESCAPE, false, false, false);
        assert!(!st.find.open);
        assert_eq!(st.focus, Focus::Text);
    }

    #[test]
    fn toggles_change_the_matches() {
        let mut st = state(None, "|Foo foo food");
        st.run(Action::Find);
        type_in(&mut st, "foo");
        assert_eq!(st.find.matches.len(), 3);
        key(&mut st, VK_C, false, false, true);
        assert_eq!(st.find.matches.len(), 2);
        key(&mut st, VK_W, false, false, true);
        assert_eq!(st.find.matches, vec![(4, 7)]);
    }

    #[test]
    fn replace_one_and_all() {
        // Nothing to seed from (the caret is not on a word): focus stays in find.
        let mut st = state(None, "| a1 b2 a3");
        st.run(Action::Replace);
        assert_eq!(st.focus, Focus::Find);
        type_in(&mut st, "a");
        key(&mut st, VK_TAB, false, false, false);
        assert_eq!(st.focus, Focus::Replace);
        type_in(&mut st, "X");
        key(&mut st, VK_RETURN, false, false, false);
        assert_eq!(shown(&st), " X1 b2 [a]3");
        // Regex replace-all with groups, as one undo step.
        let mut st = state(None, "| a1 b2 a3");
        st.run(Action::Replace);
        key(&mut st, VK_R, false, false, true);
        type_in(&mut st, r"(\w)(\d)");
        key(&mut st, VK_TAB, false, false, false);
        type_in(&mut st, "$2$1");
        key(&mut st, VK_RETURN, true, false, true);
        assert_eq!(st.doc.buffer.text(), " 1a 2b 3a");
        st.focus = Focus::Text;
        st.run(Action::Undo);
        assert_eq!(st.doc.buffer.text(), " a1 b2 a3");
        // Seeded from the word at the caret, Ctrl+H goes to the replace input.
        let mut st = state(None, "a|1 b2");
        st.run(Action::Replace);
        assert_eq!((st.focus, st.find.find.text.as_str()), (Focus::Replace, "a1"));
    }

    #[test]
    fn go_to_line_moves_or_cancels() {
        let text = (1..=50).map(|i| format!("  line {i}")).collect::<Vec<_>>().join("\n");
        let mut st = state(None, &format!("|{text}"));
        st.view.width = 600.0;
        st.view.height = 200.0;
        st.run(Action::GoToLine);
        assert_eq!(st.focus, Focus::GoTo);
        type_in(&mut st, "40");
        assert!(st.view.scroll_y > 0.0);
        key(&mut st, VK_RETURN, false, false, false);
        assert_eq!(st.doc.buffer.pos(st.doc.sels.primary().head), crate::text::Pos::new(39, 2));
        assert_eq!(st.focus, Focus::Text);
        st.run(Action::GoToLine);
        type_in(&mut st, "3:5");
        key(&mut st, VK_ESCAPE, false, false, false);
        assert_eq!(st.doc.buffer.pos(st.doc.sels.primary().head).line, 39);
        st.run(Action::GoToLine);
        type_in(&mut st, "3:5");
        key(&mut st, VK_RETURN, false, false, false);
        assert_eq!(st.doc.buffer.pos(st.doc.sels.primary().head), crate::text::Pos::new(2, 4));
    }
}

#[cfg(test)]
mod suggest_tests {
    use super::tests::{shown, state};
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;

    fn type_in(st: &mut State, s: &str) {
        for c in s.chars() {
            st.type_char(c);
        }
    }

    fn key(st: &mut State, vk: VIRTUAL_KEY) {
        st.key(Chord::new(vk.0, false, false, false));
    }

    fn labels(st: &State) -> Vec<String> {
        st.suggest.rows.iter().map(|r| st.suggest.items[r.item].label.clone()).collect()
    }

    struct Schema;

    impl CompletionProvider for Schema {
        fn complete(&self, cx: &CompletionContext) -> Vec<CompletionItem> {
            if cx.trigger == Some('.') || cx.text[..cx.offset - cx.prefix.len()].ends_with('.') {
                return ["id", "name"].iter().map(|c| CompletionItem::new(*c, CompletionKind::Field)).collect();
            }
            vec![CompletionItem {
                label: "customers".into(),
                detail: Some("table".into()),
                kind: CompletionKind::Class,
                insert_text: None,
            }]
        }

        fn trigger_characters(&self) -> Vec<char> {
            vec!['.']
        }
    }

    #[cfg(feature = "lang-sql")]
    #[test]
    fn typing_opens_filters_and_accepts() {
        let mut st = state(Some("sql"), "|");
        type_in(&mut st, "sel");
        assert!(st.suggest.open);
        assert_eq!(labels(&st)[0], "SELECT");
        key(&mut st, VK_RETURN);
        assert!(!st.suggest.open);
        assert_eq!(shown(&st), "SELECT|");
        // Esc closes; a space closes too.
        type_in(&mut st, " FR");
        assert!(st.suggest.open);
        key(&mut st, VK_ESCAPE);
        assert!(!st.suggest.open);
        type_in(&mut st, "OM");
        assert!(st.suggest.open);
        type_in(&mut st, " ");
        assert!(!st.suggest.open);
    }

    #[cfg(feature = "lang-sql")]
    #[test]
    fn provider_items_and_trigger_characters() {
        let mut st = state(Some("sql"), "SELECT * FROM |");
        st.provider = Some(Rc::new(Schema));
        type_in(&mut st, "cu");
        assert_eq!(labels(&st)[0], "customers");
        key(&mut st, VK_TAB);
        type_in(&mut st, " c.");
        assert!(st.suggest.open);
        assert_eq!(labels(&st), vec!["id", "name"]);
        key(&mut st, VK_DOWN);
        key(&mut st, VK_RETURN);
        assert_eq!(shown(&st), "SELECT * FROM customers c.name|");
    }

    #[test]
    fn buffer_words_and_explicit_trigger() {
        let mut st = state(None, "alpha beta\n|");
        st.run(Action::TriggerSuggest);
        assert!(st.suggest.open);
        assert_eq!(labels(&st).len(), 2);
        type_in(&mut st, "b");
        assert_eq!(labels(&st), vec!["beta"]);
        // Backspace past the word start closes the popup.
        st.run(Action::Backspace);
        assert!(st.suggest.open);
        st.run(Action::Backspace);
        assert!(!st.suggest.open);
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn no_quick_suggestions_in_strings() {
        let mut st = state(Some("python"), "x = 'pri|'\nprint");
        type_in(&mut st, "n");
        assert!(!st.suggest.open);
        let mut st = state(Some("python"), "print\npri|");
        type_in(&mut st, "n");
        assert!(st.suggest.open);
    }
}

#[cfg(test)]
mod fold_wrap_tests {
    use super::tests::{shown, state};
    use super::*;

    fn sized(mut st: State, w: f32) -> State {
        st.view.width = w;
        st.view.height = 300.0;
        st.ensure_fonts();
        st
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_fold_regions_from_the_tree() {
        // (Braces, since the test markup uses square brackets for selections.)
        let src = "def f(x):\n    if x:\n        a = {\n            1: 2,\n        }\n    return x\n\nclass C:\n    pass\n";
        let mut st = state(Some("python"), &format!("|{src}"));
        let r = st.fold_regions(0, 10);
        assert_eq!(r, vec![(0, 5), (1, 4), (2, 3), (7, 8)]);
    }

    #[test]
    fn folding_hides_lines_and_moves_carets() {
        let mut st = sized(state(None, "a:\n  b\n  c|\nd\n"), 600.0);
        st.run(Action::Fold);
        assert_eq!(st.folds.len(), 1);
        assert_eq!(st.view.lines, 3);
        assert_eq!(shown(&st), "a:|\n  b\n  c\nd\n");
        // Down from the folded header skips the hidden lines.
        st.run(Action::Move(Motion::Down, false));
        assert_eq!(st.doc.buffer.line_of(st.doc.sels.primary().head), 3);
        // Editing above the fold moves it; jumping into it unfolds.
        st.doc.set_selections(Selections::single(Selection::cursor(0)));
        st.type_text("x\n");
        assert_eq!(st.folds.len(), 1);
        assert!(st.is_folded(1));
        st.run(Action::GoToLine);
        for c in "3".chars() {
            st.on_char(c as u16);
        }
        st.goto_close(true);
        assert!(st.folds.is_empty());
        assert_eq!(st.view.lines, 6);
    }

    #[test]
    fn fold_all_and_chevron_toggle() {
        let mut st = sized(state(None, "|a:\n  b\nc:\n  d\n"), 600.0);
        st.run(Action::FoldAll);
        assert_eq!(st.view.lines, 3);
        st.toggle_fold(0);
        assert_eq!(st.view.lines, 4);
        st.run(Action::UnfoldAll);
        assert_eq!(st.view.lines, 5);
    }

    #[test]
    fn word_wrap_rows_and_vertical_moves() {
        let long = "word ".repeat(40);
        let mut st = sized(state(None, &format!("|{long}\nshort")), 300.0);
        st.run(Action::ToggleWordWrap);
        assert!(st.opts.word_wrap);
        let rows = st.view.lines;
        assert!(rows > 3, "{rows} rows");
        // Down moves one row within the wrapped line.
        st.run(Action::Move(Motion::Down, false));
        let head = st.doc.sels.primary().head;
        assert_eq!(st.doc.buffer.line_of(head), 0);
        assert_eq!(st.row_of(head), 1);
        // Typing on a wrapped line rewraps it.
        st.type_text(&"z".repeat(50));
        assert!(st.view.lines > rows);
        // Hit testing finds the row's text.
        let y = st.view.line_y(1) + 2.0;
        let off = st.hit(st.view.text_x() + 1.0, y);
        assert_eq!(st.row_of(off), 1);
        st.run(Action::ToggleWordWrap);
        assert_eq!(st.view.lines, 2);
    }
}
