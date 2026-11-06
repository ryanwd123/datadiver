# Opening data

DataDiver opens CSV, TSV, Excel (`.xlsx`) and Parquet files, tables and views in DuckDB databases, the result of SQL you write, and tables pasted from the clipboard.

## Files

- **Ctrl+O** opens a file. A path on the command line works too.
- **Ctrl+P** (**Files and views**) lists recent files, watched files and folders, saved views, SQL sources and scripts. Type to narrow the list and press **Enter** to open, or **Shift+Enter** to open in a new window.
- **Drag and drop** files onto a window. The first opens there and the rest open in new windows.
- **Ctrl+R** reads the file again, and **Ctrl+Shift+R** reads it again and resets the view.

A file opens as a table that you can scroll, sort, search and filter whole. Open windows notice when a file changes on disk and refresh their grids and pivots.

CSV and TSV files need a header row; column types are detected. Excel files are read from the first worksheet, with a header row. Older `.xls`, `.xlsb` and `.ods` files are not supported.

## Watched files and folders

Watched files are listed under **Sources** for quick opening; nothing is read until you open one.

- Press **N** in the grid and drop files on the overlay, or use **Add watched files** in the command palette, to watch single files.
- **Add folder** watches a whole folder. Its supported files appear under **Ctrl+P** and in the database tree, and new files show up as they arrive.

Each watched file also gets a view in the profile database's `files` schema, so SQL can read it by name: `sales.csv` becomes `files.sales_csv`. Names are lowercase with punctuation replaced by underscores, and a second file with the same name gets a number, such as `files.sales_csv_2`. The view reads the file each time it is queried, so it always shows the file as it is now. **DuckDB: views of watched files** in Settings turns these views off.

## The clipboard

**Ctrl+V** in the grid opens what is on the clipboard: cells copied from Excel or a web page are saved as a TSV or CSV file and open like any other file. Files copied in Explorer open directly.

## Databases

Dropping a DuckDB or SQLite database file on a window attaches it read-only and shows it in the database tree. To write to it, run `DETACH name; ATTACH 'path' AS name;` in the [SQL editor](sql).

The profile database, `datadiver.duckdb` in the profile folder, keeps anything you create in SQL. The [startup script](sql#the-startup-script) can attach other databases every time it opens.

## The database tree

**Ctrl+B** shows or hides a sidebar with:

- saved views and SQL scripts,
- tables and views in the profile database and in attached databases,
- SQL sources, watched files and folders, and recent files,
- DuckDB's `duckdb_*()` metadata functions.

Type to filter the tree, press **Enter** to open the selected item, or **Shift+Enter** to open it in a new window. The refresh button picks up tables created while the tree is open.

**Delete** removes what the selected item stands for: a saved view is archived, a recent file is forgotten, a watched file or folder stops being watched, and an attached database is detached. A table or view in the profile database is dropped after you confirm. Deleting a file inside a watched folder offers to stop watching the folder, since folders are watched whole.

## Query engines

Each file is read by one engine, and everything you do with it, from search and slicers to grouping, totals and export, runs in that engine.

| Engine | Good for |
| --- | --- |
| **Polars** (the default) | Reads files into memory very quickly. Windows showing the same unchanged file share one copy. SQL cards and expressions use Polars SQL. |
| **DuckDB** | SQL cards and expressions can use all of DuckDB's SQL. Reading large files is slower. |

Switch a file with **Query engine** in **View options** (the gear at the end of the search row) or **Choose query engine** in the command palette. The choice is kept with the file, and saved views follow it. **Settings** (**Ctrl+,**) sets the default for files without a choice of their own.

**Preserve order**, for DuckDB, keeps rows in file order. Turning it off reads large files faster.

**Polars: categorical text**, in **View options**, reads a file's text columns as categoricals, which hold each distinct value once. That saves memory, and speeds up grouping, when values repeat, such as regions or product codes. The columns show the type `ENUM`, and search, filters, slicers, grouping, SQL and export work as with plain text. The choice is kept with the file; **Settings** sets the default for files without a choice of their own.

## Read mode

**Read mode**, in **View options**, decides whether a file is read into memory at all.

- **In memory** (the default) reads the file with its engine. The row **Limit** beside the search caps how many rows are read, so a file too big for the machine never fills it; when a file has more rows than the limit, the status bar says **Read the first N rows of the file**.
- **Scan** leaves the file where it is: it opens as a query that DuckDB reads in place on every run, so files too big to hold still group, pivot and filter. Each run reads the file again, so it is slower than in memory.

## Favorites

**Ctrl+Shift+1** through **Ctrl+Shift+9** assigns what is open to a slot, and **Ctrl+1** through **Ctrl+9** opens it again.
