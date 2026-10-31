//! Colors: every ink the editor paints with, and the dark and light palettes
//! (Monaco's `vs-dark` and `vs`). Hosts can override single colors.

/// Something the editor paints. Chrome colors come first, then syntax tokens.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ink {
    Bg,
    Text,
    LineNumber,
    LineNumberActive,
    CurrentLine,
    Selection,
    SelectionInactive,
    Caret,
    ScrollbarThumb,
    ScrollbarThumbHover,
    IndentGuide,
    IndentGuideActive,
    Whitespace,
    BracketMatch,
    BracketMatchBorder,
    WordHighlight,
    FindMatch,
    FindMatchCurrent,
    WidgetBg,
    WidgetBorder,
    WidgetText,
    WidgetDim,
    InputBg,
    InputBorder,
    Focus,
    ToggleOn,
    ListSelected,
    ListHover,
    ListMatch,
    Shadow,
    FoldIcon,
    FoldedBg,
    Error,
    Warning,
    Info,
    /// Tab stops being filled in.
    Placeholder,
    /// Text the editor shows after a line, such as a host's result for it.
    Hint,
    // Syntax tokens.
    Keyword,
    Control,
    String,
    Escape,
    Comment,
    Number,
    Constant,
    Function,
    Type,
    Variable,
    VariableBuiltin,
    Parameter,
    Property,
    Operator,
    Punctuation,
    Decorator,
}

impl Ink {
    pub const ALL: [Ink; INK_COUNT] = [
        Ink::Bg,
        Ink::Text,
        Ink::LineNumber,
        Ink::LineNumberActive,
        Ink::CurrentLine,
        Ink::Selection,
        Ink::SelectionInactive,
        Ink::Caret,
        Ink::ScrollbarThumb,
        Ink::ScrollbarThumbHover,
        Ink::IndentGuide,
        Ink::IndentGuideActive,
        Ink::Whitespace,
        Ink::BracketMatch,
        Ink::BracketMatchBorder,
        Ink::WordHighlight,
        Ink::FindMatch,
        Ink::FindMatchCurrent,
        Ink::WidgetBg,
        Ink::WidgetBorder,
        Ink::WidgetText,
        Ink::WidgetDim,
        Ink::InputBg,
        Ink::InputBorder,
        Ink::Focus,
        Ink::ToggleOn,
        Ink::ListSelected,
        Ink::ListHover,
        Ink::ListMatch,
        Ink::Shadow,
        Ink::FoldIcon,
        Ink::FoldedBg,
        Ink::Error,
        Ink::Warning,
        Ink::Info,
        Ink::Placeholder,
        Ink::Hint,
        Ink::Keyword,
        Ink::Control,
        Ink::String,
        Ink::Escape,
        Ink::Comment,
        Ink::Number,
        Ink::Constant,
        Ink::Function,
        Ink::Type,
        Ink::Variable,
        Ink::VariableBuiltin,
        Ink::Parameter,
        Ink::Property,
        Ink::Operator,
        Ink::Punctuation,
        Ink::Decorator,
    ];

    /// The ink for a token name used in highlight queries and theme overrides
    /// (`keyword`, `string.escape`, `variable.builtin`, ...). A dotted name falls
    /// back to its prefix.
    pub fn from_name(name: &str) -> Option<Ink> {
        let ink = match name {
            "background" => Ink::Bg,
            "foreground" | "text" => Ink::Text,
            "lineNumber" => Ink::LineNumber,
            "lineNumber.active" => Ink::LineNumberActive,
            "currentLine" => Ink::CurrentLine,
            "selection" => Ink::Selection,
            "selection.inactive" => Ink::SelectionInactive,
            "caret" => Ink::Caret,
            "keyword" => Ink::Keyword,
            "keyword.control" => Ink::Control,
            "string" => Ink::String,
            "string.escape" => Ink::Escape,
            "comment" => Ink::Comment,
            "number" => Ink::Number,
            "constant" | "constant.builtin" | "boolean" => Ink::Constant,
            "function" | "function.call" | "function.method" | "function.builtin" => Ink::Function,
            "type" | "type.builtin" | "constructor" => Ink::Type,
            "variable" => Ink::Variable,
            "variable.builtin" => Ink::VariableBuiltin,
            "variable.parameter" => Ink::Parameter,
            "property" | "field" => Ink::Property,
            "operator" => Ink::Operator,
            "punctuation" | "punctuation.bracket" | "punctuation.delimiter" => Ink::Punctuation,
            "decorator" | "attribute" => Ink::Decorator,
            "error" => Ink::Error,
            "warning" => Ink::Warning,
            "info" => Ink::Info,
            "placeholder" => Ink::Placeholder,
            "hint" => Ink::Hint,
            _ => return name.rsplit_once('.').and_then(|(p, _)| Ink::from_name(p)),
        };
        Some(ink)
    }
}

