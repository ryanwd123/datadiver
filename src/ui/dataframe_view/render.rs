//! Drawing the window: the toolbar, the body with its panes, the status bar,
//! and overlays.

use super::*;
use crate::ui::hit::{Button, Splitter};
use crate::ui::platform::{Align, Canvas, Color, Cursor, Rect, rect, text};
use crate::ui::space_menu::{SpaceMenu, paint_action_menu};
use crate::ui::status;
use crate::ui::widgets::{ButtonStyle, Row, THINKING_FRAME, button, button_height, button_width, colors, thinking_dots};

const LIMIT_WARNING: Color = Color::rgb(0xff0000);

impl DataFrameView {
    pub(crate) fn render(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        self.frames.set(self.frames.get() + 1);
        self.sync_completion_columns();
        if self.sql_panel_open {
            self.update_statement_mark();
        }
        self.drain_grid_events(cx);
        self.sync_notices(cx);
        // A removed slicer card, or a closed filter, takes its focus with it.
        if self.slicer_sidebar.lost_focus(self.focus) {
            self.focus_grid(cx);
        }
        canvas.clear(colors::BACKGROUND);
        if self.keymap_editor.is_some() {
            let fonts = self.fonts.clone();
            let focus = self.focus;
            let bounds = canvas.bounds();
            if let Some(editor) = &mut self.keymap_editor {
                editor.paint(canvas, bounds, &fonts, focus);
            }
            return;
        }
        let modal = self.modal_dialog() || self.cache_drop_open;
        let mouse = canvas.mouse();
        if modal {
            canvas.set_mouse(None);
        }
        let fonts = self.fonts.clone();
        let font = &fonts.ui;
        let line = text::line_height(font);
        let bounds = canvas.bounds();
        let row_count = self.session.result.row_count();
        let limit_reached = self.limit_reached();
        // Header.
        let input_height = Editor::line_box_height(font);
        let header = rect(bounds.x, bounds.y, bounds.w, input_height + 8.0);
        let mut items = Row::new(header.inset_xy(12.0, 4.0), 8.0);
        let button_top = header.y + (header.h - button_height(font)) / 2.0;
        let gear = items.right(button_height(font));
        let gear = rect(gear.x, button_top, gear.w, button_height(font));
        button(canvas, gear, "", font, ButtonStyle::Ghost, true, Hit::Button(Button::ViewOptions));
        paint_gear(canvas, gear, colors::FOREGROUND);
        canvas.hit_with(gear, Hit::Button(Button::ViewOptions), Cursor::Arrow, Some("View options"));
        let log_button = items.right(button_height(font));
        self.notices
            .paint_button(canvas, rect(log_button.x, button_top, log_button.w, button_height(font)), font);
        let search_width = 320.0_f32.min((items.rest().w * 0.4).max(128.0));
        let search = items.left(search_width);
        let search_id = self.search_input.id;
        let focus = self.focus;
        self.search_input
            .paint(canvas, search, font, focus == Some(search_id), |part| Hit::Editor(search_id, part));
        if self.limit_applies() {
            let label = items.left(text::measure("Limit", font).0 + 2.0);
            canvas.text(label, "Limit", font, if limit_reached { LIMIT_WARNING } else { colors::FOREGROUND }, Align::Left);
            let limit = items.left(128.0);
            let limit_id = self.limit_input.id;
            self.limit_input
                .paint(canvas, limit, font, focus == Some(limit_id), |part| Hit::Editor(limit_id, part));
            if limit_reached {
                canvas.stroke_rounded(limit, 5.0, LIMIT_WARNING, 3.0);
            }
        }
        if !self.session.sort.is_empty() {
            let r = items.left(button_width("Clear sort", font));
            button(canvas, rect(r.x, button_top, r.w, button_height(font)), "Clear sort", font, ButtonStyle::Outline, true, Hit::Button(Button::ClearSort));
        }
        if self.session.has_filters() {
            let r = items.left(button_width("Clear filters", font));
            button(canvas, rect(r.x, button_top, r.w, button_height(font)), "Clear filters", font, ButtonStyle::Outline, true, Hit::Button(Button::ClearFilters));
        }
        if self.loading || self.busy {
            let now = cx.now();
            let since = *self.thinking_since.get_or_insert(now);
            let size = (line * 1.2).round();
            let r = items.left(size + 8.0);
            let dots = rect(r.x + 8.0, r.y + (r.h - size) / 2.0, size, size);
            thinking_dots(canvas, dots, now.saturating_duration_since(since), colors::SELECTED_STRONG);
            let label = if self.loading { "Loading…" } else { "Searching…" };
            canvas.hit_with(dots, Hit::Surface, Cursor::Arrow, Some(label));
            if !self.thinking_frame.as_ref().is_some_and(Task::is_pending) {
                self.thinking_frame = Some(cx.timer(THINKING_FRAME, |_, cx| cx.notify()));
            }
        } else {
            self.thinking_since = None;
            self.thinking_frame = None;
        }
        self.paint_title(canvas, items.rest());
        let mut top = header.bottom();
        // Value filter chips, then the conditions on a query's result.
        if !self.session.value_filters().is_empty() || !self.session.result_filters().is_empty() {
            let small = &fonts.small;
            let chip_height = button_height(small);
            let mut x = bounds.x + 12.0;
            let mut y = top + 4.0;
            let label_width = text::measure("Filters", small).0;
            canvas.text(rect(x, y, label_width + 4.0, chip_height), "Filters", small, colors::MUTED, Align::Left);
            x += label_width + 12.0;
            let value_chips = self
                .session
                .value_filters()
                .iter()
                .enumerate()
                .map(|(index, filter)| (filter.label(), Button::RemoveValueFilter(index)));
            let result_chips = self
                .session
                .result_filters()
                .iter()
                .enumerate()
                .map(|(index, filter)| (filter.label(), Button::RemoveResultFilter(index)));
            for (label, remove) in value_chips.chain(result_chips) {
                let label = format!("{label}  ✕");
                let width = button_width(&label, small).min(bounds.w - 24.0);
                if x + width > bounds.right() - 12.0 && x > bounds.x + 12.0 + label_width + 12.0 {
                    x = bounds.x + 12.0 + label_width + 12.0;
                    y += chip_height + 6.0;
                }
                button(canvas, rect(x, y, width, chip_height), &label, small, ButtonStyle::Outline, true, Hit::Button(remove));
                x += width + 8.0;
            }
            top = y + chip_height + 4.0;
        }
        let pivot_needs_setup = self.session.working.as_ref().is_some_and(|view| {
            matches!(&view.view, ViewKind::Query(config) if !config.pivot_by.is_empty() && config.group_by.is_empty())
        });
        if pivot_needs_setup {
            canvas.text(rect(bounds.x + 12.0, top + 4.0, bounds.w - 24.0, line + 8.0), "Add a group by to set up this pivot", font, colors::FOREGROUND, Align::Left);
            top += line + 16.0;
        }
        // Status bar.
        let status_height = line + 8.0;
        let status_rect = rect(bounds.x, bounds.bottom() - status_height, bounds.w, status_height);
        self.paint_status(canvas, status_rect, row_count, limit_reached);
        // Body.
        let body = rect(bounds.x, top, bounds.w, (status_rect.y - top).max(0.0));
        self.paint_body(canvas, body, cx);
        // Overlays.
        if self.focus == Some(search_id) {
            self.search_completion
                .paint(canvas, self.search_input.caret_rect(), font, search_id);
        }
        if self.space_menu_open || self.command_menu_open {
            let menu = SpaceMenu {
                sql: self.session.is_sql(),
                sql_source: self.sql_source_session(),
                sql_slicers: self.sql_result_complete,
                table: self.is_table(),
                has_sort: !self.session.sort.is_empty(),
            };
            if self.space_menu_open {
                menu.paint(canvas, &fonts, &self.keymap);
            } else {
                menu.paint_commands(canvas, &fonts, &self.keymap);
            }
        }
        if self.sql_helper_open
            && let Some(panel) = &self.sql_panel
        {
            crate::ui::sql_helper_menu::paint(canvas, &fonts, &self.keymap, panel.editor_area);
        }
        if self.action_menu_open {
            let actions = self
                .view_actions()
                .into_iter()
                .map(|action| (action.key, action.description))
                .collect::<Vec<_>>();
            paint_action_menu(canvas, &fonts, &actions);
        }
        self.notices.paint_log(canvas, bounds, header.bottom(), &fonts);
        if modal {
            canvas.set_mouse(mouse);
        }
        if self.cache_drop_open {
            self.paint_cache_drop(canvas);
        }
        self.paint_dialog(canvas, cx);
        // Messages pop up over everything, dialogs included.
        let now = cx.now();
        self.notices.paint_toast(canvas, bounds, header.bottom(), &fonts, now);
        // Painting the grid can measure column widths; record them now.
        self.drain_grid_events(cx);
    }

