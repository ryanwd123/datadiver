use crate::application::theme::{ThemeColor, ThemeRole, ThemeSettings};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Color, Point, Rect, rect, text};
use crate::ui::widgets::color_picker::{ColorPicker, Swatches};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{Column, Fonts, Row, WidgetId, colors};

/// The color picker that opens from a role's swatch.
const PICKER_WIDTH: f32 = 248.0;
const PICKER_PADDING: f32 = 12.0;

pub struct ThemeEditor {
    pub(crate) pickers: Vec<(ThemeRole, Editor)>,
    pub(crate) preview: ThemeSettings,
    error: Option<String>,
    /// The role whose picker is open, and the picker.
    pub(crate) open: Option<(usize, ColorPicker)>,
    /// Where the open role's swatch was last painted, to open the picker by.
    anchor: Rect,
}

impl ThemeEditor {
    pub fn new(theme: ThemeSettings) -> Self {
        let pickers = ThemeRole::ALL
            .into_iter()
            .map(|role| {
                let mut input = Editor::single_line();
                input.set_text(role.color(&theme).hex());
                (role, input)
            })
            .collect();
        Self {
            pickers,
            preview: theme,
            error: None,
            open: None,
            anchor: Rect::default(),
        }
    }

    /// Opens the picker for role `index`, or closes it when it's open.
    pub fn toggle_picker(&mut self, index: usize) {
        self.open = match self.open.take() {
            Some((open, _)) if open == index => None,
            _ if index < self.pickers.len() => Some((index, ColorPicker::new(self.color(index)))),
            _ => None,
        };
    }

    pub fn close_picker(&mut self) -> bool {
        self.open.take().is_some()
    }

    /// The open picker's color changed: its role takes it.
    fn picked(&mut self) {
        if let Some((index, picker)) = &self.open {
            let (index, rgb) = (*index, picker.rgb());
            self.set_color(index, rgb);
        }
    }

    fn update_preview(&mut self) {
        if let Ok(theme) = self.build() {
            self.preview = theme;
            self.error = None;
        }
    }

    pub fn build(&self) -> Result<ThemeSettings, String> {
        let mut theme = ThemeSettings::default();
        for (role, input) in &self.pickers {
            let color = ThemeColor::parse(input.text()).map_err(|error| format!("{}: {error}", role.label()))?;
            role.set(&mut theme, color);
        }
        Ok(theme)
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn focus_first(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.pickers[0].1.id);
    }

    pub fn reset(&mut self) {
        self.preview = ThemeSettings::default();
        for (role, input) in &mut self.pickers {
            input.set_text(role.color(&self.preview).hex());
        }
        self.error = None;
    }

    /// Sets role `index` to `0xRRGGBB`, keeping its alpha.
    pub fn set_color(&mut self, index: usize, rgb: u32) {
        let Some((role, input)) = self.pickers.get_mut(index) else {
            return;
        };
        let alpha = ThemeColor::parse(input.text())
            .map(|color| color.rgba() & 0xff)
            .unwrap_or(0xff);
        let color = ThemeColor::parse(&format!("#{:08X}", (rgb << 8) | alpha)).expect("valid color");
        let _ = role;
        input.set_text(color.hex());
        self.update_preview();
    }

    /// The color of role `index` as `0xRRGGBB`, for the color picker.
    pub fn color(&self, index: usize) -> u32 {
        self.pickers
            .get(index)
            .and_then(|(_, input)| ThemeColor::parse(input.text()).ok())
            .map_or(0, |color| color.rgba() >> 8)
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        self.pickers.iter().any(|(_, input)| input.id == id)
    }

    fn input_mut(&mut self, id: WidgetId) -> Option<&mut Editor> {
        self.pickers
            .iter_mut()
            .map(|(_, input)| input)
            .find(|input| input.id == id)
    }

