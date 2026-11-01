//! The conditional formatting manager: the view's format sets, each rules in
//! the formatting language and the columns they apply to, with presets.
//! It fills a window of its own beside the grid's, and every change applies
//! at once, so the grid shows it while typing.

use std::cell::RefCell;
use std::rc::Rc;

use diver_format::editor::FormatLanguage;
use diver_format::{PRESETS, Severity};
use windows::Win32::Foundation::HWND;

use crate::domain::documents::ConditionalFormat;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Align, Canvas, Cursor, Point, Rect, rect, text};
use crate::ui::widgets::hosted_editor::{HostedEditor, MarkKind};
use crate::ui::widgets::scroll::{self, Scroll};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

/// The command "Pick a color…" sends.
const PICK_COLOR: u32 = 1;
const LIST_WIDTH: f32 = 240.0;
/// The presets drop-down, which may be wider than the list it opens from.
const PRESETS_WIDTH: f32 = 300.0;

pub enum FormatManagerEvent {
    Changed(Vec<ConditionalFormat>),
    /// Choose a color for the `#…` at the caret (or insert one there).
    PickColor,
}

/// Groups and presets as the list shows them.
enum Entry {
    Group(&'static str),
    Preset(usize),
}

fn preset_entries() -> Vec<Entry> {
    let mut out = Vec::new();
    let mut group = "";
    for (index, preset) in PRESETS.iter().enumerate() {
        if preset.group != group {
            group = preset.group;
            out.push(Entry::Group(group));
        }
        out.push(Entry::Preset(index));
    }
    out
}

/// Where the open presets list scrolls, from the last frame.
#[derive(Clone, Copy)]
struct PresetList {
    track: Rect,
    viewport: f32,
    content: f32,
}

pub struct FormatManager {
    pub(crate) formats: Vec<ConditionalFormat>,
    pub(crate) selected: usize,
    pub(crate) editor: HostedEditor,
    /// The view's columns, shared with the editor's language service.
    columns: Rc<RefCell<Vec<diver_format::Column>>>,
    problems: Vec<diver_format::Diagnostic>,
    rules_running: usize,
    pub(crate) presets_open: bool,
    preset_scroll: Scroll,
    preset_list: Option<PresetList>,
    events: Vec<FormatManagerEvent>,
}

impl FormatManager {
    /// Opens on `formats`, selecting the first that formats `active` (the
    /// grid's column), or a new set for it when none does.
    pub fn new(
        host: Option<HWND>,
        mut formats: Vec<ConditionalFormat>,
        columns: Vec<diver_format::Column>,
        active: Option<String>,
    ) -> Self {
        let selected = match active.as_ref().and_then(|name| formats.iter().position(|f| f.columns.contains(name))) {
            Some(index) => index,
            None if formats.is_empty() || active.is_some() => {
                formats.push(ConditionalFormat { rules: String::new(), columns: active.into_iter().collect() });
                formats.len() - 1
            }
            None => 0,
        };
        let columns = Rc::new(RefCell::new(columns));
        let editor = HostedEditor::rules(host, FormatLanguage::new(columns.clone(), PICK_COLOR));
        let mut manager = FormatManager {
            formats,
            selected,
            editor,
            columns,
            problems: Vec::new(),
            rules_running: 0,
            presets_open: false,
            preset_scroll: Scroll::default(),
            preset_list: None,
            events: Vec::new(),
        };
        manager.load_selected();
        manager
    }

    pub fn take_events(&mut self) -> Vec<FormatManagerEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn focus(&self, focus: &mut Option<WidgetId>) {
        *focus = Some(self.editor.id);
    }

    pub fn contains(&self, id: WidgetId) -> bool {
        id == self.editor.id
    }

    /// Takes in formats and columns changed outside the manager, such as by
    /// undo or a new result, leaving what already matches alone.
    pub fn sync_scope(&mut self, formats: &[ConditionalFormat], columns: Vec<diver_format::Column>) {
        if *self.columns.borrow() != columns {
            *self.columns.borrow_mut() = columns;
            self.check();
        }
        if self.kept() != formats {
            self.formats = formats.to_vec();
            if self.formats.is_empty() {
                self.formats.push(ConditionalFormat::default());
            }
            self.selected = self.selected.min(self.formats.len() - 1);
            self.load_selected();
        }
    }

