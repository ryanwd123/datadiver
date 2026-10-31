//! The queries inside a script: the ones around a position, what one needs
//! to run on its own, and rewrites that move queries between CTEs and
//! subqueries.

use rowan::{TextRange, TextSize};

use crate::ast::{AstNode, Cte, FromItem, Query, SelectCore};
use crate::complete::Name;
use crate::edit::{
    EditStyle, Edits, Piece, Rewrite, kw, line_indent, line_start, reindent, size, span, starts_line, token_near,
    unique_name, upper_keywords,
};
use crate::{SyntaxKind, SyntaxNode, parse};

/// What a query is to the script around it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryNodeKind {
    /// The query of a statement.
    Statement,
    Cte { name: String },
    /// `(SELECT ...) AS alias` in FROM.
    Subquery { alias: Option<String> },
    /// A subquery in an expression, or a parenthesized side of a set operation.
    Nested,
}

/// A query in the script: a statement's, a CTE's or a subquery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryNode {
    pub kind: QueryNodeKind,
    /// The query, without parentheses around it.
    pub range: TextRange,
}

impl QueryNode {
    /// How messages name it: "CTE orders", "subquery t", "the query".
    pub fn label(&self) -> String {
        match &self.kind {
            QueryNodeKind::Statement => "the statement".into(),
            QueryNodeKind::Cte { name } => format!("CTE {name}"),
            QueryNodeKind::Subquery { alias: Some(alias) } => format!("subquery {alias}"),
            QueryNodeKind::Subquery { alias: None } | QueryNodeKind::Nested => "the subquery".into(),
        }
    }
}

/// The queries around `offset`, innermost first: a subquery, the CTE it is
/// in, the statement.
pub fn query_nodes_at(text: &str, offset: TextSize) -> Vec<QueryNode> {
    let root = parse(text).syntax();
    let Some(token) = token_near(&root, offset) else {
        return Vec::new();
    };
    nodes_around(token.parent()).into_iter().map(|(node, _)| node).collect()
}

fn nodes_around(start: Option<SyntaxNode>) -> Vec<(QueryNode, SyntaxNode)> {
    let mut out = Vec::new();
    for node in std::iter::successors(start, SyntaxNode::parent) {
        match node.kind() {
            SyntaxKind::CTE => {
                let cte = Cte::cast(node.clone()).unwrap();
                if let Some(query) = cte.query() {
                    let name = cte.name().map(|n| n.text).unwrap_or_default();
                    let range = query.syntax().text_range();
                    out.push((
                        QueryNode {
                            kind: QueryNodeKind::Cte { name },
                            range,
                        },
                        query.syntax().clone(),
                    ));
                }
            }
            SyntaxKind::QUERY => {
                let parent = node.parent();
                let kind = match parent.as_ref().map(SyntaxNode::kind) {
                    Some(SyntaxKind::CTE) => continue,
                    Some(SyntaxKind::SUBQUERY_REF) => QueryNodeKind::Subquery {
                        alias: parent
                            .and_then(FromItem::cast)
                            .and_then(|item| item.alias())
                            .and_then(|alias| alias.name())
                            .map(|name| name.text),
                    },
                    _ if is_statement_query(&node) => QueryNodeKind::Statement,
                    _ => QueryNodeKind::Nested,
                };
                out.push((
                    QueryNode {
                        kind,
                        range: node.text_range(),
                    },
                    node,
                ));
            }
            _ => {}
        }
    }
    out
}

fn is_statement_query(query: &SyntaxNode) -> bool {
    query
        .parent()
        .and_then(|p| p.parent())
        .is_some_and(|gp| gp.kind() == SyntaxKind::SOURCE_FILE)
}

/// The QUERY node with exactly `range`.
fn find_query(root: &SyntaxNode, range: TextRange) -> Option<SyntaxNode> {
    let element = root.covering_element(range);
    let start = match element {
        rowan::NodeOrToken::Node(n) => Some(n),
        rowan::NodeOrToken::Token(t) => t.parent(),
    };
    std::iter::successors(start, SyntaxNode::parent)
        .find(|n| n.kind() == SyntaxKind::QUERY && n.text_range() == range)
}

