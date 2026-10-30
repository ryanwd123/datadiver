//! Expressions: a Pratt loop over binary operators, prefix operators, and
//! atoms with their postfix forms (`x::INT`, `a[1]`, `f(x) OVER (...)`).
//!
//! Binding powers follow DuckDB's grammar (`%left`/`%nonassoc` in
//! `grammar.y`), from loosest to tightest: `->` lambdas, OR, AND, NOT, IS,
//! comparisons, BETWEEN/IN/LIKE, other operators, `+ -`, `* / // %`,
//! `^ **`, AT TIME ZONE, COLLATE, unary minus, then postfix forms.

use super::super::{CompletedMarker, Parser};
use super::names::{self, NameRule};
use super::{select, types};
use crate::complete::{Expected, FunctionKind, Name};
use crate::{Kw, SyntaxKind as K, dialect};

const BP_ARROW: u8 = 1;
const BP_OR: u8 = 3;
const BP_AND: u8 = 5;
const BP_NOT: u8 = 7;
const BP_IS: u8 = 9;
const BP_CMP: u8 = 11;
const BP_LIKE: u8 = 13;
const BP_OTHER: u8 = 15;
const BP_ADD: u8 = 17;
const BP_MUL: u8 = 19;
const BP_POW: u8 = 21;
const BP_AT: u8 = 23;
const BP_COLLATE: u8 = 25;
const BP_UNARY: u8 = 27;

/// Keywords an expression can start with, offered at the hole.
const EXPR_START: &[Kw] = &[
    Kw::Case,
    Kw::Cast,
    Kw::TryCast,
    Kw::Not,
    Kw::Exists,
    Kw::Null,
    Kw::True,
    Kw::False,
    Kw::Interval,
    Kw::Lambda,
    Kw::Columns,
];

pub(super) fn expr(p: &mut Parser) -> Option<CompletedMarker> {
    expr_bp(p, 0)
}

/// An expression whose loosest operator binds at least as tight as `min_bp`.
pub(super) fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    p.nested(|p| expr_bp_inner(p, min_bp))
}

fn expr_bp_inner(p: &mut Parser, min_bp: u8) -> Option<CompletedMarker> {
    let mut lhs = prefix(p)?;
    while let Some((op, l_bp)) = infix_op(p) {
        if l_bp < min_bp {
            break;
        }
        let m = lhs.precede(p);
        let kind = match op {
            Op::Binary(n) => {
                for _ in 0..n {
                    p.bump();
                }
                rhs(p, l_bp + 1);
                K::BIN_EXPR
            }
            Op::KwBinary(kw) => {
                p.bump_kw(kw);
                rhs(p, l_bp + 1);
                K::BIN_EXPR
            }
            Op::Arrow => {
                p.bump();
                rhs(p, l_bp);
                K::LAMBDA_EXPR
            }
            Op::Comparison => {
                p.bump();
                if !quantified(p) {
                    rhs(p, l_bp + 1);
                }
                K::BIN_EXPR
            }
            Op::Is => {
                is_rest(p);
                K::IS_EXPR
            }
            Op::NullTest(kw) => {
                p.bump_kw(kw);
                K::IS_EXPR
            }
            Op::In => {
                p.eat_kw(Kw::Not);
                p.bump_kw(Kw::In);
                in_rhs(p);
                K::IN_EXPR
            }
            Op::Between => {
                p.eat_kw(Kw::Not);
                p.bump_kw(Kw::Between);
                if !p.eat_kw(Kw::Symmetric) {
                    p.eat_kw(Kw::Asymmetric);
                }
                rhs(p, BP_LIKE + 1);
                p.expect_kw(Kw::And);
                rhs(p, BP_LIKE + 1);
                K::BETWEEN_EXPR
            }
            Op::Like => {
                p.eat_kw(Kw::Not);
                if p.peek_kw(Kw::Similar) {
                    p.bump_kw(Kw::Similar);
                    p.expect_kw(Kw::To);
                } else {
                    p.bump_any_kw();
                }
                rhs(p, BP_LIKE + 1);
                if p.eat_kw(Kw::Escape) {
                    rhs(p, BP_LIKE + 1);
                }
                K::LIKE_EXPR
            }
            Op::Collate => {
                p.bump_kw(Kw::Collate);
                if names::path(p, NameRule::Any, &|_, _| {}).is_none() {
                    p.error("expected a collation");
                }
                K::COLLATE_EXPR
            }
            Op::AtTimeZone => {
                p.bump_kw(Kw::At);
                p.expect_kw(Kw::Time);
                p.expect_kw(Kw::Zone);
                rhs(p, BP_AT + 1);
                K::AT_TIME_ZONE_EXPR
            }
        };
        lhs = m.complete(p, kind);
    }
    Some(lhs)
}

