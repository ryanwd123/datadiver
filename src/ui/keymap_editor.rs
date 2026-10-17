use crate::application::keymap::{
    ACTIONS, ActionId, KeyContext, KeymapFile, action_name, canonical_keystroke, supported_contexts,
};
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::KeyEvent;
use crate::ui::platform::{Align, Canvas, Point, Rect, rect, text};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::{ButtonStyle, Fonts, Row, WidgetId, button, button_height, button_width, colors, scroll};

#[derive(Clone, Debug)]
pub enum KeymapEditorEvent {
    Changed(KeymapFile),
    Back,
}

struct KeymapRow {
    action: ActionId,
    context: KeyContext,
    strokes: Vec<(String, bool)>,
}

pub struct KeymapEditor {
    pub keymap: KeymapFile,
    pub search: Editor,
    /// Takes key presses while capturing a binding.
    pub id: WidgetId,
    capture: Option<(ActionId, KeyContext)>,
    pending_replace: Option<(ActionId, KeyContext, String, ActionId)>,
    message: Option<String>,
    reset_confirmation: bool,
    rows: Vec<KeymapRow>,
    scroll: scroll::Scroll,
    rows_area: Rect,
    content_height: f32,
    events: Vec<KeymapEditorEvent>,
}

impl KeymapEditor {
    pub fn new(keymap: KeymapFile) -> Self {
        Self {
            keymap,
            search: Editor::single_line().with_placeholder("Filter actions, shortcuts, contexts"),
            id: WidgetId::new(),
            capture: None,
            pending_replace: None,
            message: None,
            reset_confirmation: false,
            rows: Vec::new(),
            scroll: scroll::Scroll::default(),
            rows_area: Rect::default(),
            content_height: 0.0,
            events: Vec::new(),
        }
    }

    pub fn take_events(&mut self) -> Vec<KeymapEditorEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    fn changed(&mut self) {
        self.message = None;
        self.events.push(KeymapEditorEvent::Changed(self.keymap.clone()));
    }

    fn bind(&mut self, action: ActionId, context: KeyContext, stroke: String) {
        match canonical_keystroke(&stroke) {
            Ok(stroke) => {
                let conflict = self.keymap.direct_binding_for(&stroke, context);
                if let Some(conflict) = conflict.filter(|conflict| *conflict != action)
                    && ACTIONS
                        .iter()
                        .filter(|spec| spec.id == conflict && spec.context == context)
                        .flat_map(|spec| spec.default_bindings)
                        .any(|default| default.eq_ignore_ascii_case(&stroke))
                {
                    self.pending_replace = Some((action, context, stroke.clone(), conflict));
                    self.message = Some(format!(
                        "{} uses {stroke} in {}. Replace it?",
                        action_name(conflict),
                        context.label()
                    ));
                    return;
                }
                match self.keymap.add(action, context, stroke, false) {
                    Ok(_) => self.changed(),
                    Err(error) => self.message = Some(error),
                }
            }
            Err(error) => self.message = Some(error),
        }
    }

    fn rebuild_rows(&mut self) {
        let query = self.search.text().to_lowercase();
        self.rows.clear();
        for spec in ACTIONS {
            for context in supported_contexts(spec.context) {
                let strokes = self.keymap.bindings_for(spec.id, context);
                let source = if strokes.iter().any(|(_, user)| *user) {
                    "User"
                } else {
                    "Default"
                };
                let stroke_text = stroke_text(&strokes);
                let filter_text = format!("{} {stroke_text} {} {source}", spec.name, context.label()).to_lowercase();
                if !filter_text.contains(&query) {
                    continue;
                }
                self.rows.push(KeymapRow {
                    action: spec.id,
                    context,
                    strokes,
                });
            }
        }
    }

