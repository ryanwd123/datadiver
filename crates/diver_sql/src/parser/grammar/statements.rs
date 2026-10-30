//! Statement dispatch and unknown statements.

use super::super::Parser;
use super::{ddl, dml, misc, select};
use crate::complete::Expected;
use crate::{Kw, SyntaxKind as K};

/// Words a statement can start with, offered at the start of a statement.
pub(super) const STATEMENT_START: &[Kw] = &[
    Kw::Select,
    Kw::From,
    Kw::With,
    Kw::Values,
    Kw::Table,
    Kw::Insert,
    Kw::Update,
    Kw::Delete,
    Kw::Merge,
    Kw::Create,
    Kw::Drop,
    Kw::Alter,
    Kw::Copy,
    Kw::Attach,
    Kw::Detach,
    Kw::Use,
    Kw::Export,
    Kw::Import,
    Kw::Describe,
    Kw::Show,
    Kw::Summarize,
    Kw::Explain,
    Kw::Pragma,
    Kw::Set,
    Kw::Reset,
    Kw::Install,
    Kw::Load,
    Kw::Force,
    Kw::Begin,
    Kw::Start,
    Kw::Commit,
    Kw::Rollback,
    Kw::Abort,
    Kw::End,
    Kw::Checkpoint,
    Kw::Prepare,
    Kw::Execute,
    Kw::Deallocate,
    Kw::Call,
    Kw::Vacuum,
    Kw::Analyze,
    Kw::Comment,
    Kw::Truncate,
    Kw::Pivot,
    Kw::Unpivot,
];

pub(super) fn statement(p: &mut Parser) {
    if p.at_hole() {
        for &kw in STATEMENT_START {
            p.record(Expected::Keyword(kw));
        }
    }
    if p.current() == K::L_PAREN {
        select_stmt(p);
        return;
    }
    match p.nth_kw(0) {
        Some(Kw::Select | Kw::From | Kw::Values | Kw::Table) => select_stmt(p),
        Some(Kw::With) => with_stmt(p),
        Some(Kw::Insert) => dml::insert(p, None),
        Some(Kw::Update) if p.nth_at_kw(1, Kw::Extensions) => misc::update_extensions(p),
        Some(Kw::Update) => dml::update(p, None),
        Some(Kw::Delete) => dml::delete(p, None),
        Some(Kw::Merge) => dml::merge(p, None),
        Some(Kw::Create) => ddl::create(p),
        Some(Kw::Drop) => ddl::drop(p),
        Some(Kw::Alter) => ddl::alter(p),
        Some(Kw::Comment) if p.nth_at_kw(1, Kw::On) => ddl::comment(p),
        Some(Kw::Copy) => misc::copy(p),
        Some(Kw::Attach) => misc::attach(p),
        Some(Kw::Detach) => misc::detach(p),
        Some(Kw::Use) => misc::use_stmt(p),
        Some(Kw::Export) => misc::export(p),
        Some(Kw::Import) => misc::import(p),
        Some(Kw::Describe | Kw::Desc) => misc::describe(p, K::DESCRIBE_STMT),
        Some(Kw::Show) => misc::describe(p, K::SHOW_STMT),
        Some(Kw::Summarize) => misc::describe(p, K::SUMMARIZE_STMT),
        Some(Kw::Explain) => misc::explain(p),
        Some(Kw::Pragma) => misc::pragma(p),
        Some(Kw::Set) => misc::set(p),
        Some(Kw::Reset) => misc::reset(p),
        Some(Kw::Install | Kw::Load) => misc::install(p),
        Some(Kw::Force) if p.nth_at_kw(1, Kw::Install) => misc::install(p),
        Some(Kw::Force | Kw::Checkpoint) => misc::checkpoint(p),
        Some(Kw::Begin | Kw::Start | Kw::Commit | Kw::Rollback | Kw::Abort | Kw::End) => {
            misc::transaction(p)
        }
        Some(Kw::Prepare) => misc::prepare(p),
        Some(Kw::Execute) => misc::execute(p),
        Some(Kw::Deallocate) => misc::deallocate(p),
        Some(Kw::Call) => misc::call(p),
        Some(Kw::Vacuum | Kw::Analyze | Kw::Analyse) => misc::vacuum(p),
        Some(Kw::Truncate) => dml::truncate(p),
        Some(Kw::Pivot | Kw::PivotWider | Kw::Unpivot | Kw::PivotLonger) => misc::pivot_stmt(p),
        _ => unknown(p),
    }
}

fn select_stmt(p: &mut Parser) {
    let m = p.start();
    select::query(p);
    m.complete(p, K::SELECT_STMT);
}

/// `WITH ...` and then a query, INSERT, UPDATE, DELETE or MERGE.
fn with_stmt(p: &mut Parser) {
    let with = select::with_clause(p);
    if p.at_kw(Kw::Insert) {
        dml::insert(p, Some(with));
    } else if p.at_kw(Kw::Update) {
        dml::update(p, Some(with));
    } else if p.at_kw(Kw::Delete) {
        dml::delete(p, Some(with));
    } else if p.at_kw(Kw::Merge) {
        dml::merge(p, Some(with));
    } else {
        let query = with.precede(p);
        let query = select::query_rest(p, query);
        query.precede(p).complete(p, K::SELECT_STMT);
    }
}

/// A statement the grammar doesn't know: everything up to the next `;`.
fn unknown(p: &mut Parser) {
    let m = p.start();
    while !p.at_eof() {
        p.bump();
    }
    m.complete(p, K::UNKNOWN_STMT);
}