    fn load_selected(&mut self) {
        let text = self.formats.get(self.selected).map(|f| f.rules.clone()).unwrap_or_default();
        self.editor.set_text(&text);
        self.editor.set_selection(text.len()..text.len());
        self.check();
    }

    fn check(&mut self) {
        let rules = self.formats.get(self.selected).map(|f| f.rules.clone()).unwrap_or_default();
        let checked = diver_format::check(&rules, &self.columns.borrow());
        self.rules_running = checked.program.rules.len();
        self.problems = checked.diagnostics;
        let swatches = diver_format::help::color_spans(&rules)
            .into_iter()
            .map(|(range, color)| {
                let options = code_editor::DecorationOptions {
                    background: Some(color.argb()),
                    foreground: Some(color.contrast_text().argb()),
                    stickiness: code_editor::Stickiness::NeverGrows,
                    ..Default::default()
                };
                (range, options)
            })
            .collect();
        self.editor.set_marks(MarkKind::Colors, swatches);
    }

    /// Format sets worth keeping: blank ones are left out.
    fn kept(&self) -> Vec<ConditionalFormat> {
        self.formats.iter().filter(|f| !f.rules.trim().is_empty()).cloned().collect()
    }

    fn changed(&mut self) {
        self.check();
        self.events.push(FormatManagerEvent::Changed(self.kept()));
    }

    /// Takes the editor's text and commands, after input reached it.
    fn sync(&mut self) {
        if self.editor.take_changed() {
            let text = self.editor.text();
            if let Some(format) = self.formats.get_mut(self.selected) {
                format.rules = text;
            }
            self.changed();
        }
        if self.editor.take_commands().contains(&PICK_COLOR) {
            self.events.push(FormatManagerEvent::PickColor);
        }
    }

    /// Puts `hex` in place of the `#…` at the caret, or at the caret.
    pub fn insert_color(&mut self, hex: &str) {
        let text = self.editor.text();
        let caret = self.editor.cursor();
        let range = diver_format::hex_at(&text, caret).unwrap_or(caret..caret);
        self.editor.replace(range.clone(), hex);
        self.editor.set_selection(range.start + hex.len()..range.start + hex.len());
        self.sync();
    }

    /// The `#…` color at the caret, as `0xRRGGBB`, to start a picker on.
    pub fn color_at_caret(&self) -> Option<u32> {
        let text = self.editor.text();
        let range = diver_format::hex_at(&text, self.editor.cursor())?;
        let color = diver_format::Color::parse_hex(&text[range])?;
        Some(u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b))
    }

    fn select(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        self.selected = index.min(self.formats.len().saturating_sub(1));
        self.load_selected();
        self.focus(focus);
    }

    pub(crate) fn new_set(&mut self, focus: &mut Option<WidgetId>) {
        self.formats.push(ConditionalFormat::default());
        self.select(self.formats.len() - 1, focus);
    }

    pub(crate) fn delete_set(&mut self, focus: &mut Option<WidgetId>) {
        if self.selected < self.formats.len() {
            self.formats.remove(self.selected);
        }
        if self.formats.is_empty() {
            self.formats.push(ConditionalFormat::default());
        }
        self.select(self.selected.saturating_sub(1), focus);
        self.changed();
    }

    pub(crate) fn toggle_column(&mut self, index: usize) {
        let Some(name) = self.columns.borrow().get(index).map(|c| c.name.clone()) else { return };
        let Some(format) = self.formats.get_mut(self.selected) else { return };
        match format.columns.iter().position(|c| *c == name) {
            Some(at) => {
                format.columns.remove(at);
            }
            None => format.columns.push(name),
        }
        self.changed();
    }

    /// Adds a preset's rule after the selected set's rules.
    pub(crate) fn add_preset(&mut self, index: usize, focus: &mut Option<WidgetId>) {
        self.presets_open = false;
        let Some(preset) = PRESETS.get(index) else { return };
        let target = self.formats.get(self.selected).and_then(|f| f.columns.first().cloned());
        let rule = preset.text(&self.columns.borrow(), target.as_deref());
        let text = self.editor.text();
        let kept = text.trim_end().len();
        let insert = match text[..kept].chars().last() {
            None => rule,
            Some(';') => format!("\n{rule}"),
            Some(_) => format!(";\n{rule}"),
        };
        self.editor.replace(kept..text.len(), &insert);
        let end = kept + insert.len();
        self.editor.set_selection(end..end);
        self.focus(focus);
        self.sync();
    }

