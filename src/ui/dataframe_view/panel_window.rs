//! The window of the Group By, Pivot By, Aggregations and Order By managers:
//! owned by the grid's window, so the grid stays in view while its query is
//! built. The manager is the open dialog; this window paints it and passes
//! it input. It opens where its kind was last, relative to the grid's
//! window, at its last size.

use super::dialogs::Dialog;
use super::*;
use crate::application::keymap::{ActionId, canonical_keystroke};
use crate::ui::platform::{Canvas, Event, Placement, WindowId, WindowOptions};
use crate::ui::widgets::colors;

pub(crate) struct PanelWindow {
    pub(crate) id: WindowId,
    /// The grid's window, which owns this one.
    owner: WindowId,
    /// Names the manager in saved placements.
    kind: &'static str,
    active: bool,
    /// The manager's control that had focus when its window was left.
    last_focus: Option<WidgetId>,
}

impl Dialog {
    /// The manager's name in saved placements, title and default size, for
    /// the dialogs that open in a window of their own.
    pub(crate) fn window(&self) -> Option<(&'static str, &'static str, f32, f32)> {
        match self {
            Dialog::Pivot(panel) => Some(match panel.kind() {
                PivotFieldKind::GroupBy => ("group_by", "Group By", 980.0, 620.0),
                PivotFieldKind::PivotBy => ("pivot_by", "Pivot By", 980.0, 620.0),
            }),
            Dialog::Aggregation(_) => Some(("aggregations", "Aggregations", 1040.0, 680.0)),
            Dialog::OrderBy(_) => Some(("order_by", "Order By", 760.0, 460.0)),
            _ => None,
        }
    }
}

impl DataFrameView {
    /// Whether `cx` is a manager's window, open or on its way out.
    pub(crate) fn is_panel_window(&self, cx: &Cx<Self>) -> bool {
        self.panel_window_id(cx.window_id())
    }

    pub(crate) fn panel_window_id(&self, id: WindowId) -> bool {
        self.panel_window.as_ref().is_some_and(|window| window.id == id) || self.closing_panels.contains(&id)
    }

    /// Whether the open dialog covers the grid's window and takes its input,
    /// rather than showing in a window of its own.
    pub(crate) fn modal_dialog(&self) -> bool {
        self.dialog.is_some() && self.panel_window.is_none()
    }

    /// Opens the window for the manager that is the open dialog.
    pub(crate) fn open_panel_window(&mut self, kind: &'static str, title: &str, width: f32, height: f32, cx: &mut Cx<Self>) {
        // A manager opened from a manager's window belongs to the grid's.
        let owner = self.panel_owner.take().unwrap_or(cx.window_id());
        let options = WindowOptions {
            title: title.into(),
            width,
            height,
            placement: self.panel_window_placement(kind, &cx.for_window(owner)),
            visible: true,
        };
        let id = cx.for_window(owner).open_owned_window(options, |_, cx| cx.notify());
        self.panel_window = Some(PanelWindow { id, owner, kind, active: true, last_focus: None });
    }

    fn panel_window_placement(&self, kind: &str, owner: &Cx<Self>) -> Option<Placement> {
        let saved = self.repository.load_history().ok()?.panel_windows.get(kind)?.clone();
        let owner = owner.window_rect()?;
        let at = crate::infrastructure::json_store::WindowGeometry {
            x: owner.x + saved.x,
            y: owner.y + saved.y,
            ..saved
        };
        crate::ui::window_placement::restored_bounds(&at, &crate::ui::platform::window::displays())
    }

    /// Closes the manager's window as its dialog closes, keeping its place.
    pub(crate) fn close_panel_window(&mut self, cx: &mut Cx<Self>) {
        let Some(window) = self.panel_window.take() else { return };
        self.save_panel_placement(&window, cx);
        self.closing_panels.push(window.id);
        self.panel_owner = Some(window.owner);
        cx.for_window(window.id).close_window();
        cx.for_window(window.owner).notify();
    }

