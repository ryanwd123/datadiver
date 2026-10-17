//! Drawing the window: the toolbar, the body with its panes, the status bar,
//! and overlays.

use super::*;
use crate::ui::hit::{Button, Splitter};
use crate::ui::platform::{Align, Canvas, Color, Cursor, Rect, rect, text};
use crate::ui::space_menu::{SpaceMenu, paint_action_menu};
use crate::ui::status;
use crate::ui::widgets::{ButtonStyle, Row, button, button_height, button_width, colors};

const LIMIT_WARNING: Color = Color::rgb(0xff0000);

impl DataFrameView {
    pub(crate) fn render(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        self.frames.set(self.frames.get() + 1);
        self.sync_completion_columns();
        self.drain_grid_events(cx);
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
        let modal = self.dialog.is_some() || self.cache_drop_open;
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
        let search_width = 320.0_f32.min((items.rest().w * 0.4).max(128.0));
        let search = items.left(search_width);
        let search_id = self.search_input.id;
        let focus = self.focus;
        self.search_input
            .paint(canvas, search, font, focus == Some(search_id), |part| Hit::Editor(search_id, part));
        if !self.is_table() {
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
        let button_top = header.y + (header.h - button_height(font)) / 2.0;
        if !self.session.sort.is_empty() {
            let r = items.left(button_width("Clear sort", font));
            button(canvas, rect(r.x, button_top, r.w, button_height(font)), "Clear sort", font, ButtonStyle::Outline, true, Hit::Button(Button::ClearSort));
        }
        if self.session.has_filters() {
            let r = items.left(button_width("Clear filters", font));
            button(canvas, rect(r.x, button_top, r.w, button_height(font)), "Clear filters", font, ButtonStyle::Outline, true, Hit::Button(Button::ClearFilters));
        }
        if self.loading || self.busy {
            let label = if self.loading { "Loading…" } else { "Searching…" };
            let r = items.left(text::measure(label, font).0 + 12.0);
            canvas.text(r.offset(8.0, 0.0), label, font, colors::FOREGROUND, Align::Left);
        }
        let title = self
            .session
            .working
            .as_ref()
            .map(|view| view.name.clone())
            .or_else(|| {
                self.session.source_path.as_deref().map(|path| {
                    PathBuf::from(path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.into())
                })
            })
            .unwrap_or_else(|| "DataDiver".into());
        if bounds.w >= 1000.0
            && let Some(modified) = &self.source_modified
        {
            let r = items.right(text::measure(modified, font).0 + 4.0);
            canvas.text(r, modified, font, colors::FOREGROUND, Align::Left);
        }
        if self.session.dirty {
            let r = items.right(text::measure("● Unsaved", font).0 + 4.0);
            canvas.text(r, "● Unsaved", font, colors::WARNING, Align::Left);
        }
        let rest = items.rest();
        canvas.text(rest, &title, font, colors::FOREGROUND, Align::Right);
        let mut top = header.bottom();
        // Errors.
        for error in [&self.error, &self.table_preferences_error, &self.limit_error].into_iter().flatten() {
            let area = rect(bounds.x + 12.0, top, bounds.w - 24.0, line * 3.0);
            let layout = text::layout(error, font, crate::ui::platform::TextOptions::wrapped(area.w));
            let height = layout.height.min(line * 3.0);
            canvas.push_clip(rect(area.x, top, area.w, height));
            canvas.draw_layout(&layout, area.origin(), colors::DANGER);
            canvas.pop_clip();
            top += height;
        }
        // Value filter chips.
        if !self.session.value_filters().is_empty() {
            let small = &fonts.small;
            let chip_height = button_height(small);
            let mut x = bounds.x + 12.0;
            let mut y = top + 4.0;
            let label_width = text::measure("Filters", small).0;
            canvas.text(rect(x, y, label_width + 4.0, chip_height), "Filters", small, colors::MUTED, Align::Left);
            x += label_width + 12.0;
            for (index, filter) in self.session.value_filters().iter().enumerate() {
                let label = format!("{}  ✕", filter.label());
                let width = button_width(&label, small).min(bounds.w - 24.0);
                if x + width > bounds.right() - 12.0 && x > bounds.x + 12.0 + label_width + 12.0 {
                    x = bounds.x + 12.0 + label_width + 12.0;
                    y += chip_height + 6.0;
                }
                button(canvas, rect(x, y, width, chip_height), &label, small, ButtonStyle::Outline, true, Hit::Button(Button::RemoveValueFilter(index)));
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
        if self.space_menu_open {
            SpaceMenu {
                sql: self.session.is_sql(),
                sql_slicers: self.sql_result_complete,
                table: self.is_table(),
                has_sort: !self.session.sort.is_empty(),
            }
            .paint(canvas, &fonts, &self.keymap);
        }
        if self.action_menu_open {
            let actions = self
                .view_actions()
                .into_iter()
                .map(|action| (action.key, action.description))
                .collect::<Vec<_>>();
            paint_action_menu(canvas, &fonts, &actions);
        }
        if modal {
            canvas.set_mouse(mouse);
        }
        if self.cache_drop_open {
            self.paint_cache_drop(canvas);
        }
        self.paint_dialog(canvas, cx);
        // Painting the grid can measure column widths; record them now.
        self.drain_grid_events(cx);
    }

    /// Whether the shown query stopped at the row limit. It is judged by the
    /// query's own rows, before search narrowed them, so the warning stays
    /// while searching a truncated result.
    pub(crate) fn limit_reached(&self) -> bool {
        let query_rows = self
            .query_cache
            .as_ref()
            .map_or(self.session.result.row_count(), |cache| cache.frame.row_count());
        !self.busy
            && !self.loading
            && self.error.is_none()
            && !self.is_table()
            && query_limit_reached(query_rows, self.query_limit)
    }

    fn paint_status(&mut self, canvas: &mut Canvas<Hit>, r: Rect, row_count: usize, limit_reached: bool) {
        let font = &self.fonts.ui;
        let column_count = self.session.result.column_count();
        // Row total before search and filters narrowed it, shown as "N of M rows".
        let unfiltered_rows = if self.loading || self.session.is_sql() {
            None
        } else if self.is_table() {
            self.session.has_filters().then(|| self.session.source.row_count())
        } else if self.session.search_text.trim().is_empty() {
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
        if limit_reached {
            let message = status::limited_rows(self.query_limit);
            let width = text::measure(&message, font).0 + 2.0;
            canvas.text(items.left(width), &message, font, LIMIT_WARNING, Align::Left);
        }
        // One line at the grid's size: statistics that do not fit are cut off
        // on the right rather than wrapping into the grid's space.
        let mut parts: Vec<(String, Color, bool, Option<&str>)> = Vec::new();
        if let Some(copy) = &self.copy_status {
            parts.push((copy.clone(), colors::FOREGROUND, false, None));
        }
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
        if let Some(error) = &self.statistics_error {
            parts.push((format!("Statistics failed: {error}"), colors::FOREGROUND, false, None));
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
        if self.dialog.is_none() {
            self.grid.paint_tooltip(canvas, &fonts.ui);
        }
    }
}
