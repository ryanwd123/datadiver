# code_editor

A native Windows code editor component modeled on Monaco: plain Win32,
Direct2D and DirectWrite, tree-sitter for syntax. DataDiver hosts it in the SQL
panel with `create_hosted` and `paint_into`.

It started as the `editor` crate of win_monaco (commit `4924080`); this copy is
the maintained one. Paths below that mention the demo host (`src/`, settings)
refer to win_monaco.

```bash
cargo test -p code_editor
cargo run -p code_editor --example hosted
cargo run -p code_editor --example two_editors
```

## Embedding the editor

```rust
use code_editor::{Editor, Event, Options};

let ed = Editor::create(parent_hwnd, Options::default())?;
ed.set_bounds(0, 0, 800, 600);
ed.load_file(std::path::Path::new("query.sql"))?;
ed.add_command("Ctrl+S", 1);
ed.on_event(|ed, e| match e {
    Event::Command(1) => { let _ = std::fs::write("query.sql", ed.to_bytes()); ed.mark_saved(); }
    Event::ContentChanged { changes, .. } => { /* edit deltas, Monaco-style */ }
    _ => {}
});
```

- **Panel lifecycle.** `set_visible(false)` hides the editor but keeps text,
  history, selections and scroll; it stops timers and frees the render target.
  `close()` (or dropping the last handle) destroys it. Focus inside a hidden or
  closed editor moves to the parent.
- **Events** (`on_event`) are delivered after the editor finishes handling a
  message, so handlers can call back into it: content changed (with edit
  deltas), selection changed, focus, host commands, dropped files, warnings
  (invalid bytes on load), language changed.
- **API:** `text`/`set_text`, `load_file`/`load_bytes`/`save_file`/`to_bytes`,
  `apply_edits` (one undo step), `selections`/`set_selections`, `set_options`,
  `set_theme` and `set_theme_color`, `set_language`, `set_read_only`,
  `reveal_line`, `focus`, `add_command`, `bind_key`, `run(Action)`,
  `delta_decorations`, `set_completion_provider`.
- **Decorations** are tracked ranges that move with edits (Monaco's
  `deltaDecorations` and stickiness): line and inline backgrounds, text
  colors, error/warning/info squiggles, gutter markers, hover messages.
- **Completion:** a `CompletionProvider` returns items for the text and caret;
  the popup merges them with the language's keywords and nearby words and
  filters them with `nucleo-matcher`.
- **Hosted mode** for apps that draw everything into one window:
  `Editor::create_hosted(host, ..)`, forward input with
  `handle_message(msg, wparam, lparam)`, and call `paint_into(&rt)` between your
  `BeginDraw`/`EndDraw`. See `editor/examples/hosted.rs`.
- Several editors can run in one process (one UI thread); they share the
  D2D/DWrite factories and compiled grammars. The host's manifest must declare
  PerMonitorV2 DPI awareness, as the demo app's does.

## Editing (Monaco's default keys)

