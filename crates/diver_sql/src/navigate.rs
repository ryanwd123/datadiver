//! Where names are defined and used: CTEs, relation aliases and the columns
//! queries give names to. Tables and their columns are the catalog's, so
//! they have no definition in the text.

use rowan::{TextRange, TextSize};

use crate::ast::{self, AstNode, Cte, FromItem, Query, SelectCore, SelectItem};
use crate::complete::Name;
use crate::edit::token_near;
use crate::query::resolve_cte;
use crate::{SyntaxKind, SyntaxNode, SyntaxToken, parse};

/// The name that defines what the name at `offset` refers to: a CTE's
/// name, a relation's alias, a column's alias or the column a subquery
/// selects. A definition is its own definition.
pub fn definition(text: &str, offset: TextSize) -> Option<TextRange> {
    let root = parse(text).syntax();
    let token = token_near(&root, offset).filter(is_name)?;
    definition_of(&token)
}

/// Every use of what the name at `offset` refers to, its definition
/// included, in text order.
pub fn references(text: &str, offset: TextSize) -> Vec<TextRange> {
    let root = parse(text).syntax();
    let Some(token) = token_near(&root, offset).filter(is_name) else {
        return Vec::new();
    };
    let Some(def) = definition_of(&token) else {
        return Vec::new();
    };
    let Some(name) = root.covering_element(def).into_token().map(|t| ast::token_name(&t)) else {
        return Vec::new();
    };
    let mut out: Vec<TextRange> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| is_name(t) && name.matches(&ast::token_name(t).text))
        .filter(|t| definition_of(t) == Some(def))
        .map(|t| t.text_range())
        .collect();
    out.push(def);
    out.sort_by_key(|r| r.start());
    out.dedup();
    out
}

/// Whether the definition at `range` is a name the script gives (a CTE or
/// an alias), which renaming can change everywhere it is used.
pub fn is_renamable(text: &str, range: TextRange) -> bool {
    let root = parse(text).syntax();
    root.covering_element(range)
        .into_token()
        .and_then(|t| t.parent())
        .is_some_and(|p| matches!(p.kind(), SyntaxKind::NAME))
}

fn is_name(token: &SyntaxToken) -> bool {
    matches!(token.kind(), SyntaxKind::IDENT | SyntaxKind::QUOTED_IDENT)
        || (token.kind().is_keyword() && token.parent().is_some_and(|p| matches!(p.kind(), SyntaxKind::NAME | SyntaxKind::PATH)))
}

fn definition_of(token: &SyntaxToken) -> Option<TextRange> {
    definition_at(token, 0)
}

/// How far definitions are followed through queries that select a column
/// from another one (and loops, in recursive CTEs).
const MAX_DEPTH: u32 = 8;