    /// Returns whether the key was used and whether Enter asked to save.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> (bool, bool) {
        // Escape puts an open picker away before it closes the dialog.
        if event.is("escape") && self.close_picker() {
            return (true, false);
        }
        let Some(id) = *focus else {
            return (false, false);
        };
        if event.key == "tab" && !event.modifiers.control && !event.modifiers.alt {
            let ids = self.pickers.iter().map(|(_, input)| input.id).collect::<Vec<_>>();
            if let Some(position) = ids.iter().position(|input| *input == id) {
                let next = if event.modifiers.shift {
                    (position + ids.len() - 1) % ids.len()
                } else {
                    (position + 1) % ids.len()
                };
                *focus = Some(ids[next]);
                return (true, false);
            }
        }
        let Some(input) = self.input_mut(id) else {
            return (false, false);
        };
        let outcome = input.key(event);
        if outcome.changed {
            self.update_preview();
        }
        (outcome.handled, outcome.submit)
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        let Some(input) = focus.and_then(|id| self.input_mut(id)) else {
            return false;
        };
        input.type_text(typed);
        self.update_preview();
        true
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        if let Hit::ColorPicker(id, part) = hit
            && let Some((_, picker)) = self.open.as_mut().filter(|(_, picker)| picker.id == *id)
        {
            if picker.mouse_down(*part, pos) {
                self.picked();
            }
            return true;
        }
        // A click anywhere else puts the picker away.
        if !matches!(hit, Hit::Surface) || self.open.is_none() {
            self.open = None;
        }
        if let Hit::Editor(id, part) = hit
            && let Some(input) = self.input_mut(*id)
        {
            input.mouse_down(*part, pos, clicks, shift);
            *focus = Some(*id);
            return true;
        }
        false
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        if let Some((_, picker)) = &mut self.open
            && picker.mouse_drag(pos)
        {
            self.picked();
        }
        for (_, input) in &mut self.pickers {
            if input.is_dragging() {
                input.mouse_drag(pos);
            }
        }
    }

    pub fn mouse_up(&mut self) {
        if let Some((_, picker)) = &mut self.open {
            picker.mouse_up();
        }
        for (_, input) in &mut self.pickers {
            input.mouse_up();
        }
    }

    pub fn height(&self, fonts: &Fonts) -> f32 {
        let line = text::line_height(&fonts.small);
        let input = Editor::line_box_height(&fonts.small);
        line + 8.0
            + if self.error.is_some() { line + 8.0 } else { 0.0 }
            + (line + 10.0) * 3.0
            + 8.0
            + (input + 8.0) * self.pickers.len() as f32
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.small;
        let line = text::line_height(font);
        let mut column = Column::new(r, 8.0);
        canvas.text(column.next(line), "Choose grid colors. Changes apply when you save.", font, colors::FOREGROUND, Align::Left);
        if let Some(error) = &self.error {
            canvas.text(column.next(line), error, font, Color::rgb(0xb42318), Align::Left);
        }
        let theme = &self.preview;
        let color = |value: ThemeColor| Color::rgba(value.rgba());
        let cell_height = line + 10.0;
        let preview = column.next(cell_height * 3.0);
        let cell_width = preview.w / 3.0;
        for row in 0..3 {
            for column_index in 0..3 {
                let background = match (row, column_index) {
                    (0, 0) => theme.frozen_header_background,
                    (0, _) => theme.header_background,
                    (1, 0) => theme.frozen_background,
                    (2, 0) => theme.frozen_stripe_background,
                    (1, 1) => theme.selection_background,
                    (1, _) => theme.background,
                    _ => theme.row_stripe_background,
                };
                let cell = rect(
                    preview.x + column_index as f32 * cell_width,
                    preview.y + row as f32 * cell_height,
                    cell_width,
                    cell_height,
                );
                canvas.fill(cell, color(background));
                canvas.stroke(
                    cell,
                    color(if row == 1 && column_index == 1 {
                        theme.active_cell_border
                    } else {
                        theme.grid_border
                    }),
                    1.0,
                );
                let label = match (row, column_index) {
                    (0, 0) => "Frozen",
                    (0, 1) => "Region",
                    (0, _) => "Amount",
                    (1, 0) => "2025",
                    (2, 0) => "2026",
                    (_, 1) => "West",
                    _ => "120.00",
                };
                canvas.text(cell.inset_xy(12.0, 0.0), label, font, color(theme.foreground), Align::Left);
            }
        }
        let input_height = Editor::line_box_height(font);
        for (index, (role, input)) in self.pickers.iter_mut().enumerate() {
            let row = column.next(input_height);
            let mut items = Row::new(row, 12.0);
            let hex = items.right(135.0);
            let swatch = items.right(input_height);
            let current = ThemeColor::parse(input.text()).ok();
            canvas.fill_rounded(swatch, 4.0, Color::rgb(0xffffff));
            if let Some(current) = current {
                canvas.fill_rounded(swatch.inset(3.0), 3.0, Color::rgba(current.rgba()));
            }
            canvas.stroke_rounded(swatch, 4.0, colors::BORDER, 1.0);
            if self.open.as_ref().is_some_and(|(open, _)| *open == index) {
                canvas.stroke_rounded(swatch, 4.0, colors::SELECTED_STRONG, 2.0);
                self.anchor = swatch;
            }
            canvas.hit_with(swatch, Hit::Button(Button::ThemeSwatch(index)), crate::ui::platform::Cursor::Hand, Some("Choose a color"));
            canvas.text(items.rest(), role.label(), font, colors::FOREGROUND, Align::Left);
            let id = input.id;
            input.paint(canvas, hex, font, focus == Some(id), |part| Hit::Editor(id, part));
        }
    }
}

