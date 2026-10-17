//! A tree of folders and leaves with keyboard and mouse navigation.

use super::{WidgetId, colors, scroll};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Font, Point, Rect, rect, text};

#[derive(Clone, Debug, PartialEq)]
pub struct TreeItem {
    pub id: String,
    pub label: String,
    pub children: Vec<TreeItem>,
    expanded: bool,
}

impl TreeItem {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            children: Vec::new(),
            expanded: false,
        }
    }

    pub fn children(mut self, children: Vec<TreeItem>) -> Self {
        self.children = children;
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn is_folder(&self) -> bool {
        !self.children.is_empty()
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }
}

/// A visible row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub depth: usize,
    pub folder: bool,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Row(usize),
    Bar,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub handled: bool,
    /// A folder to open (`true`) or close.
    pub toggle: Option<(String, bool)>,
    /// A leaf was clicked.
    pub clicked: bool,
}

pub struct Tree {
    pub id: WidgetId,
    entries: Vec<Entry>,
    selected: Option<usize>,
    scroll: scroll::Scroll,
    row_height: f32,
    viewport: f32,
    bar: Rect,
    reveal: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

fn flatten(items: &[TreeItem], depth: usize, entries: &mut Vec<Entry>) {
    for item in items {
        entries.push(Entry {
            id: item.id.clone(),
            label: item.label.clone(),
            depth,
            folder: item.is_folder(),
            expanded: item.expanded,
        });
        if item.expanded {
            flatten(&item.children, depth + 1, entries);
        }
    }
}

impl Tree {
    pub fn new() -> Self {
        Self {
            id: WidgetId::new(),
            entries: Vec::new(),
            selected: None,
            scroll: scroll::Scroll::default(),
            row_height: 24.0,
            viewport: 0.0,
            bar: Rect::default(),
            reveal: false,
        }
    }

    pub fn set_items(&mut self, items: Vec<TreeItem>) {
        self.entries.clear();
        flatten(&items, 0, &mut self.entries);
        if self.selected.is_some_and(|index| index >= self.entries.len()) {
            self.selected = None;
        }
    }

    pub fn entry(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.selected.and_then(|index| self.entries.get(index))
    }

    pub fn set_selected_index(&mut self, index: Option<usize>) {
        self.selected = index.filter(|index| *index < self.entries.len());
        self.reveal = true;
    }

    /// Up, Down, Home, End, Page keys, and Left/Right to close and open folders.
    pub fn key(&mut self, event: &KeyEvent) -> Outcome {
        if event.modifiers.modified() {
            return Outcome::default();
        }
        let last = self.entries.len().saturating_sub(1);
        let page = ((self.viewport / self.row_height).floor() as usize).max(1);
        let current = self.selected;
        let select = |tree: &mut Self, index: usize| {
            if !tree.entries.is_empty() {
                tree.set_selected_index(Some(index.min(last)));
            }
            Outcome {
                handled: true,
                ..Default::default()
            }
        };
        match event.key.as_str() {
            "up" => select(self, current.map_or(0, |index| index.saturating_sub(1))),
            "down" => select(self, current.map_or(0, |index| (index + 1).min(last))),
            "home" => select(self, 0),
            "end" => select(self, last),
            "pageup" => select(self, current.unwrap_or(0).saturating_sub(page)),
            "pagedown" => select(self, current.unwrap_or(0) + page),
            "right" => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return Outcome::default();
                };
                if entry.folder && !entry.expanded {
                    return Outcome {
                        handled: true,
                        toggle: Some((entry.id, true)),
                        clicked: false,
                    };
                }
                if entry.folder {
                    return select(self, current.unwrap_or(0) + 1);
                }
                Outcome {
                    handled: true,
                    ..Default::default()
                }
            }
            "left" => {
                let Some(entry) = self.selected_entry().cloned() else {
                    return Outcome::default();
                };
                if entry.folder && entry.expanded {
                    return Outcome {
                        handled: true,
                        toggle: Some((entry.id, false)),
                        clicked: false,
                    };
                }
                Outcome::default()
            }
            _ => Outcome::default(),
        }
    }

    pub fn mouse_down(&mut self, part: Part, pos: Point) -> Outcome {
        match part {
            Part::Row(index) => {
                let Some(entry) = self.entries.get(index).cloned() else {
                    return Outcome::default();
                };
                self.selected = Some(index);
                if entry.folder {
                    Outcome {
                        handled: true,
                        toggle: Some((entry.id, !entry.expanded)),
                        clicked: false,
                    }
                } else {
                    Outcome {
                        handled: true,
                        toggle: None,
                        clicked: true,
                    }
                }
            }
            Part::Bar => {
                let content = self.entries.len() as f32 * self.row_height;
                self.scroll.press(pos.y - self.bar.y, self.bar.h, self.viewport, content);
                Outcome {
                    handled: true,
                    ..Default::default()
                }
            }
        }
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        let content = self.entries.len() as f32 * self.row_height;
        self.scroll.drag(pos.y - self.bar.y, self.bar.h, self.viewport, content);
    }

    pub fn mouse_up(&mut self) {
        self.scroll.release();
    }

    pub fn wheel(&mut self, delta: Point) -> bool {
        let content = self.entries.len() as f32 * self.row_height;
        self.scroll
            .scroll_by(-delta.y * self.row_height, self.viewport, content)
    }

    pub fn paint<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, font: &Font, focused: bool, hit: impl Fn(Part) -> H) {
        self.row_height = (text::line_height(font) + 6.0).round();
        self.viewport = r.h;
        let content = self.entries.len() as f32 * self.row_height;
        if self.reveal {
            self.reveal = false;
            if let Some(index) = self.selected {
                self.scroll.reveal(index as f32 * self.row_height, self.row_height, r.h);
            }
        }
        self.scroll.clamp(r.h, content);
        canvas.push_clip(r);
        let first = (self.scroll.offset / self.row_height).floor() as usize;
        let count = (r.h / self.row_height).ceil() as usize + 1;
        let bold = font.semibold();
        for (index, entry) in self.entries.iter().enumerate().skip(first).take(count) {
            let y = r.y + index as f32 * self.row_height - self.scroll.offset;
            let row = rect(r.x, y, r.w - scroll::BAR_WIDTH, self.row_height);
            if self.selected == Some(index) {
                canvas.fill(row, if focused { colors::SELECTED } else { colors::HOVER });
            } else if canvas.hovered(row) {
                canvas.fill(row, colors::HOVER);
            }
            let x = row.x + 6.0 + entry.depth as f32 * 12.0;
            let chevron = match (entry.folder, entry.expanded) {
                (true, true) => "▾",
                (true, false) => "▸",
                (false, _) => "",
            };
            canvas.text(rect(x, y, 12.0, row.h), chevron, font, colors::MUTED, Align::Left);
            canvas.text(
                rect(x + 14.0, y, (row.right() - x - 18.0).max(0.0), row.h),
                &entry.label,
                if entry.depth == 0 { &bold } else { font },
                colors::FOREGROUND,
                Align::Left,
            );
            canvas.hit(row, hit(Part::Row(index)));
        }
        canvas.pop_clip();
        self.bar = rect(r.right() - scroll::BAR_WIDTH, r.y, scroll::BAR_WIDTH, r.h);
        scroll::vertical(canvas, self.bar, r.h, content, &self.scroll, hit(Part::Bar));
    }
}
