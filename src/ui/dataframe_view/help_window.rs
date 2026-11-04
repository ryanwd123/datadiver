//! The help window: owned by the grid's window and opened by F1 on the page
//! about whatever had focus. Escape or Ctrl+W closes it.

use super::*;
use crate::application::keymap::{ActionId, canonical_keystroke};
use crate::ui::help_view::HelpView;
use crate::ui::platform::{Canvas, Event, WindowId, WindowOptions};
use crate::ui::widgets::colors;

const DEFAULT_WIDTH: f32 = 1060.0;
const DEFAULT_HEIGHT: f32 = 760.0;

pub(crate) struct HelpWindow {
    pub(crate) id: WindowId,
    /// The grid's window, which owns this one.
    owner: WindowId,
    pub(crate) view: HelpView,
}

impl DataFrameView {
    /// Whether `cx` is the help window rather than the grid's.
    pub(crate) fn is_help_window(&self, cx: &Cx<Self>) -> bool {
        self.help_window.as_ref().is_some_and(|window| window.id == cx.window_id())
    }

    /// Opens the help on the page about what has focus, or turns the open
    /// help window to it.
    pub(crate) fn open_help(&mut self, cx: &mut Cx<Self>) {
        let topic = self.help_topic();
        self.open_help_topic(topic, cx);
    }

    /// Opens the help at `topic`, a page id with an optional `#heading`.
    /// `cx` is the grid's window, which owns the help.
    pub(crate) fn open_help_topic(&mut self, topic: &str, cx: &mut Cx<Self>) {
        self.space_menu_open = false;
        self.action_menu_open = false;
        if let Some(window) = &mut self.help_window {
            window.view.show(topic);
            let id = window.id;
            let mut help = cx.for_window(id);
            help.activate();
            help.notify();
            return;
        }
        let mut view = HelpView::new(&self.keymap);
        view.show(topic);
        let options = WindowOptions {
            title: "DataDiver help".into(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            placement: None,
            visible: true,
        };
        let owner = cx.window_id();
        let id = cx.open_owned_window(options, |_, cx| cx.notify());
        self.help_window = Some(HelpWindow { id, owner, view });
        cx.notify();
    }

    /// The help page for what has focus now.
    pub(crate) fn help_topic(&self) -> &'static str {
        if self.keymap_editor.is_some() {
            return "shortcuts";
        }
        if let Some(dialog) = &self.dialog {
            return match dialog {
                Dialog::Pivot(_) | Dialog::Aggregation(_) => "pivot",
                Dialog::OrderBy(_) => "pivot#order-by",
                Dialog::SlicerPicker(_) => "slicers",
                Dialog::PasteFilter(_) => "slicers#filter-by-pasted-values",
                Dialog::Actions(_) => "views#view-actions",
                Dialog::AiSettings(_) => "ask-ai#set-up-a-provider",
                Dialog::Theme(_) => "settings#themes-and-fonts",
                Dialog::Information(_) => "grid#information-and-column-profiles",
                Dialog::Save { .. } => "views#saving-views",
                Dialog::Palette(_) | Dialog::Confirm(_) => "start",
            };
        }
        if self.sql_helper_open {
            return "sql#the-helper-menu";
        }
        if self.space_menu_open || self.action_menu_open {
            return "grid#the-helper-menu";
        }
        let Some(focus) = self.focus else { return "start" };
        if let Some(panel) = &self.sql_panel
            && self.sql_panel_open
            && panel.contains(focus)
        {
            return if focus == panel.ask_input.id { "ask-ai" } else { "sql" };
        }
        if focus == self.search_input.id {
            return "search";
        }
        if focus == self.limit_input.id {
            return "grid#the-row-limit";
        }
        if self.slicer_sidebar.contains(focus) {
            return "slicers";
        }
        if self.database_tree_open && self.database_tree.contains(focus) {
            return "data#the-database-tree";
        }
        if self.cell_detail.as_ref().is_some_and(|pane| pane.editor.id == focus) {
            return "grid#cell-detail";
        }
        if focus == self.grid.id {
            let config = self.session.query_config();
            let grouped = !config.group_by.is_empty() || !config.pivot_by.is_empty();
            return if grouped { "pivot" } else { "grid" };
        }
        "start"
    }

    pub(crate) fn help_window_closed(&mut self, cx: &mut Cx<Self>) {
        let Some(window) = self.help_window.take() else { return };
        cx.for_window(window.owner).notify();
    }

    pub(crate) fn paint_help_window(&mut self, canvas: &mut Canvas<Hit>, cx: &mut Cx<Self>) {
        canvas.clear(colors::BACKGROUND);
        let fonts = self.fonts.clone();
        let Some(window) = self.help_window.as_mut().filter(|window| window.id == cx.window_id()) else {
            return;
        };
        window.view.paint(canvas, canvas.bounds(), &fonts);
    }

    pub(crate) fn help_window_event(&mut self, event: &Event<Hit>, cx: &mut Cx<Self>) -> bool {
        let line = crate::ui::platform::text::line_height(&self.fonts.ui);
        let closes = match event {
            Event::Key(key) => {
                key.is("escape")
                    || (key.key == "w" && key.modifiers.control && !key.modifiers.shift && !key.modifiers.alt)
                    || canonical_keystroke(&key.stroke())
                        .is_ok_and(|stroke| self.keymap.binding_for(&stroke, KeyContext::Window) == Some(ActionId::CloseWindow))
            }
            _ => false,
        };
        let Some(window) = self.help_window.as_mut() else { return false };
        let handled = match event {
            Event::Key(key) => {
                if closes {
                    cx.close_window();
                    true
                } else {
                    window.view.key(key, line)
                }
            }
            Event::MouseDown { pos, hit, .. } => window.view.mouse_down(hit.as_ref(), *pos),
            Event::MouseMove { pos, left_down: true, .. } => {
                window.view.mouse_drag(*pos);
                true
            }
            Event::MouseUp { .. } => {
                window.view.mouse_up();
                true
            }
            Event::Wheel { delta, .. } => window.view.wheel(*delta, line),
            _ => false,
        };
        if handled {
            cx.notify();
        }
        handled
    }
}
