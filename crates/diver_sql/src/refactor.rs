//! Small rewrites of the query at a position: GROUP BY from the select
//! list, JOIN conditions from shared columns, `*` to and from its columns,
//! keyword case, and SQL made from pasted values.

use rowan::{TextRange, TextSize};

use crate::ast::{AstNode, FromItem, Query, SelectCore, StarExpr};
use crate::catalog::Catalog;
use crate::edit::{EditStyle, Edits, Piece, Rewrite, kw, line_indent, starts_line, token_near, upper_keywords};
use crate::items::{quote_if_needed, relation_columns};
use crate::scope::{VisibleRelation, item_relations};
use crate::{DocumentObject, SyntaxKind, SyntaxNode, SyntaxToken, document_objects, parse};

/// Aggregate functions DuckDB has built in, for when the catalog hasn't
/// listed its functions yet.
const AGGREGATES: &[&str] = &[
    "any_value", "approx_count_distinct", "approx_quantile", "arbitrary", "arg_max", "arg_min", "argmax",
    "argmin", "array_agg", "avg", "bit_and", "bit_or", "bit_xor", "bitstring_agg", "bool_and", "bool_or",
    "corr", "count", "count_star", "covar_pop", "covar_samp", "entropy", "favg", "first", "fsum",
    "geomean", "group_concat", "histogram", "kurtosis", "last", "list", "listagg", "mad", "max", "max_by",
    "mean", "median", "min", "min_by", "mode", "product", "quantile", "quantile_cont", "quantile_disc",
    "regr_avgx", "regr_avgy", "regr_count", "regr_intercept", "regr_r2", "regr_slope", "regr_sxx",
    "regr_sxy", "regr_syy", "skewness", "stddev", "stddev_pop", "stddev_samp", "string_agg", "sum",
    "sumkahan", "var_pop", "var_samp", "variance",
];

fn core_at(root: &SyntaxNode, offset: TextSize) -> Option<SelectCore> {
    let token = token_near(root, offset)?;
    for node in token.parent_ancestors() {
        if let Some(core) = SelectCore::cast(node.clone()) {
            return Some(core);
        }
        if let Some(query) = Query::cast(node) {
            return query.first_core().and_then(SelectCore::cast);
        }
    }
    None
}

fn child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

fn is_aggregate(call: &SyntaxNode, aggregates: &[String]) -> bool {
    let Some(name) = call
        .children()
        .find(|c| c.kind() == SyntaxKind::PATH)
        .and_then(crate::ast::Path::cast)
        .and_then(|p| p.parts().pop())
    else {
        return false;
    };
    aggregates.iter().any(|a| name.matches(a)) || AGGREGATES.iter().any(|a| name.matches(a))
}

/// Whether an expression aggregates: calls an aggregate outside a
/// subquery, or is a window function.
fn aggregates_rows(expr: &SyntaxNode, aggregates: &[String]) -> bool {
    let mut stack = vec![expr.clone()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            SyntaxKind::SUBQUERY_EXPR | SyntaxKind::EXISTS_EXPR | SyntaxKind::QUERY => continue,
            SyntaxKind::OVER_CLAUSE => return true,
            SyntaxKind::CALL_EXPR if is_aggregate(&node, aggregates) => return true,
            _ => stack.extend(node.children()),
        }
    }
    false
}

