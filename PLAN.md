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

## Dashboards

- [ ] **19. Ask before trusting a dashboard.** Networked sources stay allowed, so instead of
  turning DuckDB's network off (see `docs/dashboards/decisions.md`, decision 6), DataDiver shows
  what a dashboard can reach before its first query runs, and remembers the user's answer.
  - **When:** opening a dashboard that isn't trusted yet. The page may load, but its queries
    wait until the user answers. `dashboard check` never asks, since it runs out of sight for
    agents.
  - **What it shows**, from the manifest:
    - each source and where it reads: a local file, a file on a network share (the UNC host), a
      URL, or a profile relation and the database it lives in (local file, or a server such as
      Postgres or MySQL, with its host);
    - which of those reach the network, marked;
    - a plain note that the page's own queries can also read other files and URLs, so the list
      shows what the dashboard declares, not everything it can reach;
    - a stronger warning when the file came from the internet (its `Zone.Identifier` mark).
  - **Buttons:** **Trust this dashboard** (remembered, so it never asks again), **Open once**
    (asks again next time), and **Don't open**. Maybe also **Trust this folder**, for a project
    folder where an agent writes dashboards.
  - **Remembered** in the profile, beside the recent dashboards in `history.json`, with a
    command to forget trusted dashboards.
  - **To decide:** what keeps a dashboard trusted after its file changes. Keyed by path alone,
    an edit stays trusted, which is what an agent building one needs. Keyed by contents, every
    edit asks again. A middle way asks again only when the manifest's sources change.

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
