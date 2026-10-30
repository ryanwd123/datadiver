//! Queries: WITH, query blocks (SELECT-first and FROM-first), set
//! operations, VALUES, ORDER BY and LIMIT.

use super::super::{CompletedMarker, Parser};
use super::expressions::{self, expr};
use super::names::{self, NameRule};
use super::{from, misc, statements};
use crate::complete::{Expected, NewNameKind};
use crate::dialect::Feature;
use crate::{Kw, SyntaxKind as K};

/// Words a query (or a statement DuckDB takes as one) starts with.
const QUERY_START: &[Kw] = &[
    Kw::Select,
    Kw::From,
    Kw::With,
    Kw::Values,
    Kw::Table,
    Kw::Pivot,
    Kw::PivotWider,
    Kw::Unpivot,
    Kw::PivotLonger,
    Kw::Summarize,
    Kw::Describe,
    Kw::Show,
];

/// Whether a query starts here. With `inside_paren`, the current token is a
/// `(` and the question is whether it holds a query (`FROM (SELECT ...)`,
/// `INSERT INTO t (SELECT ...)`) rather than a join or a column list.
pub(super) fn at_query_start(p: &mut Parser, inside_paren: bool) -> bool {
    if p.at_hole() {
        for kw in [
            Kw::Select,
            Kw::From,
            Kw::With,
            Kw::Values,
            Kw::Table,
            Kw::Pivot,
            Kw::Unpivot,
        ] {
            p.record(Expected::Keyword(kw));
        }
        return false;
    }
    if inside_paren && p.current() == K::L_PAREN {
        query_inside(p, 1)
    } else {
        query_inside(p, 0)
    }
}

/// Whether the n-th token starts a query, possibly a parenthesized one.
pub(super) fn query_inside(p: &Parser, n: usize) -> bool {
    if p.raw_nth_kw(n).is_some_and(|kw| QUERY_START.contains(&kw)) {
        return true;
    }
    p.raw_nth(n) == K::L_PAREN && paren_query(p, n)
}

