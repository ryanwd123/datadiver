//! Go to line (Ctrl+G): an input at the top of the editor that previews the
//! line as you type.

use super::Rect;
use super::input::Input;
use crate::selection::Selections;

const WIDTH: f32 = 420.0;
pub const HEIGHT: f32 = 36.0;

pub struct GoTo {
    pub input: Input,
    /// Where to go back to on Esc.
    pub scroll: (f32, f32),
    pub sels: Selections,
}

impl GoTo {
    pub fn frame(width: f32) -> Rect {
        let w = WIDTH.min(width - 20.0).max(200.0);
        let x = ((width - w) / 2.0).round();
        Rect::new(x, 0.0, x + w, HEIGHT + 4.0)
    }

    pub fn input_rect(width: f32) -> Rect {
        let f = GoTo::frame(width);
        Rect::new(f.l + 6.0, f.t + 6.0, f.r - 6.0, f.b - 6.0)
    }
}

/// Parses `line` or `line:column` (1-based) into a 0-based line and column.
/// Negative lines count from the end. `lines` is the line count.
pub fn parse(s: &str, lines: usize) -> Option<(usize, Option<usize>)> {
    let s = s.trim();
    let (l, c) = match s.split_once([':', ',']) {
        Some((l, c)) => (l.trim(), Some(c.trim())),
        None => (s, None),
    };
    let l: i64 = l.parse().ok()?;
    let line = if l < 0 { lines as i64 + l } else { l - 1 };
    let line = line.clamp(0, lines.saturating_sub(1) as i64) as usize;
    let col = match c {
        Some(c) if !c.is_empty() => Some(c.parse::<usize>().ok()?.saturating_sub(1)),
        _ => None,
    };
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(parse("12", 100), Some((11, None)));
        assert_eq!(parse(" 3:7 ", 100), Some((2, Some(6))));
        assert_eq!(parse("999", 100), Some((99, None)));
        assert_eq!(parse("0", 100), Some((0, None)));
        assert_eq!(parse("-1", 100), Some((99, None)));
        assert_eq!(parse("4:", 100), Some((3, None)));
        assert_eq!(parse("x", 100), None);
        assert_eq!(parse("", 100), None);
    }
}
