//! A text editor for single-line inputs and multi-line code: selection,
//! clipboard, undo, mouse selection, SQL colors, and a completion menu.

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use super::{WidgetId, colors, scroll};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::system;
use crate::ui::platform::{Align, Canvas, Color, Cursor, Font, Point, Rect, TextOptions, point, rect, text};

/// A completion offered at the cursor. Accepting it replaces `range` with `insert`.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub insert: String,
    pub range: Range<usize>,
}

/// Offers completions for the text at a cursor.
pub trait Completer {
    fn complete(&self, text: &str, cursor: usize) -> Vec<CompletionItem>;
}

#[derive(Clone, Debug)]
pub struct CompletionMenu {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

const MENU_ROWS: usize = 12;

/// The parts of an editor that answer the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Text,
    VerticalBar,
    HorizontalBar,
    Completion(usize),
}

/// What an editor did with input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub handled: bool,
    pub changed: bool,
    /// Enter in a single-line editor.
    pub submit: bool,
}

impl Outcome {
    fn handled() -> Self {
        Self {
            handled: true,
            ..Default::default()
        }
    }

    fn changed() -> Self {
        Self {
            handled: true,
            changed: true,
            submit: false,
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
    anchor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Character,
    Word,
    Line,
}

/// Where the last paint put things, for mouse input between frames.
#[derive(Clone, Default)]
struct Geometry {
    text_area: Rect,
    line_height: f32,
    /// Top of each logical line in content coordinates, plus the end.
    line_tops: Vec<f32>,
    content_width: f32,
    caret: Option<Rect>,
    vertical_bar: Rect,
    horizontal_bar: Rect,
    menu_rows: Vec<Rect>,
}

pub struct Editor {
    pub id: WidgetId,
    text: String,
    cursor: usize,
    anchor: usize,
    goal_x: Option<f32>,
    multiline: bool,
    pub placeholder: String,
    pub masked: bool,
    pub read_only: bool,
    pub highlight_sql: bool,
    pub line_numbers: bool,
    pub soft_wrap: bool,
    /// Typing a bracket or quote also types its closing pair.
    pub auto_pairs: bool,
    /// Draws a bordered box around single-line text.
    pub framed: bool,
    pub font: Font,
    scroll_x: scroll::Scroll,
    scroll_y: scroll::Scroll,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_edit: Option<(EditKind, Instant)>,
    version: u64,
    completer: Option<Rc<dyn Completer>>,
    pub completion: Option<CompletionMenu>,
    drag: Option<Granularity>,
    drag_origin: Range<usize>,
    geometry: Geometry,
    highlight_cache: Option<(u64, Vec<(usize, usize, Color)>)>,
    /// The widest line's width, for the text version and font it was measured with.
    width_cache: Option<(u64, Font, f32)>,
    /// Reveal the cursor on the next paint, after an edit or a move.
    reveal: bool,
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn class(ch: char) -> u8 {
    if ch.is_whitespace() {
        0
    } else if is_word(ch) {
        1
    } else {
        2
    }
}

impl Editor {
    pub fn single_line() -> Self {
        Self::new(false)
    }

    pub fn multi_line() -> Self {
        Self::new(true)
    }

    fn new(multiline: bool) -> Self {
        Self {
            id: WidgetId::new(),
            text: String::new(),
            cursor: 0,
            anchor: 0,
            goal_x: None,
            multiline,
            placeholder: String::new(),
            masked: false,
            read_only: false,
            highlight_sql: false,
            line_numbers: false,
            soft_wrap: false,
            auto_pairs: false,
            framed: !multiline,
            font: Font::new(super::DEFAULT_FAMILY, 13.0),
            scroll_x: scroll::Scroll::default(),
            scroll_y: scroll::Scroll::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit: None,
            version: 0,
            completer: None,
            completion: None,
            drag: None,
            drag_origin: 0..0,
            geometry: Geometry::default(),
            highlight_cache: None,
            width_cache: None,
            reveal: false,
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// A multi-line SQL editor: colors, line numbers, and paired brackets.
    pub fn sql() -> Self {
        let mut editor = Self::multi_line();
        editor.highlight_sql = true;
        editor.line_numbers = true;
        editor.auto_pairs = true;
        editor
    }

    pub fn set_completer(&mut self, completer: Option<Rc<dyn Completer>>) {
        self.completer = completer;
    }

    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Bumps whenever the text changes.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn selected_range(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selected_range()]
    }

    /// Replaces the text, as a program change: undo history starts over.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let mut text = text.into();
        if !self.multiline {
            text = text.replace(['\r', '\n'], " ");
        }
        if text == self.text {
            return;
        }
        self.text = text;
        self.cursor = self.text.len();
        self.anchor = self.cursor;
        self.undo.clear();
        self.redo.clear();
        self.last_edit = None;
        self.completion = None;
        self.scroll_x.offset = 0.0;
        self.scroll_y.offset = 0.0;
        self.changed();
    }

    /// Selects bytes `range`, which must fall on character boundaries.
    pub fn set_selection(&mut self, range: Range<usize>) {
        let end = self.clamp_offset(range.end);
        let start = self.clamp_offset(range.start);
        self.anchor = start;
        self.cursor = end;
        self.goal_x = None;
        self.reveal = true;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.text.len();
    }

    fn clamp_offset(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn changed(&mut self) {
        self.version += 1;
        self.highlight_cache = None;
        self.reveal = true;
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            cursor: self.cursor,
            anchor: self.anchor,
        }
    }

    fn record(&mut self, kind: EditKind) {
        let now = Instant::now();
        let grouped = matches!(self.last_edit, Some((last, at)) if last == kind
            && kind != EditKind::Other
            && now.duration_since(at).as_millis() < 1000);
        if !grouped {
            self.undo.push(self.snapshot());
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit = Some((kind, now));
    }

    /// Replaces the selection with `inserted`, as typing or pasting would.
    pub fn replace_selection(&mut self, inserted: &str) {
        self.replace_range(self.selected_range(), inserted, EditKind::Other);
    }

    /// Inserts `inserted` at the cursor, replacing any selection.
    pub fn insert(&mut self, inserted: &str) {
        self.replace_selection(inserted);
    }

    /// Replaces bytes `range` with `inserted` and puts the cursor after it.
    pub fn replace(&mut self, range: Range<usize>, inserted: &str) {
        self.replace_range(range, inserted, EditKind::Other);
    }

    fn replace_range(&mut self, range: Range<usize>, inserted: &str, kind: EditKind) {
        let inserted = if self.multiline {
            inserted.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            inserted.replace("\r\n", " ").replace(['\r', '\n'], " ")
        };
        let start = self.clamp_offset(range.start);
        let end = self.clamp_offset(range.end.max(start));
        self.record(kind);
        self.text.replace_range(start..end, &inserted);
        self.cursor = start + inserted.len();
        self.anchor = self.cursor;
        self.goal_x = None;
        self.changed();
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.text = snapshot.text;
        self.cursor = self.clamp_offset(snapshot.cursor);
        self.anchor = self.clamp_offset(snapshot.anchor);
        self.last_edit = None;
        self.completion = None;
        self.changed();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(self.text.len(), |ch| offset + ch.len_utf8())
    }

    fn word_left(&self, offset: usize) -> usize {
        let mut chars = self.text[..offset].char_indices().rev().peekable();
        while let Some((_, ch)) = chars.peek() {
            if !ch.is_whitespace() {
                break;
            }
            chars.next();
        }
        let Some(&(_, first)) = chars.peek() else {
            return 0;
        };
        let kind = class(first);
        let mut start = offset;
        for (index, ch) in chars {
            if class(ch) != kind {
                break;
            }
            start = index;
        }
        if start == offset { 0 } else { start }
    }

    fn word_right(&self, offset: usize) -> usize {
        let mut chars = self.text[offset..].char_indices().peekable();
        while let Some((_, ch)) = chars.peek() {
            if !ch.is_whitespace() || *ch == '\n' && self.multiline && false {
                break;
            }
            chars.next();
        }
        let Some(&(_, first)) = chars.peek() else {
            return self.text.len();
        };
        let kind = class(first);
        for (index, ch) in chars {
            if class(ch) != kind {
                return offset + index;
            }
        }
        self.text.len()
    }

    fn word_at(&self, offset: usize) -> Range<usize> {
        let ch = self.text[offset..].chars().next().or_else(|| self.text[..offset].chars().next_back());
        let Some(ch) = ch else {
            return offset..offset;
        };
        let kind = class(ch);
        let start = self.text[..offset]
            .char_indices()
            .rev()
            .take_while(|(_, ch)| class(*ch) == kind && *ch != '\n')
            .last()
            .map_or(offset, |(index, _)| index);
        let end = self.text[offset..]
            .char_indices()
            .take_while(|(_, ch)| class(*ch) == kind && *ch != '\n')
            .last()
            .map_or(offset, |(index, ch)| offset + index + ch.len_utf8());
        start..end
    }

    fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |index| index + 1)
    }

    fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |index| offset + index)
    }

