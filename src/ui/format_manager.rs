//! The conditional formatting manager: the view's format sets, each rules in
//! the formatting language and the columns they apply to, with presets.
//! It fills a window of its own beside the grid's, and every change applies
//! at once, so the grid shows it while typing.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use diver_format::editor::FormatLanguage;
use diver_format::{PRESETS, Severity};
use windows::Win32::Foundation::HWND;

use crate::domain::documents::ConditionalFormat;
use crate::ui::hit::{Button, Hit};
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Align, Canvas, Cursor, Point, Rect, point, rect, text};
use crate::ui::widgets::color_picker::{self, ColorPicker, Swatches};
use crate::ui::widgets::editor::Editor;
use crate::ui::widgets::hosted_editor::{HostedEditor, MarkKind};
use crate::ui::widgets::scroll::{self, Scroll};
use crate::ui::widgets::{ButtonStyle, Column, Fonts, Row, WidgetId, button, button_height, button_width, colors};

const LIST_WIDTH: f32 = 240.0;
/// The colors panel on the right.
const COLORS_WIDTH: f32 = 252.0;
/// The color the panel starts on before the rules have one.
const START_COLOR: u32 = 0x638ec6;
/// At most this many of the rules' own colors are offered.
const RULE_SWATCHES: usize = 16;
/// The presets drop-down, which may be wider than the list it opens from.
const PRESETS_WIDTH: f32 = 300.0;

pub enum FormatManagerEvent {
    Changed(Vec<ConditionalFormat>),
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
    /// The colors panel: shown, its picker, and the picker's hex input.
    pub(crate) colors_open: bool,
    pub(crate) picker: ColorPicker,
    pub(crate) hex: Editor,
    /// The selected set's column pattern.
    pub(crate) selector: Editor,
    /// What the selected set's pattern picks from which names, so painting
    /// doesn't match it again until one of them changes.
    matched: RefCell<Option<SelectorMatch>>,
    events: Vec<FormatManagerEvent>,
}

/// A pattern matched against column names: the indices it picks, or what
/// is wrong with it.
struct SelectorMatch {
    pattern: String,
    names: Vec<String>,
    picked: Result<Vec<usize>, String>,
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
        let names: Vec<&str> = columns.iter().map(|column| column.name.as_str()).collect();
        let applies = |format: &ConditionalFormat, name: &String| {
            format.targets(&names).into_iter().any(|index| names[index] == name.as_str())
        };
        let selected = match active.as_ref().and_then(|name| formats.iter().position(|f| applies(f, name))) {
            Some(index) => index,
            None if formats.is_empty() || active.is_some() => {
                formats.push(ConditionalFormat { rules: String::new(), columns: active.into_iter().collect(), ..Default::default() });
                formats.len() - 1
            }
            None => 0,
        };
        let columns = Rc::new(RefCell::new(columns));
        let editor = HostedEditor::rules(host, FormatLanguage::new(columns.clone()));
        let picker = ColorPicker::new(START_COLOR);
        let mut hex = Editor::single_line();
        hex.set_text(picker.hex());
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
            colors_open: false,
            picker,
            hex,
            selector: Editor::single_line().with_placeholder(r#"Pick by name: 2021..2025, "Region", /^q\d/, -total"#),
            matched: RefCell::new(None),
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
        id == self.editor.id || id == self.hex.id || id == self.selector.id
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
        let pattern = self.formats.get(self.selected).map(|f| f.selector.clone()).unwrap_or_default();
        if self.selector.text() != pattern {
            self.selector.set_text(pattern);
        }
        self.check();
    }

    /// The columns (by index) the selected set's pattern picks, or what is
    /// wrong with the pattern.
    fn selector_picks(&self) -> Result<Vec<usize>, String> {
        let pattern = self.formats.get(self.selected).map(|f| f.selector.as_str()).unwrap_or_default();
        let columns = self.columns.borrow();
        let mut matched = self.matched.borrow_mut();
        let current = matched.as_ref().is_some_and(|m| {
            m.pattern == pattern && m.names.len() == columns.len() && m.names.iter().zip(columns.iter()).all(|(a, b)| *a == b.name)
        });
        if !current {
            let names: Vec<String> = columns.iter().map(|column| column.name.clone()).collect();
            let picked = if pattern.trim().is_empty() {
                Ok(Vec::new())
            } else {
                crate::domain::column_selector::ColumnSelector::parse(pattern).map(|selector| selector.select(&names))
            };
            *matched = Some(SelectorMatch { pattern: pattern.to_string(), names, picked });
        }
        matched.as_ref().expect("matched above").picked.clone()
    }

