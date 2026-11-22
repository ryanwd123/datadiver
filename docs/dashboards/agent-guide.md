# Building a DataDiver dashboard

A DataDiver dashboard is one HTML file with the extension `.ddash`. DataDiver
opens it in a window of its own, in an Edge WebView, and the page gets its
data by sending SQL to DataDiver, which runs it with DuckDB. The page holds no
data itself: build the layout, charts and controls in HTML, CSS and
JavaScript, and ask `window.datadiver.query` for every number.

## The workflow

1. Look at the data first:
   - `datadiver dashboard sources` lists the profile database's tables and
     views with their columns, the databases attached to it, and the path of
     its startup script. `datadiver dashboard sources mypg` lists only names
     containing `mypg`. Watched files are views in the `files` schema.
   - `datadiver dashboard schema data/orders.parquet` (or `files.orders`, or
     `mypg.public.orders`) shows columns, the row count and sample rows.
   - `datadiver dashboard query --source orders=data/orders.parquet "select …" --format table`
     runs SQL the way the dashboard will and prints an aligned table; use
     `--format tsv` to pipe rows on, or leave it out for JSON. Every `--source`
     is `alias=path` or `alias=profile:name`, and a `.ddash` before the SQL
     brings its manifest's sources.
2. Write the `.ddash` file: a manifest naming the sources, and the page.
3. Check it: `datadiver dashboard check board.ddash --capture board.png --full-page`
   opens the page out of sight, waits until its queries finish, and prints
   JSON with every query (time, rows, error; the SQL in full when it failed),
   every `console.error` or uncaught error of the page, and source problems.
   `"ok": true` means nothing failed. `--full-page` captures the whole page,
   not just one screen. Look at the PNG to judge the layout. Fix and check again.
   - To check another state, such as a filter or a date range, add
     `--run "<javascript>"`: it runs in the page once the page settles, and
     the check waits for the queries it starts before the next `--run` or the
     capture. For example `--run "document.querySelector('#range-90d').click()"`.
     The report shows each script's value or the error it threw. Use this
     instead of editing a copy of the dashboard.
4. Tell the user to open it: double-click the file (after
   `datadiver dashboard register`), drag it onto DataDiver, or
   `datadiver board.ddash`. Editing the file while it is open reloads it when
   the window is next activated; Ctrl+R reloads at once.

Commands print JSON on stdout and problems on stderr, and exit with 1 on failure.

### Databases other than files

The profile database is DuckDB with every database its startup script
attaches: Postgres, MySQL, SQLite or other DuckDB files. `dashboard sources`
prints the script's path as `startupScript` (it is `duckdb_startup.sql` in the
profile folder) and what it attaches as `attachedDatabases`. To use a database
that isn't attached yet, add its `INSTALL`/`LOAD` and `ATTACH … (READ_ONLY)`
lines to that file, after asking the user, since it changes their profile and
may hold a password. Then name its tables as profile sources, such as
`{"profile": "mypg.public.orders"}`. The page's queries may run in parallel
against any of them.

## The manifest

Put it in the page as a JSON script with the id `datadiver`:

```html
<script type="application/json" id="datadiver">
{
  "version": 1,
  "title": "Sales overview",
  "sources": {
    "orders": "data/orders.parquet",
    "customers": { "profile": "files.customers" },
    "targets": { "file": "~/Documents/targets.xlsx" }
  },
  "commands": [
    { "id": "reset", "title": "Reset all filters", "key": "ctrl+shift+x" },
    { "id": "next-region", "title": "Next region", "key": "j" }
  ]
}
</script>
```

- **sources**: each key is an alias the page's SQL uses as a table name
  (`select … from orders`). Aliases are letters, digits and `_`.
  - A string or `{"file": "…"}` is a data file: Parquet, CSV, TSV, `.csv.gz`
    or XLSX. Relative paths start at the dashboard's folder, `~` is the home
    folder, and globs such as `data/*.parquet` work for Parquet and CSV.
  - `{"profile": "schema.name"}` is a table or view in the user's profile
    database: a watched file's view (`files.orders`) or a table of a database
    the startup script attaches (`pg.public.customers`). Use the names
    `datadiver dashboard sources` prints.
- **commands** (optional): each appears in DataDiver's command palette
  (Ctrl+Shift+P) under the dashboard's title. `key` is optional, written like
  `ctrl+shift+r`, `alt+1`, `f5` or a single key like `j`. A single key does
  nothing while a text box has the keyboard. Keys DataDiver uses for its own
  commands (Ctrl+P, Ctrl+Shift+P, Ctrl+B, Ctrl+O, Ctrl+N, Ctrl+R,
  Ctrl+Shift+R, F1 and others) stay DataDiver's; `check` reports a command
  whose key is taken.
- **title** (optional): the window's title. Without it, the page's `<title>`.
- Unknown fields are errors, except ones starting with `_` (for notes).

## window.datadiver

DataDiver adds this before the page's scripts run.

```js
const result = await datadiver.query(
  "select region, sum(amount) as total from orders where year = $year group by all order by total desc",
  { year: 2025 }
);
// result.columns: [{ name: "region", type: "VARCHAR" }, { name: "total", type: "DOUBLE" }]
// result.rows:    [{ region: "West", total: 1234.5 }, …]
// result.values:  [["West", 1234.5], …]   the same rows as arrays
// result.truncated: true when the row limit cut the result
// result.elapsedMs: 12.3
```