fn node_kind(query: &SyntaxNode) -> QueryNodeKind {
    nodes_around(Some(query.clone()))
        .into_iter()
        .next()
        .map(|(node, _)| node.kind)
        .unwrap_or(QueryNodeKind::Nested)
}

/// A CTE and the WITH clause it is in.
#[derive(Clone)]
pub(crate) struct CteDef {
    pub cte: Cte,
    pub name: Name,
    pub recursive: bool,
}

/// The CTEs visible at `node`, innermost WITH first.
pub(crate) fn visible_ctes(node: &SyntaxNode) -> Vec<CteDef> {
    let mut out = Vec::new();
    let mut child = node.clone();
    for ancestor in node.ancestors().skip(1) {
        if let Some(query) = Query::cast(ancestor.clone()) {
            let ctes = query.ctes();
            let recursive = query.is_recursive();
            let inside = ctes
                .iter()
                .position(|c| c.syntax().text_range().contains_range(child.text_range()));
            let visible = match inside {
                Some(i) if recursive => &ctes[..=i],
                Some(i) => &ctes[..i],
                None => &ctes[..],
            };
            // Later CTEs of a WITH shadow earlier ones of the same name.
            for cte in visible.iter().rev() {
                if let Some(name) = cte.name() {
                    out.push(CteDef {
                        cte: cte.clone(),
                        name,
                        recursive,
                    });
                }
            }
        }
        child = ancestor;
    }
    out
}

/// The CTE a one-part table reference names, as seen from where it is.
pub(crate) fn resolve_cte(table_ref: &SyntaxNode) -> Option<CteDef> {
    let item = FromItem::cast(table_ref.clone()).filter(|i| i.kind() == SyntaxKind::TABLE_REF)?;
    let path = item.path();
    let [name] = path.as_slice() else {
        return None;
    };
    visible_ctes(table_ref)
        .into_iter()
        .find(|def| def.name.matches(&name.text))
}

/// The table references inside `node` that name CTEs from outside it.
fn outside_cte_refs(node: &SyntaxNode) -> Vec<(SyntaxNode, CteDef)> {
    node.descendants()
        .filter(|n| n.kind() == SyntaxKind::TABLE_REF)
        .filter_map(|n| {
            let def = resolve_cte(&n)?;
            (!node.text_range().contains_range(def.cte.syntax().text_range())).then_some((n, def))
        })
        .collect()
}

/// The CTEs `node` needs, directly or through other CTEs, in text order.
fn needed_ctes(node: &SyntaxNode) -> Vec<CteDef> {
    let mut out: Vec<CteDef> = Vec::new();
    let mut todo = vec![node.clone()];
    while let Some(next) = todo.pop() {
        for (_, def) in outside_cte_refs(&next) {
            if out.iter().any(|d| d.cte == def.cte) {
                continue;
            }
            todo.push(def.cte.syntax().clone());
            out.push(def);
        }
    }
    out.sort_by_key(|d| d.cte.syntax().text_range().start());
    out
}

/// The names relations in a FROM item go by.
fn relation_names(item: &FromItem, out: &mut Vec<Name>) {
    if let Some(alias) = item.alias().and_then(|a| a.name()) {
        out.push(alias);
    } else if let Some(last) = item.path().pop() {
        out.push(last);
    }
    for inner in item.inner_items() {
        relation_names(&inner, out);
    }
}

/// A relation of a query around `query` that `query` refers to, which keeps
/// it from running or moving on its own.
fn outer_reference(query: &SyntaxNode) -> Option<String> {
    let range = query.text_range();
    let mut outer = Vec::new();
    for core in query.ancestors().skip(1).filter_map(SelectCore::cast) {
        for item in core.from_items() {
            if !item.syntax().text_range().contains_range(range) {
                relation_names(&item, &mut outer);
            }
        }
    }
    if outer.is_empty() {
        return None;
    }
    let mut inner = Vec::new();
    for item in query.descendants().filter_map(FromItem::cast) {
        relation_names(&item, &mut inner);
    }
    for path in query
        .descendants()
        .filter(|n| matches!(n.kind(), SyntaxKind::COLUMN_REF | SyntaxKind::STAR_EXPR))
        .filter_map(|n| n.children().find_map(crate::ast::Path::cast))
    {
        let parts = path.parts();
        let qualifier = match path.syntax().parent().map(|p| p.kind()) {
            Some(SyntaxKind::STAR_EXPR) => parts.first(),
            _ if parts.len() >= 2 => parts.first(),
            _ => None,
        };
        if let Some(q) = qualifier
            && outer.iter().any(|o| o.matches(&q.text))
            && !inner.iter().any(|i| i.matches(&q.text))
        {
            return Some(q.text.clone());
        }
    }
    None
}