    /// Pops up the messages that changed since the last frame, and redraws
    /// when the toast's time is up.
    fn sync_notices(&mut self, cx: &mut Cx<Self>) {
        use crate::ui::notices::NoticeSource;
        let now = cx.now();
        let statistics = self.statistics_error.as_ref().map(|error| format!("Statistics failed: {error}"));
        for (source, message) in [
            (NoticeSource::Error, self.error.as_deref()),
            (NoticeSource::TablePreferences, self.table_preferences_error.as_deref()),
            (NoticeSource::Limit, self.limit_error.as_deref()),
            (NoticeSource::Status, self.copy_status.as_deref()),
            (NoticeSource::Statistics, statistics.as_deref()),
        ] {
            self.notices.observe(source, message, now);
        }
        self.notices.expire(now);
        let deadline = self.notices.deadline();
        if self.notice_timer.as_ref().map(|(at, _)| *at) != deadline {
            self.notice_timer = deadline.map(|at| (at, cx.timer(at.saturating_duration_since(now), |_, cx| cx.notify())));
        }
    }

    /// The end of the search row: what is open, whether it is saved, and
    /// when its file changed. When the row is narrow the file time goes
    /// first, then the saved state, and the title is cut off last.
    fn paint_title(&self, canvas: &mut Canvas<Hit>, r: Rect) {
        let font = &self.fonts.ui;
        let title = self
            .session
            .working
            .as_ref()
            .map(|view| view.name.clone())
            .or_else(|| self.session.source_path.as_deref().map(|path| source_file_name(std::path::Path::new(path))))
            .unwrap_or_else(|| "DataDiver".into());
        let mut labels = vec![(title, font.semibold(), colors::FOREGROUND)];
        if self.session.dirty {
            labels.push(("● Unsaved".into(), font.clone(), colors::WARNING));
        }
        if let Some(modified) = &self.source_modified {
            labels.push((format!("Modified {modified}"), font.clone(), colors::MUTED));
        }
        const GAP: f32 = 12.0;
        let widths = |labels: &[(String, crate::ui::platform::Font, Color)]| {
            labels.iter().map(|(label, font, _)| text::measure(label, font).0 + 4.0 + GAP).sum::<f32>() - GAP
        };
        while labels.len() > 1 && widths(&labels) > r.w {
            labels.pop();
        }
        let mut x = (r.right() - widths(&labels)).max(r.x);
        canvas.push_clip(r);
        for (label, font, color) in &labels {
            let width = text::measure(label, font).0 + 4.0;
            canvas.text(rect(x, r.y, width.min(r.right() - x), r.h), label, font, *color, Align::Left);
            x += width + GAP;
        }
        canvas.pop_clip();
    }

