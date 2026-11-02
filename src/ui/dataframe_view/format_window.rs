//! The conditional formatting manager's window: owned by the grid's window
//! and showing the same view, so edits reach the grid as they're typed. It
//! opens where it was last, relative to the grid's window, at its last size.

use super::*;
use crate::application::keymap::{ActionId, canonical_keystroke};
use crate::ui::format_manager::{FormatManager, FormatManagerEvent};
use crate::ui::platform::{Canvas, Event, Placement, WindowId, WindowOptions, system};
use crate::ui::widgets::colors;

const DEFAULT_WIDTH: f32 = 1040.0;
const DEFAULT_HEIGHT: f32 = 660.0;

pub(crate) struct FormatWindow {
    pub(crate) id: WindowId,
    /// The grid's window, which owns this one.
    owner: WindowId,
    /// Made once the window exists, since its editor paints into it.
    pub(crate) manager: Option<FormatManager>,
    pub(crate) focus: Option<WidgetId>,
    active: bool,
}

impl DataFrameView {
    /// Whether `cx` is the formatting manager's window rather than the grid's.
    pub(crate) fn is_format_window(&self, cx: &Cx<Self>) -> bool {
        self.format_window.as_ref().is_some_and(|window| window.id == cx.window_id())
    }

    /// Opens the manager's window, or brings it forward when it's open.
    pub(crate) fn open_format_manager(&mut self, cx: &mut Cx<Self>) {
        self.space_menu_open = false;
        self.action_menu_open = false;
        if let Some(window) = &self.format_window {
            cx.for_window(window.id).activate();
            return;
        }
        let options = WindowOptions {
            title: "Conditional formatting".into(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            placement: self.format_window_placement(cx),
            visible: true,
        };
        let owner = cx.window_id();
        let id = cx.open_owned_window(options, |view, cx| view.start_format_manager(cx));
        self.format_window = Some(FormatWindow { id, owner, manager: None, focus: None, active: true });
        cx.notify();
    }

    /// Where the manager opens: its saved offset from this window's corner,
    /// at its saved size, kept on screen.
    fn format_window_placement(&self, cx: &Cx<Self>) -> Option<Placement> {
        let saved = self.repository.load_history().ok()?.format_window?;
        let owner = cx.window_rect()?;
        let at = crate::infrastructure::json_store::WindowGeometry {
            x: owner.x + saved.x,
            y: owner.y + saved.y,
            ..saved
        };
        crate::ui::window_placement::restored_bounds(&at, &crate::ui::platform::window::displays())
    }

    /// Builds the manager in its new window, on the view's formats and the
    /// grid's columns, starting on the grid's column.
    fn start_format_manager(&mut self, cx: &mut Cx<Self>) {
        let formats = self.session.working.as_ref().map(|view| view.formats.clone()).unwrap_or_default();
        let active = self
            .grid
            .selection()
            .and_then(|selection| self.session.result.columns.get(selection.active.column))
            .map(|column| column.0.clone());
        let manager = FormatManager::new(cx.hwnd(), formats, self.format_columns(), active);
        let Some(window) = self.format_window.as_mut().filter(|window| window.id == cx.window_id()) else {
            return;
        };
        manager.focus(&mut window.focus);
        window.manager = Some(manager);
        cx.notify();
    }

    /// The columns rules can name: the grid's first, in its order, then hidden ones.
    fn format_columns(&self) -> Vec<diver_format::Column> {
        let mut columns = self
            .format_scope()
            .map(|scope| crate::application::conditional_format::rule_columns(&scope.schema))
            .unwrap_or_default();
        let shown: std::collections::HashMap<&str, usize> =
            self.session.result.columns.iter().enumerate().map(|(index, column)| (column.0.as_str(), index)).collect();
        columns.sort_by_key(|column| shown.get(column.name.as_str()).copied().unwrap_or(usize::MAX));
        columns
    }

    /// Redraws the manager's window when what it shows has changed, such as
    /// after undo or a new result. The grid's other redraws leave it alone.
    pub(crate) fn refresh_format_window(&self, cx: &mut Cx<Self>) {
        let Some(window) = &self.format_window else { return };
        let Some(manager) = &window.manager else { return };
        let formats = self.session.working.as_ref().map(|view| view.formats.as_slice()).unwrap_or_default();
        if !manager.shows(formats, &self.format_columns()) {
            cx.for_window(window.id).redraw();
        }
    }

    /// The manager's window is going away: undo takes the formatting as it
    /// was then, and the window's place is kept for next time.
    pub(crate) fn format_window_closed(&mut self, cx: &mut Cx<Self>) {
        let Some(window) = self.format_window.take() else { return };
        self.undo_history.record(self.session.view_state());
        let owner = cx.for_window(window.owner).window_rect();
        if let (Some(owner), Some(placement)) = (owner, cx.placement()) {
            let geometry = crate::infrastructure::json_store::WindowGeometry {
                x: placement.x - owner.x,
                y: placement.y - owner.y,
                width: placement.width,
                height: placement.height,
                maximized: placement.maximized,
            };
            if let Err(error) = self.repository.save_format_window_geometry(&geometry) {
                eprintln!("cannot save the formatting window's placement: {error}");
            }
        }
        cx.for_window(window.owner).notify();
    }

    pub(crate) fn paint_format_window(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        canvas.clear(colors::BACKGROUND);
        // Undo, another view or a new result can change what the manager shows.
        let formats = self.session.working.as_ref().map(|view| view.formats.clone()).unwrap_or_default();
        let columns = self.format_columns();
        let fonts = self.fonts.clone();
        let Some(window) = self.format_window.as_mut().filter(|window| window.id == cx.window_id()) else {
            return;
        };
        let Some(manager) = &mut window.manager else { return };
        manager.sync_scope(&formats, columns);
        let focus = window.focus.filter(|_| window.active);
        manager.paint(canvas, canvas.bounds().inset(16.0), &fonts, focus);
    }

    pub(crate) fn format_window_caret(&self, id: WindowId) -> Option<crate::ui::platform::Rect> {
        let window = self.format_window.as_ref().filter(|window| window.id == id)?;
        window.manager.as_ref()?.caret(window.focus)
    }

    /// Input to the manager's window. Its changes go through the grid's
    /// window, so work they start outlives the manager.
    pub(crate) fn format_window_event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
        let closes = match event {
            Event::Key(key) => {
                key.is("escape")
                    || canonical_keystroke(&key.stroke())
                        .is_ok_and(|stroke| self.keymap.binding_for(&stroke, KeyContext::Window) == Some(ActionId::CloseWindow))
            }
            _ => false,
        };
        let Some(window) = self.format_window.as_mut() else { return false };
        let owner = window.owner;
        let Some(manager) = &mut window.manager else { return false };
        let mut focus = window.focus;
        let handled = match event {
            Event::Key(key) => {
                let used = manager.key(key, &mut focus);
                if !used && closes {
                    cx.close_window();
                }
                used || closes
            }
            Event::Text(text) => manager.type_text(text, focus),
            Event::MouseDown { pos, clicks, modifiers, hit, .. } => {
                manager.mouse_down(hit.as_ref(), *pos, *clicks, *modifiers, &mut focus);
                true
            }
            Event::MouseMove { pos, left_down, hit, .. } => {
                if *left_down {
                    manager.mouse_drag(*pos);
                } else {
                    manager.mouse_move(*pos, hit.as_ref());
                }
                false
            }
            Event::MouseUp { .. } => {
                manager.mouse_up();
                true
            }
            Event::Wheel { delta, modifiers, hit, .. } => {
                hit.as_ref().is_some_and(|hit| manager.wheel(hit, *delta, *modifiers))
            }
            Event::MouseLeave => {
                manager.mouse_move(crate::ui::platform::point(-1.0, -1.0), None);
                false
            }
            Event::Activated(active) => {
                window.active = *active;
                false
            }
            Event::Drop { .. } | Event::Moved => false,
        };
        window.focus = focus;
        let picker = cx.hwnd();
        self.handle_format_events(picker, &mut cx.for_window(owner));
        cx.notify();
        handled
    }

