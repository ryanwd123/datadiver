use std::{cell::Cell, rc::Rc, sync::Arc};

use crate::domain::sql_editor::{self, Relation};
use crate::infrastructure::path_completion::{self, PathRoots};
use anyhow::Result;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render,
    Styled as _, Subscription, Task, Window, anchored, deferred, div, point,
    component::{
        ActiveTheme as _,
        input::{CompletionProvider, Editor, EditorState, RopeExt, TabSize},
    },
    prelude::FluentBuilder as _,
    px,
};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};
use ropey::Rope;
use std::sync::RwLock;

/// A Kit editor with a small, local word completion source.
///
/// The editor state is public so a host can read or set its value and subscribe
/// to Kit's normal input events.
pub struct InputWithAutocompletion {
    state: Entity<EditorState>,
    /// Set when this input draws its own completion menu; see [`Self::completion_menu`].
    menu: Option<Rc<Cell<MenuCursor>>>,
    _keystrokes: Subscription,
}

/// The highlighted row of a self-drawn menu, for the menu contents of `revision`.
#[derive(Clone, Copy, Default)]
struct MenuCursor {
    selected: usize,
    revision: u64,
}

/// gpui-base's popup priority: above dialogs, which draw at 10 and up.
const ABOVE_DIALOGS: usize = 100;
const MENU_ROWS: usize = 12;

fn menu_cursor(menu: &Cell<MenuCursor>, state: &EditorState) -> MenuCursor {
    let revision = state.completion_menu_state().revision();
    let cursor = menu.get();
    if cursor.revision == revision {
        cursor
    } else {
        MenuCursor {
            selected: 0,
            revision,
        }
    }
}

fn accept_completion(state: &Entity<EditorState>, index: usize, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, cx| {
        let Some(item) = state.completion_menu_state().items.get(index).cloned() else {
            return;
        };
        let cursor = state.cursor();
        state.dismiss_completion_overlay(cx);
        state.insert_completion(&item, cursor..cursor, window, cx);
    });
}

/// Arrows, Enter or Tab, and Escape for a self-drawn menu. Returns whether
/// the keystroke was used.
fn drive_menu(
    menu: &Cell<MenuCursor>,
    state: &Entity<EditorState>,
    keystroke: &gpui_kit::Keystroke,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let count = state.read(cx).completion_menu_state().items.len();
    if count == 0 || keystroke.modifiers.modified() {
        return false;
    }
    let mut cursor = menu_cursor(menu, state.read(cx));
    match keystroke.key.as_str() {
        "up" => cursor.selected = (cursor.selected + count - 1) % count,
        "down" => cursor.selected = (cursor.selected + 1) % count,
        "enter" | "tab" => accept_completion(state, cursor.selected, window, cx),
        "escape" => state.update(cx, |state, cx| state.dismiss_completion_overlay(cx)),
        _ => return false,
    }
    menu.set(cursor);
    window.refresh();
    true
}

impl InputWithAutocompletion {
    pub fn new(
        words: impl IntoIterator<Item = impl Into<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_provider(WordCompletionProvider::new(words), window, cx)
    }

    pub fn sql_columns(
        words: impl IntoIterator<Item = impl Into<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut provider = WordCompletionProvider::new(words);
        provider.quote_identifiers = true;
        // These editors live in dialogs, where the kit's menu is drawn underneath.
        Self::with_menu(provider, true, window, cx)
    }

    fn with_provider(
        provider: impl CompletionProvider + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_menu(provider, false, window, cx)
    }

