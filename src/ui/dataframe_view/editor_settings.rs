//! The Editor options list: how the SQL editor and its completion list look
//! and behave. Switches change in place; sizes and fonts open a list of
//! choices and come back here.

use super::*;
use crate::infrastructure::json_store::EditorSettings;
use crate::ui::font_picker::FontTarget;
use crate::ui::widgets::list::Item;

/// The rows of the Editor options list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorOption {
    Font,
    PopupFontSize,
    CompletionRows,
    SuggestWhileTyping,
    SuggestAfterDot,
    SuggestAfterQuote,
    SuggestAfterFrom,
    QuoteNames,
    SignatureHelp,
    Problems,
    Hover,
    WordWrap,
    LineNumbers,
    Whitespace,
    IndentGuides,
    CurrentLine,
    Occurrences,
    MatchBrackets,
    AutoClosing,
    CursorBlinking,
    TabSize,
    InsertSpaces,
    DetectIndentation,
    WheelZoom,
}

const OPTIONS: [EditorOption; 24] = [
    EditorOption::Font,
    EditorOption::PopupFontSize,
    EditorOption::CompletionRows,
    EditorOption::SuggestWhileTyping,
    EditorOption::SuggestAfterDot,
    EditorOption::SuggestAfterQuote,
    EditorOption::SuggestAfterFrom,
    EditorOption::QuoteNames,
    EditorOption::SignatureHelp,
    EditorOption::Problems,
    EditorOption::Hover,
    EditorOption::WordWrap,
    EditorOption::LineNumbers,
    EditorOption::Whitespace,
    EditorOption::IndentGuides,
    EditorOption::CurrentLine,
    EditorOption::Occurrences,
    EditorOption::MatchBrackets,
    EditorOption::AutoClosing,
    EditorOption::CursorBlinking,
    EditorOption::TabSize,
    EditorOption::InsertSpaces,
    EditorOption::DetectIndentation,
    EditorOption::WheelZoom,
];

/// Popup font sizes on offer, as percents of the editor's.
const POPUP_PERCENTS: [u8; 10] = [70, 75, 80, 85, 90, 100, 110, 125, 150, 175];

/// The values a choice row offers, `None` for its default.
fn choices(option: EditorOption) -> Vec<Option<u8>> {
    match option {
        EditorOption::PopupFontSize => POPUP_PERCENTS.into_iter().map(Some).collect(),
        EditorOption::CompletionRows => [6, 8, 10, 12, 16, 20, 30].into_iter().map(Some).collect(),
        EditorOption::TabSize => [2, 4, 8].into_iter().map(Some).collect(),
        EditorOption::QuoteNames => vec![Some(0), Some(1)],
        _ => Vec::new(),
    }
}

/// How a choice row names one of its values.
fn choice_label(option: EditorOption, value: u8) -> String {
    match option {
        EditorOption::PopupFontSize => format!("{value}% of the editor font"),
        EditorOption::QuoteNames => quote_names_label(value == 1).into(),
        _ => value.to_string(),
    }
}

fn quote_names_label(always: bool) -> &'static str {
    if always { "Always" } else { "Only when needed" }
}

