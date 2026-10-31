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

## Performance

- [ ] **SQL sources run again on every open, and never refresh on their own.**
  Each navigation to a view on a SQL source runs the whole script again
  (`navigation.rs:226`, `navigation.rs:936`), including Back/Forward and
  every window separately. Meanwhile, once a view is open, it never notices
  the source being edited and saved, because `check_source` returns when
  there is no `source_identity` (`src/ui/dataframe_view.rs:944`). Cache the
  rows by (source id, text, engine, database revision), as `Sources` does
  for files, and use that key to detect changes.
- [ ] **SQL statements wait while a file imports with Preserve order off.**
  To keep results ordered, `SqlExecution::execute` and query
  materialization take `SourceDatabase::keeping_order`. A long import with
  Preserve order off therefore delays SQL runs and pivots in other windows,
  and Stop can't interrupt a statement that's still waiting for the lock.
  Importing into a separate DuckDB instance would remove the wait.
