@AGENTS.md

## Workspace crates

`crates/diver_sql` (the SQL parser) and `crates/code_editor` (the Monaco-style
editor) are used only by DataDiver, not by other projects. Change their APIs,
behavior and internals as needed for the best outcome in DataDiver; there is
no outside compatibility to keep.
