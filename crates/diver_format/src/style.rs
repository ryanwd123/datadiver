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
    pub hide_text: bool,
}

impl CellStyle {
    pub fn is_plain(&self) -> bool {
        *self == CellStyle::default()
    }
}
