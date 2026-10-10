use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::StyledExt as _;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::*;

use crate::application::commands::{COMMANDS, CommandId};
use crate::application::keymap::{ActionId, KeyContext, KeymapFile};

#[derive(Clone, Copy)]
struct MenuEntry {
    key: &'static str,
    shifted: bool,
    command: CommandId,
}

const SHARED: &[MenuEntry] = &[
    MenuEntry {
        key: "c",
        shifted: false,
        command: CommandId::Copy,
    },
    MenuEntry {
        key: "c",
        shifted: true,
        command: CommandId::CopyAs,
    },
    MenuEntry {
        key: "e",
        shifted: false,
        command: CommandId::Export,
    },
    MenuEntry {
        key: "s",
        shifted: false,
        command: CommandId::Save,
    },
    MenuEntry {
        key: "/",
        shifted: false,
        command: CommandId::Search,
    },
    MenuEntry {
        key: "g",
        shifted: false,
        command: CommandId::Group,
    },
    MenuEntry {
        key: "z",
        shifted: false,
        command: CommandId::AddSlicer,
    },
    MenuEntry {
        key: "a",
        shifted: false,
        command: CommandId::Aggregation,
    },
    MenuEntry {
        key: "p",
        shifted: false,
        command: CommandId::Pivot,
    },
    MenuEntry {
        key: "x",
        shifted: false,
        command: CommandId::RunAction,
    },
];

const TABLE: &[MenuEntry] = &[
    MenuEntry {
        key: "[",
        shifted: false,
        command: CommandId::SortAscending,
    },
    MenuEntry {
        key: "]",
        shifted: false,
        command: CommandId::SortDescending,
    },
    MenuEntry {
        key: "x",
        shifted: true,
        command: CommandId::ClearSort,
    },
    MenuEntry {
        key: "f",
        shifted: false,
        command: CommandId::ToggleFrozen,
    },
    MenuEntry {
        key: "f",
        shifted: true,
        command: CommandId::UnfreezeAll,
    },
    MenuEntry {
        key: "h",
        shifted: false,
        command: CommandId::HideColumns,
    },
    MenuEntry {
        key: "h",
        shifted: true,
        command: CommandId::ChooseColumns,
    },
    MenuEntry {
        key: "r",
        shifted: false,
        command: CommandId::Reload,
    },
];

const QUERY: &[MenuEntry] = &[
    MenuEntry {
        key: "t",
        shifted: false,
        command: CommandId::ToggleTotals,
    },
    MenuEntry {
        key: "m",
        shifted: false,
        command: CommandId::ToggleHeatmap,
    },
    // Shown for SQL views only.
    MenuEntry {
        key: "f",
        shifted: false,
        command: CommandId::ToggleFrozen,
    },
    MenuEntry {
        key: "f",
        shifted: true,
        command: CommandId::UnfreezeAll,
    },
];

fn entries(table: bool, has_sort: bool) -> impl Iterator<Item = MenuEntry> {
    SHARED
        .iter()
        .chain(table.then_some(TABLE).into_iter().flatten())
        .chain((!table).then_some(QUERY).into_iter().flatten())
        .copied()
        .filter(move |entry| has_sort || entry.command != CommandId::ClearSort)
}

pub fn command_for_key(key: &str, shifted: bool, table: bool) -> Option<CommandId> {
    entries(table, true)
        .find(|entry| entry.key == key && entry.shifted == shifted)
        .map(|entry| entry.command)
}

#[derive(IntoElement)]
pub struct SpaceMenu {
    sql: bool,
    sql_slicers: bool,
    table: bool,
    has_sort: bool,
    keymap: KeymapFile,
    on_select: SpaceMenuHandler,
}

type SpaceMenuHandler = Rc<dyn Fn(CommandId, &mut Window, &mut App)>;

