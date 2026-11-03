//! What is visible at a position: relations (with the columns the crate can
//! infer), select-list aliases and CTEs.
//!
//! The walk goes outward from the position through the enclosing query
//! blocks and statements; inner blocks shadow outer ones.

use std::sync::Arc;

use rowan::TextSize;

use crate::ast::{self, AstNode, Cte, FromItem, Query, SelectCore, SelectItem, StarExpr};
use crate::complete::Name;
use crate::{SyntaxElement, SyntaxKind, SyntaxNode};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scope {
    pub relations: Vec<VisibleRelation>,
    /// Select-list aliases DuckDB lets the position refer to.
    pub aliases: Vec<Name>,
    /// CTEs visible here, innermost first.
    pub ctes: Vec<Name>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VisibleRelation {
    /// `o` in `orders o`.
    pub alias: Option<Name>,
    pub source: RelationSource,
    pub columns: Columns,
    /// 0 = the position's own query block, 1 = the one around it, ...
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RelationSource {
    /// `db.schema.table`
    Table {
        path: Vec<Name>,
    },
    /// `FROM 'x.parquet'`
    File {
        path: String,
    },
    /// A table function with its string arguments, for the app's DESCRIBE.
    TableFunction {
        name: Vec<Name>,
        args: Vec<String>,
        /// The call as written, with named arguments too: `read_csv('x', sep = ';')`.
        call: String,
    },
    Cte {
        name: Name,
    },
    Subquery,
    Values,
    Pivot,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Columns {
    /// From a select list, VALUES or column aliases `AS t(a, b)`.
    Known(Vec<ColumnSource>),
    /// A table, file or table function: ask the catalog.
    FromCatalog,
    /// Nothing worth guessing (PIVOT output).
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColumnSource {
    Named(Name),
    /// A column whose type the text tells, or lets the catalog tell.
    Typed { name: Name, data_type: ColumnType },
    /// `*` over these relations, expanded through the catalog, without
    /// the excluded columns and the first `skip` (which column aliases renamed).
    Star {
        relations: Vec<VisibleRelation>,
        exclude: Vec<Name>,
        rename: Vec<(Name, Name)>,
        skip: usize,
    },
    /// An expression without an alias; DuckDB names it after the expression text.
    Unnamed(String),
}

/// Where a column's type comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnType {
    /// A DuckDB type as written, as `INTEGER` or `STRUCT(a INTEGER, b)`. A
    /// struct literal's fields have no type unless they are themselves typed.
    Text(String),
    /// The type of the column `path` names (`t.col`, `col`, `t.col.field`)
    /// among `relations`, the relations of the query block it was written in.
    Of {
        relations: Arc<Vec<VisibleRelation>>,
        path: Vec<Name>,
    },
}

impl VisibleRelation {
    /// The name the relation is referred to by: its alias, or a default.
    pub fn name(&self) -> Option<Name> {
        if let Some(alias) = &self.alias {
            return Some(alias.clone());
        }
        match &self.source {
            RelationSource::Table { path } => path.last().cloned(),
            RelationSource::TableFunction { name, .. } => name.last().cloned(),
            RelationSource::Cte { name } => Some(name.clone()),
            RelationSource::File { path } => Some(Name::new(file_stem(path))),
            _ => None,
        }
    }

    /// Whether `t.` (or `s.t.`, `db.s.t.`) refers to this relation.
    pub fn matches_qualifier(&self, qualifier: &[Name]) -> bool {
        if qualifier.is_empty() {
            return false;
        }
        if let Some(alias) = &self.alias {
            return qualifier.len() == 1 && alias.matches(&qualifier[0].text);
        }
        let path: Vec<Name> = match &self.source {
            RelationSource::Table { path } => path.clone(),
            RelationSource::TableFunction { name, .. } => name.clone(),
            _ => self.name().into_iter().collect(),
        };
        if path.is_empty() {
            return false;
        }
        // compare from the end; `main.orders.x` reaches `FROM orders` too
        qualifier
            .iter()
            .rev()
            .zip(path.iter().rev())
            .all(|(q, p)| p.matches(&q.text))
            && qualifier.len() <= path.len().max(3)
            && path
                .last()
                .is_some_and(|p| p.matches(&qualifier[qualifier.len() - 1].text))
    }
}

/// A file or table function FROM item as a relation source.
fn file_source(item: &FromItem) -> Option<RelationSource> {
    match item.kind() {
        SyntaxKind::FILE_REF => Some(RelationSource::File {
            path: item.file().unwrap_or_default(),
        }),
        SyntaxKind::TABLE_FUNCTION => Some(RelationSource::TableFunction {
            name: item.path(),
            args: item.string_args(),
            call: item.text_without_alias(),
        }),
        _ => None,
    }
}

/// The files and table functions a script reads from, each once, in order:
/// the relations whose columns only the catalog can tell.
pub fn file_relations(parse: &crate::Parse) -> Vec<RelationSource> {
    let mut out = Vec::new();
    for item in parse.syntax().descendants().filter_map(FromItem::cast) {
        if let Some(source) = file_source(&item)
            && !out.contains(&source)
        {
            out.push(source);
        }
    }
    out
}

pub(crate) fn file_stem(path: &str) -> String {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    base.split('.').next().unwrap_or(base).to_string()
}

impl Scope {
    /// The relations `qualifier` refers to, innermost first; only the
    /// innermost depth that has a match counts, so inner aliases shadow outer ones.
    pub fn resolve(&self, qualifier: &[Name]) -> Vec<&VisibleRelation> {
        resolve_in(&self.relations, qualifier)
    }
}

/// The relations of `relations` that `qualifier` refers to, as [`Scope::resolve`].
pub fn resolve_in<'a>(relations: &'a [VisibleRelation], qualifier: &[Name]) -> Vec<&'a VisibleRelation> {
    let matches: Vec<&VisibleRelation> = relations
        .iter()
        .filter(|r| r.matches_qualifier(qualifier))
        .collect();
    let Some(depth) = matches.iter().map(|r| r.depth).min() else {
        return matches;
    };
    matches.into_iter().filter(|r| r.depth == depth).collect()
}

/// A visible CTE. Lists of them go innermost first, so the CTEs a body can
/// see are the ones after it in the list.
#[derive(Clone)]
struct CteDef {
    name: Name,
    cte: Cte,
}

/// How deep column inference may nest (recursive CTEs, CTEs of CTEs).
const MAX_DEPTH: u32 = 8;

pub(crate) fn scope_at(root: &SyntaxNode, offset: TextSize) -> Scope {
    let Some(anchor) = anchor(root, offset) else {
        return Scope::default();
    };
    let mut walker = Walker {
        offset,
        path: ancestors(&anchor).collect(),
        ctes: Vec::new(),
    };
    walker.collect_ctes(&anchor);
    walker.scope(&anchor)
}

/// The element the position is at: the hole, or the token at the offset.
fn anchor(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxElement> {
    if let Some(cursor) = root
        .descendants_with_tokens()
        .find(|e| e.kind() == SyntaxKind::CURSOR && e.text_range().start() == offset)
    {
        return Some(cursor);
    }
    if let Some(cursor) = root
        .descendants_with_tokens()
        .find(|e| e.kind() == SyntaxKind::CURSOR)
    {
        return Some(cursor);
    }
    if offset > TextSize::of(root.text().to_string().as_str()) {
        return None;
    }
    let token = root.token_at_offset(offset);
    let token = match token {
        rowan::TokenAtOffset::None => return None,
        rowan::TokenAtOffset::Single(t) => t,
        rowan::TokenAtOffset::Between(l, r) => {
            if l.kind().is_trivia() {
                r
            } else {
                l
            }
        }
    };
    Some(SyntaxElement::Token(token))
}

struct Walker {
    offset: TextSize,
    /// The nodes around the position, innermost first.
    path: Vec<SyntaxNode>,
    ctes: Vec<CteDef>,
}

fn ancestors(element: &SyntaxElement) -> impl Iterator<Item = SyntaxNode> + use<> {
    let parent = match element {
        SyntaxElement::Node(n) => Some(n.clone()),
        SyntaxElement::Token(t) => t.parent(),
    };
    std::iter::successors(parent, |n| n.parent())
}

impl Walker {
    /// Whether the position is inside `node`.
    fn holds(&self, node: &SyntaxNode) -> bool {
        self.path.contains(node)
    }

    /// Whether `node` lies wholly before the position.
    fn before(&self, node: &SyntaxNode) -> bool {
        !self.holds(node) && node.text_range().end() <= self.offset
    }

    /// Visible CTEs, innermost first.
    fn collect_ctes(&mut self, anchor: &SyntaxElement) {
        let mut found = Vec::new();
        for node in ancestors(anchor) {
            let with = match node.kind() {
                SyntaxKind::QUERY => Query::cast(node.clone())
                    .and_then(|q| q.with_clause().map(|w| (w, q.is_recursive()))),
                SyntaxKind::INSERT_STMT
                | SyntaxKind::UPDATE_STMT
                | SyntaxKind::DELETE_STMT
                | SyntaxKind::MERGE_STMT => {
                    ast::child_of_kind(&node, SyntaxKind::WITH_CLAUSE).map(|w| {
                        let rec = w
                            .children_with_tokens()
                            .any(|e| e.kind() == SyntaxKind::RECURSIVE_KW);
                        (w, rec)
                    })
                }
                _ => None,
            };
            let Some((with, recursive)) = with else {
                continue;
            };
            let ctes: Vec<Cte> = with.children().filter_map(Cte::cast).collect();
            // Inside a CTE body, only earlier CTEs are visible (and itself when recursive).
            let inside = ctes.iter().position(|c| self.holds(c.syntax()));
            let visible: Vec<&Cte> = match inside {
                Some(i) => ctes[..if recursive { i + 1 } else { i }].iter().collect(),
                None => ctes.iter().collect(),
            };
            // later CTEs of a level first, so each sees the earlier ones after it
            found.extend(visible.into_iter().rev().filter_map(|c| {
                Some(CteDef {
                    name: c.name()?,
                    cte: c.clone(),
                })
            }));
        }
        self.ctes = found;
    }

    fn find_cte(&self, name: &Name) -> Option<(usize, &CteDef)> {
        self.ctes
            .iter()
            .enumerate()
            .find(|(_, c)| c.name.matches(&name.text))
    }

    fn scope(&self, anchor: &SyntaxElement) -> Scope {
        let mut scope = Scope {
            ctes: self.ctes.iter().map(|c| c.name.clone()).collect(),
            ..Scope::default()
        };
        let mut depth = 0u32;
        let mut prev: Option<SyntaxNode> = match anchor {
            SyntaxElement::Node(n) => Some(n.clone()),
            SyntaxElement::Token(_) => None,
        };
        for node in ancestors(anchor) {
            let child = prev.clone();
            match node.kind() {
                SyntaxKind::SELECT_CORE => {
                    let core = SelectCore::cast(node.clone()).unwrap();
                    let child_kind = child.as_ref().map(|c| c.kind());
                    let relations = self.core_relations_at(&core, child.as_ref(), depth);
                    scope.relations.extend(relations);
                    match child_kind {
                        Some(SyntaxKind::SELECT_CLAUSE) => {
                            for item in core.select_items() {
                                if self.before(item.syntax()) {
                                    scope.aliases.extend(item.alias());
                                }
                            }
                        }
                        Some(SyntaxKind::FROM_CLAUSE) | None => {}
                        _ => scope
                            .aliases
                            .extend(core.select_items().iter().filter_map(SelectItem::alias)),
                    }
                    depth += 1;
                }
                SyntaxKind::QUERY => {
                    // ORDER BY / LIMIT see the first block's FROM items and aliases.
                    let child_kind = child.as_ref().map(|c| c.kind());
                    if matches!(
                        child_kind,
                        Some(
                            SyntaxKind::ORDER_BY_CLAUSE
                                | SyntaxKind::LIMIT_CLAUSE
                                | SyntaxKind::OFFSET_CLAUSE
                        )
                    ) {
                        let q = Query::cast(node.clone()).unwrap();
                        if let Some(core) = q.body().and_then(SelectCore::cast) {
                            scope.relations.extend(self.core_relations(&core, depth, 0));
                            scope
                                .aliases
                                .extend(core.select_items().iter().filter_map(SelectItem::alias));
                            depth += 1;
                        }
                    }
                }
                SyntaxKind::INSERT_STMT
                | SyntaxKind::UPDATE_STMT
                | SyntaxKind::DELETE_STMT
                | SyntaxKind::MERGE_STMT
                | SyntaxKind::ALTER_STMT
                | SyntaxKind::CREATE_STMT
                | SyntaxKind::PIVOT_STMT
                | SyntaxKind::TRUNCATE_STMT
                | SyntaxKind::VACUUM_STMT
                | SyntaxKind::ANALYZE_STMT
                | SyntaxKind::COPY_STMT => {
                    let child_kind = child.as_ref().map(|c| c.kind());
                    // The query of INSERT ... SELECT / CTAS doesn't see the target.
                    if matches!(
                        child_kind,
                        Some(SyntaxKind::QUERY | SyntaxKind::WITH_CLAUSE)
                    ) && matches!(
                        node.kind(),
                        SyntaxKind::INSERT_STMT | SyntaxKind::CREATE_STMT | SyntaxKind::COPY_STMT
                    ) {
                        prev = Some(node);
                        continue;
                    }
                    for item in node.children().filter_map(FromItem::cast) {
                        if self.holds(item.syntax()) {
                            // in the target name itself
                            if !matches!(node.kind(), SyntaxKind::PIVOT_STMT) {
                                continue;
                            }
                        }
                        scope.relations.extend(self.item_relations(&item, depth, 0));
                    }
                    for clause in node.children().filter(|c| {
                        matches!(c.kind(), SyntaxKind::USING_CLAUSE | SyntaxKind::FROM_CLAUSE)
                    }) {
                        if child.as_ref() == Some(&clause) && node.kind() != SyntaxKind::MERGE_STMT
                        {
                            // inside UPDATE ... FROM / DELETE ... USING items: earlier ones only
                            for item in clause.children().filter_map(FromItem::cast) {
                                if self.before(item.syntax()) {
                                    scope.relations.extend(self.item_relations(&item, depth, 0));
                                }
                            }
                            continue;
                        }
                        for item in clause.children().filter_map(FromItem::cast) {
                            scope.relations.extend(self.item_relations(&item, depth, 0));
                        }
                    }
                    depth += 1;
                }
                SyntaxKind::PIVOT_REF | SyntaxKind::UNPIVOT_REF => {
                    // inside PIVOT (...): the pivoted relation's columns
                    if let Some(source) = node.children().find_map(FromItem::cast)
                        && child.as_ref() != Some(source.syntax())
                    {
                        scope
                            .relations
                            .extend(self.item_relations(&source, depth, 0));
                    }
                }
                _ => {}
            }
            prev = Some(node);
        }
        scope
    }

    /// The relations of a block visible from its child `child`: inside FROM,
    /// only the items before the one holding the position (DuckDB's implicit
    /// lateral) and, inside a join condition, that join's sides.
    fn core_relations_at(
        &self,
        core: &SelectCore,
        child: Option<&SyntaxNode>,
        depth: u32,
    ) -> Vec<VisibleRelation> {
        let in_from = child.is_some_and(|c| c.kind() == SyntaxKind::FROM_CLAUSE);
        if !in_from {
            return self.core_relations(core, depth, 0);
        }
        let mut out = Vec::new();
        for item in core.from_items() {
            if self.holds(item.syntax()) {
                out.extend(self.relations_inside(&item, depth));
            } else if self.before(item.syntax()) {
                out.extend(self.item_relations(&item, depth, 0));
            }
        }
        out
    }

    /// Relations visible from a position inside a FROM item: in a join
    /// condition, both sides; in a later join operand, the earlier ones.
    fn relations_inside(&self, item: &FromItem, depth: u32) -> Vec<VisibleRelation> {
        if item.kind() != SyntaxKind::JOIN_EXPR {
            return Vec::new();
        }
        let in_condition = item
            .syntax()
            .children()
            .filter(|c| matches!(c.kind(), SyntaxKind::JOIN_ON | SyntaxKind::JOIN_USING))
            .any(|c| self.holds(&c));
        let mut out = Vec::new();
        for side in item.inner_items() {
            if in_condition || self.before(side.syntax()) {
                out.extend(self.item_relations(&side, depth, 0));
            } else if self.holds(side.syntax()) {
                out.extend(self.relations_inside(&side, depth));
            }
        }
        out
    }

    fn core_relations(&self, core: &SelectCore, depth: u32, nesting: u32) -> Vec<VisibleRelation> {
        core.from_items()
            .iter()
            .flat_map(|item| self.item_relations(item, depth, nesting))
            .collect()
    }

    /// The relations a FROM item brings in (a join brings both sides).
    fn item_relations(&self, item: &FromItem, depth: u32, nesting: u32) -> Vec<VisibleRelation> {
        let alias = item.alias();
        let alias_name = alias.as_ref().and_then(|a| a.name());
        let column_aliases = alias.as_ref().map(|a| a.columns()).unwrap_or_default();
        let relation = |source: RelationSource, columns: Columns| VisibleRelation {
            alias: alias_name.clone(),
            columns: rename_columns(columns, &column_aliases, &source, depth),
            source,
            depth,
        };
        match item.kind() {
            SyntaxKind::JOIN_EXPR | SyntaxKind::PAREN_JOIN => {
                let mut out: Vec<VisibleRelation> = item
                    .inner_items()
                    .iter()
                    .flat_map(|i| self.item_relations(i, depth, nesting))
                    .collect();
                if item.kind() == SyntaxKind::PAREN_JOIN && alias_name.is_some() {
                    // `(a JOIN b) j` hides a and b behind j
                    let columns = Columns::Known(vec![star_of(std::mem::take(&mut out))]);
                    out.push(relation(RelationSource::Subquery, columns));
                }
                out
            }
            SyntaxKind::TABLE_REF => {
                let path = item.path();
                if path.len() == 1
                    && let Some((i, cte)) = self.find_cte(&path[0])
                {
                    let columns = if nesting < MAX_DEPTH {
                        self.cte_columns(cte, i, nesting + 1)
                    } else {
                        Columns::Unknown
                    };
                    return vec![relation(
                        RelationSource::Cte {
                            name: cte.name.clone(),
                        },
                        columns,
                    )];
                }
                vec![relation(
                    RelationSource::Table { path },
                    Columns::FromCatalog,
                )]
            }
            SyntaxKind::FILE_REF | SyntaxKind::TABLE_FUNCTION => file_source(item)
                .map(|source| relation(source, Columns::FromCatalog))
                .into_iter()
                .collect(),
            SyntaxKind::SUBQUERY_REF => {
                let Some(query) = item.query() else {
                    return Vec::new();
                };
                let values = query
                    .body()
                    .is_some_and(|b| b.kind() == SyntaxKind::VALUES_CLAUSE);
                let columns = if nesting < MAX_DEPTH {
                    self.query_columns(&query, 0, nesting + 1)
                } else {
                    Columns::Unknown
                };
                vec![relation(
                    if values {
                        RelationSource::Values
                    } else {
                        RelationSource::Subquery
                    },
                    columns,
                )]
            }
            SyntaxKind::PIVOT_REF | SyntaxKind::UNPIVOT_REF => {
                vec![relation(RelationSource::Pivot, Columns::Unknown)]
            }
            _ => Vec::new(),
        }
    }

    fn cte_columns(&self, cte: &CteDef, index: usize, nesting: u32) -> Columns {
        let Some(query) = cte.cte.query() else {
            return Columns::Unknown;
        };
        let aliases = cte.cte.column_aliases();
        // A recursive CTE's own name inside its body resolves to itself;
        // its columns come from the first (non-recursive) branch.
        let columns = self.query_columns(&query, index + 1, nesting);
        rename_columns(
            columns,
            &aliases,
            &RelationSource::Cte {
                name: cte.name.clone(),
            },
            0,
        )
    }

    /// The columns a query produces, from its first query block.
    fn query_columns(&self, query: &Query, cte_from: usize, nesting: u32) -> Columns {
        let Some(core) = query.first_core() else {
            return Columns::Unknown;
        };
        match core.kind() {
            SyntaxKind::VALUES_CLAUSE => {
                let n = core
                    .children()
                    .find(|c| c.kind() == SyntaxKind::VALUES_ROW)
                    .map(|row| row.children().count())
                    .unwrap_or(0);
                Columns::Known(
                    (0..n)
                        .map(|i| ColumnSource::Named(Name::new(format!("col{i}"))))
                        .collect(),
                )
            }
            SyntaxKind::SELECT_CORE => {
                let core = SelectCore::cast(core).unwrap();
                let inner = Walker {
                    offset: self.offset,
                    path: Vec::new(),
                    ctes: self.inner_ctes(query, cte_from),
                };
                let relations = inner.core_relations(&core, 0, nesting);
                if !core.has_select_clause() {
                    return Columns::Known(vec![star_of(relations)]);
                }
                let mut out = Vec::new();
                let shared = Arc::new(relations);
                for item in core.select_items() {
                    out.push(item_column(&item, &shared));
                }
                Columns::Known(out)
            }
            _ => Columns::Unknown,
        }
    }

    /// The CTEs visible inside `query`: its own, then the ones of this list from `from` on.
    fn inner_ctes(&self, query: &Query, from: usize) -> Vec<CteDef> {
        let mut out: Vec<CteDef> = query
            .ctes()
            .into_iter()
            .rev()
            .filter_map(|c| {
                Some(CteDef {
                    name: c.name()?,
                    cte: c,
                })
            })
            .collect();
        out.extend(self.ctes.iter().skip(from).cloned());
        out
    }
}

fn star_of(relations: Vec<VisibleRelation>) -> ColumnSource {
    ColumnSource::Star {
        relations,
        exclude: Vec::new(),
        rename: Vec::new(),
        skip: 0,
    }
}

/// The column a select item produces.
fn item_column(item: &SelectItem, relations: &Arc<Vec<VisibleRelation>>) -> ColumnSource {
    if let Some(name) = item.output_name() {
        return match item.expr().and_then(|expr| expr_type(&expr, relations)) {
            Some(data_type) => ColumnSource::Typed { name, data_type },
            None => ColumnSource::Named(name),
        };
    }
    let Some(expr) = item.expr() else {
        return ColumnSource::Unnamed(String::new());
    };
    if let Some(star) = StarExpr::cast(expr.clone()) {
        return star_column(&star, relations);
    }
    // COLUMNS(*), COLUMNS(* EXCLUDE (...))
    if expr.kind() == SyntaxKind::CALL_EXPR
        && expr
            .children()
            .find_map(ast::Path::cast)
            .is_some_and(|p| p.parts().last().is_some_and(|n| n.matches("columns")))
        && let Some(star) = expr.descendants().find_map(StarExpr::cast)
    {
        return star_column(&star, relations);
    }
    ColumnSource::Unnamed(expr.text().to_string())
}

fn star_column(star: &StarExpr, relations: &[VisibleRelation]) -> ColumnSource {
    let qualifier = star.qualifier();
    let relations: Vec<VisibleRelation> = if qualifier.is_empty() {
        relations.to_vec()
    } else {
        relations
            .iter()
            .filter(|r| r.matches_qualifier(&qualifier))
            .cloned()
            .collect()
    };
    ColumnSource::Star {
        relations,
        exclude: star.exclude(),
        rename: star.rename(),
        skip: 0,
    }
}

/// The type of an expression, where the text tells it: a cast, a struct
/// literal or `struct_pack`, or a column reference, whose type the catalog
/// may know.
fn expr_type(expr: &SyntaxNode, relations: &Arc<Vec<VisibleRelation>>) -> Option<ColumnType> {
    if expr.kind() == SyntaxKind::COLUMN_REF {
        let path = expr.children().find_map(ast::Path::cast)?.parts();
        return Some(ColumnType::Of {
            relations: relations.clone(),
            path,
        });
    }
    text_type(expr).map(ColumnType::Text)
}

/// A type name's text with its spacing collapsed: `STRUCT(a INT)`.
pub(crate) fn type_text(node: &SyntaxNode) -> String {
    node.text().to_string().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The type an expression has on its own, as DuckDB writes types.
fn text_type(expr: &SyntaxNode) -> Option<String> {
    match expr.kind() {
        SyntaxKind::CAST_EXPR => ast::child_of_kind(expr, SyntaxKind::TYPE_NAME).map(|t| type_text(&t)),
        SyntaxKind::PAREN_EXPR => expr.first_child().and_then(|inner| text_type(&inner)),
        SyntaxKind::STRUCT_EXPR => {
            let fields = expr
                .children()
                .filter(|c| c.kind() == SyntaxKind::STRUCT_FIELD)
                .filter_map(|field| {
                    let mut parts = field.children();
                    let key = parts.next()?;
                    let name = match key.kind() {
                        SyntaxKind::LITERAL => {
                            let text = key.text().to_string();
                            let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
                            inner.replace("''", "'")
                        }
                        SyntaxKind::COLUMN_REF => key
                            .children()
                            .find_map(ast::Path::cast)?
                            .parts()
                            .pop()?
                            .text,
                        _ => return None,
                    };
                    Some((name, parts.next().and_then(|value| text_type(&value))))
                });
            Some(struct_type(fields))
        }
        SyntaxKind::CALL_EXPR => {
            let callee = expr.children().find_map(ast::Path::cast)?.parts();
            if !callee.last()?.matches("struct_pack") {
                return None;
            }
            let args = ast::child_of_kind(expr, SyntaxKind::ARG_LIST)?;
            let fields = args
                .children()
                .filter(|c| c.kind() == SyntaxKind::NAMED_ARG)
                .filter_map(|arg| {
                    let name = ast::first_name(&ast::child_of_kind(&arg, SyntaxKind::NAME)?)?;
                    let value = arg.children().filter(|c| c.kind() != SyntaxKind::NAME).last();
                    Some((name.text, value.and_then(|value| text_type(&value))))
                });
            Some(struct_type(fields))
        }
        _ => None,
    }
}

fn struct_type(fields: impl Iterator<Item = (String, Option<String>)>) -> String {
    let fields: Vec<String> = fields
        .map(|(name, data_type)| {
            let name = crate::items::quote_if_needed(&name);
            match data_type {
                Some(data_type) => format!("{name} {data_type}"),
                None => name,
            }
        })
        .collect();
    format!("STRUCT({})", fields.join(", "))
}

/// Applies `t(a, b)` column aliases: they rename the first columns.
fn rename_columns(
    columns: Columns,
    aliases: &[Name],
    source: &RelationSource,
    depth: u32,
) -> Columns {
    if aliases.is_empty() {
        return columns;
    }
    let named = aliases.iter().cloned().map(ColumnSource::Named);
    match columns {
        Columns::Known(cols) if !cols.iter().any(|c| matches!(c, ColumnSource::Star { .. })) => {
            Columns::Known(named.chain(cols.into_iter().skip(aliases.len())).collect())
        }
        Columns::Unknown => Columns::Known(named.collect()),
        // Stars and catalog columns: rename the first ones once they are known.
        columns => {
            let original = VisibleRelation {
                alias: None,
                source: source.clone(),
                columns,
                depth,
            };
            let rest = ColumnSource::Star {
                relations: vec![original],
                exclude: Vec::new(),
                rename: Vec::new(),
                skip: aliases.len(),
            };
            Columns::Known(named.chain(std::iter::once(rest)).collect())
        }
    }
}

/// The columns a query produces, as far as the text tells.
pub(crate) fn query_columns(query: &Query) -> Columns {
    Walker {
        offset: TextSize::new(0),
        path: Vec::new(),
        ctes: Vec::new(),
    }
    .query_columns(query, 0, 1)
}

/// The relations a FROM item brings in, with the CTEs visible where it is.
pub(crate) fn item_relations(item: &FromItem) -> Vec<VisibleRelation> {
    let anchor = SyntaxElement::Node(item.syntax().clone());
    let mut walker = Walker {
        offset: item.syntax().text_range().start(),
        path: ancestors(&anchor).collect(),
        ctes: Vec::new(),
    };
    walker.collect_ctes(&anchor);
    walker.item_relations(item, 0, 0)
}
