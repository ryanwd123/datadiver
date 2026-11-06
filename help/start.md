# Getting started

DataDiver is a fast desktop tool for exploring data and looking things up. Open a file, a SQL query or a database table, then search, filter, slice, group and pivot it without writing code. Everything you set up can be saved as a view and opened again later.

> Press **F1** anywhere to open this help on the page about what you are doing. **Ctrl+Tab** and **Ctrl+Shift+Tab** (or **Left** and **Right**) move between the tabs above, the arrow and page keys scroll, and **Esc** or **Ctrl+W** closes the window.

## A typical session

1. Press **Ctrl+O** to open a CSV, TSV, Excel or Parquet file, or **Ctrl+P** to pick a recent file, a watched file or a saved view. See [Opening data](data).
2. Press **/** to search the rows. Searches are words, column terms and comparisons; see [Search](search).
3. Press **Z** to add a slicer card that filters by a column, or **=** and **-** to keep or exclude rows like the active cell. See [Slicers & filters](slicers).
4. Press **G** to group, **A** to choose aggregations and **P** to pivot. **T** adds totals. See [Group & pivot](pivot).
5. Press **Enter** on a result cell to drill down to the rows behind it, and **U** to go back.
6. Select cells and press **Ctrl+C** to copy, or **Ctrl+Shift+C** to pick a format such as CSV, JSON or a Markdown table.
7. Press **Ctrl+S** to save everything as a view. See [Views & actions](views).

## The window

- **The search row** along the top holds the search box and, for files read into memory, the row **Limit**. At its right end are the name of what is open, **● Unsaved** when it has unsaved changes, the message log button and the gear for **View options**.
- **The grid** shows the rows or the query result. Most single-key commands work when it has focus; **Ctrl+D** returns focus to it from anywhere.
- **The slicer sidebar** holds slicer cards once you add them.
- **The database tree** (**Ctrl+B**) lists views, tables, SQL sources, scripts and watched files.
- **The SQL editor** (**Ctrl+J**) opens in a panel under the grid for SQL sources and scripts.
- **The cell detail pane** (**D**) shows the whole value of the active cell.
- **The status bar** shows row counts, the selection's statistics and anything that limits the rows.

Messages, such as a finished export or an error, pop up in the top right corner for a moment. The list button in the search row opens a log of every message.

## Finding commands

Every command is in the **command palette**: press **Ctrl+Shift+P** and type part of its name. Commands that have a key show it, so the palette also teaches the shortcuts.

The **Helper menu** opens with **Space** in the grid. It lists the grid's keys; press one to run it, or click it. **Ctrl+K** opens the **Command menu**, with keys for commands used less often. In the SQL editor, **Ctrl+K** opens the **SQL Helper** menu instead, with commands that work on the query at the caret.

The [Shortcuts](shortcuts) tab lists every key as it is bound now, including your own changes from the Keymap Editor.

## Windows

**Ctrl+N** opens a new window and **Ctrl+Q** closes one. Each window has its own file or view, search, history and SQL editor. **Shift+Enter** opens a drill-down or a tree item in a new window, and dropping several files on a window opens the extra ones in windows of their own.

When you start DataDiver again it reopens what was open last, at the same size and place.

## Where to go next

- [Opening data](data): files, folders, the clipboard, databases and query engines.
- [Grid](grid): moving around, selecting, columns, copying and exporting.
- [SQL editor](sql) and [Ask AI](ask-ai): writing queries with completion, checks and help from a model.
- [Formatting](formatting): coloring cells with rules.
- [Settings](settings): fonts, themes, keys and the profile folder.
