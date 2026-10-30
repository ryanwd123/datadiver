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

- [x] **7. Monaco-style SQL editor.** `crates/code_editor` hosted in the SQL panel
  (`ui::widgets::hosted_editor`). DataDiver translates its own events into the editor's input
  calls rather than forwarding window messages, so the keymap keeps priority and headless tests
  drive it. The editor's find widget replaced the panel's. The SQL Editor key context holds Run,
  Run script, Ask AI and the editor's actions (rebindable; an unbound default stays off), and the
  palette lists them while the editor is open. Hovers come from the editor's own rest timer.
  Caret blink while active: about two full-window repaints a second, 16 ms of CPU over 8 s in a
  debug build.
- [~] **8. SQL language features from `diver_sql`.** Done for the SQL panel through the editor's
  `LanguageService` (`ui::sql_language`) and `domain::sql_editor::SqlCatalog` (relations with
  column types, databases, schemas, functions with signatures from `duckdb_functions()`,
  settings, extensions): context completion, tree-based highlighting, problems after a typing
  pause (`diver_sql`'s parser passes 99.99% of DuckDB's own test statements, so it is the
  authority rather than `duckdb_extract_statements`), hovers, signature help, folding and smart
  select. Statement splitting, path detection and the smaller editors' colors use `diver_sql`
  too; the hand-written tokenizer and completer are gone.
  - Left: the group-by, pivot, and aggregation editors and the condition and SQL cards still use
    the old widget's `"` column list (`ui::column_completion`); move them to `Entry::Expression`
    with the file's dialect (DuckDB or Polars) and `Catalog::implicit_columns`.
  - Left: columns of files and table functions (`FROM 'x.parquet'`, `read_csv(...)`) need a
    background `DESCRIBE` and a way to reopen the list when it finishes.
  - Left: drop tree-sitter-sequel (about 2.4 MB) once brackets and auto-close contexts come
    from the service too.

### Design: the Monaco-style editor (item 7)

**Where the code lives.** Copy `win_monaco/editor` into this repo as `crates/code_editor` (package
renamed from `editor` so it isn't confused with `ui::widgets::editor`), recording win_monaco's commit
(`4924080`) in the commit message. win_monaco has no remote, so a path dependency outside the repo
would break the GitHub release build; this copy becomes the maintained one, and the win_monaco demo
can later point at it by path. Build with `default-features = false, features = ["lang-sql"]`
(no Python grammar); `windows` 0.62 already matches.

**Hosting.** DataDiver paints one window through `Canvas` over an `ID2D1RenderTarget`, which is the
case win_monaco's hosted mode was built for:
- `Editor::create_hosted(hwnd, ..)` when the SQL panel opens; `set_bounds` from the panel's layout
  each paint; `set_visible(false)` when the panel hides (keeps text, undo and scroll; frees timers).
- Paint: `paint_into(canvas.target())` at the panel's place in the frame, so dialogs and the command
  palette still draw over it. The editor clips itself (popups included) to its bounds.
- Input: the platform layer gets an escape hatch, `View::raw_message(msg, wparam, lparam) ->
  Option<LRESULT>`, called before DataDiver's own translation. The SQL panel forwards mouse
  messages over the editor's bounds and keyboard/IME messages while it has focus, so the editor
  sees exactly what it would in its own window. Focus moves between the two by DataDiver's focus
  model (`focus` / `has_focus`), with Escape and the app's panel keys leaving the editor.
- Timers live on the editor's own message-only window, so they don't collide with DataDiver's.
- To check: the editor creates its own D2D factory; drawing with brushes and DWrite layouts into a
  target from another factory is fine, but any geometry it draws must come from the target's
  factory (`rt.GetFactory()`). Verify with the debug layer on first paint.
- Snapshots: `--capture` must paint the hosted editor into the headless target too.

**Keys and commands.** Monaco's defaults cover old item 8 entirely (multi-cursor, Ctrl+D,
line moves, comments, bracket jump, go to line, expand selection, folding, zoom). DataDiver's
keymap stays in charge where they clash:
- `ctrl-enter` runs the statement (Monaco: insert line below), `ctrl-shift-enter` follows the
  keymap too; registered with `add_command(chord, id)` from DataDiver's keymap, so rebinding a key
  in the keymap editor rebinds it here.
- The command palette lists the editor's actions (`run(Action)`), with their keys.
- The editor's find/replace widget (regex, whole word, case) replaces the SQL panel's find bar.

**Look.** Theme colors from DataDiver's theme (`set_theme` / `set_theme_color`), the font and size
from the font picker, zoom saved with the other editor settings.

**Wiring to the panel.** `Event::ContentChanged` → `SqlPanelEvent::Changed`; statement under the
cursor from `selections()`; the AI proposal uses `apply_edits` (one undo step) and `set_selections`.
Completion first goes through a `CompletionProvider` wrapping today's `domain::sql_editor`
completions and path completion, so nothing is lost before `diver_sql` arrives.

**Editor API additions** (needed for item 8, done in `crates/code_editor`):
- Completion items with their own replace range (quoted names, paths with slashes) and
  documentation, and a way to reopen the list when a background `DESCRIBE` finishes.
- An external highlighter hook (token classes for a changed range) so `diver_sql` can replace
  tree-sitter-sequel (≈2.4 MB of the binary) once its highlighting lands; folding ranges and
  selection ranges from the host too, with today's indentation/scan fallbacks.
- Hover providers (function and statement docs, not just decoration messages) and a signature
  help popup.

**Tests.** `ui::dataframe_view::sql_tests` drives the old editor's API; port them to the hosted
editor's headless API (`editor.rs` is testable without a window). The old widget stays for
single-line inputs and the smaller editors; moving the multi-line expression editors and the cell
detail view is a later step.

**Phases.** (1) vendor the crate and get it building in the workspace; (2) host it in the SQL
panel with keys, theme, persistence and the old completions, porting the SQL tests; (3) the API
additions; (4) `diver_sql` integration (its phase 6).

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
