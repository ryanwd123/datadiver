//! Completion sources for the SQL editors: tables, columns, and paths for the
//! SQL panel, and column names for the expression editors.

use std::sync::{Arc, RwLock};

use crate::domain::sql_editor::{self, Relation};
use crate::infrastructure::path_completion::{self, PathRoots};
use crate::ui::widgets::editor::{Completer, CompletionItem, Editor};

/// An expression editor that completes column names after `"`.
pub fn sql_columns(words: impl IntoIterator<Item = impl Into<String>>) -> Editor {
    let mut editor = Editor::sql();
    editor.line_numbers = false;
    editor.framed = true;
    let mut provider = WordCompletionProvider::new(words);
    provider.quote_identifiers = true;
    editor.set_completer(Some(std::rc::Rc::new(provider)));
    editor
}

/// The SQL panel's completions: tables, columns, and file paths.
pub fn sql_catalog(catalog: Arc<RwLock<Vec<Relation>>>, path_roots: Arc<RwLock<PathRoots>>) -> impl code_editor::CompletionProvider {
    SqlCompletionProvider { catalog, path_roots }
}

/// File and folder completions for a path typed inside a SQL string literal.
/// Folders end in a separator so accepting one is ready for the next segment.
pub fn path_completions(text: &str, offset: usize, roots: &RwLock<PathRoots>) -> Option<Vec<CompletionItem>> {
    let query = sql_editor::path_query(text, offset)?;
    let roots = roots.read().ok()?.clone();
    let separator = match query.typed.rfind(['/', '\\']) {
        Some(at) if query.typed[at..].starts_with('\\') => '\\',
        _ => '/',
    };
    Some(
        path_completion::list(&query.typed, &roots)
            .into_iter()
            .map(|candidate| {
                let suffix = if candidate.is_dir {
                    separator.to_string()
                } else {
                    String::new()
                };
                CompletionItem {
                    label: format!("{}{suffix}", candidate.name),
                    detail: Some(candidate.detail).filter(|detail| !detail.is_empty()),
                    insert: format!("{}{suffix}", candidate.name.replace('\'', "''")),
                    range: query.range.clone(),
                }
            })
            .collect(),
    )
}

struct SqlCompletionProvider {
    catalog: Arc<RwLock<Vec<Relation>>>,
    path_roots: Arc<RwLock<PathRoots>>,
}

impl code_editor::CompletionProvider for SqlCompletionProvider {
    fn complete(&self, cx: &code_editor::CompletionContext) -> Vec<code_editor::CompletionItem> {
        use code_editor::{CompletionItem, CompletionKind};
        // Paths replace the segment being typed and list a folder once it is chosen.
        if let Some(paths) = path_completions(cx.text, cx.offset, &self.path_roots) {
            return paths
                .into_iter()
                .map(|item| CompletionItem {
                    retrigger: item.insert.ends_with(['/', '\\']),
                    label: item.label,
                    insert_text: Some(item.insert),
                    detail: item.detail,
                    kind: CompletionKind::Text,
                    range: Some(item.range),
                })
                .collect();
        }
        let word_start = cx.offset - cx.prefix.len();
        self.catalog
            .read()
            .map(|catalog| {
                sql_editor::completions(cx.text, cx.offset, &catalog)
                    .into_iter()
                    .map(|completion| CompletionItem {
                        label: completion.label,
                        insert_text: Some(completion.insert),
                        kind: CompletionKind::Field,
                        // A plain word lets the editor filter and add keywords; a
                        // quoted name replaces its own range.
                        range: (completion.range.start != word_start).then_some(completion.range),
                        ..Default::default()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn trigger_characters(&self) -> Vec<char> {
        vec!['.', '\'', '"', '/', '\\']
    }
}

pub(crate) struct WordCompletionProvider {
    words: Arc<[String]>,
    quote_identifiers: bool,
}

impl WordCompletionProvider {
    pub(crate) fn new(words: impl IntoIterator<Item = impl Into<String>>) -> Self {
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
    fn candidates(&self, text: &str, offset: usize) -> (usize, usize, Vec<&String>) {
        if !self.quote_identifiers {
            let (start, end, prefix) = identifier_range(text, offset);
            return (start, end, self.matches(&prefix));
        }
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        if let Some(query) = crate::domain::query::sql_column_name_query(text, offset) {
            let names = self.words.iter().map(String::as_str).collect::<Vec<_>>();
            let ranked = crate::domain::query::rank_columns(&names, &query.typed);
            let words = ranked
                .into_iter()
                .filter_map(|name| self.words.iter().find(|word| word.as_str() == name))
                .collect();
            return (query.range.start, query.range.end, words);
        }
        if in_sql_string(&text[..offset]) {
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

impl Completer for WordCompletionProvider {
    fn complete(&self, text: &str, offset: usize) -> Vec<CompletionItem> {
        let (start, end, words) = self.candidates(text, offset);
        // A bare word completes only once something is typed.
        if start == end && !self.quote_identifiers {
            return Vec::new();
        }
        let typed_prefix = !text[start..offset.min(text.len()).max(start)].is_empty();
        if !typed_prefix && !text[start..end].starts_with('"') {
            return Vec::new();
        }
        words
            .into_iter()
            .map(|word| CompletionItem {
                label: word.clone(),
                detail: None,
                insert: if self.quote_identifiers {
                    crate::infrastructure::duckdb_store::quote_identifier(word)
                } else {
                    word.clone()
                },
                range: start..end,
            })
            .collect()
    }
}

fn identifier_range(text: &str, offset: usize) -> (usize, usize, String) {
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

    #[test]
    fn identifier_range_replaces_the_whole_word_at_the_cursor() {
        let offset = "country = Mex".len();
        let text = "country = Mexico";
        let (start, end, prefix) = identifier_range(text, offset);
        assert_eq!(&text[start..end], "Mexico");
        assert_eq!(prefix, "Mex");
    }

    #[test]
    fn identifier_range_counts_unicode_bytes_correctly() {
        let offset = "café = ap".len();
        let text = "café = apple";
        let (start, end, prefix) = identifier_range(text, offset);
        assert_eq!(&text[start..end], "apple");
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
            let (start, end, words) = provider.candidates(sql, offset);
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