/// SQL that runs the query at `range` on its own: with the CTEs it needs in
/// front of it.
pub fn runnable_query(text: &str, range: TextRange) -> Result<String, String> {
    let root = parse(text).syntax();
    let query = find_query(&root, range).ok_or("No query here")?;
    if let Some(name) = outer_reference(&query) {
        return Err(format!(
            "This query uses {name} from the query around it, so it can't run on its own"
        ));
    }
    let mut needed = needed_ctes(&query);
    let own = Query::cast(query.clone()).unwrap();
    let mut body = query.to_string();
    // A CTE whose columns are renamed, or that reads itself, runs as itself.
    if let Some(cte) = query.parent().and_then(Cte::cast) {
        let reads_itself = outside_cte_refs(&query).iter().any(|(_, d)| d.cte == cte)
            || query
                .descendants()
                .filter(|n| n.kind() == SyntaxKind::TABLE_REF)
                .filter_map(|n| resolve_cte(&n))
                .any(|d| d.cte == cte);
        if reads_itself || !cte.column_aliases().is_empty() {
            needed.retain(|d| d.cte != cte);
            let recursive = visible_ctes(&query).iter().any(|d| d.cte == cte && d.recursive);
            needed.push(CteDef {
                name: cte.name().unwrap_or_else(|| Name::new("")),
                cte: cte.clone(),
                recursive,
            });
            let upper = upper_keywords(&query);
            let name = cte.syntax().children().find(|c| c.kind() == SyntaxKind::NAME).map(|n| n.to_string());
            body = format!("{} * {} {}", kw(upper, "select"), kw(upper, "from"), name.unwrap_or_default());
        }
    }
    if needed.is_empty() {
        return Ok(body);
    }
    let upper = upper_keywords(&query);
    let mut ctes: Vec<String> = needed.iter().map(|d| d.cte.syntax().to_string()).collect();
    let mut recursive = needed.iter().any(|d| d.recursive);
    // The query's own WITH joins the one in front of it.
    if body == query.to_string()
        && let Some(with) = own.with_clause()
    {
        recursive |= own.is_recursive();
        ctes.extend(own.ctes().iter().map(|c| c.syntax().to_string()));
        let rest = usize::from(with.text_range().end() - query.text_range().start());
        body = body[rest..].trim_start().to_string();
    }
    let mut out = kw(upper, "with");
    if recursive {
        out.push(' ');
        out.push_str(&kw(upper, "recursive"));
    }
    out.push('\n');
    out.push_str(&ctes.join(",\n"));
    out.push('\n');
    out.push_str(&body);
    Ok(out)
}

/// Every CTE of the statement at `offset` and its query, with SQL that runs
/// each on its own: (name or `None` for the statement's own query, where
/// to show its result, SQL).
pub fn pipeline_queries(text: &str, offset: TextSize) -> Vec<(Option<String>, TextSize, String)> {
    let Some(statement) = query_nodes_at(text, offset)
        .into_iter()
        .find(|n| n.kind == QueryNodeKind::Statement)
    else {
        return Vec::new();
    };
    let root = parse(text).syntax();
    let Some(query) = find_query(&root, statement.range).and_then(Query::cast) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for cte in query.ctes() {
        let (Some(name), Some(inner)) = (cte.name(), cte.query()) else {
            continue;
        };
        if let Ok(sql) = runnable_query(text, inner.syntax().text_range()) {
            out.push((Some(name.text), cte.syntax().text_range().start(), sql));
        }
    }
    if let Some(body) = query.body() {
        out.push((None, body.text_range().start(), query.syntax().to_string()));
    }
    out
}

