//! Completion sources for the SQL editors: tables, columns, and paths for the
//! SQL panel, and expressions for the group-by, pivot and aggregation editors.

use std::rc::Rc;
use std::sync::{Arc, RwLock};

use diver_sql::Dialect;

use crate::domain::expression_completion::{self, ExpressionCatalog};

use crate::domain::sql_editor::{self, CompletionOptions, SqlCatalog};
use crate::infrastructure::path_completion::{self, PathRoots};
use crate::ui::widgets::editor::{Completer, CompletionItem, Editor};

/// An expression editor completing from `diver_sql` in `dialect`: columns
/// after `"`, and the columns, functions and keywords a typed word starts.
pub fn sql_expression(catalog: ExpressionCatalog, dialect: Dialect) -> Editor {
    let mut editor = Editor::sql();
    editor.line_numbers = false;
    editor.framed = true;
    editor.set_completer(Some(expression_completer(catalog, dialect)));
    editor
}

pub fn expression_completer(catalog: ExpressionCatalog, dialect: Dialect) -> Rc<dyn Completer> {
    Rc::new(ExpressionCompleter { catalog, dialect })
}

struct ExpressionCompleter {
    catalog: ExpressionCatalog,
    dialect: Dialect,
}

impl Completer for ExpressionCompleter {
    fn complete(&self, text: &str, offset: usize) -> Vec<CompletionItem> {
        expression_completion::completions(text, offset, &self.catalog, self.dialect)
            .into_iter()
            .map(|item| CompletionItem {
                label: item.label,
                detail: item.detail,
                insert: item.insert,
                range: item.range,
            })
            .collect()
    }
}

/// The SQL panel's completions: tables, columns, and file paths, opened by
/// the characters `options` turns on.
pub fn sql_catalog(
    catalog: Arc<RwLock<SqlCatalog>>,
    path_roots: Arc<RwLock<PathRoots>>,
    options: Arc<RwLock<CompletionOptions>>,
) -> impl code_editor::CompletionProvider {
    SqlCompletionProvider { catalog, path_roots, options }
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
    catalog: Arc<RwLock<SqlCatalog>>,
    path_roots: Arc<RwLock<PathRoots>>,
    options: Arc<RwLock<CompletionOptions>>,
}

impl SqlCompletionProvider {
    fn options(&self) -> CompletionOptions {
        self.options.read().map(|o| *o).unwrap_or_default()
    }
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
                    sort_text: None,
                    snippet: false,
                })
                .collect();
        }
        // A space opens the list only where a table goes, and `(` only where
        // a call takes a variable, sequence or setting name.
        if cx.trigger == Some(' ') && !sql_editor::after_relation_keyword(cx.text, cx.offset) {
            return Vec::new();
        }
        if cx.trigger == Some('(') && !sql_editor::at_string_name(cx.text, cx.offset) {
            return Vec::new();
        }
        let Ok(catalog) = self.catalog.read() else {
            return Vec::new();
        };
        let items = sql_editor::completions_with(cx.text, cx.offset, &catalog, self.options().quoting);
        // A plain word lets the editor filter what fits the caret; a quoted
        // name replaces its own range, so the list is filtered here.
        let word_start = cx.offset - cx.prefix.len();
        let quoted = items.first().is_some_and(|item| usize::from(item.replace_range.start()) != word_start);
        let typed = items
            .first()
            .map(|item| {
                let start = usize::from(item.replace_range.start()).min(cx.offset);
                cx.text[start..cx.offset].trim_start_matches('"').to_lowercase()
            })
            .unwrap_or_default();
        let mut out: Vec<CompletionItem> = items
            .into_iter()
            .filter(|item| !quoted || item.label.trim_matches('"').to_lowercase().contains(&typed))
            .enumerate()
            .map(|(rank, item)| CompletionItem {
                kind: completion_kind(item.kind),
                range: quoted.then(|| usize::from(item.replace_range.start())..usize::from(item.replace_range.end())),
                // diver_sql ranks columns in scope first, keywords last.
                sort_text: Some(format!("{:04}{rank:05}", item.sort_key)),
                detail: item.detail,
                insert_text: Some(item.insert_text),
                label: item.label,
                retrigger: false,
                snippet: false,
            })
            .collect();
        // Snippets follow what was typed, after everything else.
        if !quoted && !cx.prefix.is_empty() {
            out.extend(SNIPPETS.iter().map(|(label, detail, body)| CompletionItem {
                label: (*label).to_owned(),
                insert_text: Some((*body).to_owned()),
                detail: Some((*detail).to_owned()),
                kind: CompletionKind::Snippet,
                sort_text: Some(format!("9999{label}")),
                snippet: true,
                ..Default::default()
            }));
        }
        out
    }

    fn trigger_characters(&self) -> Vec<char> {
        let options = self.options();
        let mut triggers = vec!['\'', '/', '\\', '('];
        for (on, c) in [(options.after_dot, '.'), (options.after_quote, '"'), (options.after_from, ' ')] {
            if on {
                triggers.push(c);
            }
        }
        triggers
    }

    /// diver_sql offers the keywords that fit the caret.
    fn extra_words(&self) -> bool {
        false
    }
}

/// Snippets the SQL editor offers: (label, description, body). Tab moves
/// through the `${n:…}` fields; the same number is one field typed in twice.
const SNIPPETS: &[(&str, &str, &str)] = &[
    ("sel", "select … from …", "select\n    ${1:*}\nfrom ${2:table}$0"),
    ("join", "join … on …", "join ${1:table} ${2:alias} on $0"),
    (
        "case",
        "case when … end",
        "case\n    when ${1:condition} then ${2:value}\n    else ${3:other}\nend$0",
    ),
    (
        "window",
        "… over (partition by … order by …)",
        "${1:sum}(${2:column}) over (partition by ${3:group} order by ${4:sort})$0",
    ),
    ("pivot", "pivot … on … using …", "pivot ${1:table}\non ${2:column}\nusing ${3:sum}(${4:value})$0"),
    ("cte", "with … as (…) select", "with ${1:name} as (\n    $0\n)\nselect * from $1"),
];

fn completion_kind(kind: diver_sql::ItemKind) -> code_editor::CompletionKind {
    use code_editor::CompletionKind;
    use diver_sql::ItemKind;
    match kind {
        ItemKind::Keyword | ItemKind::Operator => CompletionKind::Keyword,
        ItemKind::Column => CompletionKind::Field,
        ItemKind::Alias | ItemKind::Variable => CompletionKind::Variable,
        ItemKind::Cte | ItemKind::Table | ItemKind::View | ItemKind::Type => CompletionKind::Class,
        ItemKind::Schema | ItemKind::Database => CompletionKind::Module,
        ItemKind::Function | ItemKind::TableFunction => CompletionKind::Function,
        ItemKind::Setting | ItemKind::Sequence | ItemKind::Extension | ItemKind::Pragma => {
            CompletionKind::Constant
        }
        ItemKind::File | ItemKind::Folder => CompletionKind::Text,
    }
}
