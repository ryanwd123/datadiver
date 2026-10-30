# diver_sql notes

What has been verified, and decisions made while building the crate. See
`PLAN.md` for the design.

## Workflow

- Keywords: edit `keywords.csv` / `extra_keywords.csv`, run
  `cargo test -p diver_sql` twice (the sourcegen test rewrites
  `src/syntax_kind/generated.rs` and fails once when it was stale).
- CST snapshots: `UPDATE_EXPECT=1 cargo test -p diver_sql` rewrites the
  `tests/data/*.rast` dumps and the `expect![[...]]` snapshots; review the diff.
- `cargo run -p diver_sql --example dump -- "SELECT ..."` prints a tree.

## Verified

- `keywords.csv` is `FROM duckdb_keywords()` of the DuckDB 1.5.6 CLI (489
  words: 75 reserved, 330 unreserved, 35 type_function, 49 column_name).
  DuckDB 1.5.6 already lists ASOF, POSITIONAL, SUMMARIZE, RENAME, EXCLUDE,
  REPLACE, PIVOT, QUALIFY, ANTI, SEMI, ... so `extra_keywords.csv` only adds
  BERNOULLI, RESERVOIR and DATABASES.
- Every statement in `tests/corpus/*.sql` was checked with DuckDB 1.5.6's
  `extract_statements` (Python package 1.5.6). Things it refused and that were
  dropped from the corpus: `DROP TABLE a, b` (one object at a time),
  `INTERVAL '1' YEAR TO MONTH`, `'abc'.upper()` (method calls need a column),
  mixing `$1` and `?` in one statement, `ATTACH DATABASE IF NOT EXISTS`
  (`ATTACH IF NOT EXISTS` works), `RESET ALL`, `CREATE SECRET s IN local`.
- Identifiers: DuckDB matches them case-insensitively, quoted or not, so
  `Name::matches` folds case; quoting only decides what completion inserts.

## Decisions

- `SyntaxKind` is a `u16` newtype rather than an enum, so the generated
  keyword kinds (from 1024 up) and the hand-written ones (`kinds!` in
  `syntax_kind.rs`) live in separate files.
- Words are `IDENT` until the grammar asks for a keyword (`at_kw` /
  `bump_kw`). A keyword used as a name stays `IDENT` in the tree.
- Bare-name rules follow DuckDB's categories: table aliases and column names
  take unreserved and column_name words, function and type names take
  unreserved and type_function words, bare select labels take only unreserved
  ones. A few unreserved words that follow a FROM item (SET, SAMPLE, DEFAULT,
  BY, VALUES) are refused as bare table aliases.
- `Expected::Phrase` was added to the plan's enum so that `GROUP BY`,
  `IF NOT EXISTS`, `USING SAMPLE`, ... come as one item; the grammar records
  the phrase where it checks for its first word.
- At the hole, optional names (aliases) record `NewName` but do not take the
  hole, so `FROM t |` still offers JOIN, WHERE, ...; required names take it.
- An unterminated string or quoted identifier the cursor is in is cut at the
  cursor for completion, and the rest of the text lexed again, so
  `SELECT "my co| FROM t` keeps its FROM clause.
- `ColumnSource::Star` carries the relations it expands over (and a `skip`
  count for column aliases like `t(a, b)` over a table), since expanding a
  star needs the FROM items of the block it was written in.
- `COLUMNS('regex')` is not expanded (no regex dependency); `COLUMNS(*)` and
  `COLUMNS(* EXCLUDE ...)` are.
- Fuel: 16384 checks without consuming a token mark the parser stuck; nesting
  deeper than 500 (expressions, FROM items, types, queries) skips the rest of
  the statement, so deep input can't overflow the stack.
- Script-created objects live in `document.rs` (`document_objects`), not in
  `scope.rs`.

## DuckDB test corpus

`tools/fetch_duckdb_corpus.ps1` sparse-checks out `test/sql` of v1.5.6 into
`corpus/duckdb` (gitignored); `cargo test -p diver_sql --test duckdb_corpus
-- --ignored --nocapture` reads the `statement ok` / `query` records, skips
sqllogictest templates (`${...}`, `{name}`, `__TEST_DIR__`), dedupes, and
parses each single statement.

- Without DuckDB: 29,697 distinct statements, 99.96% parse cleanly. The rest
  are mostly records with sqllogictest `#` comments in them, which DuckDB
  rejects too.
- With `--features duckdb-diff` (only statements DuckDB 1.5.6's parser
  accepts): 29,591 statements, 99.99% clean. Left: a string prefix alias
  (`FROM 'x' : (VALUES ...)`), `SETOF ... ARRAY`, and nothing else. Dabbler's
  Lark grammar was at 98.1% of 25,041.
- The ratchet in `tests/duckdb_corpus.rs` is 99.9%.
- Statement splitting: DuckDB's `extract_statements` expands some statements
  into several (PIVOT, `ALTER ... DEFAULT nextval()`, `COPY FROM DATABASE`),
  so the differential test only flags cases where we split more than DuckDB;
  there are none.