    fn with_menu(
        provider: impl CompletionProvider + 'static,
        own_menu: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let provider = Rc::new(provider);
        let state = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("sql")
                .tab_size(TabSize {
                    tab_size: 4,
                    hard_tabs: false,
                });
            state.lsp_mut().completion_provider = Some(provider);
            state
        });
        let menu = own_menu.then(|| Rc::new(Cell::new(MenuCursor::default())));
        let editor = state.downgrade();
        let keystroke_menu = menu.clone();
        let keystrokes = cx.intercept_keystrokes(move |event, window, cx| {
            let Some(state) = editor
                .upgrade()
                .filter(|state| state.read(cx).focus_handle(cx).is_focused(window))
            else {
                return;
            };
            let keystroke = &event.keystroke;
            let open = state.read(cx).completion_menu_state().open;
            if open
                && let Some(menu) = &keystroke_menu
                && drive_menu(menu, &state, keystroke, window, cx)
            {
                cx.stop_propagation();
                return;
            }
            if keystroke.key == "tab" && !keystroke.modifiers.modified() && open {
                // Tab accepts the highlighted completion, as in VS Code.
                let enter = gpui_kit::component::input::Enter {
                    secondary: false,
                    shift: false,
                };
                state.update(cx, |state, cx| {
                    state.route_overlay_action(Box::new(enter), window, cx)
                });
                cx.stop_propagation();
                return;
            }
            reset_completion_start(&state, cx);
        });
        Self {
            state,
            menu,
            _keystrokes: keystrokes,
        }
    }

    /// The completion menu for an input inside a dialog. The kit draws its own
    /// menu below dialogs, so hosts put this element beside the editor.
    pub fn completion_menu(&self, cx: &App) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let state = self.state.read(cx);
        let completion = state.completion_menu_state();
        if !completion.open || completion.items.is_empty() {
            return None;
        }
        let (cursor, line_height) = state.cursor_layout()?;
        let origin = cursor.origin + state.scroll_offset() + point(px(-4.), line_height + px(4.));
        let selected = menu_cursor(menu, state).selected;
        let first = selected
            .saturating_sub(MENU_ROWS - 1)
            .min(completion.items.len().saturating_sub(MENU_ROWS));
        let theme = cx.theme();
        let list = div()
            .id("editor-completion")
            .min_w(px(160.))
            .max_w(px(420.))
            .py_1()
            .flex()
            .flex_col()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius)
            .shadow_md()
            .children(
                completion
                    .items
                    .iter()
                    .enumerate()
                    .skip(first)
                    .take(MENU_ROWS)
                    .map(|(index, item)| {
                        let state = self.state.downgrade();
                        div()
                            .id(("editor-completion-item", index))
                            .px_2()
                            .py(px(2.))
                            .truncate()
                            .when(index == selected, |row| {
                                row.bg(theme.accent).text_color(theme.accent_foreground)
                            })
                            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                if let Some(state) = state.upgrade() {
                                    accept_completion(&state, index, window, cx);
                                }
                                cx.stop_propagation();
                            })
                            .child(item.label.clone())
                    }),
            );
        Some(
            deferred(
                anchored()
                    .position(origin)
                    .snap_to_window_with_margin(px(8.))
                    .child(list),
            )
            .with_priority(ABOVE_DIALOGS)
            .into_any_element(),
        )
    }

    pub fn state(&self) -> &Entity<EditorState> {
        &self.state
    }

    pub fn sql_catalog(
        catalog: Arc<RwLock<Vec<Relation>>>,
        path_roots: Arc<RwLock<PathRoots>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = Self::with_provider(
            SqlCompletionProvider {
                catalog,
                path_roots,
            },
            window,
            cx,
        );
        editor.state.update(cx, |state, _| {
            state.lsp_mut().completion_menu.max_width = px(560.);
        });
        editor
    }
}

/// File and folder completions for a path typed inside a SQL string literal.
/// Folders end in a separator so accepting one is ready for the next segment.
pub fn path_completions(
    text: &Rope,
    offset: usize,
    roots: &RwLock<PathRoots>,
    cx: &App,
) -> Option<Task<Vec<CompletionItem>>> {
    let query = sql_editor::path_query(&text.to_string(), offset)?;
    let roots = roots.read().ok()?.clone();
    let range = lsp_types::Range {
        start: text.offset_to_position(query.range.start),
        end: text.offset_to_position(query.range.end),
    };
    let separator = match query.typed.rfind(['/', '\\']) {
        Some(at) if query.typed[at..].starts_with('\\') => '\\',
        _ => '/',
    };
    Some(cx.background_spawn(async move {
        path_completion::list(&query.typed, &roots)
            .into_iter()
            .map(|candidate| {
                let suffix = if candidate.is_dir {
                    separator.to_string()
                } else {
                    String::new()
                };
                let label = format!("{}{suffix}", candidate.name);
                CompletionItem {
                    filter_text: Some(label.clone()),
                    kind: Some(if candidate.is_dir {
                        CompletionItemKind::FOLDER
                    } else {
                        CompletionItemKind::FILE
                    }),
                    detail: Some(candidate.detail).filter(|detail| !detail.is_empty()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: format!("{}{suffix}", candidate.name.replace('\'', "''")),
                    })),
                    label,
                    ..Default::default()
                }
            })
            .collect()
    }))
}

