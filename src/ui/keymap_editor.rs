use gpui_kit::component::ActiveTheme;
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::application::keymap::{
    ACTIONS, ActionId, KeyContext, KeymapFile, action_name, canonical_keystroke, supported_contexts,
};

#[derive(Clone, Debug)]
pub enum KeymapEditorEvent {
    Changed(KeymapFile),
    Back,
}

impl EventEmitter<KeymapEditorEvent> for KeymapEditor {}

pub struct KeymapEditor {
    pub keymap: KeymapFile,
    pub search: Entity<InputState>,
    query: String,
    capture: Option<(ActionId, KeyContext)>,
    pending_replace: Option<(ActionId, KeyContext, String, ActionId)>,
    message: Option<String>,
    reset_confirmation: bool,
    focus: FocusHandle,
}

impl KeymapEditor {
    pub fn new(keymap: KeymapFile, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            keymap,
            search: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Filter actions, shortcuts, contexts")
            }),
            query: String::new(),
            capture: None,
            pending_replace: None,
            message: None,
            reset_confirmation: false,
            focus: cx.focus_handle(),
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.message = None;
        cx.emit(KeymapEditorEvent::Changed(self.keymap.clone()));
        cx.notify();
    }

    fn bind(
        &mut self,
        action: ActionId,
        context: KeyContext,
        stroke: String,
        cx: &mut Context<Self>,
    ) {
        match canonical_keystroke(&stroke) {
            Ok(stroke) => {
                let conflict = self.keymap.direct_binding_for(&stroke, context);
                if let Some(conflict) = conflict.filter(|conflict| *conflict != action)
                    && ACTIONS
                        .iter()
                        .filter(|spec| spec.id == conflict && spec.context == context)
                        .flat_map(|spec| spec.default_bindings)
                        .any(|default| default.eq_ignore_ascii_case(&stroke))
                {
                    self.pending_replace = Some((action, context, stroke.clone(), conflict));
                    self.message = Some(format!(
                        "{} uses {stroke} in {}. Replace it?",
                        action_name(conflict),
                        context.label()
                    ));
                    cx.notify();
                    return;
                }
                match self.keymap.add(action, context, stroke, false) {
                    Ok(_) => self.changed(cx),
                    Err(error) => {
                        self.message = Some(error);
                        cx.notify();
                    }
                }
            }
            Err(error) => {
                self.message = Some(error);
                cx.notify();
            }
        }
    }
}

