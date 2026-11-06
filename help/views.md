# Views & actions

A view saves what you set up on a source, so you can open it again: the search, slicers, filters, grouping, pivots, aggregations, sorting, column layout, formatting and actions.

## Saving views

Press **Ctrl+S** to save a view and give it a name. Saved views are listed in **Ctrl+P** and in the database tree, and open with everything as you left it.

Views store the source's path and the setup, not the data. Opening a view reads its source again, so sharing a view also means making its source available.

The end of the search row shows what is open, and **● Unsaved** when it has changes.

## Auto-save

Turn on **Auto-save** in **View options** (the gear at the end of the search row) and a saved view stores each change two seconds after you make it, and before the window opens something else or closes. Turning it on for an unsaved view asks for a name first.

## View options

The gear at the end of the search row opens **View options**, the settings that apply to what is open:

- the [query engine](data#query-engines), **Read mode**, **Preserve order** and **Polars: categorical text** for a file;
- **Auto-save**, and **Auto-run** for SQL sources;
- the [slicer options](slicers#slicer-options).

**Enter** switches an option in place.

## Archiving and recovering

Deleting a saved view in the database tree, or with **Ctrl+Delete** in **Ctrl+P**, archives it. **Recover archived views and SQL sources** in the command palette brings one back.

## View actions

Actions turn the active row into a lookup: open a web search, a record in an internal site, or a file in another program.

Press **Ctrl+K X** to show the view's actions, then press an action's key to run it for the active row. With no actions yet, **Ctrl+K X** (or **Edit view actions** in the command palette) opens the editor, where each action has a one-character key, a description and a command.

In the command, `{Column}` inserts the active row's value for that column, and `{{` and `}}` insert literal braces.

- Commands starting with `http://` or `https://` open in the browser, with inserted values URL-encoded:

```text
https://www.google.com/search?q={Customer}
```

- Any other command starts a program directly, without a shell. The first word is the program and double quotes group an argument; an inserted value always stays inside its argument:

```text
code "C:/notes/{Id}.md"
```

- For shell features such as pipes or `start`, call the shell yourself: `cmd /C start "" "{Path}"`.

In the editor, a preview under the command shows it filled in with the active row's values, **Run** tries the command without saving it, and **Use** loads one of the examples. Actions are saved with the view.

## History

**U** goes back and **R** goes forward through the files and views opened in the window. History remembers each view's setup, including unsaved changes, and its search, for up to 100 entries until the window closes. It stores no data: going back rebuilds results from the source.

## Undo

**Ctrl+Z** and **Ctrl+Y** (or **Ctrl+Shift+Z**) undo and redo changes to the view: grouping, pivots, aggregations, totals, sorting, slicers, filters and column layout. Undo history starts over when you open something else or save. Text boxes keep their own **Ctrl+Z**.
