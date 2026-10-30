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
- **Polars scan mode** for files bigger than memory (the IceSlice plan has the design). Today a
  big file is read whole by either engine; DuckDB tables from an attached database are the way
  around it.
- **Reset database connection** detaches databases, reruns the startup script, and replaces the
  SQL editor's connection, but cannot close the database file: every open DuckDB table holds a
  connection, and DuckDB refuses a second open of the file in one process. `SET GLOBAL` options
  and loaded extensions survive a reset.
- **Settings to consider adding:** default row limit, restore the last session on startup,
  confirm before closing unsaved views, default slicer options for new files.
- **Preserve order** is applied with `SET preserve_insertion_order`, which is global in DuckDB, so
  a read with it off briefly affects queries running in other windows.

## Improve working with bigger data sources
- It takes to long to bring up the slicer list on bigger data
- make a big parquet file a duckdb table to expierement with (300M lines, 100 cols)
- remove some calculated values from info panel?
## Tier 2: SQL editor

- [~] **8. SQL language features from `diver_sql`.** Done for the SQL panel through the editor's
  `LanguageService` (`ui::sql_language`) and `domain::sql_editor::SqlCatalog` (relations with
  column types, databases, schemas, functions with signatures from `duckdb_functions()`,
  settings, extensions): context completion, tree-based highlighting, problems after a typing
  pause (`diver_sql`'s parser passes 99.99% of DuckDB's own test statements, so it is the
  authority rather than `duckdb_extract_statements`), hovers, signature help, folding and smart
  select. Statement splitting, path detection and the smaller editors' colors use `diver_sql`
  too; the hand-written tokenizer and completer are gone.
  - The group-by, pivot, and aggregation editors complete through
    `domain::expression_completion` (`Entry::Expression` in the file's dialect, the frame's
    columns as `Catalog::implicit_columns`, and DuckDB's aggregates in the aggregation editor);
    SQL cards use it for their `"` list, and the search language keeps its own.
  - Left: the expression editors are still the old widget; moving them to the hosted editor
    would bring colors, problems and signature help there too.
  - Left: columns of files and table functions (`FROM 'x.parquet'`, `read_csv(...)`) need a
    background `DESCRIBE` and a way to reopen the list when it finishes.
  - Left: drop tree-sitter-sequel (about 2.4 MB) once brackets and auto-close contexts come
    from the service too.

## Tier 4: bigger features

- [ ] **13. Conditional formatting (beyond the heat map).** Today `QueryConfig.heatmap: bool` gives
  one green linear scale on query measures (`src/domain/heatmap.rs`, `grid.rs` `heat_ranges`).
  - **Model:** replace the bool with `formats: Vec<FormatRule>` on both `QueryConfig` and
    `TableConfig`, saved with the view and covered by undo. Each rule has target columns and a kind:
    - **Color scale:** 2-color or 3-color. Diverging around 0 or the midpoint.
      Palettes: green, red, blue, red–green.
    - **Data bars:** a bar proportional to the value.
    - **Condition rule:** a condition in the *same language as condition cards* (`>100`,
      `=North`, `2024-01..`) → text or background color. Reuses `parse_condition` and
      `column_predicate`.
    - **Duplicates / top-N / bottom-N.**
  - **Evaluation:**
    - Scales still use min/max over the whole result (`numeric_ranges`, cached).
    - Condition rules are evaluated in SQL as a `CASE` → rule index per row, fetched with each
      page, so they work over tables of any size.
  - **UI:** a *Conditional formatting* dialog listing rules (add, edit, reorder, delete), plus
    `Space m` as the quick toggle of today's heat map.
- [ ] **14. Expand / collapse in group-by and pivot (Excel-style outline).** See the design below.
  Build it in phases so nothing that works today changes until the outline is switched on.

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

## Tier 6: packaging

- [ ] **18. Python package and DuckDB extension** (in addition to the standalone exe).
  - **Step 1, recommended:** a `datadiver` Python package that ships the exe and offers
    `datadiver.show(con_or_df_or_relation)`. It writes the data to a temp parquet or DuckDB file
    (or passes the database path) and launches the exe on it.
    - Cheap, and it needs only a "open this file read-only, no profile" CLI mode.
  - **Step 2:** a DuckDB community extension (`CALL datadiver()` / `datadiver('table')`).
    - Possible in pure Rust through the C extension API (the official Rust template, or
      `quack-rs`).
    - Its real advantage is seeing the host's in-memory tables directly.
    - Costs:
      - The app would have to run against a *borrowed* connection instead of its own profile
        database.
      - The GUI event loop must run on the main thread on macOS, which a Python or CLI host
        controls.
      - Builds for every platform in the community CI, and a rebuild per DuckDB release.
    - Worth a spike after step 1.

## Design: expand / collapse (item 14)

The current result is flat: there are no subtotals, ROLLUP, or per-level keys. The risk is in
the features that assume a flat result, so the design keeps them unchanged until outline mode is on.

- **Phase A, field level only (low risk).** `ctrl-alt-left` on a group-by column collapses that
  *field*: the query groups by the fields to its left only. `ctrl-alt-right` expands the next
  field again.
  - New `QueryConfig.depth: Option<usize>`.
  - The result is still flat and correctly aggregated from source rows. Sort, search, drill,
    heat map, totals, and export all work unchanged.
  - The collapsed fields show in the pivot panel as greyed out.
- **Phase B, item level + outline layout.** `alt-left` / `alt-right` collapse or expand one item.
  These keys are free in `QueryGrid`; in tables they move columns.
  - **SQL:** `GROUP BY GROUPING SETS ((k1), (k1,k2), …, (k1..kn))` with `GROUPING_ID()` as a
    hidden `__level` column.
    - Every level aggregates source rows, so mean, median, and distinct-count stay correct, as with
      totals.
    - Visible rows:
      - a leaf row whose ancestors are all expanded,
      - a subtotal row for each collapsed item,
      - in outline layout, a header or subtotal row for each expanded item.
      - Collapsed paths are passed as a `VALUES` list.
    - Order by `k1, GROUPING(k2), k2, …` so each parent sits next to its children.
    - **Pivot:** `PIVOT` can't take GROUPING SETS, so UNION ALL one pivot per depth. The
      `(Total)` LEFT JOIN code is the template.
  - **State:** `QueryConfig.collapsed: Vec<Vec<Value>>` (key paths) and
    `layout: Tabular | Outline | Compact`, saved and undoable.
  - **Layouts:**
    - **Tabular:** each field in its own column, labels repeated.
    - **Outline:** a subtotal row above its children, with the parent key cells blank on child
      rows.
    - **Compact:** one indented "Row labels" column.
    - The layout is mostly a grid rendering choice driven by `__level`, plus a ▸/▾ glyph.
  - **Must adapt** (each with tests):
    - Result sort sorts within siblings, or is disabled while collapsed.
    - Search keeps the ancestors of matching rows.
    - `drill::cell_filters` uses only the keys at the row's level, never `IS NULL` for rolled-up
      keys.
    - The heat map excludes subtotal rows or scales per level.
    - `filter_active_value` on a subtotal row filters by its path.
    - Export writes what is shown, with a level column.
    - Hide `__level` the way hidden columns work for tables.
- **Phase C:** subtotal on/off per field, and "expand to level N".

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
- Heat map: covered by item 13.
- Subtotals: covered by item 14.
