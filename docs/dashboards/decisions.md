# Dashboards: key decisions

Decisions made while building the first version of dashboards, with the
reasoning and the options turned down. Each one has a **Your notes** line for
you to agree, disagree, or record why it is right. Decisions are added in the
order they came up, so later ones sometimes refine earlier ones.

---

## 1. One `.ddash` file of HTML, with a JSON manifest inside it

A dashboard is a single HTML file with the extension `.ddash`. The data it may
query is listed in a manifest inside the page:

```html
<script type="application/json" id="datadiver">
{ "title": "Sales", "sources": { "orders": "data/orders.parquet" } }
</script>
```

**Why:** one file is what Claude already writes well and what you can mail,
copy or put next to its data. Its own extension lets Windows send a
double-click to DataDiver without touching `.html`. The manifest lives in the
page, so the file cannot lose it, and a browser ignores it.

**Turned down:** a folder bundle (`dashboard/index.html` + `manifest.json`),
which is harder to share and to double-click; a manifest in an HTML comment,
which editors don't treat as JSON.

**Your notes:**

## 2. The manifest is strict

Unknown fields are errors, and so are bad aliases, duplicate aliases, two
commands on one key, and a newer manifest version. Fields starting with `_`
or `$` are ignored, for notes.

**Why:** a typo like `"sourcse"` would otherwise give a dashboard with no data
and no explanation. Strictness turns that into an error message Claude can
read and fix in one step. Skipping `_` fields leaves room for notes.

**Your notes:**

## 3. Sources are aliases; the page's SQL never names a path

Each source gets an alias, and DataDiver makes a temporary view under that
name, so the page writes `select … from orders`. There are two kinds:

- `"orders": "data/orders.parquet"` or `{"file": "…"}`: a data file. Relative
  paths start at the dashboard's folder; `~` is your home folder.
- `{"profile": "files.orders"}`: a table or view in the profile database, such
  as a watched file's view or a table of a database the startup script
  attaches (Postgres, SQLite, …).

**Why:** moving the data, or switching from a file to a watched file to a
database table, means changing one line of the manifest and none of the
page's SQL. It also lets DataDiver list and check every source before the page
runs.

**Your notes:**

## 4. Files are read the way the profile's file views read them

A `file` source uses the same reader SQL as the views of watched files
(`read_parquet`, `read_csv(…, auto_detect = true)`, `read_xlsx`).

**Why:** the same file should give the same rows in a dashboard, in the SQL
editor, and in `files.*`. A second set of read options would be a second way
for the numbers to differ.

**Limits:** per-file read settings from the grid (the Polars engine, a chosen
delimiter) don't apply. Database files can't be sources directly yet; attach
them in the startup script and use `profile`.

**Your notes:**

## 5. DuckDB only, on the profile database

Every dashboard query runs on DuckDB, on connections to the profile database.

**Why:** the watched files and every database the startup script attaches are
already there; extensions such as Postgres and SQLite come with it; and Claude
only has to know one dialect. Dashboard queries are mostly aggregations over
columns, which DuckDB does well. Sharing the profile database inside one
process also avoids DuckDB's one-process-per-file lock.

**Turned down:** Polars. The manifest has a `version` field for later
changes, so another engine could still be added without breaking old
dashboards.

**Your notes:**

## 6. Queries are read-only, checked twice

A query must be one statement starting with SELECT, WITH, FROM, VALUES,
TABLE, PIVOT, UNPIVOT, SUMMARIZE or DESCRIBE, and it runs inside
`BEGIN TRANSACTION READ ONLY`.

**Why:** the page is code someone else (or a model) wrote. The keyword check
gives a clear message; the read-only transaction makes DuckDB itself refuse
any write that gets past it. Neither one alone is enough.

**Limits:** a SELECT can still call `read_csv('C:/anything')`, so a dashboard
can read files outside its manifest. DuckDB's `enable_external_access` would
stop that, but it applies to the whole profile database. Blocking the network
(decision 11) means such data can be shown but not sent anywhere.

**Your notes:**

## 7. Parameters are bound, and lists become list literals

