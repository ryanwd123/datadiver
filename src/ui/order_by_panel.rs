use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Enter, IndentInline, OutdentInline};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, StyledExt as _};
use gpui_kit::*;

use crate::domain::documents::SortKey;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderByEdit {
    Add(String),
    Remove(String),
    Move { column: String, direction: i32 },
    ToggleDirection(String),
    SetDirection { column: String, descending: bool },
}

struct ConfiguredSortKeys {
    keys: Vec<SortKey>,
    selected: Option<IndexPath>,
}

impl ListDelegate for ConfiguredSortKeys {
    type Item = ListItem;

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.keys.len()
    }

    fn render_item(
        &mut self,
        index: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let key = self.keys.get(index.row)?;
        let direction = if key.descending {
            "Descending"
        } else {
            "Ascending"
        };
        Some(
            ListItem::new(format!("order-by-key-{}", index.row))
                .h(px(34.))
                .selected(self.selected == Some(index))
                .child(format!("{} · {direction}", key.column)),
        )
    }

    fn set_selected_index(
        &mut self,
        index: Option<IndexPath>,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = index;
        cx.notify();
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> impl IntoElement {
        div().py_3().child("No sort keys configured")
    }
}

pub struct OrderByPanel {
    columns: Vec<String>,
    configured: Vec<SortKey>,
    command: Entity<CommandState>,
    list: Entity<ListState<ConfiguredSortKeys>>,
}

impl EventEmitter<OrderByEdit> for OrderByPanel {}

impl OrderByPanel {
    pub fn new(
        columns: Vec<String>,
        configured: Vec<SortKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let command = cx.new(|cx| CommandState::new(window, cx));
        let list = cx.new(|cx| {
            ListState::new(
                ConfiguredSortKeys {
                    keys: configured.clone(),
                    selected: None,
                },
                window,
                cx,
            )
        });
        Self {
            columns,
            configured,
            command,
            list,
        }
    }

    pub fn set_configured(
        &mut self,
        keys: Vec<SortKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self
            .list
            .read(cx)
            .selected_index()
            .and_then(|index| self.configured.get(index.row).map(|key| key.column.clone()));
        self.configured = keys.clone();
        let next = selected.and_then(|column| keys.iter().position(|key| key.column == column));
        self.list.update(cx, |list, cx| {
            list.delegate_mut().keys = keys;
            list.set_selected_index(next.map(IndexPath::new), window, cx);
            cx.notify();
        });
        cx.notify();
    }

    pub fn focus_available(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command
            .update(cx, |command, cx| command.focus(window, cx));
    }

    fn selected_key(&self, cx: &App) -> Option<SortKey> {
        self.list
            .read(cx)
            .selected_index()
            .and_then(|index| self.configured.get(index.row).cloned())
    }

    fn switch_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list_focus = self.list.read(cx).focus_handle(cx);
        if window.focused(cx).as_ref() == Some(&list_focus) {
            self.focus_available(window, cx);
        } else {
            self.list.update(cx, |list, cx| {
                let first = (!self.configured.is_empty()).then(|| IndexPath::new(0));
                list.set_selected_index(first, window, cx);
                list.focus(window, cx);
            });
        }
        cx.stop_propagation();
    }

    fn add_selected(&mut self, cx: &mut Context<Self>) {
        let selected = self.command.read(cx).selected_index();
        let available = self
            .columns
            .iter()
            .filter(|column| !self.configured.iter().any(|key| &key.column == *column))
            .collect::<Vec<_>>();
        if let Some(column) = selected.and_then(|index| available.get(index.row)) {
            cx.emit(OrderByEdit::Add((*column).clone()));
        }
        cx.stop_propagation();
    }
}

impl Render for OrderByPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let columns = self.columns.clone();
        let configured = self.configured.clone();
        let panel = cx.entity();
        let list_height = (window.bounds().size.height.as_f32() - 230.0).clamp(100.0, 300.0);
        div()
            .flex()
            .gap_3()
            .w_full()
            .on_action(
                cx.listener(|panel, _: &IndentInline, window, cx| panel.switch_pane(window, cx)),
            )
            .on_action(
                cx.listener(|panel, _: &OutdentInline, window, cx| panel.switch_pane(window, cx)),
            )
            .on_action(cx.listener(|panel, _: &Enter, window, cx| {
                let command_focus = panel.command.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() == Some(&command_focus) {
                    panel.add_selected(cx);
                } else if let Some(key) = panel.selected_key(cx) {
                    cx.emit(OrderByEdit::ToggleDirection(key.column));
                    cx.stop_propagation();
                }
            }))
            .on_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.to_ascii_lowercase();
                let list_focus = panel.list.read(cx).focus_handle(cx);
                if window.focused(cx).as_ref() != Some(&list_focus) {
                    return;
                }
                let Some(sort_key) = panel.selected_key(cx) else {
                    return;
                };
                if key == "delete" {
                    cx.emit(OrderByEdit::Remove(sort_key.column));
                    cx.stop_propagation();
                } else if matches!(key.as_str(), "left" | "right") {
                    cx.emit(OrderByEdit::SetDirection {
                        column: sort_key.column,
                        descending: key == "right",
                    });
                    cx.stop_propagation();
                } else if matches!(key.as_str(), "[" | "]") {
                    cx.emit(OrderByEdit::SetDirection {
                        column: sort_key.column,
                        descending: key == "]",
                    });
                    cx.stop_propagation();
                } else if event.keystroke.modifiers.alt && matches!(key.as_str(), "up" | "down") {
                    cx.emit(OrderByEdit::Move {
                        column: sort_key.column,
                        direction: if key == "up" { -1 } else { 1 },
                    });
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_semibold().child("Available columns"))
                    .child(
                        Command::new(&self.command)
                            .items(
                                columns
                                    .iter()
                                    .filter(|column| {
                                        !configured.iter().any(|key| key.column == **column)
                                    })
                                    .map(|column| CommandItem::new().label(column.clone())),
                            )
                            .placeholder("Find a column")
                            .empty(|_, _, _| div().child("No available columns"))
                            .on_confirm(move |index, _, cx| {
                                let available = columns
                                    .iter()
                                    .filter(|column| {
                                        !configured.iter().any(|key| key.column == **column)
                                    })
                                    .collect::<Vec<_>>();
                                if let Some(column) = available.get(index.row) {
                                    panel.update(cx, |_, cx| {
                                        cx.emit(OrderByEdit::Add((*column).clone()))
                                    });
                                }
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_semibold().child("Sort priority"))
                    .child(div().h(px(list_height)).child(List::new(&self.list)))
                    .child(div().text_color(cx.theme().muted_foreground).child(
                        "[ / ← ascending · ] / → descending · Alt+↑/↓ changes priority · Delete removes",
                    )),
            )
    }
}