Grammar learned from the corpus: trailing commas in most lists (VALUES rows
and lists, FROM, GROUP BY, SET, EXCLUDE / REPLACE / RENAME, PIVOT USING),
`(DESCRIBE ...)` / `(SHOW ...)` / `(PIVOT ...)` as subqueries, `*COLUMNS(...)`,
`agg(x) EXPORT_STATE`, `x NOT NULL`, `10!`, nested joins
(`a JOIN b JOIN c ON x ON y`), string names (`AS 't'('k')`,
`CREATE TABLE 'x'`), `map` / `struct` / `try_cast` as table and column
names, `interval` as a column name, FK `ON UPDATE / DELETE` actions,
`ON COMMIT PRESERVE ROWS`, multi-column PIVOT `FOR a IN (...) b IN (...)`,
`MERGE ... INSERT BY NAME` / `UPDATE SET *`, `SET TIME ZONE`.

Whether `(` starts a query or a join is decided by lookahead: `((SELECT 1)
UNION ...)` is a query, `((VALUES (1)) t JOIN ...)` a join and `((SELECT 1) +
1)` an expression, by what follows the inner parenthesis.

## Editor features

- `highlight` classes tokens by their place in the tree, so a keyword used as
  a name (`SELECT name, year`) highlights as a column; `highlight_with` marks
  the catalog's aggregates.
- `diagnostics` reports lexer problems (unterminated strings, quoted names,
  comments, stray characters), parse errors (one per position) and a hint on
  statements the grammar doesn't know. `diagnostics_with` adds "unknown
  table" warnings, only for schemas the catalog lists tables for, and never
  for CTEs, the script's own tables or table functions.
- Statement hover docs are Dabbler's `statement_docs.py` (adapted from
  DuckDB's MIT-licensed docs) as `src/statement_docs.rs`; the key is picked
  from the statement node the hovered keyword belongs to, so `DO UPDATE` in
  ON CONFLICT doesn't show the UPDATE doc.

## Polars dialect

`ParseOptions { dialect: Dialect::Polars, .. }` parses the same grammar and
reports, as errors, what polars-sql 0.55 lacks; completion stops offering
it. What Polars has was read from polars-sql's source (sqlparser's
`GenericDialect`, plus the `Statement` / `SQLExpr` / `JoinOperator` cases
its context handles):

- Statements: queries (FROM-first included), SHOW TABLES, CREATE TABLE
  [AS], DROP TABLE, EXPLAIN, TRUNCATE, DELETE.
- Not in Polars: lambdas, struct and MAP literals, list comprehensions,
  COLUMNS(...), prefix aliases, `#n`, `!`, EXPORT_STATE, GLOB, SIMILAR TO,
  COLLATE, AT TIME ZONE, PIVOT / UNPIVOT, samples, ASOF / POSITIONAL joins,
  LATERAL, GROUPING SETS / CUBE / ROLLUP.
- Polars has: QUALIFY, named windows, NATURAL / SEMI / ANTI / CROSS joins,
  UNION BY NAME, `* EXCLUDE / REPLACE / RENAME`, FILTER, `~` regex matches,
  `::` casts, TRY_CAST, list literals, read_csv / read_parquet / read_ipc /
  read_json.

`tests/polars_corpus` holds the app's Polars SQL: the aggregate examples of
`infrastructure::polars::functions`, expressions and conditions the
aggregation and pivot editors take, and queries. Every line was run through
Polars 0.55 (`polars::sql::sql_expr` for expressions, an `SQLContext` with a
table `t` for queries); the one it refused, `x > ALL (1, 2)`, was dropped.
`year(day)` stays: it is the app's own shorthand, rewritten before Polars
sees it.

`Entry::Expression` parses one expression with an optional alias
(`EXPRESSION_ROOT`). Expression editors have no FROM clause, so
`Catalog::implicit_columns` supplies the columns of the frame being edited.

## Performance

`cargo bench -p diver_sql` (release, this machine):

| | time |
|---|---|
| parse, 30-line query | 45 µs |
| complete, 30-line query | 62 µs |
| complete_with (catalog + document objects), 30-line query | 106 µs |
| parse, 5k-line script (215 KB, 105k tokens) | 11.4 ms |
| complete in the 5k-line script | 1.9 ms |

For the 5k-line script, lexing is about 1.8 ms, the grammar 4 ms and
building the rowan tree 4.3 ms; the plan's "a few ms" for a full parse is
not met, but completion parses only the statement under the cursor and
stays under 2 ms. `document_objects` needs the full parse, so the app should
compute it once per edit, not per keystroke.

## Dabbler parity

All of Dabbler's `test_completion_cases.py` cases (columns, derived tables,
tables, functions, keywords, special contexts, parser contexts, scopes,
column alias lists, the projection resolver, DML scopes and select-list
aliases) are ported in `tests/completion.rs` and pass, except where labels
differ by design: extensions are offered as `httpfs` rather than `'httpfs'`,
and `USING SAMPLE` comes as a phrase. Dabbler's `columns('regex')` cases are
left out (see above).