    fn line_starts(&self) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(self.text.match_indices('\n').map(|(index, _)| index + 1));
        starts
    }

    fn line_index(&self, offset: usize) -> usize {
        self.text[..offset].matches('\n').count()
    }

    fn display(&self, text: &str) -> String {
        if self.masked {
            "•".repeat(text.chars().count())
        } else {
            text.to_owned()
        }
    }

    /// Display offset of byte `offset` in `line`, which masking changes.
    fn display_offset(&self, line: &str, offset: usize) -> usize {
        if self.masked {
            line[..offset].chars().count() * '•'.len_utf8()
        } else {
            offset
        }
    }

    fn source_offset(&self, line: &str, display: usize) -> usize {
        if self.masked {
            line.char_indices()
                .nth(display / '•'.len_utf8())
                .map_or(line.len(), |(index, _)| index)
        } else {
            display.min(line.len())
        }
    }

    fn line_options(&self) -> TextOptions {
        if self.soft_wrap {
            TextOptions::wrapped(self.geometry.text_area.w.max(20.0))
        } else {
            TextOptions::default()
        }
    }

    fn line_layout(&self, line: &str) -> Rc<crate::ui::platform::TextLayout> {
        text::layout(&self.display(line), &self.font, self.line_options())
    }

    /// The caret's x, and its top in content coordinates.
    fn caret_position(&self, offset: usize) -> (f32, f32) {
        let start = self.line_start(offset);
        let end = self.line_end(offset);
        let line = &self.text[start..end];
        let layout = self.line_layout(line);
        let (x, top, _) = layout.caret(self.display_offset(line, offset - start));
        let line_index = self.line_index(offset);
        let line_top = self
            .geometry
            .line_tops
            .get(line_index)
            .copied()
            .unwrap_or(line_index as f32 * self.line_height());
        (x, line_top + top)
    }

    fn line_height(&self) -> f32 {
        if self.geometry.line_height > 0.0 {
            self.geometry.line_height
        } else {
            text::line_height(&self.font)
        }
    }

    /// The byte offset at content point (`x`, `y`).
    fn offset_at(&self, x: f32, y: f32) -> usize {
        let starts = self.line_starts();
        let line_index = if self.geometry.line_tops.len() == starts.len() + 1 {
            self.geometry
                .line_tops
                .iter()
                .skip(1)
                .position(|bottom| y < *bottom)
                .unwrap_or(starts.len() - 1)
        } else {
            ((y / self.line_height()).floor().max(0.0) as usize).min(starts.len() - 1)
        };
        let start = starts[line_index];
        let end = self.line_end(start);
        let line = &self.text[start..end];
        let layout = self.line_layout(line);
        let top = self
            .geometry
            .line_tops
            .get(line_index)
            .copied()
            .unwrap_or(0.0);
        start + self.source_offset(line, layout.hit_test(x, y - top))
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        self.cursor = self.clamp_offset(offset);
        if !extend {
            self.anchor = self.cursor;
        }
        self.reveal = true;
    }

    fn move_vertically(&mut self, lines: isize, extend: bool) {
        let (x, top) = self.caret_position(self.cursor);
        let x = *self.goal_x.get_or_insert(x);
        let target = top + self.line_height() * (lines as f32 + 0.5);
        let total = self.geometry.line_tops.last().copied().unwrap_or(0.0);
        let offset = if target < 0.0 {
            0
        } else if total > 0.0 && target >= total {
            self.text.len()
        } else {
            self.offset_at(x, target)
        };
        let goal = self.goal_x;
        self.move_to(offset, extend);
        self.goal_x = goal;
    }

    fn page_lines(&self) -> isize {
        ((self.geometry.text_area.h / self.line_height()).floor() as isize - 1).max(1)
    }

    fn delete(&mut self, range: Range<usize>) -> Outcome {
        if range.is_empty() || self.read_only {
            return Outcome::handled();
        }
        self.replace_range(range, "", EditKind::Deleting);
        Outcome::changed()
    }

    fn copy(&self) {
        let range = self.selected_range();
        if self.masked {
            return;
        }
        if range.is_empty() {
            if self.multiline {
                let start = self.line_start(self.cursor);
                let end = self.next_boundary(self.line_end(self.cursor));
                system::write_clipboard(&self.text[start..end]);
            }
            return;
        }
        system::write_clipboard(&self.text[range]);
    }

    /// Typed text, which may close a bracket pair or open the completion menu.
    pub fn type_text(&mut self, typed: &str) -> Outcome {
        if self.read_only {
            return Outcome::default();
        }
        let range = self.selected_range();
        let closing = |ch: char| match ch {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '\'' => Some('\''),
            '"' => Some('"'),
            _ => None,
        };
        let mut chars = typed.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(ch), None) => Some(ch),
            _ => None,
        };
        if self.auto_pairs
            && let Some(ch) = single
        {
            let next = self.text[self.cursor..].chars().next();
            // Typing the closing character steps over the one already there.
            if range.is_empty() && matches!(ch, ')' | ']' | '}' | '\'' | '"') && next == Some(ch) {
                self.move_to(self.cursor + ch.len_utf8(), false);
                self.refresh_completion(true);
                return Outcome::handled();
            }
            if let Some(close) = closing(ch) {
                let before = self.text[..range.start].chars().next_back();
                let quote = matches!(ch, '\'' | '"');
                let in_word = before.is_some_and(is_word) || next.is_some_and(is_word);
                let inside_string = quote && in_string(&self.text[..range.start], ch);
                if !range.is_empty() {
                    let selected = self.text[range.clone()].to_owned();
                    self.replace_range(range.clone(), &format!("{ch}{selected}{close}"), EditKind::Other);
                    self.anchor = range.start + ch.len_utf8();
                    self.cursor = self.anchor + selected.len();
                    return Outcome::changed();
                }
                if !(quote && (in_word || inside_string)) && !next.is_some_and(is_word) {
                    self.replace_range(range, &format!("{ch}{close}"), EditKind::Typing);
                    self.cursor -= close.len_utf8();
                    self.anchor = self.cursor;
                    self.refresh_completion(true);
                    return Outcome::changed();
                }
            }
        }
        self.replace_range(range, typed, EditKind::Typing);
        self.refresh_completion(true);
        Outcome::changed()
    }

    /// Asks the completer again for the text at the cursor; `open` lets a
    /// closed menu open.
    pub fn refresh_completion(&mut self, open: bool) {
        let Some(completer) = self.completer.clone() else {
            return;
        };
        if !open && self.completion.is_none() {
            return;
        }
        let items = completer.complete(&self.text, self.cursor);
        self.completion = (!items.is_empty()).then(|| CompletionMenu { items, selected: 0 });
    }

    /// Shows `items` as the completion menu, as a completer would.
    pub fn present_completion(&mut self, items: Vec<CompletionItem>) {
        self.completion = (!items.is_empty()).then(|| CompletionMenu { items, selected: 0 });
    }

    pub fn close_completion(&mut self) -> bool {
        self.completion.take().is_some()
    }

    pub fn accept_completion(&mut self, index: Option<usize>) -> bool {
        let Some(menu) = self.completion.take() else {
            return false;
        };
        let Some(item) = menu.items.get(index.unwrap_or(menu.selected)).cloned() else {
            return false;
        };
        let start = self.clamp_offset(item.range.start);
        let end = self.clamp_offset(item.range.end.max(start));
        self.replace_range(start..end, &item.insert, EditKind::Other);
        // Accepting a folder lists what is inside it.
        if item.insert.ends_with(['/', '\\']) {
            self.refresh_completion(true);
        }
        true
    }

    /// Keys the editor handles itself.
    pub fn key(&mut self, event: &KeyEvent) -> Outcome {
        let m = event.modifiers;
        let key = event.key.as_str();
        if let Some(menu) = &mut self.completion
            && !m.control
            && !m.alt
        {
            let count = menu.items.len();
            match key {
                "up" if !m.shift => {
                    menu.selected = (menu.selected + count - 1) % count;
                    return Outcome::handled();
                }
                "down" if !m.shift => {
                    menu.selected = (menu.selected + 1) % count;
                    return Outcome::handled();
                }
                "enter" | "tab" if !m.shift => {
                    self.accept_completion(None);
                    return Outcome::changed();
                }
                "escape" => {
                    self.completion = None;
                    return Outcome::handled();
                }
                _ => {}
            }
        }
        let extend = m.shift;
        let word = m.control;
        let outcome = match (key, m.control, m.alt) {
            ("left", _, false) => {
                let range = self.selected_range();
                let target = if !extend && !range.is_empty() && !word {
                    range.start
                } else if word {
                    self.word_left(self.cursor)
                } else {
                    self.previous_boundary(self.cursor)
                };
                self.goal_x = None;
                self.move_to(target, extend);
                Outcome::handled()
            }
            ("right", _, false) => {
                let range = self.selected_range();
                let target = if !extend && !range.is_empty() && !word {
                    range.end
                } else if word {
                    self.word_right(self.cursor)
                } else {
                    self.next_boundary(self.cursor)
                };
                self.goal_x = None;
                self.move_to(target, extend);
                Outcome::handled()
            }
            ("up", false, false) if self.multiline => {
                self.move_vertically(-1, extend);
                Outcome::handled()
            }
            ("down", false, false) if self.multiline => {
                self.move_vertically(1, extend);
                Outcome::handled()
            }
            ("pageup", false, false) if self.multiline => {
                self.move_vertically(-self.page_lines(), extend);
                Outcome::handled()
            }
            ("pagedown", false, false) if self.multiline => {
                self.move_vertically(self.page_lines(), extend);
                Outcome::handled()
            }
            ("home", true, false) => {
                self.goal_x = None;
                self.move_to(0, extend);
                Outcome::handled()
            }
            ("end", true, false) => {
                self.goal_x = None;
                self.move_to(self.text.len(), extend);
                Outcome::handled()
            }
            ("home", false, false) => {
                let start = self.line_start(self.cursor);
                let indent = start
                    + self.text[start..self.line_end(self.cursor)]
                        .chars()
                        .take_while(|ch| *ch == ' ' || *ch == '\t')
                        .map(char::len_utf8)
                        .sum::<usize>();
                // Home goes to the indentation first, then the line start.
                let target = if self.cursor == indent { start } else { indent };
                self.goal_x = None;
                self.move_to(target, extend);
                Outcome::handled()
            }
            ("end", false, false) => {
                self.goal_x = None;
                self.move_to(self.line_end(self.cursor), extend);
                Outcome::handled()
            }
            ("a", true, false) if !m.shift => {
                self.select_all();
                Outcome::handled()
            }
            ("c", true, false) if !m.shift => {
                self.copy();
                Outcome::handled()
            }
            ("insert", true, false) => {
                self.copy();
                Outcome::handled()
            }
            ("x", true, false) if !m.shift => {
                self.copy();
                if self.read_only || self.masked {
                    return Outcome::handled();
                }
                let range = self.selected_range();
                if range.is_empty() && self.multiline {
                    let start = self.line_start(self.cursor);
                    let end = self.next_boundary(self.line_end(self.cursor));
                    return self.delete(start..end);
                }
                self.delete(range)
            }
            ("v", true, false) if !m.shift => {
                if self.read_only {
                    return Outcome::handled();
                }
                match system::read_clipboard_text() {
                    Some(pasted) => {
                        self.replace_range(self.selected_range(), &pasted, EditKind::Other);
                        self.completion = None;
                        Outcome::changed()
                    }
                    None => Outcome::handled(),
                }
            }
            ("insert", false, false) if m.shift => {
                if let Some(pasted) = system::read_clipboard_text()
                    && !self.read_only
                {
                    self.replace_range(self.selected_range(), &pasted, EditKind::Other);
                    return Outcome::changed();
                }
                Outcome::handled()
            }
            ("z", true, false) if !m.shift => {
                let changed = !self.read_only && self.undo();
                Outcome {
                    handled: true,
                    changed,
                    submit: false,
                }
            }
            ("y", true, false) | ("z", true, false) => {
                let changed = !self.read_only && self.redo();
                Outcome {
                    handled: true,
                    changed,
                    submit: false,
                }
            }
            ("backspace", _, false) => {
                if self.read_only {
                    return Outcome::handled();
                }
                let range = self.selected_range();
                let outcome = if !range.is_empty() {
                    self.delete(range)
                } else if word {
                    self.delete(self.word_left(self.cursor)..self.cursor)
                } else {
                    let start = self.previous_boundary(self.cursor);
                    // Deleting an opening bracket also deletes its empty pair.
                    let pair = self.auto_pairs
                        && matches!(
                            (self.text[start..self.cursor].chars().next(), self.text[self.cursor..].chars().next()),
                            (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}')) | (Some('\''), Some('\'')) | (Some('"'), Some('"'))
                        );
                    let end = if pair { self.cursor + 1 } else { self.cursor };
                    self.delete(start..end)
                };
                self.refresh_completion(false);
                outcome
            }
            ("delete", _, false) if !m.shift => {
                if self.read_only {
                    return Outcome::handled();
                }
                let range = self.selected_range();
                let outcome = if !range.is_empty() {
                    self.delete(range)
                } else if word {
                    self.delete(self.cursor..self.word_right(self.cursor))
                } else {
                    self.delete(self.cursor..self.next_boundary(self.cursor))
                };
                self.refresh_completion(false);
                outcome
            }
            ("enter", false, false) if self.multiline && !m.shift => {
                if self.read_only {
                    return Outcome::default();
                }
                let start = self.line_start(self.cursor);
                let indent = self.text[start..self.cursor]
                    .chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .collect::<String>();
                self.replace_range(self.selected_range(), &format!("\n{indent}"), EditKind::Other);
                self.completion = None;
                Outcome::changed()
            }
            ("enter", false, false) if !self.multiline && !m.shift => Outcome {
                handled: true,
                changed: false,
                submit: true,
            },
            ("tab", false, false) if self.multiline && !self.read_only => {
                if m.shift {
                    self.outdent();
                } else {
                    self.indent_or_tab();
                }
                Outcome::changed()
            }
            ("]", true, false) if self.multiline && !self.read_only => {
                self.indent_lines();
                Outcome::changed()
            }
            ("[", true, false) if self.multiline && !self.read_only => {
                self.outdent();
                Outcome::changed()
            }
            _ => Outcome::default(),
        };
        if outcome.handled && !outcome.changed && self.completion.is_some() && matches!(key, "left" | "right" | "home" | "end") {
            self.refresh_completion(false);
        }
        outcome
    }

    fn selected_lines(&self) -> Vec<usize> {
        let range = self.selected_range();
        let first = self.line_start(range.start);
        let mut end = range.end;
        if end > range.start && self.text[..end].ends_with('\n') {
            end -= 1;
        }
        let mut starts = vec![first];
        starts.extend(
            self.text[first..end]
                .match_indices('\n')
                .map(|(index, _)| first + index + 1),
        );
        starts
    }

    fn indent_or_tab(&mut self) {
        let range = self.selected_range();
        if range.is_empty() || !self.text[range.clone()].contains('\n') {
            let column = self.text[self.line_start(self.cursor)..self.cursor].chars().count();
            let spaces = 4 - column % 4;
            self.replace_range(range, &" ".repeat(spaces), EditKind::Other);
        } else {
            self.indent_lines();
        }
    }

    fn indent_lines(&mut self) {
        let starts = self.selected_lines();
        self.record(EditKind::Other);
        let (mut anchor, mut cursor) = (self.anchor, self.cursor);
        for &start in starts.iter().rev() {
            self.text.insert_str(start, "    ");
            if anchor >= start {
                anchor += 4;
            }
            if cursor >= start {
                cursor += 4;
            }
        }
        self.anchor = anchor;
        self.cursor = cursor;
        self.changed();
    }

    fn outdent(&mut self) {
        let starts = self.selected_lines();
        self.record(EditKind::Other);
        let (mut anchor, mut cursor) = (self.anchor, self.cursor);
        for &start in starts.iter().rev() {
            let width = self.text[start..]
                .chars()
                .take(4)
                .take_while(|ch| *ch == ' ')
                .count();
            let width = if width == 0 && self.text[start..].starts_with('\t') { 1 } else { width };
            if width == 0 {
                continue;
            }
            self.text.replace_range(start..start + width, "");
            let shift = |offset: usize| {
                if offset >= start + width {
                    offset - width
                } else if offset > start {
                    start
                } else {
                    offset
                }
            };
            anchor = shift(anchor);
            cursor = shift(cursor);
        }
        self.anchor = anchor;
        self.cursor = cursor;
        self.changed();
    }

    /// A press on `part` at window point `pos`.
    pub fn mouse_down(&mut self, part: Part, pos: Point, clicks: u32, shift: bool) -> Outcome {
        match part {
            Part::Completion(index) => {
                self.accept_completion(Some(index));
                Outcome::changed()
            }
            Part::VerticalBar => {
                let bar = self.geometry.vertical_bar;
                let content = self.geometry.line_tops.last().copied().unwrap_or(0.0);
                self.scroll_y
                    .press(pos.y - bar.y, bar.h, self.geometry.text_area.h, content + self.line_height());
                Outcome::handled()
            }
            Part::HorizontalBar => {
                let bar = self.geometry.horizontal_bar;
                self.scroll_x.press(
                    pos.x - bar.x,
                    bar.w,
                    self.geometry.text_area.w,
                    self.geometry.content_width,
                );
                Outcome::handled()
            }
            Part::Text => {
                self.completion = None;
                let offset = self.offset_at_window(pos);
                self.goal_x = None;
                let granularity = match clicks {
                    1 => Granularity::Character,
                    2 => Granularity::Word,
                    _ => Granularity::Line,
                };
                match granularity {
                    Granularity::Character => {
                        self.move_to(offset, shift);
                        self.drag_origin = self.anchor..self.anchor;
                    }
                    Granularity::Word => {
                        let word = self.word_at(offset);
                        self.anchor = word.start;
                        self.cursor = word.end;
                        self.drag_origin = word;
                    }
                    Granularity::Line => {
                        let start = self.line_start(offset);
                        let end = self.next_boundary(self.line_end(offset));
                        self.anchor = start;
                        self.cursor = end;
                        self.drag_origin = start..end;
                    }
                }
                self.drag = Some(granularity);
                Outcome::handled()
            }
        }
    }

    fn offset_at_window(&self, pos: Point) -> usize {
        let area = self.geometry.text_area;
        self.offset_at(
            pos.x - area.x + self.scroll_x.offset,
            pos.y - area.y + self.scroll_y.offset,
        )
    }

    /// The pointer moved while this editor holds the mouse.
    pub fn mouse_drag(&mut self, pos: Point) {
        if self.scroll_y.dragging() {
            let bar = self.geometry.vertical_bar;
            let content = self.geometry.line_tops.last().copied().unwrap_or(0.0);
            self.scroll_y
                .drag(pos.y - bar.y, bar.h, self.geometry.text_area.h, content + self.line_height());
            return;
        }
        if self.scroll_x.dragging() {
            let bar = self.geometry.horizontal_bar;
            self.scroll_x
                .drag(pos.x - bar.x, bar.w, self.geometry.text_area.w, self.geometry.content_width);
            return;
        }
        let Some(granularity) = self.drag else {
            return;
        };
        let offset = self.offset_at_window(pos);
        match granularity {
            Granularity::Character => {
                self.cursor = offset;
            }
            Granularity::Word | Granularity::Line => {
                let unit = if granularity == Granularity::Word {
                    self.word_at(offset)
                } else {
                    self.line_start(offset)..self.next_boundary(self.line_end(offset))
                };
                if offset < self.drag_origin.start {
                    self.anchor = self.drag_origin.end;
                    self.cursor = unit.start;
                } else {
                    self.anchor = self.drag_origin.start;
                    self.cursor = unit.end.max(self.drag_origin.end);
                }
            }
        }
        self.reveal = true;
    }

    pub fn mouse_up(&mut self) {
        self.drag = None;
        self.scroll_x.release();
        self.scroll_y.release();
    }

    /// How far the text is scrolled down.
    #[cfg(test)]
    pub fn scroll_offset(&self) -> f32 {
        self.scroll_y.offset
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some() || self.scroll_x.dragging() || self.scroll_y.dragging()
    }

    /// Scrolls by wheel `delta` in lines. Returns whether anything moved.
    pub fn wheel(&mut self, delta: Point) -> bool {
        let line = self.line_height();
        let content = self.geometry.line_tops.last().copied().unwrap_or(0.0) + line;
        let moved_y = self
            .scroll_y
            .scroll_by(-delta.y * line, self.geometry.text_area.h, content);
        let moved_x = self.scroll_x.scroll_by(
            -delta.x * line * 2.0,
            self.geometry.text_area.w,
            self.geometry.content_width,
        );
        moved_y || moved_x
    }

    /// Where the caret was last drawn, in window coordinates.
    pub fn caret_rect(&self) -> Option<Rect> {
        self.geometry.caret
    }

    /// The height a single-line editor needs for `font`.
    pub fn line_box_height(font: &Font) -> f32 {
        (text::line_height(font) + 12.0).round()
    }

    /// Draws the editor into `r`, answering the mouse with `hit(part)`.
    pub fn paint<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, font: &Font, focused: bool, hit: impl Fn(Part) -> H) {
        self.font = font.clone();
        if self.multiline {
            self.paint_multi(canvas, r, focused, &hit);
        } else {
            self.paint_single(canvas, r, focused, &hit);
        }
    }

    fn paint_single<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, focused: bool, hit: &impl Fn(Part) -> H) {
        if self.framed {
            canvas.fill_rounded(r, 5.0, colors::BACKGROUND);
            canvas.stroke_rounded(
                r,
                5.0,
                if focused { colors::FOCUS } else { colors::INPUT_BORDER },
                1.0,
            );
        }
        let area = if self.framed { r.inset_xy(8.0, 0.0) } else { r };
        let layout = self.line_layout(&self.text);
        let line_height = layout.height.max(text::line_height(&self.font));
        self.geometry.text_area = area;
        self.geometry.line_height = line_height;
        let top = area.y + ((area.h - line_height) / 2.0).max(0.0);
        self.geometry.line_tops = vec![0.0, line_height];
        self.geometry.content_width = layout.width + 2.0;
        let display_cursor = self.display_offset(&self.text, self.cursor);
        let (caret_x, _, _) = layout.caret(display_cursor);
        if self.reveal {
            self.reveal = false;
            self.scroll_x.reveal(caret_x, 2.0, area.w);
        }
        self.scroll_x.clamp(area.w, layout.width + 2.0);
        let origin = point(area.x - self.scroll_x.offset, top);
        canvas.push_clip(area);
        if focused {
            let range = self.selected_range();
            if !range.is_empty() {
                let start = self.display_offset(&self.text, range.start);
                let end = self.display_offset(&self.text, range.end);
                for selection in layout.range_rects(start, end) {
                    canvas.fill(selection.offset(origin.x, origin.y), colors::SELECTED);
                }
            }
        }
        if self.text.is_empty() {
            let placeholder = text::layout(&self.placeholder, &self.font, TextOptions::default());
            canvas.draw_layout(&placeholder, origin, colors::MUTED);
        } else {
            canvas.draw_layout(&layout, origin, colors::FOREGROUND);
        }
        let caret = rect(origin.x + caret_x, top, 1.5, line_height);
        if focused {
            canvas.fill(caret, colors::FOREGROUND);
        }
        self.geometry.caret = Some(caret);
        canvas.pop_clip();
        canvas.hit_cursor(r, hit(Part::Text), Cursor::IBeam);
    }

    fn paint_multi<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, focused: bool, hit: &impl Fn(Part) -> H) {
        canvas.fill(r, colors::BACKGROUND);
        if self.framed {
            canvas.stroke(r, if focused { colors::FOCUS } else { colors::BORDER }, 1.0);
        }
        let inner = if self.framed { r.inset(1.0) } else { r };
        let line_height = text::line_height(&self.font);
        self.geometry.line_height = line_height;
        let starts = self.line_starts();
        let gutter = if self.line_numbers {
            text::measure(&starts.len().max(99).to_string(), &self.font).0 + 24.0
        } else {
            6.0
        };
        let bar = scroll::BAR_WIDTH;
        let area = rect(
            inner.x + gutter,
            inner.y + 4.0,
            (inner.w - gutter - bar).max(10.0),
            (inner.h - 4.0 - if self.soft_wrap { 0.0 } else { bar }).max(10.0),
        );
        self.geometry.text_area = area;
        // Line tops: every line is one line high unless it wraps.
        let mut tops = Vec::with_capacity(starts.len() + 1);
        let mut y = 0.0;
        for (index, &start) in starts.iter().enumerate() {
            tops.push(y);
            let end = starts.get(index + 1).map_or(self.text.len(), |next| next - 1);
            if self.soft_wrap {
                let layout = self.line_layout(&self.text[start..end]);
                y += layout.height.max(line_height);
            } else {
                y += line_height;
            }
        }
        tops.push(y);
        self.geometry.line_tops = tops;
        if !self.soft_wrap
            && self
                .width_cache
                .as_ref()
                .is_none_or(|(version, font, _)| *version != self.version || *font != self.font)
        {
            let widest = starts
                .iter()
                .enumerate()
                .map(|(index, &start)| {
                    let end = starts.get(index + 1).map_or(self.text.len(), |next| next - 1);
                    self.line_layout(&self.text[start..end]).width
                })
                .fold(0.0_f32, f32::max);
            self.width_cache = Some((self.version, self.font.clone(), widest + 4.0));
        }
        self.geometry.content_width = self.width_cache.as_ref().map_or(0.0, |(_, _, width)| *width);
        let content_height = y + line_height;
        if self.reveal {
            self.reveal = false;
            let (x, top) = self.caret_position(self.cursor);
            self.scroll_y.reveal(top, line_height, area.h);
            if !self.soft_wrap {
                self.scroll_x.reveal(x, 2.0, area.w);
            }
        }
        self.scroll_y.clamp(area.h, content_height);
        let first = self.geometry.line_tops.partition_point(|top| *top <= self.scroll_y.offset).saturating_sub(1);
        let visible_bottom = self.scroll_y.offset + area.h;
        let highlight = if self.highlight_sql {
            let version = self.version;
            if self.highlight_cache.as_ref().is_none_or(|(cached, _)| *cached != version) {
                self.highlight_cache = Some((version, super::sql_highlight::runs(&self.text)));
            }
            self.highlight_cache.as_ref().map(|(_, runs)| runs.clone())
        } else {
            None
        };
        let selection = self.selected_range();
        let cursor_line = self.line_index(self.cursor);
        canvas.push_clip(inner);
        self.geometry.caret = None;
        let mut index = first;
        while index < starts.len() && self.geometry.line_tops[index] < visible_bottom {
            let start = starts[index];
            let end = starts.get(index + 1).map_or(self.text.len(), |next| next - 1);
            let line = &self.text[start..end];
            let layout = self.line_layout(line);
            let top = area.y + self.geometry.line_tops[index] - self.scroll_y.offset;
            let origin = point(area.x - self.scroll_x.offset, top);
            if focused && index == cursor_line && selection.is_empty() && !self.read_only {
                canvas.fill(
                    rect(inner.x, top, inner.w, layout.height.max(line_height)),
                    Color::rgba(0x0000000a),
                );
            }
            if self.line_numbers {
                canvas.text(
                    rect(inner.x, top, gutter - 12.0, line_height),
                    &(index + 1).to_string(),
                    &self.font,
                    if index == cursor_line { colors::FOREGROUND } else { colors::MUTED },
                    Align::Right,
                );
            }
            canvas.push_clip(rect(area.x - 2.0, inner.y, area.w + 2.0, inner.h));
            if !selection.is_empty() && selection.start <= end && selection.end > start {
                let from = selection.start.max(start) - start;
                let to = selection.end.min(end) - start;
                for part in layout.range_rects(from, to) {
                    canvas.fill(part.offset(origin.x, origin.y), colors::SELECTED);
                }
                if selection.end > end && end < self.text.len() {
                    let (x, _, _) = layout.caret(line.len());
                    canvas.fill(rect(origin.x + x, top, 6.0, line_height), colors::SELECTED);
                }
            }
            match &highlight {
                Some(runs) => {
                    let line_runs = runs
                        .iter()
                        .filter(|(run_start, run_end, _)| *run_end > start && *run_start < end)
                        .map(|(run_start, run_end, color)| {
                            ((*run_start).max(start) - start, (*run_end).min(end) - start, *color)
                        })
                        .collect::<Vec<_>>();
                    canvas.draw_layout_runs(&layout, origin, colors::FOREGROUND, &line_runs);
                }
                None => canvas.draw_layout(&layout, origin, colors::FOREGROUND),
            }
            if index == cursor_line {
                let (x, caret_top, caret_height) = layout.caret(self.cursor - start);
                let caret = rect(origin.x + x, top + caret_top, 1.5, caret_height.max(line_height));
                if focused {
                    canvas.fill(caret, colors::FOREGROUND);
                }
                self.geometry.caret = Some(caret);
            }
            canvas.pop_clip();
            index += 1;
        }
        if self.text.is_empty() && !self.placeholder.is_empty() {
            canvas.text(
                rect(area.x, area.y, area.w, line_height),
                &self.placeholder,
                &self.font,
                colors::MUTED,
                Align::Left,
            );
        }
        canvas.pop_clip();
        if !self.soft_wrap {
            self.scroll_x.clamp(area.w, self.geometry.content_width);
        }
        canvas.hit_cursor(rect(inner.x, inner.y, inner.w - bar, inner.h), hit(Part::Text), Cursor::IBeam);
        let vertical = rect(inner.right() - bar, inner.y, bar, inner.h - if self.soft_wrap { 0.0 } else { bar });
        self.geometry.vertical_bar = vertical;
        scroll::vertical(canvas, vertical, area.h, content_height, &self.scroll_y, hit(Part::VerticalBar));
        if !self.soft_wrap {
            let horizontal = rect(area.x, inner.bottom() - bar, area.w, bar);
            self.geometry.horizontal_bar = horizontal;
            scroll::horizontal(
                canvas,
                horizontal,
                area.w,
                self.geometry.content_width,
                &self.scroll_x,
                hit(Part::HorizontalBar),
            );
        }
    }

    /// Draws the completion menu under the caret; call after everything the
    /// menu may cover.
    pub fn paint_completion<H: Clone>(&mut self, canvas: &mut Canvas<H>, focused: bool, hit: impl Fn(Part) -> H) {
        self.geometry.menu_rows.clear();
        let (Some(menu), Some(caret), true) = (&self.completion, self.geometry.caret, focused) else {
            return;
        };
        let font = &self.font;
        let row_height = (text::line_height(font) + 4.0).round();
        let first = menu
            .selected
            .saturating_sub(MENU_ROWS - 1)
            .min(menu.items.len().saturating_sub(MENU_ROWS));
        let shown = &menu.items[first..(first + MENU_ROWS).min(menu.items.len())];
        let width = shown
            .iter()
            .map(|item| {
                text::measure(&item.label, font).0
                    + item
                        .detail
                        .as_ref()
                        .map_or(0.0, |detail| text::measure(detail, font).0 + 24.0)
            })
            .fold(160.0_f32, f32::max)
            .min(560.0)
            + 16.0;
        let height = shown.len() as f32 * row_height + 8.0;
        let mut x = caret.x - 4.0;
        let mut y = caret.bottom() + 4.0;
        if x + width > canvas.width - 8.0 {
            x = (canvas.width - 8.0 - width).max(8.0);
        }
        if y + height > canvas.height - 8.0 {
            y = (caret.y - height - 4.0).max(8.0);
        }
        let menu_rect = rect(x, y, width, height);
        canvas.shadow(menu_rect, 6.0);
        canvas.fill_rounded(menu_rect, 6.0, colors::POPOVER);
        canvas.stroke_rounded(menu_rect, 6.0, colors::BORDER, 1.0);
        for (offset, item) in shown.iter().enumerate() {
            let index = first + offset;
            let row = rect(x + 4.0, y + 4.0 + offset as f32 * row_height, width - 8.0, row_height);
            if index == menu.selected {
                canvas.fill_rounded(row, 4.0, colors::SELECTED);
            } else if canvas.hovered(row) {
                canvas.fill_rounded(row, 4.0, colors::HOVER);
            }
            let label_width = text::measure(&item.label, font).0;
            canvas.text(row.inset_xy(6.0, 0.0), &item.label, font, colors::FOREGROUND, Align::Left);
            if let Some(detail) = &item.detail {
                let detail_area = rect(row.x + 6.0 + label_width + 16.0, row.y, (row.w - label_width - 28.0).max(0.0), row.h);
                canvas.text(detail_area, detail, font, colors::MUTED, Align::Right);
            }
            canvas.hit(row, hit(Part::Completion(index)));
            self.geometry.menu_rows.push(row);
        }
    }
}

