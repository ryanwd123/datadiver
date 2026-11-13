# Search

Press **/** in the grid, or **Ctrl+F** anywhere, to search. The grid narrows as you type. Search has a small language of its own; this page describes all of it.

## Words

Every word must appear somewhere in the row, in any column, ignoring case.

| Search | Keeps rows that |
| --- | --- |
| `north` | contain "north" anywhere |
| `new york` | contain both "new" and "york" |
| `"new york"` | contain the phrase "new york" |
| `-closed` | do not contain "closed" |
| `>100` | have a number column greater than 100 |
| `<=0` | have a number column at most 0 |

## Either or

Separate alternatives with `|`. Each side works like a whole search of its own.

| Search | Keeps rows that |
| --- | --- |
| `kansas \| iowa` | contain "kansas" or "iowa" |
| `new york \| kansas` | contain both "new" and "york", or contain "kansas" |

Put a `|` inside double quotes to search for it as text.

## One column

Put a column name before the term to search only that column.

| Term | Means |
| --- | --- |
| `region:north` | region contains "north" |
| `region=North` | region equals "North" |
| `region!=North` | region is not "North" |
| `amount>100` | amount is more than 100; `>=`, `<` and `<=` work too |
| `-region:west` | region does not contain "west" |
| `name:"john smith"` | quote values with spaces |
| `"order date">=2024-01-01` | quote names with spaces |

Number columns compare as numbers, and dates and other types compare as their type, so dates work as you would expect. The column name can be any unique start of a name: `reg:north` searches `region`. A name that matches no column searches the whole word as text, so `10:30` still finds times.

## Comparing two columns

Put the second column's name in double quotes after the operator: `"small bags">"large bags"` keeps rows where the first is larger.

## Completion

Typing `"` lists the matching column names; press **Tab** or **Enter** to insert one. Names are ranked by how well they match.

## What search changes

Search words are highlighted in the visible cells; terms for one column are highlighted only in that column. While a search or filters narrow the rows, the status bar shows **N of M rows**.

Search narrows the rows you see. In a query it applies to the result, after grouping, while slicers and filters narrow the source rows before grouping; see [Slicers & filters](slicers#how-filters-combine).

## Finding without filtering

**Search: find without filtering** (**Ctrl+Shift+F**, or the switch in **View options**) keeps every row and uses the search to find instead. The same language applies, matches are highlighted, and the status bar counts the matching rows.

| Key | Moves to |
| --- | --- |
| **Enter** in the search box, or **F3** anywhere | the next matching row |
| **Shift+Enter** in the search box, or **Shift+F3** | the previous matching row |

Stepping wraps around at the ends and lands on the first cell in the row that shows a match. The status bar shows **Match N of M** while the active cell is on a matching row. Press **Ctrl+Shift+F** again to go back to filtering by the same text. DataDiver remembers the choice.

**Down** moves from the search box to the grid, and **Esc** returns to the grid too.
