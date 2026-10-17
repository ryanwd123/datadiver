//! A side pane showing the active cell's whole value, for long, multi-line,
//! and JSON values that do not fit in a grid cell.

use crate::domain::cell_detail::{DetailText, detail_text};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Fonts, Row, WidgetId, button, button_height, button_width, colors};

/// What the pane shows: the active cell, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellContent {
    Cell {
        column: String,
        dtype: String,
        row: usize,
        value: Option<String>,
    },
    Message(String),
}

pub struct CellDetailPane {
    content: CellContent,
    detail: Option<DetailText>,
    pub(crate) editor: Editor,
}

impl Default for CellDetailPane {
    fn default() -> Self {
        Self::new()
    }
}

impl CellDetailPane {
    pub fn new() -> Self {
        let mut editor = Editor::multi_line();
        editor.read_only = true;
        editor.soft_wrap = true;
        editor.framed = false;
        Self {
            content: CellContent::Message(String::new()),
            detail: None,
            editor,
        }
    }

    pub fn set_content(&mut self, content: CellContent) {
        if self.content == content {
            return;
        }
        self.detail = match &content {
            CellContent::Cell {
                value: Some(value), ..
            } => Some(detail_text(value)),
            _ => None,
        };
        self.content = content;
        let text = self.text().unwrap_or_default().to_owned();
        self.editor.set_text(text);
        self.editor.set_selection(0..0);
    }

    /// The text shown for the active cell, if it has a value.
    pub fn text(&self) -> Option<&str> {
        self.detail.as_ref().map(|detail| detail.text.as_str())
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Button(Button::CellDetailCopy) => {
                if let Some(text) = self.text() {
                    crate::ui::platform::system::write_clipboard(text);
                }
                true
            }
            Hit::Editor(id, part) if *id == self.editor.id => {
                self.editor.mouse_down(*part, pos, clicks, shift);
                *focus = Some(*id);
                true
            }
            _ => false,
        }
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        canvas.fill(r, colors::BACKGROUND);
        canvas.fill(rect(r.x, r.y, 1.0, r.h), colors::BORDER);
        let inner = rect(r.x + 9.0, r.y + 8.0, r.w - 17.0, r.h - 16.0);
        let (title, subtitle) = match &self.content {
            CellContent::Cell {
                column,
                dtype,
                row,
                value,
            } => {
                let mut subtitle = format!(
                    "{} · row {}",
                    crate::domain::formatting::type_label(dtype),
                    row + 1
                );
                match (value, &self.detail) {
                    (None, _) => subtitle.push_str(" · NULL"),
                    (Some(_), Some(detail)) if detail.json => subtitle.push_str(" · JSON"),
                    (Some(value), _) => subtitle.push_str(&format!(" · {} characters", value.chars().count())),
                }
                (column.clone(), subtitle)
            }
            CellContent::Message(message) => ("Cell detail".to_owned(), message.clone()),
        };
        let small = &fonts.small;
        let header = rect(inner.x, inner.y, inner.w, button_height(small));
        let mut items = Row::new(header, 4.0);
        let close = items.right(button_width("×", small));
        button(canvas, close, "×", small, ButtonStyle::Outline, true, Hit::Button(Button::CellDetailClose));
        if self.text().is_some() {
            let copy = items.right(button_width("Copy", small));
            button(canvas, copy, "Copy", small, ButtonStyle::Outline, true, Hit::Button(Button::CellDetailCopy));
        }
        canvas.text(items.rest(), &title, &font.semibold(), colors::FOREGROUND, Align::Left);
        let subtitle_rect = rect(inner.x, header.bottom() + 4.0, inner.w, line);
        canvas.text(subtitle_rect, &subtitle, font, colors::MUTED, Align::Left);
        let body = rect(inner.x, subtitle_rect.bottom() + 4.0, inner.w, (inner.bottom() - subtitle_rect.bottom() - 4.0).max(0.0));
        let id = self.editor.id;
        // The grid's font, not the code editors' monospace font.
        self.editor.paint(canvas, body, font, focus == Some(id), |part| Hit::Editor(id, part));
    }
}
