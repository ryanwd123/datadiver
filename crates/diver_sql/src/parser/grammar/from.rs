//! FROM items: tables, files, table functions, subqueries, joins, PIVOT /
//! UNPIVOT and samples.

use super::super::{CompletedMarker, Parser};
use super::expressions::{self, FirstArg, expr};
use super::names::{self, NameRule};
use super::select;
use crate::complete::{Expected, FunctionKind, NewNameKind};
use crate::dialect::{self, Feature};
use crate::{Kw, SyntaxKind as K};

pub(super) fn from_clause(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::From);
    from_list(p, &[]);
    m.complete(p, K::FROM_CLAUSE);
}

/// Comma-separated FROM items, as in FROM, USING and UPDATE ... FROM.
pub(super) fn from_list(p: &mut Parser, stop: &[Kw]) {
    loop {
        if from_item(p, stop).is_none() {
            p.err_recover("expected a table", stop);
            break;
        }
        if !p.eat_list_comma() {
            break;
        }
    }
}

/// A FROM item and the joins that follow it.
pub(super) fn from_item(p: &mut Parser, stop: &[Kw]) -> Option<CompletedMarker> {
    p.nested(|p| from_item_inner(p, stop))
}

fn from_item_inner(p: &mut Parser, stop: &[Kw]) -> Option<CompletedMarker> {
    let mut lhs = table_primary(p, stop)?;
    loop {
        let m = lhs.precede(p);
        let Some(needs_condition) = join_keywords(p) else {
            m.abandon(p);
            break;
        };
        if join_operand(p, stop, needs_condition).is_none() {
            p.error("expected a table");
        }
        if needs_condition {
            join_condition(p, stop);
        }
        lhs = m.complete(p, K::JOIN_EXPR);
    }
    Some(lhs)
}

/// The right side of a join. When the join wants a condition but another
/// join follows instead, that join nests: `a JOIN b JOIN c ON x ON y` is
/// `a JOIN (b JOIN c ON x) ON y`.
fn join_operand(p: &mut Parser, stop: &[Kw], needs_condition: bool) -> Option<CompletedMarker> {
    let mut rhs = table_primary(p, stop)?;
    if !needs_condition {
        return Some(rhs);
    }
    while !p.peek_kw(Kw::On) && !p.peek_kw(Kw::Using) {
        let inner = rhs.precede(p);
        let Some(inner_needs) = join_keywords(p) else {
            inner.abandon(p);
            break;
        };
        let operand = p.nested(|p| join_operand(p, stop, inner_needs));
        if operand.is_none() {
            p.error("expected a table");
        }
        if inner_needs {
            join_condition(p, stop);
        }
        rhs = inner.complete(p, K::JOIN_EXPR);
    }
    Some(rhs)
}

/// `ON cond` or `USING (cols)` after a join.
fn join_condition(p: &mut Parser, stop: &[Kw]) {
    if p.at_kw(Kw::On) {
        let c = p.start();
        p.bump_kw(Kw::On);
        if expr(p).is_none() {
            p.err_recover("expected a join condition", stop);
        }
        c.complete(p, K::JOIN_ON);
    } else if p.at_kw(Kw::Using) && p.nth(1) == K::L_PAREN {
        let c = p.start();
        p.bump_kw(Kw::Using);
        names::column_list(p, true);
        c.complete(p, K::JOIN_USING);
    }
}

/// `[NATURAL] [ASOF] [POSITIONAL] [CROSS | LEFT | ...] [OUTER] JOIN`; returns
/// whether the join takes an ON or USING condition.
fn join_keywords(p: &mut Parser) -> Option<bool> {
    const LEADING: &[Kw] = &[
        Kw::Natural,
        Kw::Asof,
        Kw::Positional,
        Kw::Cross,
        Kw::Left,
        Kw::Right,
        Kw::Full,
        Kw::Inner,
        Kw::Semi,
        Kw::Anti,
        Kw::Outer,
    ];
    if p.at_hole() {
        p.record(Expected::Keyword(Kw::Join));
        for &kw in LEADING {
            let allowed = match kw {
                Kw::Outer => false,
                Kw::Asof => p.allows(Feature::AsofJoin),
                Kw::Positional => p.allows(Feature::PositionalJoin),
                _ => true,
            };
            if allowed {
                p.record(Expected::Keyword(kw));
            }
        }
        return None;
    }
    let mut n = 0;
    while p.nth_kw(n).is_some_and(|kw| LEADING.contains(&kw)) {
        n += 1;
    }
    if !p.nth_at_kw(n, Kw::Join) {
        return None;
    }
    let mut needs = true;
    for _ in 0..=n {
        if matches!(p.nth_kw(0), Some(Kw::Cross | Kw::Natural | Kw::Positional)) {
            needs = false;
        }
        match p.nth_kw(0) {
            Some(Kw::Asof) => p.gate(Feature::AsofJoin),
            Some(Kw::Positional) => p.gate(Feature::PositionalJoin),
            _ => {}
        }
        p.bump_any_kw();
    }
    Some(needs)
}

