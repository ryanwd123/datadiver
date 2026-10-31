use std::collections::HashMap;

use crate::domain::documents::ScalarKey;
use crate::domain::slicers::{self, SlicerCard};
use crate::ui::column_completion::{ColumnCompletion, ColumnSyntax};
use crate::ui::hit::{Button, Hit, SlicerHit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Font, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::list::{Item, Palette};
use crate::ui::widgets::{ButtonStyle, Fonts, Row, WidgetId, button, button_height, button_width, colors, scroll};

#[derive(Clone, Debug, PartialEq)]
pub enum SlicerEvent {
    Select {
        column: String,
        selected: Vec<ScalarKey>,
    },
    Clear(String),
    Remove(String),
    /// A condition card's text was committed.
    Condition {
        column: String,
        text: String,
    },
}

struct Drag {
    column: String,
    start: usize,
    end: usize,
    additive: bool,
    initial: Vec<ScalarKey>,
    moved: bool,
}

/// A card's value filter, shown under its header while open.
struct CardSearch {
    column: String,
    input: Editor,
}

/// A condition card's text input.
struct ConditionInput {
    column: String,
    input: Editor,
    completion: ColumnCompletion,
    /// The card text last copied into, or committed from, the input.
    synced: String,
}

const CONDITION_PLACEHOLDER: &str = ">=10 <100 · 2024-01..2024-06 · a..m · -30d..today";
const SQL_PLACEHOLDER: &str = "amount > 100 AND region = 'North'";

#[derive(Default)]
pub struct SlicerSidebar {
    cards: Vec<SlicerCard>,
    drag: Option<Drag>,
    searches: Vec<CardSearch>,
    conditions: Vec<ConditionInput>,
    /// Column names offered after `"` in condition and SQL cards.
    columns: Vec<String>,
    dialect: diver_sql::Dialect,
    /// Each value card's list scroll, by column.
    value_scrolls: HashMap<String, (scroll::Scroll, Rect, f32)>,
    /// Each column of cards' scroll, with its area and content height.
    column_scrolls: Vec<(scroll::Scroll, Rect, f32)>,
    /// A scroll bar being dragged: a card's column, or a column of cards.
    dragging_bar: Option<SlicerHit>,
    events: Vec<SlicerEvent>,
}

impl SlicerSidebar {
    pub fn take_events(&mut self) -> Vec<SlicerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn set_columns(&mut self, columns: Vec<String>) {
        for condition in &mut self.conditions {
            condition.completion.set_columns(columns.clone());
        }
        self.columns = columns;
    }

    /// The SQL dialect SQL cards are written in.
    pub fn set_dialect(&mut self, dialect: diver_sql::Dialect) {
        for condition in &mut self.conditions {
            condition.completion.set_dialect(dialect);
        }
        self.dialect = dialect;
    }

    /// The column names listed under a card's input, while the list is open.
    #[cfg(test)]
    pub fn completion_items(&self) -> Option<Vec<String>> {
        self.conditions
            .iter()
            .find_map(|condition| condition.completion.items())
    }

    pub fn set_cards(&mut self, cards: Vec<SlicerCard>) {
        self.searches
            .retain(|search| cards.iter().any(|card| card.column == search.column));
        self.cards = cards;
        self.drag = None;
        self.sync_conditions();
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// Creates inputs for new condition cards and copies card text into inputs
    /// whose text changed elsewhere, such as Clear filters.
    fn sync_conditions(&mut self) {
        let cards = &self.cards;
        self.conditions.retain(|input| {
            cards
                .iter()
                .any(|card| card.column == input.column && card.condition.is_some())
        });
        for card in &self.cards {
            let Some(text) = &card.condition else {
                continue;
            };
            if let Some(input) = self
                .conditions
                .iter_mut()
                .find(|input| input.column == card.column)
            {
                if &input.synced != text {
                    input.synced = text.clone();
                    input.input.set_text(text.clone());
                }
                continue;
            }
            let mut input = Editor::single_line().with_placeholder(if card.sql {
                SQL_PLACEHOLDER
            } else {
                CONDITION_PLACEHOLDER
            });
            input.set_text(text.clone());
            let mut completion = ColumnCompletion::new(if card.sql {
                ColumnSyntax::Sql
            } else {
                ColumnSyntax::Search
            });
            completion.set_columns(self.columns.clone());
            completion.set_dialect(self.dialect);
            self.conditions.push(ConditionInput {
                column: card.column.clone(),
                input,
                completion,
                synced: text.clone(),
            });
        }
    }

    fn commit_condition(&mut self, column: &str) {
        let Some(input) = self
            .conditions
            .iter_mut()
            .find(|input| input.column == column)
        else {
            return;
        };
        let text = input.input.text().trim().to_owned();
        if text == input.synced {
            return;
        }
        input.synced = text.clone();
        self.events.push(SlicerEvent::Condition {
            column: column.to_owned(),
            text,
        });
    }

    fn search(&self, column: &str) -> Option<&CardSearch> {
        self.searches.iter().find(|search| search.column == column)
    }

    /// Indices of `column`'s values that pass its card filter.
    fn visible(&self, column: &str) -> Vec<usize> {
        let filter = self
            .search(column)
            .map(|search| search.input.text().to_owned())
            .unwrap_or_default();
        self.cards
            .iter()
            .find(|card| card.column == column)
            .map(|card| slicers::visible_values(card, &filter))
            .unwrap_or_default()
    }

    /// Shows or hides a card's filter; hiding it clears the filter.
    fn toggle_search(&mut self, column: &str, focus: &mut Option<WidgetId>) {
        if let Some(index) = self
            .searches
            .iter()
            .position(|search| search.column == column)
        {
            self.searches.remove(index);
            return;
        }
        let input = Editor::single_line().with_placeholder("Filter values");
        *focus = Some(input.id);
        self.searches.push(CardSearch {
            column: column.to_owned(),
            input,
        });
    }

    /// A card's text input: a condition card's text, or a value card's open filter.
    fn input_id(&self, column: &str) -> Option<WidgetId> {
        self.conditions
            .iter()
            .find(|input| input.column == column)
            .map(|input| input.input.id)
            .or_else(|| self.search(column).map(|search| search.input.id))
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        self.conditions.iter().any(|input| input.input.id == id)
            || self.searches.iter().any(|search| search.input.id == id)
    }

    /// Moves focus to the next card's input, wrapping after the last card. A
    /// value card's filter opens to take focus, and closes again when focus
    /// moves on while it is empty. Returns false when there are no cards.
    pub fn focus_next_input(&mut self, focus: &mut Option<WidgetId>) -> bool {
        if self.cards.is_empty() {
            return false;
        }
        self.sync_conditions();
        let current = self
            .cards
            .iter()
            .position(|card| self.input_id(&card.column).is_some_and(|id| Some(id) == *focus));
        let next = current.map_or(0, |index| (index + 1) % self.cards.len());
        if let Some(index) = current.filter(|&index| index != next) {
            let column = self.cards[index].column.clone();
            self.blur(focus.unwrap());
            if self.cards[index].condition.is_none()
                && self
                    .search(&column)
                    .is_some_and(|search| search.input.text().is_empty())
            {
                self.searches.retain(|search| search.column != column);
            }
        }
        let column = self.cards[next].column.clone();
        match self.input_id(&column) {
            Some(id) => *focus = Some(id),
            None => self.toggle_search(&column, focus),
        }
        true
    }

    /// Opens the first card's filter with `text`, for screenshots.
    pub fn open_first_search(&mut self, text: &str, focus: &mut Option<WidgetId>) {
        let Some(column) = self.cards.first().map(|card| card.column.clone()) else {
            return;
        };
        if self.search(&column).is_none() {
            self.toggle_search(&column, focus);
        }
        if let Some(search) = self.searches.iter_mut().find(|search| search.column == column) {
            search.input.set_text(text);
        }
    }

    fn select_visible(&mut self, column: &str) {
        let visible = self.visible(column);
        let Some(card) = self.cards.iter_mut().find(|card| card.column == column) else {
            return;
        };
        card.selected = slicers::select_visible(card, &visible);
        self.events.push(SlicerEvent::Select {
            column: card.column.clone(),
            selected: card.selected.clone(),
        });
    }

    fn press(&mut self, column: &str, index: usize, additive: bool) {
        let Some(card) = self.cards.iter_mut().find(|card| card.column == column) else {
            return;
        };
        if !card.selectable(index) {
            return;
        }
        self.drag = Some(Drag {
            column: column.into(),
            start: index,
            end: index,
            additive,
            initial: card.selected.clone(),
            moved: false,
        });
        card.selected = slicers::click_selection(card, index, additive);
    }

    fn sweep(&mut self, column: &str, index: usize) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.column != column || drag.end == index {
            return;
        }
        drag.moved = true;
        drag.end = index;
        let (start, additive, initial) = (drag.start, drag.additive, drag.initial.clone());
        let visible = self.visible(column);
        let Some(card) = self.cards.iter_mut().find(|card| card.column == column) else {
            return;
        };
        card.selected = slicers::drag_selection(card, &visible, start, index, additive, &initial);
    }

    fn release(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Some(card) = self.cards.iter().find(|card| card.column == drag.column) {
            self.events.push(SlicerEvent::Select {
                column: card.column.clone(),
                selected: card.selected.clone(),
            });
        }
    }

    /// Focus left input `id`: a condition commits its text.
    pub fn blur(&mut self, id: WidgetId) {
        let column = self
            .conditions
            .iter_mut()
            .find(|input| input.input.id == id)
            .map(|input| {
                input.completion.close();
                input.column.clone()
            });
        if let Some(column) = column {
            self.commit_condition(&column);
        }
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        let Some(id) = *focus else {
            return false;
        };
        if let Some(index) = self.conditions.iter().position(|input| input.input.id == id) {
            let condition = &mut self.conditions[index];
            let (handled, accepted) = condition.completion.key(event, &mut condition.input);
            if handled {
                if accepted {
                    condition.completion.refresh(&condition.input);
                }
                return true;
            }
            let outcome = condition.input.key(event);
            if outcome.changed || outcome.handled {
                condition.completion.refresh(&condition.input);
            }
            if outcome.submit {
                let column = condition.column.clone();
                self.commit_condition(&column);
            }
            return outcome.handled;
        }
        if let Some(index) = self.searches.iter().position(|search| search.input.id == id) {
            if event.is("escape") {
                let column = self.searches[index].column.clone();
                self.toggle_search(&column, focus);
                return true;
            }
            let outcome = self.searches[index].input.key(event);
            if outcome.submit {
                let column = self.searches[index].column.clone();
                self.select_visible(&column);
            }
            return outcome.handled;
        }
        false
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        let Some(id) = focus else {
            return false;
        };
        if let Some(condition) = self.conditions.iter_mut().find(|input| input.input.id == id) {
            condition.input.type_text(typed);
            condition.completion.refresh(&condition.input);
            return true;
        }
        if let Some(search) = self.searches.iter_mut().find(|search| search.input.id == id) {
            search.input.type_text(typed);
            return true;
        }
        false
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, modifiers: crate::ui::platform::Modifiers, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Slicer(SlicerHit::Value { column, index }) => {
                let (column, index) = (column.clone(), *index);
                self.press(&column, index, modifiers.control);
            }
            Hit::Slicer(SlicerHit::Bar(column)) => {
                let column = column.clone();
                if let Some((scroll, area, content)) = self.value_scrolls.get_mut(&column) {
                    let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
                    scroll.press(pos.y - bar.y, bar.h, area.h, *content);
                }
                self.dragging_bar = Some(SlicerHit::Bar(column));
            }
            Hit::Slicer(SlicerHit::ColumnBar(index)) => {
                if let Some((scroll, area, content)) = self.column_scrolls.get_mut(*index) {
                    let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
                    scroll.press(pos.y - bar.y, bar.h, area.h, *content);
                }
                self.dragging_bar = Some(SlicerHit::ColumnBar(*index));
            }
            Hit::Button(Button::SlicerSearch(column)) => {
                let column = column.clone();
                self.toggle_search(&column, focus);
            }
            Hit::Button(Button::SlicerClear(column)) => self.events.push(SlicerEvent::Clear(column.clone())),
            Hit::Button(Button::SlicerRemove(column)) => self.events.push(SlicerEvent::Remove(column.clone())),
            Hit::ColumnCompletion(id, index) => {
                let Some(condition) = self.conditions.iter_mut().find(|input| input.input.id == *id) else {
                    return false;
                };
                condition.completion.accept(Some(*index), &mut condition.input);
                *focus = Some(*id);
            }
            Hit::Editor(id, part) => {
                let id = *id;
                if let Some(condition) = self.conditions.iter_mut().find(|input| input.input.id == id) {
                    condition.input.mouse_down(*part, pos, clicks, modifiers.shift);
                } else if let Some(search) = self.searches.iter_mut().find(|search| search.input.id == id) {
                    search.input.mouse_down(*part, pos, clicks, modifiers.shift);
                } else {
                    return false;
                }
                *focus = Some(id);
            }
            _ => return false,
        }
        true
    }

    /// The pointer moved with the button held after a press here.
    pub fn mouse_drag(&mut self, pos: Point, hit: Option<&Hit>) {
        match &self.dragging_bar {
            Some(SlicerHit::Bar(column)) => {
                if let Some((scroll, area, content)) = self.value_scrolls.get_mut(column) {
                    let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
                    scroll.drag(pos.y - bar.y, bar.h, area.h, *content);
                }
                return;
            }
            Some(SlicerHit::ColumnBar(index)) => {
                if let Some((scroll, area, content)) = self.column_scrolls.get_mut(*index) {
                    let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
                    scroll.drag(pos.y - bar.y, bar.h, area.h, *content);
                }
                return;
            }
            _ => {}
        }
        for input in self
            .conditions
            .iter_mut()
            .map(|input| &mut input.input)
            .chain(self.searches.iter_mut().map(|search| &mut search.input))
        {
            if input.is_dragging() {
                input.mouse_drag(pos);
            }
        }
        if let Some(Hit::Slicer(SlicerHit::Value { column, index })) = hit {
            let (column, index) = (column.clone(), *index);
            self.sweep(&column, index);
        }
    }

    pub fn mouse_up(&mut self) {
        self.release();
        self.dragging_bar = None;
        for (scroll, _, _) in self.value_scrolls.values_mut() {
            scroll.release();
        }
        for (scroll, _, _) in &mut self.column_scrolls {
            scroll.release();
        }
        for input in self
            .conditions
            .iter_mut()
            .map(|input| &mut input.input)
            .chain(self.searches.iter_mut().map(|search| &mut search.input))
        {
            input.mouse_up();
        }
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, line: f32) -> bool {
        match hit {
            Hit::Slicer(SlicerHit::Value { column, .. } | SlicerHit::Values(column) | SlicerHit::Bar(column)) => {
                if let Some((scroll, area, content)) = self.value_scrolls.get_mut(column)
                    && scroll.scroll_by(-delta.y * 28.0, area.h, *content)
                {
                    return true;
                }
                self.wheel_column(0, delta, line)
            }
            Hit::Slicer(SlicerHit::Column(index) | SlicerHit::ColumnBar(index)) => self.wheel_column(*index, delta, line),
            _ => false,
        }
    }

    fn wheel_column(&mut self, index: usize, delta: Point, line: f32) -> bool {
        let Some((scroll, area, content)) = self.column_scrolls.get_mut(index) else {
            return false;
        };
        scroll.scroll_by(-delta.y * line * 2.0, area.h, *content)
    }

    pub fn width(&self, window_width: f32) -> f32 {
        240.0 * self.column_count(window_width) as f32
    }

    fn column_count(&self, window_width: f32) -> usize {
        if self.cards.len() > 1 && window_width >= 960.0 {
            2
        } else {
            1
        }
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>, window_width: f32, window_height: f32) {
        self.sync_conditions();
        canvas.fill(rect(r.right() - 1.0, r.y, 1.0, r.h), colors::BORDER);
        let column_count = self.column_count(window_width);
        let inner = rect(r.x + 8.0, r.y + 8.0, r.w - 17.0, r.h - 16.0);
        let column_width = (inner.w - 8.0 * (column_count - 1) as f32) / column_count as f32;
        self.column_scrolls.resize(column_count, (scroll::Scroll::default(), Rect::default(), 0.0));
        let line = text::line_height(&fonts.ui);
        for column in 0..column_count {
            let cards = self
                .cards
                .iter()
                .enumerate()
                .skip(column)
                .step_by(column_count)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let available = (window_height - 130.0 - cards.len() as f32 * 8.0).max(0.0);
            let counts = cards
                .iter()
                .map(|&index| {
                    let card = &self.cards[index];
                    if card.condition.is_some() {
                        // Header, input, bounds, and an error line.
                        return 2 + usize::from(card.error.is_some());
                    }
                    let search = self.search(&card.column).is_some();
                    self.visible(&card.column).len().max(usize::from(search)) + usize::from(search)
                })
                .collect::<Vec<_>>();
            let heights = card_heights(&counts, available);
            let area = rect(inner.x + column as f32 * (column_width + 8.0), inner.y, column_width, inner.h);
            let content = heights.iter().sum::<f32>() + 8.0 * heights.len().saturating_sub(1) as f32;
            let scroll_offset = {
                let (scroll, stored_area, stored_content) = &mut self.column_scrolls[column];
                *stored_area = area;
                *stored_content = content;
                scroll.clamp(area.h, content);
                scroll.offset
            };
            canvas.hit(area, Hit::Slicer(SlicerHit::Column(column)));
            canvas.push_clip(area);
            let mut y = area.y - scroll_offset;
            let card_width = if scroll::has_bar(area.h, content) { area.w - scroll::BAR_WIDTH } else { area.w };
            for (position, &index) in cards.iter().enumerate() {
                let card_rect = rect(area.x, y, card_width, heights[position]);
                if card_rect.bottom() >= area.y && card_rect.y <= area.bottom() {
                    let card = self.cards[index].clone();
                    if card.condition.is_some() {
                        self.paint_condition_card(canvas, &card, card_rect, fonts, focus);
                    } else {
                        self.paint_value_card(canvas, &card, card_rect, fonts, focus, line);
                    }
                }
                y += heights[position] + 8.0;
            }
            canvas.pop_clip();
            let bar = rect(area.right() - scroll::BAR_WIDTH, area.y, scroll::BAR_WIDTH, area.h);
            let scroll_state = self.column_scrolls[column].0;
            scroll::vertical(canvas, bar, area.h, content, &scroll_state, Hit::Slicer(SlicerHit::ColumnBar(column)));
        }
    }

    fn card_header(canvas: &mut Canvas<Hit>, card_rect: Rect, title: &str, card: &SlicerCard, search: Option<bool>, fonts: &Fonts) -> f32 {
        let small = &fonts.small;
        let height = button_height(small);
        let header = rect(card_rect.x + 8.0, card_rect.y + 4.0, card_rect.w - 16.0, height);
        let mut items = Row::new(header, 4.0);
        let remove = items.right(button_width("×", small));
        button(canvas, remove, "×", small, ButtonStyle::Outline, true, Hit::Button(Button::SlicerRemove(card.column.clone())));
        let clear = items.right(button_width("Clear", small));
        button(canvas, clear, "Clear", small, ButtonStyle::Outline, true, Hit::Button(Button::SlicerClear(card.column.clone())));
        if let Some(open) = search {
            let search_button = items.right(height);
            button(
                canvas,
                search_button,
                "⌕",
                small,
                if open { ButtonStyle::Selected } else { ButtonStyle::Outline },
                true,
                Hit::Button(Button::SlicerSearch(card.column.clone())),
            );
        }
        canvas.text(items.rest(), title, &fonts.ui.semibold(), colors::FOREGROUND, Align::Left);
        header.bottom() + 4.0 - card_rect.y
    }

    fn paint_value_card(&mut self, canvas: &mut Canvas<Hit>, card: &SlicerCard, card_rect: Rect, fonts: &Fonts, focus: Option<WidgetId>, line: f32) {
        canvas.fill_rounded(card_rect, 6.0, colors::BACKGROUND);
        canvas.stroke_rounded(card_rect, 6.0, colors::BORDER, 1.0);
        let search_open = self.search(&card.column).is_some();
        let mut top = card_rect.y + Self::card_header(canvas, card_rect, &card.column, card, Some(search_open), fonts);
        if let Some(search) = self.searches.iter_mut().find(|search| search.column == card.column) {
            let height = Editor::line_box_height(&fonts.small);
            let input = rect(card_rect.x + 8.0, top, card_rect.w - 16.0, height);
            let id = search.input.id;
            search
                .input
                .paint(canvas, input, &fonts.small, focus == Some(id), |part| Hit::Editor(id, part));
            top += height + 4.0;
        }
        let values = rect(card_rect.x + 1.0, top, card_rect.w - 2.0, (card_rect.bottom() - top - 4.0).max(0.0));
        let visible = self.visible(&card.column);
        let row_height = 28.0;
        let content = visible.len() as f32 * row_height;
        let (scroll_state, offset) = {
            let entry = self
                .value_scrolls
                .entry(card.column.clone())
                .or_insert((scroll::Scroll::default(), Rect::default(), 0.0));
            entry.1 = values;
            entry.2 = content;
            entry.0.clamp(values.h, content);
            (entry.0, entry.0.offset)
        };
        canvas.hit(values, Hit::Slicer(SlicerHit::Values(card.column.clone())));
        canvas.push_clip(values);
        let message_font = &fonts.small;
        if card.values.is_empty() && card.error.is_none() {
            canvas.text(rect(values.x + 8.0, values.y, values.w - 16.0, line), "No source values", message_font, colors::FOREGROUND, Align::Left);
        }
        if let Some(error) = &card.error {
            canvas.paragraph(rect(values.x + 8.0, values.y, values.w - 16.0, values.h), error, message_font, colors::MUTED);
        }
        if !card.values.is_empty() && visible.is_empty() {
            canvas.text(rect(values.x + 8.0, values.y, values.w - 16.0, line), "No matching values", message_font, colors::FOREGROUND, Align::Left);
        }
        let first = (offset / row_height).floor() as usize;
        let count = (values.h / row_height).ceil() as usize + 1;
        for (position, &index) in visible.iter().enumerate().skip(first).take(count) {
            let value = &card.values[index];
            let row = rect(values.x + 3.0, values.y + position as f32 * row_height - offset, values.w - 6.0 - scroll::BAR_WIDTH, row_height);
            if card.highlighted(index) {
                canvas.fill_rounded(row, 4.0, colors::SELECTED);
            } else if canvas.hovered(row) && card.selectable(index) {
                canvas.fill_rounded(row, 4.0, colors::HOVER);
            }
            canvas.text(
                row.inset_xy(8.0, 0.0),
                &value.label,
                &fonts.ui,
                if value.available { colors::FOREGROUND } else { colors::MUTED },
                Align::Left,
            );
            canvas.hit(row, Hit::Slicer(SlicerHit::Value {
                column: card.column.clone(),
                index,
            }));
        }
        canvas.pop_clip();
        let bar = rect(values.right() - scroll::BAR_WIDTH, values.y, scroll::BAR_WIDTH, values.h);
        scroll::vertical(canvas, bar, values.h, content, &scroll_state, Hit::Slicer(SlicerHit::Bar(card.column.clone())));
    }

    fn paint_condition_card(&mut self, canvas: &mut Canvas<Hit>, card: &SlicerCard, card_rect: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        canvas.fill_rounded(card_rect, 6.0, colors::BACKGROUND);
        canvas.stroke_rounded(card_rect, 6.0, colors::BORDER, 1.0);
        let title = if card.sql { "SQL filter".to_owned() } else { card.column.clone() };
        let mut top = card_rect.y + Self::card_header(canvas, card_rect, &title, card, None, fonts);
        let small = &fonts.small;
        let line = text::line_height(small);
        if let Some(condition) = self.conditions.iter_mut().find(|input| input.column == card.column) {
            let height = Editor::line_box_height(small);
            let input = rect(card_rect.x + 8.0, top, card_rect.w - 16.0, height);
            let id = condition.input.id;
            condition
                .input
                .paint(canvas, input, small, focus == Some(id), |part| Hit::Editor(id, part));
            top += height + 4.0;
        }
        if let Some((min, max)) = &card.bounds {
            canvas.text(rect(card_rect.x + 8.0, top, card_rect.w - 16.0, line), &format!("{min} … {max}"), small, colors::MUTED, Align::Left);
            top += line;
        }
        if let Some(error) = &card.error {
            let area = rect(card_rect.x + 8.0, top, card_rect.w - 16.0, (card_rect.bottom() - top).max(0.0));
            canvas.push_clip(area);
            canvas.paragraph(area, error, small, colors::DANGER);
            canvas.pop_clip();
        }
    }

    /// The `"` lists under condition inputs, drawn above the cards.
    pub fn paint_overlay(&self, canvas: &mut Canvas<Hit>, font: &Font) {
        for condition in &self.conditions {
            condition
                .completion
                .paint(canvas, condition.input.caret_rect(), font, condition.input.id);
        }
    }

    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        let id = focus?;
        self.conditions
            .iter()
            .map(|input| &input.input)
            .chain(self.searches.iter().map(|search| &search.input))
            .find(|input| input.id == id)
            .and_then(|input| input.caret_rect())
    }

    /// Text in a condition input, for tests.
    #[cfg(test)]
    pub fn condition_text(&self, column: &str) -> Option<String> {
        self.conditions
            .iter()
            .find(|input| input.column == column)
            .map(|input| input.input.text().to_owned())
    }

    #[cfg(test)]
    pub fn input_for(&self, column: &str) -> Option<WidgetId> {
        self.input_id(column)
    }
}

