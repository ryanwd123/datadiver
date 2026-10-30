//! Everything else: COPY, ATTACH, USE, EXPORT / IMPORT, DESCRIBE / SHOW /
//! SUMMARIZE, EXPLAIN, PRAGMA, SET / RESET, INSTALL / LOAD, transactions,
//! CHECKPOINT, PREPARE / EXECUTE / DEALLOCATE, CALL, VACUUM / ANALYZE and the
//! PIVOT / UNPIVOT statements.

use super::super::Parser;
use super::expressions::{self, expr};
use super::names::{self, NameRule};
use super::{from, select, statements};
use crate::complete::{Expected, FunctionKind, NewNameKind};
use crate::{Kw, SyntaxKind as K};

/// `(name value, name = value, ...)`, as COPY, ATTACH, EXPORT and CREATE SECRET take.
pub(super) fn option_list(p: &mut Parser) {
    let m = p.start();
    if !p.expect(K::L_PAREN) {
        m.abandon(p);
        return;
    }
    while !p.at(K::R_PAREN) && !p.at_eof() {
        let o = p.start();
        if !names::at_name(p, NameRule::Any) && !p.at_hole() {
            o.abandon(p);
            p.err_recover("expected an option", &[]);
            break;
        }
        names::new_name(p, NameRule::Any, NewNameKind::Other);
        p.eat(K::EQ);
        if !p.at(K::COMMA) && !p.at(K::R_PAREN) && expr(p).is_none() {
            p.err_recover("expected a value", &[]);
        }
        o.complete(p, K::OPTION);
        if !p.eat(K::COMMA) {
            break;
        }
    }
    if !p.eat(K::R_PAREN) {
        p.err_recover("expected ')'", &[]);
        p.eat(K::R_PAREN);
    }
    m.complete(p, K::OPTION_LIST);
}

/// A string naming a file or directory.
fn path_string(p: &mut Parser) -> bool {
    if p.current().is_string() {
        p.record_string(Expected::FilePath);
        p.bump();
        true
    } else if p.current() == K::PARAM {
        p.bump();
        true
    } else {
        false
    }
}

pub(super) fn copy(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Copy);
    if p.at_kw(Kw::From) && p.nth_at_kw(1, Kw::Database) {
        // COPY FROM DATABASE a TO b
        p.bump_kw(Kw::From);
        p.bump_kw(Kw::Database);
        db_name(p);
        p.expect_kw(Kw::To);
        db_name(p);
    } else {
        if p.current() == K::L_PAREN {
            p.bump();
            select::query(p);
            p.expect(K::R_PAREN);
        } else {
            let t = p.start();
            if names::table_path(p).is_none() {
                p.error("expected a table or a query");
            }
            t.complete(p, K::TABLE_REF);
            if p.current() == K::L_PAREN {
                names::column_list(p, true);
            }
        }
        if !p.eat_kw(Kw::To) && !p.eat_kw(Kw::From) {
            p.error("expected TO or FROM");
        }
        if !path_string(p) && names::path(p, NameRule::Any, &|_, _| {}).is_none() {
            p.error("expected a file");
        }
        p.eat_kw(Kw::With);
        if p.current() == K::L_PAREN {
            option_list(p);
        } else {
            // the old `COPY t TO 'f' DELIMITER ',' HEADER` form
            while names::at_name(p, NameRule::Any) {
                let o = p.start();
                p.bump_any_kw();
                if !names::at_name(p, NameRule::Any) && !p.at_eof() {
                    expr(p);
                }
                o.complete(p, K::OPTION);
            }
        }
    }
    m.complete(p, K::COPY_STMT);
}

fn db_name(p: &mut Parser) {
    let m = p.start();
    if p.at_hole() {
        p.record(Expected::DatabaseName);
    } else if !names::at_name(p, NameRule::ColId) {
        p.error("expected a database");
        m.abandon(p);
        return;
    }
    names::bump_name(p);
    m.complete(p, K::NAME);
}

pub(super) fn attach(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Attach);
    if p.at_phrase(&[Kw::Or, Kw::Replace]) {
        p.bump_kw(Kw::Or);
        p.expect_kw(Kw::Replace);
    }
    p.eat_kw(Kw::Database);
    if p.at_phrase(&[Kw::If, Kw::Not, Kw::Exists]) {
        p.bump_kw(Kw::If);
        p.expect_kw(Kw::Not);
        p.expect_kw(Kw::Exists);
    }
    if !path_string(p) && expr(p).is_none() {
        p.error("expected a database file");
    }
    if p.eat_kw(Kw::As) {
        names::new_name(p, NameRule::Any, NewNameKind::Database);
    }
    if p.current() == K::L_PAREN {
        option_list(p);
    }
    m.complete(p, K::ATTACH_STMT);
}

