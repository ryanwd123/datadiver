//! CREATE, DROP, ALTER and COMMENT ON.

use super::super::Parser;
use super::expressions::{self, expr};
use super::names::{self, NameRule};
use super::{misc, select, types};
use crate::complete::{Expected, FunctionKind, Name, NewNameKind};
use crate::dialect::Feature;
use crate::{Kw, SyntaxKind as K};

/// What follows `CREATE [OR REPLACE] [TEMP]`.
const CREATE_KINDS: &[Kw] = &[
    Kw::Table,
    Kw::View,
    Kw::Macro,
    Kw::Function,
    Kw::Type,
    Kw::Sequence,
    Kw::Schema,
    Kw::Secret,
    Kw::Index,
];

pub(super) fn create(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Create);
    let or_replace = p.at_phrase(&[Kw::Or, Kw::Replace]);
    if or_replace {
        p.bump_kw(Kw::Or);
        p.expect_kw(Kw::Replace);
    }
    if !p.eat_kw(Kw::Temp) && !p.eat_kw(Kw::Temporary) {
        p.eat_kw(Kw::Persistent);
    }
    if p.peek_kw(Kw::Recursive) {
        p.bump_kw(Kw::Recursive);
    }
    if !or_replace {
        p.eat_kw(Kw::Unique);
    }
    for &kw in CREATE_KINDS {
        // DuckDB refuses CREATE OR REPLACE INDEX; Polars creates only tables
        if !(or_replace && kw == Kw::Index) && (p.allows(Feature::Statement) || kw == Kw::Table) {
            p.record(Expected::Keyword(kw));
        }
    }
    if p.nth_kw(0)
        .is_some_and(|kw| kw != Kw::Table && CREATE_KINDS.contains(&kw))
    {
        p.gate(Feature::Statement);
    }
    match p.nth_kw(0) {
        Some(Kw::Table) => {
            p.bump_kw(Kw::Table);
            if_not_exists(p);
            object_name(p, NewNameKind::Table);
            if p.current() == K::L_PAREN && !select::at_query_start(p, true) {
                table_elements(p);
            }
            if p.at_kw(Kw::On) && p.nth_at_kw(1, Kw::Commit) {
                p.bump_kw(Kw::On);
                p.bump_kw(Kw::Commit);
                if !p.eat_kw(Kw::Preserve) {
                    p.expect_kw(Kw::Delete);
                }
                p.expect_kw(Kw::Rows);
            }
            if p.at_kw(Kw::As) {
                p.bump_kw(Kw::As);
                select::query(p);
                if p.at_kw(Kw::With) {
                    p.bump_kw(Kw::With);
                    p.eat_kw(Kw::No);
                    p.expect_kw(Kw::Data);
                }
            }
        }
        Some(Kw::View) => {
            p.bump_kw(Kw::View);
            if_not_exists(p);
            object_name(p, NewNameKind::View);
            if p.current() == K::L_PAREN {
                names::column_aliases(p);
            }
            if p.expect_kw(Kw::As) {
                select::query(p);
            }
        }
        Some(Kw::Macro | Kw::Function) => {
            p.bump_any_kw();
            if_not_exists(p);
            object_name(p, NewNameKind::Macro);
            loop {
                macro_params(p);
                if !p.expect_kw(Kw::As) {
                    break;
                }
                if p.eat_kw(Kw::Table) {
                    select::query(p);
                } else if expr(p).is_none() {
                    p.error("expected the macro body");
                }
                if !(p.current() == K::COMMA && p.nth(1) == K::L_PAREN) {
                    break;
                }
                p.bump();
            }
        }
        Some(Kw::Type) => {
            p.bump_kw(Kw::Type);
            if_not_exists(p);
            object_name(p, NewNameKind::Type);
            if p.expect_kw(Kw::As) {
                if p.at_kw(Kw::Enum) && p.nth(1) == K::L_PAREN && select::query_inside(p, 2) {
                    p.bump_kw(Kw::Enum);
                    p.bump();
                    select::query(p);
                    p.expect(K::R_PAREN);
                } else {
                    types::type_name(p);
                }
            }
        }
        Some(Kw::Sequence) => {
            p.bump_kw(Kw::Sequence);
            if_not_exists(p);
            object_name(p, NewNameKind::Other);
            sequence_options(p);
        }
        Some(Kw::Schema) => {
            p.bump_kw(Kw::Schema);
            if_not_exists(p);
            object_name(p, NewNameKind::Schema);
        }
        Some(Kw::Index) => {
            p.bump_kw(Kw::Index);
            if_not_exists(p);
            if !p.at_kw(Kw::On) {
                names::new_name(p, NameRule::ColId, NewNameKind::Other);
            }
            if p.expect_kw(Kw::On) {
                let t = p.start();
                if names::table_path(p).is_none() {
                    p.error("expected a table");
                }
                t.complete(p, K::TABLE_REF);
            }
            if p.eat_kw(Kw::Using) {
                names::new_name(p, NameRule::Any, NewNameKind::Other);
            }
            if p.expect(K::L_PAREN) {
                loop {
                    if expr(p).is_none() {
                        p.err_recover("expected an expression", &[]);
                        break;
                    }
                    if !p.eat_kw(Kw::Asc) {
                        p.eat_kw(Kw::Desc);
                    }
                    if !p.eat(K::COMMA) {
                        break;
                    }
                }
                p.expect(K::R_PAREN);
            }
            if p.at_kw(Kw::With) {
                p.bump_kw(Kw::With);
                misc::option_list(p);
            }
        }
        Some(Kw::Secret) => {
            p.bump_kw(Kw::Secret);
            if_not_exists(p);
            if names::at_name(p, NameRule::ColId) && !p.peek_kw(Kw::In) {
                names::new_name(p, NameRule::ColId, NewNameKind::Other);
            }
            if p.eat_kw(Kw::In) {
                names::new_name(p, NameRule::Any, NewNameKind::Other);
            }
            misc::option_list(p);
        }
        _ => {
            p.err_recover(
                "expected TABLE, VIEW, MACRO, FUNCTION, TYPE, SEQUENCE, SCHEMA, INDEX or SECRET",
                &[],
            );
        }
    }
    m.complete(p, K::CREATE_STMT);
}

