use crate::infrastructure::json_store::Settings;
use crate::ui::widgets::{DEFAULT_EDITOR_FAMILY, DEFAULT_FAMILY, Fonts};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontPreferences {
    pub size: u8,
    pub family: Option<String>,
    /// Font of the SQL and expression editors; `None` keeps the default monospace font.
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

impl FontPreferences {
    pub fn fonts(&self) -> Fonts {
        Fonts::new(
            f32::from(self.size),
            self.family.as_deref(),
            self.editor_family.as_deref(),
        )
    }
}

/// Which font a font picker changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontTarget {
    Grid,
    Editor,
    /// The SQL editor's completion list.
    Completion,
}

pub(crate) fn default_family(target: FontTarget) -> &'static str {
    match target {
        FontTarget::Grid => DEFAULT_FAMILY,
        FontTarget::Editor => DEFAULT_EDITOR_FAMILY,
        FontTarget::Completion => DEFAULT_FAMILY,
    }
}

/// Whether a font family name looks monospaced; used to list those first when
/// choosing the editor font. The text system does not report it directly.
pub(crate) fn looks_monospaced(family: &str) -> bool {
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
