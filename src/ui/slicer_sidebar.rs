use crate::domain::documents::ScalarKey;
use crate::domain::slicers::{self, SlicerCard};
use crate::ui::column_completion::{ColumnCompletion, ColumnSyntax, with_column_completion};
use gpui_kit::component::button::Button;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::scroll::{ScrollableElement as _, Scrollbar, ScrollbarMode};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _};
use gpui_kit::component::{IconName, Selectable as _};
use gpui_kit::*;

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
    position: Point<Pixels>,
    moved: bool,
}

/// A card's value filter, shown under its header while open.
struct CardSearch {
    column: String,
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// A condition card's text input.
struct ConditionInput {
    column: String,
    input: Entity<InputState>,
    completion: Entity<ColumnCompletion>,
    /// The card text last copied into, or committed from, the input.
    synced: String,
    _subscription: Subscription,
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
}

impl EventEmitter<SlicerEvent> for SlicerSidebar {}

impl SlicerSidebar {
    pub fn set_columns(&mut self, columns: Vec<String>, cx: &mut Context<Self>) {
        for condition in &self.conditions {
            condition
                .completion
                .update(cx, |completion, _| completion.set_columns(columns.clone()));
        }
        self.columns = columns;
    }

    /// The column names listed under a card's input, while the list is open.
    #[cfg(all(test, feature = "snapshots"))]
    pub fn completion_items(&self, cx: &App) -> Option<Vec<String>> {
        self.conditions
            .iter()
            .find_map(|condition| condition.completion.read(cx).items())
    }

    pub fn set_cards(&mut self, cards: Vec<SlicerCard>, cx: &mut Context<Self>) {
        self.searches
            .retain(|search| cards.iter().any(|card| card.column == search.column));
        self.cards = cards;
        self.drag = None;
        cx.notify();
    }

