//! The Monaco-style code editor (`code_editor`), drawn into the window's own
//! frame. DataDiver keeps its keymap, focus, and hit testing, and passes the
//! editor the keys, text, and mouse input that fall to it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use code_editor::{
    Action, Chord, CompletionProvider, DecorationId, DecorationOptions, Editor, Event, LanguageService, Mods, Options,
    Theme,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};

use super::WidgetId;
use crate::infrastructure::json_store::EditorSettings;
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Canvas, Cursor, Font, Point, Rect, rect};

/// Kinds of marks the host puts on the text; setting a kind replaces its
/// earlier marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkKind {
    /// Where the last run failed.
    RunError,
    /// The statement Ctrl+Enter runs.
    Statement,
    /// Row counts after CTEs.
    Counts,
    /// The query a helper command acts on, shown for a moment.
    Flash,
    /// Colors written in formatting rules, shown in their own color.
    Colors,
}

pub struct HostedEditor {
    pub id: WidgetId,
    editor: Editor,
    marks: HashMap<MarkKind, Vec<DecorationId>>,
    /// Set by the editor when its text changes; read with [`Self::take_changed`].
    changed: Rc<Cell<bool>>,
    /// Host commands the editor sent, such as a completion's; read with
    /// [`Self::take_commands`].
    commands: Rc<RefCell<Vec<u32>>>,
    /// Where the editor was last painted, in window DIPs.
    bounds: Rect,
    font: Option<Font>,
    settings: EditorSettings,
    hovered: bool,
    dragging: bool,
}

impl HostedEditor {
    /// A SQL editor painted into `host`'s frames, with the host's completions
    /// and language features. Without a window (headless tests) it still
    /// edits; it just never asks for a repaint.
    pub fn sql(
        host: Option<HWND>,
        completions: impl CompletionProvider + 'static,
        language: impl LanguageService + 'static,
    ) -> Self {
        let options = Options {
            // Lines stay lines: the panel is narrow and SQL is easier to read unwrapped.
            word_wrap: false,
            ..Options::default()
        };
        Self::new(host, options, completions, language)
    }

    /// An editor for conditional formatting rules. It borrows the SQL grammar
    /// for `--` comments and auto-closed quotes and brackets; the rules' own
    /// service colors the text.
    pub fn rules(host: Option<HWND>, language: diver_format::editor::FormatLanguage) -> Self {
        let options = Options {
            word_wrap: true,
            sticky_scroll: false,
            folding: false,
            occurrences_highlight: false,
            ..Options::default()
        };
        Self::new(host, options, language.clone(), language)
    }

    fn new(
        host: Option<HWND>,
        options: Options,
        completions: impl CompletionProvider + 'static,
        language: impl LanguageService + 'static,
    ) -> Self {
        let editor = Editor::create_hosted(host.unwrap_or_default(), options, Theme::Light).expect("create the code editor");
        editor.set_language("sql");
        editor.set_completion_provider(completions);
        editor.set_language_service(Some(Rc::new(language)));
        let changed = Rc::new(Cell::new(false));
        let commands = Rc::new(RefCell::new(Vec::new()));
        let (flag, sent) = (changed.clone(), commands.clone());
        editor.on_event(move |_, event| match event {
            Event::ContentChanged { .. } => flag.set(true),
            Event::Command(id) => sent.borrow_mut().push(*id),
            _ => {}
        });
        Self {
            id: WidgetId::new(),
            editor,
            marks: HashMap::new(),
            changed,
            commands,
            bounds: Rect::default(),
            font: None,
            settings: EditorSettings::default(),
            hovered: false,
            dragging: false,
        }
    }

    pub fn text(&self) -> String {
        self.editor.text()
    }

    /// Replaces the whole text, as loading a file would.
    pub fn set_text(&mut self, text: impl AsRef<str>) {
        self.editor.set_text(text.as_ref());
        self.changed.set(false);
    }

    pub fn selected_range(&self) -> Range<usize> {
        self.editor.selection_range()
    }

    pub fn selected_text(&self) -> String {
        let range = self.selected_range();
        self.text().get(range).unwrap_or_default().to_owned()
    }

    pub fn cursor(&self) -> usize {
        self.editor.caret_offset()
    }

    /// Selects `range`, with the caret at its end.
    pub fn set_selection(&mut self, range: Range<usize>) {
        self.editor.select_range(range.start, range.end);
    }

