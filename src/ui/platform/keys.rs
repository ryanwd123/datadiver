//! Keystrokes: what a key press reports, and the `ctrl-shift-p` text form
//! the keymap stores.

pub use crate::domain::keystroke::{Modifiers, display, parse_keystroke, stroke};

/// A key press. `key` names the physical key as its unshifted character
/// (`a`, `1`, `,`) or a name (`enter`, `left`, `f5`); `key_char` is the
/// character the press types, when it types one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: String,
    pub modifiers: Modifiers,
    pub key_char: Option<String>,
}

impl KeyEvent {
    pub fn new(key: impl Into<String>, modifiers: Modifiers, key_char: Option<String>) -> Self {
        Self {
            key: key.into(),
            modifiers,
            key_char,
        }
    }

    /// A press as a test or the keymap writes it, such as `ctrl-shift-p` or `)`.
    pub fn parse(source: &str) -> Result<Self, String> {
        let (modifiers, key) = parse_keystroke(source)?;
        let key_char = if modifiers.control || modifiers.alt || modifiers.platform {
            None
        } else if key == "space" {
            Some(" ".into())
        } else if key.chars().count() == 1 {
            Some(if modifiers.shift {
                shifted(&key)
            } else {
                key.clone()
            })
        } else {
            None
        };
        Ok(Self {
            key,
            modifiers,
            key_char,
        })
    }

    /// `ctrl-alt-shift-cmd-key`, the keymap's canonical form.
    pub fn stroke(&self) -> String {
        stroke(self.modifiers, &self.key)
    }

    pub fn is(&self, key: &str) -> bool {
        self.key == key && !self.modifiers.modified()
    }

    /// The character typed when neither Ctrl, Alt, nor the Windows key is held.
    pub fn typed(&self) -> Option<&str> {
        if self.modifiers.control || self.modifiers.alt || self.modifiers.platform {
            return None;
        }
        self.key_char.as_deref()
    }
}

/// The US-layout character Shift produces on `key`, for test presses.
fn shifted(key: &str) -> String {
    let mapped = match key {
        "1" => "!",
        "2" => "@",
        "3" => "#",
        "4" => "$",
        "5" => "%",
        "6" => "^",
        "7" => "&",
        "8" => "*",
        "9" => "(",
        "0" => ")",
        "-" => "_",
        "=" => "+",
        "[" => "{",
        "]" => "}",
        "\\" => "|",
        ";" => ":",
        "'" => "\"",
        "," => "<",
        "." => ">",
        "/" => "?",
        "`" => "~",
        other => return other.to_uppercase(),
    };
    mapped.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_minus_keys() {
        assert_eq!(
            parse_keystroke("ctrl-shift-p").unwrap(),
            (
                Modifiers {
                    control: true,
                    shift: true,
                    ..Default::default()
                },
                "p".into()
            )
        );
        assert_eq!(parse_keystroke("-").unwrap(), (Modifiers::NONE, "-".into()));
        assert_eq!(
            parse_keystroke("ctrl--").unwrap(),
            (
                Modifiers {
                    control: true,
                    ..Default::default()
                },
                "-".into()
            )
        );
        assert_eq!(parse_keystroke(")").unwrap().1, ")");
        assert_eq!(parse_keystroke("Ctrl-Enter").unwrap().1, "enter");
        assert!(parse_keystroke("hyper-x").is_err());
        assert!(parse_keystroke("ctrl-").is_err());
        assert!(parse_keystroke("").is_err());
    }

    #[test]
    fn strokes_round_trip_and_display() {
        let event = KeyEvent::parse("shift-alt-ctrl-k").unwrap();
        assert_eq!(event.stroke(), "ctrl-alt-shift-k");
        assert_eq!(display("ctrl-shift-p"), "Ctrl+Shift+P");
        assert_eq!(display("f1"), "F1");
        assert_eq!(display("ctrl--"), "Ctrl+-");
        assert_eq!(KeyEvent::parse("shift-0").unwrap().key_char.as_deref(), Some(")"));
        assert_eq!(KeyEvent::parse("ctrl-c").unwrap().key_char, None);
        assert_eq!(KeyEvent::parse("space").unwrap().key_char.as_deref(), Some(" "));
    }
}
