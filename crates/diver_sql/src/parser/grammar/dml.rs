//! INSERT, UPDATE, DELETE, MERGE and TRUNCATE.

use super::super::{CompletedMarker, Marker, Parser};
use super::expressions::{self, expr};
use super::names::{self, NameRule};
use super::{from, select};
use crate::complete::{Expected, NewNameKind};
use crate::{Kw, SyntaxKind as K};

/// The statement node: around the WITH clause when there is one.
fn start(p: &mut Parser, with: Option<CompletedMarker>) -> Marker {
    match with {
        Some(with) => with.precede(p),
        None => p.start(),
    }
}

/// The target table and its alias, as a `TABLE_REF`.
fn target(p: &mut Parser, stop: &[Kw]) {
    let m = p.start();
    if names::table_path(p).is_none() {
        p.error("expected a table");
    }
    names::table_alias(p, stop);
    m.complete(p, K::TABLE_REF);
}

pub(super) fn insert(p: &mut Parser, with: Option<CompletedMarker>) {
    let m = start(p, with);
    p.bump_kw(Kw::Insert);
    if p.at_kw(Kw::Or) {
        p.bump_kw(Kw::Or);
        if !p.eat_kw(Kw::Replace) {
            p.expect_kw(Kw::Ignore);
        }
    }
    p.expect_kw(Kw::Into);
    target(p, &[Kw::By, Kw::Default, Kw::Overriding]);
    if p.current() == K::L_PAREN && !select::at_query_start(p, true) {
        let c = p.start();
        p.bump();
        loop {
            if names::path(p, NameRule::Any, &|p, q| {
                p.record(Expected::ColumnRef { qualifier: q })
            })
            .is_none()
            {
                p.error("expected a column");
                break;
            }
            if !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
        c.complete(p, K::INSERT_COLUMNS);
    }
    if p.at_kw(Kw::By) {
        p.bump_kw(Kw::By);
        if !p.eat_kw(Kw::Name) {
            p.expect_kw(Kw::Position);
        }
    } else {
        p.record(Expected::Phrase(&[Kw::By, Kw::Name]));
        p.record(Expected::Phrase(&[Kw::By, Kw::Position]));
    }
    if p.at_phrase(&[Kw::Default, Kw::Values]) {
        p.bump_kw(Kw::Default);
        p.expect_kw(Kw::Values);
    } else if select::at_query_start(p, true) {
        select::query(p);
    } else {
        p.err_recover("expected VALUES or a query", &[Kw::On]);
    }
    if p.at_kw(Kw::On) && p.nth_at_kw(1, Kw::Conflict) {
        on_conflict(p);
    }
    returning(p);
    m.complete(p, K::INSERT_STMT);
}

fn on_conflict(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::On);
    p.bump_kw(Kw::Conflict);
    if p.current() == K::L_PAREN {
        names::column_list(p, true);
    }
    if p.eat_kw(Kw::On) {
        p.expect_kw(Kw::Constraint);
        names::new_name(p, NameRule::Any, NewNameKind::Other);
    }
    where_clause(p);
    p.expect_kw(Kw::Do);
    if !p.eat_kw(Kw::Nothing) && p.expect_kw(Kw::Update) {
        set_clause(p);
        where_clause(p);
    }
    m.complete(p, K::ON_CONFLICT_CLAUSE);
}

fn where_clause(p: &mut Parser) {
    if p.at_kw(Kw::Where) {
        let m = p.start();
        p.bump_kw(Kw::Where);
        if expr(p).is_none() {
            p.err_recover("expected a condition", &[]);
        }
        m.complete(p, K::WHERE_CLAUSE);
    }
}

fn returning(p: &mut Parser) {
    if p.at_kw(Kw::Returning) {
        let m = p.start();
        p.bump_kw(Kw::Returning);
        loop {
            let item = p.start();
            if expr(p).is_none() {
                item.abandon(p);
                p.err_recover("expected an expression", &[]);
                break;
            }
            select::select_alias(p);
            item.complete(p, K::SELECT_ITEM);
            if !p.eat(K::COMMA) {
                break;
            }
        }
        m.complete(p, K::RETURNING_CLAUSE);
    }
}

