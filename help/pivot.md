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

Custom expressions in the Group By, Pivot By and Aggregations managers are written in the same editor as the [SQL panel](sql): it colors the SQL, completes columns after `"` and functions as you type, underlines syntax problems, and follows your **Editor options**.

Each manager, Order By and Having too, opens in a window of its own, which you can move and resize beside the grid so the result stays in view while you build it. The managers share one window size and place: each opens where you left the last one, relative to the main window. The managers stay open for more edits; close them with **Esc** or the close button.

## Order By

Press **O** to open the Order By manager and choose what the rows are sorted by, in priority order, each ascending or descending. **[** and **]** in the grid sort by one column quickly.

Select a key to edit its SQL, or use **+ Custom expression** to sort by an expression over each row, such as an order of your own:

```sql
CASE status WHEN 'open' THEN 0 WHEN 'waiting' THEN 1 ELSE 2 END
length("name")
```

In the list, **[** or **Left** sorts the selected key ascending, **]** or **Right** descending, and **Enter** or a double click flips it. **Ctrl+Enter** runs an edit; one that fails shows its error and keeps the previous sort. A quoted column name, such as `"sales"`, sorts by that column, and the column's header shows it.

## Having

Press **H** in a grouped or pivoted view to open the Having manager, which keeps only the groups that meet SQL conditions, like SQL's `HAVING`. Conditions use the result's columns, after grouping:

```sql
"Count" > 1
"Sum of sales" >= 1000 AND region <> 'Test'
```

Pick a result column on the left and press **Enter** to write its quoted name into the condition. A group stays when every condition holds. **Ctrl+Enter** runs a condition; one that fails shows its error and keeps the last good result. The conditions show as **Having** chips beside the filters, where **✕** removes one, and they see every group even when the row limit cuts the result. Totals still cover all groups. For a quick condition on one column, **W** takes the [condition card](slicers#filtering-aggregation-columns) language instead.

## Totals

**T** shows or hides totals: a grand total row under the grid and, for pivots, a **(Total)** column for each row. Totals aggregate the source rows, so means, medians and distinct counts total correctly rather than summing the cells above.

## Expand and collapse

A query grouped by several fields can be read at different depths, like an Excel PivotTable's outline. Turn **outline mode** on or off from the command menu: **Ctrl+K**, then **O**. It starts in the compact layout, with every expanded item's subtotal on its own row. Turning it off shows the flat result of every field again.

In outline mode:

- **Alt+Shift+Left** collapses every item of the cursor's field into its subtotal row, each still marked **▸** so it expands on its own; the fields before it stay expanded. On a measure column, or pressed again, it collapses the level above. At the top level nothing changes, and the status bar says so. **Alt+Shift+Right** expands every item of the deepest collapsed level, and with nothing collapsed lists the columns to expand every item into.
- **Alt+Left** collapses one item into a single subtotal row, marked **▸**: on a key cell, the item of that column; elsewhere, the row's parent. **Alt+Right** on a ▸ row expands it again, and a click on a **▸** or **▾** marker expands or collapses its item. On a row of the lowest level, **Alt+Right** opens that item alone into the next field: a collapsed one, or a column you choose from a list. Expanded items are marked **▾** where they show a row of their own.

Every subtotal aggregates the source rows, as totals do, so means, medians and distinct counts stay correct. Subtotal rows are bold, and the keys they total over are blank.

The **Outline:** commands in the command palette do the rest:

| Command | What it does |
| --- | --- |
| Outline: Turn outline mode on or off | The same as **Ctrl+K**, **O**. |
| Outline: Choose a layout | **Compact** shows one indented **Row labels** column. **Outline** puts a row above each expanded item and leaves its keys blank on the children. **Tabular** keeps every field in its own column with the keys on every row. |
| Outline: Show or hide this field's subtotals | In the tabular layout, removes or adds the subtotal row above each expanded item of the cursor's field. In the others, blanks or shows the values on the item's own row. |
| Outline: Expand to this field's level | Collapses every item of the cursor's field, leaving the fields before it expanded. Alt+Right then opens items one at a time. |
| Outline: Expand every item | Opens every collapsed item. |

While outline mode is on:

- A search keeps the rows above each match, so a match stays under its parents.
- The rows keep the outline's order, each item above its children, so sorting asks you to turn outline mode off first.
- **Enter** on a subtotal row drills down by its own keys only, and **=** filters by the keys a row shows. In the compact layout, **=** on a label filters by its item's keys.
- Conditional formatting scales read the leaf rows only, so subtotals don't stretch them.
- An export writes every row's keys, whatever the layout shows, with a **Level** column (1 for the first field).

Outline mode, the collapsed fields and items, the layout and subtotals are saved with the view, and **Ctrl+Z** undoes each change.

## Drilling down

Press **Enter** on a result cell to open the source rows behind it: the row's group-by values, plus the pivot values when the cell is a pivoted measure. The query's slicers and filters carry over. Press **U** to return to the query, or **Shift+Enter** to open the drill-down in a new window.

## Big results

A query's result is capped by the row [limit](grid#the-row-limit). For files too big to read into memory, set **Read mode** to **Scan** so grouping runs over every row in place; see [Opening data](data#read-mode).
