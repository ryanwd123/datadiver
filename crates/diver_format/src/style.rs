//! What the rules decide for one cell.

use crate::catalog::Shape;
use crate::colors::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub color: Color,
    /// In DIPs.
    pub width: f32,
    pub style: LineStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icon {
    pub shape: Shape,
    pub color: Color,
}

/// A data bar, as fractions of the cell's width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub start: f32,
    pub end: f32,
    pub color: Color,
    pub gradient: bool,
    /// Where zero is, when the column has negative and positive values.
    pub axis: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sparkline {
    pub values: Vec<Option<f64>>,
    pub color: Color,
    pub bars: bool,
}

/// What `tooltip(…)` shows: text, parts of it in their own color or weight,
/// and how the popup looks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tooltip {
    pub text: String,
    pub spans: Vec<TooltipSpan>,
    pub look: TooltipLook,
}

impl Tooltip {
    /// Adds `other` on a line of its own. Its look wins where it sets one.
    pub fn push(&mut self, other: Tooltip) {
        let start = if self.text.is_empty() {
            0
        } else {
            self.text.push('\n');
            self.text.len()
        };
        self.text.push_str(&other.text);
        self.spans.extend(other.spans.into_iter().map(|span| TooltipSpan {
            range: span.range.start + start..span.range.end + start,
            ..span
        }));
        self.look.merge(other.look);
    }
}

/// A byte range of a tooltip's text, styled by its template field.
#[derive(Clone, Debug, PartialEq)]
pub struct TooltipSpan {
    pub range: std::ops::Range<usize>,
    pub color: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TooltipShape {
    /// A box with an arrow pointing at the cell.
    #[default]
    Callout,
    Box,
}

/// How a tooltip looks; what's left unset keeps the host's own look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TooltipLook {
    pub background: Option<Color>,
    pub text: Option<Color>,
    /// The border's color; transparent for none.
    pub border: Option<Color>,
    /// In DIPs, like the rest below.
    pub border_width: Option<f32>,
    pub padding: Option<f32>,
    pub radius: Option<f32>,
    pub max_width: Option<f32>,
    pub shape: Option<TooltipShape>,
    /// A fixed-width font, which lines up padded fields.
    pub mono: Option<bool>,
    /// How long the pointer rests on the cell first, in milliseconds.
    pub delay: Option<u32>,
}

impl TooltipLook {
    /// Takes what `other` sets.
    pub fn merge(&mut self, other: TooltipLook) {
        macro_rules! take {
            ($($field:ident),*) => {$(if other.$field.is_some() { self.$field = other.$field; })*};
        }
        take!(background, text, border, border_width, padding, radius, max_width, shape, mono, delay);
    }
}

/// One cell's formatting. Everything left at its default keeps the grid's
/// own look.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellStyle {
    pub background: Option<Color>,
    pub text: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub border: Option<Border>,
    pub align: Option<Align>,
    pub icon: Option<Icon>,
    pub bar: Option<Bar>,
    pub sparkline: Option<Sparkline>,
    /// Text shown in place of the cell's value, from `display(…)`.
    pub display: Option<String>,
    /// Shown while the pointer rests on the cell, from `tooltip(…)`: a line
    /// for each tooltip the rules set.
    pub tooltip: Option<Tooltip>,
    pub hide_text: bool,
}

impl CellStyle {
    pub fn is_plain(&self) -> bool {
        *self == CellStyle::default()
    }
}
