//! The SQL helper menu, opened with Ctrl+K in the SQL editor: a key for
//! each command that works on the query at the caret.

use crate::application::keymap::{ACTIONS, KeyContext, KeymapFile};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::{Align, Canvas, Rect, rect, text};
use crate::ui::widgets::{Fonts, colors, key_cap};

const COLUMN_WIDTH: f32 = 270.0;

/// The menu's (label, keys, command), in the keymap's order.
fn items(keymap: &KeymapFile) -> Vec<(&'static str, Vec<String>, Button)> {
    ACTIONS
        .iter()
        .filter(|spec| spec.context == KeyContext::SqlHelperMenu)
        .filter_map(|spec| {
            let command = spec.command?;
            let strokes: Vec<String> = keymap
                .bindings_for(spec.id, spec.context)
                .into_iter()
                .map(|(stroke, _)| stroke)
                .collect();
            (!strokes.is_empty()).then_some((spec.name, strokes, Button::SqlHelperCommand(command)))
        })
        .collect()
}

/// Paints the menu over the top right of the editor at `area`.
pub fn paint(canvas: &mut Canvas<Hit>, fonts: &Fonts, keymap: &KeymapFile, area: Rect) {
    let font = &fonts.ui;
    let title_font = font.semibold();
    let row_height = text::line_height(font) + 6.0;
    let items = items(keymap);
    let room = (canvas.width - 16.0).max(COLUMN_WIDTH);
    let columns = if room >= COLUMN_WIDTH * 2.0 + 16.0 { 2 } else { 1 };
    let rows = items.len().div_ceil(columns);
    let width = COLUMN_WIDTH * columns as f32 + 16.0;
    let height = ((rows + 1) as f32 * row_height + 24.0).min((canvas.height - 16.0).max(row_height * 3.0));
    let right = (area.right() - 8.0).min(canvas.width - 8.0);
    let menu = rect((right - width).max(8.0), (area.y + 8.0).min(canvas.height - height - 8.0).max(8.0), width, height);
    canvas.shadow(menu, 6.0);
    canvas.fill_rounded(menu, 6.0, colors::POPOVER);
    canvas.stroke_rounded(menu, 6.0, colors::BORDER, 1.0);
    canvas.hit(menu, Hit::Surface);
    canvas.push_clip(menu);
    let title = "SQL Helper";
    let title_width = text::measure(title, &title_font).0;
    canvas.text(rect(menu.x + 16.0, menu.y + 8.0, title_width + 4.0, row_height), title, &title_font, colors::FOREGROUND, Align::Left);
    canvas.text(
        rect(menu.x + 28.0 + title_width, menu.y + 8.0, (width - title_width - 44.0).max(0.0), row_height),
        "acts on the query at the caret · Esc closes",
        &fonts.small,
        colors::MUTED,
        Align::Left,
    );
    let top = menu.y + 8.0 + row_height + 4.0;
    for (index, (label, strokes, button)) in items.iter().enumerate() {
        let row = rect(
            menu.x + 8.0 + (index / rows) as f32 * COLUMN_WIDTH,
            top + (index % rows) as f32 * row_height,
            COLUMN_WIDTH - 4.0,
            row_height,
        );
        if canvas.hovered(row) {
            canvas.fill_rounded(row, 4.0, colors::STRIPE);
        }
        // Keys first, so the letter to press lines up down the column.
        let mut x = row.x + 6.0;
        for stroke in strokes {
            x += key_cap(canvas, x, row.y + 3.0, row.h - 6.0, stroke, &fonts.small) + 4.0;
        }
        let label_x = (row.x + 52.0).max(x + 4.0);
        canvas.text(rect(label_x, row.y, (row.right() - label_x - 4.0).max(0.0), row.h), label, font, colors::FOREGROUND, Align::Left);
        canvas.hit(row, Hit::Button(button.clone()));
    }
    canvas.pop_clip();
}
