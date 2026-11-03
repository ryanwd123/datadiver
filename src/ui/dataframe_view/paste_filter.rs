//! Filtering by values pasted from the clipboard: an overlay that names what
//! it filters, previews the clipboard, and applies it on Ctrl+V or Enter.

use super::*;
use crate::domain::pasted;
use crate::ui::hit::Button;
use crate::ui::platform::{Align, Canvas, KeyEvent, Rect, rect, system, text};
use crate::ui::widgets::{ButtonStyle, Row, button, button_height, button_width, colors};

/// A filter lists at most this many pasted values.
const MAX_VALUES: usize = 10_000;

/// What pasted values filter.
pub enum PasteTarget {
    /// A value filter on a grid column.
    Column { target: Dimension, name: String },
    /// A value card's selection.
    Slicer(String),
}

pub struct PasteFilterDialog {
    pub(crate) target: PasteTarget,
    /// Remove the rows with the pasted values instead of keeping them.
    pub(crate) exclude: bool,
    /// The clipboard's values, read when the overlay opened.
    pub(crate) values: Vec<String>,
}

impl PasteFilterDialog {
    fn name(&self) -> &str {
        match &self.target {
            PasteTarget::Column { name, .. } | PasteTarget::Slicer(name) => name,
        }
    }

    pub(crate) fn height(&self, fonts: &crate::ui::widgets::Fonts) -> f32 {
        let line = text::line_height(&fonts.ui);
        line + 8.0 + button_height(&fonts.ui) + 12.0 + line + 8.0 + text::line_height(&fonts.small)
    }

    pub(crate) fn paint(&self, canvas: &mut Canvas<Hit>, content: Rect, fonts: &crate::ui::widgets::Fonts, matches: Option<usize>) {
        let font = &fonts.ui;
        let line = text::line_height(font);
        let what = match self.target {
            PasteTarget::Column { .. } => "Column",
            PasteTarget::Slicer(_) => "Slicer",
        };
        canvas.text(rect(content.x, content.y, content.w, line), &format!("{what}: {}", self.name()), &font.semibold(), colors::FOREGROUND, Align::Left);
        let mut y = content.y + line + 8.0;
        let height = button_height(font);
        let mut modes = Row::new(rect(content.x, y, content.w, height), 8.0);
        for (label, exclude) in [("Keep  =", false), ("Exclude  -", true)] {
            let r = modes.left(button_width(label, font));
            let style = if self.exclude == exclude { ButtonStyle::Selected } else { ButtonStyle::Outline };
            button(canvas, r, label, font, style, true, Hit::Button(Button::PasteFilterMode(exclude)));
        }
        y += height + 12.0;
        let summary = if self.values.is_empty() {
            "The clipboard has no values. Copy a list, such as cells in Excel, then press Ctrl+V.".to_owned()
        } else {
            let count = self.values.len();
            let mut summary = format!(
                "{count} {} on the clipboard: {}",
                if count == 1 { "value" } else { "values" },
                pasted::preview(&self.values, 4)
            );
            if let Some(matches) = matches {
                summary.push_str(&format!(" · {matches} in this slicer"));
            }
            summary
        };
        let color = if self.values.is_empty() { colors::MUTED } else { colors::FOREGROUND };
        canvas.text(rect(content.x, y, content.w, line), &summary, font, color, Align::Left);
        y += line + 8.0;
        canvas.text(
            rect(content.x, y, content.w, text::line_height(&fonts.small)),
            "Ctrl+V pastes and applies · Enter applies · Tab switches keep and exclude · Esc cancels",
            &fonts.small,
            colors::MUTED,
            Align::Left,
        );
    }
}

impl DataFrameView {
    /// Opens the overlay for a value card's slicer, or else for the grid's
    /// active column.
    pub(crate) fn open_paste_filter(&mut self, slicer: Option<String>, cx: &mut Cx<Self>) {
        let target = match slicer {
            Some(column) => PasteTarget::Slicer(column),
            None => {
                let column = match self.grid.selection() {
                    Some(selection) if selection.active.column < self.session.result.column_count() => selection.active.column,
                    _ => {
                        self.error = Some("Select a cell in the column to filter".into());
                        cx.notify();
                        return;
                    }
                };
                match self.value_filter_target(column) {
                    Ok(target) => PasteTarget::Column {
                        target,
                        name: self.session.result.columns[column].0.clone(),
                    },
                    Err(error) => {
                        self.error = Some(error);
                        cx.notify();
                        return;
                    }
                }
            }
        };
        let values = clipboard_values();
        // Focus stays where it was, so it returns there once the overlay closes.
        let focus = self.focus;
        self.open_dialog(Dialog::PasteFilter(PasteFilterDialog { target, exclude: false, values }), focus, cx);
    }

    /// Keys while the overlay is open.
    pub(crate) fn paste_filter_key(&mut self, event: &KeyEvent, cx: &mut Cx<Self>) -> bool {
        let Some(Dialog::PasteFilter(dialog)) = &mut self.dialog else {
            return false;
        };
        let m = event.modifiers;
        if event.key == "v" && m.control && !m.alt && !m.shift {
            dialog.values = clipboard_values();
            self.apply_paste_filter(cx);
            return true;
        }
        match event.key.as_str() {
            "enter" if !m.modified() => self.apply_paste_filter(cx),
            "=" if !m.modified() => dialog.exclude = false,
            "-" if !m.modified() => dialog.exclude = true,
            "tab" if !m.control && !m.alt => dialog.exclude = !dialog.exclude,
            _ => return false,
        }
        cx.notify();
        true
    }

    /// Filters by the overlay's values and closes it; with no values it stays
    /// open to wait for some.
    pub(crate) fn apply_paste_filter(&mut self, cx: &mut Cx<Self>) {
        let Some(Dialog::PasteFilter(dialog)) = &self.dialog else {
            return;
        };
        if dialog.values.is_empty() {
            self.error = Some("The clipboard has no values to filter by".into());
            cx.notify();
            return;
        }
        if dialog.values.len() > MAX_VALUES {
            self.error = Some(format!(
                "Paste at most {} values to filter by",
                crate::domain::formatting::group_numeric_text(&MAX_VALUES.to_string())
            ));
            cx.notify();
            return;
        }
        let Some(Dialog::PasteFilter(dialog)) = self.dialog.take() else {
            return;
        };
        self.error = None;
        let PasteFilterDialog { target, exclude, values } = dialog;
        match target {
            PasteTarget::Column { target, .. } => {
                let filter = ValueFilter {
                    target,
                    values: values.into_iter().map(Some).collect(),
                    exclude,
                };
                self.add_value_filters(vec![filter], cx);
            }
            PasteTarget::Slicer(column) => {
                match self.slicer_sidebar.select_pasted(&column, &values, exclude) {
                    Some(0) => {
                        self.error = Some(format!("{column} has none of the {} pasted values", values.len()));
                    }
                    Some(matched) => {
                        let missing = values.len() - matched;
                        let mut status = format!("{matched} of {} pasted values found in {column}", values.len());
                        if missing > 0 {
                            status.push_str(&format!(" · {missing} not found"));
                        }
                        self.copy_status = Some(status);
                    }
                    None => self.error = Some(format!("The {column} slicer is gone")),
                }
                self.drain_slicer_events(cx);
            }
        }
        cx.notify();
    }
}

fn clipboard_values() -> Vec<String> {
    system::read_clipboard_text()
        .map(|text| pasted::pasted_values(&text))
        .unwrap_or_default()
}
