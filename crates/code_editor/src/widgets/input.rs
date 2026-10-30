//! A single-line text input used by every overlay (find, replace, go to line).

use crate::keymap::Chord;
use crate::movement::{next_boundary, prev_boundary, word_end_right, word_start_left};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Input {
    pub text: String,
    /// Caret and selection anchor, as byte offsets.
    pub caret: usize,
    pub anchor: usize,
    /// Horizontal scroll in DIPs, kept so the caret stays visible.
    pub scroll: f32,
}

/// What a key did to an input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edited {
    /// Not an input key.
    No,
    /// The caret or selection moved.
    Moved,
    /// The text changed.
    Text,
}

impl Input {
    /// Replaces the text (first line only) and selects all of it.
    pub fn set(&mut self, s: &str) {
        self.text = s.lines().next().unwrap_or("").to_string();
        self.select_all();
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[a..b]
    }

    /// Replaces the selection with `s` (line breaks become spaces).
    pub fn insert(&mut self, s: &str) {
        let s = s.replace(['\r', '\n'], " ");
        let (a, b) = self.selection();
        self.text.replace_range(a..b, &s);
        self.caret = a + s.len();
        self.anchor = self.caret;
    }

    /// Cuts the selection, returning it.
    pub fn cut(&mut self) -> Option<String> {
        let t = self.selected().to_string();
        if t.is_empty() {
            return None;
        }
        self.insert("");
        Some(t)
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.caret = to;
        if !extend {
            self.anchor = to;
        }
    }

    /// Handles editing and caret keys.
    pub fn key(&mut self, c: Chord) -> Edited {
        let k = VIRTUAL_KEY(c.key);
        let t = &self.text;
        let (a, b) = self.selection();
        match (k, c.ctrl, c.alt) {
            (VK_LEFT, ctrl, false) => {
                let to = if !c.shift && a != b && !ctrl {
                    a
                } else if ctrl {
                    word_start_left(t, self.caret)
                } else {
                    prev_boundary(t, self.caret)
                };
                self.move_to(to, c.shift);
                Edited::Moved
            }
            (VK_RIGHT, ctrl, false) => {
                let to = if !c.shift && a != b && !ctrl {
                    b
                } else if ctrl {
                    word_end_right(t, self.caret)
                } else {
                    next_boundary(t, self.caret)
                };
                self.move_to(to, c.shift);
                Edited::Moved
            }
            (VK_HOME, _, false) => {
                self.move_to(0, c.shift);
                Edited::Moved
            }
            (VK_END, _, false) => {
                let n = t.len();
                self.move_to(n, c.shift);
                Edited::Moved
            }
            (VK_BACK, ctrl, false) => {
                if a == b {
                    let to = if ctrl { word_start_left(t, a) } else { prev_boundary(t, a) };
                    self.anchor = to;
                }
                self.insert("");
                Edited::Text
            }
            (VK_DELETE, ctrl, false) if !c.shift => {
                if a == b {
                    let to = if ctrl { word_end_right(t, a) } else { next_boundary(t, a) };
                    self.anchor = to;
                }
                self.insert("");
                Edited::Text
            }
            (_, true, false) if k.0 == b'A' as u16 => {
                self.select_all();
                Edited::Moved
            }
            _ => Edited::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: &mut Input, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> Edited {
        i.key(Chord::new(vk.0, ctrl, shift, false))
    }

    #[test]
    fn typing_and_editing() {
        let mut i = Input::default();
        i.insert("hello world");
        assert_eq!((i.text.as_str(), i.caret), ("hello world", 11));
        key(&mut i, VK_BACK, true, false);
        assert_eq!(i.text, "hello ");
        key(&mut i, VK_HOME, false, false);
        key(&mut i, VK_RIGHT, false, true);
        key(&mut i, VK_RIGHT, false, true);
        assert_eq!(i.selected(), "he");
        i.insert("Y");
        assert_eq!(i.text, "Yllo ");
        assert_eq!(i.cut(), None);
        key(&mut i, VK_A, true, false);
        assert_eq!(i.cut().as_deref(), Some("Yllo "));
        assert_eq!(i.text, "");
        i.insert("a\nb");
        assert_eq!(i.text, "a b");
        assert_eq!(key(&mut i, VK_F5, false, false), Edited::No);
    }

    #[test]
    fn set_selects_all_and_keeps_one_line() {
        let mut i = Input::default();
        i.set("one\ntwo");
        assert_eq!(i.text, "one");
        assert_eq!(i.selection(), (0, 3));
        key(&mut i, VK_LEFT, false, false);
        assert_eq!((i.caret, i.anchor), (0, 0));
    }
}
