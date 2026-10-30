use crate::application::commands::{COMMANDS, CommandId};
use crate::application::keymap::{ActionId, KeyContext, KeymapFile};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, rect, text};
use crate::ui::widgets::{Fonts, colors, key_cap};

#[derive(Clone, Copy)]
struct MenuEntry {
    key: &'static str,
    shifted: bool,
    command: CommandId,
}

const fn entry(key: &'static str, shifted: bool, command: CommandId) -> MenuEntry {
    MenuEntry {
        key,
        shifted,
        command,
    }
}

const SHARED: &[MenuEntry] = &[
    entry("c", false, CommandId::Copy),
    entry("c", true, CommandId::CopyAs),
    entry("e", false, CommandId::Export),
    entry("s", false, CommandId::Save),
    entry("/", false, CommandId::Search),
    entry("g", false, CommandId::Group),
    entry("z", false, CommandId::AddSlicer),
    entry("a", false, CommandId::Aggregation),
    entry("p", false, CommandId::Pivot),
    entry("x", false, CommandId::RunAction),
];

const TABLE: &[MenuEntry] = &[
    entry("[", false, CommandId::SortAscending),
    entry("]", false, CommandId::SortDescending),
    entry("x", true, CommandId::ClearSort),
    entry("f", false, CommandId::ToggleFrozen),
    entry("f", true, CommandId::UnfreezeAll),
    entry("h", false, CommandId::HideColumns),
    entry("h", true, CommandId::ChooseColumns),
    entry("r", false, CommandId::Reload),
];

