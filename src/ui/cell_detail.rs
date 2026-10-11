//! A side pane showing the active cell's whole value, for long, multi-line,
//! and JSON values that do not fit in a grid cell.
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::domain::cell_detail::{DetailText, detail_text};

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

pub enum CellDetailEvent {
    Close,
}

pub struct CellDetailPane {
    content: CellContent,
    detail: Option<DetailText>,
    /// The editor still shows earlier content; it updates on the next render.
    stale: bool,
    pub(crate) editor: Entity<EditorState>,
}

impl EventEmitter<CellDetailEvent> for CellDetailPane {}

impl CellDetailPane {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .soft_wrap(true)
        });
        Self {
            content: CellContent::Message(String::new()),
            detail: None,
            stale: false,
            editor,
        }
    }

    pub fn set_content(&mut self, content: CellContent, cx: &mut Context<Self>) {
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
        self.stale = true;
        cx.notify();
    }

    /// The text shown for the active cell, if it has a value.
    pub fn text(&self) -> Option<&str> {
        self.detail.as_ref().map(|detail| detail.text.as_str())
    }
}

impl Render for CellDetailPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.stale) {
            let text = self.text().unwrap_or_default().to_owned();
            self.editor
                .update(cx, |editor, cx| editor.set_value(text, window, cx));
        }
        let muted = cx.theme().tokens.muted_foreground;
        // Match the grid, which uses the window's font, rather than the
        // editor's monospace code font.
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
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
                    (Some(value), _) => {
                        subtitle.push_str(&format!(" · {} characters", value.chars().count()))
                    }
                }
                (column.clone(), subtitle)
            }
            CellContent::Message(message) => ("Cell detail".to_owned(), message.clone()),
        };
        let copy_text = self.text().map(str::to_owned);
        div()
            .id("cell-detail")
            .flex()
            .flex_col()
            .h_full()
            .gap_1()
            .p_2()
            .border_l_1()
            .border_color(cx.theme().tokens.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when_some(copy_text, |header, text| {
                        header.child(
                            Button::new("cell-detail-copy")
                                .small()
                                .label("Copy")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
                                }),
                        )
                    })
                    .child(
                        Button::new("cell-detail-close")
                            .small()
                            .label("×")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CellDetailEvent::Close))),
                    ),
            )
            .child(div().text_color(muted).truncate().child(subtitle))
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.editor)
                        .readonly(true)
                        .font_family(text_style.font_family)
                        .text_size(font_size)
                        .h_full(),
                ),
            )
    }
}