    /// Creates inputs for new condition cards and copies card text into inputs
    /// whose text changed elsewhere, such as Clear filters.
    fn sync_conditions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                    input
                        .input
                        .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
                }
                continue;
            }
            let input = cx.new(|cx| {
                let placeholder = if card.sql {
                    SQL_PLACEHOLDER
                } else {
                    CONDITION_PLACEHOLDER
                };
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                input.set_value(text.clone(), window, cx);
                input
            });
            let syntax = if card.sql {
                ColumnSyntax::Sql
            } else {
                ColumnSyntax::Search
            };
            let completion = cx.new(|cx| {
                let mut completion = ColumnCompletion::new(input.clone(), syntax, cx);
                completion.set_columns(self.columns.clone());
                completion
            });
            let column = card.column.clone();
            let _subscription = cx.subscribe(&input, move |sidebar, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    sidebar.commit_condition(&column, cx);
                }
            });
            self.conditions.push(ConditionInput {
                column: card.column.clone(),
                input,
                completion,
                synced: text.clone(),
                _subscription,
            });
        }
    }

    fn commit_condition(&mut self, column: &str, cx: &mut Context<Self>) {
        let Some(input) = self
            .conditions
            .iter_mut()
            .find(|input| input.column == column)
        else {
            return;
        };
        let text = input.input.read(cx).value().trim().to_owned();
        if text == input.synced {
            return;
        }
        input.synced = text.clone();
        cx.emit(SlicerEvent::Condition {
            column: column.to_owned(),
            text,
        });
    }

    fn search(&self, column: &str) -> Option<&CardSearch> {
        self.searches.iter().find(|search| search.column == column)
    }

    /// Indices of `column`'s values that pass its card filter.
    fn visible(&self, column: &str, cx: &App) -> Vec<usize> {
        let filter = self
            .search(column)
            .map(|search| search.input.read(cx).value().to_string())
            .unwrap_or_default();
        self.cards
            .iter()
            .find(|card| card.column == column)
            .map(|card| slicers::visible_values(card, &filter))
            .unwrap_or_default()
    }

    /// Shows or hides a card's filter; hiding it clears the filter.
    fn toggle_search(&mut self, column: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self
            .searches
            .iter()
            .position(|search| search.column == column)
        {
            self.searches.remove(index);
            cx.notify();
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter values"));
        let select_column = column.to_owned();
        let _subscription =
            cx.subscribe(
                &input,
                move |sidebar, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => sidebar.select_visible(&select_column, cx),
                    _ => {}
                },
            );
        let focus = input.read(cx).focus_handle(cx);
        self.searches.push(CardSearch {
            column: column.to_owned(),
            input,
            _subscription,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    /// A card's text input: a condition card's text, or a value card's open filter.
    fn input(&self, column: &str) -> Option<Entity<InputState>> {
        self.conditions
            .iter()
            .find(|input| input.column == column)
            .map(|input| input.input.clone())
            .or_else(|| self.search(column).map(|search| search.input.clone()))
    }

    /// Moves focus to the next card's input, wrapping after the last card. A
    /// value card's filter opens to take focus, and closes again when focus
    /// moves on while it is empty. Returns false when there are no cards.
    pub fn focus_next_input(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cards.is_empty() {
            return false;
        }
        self.sync_conditions(window, cx);
        let current = self.cards.iter().position(|card| {
            self.input(&card.column)
                .is_some_and(|input| input.read(cx).focus_handle(cx).is_focused(window))
        });
        let next = current.map_or(0, |index| (index + 1) % self.cards.len());
        if let Some(index) = current.filter(|&index| index != next) {
            let column = self.cards[index].column.clone();
            if self.cards[index].condition.is_none()
                && self
                    .search(&column)
                    .is_some_and(|search| search.input.read(cx).value().is_empty())
            {
                self.searches.retain(|search| search.column != column);
            }
        }
        let column = self.cards[next].column.clone();
        match self.input(&column) {
            Some(input) => window.focus(&input.read(cx).focus_handle(cx), cx),
            None => self.toggle_search(&column, window, cx),
        }
        cx.notify();
        true
    }

    /// Opens the first card's filter with `text`, for screenshots.
    pub fn open_first_search(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(column) = self.cards.first().map(|card| card.column.clone()) else {
            return;
        };
        if self.search(&column).is_none() {
            self.toggle_search(&column, window, cx);
        }
        if let Some(search) = self.search(&column) {
            let input = search.input.clone();
            input.update(cx, |input, cx| input.set_value(text, window, cx));
        }
        cx.notify();
    }

    fn select_visible(&mut self, column: &str, cx: &mut Context<Self>) {
        let visible = self.visible(column, cx);
        let Some(card) = self.cards.iter_mut().find(|card| card.column == column) else {
            return;
        };
        card.selected = slicers::select_visible(card, &visible);
        cx.emit(SlicerEvent::Select {
            column: card.column.clone(),
            selected: card.selected.clone(),
        });
        cx.notify();
    }

    fn press(
        &mut self,
        column: &str,
        index: usize,
        additive: bool,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
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
            position,
            moved: false,
        });
        card.selected = slicers::click_selection(card, index, additive);
        cx.notify();
    }

    fn sweep(
        &mut self,
        column: &str,
        index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.column != column || (drag.end == index && (drag.moved || position == drag.position))
        {
            return;
        }
        drag.moved = true;
        drag.end = index;
        let (start, additive, initial) = (drag.start, drag.additive, drag.initial.clone());
        let visible = self.visible(column, cx);
        let Some(card) = self.cards.iter_mut().find(|card| card.column == column) else {
            return;
        };
        card.selected = slicers::drag_selection(card, &visible, start, index, additive, &initial);
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if let Some(card) = self.cards.iter().find(|card| card.column == drag.column) {
            cx.emit(SlicerEvent::Select {
                column: card.column.clone(),
                selected: card.selected.clone(),
            });
        }
        cx.notify();
    }
    fn render_card(
        &self,
        card: &SlicerCard,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroll = window
            .use_keyed_state(format!("slicer-scroll:{}", card.column), cx, |_, _| {
                ScrollHandle::default()
            })
            .read(cx)
            .clone();
        if card.condition.is_some() {
            return self.render_condition_card(card, height, window, cx);
        }
        let clear_column = card.column.clone();
        let remove_column = card.column.clone();
        let search_column = card.column.clone();
        let search = self.search(&card.column).map(|search| search.input.clone());
        let visible = self.visible(&card.column, cx);
        div()
            .id(format!("slicer-card:{}", card.column))
            .h(px(height))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().tokens.border)
            .bg(cx.theme().tokens.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .child(card.column.clone()),
                    )
                    .child(
                        Button::new(format!("slicer-search:{}", card.column))
                            .small()
                            .tab_stop(false)
                            .icon(IconName::Search)
                            .selected(search.is_some())
                            .tooltip("Filter values")
                            .on_click(cx.listener(move |sidebar, _, window, cx| {
                                sidebar.toggle_search(&search_column, window, cx)
                            })),
                    )
                    .child(
                        Button::new(format!("slicer-clear:{}", card.column))
                            .small()
                            .tab_stop(false)
                            .label("Clear")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SlicerEvent::Clear(clear_column.clone()))
                            })),
                    )
                    .child(
                        Button::new(format!("slicer-remove:{}", card.column))
                            .small()
                            .tab_stop(false)
                            .label("×")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SlicerEvent::Remove(remove_column.clone()))
                            })),
                    ),
            )
            .children(search.map(|input| {
                let close_column = card.column.clone();
                div()
                    .px_2()
                    .pb_1()
                    .flex_shrink_0()
                    .capture_key_down(cx.listener(
                        move |sidebar, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key.eq_ignore_ascii_case("escape") {
                                sidebar.toggle_search(&close_column, window, cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                    .child(Input::new(&input).small())
            }))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id(format!("slicer-values:{}", card.column))
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&scroll)
                            .pr(px(12.))
                            .children(
                                (card.values.is_empty() && card.error.is_none())
                                    .then(|| div().px_2().text_sm().child("No source values")),
                            )
                            .children(card.error.iter().map(|error| {
                                div()
                                    .px_2()
                                    .text_sm()
                                    .text_color(cx.theme().tokens.muted_foreground)
                                    .child(error.clone())
                            }))
                            .children(
                                (!card.values.is_empty() && visible.is_empty())
                                    .then(|| div().px_2().text_sm().child("No matching values")),
                            )
                            .children(visible.iter().map(|&index| {
                                let value = &card.values[index];
                                let press_column = card.column.clone();
                                let sweep_column = card.column.clone();
                                ListItem::new(format!("slicer-value:{}:{index}", card.column))
                                    .h(px(28.))
                                    .px_2()
                                    .selected(card.highlighted(index))
                                    .disabled(!card.selectable(index))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |sidebar, event: &MouseDownEvent, _, cx| {
                                                sidebar.press(
                                                    &press_column,
                                                    index,
                                                    event.modifiers.control,
                                                    event.position,
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .on_mouse_enter(cx.listener(
                                        move |sidebar, event: &MouseMoveEvent, _, cx| {
                                            if event.pressed_button == Some(MouseButton::Left) {
                                                sidebar.sweep(
                                                    &sweep_column,
                                                    index,
                                                    event.position,
                                                    cx,
                                                );
                                            }
                                        },
                                    ))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_color(if value.available {
                                                cx.theme().tokens.foreground
                                            } else {
                                                cx.theme().tokens.muted_foreground
                                            })
                                            .child(value.label.clone()),
                                    )
                            })),
                    )
                    .child(
                        Scrollbar::vertical(&scroll)
                            .id(format!("slicer-scrollbar:{}", card.column))
                            .mode(ScrollbarMode::Always),
                    ),
            )
            .into_any_element()
    }
}

