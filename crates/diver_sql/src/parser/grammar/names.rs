//! Names: single identifiers, dotted paths, aliases and name lists.

use super::super::Parser;
use crate::complete::{Expected, Name, NewNameKind};
use crate::{Kw, KwCategory, SyntaxKind as K};

/// Which words may stand as a bare (unquoted) name in a position.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NameRule {
    /// Any word, reserved ones included (after `.` or `AS`).
    Any,
    /// Column and table names: no reserved or type/function-only keywords.
    ColId,
    /// Function and type names: no reserved or column-only keywords.
    TypeFunc,
    /// Names that are not reserved: functions in calls, column refs.
    NotReserved,
    /// A column label without `AS`: only words that are not keywords, or unreserved ones.
    BareLabel,
}

pub(super) fn word_ok(p: &Parser, n: usize, rule: NameRule) -> bool {
    match p.nth(n) {
        K::QUOTED_IDENT | K::UNTERMINATED_QUOTED_IDENT => true,
        K::IDENT => {
            let Some(kw) = p.nth_kw(n) else { return true };
            let cat = kw.category();
            match rule {
                NameRule::Any => true,
                NameRule::ColId => {
                    matches!(
                        cat,
                        KwCategory::Unreserved | KwCategory::ColumnName | KwCategory::Contextual
                    )
                }
                NameRule::TypeFunc => {
                    matches!(
                        cat,
                        KwCategory::Unreserved | KwCategory::TypeFunction | KwCategory::Contextual
                    )
                }
                NameRule::NotReserved => cat != KwCategory::Reserved,
                NameRule::BareLabel => {
                    matches!(cat, KwCategory::Unreserved | KwCategory::Contextual)
                }
            }
        }
        _ => false,
    }
}

pub(super) fn at_name(p: &Parser, rule: NameRule) -> bool {
    word_ok(p, 0, rule)
}

/// Whether the n-th token is a word of any kind or the hole.
pub(super) fn nth_is_word(p: &Parser, n: usize) -> bool {
    matches!(
        p.nth(n),
        K::IDENT | K::QUOTED_IDENT | K::UNTERMINATED_QUOTED_IDENT | K::CURSOR
    )
}

/// The name the current token spells.
pub(super) fn current_name(p: &Parser) -> Name {
    Name::from_token(p.current(), p.nth_text(0))
}

/// Bumps the current word as a name: it keeps its token kind, so a keyword
/// used as a name stays an `IDENT`.
pub(super) fn bump_name(p: &mut Parser) {
    p.bump();
}

/// A new name (`NAME`): an alias, a CTE, a created object. At the hole it
/// records `NewName(kind)` and takes the hole as the name.
pub(super) fn new_name(p: &mut Parser, rule: NameRule, kind: NewNameKind) -> bool {
    if p.at_hole() {
        p.record(Expected::NewName(kind));
    } else if !at_name(p, rule) {
        p.error("expected a name");
        return false;
    }
    let m = p.start();
    bump_name(p);
    m.complete(p, K::NAME);
    true
}

/// A dotted path `a.b.c` (`PATH`). At the hole, as the first part or after a
/// dot, it calls `on_hole` with the parts before it and takes the hole as a part.
/// Stops before `.*`, which the caller handles.
pub(super) fn path(
    p: &mut Parser,
    rule: NameRule,
    on_hole: &dyn Fn(&mut Parser, Vec<Name>),
) -> Option<super::super::CompletedMarker> {
    let mut parts = Vec::new();
    if p.at_hole() {
        on_hole(p, parts.clone());
    } else if !at_name(p, rule) {
        return None;
    }
    let m = p.start();
    parts.push(current_name(p));
    bump_name(p);
    while p.current() == K::DOT {
        if nth_is_word(p, 1) {
            p.bump();
            if p.at_hole() {
                on_hole(p, parts.clone());
            }
            parts.push(current_name(p));
            bump_name(p);
        } else if p.nth(1) == K::STAR {
            break;
        } else {
            p.bump();
            p.error("expected a name after '.'");
            break;
        }
    }
    Some(m.complete(p, K::PATH))
}

/// `[AS] alias [(col, ...)]` after a FROM item. Without `AS` only `ColId`
/// words that are not in `stop` count. At the hole it records the alias but
/// leaves the hole for what may follow.
pub(super) fn table_alias(p: &mut Parser, stop: &[Kw]) -> bool {
    let m = p.start();
    if p.eat_kw(Kw::As) {
        new_name(p, NameRule::Any, NewNameKind::Alias);
    } else if p.at_hole() {
        p.record(Expected::NewName(NewNameKind::Alias));
        m.abandon(p);
        return false;
    } else if at_name(p, NameRule::ColId)
        && !p
            .nth_kw(0)
            .is_some_and(|kw| stop.contains(&kw) || TABLE_ALIAS_STOP.contains(&kw))
    {
        new_name(p, NameRule::ColId, NewNameKind::Alias);
    } else {
        m.abandon(p);
        return false;
    }
    if p.current() == K::L_PAREN {
        column_aliases(p);
    }
    m.complete(p, K::ALIAS);
    true
}

/// Unreserved words that follow a FROM item in some statement and so can't
/// be its bare alias.
const TABLE_ALIAS_STOP: &[Kw] = &[Kw::Set, Kw::Sample, Kw::Default, Kw::By, Kw::Values];

/// `(a, b, c)` naming the columns of an aliased relation.
pub(super) fn column_aliases(p: &mut Parser) {
    let m = p.start();
    p.bump();
    loop {
        if !new_name(p, NameRule::Any, NewNameKind::Column) {
            break;
        }
        if !p.eat(K::COMMA) {
            break;
        }
    }
    p.expect(K::R_PAREN);
    m.complete(p, K::COLUMN_ALIASES);
}

/// `(a, b.c, ...)` or a single path, of existing columns.
pub(super) fn column_list(p: &mut Parser, parens_required: bool) {
    let m = p.start();
    let parens = p.eat(K::L_PAREN);
    if !parens && parens_required {
        p.error("expected '('");
        m.abandon(p);
        return;
    }
    loop {
        if path(p, NameRule::Any, &|p, q| {
            p.record(Expected::ColumnRef { qualifier: q })
        })
        .is_none()
        {
            if !(parens && p.at(K::R_PAREN)) {
                p.error("expected a column name");
            }
            break;
        }
        if !parens || !p.eat(K::COMMA) {
            break;
        }
    }
    if parens {
        p.expect(K::R_PAREN);
    }
    m.complete(p, K::NAME_LIST);
}

/// A path naming a table: records what a table position accepts at the hole.
pub(super) fn table_path(p: &mut Parser) -> Option<super::super::CompletedMarker> {
    path(p, NameRule::ColId, &record_table)
}

pub(super) fn record_table(p: &mut Parser, q: Vec<Name>) {
    p.record(Expected::TableRef {
        qualifier: q.clone(),
    });
    if q.is_empty() {
        p.record(Expected::CteName);
    }
    if q.len() < 2 {
        p.record(Expected::SchemaName {
            database: q.first().cloned(),
        });
    }
    if q.is_empty() {
        p.record(Expected::DatabaseName);
    }
}
