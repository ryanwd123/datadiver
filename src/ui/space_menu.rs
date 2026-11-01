use crate::application::commands::{COMMANDS, CommandId};
use crate::application::keymap::{ACTIONS as KEYMAP_ACTIONS, ActionCategory, ActionId, KeyContext, KeymapFile};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Rect, rect, text};
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
    entry("m", false, CommandId::ConditionalFormatting),
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

/// The Helper menu, opened with Space: the grid's own shortcuts on top, which
/// also work while the menu is open, and below them the commands only the
/// menu's keys run.
pub struct SpaceMenu {
    pub sql: bool,
    /// A saved SQL source, whose rows views can be built on.
    pub sql_source: bool,
    pub sql_slicers: bool,
    pub table: bool,
    pub has_sort: bool,
}

/// One line of the menu: a label, its keys, and what a click does.
struct Item {
    label: &'static str,
    strokes: Vec<String>,
    button: Button,
}

const COLUMN_WIDTH: f32 = 290.0;
const MIN_COLUMN_WIDTH: f32 = 240.0;

impl SpaceMenu {
    /// Whether the view offers `command`; menu entries and grid keys share this.
    fn offers(&self, command: CommandId) -> bool {
        // Pivot views freeze their group-by columns themselves.
        if !self.table && !self.sql && matches!(command, CommandId::ToggleFrozen | CommandId::UnfreezeAll) {
            return false;
        }
        if command == CommandId::ClearSort && !self.has_sort {
            return false;
        }
        !self.sql
            || (command == CommandId::AddSlicer && self.sql_slicers)
            || (self.sql_source && matches!(command, CommandId::Group | CommandId::Pivot | CommandId::Aggregation))
            || !matches!(
                command,
                CommandId::Group
                    | CommandId::Pivot
                    | CommandId::Aggregation
                    | CommandId::AddSlicer
                    | CommandId::ToggleTotals
                    | CommandId::DrillDown
                    | CommandId::DrillDownNewWindow
            )
    }

    fn shown(&self) -> Vec<MenuEntry> {
        entries(self.table, self.has_sort)
            .filter(|entry| self.offers(entry.command))
            .collect()
    }

    /// The grid's bound shortcuts, leaving out moving the cursor and the
    /// selection, grouped by category.
    fn grid_items(&self, keymap: &KeymapFile) -> Vec<Item> {
        let context = if self.table { KeyContext::TableGrid } else { KeyContext::QueryGrid };
        let mut specs: Vec<_> = KEYMAP_ACTIONS
            .iter()
            .filter(|spec| spec.context == KeyContext::Grid || spec.context == context)
            .filter(|spec| !matches!(spec.category, ActionCategory::Navigation | ActionCategory::Selection))
            .filter(|spec| spec.id != ActionId::Space)
            .filter(|spec| spec.command.is_none_or(|command| self.offers(command)))
            // Only pivot views have group-by columns to remove.
            .filter(|spec| spec.id != ActionId::RemoveActiveGroupColumn || !self.sql)
            .collect();
        specs.sort_by_key(|spec| ActionCategory::ALL.iter().position(|category| *category == spec.category));
        let mut items: Vec<Item> = Vec::new();
        for spec in specs {
            // An action listed for several contexts appears once.
            if items.iter().any(|item| item.button == Button::GridKey(spec.id)) {
                continue;
            }
            let strokes: Vec<String> = keymap
                .bindings_for(spec.id, context)
                .into_iter()
                .map(|(stroke, _)| stroke)
                .collect();
            if !strokes.is_empty() {
                items.push(Item {
                    label: spec.name,
                    strokes,
                    button: Button::GridKey(spec.id),
                });
            }
        }
        items
    }

    /// The menu's own commands, leaving out those a grid key already runs,
    /// since grid keys work while the menu is open.
    fn menu_items(&self, keymap: &KeymapFile, grid_items: &[Item]) -> Vec<Item> {
        let context = if self.table {
            KeyContext::TableSpaceMenu
        } else {
            KeyContext::QuerySpaceMenu
        };
        let grid_commands: Vec<CommandId> = grid_items
            .iter()
            .filter_map(|item| match item.button {
                Button::GridKey(action) => KEYMAP_ACTIONS.iter().find(|spec| spec.id == action).and_then(|spec| spec.command),
                _ => None,
            })
            .collect();
        self.shown()
            .into_iter()
            .filter(|entry| !grid_commands.contains(&entry.command))
            .map(|entry| Item {
                label: COMMANDS
                    .iter()
                    .find(|command| command.id == entry.command)
                    .map_or("Command", |command| command.label),
                strokes: menu_action(entry.command)
                    .map(|action| keymap.bindings_for(action, context).into_iter().map(|(stroke, _)| stroke).collect())
                    .unwrap_or_default(),
                button: Button::SpaceCommand(entry.command),
            })
            .collect()
    }

