//! Routing keys, text, and the mouse to the window's controls.

use super::*;
use crate::application::keymap::{ActionId, canonical_keystroke};
use crate::ui::hit::{Button, SlicerHit, Splitter};
use crate::ui::platform::{Event, KeyEvent, Modifiers, MouseButton, Point, View};

impl View for DataFrameView {
    type Hit = Hit;

    fn paint(&mut self, canvas: &mut crate::ui::platform::Canvas<Hit>, cx: &mut Cx<Self>) {
        if self.is_format_window(cx) {
            self.paint_format_window(canvas, cx);
            return;
        }
        self.sync_profiles(cx);
        self.render(canvas, cx);
        self.refresh_format_window(cx);
    }

    fn event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
        if self.is_format_window(cx) {
            return self.format_window_event(event, cx);
        }
        self.sync_profiles(cx);
        let before = self.focus;
        let handled = match event {
            Event::Key(key) => self.key(key, cx),
            Event::Text(text) => self.text_input(text, cx),
            Event::MouseDown {
                pos,
                button,
                modifiers,
                clicks,
                hit,
            } => {
                self.mouse_down(*pos, *button, *modifiers, *clicks, hit.as_ref(), cx);
                true
            }
            Event::MouseMove { pos, left_down, hit, .. } => {
                self.mouse_move(*pos, *left_down, hit.as_ref(), cx);
                false
            }
            Event::MouseUp { .. } => {
                self.mouse_up(cx);
                true
            }
            Event::Wheel { delta, modifiers, hit, .. } => self.wheel(*delta, *modifiers, hit.as_ref(), cx),
            Event::MouseLeave => {
                self.grid.mouse_leave();
                if let Some(panel) = &mut self.sql_panel {
                    panel.mouse_leave();
                }
                cx.notify();
                false
            }
            Event::Drop { paths, .. } => {
                if self.cache_drop_open {
                    self.cache_files(paths.clone(), cx);
                } else {
                    self.open_paths(paths.clone(), cx);
                }
                true
            }
            Event::Activated(active) => {
                self.window_active = *active;
                if let Some(panel) = &mut self.sql_panel {
                    panel.window_active = *active;
                    cx.notify();
                }
                if *active {
                    self.check_source(cx);
                    self.check_database_tree(cx);
                }
                false
            }
            Event::Moved => {
                self.window_moved(cx);
                false
            }
        };
        self.focus_changed(before, cx);
        self.after_input(cx);
        handled
    }

    fn close_requested(&mut self, cx: &mut Cx<Self>) -> bool {
        if !self.is_format_window(cx) {
            self.prepare_close(cx);
        }
        true
    }

    fn closing(&mut self, cx: &mut Cx<Self>) {
        if self.is_format_window(cx) {
            self.format_window_closed(cx);
            return;
        }
        self.flush_auto_save();
        self.autosave_table_preferences(cx);
        self.save_geometry();
    }

    fn caret(&self, window: crate::ui::platform::WindowId) -> Option<crate::ui::platform::Rect> {
        if self.format_window.as_ref().is_some_and(|format| format.id == window) {
            return self.format_window_caret(window);
        }
        let focus = self.focus?;
        if focus == self.search_input.id {
            return self.search_input.caret_rect();
        }
        if focus == self.limit_input.id {
            return self.limit_input.caret_rect();
        }
        self.sql_panel
            .as_ref()
            .and_then(|panel| panel.caret(Some(focus)))
            .or_else(|| self.slicer_sidebar.caret(Some(focus)))
    }
}

impl DataFrameView {
    /// Reacts to what controls reported while handling input.
    pub(crate) fn after_input(&mut self, cx: &mut Cx<Self>) {
        self.drain_grid_events(cx);
        self.drain_slicer_events(cx);
        self.handle_sql_panel_events(cx);
        self.handle_tree_events(cx);
        self.handle_panel_events(cx);
        self.handle_keymap_editor_events(cx);
    }

