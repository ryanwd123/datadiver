//! Signature help: the innermost call around a position and its active argument.

use rowan::{TextRange, TextSize};

use crate::ast::{self, AstNode};
use crate::catalog::Catalog;
use crate::complete::{Name, eq_fold};
use crate::{SyntaxKind, SyntaxNode, parse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    /// The function's name, with its schema when written: `[main, my_macro]`.
    pub function: Vec<Name>,
    /// The argument the position is in, counting from 0.
    pub active_argument: usize,
    /// The name of a named argument (`header := true`) the position is in.
    pub active_name: Option<String>,
    /// The parenthesized argument list.
    pub arguments: TextRange,
    /// Signatures from the catalog.
    pub signatures: Vec<String>,
    pub description: Option<String>,
}

/// The call around `offset`, without catalog signatures.
pub fn signature_help(text: &str, offset: TextSize) -> Option<SignatureHelp> {
    let root = parse(text).syntax();
    let args = innermost_args(&root, offset)?;
    let call = args.parent()?;
    let function = callee(&call)?;
    let mut active_argument = 0;
    let mut active_name = None;
    for element in args.children_with_tokens() {
        if element.text_range().start() >= offset {
            break;
        }
        if element.kind() == SyntaxKind::COMMA {
            active_argument += 1;
            active_name = None;
        }
        if let Some(node) = element.as_node()
            && node.kind() == SyntaxKind::NAMED_ARG
        {
            active_name = ast::child_of_kind(node, SyntaxKind::NAME)
                .and_then(|n| ast::first_name(&n))
                .map(|n| n.text);
        }
    }
    Some(SignatureHelp {
        function,
        active_argument,
        active_name,
        arguments: args.text_range(),
        signatures: Vec::new(),
        description: None,
    })
}

/// `signature_help` with the function's signatures and description from the catalog.
pub fn signature_help_with(
    text: &str,
    offset: TextSize,
    catalog: &dyn Catalog,
) -> Option<SignatureHelp> {
    let mut help = signature_help(text, offset)?;
    let name = help.function.last()?.text.clone();
    let schema =
        (help.function.len() > 1).then(|| help.function[help.function.len() - 2].text.clone());
    for f in catalog.functions() {
        let schema_ok = match (&schema, &f.schema) {
            (Some(s), Some(fs)) => eq_fold(s, fs),
            (Some(_), None) => false,
            _ => true,
        };
        if eq_fold(&f.name, &name) && schema_ok {
            help.signatures.extend(f.signatures.iter().cloned());
            if help.description.is_none() {
                help.description = f.description.clone();
            }
        }
    }
    Some(help)
}

/// The innermost argument list whose parentheses hold `offset`; an unclosed
/// one reaches to the end of its statement.
fn innermost_args(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    root.descendants()
        .filter(|n| n.kind() == SyntaxKind::ARG_LIST)
        .filter(|args| {
            let tokens: Vec<_> = args
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .collect();
            let Some(open) = tokens.iter().find(|t| t.kind() == SyntaxKind::L_PAREN) else {
                return false;
            };
            if offset < open.text_range().end() {
                return false;
            }
            match tokens
                .iter()
                .rev()
                .find(|t| t.kind() == SyntaxKind::R_PAREN)
            {
                Some(close) => offset <= close.text_range().start(),
                None => {
                    let stmt = args.ancestors().find(|a| {
                        a.parent()
                            .is_some_and(|p| p.kind() == SyntaxKind::SOURCE_FILE)
                    });
                    let end = stmt
                        .map(|s| s.text_range().end())
                        .unwrap_or(args.text_range().end());
                    // trailing whitespace after an unclosed call belongs to it
                    offset <= end || root.text_range().end() == offset
                }
            }
        })
        .max_by_key(|args| args.text_range().start())
}

/// The name of the function an argument list belongs to.
fn callee(call: &SyntaxNode) -> Option<Vec<Name>> {
    match call.kind() {
        SyntaxKind::CALL_EXPR
        | SyntaxKind::TABLE_FUNCTION
        | SyntaxKind::SPECIAL_CALL
        | SyntaxKind::PRAGMA_STMT => {
            if let Some(path) = call.children().find_map(ast::Path::cast) {
                return Some(path.parts());
            }
            // `x.method(...)`
            let field = call
                .children()
                .find(|c| c.kind() == SyntaxKind::FIELD_EXPR)?;
            let name =
                ast::child_of_kind(&field, SyntaxKind::NAME).and_then(|n| ast::first_name(&n))?;
            Some(vec![name])
        }
        _ => None,
    }
}
