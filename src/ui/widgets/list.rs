//! Lists: a selectable list of rows, and a palette that filters a list with
//! a query typed above it.

use super::editor::{self, Editor};
use super::{WidgetId, colors, scroll};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Font, Point, Rect, rect, text};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub label: String,
    /// Shown dimmed on the right.
    pub detail: Option<String>,
    /// Extra words the filter matches.
    pub keywords: String,
    /// Index into the palette's section titles.
    pub section: usize,
    /// Drawn dimmed, such as a hidden column.
    pub muted: bool,
    /// A check mark column: `Some(true)` checked.
    pub check: Option<bool>,
    /// A second line under the label.
    pub description: Option<String>,
    /// Drawn in this font family, for the font picker's previews.
    pub preview_family: Option<String>,
}

impl Item {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn keywords(mut self, keywords: impl Into<String>) -> Self {
        self.keywords = keywords.into();
        self
    }

    pub fn section(mut self, section: usize) -> Self {
        self.section = section;
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.description = (!description.is_empty()).then_some(description);
        self
    }

    fn matches(&self, terms: &[String]) -> bool {
        if terms.is_empty() {
            return true;
        }
        let searchable = format!(
            "{} {} {}",
            self.label,
            self.keywords,
            self.detail.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        terms.iter().all(|term| searchable.contains(term))
    }
}

/// The parts of a list that answer the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Row(usize),
    Bar,
    Query(editor::Part),
}

/// What a list did with input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub handled: bool,
    /// The highlighted row moved.
    pub selected: bool,
    /// An item was chosen with Enter or a click: its index in the items.
    pub confirmed: Option<usize>,
    /// The palette's query changed.
    pub query_changed: bool,
}

/// Rows with one highlighted, scrolled to keep it in view.
pub struct List {
    pub id: WidgetId,
    items: Vec<Item>,
    sections: Vec<String>,
    /// Indices of the items shown, in order.
    visible: Vec<usize>,
    /// Position in `visible`.
    selected: Option<usize>,
    scroll: scroll::Scroll,
    row_height: f32,
    viewport: f32,
    bar: Rect,
    reveal: bool,
    pub empty_text: String,
    /// Selection follows clicks without confirming; double-click confirms.
    pub select_on_click: bool,
}

impl Default for List {
    fn default() -> Self {
        Self::new()
    }
}

