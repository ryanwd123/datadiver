//! Type names: `INTEGER`, `DECIMAL(10, 2)`, `VARCHAR[]`, `STRUCT(a INT)`,
//! `MAP(VARCHAR, INT)`, `TIMESTAMP WITH TIME ZONE`, ...

use super::super::{CompletedMarker, Parser};
use super::expressions;
use super::names::{self, NameRule};
use crate::complete::{Expected, NewNameKind};
use crate::{Kw, SyntaxKind as K};

/// Types whose name can prefix a string to make a typed literal (`DATE '2024-01-01'`).
pub(super) fn is_literal_type(word: &str) -> bool {
    const TYPES: &[&str] = &[
        "date",
        "time",
        "timestamp",
        "timestamptz",
        "timetz",
        "datetime",
        "json",
        "uuid",
        "varchar",
        "text",
        "string",
        "blob",
        "bytea",
        "integer",
        "int",
        "bigint",
        "smallint",
        "tinyint",
        "hugeint",
        "double",
        "float",
        "real",
        "decimal",
        "numeric",
        "boolean",
        "bool",
        "bit",
        "timestamp_s",
        "timestamp_ms",
        "timestamp_ns",
        "time_ns", "bitstring", "varint", "bignum", "uhugeint", "ubigint", "uinteger",
        "usmallint", "utinyint", "int1", "int2", "int4", "int8", "float4", "float8", "char", "bpchar",
        "timestamp_us",
    ];
    TYPES.iter().any(|t| t.eq_ignore_ascii_case(word))
}

pub(super) fn type_name(p: &mut Parser) -> Option<CompletedMarker> {
    p.nested(type_name_inner)
}

fn type_name_inner(p: &mut Parser) -> Option<CompletedMarker> {
    let m = p.start();
    if p.at_hole() {
        p.record(Expected::TypeName);
        names::bump_name(p);
    } else {
        let kw = p.nth_kw(0);
        let composite =
            matches!(kw, Some(Kw::Struct | Kw::Row | Kw::Union)) && p.nth(1) == K::L_PAREN;
        if !composite && !names::word_ok(p, 0, NameRule::NotReserved) {
            p.error("expected a type");
            m.abandon(p);
            return None;
        }
        match kw {
            Some(Kw::Struct | Kw::Row | Kw::Union) if p.nth(1) == K::L_PAREN => {
                p.bump_any_kw();
                struct_fields(p);
            }
            Some(Kw::Double) => {
                p.bump_kw(Kw::Double);
                p.eat_kw(Kw::Precision);
            }
            Some(Kw::Character | Kw::Char | Kw::Bit | Kw::National | Kw::Nchar) => {
                p.bump_any_kw();
                p.eat_kw(Kw::Character);
                p.eat_kw(Kw::Varying);
                type_args(p);
            }
            Some(Kw::Timestamp | Kw::Time) => {
                p.bump_any_kw();
                type_args(p);
                if (p.peek_kw(Kw::With) || p.peek_kw(Kw::Without)) && p.nth_at_kw(1, Kw::Time) {
                    p.bump_any_kw();
                    p.bump_kw(Kw::Time);
                    p.expect_kw(Kw::Zone);
                }
            }
            Some(Kw::Interval) => {
                p.bump_kw(Kw::Interval);
                // INTERVAL DAY TO SECOND
                if p.current() == K::IDENT && !p.nth_kw(0).is_some_and(|kw| kw.is_reserved()) && p.nth_at_kw(1, Kw::To) {
                    p.bump_any_kw();
                    p.bump_kw(Kw::To);
                    p.bump_any_kw();
                }
            }
            _ => {
                if names::path(p, NameRule::NotReserved, &|p, _| {
                    p.record(Expected::TypeName)
                })
                .is_none()
                {
                    p.error("expected a type");
                }
                type_args(p);
            }
        }
    }
    // `INT[]`, `INT[3]`, `INT[][]`, `INT ARRAY`
    loop {
        if p.current() == K::L_BRACK {
            p.bump();
            if p.current() == K::INT_NUMBER {
                p.bump();
            }
            p.expect(K::R_BRACK);
        } else if p.peek_kw(Kw::Array) {
            p.bump_kw(Kw::Array);
        } else {
            break;
        }
    }
    Some(m.complete(p, K::TYPE_NAME))
}

/// `(10, 2)`, `(VARCHAR, INT)` or `('a', 'b')` after a type name.
fn type_args(p: &mut Parser) {
    if p.current() != K::L_PAREN {
        return;
    }
    let m = p.start();
    p.bump();
    while !p.at(K::R_PAREN) && !p.at_eof() {
        let k = p.current();
        if k.is_string() || matches!(k, K::INT_NUMBER | K::FLOAT_NUMBER | K::MINUS) {
            expressions::expr(p);
        } else if type_name(p).is_none() {
            p.err_recover("expected a type argument", &[]);
            break;
        }
        if !p.eat(K::COMMA) {
            break;
        }
    }
    p.expect(K::R_PAREN);
    m.complete(p, K::TYPE_ARGS);
}

/// `(name type, ...)` of STRUCT, ROW and UNION.
fn struct_fields(p: &mut Parser) {
    let m = p.start();
    p.bump();
    while !p.at(K::R_PAREN) && !p.at_eof() {
        let f = p.start();
        if !names::new_name(p, NameRule::Any, NewNameKind::Column) {
            f.abandon(p);
            p.err_recover("expected a field name", &[]);
            break;
        }
        type_name(p);
        f.complete(p, K::STRUCT_TYPE_FIELD);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    p.expect(K::R_PAREN);
    m.complete(p, K::TYPE_ARGS);
}
