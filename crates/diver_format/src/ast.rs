//! The parsed shape of rule text. Spans are byte ranges into the text.

use std::ops::Range;

pub type Span = Range<usize>;

/// Rules, applied in order to each cell of the columns they're for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub rules: Vec<Rule>,
}

/// One `;`-separated rule: `if … then …`, optional `elif`s and `else`, or
/// formatting with no condition.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub branches: Vec<Branch>,
    pub otherwise: Option<Vec<Action>>,
    pub span: Span,
}

/// A condition and what it applies; no condition applies to every cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub condition: Option<Expr>,
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    pub name_span: Span,
    pub kind: ActionKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActionKind {
    /// `bold`.
    Flag,
    /// `bg: red`, `border: 1px dashed red`: the space-separated values.
    Property(Vec<Expr>),
    /// `bar(green, min=0)`.
    Call(Vec<Arg>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// `min` in `min=0`.
    pub name: Option<(String, Span)>,
    pub value: Expr,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Like,
    Contains,
    StartsWith,
    EndsWith,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    /// `c`: the cell being formatted.
    Cell,
    /// `"name"`: another column of the same row.
    Column(String),
    Number(f64),
    /// `10%`, kept as written (10).
    Percent(f64),
    /// `2px`.
    Length(f64),
    /// `'text'`.
    Str(String),
    Bool(bool),
    Null,
    /// `#ff0000`, with the `#`.
    Hex(String),
    /// A bare word: a color, an icon, a keyword value like `dashed`.
    Name(String),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    IsNull { expr: Box<Expr>, negated: bool },
    Between { expr: Box<Expr>, low: Box<Expr>, high: Box<Expr>, negated: bool },
    In { expr: Box<Expr>, list: Vec<Expr>, negated: bool },
    Call { name: String, name_span: Span, args: Vec<Arg> },
    /// `c[0]`: an item of a list, counting from 0 (from the end when
    /// negative), or `c['name']`, a struct's field.
    Index { expr: Box<Expr>, index: Box<Expr> },
    /// `c.name` or `c."field name"`: a struct's field.
    Field { expr: Box<Expr>, name: String, name_span: Span },
    /// Where a parse error left a hole.
    Error,
}