/// Whether `before` ends inside a string opened by `quote`.
fn in_string(before: &str, quote: char) -> bool {
    let mut open = false;
    for ch in before.chars() {
        if ch == quote {
            open = !open;
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(editor: &mut Editor, stroke: &str) -> Outcome {
        editor.key(&KeyEvent::parse(stroke).unwrap())
    }

    #[test]
    fn typing_moving_and_deleting_by_words() {
        let mut editor = Editor::single_line();
        editor.type_text("hello wide world");
        assert_eq!(editor.text(), "hello wide world");
        press(&mut editor, "ctrl-left");
        assert_eq!(editor.cursor(), "hello wide ".len());
        press(&mut editor, "ctrl-backspace");
        assert_eq!(editor.text(), "hello world");
        press(&mut editor, "home");
        press(&mut editor, "shift-end");
        assert_eq!(editor.selected_text(), "hello world");
        editor.type_text("x");
        assert_eq!(editor.text(), "x");
        press(&mut editor, "ctrl-z");
        assert_eq!(editor.text(), "hello world");
        press(&mut editor, "ctrl-y");
        assert_eq!(editor.text(), "x");
        assert!(press(&mut editor, "enter").submit);
    }

    #[test]
    fn multiline_indents_pairs_and_unicode() {
        let mut editor = Editor::sql();
        editor.type_text("sum");
        editor.type_text("(");
        assert_eq!(editor.text(), "sum()");
        editor.type_text("界");
        editor.type_text(")");
        assert_eq!(editor.text(), "sum(界)");
        press(&mut editor, "home");
        editor.type_text("    ");
        press(&mut editor, "end");
        press(&mut editor, "enter");
        assert_eq!(editor.text(), "    sum(界)\n    ");
        press(&mut editor, "backspace");
        press(&mut editor, "left");
        press(&mut editor, "backspace");
        assert_eq!(editor.text(), "    sum(界)\n  ");
        editor.set_selection(0..editor.text().len());
        press(&mut editor, "shift-tab");
        assert_eq!(editor.text(), "sum(界)\n");
    }

    struct Words;
    impl Completer for Words {
        fn complete(&self, text: &str, cursor: usize) -> Vec<CompletionItem> {
            let start = text[..cursor].rfind(' ').map_or(0, |i| i + 1);
            ["apple", "apricot"]
                .iter()
                .filter(|w| w.starts_with(&text[start..cursor]) && cursor > start)
                .map(|w| CompletionItem {
                    label: w.to_string(),
                    detail: None,
                    insert: w.to_string(),
                    range: start..cursor,
                })
                .collect()
        }
    }

    #[test]
    fn completion_opens_moves_and_accepts() {
        let mut editor = Editor::single_line();
        editor.set_completer(Some(Rc::new(Words)));
        editor.type_text("a");
        assert_eq!(editor.completion.as_ref().unwrap().items.len(), 2);
        editor.type_text("pr");
        assert_eq!(editor.completion.as_ref().unwrap().items.len(), 1);
        press(&mut editor, "tab");
        assert_eq!(editor.text(), "apricot");
        assert!(editor.completion.is_none());
        editor.type_text(" a");
        press(&mut editor, "down");
        press(&mut editor, "enter");
        assert_eq!(editor.text(), "apricot apricot");
        editor.type_text(" a");
        press(&mut editor, "escape");
        assert!(editor.completion.is_none());
    }
}
