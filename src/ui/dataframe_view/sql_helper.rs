//! The SQL editor's helper menu (Ctrl+K) and what its keys do: rewrites of
//! the query at the caret, runs of one part of a script, row counts, and
//! lists of places and past runs to go back to.

use super::*;
use crate::application::keymap::{ACTIONS as KEYMAP_ACTIONS, ActionId, canonical_keystroke};
use crate::domain::documents::Engine;
use crate::domain::sql_editor;
use crate::ui::platform::KeyEvent;
use crate::ui::widgets::list::Item;
use crate::infrastructure::json_store::SqlHistoryEntry;
use crate::ui::widgets::hosted_editor::MarkKind;
use code_editor::{DecorationOptions, Severity};
use diver_sql::{EditStyle, QueryNode, RelationSql, Rewrite, TextRange, TextSize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The query a helper command acts on, tinted for a moment.
const FLASH_COLOR: u32 = 0x334A90E2;
/// The statement Ctrl+Enter runs, when the script has several.
const STATEMENT_COLOR: u32 = 0x103794FF;

fn size(offset: usize) -> TextSize {
    TextSize::new(offset as u32)
}

fn span(range: TextRange) -> std::ops::Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

impl DataFrameView {
    /// Opens the helper menu over the SQL editor; its keys run commands on
    /// the query at the caret.
    pub(crate) fn open_sql_helper(&mut self, cx: &mut Cx<Self>) {
        if !self.session.is_sql() {
            return;
        }
        if !self.sql_panel_open {
            self.open_sql_editor(cx);
        }
        let Some(id) = self.sql_panel.as_ref().map(|panel| panel.editor.id) else {
            return;
        };
        self.sql_helper_open = true;
        self.set_focus(Some(id), cx);
        cx.notify();
    }

    pub(crate) fn handle_sql_helper_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) {
        let Ok(stroke) = canonical_keystroke(&event.stroke()) else {
            return;
        };
        let reopen = self.keymap.binding_for(&stroke, KeyContext::SqlEditor) == Some(ActionId::SqlHelperMenu);
        if stroke == "escape" || reopen {
            self.sql_helper_open = false;
            cx.notify();
            return;
        }
        let command = self
            .keymap
            .binding_for(&stroke, KeyContext::SqlHelperMenu)
            .and_then(|action| KEYMAP_ACTIONS.iter().find(|spec| spec.id == action))
            .and_then(|spec| spec.command);
        if let Some(command) = command {
            self.select_sql_helper_command(command, cx);
        }
    }

    pub(crate) fn select_sql_helper_command(&mut self, command: CommandId, cx: &mut Cx<Self>) {
        self.sql_helper_open = false;
        if let Some(id) = self.sql_panel.as_ref().map(|panel| panel.editor.id) {
            self.set_focus(Some(id), cx);
        }
        self.dispatch_command(command, cx);
        cx.notify();
    }

    /// Runs a helper command on the editor's SQL at the caret.
    pub(crate) fn sql_helper_command(&mut self, id: CommandId, cx: &mut Cx<Self>) {
        if !self.session.is_sql() {
            self.copy_status = Some("The SQL helper works in the SQL editor of a SQL source or script".into());
            cx.notify();
            return;
        }
        if !self.sql_panel_open {
            self.open_sql_editor(cx);
        }
        let Some(panel) = &self.sql_panel else {
            return;
        };
        let text = panel.editor.text();
        let caret = panel.editor.cursor();
        let selection = panel.editor.selected_range();
        let style = EditStyle {
            indent: panel.editor.indent_unit(),
        };
        let catalog = panel.catalog.clone();
        let at = size(caret);
        let outcome: Result<(), String> = match id {
            CommandId::SqlExtractCte => self
                .query_node(id, &text, caret, cx)
                .and_then(|node| diver_sql::extract_cte(&text, node.range, &style))
                .map(|rewrite| self.apply_sql_rewrite(&rewrite)),
            CommandId::SqlInlineCte => diver_sql::inline_cte(&text, at, &style).map(|r| self.apply_sql_rewrite(&r)),
            CommandId::SqlWrapQuery => self
                .query_node(id, &text, caret, cx)
                .and_then(|node| diver_sql::wrap_query(&text, node.range, &style))
                .map(|rewrite| self.apply_sql_rewrite(&rewrite)),
            CommandId::SqlFormat => {
                let range = (!selection.is_empty()).then(|| TextRange::new(size(selection.start), size(selection.end)));
                diver_sql::format_sql(&text, range, &style).map(|r| self.apply_sql_rewrite(&r))
            }
            CommandId::SqlExpandStar => with_catalog(&catalog, |c| diver_sql::expand_star(&text, at, c, &style))
                .map(|r| self.apply_sql_rewrite(&r)),
            CommandId::SqlCollapseStar => {
                with_catalog(&catalog, |c| diver_sql::collapse_star(&text, at, c)).map(|r| self.apply_sql_rewrite(&r))
            }
            CommandId::SqlGroupBy => {
                with_catalog(&catalog, |c| diver_sql::group_by(&text, at, c)).map(|r| self.apply_sql_rewrite(&r))
            }
            CommandId::SqlJoinOn => {
                with_catalog(&catalog, |c| diver_sql::join_on(&text, at, c)).map(|r| self.apply_sql_rewrite(&r))
            }
            CommandId::SqlToggleLimit => diver_sql::toggle_limit(&text, at).map(|r| self.apply_sql_rewrite(&r)),
            CommandId::SqlKeywordCase => {
                let range = (!selection.is_empty()).then(|| TextRange::new(size(selection.start), size(selection.end)));
                self.apply_sql_rewrite(&diver_sql::keyword_case(&text, range));
                Ok(())
            }
            CommandId::SqlPasteInList | CommandId::SqlPasteValues => {
                let pasted = crate::ui::platform::system::read_clipboard_text().unwrap_or_default();
                let upper = text.contains("SELECT") || text.contains("FROM");
                let sql = if id == CommandId::SqlPasteInList {
                    diver_sql::in_list(&pasted, &style)
                } else {
                    diver_sql::values_table(&pasted, upper, &style)
                };
                sql.map(|sql| {
                    if let Some(panel) = &mut self.sql_panel {
                        panel.editor.replace(selection.clone(), &sql);
                    }
                })
            }
            CommandId::SqlCopyOneLine => match sql_editor::current_statement(&text, caret) {
                Some(range) => {
                    crate::ui::platform::system::write_clipboard(&diver_sql::one_line_string(&text[range]));
                    self.set_sql_status("Copied the statement as a one-line string".into());
                    Ok(())
                }
                None => Err("Put the caret in a statement".into()),
            },
            CommandId::SqlRunQuery => self.query_node(id, &text, caret, cx).and_then(|node| {
                let sql = diver_sql::runnable_query(&text, node.range)?;
                self.run_helper_sql(sql, node.label(), cx);
                Ok(())
            }),
            CommandId::SqlExplain | CommandId::SqlExplainAnalyze => self.query_node(id, &text, caret, cx).and_then(|node| {
                let sql = diver_sql::runnable_query(&text, node.range)?;
                let analyze = if id == CommandId::SqlExplainAnalyze { "ANALYZE " } else { "" };
                let label = format!("Plan of {}", node.label());
                self.run_helper_sql(format!("EXPLAIN {analyze}{sql}"), label, cx);
                Ok(())
            }),
            CommandId::SqlCountRows => self.query_node(id, &text, caret, cx).and_then(|node| {
                let sql = diver_sql::runnable_query(&text, node.range)?;
                self.count_sql_rows(sql, node.label(), cx);
                Ok(())
            }),
            CommandId::SqlPeek | CommandId::SqlDescribe => self.peek_or_describe(id, &text, caret, cx),
            CommandId::SqlPipelineCounts => self.pipeline_counts(&text, caret, cx),
            CommandId::SqlOutline => {
                self.open_sql_outline(&text, cx);
                Ok(())
            }
            CommandId::SqlHistory => {
                self.open_sql_history(cx);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(message) = outcome {
            self.set_sql_status(message);
        }
        self.handle_sql_panel_events(cx);
        cx.notify();
    }

    fn set_sql_status(&mut self, message: String) {
        if let Some(panel) = &mut self.sql_panel {
            panel.status = message;
        }
    }

    fn apply_sql_rewrite(&mut self, rewrite: &Rewrite) {
        if let Some(panel) = &mut self.sql_panel {
            panel.editor.apply_rewrite(rewrite);
        }
    }

    /// The query at the caret for `command`. The same command again at the
    /// same place takes the query around the last one, then starts over.
    fn query_node(&mut self, command: CommandId, text: &str, caret: usize, cx: &mut Cx<Self>) -> Result<QueryNode, String> {
        let nodes = diver_sql::query_nodes_at(text, size(caret));
        if nodes.is_empty() {
            return Err("Put the caret in a query".into());
        }
        let level = match &self.sql_node_repeat {
            Some((last, last_text, last_caret, level)) if *last == command && last_text == text && *last_caret == caret => {
                (level + 1) % nodes.len()
            }
            _ => 0,
        };
        self.sql_node_repeat = Some((command, text.to_owned(), caret, level));
        let node = nodes[level].clone();
        self.flash_sql_range(span(node.range), cx);
        Ok(node)
    }

    /// Tints `range` of the editor for a moment.
    fn flash_sql_range(&mut self, range: std::ops::Range<usize>, cx: &mut Cx<Self>) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let options = DecorationOptions {
            background: Some(FLASH_COLOR),
            ..Default::default()
        };
        panel.editor.set_marks(MarkKind::Flash, vec![(range, options)]);
        self.sql_flash_revision += 1;
        let revision = self.sql_flash_revision;
        cx.spawn(
            || std::thread::sleep(Duration::from_millis(700)),
            move |view: &mut Self, cx, ()| {
                if view.sql_flash_revision == revision
                    && let Some(panel) = &mut view.sql_panel
                {
                    panel.editor.set_marks(MarkKind::Flash, Vec::new());
                    cx.notify();
                }
            },
        )
        .detach();
    }

    /// Runs SQL the helper made, showing its result in the grid; `label`
    /// names what it runs in the status line.
    fn run_helper_sql(&mut self, sql: String, label: String, cx: &mut Cx<Self>) {
        if self.sql_running {
            return self.set_sql_status("A query is running: wait for it or press Stop".into());
        }
        self.sql_run_label = Some(label);
        self.sql_run_start = None;
        self.sql_limit_rerun = false;
        self.run_sql_text(sql, cx);
    }

    /// Counts the rows of `sql` on the editor's connection and says how many
    /// in the status line, leaving the grid as it is.
    fn count_sql_rows(&mut self, sql: String, label: String, cx: &mut Cx<Self>) {
        if self.sql_engine() == Engine::Polars {
            let sql = format!("SELECT count(*) AS rows FROM (\n{sql}\n)");
            return self.run_helper_sql(sql, format!("Rows of {label}"), cx);
        }
        self.set_sql_status(format!("Counting the rows of {label}…"));
        let service = self.sql_service.clone();
        cx.spawn(
            move || with_service(&service, |s| s.count_rows(&sql)),
            move |view: &mut Self, cx, outcome| {
                let message = match outcome {
                    Ok(rows) => format!("{} has {} {}", capitalize(&label), group_digits(rows), if rows == 1 { "row" } else { "rows" }),
                    Err(error) => format!("Counting {label} failed: {error}"),
                };
                view.set_sql_status(message);
                cx.notify();
            },
        )
        .detach();
    }

    fn peek_or_describe(&mut self, id: CommandId, text: &str, caret: usize, cx: &mut Cx<Self>) -> Result<(), String> {
        let relation = diver_sql::relation_at(text, size(caret));
        let (label, source) = match relation {
            Some((label, RelationSql::Query(range))) => {
                self.flash_sql_range(span(range), cx);
                (label, format!("(\n{}\n)", diver_sql::runnable_query(text, range)?))
            }
            Some((label, RelationSql::From(from))) => (label, from),
            None if id == CommandId::SqlDescribe => {
                let node = self.query_node(id, text, caret, cx)?;
                (node.label(), format!("(\n{}\n)", diver_sql::runnable_query(text, node.range)?))
            }
            None => return Err("Put the caret on a table, file or CTE name to peek at it".into()),
        };
        if id == CommandId::SqlPeek {
            self.run_helper_sql(format!("SELECT * FROM {source}"), format!("Rows of {label}"), cx);
        } else {
            self.run_helper_sql(format!("DESCRIBE SELECT * FROM {source}"), format!("Columns of {label}"), cx);
        }
        Ok(())
    }

    /// Counts the rows of each CTE of the statement at the caret and of its
    /// query, and shows each count after the line it starts on.
    fn pipeline_counts(&mut self, text: &str, caret: usize, cx: &mut Cx<Self>) -> Result<(), String> {
        if self.sql_engine() == Engine::Polars {
            return Err("Row counts of each CTE run on DuckDB: switch the engine to count them".into());
        }
        let steps = diver_sql::pipeline_queries(text, size(caret));
        if steps.is_empty() {
            return Err("Put the caret in a query with CTEs".into());
        }
        self.set_sql_status(format!("Counting the rows of {} queries…", steps.len()));
        let service = self.sql_service.clone();
        let version = text.to_owned();
        cx.spawn(
            move || {
                steps
                    .into_iter()
                    .map(|(name, anchor, sql)| (name, usize::from(anchor), with_service(&service, |s| s.count_rows(&sql))))
                    .collect::<Vec<_>>()
            },
            move |view: &mut Self, cx, counts| {
                let Some(panel) = &mut view.sql_panel else {
                    return;
                };
                // Counts of text that changed meanwhile would sit on the wrong lines.
                if panel.editor.text() != version {
                    return;
                }
                let failed = counts.iter().filter(|(_, _, c)| c.is_err()).count();
                let marks = counts
                    .iter()
                    .map(|(_, anchor, count)| {
                        let after = match count {
                            Ok(rows) => format!("→ {} {}", group_digits(*rows), if *rows == 1 { "row" } else { "rows" }),
                            Err(_) => "→ failed".into(),
                        };
                        let hover = match count {
                            Ok(_) => None,
                            Err(error) => Some(error.clone()),
                        };
                        (*anchor..*anchor, DecorationOptions { after: Some(after), hover, ..Default::default() })
                    })
                    .collect();
                panel.editor.set_marks(MarkKind::Counts, marks);
                panel.status = if failed == 0 {
                    "Row counts after each CTE · they clear when the SQL changes".into()
                } else {
                    format!("{failed} of the counts failed: rest the mouse on one to see why")
                };
                cx.notify();
            },
        )
        .detach();
        Ok(())
    }

    fn open_sql_outline(&mut self, text: &str, cx: &mut Cx<Self>) {
        let outline = diver_sql::outline(text);
        if outline.is_empty() {
            return self.set_sql_status("The editor has no statements yet".into());
        }
        let items = outline
            .iter()
            .map(|item| {
                let label = format!("{}{}", "    ".repeat(item.depth), item.label);
                let detail = match item.kind {
                    diver_sql::OutlineKind::Cte => "CTE",
                    diver_sql::OutlineKind::Statement => "statement",
                };
                Item::new(label).detail(detail)
            })
            .collect();
        let ranges = outline.iter().map(|item| span(item.range)).collect();
        self.palette("Go to CTE or statement", "Type to filter", items, dialogs::PaletteKind::SqlOutline(ranges), cx);
    }

    /// Selects `range` of the editor, after the outline chose it.
    pub(crate) fn go_to_sql_range(&mut self, range: std::ops::Range<usize>, cx: &mut Cx<Self>) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let id = panel.editor.id;
        panel.editor.set_selection(range);
        self.set_focus(Some(id), cx);
        cx.notify();
    }

    fn open_sql_history(&mut self, cx: &mut Cx<Self>) {
        let mut history = self.repository.load_sql_history().unwrap_or_default();
        if history.is_empty() {
            return self.set_sql_status("No SQL has run yet: runs from any editor show here".into());
        }
        history.reverse();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let items = history
            .iter()
            .map(|entry| {
                let words = entry.sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let label = match words.char_indices().nth(120) {
                    Some((i, _)) => format!("{}…", &words[..i]),
                    None => words,
                };
                let mut detail = vec![ago(now.saturating_sub(entry.ran_at_unix_ms))];
                match (&entry.error, entry.rows) {
                    (Some(_), _) => detail.push("failed".into()),
                    (None, Some(rows)) => detail.push(format!("{} {}", group_digits(rows), if rows == 1 { "row" } else { "rows" })),
                    (None, None) => {}
                }
                detail.push(format!("{:.2} s", entry.elapsed_ms as f64 / 1000.0));
                detail.push(entry.engine.clone());
                Item::new(label).detail(detail.join(" · "))
            })
            .collect();
        let sql = history.into_iter().map(|entry| entry.sql).collect();
        self.palette("Query history", "Type to filter past runs", items, dialogs::PaletteKind::SqlHistory(sql), cx);
    }

    /// Puts SQL from the history at the end of the editor, selected, so
    /// Ctrl+Enter runs it.
    pub(crate) fn insert_sql_from_history(&mut self, sql: &str, cx: &mut Cx<Self>) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let current = panel.editor.text();
        let end = current.len();
        let (text, inner) = crate::domain::ai::insertion(&current, end..end, sql);
        panel.editor.replace(end..end, &text);
        panel.editor.set_selection(end + inner.start..end + inner.end);
        let id = panel.editor.id;
        self.set_focus(Some(id), cx);
        self.handle_sql_panel_events(cx);
        cx.notify();
    }

    /// Notes a finished run in the SQL history, on a worker.
    pub(crate) fn record_sql_run(&mut self, sql: String, engine: Engine, elapsed: Duration, rows: Option<u64>, error: Option<String>, cx: &mut Cx<Self>) {
        let repository = self.repository.clone();
        let entry = SqlHistoryEntry {
            sql,
            engine: super::sql_editor::sql_engine_label(engine).into(),
            ran_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()),
            elapsed_ms: elapsed.as_millis() as u64,
            rows,
            error,
        };
        cx.spawn(move || repository.append_sql_history(entry), |_: &mut Self, _, _| {}).detach();
    }

    /// Underlines where DuckDB says a run failed and puts the caret there.
    /// `script` is the SQL that ran, which starts at `start` in the editor.
    pub(crate) fn mark_sql_error(&mut self, script: &str, start: usize, error: &str) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let Some(offset) = error_offset(script, error) else {
            return;
        };
        let text = panel.editor.text();
        let at = start + offset;
        if at > text.len() || !text.is_char_boundary(at) {
            return;
        }
        let word = text[at..]
            .char_indices()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '"' || *c == '.'))
            .map_or(text.len() - at, |(i, _)| i)
            .max(1);
        let end = (at + word).min(text.len());
        let end = (at..=end).rev().find(|e| text.is_char_boundary(*e)).unwrap_or(at);
        let message = error.split("\n\nLINE ").next().unwrap_or(error).trim().to_string();
        let options = DecorationOptions {
            squiggle: Some(Severity::Error),
            hover: Some(message),
            ..Default::default()
        };
        panel.editor.set_marks(MarkKind::RunError, vec![(at..end, options)]);
        panel.editor.set_selection(at..at);
    }

    /// Clears what the last run left in the editor: its error mark.
    pub(crate) fn clear_sql_run_marks(&mut self) {
        if let Some(panel) = &mut self.sql_panel {
            panel.editor.set_marks(MarkKind::RunError, Vec::new());
        }
    }

    /// Tints the statement Ctrl+Enter would run, when there are several and
    /// nothing is selected.
    pub(crate) fn update_statement_mark(&mut self) {
        let Some(panel) = &mut self.sql_panel else {
            return;
        };
        let key = (panel.editor.version(), panel.editor.cursor(), panel.editor.selected_range().is_empty());
        if self.sql_statement_key == Some(key) {
            return;
        }
        self.sql_statement_key = Some(key);
        let text = panel.editor.text();
        let marks = if key.2 && sql_editor::statements(&text).len() > 1 {
            sql_editor::current_statement(&text, key.1)
                .map(|range| {
                    let options = DecorationOptions {
                        line_background: Some(STATEMENT_COLOR),
                        stickiness: code_editor::Stickiness::NeverGrows,
                        ..Default::default()
                    };
                    // A statement's range starts after the `;` before it, on that line.
                    let body = &text[range.clone()];
                    let start = range.start + (body.len() - body.trim_start().len());
                    vec![(start..range.end, options)]
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        panel.editor.set_marks(MarkKind::Statement, marks);
    }
}

fn with_catalog<T>(catalog: &std::sync::RwLock<sql_editor::SqlCatalog>, f: impl FnOnce(&sql_editor::SqlCatalog) -> T) -> T {
    match catalog.read() {
        Ok(catalog) => f(&catalog),
        Err(_) => f(&sql_editor::SqlCatalog::default()),
    }
}

fn with_service<T>(
    service: &std::sync::Mutex<Option<crate::infrastructure::sql_execution::SqlExecution>>,
    f: impl FnOnce(&crate::infrastructure::sql_execution::SqlExecution) -> Result<T, String>,
) -> Result<T, String> {
    let guard = service.lock().map_err(|_| "SQL connection lock poisoned".to_string())?;
    f(guard.as_ref().ok_or_else(|| "SQL connection is still opening".to_string())?)
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn ago(ms: u128) -> String {
    let minutes = ms / 60_000;
    match minutes {
        0 => "just now".into(),
        1..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        _ => format!("{} days ago", minutes / 1440),
    }
}

/// Where in `script` a DuckDB error points: by the statement number the run
/// gives and the `LINE n:` excerpt with its `^` under the spot. The excerpt
/// is found by its text, since DuckDB may have run the statement wrapped.
pub(crate) fn error_offset(script: &str, error: &str) -> Option<usize> {
    let statements = sql_editor::statements(script);
    let (index, message) = match error.strip_prefix("Statement ").and_then(|rest| rest.split_once(": ")) {
        Some((n, message)) => (n.parse::<usize>().ok()?.checked_sub(1)?, message),
        None => (0, error),
    };
    let statement = statements.get(index)?.clone();
    let lines: Vec<&str> = message.lines().collect();
    let at = lines.iter().position(|l| l.starts_with("LINE "))?;
    let (label, excerpt) = lines[at].split_once(": ")?;
    let line_number: usize = label.trim_start_matches("LINE ").trim().parse().ok()?;
    let caret = lines.get(at + 1)?.find('^')?;
    let mut column = caret.checked_sub(label.len() + 2)?;
    let mut excerpt = excerpt;
    if let Some(rest) = excerpt.strip_prefix("...") {
        excerpt = rest;
        column = column.saturating_sub(3);
    }
    let excerpt = excerpt.strip_suffix("...").unwrap_or(excerpt);
    let body = &script[statement.clone()];
    let line_of = |offset: usize| body[..offset].matches('\n').count() + 1;
    let found = body
        .match_indices(excerpt)
        .map(|(i, _)| i)
        .min_by_key(|i| line_of(*i).abs_diff(line_number))?;
    let offset = found + column.min(excerpt.len());
    body.is_char_boundary(offset).then_some(statement.start + offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckdb_errors_point_into_the_statement_that_failed() {
        let script = "select 1;\nselect y\nfrom t";
        let error = "Statement 2: Binder Error: Referenced column \"y\" not found in FROM clause!\n\nLINE 2: select y\n               ^";
        assert_eq!(error_offset(script, error), Some(script.find('y').unwrap()));
        // Wrapped for keeping all rows, the line number is off by one; the text still matches.
        let error = "Statement 2: Binder Error: x\n\nLINE 3: from t\n        ^";
        assert_eq!(error_offset(script, error), Some(script.find("from t").unwrap()));
        assert_eq!(error_offset(script, "Statement 1: Catalog Error: no LINE here"), None);
    }

    #[test]
    fn counts_and_times_read_well() {
        assert_eq!(group_digits(1234567), "1,234,567");
        assert_eq!(group_digits(12), "12");
        assert_eq!(ago(90_000), "1 min ago");
        assert_eq!(capitalize("the statement"), "The statement");
    }
}
