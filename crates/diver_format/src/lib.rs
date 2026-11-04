//! Conditional formatting rules for DataDiver's grids: a small language
//! somewhere between Python and SQL.
//!
//! ```text
//! if c > "target" then bg: #c6efce, bold;
//! if duplicate(c) then bg: mistyrose, text: firebrick;
//! if c > 0 then icon: up elif c < 0 then icon: down else text: gray;
//! scale(red, white, green, mid=0);
//! bar(steelblue, fill=gradient)
//! ```
//!
//! Rules are separated by `;` and run in order for each cell of the columns
//! they're applied to; later rules override what earlier ones set, and
//! `stop` ends the run for that cell. `c` is the cell's own value, `"name"`
//! another column of the row, `'text'` text.
//!
//! - [`check`] parses rules and checks them against the table's columns,
//!   returning a [`Program`] and the problems found.
//! - [`Program::style`] formats one cell, given its row and the columns'
//!   [`Stats`] ([`TableStats`] works them out from columns in memory).
//! - [`draw::layout`] turns a [`CellStyle`] into marks to draw.
//! - [`complete()`], [`help`] and [`editor::FormatLanguage`] serve an editor.

pub mod ast;
pub mod catalog;
pub mod check;
pub mod colors;
pub mod complete;
pub mod draw;
#[cfg(feature = "editor")]
pub mod editor;
pub mod format;
pub mod help;
pub mod lexer;
pub mod parser;
pub mod presets;
pub mod program;
pub mod stats;
pub mod style;
pub mod value;

use std::ops::Range;

pub use check::{Checked, check};
pub use colors::Color;
pub use complete::{Completion, complete, hex_at};
pub use presets::{PRESETS, Preset};
pub use program::{Program, Row, ValuesRow};
pub use stats::{Aggregate, NoStats, Stats, TableStats};
pub use style::CellStyle;
pub use value::{Column, Value, ValueKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A problem in rule text, at a byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Range<usize>,
    pub message: String,
    pub severity: Severity,
}

/// Rules and the columns they format: one entry of a grid's formatting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatSet {
    pub rules: String,
    pub columns: Vec<String>,
}
