//! Hover: statement docs on statement keywords, signatures on functions, and
//! what a table, CTE, alias or column refers to.

use rowan::{TextRange, TextSize};

use crate::ast::{self, AstNode};
use crate::catalog::Catalog;
use crate::complete::{Name, eq_fold};
use crate::document::document_objects;
use crate::items::relation_columns;
use crate::scope::{self, RelationSource, VisibleRelation};
use crate::statement_docs::STATEMENT_DOCS;
use crate::{SyntaxKind, SyntaxNode, SyntaxToken, parse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    /// The token the hover is about.
    pub range: TextRange,
    pub markdown: String,
}

pub fn hover(text: &str, offset: TextSize, catalog: &dyn Catalog) -> Option<Hover> {
    let parse = parse(text);
    let root = parse.syntax();
    let token = token_at(&root, offset)?;
    let range = token.text_range();
    let markdown = if token.kind().is_keyword() {
        statement_doc(&token)?.to_string()
    } else if ast::is_name_token(token.kind()) {
        name_hover(&root, &token, catalog, &parse)?
    } else {
        return None;
    };
    Some(Hover { range, markdown })
}

/// The word at `offset`, preferring the one ending there over whitespace after it.
fn token_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    let pick = |t: &SyntaxToken| t.kind().is_keyword() || ast::is_name_token(t.kind());
    match root.token_at_offset(offset) {
        rowan::TokenAtOffset::None => None,
        rowan::TokenAtOffset::Single(t) => pick(&t).then_some(t),
        rowan::TokenAtOffset::Between(l, r) => {
            if pick(&r) {
                Some(r)
            } else {
                pick(&l).then_some(l)
            }
        }
    }
}

pub(crate) fn doc(key: &str) -> Option<&'static str> {
    STATEMENT_DOCS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, md)| *md)
}

/// The doc of the statement whose leading keywords include `token`.
fn statement_doc(token: &SyntaxToken) -> Option<&'static str> {
    let parent = token.parent()?;
    let has =
        |node: &SyntaxNode, kind: SyntaxKind| node.children_with_tokens().any(|e| e.kind() == kind);
    let key = match parent.kind() {
        SyntaxKind::SELECT_CLAUSE if token.kind() == SyntaxKind::SELECT_KW => "select",
        SyntaxKind::INSERT_STMT if has(&parent, SyntaxKind::EXTENSIONS_KW) => "update_extensions",
        SyntaxKind::INSERT_STMT => "insert",
        SyntaxKind::UPDATE_STMT => "update",
        SyntaxKind::DELETE_STMT => "delete",
        SyntaxKind::TRUNCATE_STMT => "truncate",
        SyntaxKind::MERGE_STMT => "merge",
        SyntaxKind::CREATE_STMT => {
            if has(&parent, SyntaxKind::VIEW_KW) {
                "create_view"
            } else if has(&parent, SyntaxKind::MACRO_KW) || has(&parent, SyntaxKind::FUNCTION_KW) {
                "create_macro"
            } else if has(&parent, SyntaxKind::SEQUENCE_KW) {
                "create_sequence"
            } else if has(&parent, SyntaxKind::INDEX_KW) {
                "create_index"
            } else if has(&parent, SyntaxKind::SCHEMA_KW) {
                "create_schema"
            } else if has(&parent, SyntaxKind::SECRET_KW) {
                "create_secret"
            } else if has(&parent, SyntaxKind::TYPE_KW) {
                "create_type"
            } else {
                "create_table"
            }
        }
        SyntaxKind::DROP_STMT if has(&parent, SyntaxKind::SECRET_KW) => "drop_secret",
        SyntaxKind::DROP_STMT => "drop",
        SyntaxKind::ALTER_STMT if has(&parent, SyntaxKind::VIEW_KW) => "alter_view",
        SyntaxKind::ALTER_STMT => "alter_table",
        SyntaxKind::COPY_STMT => "copy",
        SyntaxKind::ATTACH_STMT => "attach",
        SyntaxKind::DETACH_STMT => "detach",
        SyntaxKind::USE_STMT => "use",
        SyntaxKind::SET_STMT | SyntaxKind::RESET_STMT if has(&parent, SyntaxKind::VARIABLE_KW) => {
            "set_variable"
        }
        SyntaxKind::SET_STMT | SyntaxKind::RESET_STMT => "set",
        SyntaxKind::INSTALL_STMT => "install",
        SyntaxKind::LOAD_STMT => "load",
        SyntaxKind::PREPARE_STMT => "prepare",
        SyntaxKind::EXECUTE_STMT => "execute",
        SyntaxKind::DEALLOCATE_STMT => "deallocate",
        SyntaxKind::EXPLAIN_STMT => "explain",
        SyntaxKind::DESCRIBE_STMT => "describe",
        SyntaxKind::SHOW_STMT => "show",
        SyntaxKind::SUMMARIZE_STMT => "summarize",
        SyntaxKind::CALL_STMT => "call",
        SyntaxKind::CHECKPOINT_STMT => "checkpoint",
        SyntaxKind::VACUUM_STMT => "vacuum",
        SyntaxKind::ANALYZE_STMT => "analyze",
        SyntaxKind::COMMENT_STMT => "comment",
        SyntaxKind::EXPORT_STMT => "export",
        SyntaxKind::IMPORT_STMT => "import",
        SyntaxKind::PIVOT_STMT
            if has(&parent, SyntaxKind::UNPIVOT_KW)
                || has(&parent, SyntaxKind::PIVOT_LONGER_KW) =>
        {
            "unpivot"
        }
        SyntaxKind::PIVOT_STMT | SyntaxKind::PIVOT_REF => "pivot",
        SyntaxKind::UNPIVOT_REF => "unpivot",
        SyntaxKind::TRANSACTION_STMT => "transaction",
        _ => return None,
    };
    // Only the statement's own keywords, not ones deeper inside it (ON
    // CONFLICT DO UPDATE is not an UPDATE statement).
    if token.kind() == SyntaxKind::UPDATE_KW && parent.kind() != SyntaxKind::UPDATE_STMT {
        return None;
    }
    doc(key)
}

