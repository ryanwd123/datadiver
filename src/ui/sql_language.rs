//! The SQL panel editor's language features from `diver_sql`: colors by
//! where a token sits in the parse tree, problems, hovers, signature help,
//! folding and smart select, over the connection's catalog.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

use code_editor::{Diagnostic, Ink, LanguageService, Severity, SignatureHelp};
use diver_sql::{HighlightClass, Parse, TextRange, TextSize};

use crate::domain::sql_editor::SqlCatalog;

pub struct SqlLanguage {
    catalog: Arc<RwLock<SqlCatalog>>,
    /// The last text parsed and its tree: the editor asks for colors,
    /// problems, folds and smart select of the same text in turn.
    last_parse: RefCell<Option<(String, Rc<Parse>)>>,
}

impl SqlLanguage {
    pub fn new(catalog: Arc<RwLock<SqlCatalog>>) -> Self {
        Self { catalog, last_parse: RefCell::default() }
    }

    fn parse(&self, text: &str) -> Rc<Parse> {
        let mut last = self.last_parse.borrow_mut();
        if let Some((parsed, tree)) = last.as_ref()
            && parsed == text
        {
            return tree.clone();
        }
        let tree = Rc::new(diver_sql::parse(text));
        *last = Some((text.to_owned(), tree.clone()));
        tree
    }

    fn with_catalog<T>(&self, f: impl FnOnce(&SqlCatalog) -> T) -> T {
        match self.catalog.read() {
            Ok(catalog) => f(&catalog),
            Err(_) => f(&SqlCatalog::default()),
        }
    }
}

fn range(r: TextRange) -> Range<usize> {
    usize::from(r.start())..usize::from(r.end())
}

fn offset(offset: usize) -> TextSize {
    TextSize::new(offset as u32)
}

fn ink(class: HighlightClass) -> Option<Ink> {
    Some(match class {
        HighlightClass::Keyword => Ink::Keyword,
        HighlightClass::Function | HighlightClass::Aggregate => Ink::Function,
        HighlightClass::Type | HighlightClass::Table | HighlightClass::Cte | HighlightClass::Schema => Ink::Type,
        HighlightClass::Column | HighlightClass::Alias => Ink::Variable,
        HighlightClass::Parameter => Ink::Parameter,
        HighlightClass::String | HighlightClass::FilePath => Ink::String,
        HighlightClass::Number => Ink::Number,
        HighlightClass::Operator => Ink::Operator,
        HighlightClass::Comment => Ink::Comment,
        HighlightClass::Error | HighlightClass::Identifier => return None,
    })
}

/// Hover markdown as the editor's plain-text tooltip: code fences, bold and
/// backticks dropped, list items as bullets.
pub fn plain_text(markdown: &str) -> String {
    let lines = markdown
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .map(|line| {
            let line = line.replace("**", "").replace('`', "");
            match line.strip_prefix("- ") {
                Some(item) => format!("\u{2022} {item}"),
                None => line,
            }
        })
        .collect::<Vec<_>>();
    lines.join("\n").trim().to_string()
}

/// The parameter list of a signature like `round(x DOUBLE, n INTEGER) -> DOUBLE`:
/// the byte range of each argument.
fn argument_ranges(signature: &str) -> Vec<Range<usize>> {
    let Some(open) = signature.find('(') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = open + 1;
    for (i, c) in signature.char_indices().skip_while(|(i, _)| *i <= open) {
        match c {
            '(' | '[' | '<' => depth += 1,
            ')' if depth == 0 => {
                if i > start {
                    out.push(start..i);
                }
                break;
            }
            ')' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(start..i);
                start = i + 1;
            }
            _ => {}
        }
    }
    // Leading spaces are not part of the argument.
    out.into_iter()
        .map(|r| {
            let trimmed = signature[r.clone()].len() - signature[r.clone()].trim_start().len();
            r.start + trimmed..r.end
        })
        .collect()
}

impl LanguageService for SqlLanguage {
    fn highlight(&self, text: &str) -> Option<Vec<(Range<usize>, Ink)>> {
        let parse = self.parse(text);
        let classes = self.with_catalog(|catalog| diver_sql::highlight_with(&parse, catalog));
        Some(
            classes
                .into_iter()
                .filter_map(|(r, class)| Some((range(r), ink(class)?)))
                .collect(),
        )
    }

    fn diagnostics(&self, text: &str) -> Vec<Diagnostic> {
        let parse = self.parse(text);
        self.with_catalog(|catalog| diver_sql::diagnostics_with(&parse, catalog))
            .into_iter()
            .map(|d| Diagnostic {
                range: range(d.range),
                severity: match d.severity {
                    diver_sql::Severity::Error => Severity::Error,
                    diver_sql::Severity::Warning => Severity::Warning,
                    _ => Severity::Info,
                },
                message: d.message,
            })
            .collect()
    }

    fn hover(&self, text: &str, at: usize) -> Option<String> {
        let hover = self.with_catalog(|catalog| diver_sql::hover(text, offset(at), catalog))?;
        Some(plain_text(&hover.markdown)).filter(|h| !h.is_empty())
    }

