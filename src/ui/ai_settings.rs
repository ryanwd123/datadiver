use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::domain::ai::{AiHeader, AiProvider, AiSettings};
use crate::infrastructure::ai_client;

pub enum AiSettingsEvent {
    Changed(AiSettings),
}

type HeaderInputs = (Entity<InputState>, Entity<InputState>);

/// Adds, edits, tests, and removes AI providers.
pub struct AiSettingsEditor {
    settings: AiSettings,
    /// Name of the provider loaded into the form, or `None` when adding.
    editing: Option<String>,
    name: Entity<InputState>,
    base_url: Entity<InputState>,
    model: Entity<InputState>,
    api_key: Entity<InputState>,
    headers: Vec<HeaderInputs>,
    send_data: bool,
    error: Option<String>,
    /// Outcome of the last connection test: `Ok` text or an error.
    test: Option<Result<String, String>>,
    testing: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AiSettingsEvent> for AiSettingsEditor {}

impl AiSettingsEditor {
    pub fn new(settings: AiSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let name = input("AI Factory", window, cx);
        let base_url = input("https://openrouter.ai/api/v1", window, cx);
        let model = input("gpt-4.1", window, cx);
        let api_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("sk-… or {env:OPENAI_API_KEY}")
                .masked(true)
        });
        let _subscriptions = [&name, &base_url, &model, &api_key]
            .into_iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |editor, _, event: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter { .. } = event {
                            editor.save(window, cx);
                        }
                    },
                )
            })
            .collect();
        let mut editor = Self {
            settings,
            editing: None,
            name,
            base_url,
            model,
            api_key,
            headers: Vec::new(),
            send_data: false,
            error: None,
            test: None,
            testing: None,
            _subscriptions,
        };
        // With one provider there is nothing to choose, so open it for editing.
        if let [only] = editor.settings.providers.as_slice() {
            let only = only.clone();
            editor.fill_form(Some(&only), window, cx);
        }
        editor
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let focus = self.name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    fn fill_form(
        &mut self,
        provider: Option<&AiProvider>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing = provider.map(|provider| provider.name.clone());
        let blank = AiProvider::default();
        let provider = provider.unwrap_or(&blank);
        for (input, value) in [
            (&self.name, &provider.name),
            (&self.base_url, &provider.base_url),
            (&self.model, &provider.model),
            (&self.api_key, &provider.api_key),
        ] {
            input.update(cx, |input, cx| input.set_value(value.clone(), window, cx));
        }
        self.headers.clear();
        for header in &provider.headers {
            self.push_header(header, window, cx);
        }
        self.send_data = provider.send_data;
        self.error = None;
        self.test = None;
        self.testing = None;
        self.focus(window, cx);
        cx.notify();
    }

    fn push_header(&mut self, header: &AiHeader, window: &mut Window, cx: &mut Context<Self>) {
        let name = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Header name");
            input.set_value(header.name.clone(), window, cx);
            input
        });
        let value = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Value");
            input.set_value(header.value.clone(), window, cx);
            input
        });
        self.headers.push((name, value));
    }

    fn add_header(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.push_header(&AiHeader::default(), window, cx);
        if let Some((name, _)) = self.headers.last() {
            let focus = name.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn form_provider(&self, cx: &App) -> AiProvider {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_owned();
        AiProvider {
            name: value(&self.name),
            base_url: value(&self.base_url),
            model: value(&self.model),
            api_key: value(&self.api_key),
            headers: self
                .headers
                .iter()
                .map(|(name, val)| AiHeader {
                    name: value(name),
                    value: value(val),
                })
                .filter(|header| !header.name.is_empty() || !header.value.is_empty())
                .collect(),
            send_data: self.send_data,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = self.form_provider(cx);
        if let Err(error) = provider.validate(&self.settings.providers, self.editing.as_deref()) {
            self.error = Some(error);
            cx.notify();
            return;
        }
        let position = self.editing.as_ref().and_then(|name| {
            self.settings
                .providers
                .iter()
                .position(|saved| &saved.name == name)
        });
        if let Some(index) = position {
            let old = std::mem::replace(&mut self.settings.providers[index], provider.clone());
            if self.settings.default_provider.as_ref() == Some(&old.name) {
                self.settings.default_provider = Some(provider.name.clone());
            }
        } else {
            self.settings.providers.push(provider.clone());
        }
        cx.emit(AiSettingsEvent::Changed(self.settings.clone()));
        self.fill_form(Some(&provider), window, cx);
    }

    fn delete(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.settings.providers.len() {
            return;
        }
        let removed = self.settings.providers.remove(index);
        if self.settings.default_provider.as_ref() == Some(&removed.name) {
            self.settings.default_provider = None;
        }
        cx.emit(AiSettingsEvent::Changed(self.settings.clone()));
        if self.editing.as_ref() == Some(&removed.name) {
            self.fill_form(None, window, cx);
        }
        cx.notify();
    }

    fn make_default(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.providers.get(index) else {
            return;
        };
        self.settings.default_provider = Some(provider.name.clone());
        cx.emit(AiSettingsEvent::Changed(self.settings.clone()));
        cx.notify();
    }

    /// Tests the form as typed, saved or not.
    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let provider = self.form_provider(cx);
        if let Err(error) = provider.validate(&[], None) {
            self.test = Some(Err(error));
            cx.notify();
            return;
        }
        self.test = None;
        self.testing = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ai_client::test_connection(&provider) })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.test = Some(result);
                editor.testing = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for AiSettingsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let highlight = theme.tokens.table_even;
        let error_color: Hsla = rgb(0xb33c3c).into();
        let field = |label: &'static str, input: AnyElement| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_sm().text_color(muted).child(label))
                .child(input)
        };
        let default_name = self.settings.active().map(|p| p.name.clone());
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
                    .when(self.settings.providers.is_empty(), |list| {
                        list.child(div().text_color(muted).child(
                            "No providers yet. Any OpenAI-compatible endpoint works: \
                             OpenAI, OpenRouter, company gateways, Ollama, LM Studio.",
                        ))
                    })
                    .children(self.settings.providers.iter().enumerate().map(
                        |(index, provider)| {
                            let editing = self.editing.as_ref() == Some(&provider.name);
                            let is_default = default_name.as_ref() == Some(&provider.name);
                            div()
                                .id(("ai-provider", index))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .when(editing, |row| row.bg(highlight))
                                .child(div().flex_shrink_0().child(provider.name.clone()))
                                .when(is_default, |row| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_sm()
                                            .text_color(muted)
                                            .child("default"),
                                    )
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(format!(
                                            "{} · {}",
                                            provider.model, provider.base_url
                                        )),
                                )
                                .when(!is_default, |row| {
                                    row.child(
                                        Button::new(("ai-provider-default", index))
                                            .label("Make default")
                                            .small()
                                            .ghost()
                                            .on_click(cx.listener(move |editor, _, _, cx| {
                                                editor.make_default(index, cx)
                                            })),
                                    )
                                })
                                .child(
                                    Button::new(("ai-provider-edit", index))
                                        .label("Edit")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(move |editor, _, window, cx| {
                                            let provider =
                                                editor.settings.providers.get(index).cloned();
                                            editor.fill_form(provider.as_ref(), window, cx);
                                        })),
                                )
                                .child(
                                    Button::new(("ai-provider-delete", index))
                                        .label("Delete")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(move |editor, _, window, cx| {
                                            editor.delete(index, window, cx)
                                        })),
                                )
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(field("Name", Input::new(&self.name).into_any_element())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(field("Model", Input::new(&self.model).into_any_element())),
                    ),
            )
            .child(field(
                "Base URL (the part before /chat/completions)",
                Input::new(&self.base_url).into_any_element(),
            ))
            .child(field(
                "API key · {env:NAME} reads an environment variable; leave empty for local servers",
                Input::new(&self.api_key).mask_toggle().into_any_element(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_sm().text_color(muted).child("Headers"))
                    .children(
                        self.headers
                            .iter()
                            .enumerate()
                            .map(|(index, (name, value))| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().w(px(220.)).child(Input::new(name)))
                                    .child(div().flex_1().child(Input::new(value)))
                                    .child(
                                        Button::new(("ai-header-remove", index))
                                            .label("Remove")
                                            .small()
                                            .ghost()
                                            .on_click(cx.listener(move |editor, _, _, cx| {
                                                if index < editor.headers.len() {
                                                    editor.headers.remove(index);
                                                }
                                                cx.notify();
                                            })),
                                    )
                            }),
                    )
                    .child(
                        div().flex().child(
                            Button::new("ai-header-add")
                                .label("Add header")
                                .small()
                                .on_click(cx.listener(|editor, _, window, cx| {
                                    editor.add_header(window, cx)
                                })),
                        ),
                    ),
            )
            .child(
                Checkbox::new("ai-send-data")
                    .label("Allow sending data to improve SQL output")
                    .checked(self.send_data)
                    .on_change(cx.listener(|editor, checked: &bool, _, cx| {
                        editor.send_data = *checked;
                        cx.notify();
                    })),
            )
            .child(div().text_sm().text_color(muted).child(
                "Requests go to <base URL>/chat/completions. Always sent: table and column \
                 names, types, comments, and the SQL editor text; for tables named with @, \
                 row, null, and distinct counts. Only with data allowed: their min and max \
                 values and their most frequent or (for 25 or fewer) all values.",
            ))
            .when(self.testing.is_some(), |content| {
                content.child(div().text_sm().text_color(muted).child("Testing…"))
            })
            .when_some(self.test.clone(), |content, test| {
                let (text, color) = match test {
                    Ok(text) => (format!("✓ {text}"), muted),
                    Err(error) => (format!("Test failed: {error}"), error_color),
                };
                content.child(
                    div()
                        .id("ai-test-result")
                        .text_sm()
                        .text_color(color)
                        .child(text),
                )
            })
            .when_some(self.error.clone(), |content, error| {
                content.child(div().text_color(error_color).child(error))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .when(self.editing.is_some(), |buttons| {
                        buttons.child(
                            Button::new("ai-provider-new")
                                .label("New provider")
                                .on_click(cx.listener(|editor, _, window, cx| {
                                    editor.fill_form(None, window, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("ai-provider-test")
                            .label("Test connection")
                            .disabled(self.testing.is_some())
                            .on_click(cx.listener(|editor, _, _, cx| editor.test_connection(cx))),
                    )
                    .child(
                        Button::new("ai-provider-save")
                            .primary()
                            .label(if self.editing.is_some() {
                                "Update provider"
                            } else {
                                "Add provider"
                            })
                            .on_click(cx.listener(|editor, _, window, cx| editor.save(window, cx))),
                    ),
            )
    }
}

#[cfg(all(test, feature = "snapshots"))]
mod tests {
    use super::{AiSettingsEditor, AiSettingsEvent};
    use crate::domain::ai::{AiProvider, AiSettings};
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, TestAppContext, px, size};
    use std::cell::RefCell;
    use std::rc::Rc;

    #[gpui_kit::test]
    fn adds_edits_renames_defaults_and_deletes_providers(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let handle = cx.open_window(size(px(800.), px(700.)), |window, cx| {
            let entity = cx.new(|cx| AiSettingsEditor::new(AiSettings::default(), window, cx));
            editor = Some(entity.clone());
            Root::new(entity, window, cx)
        });
        let editor = editor.unwrap();
        let emitted = Rc::new(RefCell::new(Vec::<AiSettings>::new()));
        cx.update(|cx| {
            let emitted = emitted.clone();
            cx.subscribe(&editor, move |_, event: &AiSettingsEvent, _| {
                let AiSettingsEvent::Changed(settings) = event;
                emitted.borrow_mut().push(settings.clone());
            })
            .detach();
        });
        let fill = |cx: &mut TestAppContext, name: &str, headers: &[(&str, &str)]| {
            let name = name.to_owned();
            let headers = headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect::<Vec<_>>();
            cx.update_window(handle.into(), |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    for (input, value) in [
                        (editor.name.clone(), name.clone()),
                        (editor.base_url.clone(), "https://ai.example.com/v1".into()),
                        (editor.model.clone(), "gpt-6-sole".into()),
                        (editor.api_key.clone(), "{env:AI_KEY}".into()),
                    ] {
                        input.update(cx, |input, cx| input.set_value(value, window, cx));
                    }
                    editor.headers.clear();
                    for (n, v) in &headers {
                        editor.add_header(window, cx);
                        let (name_input, value_input) = editor.headers.last().unwrap().clone();
                        name_input.update(cx, |i, cx| i.set_value(n.clone(), window, cx));
                        value_input.update(cx, |i, cx| i.set_value(v.clone(), window, cx));
                    }
                    // A blank header row is dropped.
                    editor.add_header(window, cx);
                    editor.save(window, cx);
                })
            })
            .unwrap();
        };

        fill(
            cx,
            "AI Factory",
            &[
                ("OpenAI-Organization", "org"),
                ("X-AI-Factory-Client", "dd"),
            ],
        );
        let saved = emitted.borrow().last().unwrap().clone();
        assert_eq!(saved.providers.len(), 1);
        let provider: &AiProvider = &saved.providers[0];
        assert_eq!(provider.api_key, "{env:AI_KEY}");
        assert_eq!(provider.headers.len(), 2);
        assert_eq!(provider.headers[1].name, "X-AI-Factory-Client");

        // Saving again updates in place; a rename keeps it the default.
        cx.update_window(handle.into(), |_, _, cx| {
            editor.update(cx, |editor, cx| editor.make_default(0, cx))
        })
        .unwrap();
        fill(cx, "Factory", &[]);
        let saved = emitted.borrow().last().unwrap().clone();
        assert_eq!(saved.providers.len(), 1);
        assert_eq!(saved.default_provider.as_deref(), Some("Factory"));
        assert!(saved.providers[0].headers.is_empty());

        // A second provider with the same name is rejected.
        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| editor.fill_form(None, window, cx))
        })
        .unwrap();
        let count = emitted.borrow().len();
        fill(cx, "Factory", &[]);
        assert_eq!(emitted.borrow().len(), count);
        cx.update(|cx| {
            assert!(
                editor
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("already")
            )
        });

        cx.update_window(handle.into(), |_, window, cx| {
            editor.update(cx, |editor, cx| editor.delete(0, window, cx))
        })
        .unwrap();
        let saved = emitted.borrow().last().unwrap().clone();
        assert!(saved.providers.is_empty());
        assert_eq!(saved.default_provider, None);
    }
}
