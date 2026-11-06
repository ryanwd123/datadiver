use crate::application::commands::CommandId;
use crate::application::keymap::{ACTIONS as KEYMAP_ACTIONS, ActionCategory, ActionId, KeyContext, KeymapFile};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Rect, rect, text};
use crate::ui::widgets::{Fonts, colors, key_cap};

/// The grid's two key menus. The Helper menu, opened with Space, lists the
/// grid's own shortcuts, which also work while it is open. The Command menu,
/// opened with Ctrl+K, has keys for commands used less often, which work
/// only while it is open.
pub struct SpaceMenu {
    pub sql: bool,
    /// A saved SQL source, whose rows views can be built on.
    pub sql_source: bool,
    pub sql_slicers: bool,
    pub table: bool,
    pub has_sort: bool,
}

/// One line of a menu: a label, its keys, and what a click does.
struct Item {
    label: &'static str,
    strokes: Vec<String>,
    button: Button,
}

const COLUMN_WIDTH: f32 = 290.0;
const MIN_COLUMN_WIDTH: f32 = 240.0;

impl SpaceMenu {
    /// Whether the view offers `command`; both menus share this.
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

    /// The Command menu's commands that the view offers, in the keymap's order.
    fn command_items(&self, keymap: &KeymapFile) -> Vec<Item> {
        let context = if self.table { KeyContext::TableCommandMenu } else { KeyContext::QueryCommandMenu };
        KEYMAP_ACTIONS
            .iter()
            .filter(|spec| spec.context == KeyContext::CommandMenu || spec.context == context)
            .filter_map(|spec| {
                let command = spec.command.filter(|command| self.offers(*command))?;
                let strokes: Vec<String> = keymap
                    .bindings_for(spec.id, spec.context)
                    .into_iter()
                    .map(|(stroke, _)| stroke)
                    .collect();
                (!strokes.is_empty()).then_some(Item {
                    label: spec.name,
                    strokes,
                    button: Button::CommandMenuItem(command),
                })
            })
            .collect()
    }

    /// The Helper menu: every grid shortcut.
    pub fn paint(&self, canvas: &mut Canvas<Hit>, fonts: &Fonts, keymap: &KeymapFile) {
        let more = keymap
            .bindings_for(ActionId::CommandMenu, KeyContext::Grid)
            .first()
            .map(|(stroke, _)| format!(" · {} for more commands", crate::ui::platform::keys::display(stroke)))
            .unwrap_or_default();
        let hint = format!("the grid's keys, which work here too{more}");
        paint_menu(canvas, fonts, "Helper Menu", &hint, &self.grid_items(keymap), false);
    }

    /// The Command menu, where the Helper menu opens.
    pub fn paint_commands(&self, canvas: &mut Canvas<Hit>, fonts: &Fonts, keymap: &KeymapFile) {
        let items = self.command_items(keymap);
        paint_menu(canvas, fonts, "Command Menu", "press a key to run its command · Esc closes", &items, true);
    }
}

