# SQL editor

Write SQL in a panel under the grid, with completion, checks and documentation as you type. The grid shows the result, which you can search, slice, group and pivot like any other source.

## SQL sources and scripts

- A **SQL source** is SQL whose rows you build views on. Save it with **Ctrl+S**; it is listed under **Sources › SQL sources** in the database tree and in **Ctrl+P**. Opening it shows the rows its last statement returns, and each view built on it runs the SQL again when it opens or reloads (**F5**). It should create only temporary tables, or use `CREATE OR REPLACE`.
- A **SQL script** is a scratchpad for statements you run now and then, such as maintenance or one-off checks. Scripts are listed under **SQL scripts**; views are not built on them, and results are not saved.

Start one with **New SQL source** or **New SQL script** in the command palette. **Ctrl+J** shows the editor again for the open source or script, or opens the source behind a view built on one. Drag the line above the panel to resize it.

## Running SQL

| Key | Action |
| --- | --- |
| Ctrl+Enter | Run the selection, or the statement at the cursor |
| Ctrl+Shift+Enter | Run the whole script, in order (**Run All**) |
| Stop | Interrupt the running SQL |

SQL runs on DuckDB against the profile database and attached databases, and can read files with `read_csv('path')`, `read_parquet('path')` or a watched file's view, such as `files.sales_csv`. Temporary tables and open transactions last between runs in the same editor. **Run All** stops at the first error.

The row [limit](grid#the-row-limit) caps the rows a result keeps, without limiting writes. When the last statement is a plain query run outside a transaction, the whole result is kept, so slicers, filters, search and sort see every row.

A SQL source or script can run on **Polars** instead, in **View options**. Polars SQL reads files with `read_csv('…')` and `read_parquet('…')`, and completion and checks then follow Polars SQL.

## Editing

The editor works like VS Code's:

| Key | Action |
| --- | --- |
| Alt+Click, Ctrl+Alt+Up / Down | Add cursors |
| Ctrl+D | Add the next occurrence to the selection |
| Ctrl+Shift+L | Select every occurrence |
| Shift+Alt+drag | Column selection |
| Alt+Up / Alt+Down | Move lines |
| Alt+Shift+Up / Down | Copy lines |
| Ctrl+Shift+K | Delete lines |
| Ctrl+/ | Toggle a line comment |
| Ctrl+F / Ctrl+H | Find / replace, with case, whole word and regex |
| Ctrl+G | Go to a line |
| Ctrl+Shift+[ / ] | Fold / unfold |
| Alt+Z | Toggle word wrap |
| Ctrl+wheel | Zoom |

Right-click for the context menu. **Esc** closes suggestions and signature help, then clears extra cursors or the selection. These keys are in the Keymap Editor's **SQL Editor** context, so they can be rebound.

## Completion

Completion offers what fits the cursor:

- statement keywords;
- tables, views, schemas and databases after `FROM` or `JOIN`;
- columns of the tables in scope, including CTEs, subqueries and tables the script creates;
- functions, types after `CAST(x AS`, settings after `SET` and extensions after `LOAD`;
- after `a.`, the columns of alias `a`, and after `a.col.`, the fields of a struct column;
- after `"`, names to quote;
- inside a `'…'` string, folders and files on disk;
- snippets for common statements.

**Ctrl+Space** opens the list by hand. **Enter** or **Tab** accepts a suggestion.

## Hover, signatures and problems

- **Hover** over a statement keyword for its syntax and examples, or over a table, column or alias to see what it refers to.
- **Signature help** shows the function being called and highlights the argument being typed, after `(` or `,`, or with **Ctrl+Shift+Space**.
- **Problems** are underlined once typing pauses: syntax errors, unterminated strings and tables neither the database nor the script knows. Rest the mouse on one to read it.
- **F12** goes to where a name is defined, **Shift+F12** finds its uses, and **F2** renames it.

## The Helper menu

**Ctrl+K** opens the SQL Helper menu, with commands that work on the query at the caret. Press a key to run one:

| Key | Command |
| --- | --- |
| A | Ask AI for SQL |
| T | Run the query at the caret |
| F | Format the SQL |
| C / I | Move the query to a CTE / inline a CTE |
| W | Wrap in `SELECT * FROM (…)` |
| E / Shift+E | Expand `*` into columns / collapse columns into `*` |
| N | Count the rows |
| P / D | Peek at / describe the table or CTE |
| X / Shift+X | Explain / explain analyze |
| G | Add or switch `GROUP BY` |
| J | Fill in `JOIN … ON` |
| L | Add or remove `LIMIT` |
| V / Shift+V | Paste lines as an `IN` list / rows as a `VALUES` table |
| O | Go to a CTE or statement |
| H | Query history |
| U | Change keyword case |
| Y | Copy as a one-line string |
| R | Show the row counts of each CTE |

## The startup script

**Open startup SQL script** in the command palette edits `duckdb_startup.sql`, which runs whenever the profile database opens. Use it for settings and attachments you always want:

```sql
ATTACH 'C:\data\warehouse.duckdb' AS warehouse (READ_ONLY);
SET file_search_path = 'C:\data';
```

**Reset database connection** starts the DuckDB session over without restarting the app: it detaches every attached database, runs the startup script again and gives the editor a new connection. Use it after editing the startup script.

## Asking AI

**Ctrl+I** opens [Ask AI](ask-ai) beside the editor, to describe a query in words and get SQL that DuckDB has checked.