impl SlicerSidebar {
    fn render_condition_card(
        &self,
        card: &SlicerCard,
        height: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let clear_column = card.column.clone();
        let remove_column = card.column.clone();
        let muted = cx.theme().tokens.muted_foreground;
        let input = self
            .conditions
            .iter()
            .find(|input| input.column == card.column)
            .map(|input| (input.input.clone(), input.completion.clone()));
        div()
            .id(format!("slicer-card:{}", card.column))
            .h(px(height))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().tokens.border)
            .bg(cx.theme().tokens.background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .flex_shrink_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .child(if card.sql {
                                "SQL filter".to_owned()
                            } else {
                                card.column.clone()
                            }),
                    )
                    .child(
                        Button::new(format!("slicer-clear:{}", card.column))
                            .small()
                            .tab_stop(false)
                            .label("Clear")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SlicerEvent::Clear(clear_column.clone()))
                            })),
                    )
                    .child(
                        Button::new(format!("slicer-remove:{}", card.column))
                            .small()
                            .tab_stop(false)
                            .label("×")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(SlicerEvent::Remove(remove_column.clone()))
                            })),
                    ),
            )
            .children(input.map(|(input, completion)| {
                with_column_completion(div().px_2(), &completion, window, cx).child(
                    Input::new(&input)
                        .id(if card.sql {
                            "slicer-sql".to_owned()
                        } else {
                            format!("slicer-condition:{}", card.column)
                        })
                        .small(),
                )
            }))
            .children(card.bounds.as_ref().map(|(min, max)| {
                div()
                    .px_2()
                    .text_sm()
                    .text_color(muted)
                    .truncate()
                    .child(format!("{min} … {max}"))
            }))
            .children(card.error.iter().map(|error| {
                div()
                    .id(format!("slicer-condition-error:{}", card.column))
                    .px_2()
                    .text_sm()
                    .text_color(rgb(0xb33c3c))
                    .child(error.clone())
            }))
            .into_any_element()
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

