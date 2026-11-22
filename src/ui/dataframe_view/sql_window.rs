//! The SQL editor's window: owned by the grid's window, so the grid stays in
//! view with the results while the SQL is written. The editor is the view's
//! SQL panel; this window paints it and passes it input. It opens where it
//! was last, relative to the grid's window, at its last size.
//!
//! The editor shows while `sql_panel_open` is set and there is a panel; the
//! grid's window opens and closes this window to match as it paints.

use std::hash::{Hash, Hasher};

use super::*;
use crate::application::keymap::{ActionId, canonical_keystroke};
use crate::ui::hit::Button;
use crate::ui::platform::{Canvas, Event, MouseButton, Placement, WindowId, WindowOptions};
use crate::ui::widgets::colors;

const DEFAULT_WIDTH: f32 = 1000.0;
const DEFAULT_HEIGHT: f32 = 640.0;

pub(crate) struct SqlWindow {
    pub(crate) id: WindowId,
    /// The grid's window, which owns this one.
    owner: WindowId,
    active: bool,
    /// The editor's control that had focus when its window was left.
    last_focus: Option<WidgetId>,
    /// Whether focus was in the editor's window when last checked, so a
    /// move between the two windows brings the other one forward.
    focus_inside: bool,
    /// The window exists. Until then its activation leaves focus alone.
    opened: bool,
    /// What the window showed when it last painted. The grid's window
    /// redraws it only when that changed, since the editor redraws itself.
    shown: u64,
}

impl DataFrameView {
    /// Whether `cx` is the SQL editor's window, open or on its way out.
    pub(crate) fn is_sql_window(&self, cx: &Cx<Self>) -> bool {
        self.sql_window_id(cx.window_id())
    }

    pub(crate) fn sql_window_id(&self, id: WindowId) -> bool {
        self.sql_window.as_ref().is_some_and(|window| window.id == id) || self.closing_sql_windows.contains(&id)
    }

    /// Whether focus is on one of the SQL editor's controls.
    pub(crate) fn sql_focused(&self) -> bool {
        self.focus
            .is_some_and(|focus| self.sql_panel.as_ref().is_some_and(|panel| panel.contains(focus)))
    }

    /// Opens or closes the editor's window to match whether the editor
    /// shows. `cx` is the grid's window.
    pub(crate) fn sync_sql_window(&mut self, cx: &mut Cx<Self>) {
        let shown = self.sql_panel_open && self.sql_panel.is_some();
        match (&self.sql_window, shown) {
            (None, true) => self.open_sql_window(cx),
            (Some(_), false) => self.close_sql_window(cx),
            _ => {}
        }
    }

