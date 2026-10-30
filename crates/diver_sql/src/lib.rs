//! An editor-oriented parser for DuckDB SQL: lossless syntax trees,
//! completion contexts, scope analysis, highlighting, diagnostics and
//! signature help for the text being typed. See `PLAN.md` for the design.
//!
//! Everything is a pure function over `&str`; offsets are UTF-8 byte offsets.

pub mod ast;
mod catalog;
mod complete;
mod diagnostics;
mod dialect;
mod document;
mod folding;
mod highlight;
mod hover;
mod items;
mod lexer;
mod parser;
mod scope;
mod signature;
mod statement_docs;
mod syntax_kind;

pub use rowan::{TextRange, TextSize};

#[cfg(feature = "mock")]
pub use catalog::MockCatalog;
pub use catalog::{
    Catalog, ColumnInfo, EmptyCatalog, FunctionInfo, PathInfo, SettingInfo, TableInfo, TableKind,
    builtin_pragmas, builtin_types,
};
pub use complete::{
    CompletionContext, Expected, FunctionKind, Name, NewNameKind, complete, complete_with_options,
};
pub use diagnostics::{Diagnostic, Severity, diagnostics, diagnostics_with};
pub use dialect::{Dialect, takes_path};
pub use document::{DocumentObject, ObjectKind, document_objects};
pub use folding::{folding_ranges, selection_ranges};
pub use highlight::{HighlightClass, highlight, highlight_with};
pub use hover::{Hover, hover};
pub use items::{CompletionItem, ItemKind, items, needs_quotes, quote_if_needed, relation_columns};
pub use lexer::{Token, tokenize};
pub use scope::{ColumnSource, Columns, RelationSource, Scope, VisibleRelation, file_relations};
pub use signature::{SignatureHelp, signature_help, signature_help_with};
pub use syntax_kind::{
    DuckSql, Kw, KwCategory, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken,
};

/// What to parse the text as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Entry {
    /// Statements separated by `;`.
    #[default]
    Script,
    /// One expression with an optional `AS alias`, as expression editors hold.
    Expression,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParseOptions {
    pub dialect: Dialect,
    pub entry: Entry,
}

/// A syntax error: what the parser expected and where.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SyntaxError {
    pub message: String,
    pub range: TextRange,
}

/// The result of parsing: a lossless tree and the errors found on the way.
#[derive(Clone, Debug)]
pub struct Parse {
    green: rowan::GreenNode,
    errors: Vec<SyntaxError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }

    /// The tree as an indented dump, for tests and debugging.
    pub fn debug_tree(&self) -> String {
        let mut out = String::new();
        dump(&mut out, &self.syntax(), 0);
        for e in &self.errors {
            out.push_str(&format!("error {:?}: {}\n", e.range, e.message));
        }
        out
    }
}

fn dump(out: &mut String, node: &SyntaxNode, indent: usize) {
    use std::fmt::Write;
    let _ = writeln!(
        out,
        "{:indent$}{:?}@{:?}",
        "",
        node.kind(),
        node.text_range()
    );
    for child in node.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Node(n) => dump(out, &n, indent + 2),
            rowan::NodeOrToken::Token(t) => {
                if t.kind() != SyntaxKind::WHITESPACE {
                    let _ = writeln!(
                        out,
                        "{:w$}{:?}@{:?} {:?}",
                        "",
                        t.kind(),
                        t.text_range(),
                        t.text(),
                        w = indent + 2
                    );
                }
            }
        }
    }
}

/// Parses a DuckDB script.
pub fn parse(text: &str) -> Parse {
    parse_with(text, ParseOptions::default())
}

pub fn parse_with(text: &str, options: ParseOptions) -> Parse {
    let tokens = lexer::tokenize(text);
    let out = parser::parse_tokens(text, &tokens, options, None, None);
    Parse {
        green: out.green,
        errors: out.errors,
    }
}

/// The ranges of the statements in `text`, split at `;` (which the ranges
/// leave out), without empty ones.
pub fn statements(text: &str) -> Vec<TextRange> {
    let mut out = Vec::new();
    let mut start: Option<TextSize> = None;
    let mut end = TextSize::new(0);
    let mut pos = TextSize::new(0);
    for t in lexer::tokenize(text) {
        let range = TextRange::at(pos, t.len);
        pos += t.len;
        if t.kind == SyntaxKind::SEMICOLON {
            if let Some(s) = start.take() {
                out.push(TextRange::new(s, end));
            }
        } else if !t.kind.is_trivia() {
            start.get_or_insert(range.start());
            end = range.end();
        }
    }
    if let Some(s) = start {
        out.push(TextRange::new(s, end));
    }
    out
}

/// The statement the offset is in or right after.
pub fn statement_at(text: &str, offset: TextSize) -> Option<TextRange> {
    let stmts = statements(text);
    stmts
        .iter()
        .copied()
        .find(|r| r.start() <= offset && offset <= r.end())
        .or_else(|| stmts.iter().copied().rev().find(|r| r.end() <= offset))
}

/// Completion items at `offset`: the context, the script's own objects and the catalog.
pub fn complete_with(text: &str, offset: TextSize, catalog: &dyn Catalog) -> Vec<CompletionItem> {
    let ctx = complete(text, offset);
    let objects = document_objects(&parse(text));
    items(&ctx, catalog, &objects)
}