struct SqlCompletionProvider {
    catalog: Arc<RwLock<Vec<Relation>>>,
    path_roots: Arc<RwLock<PathRoots>>,
}

impl CompletionProvider for SqlCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        if let Some(paths) = path_completions(text, offset, &self.path_roots, cx) {
            return cx.background_spawn(async move { Ok(CompletionResponse::Array(paths.await)) });
        }
        let items = self
            .catalog
            .read()
            .map(|catalog| {
                sql_editor::completions(&text.to_string(), offset, &catalog)
                    .into_iter()
                    .map(|c| CompletionItem {
                        label: c.label.clone(),
                        filter_text: Some(c.label),
                        kind: Some(if c.is_column {
                            CompletionItemKind::FIELD
                        } else {
                            CompletionItemKind::CLASS
                        }),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: lsp_types::Range {
                                start: text.offset_to_position(c.range.start),
                                end: text.offset_to_position(c.range.end),
                            },
                            new_text: c.insert,
                        })),
                        ..Default::default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }
    fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
        true
    }
}

/// The editor keeps the offset of the first completion trigger and never clears
/// it, then ignores typing before that offset, so completion went quiet once
/// the cursor moved back. Restart it at the cursor before each keystroke.
fn reset_completion_start(state: &Entity<EditorState>, cx: &mut App) {
    let editor = state.read(cx);
    let menu = editor.completion_menu_state();
    let cursor = editor.selected_range().start;
    let stale = match menu.trigger_start_offset {
        Some(start) => start != cursor && (!menu.open || cursor < start),
        None => false,
    };
    if stale {
        state.update(cx, |state, cx| {
            state.present_completion_items(cursor, "", Vec::new(), cx)
        });
    }
}

impl Render for InputWithAutocompletion {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Editor::new(&self.state).h(px(220.))
    }
}

struct WordCompletionProvider {
    words: Arc<[String]>,
    quote_identifiers: bool,
}

impl WordCompletionProvider {
    fn new(words: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            quote_identifiers: false,
            words: words.into_iter().map(Into::into).collect::<Vec<_>>().into(),
        }
    }

    fn matches(&self, prefix: &str) -> Vec<&String> {
        let prefix = prefix.to_lowercase();
        self.words
            .iter()
            .filter(|word| word.to_lowercase().starts_with(&prefix))
            .collect()
    }

    /// The text range a completion replaces and the words offered for it.
    /// In SQL mode an open `"` lists every column ranked like the search box
    /// (prefix, then word start, contains, fuzzy), and `'strings'` get nothing.
    fn candidates(&self, text: &Rope, offset: usize) -> (usize, usize, Vec<&String>) {
        if !self.quote_identifiers {
            let (start, end, prefix) = identifier_range(text, offset);
            return (start, end, self.matches(&prefix));
        }
        let raw = text.to_string();
        let mut offset = offset.min(raw.len());
        while !raw.is_char_boundary(offset) {
            offset -= 1;
        }
        if let Some(query) = crate::domain::query::sql_column_name_query(&raw, offset) {
            let names = self.words.iter().map(String::as_str).collect::<Vec<_>>();
            let ranked = crate::domain::query::rank_columns(&names, &query.typed);
            let words = ranked
                .into_iter()
                .filter_map(|name| self.words.iter().find(|word| word.as_str() == name))
                .collect();
            return (query.range.start, query.range.end, words);
        }
        if in_sql_string(&raw[..offset]) {
            return (offset, offset, Vec::new());
        }
        let (start, end, prefix) = identifier_range(text, offset);
        (start, end, self.matches(&prefix))
    }
}

