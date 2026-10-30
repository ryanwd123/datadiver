//! Diagnostics: lexer problems and parse errors.
//!
//! Parse errors are only as trustworthy as the grammar's coverage, so the
//! app should treat DuckDB's own parser as the authority and these as instant
//! hints.

use rowan::TextRange;

use crate::ast::{AstNode, FromItem};
use crate::catalog::Catalog;
use crate::complete::eq_fold;
use crate::document::document_objects;
use crate::{Parse, SyntaxKind, SyntaxNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub range: TextRange,
    pub message: String,
    pub severity: Severity,
}

/// Lexer and parse errors, in text order, one per position.
pub fn diagnostics(parse: &Parse) -> Vec<Diagnostic> {
    let root = parse.syntax();
    let mut out = Vec::new();
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        let message = match token.kind() {
            SyntaxKind::UNTERMINATED_STRING => "unterminated string",
            SyntaxKind::UNTERMINATED_QUOTED_IDENT => "unterminated quoted identifier",
            SyntaxKind::UNTERMINATED_BLOCK_COMMENT => "unterminated comment",
            SyntaxKind::ERROR_TOKEN => "unexpected character",
            _ => continue,
        };
        out.push(Diagnostic {
            range: token.text_range(),
            message: message.into(),
            severity: Severity::Error,
        });
    }
    for e in parse.errors() {
        out.push(Diagnostic {
            range: e.range,
            message: e.message.clone(),
            severity: Severity::Error,
        });
    }
    for stmt in root
        .children()
        .filter(|n| n.kind() == SyntaxKind::UNKNOWN_STMT)
    {
        let range = stmt
            .first_token()
            .map(|t| t.text_range())
            .unwrap_or(stmt.text_range());
        out.push(Diagnostic {
            range,
            message: "statement not checked: the parser doesn't know it".into(),
            severity: Severity::Hint,
        });
    }
    out.sort_by_key(|d| (d.range.start(), d.severity));
    // one diagnostic per position: the first error there explains the rest
    out.dedup_by_key(|d| d.range.start());
    out
}

/// `diagnostics`, plus warnings for tables neither the catalog, the script
/// nor a CTE knows. Schemas the catalog lists no tables for are not checked.
pub fn diagnostics_with(parse: &Parse, catalog: &dyn Catalog) -> Vec<Diagnostic> {
    let mut out = diagnostics(parse);
    let root = parse.syntax();
    let objects = document_objects(parse);
    let ctes: Vec<String> = root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::CTE)
        .filter_map(|n| {
            crate::ast::child_of_kind(&n, SyntaxKind::NAME).and_then(|n| crate::ast::first_name(&n))
        })
        .map(|n| n.text)
        .collect();
    for item in root.descendants().filter_map(FromItem::cast) {
        if item.kind() != SyntaxKind::TABLE_REF || !in_query(item.syntax()) {
            continue;
        }
        let path = item.path();
        let Some(name) = path.last() else { continue };
        if name.text.is_empty() || (path.len() == 1 && ctes.iter().any(|c| eq_fold(c, &name.text)))
        {
            continue;
        }
        if objects.iter().any(|o| o.matches_path(&path)) {
            continue;
        }
        let (db, schema) = match path.as_slice() {
            [_] => (None, None),
            [s, _] => (None, Some(s.text.as_str())),
            [d, s, _] => (Some(d.text.as_str()), Some(s.text.as_str())),
            _ => continue,
        };
        let mut tables = catalog.tables(db, schema);
        if let [d, _] = path.as_slice() {
            // `db.table` reaches db's default schema
            tables.extend(catalog.tables(Some(&d.text), None));
        }
        if tables.is_empty() || tables.iter().any(|t| eq_fold(&t.name, &name.text)) {
            continue;
        }
        if catalog
            .functions()
            .iter()
            .any(|f| eq_fold(&f.name, &name.text))
        {
            continue;
        }
        let range = item
            .syntax()
            .children()
            .find(|c| c.kind() == SyntaxKind::PATH)
            .map(|p| p.text_range());
        out.push(Diagnostic {
            range: range.unwrap_or(item.syntax().text_range()),
            message: format!("unknown table {}", name.text),
            severity: Severity::Warning,
        });
    }
    out.sort_by_key(|d| (d.range.start(), d.severity));
    out
}

/// Table references in FROM clauses and DML targets (not DROP / CREATE names).
fn in_query(node: &SyntaxNode) -> bool {
    node.ancestors()
        .skip(1)
        .find(|a| a.kind() != SyntaxKind::JOIN_EXPR && a.kind() != SyntaxKind::PAREN_JOIN)
        .is_some_and(|a| {
            matches!(
                a.kind(),
                SyntaxKind::FROM_CLAUSE
                    | SyntaxKind::USING_CLAUSE
                    | SyntaxKind::INSERT_STMT
                    | SyntaxKind::UPDATE_STMT
                    | SyntaxKind::DELETE_STMT
                    | SyntaxKind::MERGE_STMT
            )
        })
}