fn definition_at(token: &SyntaxToken, depth: u32) -> Option<TextRange> {
    let parent = token.parent()?;
    match parent.kind() {
        SyntaxKind::NAME => {
            let owner = parent.parent()?;
            match owner.kind() {
                SyntaxKind::CTE | SyntaxKind::ALIAS | SyntaxKind::COLUMN_ALIASES => Some(token.text_range()),
                _ => None,
            }
        }
        SyntaxKind::PATH => {
            let owner = parent.parent()?;
            let parts: Vec<SyntaxToken> = parent
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| ast::is_name_token(t.kind()))
                .collect();
            let index = parts.iter().position(|t| t == token)?;
            match owner.kind() {
                SyntaxKind::TABLE_REF if parts.len() == 1 => cte_name(&resolve_cte(&owner)?.cte),
                SyntaxKind::COLUMN_REF if index + 1 == parts.len() => {
                    let qualifier = (index > 0).then(|| ast::token_name(&parts[index - 1]));
                    column_definition(&owner, qualifier.as_ref(), &ast::token_name(token), depth)
                }
                SyntaxKind::COLUMN_REF if index + 2 == parts.len() => relation_definition(&owner, &ast::token_name(token)),
                SyntaxKind::STAR_EXPR if index + 1 == parts.len() => relation_definition(&owner, &ast::token_name(token)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn cte_name(cte: &Cte) -> Option<TextRange> {
    let name = ast::child_of_kind(cte.syntax(), SyntaxKind::NAME)?;
    name_token(&name).map(|t| t.text_range())
}

fn name_token(name: &SyntaxNode) -> Option<SyntaxToken> {
    name.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| ast::is_name_token(t.kind()))
}

/// The query blocks a reference at `node` sees, innermost first. ORDER BY
/// and LIMIT of a query see its first block.
fn cores_around(node: &SyntaxNode) -> Vec<SelectCore> {
    let mut out = Vec::new();
    let mut child = node.clone();
    for ancestor in node.ancestors() {
        if let Some(core) = SelectCore::cast(ancestor.clone()) {
            out.push(core);
        } else if let Some(query) = Query::cast(ancestor.clone())
            && child.kind() != SyntaxKind::SELECT_CORE
            && child.kind() != SyntaxKind::SET_OP
            && let Some(first) = query.first_core().and_then(SelectCore::cast)
        {
            out.push(first);
        }
        child = ancestor;
    }
    out
}

/// The FROM items of a block, joins flattened.
fn relations(core: &SelectCore) -> Vec<FromItem> {
    fn walk(item: FromItem, out: &mut Vec<FromItem>) {
        match item.kind() {
            SyntaxKind::JOIN_EXPR | SyntaxKind::PAREN_JOIN => {
                for inner in item.inner_items() {
                    walk(inner, out);
                }
            }
            _ => out.push(item),
        }
    }
    let mut out = Vec::new();
    for item in core.from_items() {
        walk(item, &mut out);
    }
    out
}

/// The name a FROM item is referred to by: its alias, a CTE or the item.
fn item_name_definition(item: &FromItem, name: &Name) -> Option<TextRange> {
    if let Some(alias) = item.alias() {
        let alias_name = alias.name()?;
        if !alias_name.matches(&name.text) {
            return None;
        }
        let node = ast::child_of_kind(alias.syntax(), SyntaxKind::NAME)?;
        return name_token(&node).map(|t| t.text_range());
    }
    match item.kind() {
        SyntaxKind::TABLE_REF => {
            let last = item.path().pop()?;
            if !last.matches(&name.text) {
                return None;
            }
            if let Some(def) = resolve_cte(item.syntax()) {
                return cte_name(&def.cte);
            }
            let path = item.syntax().children().find_map(ast::Path::cast)?;
            path.syntax()
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| ast::is_name_token(t.kind()))
                .last()
                .map(|t| t.text_range())
        }
        _ => None,
    }
}

fn relation_definition(node: &SyntaxNode, name: &Name) -> Option<TextRange> {
    cores_around(node)
        .iter()
        .flat_map(relations)
        .find_map(|item| item_name_definition(&item, name))
}

/// Where a query names its output column `name`.
fn output_column(query: &Query, name: &Name, depth: u32) -> Option<TextRange> {
    let core = query.first_core().and_then(SelectCore::cast)?;
    core.select_items()
        .iter()
        .find(|item| item.output_name().is_some_and(|n| n.matches(&name.text)))
        .and_then(|item| item_name_range(item, depth))
}

/// The alias of a select item; without one, where the column it selects is
/// defined, or that column itself.
fn item_name_range(item: &SelectItem, depth: u32) -> Option<TextRange> {
    if let Some(alias) = item.syntax().children().find(|c| c.kind() == SyntaxKind::ALIAS) {
        let node = ast::child_of_kind(&alias, SyntaxKind::NAME)?;
        return name_token(&node).map(|t| t.text_range());
    }
    let token = item
        .syntax()
        .descendants()
        .find(|n| n.kind() == SyntaxKind::PATH)?
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| ast::is_name_token(t.kind()))
        .last()?;
    if depth < MAX_DEPTH
        && let Some(def) = definition_at(&token, depth + 1)
    {
        return Some(def);
    }
    Some(token.text_range())
}

/// Where a relation in FROM gets its column `name`, when the text says.
fn item_column(item: &FromItem, name: &Name, depth: u32) -> Option<TextRange> {
    if let Some(alias) = item.alias()
        && let Some(columns) = ast::child_of_kind(alias.syntax(), SyntaxKind::COLUMN_ALIASES)
    {
        for node in columns.children().filter(|c| c.kind() == SyntaxKind::NAME) {
            if ast::first_name(&node).is_some_and(|n| n.matches(&name.text)) {
                return name_token(&node).map(|t| t.text_range());
            }
        }
    }
    match item.kind() {
        SyntaxKind::SUBQUERY_REF => output_column(&item.query()?, name, depth),
        SyntaxKind::TABLE_REF => {
            let cte = resolve_cte(item.syntax())?.cte;
            if let Some(columns) = ast::child_of_kind(cte.syntax(), SyntaxKind::COLUMN_ALIASES) {
                return columns
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::NAME)
                    .find(|n| ast::first_name(n).is_some_and(|n| n.matches(&name.text)))
                    .and_then(|n| name_token(&n))
                    .map(|t| t.text_range());
            }
            output_column(&cte.query()?, name, depth)
        }
        _ => None,
    }
}