    /// Whether the row limit applies to what is open: a query's result, or a
    /// file read into memory.
    pub(crate) fn limit_applies(&self) -> bool {
        !self.is_table() || self.file_in_memory().is_some() || self.sql_source().is_some()
    }

    /// Whether the file in memory was read in file order, so rows cut at the
    /// limit are its first ones. Only DuckDB can read out of order.
    pub(crate) fn read_in_file_order(&self) -> bool {
        self.session.source.in_file_order
    }

    /// The saved SQL source this view reads its rows from.
    pub(crate) fn sql_source(&self) -> Option<String> {
        self.session.working.as_ref()?.sql_source.clone()
    }

    /// The file whose rows were read into memory, while one is open.
    pub(crate) fn file_in_memory(&self) -> Option<PathBuf> {
        self.session
            .source
            .source_identity
            .as_ref()
            .and(self.session.source_path.as_ref())
            .map(PathBuf::from)
    }

    /// Whether the shown rows stopped at the row limit: the file read into
    /// memory has more rows, or the query does. A query is judged by its own
    /// rows, before search narrowed them, so the warning stays while
    /// searching a truncated result. A SQL run knows whether it stopped.
    pub(crate) fn limit_reached(&self) -> bool {
        if self.session.is_sql() {
            return !self.busy && !self.loading && self.error.is_none() && self.sql_result_truncated;
        }
        let query_cut = self.query_cache.as_ref().is_some_and(|cache| cache.frame.cut_at.is_some());
        !self.busy
            && !self.loading
            && self.error.is_none()
            && (self.session.source.cut_at.is_some() || !self.is_table() && query_cut)
    }