/// The query of the statement at `offset`.
fn statement_query(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxNode> {
    let token = token_near(root, offset)?;
    token
        .parent_ancestors()
        .filter(|n| n.kind() == SyntaxKind::QUERY)
        .last()
        .filter(is_statement_query)
}

/// The elements of a query after its WITH clause.
fn query_body_start(query: &Query) -> TextSize {
    let after_with = query.with_clause().map(|w| w.text_range().end());
    query
        .syntax()
        .children_with_tokens()
        .filter(|e| !e.kind().is_trivia() && e.kind() != SyntaxKind::WITH_CLAUSE)
        .map(|e| e.text_range().start())
        .find(|start| after_with.is_none_or(|end| *start >= end))
        .unwrap_or(query.syntax().text_range().end())
}

/// Moves the query at `range` into a CTE. A statement's own query becomes
/// the last CTE and the statement selects from it; a subquery becomes a CTE
/// before the one it is in and is replaced by its name. Without an alias to
/// name it, the new name is a placeholder selected wherever it is used.
pub fn extract_cte(text: &str, range: TextRange, style: &EditStyle) -> Result<Rewrite, String> {
    let root = parse(text).syntax();
    let query = find_query(&root, range).ok_or("No query here")?;
    let kind = node_kind(&query);
    if matches!(kind, QueryNodeKind::Cte { .. }) {
        return Err("This query is a CTE already: put the caret in the statement's query or a subquery".into());
    }
    if let Some(name) = outer_reference(&query) {
        return Err(format!("This query uses {name} from the query around it, so it can't move to a CTE"));
    }
    let upper = upper_keywords(&query);
    let ind = &style.indent;
    let top = query
        .ancestors()
        .filter(|n| n.kind() == SyntaxKind::QUERY)
        .last()
        .and_then(Query::cast)
        .ok_or("No query here")?;
    let existing: Vec<String> = top.ctes().iter().filter_map(|c| c.name()).map(|n| n.text).collect();
    let mut edits = Edits::default();

    if kind == QueryNodeKind::Statement {
        let own = Query::cast(query.clone()).unwrap();
        let start = query_body_start(&own);
        let end = query.text_range().end();
        let base = line_indent(text, usize::from(query.text_range().start())).to_string();
        let column = usize::from(start) - line_start(text, usize::from(start));
        let body = reindent(&text[span(TextRange::new(start, end))], column, &format!("{base}{ind}"));
        let name = unique_name("cte_name", &existing);
        let mut piece = Piece::default();
        let new_cte = |piece: &mut Piece| {
            piece.mark(0, &name).push(&format!(" {} (\n{body}\n{base})", kw(upper, "as")));
        };
        if let Some(last) = own.ctes().last() {
            piece.push(&format!(",\n{base}"));
            new_cte(&mut piece);
            piece.push(&format!("\n{base}"));
            edits.replace(TextRange::new(last.syntax().text_range().end(), start), piece);
        } else {
            piece.push(&kw(upper, "with")).push(" ");
            new_cte(&mut piece);
            piece.push(&format!("\n{base}"));
            edits.insert(start, piece);
        }
        let mut tail = Piece::new(format!(
            "{}\n{base}{ind}*\n{base}{} ",
            kw(upper, "select"),
            kw(upper, "from")
        ));
        tail.mark(0, &name);
        edits.replace(TextRange::new(start, end), tail);
        return Ok(edits.apply(text));
    }

    // A subquery moves to the statement's WITH, in front of the CTE it is in.
    let inside = top
        .ctes()
        .into_iter()
        .find(|c| c.syntax().text_range().contains_range(query.text_range()));
    for def in needed_ctes(&query) {
        let at_top = top.ctes().contains(&def.cte);
        let before = inside
            .as_ref()
            .is_none_or(|c| def.cte.syntax().text_range().start() < c.syntax().text_range().start());
        if !at_top || !before {
            return Err(format!(
                "This query uses {}, which isn't visible at the statement's WITH, so it can't move there",
                def.name.text
            ));
        }
    }
    let item = query.parent().and_then(FromItem::cast).filter(|i| i.kind() == SyntaxKind::SUBQUERY_REF);
    let alias = item.as_ref().and_then(|i| i.alias());
    let alias_name = alias
        .as_ref()
        .and_then(|a| a.name())
        .filter(|n| !n.quoted && !existing.iter().any(|e| e.eq_ignore_ascii_case(&n.text)));
    let (name, placeholder) = match &alias_name {
        Some(n) => (n.text.clone(), false),
        None => (unique_name("cte_name", &existing), true),
    };
    let base = line_indent(text, usize::from(top.syntax().text_range().start())).to_string();
    let start = usize::from(query.text_range().start());
    let column = start - line_start(text, start);
    let body = reindent(&query.to_string(), column, &format!("{base}{ind}"));
    let mut cte = Piece::default();
    let add_cte = |piece: &mut Piece| {
        if placeholder {
            piece.mark(0, &name);
        } else {
            piece.push(&name);
        }
        piece.push(&format!(" {} (\n{body}\n{base})", kw(upper, "as")));
    };
    match (&inside, top.ctes().last()) {
        (Some(c), _) => {
            add_cte(&mut cte);
            cte.push(&format!(",\n{base}"));
            edits.insert(c.syntax().text_range().start(), cte);
        }
        (None, Some(last)) => {
            cte.push(&format!(",\n{base}"));
            add_cte(&mut cte);
            edits.insert(last.syntax().text_range().end(), cte);
        }
        (None, None) => {
            cte.push(&kw(upper, "with")).push(" ");
            add_cte(&mut cte);
            cte.push(&format!("\n{base}"));
            edits.insert(top.syntax().text_range().start(), cte);
        }
    }
    // Without a placeholder to name, the caret goes where the subquery was.
    let use_name = |piece: &mut Piece| {
        if placeholder {
            piece.mark(0, &name);
        } else {
            piece.push(&name).mark(0, "");
        }
    };
    let mut use_piece = Piece::default();
    if let Some(item) = &item {
        use_name(&mut use_piece);
        // Column aliases still rename the CTE's columns.
        if let Some(alias) = alias.filter(|a| !a.columns().is_empty() || alias_name.is_none()) {
            use_piece.push(" ").push(alias.syntax().to_string().trim());
        }
        edits.replace(item.syntax().text_range(), use_piece);
    } else {
        use_piece.push(&format!("{} * {} ", kw(upper, "select"), kw(upper, "from")));
        use_name(&mut use_piece);
        edits.replace(query.text_range(), use_piece);
    }
    Ok(edits.apply(text))
}

/// Replaces uses of the CTE at `offset` with its query. On the CTE's name,
/// every use is replaced; on one use, only that one. A CTE left unused is
/// removed.
pub fn inline_cte(text: &str, offset: TextSize, style: &EditStyle) -> Result<Rewrite, String> {
    let root = parse(text).syntax();
    let token = token_near(&root, offset).ok_or("Put the caret on a CTE's name or a use of it")?;
    let (def, only) = if let Some(cte) = token
        .parent_ancestors()
        .take_while(|n| matches!(n.kind(), SyntaxKind::NAME | SyntaxKind::CTE))
        .find_map(Cte::cast)
    {
        let def = visible_ctes(cte.query().map(|q| q.syntax().clone()).as_ref().unwrap_or(cte.syntax()))
            .into_iter()
            .chain(std::iter::once(CteDef {
                name: cte.name().unwrap_or_else(|| Name::new("")),
                cte: cte.clone(),
                recursive: false,
            }))
            .find(|d| d.cte == cte)
            .unwrap();
        (def, None)
    } else {
        let table_ref = token
            .parent_ancestors()
            .take_while(|n| matches!(n.kind(), SyntaxKind::PATH | SyntaxKind::TABLE_REF))
            .find(|n| n.kind() == SyntaxKind::TABLE_REF)
            .ok_or("Put the caret on a CTE's name or a use of it")?;
        let def = resolve_cte(&table_ref).ok_or("This table isn't a CTE")?;
        (def, Some(table_ref))
    };
    let query = def.cte.query().ok_or("This CTE has no query")?;
    let scope = def
        .cte
        .syntax()
        .ancestors()
        .find(|n| n.kind() == SyntaxKind::QUERY)
        .ok_or("No query here")?;
    let uses: Vec<SyntaxNode> = scope
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::TABLE_REF)
        .filter(|n| resolve_cte(n).is_some_and(|d| d.cte == def.cte))
        .collect();
    if uses
        .iter()
        .any(|u| def.cte.syntax().text_range().contains_range(u.text_range()))
    {
        return Err(format!("{} reads itself, so it can't be inlined", def.name.text));
    }
    let targets: Vec<SyntaxNode> = match &only {
        Some(one) => vec![one.clone()],
        None => uses.clone(),
    };
    if targets.is_empty() {
        return Err(format!("{} isn't used", def.name.text));
    }
    let upper = upper_keywords(def.cte.syntax());
    let query_text = query.syntax().to_string();
    let query_start = usize::from(query.syntax().text_range().start());
    let column = query_start - line_start(text, query_start);
    let mut edits = Edits::default();
    for target in &targets {
        let item = FromItem::cast(target.clone()).unwrap();
        let base = line_indent(text, usize::from(target.text_range().start())).to_string();
        let body = reindent(&query_text, column, &format!("{base}{}", style.indent));
        let alias = match item.alias() {
            Some(alias) => alias.syntax().to_string(),
            None => {
                let mut alias = format!("{} {}", kw(upper, "as"), def.name.text);
                let columns = def.cte.column_aliases();
                if !columns.is_empty() {
                    let names: Vec<String> = columns.iter().map(|c| c.text.clone()).collect();
                    alias.push_str(&format!("({})", names.join(", ")));
                }
                alias
            }
        };
        let mut piece = Piece::default();
        piece.mark(0, "").push(&format!("(\n{body}\n{base}) {alias}"));
        edits.replace(target.text_range(), piece);
    }
    if targets.len() == uses.len() {
        edits.replace(cte_removal(text, &def.cte), Piece::default());
    }
    let mut rewrite = edits.apply(text);
    rewrite.stops.truncate(1);
    if let Some(first) = rewrite.stops.first_mut() {
        first.truncate(1);
    }
    Ok(rewrite)
}