fn table_primary(p: &mut Parser, stop: &[Kw]) -> Option<CompletedMarker> {
    let m = p.start();
    // `alias: table`
    if names::at_name(p, NameRule::ColId) && p.nth(1) == K::COLON {
        let a = p.start();
        names::new_name(p, NameRule::ColId, NewNameKind::Alias);
        a.complete(p, K::ALIAS);
        p.bump();
    }
    if p.peek_kw(Kw::Lateral) {
        p.gate(Feature::Lateral);
    }
    let lateral = p.eat_kw(Kw::Lateral);
    let kind = if p.at_hole() {
        p.record(Expected::FunctionName {
            kind: FunctionKind::Table,
            qualifier: Vec::new(),
        });
        names::table_path(p);
        if p.current() == K::L_PAREN {
            expressions::arg_list(p, FirstArg::Any);
            K::TABLE_FUNCTION
        } else {
            K::TABLE_REF
        }
    } else if p.current() == K::L_PAREN {
        if select::at_query_start(p, true) {
            p.bump();
            select::query(p);
            if !p.eat(K::R_PAREN) {
                p.err_recover("expected ')'", stop);
                p.eat(K::R_PAREN);
            }
            K::SUBQUERY_REF
        } else {
            p.bump();
            if from_item(p, stop).is_none() {
                p.error("expected a table");
            }
            p.expect(K::R_PAREN);
            K::PAREN_JOIN
        }
    } else if p.current().is_string() {
        p.record_string(Expected::FilePath);
        p.bump();
        K::FILE_REF
    } else if p.peek_kw(Kw::Values) {
        let q = p.start();
        select::query_rest(p, q);
        K::SUBQUERY_REF
    } else if names::at_name(p, NameRule::TypeFunc) || names::at_name(p, NameRule::ColId) {
        let first_arg = if dialect::takes_path(p.nth_text(0)) || last_part_takes_path(p) {
            FirstArg::Path
        } else {
            FirstArg::Any
        };
        let on_hole = |p: &mut Parser, q: Vec<crate::complete::Name>| {
            p.record(Expected::FunctionName {
                kind: FunctionKind::Table,
                qualifier: q.clone(),
            });
            names::record_table(p, q);
        };
        names::path(p, NameRule::Any, &on_hole);
        if p.current() == K::L_PAREN {
            expressions::arg_list(p, first_arg);
            if p.at_kw(Kw::With) && p.nth_at_kw(1, Kw::Ordinality) {
                p.bump_kw(Kw::With);
                p.bump_kw(Kw::Ordinality);
            }
            K::TABLE_FUNCTION
        } else {
            K::TABLE_REF
        }
    } else {
        m.abandon(p);
        if lateral {
            p.error("expected a subquery or table function");
        }
        return None;
    };
    names::table_alias(p, stop);
    let tablesample = if p.allows(Feature::Sample) {
        p.at_kw(Kw::Tablesample)
    } else {
        p.peek_kw(Kw::Tablesample)
    };
    if tablesample || (p.peek_kw(Kw::Using) && p.nth_at_kw(1, Kw::Sample)) {
        p.gate(Feature::Sample);
        let s = p.start();
        if !p.eat_kw(Kw::Tablesample) {
            p.bump_kw(Kw::Using);
            p.bump_kw(Kw::Sample);
        }
        sample_spec(p);
        s.complete(p, K::SAMPLE_CLAUSE);
    }
    let mut done = m.complete(p, kind);
    loop {
        let pivot = if p.allows(Feature::Pivot) {
            p.at_kw(Kw::Pivot)
        } else {
            p.peek_kw(Kw::Pivot)
        };
        let unpivot = if p.allows(Feature::Pivot) {
            p.at_kw(Kw::Unpivot)
        } else {
            p.peek_kw(Kw::Unpivot)
        };
        if pivot || unpivot {
            p.gate(Feature::Pivot);
        }
        if pivot {
            let m = done.precede(p);
            pivot_body(p);
            names::table_alias(p, stop);
            done = m.complete(p, K::PIVOT_REF);
        } else if unpivot {
            let m = done.precede(p);
            unpivot_body(p);
            names::table_alias(p, stop);
            done = m.complete(p, K::UNPIVOT_REF);
        } else {
            break;
        }
    }
    Some(done)
}

/// Whether the last part of a dotted name ahead takes a path (`main.read_csv(`).
fn last_part_takes_path(p: &Parser) -> bool {
    let mut n = 0;
    while p.nth(n + 1) == K::DOT && names::nth_is_word(p, n + 2) {
        n += 2;
    }
    n > 0 && dialect::takes_path(p.nth_text(n))
}

