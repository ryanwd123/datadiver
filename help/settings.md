# Settings

Make DataDiver comfortable: fonts and themes, your own keys, editor options and where things are stored.

## The Settings list

**Settings** (**Ctrl+,**) gathers everything in one searchable list: the default query engine, fonts, the theme, shortcuts, editor options, AI providers, the DuckDB startup script, views of watched files and the settings folder. **Enter** switches or opens the selected setting.

## Keymap Editor

**Ctrl+Shift+K** opens the Keymap Editor, to rebind any command. Bindings belong to a context, such as the grid, the search box, the SQL editor or the Helper menu, so one key can do different things in different places. The [Shortcuts](shortcuts) tab of this help always shows your keys as they are bound now.

## Themes and fonts

- **Theme Editor** adjusts the theme's colors.
- **Font picker** sets the font of the grid and the rest of the window.
- **Choose editor font** sets the monospaced font of the code editors.
- **Ctrl+=** and **Ctrl+-** make both larger or smaller.

## Editor options

**Settings › Editor options** changes how the SQL editor looks and behaves: the size of completion, signature and hover text, whether completions and signature help open while typing, problem underlines and hover docs, word wrap, line numbers, whitespace, indent guides and highlights, closing brackets and quotes, the blinking cursor, tab size and spaces, and Ctrl+wheel zoom. The options apply to every window.

## The profile folder

Everything DataDiver keeps is in one profile folder, `%LOCALAPPDATA%\datadiver` unless you start it with `--home`. **Open settings directory** opens it in File Explorer.

| Path | Contents |
| --- | --- |
| `datadiver.duckdb` | Anything you create in SQL |
| `settings.json` | Preferences, favorites and AI providers |
| `keymap.json`, `theme.json` | Your shortcuts and theme |
| `duckdb_startup.sql` | The startup SQL script |
| `state.json` | What reopens at startup |
| `views/` | Saved views; archived ones move to `deleted/` |
| `pasted/` | Tables opened from the clipboard |

## Command-line options

| Option | Purpose |
| --- | --- |
| `--file PATH` | Open a file; a path on its own works too |
| `--view PATH` | Open a saved view |
| `--search TEXT` | Start with a search |
| `--slicer COLUMN` | Add a slicer for a column; may be repeated |
| `--home DIRECTORY` | Keep the profile in this folder |
| `--ephemeral` | Use a temporary profile that leaves nothing behind |
| `--no-restore` | Skip reopening the last session |
| `--window WxH` | Set the window size |
| `--rows N` | Open generated sample data |

For example:

```text
datadiver --file "C:\data\sales.csv" --search "North" --slicer "Region"
```
