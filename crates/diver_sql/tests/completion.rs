//! Completion: the plan's acceptance cases as snapshots of the context, and
//! Dabbler's completion cases as label checks against a mock catalog.

mod common;

use common::{context, labels, render};
use expect_test::{Expect, expect};

fn check(sql: &str, expect: Expect) {
    expect.assert_eq(&render(&context(sql)));
}

#[test]
fn create_or_replace() {
    check(
        "CREATE OR REPLACE |",
        expect![[r#"
        expected: TEMP, TEMPORARY, PERSISTENT, TABLE, VIEW, MACRO, FUNCTION, TYPE, SEQUENCE, SCHEMA, SECRET
        replace: 18..18 ""
    "#]],
    );
}

#[test]
fn drop_table() {
    check(
        "DROP TABLE |",
        expect![[r#"
        expected: IF EXISTS, TableRef, CteName, SchemaName, DatabaseName
        replace: 11..11 ""
    "#]],
    );
}

#[test]
fn drop_table_qualified() {
    check(
        "DROP TABLE main.|",
        expect![[r#"
        expected: TableRef(main), SchemaName(main)
        replace: 16..16 ""
    "#]],
    );
}

#[test]
fn statement_start_prefix() {
    check(
        "SEL|",
        expect![[r#"
        expected: SELECT, FROM, WITH, VALUES, TABLE, INSERT, UPDATE, DELETE, MERGE, CREATE, DROP, ALTER, COPY, ATTACH, DETACH, USE, EXPORT, IMPORT, DESCRIBE, SHOW, SUMMARIZE, EXPLAIN, PRAGMA, SET, RESET, INSTALL, LOAD, FORCE, BEGIN, START, COMMIT, ROLLBACK, ABORT, END, CHECKPOINT, PREPARE, EXECUTE, DEALLOCATE, CALL, VACUUM, ANALYZE, COMMENT, TRUNCATE, PIVOT, UNPIVOT
        replace: 0..3 "SEL"
    "#]],
    );
}

#[test]
fn select_list() {
    check(
        "SELECT | FROM orders o",
        expect![[r#"
        expected: DISTINCT, ALL, Expr, CASE, CAST, TRY_CAST, NOT, EXISTS, NULL, TRUE, FALSE, INTERVAL, LAMBDA, COLUMNS, ColumnRef, Function(Any)
        replace: 7..7 ""
        scope: orders AS o
    "#]],
    );
}

#[test]
fn qualified_column() {
    check(
        "SELECT o.| FROM orders o JOIN c ON o.id = c.id",
        expect![[r#"
        expected: ColumnRef(o), Function(Any, o)
        replace: 9..9 ""
        scope: orders AS o, c
    "#]],
    );
}

#[test]
fn from_first() {
    check(
        "FROM orders SELECT |",
        expect![[r#"
        expected: DISTINCT, ALL, Expr, CASE, CAST, TRY_CAST, NOT, EXISTS, NULL, TRUE, FALSE, INTERVAL, LAMBDA, COLUMNS, ColumnRef, Function(Any)
        replace: 19..19 ""
        scope: orders
    "#]],
    );
}

#[test]
fn star_exclude() {
    check(
        "SELECT * EXCLUDE (|) FROM t",
        expect![[r#"
        expected: ColumnRef
        replace: 18..18 ""
        scope: t
    "#]],
    );
}

#[test]
fn after_where_condition() {
    check(
        "SELECT a FROM t WHERE a = 1 |",
        expect![[r#"
        expected: AND, OR, IS, IN, NOT, BETWEEN, LIKE, ILIKE, GLOB, COLLATE, ISNULL, NOTNULL, SIMILAR TO, AT TIME ZONE, GROUP BY, GROUP BY ALL, HAVING, WINDOW, QUALIFY, USING SAMPLE, UNION, EXCEPT, INTERSECT, ORDER BY, ORDER BY ALL, LIMIT, OFFSET, FETCH
        replace: 28..28 ""
    "#]],
    );
}

#[test]
fn group_by() {
    check(
        "SELECT a FROM t GROUP BY |",
        expect![[r#"
        expected: ALL, GROUPING, CUBE, ROLLUP, Expr, CASE, CAST, TRY_CAST, NOT, EXISTS, NULL, TRUE, FALSE, INTERVAL, LAMBDA, COLUMNS, ColumnRef, Function(Any)
        replace: 25..25 ""
        scope: t
    "#]],
    );
}

#[test]
fn cte_columns() {
    check(
        "WITH x AS (SELECT a, b AS c FROM t) SELECT | FROM x",
        expect![[r#"
        expected: DISTINCT, ALL, Expr, CASE, CAST, TRY_CAST, NOT, EXISTS, NULL, TRUE, FALSE, INTERVAL, LAMBDA, COLUMNS, ColumnRef, Function(Any)
        replace: 43..43 ""
        scope: cte x [a, c]
        ctes: x
    "#]],
    );
}

#[test]
fn read_parquet_path() {
    check(
        "SELECT * FROM read_parquet('|",
        expect![[r#"
        expected: FilePath
        replace: 28..28 ""
        in_string
    "#]],
    );
}

#[test]
fn from_string() {
    check(
        "SELECT * FROM '|",
        expect![[r#"
        expected: FilePath
        replace: 15..15 ""
        in_string
    "#]],
    );
}

#[test]
fn second_statement() {
    check(
        "SELECT * FROM t; DROP |",
        expect![[r#"
        expected: TABLE, VIEW, INDEX, SEQUENCE, SCHEMA, MACRO, FUNCTION, TYPE, SECRET, DATABASE, TEMPORARY, PERSISTENT
        replace: 22..22 ""
    "#]],
    );
}

#[test]
fn in_comment() {
    check(
        "SELECT -- comment |",
        expect![[r#"
        expected: 
        replace: 18..18 ""
        in_comment
    "#]],
    );
}

#[test]
fn insert_columns() {
    check(
        "INSERT INTO t (|",
        expect![[r#"
        expected: ColumnRef
        replace: 15..15 ""
        scope: t
    "#]],
    );
}

#[test]
fn set() {
    check(
        "SET |",
        expect![[r#"
            expected: TIME ZONE, TRANSACTION, VARIABLE, GLOBAL, SESSION, LOCAL, SettingName
            replace: 4..4 ""
        "#]],
    );
}

#[test]
fn cast_type() {
    check(
        "SELECT CAST(x AS |",
        expect![[r#"
        expected: TypeName
        replace: 17..17 ""
    "#]],
    );
}

#[test]
fn quoted_partial() {
    check(
        "SELECT \"my ta| FROM t",
        expect![[r#"
        expected: DISTINCT, ALL, Expr, CASE, CAST, TRY_CAST, NOT, EXISTS, NULL, TRUE, FALSE, INTERVAL, LAMBDA, COLUMNS, ColumnRef, Function(Any)
        replace: 7..13 "my ta"
        scope: t
    "#]],
    );
}

/// `includes` must all be offered, `excludes` none, after prefix filtering.
fn case(sql: &str, includes: &[&str], excludes: &[&str]) -> Option<String> {
    let got = labels(sql);
    let missing: Vec<&&str> = includes
        .iter()
        .filter(|x| !got.iter().any(|g| g == **x))
        .collect();
    let unexpected: Vec<&&str> = excludes
        .iter()
        .filter(|x| got.iter().any(|g| g == **x))
        .collect();
    if missing.is_empty() && unexpected.is_empty() {
        None
    } else {
        Some(format!(
            "{sql}\n    missing={missing:?} unexpected={unexpected:?}\n    got={:?}",
            &got[..got.len().min(30)]
        ))
    }
}

fn run(cases: &[(&str, &[&str], &[&str])]) {
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(sql, inc, exc)| case(sql, inc, exc))
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

const ORDERS: &[&str] = &["order_id", "customer_id", "amount", "\"order date\""];
const CUSTOMERS: &[&str] = &["customer_id", "name", "region"];

fn cat<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<&'a str> {
    a.iter().chain(b).copied().collect()
}

#[test]
fn dabbler_columns() {
    let orders_plus_name = cat(ORDERS, &["name"]);
    run(&[
        ("select o.| from orders o", ORDERS, &["name", "region"]),
        (
            "select o.order_id, c.| from orders o join customers c on o.customer_id = c.customer_id",
            CUSTOMERS,
            &["amount"],
        ),
        (
            "select * from orders o join customers c on o.|",
            ORDERS,
            &[],
        ),
        ("select * from orders o where o.|", ORDERS, &[]),
        (
            "select * from orders o\nwhere o.customer_id = 1\n  and o.|",
            ORDERS,
            &[],
        ),
        ("select * from orders o group by o.|", ORDERS, &[]),
        ("select * from orders o order by o.|", ORDERS, &[]),
        (
            "select count(*) from orders o group by all having o.|",
            ORDERS,
            &[],
        ),
        (
            "select * from orders o qualify row_number() over (partition by o.|) = 1",
            ORDERS,
            &[],
        ),
        (
            "select * from orders o, customers c where c.|",
            CUSTOMERS,
            &[],
        ),
        ("create table x as select o.| from orders o", ORDERS, &[]),
        (
            "select o.| from orders o; select c.name from customers c",
            ORDERS,
            &[],
        ),
        (
            "select * from orders; select c.| from customers c",
            CUSTOMERS,
            &[],
        ),
        ("select * from test_view t where t.|", ORDERS, &[]),
        (
            "select * from test_schema.schema_table s where s.|",
            &["column1", "column2"],
            &[],
        ),
        (
            "select * from test_schema.\"org.category/dotted_view\" v where v.|",
            CUSTOMERS,
            &["amount"],
        ),
        (
            "select reg| from test_schema.\"org.category/dotted_view\"",
            &["region"],
            &[],
        ),
        (
            "select test_schema.\"org.category/dotted_view\".| from test_schema.\"org.category/dotted_view\"",
            CUSTOMERS,
            &[],
        ),
        (
            "select schema_table.| from test_schema.schema_table",
            &["column1", "column2"],
            &[],
        ),
        (
            "select * from test_schema.schema_table where test_schema.schema_table.|",
            &["column1", "column2"],
            &[],
        ),
        (
            "select \"org.category/dotted_view\".| from test_schema.\"org.category/dotted_view\"",
            CUSTOMERS,
            &[],
        ),
        (
            "delete from test_schema.schema_table where schema_table.|",
            &["column1", "column2"],
            &[],
        ),
        ("select am| from orders", &["amount"], &["name"]),
        ("select * from orders where am|", &["amount"], &[]),
        (
            "select cu| from orders o join customers c on o.customer_id = c.customer_id",
            &["customer_id"],
            &[],
        ),
        (
            "select cu| from orders o, customers c",
            &["customer_id"],
            &[],
        ),
        (
            "select * from orders o where o.amount in (select am| from orders)",
            &["amount"],
            &[],
        ),
        ("SELECT O.| FROM ORDERS O", ORDERS, &[]),
        ("select * from \"orders\" o where o.|", ORDERS, &[]),
        ("select o.amount, sum(o.| from orders o", ORDERS, &[]),
        (
            "select cu| from orders o join customers c using (customer_id)",
            &["customer_id"],
            &[],
        ),
        (
            "select * from orders o join customers c using (customer_id) where c.|",
            CUSTOMERS,
            &[],
        ),
        (
            "select c.name, o.| from customers c, lateral (select order_id, amount from orders where customer_id = c.customer_id) o",
            &["order_id", "amount"],
            &["name"],
        ),
        (
            "select c.name, o.| from customers c join lateral (select order_id as oid from orders) o on true",
            &["oid"],
            &[],
        ),
        (
            "select c.name, o.| from customers c, lateral (select * from orders where customer_id = c.customer_id) o",
            ORDERS,
            &[],
        ),
        (
            "select c.name, o.| from customers c, (select * from orders where customer_id = c.customer_id) o",
            ORDERS,
            &[],
        ),
        (
            "select o.| from customers c join lateral (select o2.*, c.name from orders o2 where o2.customer_id = c.customer_id) o on true",
            &orders_plus_name,
            &[],
        ),
        ("select * from orders o qualify o.|", ORDERS, &[]),
        (
            "select * from orders o qualify row_number() over (order by o.amount) = 1 order by o.|",
            ORDERS,
            &[],
        ),
        ("select * from orders \"O\" where o.|", ORDERS, &[]),
        ("select o.| from main.orders o", ORDERS, &[]),
        ("select o.| from MEMORY.MAIN.ORDERS o", ORDERS, &[]),
        ("select sum(o.amount), count(o.| from orders o", ORDERS, &[]),
    ]);
}

#[test]
fn dabbler_derived_tables() {
    run(&[
        (
            "select s.| from (select order_id as oid, amount from orders) s",
            &["oid", "amount"],
            &["order_id", "customer_id"],
        ),
        (
            "with x as (select order_id, amount from orders) select x.| from x",
            &["order_id", "amount"],
            &["customer_id"],
        ),
        (
            "with x as (select order_id, amount from orders) select * from |",
            &["x", "orders"],
            &[],
        ),
        (
            "with cte_1 as materialized (select col1, col2 as c2, col3 as \"Column 3\" from bbb)\nselect a.| from cte_1 a",
            &["col1", "c2", "\"Column 3\""],
            &[],
        ),
        (
            "with cte_a as (select order_id from orders), cte_b as (select * from cte_|)",
            &["cte_a"],
            &[],
        ),
        (
            "with a as (select order_id from orders), b as (select * from |)",
            &["a", "orders"],
            &[],
        ),
    ]);
}

#[test]
fn dabbler_tables() {
    run(&[
        (
            "select * from |",
            &[
                "orders",
                "customers",
                "test_view",
                "test_schema",
                "main",
                "attached_db",
                "my_table_macro",
            ],
            &[],
        ),
        ("select * from ord|", &["orders"], &["customers"]),
        (
            "select * from orders o join |",
            &["orders", "customers"],
            &[],
        ),
        ("select * from orders o left join |", &["customers"], &[]),
        ("insert into |", &["orders"], &[]),
        ("delete from |", &["orders"], &[]),
        ("describe |", &["orders"], &[]),
        ("summarize |", &["orders"], &[]),
        ("truncate |", &["orders"], &[]),
        ("create index i on |", &["orders"], &[]),
        ("exp|", &["EXPLAIN"], &[]),
        (
            "select * from test_schema.|",
            &["schema_table", "test_view2"],
            &["orders"],
        ),
        (
            "select * from main.|",
            &["orders", "customers"],
            &["schema_table"],
        ),
        ("select * from attached_db.|", &["attached_schema"], &[]),
        (
            "select * from attached_db.attached_schema.|",
            &["db_table"],
            &[],
        ),
        (
            "select * from v.|",
            &["my_table_macro_in_v", "test_view3"],
            &[],
        ),
        ("select * from read_c|", &["read_csv", "read_csv_auto"], &[]),
    ]);
}

#[test]
fn dabbler_functions_and_keywords() {
    run(&[
        ("select su|(amount) from orders", &["sum", "substring"], &[]),
        ("select add|", &["addition"], &[]),
        ("select v.|", &["add"], &[]),
        ("s|", &["SELECT", "SET"], &[]),
        ("w|", &["WITH"], &[]),
        ("select * f|", &["FROM"], &[]),
        ("select * from orders o j|", &["JOIN"], &[]),
        ("select * from orders o l|", &["LEFT", "LIMIT"], &[]),
        ("select * from orders o join customers c o|", &["ON"], &[]),
        ("select * from orders w|", &["WHERE"], &[]),
        (
            "select * from orders o where o.amount > 1 a|",
            &["AND"],
            &[],
        ),
        (
            "select * from orders o where o.amount i|",
            &["IN", "IS", "ILIKE"],
            &[],
        ),
        (
            "select c1, sum(c2) from abc g|",
            &["GROUP BY", "GROUP BY ALL"],
            &[],
        ),
        (
            "select * from orders o order by o.amount d|",
            &["DESC"],
            &[],
        ),
        ("select * from orders u|", &["UNION", "USING SAMPLE"], &[]),
        (
            "select * from orders o where o.amount > 1 q|",
            &["QUALIFY"],
            &[],
        ),
    ]);
}

#[test]
fn dabbler_special_contexts() {
    run(&[
        ("install |", &["httpfs", "parquet"], &[]),
        ("load |", &["httpfs"], &[]),
        ("pragma |", &["version", "database_size"], &[]),
        ("set |", &["max_memory", "threads", "SESSION"], &[]),
        (
            "set global |",
            &["max_memory", "threads"],
            &["SESSION", "GLOBAL"],
        ),
        ("SET Session |", &["threads"], &[]),
        ("reset local |", &["threads"], &[]),
        ("set thr|", &["threads"], &[]),
        ("select 1;\nset global |", &["threads"], &[]),
        ("set variable |", &[], &["threads"]),
        (
            "select amount::| from orders",
            &["DOUBLE", "VARCHAR", "DATE"],
            &[],
        ),
        ("select cast(amount as |) from orders", &["DOUBLE"], &[]),
        (
            "select try_cast(sum(amount) as |) from orders",
            &["DOUBLE"],
            &[],
        ),
        ("create table t (a |", &["INTEGER", "VARCHAR"], &[]),
        (
            "select * from read_parquet('|')",
            &["data/", "data/orders.parquet"],
            &[],
        ),
        ("select * from 'data/|'", &["data/orders.parquet"], &[]),
    ]);
}

#[test]
fn dabbler_parser_contexts() {
    run(&[
        (
            "select extract(year from |) from orders o",
            &[],
            &["orders", "customers"],
        ),
        (
            "select extract(year from o.|) from orders o",
            &["amount"],
            &[],
        ),
        (
            "select * from orders where note = 'shipped from |'",
            &[],
            &["orders"],
        ),
        ("select * from orders o -- join |", &[], &["customers"]),
        (
            "select \"Order Lines\".| from \"Order Lines\"",
            &["line_id", "qty"],
            &[],
        ),
        (
            "select ol.| from \"Order Lines\" ol",
            &["line_id", "qty"],
            &[],
        ),
        ("update |", &["orders", "customers"], &[]),
        ("update orders set am|", &["amount"], &[]),
        (
            "select * from orders o where o.amount = cu|",
            &["customer_id"],
            &[],
        ),
        ("select case when am| from orders", &["amount"], &[]),
        (
            "select case when amount > 1 then am| from orders",
            &["amount"],
            &[],
        ),
        (
            "select * from orders where amount between 1 + 1 and am|",
            &["amount"],
            &[],
        ),
        (
            "select [x + 1 for x in [1, 2] if x > am|] from orders",
            &["amount"],
            &[],
        ),
        (
            "select list_transform([1], lambda x: x + am|) from orders",
            &["amount"],
            &[],
        ),
        (
            "select o.amount::| from orders o",
            &["INTEGER", "VARCHAR"],
            &["orders"],
        ),
        (
            "select * from read_csv('x.csv', header := true) r where r.| ",
            &[],
            &["orders"],
        ),
    ]);
}

#[test]
fn dabbler_scopes() {
    run(&[
        (
            "select * from orders o where exists (select 1 from customers c where c.customer_id = o.|)",
            ORDERS,
            &[],
        ),
        (
            "select o.order_id, (select max(c.name) from customers c where c.customer_id = o.|) from orders o",
            ORDERS,
            &[],
        ),
        (
            "select * from orders o where exists (select 1 from customers c where c.customer_id = cu|)",
            &["customer_id"],
            &[],
        ),
        (
            "select * from orders o where o.customer_id in (select c.| from customers c)",
            CUSTOMERS,
            &["amount"],
        ),
        (
            "select * from orders o where exists (select 1 from customers o where o.|)",
            CUSTOMERS,
            &["amount"],
        ),
        (
            "select c.| from orders o where exists (select 1 from customers c)",
            &[],
            &["name"],
        ),
        (
            "with x as (select order_id from orders) select * from customers c where exists (select 1 from x where x.order_id = c.|)",
            CUSTOMERS,
            &[],
        ),
        (
            "with x as (select order_id from orders) select * from x where exists (select 1 from customers c where c.customer_id = x.|)",
            &["order_id"],
            &[],
        ),
        (
            "select t.| from (select 1, 2) t(a, b)",
            &["a", "b"],
            &["1", "2"],
        ),
        (
            "select o.| from orders o(id, cust)",
            &["id", "cust", "amount"],
            &["order_id"],
        ),
        (
            "with v(k1, k2) as (select 1, 2) select v.| from v",
            &["k1", "k2"],
            &[],
        ),
        (
            "with v(k1) as (select 1), w as (select k1 from v) select w.| from w",
            &["k1"],
            &[],
        ),
    ]);
}

#[test]
fn dabbler_resolver() {
    run(&[
        (
            "with x as (select * exclude (amount) from orders) select x.| from x",
            &["order_id", "customer_id", "\"order date\""],
            &["amount"],
        ),
        (
            "with x as (select * replace (amount * 2 as amount) from orders) select x.| from x",
            ORDERS,
            &[],
        ),
        (
            "with x as (select o.*, c.name from orders o join customers c using (customer_id)) select x.| from x",
            &cat(ORDERS, &["name"]),
            &["region"],
        ),
        (
            "with x as (select columns(* exclude (amount)) from orders) select x.| from x",
            &["order_id"],
            &["amount"],
        ),
        (
            "with x as (select * rename (amount as total) from orders) select x.| from x",
            &["order_id", "total"],
            &["amount"],
        ),
        (
            "with x as (select o.* exclude (o.amount) from orders o) select x.| from x",
            &["order_id"],
            &["amount"],
        ),
        (
            "with x as (select * exclude amount from orders) select x.| from x",
            &["order_id"],
            &["amount"],
        ),
        (
            "with x as (select order_id as id from orders union all select customer_id from customers) select x.| from x",
            &["id"],
            &["customer_id"],
        ),
        (
            "with recursive r(n) as (select 1 union all select n + 1 from r where n < 3) select r.| from r",
            &["n"],
            &[],
        ),
        ("select v.| from (values (1, 2)) v(a, b)", &["a", "b"], &[]),
        ("select v.| from (values (1, 2)) v", &["col0", "col1"], &[]),
        ("select r.| from range(3) r", &["range"], &[]),
        (
            "with x as (select * from range(3)) select x.| from x",
            &["range"],
            &[],
        ),
        (
            "select r.| from recent_orders(5) r",
            &["order_id", "total"],
            &["amount"],
        ),
        (
            "with x as (select * from recent_orders(5)) select x.| from x",
            &["order_id", "total"],
            &[],
        ),
        (
            "with x as (select order_id, amount * 2 from orders) select x.| from x",
            &["order_id"],
            &[],
        ),
        (
            "select t.| from (select 1, amount * 2, order_id from orders) t(a, b)",
            &["a", "b", "order_id"],
            &[],
        ),
    ]);
}

#[test]
fn dabbler_dml_scopes() {
    run(&[
        ("update orders o set amount = o.| where true", ORDERS, &[]),
        (
            "update orders set amount = 1 where cu|",
            &["customer_id"],
            &[],
        ),
        (
            "update orders set amount = c.| from customers c",
            CUSTOMERS,
            &[],
        ),
        ("delete from orders o where o.|", ORDERS, &[]),
        ("delete from orders where cu|", &["customer_id"], &[]),
        (
            "delete from orders o using customers c where c.|",
            CUSTOMERS,
            &[],
        ),
        (
            "insert into customers (select o.| from orders o)",
            ORDERS,
            &[],
        ),
        (
            "with s as (select customer_id as cid from customers) insert into orders select s.| from s",
            &["cid"],
            &[],
        ),
        ("from orders o where o.|", ORDERS, &[]),
        ("select * from orders o, lateral range(o.|)", ORDERS, &[]),
        (
            "merge into orders o using customers c on o.| when matched then delete",
            ORDERS,
            &[],
        ),
        (
            "merge into orders o using customers c on o.customer_id = c.| when matched then delete",
            CUSTOMERS,
            &[],
        ),
        (
            "merge into orders o using customers c on o.customer_id = c.customer_id when matched then update set amount = c.|",
            CUSTOMERS,
            &[],
        ),
        (
            "with s as (select customer_id as cid from customers) merge into orders o using s on o.customer_id = s.| when matched then delete",
            &["cid"],
            &[],
        ),
        (
            "merge into orders using (select customer_id as cid from customers) s on orders.customer_id = s.| when matched then delete",
            &["cid"],
            &[],
        ),
        ("explain select o.| from orders o", ORDERS, &[]),
        ("describe select o.| from orders o", ORDERS, &[]),
        ("insert into orders (|", &["order_id", "amount"], &[]),
        ("alter table orders alter |", &["amount"], &[]),
        ("create index i on orders (|)", &["amount"], &[]),
        ("insert into orders select 1 returning |", &["amount"], &[]),
    ]);
}

#[test]
fn dabbler_aliases() {
    run(&[
        (
            "select\n    1 + 1 as column_1,\n    1 + 1 as column_2,\n    col|",
            &["column_1", "column_2"],
            &[],
        ),
        (
            "select amount * 2 as doubled, dou| from orders",
            &["doubled"],
            &[],
        ),
        (
            "select dou|, amount * 2 as doubled from orders",
            &[],
            &["doubled"],
        ),
        (
            "select amount * 2 as doubled from orders where dou|",
            &["doubled"],
            &[],
        ),
        (
            "select order_id % 2 as parity, count(*) as n from orders group by par|",
            &["parity"],
            &[],
        ),
        (
            "select order_id % 2 as parity, count(*) as n from orders group by parity having |",
            &["n", "parity"],
            &[],
        ),
        (
            "select order_id, row_number() over (order by amount) as rn from orders qualify |",
            &["rn"],
            &[],
        ),
        (
            "select amount + 1 as total from orders order by tot|",
            &["total"],
            &[],
        ),
        (
            "select o.amount + 1 as total from orders o join customers c on to|",
            &[],
            &["total"],
        ),
        ("select amount as x, (select |) from orders", &["x"], &[]),
        ("select 1 as abc, ab| from orders", &["abc"], &[]),
        ("select | from orders o", &["amount", "o", "sum"], &[]),
        ("select o.order_id, | from orders o", &["amount", "o"], &[]),
        (
            "select * from orders o where |",
            &["amount", "customer_id"],
            &[],
        ),
    ]);
}

#[test]
fn document_objects_are_offered() {
    run(&[
        (
            "create table t1 (a int, b varchar);\nselect * from |",
            &["t1", "orders"],
            &[],
        ),
        (
            "create table t1 (a int, b varchar);\nselect x.| from t1 x",
            &["a", "b"],
            &[],
        ),
        (
            "create view v1 as select order_id as oid from orders;\nselect v.| from v1 v",
            &["oid"],
            &[],
        ),
        (
            "create table t2 as select o.* from orders o;\nselect t2.| from t2",
            ORDERS,
            &[],
        ),
        (
            "create macro m1(x) as table select x as y;\nselect * from |",
            &["m1"],
            &[],
        ),
    ]);
}

#[test]
fn no_duplicate_labels() {
    for sql in [
        "select o.| from orders o",
        "select orders.| from orders",
        "SELECT ORDERS.| FROM ORDERS",
        "select * from |",
    ] {
        let got = labels(sql);
        let mut seen = std::collections::HashSet::new();
        let dupes: Vec<&String> = got.iter().filter(|l| !seen.insert(*l)).collect();
        assert!(dupes.is_empty(), "{sql}: {dupes:?}");
    }
}

#[test]
fn quoted_insert_text() {
    let (text, offset) = common::split_cursor("select \"am| from orders");
    let items = diver_sql::complete_with(&text, offset, &common::catalog());
    let amount = items
        .iter()
        .find(|i| i.label == "amount")
        .expect("amount offered");
    assert_eq!(amount.insert_text, "\"amount\"");
    assert_eq!(
        amount.replace_range,
        diver_sql::TextRange::new(7.into(), 10.into())
    );
}

/// Names with dots, slashes and a leading underscore resolve through FROM-first
/// queries, from the catalog or from the script's own CREATE statements.
#[test]
fn unusual_names_resolve_and_complete() {
    let catalog = diver_sql::MockCatalog::new()
        .schema("memory", "_sys_bic")
        .view_in("memory", "_sys_bic", "org.department.abc/myview_name", CUSTOMERS);
    let labels = |sql: &str| -> Vec<String> {
        let (text, offset) = common::split_cursor(sql);
        diver_sql::complete_with(&text, offset, &catalog)
            .into_iter()
            .map(|i| i.label)
            .collect()
    };
    let view = "\"org.department.abc/myview_name\"";
    for sql in [
        "from _sys_bic.\"org.department.abc/myview_name\" a\nselect a.|",
        "from _sys_bic.\"org.department.abc/myview_name\" AS a select a.| where true",
        "select a.| from _sys_bic.\"org.department.abc/myview_name\" a",
        "from _sys_bic.\"org.department.abc/myview_name\" select \"org.department.abc/myview_name\".|",
        "CREATE SCHEMA s; CREATE VIEW s.\"x.y/z\" AS SELECT 1 AS customer_id, 2 AS name, 3 AS region;\nfrom s.\"x.y/z\" a select a.|",
    ] {
        let got = labels(sql);
        for c in CUSTOMERS {
            assert!(got.iter().any(|g| g == c), "{sql}: {got:?}");
        }
    }
    assert!(labels("from _sys_bic.|").contains(&view.to_string()));
    assert!(labels("from _|").contains(&"_sys_bic".to_string()));
    // A quoted name being typed replaces the whole quoted word.
    let (text, offset) = common::split_cursor("from _sys_bic.\"org.dep|");
    let item = diver_sql::complete_with(&text, offset, &catalog)
        .into_iter()
        .find(|i| i.label == view)
        .expect("the view is offered");
    assert_eq!(item.insert_text, view);
    assert_eq!(usize::from(item.replace_range.start()), "from _sys_bic.".len());
}

/// A table the script creates lends its columns to later statements, by
/// alias and by name.
#[test]
fn script_tables_complete_their_columns() {
    let script = "CREATE TABLE abc(\n    t VARCHAR,\n    z VARCHAR\n);\n\n";
    for query in ["from abc a SELECT a.|", "from abc SELECT abc.|", "from abc select |"] {
        let got = labels(&format!("{script}{query}"));
        assert!(got.iter().any(|g| g == "t") && got.iter().any(|g| g == "z"), "{query}: {got:?}");
    }
}

#[test]
fn string_argument_names() {
    run(&[
        (
            "SET VARIABLE qq = 12;
SELECT getvariable(|",
            &["qq", "db_var"],
            &["db_seq", "threads"],
        ),
        ("SELECT getvariable('|')", &["db_var"], &["order_id"]),
        (
            "CREATE SEQUENCE ser START WITH 1;
SELECT nextval(|",
            &["ser", "db_seq"],
            &["db_var"],
        ),
        ("SELECT currval('d|", &["db_seq"], &[]),
        ("SELECT current_setting(|", &["threads", "max_memory"], &["db_var"]),
        ("SELECT current_setting('th|", &["threads"], &[]),
        // only the first argument names something
        ("SELECT nextval('ser', |", &[], &["db_seq"]),
        ("SELECT * FROM orders WHERE name = '|'", &[], &["db_var", "threads"]),
    ]);
}

/// Outside a string a name goes in as a string literal; inside one, bare.
#[test]
fn string_argument_names_are_quoted_outside_strings() {
    let insert = |sql: &str, label: &str| {
        let (text, offset) = common::split_cursor(sql);
        diver_sql::complete_with(&text, offset, &common::catalog())
            .into_iter()
            .find(|i| i.label == label)
            .map(|i| i.insert_text)
    };
    assert_eq!(insert("SELECT getvariable(|", "db_var").as_deref(), Some("'db_var'"));
    assert_eq!(insert("SELECT getvariable(d|", "db_var").as_deref(), Some("'db_var'"));
    assert_eq!(insert("SELECT getvariable('|')", "db_var").as_deref(), Some("db_var"));
    assert_eq!(
        insert("CREATE SEQUENCE s.ser;
SELECT nextval(|", "s.ser").as_deref(),
        Some("'s.ser'")
    );
}

/// Labels and details after `.` on a struct, against a table with struct columns.
fn struct_fields_at(sql: &str) -> Vec<(String, Option<String>)> {
    let catalog = diver_sql::MockCatalog::new()
        .table("cars", &["mpg DOUBLE", "cylinders INTEGER"])
        .table("events", &["id INTEGER", "info STRUCT(kind VARCHAR, \"place name\" STRUCT(city VARCHAR, zip INTEGER))"]);
    let (text, offset) = common::split_cursor(sql);
    diver_sql::complete_with(&text, offset, &catalog)
        .into_iter()
        .filter(|i| i.kind == diver_sql::ItemKind::Column)
        .map(|i| (i.label, i.detail))
        .collect()
}

fn field(name: &str, detail: Option<&str>) -> (String, Option<String>) {
    (name.to_string(), detail.map(str::to_string))
}

#[test]
fn struct_literal_fields_complete_through_a_cte() {
    let sql = "with stats as (
        from cars a
        SELECT
            a.cylinders,
            {'item1':avg(a.mpg), 'item2':sum(a.mpg)} as st
        GROUP BY ALL
)
select
    s.st.|
from stats s;";
    assert_eq!(struct_fields_at(sql), [field("item1", None), field("item2", None)]);
    // unqualified, and through struct_pack with a nested literal
    let sql = "with stats as (select struct_pack(k := 1, inner := {'x': 1::BIGINT}) p from cars)
select p.inner.| from stats";
    assert_eq!(struct_fields_at(sql), [field("x", Some("BIGINT"))]);
    // the CTE's other columns keep the catalog's types
    let sql = "with stats as (select a.cylinders, {'n': 1} st from cars a) select s.| from stats s";
    assert_eq!(
        struct_fields_at(sql),
        [field("cylinders", Some("INTEGER")), field("st", Some("STRUCT(n)"))]
    );
}

#[test]
fn catalog_struct_columns_complete_their_fields() {
    let fields = struct_fields_at("select e.info.| from events e");
    assert_eq!(
        fields,
        [
            field("kind", Some("VARCHAR")),
            field("\"place name\"", Some("STRUCT(city VARCHAR, zip INTEGER)")),
        ]
    );
    assert_eq!(
        struct_fields_at("select info.\"place name\".| from events"),
        [field("city", Some("VARCHAR")), field("zip", Some("INTEGER"))]
    );
    // through a CTE that passes the column on, and a star
    assert_eq!(
        struct_fields_at("with x as (select e.info from events e), y as (from x) select info.| from y")
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        ["kind", "\"place name\""]
    );
    // not a struct: nothing
    assert_eq!(struct_fields_at("select e.id.| from events e"), []);
}