impl ThemeEditor {
    /// The open picker, over the dialog by its role's swatch: below it, or
    /// above when the window has no room below. Paint it after the dialog.
    pub fn paint_picker(&mut self, canvas: &mut Canvas<Hit>, fonts: &Fonts) {
        let Some(index) = self.open.as_ref().map(|(index, _)| *index) else { return };
        // Typing in the role's hex moves the picker, unless it's being dragged.
        let current = self.color(index);
        let theme: Vec<(String, u32)> = {
            let mut seen = Vec::new();
            for (role, input) in &self.pickers {
                if let Ok(color) = ThemeColor::parse(input.text()) {
                    let rgb = color.rgba() >> 8;
                    if !seen.iter().any(|(_, s)| *s == rgb) {
                        seen.push((role.label().to_string(), rgb));
                    }
                }
            }
            seen
        };
        let palette: Vec<(String, u32)> = diver_format::colors::PALETTE
            .iter()
            .map(|(name, color)| {
                (format!("{name} \u{b7} {}", color.to_hex()), u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b))
            })
            .collect();
        let groups = [Swatches { title: "IN THIS THEME", colors: &theme }, Swatches { title: "PALETTE", colors: &palette }];
        let heading = fonts.small.semibold();
        let line = text::line_height(&fonts.small);
        let inner = ColorPicker::height(PICKER_WIDTH, &groups, &heading);
        let size = (PICKER_WIDTH + 2.0 * PICKER_PADDING, inner + line + 8.0 + 2.0 * PICKER_PADDING);
        let window = canvas.bounds();
        let anchor = self.anchor;
        let x = (anchor.right() - size.0).clamp(window.x + 8.0, (window.right() - size.0 - 8.0).max(window.x + 8.0));
        let below = anchor.bottom() + 6.0;
        let y = if below + size.1 <= window.bottom() - 8.0 { below } else { (anchor.y - 6.0 - size.1).max(window.y + 8.0) };
        let frame = rect(x, y, size.0, size.1);
        let Some((_, picker)) = &mut self.open else { return };
        if !picker.is_dragging() {
            picker.set_rgb(current);
        }
        canvas.shadow(frame, 6.0);
        canvas.fill_rounded(frame, 6.0, colors::POPOVER);
        canvas.stroke_rounded(frame, 6.0, colors::BORDER, 1.0);
        canvas.hit(frame, Hit::Surface);
        let inside = frame.inset(PICKER_PADDING);
        let label = self.pickers.get(index).map(|(role, _)| role.label()).unwrap_or_default();
        canvas.text(rect(inside.x, inside.y, inside.w, line), label, &fonts.small, colors::FOREGROUND, Align::Left);
        let hex = format!("#{current:06x}");
        canvas.text(rect(inside.x, inside.y, inside.w, line), &hex, &fonts.small, colors::MUTED, Align::Right);
        picker.paint(canvas, rect(inside.x, inside.y + line + 8.0, inside.w, inner), &groups, &heading);
    }
}

#[cfg(test)]
mod tests {
    use super::ThemeEditor;
    use crate::application::theme::{ThemeColor, ThemeSettings};

    #[test]
    fn theme_drafts_preserve_exact_colors_validate_hex_and_reset() {
        let original = ThemeSettings {
            selection_background: ThemeColor::parse("#12345680").unwrap(),
            ..Default::default()
        };
        let mut editor = ThemeEditor::new(original.clone());
        assert_eq!(editor.build().unwrap(), original);
        editor.pickers[0].1.set_text("#bad");
        assert!(editor.build().unwrap_err().contains("Header background"));
        editor.pickers[0].1.set_text("#AABBCC");
        assert_eq!(editor.build().unwrap().header_background, ThemeColor::rgb(0xaabbcc));
        editor.reset();
        assert_eq!(editor.build().unwrap(), ThemeSettings::default());
    }

    #[test]
    fn choosing_a_color_updates_the_preview() {
        let mut editor = ThemeEditor::new(ThemeSettings::default());
        editor.set_color(0, 0xff0000);
        assert_eq!(editor.preview.header_background, ThemeColor::rgb(0xff0000));
        assert_eq!(editor.build().unwrap().header_background, ThemeColor::rgb(0xff0000));
    }
}