fn column_definition(column_ref: &SyntaxNode, qualifier: Option<&Name>, name: &Name, depth: u32) -> Option<TextRange> {
    // ORDER BY of a query names its output columns first.
    if qualifier.is_none()
        && let Some(order) = column_ref.ancestors().find(|n| n.kind() == SyntaxKind::ORDER_BY_CLAUSE)
        && order.parent().is_some_and(|p| p.kind() == SyntaxKind::QUERY)
        && column_ref.ancestors().take_while(|n| *n != order).all(|n| n.kind() != SyntaxKind::SELECT_CORE)
        && let Some(found) = output_column(&Query::cast(order.parent()?)?, name, depth)
        && !found.contains_range(column_ref.text_range())
    {
        return Some(found);
    }
    for core in cores_around(column_ref) {
        let items = relations(&core);
        if let Some(qualifier) = qualifier {
            let item = items.iter().find(|item| item_name_definition(item, qualifier).is_some());
            if let Some(item) = item {
                return item_column(item, name, depth);
            }
            continue;
        }
        if let Some(found) = items.iter().find_map(|item| item_column(item, name, depth)) {
            return Some(found);
        }
        // Select-list aliases, which DuckDB lets the rest of the block use.
        let own_item = column_ref.ancestors().find(|n| n.kind() == SyntaxKind::SELECT_ITEM);
        let alias = core.select_items().into_iter().find(|item| {
            Some(item.syntax()) != own_item.as_ref() && item.alias().is_some_and(|a| a.matches(&name.text))
        });
        if let Some(item) = alias {
            return item_name_range(&item, depth);
        }
        // A table or file whose columns only the catalog knows may have it.
        if !items.is_empty() {
            return None;
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutlineKind {
    Statement,
    Cte,
}

/// A place to jump to: a statement or a CTE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub kind: OutlineKind,
    pub label: String,
    /// What to select: the CTE's name, or the statement.
    pub range: TextRange,
    /// How many CTEs it is inside.
    pub depth: usize,
}

/// The statements of the script and the CTEs in them, in text order.
pub fn outline(text: &str) -> Vec<OutlineItem> {
    let root = parse(text).syntax();
    let mut out = Vec::new();
    for statement in root.children() {
        let words = statement.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
        if words.is_empty() {
            continue;
        }
        let label = match words.char_indices().nth(80) {
            Some((i, _)) => format!("{}…", &words[..i]),
            None => words,
        };
        let start = statement
            .descendants_with_tokens()
            .find(|e| !e.kind().is_trivia() && e.as_token().is_some())
            .map_or(statement.text_range().start(), |e| e.text_range().start());
        out.push(OutlineItem {
            kind: OutlineKind::Statement,
            label,
            range: TextRange::empty(start),
            depth: 0,
        });
        for cte in statement.descendants().filter_map(Cte::cast) {
            let (Some(name), Some(range)) = (cte.name(), cte_name(&cte)) else {
                continue;
            };
            let depth = cte.syntax().ancestors().skip(1).filter(|n| n.kind() == SyntaxKind::CTE).count() + 1;
            out.push(OutlineItem {
                kind: OutlineKind::Cte,
                label: name.text,
                range,
                depth,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::{size, span};

    fn nth(text: &str, needle: &str, n: usize) -> TextSize {
        size(text.match_indices(needle).nth(n).unwrap().0)
    }

    fn texts<'a>(text: &'a str, ranges: &[TextRange]) -> Vec<(usize, &'a str)> {
        ranges.iter().map(|r| (usize::from(r.start()), &text[span(*r)])).collect()
    }

    const SQL: &str = "with orders as (select id, sum(amount) total from raw group by id)\nselect o.id, o.total, total from orders o where o.total > 0 order by total";

    #[test]
    fn ctes_aliases_and_columns_have_definitions() {
        // `orders` in FROM is the CTE.
        let def = definition(SQL, nth(SQL, "orders", 1)).unwrap();
        assert_eq!(def.start(), nth(SQL, "orders", 0));
        // `o.` is the alias.
        let def = definition(SQL, nth(SQL, "o.id", 0)).unwrap();
        assert_eq!(def.start(), nth(SQL, "o where", 0));
        // `o.total` and a bare `total` are the CTE's column alias.
        let alias = nth(SQL, "total", 0);
        assert_eq!(definition(SQL, nth(SQL, "total", 1)).unwrap().start(), alias);
        assert_eq!(definition(SQL, nth(SQL, "total", 2)).unwrap().start(), alias);
        // `raw` is a table: the catalog's.
        assert_eq!(definition(SQL, nth(SQL, "raw", 0)), None);
    }

    #[test]
    fn references_find_every_use() {
        let refs = references(SQL, nth(SQL, "o where", 0));
        assert_eq!(texts(SQL, &refs).len(), 4);
        let refs = references(SQL, nth(SQL, "orders", 0));
        assert_eq!(texts(SQL, &refs), [(5, "orders"), (usize::from(nth(SQL, "orders", 1)), "orders")]);
        // The ORDER BY's `total` is the output column `o.total`, so the CTE's alias.
        let refs = references(SQL, nth(SQL, "total", 4));
        assert_eq!(refs.len(), 5, "{:?}", texts(SQL, &refs));
        assert!(is_renamable(SQL, refs[0]));
        // A column the outer query takes from a table's column is that column.
        let text = "with a as (select x from t) select x from a";
        let refs = references(text, nth(text, "x", 1));
        assert_eq!(texts(text, &refs), [(18, "x"), (35, "x")]);
        assert!(!is_renamable(text, refs[0]));
    }

    #[test]
    fn outline_lists_statements_and_ctes() {
        let items = outline("with a as (select 1), b as (with c as (select 2) select * from c) select 1;\nselect 2");
        let labels: Vec<(&str, usize)> = items.iter().map(|i| (i.label.as_str(), i.depth)).collect();
        assert_eq!(
            labels,
            [
                ("with a as (select 1), b as (with c as (select 2) select * from c) select 1", 0),
                ("a", 1),
                ("b", 1),
                ("c", 2),
                ("select 2", 0)
            ]
        );
    }
}