/// The text to delete to remove a CTE: with the comma next to it, or the
/// whole WITH when it is the only CTE.
fn cte_removal(text: &str, cte: &Cte) -> TextRange {
    let with = cte.syntax().parent().unwrap();
    let ctes: Vec<Cte> = with.children().filter_map(Cte::cast).collect();
    let i = ctes.iter().position(|c| c == cte).unwrap();
    let range = cte.syntax().text_range();
    if ctes.len() == 1 {
        let end = usize::from(with.text_range().end());
        let after = text[end..].len() - text[end..].trim_start().len();
        return TextRange::new(with.text_range().start(), size(end + after));
    }
    if let Some(next) = ctes.get(i + 1) {
        TextRange::new(range.start(), next.syntax().text_range().start())
    } else {
        TextRange::new(ctes[i - 1].syntax().text_range().end(), range.end())
    }
}

/// Wraps the query at `range` as `SELECT * FROM (query)`, with `*` selected
/// and a tab stop after the parenthesis.
pub fn wrap_query(text: &str, range: TextRange, style: &EditStyle) -> Result<Rewrite, String> {
    let root = parse(text).syntax();
    let query = find_query(&root, range).ok_or("No query here")?;
    let upper = upper_keywords(&query);
    let start = usize::from(range.start());
    let inner = query.to_string();
    let mut piece = Piece::default();
    if !inner.contains('\n') && !starts_line(text, start) {
        piece
            .push(&kw(upper, "select"))
            .push(" ")
            .mark(0, "*")
            .push(&format!(" {} ({inner})", kw(upper, "from")))
            .mark(1, "");
    } else {
        let base = if starts_line(text, start) {
            line_indent(text, start).to_string()
        } else {
            format!("{}{}", line_indent(text, start), style.indent)
        };
        let ind = &style.indent;
        let column = start - line_start(text, start);
        let body = reindent(&inner, column, &format!("{base}{ind}"));
        piece
            .push(&format!("{}\n{base}{ind}", kw(upper, "select")))
            .mark(0, "*")
            .push(&format!("\n{base}{} (\n{body}\n{base})", kw(upper, "from")))
            .mark(1, "");
    }
    let mut edits = Edits::default();
    edits.replace(range, piece);
    Ok(edits.apply(text))
}