| Keys | Action |
|---|---|
| Ctrl+Z / Ctrl+Y, Ctrl+Shift+Z | Undo / redo (typing is grouped) |
| Ctrl+X / C / V | Cut / copy / paste (whole line when nothing is selected) |
| Ctrl+Left/Right, Ctrl+Backspace/Delete | Word moves and deletes (Monaco word separators) |
| Home | Smart home (first non-space, then line start) |
| Tab / Shift+Tab, Ctrl+] / Ctrl+[ | Indent / outdent |
| Enter | Auto-indent (Python after `:`, dedent after `return`/`pass`/...; brackets split) |
| `(` `[` `{` `"` `'` | Auto-close in code, wrap a selection, type over, delete pairs |
| Ctrl+/ , Shift+Alt+A | Toggle line / block comment |
| Ctrl+Shift+\ | Jump to matching bracket |
| Alt+Click, Ctrl+Alt+Up/Down | Add a caret |
| Shift+Alt+drag | Column selection |
| Ctrl+D, Ctrl+Shift+L | Add next occurrence / select all occurrences |
| Esc | One caret again, then clear the selection |
| Alt+Up/Down, Shift+Alt+Up/Down | Move / copy lines |
| Ctrl+Shift+K, Ctrl+L | Delete line / select line |
| Ctrl+Enter, Ctrl+Shift+Enter | Insert line below / above |
| Shift+Alt+Right/Left | Expand / shrink selection along syntax nodes |
| Ctrl+F, Ctrl+H, F3/Shift+F3 | Find, replace, next/previous match (Alt+C/W/R toggles) |
| Ctrl+Alt+Enter (in find) | Replace all (one undo step) |
| Ctrl+G | Go to line (`line` or `line:column`) |
| Ctrl+Space | Suggestions |
| Ctrl+Shift+[ / ] | Fold / unfold (or click the gutter chevrons) |
| Alt+Z | Toggle word wrap |
| Ctrl+wheel, Ctrl+= / Ctrl+- / Ctrl+0 | Zoom |

## Architecture

```
crates/code_editor/
  examples/       hosted.rs (paint_into), two_editors.rs (child windows)
  queries/        highlights.scm for Python and SQL
  src/
    lib.rs        public API types: Options, Event, Range, ContentChange
    text.rs       rope buffer, EOL/BOM/UTF-16, transactions, offset mapping
    selection.rs  multi-cursor selections
    history.rs    undo groups with typing coalescing
    movement.rs   grapheme/word/line/vertical motion
    commands.rs   pure editing operations: (text, selections) -> transaction
    doc.rs        buffer + selections + history
    lang.rs       LANGS table and detection
    syntax.rs     tree-sitter parsing, highlighting, brackets, folds
    indent.rs     indentation detection, Enter rules, indentation folding
    search.rs     find/replace with regex
    decorations.rs tracked ranges
    keymap.rs     actions, Monaco default keys, host bindings
    view.rs       geometry, scrolling, view-line map (folds, wrap)
    editor.rs     the editor's state and behavior (no window of its own)
    widgets/      find/replace, go to line, suggest, single-line input
    render.rs     Direct2D/DirectWrite painting, layout cache, palettes
    window.rs     child-window and hosted plumbing, the Editor handle
    platform.rs   clipboard, context menu, tracing
    dragdrop.rs   OLE drop target
```

`text`, `selection`, `history`, `movement`, `commands`, `syntax`, `indent`,
`search`, `decorations` and `view` never touch Win32 and are covered by unit
tests; `editor.rs` is tested headless (DirectWrite works without a window),
and window smoke tests drive real messages through a hidden window.

Startup creates only what the first frame needs: the first frames paint on a
software render target while the GPU driver loads on a worker thread, and
highlight queries compile on a worker thread when a file's language is known.
Nothing polls while idle: the caret blinks only while focused, and the other
timers are one-shots.

## Performance

Measured on the release build (Windows 11, 1000x780 window, 96 DPI). Set
`WIN_MONACO_TRACE=1` (OutputDebugString) or `WIN_MONACO_TRACE=<file>` to get
these timings from any build.

| Metric | Target | Measured |
|---|---|---|
| Process start → first frame, small file | ≤ Penmark's, ~< 50 ms | 56–83 ms. About 19 ms of it is the OS showing and activating the window and ~8 ms is DLL loading before `main`. Creating a hardware render target first (Penmark's path) took ~90 ms alone here. |
| Idle CPU | 0 | 0 ms over 5 s, focused (blinking) or minimized |
| Working set, small file | Same order as Penmark | 57 MB (Penmark: 40 MB) |
| Keystroke → frame, 10k-line Python | < 4 ms incl. reparse | 3–4.4 ms typing code (handle ~1.3 ms incl. ~1.2 ms reparse, paint ~1.4 ms). The first edit after opening is slower (~19 ms, tree-sitter's first incremental parse). |
| Keystroke → frame, 100k-line Python | | ~4 ms (reparse deferred to a typing pause) |
| Open a 100k-line file | < 150 ms to first frame | 61–64 ms (first parse on a worker thread) |
| Release binary | record | 4.77 MB both grammars; 2.34 MB Python only; 4.31 MB SQL only; 1.88 MB none |

## Not in this buildout

LSP, the diff editor, the minimap, tabs and split panes (the host's job),
extensions, settings UI, snippets, formatting, code lens, inlay hints.
Other known limits: wrapped rows are not indented like their first row;
word wrap and column selection count CJK characters as one column; completion
providers run synchronously; IME composition is drawn by the system's
composition window.