    pub fn key(&mut self, event: &KeyEvent, focus: &mut Option<WidgetId>) -> bool {
        if self.presets_open && event.is("escape") {
            self.presets_open = false;
            return true;
        }
        if *focus != Some(self.editor.id) {
            return false;
        }
        let handled = self.editor.key(event);
        self.sync();
        handled
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus != Some(self.editor.id) {
            return false;
        }
        self.editor.type_text(typed);
        self.sync();
        true
    }

    pub fn mouse_down(&mut self, hit: Option<&Hit>, pos: Point, clicks: u32, modifiers: Modifiers, focus: &mut Option<WidgetId>) {
        // A click outside the presets list closes it, and still does its own thing.
        let in_presets = matches!(
            hit,
            Some(Hit::Button(
                Button::FormatPreset(_) | Button::FormatPresets | Button::FormatPresetScroll | Button::FormatPresetsToggle
            ))
        );
        if !in_presets {
            self.presets_open = false;
        }
        let Some(hit) = hit else { return };
        match hit {
            Hit::Button(Button::FormatPresetsToggle) => self.presets_open = !self.presets_open,
            Hit::Button(Button::FormatPresetScroll) => {
                if let Some(list) = self.preset_list {
                    self.preset_scroll.press(pos.y - list.track.y, list.track.h, list.viewport, list.content);
                }
            }
            Hit::CodeEditor(id) if *id == self.editor.id => {
                self.editor.mouse_down(pos, clicks, modifiers);
                *focus = Some(self.editor.id);
            }
            Hit::Button(Button::FormatSet(index)) => self.select(*index, focus),
            Hit::Button(Button::FormatNewSet) => self.new_set(focus),
            Hit::Button(Button::FormatDeleteSet) => self.delete_set(focus),
            Hit::Button(Button::FormatColumn(index)) => {
                self.toggle_column(*index);
                self.focus(focus);
            }
            Hit::Button(Button::FormatPreset(index)) => self.add_preset(*index, focus),
            Hit::Button(Button::FormatProblem(index)) => {
                if let Some(problem) = self.problems.get(*index) {
                    self.editor.set_selection(problem.span.clone());
                }
                self.focus(focus);
            }
            _ => {}
        }
        self.sync();
    }

    pub fn mouse_move(&mut self, pos: Point, hit: Option<&Hit>) {
        let over = hit == Some(&Hit::CodeEditor(self.editor.id));
        self.editor.mouse_move(pos, over);
    }

    pub fn mouse_drag(&mut self, pos: Point) {
        if self.preset_scroll.dragging()
            && let Some(list) = self.preset_list
        {
            self.preset_scroll.drag(pos.y - list.track.y, list.track.h, list.viewport, list.content);
        } else if self.editor.is_dragging() {
            self.editor.mouse_move(pos, true);
        }
    }

    pub fn mouse_up(&mut self) {
        self.preset_scroll.release();
        self.editor.mouse_up();
    }

    pub fn wheel(&mut self, hit: &Hit, delta: Point, modifiers: Modifiers) -> bool {
        match hit {
            Hit::CodeEditor(id) if *id == self.editor.id => self.editor.wheel(delta, modifiers),
            Hit::Button(Button::FormatPreset(_) | Button::FormatPresets | Button::FormatPresetScroll) => {
                let Some(list) = self.preset_list else { return false };
                let row = list.content / preset_entries().len().max(1) as f32;
                self.preset_scroll.scroll_by(-delta.y * row, list.viewport, list.content)
            }
            _ => false,
        }
    }