    fn paint_status(&mut self, canvas: &mut Canvas<Hit>, r: Rect, row_count: usize, limit_reached: bool) {
        let font = &self.fonts.ui;
        let column_count = self.session.result.column_count();
        // Row total before search and filters narrowed it, shown as "N of M rows".
        let unfiltered_rows = if self.loading || self.session.is_sql() {
            None
        } else if self.is_table() {
            self.session.has_filters().then(|| self.session.source.row_count())
        } else if self.session.search_text.trim().is_empty() && self.session.result_filters().is_empty() {
            None
        } else {
            self.query_cache.as_ref().map(|cache| cache.frame.row_count())
        };
        let mut items = Row::new(r.inset_xy(12.0, 0.0), 8.0);
        let dimensions = status::dimensions(row_count, column_count);
        let width = text::measure(&dimensions, font).0 + 2.0;
        canvas.text(items.left(width), &dimensions, font, colors::FOREGROUND, Align::Left);
        if let Some(total) = unfiltered_rows {
            let label = status::filtered_rows(row_count, total);
            let width = text::measure(&label, font).0 + 2.0;
            canvas.text(items.left(width), &label, font, colors::FOREGROUND, Align::Left);
        }
        if let Some(label) = self.find_status() {
            let width = text::measure(&label, font).0 + 2.0;
            canvas.text(items.left(width), &label, font, colors::FOREGROUND, Align::Left);
        }
        if limit_reached {
            let message = match self.session.source.cut_at {
                Some(limit) if self.sql_source().is_some() => status::sql_source_cut_at(limit),
                Some(limit) => status::file_cut_at(limit, self.read_in_file_order()),
                None => status::limited_rows(self.query_limit),
            };
            let width = text::measure(&message, font).0 + 2.0;
            canvas.text(items.left(width), &message, font, LIMIT_WARNING, Align::Left);
        }
        // One line at the grid's size: statistics that do not fit are cut off
        // on the right rather than wrapping into the grid's space.
        let mut parts: Vec<(String, Color, bool, Option<&str>)> = Vec::new();
        if let Some((rows, columns, cells)) = self.selection_counts {
            parts.push((
                format!(
                    "Selected: {} · {} cells",
                    status::dimensions(rows, columns),
                    crate::domain::formatting::group_numeric_text(&cells.to_string())
                ),
                colors::FOREGROUND,
                false,
                None,
            ));
        }
        if self.statistics_pending {
            parts.push(("Calculating statistics…".into(), colors::FOREGROUND, false, None));
        }
        if let Some(stats) = &self.statistics {
            parts.push((status::nulls_label(stats), colors::FOREGROUND, false, None));
            let distinct = status::selection_is_distinct(stats);
            parts.push((
                status::unique_label(stats),
                if distinct { Color::hsla(142.0 / 360.0, 0.7, 0.42, 1.0) } else { colors::FOREGROUND },
                distinct,
                distinct.then_some("Every selected row is distinct"),
            ));
            for label in status::statistics(stats) {
                parts.push((label, colors::FOREGROUND, false, None));
            }
        }
        let area = items.rest();
        let bold = font.bold();
        let widths = parts
            .iter()
            .map(|(label, _, strong, _)| text::measure(label, if *strong { &bold } else { font }).0)
            .collect::<Vec<_>>();
        let total = widths.iter().sum::<f32>() + 8.0 * widths.len().saturating_sub(1) as f32;
        let mut x = if total > area.w { area.x } else { area.right() - total };
        canvas.push_clip(area);
        for ((label, color, strong, tooltip), width) in parts.iter().zip(widths) {
            let part = rect(x, r.y, width + 2.0, r.h);
            canvas.text(part, label, if *strong { &bold } else { font }, *color, Align::Left);
            if let Some(tooltip) = tooltip {
                canvas.hit_with(part, Hit::Surface, Cursor::Arrow, Some(tooltip));
            }
            x += width + 8.0;
        }
        canvas.pop_clip();
    }

