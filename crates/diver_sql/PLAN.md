# diver_sql plan

An editor-oriented parser for DuckDB SQL (and later Polars SQL): lossless syntax
trees, completion contexts, scope analysis, highlighting, diagnostics and
signature help for the text being typed. DuckDB still does the real parsing and
running; this crate only has to understand the text well enough to help.

Goal: match or beat Dabbler (`python_projects/dabbler`), whose Lark grammar
parses 98.1% of the 25,041 statements in DuckDB 1.5.6's test suite and whose
completions cover tables, columns, functions, paths, keywords, settings, types
and extensions.

## Constraints

- Workspace member at `crates/diver_sql`. The root `Cargo.toml` gets a
  `[workspace]` table (`members = [".", "crates/diver_sql"]`); the root package
  stays the default member, so `cargo build`, `cargo test`, maturin and the
  release workflow build exactly what they build today.
- No dependency on duckdb or the app. Runtime deps: `rowan` (which brings
  `text-size`). Dev deps: `expect-test`, `proptest`, `criterion`. The lexer is
  hand-written.
- Pure functions over `&str`. No threads, no globals, no caches between calls;
  the caller keeps a `Parse` if it wants to reuse one.
- Offsets are UTF-8 byte offsets (`TextSize`/`TextRange`), which is what both
  editors use. The app converts to UTF-16 only where Win32 needs it.

## Architecture

```
text ──lexer──▶ tokens (kind, len), trivia included
        │
        └─▶ parser (input = non-trivia tokens) ──▶ events + expected set
                                                      │
                     tree builder (events + all tokens)┘──▶ rowan GreenNode
                                                      │
            ast (typed views over the CST) ◀──────────┘
              │
              ├─ scope      visible relations and their columns at an offset
              ├─ complete   CompletionContext, then items via &dyn Catalog
              ├─ highlight  semantic classes per token
              ├─ diagnostics
              └─ signature  function name + active argument
```

### Lexer (`lexer.rs`)

- Bytes in, `Vec<Token { kind: SyntaxKind, len: TextSize }>` out; never fails,
  never panics, covers every byte.
- Words lex as `IDENT`. Keywords are decided by the parser (see below), since
  330 of DuckDB's 489 keywords are unreserved and can be names (`year`,
  `value`, `name`).
- `"quoted"` identifiers with `""` escapes; `'strings'` with `''`; `E'…'` with
  backslash escapes; `$$…$$` and `$tag$…$tag$`; `X'…'`/`B'…'` blobs.
- Numbers: integers, decimals, `.5`, `1e10`, `1_000`, `0x1F`, `0b101`.
- Comments: `--` to end of line, nested `/* /* */ */`.
- Parameters: `?`, `$1`, `$name`.
- Operators, longest match first: `::` `->` `->>` `**` `//` `||` `@>` `<@`
  `<<` `>>` `<=` `>=` `<>` `!=` `==` `!~~` `~~` `!~~*` `~~*` `^@` `&&` `:=`
  `=>`, plus singles.
- Unterminated strings, quoted identifiers and comments run to the end of the
  text and get their own kinds (`UNTERMINATED_STRING`, ...). The cursor is
  often inside one (`FROM '|`), so completion sees "in a string, opened here".

### Keywords: one source of truth

- `keywords.csv` is `duckdb -csv -c "FROM duckdb_keywords()"` from DuckDB
  1.5.6 (the version the app bundles): word plus category (`reserved`,
  `unreserved`, `type_function`, `column_name`).
- `src/syntax_kind/generated.rs` holds the `Kw` enum, one `SyntaxKind` per
  keyword (`SELECT_KW`, ...), the category table and a sorted lookup. A
  sourcegen test (rust-analyzer style) regenerates it from `keywords.csv` and
  fails when the committed file is stale, so editing the CSV and running
  `cargo test -p diver_sql` is the whole workflow.
- Words the grammar needs that DuckDB lexes as plain identifiers (`ASOF`,
  `POSITIONAL`, `SUMMARIZE`, `RENAME`, `EXCLUDE`, `REPLACE` in star
  expressions, ...) go in a small `extra_keywords.csv`, marked as contextual.

