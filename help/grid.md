# Grid

The grid shows a table's rows or a query's result. Single-key commands work while it has focus, as in a spreadsheet's keyboard mode; press **Ctrl+D** to focus it from anywhere.

## Moving and selecting

| Key | Action |
| --- | --- |
| Arrow keys | Move the active cell |
| Ctrl+Left / Ctrl+Right | First or last column |
| Ctrl+Up / Ctrl+Down | First or last row |
| Home / End | First or last column of the row |
| Ctrl+Home / Ctrl+End | First or last cell |
| Page Up / Page Down | Move a screen at a time |
| Shift with any of these | Extend the selection |
| Ctrl+G | Jump to a column by name |

Click to select a cell, and drag or Shift+click to select a range.

## Selection statistics

The status bar summarizes the selected cells: how many, their nulls and unique values, the characters in text, and the sum and mean of numbers. For one number column it adds the median, minimum, maximum and standard deviation. With several columns selected, **Unique rows** counts distinct combinations and turns green when every row is distinct, a quick check that the columns form a unique key.

## Sorting

**[** sorts by the active column ascending and **]** descending. Clicking a column header sorts by it too, and right-clicking the header clears its sort. **Shift+X** clears the sort of a table. In a query result, sorting applies to the result rows.

## Columns

- **F** freezes columns up to the active one, so they stay in view while you scroll right; **Shift+F** unfreezes all.
- **Alt+Left** and **Alt+Right** move the selected columns of a table; they move together and stay in view.
- **Ctrl+K H** hides the selected columns and **Ctrl+K Shift+H** chooses which columns show. Hidden columns keep their place, and a `column:` search term still searches them.
- **(** and **)** narrow or widen the selected columns. **Recalculate column widths** in the command palette fits every column to its contents again.

## Number formatting

**,** turns thousands grouping on or off for the selected columns, and **<** and **>** remove or add decimal places. For more, such as percentages, currency or custom date formats, use `display()` in [conditional formatting](formatting#showing-another-value).

## Cell detail

**D** opens a pane beside the grid with the active cell's whole value, following the cursor. Long text wraps, and JSON objects and arrays are pretty-printed. You can select and copy from the pane.

## Information and column profiles

**I** opens the Info dialog. It profiles every column: nulls, unique count, minimum, maximum and the five most frequent values. It also shows the SQL behind what you see.

## The Helper menu

**Space** in the grid opens the Helper menu. It lists the grid's keys, so it doubles as a reminder of them. They work while the menu is open, and close it; click one to run it. **Space** or **Escape** closes the menu.

## The Command menu

**Ctrl+K** in the grid opens the Command menu, with keys for commands used less often. Press a key to run its command, or click it; **Escape** or **Ctrl+K** closes the menu. Only the menu's own keys work while it is open.

| Key after Ctrl+K | Action |
| --- | --- |
| C / Shift+C | Copy / copy as… |
| S | Save the view |
| / | Search |
| G / A / P / Z | Group By / Aggregations / Pivot By / add a slicer |
| E | Export the result to a file |
| T | Show or hide totals |
| M | Conditional formatting |
| H / Shift+H | Hide the selected columns / choose columns |
| X | Run a view action |

## Copying

- **Ctrl+C** (or **C**) copies the selection as tab-separated text, which pastes into Excel as cells.
- **Ctrl+Shift+C** chooses a format: TSV (raw or as formatted), CSV, JSON records, JSON columns, Python lists, or an HTML or Markdown table, with or without headers. An HTML table pastes as a table into Excel, Word or an email.

## Exporting

**Export result to file** (**Ctrl+K E**) writes the rows on screen, with the search, filters, sort, column order and hidden columns applied. The file name's extension picks the format: `.csv`, `.tsv`, `.parquet`, `.json`, `.ndjson` or `.jsonl`, or `.xlsx`. Text formats can be compressed by adding `.gz` or `.zst`. A query result cut off at the row limit is run again without the limit, so the file is complete.

## The row limit

The **Limit** input beside the search caps how many rows a query returns and how many rows of a file are read into memory. When a result reaches it, the input turns red and the status bar says **Limited to N rows**; search and sort then run inside the query, so they still see every row. Raise the limit to see more.

## Back, forward and undo

- **U** goes back and **R** goes forward through the files and views opened in the window, like a browser. Going back restores the view as it was, including unsaved changes and the search.
- **Ctrl+Z** and **Ctrl+Y** undo and redo changes to the view: grouping, pivots, aggregations, totals, sorting, slicers, filters and column layout. Undo history starts over when you open something else or save.