`datadiver.query(sql, {region: "West"})` binds `$region`; an array binds `?`
and `$1` in order. A parameter holding a list, such as a multi-select
slicer's values, is written into the SQL as a DuckDB list literal
(`['a', 'b']`) with each item escaped.

**Why:** binding means a value can never change the SQL around it. DuckDB's
Rust API cannot bind lists, and slicers need them
(`where region in (select unnest($regions))`), so lists are the one case
written into the SQL, through the SQL lexer, so a `$name` inside a string or
comment is left alone.

**Your notes:**

## 8. Results: rows of values, with numbers that can't round silently

A result is `{columns: [{name, type}], rows, truncated, elapsedMs}`, with
DuckDB's type names. Numbers, booleans and text arrive as JSON values.
Everything else (dates, timestamps, intervals, lists, structs) arrives as the
text DuckDB writes for it, the same text the grid shows. Integers beyond
±2^53 and decimals with more than 15 digits arrive as text, as do NaN and
Infinity.

**Why:** JavaScript numbers are doubles, so `9007199254740993` would quietly
turn into `9007199254740992`. Sending it as text keeps "I can trust the data
output" true even in the browser. Using DuckDB's own text for dates avoids a
second formatter that could disagree with the grid.

**Your notes:**

## 9. A row limit on every query, and a flag when it is reached

Queries return at most 10,000 rows unless they ask for more (up to
1,000,000). When there are more, `truncated` is true.

**Why:** the page should aggregate in SQL, not pull a whole table into the
browser. The flag lets a chart say "showing 10,000 of more" instead of
showing a partial total as if it were complete.

**Your notes:**

## 10. WebView2, started only when a dashboard opens

The page runs in Microsoft's WebView2 (the Edge engine Windows 11 ships),
through the `webview2-com` crate. That crate links the WebView2 loader
statically, so `datadiver.exe` stays one file. Nothing starts until a window
opens a dashboard, and closing it lets the browser processes exit.

**Why:** it is already on every Windows 11 machine, it is the browser Claude's
HTML is tested against, and it keeps the exe portable. The browser runs in its
own processes, so a heavy or broken page can't take the grid down with it.

**Turned down:** `wry` (brings a windowing layer DataDiver doesn't need);
embedding a browser engine (hundreds of MB).

**Your notes:**

## 11. The page may load libraries from a few CDNs and nothing else

Requests to the page's own address and to files beside the dashboard are
answered by DataDiver. Scripts, styles and fonts may come from
`cdnjs.cloudflare.com`, `cdn.jsdelivr.net`, `unpkg.com` and Google Fonts.
Every other request gets a 403, the page has a content security policy with
`connect-src 'self'`, navigation away from the page is cancelled, and popups
don't open.

**Why:** Claude's dashboards often use a chart library from a CDN, and
blocking those would make many of them fail. Blocking everything else means
a page can show data but can't send it anywhere, which is what makes
decision 6's limits acceptable. A library saved next to the dashboard
(`lib/echarts.min.js`) works offline.

**Turned down:** no network at all (breaks most generated dashboards); letting
the manifest widen the list (a shared dashboard could then send data out).

**Your notes:**

## 12. The page is served from `https://dashboard.datadiver.example/`

DataDiver answers the page's requests itself instead of navigating to the
file.

**Why:** a real `https` origin gives the page `fetch`, modules and storage
like any site, and `.example` names can never be real hosts. Serving the file
lets DataDiver set the security policy and reload new HTML without a new
browser. Files beside the dashboard are served too, but nothing outside its
folder (`..` and absolute paths get a 404).

**Your notes:**

## 13. The bridge: `postMessage` with JSON, a small `window.datadiver`

`dashboard_bridge.js` runs before the page's scripts and defines
`datadiver.query`, `sources`, `onCommand` and `ready`. Queries travel as
WebView2 web messages and run on DataDiver's worker pool; answers come back
as messages with the query's id. Results arrive both as `rows` (objects, easy
for charts) and `values` (arrays).

**Why:** it is the simplest channel WebView2 offers and needs no server or
port. Rows travel as compact arrays and become objects in the page.