impl List {
    pub fn new() -> Self {
        Self {
            id: WidgetId::new(),
            items: Vec::new(),
            sections: Vec::new(),
            visible: Vec::new(),
            selected: None,
            scroll: scroll::Scroll::default(),
            row_height: 28.0,
            viewport: 0.0,
            bar: Rect::default(),
            reveal: false,
            empty_text: String::new(),
            select_on_click: false,
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn set_sections(&mut self, sections: Vec<String>) {
        self.sections = sections;
    }

    /// Replaces the items, keeping the highlighted item when it is still there.
    pub fn set_items(&mut self, items: Vec<Item>) {
        let previous = self.selected_item().and_then(|index| self.items.get(index)).map(|item| item.label.clone());
        self.items = items;
        self.visible = (0..self.items.len()).collect();
        self.selected = previous
            .and_then(|label| self.visible.iter().position(|&index| self.items[index].label == label))
            .or_else(|| (!self.visible.is_empty()).then_some(0));
        self.reveal = true;
    }

    /// Shows only the items matching every word of `query`.
    pub fn filter(&mut self, query: &str) {
        let terms = query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        self.visible = (0..self.items.len())
            .filter(|&index| self.items[index].matches(&terms))
            .collect();
        self.selected = (!self.visible.is_empty()).then_some(0);
        self.scroll.offset = 0.0;
    }

    /// The highlighted item's index in the items.
    pub fn selected_item(&self) -> Option<usize> {
        self.selected.and_then(|position| self.visible.get(position).copied())
    }

    /// Highlights item `index`, if it is shown.
    pub fn select_item(&mut self, index: Option<usize>) {
        self.selected = index.and_then(|index| self.visible.iter().position(|&shown| shown == index));
        self.reveal = true;
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn visible_items(&self) -> &[usize] {
        &self.visible
    }

    fn page(&self) -> usize {
        ((self.viewport / self.row_height).floor() as usize).max(1)
    }

    fn move_selection(&mut self, to: usize) -> Outcome {
        if self.visible.is_empty() {
            return Outcome {
                handled: true,
                ..Default::default()
            };
        }
        let to = to.min(self.visible.len() - 1);
        let moved = self.selected != Some(to);
        self.selected = Some(to);
        self.reveal = true;
        Outcome {
            handled: true,
            selected: moved,
            ..Default::default()
        }
    }

    /// Up, Down, Page Up/Down, Home, End, and Enter.
    pub fn key(&mut self, event: &KeyEvent, home_end: bool) -> Outcome {
        if event.modifiers.control || event.modifiers.alt {
            return Outcome::default();
        }
        let current = self.selected.unwrap_or(0);
        let last = self.visible.len().saturating_sub(1);
        match event.key.as_str() {
            "up" if !event.modifiers.shift => self.move_selection(match self.selected {
                None => 0,
                Some(0) => last,
                Some(current) => current - 1,
            }),
            "down" if !event.modifiers.shift => self.move_selection(match self.selected {
                Some(current) if current < last => current + 1,
                _ => 0,
            }),
            "pageup" => self.move_selection(current.saturating_sub(self.page())),
            "pagedown" => self.move_selection((current + self.page()).min(last)),
            "home" if home_end => self.move_selection(0),
            "end" if home_end => self.move_selection(last),
            "enter" if !event.modifiers.shift => Outcome {
                handled: true,
                confirmed: self.selected_item(),
                ..Default::default()
            },
            _ => Outcome::default(),
        }
    }

    pub fn mouse_down(&mut self, part: Part, pos: Point, clicks: u32) -> Outcome {
        match part {
            Part::Row(position) => {
                if position >= self.visible.len() {
                    return Outcome::default();
                }
                self.selected = Some(position);
                let confirm = !self.select_on_click || clicks >= 2;
                Outcome {
                    handled: true,
                    selected: true,
                    confirmed: confirm.then(|| self.visible[position]),
                    query_changed: false,
                }
            }
            Part::Bar => {
                let content = self.visible.len() as f32 * self.row_height;
                self.scroll.press(pos.y - self.bar.y, self.bar.h, self.viewport, content);
                Outcome {
                    handled: true,
                    ..Default::default()
                }
            }
            Part::Query(_) => Outcome::default(),
        }
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        let content = self.visible.len() as f32 * self.row_height;
        self.scroll.drag(pos.y - self.bar.y, self.bar.h, self.viewport, content);
    }

    pub fn mouse_up(&mut self) {
        self.scroll.release();
    }

    pub fn wheel(&mut self, delta: Point) -> bool {
        let content = self.visible.len() as f32 * self.row_height;
        self.scroll
            .scroll_by(-delta.y * self.row_height, self.viewport, content)
    }

    /// The height rows need, up to `max`.
    pub fn content_height(&self, font: &Font) -> f32 {
        let row = Self::row_height_for(font, self.items.iter().any(|item| item.description.is_some()));
        let sections = self.section_breaks();
        self.visible.len() as f32 * row + sections as f32 * row
    }

    fn row_height_for(font: &Font, two_lines: bool) -> f32 {
        let line = text::line_height(font);
        if two_lines {
            (line * 2.0 + 8.0).round()
        } else {
            (line + 10.0).round()
        }
    }

    /// How many section titles show between the visible rows.
    fn section_breaks(&self) -> usize {
        if self.sections.is_empty() {
            return 0;
        }
        let mut count = 0;
        let mut last = None;
        for &index in &self.visible {
            let section = self.items[index].section;
            if last != Some(section) {
                count += 1;
                last = Some(section);
            }
        }
        count
    }

    /// Draws the rows in `r`. `focused` draws the highlight in the strong color.
    pub fn paint<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, font: &Font, focused: bool, hit: impl Fn(Part) -> H) {
        let two_lines = self.items.iter().any(|item| item.description.is_some());
        self.row_height = Self::row_height_for(font, two_lines);
        self.viewport = r.h;
        // Section titles take a row each; they are laid out with the items.
        let mut rows: Vec<(Option<usize>, Option<usize>)> = Vec::new();
        let mut last_section = None;
        for (position, &index) in self.visible.iter().enumerate() {
            let section = self.items[index].section;
            if !self.sections.is_empty() && last_section != Some(section) {
                rows.push((None, Some(section)));
                last_section = Some(section);
            }
            rows.push((Some(position), None));
        }
        let content = rows.len() as f32 * self.row_height;
        if self.reveal {
            self.reveal = false;
            if let Some(selected) = self.selected
                && let Some(row) = rows.iter().position(|(position, _)| *position == Some(selected))
            {
                let title_above = row > 0 && rows[row - 1].1.is_some() && selected == 0;
                let top = if title_above { (row - 1) as f32 } else { row as f32 } * self.row_height;
                let height = if title_above { 2.0 } else { 1.0 } * self.row_height;
                self.scroll.reveal(top, height, r.h);
            }
        }
        self.scroll.clamp(r.h, content);
        let bar_width = if scroll::has_bar(r.h, content) { scroll::BAR_WIDTH } else { 0.0 };
        let rows_area = rect(r.x, r.y, r.w - bar_width, r.h);
        canvas.push_clip(r);
        if self.visible.is_empty() && !self.empty_text.is_empty() {
            canvas.text(
                rect(r.x + 8.0, r.y, r.w - 16.0, self.row_height),
                &self.empty_text,
                font,
                colors::MUTED,
                Align::Left,
            );
        }
        let first = (self.scroll.offset / self.row_height).floor().max(0.0) as usize;
        let count = (r.h / self.row_height).ceil() as usize + 1;
        for (row_index, (position, section)) in rows.iter().enumerate().skip(first).take(count) {
            let y = r.y + row_index as f32 * self.row_height - self.scroll.offset;
            let row = rect(rows_area.x, y, rows_area.w, self.row_height);
            if let Some(section) = section {
                let title = self.sections.get(*section).cloned().unwrap_or_default();
                canvas.text(
                    rect(row.x + 8.0, row.y + 4.0, row.w - 16.0, row.h - 4.0),
                    &title,
                    &font.sized((font.size * 0.85).round()),
                    colors::MUTED,
                    Align::Left,
                );
                continue;
            }
            let Some(position) = *position else {
                continue;
            };
            let item = &self.items[self.visible[position]];
            let inner = row.inset_xy(4.0, 1.0);
            if self.selected == Some(position) {
                canvas.fill_rounded(inner, 4.0, if focused { colors::SELECTED } else { colors::HOVER });
            } else if canvas.hovered(inner) {
                canvas.fill_rounded(inner, 4.0, colors::HOVER);
            }
            let mut x = inner.x + 8.0;
            if let Some(checked) = item.check {
                canvas.text(
                    rect(x, inner.y, 16.0, inner.h),
                    if checked { "✓" } else { "" },
                    font,
                    colors::FOREGROUND,
                    Align::Left,
                );
                x += 22.0;
            }
            let color = if item.muted { colors::MUTED } else { colors::FOREGROUND };
            let detail_width = item
                .detail
                .as_ref()
                .map_or(0.0, |detail| (text::measure(detail, font).0 + 12.0).min(inner.w * 0.5));
            let label_area = rect(x, inner.y, (inner.right() - x - detail_width - 8.0).max(0.0), inner.h);
            if let Some(description) = &item.description {
                let line = text::line_height(font);
                canvas.text(rect(label_area.x, inner.y + 3.0, label_area.w, line), &item.label, font, color, Align::Left);
                canvas.text(
                    rect(label_area.x, inner.y + 3.0 + line, label_area.w, line),
                    description,
                    font,
                    colors::MUTED,
                    Align::Left,
                );
            } else if let Some(family) = &item.preview_family {
                let label_width = 180.0_f32.min(label_area.w * 0.4);
                canvas.text(rect(label_area.x, inner.y, label_width, inner.h), &item.label, font, color, Align::Left);
                let preview = font.family(family.as_str());
                canvas.text(
                    rect(label_area.x + label_width + 12.0, inner.y, (label_area.w - label_width - 12.0).max(0.0), inner.h),
                    &format!("123 The quick brown fox — {}", item.label),
                    &preview,
                    color,
                    Align::Left,
                );
            } else {
                canvas.text(label_area, &item.label, font, color, Align::Left);
            }
            if let Some(detail) = &item.detail {
                canvas.text(
                    rect(inner.right() - detail_width - 8.0, inner.y, detail_width, inner.h),
                    detail,
                    font,
                    colors::MUTED,
                    Align::Right,
                );
            }
            canvas.hit(row, hit(Part::Row(position)));
        }
        canvas.pop_clip();
        self.bar = rect(r.right() - scroll::BAR_WIDTH, r.y, scroll::BAR_WIDTH, r.h);
        scroll::vertical(canvas, self.bar, r.h, content, &self.scroll, hit(Part::Bar));
    }
}

/// A list filtered by a query typed above it, like the command palette.
pub struct Palette {
    pub query: Editor,
    pub list: List,
    /// Filters the items by the query; off when the owner filters them.
    pub filterable: bool,
    last_query: u64,
}

impl Palette {
    pub fn new(placeholder: &str) -> Self {
        let mut query = Editor::single_line();
        query.placeholder = placeholder.into();
        Self {
            query,
            list: List::new(),
            filterable: true,
            last_query: 0,
        }
    }

    pub fn set_items(&mut self, items: Vec<Item>) {
        self.list.set_items(items);
        if self.filterable && !self.query.text().is_empty() {
            let previous = self.list.selected_item();
            self.list.filter(self.query.text());
            if previous.is_some() {
                self.list.select_item(previous);
                if self.list.selected_item().is_none() {
                    self.list.selected = (!self.list.visible.is_empty()).then_some(0);
                }
            }
        }
    }

    pub fn query_text(&self) -> &str {
        self.query.text()
    }

    pub fn set_query(&mut self, query: &str) {
        self.query.set_text(query);
        self.query_changed();
    }

    fn query_changed(&mut self) -> bool {
        if self.query.version() == self.last_query {
            return false;
        }
        self.last_query = self.query.version();
        if self.filterable {
            self.list.filter(self.query.text());
        }
        true
    }

    pub fn selected_item(&self) -> Option<usize> {
        self.list.selected_item()
    }

    /// Keys typed while the palette's query has focus.
    pub fn key(&mut self, event: &KeyEvent) -> Outcome {
        // Escape clears a typed query first; a second Escape closes.
        if event.is("escape") && !self.query.text().is_empty() {
            self.query.set_text("");
            return Outcome {
                handled: true,
                query_changed: self.query_changed(),
                ..Default::default()
            };
        }
        let outcome = self.list.key(event, false);
        if outcome.handled {
            return outcome;
        }
        let edit = self.query.key(event);
        Outcome {
            handled: edit.handled,
            query_changed: edit.changed && self.query_changed(),
            ..Default::default()
        }
    }

    pub fn type_text(&mut self, typed: &str) -> Outcome {
        let edit = self.query.type_text(typed);
        Outcome {
            handled: edit.handled,
            query_changed: edit.changed && self.query_changed(),
            ..Default::default()
        }
    }

    pub fn mouse_down(&mut self, part: Part, pos: Point, clicks: u32, shift: bool) -> Outcome {
        match part {
            Part::Query(part) => {
                self.query.mouse_down(part, pos, clicks, shift);
                Outcome {
                    handled: true,
                    ..Default::default()
                }
            }
            part => self.list.mouse_down(part, pos, clicks),
        }
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        if self.query.is_dragging() {
            self.query.mouse_drag(pos);
        } else {
            self.list.mouse_drag(pos);
        }
    }

    pub fn mouse_up(&mut self) {
        self.query.mouse_up();
        self.list.mouse_up();
    }

    /// The height the palette needs for its query and up to `max_rows` height of rows.
    pub fn height(&self, font: &Font, max_list: f32) -> f32 {
        Editor::line_box_height(font) + 8.0 + self.list.content_height(font).clamp(List::row_height_for(font, false), max_list.max(40.0))
    }

    /// Draws the query box at the top of `r` and the rows below it.
    pub fn paint<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, font: &Font, focused: bool, hit: impl Fn(Part) -> H) {
        let input_height = Editor::line_box_height(font);
        let input = rect(r.x, r.y, r.w, input_height);
        self.query.paint(canvas, input, font, focused, |part| hit(Part::Query(part)));
        let list = rect(r.x, r.y + input_height + 8.0, r.w, (r.h - input_height - 8.0).max(0.0));
        self.list.paint(canvas, list, font, true, hit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(palette: &mut Palette, stroke: &str) -> Outcome {
        palette.key(&KeyEvent::parse(stroke).unwrap())
    }

    #[test]
    fn filters_by_every_term_and_confirms_original_indices() {
        let mut palette = Palette::new("Find");
        palette.set_items(vec![
            Item::new("Order By"),
            Item::new("Reload").keywords("reload"),
            Item::new("Reload and reset").keywords("reload_reset"),
        ]);
        palette.type_text("reset");
        assert_eq!(palette.list.visible_items(), [2]);
        assert_eq!(press(&mut palette, "enter").confirmed, Some(2));
        palette.set_query("");
        assert_eq!(palette.list.visible_count(), 3);
        press(&mut palette, "up");
        assert_eq!(palette.selected_item(), Some(2));
        press(&mut palette, "down");
        assert_eq!(palette.selected_item(), Some(0));
    }
}