/// Adds a GROUP BY of the select list's items that don't aggregate, or
/// switches between that list and `GROUP BY ALL`.
pub fn group_by(text: &str, offset: TextSize, catalog: &dyn Catalog) -> Result<Rewrite, String> {
    let root = parse(text).syntax();
    let core = core_at(&root, offset).ok_or("Put the caret in a SELECT")?;
    let upper = upper_keywords(core.syntax());
    let aggregates: Vec<String> = catalog
        .functions()
        .into_iter()
        .filter(|f| f.kind == crate::FunctionKind::Aggregate)
        .map(|f| f.name)
        .collect();
    let mut list = Vec::new();
    for item in core.select_items() {
        let Some(expr) = item.expr() else { continue };
        if expr.kind() == SyntaxKind::STAR_EXPR {
            return Err("The select list has *: expand it first (e) to group by its columns".into());
        }
        let columns = expr.kind() == SyntaxKind::COLUMN_REF || expr.descendants().any(|n| n.kind() == SyntaxKind::COLUMN_REF);
        if !columns || aggregates_rows(&expr, &aggregates) {
            continue;
        }
        let name = match (expr.kind(), item.alias()) {
            (SyntaxKind::COLUMN_REF, _) | (_, None) => expr.to_string().trim().to_string(),
            (_, Some(alias)) => quote_if_needed(&alias.text),
        };
        list.push(name);
    }
    let mut edits = Edits::default();
    if let Some(group) = child(core.syntax(), SyntaxKind::GROUP_BY_CLAUSE) {
        let all = group.children_with_tokens().any(|e| e.kind() == SyntaxKind::ALL_KW);
        let by = group
            .children_with_tokens()
            .find(|e| e.kind() == SyntaxKind::BY_KW)
            .ok_or("This GROUP BY is incomplete")?;
        let after = TextRange::new(by.text_range().end(), group.text_range().end());
        let replacement = if all {
            if list.is_empty() {
                return Err("No columns to group by: every item aggregates".into());
            }
            format!(" {}", list.join(", "))
        } else {
            format!(" {}", kw(upper, "all"))
        };
        edits.replace(after, Piece::new(replacement));
        return Ok(edits.apply(text));
    }
    if list.is_empty() {
        return Err("No columns to group by: every item aggregates".into());
    }
    let anchor = [SyntaxKind::WHERE_CLAUSE, SyntaxKind::FROM_CLAUSE, SyntaxKind::SELECT_CLAUSE]
        .into_iter()
        .filter_map(|kind| child(core.syntax(), kind))
        .map(|n| n.text_range().end())
        .max()
        .ok_or("Put the caret in a SELECT")?;
    let start = usize::from(core.syntax().text_range().start());
    let gap = if core.syntax().to_string().contains('\n') {
        format!("\n{}", line_indent(text, start))
    } else {
        " ".into()
    };
    edits.insert(
        anchor,
        Piece::new(format!("{gap}{} {}", kw(upper, "group by"), list.join(", "))),
    );
    Ok(edits.apply(text))
}

/// The relations of a FROM item with their columns, as far as they are known.
fn columns_of(item: &FromItem, catalog: &dyn Catalog, objects: &[DocumentObject]) -> Vec<(VisibleRelation, Vec<String>)> {
    item_relations(item)
        .into_iter()
        .map(|relation| {
            let columns = relation_columns(&relation, catalog, objects, 0)
                .into_iter()
                .map(|c| c.name)
                .collect();
            (relation, columns)
        })
        .collect()
}

fn relation_label(relation: &VisibleRelation) -> String {
    relation.name().map_or_else(|| "a subquery".to_string(), |n| n.text)
}