**Later:** large results could stream as Arrow through a custom URL scheme
without changing `datadiver.query`.

**Your notes:**

## 14. The dashboard replaces the grid area of a normal window

A dashboard opens in a regular DataDiver window: a title row, the file tree
(Ctrl+B) beside the page, and a status bar. The search box, limit and slicers
are gone. Opening a data file from that window shows it in the grid, the way
opening any file replaces what a window shows.

**Why:** it is what you described: the page where the grid is, with the file
tree, command palette and file picker still there.

**Your notes:**

## 15. Palettes and dialogs draw over a picture of the page

WebView2 draws in a child window, which always covers DataDiver's own
drawing. When a palette, dialog, the message log or the keymap editor opens,
DataDiver takes a picture of the page (`PrintWindow`), hides the browser,
draws the picture dimmed, and draws the palette over it. Closing it shows the
live page again and gives it the keyboard back.

**Why:** it looks exactly like a dialog over the grid, needs no change to how
DataDiver draws, and costs nothing while no dialog is open.

**Turned down:** a separate popup window for each palette (a rewrite of focus
handling for every dialog); WebView2's composition mode (DataDiver would have
to move all its drawing to DirectComposition and forward the mouse itself).

**Limits:** while a dialog is open the page doesn't update. Messages that pop
up in a corner would be hidden by the page, so in a dashboard window they show
in the status bar instead.

**Your notes:**

## 16. Keys: DataDiver keeps its own, the page gets the rest

While the page has the keyboard, keys with Ctrl or Alt and F-keys reach
DataDiver first (WebView2's `AcceleratorKeyPressed`). If the key runs a
DataDiver command that works with a dashboard (Ctrl+P, Ctrl+Shift+P, Ctrl+B,
Ctrl+O, Ctrl+N, Ctrl+R, F1, …), DataDiver runs it and the page never sees it.
Everything else, including arrows, Enter, Escape and typing, goes to the page.
Browser shortcuts (print, find, reload, zoom) are turned off.

**Why:** your palette and file picker keys must work from inside a dashboard,
but the page needs its arrows and typing. Leaving plain keys alone means
DataDiver can't break a text box.

**Your notes:**

## 17. Dashboard commands are declared in the manifest

A command has an id, a title and an optional key. The bridge script handles
the key inside the page: a key with Ctrl, Alt or an F-key works anywhere; a
plain key like `j` doesn't fire while a text box has the keyboard. Every
command is listed in the command palette under the dashboard's title, so it
can be found and run without its key. `check` reports a key DataDiver
already uses.

**Why:** declaring commands, instead of the page listening for keys on its
own, is what puts them in the palette and lets DataDiver find clashes.
Handling the keys in the page keeps the page's own focus rules (text boxes)
right.

**Turned down:** VS Code style `when` clauses. The one rule (plain keys don't
fire while typing) covered every case that came up.

**Your notes:**

## 18. Only commands that make sense with a dashboard run in its window

The command palette in a dashboard window lists the dashboard's commands and
the DataDiver commands that don't need the grid (files, tree, open, windows,
reload, help, settings, theme, keymap, watched files, the startup script).
A grid command reached another way, by its key, says it needs data in the
grid.

**Why:** the grid and its session still exist behind a dashboard; letting
"Group by" or "Copy" act on an invisible grid would be confusing.

**Your notes:**

## 19. `datadiver dashboard …`: the agent's tools

`guide` prints the agent guide (`docs/dashboards/agent-guide.md`, built into
the exe). `sources`, `schema` and `query` show the data the way the page will
see it. `check` runs the dashboard in a real but off-screen window, waits
until no query has run for a second, and prints every source, query (SQL,
parameters, time, rows, error) and page error, with `"ok"` and exit code 1
when anything failed. `--capture` saves a PNG of the page. Output is JSON
with one row per line.

**Why:** an agent builds a better dashboard when it can run it, read the
errors and look at it, without you in the loop. The guide lives in the exe
so it always matches the DataDiver that runs the dashboard.

**Your notes:**

## 20. `check` uses an off-screen window, not a hidden one