    fn paint_body(&mut self, canvas: &mut Canvas<Hit>, body: Rect, cx: &mut Cx<Self>) {
        let fonts = self.fonts.clone();
        let focus = self.focus;
        let mut area = body;
        if self.database_tree_open {
            let width = self.database_tree_width.clamp(160.0, (body.w - 240.0).clamp(160.0, 900.0));
            let (tree, rest) = body.split_left(width);
            self.database_tree.paint(canvas, tree, &fonts, focus);
            let splitter = rect(tree.right() - 3.0, tree.y, 6.0, tree.h);
            canvas.hit_cursor(splitter, Hit::Splitter(Splitter::DatabaseTree), Cursor::ResizeColumns);
            area = rest;
        }
        let mut grid_area = area;
        if self.sql_panel_open && self.sql_panel.is_some() {
            let height = self.sql_editor_height.clamp(140.0, (area.h - 120.0).max(140.0)).min(area.h);
            let (panel_rect, rest) = area.split_bottom(height);
            if let Some(panel) = &mut self.sql_panel {
                panel.paint(canvas, panel_rect, &fonts, focus);
            }
            let splitter = rect(panel_rect.x, panel_rect.y - 3.0, panel_rect.w, 6.0);
            canvas.hit_cursor(splitter, Hit::Splitter(Splitter::Sql), Cursor::ResizeRows);
            grid_area = rest;
        }
        let has_slicers = self
            .session
            .working
            .as_ref()
            .is_some_and(|definition| !definition.slicers.is_empty());
        if has_slicers {
            let width = self.slicer_sidebar.width(canvas.width);
            let (sidebar, rest) = grid_area.split_left(width.min(grid_area.w * 0.6));
            self.slicer_sidebar
                .paint(canvas, sidebar, &fonts, focus, canvas.width, canvas.height);
            grid_area = rest;
        }
        if self.cell_detail.is_some() {
            let width = (canvas.width * 0.3).clamp(280.0, 560.0).min(grid_area.w * 0.6);
            let (pane, rest) = grid_area.split_right(width);
            if let Some(detail) = &mut self.cell_detail {
                detail.paint(canvas, pane, &fonts, focus);
            }
            grid_area = rest;
        }
        self.grid.paint(canvas, grid_area, &fonts.ui, cx);
        if has_slicers {
            self.slicer_sidebar.paint_overlay(canvas, &fonts.small);
        }
        if let Some(panel) = &mut self.sql_panel
            && self.sql_panel_open
        {
            panel.paint_overlay(canvas, &fonts, focus);
        }
        if !self.modal_dialog() {
            self.grid.paint_tooltip(canvas, &fonts);
        }
    }
}

/// A gear drawn from a ring and eight teeth, centered in `r`; the emoji
/// fonts draw one in color, which reads poorly next to the other controls.
fn paint_gear(canvas: &mut Canvas<Hit>, r: Rect, color: Color) {
    let size = r.w.min(r.h) * 0.55;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let ring = size * 0.3;
    let stroke = (size * 0.16).max(1.5);
    canvas.stroke_rounded(rect(cx - ring, cy - ring, ring * 2.0, ring * 2.0), ring, color, stroke);
    for tooth in 0..8 {
        let angle = tooth as f32 * std::f32::consts::FRAC_PI_4;
        let (sin, cos) = angle.sin_cos();
        let at = |radius: f32| crate::ui::platform::point(cx + cos * radius, cy + sin * radius);
        canvas.line(at(ring + stroke * 0.4), at(size / 2.0), color, stroke * 1.4);
    }
}