const QUERY: &[MenuEntry] = &[
    entry("t", false, CommandId::ToggleTotals),
    entry("m", false, CommandId::ToggleHeatmap),
    // Shown for SQL views only.
    entry("f", false, CommandId::ToggleFrozen),
    entry("f", true, CommandId::UnfreezeAll),
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

/// The commands the Space menu lists, with their shortcuts.
pub struct SpaceMenu {
    pub sql: bool,
    /// A saved SQL source, whose rows views can be built on.
    pub sql_source: bool,
    pub sql_slicers: bool,
    pub table: bool,
    pub has_sort: bool,
}

impl SpaceMenu {
    fn shown(&self) -> Vec<MenuEntry> {
        entries(self.table, self.has_sort)
            .filter(|entry| {
                // Pivot views freeze their group-by columns themselves.
                self.table || self.sql || !matches!(entry.command, CommandId::ToggleFrozen | CommandId::UnfreezeAll)
            })
            .filter(|entry| {
                !self.sql
                    || (entry.command == CommandId::AddSlicer && self.sql_slicers)
                    || (self.sql_source && matches!(entry.command, CommandId::Group | CommandId::Pivot | CommandId::Aggregation))
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
            .collect()
    }

    pub fn paint(&self, canvas: &mut Canvas<Hit>, fonts: &Fonts, keymap: &KeymapFile) {
        let context = if self.table {
            KeyContext::TableSpaceMenu
        } else {
            KeyContext::QuerySpaceMenu
        };
        let font = &fonts.ui;
        let line = text::line_height(font);
        let row_height = line + 8.0;
        let shown = self.shown();
        let width = (canvas.width - 16.0).max(180.0).min(300.0);
        let height = ((shown.len() + 1) as f32 * row_height + 16.0).min((canvas.height - 48.0).max(120.0));
        let menu = rect(canvas.width - 8.0 - width, canvas.height - 30.0 - height, width, height);
        draw_popup(canvas, menu);
        canvas.push_clip(menu);
        canvas.text(rect(menu.x + 16.0, menu.y + 8.0, width - 32.0, row_height), "Space", &font.semibold(), colors::FOREGROUND, Align::Left);
        for (index, entry) in shown.iter().enumerate() {
            let row = rect(menu.x + 8.0, menu.y + 8.0 + (index + 1) as f32 * row_height, width - 16.0, row_height);
            if canvas.hovered(row) {
                canvas.fill_rounded(row, 4.0, colors::STRIPE);
            }
            let label = COMMANDS
                .iter()
                .find(|command| command.id == entry.command)
                .map_or("Command", |command| command.label);
            let label_width = text::measure(label, font).0;
            canvas.text(rect(row.x + 8.0, row.y, label_width + 4.0, row.h), label, font, colors::FOREGROUND, Align::Left);
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
                CommandId::SortDescending => Some(ActionId::SpaceSortDescending),
                CommandId::ClearSort => Some(ActionId::SpaceClearSort),
                CommandId::ToggleFrozen => Some(ActionId::SpaceToggleFreeze),
                CommandId::UnfreezeAll => Some(ActionId::SpaceUnfreezeAll),
                CommandId::HideColumns => Some(ActionId::SpaceHideColumns),
                CommandId::ChooseColumns => Some(ActionId::SpaceChooseColumns),
                CommandId::RunAction => Some(ActionId::SpaceRunAction),
                _ => None,
            };
            let mut x = row.x + 8.0 + label_width + 8.0;
            for (stroke, _) in action.map(|action| keymap.bindings_for(action, context)).unwrap_or_default() {
                x += key_cap(canvas, x, row.y + 3.0, row.h - 6.0, &stroke, &fonts.small) + 4.0;
            }
            canvas.hit(row, Hit::Button(Button::SpaceCommand(entry.command)));
        }
        canvas.pop_clip();
    }
}

fn draw_popup(canvas: &mut Canvas<Hit>, menu: crate::ui::platform::Rect) {
    canvas.shadow(menu, 6.0);
    canvas.fill_rounded(menu, 6.0, colors::POPOVER);
    canvas.stroke_rounded(menu, 6.0, colors::BORDER, 1.0);
    canvas.hit(menu, Hit::Surface);
}

/// The view's actions, opened from the space menu. Pressing an action's key runs it.
pub fn paint_action_menu(canvas: &mut Canvas<Hit>, fonts: &Fonts, actions: &[(String, String)]) {
    let font = &fonts.ui;
    let line = text::line_height(font);
    let row_height = line + 8.0;
    let width = (canvas.width - 16.0).max(180.0).min(300.0);
    let height = ((actions.len() + 2) as f32 * row_height + 16.0).min((canvas.height - 48.0).max(120.0));
    let menu = rect(canvas.width - 8.0 - width, canvas.height - 30.0 - height, width, height);
    draw_popup(canvas, menu);
    canvas.push_clip(menu);
    canvas.text(rect(menu.x + 16.0, menu.y + 8.0, width - 32.0, row_height), "Actions", &font.semibold(), colors::FOREGROUND, Align::Left);
    for (index, (key, description)) in actions.iter().enumerate() {
        let row = rect(menu.x + 8.0, menu.y + 8.0 + (index + 1) as f32 * row_height, width - 16.0, row_height);
        if canvas.hovered(row) {
            canvas.fill_rounded(row, 4.0, colors::STRIPE);
        }
        let key_width = (text::measure(key, font).0 + 10.0).max(20.0);
        let key_rect = rect(row.x + 8.0, row.y + 3.0, key_width, row.h - 6.0);
        canvas.stroke_rounded(key_rect, 4.0, colors::BORDER, 1.0);
        canvas.text(key_rect, key, font, colors::FOREGROUND, Align::Center);
        canvas.text(
            rect(key_rect.right() + 8.0, row.y, row.right() - key_rect.right() - 16.0, row.h),
            description,
            font,
            colors::FOREGROUND,
            Align::Left,
        );
        canvas.hit(row, Hit::Button(Button::RunViewAction(index)));
    }
    let row = rect(menu.x + 8.0, menu.y + 8.0 + (actions.len() + 1) as f32 * row_height, width - 16.0, row_height);
    if canvas.hovered(row) {
        canvas.fill_rounded(row, 4.0, colors::STRIPE);
    }
    canvas.text(row.inset_xy(8.0, 0.0), "Edit actions…", font, colors::MUTED, Align::Left);
    canvas.hit(row, Hit::Button(Button::EditViewActions));
    canvas.pop_clip();
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
        assert_eq!(command_for_key("z", false, false), Some(CommandId::AddSlicer));
        assert_eq!(command_for_key("[", false, true), Some(CommandId::SortAscending));
        assert_eq!(command_for_key("[", false, false), None);
        assert_eq!(command_for_key("q", false, true), None);
        assert_eq!(command_for_key("x", false, false), Some(CommandId::RunAction));
    }
}
