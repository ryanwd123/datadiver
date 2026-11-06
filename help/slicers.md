# Slicers & filters

Slicers are cards in a sidebar that filter the rows by a column. Filters are quick conditions you add from the grid. Both narrow the source rows, so in a query they apply before grouping.

## Adding a slicer

Press **Z** to add a slicer. Pick a kind of card from the list:

- **SQL filter**, always first, takes any SQL condition.
- **Value cards** list a column's values.
- **Condition cards**, listed as *column · condition*, take a typed condition.

## Value cards

A value card lists a column's values when it has at most 1,000 of them. Click, Ctrl+click or drag to select values; rows with any selected value are kept.

The search icon in the card's header opens a filter box: type to narrow the list, press **Enter** to select every matching value, and **Esc** to close it.

Values are greyed out when no row matches the other slicers and filters. Search does not grey them out.

## Condition cards

A condition card works for any column. Type a condition and press **Enter**. Terms are separated by spaces and must all match.

| Condition | Keeps values that |
| --- | --- |
| `>=10 <100` | are at least 10 and under 100 |
| `=North` | equal "North" |
| `!=0` | are not 0 |
| `:smith` or `smith` | contain "smith" |
| `-test` | do not contain "test" |
| `10..20` | are from 10 to 20, inclusive |
| `2024-01-01..` | are on or after a date |
| `..500` | are at most 500 |
| `-30d..today` | fall in the last 30 days |

Numbers compare as numbers and dates as dates. Text ranges ignore case and include every value starting with the upper bound, so `china..italy` includes "Italy". Partial dates cover their whole period: `2024-10` is all of October, and `..2024` runs through December 31, 2024.

Dates can be relative: `today`, `yesterday`, `-30d`, `+2w`, `-6m` and `-1y`. The card shows the column's smallest and largest value.

## The SQL filter

A view can have one SQL filter. Type any SQL condition over the source columns, in the file's [engine](data#query-engines), and press **Enter**:

```sql
amount > 100 AND region <> 'Europe'
```

If a condition or SQL filter has an error, the error shows on its card and the card stops filtering until you fix it.

## Slicers from the keyboard

**Ctrl+L** moves through the cards, and **Tab** and **Shift+Tab** do too once a card has focus. Clicking a value also focuses its card. **Ctrl+D** or **Esc** returns to the grid. In a value card:

| Key | Action |
| --- | --- |
| Up / Down, Page Up / Page Down, Home / End | Move the cursor |
| Space | Select or unselect the value under the cursor |
| Enter | Select only the value under the cursor |
| Left / Right | Page through the values one at a time |
| Shift+Up / Shift+Down | Select a range |
| Ctrl+A | Select every listed value |
| Ctrl+V | Select the values on the clipboard |
| Delete | Clear the card |
| / or any character | Type into the card's filter |

## Slicer options

Two options in **View options** are kept with the file:

- **Cross filter** greys out values no row matches. Turning it off shows every value as available and skips those queries, which is faster on big sources.
- **Available first** lists available values before greyed-out ones.

## Filters from the grid

Press **=** to keep only rows whose value in the active column equals the active cell, or **-** to exclude them. With several cells selected, each selected column is filtered to (or from) its selected values, shown as `in (…)` or `not in (…)`.

Pressing **-** again on the same column adds the value to its `not in` filter. Excluding an empty cell is a filter of its own, `is not null`, and excluding values keeps empty cells. Filters match the value as the grid shows it before number formatting.

Active filters appear as chips under the header; click a chip to remove it, or use **Clear filters**. Filters are saved with a view.

## Filter by pasted values

To filter by a list from somewhere else, such as a column copied from Excel, press **Ctrl+Shift+V** on a cell of the column to filter. An overlay names the column and counts the values on the clipboard, one per line or cell. Press **Ctrl+V** or **Enter** to keep the rows with those values, or first **-** or **Tab** to exclude them instead. The values must match the cells as the grid shows them.

In a focused value card, **Ctrl+V** selects the card's values that match the clipboard, ignoring case, and says how many pasted values the card does not have.

## How filters combine

- Slicers, condition cards, the SQL filter and `=` / `-` filters narrow the **source rows**. In a query they apply before grouping.
- [Search](search) narrows the **displayed rows**. In a query it applies to the result.

Drill-downs carry slicers and filters with them, so the source rows you see match the cell you came from.