    pub fn caret(&self, focus: Option<WidgetId>) -> Option<Rect> {
        (focus == Some(self.editor.id)).then(|| self.editor.caret_rect()).flatten()
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        let font = &fonts.ui;
        let small = &fonts.small;
        let line = text::line_height(font);
        let small_line = text::line_height(small);
        let heading = small.semibold();
        let mut row = Row::new(r, 16.0);
        let list = row.left(LIST_WIDTH);
        let main = row.rest();

        // Presets open as a list over the window from a button above the sets.
        let mut column = Column::new(list, 4.0);
        canvas.text(column.next(small_line), "PRESETS", &heading, colors::MUTED, Align::Left);
        let toggle = column.next(button_height(small));
        let entries = preset_entries();
        let preset_row = (small_line + 6.0).round();
        let presets_content = entries.len() as f32 * preset_row;
        let presets_top = toggle.bottom() + 4.0;
        let presets_panel = rect(
            toggle.x,
            presets_top,
            PRESETS_WIDTH.min(r.right() - toggle.x),
            (presets_content + 8.0).min(r.bottom() - presets_top).max(preset_row * 3.0),
        );
        // What's under the open list doesn't light up as the pointer crosses it.
        let mouse = canvas.mouse();
        if self.presets_open && mouse.is_some_and(|at| presets_panel.contains(at)) {
            canvas.set_mouse(None);
        }
        let open = self.presets_open;
        canvas.fill_rounded(toggle, 4.0, if open { colors::SELECTED } else if canvas.hovered(toggle) { colors::HOVER } else { colors::BACKGROUND });
        canvas.stroke_rounded(toggle, 4.0, if open { colors::SELECTED_STRONG } else { colors::INPUT_BORDER }, 1.0);
        let label = toggle.inset_xy(10.0, 0.0);
        canvas.text(label, "Add a preset…", small, colors::FOREGROUND, Align::Left);
        canvas.text(label, if open { "▴" } else { "▾" }, small, colors::MUTED, Align::Right);
        canvas.hit_cursor(toggle, Hit::Button(Button::FormatPresetsToggle), Cursor::Hand);
        column.skip(10.0);
        canvas.text(column.next(small_line), "FORMAT SETS", &heading, colors::MUTED, Align::Left);
        let set_height = (small_line * 2.0 + 10.0).round();
        let buttons_height = button_height(small) + 8.0;
        let visible = ((column.rest().h - buttons_height) / (set_height + 4.0)).floor().max(1.0) as usize;
        let first = self.selected.saturating_sub(visible.saturating_sub(1));
        for (index, format) in self.formats.iter().enumerate().skip(first).take(visible) {
            let item = column.next(set_height);
            let hit = Hit::Button(Button::FormatSet(index));
            if index == self.selected {
                canvas.fill_rounded(item, 4.0, colors::SELECTED);
            } else if canvas.hovered(item) {
                canvas.fill_rounded(item, 4.0, colors::HOVER);
            }
            let summary = format
                .rules
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with("--"))
                .unwrap_or("(no rules yet)");
            let inner = item.inset_xy(8.0, 5.0);
            canvas.text(rect(inner.x, inner.y, inner.w, small_line), summary, small, colors::FOREGROUND, Align::Left);
            let columns = if format.columns.is_empty() { "no columns".to_string() } else { format.columns.join(", ") };
            canvas.text(rect(inner.x, inner.y + small_line, inner.w, small_line), &columns, small, colors::MUTED, Align::Left);
            canvas.hit_cursor(item, hit, Cursor::Hand);
        }
        let mut buttons = Row::new(rect(list.x, list.bottom() - button_height(small), list.w, button_height(small)), 8.0);
        for (label, hit) in [("New set", Button::FormatNewSet), ("Delete set", Button::FormatDeleteSet)] {
            let b = buttons.left(button_width(label, small));
            button(canvas, b, label, small, ButtonStyle::Outline, true, Hit::Button(hit));
        }

