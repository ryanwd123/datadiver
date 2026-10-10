use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::domain::actions::{launch_with_values, validate_action};
use crate::domain::documents::ViewAction;

pub enum ActionsEditorEvent {
    Changed(Vec<ViewAction>),
    /// Run this action for the active row; it need not be saved.
    Run(ViewAction),
}

/// Adds, edits, and removes the active view's row actions.
pub struct ActionsEditor {
    actions: Vec<ViewAction>,
    /// Key of the action loaded into the form, or `None` when adding.
    editing: Option<String>,
    key: Entity<InputState>,
    description: Entity<InputState>,
    command: Entity<InputState>,
    columns: Vec<String>,
    /// The active row's `(column, value)` pairs for previews, if there is one.
    row: Option<Vec<(String, String)>>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ActionsEditorEvent> for ActionsEditor {}

impl ActionsEditor {
    pub fn new(
        actions: Vec<ViewAction>,
        columns: Vec<String>,
        row: Option<Vec<(String, String)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("k"));
        let description = cx.new(|cx| InputState::new(window, cx).placeholder("Search the web"));
        let command = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://www.google.com/search?q={Name}")
        });
        let _subscriptions = [&key, &description, &command]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |editor, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => editor.save(window, cx),
                        // Keeps the command preview current.
                        InputEvent::Change => cx.notify(),
                        _ => {}
                    },
                )
            })
            .collect();
        Self {
            actions,
            editing: None,
            key,
            description,
            command,
            columns,
            row,
            error: None,
            _subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = self.key.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    fn fill_form(
        &mut self,
        action: Option<&ViewAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing = action.map(|action| action.key.clone());
        let fields = [
            (&self.key, action.map(|action| action.key.clone())),
            (
                &self.description,
                action.map(|action| action.description.clone()),
            ),
            (&self.command, action.map(|action| action.command.clone())),
        ];
        for (input, value) in fields {
            input.update(cx, |input, cx| {
                input.set_value(value.unwrap_or_default(), window, cx)
            });
        }
        self.error = None;
        self.focus(window, cx);
        cx.notify();
    }

    fn form_action(&self, cx: &App) -> ViewAction {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        ViewAction {
            key: value(&self.key),
            description: value(&self.description),
            command: value(&self.command),
        }
    }

    fn run_form(&mut self, cx: &mut Context<Self>) {
        let mut action = self.form_action(cx);
        if action.command.is_empty() {
            self.set_error(Some("action command is required".into()), cx);
            return;
        }
        if action.description.is_empty() {
            action.description = "Action".into();
        }
        cx.emit(ActionsEditorEvent::Run(action));
    }

    /// Example actions built on the first column, so they preview real values.
    fn examples(&self) -> [ViewAction; 3] {
        let column = self.columns.first().map_or("Column", String::as_str);
        let example = |key: &str, description: &str, command: String| ViewAction {
            key: key.into(),
            description: description.into(),
            command,
        };
        [
            example(
                "g",
                "Search the web",
                format!("https://www.google.com/search?q={{{column}}}"),
            ),
            example(
                "n",
                "Open note in Notepad",
                format!("notepad \"C:/notes/{{{column}}}.txt\""),
            ),
            example(
                "e",
                "Open folder in Explorer",
                format!("explorer \"C:/data/{{{column}}}\""),
            ),
        ]
    }

    /// Loads an example into the form as a new action, keeping its key when free.
    fn use_example(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut example) = self.examples().into_iter().nth(index) else {
            return;
        };
        if self.actions.iter().any(|action| action.key == example.key) {
            example.key.clear();
        }
        self.fill_form(Some(&example), window, cx);
        self.editing = None;
    }

    fn preview(&self, cx: &App) -> Option<Result<String, String>> {
        let command = self.command.read(cx).value();
        if command.trim().is_empty() {
            return None;
        }
        Some(match &self.row {
            Some(row) => launch_with_values(&command, row).map(|launch| launch.preview()),
            None => Err("select a row to preview".into()),
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = self.form_action(cx);
        if let Err(error) = validate_action(&action, &self.actions, self.editing.as_deref()) {
            self.set_error(Some(error), cx);
            return;
        }
        match self
            .editing
            .as_ref()
            .and_then(|key| self.actions.iter().position(|saved| saved.key == *key))
        {
            Some(index) => self.actions[index] = action,
            None => self.actions.push(action),
        }
        cx.emit(ActionsEditorEvent::Changed(self.actions.clone()));
        self.fill_form(None, window, cx);
    }

    fn delete(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.actions.len() {
            return;
        }
        let removed = self.actions.remove(index);
        cx.emit(ActionsEditorEvent::Changed(self.actions.clone()));
        if self.editing.as_ref() == Some(&removed.key) {
            self.fill_form(None, window, cx);
        }
        cx.notify();
    }
}

impl Render for ActionsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let border = theme.tokens.border;
        let highlight = theme.tokens.table_even;
        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label))
                .child(Input::new(input))
        };
        let columns = if self.columns.is_empty() {
            String::new()
        } else {
            format!("Columns: {}", self.columns.join(", "))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .when(self.actions.is_empty(), |list| {
                        list.child(div().text_color(muted).child("No actions yet"))
                    })
                    .children(self.actions.iter().enumerate().map(|(index, action)| {
                        let editing = self.editing.as_ref() == Some(&action.key);
                        div()
                            .id(("view-action", index))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .when(editing, |row| row.bg(highlight))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .min_w(px(22.))
                                    .px_1()
                                    .border_1()
                                    .border_color(border)
                                    .rounded_sm()
                                    .text_center()
                                    .child(action.key.clone()),
                            )
                            .child(div().flex_shrink_0().child(action.description.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(action.command.clone()),
                            )
                            .child(
                                Button::new(("view-action-run", index))
                                    .label("Run")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |editor, _, _, cx| {
                                        if let Some(action) = editor.actions.get(index).cloned() {
                                            cx.emit(ActionsEditorEvent::Run(action));
                                        }
                                    })),
                            )
                            .child(
                                Button::new(("view-action-edit", index))
                                    .label("Edit")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |editor, _, window, cx| {
                                        let action = editor.actions.get(index).cloned();
                                        editor.fill_form(action.as_ref(), window, cx);
                                    })),
                            )
                            .child(
                                Button::new(("view-action-delete", index))
                                    .label("Delete")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(move |editor, _, window, cx| {
                                        editor.delete(index, window, cx)
                                    })),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(70.)).child(field("Key", &self.key)))
                    .child(
                        div()
                            .flex_1()
                            .child(field("Description", &self.description)),
                    ),
            )
            .child(field("Command or URL", &self.command))
            .when_some(self.preview(cx), |content, preview| {
                let (text, color) = match preview {
                    Ok(text) => (format!("→ {text}"), muted),
                    Err(error) => (format!("Preview: {error}"), rgb(0xb33c3c).into()),
                };
                content.child(
                    div()
                        .id("view-action-preview")
                        .text_sm()
                        .text_color(color)
                        .child(text),
                )
            })
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        "{Column} inserts the active row's value; {{ and }} insert braces. \
                         http(s) URLs open in the browser with values URL-encoded. Other \
                         commands start a program directly (no shell), with quoted \
                         arguments kept together; use cmd /C … for shell features.",
                    )
                    .when(!columns.is_empty(), |hint| {
                        hint.child(div().pt_1().truncate().child(columns))
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Examples"))
                    .children(
                        self.examples()
                            .into_iter()
                            .enumerate()
                            .map(|(index, example)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .child(div().flex_shrink_0().child(example.description))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(muted)
                                            .child(example.command),
                                    )
                                    .child(
                                        Button::new(("view-action-example", index))
                                            .label("Use")
                                            .small()
                                            .ghost()
                                            .on_click(cx.listener(move |editor, _, window, cx| {
                                                editor.use_example(index, window, cx)
                                            })),
                                    )
                            }),
                    ),
            )
            .when_some(self.error.clone(), |content, error| {
                content.child(div().text_color(rgb(0xb33c3c)).child(error))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .when(self.editing.is_some(), |buttons| {
                        buttons.child(
                            Button::new("view-action-cancel-edit")
                                .label("Cancel edit")
                                .on_click(cx.listener(|editor, _, window, cx| {
                                    editor.fill_form(None, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("view-action-run-form")
                            .label("Run")
                            .on_click(cx.listener(|editor, _, _, cx| editor.run_form(cx))),
                    )
                    .child(
                        Button::new("view-action-save")
                            .primary()
                            .label(if self.editing.is_some() {
                                "Update action"
                            } else {
                                "Add action"
                            })
                            .on_click(cx.listener(|editor, _, window, cx| editor.save(window, cx))),
                    ),
            )
    }
}

#[cfg(all(test, feature = "snapshots"))]
mod tests {
    use super::{ActionsEditor, ActionsEditorEvent, ViewAction};
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[gpui_kit::test]
    fn adds_rejects_duplicates_updates_and_deletes(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let handle = cx.open_window(size(px(800.), px(600.)), |window, cx| {
            let saved = vec![ViewAction {
                key: "a".into(),
                description: "Old".into(),
                command: "echo {Name}".into(),
            }];
            let row = Some(vec![("Name".to_owned(), "Ada Lovelace".to_owned())]);
            let entity =
                cx.new(|cx| ActionsEditor::new(saved, vec!["Name".into()], row, window, cx));
            editor = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let editor = editor.unwrap();
        let emitted = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let emitted = emitted.clone();
            cx.subscribe(&editor, move |_, event: &ActionsEditorEvent, _| {
                if let ActionsEditorEvent::Changed(actions) = event {
                    emitted.borrow_mut().push(actions.clone());
                }
            })
            .detach();
        });
        let fill = |cx: &mut TestAppContext, key: &str, description: &str, command: &str| {
            let (key, description, command) =
                (key.to_owned(), description.to_owned(), command.to_owned());
            cx.update_window(handle.into(), |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    for (input, value) in [
                        (editor.key.clone(), key.clone()),
                        (editor.description.clone(), description.clone()),
                        (editor.command.clone(), command.clone()),
                    ] {
                        input.update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                    editor.save(window, cx);
                })
            })
            .unwrap();
        };

        fill(cx, "a", "Clash", "echo x");
        cx.update(|cx| {
            assert!(
                editor
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("already used")
            )
        });
        assert!(emitted.borrow().is_empty());

        fill(cx, " g ", "Search", "https://example.com/?q={Name}");
        assert_eq!(emitted.borrow().last().unwrap().len(), 2);
        assert_eq!(emitted.borrow().last().unwrap()[1].key, "g");

        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                let first = editor.actions[0].clone();
                editor.fill_form(Some(&first), window, cx);
            })
        })
        .unwrap();
        fill(cx, "a", "Renamed", "echo {Name}");
        assert_eq!(emitted.borrow().last().unwrap()[0].description, "Renamed");

        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| editor.delete(0, window, cx))
        })
        .unwrap();
        let last = emitted.borrow().last().unwrap().clone();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].key, "g");
    }

    #[gpui_kit::test]
    fn previews_examples_and_runs_unsaved_form(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let handle = cx.open_window(size(px(800.), px(600.)), |window, cx| {
            let saved = vec![ViewAction {
                key: "g".into(),
                description: "Taken".into(),
                command: "echo".into(),
            }];
            let row = Some(vec![("Name".to_owned(), "Ada Lovelace".to_owned())]);
            let entity =
                cx.new(|cx| ActionsEditor::new(saved, vec!["Name".into()], row, window, cx));
            editor = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let editor = editor.unwrap();
        let runs = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| {
            let runs = runs.clone();
            cx.subscribe(&editor, move |_, event: &ActionsEditorEvent, _| {
                if let ActionsEditorEvent::Run(action) = event {
                    runs.borrow_mut().push(action.clone());
                }
            })
            .detach();
        });
        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| {
                assert_eq!(editor.preview(cx), None);
                // The example's key `g` is taken, so it is left blank.
                editor.use_example(0, window, cx);
                assert_eq!(editor.key.read(cx).value(), "");
                assert_eq!(
                    editor.preview(cx),
                    Some(Ok("https://www.google.com/search?q=Ada%20Lovelace".into()))
                );
                editor.use_example(1, window, cx);
                assert_eq!(editor.key.read(cx).value(), "n");
                assert_eq!(
                    editor.preview(cx),
                    Some(Ok("notepad \"C:/notes/Ada Lovelace.txt\"".into()))
                );
                editor.command.update(cx, |input, cx| {
                    input.set_value("echo {Missing}", window, cx)
                });
                assert_eq!(
                    editor.preview(cx),
                    Some(Err("column Missing is missing".into()))
                );
                editor.run_form(cx);
            })
        })
        .unwrap();
        let runs = runs.borrow();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].command, "echo {Missing}");
        assert_eq!(runs[0].description, "Open note in Notepad");
    }
}