fn if_not_exists(p: &mut Parser) {
    if p.at_phrase(&[Kw::If, Kw::Not, Kw::Exists]) {
        p.bump_kw(Kw::If);
        p.expect_kw(Kw::Not);
        p.expect_kw(Kw::Exists);
    }
}

fn if_exists(p: &mut Parser) {
    if p.at_phrase(&[Kw::If, Kw::Exists]) {
        p.bump_kw(Kw::If);
        p.expect_kw(Kw::Exists);
    }
}

/// The (possibly qualified) name of a created object: a `PATH` whose last
/// part is new. At the hole it records the new name, and the schemas it may
/// go in.
fn object_name(p: &mut Parser, kind: NewNameKind) {
    let on_hole = move |p: &mut Parser, q: Vec<Name>| {
        p.record(Expected::NewName(kind));
        if q.len() < 2 {
            p.record(Expected::SchemaName {
                database: q.first().cloned(),
            });
        }
    };
    if p.current() == K::STRING {
        // CREATE TABLE 'name'
        let m = p.start();
        p.bump();
        m.complete(p, K::PATH);
    } else if names::path(p, NameRule::NotReserved, &on_hole).is_none() {
        p.error("expected a name");
    }
}

/// `(col type constraints, ..., table constraints)`.
fn table_elements(p: &mut Parser) {
    let m = p.start();
    p.bump();
    while !p.at(K::R_PAREN) && !p.at_eof() {
        if at_table_constraint(p) {
            table_constraint(p);
        } else {
            column_def(p);
        }
        if !p.eat(K::COMMA) {
            break;
        }
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(p, K::TABLE_ELEMENTS);
}

fn at_table_constraint(p: &mut Parser) -> bool {
    let hit = p.at_any_kw(&[
        Kw::Constraint,
        Kw::Primary,
        Kw::Unique,
        Kw::Check,
        Kw::Foreign,
    ]);
    hit && !(p.peek_kw(Kw::Unique) && p.nth(1) != K::L_PAREN)
}

fn table_constraint(p: &mut Parser) {
    let m = p.start();
    if p.eat_kw(Kw::Constraint) {
        names::new_name(p, NameRule::ColId, NewNameKind::Other);
    }
    match p.nth_kw(0) {
        Some(Kw::Primary) => {
            p.bump_kw(Kw::Primary);
            p.expect_kw(Kw::Key);
            names::column_list(p, true);
        }
        Some(Kw::Unique) => {
            p.bump_kw(Kw::Unique);
            names::column_list(p, true);
        }
        Some(Kw::Check) => {
            p.bump_kw(Kw::Check);
            paren_expr(p);
        }
        Some(Kw::Foreign) => {
            p.bump_kw(Kw::Foreign);
            p.expect_kw(Kw::Key);
            names::column_list(p, true);
            references(p);
        }
        _ => p.err_recover("expected a constraint", &[]),
    }
    m.complete(p, K::TABLE_CONSTRAINT);
}

fn paren_expr(p: &mut Parser) {
    if p.expect(K::L_PAREN) {
        if expr(p).is_none() {
            p.error("expected an expression");
        }
        p.expect(K::R_PAREN);
    }
}

fn references(p: &mut Parser) {
    if p.expect_kw(Kw::References) {
        let t = p.start();
        if names::table_path(p).is_none() {
            p.error("expected a table");
        }
        t.complete(p, K::TABLE_REF);
        if p.current() == K::L_PAREN {
            names::column_list(p, true);
        }
        // ON UPDATE / ON DELETE actions
        while p.at_kw(Kw::On) && (p.nth_at_kw(1, Kw::Update) || p.nth_at_kw(1, Kw::Delete)) {
            p.bump_kw(Kw::On);
            p.bump_any_kw();
            if p.eat_kw(Kw::No) {
                p.expect_kw(Kw::Action);
            } else if p.eat_kw(Kw::Set) {
                if !p.eat_kw(Kw::Null) {
                    p.expect_kw(Kw::Default);
                }
            } else if !p.eat_kw(Kw::Restrict) && !p.eat_kw(Kw::Cascade) {
                p.error("expected NO ACTION, RESTRICT, CASCADE or SET");
            }
        }
    }
}

pub(super) fn column_def(p: &mut Parser) {
    let m = p.start();
    if !names::new_name(p, NameRule::NotReserved, NewNameKind::Column) {
        m.abandon(p);
        p.err_recover("expected a column", &[]);
        return;
    }
    // `ADD COLUMN s.field.sub INTEGER` adds a struct field
    while p.current() == K::DOT && names::nth_is_word(p, 1) {
        p.bump();
        names::bump_name(p);
    }
    if !at_column_constraint(p) && !p.at(K::COMMA) && !p.at(K::R_PAREN) {
        types::type_name(p);
    }
    while at_column_constraint(p) {
        column_constraint(p);
    }
    m.complete(p, K::COLUMN_DEF);
}

fn at_column_constraint(p: &mut Parser) -> bool {
    p.at_any_kw(&[
        Kw::Not,
        Kw::Null,
        Kw::Primary,
        Kw::Unique,
        Kw::Default,
        Kw::Check,
        Kw::References,
        Kw::Collate,
        Kw::Generated,
        Kw::As,
        Kw::Constraint,
        Kw::Using,
    ])
}

fn column_constraint(p: &mut Parser) {
    let m = p.start();
    if p.eat_kw(Kw::Constraint) {
        names::new_name(p, NameRule::ColId, NewNameKind::Other);
    }
    match p.nth_kw(0) {
        Some(Kw::Not) => {
            p.bump_kw(Kw::Not);
            p.expect_kw(Kw::Null);
        }
        Some(Kw::Null) => p.bump_kw(Kw::Null),
        Some(Kw::Primary) => {
            p.bump_kw(Kw::Primary);
            p.expect_kw(Kw::Key);
        }
        Some(Kw::Unique) => p.bump_kw(Kw::Unique),
        Some(Kw::Default) => {
            p.bump_kw(Kw::Default);
            // stops before a following NOT NULL constraint
            if expressions::expr_bp(p, expressions::BP_IS).is_none() {
                p.error("expected a default");
            }
        }
        Some(Kw::Check) => {
            p.bump_kw(Kw::Check);
            paren_expr(p);
        }
        Some(Kw::References) => references(p),
        Some(Kw::Collate) => {
            p.bump_kw(Kw::Collate);
            if names::path(p, NameRule::Any, &|_, _| {}).is_none() {
                p.error("expected a collation");
            }
        }
        Some(Kw::Using) => {
            // USING COMPRESSION name
            p.bump_kw(Kw::Using);
            p.expect_kw(Kw::Compression);
            names::new_name(p, NameRule::Any, NewNameKind::Other);
        }
        Some(Kw::Generated | Kw::As) => {
            if p.eat_kw(Kw::Generated) {
                p.expect_kw(Kw::Always);
            }
            p.expect_kw(Kw::As);
            paren_expr(p);
            if !p.eat_kw(Kw::Virtual) {
                p.eat_kw(Kw::Stored);
            }
        }
        _ => p.err_recover("expected a constraint", &[]),
    }
    m.complete(p, K::COLUMN_CONSTRAINT);
}

/// `(a, b := 1, c INTEGER)`.
fn macro_params(p: &mut Parser) {
    let m = p.start();
    if !p.expect(K::L_PAREN) {
        m.abandon(p);
        return;
    }
    while !p.at(K::R_PAREN) && !p.at_eof() {
        let param = p.start();
        if !names::new_name(p, NameRule::ColId, NewNameKind::Other) {
            param.abandon(p);
            p.err_recover("expected a parameter", &[]);
            break;
        }
        if !p.at(K::COLON_EQ) && !p.at(K::COMMA) && !p.at(K::R_PAREN) {
            types::type_name(p);
        }
        if p.eat(K::COLON_EQ) && expr(p).is_none() {
            p.error("expected a default");
        }
        param.complete(p, K::MACRO_PARAM);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    p.expect(K::R_PAREN);
    m.complete(p, K::MACRO_PARAMS);
}

fn sequence_options(p: &mut Parser) {
    loop {
        if p.eat_kw(Kw::Increment) {
            p.eat_kw(Kw::By);
            signed_number(p);
        } else if p.eat_kw(Kw::Minvalue) || p.eat_kw(Kw::Maxvalue) {
            signed_number(p);
        } else if p.eat_kw(Kw::Start) {
            p.eat_kw(Kw::With);
            signed_number(p);
        } else if p.eat_kw(Kw::Cycle) {
        } else if p.eat_kw(Kw::No) {
            if !p.eat_kw(Kw::Minvalue) && !p.eat_kw(Kw::Maxvalue) {
                p.expect_kw(Kw::Cycle);
            }
        } else if p.eat_kw(Kw::Owned) {
            p.expect_kw(Kw::By);
            names::path(p, NameRule::Any, &|_, _| {});
        } else {
            break;
        }
    }
}

fn signed_number(p: &mut Parser) {
    p.eat(K::MINUS);
    if matches!(p.current(), K::INT_NUMBER | K::FLOAT_NUMBER) {
        p.bump();
    } else {
        p.error("expected a number");
    }
}

/// What DROP and COMMENT ON name, and what to offer after them.
fn object_kind(p: &mut Parser, kinds: &[Kw]) -> Option<Kw> {
    for &kw in kinds {
        if p.allows(Feature::Statement) || kw == Kw::Table {
            p.record(Expected::Keyword(kw));
        }
    }
    if p.nth_kw(0)
        .is_some_and(|kw| kw != Kw::Table && kinds.contains(&kw))
    {
        p.gate(Feature::Statement);
    }
    let kw = p.nth_kw(0).filter(|kw| kinds.contains(kw))?;
    p.bump_kw(kw);
    if kw == Kw::Macro {
        p.eat_kw(Kw::Table);
    }
    Some(kw)
}

/// Records what names a DROP / COMMENT of `kind` takes at the hole.
fn object_ref(p: &mut Parser, kind: Option<Kw>) {
    let on_hole = move |p: &mut Parser, q: Vec<Name>| match kind {
        Some(Kw::Schema) => p.record(Expected::SchemaName {
            database: q.first().cloned(),
        }),
        Some(Kw::Macro | Kw::Function) => {
            p.record(Expected::FunctionName {
                kind: FunctionKind::Macro,
                qualifier: q,
            });
        }
        Some(Kw::Type) => p.record(Expected::TypeName),
        Some(Kw::Database) => p.record(Expected::DatabaseName),
        Some(Kw::Index | Kw::Secret | Kw::Sequence) => {}
        _ => names::record_table(p, q),
    };
    if names::path(p, NameRule::NotReserved, &on_hole).is_none() {
        p.error("expected a name");
    }
}

pub(super) fn drop(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Drop);
    let kind = object_kind(
        p,
        &[
            Kw::Table,
            Kw::View,
            Kw::Index,
            Kw::Sequence,
            Kw::Schema,
            Kw::Macro,
            Kw::Function,
            Kw::Type,
            Kw::Secret,
            Kw::Database,
            Kw::Temporary,
            Kw::Persistent,
        ],
    );
    let kind = match kind {
        Some(Kw::Temporary | Kw::Persistent) => object_kind(p, &[Kw::Secret]),
        Some(kw) => Some(kw),
        None => {
            p.err_recover("expected TABLE, VIEW, SCHEMA, ...", &[]);
            m.complete(p, K::DROP_STMT);
            return;
        }
    };
    if_exists(p);
    loop {
        object_ref(p, kind);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    if kind == Some(Kw::Secret) && p.eat_kw(Kw::From) {
        names::new_name(p, NameRule::Any, NewNameKind::Other);
    }
    if !p.eat_kw(Kw::Cascade) {
        p.eat_kw(Kw::Restrict);
    }
    m.complete(p, K::DROP_STMT);
}

pub(super) fn comment(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Comment);
    p.bump_kw(Kw::On);
    let kind = object_kind(
        p,
        &[
            Kw::Table,
            Kw::Column,
            Kw::View,
            Kw::Index,
            Kw::Sequence,
            Kw::Schema,
            Kw::Macro,
            Kw::Function,
            Kw::Type,
        ],
    );
    if kind == Some(Kw::Column) {
        if names::path(p, NameRule::Any, &|p, q| {
            p.record(Expected::ColumnRef { qualifier: q })
        })
        .is_none()
        {
            p.error("expected a column");
        }
    } else {
        object_ref(p, kind);
    }
    if p.expect_kw(Kw::Is) && expr(p).is_none() {
        p.error("expected a comment");
    }
    m.complete(p, K::COMMENT_STMT);
}

pub(super) fn alter(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Alter);
    let kind = object_kind(p, &[Kw::Table, Kw::View, Kw::Sequence, Kw::Database]);
    if_exists(p);
    let t = p.start();
    object_ref(p, kind);
    t.complete(p, K::TABLE_REF);
    loop {
        alter_action(p, kind);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    m.complete(p, K::ALTER_STMT);
}

fn alter_action(p: &mut Parser, kind: Option<Kw>) {
    let m = p.start();
    let column = |p: &mut Parser| {
        if names::path(p, NameRule::Any, &|p, q| {
            p.record(Expected::ColumnRef { qualifier: q })
        })
        .is_none()
        {
            p.error("expected a column");
        }
    };
    if p.at_kw(Kw::Add) {
        p.bump_kw(Kw::Add);
        if at_table_constraint(p) {
            table_constraint(p);
        } else {
            p.eat_kw(Kw::Column);
            if_not_exists(p);
            column_def(p);
        }
    } else if p.at_kw(Kw::Drop) {
        p.bump_kw(Kw::Drop);
        p.eat_kw(Kw::Column);
        if_exists(p);
        column(p);
        if !p.eat_kw(Kw::Cascade) {
            p.eat_kw(Kw::Restrict);
        }
    } else if p.at_kw(Kw::Alter) {
        p.bump_kw(Kw::Alter);
        p.eat_kw(Kw::Column);
        column(p);
        let set_type = p.peek_kw(Kw::Set) && (p.nth_at_kw(1, Kw::Data) || p.nth_at_kw(1, Kw::Type));
        if p.at_kw(Kw::Type) || set_type {
            if p.eat_kw(Kw::Set) {
                p.eat_kw(Kw::Data);
            }
            p.expect_kw(Kw::Type);
            // ALTER col TYPE USING expr leaves the type to the expression
            if !p.peek_kw(Kw::Using) {
                types::type_name(p);
            }
            if p.eat_kw(Kw::Using) && expr(p).is_none() {
                p.error("expected an expression");
            }
        } else if p.at_kw(Kw::Set) {
            p.bump_kw(Kw::Set);
            if p.eat_kw(Kw::Default) {
                expr(p);
            } else if p.eat_kw(Kw::Not) {
                p.expect_kw(Kw::Null);
            } else {
                p.error("expected DEFAULT or NOT NULL");
            }
        } else if p.at_kw(Kw::Drop) {
            p.bump_kw(Kw::Drop);
            if p.eat_kw(Kw::Not) {
                p.expect_kw(Kw::Null);
            } else {
                p.expect_kw(Kw::Default);
            }
        } else {
            p.err_recover("expected TYPE, SET or DROP", &[]);
        }
    } else if p.at_kw(Kw::Rename) {
        p.bump_kw(Kw::Rename);
        if p.eat_kw(Kw::To) {
            names::new_name(p, NameRule::ColId, NewNameKind::Table);
        } else {
            p.eat_kw(Kw::Column);
            column(p);
            if p.expect_kw(Kw::To) {
                names::new_name(p, NameRule::ColId, NewNameKind::Column);
            }
        }
    } else if p.at_kw(Kw::Set) && kind != Some(Kw::Table) {
        // ALTER SEQUENCE / VIEW ... SET ...
        p.bump_kw(Kw::Set);
        while !p.at_eof() && !p.at(K::COMMA) {
            p.bump_one_balanced();
        }
    } else if kind == Some(Kw::Sequence) {
        sequence_options(p);
        if p.eat_kw(Kw::Restart) && p.eat_kw(Kw::With) {
            signed_number(p);
        }
    } else {
        m.abandon(p);
        p.err_recover("expected ADD, DROP, ALTER, RENAME or SET", &[]);
        return;
    }
    m.complete(p, K::ALTER_ACTION);
}