/// A switch's state, or `None` for rows that are not switches.
fn switch(settings: &mut EditorSettings, option: EditorOption) -> Option<&mut bool> {
    Some(match option {
        EditorOption::SuggestWhileTyping => &mut settings.suggest_while_typing,
        EditorOption::SuggestAfterDot => &mut settings.suggest_after_dot,
        EditorOption::SuggestAfterQuote => &mut settings.suggest_after_quote,
        EditorOption::SuggestAfterFrom => &mut settings.suggest_after_from,
        EditorOption::SignatureHelp => &mut settings.signature_help,
        EditorOption::Problems => &mut settings.problems,
        EditorOption::Hover => &mut settings.hover,
        EditorOption::WordWrap => &mut settings.word_wrap,
        EditorOption::LineNumbers => &mut settings.line_numbers,
        EditorOption::Whitespace => &mut settings.render_whitespace,
        EditorOption::IndentGuides => &mut settings.indent_guides,
        EditorOption::CurrentLine => &mut settings.highlight_current_line,
        EditorOption::Occurrences => &mut settings.highlight_occurrences,
        EditorOption::MatchBrackets => &mut settings.match_brackets,
        EditorOption::AutoClosing => &mut settings.auto_closing,
        EditorOption::CursorBlinking => &mut settings.cursor_blinking,
        EditorOption::InsertSpaces => &mut settings.insert_spaces,
        EditorOption::DetectIndentation => &mut settings.detect_indentation,
        EditorOption::WheelZoom => &mut settings.wheel_zoom,
        _ => return None,
    })
}

fn label(option: EditorOption) -> &'static str {
    match option {
        EditorOption::Font => "Editor font",
        EditorOption::PopupFontSize => "Completion, signature and hover font size",
        EditorOption::CompletionRows => "Completion list rows",
        EditorOption::SuggestWhileTyping => "Suggest completions while typing",
        EditorOption::SuggestAfterDot => "Suggest after . (columns, tables of a schema)",
        EditorOption::SuggestAfterQuote => "Suggest after \" (quoted names)",
        EditorOption::SuggestAfterFrom => "Suggest after FROM, JOIN or PIVOT and a space",
        EditorOption::QuoteNames => "Quote schema, table and column names",
        EditorOption::SignatureHelp => "Show signature help while typing",
        EditorOption::Problems => "Underline problems",
        EditorOption::Hover => "Show docs on hover",
        EditorOption::WordWrap => "Word wrap",
        EditorOption::LineNumbers => "Line numbers",
        EditorOption::Whitespace => "Show whitespace",
        EditorOption::IndentGuides => "Indent guides",
        EditorOption::CurrentLine => "Highlight the current line",
        EditorOption::Occurrences => "Highlight other occurrences of a word",
        EditorOption::MatchBrackets => "Highlight matching brackets",
        EditorOption::AutoClosing => "Close brackets and quotes",
        EditorOption::CursorBlinking => "Blinking cursor",
        EditorOption::TabSize => "Tab size",
        EditorOption::InsertSpaces => "Indent with spaces",
        EditorOption::DetectIndentation => "Detect indentation from the text",
        EditorOption::WheelZoom => "Ctrl+wheel zooms",
    }
}

impl DataFrameView {
    pub(crate) fn open_editor_settings(&mut self, select: Option<EditorOption>, cx: &mut Cx<Self>) {
        let items = self.editor_settings_items();
        self.palette("Editor options", "Find an editor option", items, dialogs::PaletteKind::EditorSettings, cx);
        if let Some(option) = select
            && let Some(dialogs::Dialog::Palette(dialog)) = &mut self.dialog
        {
            dialog.palette.list.select_item(OPTIONS.iter().position(|o| *o == option));
        }
    }

    fn editor_settings_detail(&self, option: EditorOption) -> String {
        let s = &self.editor_settings;
        let font = |family: &Option<String>, default: &str| family.clone().unwrap_or_else(|| format!("Default · {default}"));
        match option {
            EditorOption::Font => font(&self.font.editor_family, crate::ui::widgets::DEFAULT_EDITOR_FAMILY),
            EditorOption::PopupFontSize => format!("{}% of the editor font", s.popup_font_percent),
            EditorOption::CompletionRows => s.completion_rows.to_string(),
            EditorOption::TabSize => s.tab_size.to_string(),
            EditorOption::QuoteNames => quote_names_label(s.always_quote_names).into(),
            option => {
                let mut copy = s.clone();
                let on = switch(&mut copy, option).is_some_and(|on| *on);
                format!("{} · Enter switches", if on { "On" } else { "Off" })
            }
        }
    }

