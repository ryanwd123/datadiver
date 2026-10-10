use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::color_picker::{ColorPicker, ColorPickerState};
use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::application::theme::{ThemeRole, ThemeSettings};

#[derive(Default)]
pub struct ThemeRuntime {
    pub profiles: HashMap<PathBuf, ThemeSettings>,
    pub revisions: HashMap<PathBuf, u64>,
}

impl Global for ThemeRuntime {}

pub struct ThemeEditor {
    pickers: Vec<(ThemeRole, Entity<ColorPickerState>)>,
    preview: ThemeSettings,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ThemeEditor {
    pub fn new(theme: ThemeSettings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let pickers = ThemeRole::ALL
            .into_iter()
            .map(|role| {
                let state = cx.new(|cx| {
                    let mut state = ColorPickerState::new(window, cx)
                        .default_value(rgba(role.color(&theme).rgba()));
                    state.sync_pending_value(window, cx);
                    state
                });
                let input = state.read(cx).hex_input().clone();
                // Preserve serialized bytes across Kit's HSLA conversion.
                input.update(cx, |input, cx| {
                    input.set_value(role.color(&theme).hex(), window, cx)
                });
                subscriptions.push(cx.subscribe(&input, |editor, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        editor.update_preview(cx);
                    }
                }));
                subscriptions.push(cx.observe(&state, |editor, _, cx| editor.update_preview(cx)));
                (role, state)
            })
            .collect();
        Self {
            pickers,
            preview: theme,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn update_preview(&mut self, cx: &mut Context<Self>) {
        if let Ok(theme) = self.build(cx) {
            self.preview = theme;
            self.error = None;
        }
        cx.notify();
    }

    pub fn build(&self, cx: &App) -> Result<ThemeSettings, String> {
        let mut theme = ThemeSettings::default();
        for (role, picker) in &self.pickers {
            let text = picker.read(cx).hex_input().read(cx).value();
            let color = crate::application::theme::ThemeColor::parse(&text)
                .map_err(|error| format!("{}: {error}", role.label()))?;
            role.set(&mut theme, color);
        }
        Ok(theme)
    }

    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        window.focus(
            &self.pickers[0]
                .1
                .read(cx)
                .hex_input()
                .read(cx)
                .focus_handle(cx),
            cx,
        );
    }

    pub fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preview = ThemeSettings::default();
        for (role, picker) in &self.pickers {
            picker.update(cx, |picker, cx| {
                picker.set_value(rgba(role.color(&self.preview).rgba()), window, cx);
            });
            picker.read(cx).hex_input().clone().update(cx, |input, cx| {
                input.set_value(role.color(&self.preview).hex(), window, cx);
            });
        }
        self.error = None;
        cx.notify();
    }
}

impl Render for ThemeEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.preview;
        let mut preview = div()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgba(theme.grid_border.rgba()))
            .text_color(rgba(theme.foreground.rgba()));
        for row in 0..3 {
            let mut line = div().flex();
            for column in 0..3 {
                let bg = match (row, column) {
                    (0, 0) => theme.frozen_header_background,
                    (0, _) => theme.header_background,
                    (1, 0) => theme.frozen_background,
                    (2, 0) => theme.frozen_stripe_background,
                    (1, 1) => theme.selection_background,
                    (1, _) => theme.background,
                    _ => theme.row_stripe_background,
                };
                line = line.child(
                    div()
                        .flex_1()
                        .px_3()
                        .py_1()
                        .border_1()
                        .border_color(rgba(if row == 1 && column == 1 {
                            theme.active_cell_border.rgba()
                        } else {
                            theme.grid_border.rgba()
                        }))
                        .bg(rgba(bg.rgba()))
                        .child(match (row, column) {
                            (0, 0) => "Frozen",
                            (0, 1) => "Region",
                            (0, _) => "Amount",
                            (1, 0) => "2025",
                            (2, 0) => "2026",
                            (_, 1) => "West",
                            _ => "120.00",
                        }),
                );
            }
            preview = preview.child(line);
        }
        let rows = self
            .pickers
            .iter()
            .map(|(role, picker)| {
                let row = div()
                    .id(format!("theme-color:{}", role.key()))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().child(role.label()))
                    .child(ColorPicker::new(picker).accessibility_label(role.label()))
                    // Kit renders this input inside the popover while it is open.
                    .when(!picker.read(cx).is_open(), |row| {
                        row.child(
                            Input::new(picker.read(cx).hex_input())
                                .id(format!("theme-input:{}", role.key()))
                                .w(px(135.)),
                        )
                    })
                    .when(picker.read(cx).is_open(), |row| {
                        row.child(
                            div()
                                .w(px(135.))
                                .child(picker.read(cx).hex_input().read(cx).value()),
                        )
                    });
                #[cfg(feature = "snapshots")]
                let row = row.test_support();
                row
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .text_sm()
            .child("Choose grid colors. Changes apply when you save.")
            .when_some(self.error.clone(), |element, error| {
                element.child(div().text_color(rgb(0xb42318)).child(error))
            })
            .child(preview)
            .children(rows)
    }
}

#[cfg(all(test, feature = "snapshots"))]
mod tests {
    use super::ThemeEditor;
    use crate::application::theme::{ThemeColor, ThemeSettings};
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext, TestAppContext, px, rgba, size};

    #[gpui_kit::test]
    fn theme_drafts_preserve_exact_colors_validate_hex_and_reset(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let original = ThemeSettings {
            selection_background: ThemeColor::parse("#12345680").unwrap(),
            ..Default::default()
        };
        let mut editor = None;
        let handle = cx.open_window(size(px(800.), px(700.)), |window, cx| {
            let form = cx.new(|cx| ThemeEditor::new(original.clone(), window, cx));
            editor = Some(form.clone());
            Root::new(form, window, cx)
        });
        let editor = editor.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            assert_eq!(editor.read(cx).build(cx).unwrap(), original);
            let input = editor.read(cx).pickers[0].1.read(cx).hex_input().clone();
            input.update(cx, |input, cx| input.set_value("#bad", window, cx));
            assert!(
                editor
                    .read(cx)
                    .build(cx)
                    .unwrap_err()
                    .contains("Header background")
            );
            input.update(cx, |input, cx| input.set_value("#AABBCC", window, cx));
            assert_eq!(
                editor.read(cx).build(cx).unwrap().header_background,
                ThemeColor::rgb(0xaabbcc)
            );
            editor.update(cx, |editor, cx| editor.reset(window, cx));
            assert_eq!(editor.read(cx).build(cx).unwrap(), ThemeSettings::default());
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn color_picker_selection_updates_preview(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let mut editor = None;
        let handle = cx.open_window(size(px(800.), px(700.)), |window, cx| {
            let form = cx.new(|cx| ThemeEditor::new(ThemeSettings::default(), window, cx));
            editor = Some(form.clone());
            Root::new(form, window, cx)
        });
        let editor = editor.unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let picker = editor.read(cx).pickers[0].1.clone();
            picker.update(cx, |picker, cx| {
                picker.select_color(rgba(0xff0000ff).into(), window, cx)
            });
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                editor.read(cx).preview.header_background,
                ThemeColor::rgb(0xff0000)
            );
            assert_eq!(
                editor.read(cx).build(cx).unwrap().header_background,
                ThemeColor::rgb(0xff0000)
            );
        });
    }
}