/// Cap short cards and distribute the remaining height by value count.
fn card_heights(counts: &[usize], available: f32) -> Vec<f32> {
    let mut heights = vec![80.0; counts.len()];
    let mut remaining = (available - 80.0 * counts.len() as f32).max(0.0);
    let mut active = (0..counts.len()).collect::<Vec<_>>();
    while remaining > 0.5 && !active.is_empty() {
        let weight = active.iter().map(|&i| counts[i].max(1)).sum::<usize>() as f32;
        let budget = remaining;
        for &i in &active {
            let capacity = (48.0 + counts[i] as f32 * 28.0 - heights[i]).max(0.0);
            let extra = (budget * counts[i].max(1) as f32 / weight).min(capacity);
            heights[i] += extra;
            remaining -= extra;
        }
        active.retain(|&i| heights[i] + 0.5 < 48.0 + counts[i] as f32 * 28.0);
    }
    heights
}

/// A slicer card the picker can add.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlicerChoice {
    pub column: String,
    pub kind: SlicerKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlicerKind {
    Values,
    Condition,
    /// A filter over the whole row; `column` is empty.
    Sql,
}

impl SlicerChoice {
    fn label(&self) -> String {
        match self.kind {
            SlicerKind::Values => self.column.clone(),
            SlicerKind::Condition => format!("{} · condition", self.column),
            SlicerKind::Sql => "SQL filter".into(),
        }
    }
}

