//! The rules' language features for `code_editor`: colors, problems,
//! hovers, signature help, completion and color swatches.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use code_editor::{
    CompletionContext, CompletionItem, CompletionKind, CompletionProvider, Decoration, DecorationOptions, Diagnostic, Ink,
    LanguageService, Pos, Severity, SignatureHelp, Stickiness,
};

use crate::complete::{self, CompletionKind as Kind};
use crate::help::{self, Class};
use crate::value::Column;

/// The language service and completion provider for rule text, over the
/// columns of the table being formatted.
#[derive(Clone)]
pub struct FormatLanguage {
    pub columns: Rc<RefCell<Vec<Column>>>,
}

impl FormatLanguage {
    pub fn new(columns: Rc<RefCell<Vec<Column>>>) -> Self {
        FormatLanguage { columns }
    }
}

fn ink(class: Class) -> Option<Ink> {
    Some(match class {
        Class::Keyword => Ink::Keyword,
        Class::Cell => Ink::VariableBuiltin,
        Class::Column => Ink::Variable,
        Class::Function => Ink::Function,
        Class::Action => Ink::Property,
        Class::String => Ink::String,
        Class::Number => Ink::Number,
        Class::Color | Class::Value => Ink::Constant,
        Class::Operator => Ink::Operator,
        Class::Comment => Ink::Comment,
        Class::Error => return None,
    })
}

impl LanguageService for FormatLanguage {
    fn highlight(&self, text: &str) -> Option<Vec<(Range<usize>, Ink)>> {
        Some(help::highlight(text).into_iter().filter_map(|(r, c)| Some((r, ink(c)?))).collect())
    }

    fn diagnostics(&self, text: &str) -> Vec<Diagnostic> {
        let checked = crate::check(text, &self.columns.borrow());
        checked
            .diagnostics
            .into_iter()
            .map(|d| {
                // An empty range (where the text ends) underlines the
                // character before it.
                let range = if d.span.is_empty() {
                    let end = text[..d.span.start].trim_end().len();
                    let start = text[..end].char_indices().next_back().map_or(0, |(i, _)| i);
                    start..end
                } else {
                    d.span
                };
                Diagnostic {
                    range,
                    severity: match d.severity {
                        crate::Severity::Error => Severity::Error,
                        crate::Severity::Warning => Severity::Warning,
                    },
                    message: d.message,
                }
            })
            .collect()
    }

    fn hover(&self, text: &str, offset: usize) -> Option<String> {
        help::hover(text, offset, &self.columns.borrow())
    }

    fn signature_help(&self, text: &str, offset: usize) -> Option<SignatureHelp> {
        let (label, active, documentation) = help::signature(text, offset)?;
        Some(SignatureHelp { label, active, documentation: Some(documentation) })
    }
}

impl CompletionProvider for FormatLanguage {
    fn complete(&self, cx: &CompletionContext) -> Vec<CompletionItem> {
        complete::complete(cx.text, cx.offset, &self.columns.borrow())
            .into_iter()
            .map(|c| CompletionItem {
                label: c.label,
                insert_text: Some(c.insert),
                detail: c.detail,
                kind: match c.kind {
                    Kind::Keyword | Kind::Operator => CompletionKind::Keyword,
                    Kind::Action => CompletionKind::Field,
                    Kind::Function => CompletionKind::Function,
                    Kind::Column => CompletionKind::Variable,
                    Kind::Cell => CompletionKind::Variable,
                    Kind::Color | Kind::Value => CompletionKind::Constant,
                },
                range: c.range,
                retrigger: c.retrigger,
                sort_text: Some(c.sort),
                snippet: c.snippet,
                swatch: c.swatch.map(|s| s.argb()),
                command: None,
            })
            .collect()
    }

    fn trigger_characters(&self) -> Vec<char> {
        vec!['"', '#', '(', ':', ',', '=', '.']
    }

    fn extra_words(&self) -> bool {
        false
    }
}

/// A small swatch after each color in the text, as editor decorations.
pub fn color_decorations(text: &str) -> Vec<Decoration> {
    help::color_spans(text)
        .into_iter()
        .map(|(range, color)| Decoration {
            start: pos(text, range.start),
            end: pos(text, range.end),
            options: DecorationOptions {
                background: Some(color.argb()),
                foreground: Some(color.contrast_text().argb()),
                stickiness: Stickiness::NeverGrows,
                ..Default::default()
            },
        })
        .collect()
}

/// The line and byte column of `offset`.
pub fn pos(text: &str, offset: usize) -> Pos {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count();
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
    Pos::new(line, col)
}