pub(super) fn detach(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Detach);
    p.eat_kw(Kw::Database);
    if p.at_phrase(&[Kw::If, Kw::Exists]) {
        p.bump_kw(Kw::If);
        p.expect_kw(Kw::Exists);
    }
    db_name(p);
    m.complete(p, K::DETACH_STMT);
}

pub(super) fn use_stmt(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Use);
    let on_hole = |p: &mut Parser, q: Vec<crate::complete::Name>| {
        if q.is_empty() {
            p.record(Expected::DatabaseName);
        }
        p.record(Expected::SchemaName {
            database: q.first().cloned(),
        });
    };
    if names::path(p, NameRule::ColId, &on_hole).is_none() {
        p.error("expected a database or schema");
    }
    m.complete(p, K::USE_STMT);
}

pub(super) fn export(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Export);
    p.expect_kw(Kw::Database);
    if !p.current().is_string() {
        db_name(p);
        p.expect_kw(Kw::To);
    }
    if !path_string(p) {
        p.error("expected a directory");
    }
    if p.current() == K::L_PAREN {
        option_list(p);
    }
    m.complete(p, K::EXPORT_STMT);
}

pub(super) fn import(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Import);
    p.expect_kw(Kw::Database);
    if !path_string(p) {
        p.error("expected a directory");
    }
    m.complete(p, K::IMPORT_STMT);
}

/// DESCRIBE, SHOW and SUMMARIZE: of a table, a query, or (SHOW) a list.
pub(super) fn describe(p: &mut Parser, kind: K) {
    let m = p.start();
    p.bump_any_kw();
    if kind == K::SHOW_STMT {
        if p.at_kw(Kw::All) && p.nth_at_kw(1, Kw::Tables) {
            p.bump_kw(Kw::All);
            p.bump_kw(Kw::Tables);
            m.complete(p, kind);
            return;
        }
        if p.at_kw(Kw::Tables) || p.at_kw(Kw::Databases) {
            p.bump_any_kw();
            if p.eat_kw(Kw::From) {
                names::path(p, NameRule::Any, &|p, q| {
                    p.record(Expected::SchemaName {
                        database: q.first().cloned(),
                    })
                });
            }
            m.complete(p, kind);
            return;
        }
    }
    if select::at_query_start(p, true) {
        select::query(p);
    } else {
        p.eat_kw(Kw::Table);
        if p.at_hole()
            || names::at_name(p, NameRule::TypeFunc)
            || names::at_name(p, NameRule::ColId)
        {
            from_item_like(p);
        } else if p.current().is_string() {
            let f = p.start();
            path_string(p);
            f.complete(p, K::FILE_REF);
        } else if !p.at_eof() {
            p.err_recover("expected a table or a query", &[]);
        }
    }
    m.complete(p, kind);
}

/// A table or table function, as DESCRIBE and SUMMARIZE take.
fn from_item_like(p: &mut Parser) {
    let m = p.start();
    let on_hole = |p: &mut Parser, q: Vec<crate::complete::Name>| {
        names::record_table(p, q.clone());
        p.record(Expected::FunctionName {
            kind: FunctionKind::Table,
            qualifier: q,
        });
    };
    names::path(p, NameRule::Any, &on_hole);
    if p.current() == K::L_PAREN {
        expressions::arg_list(p, true);
        m.complete(p, K::TABLE_FUNCTION);
    } else {
        m.complete(p, K::TABLE_REF);
    }
}

pub(super) fn explain(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Explain);
    if !p.eat_kw(Kw::Analyze) {
        p.eat_kw(Kw::Analyse);
    }
    if p.current() == K::L_PAREN && !select::at_query_start(p, true) {
        option_list(p);
    }
    statements::statement(p);
    m.complete(p, K::EXPLAIN_STMT);
}

pub(super) fn pragma(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Pragma);
    let n = p.start();
    if p.at_hole() {
        p.record(Expected::PragmaName);
    } else if !names::at_name(p, NameRule::Any) {
        n.abandon(p);
        p.error("expected a pragma");
        m.complete(p, K::PRAGMA_STMT);
        return;
    }
    names::bump_name(p);
    n.complete(p, K::NAME);
    if p.eat(K::EQ) {
        if expr(p).is_none() {
            p.error("expected a value");
        }
    } else if p.current() == K::L_PAREN {
        expressions::arg_list(p, false);
    }
    m.complete(p, K::PRAGMA_STMT);
}

