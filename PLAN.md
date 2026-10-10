# DataDiver roadmap

Status: `[ ]` not started, `[~]` in progress, `[x]` done. Finished items are removed; git history
has their write-ups.

## Tier 2: SQL editor

- [ ] **7. Better SQL completion.** Today it completes only tables after FROM/JOIN/PIVOT and columns
  after `alias.`.
  - Columns *without* an alias, from the relations in the current FROM scope (including `FROM 'file'`
    and `read_*()` sources, via a cached `DESCRIBE`). Infer CTE columns from their select lists.
  - Functions from `duckdb_functions()` (scalar, aggregate, table), with the signature and
    description as detail. Keywords from `duckdb_keywords()`.
  - `ctrl-space` triggers completion by hand (`present_completion_items`). (`tab` already accepts
    as well as `enter`.)
  - Fix: both providers set `filter_text = label`, so the whole label highlights instead of the
    typed prefix.
  - Done: the group-by, pivot, and aggregation editors list columns after `"` (ranked like the
    search box), offer nothing inside `'strings'`, and accept with Tab. Condition and SQL cards
    share the search box's `"` list (`ui::column_completion`).
  - Later: hover docs for functions (`HoverProvider`), and red squiggles from a prepare/EXPLAIN on
    pause (`diagnostics_mut`).
- [ ] **8. VS Code editing keys in the SQL editor.** Built as app actions in `SqlPanel` (scoped to the
  editor, so `ctrl-d`/`ctrl-l`/`ctrl-g` keep their grid meaning elsewhere) using the editor's public
  API (`text`, `selected_range`, `set_selected_range`, `replace`, `apply_lsp_edits`):
  - `ctrl-/` toggle line comment, `shift-alt-a` block comment
  - `alt-up`/`alt-down` move line, `shift-alt-up`/`down` copy line, `ctrl-shift-k` delete line
  - `ctrl-l` select line, `ctrl-g` go to line, `ctrl-shift-\` jump to bracket
  - `ctrl-d` add next occurrence and `ctrl-shift-l` all occurrences. This needs the multi-cursor
    API; check that it is public before promising it.
  - `ctrl-shift-z` redo (missing on Windows), `F3`/`shift-F3` find next/previous
  - `ctrl-enter` stays "run statement".

  **What the editor already has** (gpui-kit 0.6.4 → gpui-base/gpui-component 0.6.6, all bindings in
  gpui-base `input/base/state.rs`):
  - **Display:** line numbers, indent guides, code folding (gutter, mouse only), soft wrap,
    tree-sitter SQL highlighting.
  - **Editing:** auto-close `( [ { " '` outside strings, smart indent.
  - **Movement:** arrows, `home`/`end`, `ctrl-home`/`ctrl-end`, `pageup`/`pagedown`,
    `ctrl-left`/`right` (word).
  - **Selection:** `shift-`+movement, `ctrl-shift-left`/`right` (word), `ctrl-a`, double-click
    word, triple-click line.
  - **Delete:** `ctrl-backspace`/`ctrl-delete` (word).
  - **Indent:** `tab`/`shift-tab` and `ctrl-]`/`ctrl-[`.
  - **Multi-cursor:** `ctrl-alt-up`/`down`, `alt`-click, `alt-shift`-drag column selection.
  - **Clipboard and undo:** `ctrl-c`/`x`/`v`, `ctrl-z`/`ctrl-y`.
  - **Find and replace:** `ctrl-f`, `ctrl-h` (literal, with case toggle; no regex or whole word).
  - **Menus:** right-click menu; `ctrl-.` code actions (unused).
  - **Completion menu:** `up`/`down`/`enter`/`escape`.
  - **Language hooks (unused so far):** completion (popup and inline ghost text), hover, go to
    definition, code actions, diagnostics.

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
  "common" group, and feeding the list to SQL editor completion (item 7).
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