        // The selected set: its columns, rules and problems.
        let mut column = Column::new(main, 6.0);
        canvas.text(column.next(small_line), "APPLY TO COLUMNS", &heading, colors::MUTED, Align::Left);
        let applied = self.formats.get(self.selected).map(|f| f.columns.clone()).unwrap_or_default();
        let chip_height = (small_line + 8.0).round();
        let names: Vec<String> = self.columns.borrow().iter().map(|c| c.name.clone()).collect();
        let mut x = main.x;
        let mut y = column.y;
        for (index, name) in names.iter().enumerate() {
            let width = (text::measure(name, small).0 + 30.0).min(main.w);
            if x + width > main.right() && x > main.x {
                x = main.x;
                y += chip_height + 6.0;
            }
            let chip = rect(x, y, width, chip_height);
            let on = applied.contains(name);
            canvas.fill_rounded(chip, chip_height / 2.0, if on { colors::SELECTED } else { colors::STRIPE });
            if on {
                canvas.stroke_rounded(chip, chip_height / 2.0, colors::SELECTED_STRONG, 1.0);
            }
            let mark = if on { "✓" } else { "+" };
            canvas.text(rect(chip.x + 8.0, chip.y, 12.0, chip.h), mark, small, colors::MUTED, Align::Left);
            canvas.text(rect(chip.x + 22.0, chip.y, chip.w - 26.0, chip.h), name, small, colors::FOREGROUND, Align::Left);
            canvas.hit_cursor(chip, Hit::Button(Button::FormatColumn(index)), Cursor::Hand);
            x += width + 6.0;
        }
        column.skip(y + chip_height + 10.0 - column.y);
        canvas.text(column.next(small_line), "RULES", &heading, colors::MUTED, Align::Left);
        let problem_rows = self.problems.len().clamp(1, 4) as f32;
        let below = problem_rows * (small_line + 4.0) + small_line + 18.0;
        let editor_box = column.next((column.rest().h - below).max(line * 4.0));
        canvas.stroke_rounded(editor_box, 4.0, if focus == Some(self.editor.id) { colors::FOCUS } else { colors::INPUT_BORDER }, 1.0);
        let id = self.editor.id;
        self.editor.paint(canvas, editor_box.inset(1.0), &fonts.mono, focus == Some(id), Hit::CodeEditor(id));
        if self.problems.is_empty() {
            let rules = self.formats.get(self.selected).map(|f| f.rules.trim()).unwrap_or_default();
            let message = if rules.is_empty() {
                "No rules yet: type one, or add a preset.".to_string()
            } else if applied.is_empty() {
                format!("No problems · pick the columns these {} rules apply to", self.rules_running)
            } else {
                format!("No problems · {} rule{} running", self.rules_running, if self.rules_running == 1 { "" } else { "s" })
            };
            canvas.text(column.next(small_line + 4.0), &message, small, colors::MUTED, Align::Left);
        } else {
            let rules = self.editor.text();
            for (index, problem) in self.problems.iter().take(4).enumerate() {
                let item = column.next(small_line + 4.0);
                if canvas.hovered(item) {
                    canvas.fill_rounded(item, 3.0, colors::HOVER);
                }
                let before = &rules[..problem.span.start.min(rules.len())];
                let line_number = before.matches('\n').count() + 1;
                let column_number = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
                let mut parts = Row::new(item.inset_xy(6.0, 0.0), 8.0);
                let (label, color) = match problem.severity {
                    Severity::Error => ("error", colors::DANGER),
                    Severity::Warning => ("warning", colors::WARNING),
                };
                canvas.text(parts.left(54.0), label, &heading, color, Align::Left);
                canvas.text(parts.left(76.0), &format!("Ln {line_number}, Col {column_number}"), small, colors::MUTED, Align::Left);
                canvas.text(parts.rest(), &problem.message, small, colors::FOREGROUND, Align::Left);
                canvas.hit_cursor(item, Hit::Button(Button::FormatProblem(index)), Cursor::Hand);
            }
        }
        canvas.text(
            column.next(small_line),
            "c is the cell · \"name\" another column · 'text' · ; separates rules · # picks a color · Ctrl+Space completes · Esc closes",
            small,
            colors::MUTED,
            Align::Left,
        );