impl Focusable for KeymapEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for KeymapEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.query.to_lowercase();
        let background = cx.theme().tokens.background;
        let header = cx.theme().tokens.table_head;
        let even = cx.theme().tokens.table_even;
        let mut rows = Vec::new();
        for spec in ACTIONS {
            for context in supported_contexts(spec.context) {
                let strokes = self.keymap.bindings_for(spec.id, context);
                let source = if strokes.iter().any(|(_, user)| *user) {
                    "User"
                } else {
                    "Default"
                };
                let stroke_text = if strokes.is_empty() {
                    "Unbound".to_owned()
                } else {
                    strokes
                        .iter()
                        .map(|(stroke, is_user)| {
                            format!("{stroke}{}", if *is_user { " · User" } else { "" })
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let filter_text =
                    format!("{} {stroke_text} {} {source}", spec.name, context.label())
                        .to_lowercase();
                if !filter_text.contains(&query) {
                    continue;
                }
                let action = spec.id;
                let bind_entity = cx.entity();
                let reset_entity = cx.entity();
                let remove_buttons = strokes.iter().map(|(stroke, _)| {
                    let remove_entity = cx.entity();
                    let stroke = stroke.clone();
                    div()
                        .id(format!(
                            "remove-{}-{}-{}",
                            serde_json::to_string(&action).unwrap_or_default(),
                            context.id(),
                            stroke.replace('-', "_")
                        ))
                        .cursor_pointer()
                        .px_2()
                        .rounded_sm()
                        .hover(move |style| style.bg(even))
                        .child("×")
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            remove_entity.update(cx, |this, cx| {
                                this.keymap.remove(action, context, &stroke);
                                this.changed(cx);
                            })
                        })
                });
                let row = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().tokens.border)
                    .child(div().w(px(250.)).child(spec.name))
                    .child(div().w(px(170.)).text_sm().child(context.label()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_1()
                            .min_w_0()
                            .child(div().flex_1().min_w_0().child(stroke_text))
                            .children(remove_buttons),
                    )
                    .child(div().w(px(70.)).text_sm().child(source))
                    .child(
                        Button::new(format!(
                            "bind-{}-{}",
                            serde_json::to_string(&action).unwrap_or_default(),
                            context.id()
                        ))
                        .label(if self.capture == Some((action, context)) {
                            "Press key…"
                        } else {
                            "Bind"
                        })
                        .on_click(move |_, window, cx| {
                            bind_entity.update(cx, |this, cx| {
                                this.capture = Some((action, context));
                                this.pending_replace = None;
                                this.message = Some(format!(
                                    "Press one key for {} in {}. Escape cancels.",
                                    action_name(action),
                                    context.label()
                                ));
                                window.focus(&this.focus, cx);
                                cx.notify();
                            });
                        }),
                    )
                    .child(
                        Button::new(format!(
                            "reset-{}-{}",
                            serde_json::to_string(&action).unwrap_or_default(),
                            context.id()
                        ))
                        .label("Reset")
                        .on_click(move |_, _, cx| {
                            reset_entity.update(cx, |this, cx| {
                                this.keymap.reset(action, context);
                                this.changed(cx);
                            })
                        }),
                    );
                rows.push(row);
            }
        }
        div()
            .id("keymap-editor")
            .key_context("KeymapEditor")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(background)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let Some((action, context)) = this.capture else {
                    return;
                };
                let stroke = &event.keystroke;
                if stroke.key.eq_ignore_ascii_case("escape") {
                    this.capture = None;
                    this.message = None;
                } else {
                    let key = if stroke.key == " " {
                        "space"
                    } else {
                        stroke.key.as_str()
                    };
                    let mut parts = Vec::new();
                    if stroke.modifiers.control {
                        parts.push("ctrl".to_owned());
                    }
                    if stroke.modifiers.alt {
                        parts.push("alt".to_owned());
                    }
                    if stroke.modifiers.shift {
                        parts.push("shift".to_owned());
                    }
                    if stroke.modifiers.platform {
                        parts.push("cmd".to_owned());
                    }
                    parts.push(key.to_ascii_lowercase());
                    this.capture = None;
                    this.bind(action, context, parts.join("-"), cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .bg(header)
                    .child(
                        Button::new("keymap-back")
                            .label("← Back")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(KeymapEditorEvent::Back))),
                    )
                    .child(div().text_lg().font_semibold().child("Keymap Editor"))
                    .child(div().flex_1())
                    .child(
                        Button::new("keymap-reset-all")
                            .label(if self.reset_confirmation {
                                "Confirm Reset All"
                            } else {
                                "Reset All"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.reset_confirmation {
                                    this.keymap = KeymapFile::default();
                                    this.reset_confirmation = false;
                                    this.changed(cx);
                                } else {
                                    this.reset_confirmation = true;
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(div().px_4().py_2().child(Input::new(&self.search)))
            .when_some(self.message.clone(), |this, message| {
                let entity = cx.entity();
                let pending = self.pending_replace.clone();
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_4()
                        .py_2()
                        .text_color(cx.theme().muted_foreground)
                        .child(message)
                        .when_some(pending, |row, (action, context, stroke, _)| {
                            row.child(
                                Button::new("keymap-confirm-replace")
                                    .label("Replace")
                                    .on_click(move |_, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            match this.keymap.add(
                                                action,
                                                context,
                                                stroke.clone(),
                                                true,
                                            ) {
                                                Ok(_) => {
                                                    this.pending_replace = None;
                                                    this.capture = None;
                                                    this.changed(cx);
                                                }
                                                Err(error) => {
                                                    this.message = Some(error);
                                                    cx.notify();
                                                }
                                            }
                                        })
                                    }),
                            )
                            .child(
                                Button::new("keymap-cancel-replace")
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_replace = None;
                                        this.message = None;
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .bg(header)
                    .child(div().w(px(250.)).font_semibold().child("Action"))
                    .child(div().w(px(170.)).font_semibold().child("Context"))
                    .child(div().flex_1().font_semibold().child("Keystrokes"))
                    .child(div().w(px(70.)).font_semibold().child("Source"))
                    .child(div().w(px(210.))),
            )
            .child(
                div().flex_1().min_h_0().overflow_y_scrollbar().children(
                    rows.into_iter()
                        .enumerate()
                        .map(|(index, row)| row.when(index % 2 == 1, |row| row.bg(even))),
                ),
            )
    }
}

pub fn update_query(editor: &Entity<KeymapEditor>, query: String, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        editor.query = query;
        cx.notify();
    });
}