    /// Typing in the pattern input applies it at once.
    fn selector_changed(&mut self) {
        let pattern = self.selector.text().to_string();
        if let Some(format) = self.formats.get_mut(self.selected)
            && format.selector != pattern
        {
            format.selector = pattern;
            self.changed();
        }
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
    /// Whether the manager shows `formats` over `columns` already, so
    /// [`Self::sync_scope`] would change nothing.
    pub fn shows(&self, formats: &[ConditionalFormat], columns: &[diver_format::Column]) -> bool {
        *self.columns.borrow() == columns
            && self.formats.iter().filter(|f| !f.rules.trim().is_empty()).eq(formats.iter())
    }

    fn kept(&self) -> Vec<ConditionalFormat> {
        self.formats.iter().filter(|f| !f.rules.trim().is_empty()).cloned().collect()
    }

    fn changed(&mut self) {
        self.check();
        self.events.push(FormatManagerEvent::Changed(self.kept()));
    }

    /// Takes the editor's text, after input reached it.
    fn sync(&mut self) {
        if self.editor.take_changed() {
            let text = self.editor.text();
            if let Some(format) = self.formats.get_mut(self.selected) {
                format.rules = text;
            }
            self.changed();
        }
    }

    /// The `#…` color the caret is in or right after, and its `0xRRGGBB`.
    pub fn color_at_caret(&self) -> Option<(Range<usize>, u32)> {
        let text = self.editor.text();
        let range = diver_format::hex_at(&text, self.editor.cursor())?;
        let rgb = parse_rgb(&text[range.clone()])?;
        Some((range, rgb))
    }

    /// Puts the picker's color in place of the `#…` at the caret, or at the
    /// caret, with a space after a word.
    pub fn insert_color(&mut self, focus: &mut Option<WidgetId>) {
        let hex = self.picker.hex();
        let text = self.editor.text();
        let caret = self.editor.cursor();
        let (range, insert) = match diver_format::hex_at(&text, caret) {
            Some(range) => (range, hex),
            None if text[..caret].chars().last().is_some_and(|c| c.is_alphanumeric() || c == ':') => {
                (caret..caret, format!(" {hex}"))
            }
            None => (caret..caret, hex),
        };
        self.editor.replace(range.clone(), &insert);
        let end = range.start + insert.len();
        self.editor.set_selection(end..end);
        self.focus(focus);
        self.sync();
    }

    /// The picker's color changed: the `#…` at the caret follows it, so
    /// the grid shows it at once, and the hex input shows it unless that's
    /// where it came from.
    fn picked(&mut self, from_hex: bool) {
        if !from_hex {
            self.hex.set_text(self.picker.hex());
        }
        if let Some((range, _)) = self.color_at_caret() {
            let hex = self.picker.hex();
            self.editor.adjust(range.clone(), &hex);
            let end = range.start + hex.len();
            self.editor.set_selection(end..end);
            self.sync();
        }
    }

    /// The picker shows the color at the caret while nothing is picking.
    fn follow_caret(&mut self, focus: Option<WidgetId>) {
        if self.picker.is_dragging() || focus == Some(self.hex.id) {
            return;
        }
        if let Some((_, rgb)) = self.color_at_caret() {
            self.picker.set_rgb(rgb);
        }
        let hex = self.picker.hex();
        if self.hex.text() != hex {
            self.hex.set_text(hex);
        }
    }

    pub fn toggle_colors(&mut self) {
        self.colors_open = !self.colors_open;
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
        if *focus == Some(self.selector.id) {
            // Escape and Enter go back to the rules.
            if event.is("escape") {
                self.focus(focus);
                return true;
            }
            let outcome = self.selector.key(event);
            if outcome.changed {
                self.selector_changed();
            }
            if outcome.submit {
                self.focus(focus);
            }
            return outcome.handled || outcome.submit;
        }
        if *focus == Some(self.hex.id) {
            // Escape goes back to the rules rather than closing the window.
            if event.is("escape") {
                self.focus(focus);
                return true;
            }
            let outcome = self.hex.key(event);
            if outcome.changed {
                self.hex_changed();
            }
            if outcome.submit {
                if self.color_at_caret().is_some() {
                    self.focus(focus);
                } else {
                    self.insert_color(focus);
                }
            }
            return outcome.handled || outcome.submit;
        }
        if *focus != Some(self.editor.id) {
            return false;
        }
        let handled = self.editor.key(event);
        self.sync();
        handled
    }

    pub fn type_text(&mut self, typed: &str, focus: Option<WidgetId>) -> bool {
        if focus == Some(self.selector.id) {
            self.selector.type_text(typed);
            self.selector_changed();
            return true;
        }
        if focus == Some(self.hex.id) {
            self.hex.type_text(typed);
            self.hex_changed();
            return true;
        }
        if focus != Some(self.editor.id) {
            return false;
        }
        self.editor.type_text(typed);
        self.sync();
        true
    }

    /// Typing in the hex input: a whole color moves the picker.
    fn hex_changed(&mut self) {
        let typed = self.hex.text().trim();
        let digits = typed.trim_start_matches('#').len();
        if let Some(rgb) = parse_rgb(typed).filter(|_| digits == 3 || digits == 6)
            && self.picker.set_rgb(rgb)
        {
            self.picked(true);
        }
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
            Hit::Button(Button::FormatColorsToggle) => self.toggle_colors(),
            Hit::Button(Button::FormatColorInsert) => self.insert_color(focus),
            Hit::ColorPicker(id, part) if *id == self.picker.id => {
                // One gesture, one undo step.
                self.editor.seal_undo();
                if self.picker.mouse_down(*part, pos) {
                    self.picked(false);
                }
            }
            Hit::Editor(id, part) if *id == self.selector.id => {
                if *focus != Some(self.selector.id) {
                    self.editor.seal_undo();
                }
                self.selector.mouse_down(*part, pos, clicks, modifiers.shift);
                *focus = Some(self.selector.id);
            }
            Hit::Editor(id, part) if *id == self.hex.id => {
                if *focus != Some(self.hex.id) {
                    self.editor.seal_undo();
                }
                self.hex.mouse_down(*part, pos, clicks, modifiers.shift);
                *focus = Some(self.hex.id);
            }
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
        if self.picker.is_dragging() {
            if self.picker.mouse_drag(pos) {
                self.picked(false);
            }
        } else if self.hex.is_dragging() {
            self.hex.mouse_drag(pos);
        } else if self.selector.is_dragging() {
            self.selector.mouse_drag(pos);
        } else if self.preset_scroll.dragging()
            && let Some(list) = self.preset_list
        {
            self.preset_scroll.drag(pos.y - list.track.y, list.track.h, list.viewport, list.content);
        } else if self.editor.is_dragging() {
            self.editor.mouse_move(pos, true);
        }
    }

    pub fn mouse_up(&mut self) {
        self.preset_scroll.release();
        self.picker.mouse_up();
        self.hex.mouse_up();
        self.selector.mouse_up();
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
        let panel = self.colors_open.then(|| row.right(COLORS_WIDTH));
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
            let mut parts = format.columns.clone();
            if !format.selector.trim().is_empty() {
                parts.push(format!("matching {}", format.selector.trim()));
            }
            let columns = if parts.is_empty() { "no columns".to_string() } else { parts.join(", ") };
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
        let headings = column.next(small_line);
        canvas.text(headings, "APPLY TO COLUMNS", &heading, colors::MUTED, Align::Left);
        if panel.is_none() {
            let label = "Colors ◂";
            let width = button_width(label, small);
            let toggle = rect(main.right() - width, headings.y - 4.0, width, button_height(small));
            button(canvas, toggle, label, small, ButtonStyle::Ghost, true, Hit::Button(Button::FormatColorsToggle));
        }
        let applied = self.formats.get(self.selected).map(|f| f.columns.clone()).unwrap_or_default();
        // The pattern input, and how many columns it picks or what's wrong with it.
        let picks = self.selector_picks();
        let id = self.selector.id;
        let input = column.next(Editor::line_box_height(small));
        self.selector.paint(canvas, input, small, focus == Some(id), |part| Hit::Editor(id, part));
        let (status, status_color) = match &picks {
            Err(problem) => (problem.clone(), colors::DANGER),
            Ok(_) if self.selector.text().trim().is_empty() => (
                r#"or click columns · words must all match, "exact" names, a..b runs in grid order, /regex/, -leaves out, | or"#.to_string(),
                colors::MUTED,
            ),
            Ok(picked) => (
                format!("Matches {} column{} · marked ● below", picked.len(), if picked.len() == 1 { "" } else { "s" }),
                colors::MUTED,
            ),
        };
        canvas.text(column.next(small_line), &status, small, status_color, Align::Left);
        let picked = picks.unwrap_or_default();
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
            let matched = picked.contains(&index);
            canvas.fill_rounded(chip, chip_height / 2.0, if on || matched { colors::SELECTED } else { colors::STRIPE });
            if on {
                canvas.stroke_rounded(chip, chip_height / 2.0, colors::SELECTED_STRONG, 1.0);
            }
            let mark = if on { "✓" } else if matched { "●" } else { "+" };
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
            } else if applied.is_empty() && picked.is_empty() {
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
            "c is the cell · \"name\" another column · 'text' · ; separates rules · Ctrl+Space completes · Esc closes",
            small,
            colors::MUTED,
            Align::Left,
        );

        if let Some(panel) = panel {
            self.paint_colors(canvas, panel, fonts, focus);
        }

        canvas.set_mouse(mouse);
        if self.presets_open {
            self.paint_presets(canvas, presets_panel, &entries, preset_row, &applied, small, &heading);
        } else {
            self.preset_list = None;
        }
        let id = self.editor.id;
        self.editor.paint_popups(canvas, Hit::CodeEditor(id));
    }