### Parser (`parser/`)

Hand-written recursive descent with a Pratt loop for expressions, producing
rust-analyzer style events (`Start { kind, forward_parent }`, `Token`,
`Finish`, `Error`). `Marker::precede` handles left-associative operators and
postfix forms (`x::INT`, `a[1]`, `f(x) OVER (...)`).

- **Contextual keywords.** `p.at_kw(Kw::Select)` checks an `IDENT` token's
  text; `p.bump_kw(Kw::Select)` remaps that token to `SELECT_KW` in the event
  stream. The tree records which words were used as keywords and which as
  names, which highlighting needs. Reserved words are refused as bare names
  except where DuckDB allows them (after a `.`, after `AS`).
- **Recovery.** Each rule has a recovery set: statement starts, `;`, clause
  keywords (`FROM`, `WHERE`, `GROUP`, `HAVING`, `WINDOW`, `QUALIFY`, `ORDER`,
  `LIMIT`, `OFFSET`, `UNION`, ...) and closing brackets of open groups. On an
  unexpected token, the parser wraps junk in `ERROR` until it reaches the
  set, and keeps going. Balanced brackets inside junk are skipped as a unit.
- **Unknown statements** (anything not starting with a known keyword) become
  `UNKNOWN_STMT` running to the next top-level `;`, so the statements around
  them are untouched.
- **Fuel.** A step counter turns any loop that stops consuming into an error
  instead of a hang; property tests run with it tight.

### Completion: the expected set

Completion parses the statement under the cursor with a **hole**:

1. Find the token at the cursor. Inside a comment → `in_comment`, no
   expectations. Inside a word or quoted identifier → that token is the
   partial word; `replace_range` covers it (`SEL|` → `0..3`, `"my ta|` →
   from the quote). Inside a string → `in_string` with `string_range`, and the
   string token stays.
2. Replace the partial word with a zero-width `CURSOR` token (or insert one at
   the cursor when there is no word). The text after the cursor stays, so
   `SELECT | FROM orders o` still sees its FROM clause.
3. Parse. Whenever the parser checks the token at the hole (`at`, `at_kw`,
   `expect`, `eat`), it records what it would accept there. Checks fail at the
   hole, so every alternative gets recorded. Grammar points that want a name
   call `p.expect_name(Expected::TableRef { .. })` (or `ColumnRef`, ...): at the
   hole this records the semantic expectation and consumes the hole as that
   name, so parsing continues normally after it.
4. For a string at the cursor, rules that take a path (`FROM '…'`, the first
   argument of `read_parquet`/`read_csv`/... , `COPY … TO/FROM '…'`, `ATTACH
   '…'`, `EXPORT/IMPORT DATABASE '…'`, `INSTALL … FROM '…'`) record
   `FilePath` when the string they consume is the cursor token.

```rust
pub enum Expected {
    Keyword(Kw),
    Punct(SyntaxKind),
    DatabaseName,
    SchemaName { database: Option<Name> },
    TableRef { qualifier: Vec<Name> },                 // `main.|`, `db.main.|`
    ColumnRef { qualifier: Vec<Name> },                // `o.|`, resolved through scope
    FunctionName { kind: FunctionKind, qualifier: Vec<Name> },
    TypeName,
    CteName,
    NewName(NewNameKind),                              // Table, Column, Alias, Cte, Macro, ...
    FilePath,
    SettingName,
    ExtensionName,                                     // INSTALL / LOAD
    PragmaName,
    Expr,                                              // an expression may start here
}
pub enum FunctionKind { Scalar, Aggregate, Window, Table, Macro, Any }

pub struct CompletionContext {
    pub expected: Vec<Expected>,      // deduplicated, in the order recorded
    pub replace_range: TextRange,     // the partial word (empty at a space)
    pub prefix: String,               // its text, unquoted
    pub statement: TextRange,
    pub scope: Scope,                 // visible relations and aliases at the cursor
    pub in_string: Option<TextRange>, // string token at the cursor
    pub in_comment: bool,
}
```

