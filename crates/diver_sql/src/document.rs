//! Objects a script creates itself (`CREATE TABLE`, views, macros, types,
//! schemas), so completion offers them before the script has run.

use rowan::TextRange;

use crate::ast::{self, AstNode, Query};
use crate::complete::Name;
use crate::scope::{self, ColumnSource, Columns};
use crate::{Parse, SyntaxKind, SyntaxNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Table,
    View,
    Macro,
    TableMacro,
    Type,
    Schema,
    Sequence,
    /// `SET VARIABLE name = …`
    Variable,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentObject {
    pub kind: ObjectKind,
    /// `[schema, name]` or `[name]`, as written.
    pub path: Vec<Name>,
    pub columns: Columns,
    /// The CREATE statement.
    pub range: TextRange,
}

impl DocumentObject {
    pub fn name(&self) -> Name {
        self.path.last().cloned().unwrap_or_else(|| Name::new(""))
    }

    /// Whether a reference `path` (`t`, `s.t`, `db.s.t`) names this object.
    pub fn matches_path(&self, path: &[Name]) -> bool {
        let (Some(last), Some(own)) = (path.last(), self.path.last()) else {
            return false;
        };
        if !own.matches(&last.text) {
            return false;
        }
        match (&path[..path.len() - 1], &self.path[..self.path.len() - 1]) {
            ([], []) => true,
            ([], [s]) => s.matches("main"),
            ([s], []) => s.matches("main"),
            ([s], [own]) => own.matches(&s.text),
            ([_, s], [own]) => own.matches(&s.text),
            _ => false,
        }
    }
}

/// What the script creates, in order.
pub fn document_objects(parse: &Parse) -> Vec<DocumentObject> {
    let root = parse.syntax();
    root.children()
        .filter_map(|s| match s.kind() {
            SyntaxKind::CREATE_STMT => object(&s),
            SyntaxKind::SET_STMT => variable(&s),
            _ => None,
        })
        .collect()
}

/// The variable `SET VARIABLE name = …` makes.
fn variable(stmt: &SyntaxNode) -> Option<DocumentObject> {
    if !stmt
        .children_with_tokens()
        .any(|e| e.kind() == SyntaxKind::VARIABLE_KW)
    {
        return None;
    }
    let name = ast::child_of_kind(stmt, SyntaxKind::NAME).and_then(|n| ast::first_name(&n))?;
    Some(DocumentObject {
        kind: ObjectKind::Variable,
        path: vec![name],
        columns: Columns::Unknown,
        range: stmt.text_range(),
    })
}

fn object(stmt: &SyntaxNode) -> Option<DocumentObject> {
    let keyword = |kind: SyntaxKind| stmt.children_with_tokens().any(|e| e.kind() == kind);
    let path = stmt.children().find_map(ast::Path::cast)?.parts();
    let query = stmt.children().find_map(Query::cast);
    let (kind, columns) = if keyword(SyntaxKind::TABLE_KW)
        && !keyword(SyntaxKind::MACRO_KW)
        && !keyword(SyntaxKind::FUNCTION_KW)
    {
        let columns = match ast::child_of_kind(stmt, SyntaxKind::TABLE_ELEMENTS) {
            Some(elements) => Columns::Known(
                elements
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::COLUMN_DEF)
                    .filter_map(|c| {
                        ast::child_of_kind(&c, SyntaxKind::NAME).and_then(|n| ast::first_name(&n))
                    })
                    .map(ColumnSource::Named)
                    .collect(),
            ),
            None => query
                .as_ref()
                .map(scope::query_columns)
                .unwrap_or(Columns::Unknown),
        };
        (ObjectKind::Table, columns)
    } else if keyword(SyntaxKind::VIEW_KW) {
        let mut columns = query
            .as_ref()
            .map(scope::query_columns)
            .unwrap_or(Columns::Unknown);
        let aliases =
            ast::child_of_kind(stmt, SyntaxKind::COLUMN_ALIASES).map(|c| ast::name_children(&c));
        if let Some(aliases) = aliases.filter(|a| !a.is_empty()) {
            columns = Columns::Known(aliases.into_iter().map(ColumnSource::Named).collect());
        }
        (ObjectKind::View, columns)
    } else if keyword(SyntaxKind::MACRO_KW) || keyword(SyntaxKind::FUNCTION_KW) {
        match query {
            Some(q) => (ObjectKind::TableMacro, scope::query_columns(&q)),
            None => (ObjectKind::Macro, Columns::Unknown),
        }
    } else if keyword(SyntaxKind::TYPE_KW) {
        (ObjectKind::Type, Columns::Unknown)
    } else if keyword(SyntaxKind::SCHEMA_KW) {
        (ObjectKind::Schema, Columns::Unknown)
    } else if keyword(SyntaxKind::SEQUENCE_KW) {
        (ObjectKind::Sequence, Columns::Unknown)
    } else {
        return None;
    };
    Some(DocumentObject {
        kind,
        path,
        columns,
        range: stmt.text_range(),
    })
}
