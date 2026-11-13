# Group & pivot

Turn a table into a summary without writing SQL: group rows by columns, pivot values into columns, and aggregate with counts, sums, averages and any aggregate the file's engine has.

## Group By

Press **G** to open the Group By manager. Pick the columns to group by; each becomes a column of the result, with a row for each combination of values. Add aggregations to summarize each group.

**Delete** in the grid removes the active group-by column from a query.

## Pivot By

Press **P** to open the Pivot By manager. Pivot dimensions turn their values into columns: pivoting sales by `year` gives a column per year for each aggregation. You can set up pivot dimensions first and then add a group to build the result.

## Aggregations

Press **A** to open the Aggregations manager. Choose a column and a function, such as count, sum, mean, median, minimum or maximum.

The manager has a **Columns / Functions** switch. **Functions** lists the aggregates of the file's engine, every DuckDB aggregate or those Polars SQL has, searchable by name or description, with signatures and examples. Press **Enter** to insert one into the expression.

## Custom expressions

Group By and Pivot By each have **+ Custom expression**, with a SQL editor for the selected entry:

```sql
date_trunc('month', "date") AS month
upper("region") AS region_code
```

Expressions use the source columns, row by row; aggregates and window functions belong in Aggregations. Use `AS` to name the result. Typing `"` lists column names, and typing a word lists the columns, keywords and functions it starts. The editors offer only what the file's engine supports.

## Applying and editing

- **Ctrl+Enter** applies the changes; **Revert** restores the saved entry.
- An invalid edit shows an inline error and keeps the last good grid.
- **Tab** and **Shift+Tab** move between the available columns, the configured entries and the SQL.
- **Alt+Up** and **Alt+Down** reorder the selected entry, and **Delete** removes it.

Each manager opens in a window of its own, which you can move and resize beside the grid so the result stays in view while you build it. A manager's window comes back where you left it relative to the main window. The managers stay open for more edits; close them with **Esc** or the close button.

## Order By

Press **O** to open the Order By manager and choose the columns a query result is sorted by, each ascending or descending. **[** and **]** in the grid sort by one column quickly.

## Totals

**T** shows or hides totals: a grand total row under the grid and, for pivots, a **(Total)** column for each row. Totals aggregate the source rows, so means, medians and distinct counts total correctly rather than summing the cells above.

## Drilling down

Press **Enter** on a result cell to open the source rows behind it: the row's group-by values, plus the pivot values when the cell is a pivoted measure. The query's slicers and filters carry over. Press **U** to return to the query, or **Shift+Enter** to open the drill-down in a new window.

## Big results

A query's result is capped by the row [limit](grid#the-row-limit). For files too big to read into memory, set **Read mode** to **Scan** so grouping runs over every row in place; see [Opening data](data#read-mode).