    /// The colors panel: a picker for the `#…` at the caret, or for a new
    /// color to insert there.
    fn paint_colors(&mut self, canvas: &mut Canvas<Hit>, panel: Rect, fonts: &Fonts, focus: Option<WidgetId>) {
        self.follow_caret(focus);
        let small = &fonts.small;
        let heading = small.semibold();
        let small_line = text::line_height(small);
        canvas.line(point(panel.x - 8.0, panel.y), point(panel.x - 8.0, panel.bottom()), colors::BORDER, 1.0);
        let mut column = Column::new(panel, 8.0);
        let top = column.next(small_line);
        canvas.text(top, "COLORS", &heading, colors::MUTED, Align::Left);
        let label = "Hide ▸";
        let width = button_width(label, small);
        let hide = rect(top.right() - width, top.y - 4.0, width, button_height(small));
        button(canvas, hide, label, small, ButtonStyle::Ghost, true, Hit::Button(Button::FormatColorsToggle));
        let at_caret = self.color_at_caret();
        let status = if at_caret.is_some() { "Changes the color at the caret." } else { "Insert puts this color at the caret." };
        canvas.text(column.next(small_line), status, small, colors::MUTED, Align::Left);

        // The color, its hex, and Insert.
        let input_height = Editor::line_box_height(small);
        let mut items = Row::new(column.next(input_height), 8.0);
        let swatch = items.left(input_height);
        canvas.fill_rounded(swatch, 4.0, color_picker::rgb_color(self.picker.rgb()));
        canvas.stroke_rounded(swatch, 4.0, colors::INPUT_BORDER, 1.0);
        let insert = "Insert";
        let insert_box = items.right(button_width(insert, small));
        let style = if at_caret.is_some() { ButtonStyle::Outline } else { ButtonStyle::Primary };
        button(canvas, insert_box, insert, small, style, true, Hit::Button(Button::FormatColorInsert));
        let id = self.hex.id;
        self.hex.paint(canvas, items.rest(), small, focus == Some(id), |part| Hit::Editor(id, part));
        column.skip(4.0);

        let rules = self.formats.get(self.selected).map(|f| f.rules.as_str()).unwrap_or_default();
        let mut used: Vec<(String, u32)> = Vec::new();
        for (_, color) in diver_format::help::color_spans(rules) {
            let rgb = rgb_of(color);
            if color.a > 0 && used.len() < RULE_SWATCHES && !used.iter().any(|(_, seen)| *seen == rgb) {
                used.push((format!("#{rgb:06x}"), rgb));
            }
        }
        let palette: Vec<(String, u32)> = diver_format::colors::PALETTE
            .iter()
            .map(|(name, color)| (format!("{name} · {}", color.to_hex()), rgb_of(*color)))
            .collect();
        let groups = [Swatches { title: "IN THESE RULES", colors: &used }, Swatches { title: "PALETTE", colors: &palette }];
        let height = ColorPicker::height(panel.w, &groups, &heading);
        let area = column.next(height.min(column.rest().h));
        canvas.push_clip(rect(area.x - 8.0, area.y - 8.0, area.w + 16.0, area.h + 8.0));
        self.picker.paint(canvas, area, &groups, &heading);
        canvas.pop_clip();
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

fn rgb_of(color: diver_format::Color) -> u32 {
    u32::from(color.r) << 16 | u32::from(color.g) << 8 | u32::from(color.b)
}

/// `#rgb` or `#rrggbb` (with or without `#`) as `0xRRGGBB`.
fn parse_rgb(text: &str) -> Option<u32> {
    diver_format::Color::parse_hex(text).map(rgb_of)
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
            .map(|event| match event {
                FormatManagerEvent::Changed(formats) => formats,
            })
            .collect()
    }

    #[test]
    fn opens_on_the_active_columns_set_or_a_new_one_for_it() {
        let saved = vec![ConditionalFormat { rules: "bold".into(), columns: vec!["region".into()], ..Default::default() }];
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
    fn a_pattern_picks_columns_and_says_what_is_wrong_with_it() {
        let saved = vec![ConditionalFormat { rules: "bold".into(), selector: "reg".into(), ..Default::default() }];
        // The set whose pattern picks the grid's column opens first.
        let mut manager = manager(saved, Some("region"));
        assert_eq!((manager.formats.len(), manager.selector.text()), (1, "reg"));
        assert_eq!(manager.selector_picks(), Ok(vec![1]));
        let mut focus = Some(manager.selector.id);
        manager.selector.select_all();
        manager.type_text("/(", focus);
        assert!(manager.selector_picks().unwrap_err().contains("isn't closed"));
        manager.selector.select_all();
        manager.type_text("/^s/ | \"region\"", focus);
        assert_eq!(manager.selector_picks(), Ok(vec![0, 1]));
        let last = changes(&mut manager).pop().unwrap();
        assert_eq!(last[0].selector, "/^s/ | \"region\"");
        // Columns changing match the pattern again.
        manager.sync_scope(&last, vec![Column::new("salary", ValueKind::Number), Column::new("sales", ValueKind::Number)]);
        assert_eq!(manager.selector_picks(), Ok(vec![0, 1]));
        assert!(manager.formats[0].selector.contains("region"));
        // Enter goes back to the rules.
        manager.key(&KeyEvent::new("enter", Modifiers::NONE, None), &mut focus);
        assert_eq!(focus, Some(manager.editor.id));
    }

    #[test]
    fn insert_replaces_the_hash_at_the_caret_or_adds_a_color_after_a_word() {
        let mut manager = manager(Vec::new(), Some("sales"));
        let mut focus = None;
        manager.picker.set_rgb(0xff8800);
        manager.editor.set_text("bg: #f");
        manager.editor.set_selection(6..6);
        assert_eq!(manager.color_at_caret(), None);
        manager.insert_color(&mut focus);
        assert_eq!(manager.editor.text(), "bg: #ff8800");
        assert_eq!(manager.color_at_caret(), Some((4..11, 0xff8800)));
        assert_eq!(focus, Some(manager.editor.id));
        manager.editor.set_text("if c > 1 then text");
        manager.editor.set_selection(18..18);
        manager.insert_color(&mut focus);
        assert_eq!(manager.editor.text(), "if c > 1 then text #ff8800");
    }

    #[test]
    fn the_picker_follows_the_caret_and_edits_its_color_as_one_undo_step() {
        let mut manager = manager(Vec::new(), Some("sales"));
        manager.editor.set_text("if c > 1 then bg: #ffc7ce, text: #9c0006");
        manager.editor.set_selection(22..22);
        manager.follow_caret(None);
        assert_eq!((manager.picker.rgb(), manager.hex.text()), (0xffc7ce, "#ffc7ce"));
        // Dragging along the hue bar rewrites the color under the caret each step.
        let hue_bar = rect(0.0, 130.0, 240.0, 14.0);
        manager.picker.lay_out(rect(0.0, 0.0, 240.0, 120.0), hue_bar);
        manager.editor.seal_undo();
        manager.picker.mouse_down(color_picker::Part::Hue, crate::ui::platform::point(0.0, hue_bar.y));
        manager.picked(false);
        for step in 1..=3 {
            let x = hue_bar.x + hue_bar.w * step as f32 / 6.0;
            assert!(manager.picker.mouse_drag(crate::ui::platform::point(x, hue_bar.y)));
            manager.picked(false);
        }
        manager.mouse_up();
        let after = manager.editor.text();
        assert!(after.starts_with("if c > 1 then bg: #") && after.ends_with(", text: #9c0006"), "{after}");
        assert_ne!(after, "if c > 1 then bg: #ffc7ce, text: #9c0006");
        assert_eq!(manager.formats[manager.selected].rules, after);
        assert_eq!(manager.hex.text(), manager.picker.hex());
        manager.editor.run(code_editor::Action::Undo);
        assert_eq!(manager.editor.text(), "if c > 1 then bg: #ffc7ce, text: #9c0006");

        // Typing a whole hex moves the picker and the color at the caret.
        manager.editor.set_selection(37..37);
        let mut focus = Some(manager.hex.id);
        manager.hex.select_all();
        manager.type_text("#123", focus);
        assert_eq!(manager.editor.text(), "if c > 1 then bg: #ffc7ce, text: #112233");
        // Enter goes back to the rules; Escape would too.
        manager.key(&KeyEvent::new("enter", Modifiers::NONE, None), &mut focus);
        assert_eq!(focus, Some(manager.editor.id));
    }
}