    fn save_panel_placement(&self, window: &PanelWindow, cx: &Cx<Self>) {
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
            if let Err(error) = self.repository.save_panel_window_geometry(window.kind, &geometry) {
                eprintln!("cannot save the manager window's placement: {error}");
            }
        }
    }

    /// A manager's window is going away. Closed by its own button, it takes
    /// its dialog with it.
    pub(crate) fn panel_window_closed(&mut self, cx: &mut Cx<Self>) {
        let id = cx.window_id();
        if let Some(index) = self.closing_panels.iter().position(|closing| *closing == id) {
            self.closing_panels.remove(index);
            return;
        }
        let Some(window) = self.panel_window.take() else { return };
        self.save_panel_placement(&window, cx);
        self.panel_owner = None;
        self.close_dialog(&mut cx.for_window(window.owner));
    }

    /// Redraws the manager's window along with the grid's, since results and
    /// checks that change what it shows arrive through the grid's window.
    pub(crate) fn refresh_panel_window(&self, cx: &mut Cx<Self>) {
        if let Some(window) = &self.panel_window {
            cx.for_window(window.id).redraw();
        }
    }

    pub(crate) fn paint_panel_window(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        canvas.clear(colors::BACKGROUND);
        let fonts = self.fonts.clone();
        let Some(window) = self.panel_window.as_ref().filter(|window| window.id == cx.window_id()) else {
            return;
        };
        let focus = self.focus.filter(|_| window.active);
        let content = canvas.bounds().inset(16.0);
        canvas.hit(canvas.bounds(), Hit::Surface);
        match &mut self.dialog {
            Some(Dialog::Pivot(panel)) => {
                panel.paint(canvas, content, &fonts, focus);
                panel.paint_overlay(canvas, focus);
            }
            Some(Dialog::Aggregation(panel)) => {
                panel.paint(canvas, content, &fonts, focus);
                panel.paint_overlay(canvas, focus);
            }
            Some(Dialog::OrderBy(panel)) => panel.paint(canvas, content, &fonts, focus),
            _ => {}
        }
    }

    /// Input to a manager's window goes to its dialog. Its changes go through
    /// the grid's window, so the work they start outlives the manager.
    pub(crate) fn panel_window_event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
        let Some(window) = self.panel_window.as_ref().filter(|window| window.id == cx.window_id()) else {
            return false;
        };
        let owner = window.owner;
        let before = self.focus;
        let mut main = cx.for_window(owner);
        let handled = match event {
            Event::Key(key) => {
                let stroke = canonical_keystroke(&key.stroke()).ok();
                let bound = |view: &Self, context, action| {
                    stroke.as_ref().is_some_and(|stroke| view.keymap.binding_for(stroke, context) == Some(action))
                };
                if bound(self, KeyContext::Application, ActionId::Help) {
                    self.open_help(&mut main);
                    true
                } else if self.dialog_key(key, &mut main) {
                    true
                } else if key.is("escape") || bound(self, KeyContext::Window, ActionId::CloseWindow) {
                    self.close_dialog(&mut main);
                    true
                } else {
                    false
                }
            }
            Event::Text(text) => self.dialog_text(text, &mut main),
            Event::MouseDown { pos, clicks, modifiers, hit, button } => {
                if *button == crate::ui::platform::MouseButton::Left
                    && let Some(hit) = hit
                {
                    self.dialog_mouse_down(hit, *pos, *clicks, *modifiers, &mut main);
                }
                true
            }
            Event::MouseMove { pos, left_down, .. } => {
                if *left_down {
                    self.dialog_mouse_drag(*pos);
                }
                false
            }
            Event::MouseUp { .. } => {
                self.dialog_mouse_up();
                true
            }
            Event::Wheel { delta, modifiers, hit, .. } => {
                hit.as_ref().is_some_and(|hit| self.dialog_wheel(hit, *delta, modifiers.shift))
            }
            Event::Activated(active) => {
                let active = *active;
                let in_panel = |view: &Self, id: WidgetId| !view.owns_focus(id);
                if let Some(window) = &mut self.panel_window {
                    window.active = active;
                }
                if active {
                    // Coming back from the grid, focus returns to where it was here.
                    let last = self.panel_window.as_ref().and_then(|window| window.last_focus);
                    if self.focus.is_none_or(|id| !in_panel(self, id)) && last.is_some() {
                        self.focus = last;
                    }
                } else if let Some(focus) = self.focus.filter(|id| in_panel(self, *id))
                    && let Some(window) = &mut self.panel_window
                {
                    window.last_focus = Some(focus);
                }
                false
            }
            Event::MouseLeave | Event::Drop { .. } | Event::Moved => false,
        };
        self.focus_changed(before, &mut main);
        self.after_input(&mut main);
        cx.notify();
        handled
    }

    pub(crate) fn panel_window_caret(&self) -> Option<crate::ui::platform::Rect> {
        let focus = self.focus;
        match &self.dialog {
            Some(Dialog::Pivot(panel)) => panel.caret(focus),
            Some(Dialog::Aggregation(panel)) => panel.caret(focus),
            Some(Dialog::OrderBy(panel)) => panel.caret(focus),
            _ => None,
        }
    }
}
