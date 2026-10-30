//! Semantic highlighting: a class per token, from where the token sits in the tree.

use std::collections::HashSet;

use rowan::TextRange;

use crate::ast;
use crate::catalog::Catalog;
use crate::complete::{FunctionKind, Name};
use crate::{Parse, SyntaxKind, SyntaxNode, SyntaxToken, dialect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightClass {
    Keyword,
    Function,
    Aggregate,
    Type,
    Table,
    Column,
    Alias,
    Cte,
    Schema,
    Parameter,
    String,
    FilePath,
    Number,
    Operator,
    Comment,
    Error,
    /// A name the tree doesn't classify further.
    Identifier,
}

/// Classes for every token that has one, in text order.
pub fn highlight(parse: &Parse) -> Vec<(TextRange, HighlightClass)> {
    let root = parse.syntax();
    let ctes = cte_names(&root);
    root.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter_map(|t| classify(&t, &ctes).map(|c| (t.text_range(), c)))
        .collect()
}

/// `highlight`, with function kinds from the catalog: aggregates get their own class.
pub fn highlight_with(parse: &Parse, catalog: &dyn Catalog) -> Vec<(TextRange, HighlightClass)> {
    let aggregates: HashSet<String> = catalog
        .functions()
        .into_iter()
        .filter(|f| f.kind == FunctionKind::Aggregate)
        .map(|f| f.name.to_lowercase())
        .collect();
    let root = parse.syntax();
    let mut out = highlight(parse);
    for (range, class) in &mut out {
        if *class == HighlightClass::Function {
            let text = root.text().slice(*range).to_string();
            if aggregates.contains(
                &Name::from_token(SyntaxKind::QUOTED_IDENT, &text)
                    .text
                    .to_lowercase(),
            ) || aggregates.contains(&text.to_lowercase())
            {
                *class = HighlightClass::Aggregate;
            }
        }
    }
    out
}

fn cte_names(root: &SyntaxNode) -> HashSet<String> {
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::CTE)
        .filter_map(|n| ast::child_of_kind(&n, SyntaxKind::NAME).and_then(|n| ast::first_name(&n)))
        .map(|n| n.text.to_lowercase())
        .collect()
}

fn classify(token: &SyntaxToken, ctes: &HashSet<String>) -> Option<HighlightClass> {
    use HighlightClass as H;
    let kind = token.kind();
    Some(match kind {
        SyntaxKind::WHITESPACE => return None,
        k if k.is_comment() => H::Comment,
        k if k.is_keyword() => H::Keyword,
        k if k.is_string() => {
            if is_path_string(token) {
                H::FilePath
            } else {
                H::String
            }
        }
        SyntaxKind::INT_NUMBER | SyntaxKind::FLOAT_NUMBER => H::Number,
        SyntaxKind::PARAM => H::Parameter,
        SyntaxKind::ERROR_TOKEN => H::Error,
        SyntaxKind::IDENT | SyntaxKind::QUOTED_IDENT | SyntaxKind::UNTERMINATED_QUOTED_IDENT => {
            name_class(token, ctes)
        }
        SyntaxKind::L_PAREN
        | SyntaxKind::R_PAREN
        | SyntaxKind::L_BRACK
        | SyntaxKind::R_BRACK
        | SyntaxKind::L_CURLY
        | SyntaxKind::R_CURLY
        | SyntaxKind::COMMA
        | SyntaxKind::SEMICOLON
        | SyntaxKind::DOT
        | SyntaxKind::COLON => return None,
        k if k.is_punct() => H::Operator,
        _ => return None,
    })
}

/// A string naming a file: `FROM 'x.parquet'`, `read_csv('x.csv')`, COPY's target, ...
fn is_path_string(token: &SyntaxToken) -> bool {
    let Some(parent) = token.parent() else {
        return false;
    };
    match parent.kind() {
        SyntaxKind::FILE_REF => true,
        SyntaxKind::COPY_STMT
        | SyntaxKind::ATTACH_STMT
        | SyntaxKind::EXPORT_STMT
        | SyntaxKind::IMPORT_STMT => true,
        SyntaxKind::LITERAL => {
            // the first argument of a path function
            let Some(args) = parent.parent().filter(|a| a.kind() == SyntaxKind::ARG_LIST) else {
                return false;
            };
            let first = args.children().next();
            if first.as_ref() != Some(&parent) {
                return false;
            }
            let callee = args
                .parent()
                .and_then(|c| c.children().find(|n| n.kind() == SyntaxKind::PATH));
            callee
                .and_then(|p| ast::Path::cast_node(p).parts().pop())
                .is_some_and(|n| dialect::takes_path(&n.text))
        }
        _ => false,
    }
}