/// A titled menu of `items` at the bottom right, in the fewest columns that
/// fit the window. `keys_first` lines the keys up on the left, for menus
/// whose keys are single letters to press.
fn paint_menu(canvas: &mut Canvas<Hit>, fonts: &Fonts, title: &str, hint: &str, items: &[Item], keys_first: bool) {
    let title_font = fonts.ui.semibold();
    let row_height = text::line_height(&fonts.ui) + 6.0;
    let room = (canvas.width - 16.0).max(MIN_COLUMN_WIDTH + 16.0);
    let max_columns = (((room - 16.0) / MIN_COLUMN_WIDTH).floor() as usize).clamp(1, 3);
    let height_for = |columns: usize| 24.0 + row_height * (1 + items.len().div_ceil(columns)) as f32;
    let max_height = (canvas.height - 48.0).max(120.0);
    let columns = (1..=max_columns)
        .find(|columns| height_for(*columns) <= max_height)
        .unwrap_or(max_columns);
    let title_width = text::measure(title, &title_font).0;
    // Wide enough for the title and its hint on one line; the columns share
    // any width the header needs.
    let header_width = (title_width + text::measure(hint, &fonts.small).0 + 48.0).min(room);
    let column_width = ((room - 16.0) / columns as f32).min(COLUMN_WIDTH).max((header_width - 16.0) / columns as f32);
    let width = column_width * columns as f32 + 16.0;
    let height = height_for(columns).min(max_height);
    let menu = rect(canvas.width - 8.0 - width, canvas.height - 30.0 - height, width, height);
    draw_popup(canvas, menu);
    canvas.push_clip(menu);
    canvas.text(rect(menu.x + 16.0, menu.y + 8.0, title_width + 4.0, row_height), title, &title_font, colors::FOREGROUND, Align::Left);
    canvas.text(
        rect(menu.x + 28.0 + title_width, menu.y + 8.0, (width - title_width - 44.0).max(0.0), row_height),
        hint,
        &fonts.small,
        colors::MUTED,
        Align::Left,
    );
    let top = menu.y + 8.0 + row_height + 4.0;
    let rows = items.len().div_ceil(columns).max(1);
    // Labels after the keys start where the widest keys end.
    let label_x = keys_first.then(|| items.iter().map(|item| keys_width(fonts, item)).fold(40.0, f32::max) + 12.0);
    for (index, item) in items.iter().enumerate() {
        let row = rect(
            menu.x + 8.0 + (index / rows) as f32 * column_width,
            top + (index % rows) as f32 * row_height,
            column_width - 4.0,
            row_height,
        );
        paint_item(canvas, fonts, row, item, label_x);
    }
    canvas.pop_clip();
}

/// The width of an item's key caps, with the gaps between them.
fn keys_width(fonts: &Fonts, item: &Item) -> f32 {
    item.strokes
        .iter()
        .map(|stroke| text::measure(&crate::ui::platform::keys::display(stroke), &fonts.small).0 + 14.0)
        .sum()
}

/// Paints one item: its keys at the left and the label `label_x` into the
/// row, or with `None`, the label at the left and the keys at the right edge.
fn paint_item(canvas: &mut Canvas<Hit>, fonts: &Fonts, row: Rect, item: &Item, label_x: Option<f32>) {
    if canvas.hovered(row) {
        canvas.fill_rounded(row, 4.0, colors::STRIPE);
    }
    let label = |canvas: &mut Canvas<Hit>, x: f32, right: f32| {
        canvas.text(rect(x, row.y, (right - x).max(0.0), row.h), item.label, &fonts.ui, colors::FOREGROUND, Align::Left);
    };
    if let Some(label_x) = label_x {
        let mut x = row.x + 6.0;
        for stroke in &item.strokes {
            x += key_cap(canvas, x, row.y + 3.0, row.h - 6.0, stroke, &fonts.small) + 4.0;
        }
        label(canvas, (row.x + label_x).max(x + 4.0), row.right() - 4.0);
    } else {
        // Keys sit at the right edge so they line up down a column.
        let mut x = row.right() - 4.0 - keys_width(fonts, item);
        label(canvas, row.x + 8.0, x - 4.0);
        for stroke in &item.strokes {
            x += key_cap(canvas, x, row.y + 3.0, row.h - 6.0, stroke, &fonts.small) + 4.0;
        }
    }
    canvas.hit(row, Hit::Button(item.button.clone()));
}

fn draw_popup(canvas: &mut Canvas<Hit>, menu: Rect) {
    canvas.shadow(menu, 6.0);
    canvas.fill_rounded(menu, 6.0, colors::POPOVER);
    canvas.stroke_rounded(menu, 6.0, colors::BORDER, 1.0);
    canvas.hit(menu, Hit::Surface);
}

/// The view's actions, opened from the Command menu. Pressing an action's key runs it.
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
