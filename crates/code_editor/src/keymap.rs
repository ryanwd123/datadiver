//! Keyboard shortcuts: the editor's actions, Monaco's default bindings, and
//! bindings the host adds or overrides.

use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// Where a caret motion goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    Home,
    End,
    DocStart,
    DocEnd,
    PageUp,
    PageDown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Moves every caret; `true` extends the selections.
    Move(Motion, bool),
    ScrollLineUp,
    ScrollLineDown,
    ScrollPageUp,
    ScrollPageDown,
    Backspace,
    Delete,
    DeleteWordLeft,
    DeleteWordRight,
    Newline,
    Tab,
    Outdent,
    IndentLines,
    OutdentLines,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    ContextMenu,
    /// Esc: drop extra cursors, close widgets.
    Cancel,
    ToggleLineComment,
    ToggleBlockComment,
    JumpToBracket,
    AddCursorAbove,
    AddCursorBelow,
    AddNextOccurrence,
    SelectAllOccurrences,
    MoveLinesUp,
    MoveLinesDown,
    CopyLinesUp,
    CopyLinesDown,
    DeleteLines,
    SelectLine,
    InsertLineBelow,
    InsertLineAbove,
    ExpandSelection,
    ShrinkSelection,
    Find,
    Replace,
    FindNext,
    FindPrevious,
    GoToLine,
    TriggerSuggest,
    /// Shows the signature of the call around the caret.
    TriggerParameterHints,
    Fold,
    Unfold,
    FoldAll,
    UnfoldAll,
    ToggleWordWrap,
}

/// A key with modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub key: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Chord {
    pub const fn new(key: u16, ctrl: bool, shift: bool, alt: bool) -> Self {
        Chord {
            key,
            ctrl,
            shift,
            alt,
        }
    }

    /// The chord for `key` with the modifiers held now.
    pub fn current(key: u16) -> Self {
        let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
        Chord::new(key, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU))
    }

    /// Parses `Ctrl+Shift+K`, `Alt+Up`, `F3`, `Ctrl+/` (case-insensitive).
    pub fn parse(s: &str) -> Option<Chord> {
        let mut c = Chord::new(0, false, false, false);
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        // `Ctrl++` has an empty last part for the plus key.
        let (mods, key) = match parts.as_slice() {
            [rest @ .., "", ""] => (rest, "+"),
            [rest @ .., k] => (rest, *k),
            [] => return None,
        };
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" => c.ctrl = true,
                "shift" => c.shift = true,
                "alt" => c.alt = true,
                _ => return None,
            }
        }
        c.key = key_code(key)?;
        Some(c)
    }
}

fn key_code(name: &str) -> Option<u16> {
    let lower = name.to_ascii_lowercase();
    let vk = match lower.as_str() {
        "left" => VK_LEFT,
        "right" => VK_RIGHT,
        "up" => VK_UP,
        "down" => VK_DOWN,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" => VK_PRIOR,
        "pagedown" => VK_NEXT,
        "insert" => VK_INSERT,
        "delete" | "del" => VK_DELETE,
        "backspace" => VK_BACK,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "escape" | "esc" => VK_ESCAPE,
        "space" => VK_SPACE,
        ";" => VK_OEM_1,
        "=" | "+" => VK_OEM_PLUS,
        "," => VK_OEM_COMMA,
        "-" => VK_OEM_MINUS,
        "." => VK_OEM_PERIOD,
        "/" => VK_OEM_2,
        "`" => VK_OEM_3,
        "[" => VK_OEM_4,
        "\\" => VK_OEM_5,
        "]" => VK_OEM_6,
        "'" => VK_OEM_7,
        _ => {
            let b = lower.as_bytes();
            if b.len() == 1 && (b[0].is_ascii_alphanumeric()) {
                return Some(b[0].to_ascii_uppercase() as u16);
            }
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u16>().ok())
                && (1..=24).contains(&n)
            {
                return Some(VK_F1.0 + n - 1);
            }
            return None;
        }
    };
    Some(vk.0)
}

/// What a chord does: an editor action or a host command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    Action(Action),
    Command(u32),
    /// Unbinds the default.
    Nothing,
}

const fn k(key: VIRTUAL_KEY) -> u16 {
    key.0
}