/// Whether the `(` at n holds a query and is followed by what a query can
/// continue with: `((SELECT 1) UNION ...)` is a query, `((VALUES (1)) t JOIN ...)`
/// a join and `((SELECT 1) + 1)` an expression.
fn paren_query(p: &Parser, n: usize) -> bool {
    if !query_inside(p, n + 1) {
        return false;
    }
    let mut depth = 0usize;
    let mut i = n;
    loop {
        match p.raw_nth(i) {
            K::EOF => return true,
            K::L_PAREN | K::L_BRACK | K::L_CURLY => depth += 1,
            K::R_PAREN | K::R_BRACK | K::R_CURLY => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    matches!(p.raw_nth(i + 1), K::R_PAREN | K::EOF)
        || matches!(
            p.raw_nth_kw(i + 1),
            Some(Kw::Union | Kw::Except | Kw::Intersect | Kw::Order | Kw::Limit | Kw::Offset)
        )
}

/// `[WITH ...] body [ORDER BY ...] [LIMIT ...] [OFFSET ...]`.
pub(super) fn query(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.nested(|p| {
        if p.at_kw(Kw::With) {
            with_clause(p);
        }
        body_and_tail(p);
        Some(())
    });
    m.complete(p, K::QUERY)
}

/// The query after its WITH clause, which the caller already parsed into `m`.
pub(super) fn query_rest(p: &mut Parser, m: super::super::Marker) -> CompletedMarker {
    p.nested(|p| {
        body_and_tail(p);
        Some(())
    });
    m.complete(p, K::QUERY)
}

fn body_and_tail(p: &mut Parser) {
    if query_body(p, 0).is_none() {
        p.err_recover("expected a query", &[]);
    }
    query_tail(p);
}

/// ORDER BY, LIMIT, OFFSET and FETCH after a query body.
fn query_tail(p: &mut Parser) {
    p.record(Expected::Phrase(&[Kw::Order, Kw::By]));
    p.record(Expected::Phrase(&[Kw::Order, Kw::By, Kw::All]));
    if p.peek_kw(Kw::Order) {
        order_by(p);
    }
    for _ in 0..2 {
        if p.at_kw(Kw::Limit) {
            let m = p.start();
            p.bump_kw(Kw::Limit);
            if !p.eat_kw(Kw::All) && expr(p).is_none() {
                p.error("expected a limit");
            }
            if !p.eat(K::PERCENT) {
                p.eat_kw(Kw::Percent);
            }
            m.complete(p, K::LIMIT_CLAUSE);
        } else if p.at_kw(Kw::Offset) {
            let m = p.start();
            p.bump_kw(Kw::Offset);
            if expr(p).is_none() {
                p.error("expected an offset");
            }
            if !p.eat_kw(Kw::Rows) {
                p.eat_kw(Kw::Row);
            }
            m.complete(p, K::OFFSET_CLAUSE);
        } else if p.at_kw(Kw::Fetch) {
            let m = p.start();
            p.bump_kw(Kw::Fetch);
            if !p.eat_kw(Kw::First) {
                p.expect_kw(Kw::Next);
            }
            if !p.peek_kw(Kw::Row) && !p.peek_kw(Kw::Rows) {
                expr(p);
            }
            if !p.eat_kw(Kw::Rows) {
                p.expect_kw(Kw::Row);
            }
            if !p.eat_kw(Kw::Only) {
                p.eat_kw(Kw::With);
                p.eat_kw(Kw::Ties);
            }
            m.complete(p, K::LIMIT_CLAUSE);
        }
    }
}

/// Set operations: INTERSECT binds tighter than UNION and EXCEPT.
fn query_body(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = query_term(p)?;
    loop {
        let bp = if p.at_kw(Kw::Union) || p.at_kw(Kw::Except) {
            1
        } else if p.at_kw(Kw::Intersect) {
            2
        } else {
            break;
        };
        if bp < min_bp {
            break;
        }
        let m = lhs.precede(p);
        p.bump_any_kw();
        if !p.eat_kw(Kw::All) {
            p.eat_kw(Kw::Distinct);
        }
        if p.at_phrase(&[Kw::By, Kw::Name]) {
            p.bump_kw(Kw::By);
            p.expect_kw(Kw::Name);
        }
        if query_body(p, bp + 1).is_none() {
            p.error("expected a query");
        }
        lhs = m.complete(p, K::SET_OP);
    }
    Some(lhs)
}

fn query_term(p: &mut Parser) -> Option<CompletedMarker> {
    if p.at_hole() {
        for kw in [Kw::Select, Kw::From, Kw::Values, Kw::With, Kw::Table] {
            p.record(Expected::Keyword(kw));
        }
        return None;
    }
    if p.at(K::L_PAREN) {
        let m = p.start();
        p.bump();
        query(p);
        p.expect(K::R_PAREN);
        return Some(m.complete(p, K::PAREN_QUERY));
    }
    match p.nth_kw(0) {
        Some(Kw::Select | Kw::From) => Some(select_core(p)),
        Some(Kw::Values) => Some(values(p)),
        Some(Kw::Table) => {
            let m = p.start();
            p.bump_kw(Kw::Table);
            let from = p.start();
            let t = p.start();
            if names::table_path(p).is_none() {
                p.error("expected a table");
            }
            t.complete(p, K::TABLE_REF);
            from.complete(p, K::FROM_CLAUSE);
            Some(m.complete(p, K::SELECT_CORE))
        }
        Some(Kw::Pivot | Kw::PivotWider | Kw::Unpivot | Kw::PivotLonger) => {
            p.gate(Feature::Pivot);
            Some(misc::pivot_stmt(p))
        }
        Some(Kw::Summarize | Kw::Describe | Kw::Show) => {
            // Statements DuckDB lets stand as a query (`FROM (DESCRIBE t)`).
            let m = p.start();
            statements::statement(p);
            Some(m.complete(p, K::PAREN_QUERY))
        }
        _ => None,
    }
}

/// A query block: `SELECT ... [FROM ...]` or `FROM ... [SELECT ...]`, then
/// WHERE, GROUP BY, HAVING, WINDOW, QUALIFY and USING SAMPLE.
fn select_core(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    if p.at_kw(Kw::From) {
        from::from_clause(p);
        if p.at_kw(Kw::Select) {
            select_clause(p);
        }
    } else {
        select_clause(p);
        if p.at_kw(Kw::From) {
            from::from_clause(p);
        }
    }
    if p.at_kw(Kw::Where) {
        let c = p.start();
        p.bump_kw(Kw::Where);
        condition(p);
        c.complete(p, K::WHERE_CLAUSE);
    }
    p.record(Expected::Phrase(&[Kw::Group, Kw::By]));
    p.record(Expected::Phrase(&[Kw::Group, Kw::By, Kw::All]));
    if p.peek_kw(Kw::Group) {
        group_by(p);
    }
    if p.at_kw(Kw::Having) {
        let c = p.start();
        p.bump_kw(Kw::Having);
        condition(p);
        c.complete(p, K::HAVING_CLAUSE);
    }
    if p.at_kw(Kw::Window) {
        window_clause(p);
    }
    if p.at_kw(Kw::Qualify) {
        let c = p.start();
        p.bump_kw(Kw::Qualify);
        condition(p);
        c.complete(p, K::QUALIFY_CLAUSE);
    }
    let sample = if p.allows(Feature::Sample) {
        p.at_phrase(&[Kw::Using, Kw::Sample])
    } else {
        p.peek_kw(Kw::Using)
    };
    if sample && p.nth_at_kw(1, Kw::Sample) {
        p.gate(Feature::Sample);
        let c = p.start();
        p.bump_kw(Kw::Using);
        p.bump_kw(Kw::Sample);
        from::sample_spec(p);
        c.complete(p, K::SAMPLE_CLAUSE);
    }
    m.complete(p, K::SELECT_CORE)
}

fn condition(p: &mut Parser) {
    if expr(p).is_none() {
        p.err_recover("expected a condition", &[]);
    }
}

fn select_clause(p: &mut Parser) {
    let m = p.start();
    p.expect_kw(Kw::Select);
    if p.at_kw(Kw::Distinct) {
        let d = p.start();
        p.bump_kw(Kw::Distinct);
        if p.at_kw(Kw::On) {
            p.bump_kw(Kw::On);
            if p.expect(K::L_PAREN) {
                expressions::expr_list(p);
                p.expect(K::R_PAREN);
            }
        }
        d.complete(p, K::DISTINCT_CLAUSE);
    } else if p.at_kw(Kw::All) {
        let d = p.start();
        p.bump_kw(Kw::All);
        d.complete(p, K::DISTINCT_CLAUSE);
    }
    if !at_select_list_end(p) {
        loop {
            select_item(p);
            if !p.eat(K::COMMA) {
                break;
            }
            // DuckDB allows a trailing comma
            if at_select_list_end(p) {
                break;
            }
        }
    }
    m.complete(p, K::SELECT_CLAUSE);
}

fn at_select_list_end(p: &Parser) -> bool {
    if p.at_hole() {
        return false;
    }
    p.at_recovery(&[]) || p.current() == K::SEMICOLON || p.peek_kw(Kw::Into)
}

fn select_item(p: &mut Parser) {
    let m = p.start();
    // `alias: expr`
    if names::at_name(p, NameRule::ColId) && p.nth(1) == K::COLON {
        p.gate(Feature::PrefixAlias);
        let a = p.start();
        names::new_name(p, NameRule::ColId, NewNameKind::Alias);
        a.complete(p, K::ALIAS);
        p.bump();
        if expr(p).is_none() {
            p.error("expected an expression");
        }
    } else {
        if expr(p).is_none() {
            m.abandon(p);
            p.err_recover("expected an expression", &[]);
            return;
        }
        select_alias(p);
    }
    m.complete(p, K::SELECT_ITEM);
}

/// `[AS] label` after a select item or expression.
pub(super) fn select_alias(p: &mut Parser) {
    if p.at_kw(Kw::As) {
        let a = p.start();
        p.bump_kw(Kw::As);
        if p.current().is_string() {
            p.bump();
        } else {
            names::new_name(p, NameRule::Any, NewNameKind::Alias);
        }
        a.complete(p, K::ALIAS);
    } else if p.at_hole() {
        p.record(Expected::NewName(NewNameKind::Alias));
    } else if names::at_name(p, NameRule::BareLabel) {
        let a = p.start();
        names::new_name(p, NameRule::BareLabel, NewNameKind::Alias);
        a.complete(p, K::ALIAS);
    }
}

pub(super) fn with_clause(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_kw(Kw::With);
    p.eat_kw(Kw::Recursive);
    loop {
        cte(p);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    m.complete(p, K::WITH_CLAUSE)
}

fn cte(p: &mut Parser) {
    let m = p.start();
    if !names::new_name(p, NameRule::ColId, NewNameKind::Cte) {
        m.abandon(p);
        p.err_recover("expected a CTE name", &[Kw::As]);
        return;
    }
    if p.current() == K::L_PAREN {
        names::column_aliases(p);
    }
    if p.at_phrase(&[Kw::Using, Kw::Key]) {
        p.bump_kw(Kw::Using);
        p.expect_kw(Kw::Key);
        names::column_list(p, true);
    }
    p.expect_kw(Kw::As);
    if p.at_kw(Kw::Materialized) || (p.peek_kw(Kw::Not) && p.nth_at_kw(1, Kw::Materialized)) {
        let c = p.start();
        p.eat_kw(Kw::Not);
        p.bump_kw(Kw::Materialized);
        c.complete(p, K::MATERIALIZED_CLAUSE);
    } else {
        p.record(Expected::Phrase(&[Kw::Not, Kw::Materialized]));
    }
    if p.expect(K::L_PAREN) {
        query_or_statement(p);
        if !p.eat(K::R_PAREN) {
            p.err_recover("expected ')'", &[]);
            p.eat(K::R_PAREN);
        }
    }
    m.complete(p, K::CTE);
}

/// A query, or a statement DuckDB accepts where a query goes (a CTE body,
/// `FROM (...)`): PIVOT, DESCRIBE, SHOW, SUMMARIZE.
fn query_or_statement(p: &mut Parser) {
    query(p);
}

fn values(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_kw(Kw::Values);
    loop {
        let row = p.start();
        if !p.expect(K::L_PAREN) {
            row.abandon(p);
            break;
        }
        // DuckDB allows trailing commas in a row and after the last row
        while !p.at(K::R_PAREN) && !p.at_eof() {
            if expr(p).is_none() {
                p.err_recover("expected an expression", &[]);
                break;
            }
            if !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
        row.complete(p, K::VALUES_ROW);
        if !p.eat(K::COMMA) || !p.at(K::L_PAREN) {
            break;
        }
    }
    m.complete(p, K::VALUES_CLAUSE)
}

fn group_by(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Group);
    p.expect_kw(Kw::By);
    if p.at_kw(Kw::All) {
        p.bump_kw(Kw::All);
    } else {
        loop {
            grouping_element(p);
            if !p.eat_list_comma() {
                break;
            }
        }
    }
    m.complete(p, K::GROUP_BY_CLAUSE);
}

fn grouping_element(p: &mut Parser) {
    let is_sets = p.at_kw(Kw::Grouping) && p.nth_at_kw(1, Kw::Sets);
    let is_cube = (p.at_kw(Kw::Cube) || p.at_kw(Kw::Rollup)) && p.nth(1) == K::L_PAREN;
    if is_sets || is_cube {
        p.gate(Feature::GroupingSets);
        let m = p.start();
        p.bump_any_kw();
        if is_sets {
            p.bump_kw(Kw::Sets);
        }
        p.expect(K::L_PAREN);
        while !p.at(K::R_PAREN) && !p.at_eof() {
            grouping_element(p);
            if !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
        m.complete(p, K::GROUPING_SET);
    } else if p.current() == K::L_PAREN && p.nth(1) == K::R_PAREN {
        let m = p.start();
        p.bump();
        p.bump();
        m.complete(p, K::GROUPING_SET);
    } else if expr(p).is_none() {
        p.err_recover("expected an expression", &[]);
    }
}

fn window_clause(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Window);
    loop {
        let d = p.start();
        if !names::new_name(p, NameRule::ColId, NewNameKind::Window) {
            d.abandon(p);
            break;
        }
        p.expect_kw(Kw::As);
        if p.at(K::L_PAREN) {
            expressions::window_spec(p);
        } else {
            p.error("expected '('");
        }
        d.complete(p, K::WINDOW_DEF);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    m.complete(p, K::WINDOW_CLAUSE);
}

/// `ORDER BY item, ...` or `ORDER BY ALL`.
pub(super) fn order_by(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Order);
    p.expect_kw(Kw::By);
    loop {
        let item = p.start();
        if !p.eat_kw(Kw::All) && expr(p).is_none() {
            item.abandon(p);
            p.err_recover("expected an expression", &[]);
            break;
        }
        if !p.eat_kw(Kw::Asc) {
            p.eat_kw(Kw::Desc);
        }
        if p.at_kw(Kw::Nulls) {
            p.bump_kw(Kw::Nulls);
            if !p.eat_kw(Kw::First) {
                p.expect_kw(Kw::Last);
            }
        }
        item.complete(p, K::ORDER_ITEM);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    m.complete(p, K::ORDER_BY_CLAUSE);
}