impl SpaceMenu {
    pub fn new(table: bool, has_sort: bool, keymap: KeymapFile) -> Self {
        Self {
            sql: false,
            sql_slicers: false,
            table,
            has_sort,
            keymap,
            on_select: Rc::new(|_, _, _| {}),
        }
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(CommandId, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Rc::new(handler);
        self
    }

    /// SQL results offer no group-by, pivot, or table layout commands, and
    /// slicers only when `slicers` says the result holds every row.
    pub fn sql_mode(mut self, sql: bool, slicers: bool) -> Self {
        self.sql = sql;
        self.sql_slicers = slicers;
        self
    }
}

impl RenderOnce for SpaceMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let context = if self.table {
            KeyContext::TableSpaceMenu
        } else {
            KeyContext::QuerySpaceMenu
        };
        let available_width = (window.bounds().size.width.as_f32() - 16.0).max(180.0);
        let available_height = (window.bounds().size.height.as_f32() - 48.0).max(120.0);
        div()
            .id("space-menu")
            .absolute()
            .right(px(8.))
            .bottom(px(30.))
            .w(px(available_width.min(300.0)))
            .child(
                div()
                    .max_h(px(available_height))
                    .overflow_y_scrollbar()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.tokens.border)
                    .bg(theme.tokens.popover)
                    .shadow_lg()
                    .child(div().px_2().font_semibold().child("Space"))
                    .children(
                        entries(self.table, self.has_sort)
                            .filter(|entry| {
                                // Pivot views freeze their group-by columns themselves.
                                self.table
                                    || self.sql
                                    || !matches!(
                                        entry.command,
                                        CommandId::ToggleFrozen | CommandId::UnfreezeAll
                                    )
                            })
                            .filter(|entry| {
                                !self.sql
                                    || (entry.command == CommandId::AddSlicer && self.sql_slicers)
                                    || !matches!(
                                        entry.command,
                                        CommandId::Group
                                            | CommandId::Pivot
                                            | CommandId::Aggregation
                                            | CommandId::AddSlicer
                                            | CommandId::ToggleTotals
                                            | CommandId::ToggleHeatmap
                                    )
                            })
                            .map(|entry| {
                                let label = COMMANDS
                                    .iter()
                                    .find(|command| command.id == entry.command)
                                    .map_or("Command", |command| command.label);
                                let action = match entry.command {
                                    CommandId::Copy => Some(ActionId::SpaceCopy),
                                    CommandId::CopyAs => Some(ActionId::SpaceCopyAs),
                                    CommandId::Export => Some(ActionId::SpaceExport),
                                    CommandId::ToggleTotals => Some(ActionId::SpaceToggleTotals),
                                    CommandId::ToggleHeatmap => Some(ActionId::SpaceToggleHeatmap),
                                    CommandId::Save => Some(ActionId::SpaceSave),
                                    CommandId::Search => Some(ActionId::SpaceSearch),
                                    CommandId::AddSlicer => Some(ActionId::SpaceAddSlicer),
                                    CommandId::Group => Some(ActionId::SpaceGroup),
                                    CommandId::Aggregation => Some(ActionId::SpaceAggregation),
                                    CommandId::Pivot => Some(ActionId::SpacePivot),
                                    CommandId::SortAscending => Some(ActionId::SpaceSortAscending),
                                    CommandId::SortDescending => {
                                        Some(ActionId::SpaceSortDescending)
                                    }
                                    CommandId::ClearSort => Some(ActionId::SpaceClearSort),
                                    CommandId::ToggleFrozen => Some(ActionId::SpaceToggleFreeze),
                                    CommandId::UnfreezeAll => Some(ActionId::SpaceUnfreezeAll),
                                    CommandId::HideColumns => Some(ActionId::SpaceHideColumns),
                                    CommandId::ChooseColumns => Some(ActionId::SpaceChooseColumns),
                                    CommandId::RunAction => Some(ActionId::SpaceRunAction),
                                    _ => None,
                                };
                                let shortcuts = action
                                    .map(|action| self.keymap.bindings_for(action, context))
                                    .unwrap_or_default();
                                let handler = self.on_select.clone();
                                let row = div()
                                    .id(format!(
                                        "space-{}",
                                        label.to_ascii_lowercase().replace(' ', "-")
                                    ))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .rounded_sm()
                                    .hover(|row| row.bg(theme.tokens.table_even))
                                    .on_click(move |_, window, cx| {
                                        handler(entry.command, window, cx)
                                    })
                                    .child(label);
                                row.children(shortcuts.into_iter().map(|(stroke, _)| {
                                    Kbd::new(
                                        Keystroke::parse(&stroke).expect("validated keymap stroke"),
                                    )
                                }))
                            }),
                    ),
            )
    }
}

