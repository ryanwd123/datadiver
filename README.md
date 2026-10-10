![Hooded merganser mascot](assets/duck3-small.png)

# DataDiver (Quick Pivot)

A native desktop app for fast data exploration and quick lookups. Open a data file, search and filter rows, build a pivot, etc.

DataDiver is built in Rust with GPUI Kit and DuckDB. The focus is an efficient, enjoyable workflow: responsive tables, keyboard commands, and reusable views. The Rust library is named `datadiver`; the application binary is `datadiver`.

## Features

- **Open local data:** CSV, TSV, Excel (`.xlsx`), and Parquet (`.parquet` or `.pq`).
- **Add folders:** add all files in a folder to your workspace
- **Explore tables:** search, sort, filter with column slicers, freeze columns, and inspect selection statistics.
- **Build pivots:** choose grouping columns, pivot dimensions, and aggregations such as count, sum, mean, median, minimum, and maximum.
- **Write SQL expressions:** edit grouping, pivot dimensions, and aggregations with highlighting and column completion.
- **Reuse your work:** save views, browse recent files and views, configure source folders, and restore the previous session.
- **Copy results:** send selected cells to the clipboard as TSV, CSV, JSON records, JSON columns, or Python lists, with raw and formatted TSV options.
- **Work from the keyboard:** use the command palette and customize shortcuts in the Keymap Editor.
- **Make it comfortable:** adjust themes, fonts, and font size, and open independent windows.

## Install