`Name` keeps the text and whether it was quoted. DuckDB matches identifiers
case-insensitively, quoted or not, so lookups fold case; quoting matters only
for which words need quotes on insert. `NOTES.md` records what we verify.

### Scope (`scope.rs`)

At the cursor, walk outward through the enclosing query blocks:

- The block's own FROM items, including when the cursor sits in the SELECT
  list before the FROM clause, or in FROM-first syntax (`FROM t SELECT |`).
- Join partners for `ON`, `USING (`, and laterals (`LATERAL`, and DuckDB's
  implicit lateral for later FROM items).
- Outer blocks' relations (correlated subqueries), with lower priority.
- CTEs from every enclosing `WITH`, including a recursive CTE inside its own
  body.
- Select-list aliases where DuckDB lets you reuse them: WHERE, GROUP BY,
  HAVING, QUALIFY, ORDER BY, and later items of the same select list.
- DML targets: `INSERT INTO t (|`, `UPDATE t SET |`, `DELETE FROM t WHERE`,
  `ON CONFLICT (|)`, `RETURNING |`, `UPDATE … FROM`, `DELETE … USING`.
- DDL targets: `ALTER TABLE t ALTER |`, `CREATE INDEX i ON t (|)`.

```rust
pub struct VisibleRelation {
    pub alias: Option<Name>,       // `o` in `orders o`
    pub source: RelationSource,
    pub columns: Columns,          // what the crate inferred
    pub depth: u32,                // 0 = this block, 1 = outer, ...
}
pub enum RelationSource {
    Table { path: Vec<Name> },               // db.schema.table
    File { path: String },                   // FROM 'x.parquet'
    TableFunction { name: Vec<Name>, args: Vec<String> }, // string args for the app's DESCRIBE
    Cte { name: Name },
    Subquery,
    Values,
    Pivot,
}
pub enum Columns {
    Known(Vec<ColumnSource>),   // from a select list, VALUES, column aliases `AS t(a, b)`
    FromCatalog,                // a table, file or table function: ask the Catalog
}
pub enum ColumnSource {
    Named(Name),
    Star { relation: Option<Name>, exclude: Vec<Name>, replace: Vec<Name> },  // expanded via scope/catalog
    Unnamed(String),            // expression text, DuckDB names it after the expression
}
```

`WITH x AS (SELECT a, b AS c FROM t) SELECT | FROM x` gives `x` the columns
`a`, `c`. `SELECT *` in a CTE expands through the CTE's own scope, then the
catalog. `PIVOT` output is unknown, so its relation offers nothing rather than
guesses. The app's `Catalog` turns `FromCatalog` into names.

**Document objects.** `document_objects(&Parse)` lists what the script itself
creates (`CREATE TABLE t (...)`, `CREATE TABLE t AS SELECT ...`, views, macros,
types, schemas), with columns where they can be inferred, so completion offers
them before the script has run. Dabbler has this and it matters in practice.

### Catalog and items (`catalog.rs`, `items.rs`)

```rust
pub trait Catalog {
    fn databases(&self) -> Vec<String> { vec![] }
    fn schemas(&self, database: Option<&str>) -> Vec<String> { vec![] }
    fn tables(&self, database: Option<&str>, schema: Option<&str>) -> Vec<TableInfo> { vec![] }
    fn columns(&self, table: &RelationSource) -> Option<Vec<ColumnInfo>> { None }
    fn functions(&self) -> Vec<FunctionInfo> { vec![] }   // name, kind, signatures, description
    fn types(&self) -> Vec<String> { builtin_types() }
    fn settings(&self) -> Vec<SettingInfo> { vec![] }
    fn extensions(&self) -> Vec<String> { vec![] }
    fn paths(&self, typed: &str) -> Vec<PathInfo> { vec![] }
}

pub struct CompletionItem {
    pub label: String,
    pub kind: ItemKind,          // Keyword, Table, View, Column, Function, Type, Schema, ...
    pub insert_text: String,     // quoted when needed: "Order Date", "select"
    pub replace_range: TextRange,
    pub detail: Option<String>,  // column type, table of origin, signature
    pub documentation: Option<String>,
    pub sort_key: u32,           // lower first; the editor filters by prefix
}
```