pub const INK_COUNT: usize = 53;

/// A color to paint text or a background with: a theme ink, or a host color
/// (ARGB) from a decoration.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Paint {
    Ink(Ink),
    Argb(u32),
}

/// The two base themes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// ARGB per ink, indexed by `Ink as usize`.
    pub colors: [u32; INK_COUNT],
}

impl Palette {
    pub fn new(theme: Theme) -> Self {
        let f = match theme {
            Theme::Dark => dark,
            Theme::Light => light,
        };
        Palette {
            colors: Ink::ALL.map(f),
        }
    }

    pub fn get(&self, ink: Ink) -> u32 {
        self.colors[ink as usize]
    }

    pub fn set(&mut self, ink: Ink, argb: u32) {
        self.colors[ink as usize] = argb;
    }
}

fn dark(ink: Ink) -> u32 {
    match ink {
        Ink::Bg => 0xFF1E1E1E,
        Ink::Text => 0xFFD4D4D4,
        Ink::LineNumber => 0xFF858585,
        Ink::LineNumberActive => 0xFFC6C6C6,
        Ink::CurrentLine => 0xFF282828,
        Ink::Selection => 0xFF264F78,
        Ink::SelectionInactive => 0xFF3A3D41,
        Ink::Caret => 0xFFAEAFAD,
        Ink::ScrollbarThumb => 0x66797979,
        Ink::ScrollbarThumbHover => 0xB3646464,
        Ink::IndentGuide => 0xFF404040,
        Ink::IndentGuideActive => 0xFF707070,
        Ink::Whitespace => 0x29E3E4E2,
        Ink::BracketMatch => 0x1A006400,
        Ink::BracketMatchBorder => 0xFF888888,
        Ink::WordHighlight => 0xB8575757,
        Ink::FindMatch => 0xFF623315,
        Ink::FindMatchCurrent => 0xFF515C6A,
        Ink::WidgetBg => 0xFF252526,
        Ink::WidgetBorder => 0xFF454545,
        Ink::WidgetText => 0xFFCCCCCC,
        Ink::WidgetDim => 0xFF8B8B8B,
        Ink::InputBg => 0xFF3C3C3C,
        Ink::InputBorder => 0xFF3C3C3C,
        Ink::Focus => 0xFF007FD4,
        Ink::ToggleOn => 0x66007ACC,
        Ink::ListSelected => 0xFF04395E,
        Ink::ListHover => 0xFF2A2D2E,
        Ink::ListMatch => 0xFF18A3FF,
        Ink::Shadow => 0x5C000000,
        Ink::FoldIcon => 0xFFC5C5C5,
        Ink::FoldedBg => 0x33264F78,
        Ink::Error => 0xFFF14C4C,
        Ink::Warning => 0xFFCCA700,
        Ink::Info => 0xFF3794FF,
        Ink::Placeholder => 0x40569CD6,
        Ink::Hint => 0xFF8C8C8C,
        Ink::Keyword => 0xFF569CD6,
        Ink::Control => 0xFFC586C0,
        Ink::String => 0xFFCE9178,
        Ink::Escape => 0xFFD7BA7D,
        Ink::Comment => 0xFF6A9955,
        Ink::Number => 0xFFB5CEA8,
        Ink::Constant => 0xFF569CD6,
        Ink::Function => 0xFFDCDCAA,
        Ink::Type => 0xFF4EC9B0,
        Ink::Variable => 0xFF9CDCFE,
        Ink::VariableBuiltin => 0xFF569CD6,
        Ink::Parameter => 0xFF9CDCFE,
        Ink::Property => 0xFF9CDCFE,
        Ink::Operator => 0xFFD4D4D4,
        Ink::Punctuation => 0xFFD4D4D4,
        Ink::Decorator => 0xFFDCDCAA,
    }
}