/// Fills in `ON` for the join at `offset` from the columns both sides
/// have: those named like keys (`id`, `customer_id`) when there are any.
pub fn join_on(text: &str, offset: TextSize, catalog: &dyn Catalog) -> Result<Rewrite, String> {
    let parsed = parse(text);
    let root = parsed.syntax();
    let objects = document_objects(&parsed);
    let token = token_near(&root, offset).ok_or("Put the caret in a JOIN")?;
    let join = token
        .parent_ancestors()
        .find(|n| n.kind() == SyntaxKind::JOIN_EXPR)
        .ok_or("Put the caret in a JOIN")?;
    let words: Vec<SyntaxKind> = join.children_with_tokens().map(|e| e.kind()).collect();
    if words.iter().any(|k| {
        matches!(
            *k,
            SyntaxKind::CROSS_KW | SyntaxKind::NATURAL_KW | SyntaxKind::POSITIONAL_KW | SyntaxKind::JOIN_USING
        )
    }) {
        return Err("This join takes no ON condition".into());
    }
    let empty_on = child(&join, SyntaxKind::JOIN_ON);
    if empty_on
        .as_ref()
        .is_some_and(|on| on.children().next().is_some())
    {
        return Err("This join has a condition already".into());
    }
    let items: Vec<FromItem> = join.children().filter_map(FromItem::cast).collect();
    let [left, right] = items.as_slice() else {
        return Err("This join is incomplete".into());
    };
    let left = columns_of(left, catalog, &objects);
    let right = columns_of(right, catalog, &objects);
    for (relation, columns) in left.iter().chain(&right) {
        if columns.is_empty() {
            return Err(format!("The columns of {} aren't known yet", relation_label(relation)));
        }
    }
    let mut pairs = Vec::new();
    for (r_rel, r_cols) in &right {
        for column in r_cols {
            if let Some((l_rel, _)) = left
                .iter()
                .find(|(_, cols)| cols.iter().any(|c| c.eq_ignore_ascii_case(column)))
            {
                pairs.push((l_rel, r_rel, column.clone()));
            }
        }
    }
    let key = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower == "id" || lower.ends_with("_id") || lower.ends_with("id") && lower.len() > 2 || lower.ends_with("key")
    };
    if pairs.iter().any(|(_, _, c)| key(c)) {
        pairs.retain(|(_, _, c)| key(c));
    }
    if pairs.is_empty() {
        return Err("The two sides share no column names".into());
    }
    let upper = upper_keywords(&join);
    let qualified = |relation: &VisibleRelation, column: &str| match relation.name() {
        Some(name) => format!("{}.{}", quote_if_needed(&name.text), quote_if_needed(column)),
        None => quote_if_needed(column),
    };
    let condition = pairs
        .iter()
        .map(|(l, r, c)| format!("{} = {}", qualified(l, c), qualified(r, c)))
        .collect::<Vec<_>>()
        .join(&format!(" {} ", kw(upper, "and")));
    let mut edits = Edits::default();
    match empty_on {
        Some(on) => edits.replace(on.text_range(), Piece::new(format!("{} {condition}", kw(upper, "on")))),
        None => edits.insert(
            join.text_range().end(),
            Piece::new(format!(" {} {condition}", kw(upper, "on"))),
        ),
    }
    Ok(edits.apply(text))
}

fn star_at(root: &SyntaxNode, offset: TextSize) -> Option<StarExpr> {
    let token = token_near(root, offset)?;
    if let Some(star) = token.parent_ancestors().find_map(StarExpr::cast) {
        return Some(star);
    }
    // Anywhere in a select list with one `*`.
    let core = core_at(root, offset)?;
    let mut stars = core
        .select_items()
        .into_iter()
        .filter_map(|item| item.expr().and_then(StarExpr::cast));
    let star = stars.next()?;
    stars.next().is_none().then_some(star)
}

/// Replaces the `*` at `offset` with the columns it stands for.
pub fn expand_star(text: &str, offset: TextSize, catalog: &dyn Catalog, style: &EditStyle) -> Result<Rewrite, String> {
    let parsed = parse(text);
    let root = parsed.syntax();
    let objects = document_objects(&parsed);
    let star = star_at(&root, offset).ok_or("Put the caret on a *")?;
    if child(star.syntax(), SyntaxKind::REPLACE_CLAUSE).is_some() {
        return Err("A * with REPLACE stays as it is".into());
    }
    let core = star
        .syntax()
        .ancestors()
        .find_map(SelectCore::cast)
        .ok_or("Put the caret on a * in a SELECT")?;
    let qualifier = star.qualifier();
    let mut relations = Vec::new();
    for item in core.from_items() {
        relations.extend(columns_of(&item, catalog, &objects));
    }
    if !qualifier.is_empty() {
        relations.retain(|(r, _)| r.matches_qualifier(&qualifier));
    }
    if relations.is_empty() {
        return Err("This * selects from nothing the editor knows".into());
    }
    if let Some((relation, _)) = relations.iter().find(|(_, c)| c.is_empty()) {
        return Err(format!("The columns of {} aren't known yet", relation_label(relation)));
    }
    let exclude = star.exclude();
    let rename = star.rename();
    let qualify = relations.len() > 1 || !qualifier.is_empty();
    let mut names = Vec::new();
    for (relation, columns) in &relations {
        let prefix = relation
            .name()
            .filter(|_| qualify)
            .map(|n| format!("{}.", quote_if_needed(&n.text)))
            .unwrap_or_default();
        for column in columns {
            if exclude.iter().any(|e| e.matches(column)) {
                continue;
            }
            let mut name = format!("{prefix}{}", quote_if_needed(column));
            if let Some((_, new)) = rename.iter().find(|(old, _)| old.matches(column)) {
                name.push_str(&format!(" {} {}", kw(upper_keywords(core.syntax()), "as"), quote_if_needed(&new.text)));
            }
            names.push(name);
        }
    }
    let start = usize::from(star.syntax().text_range().start());
    let indent = line_indent(text, start);
    let joined = names.join(", ");
    let list = if starts_line(text, start) {
        names.join(&format!(",\n{indent}"))
    } else if joined.len() <= 60 {
        joined
    } else {
        names.join(&format!(",\n{indent}{}", style.indent))
    };
    let mut edits = Edits::default();
    edits.replace(star.syntax().text_range(), Piece::new(list));
    Ok(edits.apply(text))
}