    fn signature_help(&self, text: &str, at: usize) -> Option<SignatureHelp> {
        let help = self.with_catalog(|catalog| diver_sql::signature_help_with(text, offset(at), catalog))?;
        let name = help.function.last()?.text.clone();
        // The first signature with room for the argument being typed.
        let label = help
            .signatures
            .iter()
            .find(|s| argument_ranges(s).len() > help.active_argument || s.contains("..."))
            .or(help.signatures.first())
            .cloned()
            .unwrap_or_else(|| format!("{name}(\u{2026})"));
        let arguments = argument_ranges(&label);
        let active = match &help.active_name {
            Some(named) => arguments
                .iter()
                .find(|r| label[(*r).clone()].split_whitespace().next() == Some(named.as_str()))
                .cloned(),
            None => arguments
                .get(help.active_argument)
                .or_else(|| arguments.last().filter(|r| label[(*r).clone()].ends_with("...")))
                .cloned(),
        };
        let others = help.signatures.len().saturating_sub(1);
        let documentation = match (help.description, others) {
            (Some(d), 0) => Some(d),
            (Some(d), n) => Some(format!("{d}\n(+{n} more signatures)")),
            (None, 0) => None,
            (None, n) => Some(format!("(+{n} more signatures)")),
        };
        Some(SignatureHelp {
            label,
            active,
            documentation,
        })
    }

    fn folding_ranges(&self, text: &str) -> Option<Vec<Range<usize>>> {
        Some(
            diver_sql::folding_ranges(&self.parse(text))
                .into_iter()
                .map(range)
                .collect(),
        )
    }

    fn selection_ranges(&self, text: &str, at: usize) -> Option<Vec<Range<usize>>> {
        Some(
            diver_sql::selection_ranges(&self.parse(text), offset(at))
                .into_iter()
                .map(range)
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::sql_editor::Relation;

    fn language() -> SqlLanguage {
        let catalog = SqlCatalog {
            relations: vec![Relation {
                catalog: "db".into(),
                schema: "main".into(),
                name: "orders".into(),
                columns: vec!["qty".into()],
                column_types: vec!["INTEGER".into()],
                current_database: true,
                ..Default::default()
            }],
            current_database: "db".into(),
            databases: vec!["db".into()],
            schemas: vec![("db".into(), "main".into())],
            functions: vec![diver_sql::FunctionInfo {
                name: "round".into(),
                schema: Some("main".into()),
                kind: diver_sql::FunctionKind::Scalar,
                signatures: vec!["round(x DOUBLE, precision INTEGER) -> DOUBLE".into()],
                description: Some("Rounds x to precision decimal places.".into()),
            }],
            ..Default::default()
        };
        SqlLanguage::new(Arc::new(RwLock::new(catalog)))
    }

    #[test]
    fn colors_follow_the_tree() {
        let text = "SELECT qty, 'a' FROM orders -- n";
        let colors = language()
            .highlight(text)
            .unwrap()
            .into_iter()
            .map(|(r, ink)| (&text[r], ink))
            .collect::<Vec<_>>();
        for expected in [
            ("SELECT", Ink::Keyword),
            ("qty", Ink::Variable),
            ("'a'", Ink::String),
            ("orders", Ink::Type),
            ("-- n", Ink::Comment),
        ] {
            assert!(colors.contains(&expected), "{expected:?} in {colors:?}");
        }
    }

    #[test]
    fn problems_hovers_and_signatures() {
        let language = language();
        let problems = language.diagnostics("SELECT FROM WHERE");
        assert!(!problems.is_empty());
        assert_eq!(problems[0].severity, Severity::Error);
        // Unknown tables are warnings once the catalog lists the schema.
        let unknown = language.diagnostics("SELECT * FROM nowhere");
        assert!(unknown.iter().any(|d| d.severity == Severity::Warning), "{unknown:?}");
        assert!(language.diagnostics("SELECT qty FROM orders").is_empty());

        let text = "SELECT qty FROM orders";
        let hover = language.hover(text, text.find("qty").unwrap() + 1).unwrap();
        assert!(hover.contains("qty") && !hover.contains('`'), "{hover}");

        let text = "SELECT round(qty, ";
        let help = language.signature_help(text, text.len()).unwrap();
        assert_eq!(help.label, "round(x DOUBLE, precision INTEGER) -> DOUBLE");
        assert_eq!(&help.label[help.active.unwrap()], "precision INTEGER");
        assert_eq!(help.documentation.as_deref(), Some("Rounds x to precision decimal places."));
    }

    #[test]
    fn argument_ranges_skip_nested_types() {
        let sig = "f(a STRUCT(x INT, y INT), b VARCHAR[]) -> INT";
        let args = argument_ranges(sig).into_iter().map(|r| &sig[r]).collect::<Vec<_>>();
        assert_eq!(args, ["a STRUCT(x INT, y INT)", "b VARCHAR[]"]);
        assert!(argument_ranges("now()").is_empty());
    }

    #[test]
    fn markdown_becomes_plain_text() {
        assert_eq!(
            plain_text("**SELECT**: rows.\n\n```sql\nSELECT 1\n```\n- `a` INT"),
            "SELECT: rows.\n\nSELECT 1\n\u{2022} a INT"
        );
    }
}