/// Whether SQL text ending here is inside a `'…'` string.
fn in_sql_string(before: &str) -> bool {
    let mut string = false;
    let mut identifier = false;
    for ch in before.chars() {
        match ch {
            '\'' if !identifier => string = !string,
            '"' if !string => identifier = !identifier,
            _ => {}
        }
    }
    string
}

impl CompletionProvider for WordCompletionProvider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let (start, end, words) = self.candidates(text, offset);
        let items = words
            .into_iter()
            .map(|word| CompletionItem {
                label: word.clone(),
                filter_text: Some(word.clone()),
                kind: Some(CompletionItemKind::FIELD),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: lsp_types::Range {
                        start: text.offset_to_position(start),
                        end: text.offset_to_position(end),
                    },
                    new_text: if self.quote_identifiers {
                        crate::infrastructure::duckdb_store::quote_identifier(word)
                    } else {
                        word.clone()
                    },
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, _: &str, _: &mut App) -> bool {
        true
    }
}

fn identifier_range(text: &Rope, offset: usize) -> (usize, usize, String) {
    let text = text.to_string();
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let after = &text[offset..];
    let start = before
        .char_indices()
        .rev()
        .find(|(_, character)| !is_identifier_character(*character))
        .map_or(0, |(index, character)| index + character.len_utf8());
    let end = after
        .char_indices()
        .find(|(_, character)| !is_identifier_character(*character))
        .map_or(text.len(), |(index, _)| offset + index);
    (start, end, text[start..offset].to_string())
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::{WordCompletionProvider, identifier_range};
    use ropey::Rope;

    #[test]
    fn identifier_range_replaces_the_whole_word_at_the_cursor() {
        let offset = "country = Mex".len();
        let text = Rope::from_str("country = Mexico");
        let (start, end, prefix) = identifier_range(&text, offset);
        assert_eq!(&text.to_string()[start..end], "Mexico");
        assert_eq!(prefix, "Mex");
    }

    #[test]
    fn identifier_range_counts_unicode_bytes_correctly() {
        let offset = "café = ap".len();
        let text = Rope::from_str("café = apple");
        let (start, end, prefix) = identifier_range(&text, offset);
        assert_eq!(&text.to_string()[start..end], "apple");
        assert_eq!(prefix, "ap");
    }

    #[test]
    fn matching_is_case_insensitive_and_returns_only_prefixes() {
        let provider = WordCompletionProvider::new(["apple", "apricot", "banana"]);
        assert_eq!(provider.matches("Ap"), ["apple", "apricot"]);
        assert!(provider.matches("zzz").is_empty());
    }

    #[test]
    fn sql_quotes_rank_every_column_and_strings_get_nothing() {
        let mut provider =
            WordCompletionProvider::new(["Order Total", "region", "a\"b", "total_units"]);
        provider.quote_identifiers = true;
        let words = |sql: &str, offset: usize| {
            let (start, end, words) = provider.candidates(&Rope::from_str(sql), offset);
            (sql[start..end].to_owned(), words.into_iter().cloned().collect::<Vec<_>>())
        };
        // An auto-closed `""` lists every column and replaces both quotes.
        assert_eq!(
            words("sum(\"\")", 5),
            ("\"\"".into(), provider.words.to_vec())
        );
        // Fuzzy: a word start beats a later substring.
        assert_eq!(
            words("sum(\"tot\")", 8),
            ("\"tot\"".into(), vec!["total_units".into(), "Order Total".into()])
        );
        assert_eq!(words("sum(\"a\"\"b", 9).1, ["a\"b"]);
        assert!(words("x = 're", 7).1.is_empty());
        // Bare words still complete by prefix.
        assert_eq!(words("sum(re", 6), ("re".into(), vec!["region".into()]));
    }
}