    fn key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        cx.notify();
        if let Some(editor) = &mut self.keymap_editor {
            let mut focus = self.focus;
            let handled = editor.key(event, &mut focus);
            self.focus = focus;
            return handled;
        }
        if self.dialog.is_some() {
            if self.dialog_key(event, cx) {
                return true;
            }
            if event.is("escape") {
                self.close_dialog(cx);
                return true;
            }
            if let Ok(stroke) = canonical_keystroke(&event.stroke())
                && self.keymap.binding_for(&stroke, KeyContext::Window) == Some(ActionId::CloseWindow)
            {
                self.dispatch_command(CommandId::CloseWindow, cx);
                return true;
            }
            return false;
        }
        if self.cache_drop_open && event.key == "escape" {
            self.set_cache_drop_open(false, cx);
            return true;
        }
        if self.notices.log_open && event.key == "escape" {
            self.notices.set_log_open(false);
            return true;
        }
        if self.action_menu_open {
            self.handle_action_menu_key(event, cx);
            return true;
        }
        if self.space_menu_open {
            self.handle_space_key(event, cx);
            return true;
        }
        if self.sql_helper_open {
            self.handle_sql_helper_key(event, cx);
            return true;
        }
        let focus = self.focus;
        let sql_editor = self.sql_panel_open && self.sql_panel.as_ref().is_some_and(|panel| focus == Some(panel.editor.id));
        if sql_editor && self.sql_editor_key(event, cx) {
            return true;
        }
        // The SQL panel takes its keys first, then Ctrl chords reach the app.
        if let Some(panel) = &mut self.sql_panel
            && self.sql_panel_open
            && focus.is_some_and(|focus| panel.contains(focus))
        {
            let mut focus = self.focus;
            let editor = focus == Some(panel.editor.id);
            let handled = panel.key(event, &mut focus);
            self.focus = focus;
            self.handle_sql_panel_events(cx);
            if handled {
                return true;
            }
            if editor {
                if event.modifiers.control
                    && let Ok(stroke) = canonical_keystroke(&event.stroke())
                    && let Some(action) = self.keymap.binding_for(&stroke, KeyContext::Application)
                {
                    self.dispatch_keymap_action(action, cx);
                    return true;
                }
                return false;
            }
            return self.window_key(event, cx);
        }
        if focus == Some(self.limit_input.id) {
            return self.limit_key(event, cx);
        }
        if focus == Some(self.search_input.id) && self.search_key(event, cx) {
            return true;
        }
        if focus == Some(self.grid.id) {
            let height = cx.viewport().1;
            if self.grid.key(event, height) {
                self.drain_grid_events(cx);
                return true;
            }
        }
        if focus.is_some_and(|focus| self.database_tree_open && self.database_tree.contains(focus)) {
            let mut focus = self.focus;
            let handled = self.database_tree.key(event, &mut focus);
            self.focus = focus;
            self.handle_tree_events(cx);
            if handled {
                return true;
            }
        }
        if focus.is_some_and(|focus| self.slicer_sidebar.contains(focus)) {
            let mut focus = self.focus;
            let handled = self.slicer_sidebar.key(event, &mut focus);
            self.focus = focus;
            self.drain_slicer_events(cx);
            if handled {
                return true;
            }
        }
        if let Some(pane) = &mut self.cell_detail
            && focus == Some(pane.editor.id)
            && pane.editor.key(event).handled
        {
            return true;
        }
        self.window_key(event, cx)
    }

    /// The search box's own keys: its column list, then moving focus.
    fn search_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let (handled, accepted) = self.search_completion.key(event, &mut self.search_input);
        if handled {
            if accepted {
                self.search_edited(cx);
            }
            return true;
        }
        let m = event.modifiers;
        match event.key.as_str() {
            "down" if !m.modified() => {
                self.focus_grid(cx);
                return true;
            }
            "tab" if !m.control && !m.alt => {
                if m.shift || !self.limit_applies() {
                    self.focus_grid(cx);
                } else {
                    let id = self.limit_input.id;
                    self.set_focus(Some(id), cx);
                }
                return true;
            }
            "escape" if !m.modified() => {
                // Escape first closes the column list, then clears the search.
                if !self.search_completion.close() && !self.search_input.text().is_empty() {
                    self.set_search_text(String::new(), cx);
                }
                return true;
            }
            _ => {}
        }
        let outcome = self.search_input.key(event);
        if outcome.changed || outcome.handled {
            self.search_completion.refresh(&self.search_input);
        }
        if outcome.changed {
            self.search_edited(cx);
        }
        outcome.handled
    }

    fn limit_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let m = event.modifiers;
        match event.key.as_str() {
            "escape" => {
                self.limit_error = None;
                self.limit_input.set_text(self.query_limit.to_string());
                return true;
            }
            "tab" if !m.control && !m.alt => {
                if m.shift {
                    let id = self.search_input.id;
                    self.set_focus(Some(id), cx);
                } else {
                    self.focus_grid(cx);
                }
                return true;
            }
            _ => {}
        }
        let outcome = self.limit_input.key(event);
        if outcome.submit {
            self.commit_query_limit(cx);
        }
        // Keys typed in the limit never reach window shortcuts, but a key that
        // types text must go unhandled, or the window drops the text it types.
        outcome.handled || (!m.control && event.typed().is_none())
    }

    /// Window shortcuts: Tab between the search and grid, favorites, and the
    /// keymap's bindings in the focused control's context.
    fn window_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let m = event.modifiers;
        let focus = self.focus;
        let grid = self.grid.id;
        let search = self.search_input.id;
        if event.key == "tab" && !m.control && !m.alt {
            if focus == Some(search) {
                return false;
            }
            let next = if focus == Some(grid) {
                if m.shift && self.limit_applies() {
                    self.limit_input.id
                } else {
                    search
                }
            } else if m.shift {
                grid
            } else {
                search
            };
            self.set_focus(Some(next), cx);
            return true;
        }
        let Ok(stroke) = canonical_keystroke(&event.stroke()) else {
            return false;
        };
        if let Some((slot, assign)) = favorite_shortcut(event) {
            self.favorite(slot, assign, cx);
            return true;
        }
        let context = if focus == Some(grid) {
            if self.is_table() {
                KeyContext::TableGrid
            } else {
                KeyContext::QueryGrid
            }
        } else if focus == Some(search) {
            KeyContext::Search
        } else {
            KeyContext::Application
        };
        // Shifted punctuation also matches the character it types, such as `?`.
        let produced = (m.shift && !m.control && !m.alt)
            .then(|| event.key_char.as_deref())
            .flatten()
            .filter(|typed| !typed.chars().any(char::is_alphanumeric));
        let action = self
            .keymap
            .binding_for(&stroke, context)
            .or_else(|| produced.and_then(|typed| self.keymap.binding_for(typed, context)));
        if let Some(action) = action {
            self.dispatch_keymap_action(action, cx);
            return true;
        }
        false
    }

    fn text_input(&mut self, typed: &str, cx: &mut Cx<Self>) -> bool {
        cx.notify();
        let mut focus = self.focus;
        if let Some(editor) = &mut self.keymap_editor {
            return editor.type_text(typed, focus);
        }
        if self.dialog.is_some() {
            return self.dialog_text(typed, cx);
        }
        if self.space_menu_open || self.action_menu_open || self.cache_drop_open || self.sql_helper_open {
            return false;
        }
        if focus == Some(self.search_input.id) {
            self.search_input.type_text(typed);
            self.search_edited(cx);
            return true;
        }
        if focus == Some(self.limit_input.id) {
            self.limit_input.type_text(typed);
            return true;
        }
        if let Some(panel) = &mut self.sql_panel
            && panel.type_text(typed, focus)
        {
            self.handle_sql_panel_events(cx);
            return true;
        }
        if self.database_tree.type_text(typed, &mut focus) {
            self.focus = focus;
            return true;
        }
        let handled = self.slicer_sidebar.type_text(typed, &mut focus);
        self.focus = focus;
        handled
    }

    fn mouse_down(&mut self, pos: Point, button: MouseButton, modifiers: Modifiers, clicks: u32, hit: Option<&Hit>, cx: &mut Cx<Self>) {
        cx.notify();
        self.capture = hit.cloned();
        let Some(hit) = hit else {
            return;
        };
        if button == MouseButton::Right {
            if let Hit::Grid(grid_hit) = hit {
                self.grid.mouse_down(*grid_hit, pos, modifiers.shift, true);
                self.drain_grid_events(cx);
            }
            self.capture = None;
            let mut focus = self.focus;
            if self.dialog.is_none()
                && self.keymap_editor.is_none()
                && let Some(panel) = &mut self.sql_panel
                && self.sql_panel_open
                && panel.context_menu(hit, pos, &mut focus)
            {
                self.focus = focus;
                self.handle_sql_panel_events(cx);
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        if let Some(editor) = &mut self.keymap_editor {
            let mut focus = self.focus;
            editor.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus);
            self.focus = focus;
            self.handle_keymap_editor_events(cx);
            return;
        }
        if self.dialog.is_some() {
            if matches!(hit, Hit::Backdrop) {
                self.close_dialog(cx);
                self.capture = None;
                return;
            }
            self.dialog_mouse_down(hit, pos, clicks, modifiers, cx);
            return;
        }
        // A click outside the SQL helper menu closes it.
        if self.sql_helper_open && !matches!(hit, Hit::Surface | Hit::Button(Button::SqlHelperCommand(_))) {
            self.sql_helper_open = false;
        }
        // So does one outside the message log; its own button toggles it.
        if self.notices.log_open && !matches!(hit, Hit::Surface | Hit::Button(Button::ClearNotices | Button::NoticeLog)) {
            self.notices.set_log_open(false);
        }
        match hit {
            Hit::Grid(grid_hit) => {
                if self.grid.mouse_down(*grid_hit, pos, modifiers.shift, false) {
                    self.focus_grid(cx);
                }
                self.drain_grid_events(cx);
            }
            Hit::Splitter(splitter) => {
                let (start, size) = match splitter {
                    Splitter::Sql => (pos.y, self.sql_editor_height),
                    Splitter::DatabaseTree => (pos.x, self.database_tree_width),
                };
                self.split_drag = Some(SplitDrag {
                    splitter: *splitter,
                    start,
                    size,
                });
            }
            Hit::Button(button) => self.button(button.clone(), cx),
            Hit::ColumnCompletion(id, index) if *id == self.search_input.id => {
                self.search_completion.accept(Some(*index), &mut self.search_input);
                self.search_edited(cx);
                let id = *id;
                self.set_focus(Some(id), cx);
            }
            Hit::Editor(id, part) if *id == self.search_input.id => {
                self.search_input.mouse_down(*part, pos, clicks, modifiers.shift);
                let id = *id;
                self.set_focus(Some(id), cx);
                self.search_completion.refresh(&self.search_input);
            }
            Hit::Editor(id, part) if *id == self.limit_input.id => {
                self.limit_input.mouse_down(*part, pos, clicks, modifiers.shift);
                let id = *id;
                self.set_focus(Some(id), cx);
            }
            Hit::Editor(id, part) if self.cell_detail.as_ref().is_some_and(|pane| pane.editor.id == *id) => {
                if let Some(pane) = &mut self.cell_detail {
                    pane.editor.mouse_down(*part, pos, clicks, modifiers.shift);
                }
                let id = *id;
                self.set_focus(Some(id), cx);
            }
            Hit::Slicer(SlicerHit::Value { .. } | SlicerHit::Bar(_) | SlicerHit::ColumnBar(_)) => {
                let mut focus = self.focus;
                self.slicer_sidebar.mouse_down(hit, pos, clicks, modifiers, &mut focus);
                self.focus = focus;
            }
            _ => {
                let mut focus = self.focus;
                if let Some(panel) = &mut self.sql_panel
                    && self.sql_panel_open
                    && panel.mouse_down(hit, pos, clicks, modifiers, &mut focus)
                {
                    self.focus = focus;
                    self.handle_sql_panel_events(cx);
                    return;
                }
                if self.database_tree_open && self.database_tree.mouse_down(hit, pos, clicks, modifiers, &mut focus) {
                    self.focus = focus;
                    self.handle_tree_events(cx);
                    return;
                }
                if self.slicer_sidebar.mouse_down(hit, pos, clicks, modifiers, &mut focus) {
                    self.focus = focus;
                    self.drain_slicer_events(cx);
                    return;
                }
                if let Some(pane) = &mut self.cell_detail
                    && pane.mouse_down(hit, pos, clicks, modifiers.shift, &mut focus)
                {
                    self.focus = focus;
                }
            }
        }
    }

    fn button(&mut self, button: Button, cx: &mut Cx<Self>) {
        match button {
            Button::ClearSort => self.clear_sort(cx),
            Button::ClearFilters => self.clear_filters(cx),
            Button::RemoveValueFilter(index) => self.remove_value_filter(index, cx),
            Button::SpaceCommand(command) => self.select_space_command(command, cx),
            Button::SqlHelperCommand(command) => self.select_sql_helper_command(command, cx),
            Button::GridKey(action) => self.select_grid_key(action, cx),
            Button::RunViewAction(index) => self.run_view_action(index, cx),
            Button::EditViewActions => self.open_actions_editor(cx),
            Button::CacheDropBrowse => {
                self.add_watched_files_with_picker(cx);
                self.set_cache_drop_open(false, cx);
            }
            Button::CacheDropCancel => self.set_cache_drop_open(false, cx),
            Button::ViewOptions => self.open_view_options(cx),
            Button::NoticeLog => {
                let open = !self.notices.log_open;
                self.notices.set_log_open(open);
            }
            Button::DismissNotice => self.notices.dismiss_toast(),
            Button::ClearNotices => self.notices.clear(),
            Button::CellDetailClose => {
                self.toggle_cell_detail(cx);
            }
            other => {
                let hit = Hit::Button(other);
                let mut focus = self.focus;
                if let Some(panel) = &mut self.sql_panel
                    && panel.mouse_down(&hit, Point::default(), 1, Modifiers::NONE, &mut focus)
                {
                    self.focus = focus;
                    self.handle_sql_panel_events(cx);
                    return;
                }
                if self.database_tree.mouse_down(&hit, Point::default(), 1, Modifiers::NONE, &mut focus) {
                    self.focus = focus;
                    self.handle_tree_events(cx);
                    return;
                }
                if self.slicer_sidebar.mouse_down(&hit, Point::default(), 1, Modifiers::NONE, &mut focus) {
                    self.focus = focus;
                    self.drain_slicer_events(cx);
                    return;
                }
                if let Some(pane) = &mut self.cell_detail {
                    pane.mouse_down(&hit, Point::default(), 1, false, &mut focus);
                }
                self.focus = focus;
            }
        }
    }

    fn mouse_move(&mut self, pos: Point, left_down: bool, hit: Option<&Hit>, cx: &mut Cx<Self>) {
        if left_down && let Some(capture) = self.capture.clone() {
            self.mouse_drag(pos, &capture, hit, cx);
            return;
        }
        if self.dialog.is_none() && self.keymap_editor.is_none() {
            let grid_hit = match hit {
                Some(Hit::Grid(grid_hit)) => Some(*grid_hit),
                _ => None,
            };
            self.grid.mouse_move(grid_hit, cx);
            if let Some(panel) = &mut self.sql_panel
                && self.sql_panel_open
            {
                panel.mouse_move(pos, hit);
            }
        }
    }

    fn mouse_drag(&mut self, pos: Point, capture: &Hit, hit: Option<&Hit>, cx: &mut Cx<Self>) {
        cx.notify();
        if let Some(drag) = self.split_drag {
            let (width, height) = cx.viewport();
            match drag.splitter {
                Splitter::Sql => {
                    let size = (drag.size - (pos.y - drag.start)).clamp(140.0, (height - 160.0).max(140.0));
                    self.sql_editor_height = size;
                }
                Splitter::DatabaseTree => {
                    let size = (drag.size + (pos.x - drag.start)).clamp(160.0, (width - 240.0).clamp(160.0, 900.0));
                    self.database_tree_width = size;
                }
            }
            return;
        }
        if let Some(editor) = &mut self.keymap_editor {
            editor.mouse_drag(pos);
            return;
        }
        if self.dialog.is_some() {
            self.dialog_mouse_drag(pos);
            return;
        }
        match capture {
            Hit::Grid(_) => {
                self.grid.mouse_drag(pos);
                self.drain_grid_events(cx);
            }
            Hit::Editor(id, _) if *id == self.search_input.id => self.search_input.mouse_drag(pos),
            Hit::Editor(id, _) if *id == self.limit_input.id => self.limit_input.mouse_drag(pos),
            Hit::Slicer(_) => self.slicer_sidebar.mouse_drag(pos, hit),
            _ => {
                if let Some(panel) = &mut self.sql_panel {
                    panel.mouse_drag(pos);
                }
                self.database_tree.mouse_drag(pos);
                self.slicer_sidebar.mouse_drag(pos, hit);
                if let Some(pane) = &mut self.cell_detail
                    && pane.editor.is_dragging()
                {
                    pane.editor.mouse_drag(pos);
                }
            }
        }
    }

    fn mouse_up(&mut self, cx: &mut Cx<Self>) {
        cx.notify();
        self.capture = None;
        if let Some(drag) = self.split_drag.take() {
            // The drag already resized the pane; save where it ended.
            match drag.splitter {
                Splitter::Sql if (self.sql_editor_height - drag.size).abs() >= 0.5 => {
                    let height = self.sql_editor_height;
                    self.save_sql_editor_height(height);
                }
                Splitter::DatabaseTree if (self.database_tree_width - drag.size).abs() >= 0.5 => {
                    let width = self.database_tree_width;
                    self.save_database_tree_width(width);
                }
                _ => {}
            }
            return;
        }
        if let Some(editor) = &mut self.keymap_editor {
            editor.mouse_up();
            return;
        }
        self.dialog_mouse_up();
        self.grid.mouse_up();
        self.search_input.mouse_up();
        self.limit_input.mouse_up();
        if let Some(panel) = &mut self.sql_panel {
            panel.mouse_up();
        }
        self.database_tree.mouse_up();
        self.slicer_sidebar.mouse_up();
        if let Some(pane) = &mut self.cell_detail {
            pane.editor.mouse_up();
        }
        self.drain_slicer_events(cx);
        self.drain_grid_events(cx);
    }

    fn wheel(&mut self, delta: Point, modifiers: Modifiers, hit: Option<&Hit>, cx: &mut Cx<Self>) -> bool {
        let Some(hit) = hit else {
            return false;
        };
        let line = crate::ui::platform::text::line_height(&self.fonts.ui);
        let moved = if let Some(editor) = &mut self.keymap_editor {
            editor.wheel(delta, line)
        } else if self.dialog.is_some() {
            self.dialog_wheel(hit, delta, modifiers.shift)
        } else {
            match hit {
                Hit::Grid(grid_hit) => {
                    let moved = self.grid.wheel(Some(*grid_hit), delta, modifiers.shift);
                    self.grid.mouse_leave();
                    moved
                }
                Hit::Slicer(_) => self.slicer_sidebar.wheel(hit, delta, line),
                Hit::Tree(..) => self.database_tree.wheel(delta),
                Hit::CodeEditor(_) => self
                    .sql_panel
                    .as_mut()
                    .is_some_and(|panel| panel.wheel(hit, delta, modifiers, line)),
                Hit::Editor(id, _) => {
                    if let Some(pane) = &mut self.cell_detail
                        && pane.editor.id == *id
                    {
                        pane.editor.wheel(delta)
                    } else {
                        false
                    }
                }
                Hit::DialogScroll | Hit::DialogBar | Hit::AskScroll | Hit::AskBar => self
                    .sql_panel
                    .as_mut()
                    .is_some_and(|panel| panel.wheel(hit, delta, modifiers, line)),
                Hit::Surface if self.notices.log_open => self.notices.wheel(delta, line),
                _ => false,
            }
        };
        if moved {
            cx.notify();
        }
        moved
    }
}