/// `10%`, `10 PERCENT`, `100 ROWS`, `reservoir(50 ROWS)`, then
/// `(method [, seed])` and `REPEATABLE (seed)`.
pub(super) fn sample_spec(p: &mut Parser) {
    fn size(p: &mut Parser) {
        if matches!(p.current(), K::INT_NUMBER | K::FLOAT_NUMBER | K::PARAM) {
            p.bump();
        } else {
            p.error("expected a sample size");
        }
        if !p.eat(K::PERCENT) && !p.eat_kw(Kw::Percent) && !p.eat_kw(Kw::Rows) {
            p.eat_kw(Kw::Row);
        }
    }
    if p.current() == K::L_PAREN {
        // USING SAMPLE (10000)
        p.bump();
        size(p);
        p.expect(K::R_PAREN);
    } else if names::at_name(p, NameRule::Any) && p.nth(1) == K::L_PAREN {
        p.bump_any_kw();
        p.bump();
        size(p);
        p.expect(K::R_PAREN);
    } else {
        size(p);
        if p.current() == K::L_PAREN {
            p.bump();
            p.bump_any_kw();
            if p.eat(K::COMMA) && p.current() == K::INT_NUMBER {
                p.bump();
            }
            p.expect(K::R_PAREN);
        }
    }
    if p.at_kw(Kw::Repeatable) {
        p.bump_kw(Kw::Repeatable);
        p.expect(K::L_PAREN);
        if p.current() == K::INT_NUMBER {
            p.bump();
        }
        p.expect(K::R_PAREN);
    }
}

/// `PIVOT (aggregates FOR col IN (values) ... [GROUP BY ...])`.
fn pivot_body(p: &mut Parser) {
    p.bump_kw(Kw::Pivot);
    p.expect(K::L_PAREN);
    pivot_aggregates(p);
    if p.at_kw(Kw::For) {
        p.bump_kw(Kw::For);
        // `FOR a IN (...) b IN (...)` pivots on several columns
        while !p.at(K::R_PAREN) && !p.at_eof() && !p.at_phrase(&[Kw::Group, Kw::By]) {
            let before = p.position();
            pivot_in(p);
            if p.position() == before {
                p.err_recover("expected a pivot column", &[Kw::Group]);
                if p.position() == before {
                    break;
                }
            }
        }
    }
    if p.at_phrase(&[Kw::Group, Kw::By]) {
        let m = p.start();
        p.bump_kw(Kw::Group);
        p.expect_kw(Kw::By);
        expressions::expr_list(p);
        m.complete(p, K::GROUP_BY_CLAUSE);
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
}

/// `UNPIVOT [INCLUDE NULLS] (value FOR name IN (columns))`.
fn unpivot_body(p: &mut Parser) {
    p.bump_kw(Kw::Unpivot);
    if p.at_kw(Kw::Include) || p.at_kw(Kw::Exclude) {
        p.bump_any_kw();
        p.expect_kw(Kw::Nulls);
    }
    p.expect(K::L_PAREN);
    if p.current() == K::L_PAREN {
        names::column_aliases(p);
    } else {
        names::new_name(p, NameRule::Any, NewNameKind::Column);
    }
    if p.at_kw(Kw::For) {
        pivot_in(p);
    } else {
        p.error("expected FOR");
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
}

pub(super) fn pivot_aggregates(p: &mut Parser) {
    loop {
        let m = p.start();
        if expr(p).is_none() {
            m.abandon(p);
            p.err_recover("expected an aggregate", &[Kw::For]);
            break;
        }
        select::select_alias(p);
        m.complete(p, K::SELECT_ITEM);
        if !p.eat_list_comma() {
            break;
        }
    }
}

/// `FOR col IN (a, b AS x, ...)` or `FOR (a, b) IN (...)`.
fn pivot_in(p: &mut Parser) {
    let m = p.start();
    p.eat_kw(Kw::For);
    if p.current() == K::L_PAREN {
        names::column_list(p, true);
    } else if expressions::expr_bp(p, 14).is_none() {
        p.error("expected a column");
    }
    p.expect_kw(Kw::In);
    if p.eat(K::L_PAREN) {
        while !p.at(K::R_PAREN) && !p.at_eof() {
            let item = p.start();
            if expr(p).is_none() {
                item.abandon(p);
                p.err_recover("expected a value", &[]);
                break;
            }
            select::select_alias(p);
            item.complete(p, K::SELECT_ITEM);
            if !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
    } else if names::path(p, NameRule::Any, &|_, _| {}).is_none() {
        p.error("expected a value list");
    }
    m.complete(p, K::PIVOT_IN);
}
