use crate::domain::documents::SortKey;
use crate::ui::hit::Hit;
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, rect, text};
use crate::ui::widgets::list::{self, Item, List, Palette};
use crate::ui::widgets::{Column, Fonts, Row, WidgetId, colors};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderByEdit {
    Add(String),
    Remove(String),
    Move { column: String, direction: i32 },
    ToggleDirection(String),
    SetDirection { column: String, descending: bool },
}

pub struct OrderByPanel {
    columns: Vec<String>,
    configured: Vec<SortKey>,
    pub(crate) command: Palette,
    pub(crate) list: List,
}

fn key_label(key: &SortKey) -> String {
    let direction = if key.descending {
        "Descending"
    } else {
        "Ascending"
    };
    format!("{} · {direction}", key.column)
}

impl OrderByPanel {
    pub fn new(columns: Vec<String>, configured: Vec<SortKey>) -> Self {
        let mut panel = Self {
            columns,
            configured: Vec::new(),
            command: Palette::new("Find a column"),
            list: List::new(),
        };
        panel.command.list.empty_text = "No available columns".into();
        panel.list.empty_text = "No sort keys configured".into();
        panel.list.select_on_click = true;
        panel.set_configured(configured);
        panel.list.select_item(None);
        panel
    }

    fn available(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| !self.configured.iter().any(|key| &key.column == *column))
            .cloned()
            .collect()
    }

    pub fn set_configured(&mut self, keys: Vec<SortKey>) {
        let selected = self
            .list
            .selected_item()
            .and_then(|index| self.configured.get(index).map(|key| key.column.clone()));
        self.configured = keys;
        self.list
            .set_items(self.configured.iter().map(|key| Item::new(key_label(key))).collect());
        let next = selected.and_then(|column| self.configured.iter().position(|key| key.column == column));
        self.list.select_item(next);
        let available = self.available().into_iter().map(Item::new).collect();
        self.command.set_items(available);
    }

    pub fn focus_available(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.command.query.id);
    }

    fn selected_key(&self) -> Option<SortKey> {
        self.list
            .selected_item()
            .and_then(|index| self.configured.get(index).cloned())
    }

    fn switch_pane(&mut self, focus: &mut Option<WidgetId>) {
        if *focus == Some(self.list.id) {
            self.focus_available(focus);
        } else {
            let first = (!self.configured.is_empty()).then_some(0);
            self.list.select_item(first);
            *focus = Some(self.list.id);
        }
    }

    fn add(&self, index: Option<usize>) -> Option<OrderByEdit> {
        let available = self.available();
        index
            .and_then(|index| available.get(index))
            .map(|column| OrderByEdit::Add(column.clone()))
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        id == self.command.query.id || id == self.list.id
    }

    /// Returns whether the key was used, and an edit it asks for.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> (bool, Option<OrderByEdit>) {
        let key = event.key.as_str();
        if key == "tab" && !event.modifiers.control && !event.modifiers.alt {
            self.switch_pane(focus);
            return (true, None);
        }
        if *focus == Some(self.list.id) {
            if let Some(sort_key) = self.selected_key() {
                let edit = match key {
                    "enter" if !event.modifiers.modified() => Some(OrderByEdit::ToggleDirection(sort_key.column)),
                    "delete" if !event.modifiers.modified() => Some(OrderByEdit::Remove(sort_key.column)),
                    "left" | "right" if !event.modifiers.modified() => Some(OrderByEdit::SetDirection {
                        column: sort_key.column,
                        descending: key == "right",
                    }),
                    "[" | "]" if !event.modifiers.modified() => Some(OrderByEdit::SetDirection {
                        column: sort_key.column,
                        descending: key == "]",
                    }),
                    "up" | "down" if event.modifiers.alt => Some(OrderByEdit::Move {
                        column: sort_key.column,
                        direction: if key == "up" { -1 } else { 1 },
                    }),
                    _ => None,
                };
                if edit.is_some() {
                    return (true, edit);
                }
            }
            let outcome = self.list.key(event, true);
            return (outcome.handled, None);
        }
        let outcome = self.command.key(event);
        if outcome.confirmed.is_some() {
            return (true, self.add(outcome.confirmed));
        }
        (outcome.handled, None)
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus == Some(self.command.query.id) {
            self.command.type_text(typed);
            return true;
        }
        false
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, focus: &mut Option<WidgetId>) -> Option<OrderByEdit> {
        match hit {
            Hit::List(id, part) if *id == self.command.list.id || *id == self.command.query.id => {
                let part = *part;
                let outcome = self.command.mouse_down(part, pos, clicks, false);
                self.focus_available(focus);
                self.add(outcome.confirmed)
            }
            Hit::Editor(id, part) if *id == self.command.query.id => {
                self.command.query.mouse_down(*part, pos, clicks, false);
                self.focus_available(focus);
                None
            }
            Hit::List(id, part) if *id == self.list.id => {
                let outcome = self.list.mouse_down(*part, pos, clicks);
                *focus = Some(self.list.id);
                outcome
                    .confirmed
                    .and_then(|index| self.configured.get(index))
                    .map(|key| OrderByEdit::ToggleDirection(key.column.clone()))
            }
            _ => None,
        }
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        self.command.mouse_drag(pos);
        self.list.mouse_drag(pos);
    }

    pub fn mouse_up(&mut self) {
        self.command.mouse_up();
        self.list.mouse_up();
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point) -> bool {
        match hit {
            Hit::List(id, _) if *id == self.command.list.id => self.command.list.wheel(delta),
            Hit::List(id, _) if *id == self.list.id => self.list.wheel(delta),
            _ => false,
        }
    }

    /// The caret of the focused input, for the window's input method.
    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        self.command.query.caret_rect().filter(|_| focus == Some(self.command.query.id))
    }

    /// Paints the manager to fill `r`.
    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let mut row = Row::new(r, 12.0);
        let half = (r.w - 12.0) / 2.0;
        let left = row.left(half);
        let right = row.left(half);
        let line = text::line_height(&fonts.ui);
        let list_height = (r.h - line * 3.0 - 24.0).max(100.0);
        let title_font = fonts.ui.semibold();
        let mut column = Column::new(left, 8.0);
        canvas.text(column.next(line), "Available columns", &title_font, colors::FOREGROUND, Align::Left);
        let command_area = column.rest();
        let command_focused = focus == Some(self.command.query.id);
        let query_id = self.command.query.id;
        let list_id = self.command.list.id;
        self.command.paint(canvas, command_area, &fonts.ui, command_focused, |part| match part {
            list::Part::Query(part) => Hit::Editor(query_id, part),
            part => Hit::List(list_id, part),
        });
        let mut column = Column::new(right, 8.0);
        canvas.text(column.next(line), "Sort priority", &title_font, colors::FOREGROUND, Align::Left);
        let list_area = column.next(list_height);
        canvas.stroke_rounded(list_area, 5.0, colors::BORDER, 1.0);
        let list_id = self.list.id;
        let focused = focus == Some(self.list.id);
        self.list
            .paint(canvas, list_area.inset(2.0), &fonts.ui, focused, |part| Hit::List(list_id, part));
        let hint = column.rest();
        canvas.paragraph(
            rect(hint.x, hint.y, hint.w, hint.h),
            "[ / ← ascending · ] / → descending · Alt+↑/↓ changes priority · Delete removes",
            &fonts.small,
            colors::MUTED,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_edit_the_selected_sort_key() {
        let mut panel = OrderByPanel::new(
            vec!["a".into(), "b".into(), "c".into()],
            vec![SortKey {
                column: "b".into(),
                descending: false,
            }],
        );
        let mut focus = None;
        panel.focus_available(&mut focus);
        assert_eq!(
            panel.key(&KeyEvent::parse("enter").unwrap(), &mut focus).1,
            Some(OrderByEdit::Add("a".into()))
        );
        panel.key(&KeyEvent::parse("tab").unwrap(), &mut focus);
        assert_eq!(focus, Some(panel.list.id));
        assert_eq!(
            panel.key(&KeyEvent::parse("]").unwrap(), &mut focus).1,
            Some(OrderByEdit::SetDirection {
                column: "b".into(),
                descending: true
            })
        );
        assert_eq!(
            panel.key(&KeyEvent::parse("alt-up").unwrap(), &mut focus).1,
            Some(OrderByEdit::Move {
                column: "b".into(),
                direction: -1
            })
        );
    }
}