/// `[GLOBAL | SESSION | LOCAL] name` of SET and RESET; returns false for `VARIABLE`.
fn setting_name(p: &mut Parser) -> bool {
    if p.at_kw(Kw::Variable) {
        p.bump_kw(Kw::Variable);
        names::new_name(p, NameRule::Any, NewNameKind::Variable);
        return false;
    }
    if !p.eat_kw(Kw::Global) && !p.eat_kw(Kw::Session) {
        p.eat_kw(Kw::Local);
    }
    let n = p.start();
    if p.at_hole() {
        p.record(Expected::SettingName);
    } else if !names::at_name(p, NameRule::Any) {
        n.abandon(p);
        p.error("expected a setting");
        return true;
    }
    names::bump_name(p);
    while p.current() == K::DOT && names::nth_is_word(p, 1) {
        p.bump();
        names::bump_name(p);
    }
    n.complete(p, K::NAME);
    true
}

pub(super) fn set(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Set);
    if p.eat_kw(Kw::Transaction) || p.peek_kw(Kw::Session) && p.nth_at_kw(1, Kw::Characteristics) {
        while !p.at_eof() {
            p.bump_any_kw();
        }
        m.complete(p, K::SET_STMT);
        return;
    }
    setting_name(p);
    if !p.eat(K::EQ) && !p.eat_kw(Kw::To) {
        p.error("expected '=' or TO");
    }
    if !p.at_eof() {
        expressions::expr_list(p);
    }
    m.complete(p, K::SET_STMT);
}

pub(super) fn reset(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Reset);
    if !p.eat_kw(Kw::All) {
        setting_name(p);
    }
    m.complete(p, K::RESET_STMT);
}

/// `[FORCE] INSTALL ext [FROM repo]` and `LOAD ext`.
pub(super) fn install(p: &mut Parser) {
    let m = p.start();
    p.eat_kw(Kw::Force);
    let install = p.peek_kw(Kw::Install);
    p.bump_any_kw();
    let n = p.start();
    if p.at_hole() {
        p.record(Expected::ExtensionName);
        names::bump_name(p);
    } else if p.current().is_string() {
        p.record_string(Expected::ExtensionName);
        if !install {
            p.record_string(Expected::FilePath);
        }
        p.bump();
    } else if names::at_name(p, NameRule::Any) {
        names::bump_name(p);
    } else {
        p.error("expected an extension");
    }
    n.complete(p, K::NAME);
    if install && p.eat_kw(Kw::From) && !path_string(p) {
        names::new_name(p, NameRule::Any, NewNameKind::Other);
    }
    if install && p.at_kw(Kw::Version) {
        p.bump_kw(Kw::Version);
        expr(p);
    }
    m.complete(
        p,
        if install {
            K::INSTALL_STMT
        } else {
            K::LOAD_STMT
        },
    );
}

/// `UPDATE EXTENSIONS [(name, ...)]`.
pub(super) fn update_extensions(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Update);
    p.bump_kw(Kw::Extensions);
    if p.eat(K::L_PAREN) {
        loop {
            if p.at_hole() {
                p.record(Expected::ExtensionName);
            }
            if !names::new_name(p, NameRule::Any, NewNameKind::Other) || !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
    }
    m.complete(p, K::INSTALL_STMT);
}

pub(super) fn checkpoint(p: &mut Parser) {
    let m = p.start();
    p.eat_kw(Kw::Force);
    p.expect_kw(Kw::Checkpoint);
    if !p.at_eof() {
        db_name(p);
    }
    m.complete(p, K::CHECKPOINT_STMT);
}

pub(super) fn transaction(p: &mut Parser) {
    let m = p.start();
    p.bump_any_kw();
    if !p.eat_kw(Kw::Transaction) {
        p.eat_kw(Kw::Work);
    }
    if p.eat_kw(Kw::Read) && !p.eat_kw(Kw::Only) {
        p.expect_kw(Kw::Write);
    }
    m.complete(p, K::TRANSACTION_STMT);
}

pub(super) fn prepare(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Prepare);
    names::new_name(p, NameRule::ColId, NewNameKind::Other);
    if p.current() == K::L_PAREN {
        p.bump();
        loop {
            if super::types::type_name(p).is_none() || !p.eat(K::COMMA) {
                break;
            }
        }
        p.expect(K::R_PAREN);
    }
    p.expect_kw(Kw::As);
    statements::statement(p);
    m.complete(p, K::PREPARE_STMT);
}

pub(super) fn execute(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Execute);
    names::new_name(p, NameRule::ColId, NewNameKind::Other);
    if p.current() == K::L_PAREN {
        expressions::arg_list(p, false);
    }
    m.complete(p, K::EXECUTE_STMT);
}