DataDiver currently ships for 64-bit Windows. Install it with [pipx](https://pipx.pypa.io/):

```powershell
pipx install datadiver
datadiver
```

Upgrade later with `pipx upgrade datadiver`. Alternatively, download `datadiver.exe` from the zip on the [Releases page](https://github.com/ryanwd123/datadiver/releases). The exe is unsigned, so Windows SmartScreen may warn the first time you run it.

## Build and run

The project has been developed and tested on Windows. The commands below use PowerShell.

You will need:

- A current stable Rust toolchain with Cargo and Rust 2024 edition support.
- For Windows builds, the MSVC Rust target, Visual Studio C++ Build Tools, and the Windows SDK.
- Internet access to download Cargo dependencies for the first build.

DuckDB is compiled from bundled source, so a separate DuckDB server or installation is not required. The first build can take a while because it includes native dependencies.

From the repository root:

```powershell
cargo build --release
.\target\release\datadiver.exe
```

Open a file directly:

```powershell
.\target\release\datadiver.exe --file "C:\data\sales.csv"
```

To try the app with generated data:

```powershell
cargo run --release -- --rows 10000 --ephemeral
```

`--ephemeral` uses a temporary profile, making it useful for trying the app without retaining settings or cached source data.

## A typical workflow

1. Press **Ctrl+O** to open a supported file, or **Ctrl+P** to browse files and saved views.
2. Press **/** to search the displayed rows. Use **Add slicer** from the command palette to filter by a source column.
3. Press **G** to choose grouping columns, **A** to configure aggregations, and **P** to choose pivot dimensions. Use **O** to configure ordering.
4. Select cells and press **Ctrl+C** to copy, or **Ctrl+Shift+C** to choose an output format.
5. Press **F12** to save the view for later.

Group By and Pivot By each provide **+ Custom expression** and an SQL editor for the selected entry. For example, group by `date_trunc('month', "date") AS month` or pivot by `upper("region") AS RegionCode`. Expressions use source columns; aggregates and window functions belong outside these row-level dimension expressions. Use `AS` for a result name or keep DuckDB's default name.

Apply with **Ctrl+Enter**, or use **Revert** to restore the saved entry. Invalid edits show an inline error and preserve the last successful grid. **Tab/Shift+Tab** cycles between available columns, configured entries, and SQL; **Alt+↑/↓** reorders and **Delete** removes a configured entry. Managers stay open for further edits; close with **Escape** or the close button. You can configure pivot dimensions first, then add a group to build the result.

Views store the source path and exploration configuration. They reference the original source file, so sharing a view also requires making that source available.

## Keyboard shortcuts

These are the default shortcuts. Single-key table commands apply when the table has focus; the Keymap Editor supports customization by context.

| Shortcut | Action |
| --- | --- |
| Ctrl+O | Open file |
| Ctrl+P | Files and views |
| Ctrl+Shift+P | Command palette |
| / | Search |
| = | Keep rows equal to the active cell |
| - | Exclude rows equal to the active cell |
| Enter | Drill down from a query cell to its source rows |
| Ctrl+G | Jump to column |
| G | Group By |
| A | Aggregations |
| P | Pivot By |
| O | Order By |
| Ctrl+C or c | Copy selection |
| Ctrl+Shift+C | Copy As |
| Ctrl+S or F12 | Save view |
| f | Freeze or unfreeze column (table grid) |
| Ctrl+Shift+1–9 | Assign the current file to a favorite |
| Ctrl+1–9 | Open a favorite |
| Ctrl+R | Reload source |
| Ctrl+Shift+R | Reload and reset |
| Ctrl+N | New window |
| Ctrl+Q | Close window |
| Ctrl+Shift+K | Keymap Editor |
| F1 | Help |

## Command-line options

```powershell
# Open a source file; a positional path also works.
cargo run --release -- --file "C:\data\sales.parquet"

# Open a saved view.
cargo run --release -- --view "C:\views\sales-summary.json"

# Open a file with an initial search and column slicer.
cargo run --release -- --file "C:\data\sales.csv" --search "North" --slicer "Region"

# Use a separate persistent profile.
cargo run --release -- --home ".\demo-profile" --rows 10000
```

| Option | Purpose |
| --- | --- |
| `--file PATH` | Open a supported source file |
| `--view PATH` | Open a saved JSON view |
| `--rows N` | Generate synthetic data |
| `--search TEXT` | Set the initial row search |
| `--slicer COLUMN` | Add a source-column slicer; may be repeated |
| `--home DIRECTORY` | Store all profile files directly in this directory |
| `--ephemeral` | Use a temporary profile |
| `--no-restore` | Skip restoring the previous session and window geometry |
| `--window WxH` | Set the initial window size |
| `--help`, `-h` | Print basic usage |

Choose one of `--file`, `--view`, or `--rows` per launch. `--home` and `--ephemeral` cannot be combined. With no explicit source, the app attempts to restore the last source or view.

## File formats and local storage

CSV and TSV imports use a header row and automatic type detection. Excel imports read the first worksheet with a header row. Older `.xls` files, `.xlsb`, and `.ods` are not supported.

The first Excel import may need internet access for DuckDB to download its `excel` extension.

Imported sources are cached in an on-disk DuckDB database. Reopening an unchanged file can reuse its imported data instead of parsing it again. Source changes are detected using file size and modification time. Open windows check on activation and every second, then refresh their grids and pivots when the data changes; **Ctrl+R** forces a fresh import when needed.

Each file has one physical table in the `files` schema: `sales.csv` becomes `files.sales_csv`. Names are lowercase, with punctuation replaced by underscores. Files that map to the same table name are rejected. Reloads replace the table transactionally; no previous table revisions are retained. The cache database format is version 2; delete an older cache database to recreate it.

On Windows, all settings, the cached database, saved views, and application data are stored in `%LOCALAPPDATA%\datadiver`. Other platforms use `~/.local/share/datadiver`. Custom `--home` profiles store everything in the specified directory.

Use **Open settings directory** in the command palette (**Ctrl+Shift+P**) to open the active profile folder in your file manager. Previous `quick_pivot\v2` profiles are not migrated.

The database is named `quick_pivot.duckdb`. On-disk storage still uses memory for queries, pivots, row maps, and display caches.

## Development and verification

Run the regular tests and the tests with UI snapshot support:

```powershell
cargo test
cargo test --features snapshots
```

Build the optimized application:

```powershell
cargo build --release
```

The app can capture its own UI when built with the `snapshots` feature:

```powershell
cargo run --features snapshots -- --rows 60 --show info --capture "target\ui\info.png" --ephemeral
```

Use `--window 1400x900` to specify capture dimensions and `--capture-delay 1000` to set the capture delay in milliseconds.

The source is organized into:

- `src/domain`: data models, pivots, selection, formatting, and copy formats.
- `src/application`: application state, commands, preferences, and editing workflows.
- `src/infrastructure`: DuckDB storage, source imports, persistence, and performance instrumentation.
- `src/ui`: the table interface, panels, editors, and palettes.

For measured performance results and implementation details, see the [storage comparison](performance/STORAGE.md), [query modes](performance/QUERY_MODES.md), and [table width behavior](performance/TABLE_WIDTHS.md).

### Releasing

From a clean `main`, run:

```powershell
.\scripts\release.ps1 0.2.0
```

The script sets the version in `Cargo.toml`, runs the tests, commits, tags `v0.2.0`, and pushes. The tag starts the Release workflow, which builds the Windows wheel with [maturin](https://www.maturin.rs/), attaches it and a zipped exe to a GitHub release, and publishes the wheel to PyPI. To check the wheel locally, run `maturin build --release` and `pipx install target\wheels\datadiver-*.whl`.

## Slicers

**Add slicer** (**Space Z**) offers three kinds of card:

- **SQL filter** is always first in the list, and a view can have one. Type any DuckDB condition over the source columns and press **Enter**, for example `amount > 100 AND region <> 'Europe'` or `amount % 2 = 0`. The card is not tied to a column.

- **Value cards** list a column's values when it has at most 1,000 of them. Click, Ctrl+click, or drag to select. The search icon in the card header opens a filter box: type to narrow the list, press **Enter** to select every matching value, and **Esc** to close it. Values are greyed out when no row matches the other slicers and the `=` / `-` filters, but search does not grey them out.
- **Condition cards** (listed as *column · condition*) work for any column. Type a condition and press **Enter**. Terms are separated by spaces and must all match: `>=10 <100`, `=North`, `!=0`, `:smith` or a bare word (contains), `-word` (does not contain), and inclusive ranges `10..20`, `2024-01-01..`, `..500`. Numbers compare as numbers and dates as dates. Comparison values can be relative dates: `today`, `yesterday`, `-30d`, `+2w`, `-6m`, `-1y` (so `-30d..today` is the last 30 days). The card shows the column's smallest and largest value.

If a condition or SQL filter has an error, the error shows on its card and the card stops filtering until you fix it.

Slicers, condition cards, the SQL filter, and `=` / `-` filters all narrow source rows, before grouping in a query. Search narrows the displayed rows. In a query, search and sort apply to the result.

When a query result reaches the row limit, the limit input turns red and the status bar says **Limited to N rows**. Search and sort then run inside the query, so they still see every result row, not just the first N.

## Filters and drill-down

Press **=** in the grid to keep only rows whose value in the active column equals the active cell, or **-** to exclude them. Filters match the value as the grid shows it before number formatting, including empty (NULL) cells. In a query, **=** and **-** work on group-by cells and filter the source rows before grouping. Active filters appear as chips under the header; click a chip to remove it, or use **Clear filters**. Filters are saved with a view.

In a query result, press **Enter** on a cell to open the source table filtered to the rows behind it: the row's group-by values, plus the pivot values when the cell is a pivoted measure. The query's slicer selections and filters carry over. Press **U** (Back) to return to the query.

Search words must all appear in a row (any column, case-insensitive). `-word` excludes rows containing it, and `>N`, `<N`, `>=N`, `<=N` match rows where any numeric column compares true. Scope a term to one column with `column:text` (contains), `column=value` (equals), `column!=value`, or `column>N` / `>=` / `<` / `<=`. Number columns compare numerically, and dates and other types compare as that type, so `"order date">=2024-01-01` works. The column name can be any unique prefix (`reg:north` for `region`); quote names or values with spaces (`name:"john smith"`). `-column:text` excludes, and a name matching no column searches the whole word as text, so `10:30` still finds times.

Search words are highlighted in visible cells; column-scoped words are highlighted only in their column. While search or filters narrow the rows, the status bar shows **N of M rows**. Press **Ctrl+G** to find a column by name and move the cursor to it.

## View actions

Actions turn the active row into a lookup: open a web search, a record in an internal site, or a file in another program. Press **Space** then **X** to show the view's actions, then press an action's key to run it for the active row. With no actions yet, **Space X** (or **Edit view actions** in the command palette) opens the editor, where each action has a single-character key (case-sensitive), a description, and a command.

In the command, `{Column}` inserts the active row's value for that column, and `{{` / `}}` insert literal braces. Commands starting with `http://` or `https://` open in the browser, with inserted values URL-encoded, for example `https://www.google.com/search?q={Customer}`. Any other command starts a program directly without a shell: the first word is the program, double quotes group an argument (`code "C:/notes/{Id}.md"`), and an inserted value always stays inside its argument. For shell features such as pipes or `start`, write the shell yourself (`cmd /C start "" "{Path}"`). In the editor, a preview under the command shows it filled in with the active row's values, **Run** tries the command in the form without saving it (each saved action has its own **Run** too), and **Use** loads one of the examples into the form. Actions are saved with the view, so save it (**Space S**) to keep them.

## View history

Press **U** in the grid to go **Back**, or **R** to go **Forward** through files and views opened in that window. The command palette and keymap editor also include Back and Forward. History remembers the current view configuration (including unsaved changes) and search text, with up to 100 entries until the window closes. It stores no data or result tables: returning rebuilds table and pivot results from the source, while SQL text reopens without execution or prior results. Opening another view after going back replaces the forward branch. **Ctrl+R** still reloads the source.

## SQL editor

Press **Ctrl+J** or choose **Open SQL editor** to open a resizable panel below the grid. **Ctrl+Enter** runs selected SQL, or the statement at the cursor; **Run All** executes the script in order. **Stop** interrupts execution. The grid shows the last result set and keeps search, sorting, navigation, and copy/export available without running SQL again.

SQL runs against the active profile database, including imported tables in the `files` schema. Database changes are allowed. Temporary objects and explicit transactions survive between runs in the same SQL document. Run All stops on the first error and follows DuckDB's normal transaction behavior. The row limit caps captured results, including `RETURNING`, without limiting writes; a changed limit applies on the next run.

Completion suggests database tables and DuckDB views after `FROM`, `JOIN`, or simplified `PIVOT`. Typing `a.` suggests columns only when the statement contains an explicit alias such as `FROM files.apples a`, including when it appears below the cursor. Completion does not infer CTE/subquery columns or include saved application views. Use Enter to accept a suggestion; Escape dismisses suggestions before returning focus to the grid.

Save SQL with **Ctrl+S**. Saved scripts appear under **SQL views** and reopen without execution. Results and temporary objects are not saved. Closing the panel keeps its text and connection while the document remains open.

SQL UI captures are available with `--show editor:sql`, `editor:sql-results`, `editor:sql-completion`, or `editor:sql-error`.