/// Replaces a select list of plain columns with `*`, adding EXCLUDE for the
/// columns it left out when they are known.
pub fn collapse_star(text: &str, offset: TextSize, catalog: &dyn Catalog) -> Result<Rewrite, String> {
    let parsed = parse(text);
    let root = parsed.syntax();
    let objects = document_objects(&parsed);
    let core = core_at(&root, offset).ok_or("Put the caret in a SELECT")?;
    let items = core.select_items();
    let (Some(first), Some(last)) = (items.first(), items.last()) else {
        return Err("This SELECT has no columns".into());
    };
    let mut chosen = Vec::new();
    for item in &items {
        match item.expr() {
            Some(expr) if expr.kind() == SyntaxKind::COLUMN_REF && item.alias().is_none() => {
                chosen.extend(item.output_name());
            }
            _ => return Err("Only a list of plain columns collapses to *".into()),
        }
    }
    let mut all = Vec::new();
    for item in core.from_items() {
        all.extend(columns_of(&item, catalog, &objects).into_iter().flat_map(|(_, c)| c));
    }
    let upper = upper_keywords(core.syntax());
    let missing: Vec<String> = all
        .iter()
        .filter(|c| !chosen.iter().any(|n| n.matches(c)))
        .map(|c| quote_if_needed(c))
        .collect();
    let star = if missing.is_empty() || missing.len() == all.len() {
        "*".to_string()
    } else {
        format!("* {} ({})", kw(upper, "exclude"), missing.join(", "))
    };
    let mut edits = Edits::default();
    edits.replace(
        TextRange::new(first.syntax().text_range().start(), last.syntax().text_range().end()),
        Piece::new(star),
    );
    Ok(edits.apply(text))
}

/// Whether a keyword token is used as a keyword rather than as a name.
fn is_keyword_use(token: &SyntaxToken) -> bool {
    token.kind().is_keyword()
        && !token
            .parent()
            .is_some_and(|p| matches!(p.kind(), SyntaxKind::PATH | SyntaxKind::NAME))
}

/// Writes the keywords in `range` (all of the text without one) in upper
/// case, or in lower case when they all are upper case already.
pub fn keyword_case(text: &str, range: Option<TextRange>) -> Rewrite {
    let root = parse(text).syntax();
    let range = range.unwrap_or(root.text_range());
    let keywords: Vec<SyntaxToken> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| range.contains_range(t.text_range()) && is_keyword_use(t))
        .collect();
    let upper = keywords
        .iter()
        .any(|t| t.text().chars().any(|c| c.is_ascii_lowercase()));
    let mut edits = Edits::default();
    for token in keywords {
        let new = if upper { token.text().to_uppercase() } else { token.text().to_lowercase() };
        if new != token.text() {
            edits.replace(token.text_range(), Piece::new(new));
        }
    }
    edits.apply(text)
}

fn is_number(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    !whole.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && (whole == "0" || !whole.starts_with('0'))
        && !(digits.contains('.') && fraction.is_empty())
}

fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Pasted lines (a column copied from a spreadsheet, say) as an IN list:
/// `('a', 'b')`, without duplicates, and unquoted when every value is a number.
pub fn in_list(pasted: &str, style: &EditStyle) -> Result<String, String> {
    let mut values: Vec<&str> = Vec::new();
    for line in pasted.lines() {
        let value = line.trim();
        if !value.is_empty() && !values.contains(&value) {
            values.push(value);
        }
    }
    if values.is_empty() {
        return Err("The clipboard has no lines to paste".into());
    }
    let numbers = values.iter().all(|v| is_number(v));
    let literals: Vec<String> = values
        .iter()
        .map(|v| if numbers { v.to_string() } else { string_literal(v) })
        .collect();
    if literals.len() <= 10 {
        return Ok(format!("({})", literals.join(", ")));
    }
    let rows: Vec<String> = literals
        .chunks(10)
        .map(|chunk| format!("{}{}", style.indent, chunk.join(", ")))
        .collect();
    Ok(format!("(\n{}\n)", rows.join(",\n")))
}

/// Pasted rows of tab-separated cells as a table: `(VALUES (…), (…)) AS t(…)`.
/// A first row of distinct names over a column of numbers is the header.
pub fn values_table(pasted: &str, upper: bool, style: &EditStyle) -> Result<String, String> {
    let rows: Vec<Vec<&str>> = pasted
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(str::trim).collect())
        .collect();
    let width = rows.iter().map(Vec::len).max().ok_or("The clipboard has no rows to paste")?;
    let numeric = |rows: &[Vec<&str>], i: usize| {
        let mut cells = rows.iter().filter_map(|r| r.get(i)).filter(|c| !c.is_empty()).peekable();
        cells.peek().is_some() && cells.all(|c| is_number(c))
    };
    let first = &rows[0];
    let header = rows.len() > 1
        && first.len() == width
        && first.iter().all(|c| !c.is_empty() && !is_number(c))
        && first.iter().enumerate().all(|(i, c)| !first[..i].contains(c))
        && (0..width).any(|i| numeric(&rows[1..], i));
    let (names, body): (Vec<String>, &[Vec<&str>]) = if header {
        (first.iter().map(|c| quote_if_needed(c)).collect(), &rows[1..])
    } else {
        ((1..=width).map(|i| format!("column{i}")).collect(), &rows[..])
    };
    let numbers: Vec<bool> = (0..width).map(|i| numeric(body, i)).collect();
    let lines: Vec<String> = body
        .iter()
        .map(|row| {
            let cells: Vec<String> = (0..width)
                .map(|i| match row.get(i).copied().filter(|c| !c.is_empty()) {
                    None => kw(upper, "null"),
                    Some(c) if numbers[i] => c.to_string(),
                    Some(c) => string_literal(c),
                })
                .collect();
            format!("{}({})", style.indent, cells.join(", "))
        })
        .collect();
    Ok(format!(
        "({}\n{}\n) {} t({})",
        kw(upper, "values"),
        lines.join(",\n"),
        kw(upper, "as"),
        names.join(", ")
    ))
}