        canvas.set_mouse(mouse);
        if self.presets_open {
            self.paint_presets(canvas, presets_panel, &entries, preset_row, &applied, small, &heading);
        } else {
            self.preset_list = None;
        }
    }

    /// The open presets list in `panel`, grouped, scrolling when it's taller.
    #[allow(clippy::too_many_arguments)]
    fn paint_presets(
        &mut self,
        canvas: &mut Canvas<Hit>,
        panel: Rect,
        entries: &[Entry],
        row_height: f32,
        applied: &[String],
        small: &crate::ui::platform::Font,
        heading: &crate::ui::platform::Font,
    ) {
        canvas.shadow(panel, 6.0);
        canvas.fill_rounded(panel, 6.0, colors::POPOVER);
        canvas.stroke_rounded(panel, 6.0, colors::BORDER, 1.0);
        canvas.hit(panel, Hit::Button(Button::FormatPresets));
        let inner = panel.inset(4.0);
        let content = entries.len() as f32 * row_height;
        self.preset_scroll.clamp(inner.h, content);
        let bar = scroll::has_bar(inner.h, content);
        let rows = if bar { inner.with_w(inner.w - scroll::BAR_WIDTH) } else { inner };
        let target = applied.first().map(String::as_str);
        let offset = self.preset_scroll.offset;
        let first = (offset / row_height).floor() as usize;
        canvas.push_clip(inner);
        for (n, entry) in entries.iter().enumerate().skip(first) {
            let item = rect(rows.x, rows.y + n as f32 * row_height - offset, rows.w, row_height);
            if item.y >= inner.bottom() {
                break;
            }
            match entry {
                Entry::Group(name) => {
                    canvas.text(item.inset_xy(6.0, 0.0), name, heading, colors::SELECTED_STRONG, Align::Left);
                }
                Entry::Preset(index) => {
                    let shown = item.intersect(&inner);
                    if canvas.hovered(shown) {
                        canvas.fill_rounded(item, 3.0, colors::HOVER);
                    }
                    let preset = &PRESETS[*index];
                    canvas.text(item.inset_xy(16.0, 0.0), preset.name, small, colors::FOREGROUND, Align::Left);
                    let tip = format!("{}\n{}", preset.description, preset.text(&self.columns.borrow(), target));
                    canvas.hit_with(shown, Hit::Button(Button::FormatPreset(*index)), Cursor::Hand, Some(&tip));
                }
            }
        }
        canvas.pop_clip();
        let track = rect(inner.right() - scroll::BAR_WIDTH, inner.y, scroll::BAR_WIDTH, inner.h);
        if bar {
            scroll::vertical(canvas, track, inner.h, content, &self.preset_scroll, Hit::Button(Button::FormatPresetScroll));
        }
        self.preset_list = Some(PresetList { track, viewport: inner.h, content });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diver_format::{Column, ValueKind};

    fn manager(formats: Vec<ConditionalFormat>, active: Option<&str>) -> FormatManager {
        let columns = vec![Column::new("sales", ValueKind::Number), Column::new("region", ValueKind::Text)];
        FormatManager::new(None, formats, columns, active.map(Into::into))
    }

    fn changes(manager: &mut FormatManager) -> Vec<Vec<ConditionalFormat>> {
        manager
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                FormatManagerEvent::Changed(formats) => Some(formats),
                FormatManagerEvent::PickColor => None,
            })
            .collect()
    }

    #[test]
    fn opens_on_the_active_columns_set_or_a_new_one_for_it() {
        let saved = vec![ConditionalFormat { rules: "bold".into(), columns: vec!["region".into()] }];
        let opened = manager(saved.clone(), Some("region"));
        assert_eq!((opened.formats.len(), opened.selected, opened.editor.text()), (1, 0, "bold".to_string()));
        let fresh = manager(saved, Some("sales"));
        assert_eq!(fresh.formats.len(), 2);
        assert_eq!(fresh.formats[1].columns, ["sales"]);
        assert_eq!(fresh.editor.text(), "");
    }

    #[test]
    fn presets_columns_and_typing_report_the_kept_sets() {
        let mut manager = manager(Vec::new(), Some("sales"));
        let mut focus = None;
        let duplicates = PRESETS.iter().position(|p| p.name == "Duplicate values").unwrap();
        manager.add_preset(duplicates, &mut focus);
        manager.add_preset(duplicates, &mut focus);
        assert_eq!(
            manager.editor.text(),
            "if duplicate(c) then bg: #ffc7ce, text: #9c0006;\nif duplicate(c) then bg: #ffc7ce, text: #9c0006"
        );
        manager.toggle_column(1);
        let changed = changes(&mut manager);
        let last = changed.last().unwrap();
        assert_eq!(last[0].columns, ["sales", "region"]);
        assert!(manager.problems.is_empty());
        manager.type_text(" zzz", Some(manager.editor.id));
        assert_eq!(manager.problems.len(), 1);
        // A blank set isn't kept.
        manager.new_set(&mut focus);
        manager.toggle_column(0);
        assert_eq!(changes(&mut manager).last().unwrap().len(), 1);
    }

    #[test]
    fn picked_colors_replace_the_hash_at_the_caret() {
        let mut manager = manager(Vec::new(), Some("sales"));
        manager.editor.set_text("bg: #f");
        manager.editor.set_selection(6..6);
        assert_eq!(manager.color_at_caret(), None);
        manager.insert_color("#ff8800");
        assert_eq!(manager.editor.text(), "bg: #ff8800");
        assert_eq!(manager.color_at_caret(), Some(0xff8800));
    }
}