fn light(ink: Ink) -> u32 {
    match ink {
        Ink::Bg => 0xFFFFFFFF,
        Ink::Text => 0xFF000000,
        Ink::LineNumber => 0xFF237893,
        Ink::LineNumberActive => 0xFF0B216F,
        Ink::CurrentLine => 0xFFF3F3F3,
        Ink::Selection => 0xFFADD6FF,
        Ink::SelectionInactive => 0xFFE5EBF1,
        Ink::Caret => 0xFF000000,
        Ink::ScrollbarThumb => 0x66646464,
        Ink::ScrollbarThumbHover => 0xB3646464,
        Ink::IndentGuide => 0xFFD3D3D3,
        Ink::IndentGuideActive => 0xFF939393,
        Ink::Whitespace => 0x33333333,
        Ink::BracketMatch => 0x1A0064FF,
        Ink::BracketMatchBorder => 0xFFB9B9B9,
        Ink::WordHighlight => 0x40575757,
        Ink::FindMatch => 0x66EA5C00,
        Ink::FindMatchCurrent => 0xFFA8AC94,
        Ink::WidgetBg => 0xFFF3F3F3,
        Ink::WidgetBorder => 0xFFC8C8C8,
        Ink::WidgetText => 0xFF616161,
        Ink::WidgetDim => 0xFF8B8B8B,
        Ink::InputBg => 0xFFFFFFFF,
        Ink::InputBorder => 0xFFCECECE,
        Ink::Focus => 0xFF0090F1,
        Ink::ToggleOn => 0x330090F1,
        Ink::ListSelected => 0xFFE4E6F1,
        Ink::ListHover => 0xFFF0F0F0,
        Ink::ListMatch => 0xFF0066BF,
        Ink::Shadow => 0x29000000,
        Ink::FoldIcon => 0xFF424242,
        Ink::FoldedBg => 0x4DADD6FF,
        Ink::Error => 0xFFE51400,
        Ink::Warning => 0xFFBF8803,
        Ink::Info => 0xFF1A85FF,
        Ink::Placeholder => 0x334A90E2,
        Ink::Hint => 0xFF8A8A8A,
        Ink::Keyword => 0xFF0000FF,
        Ink::Control => 0xFFAF00DB,
        Ink::String => 0xFFA31515,
        Ink::Escape => 0xFFEE0000,
        Ink::Comment => 0xFF008000,
        Ink::Number => 0xFF098658,
        Ink::Constant => 0xFF0000FF,
        Ink::Function => 0xFF795E26,
        Ink::Type => 0xFF267F99,
        Ink::Variable => 0xFF001080,
        Ink::VariableBuiltin => 0xFF0000FF,
        Ink::Parameter => 0xFF001080,
        Ink::Property => 0xFF001080,
        Ink::Operator => 0xFF000000,
        Ink::Punctuation => 0xFF000000,
        Ink::Decorator => 0xFF795E26,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_ink_in_order() {
        for (i, ink) in Ink::ALL.iter().enumerate() {
            assert_eq!(*ink as usize, i);
        }
    }

    #[test]
    fn token_names() {
        assert_eq!(Ink::from_name("keyword"), Some(Ink::Keyword));
        assert_eq!(Ink::from_name("string.special.foo"), Some(Ink::String));
        assert_eq!(Ink::from_name("variable.builtin"), Some(Ink::VariableBuiltin));
        assert_eq!(Ink::from_name("nonsense"), None);
    }
}