/// The view's actions, opened from the space menu. Pressing an action's key runs it.
#[derive(IntoElement)]
pub struct ActionMenu {
    actions: Vec<(String, String)>,
    on_select: ActionSelectHandler,
    on_edit: ActionEditHandler,
}

type ActionSelectHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type ActionEditHandler = Rc<dyn Fn(&mut Window, &mut App)>;

impl ActionMenu {
    /// `actions` holds each action's key and description.
    pub fn new(
        actions: Vec<(String, String)>,
        on_select: impl Fn(usize, &mut Window, &mut App) + 'static,
        on_edit: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            actions,
            on_select: Rc::new(on_select),
            on_edit: Rc::new(on_edit),
        }
    }
}

impl RenderOnce for ActionMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let available_width = (window.bounds().size.width.as_f32() - 16.0).max(180.0);
        let available_height = (window.bounds().size.height.as_f32() - 48.0).max(120.0);
        let row = |id: ElementId| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_sm()
                .hover(|row| row.bg(theme.tokens.table_even))
        };
        let on_edit = self.on_edit.clone();
        div()
            .id("action-menu")
            .absolute()
            .right(px(8.))
            .bottom(px(30.))
            .w(px(available_width.min(300.0)))
            .child(
                div()
                    .max_h(px(available_height))
                    .overflow_y_scrollbar()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.tokens.border)
                    .bg(theme.tokens.popover)
                    .shadow_lg()
                    .child(div().px_2().font_semibold().child("Actions"))
                    .children(self.actions.into_iter().enumerate().map(
                        |(index, (key, description))| {
                            let handler = self.on_select.clone();
                            row(("action-menu-item", index).into())
                                .on_click(move |_, window, cx| handler(index, window, cx))
                                .child(
                                    div()
                                        .min_w(px(20.))
                                        .px_1()
                                        .border_1()
                                        .border_color(theme.tokens.border)
                                        .rounded_sm()
                                        .text_center()
                                        .child(key),
                                )
                                .child(div().min_w_0().truncate().child(description))
                        },
                    ))
                    .child(
                        row("action-menu-edit".into())
                            .text_color(theme.muted_foreground)
                            .on_click(move |_, window, cx| on_edit(window, cx))
                            .child("Edit actions…"),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::command_for_key;
    use crate::application::commands::CommandId;

    #[test]
    fn space_keys_are_case_sensitive_and_table_commands_are_scoped() {
        assert_eq!(command_for_key("c", false, false), Some(CommandId::Copy));
        assert_eq!(command_for_key("c", true, false), Some(CommandId::CopyAs));
        assert_eq!(command_for_key("g", false, false), Some(CommandId::Group));
        assert_eq!(
            command_for_key("z", false, false),
            Some(CommandId::AddSlicer)
        );
        assert_eq!(
            command_for_key("[", false, true),
            Some(CommandId::SortAscending)
        );
        assert_eq!(command_for_key("[", false, false), None);
        assert_eq!(command_for_key("q", false, true), None);
        assert_eq!(
            command_for_key("x", false, false),
            Some(CommandId::RunAction)
        );
    }
}