`complete_with` maps expectations to items, ranks them (in-scope columns,
then aliases, CTEs and tables, then functions, then keywords; keywords that
complete a phrase like `GROUP BY` and `IF NOT EXISTS` come as one item), and
quotes identifiers that are reserved words or not plain lowercase. It does not
filter by the prefix: the editor's fuzzy filter does that, and Dabbler showed
the editor's ranking is what people expect.

`Catalog` calls must be fast; the app answers from caches and returns `None`
for a file it hasn't described yet (it can start a background `DESCRIBE` and
reopen the list when it lands).

### Highlighting, diagnostics, signature help, hover

- `highlight(&Parse) -> Vec<(TextRange, HighlightClass)>`: keyword, function,
  aggregate, type, table, column, alias, CTE, schema, parameter, string, file
  path, number, operator, comment, error. Keywords used as names highlight as
  names. Optional `highlight_with(&Parse, &dyn Catalog)` colors known tables
  and columns.
- `diagnostics(&Parse) -> Vec<Diagnostic { range, message, severity }>`:
  lexer errors (unterminated string or comment) and parse errors. Parse errors
  are only as trustworthy as the grammar's coverage, so the app should show
  DuckDB's own parser errors (`duckdb_extract_statements`, as Dabbler does) as
  the authority, and use ours as instant hints until the corpus pass rate is
  near 100%. Later: unknown table/column warnings through the catalog.
- `signature_help(text, offset)`: the innermost call around the offset, its
  name path and the active argument (commas at depth, named `x := 1`
  arguments), with signatures from the catalog.
- `hover(text, offset, &dyn Catalog)`: function docs, statement docs (port
  Dabbler's `statement_docs.py`, adapted from DuckDB's MIT-licensed docs),
  and what a table, CTE, alias or column refers to.
- Editor helpers from the same tree: `statements`, `statement_at`,
  `folding_ranges` (statements, CTEs, subqueries, CASE, parenthesized lists)
  and `selection_ranges` (expand/shrink selection along nodes).

### Dialects and Polars

Recommendation: **one crate, a `Dialect` parameter**, not a second crate.

The app uses Polars SQL in the aggregation, pivot and condition editors of
Polars files, which are expression-level. Polars SQL is a much smaller
language with mostly PostgreSQL-shaped syntax. A second crate would duplicate
the lexer, tree, scope and completion layers for a grammar that is close to a
subset of DuckDB's. Instead:

- `ParseOptions { dialect: Dialect::DuckDb | Dialect::Polars, entry: Entry::Script | Entry::Expression }`.
- `Entry::Expression` parses one expression (with optional `AS alias`), which
  both engines' expression editors need.
- `dialect/` holds per-dialect tables: which grammar features exist (Polars
  has no PIVOT statement, no `FROM`-first, no lambdas, ...), its keyword list,
  and which functions take paths. Functions come from the `Catalog` anyway, so
  the app feeds Polars' function table (`infrastructure::polars::functions`)
  through the same trait.
- If Polars SQL grows enough to need a grammar of its own, `dialect::polars`
  is the seam to lift into `diver_sql_polars`.

Polars support lands after the DuckDB phases; the dialect type exists from
phase 1 so the grammar has its gates in place.

## Module layout

```
crates/diver_sql/
  PLAN.md  NOTES.md  keywords.csv  extra_keywords.csv
  src/
    lib.rs              public API, re-exports
    syntax_kind.rs      SyntaxKind, rowan Language impl
    syntax_kind/generated.rs
    lexer.rs
    parser/
      mod.rs            Parser, markers, events, expected set, recovery
      event.rs  builder.rs
      grammar/
        statements.rs   statement dispatch, unknown statements
        select.rs       query blocks, set operations, VALUES, WITH
        from.rs         FROM items, joins, table functions, PIVOT/UNPIVOT, SAMPLE
        expressions.rs  Pratt loop, CASE, CAST, lambdas, literals, calls, OVER
        types.rs        type names, arrays, STRUCT/MAP/UNION/ENUM
        names.rs        dotted names, aliases, column lists
        ddl.rs  dml.rs  misc.rs
    ast.rs              typed views over the CST
    scope.rs
    complete.rs         CompletionContext
    items.rs            CompletionItem ranking and quoting
    catalog.rs          Catalog trait, MockCatalog (behind cfg(test) or a `mock` feature)
    highlight.rs  diagnostics.rs  signature.rs  hover.rs  folding.rs
    dialect.rs
  tests/
    lexer.rs  parse_snapshots.rs  completion.rs  scope.rs  props.rs  corpus.rs
    data/       .sql fixtures with expected CSTs (expect-test)
    corpus/     curated queries that must parse cleanly
  benches/parse.rs
  tools/fetch_duckdb_corpus.ps1
```