    /// Replaces `range` as one undo step, as an edit by the user.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        self.editor.replace_range(range, text);
    }

    /// Replaces `range` as one step of a continuing change, such as dragging
    /// a color picker; the steps since [`Self::seal_undo`] undo together.
    pub fn adjust(&mut self, range: Range<usize>, text: &str) {
        self.editor.adjust_range(range, text);
    }

    /// The next edit starts an undo step of its own.
    pub fn seal_undo(&mut self) {
        self.editor.seal_undo();
    }

    /// Applies a rewrite as one undo step and selects its tab stops.
    pub fn apply_rewrite(&mut self, rewrite: &diver_sql::Rewrite) {
        let old = self.text();
        let (range, text) = rewrite.edit(&old);
        let span = |r: diver_sql::TextRange| usize::from(r.start())..usize::from(r.end());
        let stops = rewrite.stops.iter().map(|group| group.iter().map(|r| span(*r)).collect()).collect();
        self.editor.replace_with_stops(span(range), &text, stops);
    }

    /// Counts edits, for noticing that the text changed.
    pub fn version(&self) -> u64 {
        self.editor.version()
    }

    /// Whether linked tab stops (a new name, a rename) are being typed.
    pub fn in_tab_stops(&self) -> bool {
        self.editor.in_tab_stops()
    }

    /// The tooltip showing now, such as Go to Definition's message.
    pub fn tooltip(&self) -> Option<String> {
        self.editor.tooltip()
    }

    /// One level of indentation, as the user's settings indent.
    pub fn indent_unit(&self) -> String {
        if self.settings.insert_spaces {
            " ".repeat(usize::from(self.settings.tab_size.clamp(1, 16)))
        } else {
            "\t".into()
        }
    }

    /// Replaces the marks of `kind` with `marks`, byte ranges of the text.
    pub fn set_marks(&mut self, kind: MarkKind, marks: Vec<(Range<usize>, DecorationOptions)>) {
        let old = self.marks.remove(&kind).unwrap_or_default();
        let ids = self.editor.decorate(&old, marks);
        self.marks.insert(kind, ids);
    }

    /// The marks of `kind` where they are now, as byte ranges with their hover text.
    pub fn marks(&self, kind: MarkKind) -> Vec<(Range<usize>, Option<String>, Option<String>)> {
        let text = self.text();
        let offset = |p: code_editor::Pos| {
            text.split_inclusive('\n').take(p.line).map(str::len).sum::<usize>() + p.col
        };
        self.marks
            .get(&kind)
            .into_iter()
            .flatten()
            .filter_map(|id| self.editor.decoration(*id))
            .map(|(start, end, options)| (offset(start)..offset(end), options.hover, options.after))
            .collect()
    }

    /// Whether the text changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.replace(false)
    }

    /// Host commands sent since the last call.
    pub fn take_commands(&self) -> Vec<u32> {
        std::mem::take(&mut self.commands.borrow_mut())
    }

    /// Colors and problems again from the language service, after the catalog
    /// it reads changed.
    pub fn refresh_language(&mut self) {
        self.editor.refresh_language_service();
    }

    /// The underlined problems as (start, end, message), checked now.
    pub fn diagnostics(&self) -> Vec<(usize, usize, String)> {
        self.editor.refresh_diagnostics();
        self.editor.diagnostics()
    }

    /// The hover text at a window position, as resting the mouse there shows.
    pub fn hover_text(&self, pos: Point) -> Option<String> {
        let (x, y) = self.local(pos);
        self.editor.hover_now(x, y)
    }

    /// The signature popup's label and its highlighted argument.
    pub fn signature(&self) -> Option<(String, Option<String>)> {
        self.editor.signature().map(|s| {
            let active = s.active.and_then(|r| s.label.get(r)).map(str::to_string);
            (s.label, active)
        })
    }

    /// Runs one of the editor's actions, as its key would.
    pub fn run(&mut self, action: Action) {
        self.editor.run(action);
    }

    pub fn trigger_suggest(&mut self) {
        self.editor.run(Action::TriggerSuggest);
    }

    /// Asks for completions again once the provider knows more: refreshes
    /// an open list, or, while `focused`, opens one at a `.` that found nothing.
    pub fn refresh_suggest(&mut self, focused: bool) {
        let open = self.editor.suggestions().is_some();
        let after_dot = focused && self.text().get(..self.cursor()).is_some_and(|before| before.ends_with('.'));
        if open || after_dot {
            self.trigger_suggest();
        }
    }

    /// The labels of the open completion list.
    pub fn completion_labels(&self) -> Option<Vec<String>> {
        self.editor.suggestions()
    }

    /// A key press while the editor has focus; false when the editor has no use for it.
    pub fn key(&mut self, event: &KeyEvent) -> bool {
        chord(event).is_some_and(|chord| self.editor.key_down(chord))
    }

    pub fn type_text(&mut self, typed: &str) {
        self.editor.type_chars(typed);
    }

    pub fn mouse_down(&mut self, pos: Point, clicks: u32, modifiers: Modifiers) {
        let (x, y) = self.local(pos);
        // The window counts clicks without end; the editor cycles word, line.
        let clicks = (clicks.max(1) - 1) % 3 + 1;
        self.editor.mouse_down(x, y, mods(modifiers), clicks);
        self.dragging = true;
    }

    /// The pointer moved: drags a selection, or hovers when `over` the editor.
    pub fn mouse_move(&mut self, pos: Point, over: bool) {
        if self.dragging || over {
            let (x, y) = self.local(pos);
            self.editor.mouse_move(x, y);
            self.hovered = true;
        } else {
            self.mouse_leave();
        }
    }

    pub fn mouse_up(&mut self) {
        if std::mem::take(&mut self.dragging) {
            self.editor.mouse_up();
        }
    }

    pub fn mouse_leave(&mut self) {
        if std::mem::take(&mut self.hovered) {
            self.editor.mouse_leave();
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Wheel movement in lines, as the window reports it.
    pub fn wheel(&mut self, delta: Point, modifiers: Modifiers) -> bool {
        let lines = wheel_lines();
        let mods = mods(modifiers);
        if delta.y != 0.0 {
            self.editor.wheel(delta.y / lines, false, mods);
        }
        if delta.x != 0.0 {
            self.editor.wheel(-delta.x / lines, true, mods);
        }
        true
    }

    pub fn context_menu(&mut self, pos: Point) {
        let (x, y) = self.local(pos);
        self.editor.context_menu_at(x, y);
    }

    /// The caret in window DIPs, for placing an input method's window.
    pub fn caret_rect(&self) -> Option<Rect> {
        let (x, y, h) = self.editor.caret();
        Some(rect(self.bounds.x + x, self.bounds.y + y, 1.0, h))
    }

    pub fn paint<H: Clone>(&mut self, canvas: &mut Canvas<H>, r: Rect, font: &Font, focused: bool, hit: H) {
        if self.font.as_ref() != Some(font) {
            self.font = Some(font.clone());
            self.apply_options();
        }
        let k = canvas.scale();
        self.editor.set_bounds(
            (r.x * k).round() as i32,
            (r.y * k).round() as i32,
            (r.w * k).round() as i32,
            (r.h * k).round() as i32,
        );
        // Popups may cover the whole window, not just the editor; they are
        // drawn by `paint_popups`.
        let window = canvas.bounds();
        self.editor.set_popup_area(Some((
            (window.x * k).round() as i32,
            (window.y * k).round() as i32,
            (window.w * k).round() as i32,
            (window.h * k).round() as i32,
        )));
        self.editor.set_focused(focused);
        self.bounds = r;
        let editor = &self.editor;
        canvas.paint_native(|target| editor.paint_into(target));
        canvas.hit_cursor(r, hit.clone(), Cursor::IBeam);
        for (x, y, w, h) in self.editor.arrow_areas() {
            canvas.hit_cursor(rect(r.x + x, r.y + y, w, h).intersect(&r), hit.clone(), Cursor::Arrow);
        }
    }

    /// Draws the suggest list, signature help and hover text over whatever
    /// was painted since [`Self::paint`], and takes the mouse over them.
    /// Call it once the rest of the window is drawn.
    pub fn paint_popups<H: Clone>(&mut self, canvas: &mut Canvas<H>, hit: H) {
        let editor = &self.editor;
        canvas.paint_native(|target| editor.paint_popups_into(target));
        let r = self.bounds;
        for (x, y, w, h) in self.editor.popup_frames() {
            canvas.hit_cursor(rect(r.x + x, r.y + y, w, h), hit.clone(), Cursor::Arrow);
        }
    }

    /// The user's editor options, applied now and kept across font changes.
    pub fn set_settings(&mut self, settings: &EditorSettings) {
        if self.settings != *settings {
            self.settings = settings.clone();
            self.apply_options();
        }
    }

    fn apply_options(&mut self) {
        let s = &self.settings;
        let mut options = self.editor.options();
        if let Some(font) = &self.font {
            options.font_family = Some(font.family.to_string());
            options.font_size = font.size;
        }
        options.popup_font_scale = f32::from(s.popup_font_percent.clamp(50, 200)) / 100.0;
        options.suggest_rows = usize::from(s.completion_rows);
        options.quick_suggestions = s.suggest_while_typing;
        options.parameter_hints = s.signature_help;
        options.diagnostics = s.problems;
        options.hover = s.hover;
        options.word_wrap = s.word_wrap;
        options.line_numbers = s.line_numbers;
        options.render_whitespace = s.render_whitespace;
        options.indent_guides = s.indent_guides;
        options.highlight_current_line = s.highlight_current_line;
        options.occurrences_highlight = s.highlight_occurrences;
        options.match_brackets = s.match_brackets;
        options.auto_closing = s.auto_closing;
        options.cursor_blinking = s.cursor_blinking;
        options.tab_size = usize::from(s.tab_size.clamp(1, 16));
        options.insert_spaces = s.insert_spaces;
        options.detect_indentation = s.detect_indentation;
        options.mouse_wheel_zoom = s.wheel_zoom;
        self.editor.set_options(options);
    }

    /// The editor's options as they stand, for tests.
    #[cfg(test)]
    pub fn options(&self) -> code_editor::Options {
        self.editor.options()
    }

    fn local(&self, pos: Point) -> (f32, f32) {
        (pos.x - self.bounds.x, pos.y - self.bounds.y)
    }
}

fn mods(modifiers: Modifiers) -> Mods {
    Mods {
        shift: modifiers.shift,
        ctrl: modifiers.control,
        alt: modifiers.alt,
    }
}

/// The editor's chord for a key event, from the keymap's key name.
fn chord(event: &KeyEvent) -> Option<Chord> {
    let m = event.modifiers;
    let key = match event.key.as_str() {
        "enter" => VK_RETURN,
        "escape" => VK_ESCAPE,
        "tab" => VK_TAB,
        "backspace" => VK_BACK,
        "delete" => VK_DELETE,
        "insert" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "space" => VK_SPACE,
        "menu" => VK_APPS,
        name => {
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
                && (1..=24).contains(&n)
            {
                VIRTUAL_KEY(VK_F1.0 + n - 1)
            } else {
                let mut chars = name.chars();
                let (Some(ch), None) = (chars.next(), chars.next()) else {
                    return None;
                };
                if ch.is_ascii_alphanumeric() {
                    VIRTUAL_KEY(ch.to_ascii_uppercase() as u16)
                } else {
                    // Punctuation: the key that types it on this keyboard layout.
                    let scan = unsafe { VkKeyScanW(ch as u16) };
                    if scan == -1 {
                        return None;
                    }
                    VIRTUAL_KEY((scan & 0xff) as u16)
                }
            }
        }
    };
    Some(Chord::new(key.0, m.control, m.shift, m.alt))
}

fn wheel_lines() -> f32 {
    let mut lines = 3u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut lines as *mut u32 as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    lines.clamp(1, 20) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str, control: bool, shift: bool) -> KeyEvent {
        let modifiers = Modifiers {
            control,
            shift,
            ..Modifiers::NONE
        };
        KeyEvent::new(name, modifiers, None)
    }

    #[test]
    fn key_names_map_to_virtual_keys() {
        assert_eq!(chord(&key("enter", true, false)), Some(Chord::new(VK_RETURN.0, true, false, false)));
        assert_eq!(chord(&key("d", true, false)), Some(Chord::new(b'D' as u16, true, false, false)));
        assert_eq!(chord(&key("f3", false, true)), Some(Chord::new(VK_F3.0, false, true, false)));
        assert_eq!(chord(&key("/", true, false)), Some(Chord::new(VK_OEM_2.0, true, false, false)));
    }
}