/// A statement on one line, without comments, as a double-quoted string
/// Python and Rust both read.
pub fn one_line_string(sql: &str) -> String {
    let mut line = String::new();
    for token in crate::tokenize(sql).into_iter().scan(0usize, |at, t| {
        let start = *at;
        *at += usize::from(t.len);
        Some((t.kind, &sql[start..*at]))
    }) {
        match token.0 {
            SyntaxKind::WHITESPACE => {
                if !line.is_empty() && !line.ends_with(' ') {
                    line.push(' ');
                }
            }
            kind if kind.is_comment() => {
                if !line.is_empty() && !line.ends_with(' ') {
                    line.push(' ');
                }
            }
            _ => line.push_str(token.1),
        }
    }
    let line = line.trim().trim_end_matches(';').trim_end();
    format!("\"{}\"", line.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockCatalog;
    use crate::edit::size;

    fn at(text: &str, marker: &str) -> TextSize {
        size(text.find(marker).unwrap())
    }

    fn catalog() -> MockCatalog {
        MockCatalog::new()
            .table("orders", &["id", "customer_id", "amount"])
            .table("customers", &["id", "name"])
    }

    #[test]
    fn group_by_lists_items_that_dont_aggregate() {
        let text = "select region, upper(name) n, sum(x), count(*) over () from t where x > 0";
        let rewrite = group_by(text, size(0), &catalog()).unwrap();
        assert_eq!(rewrite.text, format!("{text} group by region, n"));
        let switched = group_by(&rewrite.text, size(0), &catalog()).unwrap();
        assert_eq!(switched.text, format!("{text} group by all"));
        let back = group_by(&switched.text, size(0), &catalog()).unwrap();
        assert_eq!(back.text, rewrite.text);

        let text = "SELECT a, SUM(b)\nFROM t\nORDER BY 1";
        assert_eq!(
            group_by(text, size(0), &catalog()).unwrap().text,
            "SELECT a, SUM(b)\nFROM t\nGROUP BY a\nORDER BY 1"
        );
    }

    #[test]
    fn join_on_uses_key_columns_both_sides_have() {
        let text = "select * from orders o join customers c";
        let rewrite = join_on(text, at(text, "join"), &catalog()).unwrap();
        assert_eq!(rewrite.text, format!("{text} on o.id = c.id"));
        let error = join_on("select * from orders join nowhere n", size(20), &catalog()).unwrap_err();
        assert!(error.contains("nowhere") || error.contains("n"), "{error}");
    }

    #[test]
    fn stars_expand_and_collapse() {
        let text = "select * from orders";
        let expanded = expand_star(text, at(text, "*"), &catalog(), &EditStyle::default()).unwrap();
        assert_eq!(expanded.text, "select id, customer_id, amount from orders");
        let text = "select o.* exclude (amount) from orders o join customers c on true";
        let expanded = expand_star(text, at(text, "*"), &catalog(), &EditStyle::default()).unwrap();
        assert_eq!(
            expanded.text,
            "select o.id, o.customer_id from orders o join customers c on true"
        );
        let text = "select\n    *\nfrom customers";
        let expanded = expand_star(text, at(text, "*"), &catalog(), &EditStyle::default()).unwrap();
        assert_eq!(expanded.text, "select\n    id,\n    name\nfrom customers");

        let text = "select id, amount from orders";
        let collapsed = collapse_star(text, size(0), &catalog()).unwrap();
        assert_eq!(collapsed.text, "select * exclude (customer_id) from orders");
    }

    #[test]
    fn keyword_case_toggles() {
        let text = "select name, count(*) from t where \"select\" is not null";
        let upper = keyword_case(text, None);
        assert_eq!(upper.text, "SELECT name, count(*) FROM t WHERE \"select\" IS NOT NULL");
        assert_eq!(keyword_case(&upper.text, None).text, text);
    }

    #[test]
    fn pasted_lines_become_lists_and_tables() {
        let style = EditStyle::default();
        assert_eq!(in_list("a\r\nb\nO'Neil\na\n", &style).unwrap(), "('a', 'b', 'O''Neil')");
        assert_eq!(in_list("1\n2.5\n-3\n", &style).unwrap(), "(1, 2.5, -3)");
        assert_eq!(in_list("007\n12\n", &style).unwrap(), "('007', '12')");
        assert_eq!(
            values_table("name\tqty\nrice\t2\nbeans\t\n", false, &style).unwrap(),
            "(values\n    ('rice', 2),\n    ('beans', null)\n) as t(name, qty)"
        );
        assert_eq!(
            values_table("a\tb\nc\td\n", true, &style).unwrap(),
            "(VALUES\n    ('a', 'b'),\n    ('c', 'd')\n) AS t(column1, column2)"
        );
    }

    #[test]
    fn statements_copy_as_one_line() {
        assert_eq!(
            one_line_string("select \"a\",\n  'x' -- note\nfrom t;"),
            r#""select \"a\", 'x' from t""#
        );
    }
}
