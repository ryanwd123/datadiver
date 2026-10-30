//! The Monaco-style code editor (`code_editor`), drawn into the window's own
//! frame. DataDiver keeps its keymap, focus, and hit testing, and passes the
//! editor the keys, text, and mouse input that fall to it.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use code_editor::{Action, Chord, CompletionProvider, Editor, Event, LanguageService, Mods, Options, Theme};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};

use super::WidgetId;
use crate::ui::platform::keys::{KeyEvent, Modifiers};
use crate::ui::platform::{Canvas, Cursor, Font, Point, Rect, rect};

pub struct HostedEditor {
    pub id: WidgetId,
    editor: Editor,
    /// Set by the editor when its text changes; read with [`Self::take_changed`].
    changed: Rc<Cell<bool>>,
    /// Where the editor was last painted, in window DIPs.
    bounds: Rect,
    font: Option<Font>,
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
        let editor = Editor::create_hosted(host.unwrap_or_default(), options, Theme::Light).expect("create the SQL editor");
        editor.set_language("sql");
        editor.set_completion_provider(completions);
        editor.set_language_service(Some(Rc::new(language)));
        let changed = Rc::new(Cell::new(false));
        let flag = changed.clone();
        editor.on_event(move |_, event| {
            if matches!(event, Event::ContentChanged { .. }) {
                flag.set(true);
            }
        });
        Self {
            id: WidgetId::new(),
            editor,
            changed,
            bounds: Rect::default(),
            font: None,
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

    /// Whether the text changed since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.replace(false)
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
            let mut options = self.editor.options();
            options.font_family = Some(font.family.to_string());
            options.font_size = font.size;
            self.editor.set_options(options);
            self.font = Some(font.clone());
        }
        let k = canvas.scale();
        self.editor.set_bounds(
            (r.x * k).round() as i32,
            (r.y * k).round() as i32,
            (r.w * k).round() as i32,
            (r.h * k).round() as i32,
        );
        self.editor.set_focused(focused);
        self.bounds = r;
        let editor = &self.editor;
        canvas.paint_native(|target| editor.paint_into(target));
        canvas.hit_cursor(r, hit.clone(), Cursor::IBeam);
        for (x, y, w, h) in self.editor.arrow_areas() {
            canvas.hit_cursor(rect(r.x + x, r.y + y, w, h), hit.clone(), Cursor::Arrow);
        }
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