    /// Key presses while the editor shows. Returns whether the press was used.
    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        if let Some((action, context)) = self.capture {
            if event.key == "escape" {
                self.capture = None;
                self.message = None;
            } else {
                self.capture = None;
                self.bind(action, context, event.stroke());
            }
            return true;
        }
        if *focus == Some(self.search.id) {
            let outcome = self.search.key(event);
            if outcome.changed {
                self.scroll.offset = 0.0;
            }
            return outcome.handled;
        }
        false
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if self.capture.is_some() {
            return true;
        }
        if focus == Some(self.search.id) {
            self.search.type_text(typed);
            self.scroll.offset = 0.0;
            return true;
        }
        false
    }

    pub fn mouse_down(&mut self, hit: &Hit, pos: Point, clicks: u32, shift: bool, focus: &mut Option<WidgetId>) -> bool {
        match hit {
            Hit::Editor(id, part) if *id == self.search.id => {
                self.search.mouse_down(*part, pos, clicks, shift);
                *focus = Some(self.search.id);
            }
            Hit::Button(Button::KeymapBack) => self.events.push(KeymapEditorEvent::Back),
            Hit::Button(Button::KeymapResetAll) => {
                if self.reset_confirmation {
                    self.keymap = KeymapFile::default();
                    self.reset_confirmation = false;
                    self.changed();
                } else {
                    self.reset_confirmation = true;
                }
            }
            Hit::Button(Button::KeymapBind(row)) => {
                if let Some(row) = self.rows.get(*row) {
                    self.capture = Some((row.action, row.context));
                    self.pending_replace = None;
                    self.message = Some(format!(
                        "Press one key for {} in {}. Escape cancels.",
                        action_name(row.action),
                        row.context.label()
                    ));
                    *focus = Some(self.id);
                }
            }
            Hit::Button(Button::KeymapReset(row)) => {
                if let Some(row) = self.rows.get(*row) {
                    let (action, context) = (row.action, row.context);
                    self.keymap.reset(action, context);
                    self.changed();
                }
            }
            Hit::Button(Button::KeymapRemove(row, stroke)) => {
                if let Some(row) = self.rows.get(*row)
                    && let Some((stroke, _)) = row.strokes.get(*stroke)
                {
                    let (action, context, stroke) = (row.action, row.context, stroke.clone());
                    self.keymap.remove(action, context, &stroke);
                    self.changed();
                }
            }
            Hit::Button(Button::KeymapReplace) => {
                if let Some((action, context, stroke, _)) = self.pending_replace.clone() {
                    match self.keymap.add(action, context, stroke, true) {
                        Ok(_) => {
                            self.pending_replace = None;
                            self.capture = None;
                            self.changed();
                        }
                        Err(error) => self.message = Some(error),
                    }
                }
            }
            Hit::Button(Button::KeymapCancelReplace) => {
                self.pending_replace = None;
                self.message = None;
            }
            Hit::DialogBar => {
                let bar = rect(self.rows_area.right() - scroll::BAR_WIDTH, self.rows_area.y, scroll::BAR_WIDTH, self.rows_area.h);
                self.scroll.press(pos.y - bar.y, bar.h, self.rows_area.h, self.content_height);
            }
            _ => return false,
        }
        true
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        if self.search.is_dragging() {
            self.search.mouse_drag(pos);
        }
        let bar = rect(self.rows_area.right() - scroll::BAR_WIDTH, self.rows_area.y, scroll::BAR_WIDTH, self.rows_area.h);
        self.scroll.drag(pos.y - bar.y, bar.h, self.rows_area.h, self.content_height);
    }

    pub fn mouse_up(&mut self) {
        self.search.mouse_up();
        self.scroll.release();
    }

    pub fn wheel(&mut self, delta: Point, line: f32) -> bool {
        self.scroll
            .scroll_by(-delta.y * line, self.rows_area.h, self.content_height)
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        self.rebuild_rows();
        let font = &fonts.ui;
        let line = text::line_height(font);
        canvas.fill(r, colors::BACKGROUND);
        let header_height = button_height(font) + 24.0;
        let header = rect(r.x, r.y, r.w, header_height);
        canvas.fill(header, colors::STRIPE);
        let mut items = Row::new(header.inset_xy(16.0, 12.0), 12.0);
        let back = items.left(button_width("← Back", font));
        button(canvas, back, "← Back", font, ButtonStyle::Outline, true, Hit::Button(Button::KeymapBack));
        let reset_label = if self.reset_confirmation { "Confirm Reset All" } else { "Reset All" };
        let reset = items.right(button_width(reset_label, font));
        button(canvas, reset, reset_label, font, ButtonStyle::Outline, true, Hit::Button(Button::KeymapResetAll));
        canvas.text(items.rest(), "Keymap Editor", &fonts.large, colors::FOREGROUND, Align::Left);
        let mut y = header.bottom() + 8.0;
        let search = rect(r.x + 16.0, y, r.w - 32.0, Editor::line_box_height(font));
        let id = self.search.id;
        self.search.paint(canvas, search, font, focus == Some(id), |part| Hit::Editor(id, part));
        y = search.bottom() + 8.0;
        if let Some(message) = self.message.clone() {
            let row = rect(r.x + 16.0, y, r.w - 32.0, button_height(font));
            let mut items = Row::new(row, 8.0);
            if self.pending_replace.is_some() {
                let cancel = items.right(button_width("Cancel", font));
                button(canvas, cancel, "Cancel", font, ButtonStyle::Outline, true, Hit::Button(Button::KeymapCancelReplace));
                let replace = items.right(button_width("Replace", font));
                button(canvas, replace, "Replace", font, ButtonStyle::Outline, true, Hit::Button(Button::KeymapReplace));
            }
            canvas.text(items.rest(), &message, font, colors::MUTED, Align::Left);
            y = row.bottom() + 8.0;
        }
        let column_header = rect(r.x, y, r.w, line + 16.0);
        canvas.fill(column_header, colors::STRIPE);
        let widths = |row: Rect| {
            let mut items = Row::new(row.inset_xy(8.0, 0.0), 8.0);
            let action = items.left(250.0);
            let context = items.left(170.0);
            let buttons = items.right(210.0);
            let source = items.right(70.0);
            (action, context, items.rest(), source, buttons)
        };
        let bold = font.semibold();
        let (action, context, strokes, source, _) = widths(column_header);
        canvas.text(action, "Action", &bold, colors::FOREGROUND, Align::Left);
        canvas.text(context, "Context", &bold, colors::FOREGROUND, Align::Left);
        canvas.text(strokes, "Keystrokes", &bold, colors::FOREGROUND, Align::Left);
        canvas.text(source, "Source", &bold, colors::FOREGROUND, Align::Left);
        let rows_area = rect(r.x, column_header.bottom(), r.w, (r.bottom() - column_header.bottom()).max(0.0));
        self.rows_area = rows_area;
        let row_height = button_height(&fonts.small) + 10.0;
        self.content_height = self.rows.len() as f32 * row_height;
        self.scroll.clamp(rows_area.h, self.content_height);
        canvas.push_clip(rows_area);
        let first = (self.scroll.offset / row_height).floor() as usize;
        let count = (rows_area.h / row_height).ceil() as usize + 1;
        for (index, row) in self.rows.iter().enumerate().skip(first).take(count) {
            let top = rows_area.y + index as f32 * row_height - self.scroll.offset;
            let row_rect = rect(rows_area.x, top, rows_area.w - scroll::BAR_WIDTH, row_height);
            if index % 2 == 1 {
                canvas.fill(row_rect, colors::STRIPE);
            }
            canvas.fill(rect(row_rect.x, row_rect.bottom() - 1.0, row_rect.w, 1.0), colors::BORDER);
            let (action, context, strokes, source, buttons) = widths(row_rect);
            canvas.text(action, action_name(row.action), font, colors::FOREGROUND, Align::Left);
            canvas.text(context, row.context.label(), &fonts.small, colors::FOREGROUND, Align::Left);
            let mut stroke_items = Row::new(strokes, 6.0);
            if row.strokes.is_empty() {
                canvas.text(stroke_items.rest(), "Unbound", font, colors::MUTED, Align::Left);
            }
            for (stroke_index, (stroke, user)) in row.strokes.iter().enumerate() {
                let label = format!("{stroke}{}", if *user { " · User" } else { "" });
                let width = text::measure(&label, font).0 + 4.0;
                let label_rect = stroke_items.left(width);
                canvas.text(label_rect, &label, font, colors::FOREGROUND, Align::Left);
                let remove = stroke_items.left(20.0);
                let remove = rect(remove.x, remove.y + (remove.h - 20.0) / 2.0, 20.0, 20.0);
                if canvas.hovered(remove) {
                    canvas.fill_rounded(remove, 4.0, colors::HOVER);
                }
                canvas.text(remove, "×", font, colors::MUTED, Align::Center);
                canvas.hit(remove, Hit::Button(Button::KeymapRemove(index, stroke_index)));
            }
            let source_text = if row.strokes.iter().any(|(_, user)| *user) { "User" } else { "Default" };
            canvas.text(source, source_text, &fonts.small, colors::FOREGROUND, Align::Left);
            let mut button_row = Row::new(rect(buttons.x, buttons.y + 5.0, buttons.w, button_height(&fonts.small)), 6.0);
            let reset = button_row.right(button_width("Reset", &fonts.small));
            button(canvas, reset, "Reset", &fonts.small, ButtonStyle::Outline, true, Hit::Button(Button::KeymapReset(index)));
            let label = if self.capture == Some((row.action, row.context)) { "Press key…" } else { "Bind" };
            let bind = button_row.right(button_width(label, &fonts.small));
            button(canvas, bind, label, &fonts.small, ButtonStyle::Outline, true, Hit::Button(Button::KeymapBind(index)));
        }
        canvas.pop_clip();
        let bar = rect(rows_area.right() - scroll::BAR_WIDTH, rows_area.y, scroll::BAR_WIDTH, rows_area.h);
        scroll::vertical(canvas, bar, rows_area.h, self.content_height, &self.scroll, Hit::DialogBar);
    }
}

fn stroke_text(strokes: &[(String, bool)]) -> String {
    if strokes.is_empty() {
        return "Unbound".to_owned();
    }
    strokes
        .iter()
        .map(|(stroke, is_user)| format!("{stroke}{}", if *is_user { " · User" } else { "" }))
        .collect::<Vec<_>>()
        .join(", ")
}