const fn ch(c: u8) -> u16 {
    c as u16
}

/// Monaco's default bindings on Windows: (key, ctrl, shift, alt, action).
const DEFAULTS: &[(u16, bool, bool, bool, Action)] = {
    use Action::*;
    use Motion::*;
    &[
        (k(VK_LEFT), false, false, false, Move(Left, false)),
        (k(VK_LEFT), false, true, false, Move(Left, true)),
        (k(VK_RIGHT), false, false, false, Move(Right, false)),
        (k(VK_RIGHT), false, true, false, Move(Right, true)),
        (k(VK_UP), false, false, false, Move(Up, false)),
        (k(VK_UP), false, true, false, Move(Up, true)),
        (k(VK_DOWN), false, false, false, Move(Down, false)),
        (k(VK_DOWN), false, true, false, Move(Down, true)),
        (k(VK_LEFT), true, false, false, Move(WordLeft, false)),
        (k(VK_LEFT), true, true, false, Move(WordLeft, true)),
        (k(VK_RIGHT), true, false, false, Move(WordRight, false)),
        (k(VK_RIGHT), true, true, false, Move(WordRight, true)),
        (k(VK_HOME), false, false, false, Move(Home, false)),
        (k(VK_HOME), false, true, false, Move(Home, true)),
        (k(VK_END), false, false, false, Move(End, false)),
        (k(VK_END), false, true, false, Move(End, true)),
        (k(VK_HOME), true, false, false, Move(DocStart, false)),
        (k(VK_HOME), true, true, false, Move(DocStart, true)),
        (k(VK_END), true, false, false, Move(DocEnd, false)),
        (k(VK_END), true, true, false, Move(DocEnd, true)),
        (k(VK_PRIOR), false, false, false, Move(PageUp, false)),
        (k(VK_PRIOR), false, true, false, Move(PageUp, true)),
        (k(VK_NEXT), false, false, false, Move(PageDown, false)),
        (k(VK_NEXT), false, true, false, Move(PageDown, true)),
        (k(VK_UP), true, false, false, ScrollLineUp),
        (k(VK_DOWN), true, false, false, ScrollLineDown),
        (k(VK_PRIOR), false, false, true, ScrollPageUp),
        (k(VK_NEXT), false, false, true, ScrollPageDown),
        (k(VK_BACK), false, false, false, Backspace),
        (k(VK_BACK), false, true, false, Backspace),
        (k(VK_DELETE), false, false, false, Delete),
        (k(VK_BACK), true, false, false, DeleteWordLeft),
        (k(VK_DELETE), true, false, false, DeleteWordRight),
        (k(VK_RETURN), false, false, false, Newline),
        (k(VK_RETURN), false, true, false, Newline),
        (k(VK_TAB), false, false, false, Tab),
        (k(VK_TAB), false, true, false, Outdent),
        (k(VK_OEM_6), true, false, false, IndentLines),
        (k(VK_OEM_4), true, false, false, OutdentLines),
        (ch(b'Z'), true, false, false, Undo),
        (ch(b'Y'), true, false, false, Redo),
        (ch(b'Z'), true, true, false, Redo),
        (ch(b'X'), true, false, false, Cut),
        (k(VK_DELETE), false, true, false, Cut),
        (ch(b'C'), true, false, false, Copy),
        (k(VK_INSERT), true, false, false, Copy),
        (ch(b'V'), true, false, false, Paste),
        (k(VK_INSERT), false, true, false, Paste),
        (ch(b'A'), true, false, false, SelectAll),
        (k(VK_OEM_PLUS), true, false, false, ZoomIn),
        (k(VK_ADD), true, false, false, ZoomIn),
        (k(VK_OEM_MINUS), true, false, false, ZoomOut),
        (k(VK_SUBTRACT), true, false, false, ZoomOut),
        (ch(b'0'), true, false, false, ZoomReset),
        (k(VK_NUMPAD0), true, false, false, ZoomReset),
        (k(VK_F10), false, true, false, ContextMenu),
        (k(VK_APPS), false, false, false, ContextMenu),
        (k(VK_ESCAPE), false, false, false, Cancel),
        (k(VK_ESCAPE), false, true, false, Cancel),
        (k(VK_OEM_2), true, false, false, ToggleLineComment),
        (ch(b'A'), false, true, true, ToggleBlockComment),
        (k(VK_OEM_5), true, true, false, JumpToBracket),
        (k(VK_UP), true, false, true, AddCursorAbove),
        (k(VK_DOWN), true, false, true, AddCursorBelow),
        (ch(b'D'), true, false, false, AddNextOccurrence),
        (ch(b'L'), true, true, false, SelectAllOccurrences),
        (k(VK_F2), true, false, false, SelectAllOccurrences),
        (k(VK_UP), false, false, true, MoveLinesUp),
        (k(VK_DOWN), false, false, true, MoveLinesDown),
        (k(VK_UP), false, true, true, CopyLinesUp),
        (k(VK_DOWN), false, true, true, CopyLinesDown),
        (ch(b'K'), true, true, false, DeleteLines),
        (ch(b'L'), true, false, false, SelectLine),
        (k(VK_RETURN), true, false, false, InsertLineBelow),
        (k(VK_RETURN), true, true, false, InsertLineAbove),
        (k(VK_RIGHT), false, true, true, ExpandSelection),
        (k(VK_LEFT), false, true, true, ShrinkSelection),
        (ch(b'F'), true, false, false, Find),
        (ch(b'H'), true, false, false, Replace),
        (k(VK_F3), false, false, false, FindNext),
        (k(VK_F3), false, true, false, FindPrevious),
        (ch(b'G'), true, false, false, GoToLine),
        (k(VK_SPACE), true, false, false, TriggerSuggest),
        (ch(b'I'), true, false, false, TriggerSuggest),
        (k(VK_SPACE), true, true, false, TriggerParameterHints),
        (k(VK_OEM_4), true, true, false, Fold),
        (k(VK_OEM_6), true, true, false, Unfold),
        (ch(b'Z'), false, false, true, ToggleWordWrap),
    ]
};

