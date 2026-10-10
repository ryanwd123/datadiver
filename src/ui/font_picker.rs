use std::collections::HashMap;
use std::path::PathBuf;

use gpui_kit::component::theme::Theme;
use gpui_kit::{App, Global, SharedString, px};

use crate::infrastructure::json_store::Settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FontPreferences {
    pub size: u8,
    pub family: Option<String>,
    /// Font of the SQL and expression editors; `None` keeps the theme's monospace font.
    pub editor_family: Option<String>,
}

impl From<&Settings> for FontPreferences {
    fn from(settings: &Settings) -> Self {
        Self {
            size: settings.font_size.clamp(7, 40),
            family: settings.font_family.clone(),
            editor_family: settings.editor_font_family.clone(),
        }
    }
}

#[derive(Default)]
pub(super) struct FontRuntime {
    pub profiles: HashMap<PathBuf, FontPreferences>,
}

impl gpui_kit::Global for FontRuntime {}

/// Which font a font picker changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FontTarget {
    Grid,
    Editor,
}

/// The theme's monospace family before any editor font was applied.
struct DefaultEditorFont(SharedString);

impl Global for DefaultEditorFont {}

pub(super) fn default_editor_family(cx: &mut App) -> SharedString {
    if !cx.has_global::<DefaultEditorFont>() {
        let family = Theme::global(cx).mono_font_family.clone();
        cx.set_global(DefaultEditorFont(family));
    }
    cx.global::<DefaultEditorFont>().0.clone()
}

/// Every code editor draws in the theme's monospace font, so setting it to the
/// chosen editor family and the grid's size restyles them all at once.
pub(super) fn apply_editor_font(font: &FontPreferences, cx: &mut App) {
    // Record the theme's own family before the first change replaces it.
    let default = default_editor_family(cx);
    let family = match &font.editor_family {
        Some(family) => family.clone().into(),
        None => default,
    };
    let theme = Theme::global_mut(cx);
    theme.mono_font_family = family;
    theme.mono_font_size = px(f32::from(font.size));
}

/// Whether a font family name looks monospaced; used to list those first when
/// choosing the editor font. The text system does not report it directly.
pub(super) fn looks_monospaced(family: &str) -> bool {
    let family = family.to_lowercase();
    [
        "mono",
        "code",
        "consol",
        "courier",
        "menlo",
        "fixed",
        "terminal",
        "cascadia",
        "hack",
        "inconsolata",
        "iosevka",
        "jetbrains",
        "lucida console",
        "monaco",
        "sf mono",
    ]
    .iter()
    .any(|hint| family.contains(hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monospace_hints_match_common_code_fonts_only() {
        for family in ["Consolas", "Cascadia Code", "JetBrains Mono", "Courier New"] {
            assert!(looks_monospaced(family), "{family}");
        }
        for family in ["Segoe UI", "Arial", "Times New Roman"] {
            assert!(!looks_monospaced(family), "{family}");
        }
    }
}
