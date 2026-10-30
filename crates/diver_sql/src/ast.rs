//! Typed views over the syntax tree: small wrappers with accessors for what
//! the analyses need.

use crate::complete::Name;
use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(node: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

macro_rules! ast_node {
    ($(#[$meta:meta])* $name:ident, $($kind:ident)|+) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                matches!(kind, $(SyntaxKind::$kind)|+)
            }
            fn cast(node: SyntaxNode) -> Option<Self> {
                Self::can_cast(node.kind()).then(|| $name(node))
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

ast_node!(SourceFile, SOURCE_FILE);
ast_node!(
    /// A query: `[WITH ...] body [ORDER BY ...] [LIMIT ...]`.
    Query, QUERY
);
ast_node!(Cte, CTE);
ast_node!(
    /// A query block: SELECT, FROM, WHERE, GROUP BY, ... of one SELECT.
    SelectCore, SELECT_CORE
);
ast_node!(SelectItem, SELECT_ITEM);
ast_node!(StarExpr, STAR_EXPR);
ast_node!(Alias, ALIAS);
ast_node!(
    /// Any FROM item.
    FromItem, TABLE_REF | FILE_REF | TABLE_FUNCTION | SUBQUERY_REF | PAREN_JOIN | JOIN_EXPR | PIVOT_REF | UNPIVOT_REF
);
ast_node!(Path, PATH);

fn child<N: AstNode>(node: &SyntaxNode) -> Option<N> {
    node.children().find_map(N::cast)
}

fn children<N: AstNode>(node: &SyntaxNode) -> impl Iterator<Item = N> + use<N> {
    node.children().filter_map(N::cast)
}

pub(crate) fn child_of_kind(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

/// Whether a token is part of a name: a word, quoted or not, or the hole.
pub(crate) fn is_name_token(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::IDENT
            | SyntaxKind::QUOTED_IDENT
            | SyntaxKind::UNTERMINATED_QUOTED_IDENT
            | SyntaxKind::CURSOR
    ) || kind.is_keyword()
}

pub(crate) fn token_name(token: &SyntaxToken) -> Name {
    Name::from_token(token.kind(), token.text())
}

/// The first name token of a node.
pub(crate) fn first_name(node: &SyntaxNode) -> Option<Name> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| is_name_token(t.kind()))
        .map(|t| token_name(&t))
}

impl SourceFile {
    /// The statements, in order.
    pub fn statements(&self) -> impl Iterator<Item = SyntaxNode> + use<> {
        self.0.children()
    }
}

impl Path {
    pub fn parts(&self) -> Vec<Name> {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| is_name_token(t.kind()))
            .map(|t| token_name(&t))
            .collect()
    }
}

impl Alias {
    pub fn name(&self) -> Option<Name> {
        child_of_kind(&self.0, SyntaxKind::NAME)
            .and_then(|n| first_name(&n))
            .or_else(|| {
                // `AS 'label'`
                self.0
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == SyntaxKind::STRING)
                    .map(|t| Name {
                        text: t.text().trim_matches('\'').replace("''", "'"),
                        quoted: true,
                    })
            })
    }

    /// `t(a, b)`: the names given to the relation's columns.
    pub fn columns(&self) -> Vec<Name> {
        child_of_kind(&self.0, SyntaxKind::COLUMN_ALIASES)
            .map(|c| name_children(&c))
            .unwrap_or_default()
    }
}

/// The names of the `NAME` children of a node.
pub(crate) fn name_children(node: &SyntaxNode) -> Vec<Name> {
    node.children()
        .filter(|c| c.kind() == SyntaxKind::NAME)
        .filter_map(|n| first_name(&n))
        .collect()
}

impl Query {
    pub fn with_clause(&self) -> Option<SyntaxNode> {
        child_of_kind(&self.0, SyntaxKind::WITH_CLAUSE)
    }

    pub fn is_recursive(&self) -> bool {
        self.with_clause().is_some_and(|w| {
            w.children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::RECURSIVE_KW)
        })
    }

    pub fn ctes(&self) -> Vec<Cte> {
        self.with_clause()
            .map(|w| children(&w).collect())
            .unwrap_or_default()
    }

    /// The body: a query block, a set operation, VALUES or a parenthesized query.
    pub fn body(&self) -> Option<SyntaxNode> {
        self.0.children().find(|c| {
            matches!(
                c.kind(),
                SyntaxKind::SELECT_CORE
                    | SyntaxKind::SET_OP
                    | SyntaxKind::VALUES_CLAUSE
                    | SyntaxKind::PAREN_QUERY
            )
        })
    }

    /// The query block that names the columns: the first one of a set operation.
    pub fn first_core(&self) -> Option<SyntaxNode> {
        let mut node = self.body()?;
        loop {
            match node.kind() {
                SyntaxKind::SET_OP => node = node.first_child()?,
                SyntaxKind::PAREN_QUERY => {
                    let inner = node.children().find_map(Query::cast)?;
                    node = inner.body()?;
                }
                _ => return Some(node),
            }
        }
    }
}

impl Cte {
    pub fn name(&self) -> Option<Name> {
        child_of_kind(&self.0, SyntaxKind::NAME).and_then(|n| first_name(&n))
    }

    pub fn column_aliases(&self) -> Vec<Name> {
        child_of_kind(&self.0, SyntaxKind::COLUMN_ALIASES)
            .map(|c| name_children(&c))
            .unwrap_or_default()
    }

    pub fn query(&self) -> Option<Query> {
        child(&self.0)
    }
}

impl SelectCore {
    pub fn select_items(&self) -> Vec<SelectItem> {
        child_of_kind(&self.0, SyntaxKind::SELECT_CLAUSE)
            .map(|c| children(&c).collect())
            .unwrap_or_default()
    }