    pub fn paint(&self, canvas: &mut Canvas<Hit>, fonts: &Fonts, keymap: &KeymapFile) {
        let font = &fonts.ui;
        let title_font = font.semibold();
        let line = text::line_height(font);
        let row_height = line + 6.0;
        let grid_items = self.grid_items(keymap);
        let menu_items = self.menu_items(keymap, &grid_items);
        let sections = [
            ("Grid keys", "work in the grid and in this menu", grid_items),
            ("Menu keys", "work only while this menu is open", menu_items),
        ];
        // The fewest columns that fit the window; each section fills down its columns.
        let room = (canvas.width - 16.0).max(MIN_COLUMN_WIDTH + 16.0);
        let max_columns = (((room - 16.0) / MIN_COLUMN_WIDTH).floor() as usize).clamp(1, 3);
        let height_for = |columns: usize| {
            let rows: usize = sections.iter().map(|(_, _, items)| 1 + items.len().div_ceil(columns)).sum();
            16.0 + row_height * (1 + rows) as f32 + 8.0 * sections.len() as f32
        };
        let max_height = (canvas.height - 48.0).max(120.0);
        let columns = (1..=max_columns)
            .find(|columns| height_for(*columns) <= max_height)
            .unwrap_or(max_columns);
        let column_width = ((room - 16.0) / columns as f32).min(COLUMN_WIDTH);
        let width = column_width * columns as f32 + 16.0;
        let height = height_for(columns).min(max_height);
        let menu = rect(canvas.width - 8.0 - width, canvas.height - 30.0 - height, width, height);
        draw_popup(canvas, menu);
        canvas.push_clip(menu);
        canvas.text(rect(menu.x + 16.0, menu.y + 8.0, width - 32.0, row_height), "Helper Menu", &title_font, colors::FOREGROUND, Align::Left);
        let mut y = menu.y + 8.0 + row_height;
        for (title, hint, items) in &sections {
            y += 8.0;
            let title_width = text::measure(title, &title_font).0;
            canvas.text(rect(menu.x + 16.0, y, title_width + 4.0, row_height), title, &title_font, colors::MUTED, Align::Left);
            canvas.text(
                rect(menu.x + 26.0 + title_width, y, (width - title_width - 42.0).max(0.0), row_height),
                hint,
                &fonts.small,
                colors::MUTED,
                Align::Left,
            );
            y += row_height;
            let rows = items.len().div_ceil(columns).max(1);
            for (index, item) in items.iter().enumerate() {
                let row = rect(
                    menu.x + 8.0 + (index / rows) as f32 * column_width,
                    y + (index % rows) as f32 * row_height,
                    column_width - 4.0,
                    row_height,
                );
                paint_item(canvas, fonts, row, item);
            }
            y += items.len().div_ceil(columns) as f32 * row_height;
        }
        canvas.pop_clip();
    }
}

fn paint_item(canvas: &mut Canvas<Hit>, fonts: &Fonts, row: Rect, item: &Item) {
    if canvas.hovered(row) {
        canvas.fill_rounded(row, 4.0, colors::STRIPE);
    }
    // Keys sit at the right edge so they line up down a column.
    let keys_width: f32 = item
        .strokes
        .iter()
        .map(|stroke| text::measure(&crate::ui::platform::keys::display(stroke), &fonts.small).0 + 14.0)
        .sum();
    let mut x = row.right() - 4.0 - keys_width;
    canvas.text(
        rect(row.x + 8.0, row.y, (x - row.x - 12.0).max(0.0), row.h),
        item.label,
        &fonts.ui,
        colors::FOREGROUND,
        Align::Left,
    );
    for stroke in &item.strokes {
        x += key_cap(canvas, x, row.y + 3.0, row.h - 6.0, stroke, &fonts.small) + 4.0;
    }
    canvas.hit(row, Hit::Button(item.button.clone()));
}

/// The Helper-menu keymap action that runs `command`.
fn menu_action(command: CommandId) -> Option<ActionId> {
    match command {
        CommandId::Copy => Some(ActionId::SpaceCopy),
        CommandId::CopyAs => Some(ActionId::SpaceCopyAs),
        CommandId::Export => Some(ActionId::SpaceExport),
        CommandId::ToggleTotals => Some(ActionId::SpaceToggleTotals),
        CommandId::ConditionalFormatting => Some(ActionId::SpaceConditionalFormatting),
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
        CommandId::Reload => Some(ActionId::SpaceReload),
        _ => None,
    }
}

fn draw_popup(canvas: &mut Canvas<Hit>, menu: Rect) {
    canvas.shadow(menu, 6.0);
    canvas.fill_rounded(menu, 6.0, colors::POPOVER);
    canvas.stroke_rounded(menu, 6.0, colors::BORDER, 1.0);
    canvas.hit(menu, Hit::Surface);
}

/// The view's actions, opened from the Helper menu. Pressing an action's key runs it.
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
