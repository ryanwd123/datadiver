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
  deeper than 200 (expressions, FROM items, types, queries) skips the rest of
  the statement, so deep input can't overflow the stack.
- Script-created objects live in `document.rs` (`document_objects`), not in
  `scope.rs`.

## Dabbler parity

All of Dabbler's `test_completion_cases.py` cases (columns, derived tables,
tables, functions, keywords, special contexts, parser contexts, scopes,
column alias lists, the projection resolver, DML scopes and select-list
aliases) are ported in `tests/completion.rs` and pass, except where labels
differ by design: extensions are offered as `httpfs` rather than `'httpfs'`,
and `USING SAMPLE` comes as a phrase. Dabbler's `columns('regex')` cases are
left out (see above).