    fn open_sql_window(&mut self, cx: &mut Cx<Self>) {
        let title = if matches!(self.session.identity, SessionIdentity::StartupSql) {
            "duckdb_startup.sql"
        } else {
            "SQL editor"
        };
        let options = WindowOptions {
            title: title.into(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            placement: self.sql_window_placement(cx),
            visible: true,
            offscreen: false,
        };
        // Focus stays where it is: the editor, or the grid when a SQL source
        // opens showing its rows, whose window then comes back forward.
        let inside = self.sql_focused();
        let id = cx.open_owned_window(options, move |view, cx| {
            // The editor paints into its window, which exists now.
            let host = cx.hwnd();
            if let Some(panel) = &mut view.sql_panel {
                panel.editor.set_host(host);
            }
            let focused = view.sql_focused();
            let Some(window) = view.sql_window.as_mut().filter(|window| window.id == cx.window_id()) else {
                return;
            };
            window.opened = true;
            if !inside && !focused {
                let owner = window.owner;
                window.active = false;
                cx.for_window(owner).activate();
            }
            cx.notify();
        });
        self.sql_window = Some(SqlWindow {
            id,
            owner: cx.window_id(),
            active: inside,
            last_focus: None,
            focus_inside: inside,
            opened: false,
            shown: 0,
        });
    }

    fn sql_window_placement(&self, owner: &Cx<Self>) -> Option<Placement> {
        let saved = self.repository.load_history().ok()?.sql_window?;
        let owner = owner.window_rect()?;
        let at = crate::infrastructure::json_store::WindowGeometry {
            x: owner.x + saved.x,
            y: owner.y + saved.y,
            ..saved
        };
        crate::ui::window_placement::restored_bounds(&at, &crate::ui::platform::window::displays())
    }

    /// Closes the editor's window as the editor hides, keeping its place.
    fn close_sql_window(&mut self, cx: &mut Cx<Self>) {
        let Some(window) = self.sql_window.take() else { return };
        self.save_sql_window_placement(&window, cx);
        self.closing_sql_windows.push(window.id);
        self.host_sql_editor_in(window.owner, cx);
        if self.sql_focused() {
            self.focus_grid(cx);
        }
        cx.for_window(window.id).close_window();
        cx.for_window(window.owner).notify();
    }

    /// The editor paints into `window` until its own window opens again.
    fn host_sql_editor_in(&mut self, window: WindowId, cx: &Cx<Self>) {
        let host = cx.for_window(window).hwnd();
        if let Some(panel) = &mut self.sql_panel {
            panel.editor.set_host(host);
        }
    }

    /// The window the SQL editor paints into: its own once that is open.
    pub(crate) fn sql_editor_host(&self, cx: &Cx<Self>) -> Option<windows::Win32::Foundation::HWND> {
        match &self.sql_window {
            Some(window) => cx.for_window(window.id).hwnd().or_else(|| cx.hwnd()),
            None => cx.hwnd(),
        }
    }

    fn save_sql_window_placement(&self, window: &SqlWindow, cx: &Cx<Self>) {
        let owner = cx.for_window(window.owner).window_rect();
        let placement = cx.for_window(window.id).placement();
        if let (Some(owner), Some(placement)) = (owner, placement) {
            let geometry = crate::infrastructure::json_store::WindowGeometry {
                x: placement.x - owner.x,
                y: placement.y - owner.y,
                width: placement.width,
                height: placement.height,
                maximized: placement.maximized,
            };
            if let Err(error) = self.repository.save_sql_window_geometry(&geometry) {
                eprintln!("cannot save the SQL editor window's placement: {error}");
            }
        }
    }

    /// The editor's window is going away. Closed by its own button, it
    /// hides the editor, which keeps its SQL for when it shows again.
    pub(crate) fn sql_window_closed(&mut self, cx: &mut Cx<Self>) {
        let id = cx.window_id();
        if let Some(index) = self.closing_sql_windows.iter().position(|closing| *closing == id) {
            self.closing_sql_windows.remove(index);
            return;
        }
        let Some(window) = self.sql_window.take() else { return };
        self.save_sql_window_placement(&window, cx);
        self.sql_panel_open = false;
        self.sql_helper_open = false;
        let mut main = cx.for_window(window.owner);
        self.host_sql_editor_in(window.owner, &main);
        if self.sql_focused() {
            self.focus_grid(&mut main);
        }
        main.notify();
    }

    /// Hides the editor and closes its window.
    pub(crate) fn hide_sql_editor(&mut self, cx: &mut Cx<Self>) {
        self.sql_panel_open = false;
        self.sql_helper_open = false;
        if self.sql_focused() {
            self.focus_grid(cx);
        }
        cx.notify();
    }

    /// Brings forward the window that holds focus once focus moved between
    /// the grid's window and the editor's, such as with Ctrl+J or a dialog
    /// opened from the editor.
    pub(crate) fn follow_sql_focus(&mut self, cx: &mut Cx<Self>) {
        let inside = self.sql_focused();
        let Some(window) = &mut self.sql_window else { return };
        if window.focus_inside == inside {
            return;
        }
        window.focus_inside = inside;
        let id = if inside { window.id } else { window.owner };
        cx.for_window(id).activate();
    }

    /// The grid's window became active or was left. Focus left in the
    /// editor's window goes back to where it was in the grid's.
    pub(crate) fn grid_window_activated(&mut self, active: bool) {
        let inside = self.sql_focused();
        if active {
            if inside && !self.modal_dialog() {
                let back = self.grid_focus.filter(|id| self.owns_focus(*id) && !self.sql_panel.as_ref().is_some_and(|p| p.contains(*id)));
                self.focus = Some(back.unwrap_or(self.grid.id));
            }
        } else if !inside {
            self.grid_focus = self.focus;
        }
        let inside = self.sql_focused();
        if let Some(window) = &mut self.sql_window {
            window.focus_inside = inside;
        }
    }

    /// Redraws the editor's window when what it shows around the editor has
    /// changed, such as a run's status arriving through the grid's window.
    pub(crate) fn refresh_sql_window(&self, cx: &mut Cx<Self>) {
        if let Some(window) = &self.sql_window
            && window.shown != self.sql_window_state()
        {
            cx.for_window(window.id).redraw();
        }
    }

    /// A summary of what the editor's window shows besides the editor's text.
    fn sql_window_state(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        if let Some(panel) = &self.sql_panel {
            panel.hash_shown(&mut hasher);
        }
        self.sql_helper_open.hash(&mut hasher);
        self.focus.hash(&mut hasher);
        self.fonts.ui.size.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn paint_sql_window(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        canvas.clear(colors::BACKGROUND);
        let Some(window) = self.sql_window.as_ref().filter(|window| window.id == cx.window_id()) else {
            return;
        };
        let active = window.active;
        self.update_statement_mark();
        let fonts = self.fonts.clone();
        let focus = self.focus;
        let bounds = canvas.bounds();
        canvas.hit(bounds, Hit::Surface);
        let Some(panel) = &mut self.sql_panel else { return };
        panel.window_active = active;
        panel.paint(canvas, bounds, &fonts, focus);
        panel.paint_overlay(canvas, &fonts, focus);
        if self.sql_helper_open {
            crate::ui::sql_helper_menu::paint(canvas, &fonts, &self.keymap, panel.editor_area);
        }
        let state = self.sql_window_state();
        if let Some(window) = &mut self.sql_window {
            window.shown = state;
        }
    }

    pub(crate) fn sql_window_caret(&self) -> Option<crate::ui::platform::Rect> {
        self.sql_panel.as_ref()?.caret(self.focus)
    }

    /// Input to the editor's window goes to the editor. Its changes go
    /// through the grid's window, so the runs they start outlive this one.
    pub(crate) fn sql_window_event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
        let Some(window) = self.sql_window.as_ref().filter(|window| window.id == cx.window_id()) else {
            return false;
        };
        let owner = window.owner;
        let before = self.focus;
        let mut main = cx.for_window(owner);
        let handled = match event {
            Event::Key(key) => self.sql_window_key(key, &mut main),
            Event::Text(text) => {
                let focus = self.focus;
                self.sql_panel.as_mut().is_some_and(|panel| panel.type_text(text, focus))
            }
            Event::MouseDown { pos, button, modifiers, clicks, hit } => {
                if let Some(hit) = hit {
                    self.sql_window_mouse_down(hit, *pos, *button, *modifiers, *clicks, &mut main);
                }
                true
            }
            Event::MouseMove { pos, left_down, hit, .. } => {
                if let Some(panel) = &mut self.sql_panel {
                    if *left_down {
                        panel.mouse_drag(*pos);
                    } else {
                        panel.mouse_move(*pos, hit.as_ref());
                    }
                }
                false
            }
            Event::MouseUp { .. } => {
                if let Some(panel) = &mut self.sql_panel {
                    panel.mouse_up();
                }
                true
            }
            Event::Wheel { delta, modifiers, hit, .. } => {
                let line = crate::ui::platform::text::line_height(&self.fonts.ui);
                match (hit, &mut self.sql_panel) {
                    (Some(hit), Some(panel)) => panel.wheel(hit, *delta, *modifiers, line),
                    _ => false,
                }
            }
            Event::MouseLeave => {
                if let Some(panel) = &mut self.sql_panel {
                    panel.mouse_leave();
                }
                false
            }
            Event::Activated(active) => {
                self.sql_window_activated(*active);
                false
            }
            Event::Drop { .. } | Event::Moved => false,
        };
        self.focus_changed(before, &mut main);
        self.after_input(&mut main);
        self.follow_sql_focus(&mut main);
        cx.notify();
        handled
    }

    fn sql_window_activated(&mut self, active: bool) {
        let inside = self.sql_focused();
        let modal = self.modal_dialog();
        let Some(window) = &mut self.sql_window else { return };
        window.active = active;
        if !window.opened {
            return;
        }
        if active {
            // Coming back from the grid, focus returns to where it was here.
            if !inside && !modal {
                let panel = self.sql_panel.as_ref();
                let last = window.last_focus.filter(|id| panel.is_some_and(|p| p.contains(*id)));
                self.focus = last.or(panel.map(|panel| panel.editor.id));
            }
        } else if inside {
            window.last_focus = self.focus;
        }
        let inside = self.sql_focused();
        if let Some(window) = &mut self.sql_window {
            window.focus_inside = inside;
        }
        if let Some(panel) = &mut self.sql_panel {
            panel.window_active = active;
        }
    }

    /// Keys in the editor's window: the helper menu's while it is open, the
    /// keymap's SQL editor bindings, the editor's own keys, then Ctrl chords
    /// and the keys of other controls reach the app. `cx` is the grid's window.
    fn sql_window_key(&mut self, key: &crate::ui::platform::KeyEvent, cx: &mut Cx<Self>) -> bool {
        let stroke = canonical_keystroke(&key.stroke()).ok();
        let bound = |view: &Self, context, action| {
            stroke.as_ref().is_some_and(|stroke| view.keymap.binding_for(stroke, context) == Some(action))
        };
        if bound(self, KeyContext::Application, ActionId::Help) {
            self.open_help(cx);
            return true;
        }
        if self.sql_helper_open {
            self.handle_sql_helper_key(key, cx);
            return true;
        }
        let editor = self.sql_panel.as_ref().is_some_and(|panel| self.focus == Some(panel.editor.id));
        if editor && self.sql_editor_key(key, cx) {
            return true;
        }
        if let Some(panel) = &mut self.sql_panel
            && self.focus.is_some_and(|focus| panel.contains(focus))
        {
            let mut focus = self.focus;
            let handled = panel.key(key, &mut focus);
            self.focus = focus;
            self.handle_sql_panel_events(cx);
            if handled {
                return true;
            }
        }
        if bound(self, KeyContext::Window, ActionId::CloseWindow) {
            self.hide_sql_editor(cx);
            return true;
        }
        // In the editor only Ctrl chords reach the app, since other keys type.
        if (!editor || key.modifiers.control)
            && let Some(stroke) = &stroke
            && let Some(action) = self.keymap.binding_for(stroke, KeyContext::Application)
        {
            self.dispatch_keymap_action(action, cx);
            return true;
        }
        false
    }

    fn sql_window_mouse_down(
        &mut self,
        hit: &Hit,
        pos: crate::ui::platform::Point,
        button: MouseButton,
        modifiers: crate::ui::platform::Modifiers,
        clicks: u32,
        cx: &mut Cx<Self>,
    ) {
        let mut focus = self.focus;
        if button == MouseButton::Right {
            if let Some(panel) = &mut self.sql_panel
                && panel.context_menu(hit, pos, &mut focus)
            {
                self.focus = focus;
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        // A click outside the SQL helper menu closes it.
        if self.sql_helper_open && !matches!(hit, Hit::Surface | Hit::Button(Button::SqlHelperCommand(_))) {
            self.sql_helper_open = false;
        }
        if let Hit::Button(Button::SqlHelperCommand(command)) = hit {
            self.select_sql_helper_command(*command, cx);
            return;
        }
        if let Some(panel) = &mut self.sql_panel
            && panel.mouse_down(hit, pos, clicks, modifiers, &mut focus)
        {
            self.focus = focus;
            self.handle_sql_panel_events(cx);
        }
    }
}