fn name_class(token: &SyntaxToken, ctes: &HashSet<String>) -> HighlightClass {
    use HighlightClass as H;
    let Some(parent) = token.parent() else {
        return H::Identifier;
    };
    let grand = parent.parent();
    let grand_kind = grand.as_ref().map(|g| g.kind());
    match parent.kind() {
        SyntaxKind::PATH => {
            let is_last = !token
                .siblings_with_tokens(rowan::Direction::Next)
                .skip(1)
                .any(|e| ast::is_name_token(e.kind()));
            match grand_kind {
                Some(
                    SyntaxKind::CALL_EXPR | SyntaxKind::TABLE_FUNCTION | SyntaxKind::SPECIAL_CALL,
                ) => {
                    if is_last {
                        H::Function
                    } else {
                        H::Schema
                    }
                }
                Some(SyntaxKind::TABLE_REF) => {
                    if !is_last {
                        H::Schema
                    } else if ctes.contains(
                        &Name::from_token(token.kind(), token.text())
                            .text
                            .to_lowercase(),
                    ) && parent
                        .children_with_tokens()
                        .filter(|e| ast::is_name_token(e.kind()))
                        .count()
                        == 1
                    {
                        H::Cte
                    } else {
                        H::Table
                    }
                }
                Some(SyntaxKind::COLUMN_REF) => {
                    if is_last {
                        H::Column
                    } else {
                        H::Alias
                    }
                }
                Some(SyntaxKind::STAR_EXPR) => H::Alias,
                Some(SyntaxKind::TYPE_NAME) => H::Type,
                Some(
                    SyntaxKind::NAME_LIST
                    | SyntaxKind::RENAME_ITEM
                    | SyntaxKind::SET_ITEM
                    | SyntaxKind::INSERT_COLUMNS,
                ) => H::Column,
                Some(SyntaxKind::CREATE_STMT) => {
                    let create = grand.unwrap();
                    let has = |k: SyntaxKind| create.children_with_tokens().any(|e| e.kind() == k);
                    if !is_last || has(SyntaxKind::SCHEMA_KW) {
                        H::Schema
                    } else if has(SyntaxKind::MACRO_KW) || has(SyntaxKind::FUNCTION_KW) {
                        H::Function
                    } else if has(SyntaxKind::TYPE_KW) {
                        H::Type
                    } else {
                        H::Table
                    }
                }
                Some(SyntaxKind::DROP_STMT | SyntaxKind::ALTER_STMT | SyntaxKind::COMMENT_STMT) => {
                    if is_last {
                        H::Table
                    } else {
                        H::Schema
                    }
                }
                Some(SyntaxKind::USE_STMT) => H::Schema,
                _ => H::Identifier,
            }
        }
        SyntaxKind::NAME => match grand_kind {
            Some(SyntaxKind::ALIAS) => H::Alias,
            Some(SyntaxKind::CTE) => H::Cte,
            Some(
                SyntaxKind::COLUMN_DEF | SyntaxKind::COLUMN_ALIASES | SyntaxKind::STRUCT_TYPE_FIELD,
            ) => H::Column,
            Some(SyntaxKind::RENAME_ITEM | SyntaxKind::REPLACE_ITEM) => H::Column,
            Some(SyntaxKind::MACRO_PARAM | SyntaxKind::NAMED_ARG | SyntaxKind::LAMBDA_PARAMS) => {
                H::Parameter
            }
            Some(SyntaxKind::WINDOW_DEF | SyntaxKind::WINDOW_SPEC | SyntaxKind::OVER_CLAUSE) => {
                H::Alias
            }
            Some(SyntaxKind::OPTION) => H::Keyword,
            Some(SyntaxKind::FIELD_EXPR) => H::Column,
            Some(SyntaxKind::LIST_EXPR) => H::Parameter,
            _ => H::Identifier,
        },
        SyntaxKind::TYPE_NAME => H::Type,
        SyntaxKind::INTERVAL_EXPR | SyntaxKind::SPECIAL_CALL | SyntaxKind::SAMPLE_CLAUSE => {
            H::Keyword
        }
        SyntaxKind::COLUMN_DEF => H::Column,
        _ => H::Identifier,
    }
}

impl ast::Path {
    fn cast_node(node: SyntaxNode) -> ast::Path {
        <ast::Path as ast::AstNode>::cast(node).expect("a PATH node")
    }
}
