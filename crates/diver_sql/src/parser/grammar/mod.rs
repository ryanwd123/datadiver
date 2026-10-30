//! The grammar, one module per area. Every rule takes the parser and leaves
//! a node (or an error) behind.

mod ddl;
mod dml;
mod expressions;
mod from;
mod misc;
mod names;
mod select;
mod statements;
mod types;

use super::Parser;
use crate::SyntaxKind as K;

pub(super) fn source_file(p: &mut Parser) {
    let m = p.start();
    loop {
        while p.current() == K::SEMICOLON {
            p.bump();
        }
        if p.at_eof() {
            break;
        }
        let semi = p.next_semicolon();
        p.set_limit(semi);
        p.reset_fuel();
        statements::statement(p);
        if p.is_stuck() {
            p.error("the parser got stuck here");
            p.bump_rest_as_error();
        } else if !p.at_eof() {
            p.error("expected the end of the statement");
            p.bump_rest_as_error();
        }
        p.set_limit(None);
        if semi.is_none() {
            break;
        }
    }
    m.complete(p, K::SOURCE_FILE);
}

/// One expression with an optional alias, as the expression editors use.
pub(super) fn expression_root(p: &mut Parser) {
    let m = p.start();
    if expressions::expr(p).is_none() {
        p.error("expected an expression");
    }
    select::select_alias(p);
    if !p.at_eof() {
        p.error("expected the end of the expression");
        p.bump_rest_as_error();
    }
    m.complete(p, K::EXPRESSION_ROOT);
}