fn rhs(p: &mut Parser, min_bp: u8) {
    if expr_bp(p, min_bp).is_none() {
        p.error("expected an expression");
    }
}

#[derive(Clone, Copy)]
enum Op {
    /// A punctuation operator made of this many tokens.
    Binary(u8),
    KwBinary(Kw),
    Comparison,
    Arrow,
    Is,
    NullTest(Kw),
    In,
    Between,
    Like,
    Collate,
    AtTimeZone,
}

/// The operator at the current token and its left binding power. At the hole
/// this records every operator keyword, since any of them could follow.
fn infix_op(p: &mut Parser) -> Option<(Op, u8)> {
    if p.at_hole() {
        for kw in [
            Kw::And,
            Kw::Or,
            Kw::Is,
            Kw::In,
            Kw::Not,
            Kw::Between,
            Kw::Like,
            Kw::Ilike,
            Kw::Glob,
            Kw::Collate,
            Kw::Isnull,
            Kw::Notnull,
        ] {
            p.record(Expected::Keyword(kw));
        }
        p.record(Expected::Phrase(&[Kw::Similar, Kw::To]));
        p.record(Expected::Phrase(&[Kw::At, Kw::Time, Kw::Zone]));
        return None;
    }
    let op = match p.current() {
        K::EQ | K::EQ2 | K::NEQ | K::LT | K::GT | K::LTE | K::GTE => (Op::Comparison, BP_CMP),
        K::PLUS | K::MINUS => (Op::Binary(1), BP_ADD),
        // `LIMIT 10%` ends with a percent sign, not a modulo
        K::PERCENT if !can_start_operand(p, 1) => return None,
        K::STAR | K::SLASH | K::SLASH2 | K::PERCENT => (Op::Binary(1), BP_MUL),
        K::CARET | K::STAR2 => (Op::Binary(1), BP_POW),
        K::ARROW => (Op::Arrow, BP_ARROW),
        K::TILDE2 | K::TILDE2_STAR | K::NOT_TILDE2 | K::NOT_TILDE2_STAR => (Op::Binary(1), BP_LIKE),
        K::PIPE2
        | K::LONG_ARROW
        | K::AT_GT
        | K::LT_AT
        | K::AMP2
        | K::AMP
        | K::PIPE
        | K::HASH
        | K::SHL
        | K::SHR
        | K::CARET_AT
        | K::TILDE
        | K::TILDE_STAR
        | K::NOT_TILDE
        | K::NOT_TILDE_STAR
        | K::AT => (Op::Binary(1), BP_OTHER),
        K::IDENT => {
            let kw = p.nth_kw(0)?;
            match kw {
                Kw::Or => (Op::KwBinary(kw), BP_OR),
                Kw::And => (Op::KwBinary(kw), BP_AND),
                Kw::Is => (Op::Is, BP_IS),
                Kw::Isnull | Kw::Notnull => (Op::NullTest(kw), BP_IS),
                Kw::In => (Op::In, BP_LIKE),
                Kw::Between => (Op::Between, BP_LIKE),
                Kw::Like | Kw::Ilike | Kw::Glob => (Op::Like, BP_LIKE),
                Kw::Similar if p.nth_at_kw(1, Kw::To) => (Op::Like, BP_LIKE),
                Kw::Not => match p.nth_kw(1)? {
                    Kw::In => (Op::In, BP_LIKE),
                    Kw::Between => (Op::Between, BP_LIKE),
                    Kw::Like | Kw::Ilike | Kw::Glob | Kw::Similar => (Op::Like, BP_LIKE),
                    _ => return None,
                },
                Kw::Collate => (Op::Collate, BP_COLLATE),
                Kw::At if p.nth_at_kw(1, Kw::Time) => (Op::AtTimeZone, BP_AT),
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(op)
}

/// Whether the n-th token can begin an operand.
fn can_start_operand(p: &Parser, n: usize) -> bool {
    match p.nth(n) {
        K::IDENT => !p
            .nth_kw(n)
            .is_some_and(|kw| kw.is_reserved() && !EXPR_START.contains(&kw)),
        K::QUOTED_IDENT | K::INT_NUMBER | K::FLOAT_NUMBER | K::PARAM | K::CURSOR => true,
        K::L_PAREN | K::L_BRACK | K::L_CURLY | K::MINUS | K::PLUS | K::TILDE | K::HASH => true,
        k => k.is_string(),
    }
}

/// `IS [NOT] NULL / TRUE / FALSE / UNKNOWN / DISTINCT FROM x`.
fn is_rest(p: &mut Parser) {
    p.bump_kw(Kw::Is);
    p.eat_kw(Kw::Not);
    if p.eat_kw(Kw::Distinct) {
        p.expect_kw(Kw::From);
        rhs(p, BP_IS + 1);
        return;
    }
    for kw in [Kw::Null, Kw::True, Kw::False, Kw::Unknown] {
        if p.eat_kw(kw) {
            return;
        }
    }
    p.record(Expected::Phrase(&[Kw::Distinct, Kw::From]));
    p.error("expected NULL, TRUE, FALSE or DISTINCT FROM");
}

/// `ANY (...)`, `ALL (...)`, `SOME (...)` after a comparison.
fn quantified(p: &mut Parser) -> bool {
    let is_quantifier = p.at_any_kw(&[Kw::Any, Kw::All, Kw::Some]);
    if !is_quantifier || p.nth(1) != K::L_PAREN {
        return false;
    }
    let m = p.start();
    p.bump_any_kw();
    paren_or_subquery(p);
    m.complete(p, K::QUANTIFIED_EXPR);
    true
}

/// The right side of IN: a parenthesized list or subquery, or a list expression.
fn in_rhs(p: &mut Parser) {
    if p.at(K::L_PAREN) {
        paren_or_subquery(p);
    } else {
        rhs(p, BP_LIKE + 1);
    }
}

/// Prefix operators, then an atom with its postfix forms.
fn prefix(p: &mut Parser) -> Option<CompletedMarker> {
    let op_bp = match p.current() {
        K::MINUS | K::PLUS | K::TILDE | K::AT | K::BANG => Some(BP_UNARY),
        K::IDENT if p.peek_kw(Kw::Not) => Some(BP_NOT),
        _ => None,
    };
    if let Some(bp) = op_bp {
        let m = p.start();
        p.bump_any_kw_if(Kw::Not);
        rhs(p, bp);
        return Some(m.complete(p, K::PREFIX_EXPR));
    }
    let lhs = atom(p)?;
    Some(postfix(p, lhs))
}

impl Parser<'_> {
    /// Bumps `kw` as a keyword if the current token is it, otherwise as it is.
    fn bump_any_kw_if(&mut self, kw: Kw) {
        if self.peek_kw(kw) {
            self.bump_kw(kw)
        } else {
            self.bump()
        }
    }
}

/// `x::type`, `x[i]`, `x[a:b]`, `x.field` and `x.method(...)`.
pub(super) fn postfix(p: &mut Parser, mut lhs: CompletedMarker) -> CompletedMarker {
    loop {
        match p.current() {
            K::DOUBLE_COLON => {
                let m = lhs.precede(p);
                p.bump();
                types::type_name(p);
                lhs = m.complete(p, K::CAST_EXPR);
            }
            K::L_BRACK => {
                let m = lhs.precede(p);
                p.bump();
                index_inner(p);
                p.expect(K::R_BRACK);
                lhs = m.complete(p, K::INDEX_EXPR);
            }
            K::DOT if names::nth_is_word(p, 1) => {
                let m = lhs.precede(p);
                p.bump();
                if p.at_hole() {
                    p.record(Expected::FunctionName {
                        kind: FunctionKind::Scalar,
                        qualifier: Vec::new(),
                    });
                }
                let name = p.start();
                names::bump_name(p);
                name.complete(p, K::NAME);
                lhs = m.complete(p, K::FIELD_EXPR);
                if p.current() == K::L_PAREN {
                    let m = lhs.precede(p);
                    arg_list(p, false);
                    lhs = m.complete(p, K::CALL_EXPR);
                }
            }
            _ => return lhs,
        }
    }
}

/// The inside of `[...]` after an expression: an index or a slice.
fn index_inner(p: &mut Parser) {
    if !p.at(K::COLON) && expr(p).is_none() {
        p.error("expected an index");
    }
    for _ in 0..2 {
        if !p.eat(K::COLON) {
            break;
        }
        if !p.at(K::COLON) && !p.at(K::R_BRACK) {
            // `-` alone is "to the end" in DuckDB slices
            if p.current() == K::MINUS && p.nth(1) == K::R_BRACK {
                p.bump();
            } else {
                expr(p);
            }
        }
    }
}

fn atom(p: &mut Parser) -> Option<CompletedMarker> {
    if p.at_hole() {
        p.record(Expected::Expr);
        for &kw in EXPR_START {
            p.record(Expected::Keyword(kw));
        }
        return name_expr(p);
    }
    let m = p.start();
    let kind = match p.current() {
        K::INT_NUMBER | K::FLOAT_NUMBER => {
            p.bump();
            K::LITERAL
        }
        k if k.is_string() => {
            p.bump();
            K::LITERAL
        }
        K::PARAM => {
            p.bump();
            K::PARAM_EXPR
        }
        K::HASH if p.nth(1) == K::INT_NUMBER => {
            p.bump();
            p.bump();
            K::POSITIONAL_REF
        }
        K::L_PAREN => {
            m.abandon(p);
            return Some(paren_or_subquery(p));
        }
        K::L_BRACK => {
            list(p);
            K::LIST_EXPR
        }
        K::L_CURLY => {
            struct_body(p);
            K::STRUCT_EXPR
        }
        K::STAR => {
            m.abandon(p);
            return Some(star_expr(p, None));
        }
        K::IDENT => {
            m.abandon(p);
            return keyword_atom(p);
        }
        K::QUOTED_IDENT | K::UNTERMINATED_QUOTED_IDENT => {
            m.abandon(p);
            return name_expr(p);
        }
        _ => {
            m.abandon(p);
            return None;
        }
    };
    Some(m.complete(p, kind))
}

/// Atoms that start with a word: keyword forms, typed literals, names and calls.
fn keyword_atom(p: &mut Parser) -> Option<CompletedMarker> {
    let kw = p.nth_kw(0);
    let next = p.nth(1);
    match kw {
        Some(Kw::True | Kw::False | Kw::Null) => {
            let m = p.start();
            p.bump_any_kw();
            return Some(m.complete(p, K::LITERAL));
        }
        Some(Kw::Case) => return Some(case_expr(p)),
        Some(Kw::Cast | Kw::TryCast) if next == K::L_PAREN => return Some(cast_expr(p)),
        Some(Kw::Exists) if next == K::L_PAREN => {
            let m = p.start();
            p.bump_kw(Kw::Exists);
            paren_or_subquery(p);
            return Some(m.complete(p, K::EXISTS_EXPR));
        }
        Some(Kw::Interval) => return Some(interval(p)),
        Some(Kw::Array) if matches!(next, K::L_BRACK | K::L_PAREN) => {
            let m = p.start();
            p.bump_kw(Kw::Array);
            if p.current() == K::L_BRACK {
                list(p);
            } else {
                paren_or_subquery(p);
            }
            return Some(m.complete(p, K::LIST_EXPR));
        }
        Some(Kw::Map) if next == K::L_CURLY => {
            let m = p.start();
            p.bump_kw(Kw::Map);
            struct_body(p);
            return Some(m.complete(p, K::MAP_EXPR));
        }
        Some(Kw::Lambda) => return Some(lambda(p)),
        Some(Kw::Default) => {
            let m = p.start();
            p.bump_kw(Kw::Default);
            return Some(m.complete(p, K::DEFAULT_EXPR));
        }
        Some(Kw::Extract | Kw::Position | Kw::Substring | Kw::Trim | Kw::Overlay)
            if next == K::L_PAREN =>
        {
            return Some(special_call(p, kw.unwrap()));
        }
        Some(Kw::Any | Kw::All | Kw::Some) if next == K::L_PAREN => {
            // `x = ANY (...)` is taken by the comparison; here it is a plain call.
        }
        Some(Kw::Columns | Kw::Unpack) if next == K::L_PAREN => {}
        Some(kw) if kw.is_reserved() && next != K::L_PAREN => return None,
        _ => {}
    }
    if next.is_string() && types::is_literal_type(p.nth_text(0)) {
        let m = p.start();
        let t = p.start();
        p.bump_any_kw();
        t.complete(p, K::TYPE_NAME);
        p.bump();
        return Some(m.complete(p, K::LITERAL));
    }
    name_expr(p)
}

/// A column reference, `t.*`, or a call.
fn name_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let first_ok =
        names::word_ok(p, 0, NameRule::NotReserved) || p.nth(1) == K::L_PAREN || p.at_hole();
    if !first_ok {
        return None;
    }
    let path = names::path(p, NameRule::Any, &|p, q: Vec<Name>| {
        p.record(Expected::ColumnRef {
            qualifier: q.clone(),
        });
        p.record(Expected::FunctionName {
            kind: FunctionKind::Any,
            qualifier: q,
        });
    })?;
    if p.current() == K::DOT && p.nth(1) == K::STAR {
        let m = path.precede(p);
        p.bump();
        p.bump();
        star_modifiers(p);
        return Some(m.complete(p, K::STAR_EXPR));
    }
    if p.current() == K::L_PAREN {
        let m = path.precede(p);
        let callee = last_word_text(p);
        let path_fn = dialect::takes_path(&callee);
        call_rest(p, path_fn);
        return Some(m.complete(p, K::CALL_EXPR));
    }
    let m = path.precede(p);
    Some(m.complete(p, K::COLUMN_REF))
}

/// The text of the token just bumped.
fn last_word_text(p: &Parser) -> String {
    p.prev_text().to_string()
}

/// `(args) [WITHIN GROUP (...)] [FILTER (...)] [IGNORE NULLS] [OVER ...]` after a callee.
pub(super) fn call_rest(p: &mut Parser, path_fn: bool) {
    arg_list(p, path_fn);
    if p.at_kw(Kw::Within) && p.nth_at_kw(1, Kw::Group) {
        let m = p.start();
        p.bump_kw(Kw::Within);
        p.bump_kw(Kw::Group);
        p.expect(K::L_PAREN);
        select::order_by(p);
        p.expect(K::R_PAREN);
        m.complete(p, K::WITHIN_GROUP_CLAUSE);
    }
    if p.at_kw(Kw::Filter) && p.nth(1) == K::L_PAREN {
        let m = p.start();
        p.bump_kw(Kw::Filter);
        p.bump();
        p.eat_kw(Kw::Where);
        if expr(p).is_none() {
            p.error("expected a condition");
        }
        p.expect(K::R_PAREN);
        m.complete(p, K::FILTER_CLAUSE);
    }
    if p.at_any_kw(&[Kw::Ignore, Kw::Respect]) && p.nth_at_kw(1, Kw::Nulls) {
        p.bump_any_kw();
        p.bump_kw(Kw::Nulls);
    }
    if p.at_kw(Kw::Over) {
        over(p);
    }
}

/// `(DISTINCT a, b := 1, ... ORDER BY x)`.
pub(super) fn arg_list(p: &mut Parser, path_fn: bool) {
    let m = p.start();
    p.expect(K::L_PAREN);
    if !p.eat_kw(Kw::Distinct) {
        p.eat_kw(Kw::All);
    }
    let mut first = true;
    while !p.at(K::R_PAREN) && !p.at_eof() {
        if p.at_kw(Kw::Order) {
            break;
        }
        arg(p, first && path_fn);
        first = false;
        if !p.eat(K::COMMA) {
            break;
        }
    }
    if p.at_phrase(&[Kw::Order, Kw::By]) {
        select::order_by(p);
    }
    if p.at_any_kw(&[Kw::Ignore, Kw::Respect]) && p.nth_at_kw(1, Kw::Nulls) {
        p.bump_any_kw();
        p.bump_kw(Kw::Nulls);
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(p, K::ARG_LIST);
}

fn arg(p: &mut Parser, path_arg: bool) {
    if names::at_name(p, NameRule::Any) && matches!(p.nth(1), K::COLON_EQ | K::FAT_ARROW) {
        let m = p.start();
        let name = p.start();
        names::bump_name(p);
        name.complete(p, K::NAME);
        p.bump();
        rhs(p, 0);
        m.complete(p, K::NAMED_ARG);
        return;
    }
    if path_arg && p.current().is_string() {
        p.record_string(Expected::FilePath);
    }
    if expr(p).is_none() {
        p.err_recover("expected an argument", &[]);
    }
}

/// `OVER (spec)` or `OVER name`.
fn over(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Over);
    if p.at(K::L_PAREN) {
        window_spec(p);
    } else if !names::new_name(p, NameRule::ColId, crate::complete::NewNameKind::Window) {
        p.error("expected a window");
    }
    m.complete(p, K::OVER_CLAUSE);
}

/// `([name] [PARTITION BY ...] [ORDER BY ...] [frame])`.
pub(super) fn window_spec(p: &mut Parser) {
    let m = p.start();
    p.bump();
    if names::at_name(p, NameRule::ColId)
        && !p.peek_kw(Kw::Partition)
        && !p.peek_kw(Kw::Rows)
        && !p.peek_kw(Kw::Range)
        && !p.peek_kw(Kw::Groups)
    {
        let n = p.start();
        names::bump_name(p);
        n.complete(p, K::NAME);
    }
    if p.at_phrase(&[Kw::Partition, Kw::By]) {
        let pm = p.start();
        p.bump_kw(Kw::Partition);
        p.expect_kw(Kw::By);
        expr_list(p);
        pm.complete(p, K::PARTITION_BY);
    }
    if p.at_phrase(&[Kw::Order, Kw::By]) {
        select::order_by(p);
    }
    if p.at_any_kw(&[Kw::Rows, Kw::Range, Kw::Groups]) {
        frame(p);
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(p, K::WINDOW_SPEC);
}

fn frame(p: &mut Parser) {
    let m = p.start();
    p.bump_any_kw();
    if p.eat_kw(Kw::Between) {
        frame_bound(p);
        p.expect_kw(Kw::And);
        frame_bound(p);
    } else {
        frame_bound(p);
    }
    if p.eat_kw(Kw::Exclude) {
        if p.eat_kw(Kw::Current) {
            p.expect_kw(Kw::Row);
        } else if p.eat_kw(Kw::No) {
            p.expect_kw(Kw::Others);
        } else if !p.eat_kw(Kw::Group) && !p.eat_kw(Kw::Ties) {
            p.error("expected CURRENT ROW, GROUP, TIES or NO OTHERS");
        }
    }
    m.complete(p, K::FRAME_CLAUSE);
}

fn frame_bound(p: &mut Parser) {
    let m = p.start();
    if p.eat_kw(Kw::Unbounded) {
        if !p.eat_kw(Kw::Preceding) {
            p.expect_kw(Kw::Following);
        }
    } else if p.eat_kw(Kw::Current) {
        p.expect_kw(Kw::Row);
    } else {
        if expr_bp(p, BP_ADD).is_none() {
            p.error("expected a frame bound");
        }
        if !p.eat_kw(Kw::Preceding) {
            p.expect_kw(Kw::Following);
        }
    }
    m.complete(p, K::FRAME_BOUND);
}

/// Comma-separated expressions.
pub(super) fn expr_list(p: &mut Parser) {
    loop {
        if expr(p).is_none() {
            p.err_recover("expected an expression", &[]);
            break;
        }
        if !p.eat(K::COMMA) {
            break;
        }
    }
}

/// `(expr)`, `(a, b)` or `(subquery)`.
pub(super) fn paren_or_subquery(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.expect(K::L_PAREN);
    if select::at_query_start(p, false) {
        select::query(p);
        p.expect(K::R_PAREN);
        return m.complete(p, K::SUBQUERY_EXPR);
    }
    let mut count = 0;
    let mut trailing_comma = false;
    while !p.at(K::R_PAREN) && !p.at_eof() {
        if expr(p).is_none() {
            p.err_recover("expected an expression", &[]);
            break;
        }
        count += 1;
        trailing_comma = false;
        if !p.eat(K::COMMA) {
            break;
        }
        trailing_comma = true;
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(
        p,
        if count == 1 && !trailing_comma {
            K::PAREN_EXPR
        } else {
            K::ROW_EXPR
        },
    )
}

/// `[a, b]` or `[expr FOR x IN list IF cond]`.
fn list(p: &mut Parser) {
    p.bump();
    let mut first = true;
    while !p.at(K::R_BRACK) && !p.at_eof() {
        if expr(p).is_none() {
            p.err_recover("expected an expression", &[]);
            break;
        }
        if first && p.at_kw(Kw::For) {
            p.bump_kw(Kw::For);
            names::new_name(p, NameRule::ColId, crate::complete::NewNameKind::Other);
            while p.eat(K::COMMA) {
                names::new_name(p, NameRule::ColId, crate::complete::NewNameKind::Other);
            }
            p.expect_kw(Kw::In);
            rhs(p, 0);
            if p.eat_kw(Kw::If) {
                rhs(p, 0);
            }
            break;
        }
        first = false;
        if !p.eat(K::COMMA) {
            break;
        }
    }
    p.expect(K::R_BRACK);
}

/// `{key: value, ...}` for structs and `MAP {...}`.
fn struct_body(p: &mut Parser) {
    p.expect(K::L_CURLY);
    while !p.at(K::R_CURLY) && !p.at_eof() {
        let m = p.start();
        if expr(p).is_none() {
            m.abandon(p);
            p.err_recover("expected a key", &[]);
            break;
        }
        if p.expect(K::COLON) {
            rhs(p, 0);
        }
        m.complete(p, K::STRUCT_FIELD);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    if !p.eat(K::R_CURLY) {
        p.err_recover("expected '}'", &[]);
        p.eat(K::R_CURLY);
    }
}

fn case_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_kw(Kw::Case);
    if !p.at_kw(Kw::When) {
        expr(p);
    }
    while p.at_kw(Kw::When) {
        let w = p.start();
        p.bump_kw(Kw::When);
        rhs(p, 0);
        if p.expect_kw(Kw::Then) {
            rhs(p, 0);
        }
        w.complete(p, K::WHEN_CLAUSE);
    }
    if p.at_kw(Kw::Else) {
        let e = p.start();
        p.bump_kw(Kw::Else);
        rhs(p, 0);
        e.complete(p, K::ELSE_CLAUSE);
    }
    p.expect_kw(Kw::End);
    m.complete(p, K::CASE_EXPR)
}

fn cast_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_any_kw();
    p.bump();
    rhs(p, 0);
    if p.expect_kw(Kw::As) {
        types::type_name(p);
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(p, K::CAST_EXPR)
}

/// Units an `INTERVAL` literal can be given in.
const INTERVAL_UNITS: &[&str] = &[
    "year",
    "years",
    "month",
    "months",
    "day",
    "days",
    "hour",
    "hours",
    "minute",
    "minutes",
    "second",
    "seconds",
    "millisecond",
    "milliseconds",
    "microsecond",
    "microseconds",
    "week",
    "weeks",
    "quarter",
    "quarters",
    "decade",
    "decades",
    "century",
    "centuries",
    "millennium",
    "millennia",
];

fn at_interval_unit(p: &Parser) -> bool {
    p.current() == K::IDENT
        && INTERVAL_UNITS
            .iter()
            .any(|u| u.eq_ignore_ascii_case(p.nth_text(0)))
}

fn interval(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_kw(Kw::Interval);
    match p.current() {
        K::L_PAREN => {
            paren_or_subquery(p);
        }
        k if k.is_string() || matches!(k, K::INT_NUMBER | K::FLOAT_NUMBER | K::PARAM) => p.bump(),
        K::MINUS if matches!(p.nth(1), K::INT_NUMBER | K::FLOAT_NUMBER) => {
            p.bump();
            p.bump();
        }
        _ => {
            if expr_bp(p, BP_UNARY).is_none() {
                p.error("expected an interval");
            }
        }
    }
    if at_interval_unit(p) {
        p.bump_any_kw();
        if p.eat_kw(Kw::To) {
            if at_interval_unit(p) {
                p.bump_any_kw();
            } else {
                p.error("expected an interval unit");
            }
        }
    }
    m.complete(p, K::INTERVAL_EXPR)
}

/// `LAMBDA x, y: body`.
fn lambda(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump_kw(Kw::Lambda);
    let params = p.start();
    loop {
        if !names::new_name(p, NameRule::ColId, crate::complete::NewNameKind::Other)
            || !p.eat(K::COMMA)
        {
            break;
        }
    }
    params.complete(p, K::LAMBDA_PARAMS);
    if p.expect(K::COLON) {
        rhs(p, BP_OR);
    }
    m.complete(p, K::LAMBDA_EXPR)
}

/// EXTRACT(field FROM x), POSITION(a IN b), SUBSTRING(s FROM a FOR b),
/// TRIM([BOTH] chars FROM s), OVERLAY(s PLACING x FROM a FOR b), and the same
/// functions called with plain arguments.
fn special_call(p: &mut Parser, kw: Kw) -> CompletedMarker {
    let m = p.start();
    let callee = p.start();
    let path = p.start();
    p.bump();
    path.complete(p, K::PATH);
    callee.abandon(p);
    let args = p.start();
    p.bump();
    match kw {
        Kw::Extract => {
            if p.current().is_string() || names::at_name(p, NameRule::Any) {
                p.bump();
                if p.peek_kw(Kw::From) {
                    p.bump_kw(Kw::From);
                } else {
                    p.expect(K::COMMA);
                }
            }
            rhs(p, 0);
        }
        Kw::Position => {
            rhs(p, BP_LIKE + 1);
            if !p.eat_kw(Kw::In) {
                p.expect(K::COMMA);
            }
            rhs(p, 0);
        }
        Kw::Trim => {
            if !p.eat_kw(Kw::Both) && !p.eat_kw(Kw::Leading) {
                p.eat_kw(Kw::Trailing);
            }
            if !p.at_kw(Kw::From) {
                rhs(p, 0);
            }
            if p.eat_kw(Kw::From) || p.eat(K::COMMA) {
                rhs(p, 0);
            }
        }
        _ => {
            // SUBSTRING and OVERLAY: keyword-separated or comma-separated
            rhs(p, 0);
            loop {
                if p.eat_kw(Kw::From)
                    || p.eat_kw(Kw::For)
                    || p.eat_kw(Kw::Placing)
                    || p.eat(K::COMMA)
                {
                    rhs(p, 0);
                } else {
                    break;
                }
            }
        }
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    args.complete(p, K::ARG_LIST);
    m.complete(p, K::SPECIAL_CALL)
}

/// `*` or `t.*` (the path already bumped by the caller) with EXCLUDE /
/// REPLACE / RENAME / LIKE modifiers.
pub(super) fn star_expr(p: &mut Parser, _qualifier: Option<CompletedMarker>) -> CompletedMarker {
    let m = p.start();
    p.bump();
    star_modifiers(p);
    m.complete(p, K::STAR_EXPR)
}

fn star_modifiers(p: &mut Parser) {
    loop {
        if p.at_kw(Kw::Exclude) {
            let m = p.start();
            p.bump_kw(Kw::Exclude);
            names::column_list(p, false);
            m.complete(p, K::EXCLUDE_CLAUSE);
        } else if p.at_kw(Kw::Replace) {
            let m = p.start();
            p.bump_kw(Kw::Replace);
            let parens = p.eat(K::L_PAREN);
            loop {
                let item = p.start();
                rhs(p, 0);
                if p.expect_kw(Kw::As) {
                    names::new_name(p, NameRule::Any, crate::complete::NewNameKind::Column);
                }
                item.complete(p, K::REPLACE_ITEM);
                if !parens || !p.eat(K::COMMA) {
                    break;
                }
            }
            if parens {
                p.expect(K::R_PAREN);
            }
            m.complete(p, K::REPLACE_CLAUSE);
        } else if p.at_kw(Kw::Rename) {
            let m = p.start();
            p.bump_kw(Kw::Rename);
            let parens = p.eat(K::L_PAREN);
            loop {
                let item = p.start();
                if names::path(p, NameRule::Any, &|p, q| {
                    p.record(Expected::ColumnRef { qualifier: q })
                })
                .is_none()
                {
                    p.error("expected a column");
                }
                if p.expect_kw(Kw::As) {
                    names::new_name(p, NameRule::Any, crate::complete::NewNameKind::Column);
                }
                item.complete(p, K::RENAME_ITEM);
                if !parens || !p.eat(K::COMMA) {
                    break;
                }
            }
            if parens {
                p.expect(K::R_PAREN);
            }
            m.complete(p, K::RENAME_CLAUSE);
        } else {
            break;
        }
    }
}