- `datadiver.query(sql, params?, options?)` returns a promise.
  - One statement per call, and it must read: SELECT, WITH, FROM, VALUES,
    TABLE, PIVOT, UNPIVOT, SUMMARIZE or DESCRIBE. Anything that writes fails.
  - Write `as` before every column alias. DuckDB rejects a bare alias that is
    a keyword, such as `any_value(employee_name) name` or `count(*) weeks`.
  - `params` binds `$name` placeholders from an object, or `?` / `$1` from
    an array. Always bind values from controls instead of building SQL from
    strings. A list binds as a DuckDB list, for multi-selects:
    `where region in (select unnest($regions))` with `{regions: ["West", "East"]}`.
    `where len($regions) = 0 or region in (select unnest($regions))` lets an
    empty selection mean "all"; use `$regions::VARCHAR[]` if DuckDB cannot
    tell the type of an empty list.
  - `options.limit` caps the rows (10,000 by default, at most 1,000,000).
    Aggregate in SQL; don't fetch raw rows to sum them in JavaScript.
  - A failed query rejects with an `Error` whose `message` is DuckDB's error
    and whose `sql` is the query. Show it in the page where the chart would be.
- **Values**: numbers, booleans and text arrive as JSON. Dates, timestamps,
  times, intervals, lists and structs arrive as text the way DuckDB writes
  them (`2025-03-01`, `2025-03-01 14:05:00`). Integers beyond ±2^53 and
  decimals with more than 15 digits arrive as text so they never round; use
  `::DOUBLE` in SQL if a chart needs a number.
- `datadiver.sources()` resolves to `[{alias, kind, location, error?}]`.
- `datadiver.onCommand(id, handler)` runs `handler` when the manifest command
  `id` runs, from its key or the palette. It returns a function that removes
  the handler.
- `datadiver.ready` resolves once DataDiver has sent the settings;
  `query` works before it.

## The page

- Write one self-contained file: inline CSS and JavaScript. Plain SVG or
  canvas charts work offline and look sharp; libraries may load from
  `cdnjs.cloudflare.com`, `cdn.jsdelivr.net` or `unpkg.com` with an exact
  version, such as `https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js`.
  Fonts may come from Google Fonts. Files next to the dashboard load by
  relative path (`<script src="lib/echarts.min.js">`), which also works offline.
- No other network: `fetch` to other hosts, WebSockets, iframes and form
  posts are blocked, and links don't navigate. All data comes through
  `datadiver.query`.
- The page fills the window below DataDiver's title bar, beside the file tree
  when that is open; design for widths from about 900 to 2500 pixels. Use
  `system-ui` or `Segoe UI`, and respect `prefers-color-scheme` if you can.
- Show loading and error states per chart, and show the row count or a note
  when `truncated` is true, so the user can trust what is on screen.
- Run independent queries in parallel (`Promise.all`); re-run only the
  queries a control affects when it changes.
- To remember choices between sessions, such as filters or a split
  position, use `localStorage`. Every dashboard shares one storage, so start
  each key with a name of the dashboard's own (`hours.range`), and treat a
  missing or unreadable value as the default. `check` starts each run with
  empty storage, so it shows what a first-time viewer sees and its `--run`
  steps never change what the user saved.

## A complete example

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Sales by region</title>
<script type="application/json" id="datadiver">
{ "version": 1, "title": "Sales by region",
  "sources": { "sales": "sales.csv" },
  "commands": [{ "id": "clear", "title": "Show all regions", "key": "ctrl+shift+a" }] }
</script>
<style>
  body { font: 14px system-ui, "Segoe UI", sans-serif; margin: 24px; color: #1f2328; }
  .bar { display: flex; align-items: center; gap: 8px; margin: 4px 0; }
  .bar span { width: 120px; } .bar div { background: #4f7cff; height: 18px; }
  .error { color: #b33c3c; white-space: pre-wrap; }
</style>
</head>
<body>
<label>Region <select id="region"><option value="">All</option></select></label>
<h2 id="total">…</h2>
<div id="chart"></div>
<script>
const regionSelect = document.getElementById("region");

async function loadRegions() {
  const { rows } = await datadiver.query("select distinct region from sales order by 1");
  for (const { region } of rows) regionSelect.add(new Option(region, region));
}

async function render() {
  const params = { region: regionSelect.value };
  const where = "where $region = '' or region = $region";
  try {
    const [total, byMonth] = await Promise.all([
      datadiver.query(`select sum(amount) as total from sales ${where}`, params),
      datadiver.query(`select strftime(day::date, '%Y-%m') as month, sum(amount) as amount
                       from sales ${where} group by 1 order by 1`, params),
    ]);
    document.getElementById("total").textContent = `Total ${total.rows[0].total ?? 0}`;
    const max = Math.max(...byMonth.rows.map((row) => row.amount), 1);
    document.getElementById("chart").innerHTML = byMonth.rows
      .map((row) => `<div class="bar"><span>${row.month}</span><div style="width:${(row.amount / max) * 400}px"></div>${row.amount}</div>`)
      .join("");
  } catch (error) {
    document.getElementById("chart").innerHTML = `<p class="error">${error.message}</p>`;
  }
}

regionSelect.addEventListener("change", render);
datadiver.onCommand("clear", () => { regionSelect.value = ""; render(); });
loadRegions().then(render);
</script>
</body>
</html>
```
