# TODO

Findings from a review of the query pipeline and the UI after the SQL
source / SQL editor / row-limit work. Grouped by impact; each item names
where it starts.

## Bugs

- [ ] **A non-linear DuckDB percentile in custom SQL fails in a pivot.**
  The editor SQL for Lower, Higher, Nearest and Midpoint uses both `list`
  and `count` (`aggregate_expression` in `src/domain/pivot/sql.rs`), and
  PIVOT allows one aggregate per value. Structured percentiles still pivot,
  because they rank rows with windows instead. No one-aggregate form exists:
  PIVOT also rejects lambdas (a lambda's parameter binds as a column), and
  `quantile_disc` reads rank `ceil(n * q) - 1`, which can't be turned into
  Polars' ranks without `n`. Grouping by the group and pivot columns first,
  then pivoting with `first(…)`, would take any expression, but it has to
  keep DuckDB's headers for unaliased expressions (`F_count_star()`).
- [ ] **Archiving or deleting a SQL source while another window edits it.**
  `remove_view` resets only this window's editor ("Its editor would save it
  again"). Another window with Auto-save on will write the source back.

## Performance

- [ ] **HUGEINT results go row by row.** `SqlExecution::execute`
  (`sql_execution.rs:714`) runs one `UPDATE … WHERE rowid = ?` per row for
  each HUGEINT or UHUGEINT column. `sum(BIGINT)` returns HUGEINT (checked),
  so an ordinary grouped SQL source, which always takes this path with no
  limit (`source_rows`, `sql_execution.rs:508`), can run millions of
  single-row updates. Cast those columns to VARCHAR in the arrow stream and
  convert in one `INSERT … SELECT`, or materialize SQL sources with CTAS
  like `run` does.
- [ ] **SQL sources copy their rows twice and have no row cap.**
  `source_rows` holds every arrow batch in memory and then appends them all
  to a new in-memory database. That's two full copies at peak, with
  `u64::MAX` as the limit, while file reads are now capped at the Limit. A
  CTAS into a database the store owns would avoid the copy. Decide whether
  the Limit applies to SQL sources too, and show a cut the same way.
- [ ] **SQL sources run again on every open, and never refresh on their own.**
  Each navigation to a view on a SQL source runs the whole script again
  (`navigation.rs:226`, `navigation.rs:936`), including Back/Forward and
  every window separately. Meanwhile, once a view is open, it never notices
  the source being edited and saved, because `check_source` returns when
  there is no `source_identity` (`src/ui/dataframe_view.rs:944`). Cache the
  rows by (source id, text, engine, database revision), as `Sources` does
  for files, and use that key to detect changes.
- [ ] **Superseded queries aren't cancelled.** `ComputationController` lets
  the active pipeline finish even when newer input has replaced it
  (`src/application/computation.rs`, `pipeline.rs:252`). Typing in the
  search box over a slow pivot waits for each stale run. Interrupt the
  store's connection (DuckDB `InterruptHandle`) when `invalidate` replaces
  an active token.
- [ ] **Search and sort run twice on a truncated query result.** When the
  limit is reached, `execute_query(..., Some(view))` already applies the
  search and sort inside the query (`pipeline.rs:850`). Then
  `query_view_rows_traced(&view)` filters and sorts the narrowed result again
  and builds a row map for it. Use the natural row order for the narrowed
  store.
- [ ] **The full SQL catalog is rebuilt after every run.** `run` calls
  `sql_catalog()` (`sql_execution.rs:593`), which re-reads every function
  (about 17 ms) and DESCRIBEs each metadata table function. `catalog()`
  also matches columns to relations with a linear `find` for each column
  row (`sql_execution.rs:115`), which is quadratic in tables × columns for
  large attached databases. Cache functions and metadata columns per
  connection (refresh after `INSTALL`/`LOAD`), rebuild relations only when
  `may_mutate` saw a change, and index relations in a `HashMap`.
- [ ] **A UI-thread `stat` every second.** `check_source` probes in-place
  database views on the UI thread (`src/ui/dataframe_view.rs:921`), while
  the file branch does it on a worker. On a network share this can stall
  the window. Move it to the worker as well.
- [ ] **SQL statements wait while a file imports with Preserve order off.**
  To keep results ordered, `SqlExecution::execute` and query
  materialization take `SourceDatabase::keeping_order`. A long import with
  Preserve order off therefore delays SQL runs and pivots in other windows,
  and Stop can't interrupt a statement that's still waiting for the lock.
  Importing into a separate DuckDB instance would remove the wait.

## UI and UX

- [ ] Rerunning SQL clears the search and drops the result sort
  (`show_sql_result`, `sql_editor.rs:878`). Consider keeping both when the
  columns are unchanged.
- [ ] Changing the Limit doesn't rerun an open SQL result. It still shows
  the old truncation until the next run (`apply_query_limit`,
  `commands.rs`).
- [ ] Remote files (`s3://`, `https://`) in the SQL editor are described on
  a connection that lacks the editor's secrets and settings. When that
  fails, they're never tried again (`file_schemas.rs`, `stamp` returns
  `Ok(None)`).