pub struct SlicerPicker {
    pub(crate) command: Palette,
    columns: Vec<SlicerChoice>,
    pub loading: bool,
    /// Columns discovery has checked, and how many there are.
    pub progress: Option<(usize, usize)>,
    pub error: Option<String>,
}

impl Default for SlicerPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl SlicerPicker {
    pub fn new() -> Self {
        Self {
            command: Palette::new("Choose a source column"),
            columns: Vec::new(),
            loading: true,
            progress: None,
            error: None,
        }
    }

    #[cfg(test)]
    pub fn choices(&self) -> &[SlicerChoice] {
        &self.columns
    }

    /// `loading` while value discovery runs; `columns` may already list the
    /// choices that need no values.
    pub fn set_columns(&mut self, columns: Vec<SlicerChoice>, loading: bool, error: Option<String>) {
        self.command
            .set_items(columns.iter().map(|choice| Item::new(choice.label())).collect());
        self.columns = columns;
        self.error = error;
        self.loading = loading;
    }

    pub fn choice(&self, index: usize) -> Option<SlicerChoice> {
        self.columns.get(index).cloned()
    }

    pub fn message(&self) -> Option<String> {
        if self.loading {
            Some(match self.progress {
                Some((checked, total)) if total > 0 => {
                    format!("Finding columns with few enough values… {checked} of {total} checked")
                }
                _ => "Finding columns with few enough values…".into(),
            })
        } else if let Some(error) = &self.error {
            Some(error.clone())
        } else if self.columns.is_empty() {
            Some("Every column already has a slicer".into())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SlicerPicker, card_heights};

    #[test]
    fn slicer_picker_counts_checked_columns_while_loading() {
        let mut picker = SlicerPicker::new();
        assert_eq!(picker.message().unwrap(), "Finding columns with few enough values…");
        picker.progress = Some((38, 50));
        assert_eq!(picker.message().unwrap(), "Finding columns with few enough values… 38 of 50 checked");
        picker.set_columns(Vec::new(), false, None);
        assert_eq!(picker.message().unwrap(), "Every column already has a slicer");
    }

    #[test]
    fn slicer_card_heights_cap_short_cards_and_keep_oversubscribed_cards_reachable() {
        let heights = card_heights(&[2, 90], 600.0);
        assert!((80.0..=104.0).contains(&heights[0]));
        assert!(heights[1] > heights[0]);
        assert_eq!(card_heights(&[2, 90], 10_000.0), [104.0, 2568.0]);
        assert!((heights.iter().sum::<f32>() - 600.0).abs() < 0.5);
        assert_eq!(card_heights(&[1, 1, 1], 100.0), [80.0, 80.0, 80.0]);
        assert!(card_heights(&[], 600.0).is_empty());
    }
}
