# Dashboards

A dashboard is a web page that shows your data with any charts, filters and controls you like. DataDiver shows it where the grid would be, and every number on it comes from a query DataDiver runs when the page asks, so it is never a stale copy.

## Getting a dashboard

The easiest way is to ask Claude, or another coding agent, to build one: "Use DataDiver to make a dashboard of sales by region with a product filter". Agents learn how from `datadiver dashboard guide`, look at your data with `datadiver dashboard schema`, and test their work with `datadiver dashboard check`, which runs the page out of sight and reports every query and error.

A dashboard is one `.ddash` file. Its data can be files beside it, any file path, or a table in the profile database, such as a watched file's view in `files` or a table of a database the startup script attaches.

## Opening one

- Drag the `.ddash` file onto a window, or use **Open file** (**Ctrl+O**) or the file picker (**Ctrl+P**), which lists the dashboards you have opened.
- **Associate .ddash files with DataDiver** in the command palette makes a double-click in File Explorer open them. If DataDiver is running, the dashboard opens as a new window of it. **Remove the .ddash file association** undoes it.
- From a shell: `datadiver board.ddash`. **Add DataDiver to PATH** lets any terminal, and agents such as Claude, run `datadiver` by name; terminals already open need a restart to see it. It only adds a folder that holds DataDiver alone, since every program in a folder on PATH runs by name.

Opening a data file or view from a dashboard's window shows it in the grid there instead.

## While it shows

- **Ctrl+Shift+P** lists the dashboard's own commands first, with their keys, then the DataDiver commands that work here.
- **Ctrl+P**, **Ctrl+B** (the file tree), **Ctrl+O**, **Ctrl+N** and the other DataDiver keys work even while the page has the keyboard.
- **Ctrl+R** reloads it. Editing the file reloads it the next time you switch to its window.
- The status bar shows how many sources are ready, the last query's time and rows, and failed queries or page errors. **Dashboard: Queries and page messages** lists them; **Enter** copies one.
- **Dashboard: Open developer tools** opens the browser's tools for the page.

## What a page may do

A dashboard's queries can only read: they cannot change a table or a file. The page itself may load scripts and fonts from a few well-known library sites (cdnjs, jsDelivr, unpkg, Google Fonts) and files in its own folder, and reach nothing else on the network.

That doesn't make every dashboard safe to open. Its queries can read any file you can and every database the profile attaches, and DuckDB, which runs them, can reach the network itself, for example to read a URL. So a dashboard could send what it reads somewhere else. Open the ones you made or had an agent make, and ones from people you trust, the way you would run a script.
