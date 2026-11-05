# Formatting

Conditional formatting colors cells, makes them bold, and draws data bars, color scales, icons and sparklines, from short rules you type.

## The formatting window

Press **M** (or **Space M**) in the grid to open the formatting manager. It opens in a window of its own beside the grid, and changes show in the grid as you type. It comes back where you left it.

A view has any number of **format sets**. Each set is some rules and the columns they apply to; the active column's set opens first. Pick a rule from **Add a preset** to start from a common one.

## Rules

Rules are separated by `;` and run in order for each cell of the set's columns. What a later rule sets wins, and `stop` skips the rules after it.

```text
-- A comment.
if duplicate(c) then bg: #ffc7ce, text: #9c0006;
if c > "target" then bold, border: 1px green;
if c > 0 then icon: up elif c < 0 then icon: down else text: gray;
scale(red, white, green, mid=0);
bar(steelblue, fill=gradient)
```

- `c` is the cell being formatted, `"name"` another column of the same row, and `'text'` is text.
- Numbers can be `10`, `10%` (0.1) or widths like `2px`.
- For list and struct columns, `c[0]` is the first item and `c[-1]` the last; `c.field` or `"name"."field name"` is a struct's field. They chain: `c.stops[0].city`.

## Conditions

| Kind | Examples |
| --- | --- |
| Comparisons | `= != < <= > >=` |
| Logic | `and`, `or`, `not` |
| Blanks | `is null`, `is not null` |
| Ranges and lists | `between 1 and 10`, `in ('a', 'b')` |
| Text | `like '%x_'`, `contains`, `startswith`, `endswith` (ignoring case) |
| Arithmetic | `+ - * / %` |

## Functions

- **Over the whole column** (`c` when left out): `min`, `max`, `avg`, `mean`, `median`, `sum`, `count`, `distinct`, `stdev`, `percentile(col, 90%)`.
- **Where this row stands**: `duplicate`, `unique`, `frequency`, `rank`, `percent_rank`, `top(col, n)`, `bottom(col, n)`, `top_percent(col, p)`, `bottom_percent(col, p)`, `zscore`.
- **Others**: `abs`, `round`, `floor`, `ceil`, `len`, `lower`, `upper`, `trim`, `coalesce`, `year`, `month`, `day`, `weekday`, `today()`, `days_ago(date)`, `row()`.

Whole-column tests count the rows the slicers and filters leave, not a search's.

## Formatting

| Formatting | Example |
| --- | --- |
| `bg`, `text` | `bg: red`, `text: #9c0006`, `bg: scale(white, green)` |
| `bold`, `italic`, `underline`, `strike` | `bold, italic` |
| `border` | `border: 2px dashed red` |
| `align` | `align: center` |
| `icon` | `icon: up green`, `icon: check`, `icon: star` |
| `scale(…)` | `scale(red, yellow, green, min=0, mid=, max=, by="other")` |
| `bar(…)` | `bar(green, red, fill=gradient, min=, max=, by=)` |
| `icons(…)` | `icons(arrows, reverse, low=33%, high=67%)` |
| `sparkline(…)` | `sparkline("q1", "q2", "q3", kind=bars)` or `sparkline(c)` |
| `hide_text`, `stop` | `bar(blue), hide_text` |

Colors are CSS names, `#rgb`, `#rrggbb`, `#rrggbbaa` or `rgb(r, g, b)`. Icon sets for `icons()` are arrows, triangles, traffic, checks, flags, stars and signal.

## Showing another value

`display(value, format)` shows another value in the cell in place of its own. A list column can show its first item and keep the rest for conditions:

```text
display(c[0]); if c[1] = 'late' then bg: mistyrose
```

The optional format is Excel's, such as `'#,##0.00'`, `'0.0%'`, `'$#,##0;($#,##0)'`, `'#,##0,"K"'`, `'YYYY-MM-DD'` or `'mmm d, yyyy h:mm AM/PM'`. `blank='-'` sets what shows when the value is blank. Copying and search still use the cell's own value.

## Help while typing

- **Ctrl+Space** lists what can come next.
- Typing `"` lists column names, `.` lists a struct's fields, and `#` lists colors.
- Problems are underlined and listed under the rules.

A plain file remembers its formats; a view saves them with everything else.

## Picking colors

**Colors ◂** at the top right opens the colors panel (**Hide ▸** puts it away; the window remembers which). With the caret on a `#…` color, the panel shows that color and changing it there changes the rule, so the grid follows as you drag. One drag undoes as one step. Elsewhere, choose a color and press **Insert** to put it at the caret. You can also type a hex in the panel's box, or click a swatch: the colors already in these rules, or the palette.