The check's window is real and shown, but placed beyond the left edge of
every screen, with no taskbar button, and it never takes the keyboard.

**Why:** a hidden WebView2 doesn't paint, and its screenshot never finishes
(found when the first check hung). A check also gives up on a screenshot
after 10 seconds, so it can never hang an agent.

**Your notes:**

## 21. The command line opens its own copy of the profile

`datadiver dashboard query`, `check` and the rest run in their own process
and open the profile database themselves.

**Why:** by default the profile database lives in memory, so a second process
is fine, and an agent's checks don't touch your open windows.

**Limits:** with **DuckDB: keep a profile database** on, DuckDB lets only one
process open the file, so these commands fail while DataDiver runs. The fix
is to send the command to the running DataDiver over decision 22's channel;
not done yet.

**Your notes:**

## 22. One DataDiver per profile: a second launch hands its dashboard over

`datadiver board.ddash` (which is what a double-click runs) first looks for a
DataDiver running with the same profile. If there is one, it sends the path
with `WM_COPYDATA`, lets that process come to the front, and exits; the
running DataDiver opens the dashboard in a new window. Only a dashboard as the
sole argument (with an optional `--home`) is handed over. `--ephemeral` and
scripted runs neither hand over nor accept.

**Why:** what you asked for, and it keeps one process per profile database,
which DuckDB's file lock needs anyway. DataDiver's existing hidden "wake"
window receives the message, so nothing new runs in the background.

**Your notes:**

## 23. The `.ddash` association is per user, and follows a moved exe

**Associate .ddash files with DataDiver** (or `datadiver dashboard register`)
writes `HKCU\Software\Classes`, so no admin rights. On each normal start, if
`.ddash` is registered to a DataDiver exe that no longer exists, the
registration moves to the running exe.

**Why:** DataDiver is a portable exe; moving it would otherwise leave
double-clicks pointing at nothing. Nothing is registered unless you ask. The
"no longer exists" rule came after the first version: that version moved the
registration to whichever DataDiver started last, so running a debug build
would have taken double-clicks away from the release build.

**Your notes:**

## 24. Editing the file reloads it when you come back to the window

When a dashboard window is activated and its file changed, it reloads. Ctrl+R
reloads at once.

**Why:** the usual loop is "Claude edits the file, you switch to DataDiver".
Watching the file would reload it under you while you use it.

**Your notes:**

## 25. Opened dashboards show in the file picker

DataDiver remembers the last 20 dashboards opened (in `history.json`) and
lists them with the other sources in Ctrl+P. Agent checks are not remembered.

**Your notes:**

## 26. Add DataDiver to PATH: the user's PATH, and only a folder of its own

**Add DataDiver to PATH** appends the running exe's folder to the user's
`Path` (`HKCU\Environment`) and tells Windows it changed; **Remove DataDiver
from PATH** takes it off. It refuses a folder that holds other programs
(`.exe`, `.bat`, `.cmd`, `.ps1`), such as Downloads, and says to move
`datadiver.exe` to a folder of its own.

**Why:** agents call `datadiver dashboard …` by name. The user PATH needs no
admin rights and is what installers like rustup use. Every program in a
folder on PATH runs by name, so a shared folder would quietly expose its
other programs.

**Turned down:** copying the exe or a `.cmd` shim into a `bin` folder. A
copy goes stale when you replace the exe, and Git Bash, which Claude Code
uses, doesn't run a `.cmd` by its bare name.

**Limits:** terminals already open, Claude Code included, keep the old PATH
until restarted. Moving the exe afterwards needs the command run again. A
`datadiver` installed with pip comes first if its folder is earlier on PATH.

**Your notes:**

---

## Not in this first version

- **Remote sources and caching** (network drives, URLs, freshness checks).
  Local and UNC files are already read fresh on every query; URL sources and
  a cache are the next layer.
- **Sending command-line work to a running DataDiver** (decision 21).
- **Drill-through** from a chart to the grid, and opening a query in the SQL
  editor (the query log copies it today).
- **A trust prompt for dashboards downloaded from the internet** (the
  `Zone.Identifier` mark).
- **Arrow transport** for large results (decision 13).