/// Adds `LIMIT 100` to the statement's query at `offset`, or removes its LIMIT.
pub fn toggle_limit(text: &str, offset: TextSize) -> Result<Rewrite, String> {
    let root = parse(text).syntax();
    let query = statement_query(&root, offset).ok_or("No query here")?;
    let mut edits = Edits::default();
    if let Some(limit) = query.children().find(|c| c.kind() == SyntaxKind::LIMIT_CLAUSE) {
        let previous = limit
            .siblings_with_tokens(rowan::Direction::Prev)
            .skip(1)
            .find(|e| !e.kind().is_trivia())
            .map_or(limit.text_range().start(), |e| e.text_range().end());
        edits.replace(TextRange::new(previous, limit.text_range().end()), Piece::default());
    } else {
        let upper = upper_keywords(&query);
        let start = usize::from(query.text_range().start());
        let base = line_indent(text, start);
        let multiline = query.to_string().contains('\n');
        let gap = if multiline { format!("\n{base}") } else { " ".into() };
        let mut piece = Piece::new(format!("{gap}{} ", kw(upper, "limit")));
        piece.mark(0, "100");
        edits.insert(query.text_range().end(), piece);
    }
    Ok(edits.apply(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, marker: &str) -> TextSize {
        size(text.find(marker).unwrap())
    }

    const PIPELINE: &str = "with a as (select x from t),\nb as (select x from a),\nc as (select 1 as one)\nselect * from b join (select one from c) s on true";

    #[test]
    fn nodes_go_from_the_subquery_out_to_the_statement() {
        let nodes = query_nodes_at(PIPELINE, at(PIPELINE, "one from c"));
        let kinds: Vec<_> = nodes.iter().map(|n| n.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                QueryNodeKind::Subquery {
                    alias: Some("s".into())
                },
                QueryNodeKind::Statement
            ]
        );
        let nodes = query_nodes_at(PIPELINE, at(PIPELINE, "b as"));
        assert_eq!(nodes[0].kind, QueryNodeKind::Cte { name: "b".into() });
        assert_eq!(&PIPELINE[span(nodes[0].range)], "select x from a");
    }

    #[test]
    fn a_cte_runs_with_the_ctes_it_reads() {
        let node = &query_nodes_at(PIPELINE, at(PIPELINE, "x from a"))[0];
        assert_eq!(
            runnable_query(PIPELINE, node.range).unwrap(),
            "with\na as (select x from t)\nselect x from a"
        );
        let node = &query_nodes_at(PIPELINE, at(PIPELINE, "one from c"))[0];
        assert_eq!(
            runnable_query(PIPELINE, node.range).unwrap(),
            "with\nc as (select 1 as one)\nselect one from c"
        );
        let first = &query_nodes_at(PIPELINE, at(PIPELINE, "x from t"))[0];
        assert_eq!(runnable_query(PIPELINE, first.range).unwrap(), "select x from t");
    }

    #[test]
    fn renamed_and_recursive_ctes_run_as_themselves() {
        let text = "with recursive n(i) as (select 1 union all select i + 1 from n where i < 3) select * from n";
        let node = &query_nodes_at(text, at(text, "1 union"))[0];
        assert_eq!(
            runnable_query(text, node.range).unwrap(),
            "with recursive\nn(i) as (select 1 union all select i + 1 from n where i < 3)\nselect * from n"
        );
    }

    #[test]
    fn a_query_with_its_own_with_merges_it() {
        let text = "with a as (select 1 x), b as (with c as (select x from a) select * from c) select * from b";
        let node = &query_nodes_at(text, at(text, "* from c"))[0];
        assert_eq!(
            runnable_query(text, node.range).unwrap(),
            "with\na as (select 1 x),\nc as (select x from a)\nselect * from c"
        );
    }

    #[test]
    fn correlated_subqueries_dont_run_alone() {
        let text = "select * from orders o where exists (select 1 from items i where i.order_id = o.id)";
        let node = &query_nodes_at(text, at(text, "1 from items"))[0];
        let error = runnable_query(text, node.range).unwrap_err();
        assert!(error.contains("uses o"), "{error}");
    }

    #[test]
    fn the_statement_query_moves_to_a_new_cte() {
        let text = "select *\nfrom abc";
        let node = &query_nodes_at(text, size(0))[0];
        let rewrite = extract_cte(text, node.range, &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "with cte_name as (\n    select *\n    from abc\n)\nselect\n    *\nfrom cte_name"
        );
        let names: Vec<&str> = rewrite.stops[0].iter().map(|r| &rewrite.text[span(*r)]).collect();
        assert_eq!(names, ["cte_name", "cte_name"]);

        let text = "WITH a AS (SELECT 1)\nSELECT * FROM a";
        let node = &query_nodes_at(text, at(text, "* FROM a"))[0];
        let rewrite = extract_cte(text, node.range, &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "WITH a AS (SELECT 1),\ncte_name AS (\n    SELECT * FROM a\n)\nSELECT\n    *\nFROM cte_name"
        );
    }

    #[test]
    fn a_subquery_moves_in_front_of_its_cte() {
        let text = "with a as (select 1 x),\nb as (select * from (select x from a) s)\nselect * from b";
        let node = &query_nodes_at(text, at(text, "x from a"))[0];
        let rewrite = extract_cte(text, node.range, &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "with a as (select 1 x),\ns as (\n    select x from a\n),\nb as (select * from s)\nselect * from b"
        );
        assert_eq!(rewrite.stops[0].len(), 1);

        let text = "select * from t where id in (select id from u)";
        let node = &query_nodes_at(text, at(text, "id from u"))[0];
        let rewrite = extract_cte(text, node.range, &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "with cte_name as (\n    select id from u\n)\nselect * from t where id in (select * from cte_name)"
        );
        assert_eq!(rewrite.stops[0].len(), 2);
    }

    #[test]
    fn inlining_replaces_uses_and_drops_the_cte() {
        let text = "with a as (select 1 x)\nselect * from a";
        let rewrite = inline_cte(text, at(text, "a as"), &EditStyle::default()).unwrap();
        assert_eq!(rewrite.text, "select * from (\n    select 1 x\n) as a");

        let text = "with a as (select 1 x), b as (select 2 y)\nselect * from a, b, a a2";
        let rewrite = inline_cte(text, at(text, "a, b"), &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "with a as (select 1 x), b as (select 2 y)\nselect * from (\n    select 1 x\n) as a, b, a a2"
        );
        let rewrite = inline_cte(text, at(text, "b as"), &EditStyle::default()).unwrap();
        assert_eq!(
            rewrite.text,
            "with a as (select 1 x)\nselect * from a, (\n    select 2 y\n) as b, a a2"
        );
    }

    #[test]
    fn wrapping_selects_the_star() {
        let text = "select a\nfrom t";
        let rewrite = wrap_query(text, query_nodes_at(text, size(0))[0].range, &EditStyle::default()).unwrap();
        assert_eq!(rewrite.text, "select\n    *\nfrom (\n    select a\n    from t\n)");
        assert_eq!(&rewrite.text[span(rewrite.stops[0][0])], "*");
        assert_eq!(usize::from(rewrite.stops[1][0].start()), rewrite.text.len());
    }

    #[test]
    fn limit_toggles() {
        let text = "select * from t";
        let added = toggle_limit(text, size(0)).unwrap();
        assert_eq!(added.text, "select * from t limit 100");
        let removed = toggle_limit(&added.text, size(0)).unwrap();
        assert_eq!(removed.text, text);
    }

    #[test]
    fn pipeline_lists_each_cte_and_the_query() {
        let names: Vec<_> = pipeline_queries(PIPELINE, size(0)).into_iter().map(|(n, _, _)| n).collect();
        assert_eq!(names, [Some("a".into()), Some("b".into()), Some("c".into()), None]);
    }
}
