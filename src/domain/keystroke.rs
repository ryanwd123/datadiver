//! Keystrokes in the `ctrl-shift-p` text form the keymap stores.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows key.
    pub platform: bool,
}

impl Modifiers {
    pub fn modified(&self) -> bool {
        self.control || self.alt || self.shift || self.platform
    }

    pub const NONE: Modifiers = Modifiers {
        control: false,
        alt: false,
        shift: false,
        platform: false,
    };
}

pub fn stroke(modifiers: Modifiers, key: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.platform {
        parts.push("cmd");
    }
    let key = if key == " " { "space" } else { key };
    let key = key.to_ascii_lowercase();
    let mut text = parts.join("-");
    if !text.is_empty() {
        text.push('-');
    }
    text.push_str(&key);
    text
}

/// Splits `ctrl-shift-p` into modifiers and key. A trailing `-` is the minus
/// key, so `ctrl--` and `-` work.
pub fn parse_keystroke(source: &str) -> Result<(Modifiers, String), String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("empty keystroke".into());
    }
    let mut modifiers = Modifiers::default();
    let mut rest = source;
    loop {
        let Some((head, tail)) = rest.split_once('-') else {
            break;
        };
        if tail.is_empty() {
            // `ctrl--` leaves "-" as the key; a lone "x-" is invalid.
            if head.is_empty() {
                break;
            }
            match head.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "alt" | "shift" | "cmd" | "super" | "win" | "platform" => {
                    return Err(format!("missing key after {head}- in {source}"));
                }
                _ => return Err(format!("invalid keystroke {source}")),
            }
        }
        match head.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.control = true,
            "alt" | "option" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            "cmd" | "super" | "win" | "platform" => modifiers.platform = true,
            "fn" | "function" => {}
            "" => {
                // `--`: the remaining text after a separator is the minus key.
                rest = tail;
                break;
            }
            _ => return Err(format!("unknown modifier {head} in {source}")),
        }
        rest = tail;
        if rest == "-" {
            break;
        }
    }
    if rest.is_empty() {
        return Err(format!("missing key in {source}"));
    }
    let key = match rest.to_ascii_lowercase().as_str() {
        " " => "space".to_owned(),
        "esc" => "escape".to_owned(),
        "return" => "enter".to_owned(),
        "del" => "delete".to_owned(),
        "pgup" => "pageup".to_owned(),
        "pgdn" => "pagedown".to_owned(),
        other => other.to_owned(),
    };
    Ok((modifiers, key))
}

/// A keystroke as people read it: `ctrl-shift-p` becomes `Ctrl+Shift+P`.
pub fn display(stroke: &str) -> String {
    let Ok((modifiers, key)) = parse_keystroke(stroke) else {
        return stroke.to_owned();
    };
    let mut parts: Vec<String> = Vec::new();
    if modifiers.control {
        parts.push("Ctrl".into());
    }
    if modifiers.alt {
        parts.push("Alt".into());
    }
    if modifiers.shift {
        parts.push("Shift".into());
    }
    if modifiers.platform {
        parts.push("Win".into());
    }
    let key = match key.as_str() {
        "enter" => "Enter".to_owned(),
        "escape" => "Esc".to_owned(),
        "space" => "Space".to_owned(),
        "tab" => "Tab".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "delete" => "Delete".to_owned(),
        "insert" => "Insert".to_owned(),
        "home" => "Home".to_owned(),
        "end" => "End".to_owned(),
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDn".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        other if other.starts_with('f') && other.len() > 1 && other[1..].parse::<u8>().is_ok() => {
            other.to_uppercase()
        }
        other => other.to_uppercase(),
    };
    parts.push(key);
    parts.join("+")
}