    fn editor_settings_items(&self) -> Vec<Item> {
        OPTIONS
            .iter()
            .map(|option| {
                let mut item = Item::new(label(*option)).detail(self.editor_settings_detail(*option));
                let mut copy = self.editor_settings.clone();
                item.check = switch(&mut copy, *option).map(|on| *on);
                item
            })
            .collect()
    }

    pub(crate) fn confirm_editor_setting(&mut self, index: usize, cx: &mut Cx<Self>) {
        let Some(option) = OPTIONS.get(index).copied() else {
            return;
        };
        let mut settings = self.editor_settings.clone();
        if let Some(on) = switch(&mut settings, option) {
            // Switching stays in the list, so it shows the new value.
            *on = !*on;
            self.save_editor_settings(settings, cx);
            let items = self.editor_settings_items();
            if let Some(dialogs::Dialog::Palette(dialog)) = &mut self.dialog {
                dialog.palette.set_items(items);
                dialog.palette.list.select_item(Some(index));
            }
            cx.notify();
            return;
        }
        self.close_dialog(cx);
        match option {
            EditorOption::Font => self.open_font_picker(FontTarget::Editor, cx),
            _ => {
                let values = choices(option);
                let current = match option {
                    EditorOption::PopupFontSize => Some(self.editor_settings.popup_font_percent),
                    EditorOption::CompletionRows => Some(self.editor_settings.completion_rows),
                    EditorOption::QuoteNames => Some(u8::from(self.editor_settings.always_quote_names)),
                    _ => Some(self.editor_settings.tab_size),
                };
                let items = values
                    .iter()
                    .map(|value| {
                        let mut item = Item::new(match value {
                            Some(v) => choice_label(option, *v),
                            None => format!("Editor size · {} pt", self.font.size),
                        });
                        item.check = Some(*value == current);
                        item
                    })
                    .collect();
                let selected = values.iter().position(|v| *v == current);
                self.palette(label(option), "Choose a value", items, dialogs::PaletteKind::EditorChoice(option, values), cx);
                if let Some(dialogs::Dialog::Palette(dialog)) = &mut self.dialog {
                    dialog.palette.list.select_item(selected);
                }
            }
        }
    }

    /// A value chosen from a choice row's list; back to the options.
    pub(crate) fn choose_editor_setting(&mut self, option: EditorOption, value: Option<u8>, cx: &mut Cx<Self>) {
        let mut settings = self.editor_settings.clone();
        match option {
            EditorOption::PopupFontSize => settings.popup_font_percent = value.unwrap_or(100),
            EditorOption::CompletionRows => settings.completion_rows = value.unwrap_or(12),
            EditorOption::TabSize => settings.tab_size = value.unwrap_or(4),
            EditorOption::QuoteNames => settings.always_quote_names = value == Some(1),
            _ => {}
        }
        self.close_dialog(cx);
        self.save_editor_settings(settings, cx);
        self.open_editor_settings(Some(option), cx);
    }

    /// Saves the options, applies them here and tells the other windows.
    pub(crate) fn save_editor_settings(&mut self, settings: EditorSettings, cx: &mut Cx<Self>) {
        if settings == self.editor_settings {
            return;
        }
        let result = self.repository.load_settings().and_then(|mut saved| {
            saved.editor = settings.clone();
            self.repository.save_settings(&saved)
        });
        if let Err(error) = result {
            self.error = Some(format!("Could not save editor options: {error}"));
            cx.notify();
            return;
        }
        let profile = self.keymap_profile.clone();
        let shared = settings.clone();
        profiles::changed(|profiles| {
            profiles.editor_settings.insert(profile, shared);
        });
        self.set_editor_settings(settings);
        cx.notify_all();
    }

    pub(crate) fn set_editor_settings(&mut self, settings: EditorSettings) {
        if let Some(panel) = &mut self.sql_panel {
            panel.set_editor_settings(&settings);
        }
        self.editor_settings = settings;
    }
}