#[derive(Default, Clone)]
pub struct Keymap {
    /// Host bindings, checked before the defaults; later ones win.
    host: Vec<(Chord, Binding)>,
}

impl Keymap {
    pub fn bind(&mut self, chord: Chord, b: Binding) {
        self.host.push((chord, b));
    }

    pub fn lookup(&self, c: Chord) -> Option<Binding> {
        if let Some((_, b)) = self.host.iter().rev().find(|(h, _)| *h == c) {
            return Some(*b);
        }
        DEFAULTS
            .iter()
            .find(|d| d.0 == c.key && d.1 == c.ctrl && d.2 == c.shift && d.3 == c.alt)
            .map(|d| Binding::Action(d.4))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chords() {
        assert_eq!(Chord::parse("Ctrl+Shift+K"), Some(Chord::new(b'K' as u16, true, true, false)));
        assert_eq!(Chord::parse("alt+up"), Some(Chord::new(VK_UP.0, false, false, true)));
        assert_eq!(Chord::parse("F3"), Some(Chord::new(VK_F3.0, false, false, false)));
        assert_eq!(Chord::parse("Ctrl+/"), Some(Chord::new(VK_OEM_2.0, true, false, false)));
        assert_eq!(Chord::parse("Ctrl++"), Some(Chord::new(VK_OEM_PLUS.0, true, false, false)));
        assert_eq!(Chord::parse("Hyper+X"), None);
        assert_eq!(Chord::parse("Ctrl+Nope"), None);
    }

    #[test]
    fn host_bindings_override_defaults() {
        let mut km = Keymap::default();
        let ctrl_d = Chord::parse("Ctrl+D").unwrap();
        assert_eq!(km.lookup(ctrl_d), Some(Binding::Action(Action::AddNextOccurrence)));
        km.bind(ctrl_d, Binding::Command(7));
        assert_eq!(km.lookup(ctrl_d), Some(Binding::Command(7)));
        km.bind(ctrl_d, Binding::Nothing);
        assert_eq!(km.lookup(ctrl_d), Some(Binding::Nothing));
    }

    #[test]
    fn no_duplicate_defaults() {
        for (i, a) in DEFAULTS.iter().enumerate() {
            for b in &DEFAULTS[i + 1..] {
                assert!((a.0, a.1, a.2, a.3) != (b.0, b.1, b.2, b.3), "{a:?} and {b:?}");
            }
        }
    }
}