fn name_hover(
    root: &SyntaxNode,
    token: &SyntaxToken,
    catalog: &dyn Catalog,
    parse: &crate::Parse,
) -> Option<String> {
    let parent = token.parent()?;
    let grand = parent.parent();
    let name = ast::token_name(token);
    let objects = document_objects(parse);
    match (parent.kind(), grand.as_ref().map(|g| g.kind())) {
        (
            SyntaxKind::PATH,
            Some(SyntaxKind::CALL_EXPR | SyntaxKind::TABLE_FUNCTION | SyntaxKind::SPECIAL_CALL),
        ) => {
            let mut lines = Vec::new();
            for f in catalog
                .functions()
                .into_iter()
                .filter(|f| eq_fold(&f.name, &name.text))
            {
                for s in &f.signatures {
                    lines.push(format!("```sql\n{s}\n```"));
                }
                if let Some(d) = &f.description {
                    lines.push(d.clone());
                }
            }
            if lines.is_empty() {
                if let Some(o) = objects.iter().find(|o| o.name().matches(&name.text)) {
                    return Some(format!("macro `{}`, defined in this script", o.name()));
                }
                return None;
            }
            Some(lines.join("\n\n"))
        }
        (SyntaxKind::PATH, Some(SyntaxKind::TABLE_REF)) => {
            let item = ast::FromItem::cast(grand.clone()?)?;
            let path = item.path();
            let is_last = path.last().is_some_and(|l| l == &name);
            if !is_last {
                return Some(format!("schema `{}`", name));
            }
            // A CTE or a table: resolve it as the scope would.
            let rel = scope::item_relations(&item).into_iter().next()?;
            Some(relation_markdown(&rel, catalog, &objects))
        }
        (SyntaxKind::PATH, Some(SyntaxKind::COLUMN_REF)) => {
            let parts: Vec<Name> = ast::Path::cast(parent.clone())?.parts();
            let index = parts.iter().position(|p| p == &name)?;
            let scope = scope::scope_at(root, token.text_range().start());
            if index + 1 < parts.len() {
                // a qualifier: the relation it names
                let rel = scope.resolve(&parts[..=index]).into_iter().next()?;
                return Some(relation_markdown(rel, catalog, &objects));
            }
            let qualifier = &parts[..index];
            let candidates: Vec<&VisibleRelation> = if qualifier.is_empty() {
                scope.relations.iter().collect()
            } else {
                scope.resolve(qualifier)
            };
            for rel in candidates {
                if let Some(c) = relation_columns(rel, catalog, &objects, 0)
                    .into_iter()
                    .find(|c| eq_fold(&c.name, &name.text))
                {
                    let ty = c.data_type.map(|t| format!(" `{t}`")).unwrap_or_default();
                    return Some(format!(
                        "column `{}`{ty} of {}",
                        c.name,
                        describe_relation(rel)
                    ));
                }
            }
            if scope.aliases.iter().any(|a| a.matches(&name.text)) {
                return Some(format!("select-list alias `{name}`"));
            }
            None
        }
        (SyntaxKind::NAME, Some(SyntaxKind::ALIAS)) => {
            let item = ast::FromItem::cast(grand.as_ref()?.parent()?)?;
            let rel = scope::item_relations(&item).into_iter().next()?;
            Some(format!("alias for {}", describe_relation(&rel)))
        }
        (SyntaxKind::NAME, Some(SyntaxKind::CTE)) => {
            let cte = ast::Cte::cast(grand.clone()?)?;
            let query = cte.query()?;
            let columns = scope::query_columns(&query);
            let rel = VisibleRelation {
                alias: None,
                source: RelationSource::Cte { name: name.clone() },
                columns,
                depth: 0,
            };
            Some(relation_markdown(&rel, catalog, &objects))
        }
        _ => None,
    }
}

fn describe_relation(rel: &VisibleRelation) -> String {
    match &rel.source {
        RelationSource::Table { path } => {
            format!(
                "table `{}`",
                path.iter()
                    .map(|n| n.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            )
        }
        RelationSource::File { path } => format!("file `{path}`"),
        RelationSource::TableFunction { name, .. } => {
            format!(
                "`{}(...)`",
                name.iter()
                    .map(|n| n.text.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            )
        }
        RelationSource::Cte { name } => format!("CTE `{name}`"),
        RelationSource::Subquery => "a subquery".into(),
        RelationSource::Values => "a VALUES list".into(),
        RelationSource::Pivot => "a PIVOT".into(),
    }
}

fn relation_markdown(
    rel: &VisibleRelation,
    catalog: &dyn Catalog,
    objects: &[crate::DocumentObject],
) -> String {
    let mut out = describe_relation(rel);
    let columns = relation_columns(rel, catalog, objects, 0);
    if !columns.is_empty() {
        out.push_str("\n\n");
        for c in columns.iter().take(50) {
            match &c.data_type {
                Some(t) => out.push_str(&format!("- `{}` {t}\n", c.name)),
                None => out.push_str(&format!("- `{}`\n", c.name)),
            }
        }
        if columns.len() > 50 {
            out.push_str(&format!("- ... {} more\n", columns.len() - 50));
        }
    }
    out
}