## Testing

- **Completion fixtures.** `"SELECT | FROM t"` with `|` as the cursor, asserting
  the `CompletionContext` (expected set, replace range, scope) and, through
  `MockCatalog`, the item labels, with expect-test snapshots. Port Dabbler's
  `test_completion_cases.py` and `test_cursor_context.py` cases; they encode
  bugs found in real use.
- **CST snapshots** per grammar area, valid and broken input, in
  `tests/data/*.sql` with the tree dump beside it (expect-test `expect_file!`).
- **Properties** (proptest): arbitrary strings, and random truncations, byte
  deletions and token swaps of corpus queries, never panic, round-trip
  (`tree.to_string() == input`), and finish within the fuel limit. Completion
  at every offset of every corpus query never panics.
- **Curated corpus** (`tests/corpus/*.sql`, committed): DuckDB docs examples,
  the acceptance cases, and your own scripts; zero diagnostics required.
- **DuckDB test corpus**: `tools/fetch_duckdb_corpus.ps1` sparse-checks out
  `test/sql` of DuckDB v1.5.6 into `crates/diver_sql/corpus/duckdb`
  (gitignored). An `#[ignore]` test reads the sqllogictest records (`statement
  ok` and `query`), parses each statement, reports the pass rate and the
  largest failure groups, and fails below a threshold that ratchets up (start
  where phase 3 lands, aim above Dabbler's 98.1%).
- **Differential test** behind `--features duckdb-diff` (dev-dep on the
  `duckdb` crate, off by default): keeps only corpus statements DuckDB's own
  parser accepts, so "DuckDB accepts, we report an error" is measured exactly,
  and compares statement splitting with `duckdb_extract_statements`.
- **Bench** (criterion): parse and complete on a 5k-line script and on a
  typical 30-line query.

## Performance targets

- Typical query (≤ 100 lines): parse + complete well under 1 ms.
- 5k-line script: full parse a few ms. `complete` lexes the whole text but
  parses only the statement under the cursor (plus `document_objects`, which
  the app can compute once per edit and pass in).
- No allocation per token beyond the token vector and the green tree; keyword
  lookup by length then binary search on a static table.

## Phases

Each phase ends with `cargo test -p diver_sql` green, clippy clean, `NOTES.md`
updated, and a commit.

1. **Lexer, CST, SELECT core, completion engine.** Workspace conversion;
   keyword sourcegen; lexer with all token kinds; parser infrastructure
   (events, markers, recovery, expected set, hole); tree builder; SELECT with
   DISTINCT [ON], select items with aliases and `*`/`t.*` with EXCLUDE /
   REPLACE / RENAME, FROM-first and bare `FROM t`, tables, files, table
   functions, subqueries, all join kinds, WHERE, GROUP BY [ALL], HAVING,
   QUALIFY, ORDER BY [ALL], LIMIT/OFFSET, set operations; a basic expression
   grammar (operators, calls, literals, parentheses); DROP and CREATE heads
   (enough for their acceptance cases); statement splitting; scope for FROM
   relations; `complete`, `complete_with`, `MockCatalog`; the SELECT / DROP /
   CREATE acceptance cases; round-trip property test.
2. **Full expressions and inference.** Complete precedence table (checked
   against DuckDB's grammar), CASE, CAST / TRY_CAST / `::`, IN / BETWEEN /
   LIKE / ILIKE / SIMILAR TO / GLOB, IS [NOT] DISTINCT FROM, lambdas, list /
   struct / map literals, indexing and slicing, COLUMNS(...), window functions
   with OVER and named windows, FILTER, aggregate ORDER BY, subquery
   expressions, EXISTS, type names; WITH [RECURSIVE] … [NOT] MATERIALIZED,
   VALUES, PIVOT / UNPIVOT, SAMPLE / TABLESAMPLE, WINDOW; CTE and subquery
   column inference; select-list aliases; `document_objects`.
3. **Remaining statements.** DDL (CREATE … TABLE / VIEW / MACRO / FUNCTION /
   TYPE / SEQUENCE / SCHEMA / INDEX / SECRET, column definitions and
   constraints, CTAS, ALTER, DROP), DML (INSERT with OR REPLACE / IGNORE, BY
   NAME / POSITION, ON CONFLICT, RETURNING; UPDATE … FROM; DELETE … USING),
   misc (ATTACH / DETACH / USE, COPY, EXPORT / IMPORT DATABASE, DESCRIBE,
   SHOW, SUMMARIZE, EXPLAIN [ANALYZE], PRAGMA, SET / RESET, INSTALL / LOAD,
   transactions, CHECKPOINT, PREPARE / EXECUTE / DEALLOCATE, CALL, VACUUM,
   ANALYZE, COMMENT ON). Fetch the DuckDB corpus, measure, fix the largest
   failure groups, set the ratchet.
4. **Highlighting, diagnostics, signature help, hover, folding, selection
   ranges.** Plus the `duckdb-diff` test and the bench.
5. **Polars dialect** and `Entry::Expression`, checked against the Polars
   version the app uses (0.55) with a corpus of the app's own Polars SQL.
6. **App integration** (done; see the root `PLAN.md` item 8): replace `domain::sql_editor`'s
   tokenizer, statement splitting and completion with this crate, implement
   `Catalog` over the app's schema cache and `duckdb_functions()` /
   `duckdb_settings()` / `duckdb_types()` / `duckdb_extensions()`, and plug it
   into the editor (see the root `PLAN.md`).

## Acceptance cases (minimum)

| Input | Expected |
|---|---|
| `CREATE OR REPLACE \|` | TEMP, TEMPORARY, PERSISTENT, TABLE, VIEW, MACRO, FUNCTION, TYPE, SEQUENCE, SCHEMA, SECRET, ... (DuckDB 1.5.6 accepts all of these; INDEX is the one CREATE form it refuses with OR REPLACE) |
| `DROP TABLE \|` | IF EXISTS, TableRef |
| `DROP TABLE main.\|` | TableRef { qualifier: [main] } |
| `SEL\|` | SELECT and other statement starts; replace_range = `SEL` |
| `SELECT \| FROM orders o` | ColumnRef with scope [orders AS o], FunctionName, DISTINCT, `*`, expression keywords |
| `SELECT o.\| FROM orders o JOIN c ON …` | ColumnRef { qualifier: [o] } → orders only |
| `FROM orders SELECT \|` | ColumnRef, scope [orders] |
| `SELECT * EXCLUDE (\|) FROM t` | ColumnRef, scope [t] |
| `SELECT a FROM t WHERE a = 1 \|` | AND, OR, GROUP BY, ORDER BY, LIMIT, QUALIFY, UNION, ... |
| `SELECT a FROM t GROUP BY \|` | ALL, ColumnRef, Expr |
| `WITH x AS (SELECT a, b AS c FROM t) SELECT \| FROM x` | columns a, c |
| `SELECT * FROM read_parquet('\|` | FilePath, in_string |
| `SELECT * FROM '\|` | FilePath |
| `SELECT * FROM t; DROP \|` | the second statement only |
| `SELECT -- comment \|` | nothing, in_comment |
| `INSERT INTO t (\|` | ColumnRef, scope [t] |
| `SET \|` | SettingName |
| `SELECT CAST(x AS \|` | TypeName |

Dabbler's case list adds many more (quoted dotted names, schema-qualified
aliases, join ON, window PARTITION BY, CTAS, multi-statement); they get ported
in phases 1 and 2.
