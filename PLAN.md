# DataDiver roadmap

Status: `[ ]` not started, `[~]` in progress, `[x]` done. Finished items are removed; git history
has their write-ups.

## Two query engines: follow-ups

The engines, per-file choice, and Settings list are done (see git history). Still open:

- **Scalar function reference.** The aggregation editor lists each engine's aggregates, but
  custom group-by and pivot expressions have no function list for either engine.
- **Saved selections across engines.** Slicer keys for intervals, times, and time zones are
  spelled differently by each engine (`domain::slicers::scalar_key` vs
  `polars::expressions::scalar_key`), so a view saved under one engine can lose those selections
  under the other. Normalize the keys, or convert them on switch.
- **Reset database connection** detaches databases, reruns the startup script, and replaces the
  SQL editor's connection, but cannot close the database file: every open DuckDB table holds a
  connection, and DuckDB refuses a second open of the file in one process. `SET GLOBAL` options
  and loaded extensions survive a reset.
- **Settings to consider adding:** default row limit, restore the last session on startup,
  confirm before closing unsaved views, default slicer options for new files.

## Tier 5: AI features

- [ ] **17. More agentic ideas** (rough order of value):
  - *Fix this error:* on a failed run, one key sends the SQL plus the error and offers a fix.
  - *Describe the view you want:* "sales by region, pivot by year, only 2024" → a `QueryConfig`
    (group-by, pivot, aggregations, slicers) as structured JSON, validated against the columns.
    More DataDiver-native than SQL, and the result is an editable view rather than code.
  - *Describe a filter:* natural language → the search or condition language.
  - *Explain this SQL / this result / this selection* (selection stats + sample rows).
  - *Ask the data (agent mode):* a loop with tools — list tables, describe, profile a column, run
    a read-only query (row-capped, read-only connection). It answers with the queries it ran,
    each one clickable to open as a SQL result.
  - *Suggest expressions:* in the aggregation or pivot editor, "share of total", "YoY growth" → SQL
    expression.

## Loose ends from finished work

- Aggregate function reference: the structured operations (`pivot_editor::OPERATIONS`) as a
  "common" group.
- SQL path completion: modified time as detail.
- Database tree: share item building with `open_file_palette`, and refresh while open (it reloads
  each time it opens).
- Search column completion: operators after `"name"`, and the column's top values after `=`.
- AI providers: per-model limits (context/output tokens) and reasoning options.
- Ask AI: streaming replies, and a provider picker in the strip (it uses the default).
- Condition card: debounce while typing, and a matching-row count.
- Pasted-table files are never cleaned up (`pasted/` in the data folder).
- Cell detail pane: resizable, and its width remembered.
- Outline: sort within siblings (sorting is off while the outline is on), keep the cursor on an
  item after collapsing it, and search past the row limit (an outline cut at the limit searches
  only the rows it read; flat queries rerun the search inside the query).
- Polars scan mode: CSV types come from the first 10,000 rows, as in DuckDB's scans, so a later
  value that breaks them fails the query that reaches it.
