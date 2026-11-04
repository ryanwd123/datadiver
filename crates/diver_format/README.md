# diver_format

Conditional formatting rules for DataDiver's grids: a small language between
Python and SQL, with its parser, checker, completion, evaluation and cell
drawing. Only `code_editor` is needed (for the optional `editor` feature),
so it builds in seconds.

```bash
cargo test -p diver_format
cargo run -p diver_format --example demo
cargo run -p diver_format --example demo -- --capture out.png --set 2 --rules "if c > 1 then bg: " --suggest
```

The demo shows format sets (rules plus the columns they apply to), presets,
the rules editor with completion, the problems list and a sample grid.

## The language

Rules are separated by `;` and run in order for each cell of the columns
they apply to. What a later rule sets wins; `stop` skips the rules after it.

```text
-- A comment.
if c > "target" then bg: #c6efce, bold;
if duplicate(c) then bg: mistyrose, text: firebrick;
if c > 0 then icon: up elif c < 0 then icon: down else text: gray;
scale(red, white, green, mid=0);
bar(steelblue, fill=gradient)
```

- `c` is the cell being formatted, `"name"` another column of the same row,
  `'text'` text. Numbers can be `10`, `10%` (0.1) or widths like `2px`.
- Lists and structs: `c[0]` is a list's first item and `c[-1]` its last
  (blank past the end); `c.name`, `"col"."field name"` or `c['name']` is a
  struct's field (blank when there's none). They chain: `c.stops[0].city`.
  A host that lists a `Column`'s fields and items gets them completed
  after `.`.
- Conditions: `= != < <= > >=`, `and or not`, `is [not] null`,
  `[not] between a and b`, `[not] in (a, b)`, `like '%x_'`, `contains`,
  `startswith`, `endswith` (text tests ignore case), `+ - * / %`.
- Functions of a whole column (`c` when left out): `min max avg mean median
  sum count distinct stdev percentile(col, 90%)`, and where this row's value
  stands: `duplicate unique frequency rank percent_rank top(col, n)
  bottom(col, n) top_percent(col, p) bottom_percent(col, p) zscore`.
- Other functions: `abs round floor ceil len lower upper trim coalesce year
  month day weekday today() days_ago(date) row()`.

### Formatting

| Formatting | Example |
|---|---|
| `bg`, `text` | `bg: red`, `text: #9c0006`, `bg: scale(white, green)` |
| `bold`, `italic`, `underline`, `strike` | `bold, italic` |
| `border`, `border_color`, `border_weight`, `border_style` | `border: 2px dashed red` |
| `align` | `align: center` |
| `icon` | `icon: up green` (up, down, right, triangle_up, triangle_down, dash, circle, check, cross, warn, star, star_half, star_empty, flag, square, diamond, bars1–3) |
| `scale(…)` | `scale(red, yellow, green, min=0, mid=, max=, by="other")` |
| `bar(…)` | `bar(green, red, fill=gradient, min=, max=, by=)` |
| `icons(…)` | `icons(arrows, reverse, low=33%, high=67%)` (arrows, triangles, traffic, checks, flags, stars, signal) |
| `sparkline(…)` | `sparkline("q1", "q2", "q3", color=steelblue, kind=bars)` or `sparkline(c)` for a list column |
| `display(…)` | `display(c[0])`, `display(c * 10, '#,##0.00')`, `display(c, 'mmm d, yyyy', blank='-')` |
| `hide_text`, `stop` | `bar(blue), hide_text` |

Colors are the CSS names, `#rgb`, `#rrggbb`, `#rrggbbaa` or `rgb(r, g, b)`.

`display` shows its value in place of the cell's, in an optional Excel
format: number formats with `0 # ? . , %` and `E+00`, commas after the
digits to scale by thousands, literals in `"…"` or after `\`, and up to four
`positive;negative;zero;text` sections (`@` is the text); date formats with
`yyyy yy mmmm mmm mm m dddd ddd dd d hh h mm ss .000 AM/PM`, where `m`
after hours or before seconds is minutes. See [`format::EXAMPLES`](src/format.rs).

## Using it from a host

```rust
let checked = diver_format::check(&rules, &columns);   // Program + problems
let stats = diver_format::TableStats::new(&values);     // or your own Stats
let style = checked.program.style(column, &row, &stats);
let layout = diver_format::draw::layout(&style, cell);  // marks to draw
```

`Program::needs` lists the column statistics the rules read, so a host
whose data lives in a database can compute them ahead with one query.