impl Render for SlicerSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_conditions(window, cx);
        let column_count = if self.cards.len() > 1 && window.bounds().size.width >= px(960.) {
            2
        } else {
            1
        };
        let columns = (0..column_count)
            .map(|column| {
                let cards = self
                    .cards
                    .iter()
                    .skip(column)
                    .step_by(column_count)
                    .collect::<Vec<_>>();
                let available =
                    (window.bounds().size.height.as_f32() - 130.0 - cards.len() as f32 * 8.0)
                        .max(0.0);
                let counts = cards
                    .iter()
                    .map(|card| {
                        if card.condition.is_some() {
                            // Header, input, bounds, and an error line.
                            return 2 + usize::from(card.error.is_some());
                        }
                        let search = self.search(&card.column).is_some();
                        self.visible(&card.column, cx)
                            .len()
                            .max(usize::from(search))
                            + usize::from(search)
                    })
                    .collect::<Vec<_>>();
                let heights = card_heights(&counts, available);
                (cards, heights)
            })
            .collect::<Vec<_>>();
        let entity = cx.entity();
        let release = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                    if event.button == MouseButton::Left {
                        entity.update(cx, |sidebar, cx| sidebar.release(cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        div()
            .id("slicer-sidebar")
            .relative()
            .w(px(240. * column_count as f32))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().tokens.border)
            .p_2()
            .child(
                div().id("slicer-cards").h_full().flex().gap_2().children(
                    columns
                        .into_iter()
                        .enumerate()
                        .map(|(column, (cards, heights))| {
                            div()
                                .id(format!("slicer-column:{column}"))
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .overflow_y_scrollbar()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .children(cards.into_iter().enumerate().map(
                                    |(card_index, card)| {
                                        self.render_card(card, heights[card_index], window, cx)
                                    },
                                ))
                        }),
                ),
            )
            .child(release)
    }
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
    command: Entity<CommandState>,
    columns: Vec<SlicerChoice>,
    pub loading: bool,
    pub error: Option<String>,
}

impl EventEmitter<SlicerChoice> for SlicerPicker {}

impl SlicerPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            command: cx.new(|cx| CommandState::new(window, cx)),
            columns: Vec::new(),
            loading: true,
            error: None,
        }
    }

    #[cfg(test)]
    pub fn choices(&self) -> &[SlicerChoice] {
        &self.columns
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command
            .update(cx, |command, cx| command.focus(window, cx));
    }

    /// `loading` while value discovery runs; `columns` may already list the
    /// choices that need no values.
    pub fn set_columns(
        &mut self,
        columns: Vec<SlicerChoice>,
        loading: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.columns = columns;
        self.error = error;
        self.loading = loading;
        cx.notify();
    }
}

impl Render for SlicerPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picker = cx.entity();
        div()
            .child(
                Command::new(&self.command)
                    .placeholder("Choose a source column")
                    .items(
                        self.columns
                            .iter()
                            .map(|choice| CommandItem::new().label(choice.label())),
                    )
                    .on_confirm(move |index, window, cx| {
                        picker.update(cx, |picker, cx| {
                            if let Some(column) = picker.columns.get(index.row) {
                                cx.emit(column.clone());
                                window.close_dialog(cx);
                            }
                        });
                    }),
            )
            .children(
                self.loading
                    .then(|| div().p_2().child("Finding columns with few enough values…")),
            )
            .children(
                self.error
                    .iter()
                    .map(|error| div().p_2().child(error.clone())),
            )
            .children(
                (!self.loading && self.columns.is_empty() && self.error.is_none())
                    .then(|| div().p_2().child("Every column already has a slicer")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::card_heights;

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