    /// Applies what the manager changed, at once. A color picker opens over
    /// `picker`, the manager's window.
    fn handle_format_events(&mut self, picker: Option<windows::Win32::Foundation::HWND>, cx: &mut Cx<Self>) {
        loop {
            let Some(manager) = self.format_window.as_mut().and_then(|window| window.manager.as_mut()) else {
                return;
            };
            let events = manager.take_events();
            if events.is_empty() {
                break;
            }
            for event in events {
                match event {
                    FormatManagerEvent::Changed(formats) => {
                        self.ensure_working_view();
                        if let Err(error) = self.session.edit_definition(|definition| definition.formats = formats) {
                            self.error = Some(error);
                        }
                        self.refresh_formatting(cx);
                        self.autosave_table_preferences(cx);
                        self.schedule_auto_save(cx);
                    }
                    FormatManagerEvent::PickColor => {
                        let manager = self.format_window.as_mut().and_then(|window| window.manager.as_mut());
                        let Some(manager) = manager else { return };
                        let initial = manager.color_at_caret().unwrap_or(0x638ec6);
                        if let Some(rgb) = system::choose_color(picker, initial) {
                            manager.insert_color(&format!("#{rgb:06x}"));
                        }
                    }
                }
            }
        }
        cx.notify();
    }
}
