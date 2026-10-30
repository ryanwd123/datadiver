//! Folding and selection ranges from the tree.

use rowan::{TextRange, TextSize};

use crate::{Parse, SyntaxKind, SyntaxNode};

/// Ranges an editor can fold: multi-line statements, CTEs, subqueries, CASE
/// expressions, parenthesized lists and block comments. Sorted by start.
pub fn folding_ranges(parse: &Parse) -> Vec<TextRange> {
    let root = parse.syntax();
    let text = root.text().to_string();
    let multiline = |r: TextRange| text[r].contains('\n');
    let mut out = Vec::new();
    for node in root.descendants() {
        let foldable = node
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::SOURCE_FILE)
            || matches!(
                node.kind(),
                SyntaxKind::CTE
                    | SyntaxKind::SUBQUERY_EXPR
                    | SyntaxKind::SUBQUERY_REF
                    | SyntaxKind::PAREN_QUERY
                    | SyntaxKind::CASE_EXPR
                    | SyntaxKind::ARG_LIST
                    | SyntaxKind::TABLE_ELEMENTS
                    | SyntaxKind::VALUES_CLAUSE
                    | SyntaxKind::ROW_EXPR
                    | SyntaxKind::LIST_EXPR
                    | SyntaxKind::STRUCT_EXPR
                    | SyntaxKind::WINDOW_SPEC
                    | SyntaxKind::OPTION_LIST
            );
        if foldable && multiline(node.text_range()) {
            out.push(node.text_range());
        }
    }
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
    {
        if matches!(
            token.kind(),
            SyntaxKind::BLOCK_COMMENT | SyntaxKind::UNTERMINATED_BLOCK_COMMENT
        ) && multiline(token.text_range())
        {
            out.push(token.text_range());
        }
    }
    out.sort_by_key(|r| (r.start(), std::cmp::Reverse(r.end())));
    out.dedup();
    out
}

/// Ranges to expand a selection at `offset` through, innermost first: the
/// token, then each enclosing node with a larger range.
pub fn selection_ranges(parse: &Parse, offset: TextSize) -> Vec<TextRange> {
    let root = parse.syntax();
    let token = match root.token_at_offset(offset) {
        rowan::TokenAtOffset::None => return Vec::new(),
        rowan::TokenAtOffset::Single(t) => t,
        rowan::TokenAtOffset::Between(l, r) => {
            if r.kind().is_trivia() || r.kind().is_punct() && !l.kind().is_trivia() {
                l
            } else {
                r
            }
        }
    };
    let mut out = vec![token.text_range()];
    let ancestors = std::iter::successors(token.parent(), SyntaxNode::parent);
    for node in ancestors {
        let range = node.text_range();
        if out
            .last()
            .is_none_or(|last| range != *last && range.contains_range(*last))
        {
            out.push(range);
        }
    }
    out
}
