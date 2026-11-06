//! Colors for conditional formatting rules outside the rules editor, the
//! same ones the editor shows.

use std::cell::RefCell;
use std::rc::Rc;

use code_editor::theme::Palette;
use code_editor::{LanguageService, Theme};
use diver_format::editor::FormatLanguage;

use crate::ui::platform::Color;

/// Colored byte ranges of rule text.
pub fn runs(text: &str) -> Vec<(usize, usize, Color)> {
    let language = FormatLanguage::new(Rc::new(RefCell::new(Vec::new())));
    let palette = Palette::new(Theme::Light);
    language
        .highlight(text)
        .unwrap_or_default()
        .into_iter()
        .map(|(range, ink)| (range.start, range.end, Color::rgb(palette.get(ink) & 0xff_ffff)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_rules_like_the_rules_editor() {
        let text = "if c > \"target\" then bg: #ffc7ce, bold -- note";
        let colored: Vec<&str> = runs(text).into_iter().map(|(start, end, _)| &text[start..end]).collect();
        for token in ["if", "c", "\"target\"", "then", "bg", "#ffc7ce", "bold", "-- note"] {
            assert!(colored.contains(&token), "{token} in {colored:?}");
        }
        let palette = Palette::new(Theme::Light);
        let keyword = Color::rgb(palette.get(code_editor::Ink::Keyword) & 0xff_ffff);
        assert!(runs(text).contains(&(0, 2, keyword)));
    }
}