/// `SET col = expr, (a, b) = (1, 2), ...`.
fn set_clause(p: &mut Parser) {
    let m = p.start();
    p.expect_kw(Kw::Set);
    // MERGE ... UPDATE SET *
    if p.eat(K::STAR) {
        m.complete(p, K::SET_CLAUSE);
        return;
    }
    loop {
        let item = p.start();
        if p.current() == K::L_PAREN {
            names::column_list(p, true);
        } else if names::path(p, NameRule::Any, &|p, q| {
            p.record(Expected::ColumnRef { qualifier: q })
        })
        .is_none()
        {
            item.abandon(p);
            p.err_recover("expected a column", &[]);
            break;
        }
        if p.expect(K::EQ) && expr(p).is_none() {
            p.error("expected an expression");
        }
        item.complete(p, K::SET_ITEM);
        if !p.eat_list_comma() {
            break;
        }
    }
    m.complete(p, K::SET_CLAUSE);
}

pub(super) fn update(p: &mut Parser, with: Option<CompletedMarker>) {
    let m = start(p, with);
    p.bump_kw(Kw::Update);
    target(p, &[Kw::Set]);
    set_clause(p);
    if p.at_kw(Kw::From) {
        from::from_clause(p);
    }
    where_clause(p);
    returning(p);
    m.complete(p, K::UPDATE_STMT);
}

pub(super) fn delete(p: &mut Parser, with: Option<CompletedMarker>) {
    let m = start(p, with);
    p.bump_kw(Kw::Delete);
    p.expect_kw(Kw::From);
    target(p, &[]);
    if p.at_kw(Kw::Using) {
        let u = p.start();
        p.bump_kw(Kw::Using);
        from::from_list(p, &[]);
        u.complete(p, K::USING_CLAUSE);
    }
    where_clause(p);
    returning(p);
    m.complete(p, K::DELETE_STMT);
}

pub(super) fn merge(p: &mut Parser, with: Option<CompletedMarker>) {
    let m = start(p, with);
    p.bump_kw(Kw::Merge);
    p.expect_kw(Kw::Into);
    target(p, &[]);
    if p.expect_kw(Kw::Using) {
        let u = p.start();
        if from::from_item(p, &[Kw::On]).is_none() {
            p.error("expected a table");
        }
        u.complete(p, K::USING_CLAUSE);
    }
    if p.at_kw(Kw::On) {
        let c = p.start();
        p.bump_kw(Kw::On);
        if expr(p).is_none() {
            p.err_recover("expected a condition", &[Kw::When]);
        }
        c.complete(p, K::JOIN_ON);
    } else if p.at_kw(Kw::Using) {
        let c = p.start();
        p.bump_kw(Kw::Using);
        names::column_list(p, true);
        c.complete(p, K::JOIN_USING);
    }
    while p.at_kw(Kw::When) {
        merge_when(p);
    }
    returning(p);
    m.complete(p, K::MERGE_STMT);
}

fn merge_when(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::When);
    p.eat_kw(Kw::Not);
    p.expect_kw(Kw::Matched);
    if p.eat_kw(Kw::By) && !p.eat_kw(Kw::Source) {
        p.expect_kw(Kw::Target);
    }
    if p.eat_kw(Kw::And) && expr(p).is_none() {
        p.error("expected a condition");
    }
    p.expect_kw(Kw::Then);
    if p.at_kw(Kw::Update) {
        p.bump_kw(Kw::Update);
        by_name_or_position(p);
        if p.at_kw(Kw::Set) {
            set_clause(p);
        }
    } else if p.at_kw(Kw::Insert) {
        p.bump_kw(Kw::Insert);
        by_name_or_position(p);
        if p.current() == K::L_PAREN {
            names::column_list(p, true);
        }
        if p.eat(K::STAR) {
        } else if p.eat_kw(Kw::Default) {
            p.expect_kw(Kw::Values);
        } else if p.eat_kw(Kw::Values) {
            p.expect(K::L_PAREN);
            expressions::expr_list(p);
            p.expect(K::R_PAREN);
        }
    } else if p.eat_kw(Kw::Delete) {
    } else if p.eat_kw(Kw::Do) {
        p.expect_kw(Kw::Nothing);
    } else if p.at_kw(Kw::Error) {
        p.bump_kw(Kw::Error);
        expr(p);
    } else {
        p.err_recover("expected UPDATE, INSERT, DELETE or DO NOTHING", &[Kw::When]);
    }
    m.complete(p, K::MERGE_WHEN);
}

fn by_name_or_position(p: &mut Parser) {
    if p.at_kw(Kw::By) {
        p.bump_kw(Kw::By);
        if !p.eat_kw(Kw::Name) {
            p.expect_kw(Kw::Position);
        }
    }
}

pub(super) fn truncate(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Truncate);
    p.eat_kw(Kw::Table);
    target(p, &[]);
    m.complete(p, K::TRUNCATE_STMT);
}