pub(super) fn deallocate(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Deallocate);
    p.eat_kw(Kw::Prepare);
    if !p.eat_kw(Kw::All) {
        names::new_name(p, NameRule::ColId, NewNameKind::Other);
    }
    m.complete(p, K::DEALLOCATE_STMT);
}

pub(super) fn call(p: &mut Parser) {
    let m = p.start();
    p.bump_kw(Kw::Call);
    let f = p.start();
    let on_hole = |p: &mut Parser, q: Vec<crate::complete::Name>| {
        p.record(Expected::FunctionName {
            kind: FunctionKind::Table,
            qualifier: q,
        });
    };
    if names::path(p, NameRule::Any, &on_hole).is_none() {
        p.error("expected a function");
    }
    if p.current() == K::L_PAREN {
        expressions::arg_list(p, false);
    } else {
        p.error("expected '('");
    }
    f.complete(p, K::TABLE_FUNCTION);
    m.complete(p, K::CALL_STMT);
}

/// `VACUUM [ANALYZE] [table [(cols)]]` and `ANALYZE [table [(cols)]]`.
pub(super) fn vacuum(p: &mut Parser) {
    let m = p.start();
    let vacuum = p.peek_kw(Kw::Vacuum);
    p.bump_any_kw();
    if vacuum {
        p.eat_kw(Kw::Full);
        if !p.eat_kw(Kw::Analyze) {
            p.eat_kw(Kw::Analyse);
        }
    }
    p.eat_kw(Kw::Verbose);
    if !p.at_eof() {
        let t = p.start();
        if names::table_path(p).is_none() {
            p.error("expected a table");
        }
        t.complete(p, K::TABLE_REF);
        if p.current() == K::L_PAREN {
            names::column_list(p, true);
        }
    }
    m.complete(
        p,
        if vacuum {
            K::VACUUM_STMT
        } else {
            K::ANALYZE_STMT
        },
    );
}

/// `PIVOT t ON cols [IN (...)] USING aggs [GROUP BY ...]` and
/// `UNPIVOT t ON cols INTO NAME n VALUE v`, with the query tail after them.
pub(super) fn pivot_stmt(p: &mut Parser) {
    let m = p.start();
    let unpivot = p.peek_kw(Kw::Unpivot) || p.peek_kw(Kw::PivotLonger);
    p.bump_any_kw();
    if from::from_item(p, &[Kw::On, Kw::Using, Kw::Into]).is_none() {
        p.error("expected a table");
    }
    if p.at_kw(Kw::On) {
        let on = p.start();
        p.bump_kw(Kw::On);
        loop {
            let item = p.start();
            if p.eat_kw(Kw::Columns) {
                expressions::arg_list(p, false);
            } else if expr(p).is_none() {
                item.abandon(p);
                p.err_recover("expected a column", &[Kw::Using, Kw::Into]);
                break;
            }
            if p.at_kw(Kw::In) {
                p.bump_kw(Kw::In);
                if p.current() == K::L_PAREN {
                    expressions::paren_or_subquery(p);
                } else {
                    expr(p);
                }
            }
            select::select_alias(p);
            item.complete(p, K::SELECT_ITEM);
            if !p.eat(K::COMMA) {
                break;
            }
        }
        on.complete(p, K::PIVOT_ON);
    }
    if !unpivot && p.at_kw(Kw::Using) {
        let u = p.start();
        p.bump_kw(Kw::Using);
        from::pivot_aggregates(p);
        u.complete(p, K::PIVOT_USING);
    }
    if unpivot && p.at_kw(Kw::Into) {
        p.bump_kw(Kw::Into);
        p.expect_kw(Kw::Name);
        names::new_name(p, NameRule::Any, NewNameKind::Column);
        if p.expect_kw(Kw::Value) {
            loop {
                if !names::new_name(p, NameRule::Any, NewNameKind::Column) || !p.eat(K::COMMA) {
                    break;
                }
            }
        }
    }
    if p.at_phrase(&[Kw::Group, Kw::By]) {
        let g = p.start();
        p.bump_kw(Kw::Group);
        p.expect_kw(Kw::By);
        expressions::expr_list(p);
        g.complete(p, K::GROUP_BY_CLAUSE);
    }
    if p.at_phrase(&[Kw::Order, Kw::By]) {
        select::order_by(p);
    }
    if p.at_kw(Kw::Limit) {
        let l = p.start();
        p.bump_kw(Kw::Limit);
        expr(p);
        l.complete(p, K::LIMIT_CLAUSE);
    }
    m.complete(p, K::PIVOT_STMT);
}
