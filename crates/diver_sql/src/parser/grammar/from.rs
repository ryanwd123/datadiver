//! FROM items: tables, files, table functions, subqueries, joins, PIVOT /
//! UNPIVOT and samples.

use super::super::{CompletedMarker, Parser};
use super::expressions::{self, expr};
use super::names::{self, NameRule};
use super::select;
use crate::complete::{Expected, FunctionKind, NewNameKind};
use crate::{Kw, SyntaxKind as K, dialect};

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
        if !p.eat(K::COMMA) {
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
        if !join_keywords(p) {
            m.abandon(p);
            break;
        }
        if table_primary(p, stop).is_none() {
            p.error("expected a table");
        }
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
        lhs = m.complete(p, K::JOIN_EXPR);
    }
    Some(lhs)
}

/// `[NATURAL] [ASOF] [POSITIONAL] [CROSS | LEFT | ...] [OUTER] JOIN`.
fn join_keywords(p: &mut Parser) -> bool {
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
            if kw != Kw::Outer {
                p.record(Expected::Keyword(kw));
            }
        }
        return false;
    }
    let mut n = 0;
    while p.nth_kw(n).is_some_and(|kw| LEADING.contains(&kw)) {
        n += 1;
    }
    if !p.nth_at_kw(n, Kw::Join) {
        return false;
    }
    for _ in 0..=n {
        p.bump_any_kw();
    }
    true
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
    let lateral = p.eat_kw(Kw::Lateral);
    let kind = if p.at_hole() {
        p.record(Expected::FunctionName {
            kind: FunctionKind::Table,
            qualifier: Vec::new(),
        });
        names::table_path(p);
        if p.current() == K::L_PAREN {
            expressions::arg_list(p, false);
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
        let path_fn = dialect::takes_path(p.nth_text(0)) || last_part_takes_path(p);
        let on_hole = |p: &mut Parser, q: Vec<crate::complete::Name>| {
            p.record(Expected::FunctionName {
                kind: FunctionKind::Table,
                qualifier: q.clone(),
            });
            names::record_table(p, q);
        };
        names::path(p, NameRule::Any, &on_hole);
        if p.current() == K::L_PAREN {
            expressions::arg_list(p, path_fn);
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
    if p.at_kw(Kw::Tablesample) || (p.peek_kw(Kw::Using) && p.nth_at_kw(1, Kw::Sample)) {
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
        if p.at_kw(Kw::Pivot) {
            let m = done.precede(p);
            pivot_body(p);
            names::table_alias(p, stop);
            done = m.complete(p, K::PIVOT_REF);
        } else if p.at_kw(Kw::Unpivot) {
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
    if names::at_name(p, NameRule::Any) && p.nth(1) == K::L_PAREN {
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
    while p.at_kw(Kw::For) {
        pivot_in(p);
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
        if !p.eat(K::COMMA) {
            break;
        }
    }
}

/// `FOR col IN (a, b AS x, ...)` or `FOR (a, b) IN (...)`.
fn pivot_in(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::For);
    if p.current() == K::L_PAREN {
        names::column_list(p, true);
    } else if expressions::expr_bp(p, 14).is_none() {
        p.error("expected a column");
    }
    p.expect_kw(Kw::In);
    if p.expect(K::L_PAREN) {
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