    pub fn has_select_clause(&self) -> bool {
        child_of_kind(&self.0, SyntaxKind::SELECT_CLAUSE).is_some()
    }

    /// The top-level FROM items (the ones separated by commas).
    pub fn from_items(&self) -> Vec<FromItem> {
        child_of_kind(&self.0, SyntaxKind::FROM_CLAUSE)
            .map(|c| children(&c).collect())
            .unwrap_or_default()
    }
}

impl SelectItem {
    pub fn alias(&self) -> Option<Name> {
        child::<Alias>(&self.0).and_then(|a| a.name())
    }

    pub fn expr(&self) -> Option<SyntaxNode> {
        self.0.children().find(|c| c.kind() != SyntaxKind::ALIAS)
    }

    /// The name DuckDB gives the column: the alias, or the column referenced.
    pub fn output_name(&self) -> Option<Name> {
        if let Some(alias) = self.alias() {
            return Some(alias);
        }
        let expr = self.expr()?;
        match expr.kind() {
            SyntaxKind::COLUMN_REF => child::<Path>(&expr).and_then(|p| p.parts().pop()),
            // `x::INT` keeps the name of `x`
            SyntaxKind::CAST_EXPR => {
                let inner = expr.first_child()?;
                (inner.kind() == SyntaxKind::COLUMN_REF)
                    .then(|| child::<Path>(&inner).and_then(|p| p.parts().pop()))?
            }
            _ => None,
        }
    }
}

impl StarExpr {
    /// `t` in `t.*`.
    pub fn qualifier(&self) -> Vec<Name> {
        child::<Path>(&self.0)
            .map(|p| p.parts())
            .unwrap_or_default()
    }

    pub fn exclude(&self) -> Vec<Name> {
        let Some(clause) = child_of_kind(&self.0, SyntaxKind::EXCLUDE_CLAUSE) else {
            return Vec::new();
        };
        clause
            .descendants()
            .filter_map(Path::cast)
            .filter_map(|p| p.parts().pop())
            .collect()
    }

    /// `(old AS new)` pairs of RENAME.
    pub fn rename(&self) -> Vec<(Name, Name)> {
        let Some(clause) = child_of_kind(&self.0, SyntaxKind::RENAME_CLAUSE) else {
            return Vec::new();
        };
        clause
            .children()
            .filter(|c| c.kind() == SyntaxKind::RENAME_ITEM)
            .filter_map(|item| {
                let old = child::<Path>(&item)?.parts().pop()?;
                let new = child_of_kind(&item, SyntaxKind::NAME).and_then(|n| first_name(&n))?;
                Some((old, new))
            })
            .collect()
    }

    /// `REPLACE (expr AS name)` names: the columns keep their place.
    pub fn replace(&self) -> Vec<Name> {
        let Some(clause) = child_of_kind(&self.0, SyntaxKind::REPLACE_CLAUSE) else {
            return Vec::new();
        };
        clause
            .descendants()
            .filter(|c| c.kind() == SyntaxKind::REPLACE_ITEM)
            .flat_map(|i| name_children(&i))
            .collect()
    }
}

impl FromItem {
    pub fn kind(&self) -> SyntaxKind {
        self.0.kind()
    }

    pub fn alias(&self) -> Option<Alias> {
        child(&self.0)
    }

    pub fn path(&self) -> Vec<Name> {
        child::<Path>(&self.0)
            .map(|p| p.parts())
            .unwrap_or_default()
    }

    pub fn query(&self) -> Option<Query> {
        child(&self.0)
    }

    /// The FROM items directly inside a join or a parenthesized join, or the
    /// source of a PIVOT.
    pub fn inner_items(&self) -> Vec<FromItem> {
        children(&self.0).collect()
    }

    /// The text of a FILE_REF's string, unquoted.
    pub fn file(&self) -> Option<String> {
        self.0
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind().is_string())
            .map(|t| string_value(t.text()))
    }

    /// The item as written without its alias, such as a table function's
    /// call with all its arguments.
    pub fn text_without_alias(&self) -> String {
        let text = self.0.to_string();
        let start = self.0.text_range().start();
        let end = self
            .alias()
            .map_or(text.len(), |a| usize::from(a.syntax().text_range().start() - start));
        text[..end].trim().to_string()
    }

    /// The string arguments of a table function, unquoted.
    pub fn string_args(&self) -> Vec<String> {
        let Some(args) = child_of_kind(&self.0, SyntaxKind::ARG_LIST) else {
            return Vec::new();
        };
        args.children()
            .filter(|c| c.kind() == SyntaxKind::LITERAL)
            .filter_map(|c| c.first_token())
            .filter(|t| t.kind().is_string())
            .map(|t| string_value(t.text()))
            .collect()
    }

    pub fn is_lateral(&self) -> bool {
        self.0
            .children_with_tokens()
            .any(|e| e.kind() == SyntaxKind::LATERAL_KW)
    }
}

/// The value of a string literal: quotes removed and `''` unescaped.
pub fn string_value(text: &str) -> String {
    let body = text
        .strip_prefix(['e', 'E', 'x', 'X', 'b', 'B'])
        .filter(|t| t.starts_with('\''))
        .unwrap_or(text);
    if let Some(rest) = body.strip_prefix('$') {
        // $tag$...$tag$
        if let Some(i) = rest.find('$') {
            let tag_len = i + 2;
            let inner = &body[tag_len.min(body.len())..];
            return inner
                .get(..inner.len().saturating_sub(tag_len))
                .unwrap_or(inner)
                .to_string();
        }
    }
    let inner = body.strip_prefix('\'').unwrap_or(body);
    let inner = inner.strip_suffix('\'').unwrap_or(inner);
    inner.replace("''", "'")
}
