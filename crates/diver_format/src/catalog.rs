//! Everything the language knows by name: formatting actions, functions,
//! keywords, icon sets and icons, with the text that completion and hovers
//! show. Checking, completion and help all read these tables.

/// How an action is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `bold`
    Flag,
    /// `bg: red`
    Property(PropertyValue),
    /// `bar(green)`
    Call,
}

/// What a property takes after its `:`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyValue {
    /// A color or `scale(…)`.
    Paint,
    Color,
    /// Any of a width, a line style and a color.
    Border,
    Width,
    LineStyle,
    Align,
    /// An icon and an optional color.
    Icon,
}

pub struct ActionDef {
    pub name: &'static str,
    pub form: Form,
    pub signature: &'static str,
    pub doc: &'static str,
}

pub const ACTIONS: &[ActionDef] = &[
    ActionDef {
        name: "bg",
        form: Form::Property(PropertyValue::Paint),
        signature: "bg: <color> | scale(…)",
        doc: "Fills the cell. `bg: scale(red, green)` shades by the value.",
    },
    ActionDef {
        name: "text",
        form: Form::Property(PropertyValue::Paint),
        signature: "text: <color> | scale(…)",
        doc: "Colors the cell's text.",
    },
    ActionDef {
        name: "bold",
        form: Form::Flag,
        signature: "bold",
        doc: "Bold text.",
    },
    ActionDef {
        name: "italic",
        form: Form::Flag,
        signature: "italic",
        doc: "Italic text.",
    },
    ActionDef {
        name: "underline",
        form: Form::Flag,
        signature: "underline",
        doc: "Underlined text.",
    },
    ActionDef {
        name: "strike",
        form: Form::Flag,
        signature: "strike",
        doc: "Struck-through text.",
    },
    ActionDef {
        name: "border",
        form: Form::Property(PropertyValue::Border),
        signature: "border: [<width>] [solid|dashed|dotted] [<color>]",
        doc: "Outlines the cell, like `border: 2px dashed red`.",
    },
    ActionDef {
        name: "border_color",
        form: Form::Property(PropertyValue::Color),
        signature: "border_color: <color>",
        doc: "The outline's color (1px solid unless set).",
    },
    ActionDef {
        name: "border_weight",
        form: Form::Property(PropertyValue::Width),
        signature: "border_weight: <width>",
        doc: "The outline's width, like `2px`.",
    },
    ActionDef {
        name: "border_style",
        form: Form::Property(PropertyValue::LineStyle),
        signature: "border_style: solid|dashed|dotted",
        doc: "The outline's line style.",
    },
    ActionDef {
        name: "align",
        form: Form::Property(PropertyValue::Align),
        signature: "align: left|center|right",
        doc: "Aligns the cell's text.",
    },
    ActionDef {
        name: "icon",
        form: Form::Property(PropertyValue::Icon),
        signature: "icon: <icon> [<color>]",
        doc: "Draws an icon before the text, like `icon: up green`.",
    },
    ActionDef {
        name: "scale",
        form: Form::Call,
        signature: "scale(<low color>, [<middle color>,] <high color>, min=, mid=, max=, by=)",
        doc: "Shades the background from the column's lowest value to its highest. \
              `min`, `mid` and `max` move the stops; `by` shades by another column.",
    },
    ActionDef {
        name: "bar",
        form: Form::Call,
        signature: "bar(<color>, [<negative color>], min=, max=, fill=solid|gradient, by=)",
        doc: "A data bar as long as the value is large. Negative values grow left from zero.",
    },
    ActionDef {
        name: "icons",
        form: Form::Call,
        signature: "icons(<set>, [reverse], low=33%, high=67%, by=)",
        doc: "One of three icons by where the value falls in the column's range. \
              `low` and `high` are the cut points: a percent of the range, or values.",
    },
    ActionDef {
        name: "sparkline",
        form: Form::Call,
        signature: "sparkline(<column>, …, color=, kind=line|bars)",
        doc: "A small chart of the row's values in the columns listed, \
              or of the list in one list column.",
    },
    ActionDef {
        name: "display",
        form: Form::Call,
        signature: "display(<value>, [<format>], blank=)",
        doc: "Shows `value` in the cell in place of its own value, like `display(c[0])` or               `display(c * 10, '#,##0.00')`. The format is Excel's: `'0.0%'`, `'$#,##0;($#,##0)'`,               `'YYYY-MM-DD'`, `'mmm d, yyyy'`. `blank` is what shows when the value is blank.",
    },
    ActionDef {
        name: "hide_text",
        form: Form::Flag,
        signature: "hide_text",
        doc: "Hides the value, to show only a bar, icon or sparkline.",
    },
    ActionDef {
        name: "stop",
        form: Form::Flag,
        signature: "stop",
        doc: "Skips the rules after this one for the cell.",
    },
];

pub fn action(name: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.name.eq_ignore_ascii_case(name))
}

/// Whether positional argument `index` of call action `call` is an
/// expression rather than a single value.
pub fn takes_expression(call: &str, index: usize) -> bool {
    call.eq_ignore_ascii_case("display") && index == 0
}

/// The named arguments of the call actions, and what each takes.
pub fn named_args(call: &str) -> &'static [(&'static str, ArgValue, &'static str)] {
    match call {
        "scale" => &[
            ("min", ArgValue::Expr, "The value at the first color (the column's lowest)."),
            ("mid", ArgValue::Expr, "The value at the middle color (halfway)."),
            ("max", ArgValue::Expr, "The value at the last color (the column's highest)."),
            ("by", ArgValue::Column, "Shade by this column's value instead."),
        ],
        "bar" => &[
            ("min", ArgValue::Expr, "The value of an empty bar."),
            ("max", ArgValue::Expr, "The value of a full bar."),
            ("fill", ArgValue::Fill, "solid or gradient."),
            ("by", ArgValue::Column, "Size the bar by this column's value instead."),
        ],
        "icons" => &[
            ("low", ArgValue::Expr, "Below this, the first icon (default 33%)."),
            ("high", ArgValue::Expr, "From this up, the last icon (default 67%)."),
            ("by", ArgValue::Column, "Pick the icon by this column's value instead."),
        ],
        "sparkline" => &[
            ("color", ArgValue::Color, "The line's color."),
            ("kind", ArgValue::Kind, "line or bars."),
        ],
        "display" => &[("blank", ArgValue::Expr, "What shows when the value is blank (nothing by default).")],
        _ => &[],
    }
}

/// What a named argument takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgValue {
    Expr,
    Column,
    Color,
    Fill,
    Kind,
}

/// How a function relates to the column it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionKind {
    /// Works on the values it's given.
    Scalar,
    /// One number for a whole column.
    Aggregate,
    /// Where this row's value stands in its column.
    Relative,
}

pub struct FunctionDef {
    pub name: &'static str,
    pub kind: FunctionKind,
    pub signature: &'static str,
    pub doc: &'static str,
    pub min_args: usize,
    pub max_args: usize,
}

const fn f(
    name: &'static str,
    kind: FunctionKind,
    signature: &'static str,
    doc: &'static str,
    min_args: usize,
    max_args: usize,
) -> FunctionDef {
    FunctionDef { name, kind, signature, doc, min_args, max_args }
}

use FunctionKind::{Aggregate, Relative, Scalar};

/// Aggregate and relative functions read a column: `c` or `"name"`, and `c`
/// when left out.
pub const FUNCTIONS: &[FunctionDef] = &[
    f("min", Aggregate, "min(column)", "The column's lowest value.", 0, 1),
    f("max", Aggregate, "max(column)", "The column's highest value.", 0, 1),
    f("avg", Aggregate, "avg(column)", "The column's mean.", 0, 1),
    f("mean", Aggregate, "mean(column)", "The column's mean (same as avg).", 0, 1),
    f("median", Aggregate, "median(column)", "The column's middle value.", 0, 1),
    f("sum", Aggregate, "sum(column)", "The column's total.", 0, 1),
    f("count", Aggregate, "count(column)", "How many values the column has, blanks left out.", 0, 1),
    f("distinct", Aggregate, "distinct(column)", "How many different values the column has.", 0, 1),
    f("stdev", Aggregate, "stdev(column)", "The column's sample standard deviation.", 0, 1),
    f(
        "percentile",
        Aggregate,
        "percentile(column, p)",
        "The value below which p of the column falls, like `percentile(c, 90%)`.",
        1,
        2,
    ),
    f("duplicate", Relative, "duplicate(column)", "True when this value appears more than once in the column.", 0, 1),
    f("unique", Relative, "unique(column)", "True when this value appears only once in the column.", 0, 1),
    f("frequency", Relative, "frequency(column)", "How many times this value appears in the column.", 0, 1),
    f("rank", Relative, "rank(column)", "1 for the highest value, 2 for the next, …", 0, 1),
    f(
        "percent_rank",
        Relative,
        "percent_rank(column)",
        "Where the value stands, from 0 (lowest) to 1 (highest).",
        0,
        1,
    ),
    f("top", Relative, "top(column, n)", "True for the n highest values (10 when left out).", 0, 2),
    f("bottom", Relative, "bottom(column, n)", "True for the n lowest values (10 when left out).", 0, 2),
    f(
        "top_percent",
        Relative,
        "top_percent(column, p)",
        "True for the highest p of the values, like `top_percent(c, 10%)`.",
        0,
        2,
    ),
    f(
        "bottom_percent",
        Relative,
        "bottom_percent(column, p)",
        "True for the lowest p of the values.",
        0,
        2,
    ),
    f("zscore", Relative, "zscore(column)", "Standard deviations from the column's mean.", 0, 1),
    f("abs", Scalar, "abs(x)", "The number without its sign.", 1, 1),
    f("round", Scalar, "round(x, digits)", "Rounds to digits after the point (0 when left out).", 1, 2),
    f("floor", Scalar, "floor(x)", "Rounds down.", 1, 1),
    f("ceil", Scalar, "ceil(x)", "Rounds up.", 1, 1),
    f("len", Scalar, "len(text)", "The number of characters.", 1, 1),
    f("lower", Scalar, "lower(text)", "The text in lower case.", 1, 1),
    f("upper", Scalar, "upper(text)", "The text in upper case.", 1, 1),
    f("trim", Scalar, "trim(text)", "The text without spaces at its ends.", 1, 1),
    f("coalesce", Scalar, "coalesce(a, b, …)", "The first value that isn't blank.", 1, usize::MAX),
    f("year", Scalar, "year(date)", "The year of a date like 2024-03-15.", 1, 1),
    f("month", Scalar, "month(date)", "The month, 1 to 12.", 1, 1),
    f("day", Scalar, "day(date)", "The day of the month.", 1, 1),
    f("weekday", Scalar, "weekday(date)", "1 for Monday through 7 for Sunday.", 1, 1),
    f("today", Scalar, "today()", "Today's date, like '2024-03-15'.", 0, 0),
    f("days_ago", Scalar, "days_ago(date)", "Days from the date to today (negative for future dates).", 1, 1),
    f("row", Scalar, "row()", "The row's number, from 1.", 0, 0),
];

pub fn function(name: &str) -> Option<&'static FunctionDef> {
    FUNCTIONS.iter().find(|f| f.name.eq_ignore_ascii_case(name))
}

/// Keywords, with what they do.
pub const KEYWORDS: &[(&str, &str)] = &[
    ("if", "Starts a rule: `if <condition> then <formatting>`."),
    ("then", "Separates the condition from the formatting."),
    ("elif", "Another condition, tried when the ones before it are false."),
    ("else", "Formatting for when no condition is true."),
    ("and", "True when both sides are."),
    ("or", "True when either side is."),
    ("not", "Reverses a condition."),
    ("is", "`is null` and `is not null` test for blanks."),
    ("null", "A blank value."),
    ("true", "Yes."),
    ("false", "No."),
    ("between", "`c between 1 and 10` includes both ends."),
    ("in", "`c in ('a', 'b')` matches any value listed."),
    ("like", "Matches a pattern ignoring case: `%` is any text, `_` one character."),
    ("contains", "True when the text contains the other text, ignoring case."),
    ("startswith", "True when the text starts with the other text, ignoring case."),
    ("endswith", "True when the text ends with the other text, ignoring case."),
    ("c", "The cell being formatted. In a list column `c[0]` is the first item and `c[-1]` the last; in a struct column `c.name` is a field."),
];

pub fn keyword(name: &str) -> Option<&'static str> {
    KEYWORDS.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, doc)| *doc)
}

/// Words that can't name a color, icon or other value.
pub fn is_reserved(name: &str) -> bool {
    keyword(name).is_some()
}

/// Shapes `icon:` and the icon sets draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    ArrowUp,
    ArrowDown,
    ArrowRight,
    TriangleUp,
    TriangleDown,
    Dash,
    Circle,
    Check,
    Cross,
    Warn,
    Star,
    StarHalf,
    StarEmpty,
    Flag,
    Square,
    Diamond,
    Bars1,
    Bars2,
    Bars3,
}

/// Icon names, with each one's default color.
pub const ICONS: &[(&str, Shape, u32)] = &[
    ("up", Shape::ArrowUp, 0x21a366),
    ("down", Shape::ArrowDown, 0xe0383e),
    ("right", Shape::ArrowRight, 0xf2a900),
    ("triangle_up", Shape::TriangleUp, 0x21a366),
    ("triangle_down", Shape::TriangleDown, 0xe0383e),
    ("dash", Shape::Dash, 0xf2a900),
    ("circle", Shape::Circle, 0x808080),
    ("check", Shape::Check, 0x21a366),
    ("cross", Shape::Cross, 0xe0383e),
    ("warn", Shape::Warn, 0xf2a900),
    ("star", Shape::Star, 0xf2a900),
    ("star_half", Shape::StarHalf, 0xf2a900),
    ("star_empty", Shape::StarEmpty, 0xf2a900),
    ("flag", Shape::Flag, 0xe0383e),
    ("square", Shape::Square, 0x808080),
    ("diamond", Shape::Diamond, 0x808080),
    ("bars1", Shape::Bars1, 0x4472c4),
    ("bars2", Shape::Bars2, 0x4472c4),
    ("bars3", Shape::Bars3, 0x4472c4),
];

pub fn icon(name: &str) -> Option<(Shape, u32)> {
    ICONS
        .iter()
        .find(|(n, ..)| n.eq_ignore_ascii_case(name))
        .map(|(_, shape, color)| (*shape, *color))
}

/// Icon sets, from the icon for low values to the one for high values.
pub const ICON_SETS: &[(&str, [&str; 3], &str)] = &[
    ("arrows", ["down", "right", "up"], "Red down, yellow sideways, green up arrows."),
    ("triangles", ["triangle_down", "dash", "triangle_up"], "Red down and green up triangles."),
    ("traffic", ["circle", "circle", "circle"], "Red, yellow and green lights."),
    ("checks", ["cross", "warn", "check"], "A red cross, a yellow warning, a green check."),
    ("flags", ["flag", "flag", "flag"], "Red, yellow and green flags."),
    ("stars", ["star_empty", "star_half", "star"], "Empty, half and full stars."),
    ("signal", ["bars1", "bars2", "bars3"], "One, two or three signal bars."),
];

/// The colors of the sets whose icons share a shape.
pub fn icon_set_colors(set: &str) -> Option<[u32; 3]> {
    match set {
        "traffic" | "flags" => Some([0xe0383e, 0xf2a900, 0x21a366]),
        _ => None,
    }
}

pub fn icon_set(name: &str) -> Option<&'static (&'static str, [&'static str; 3], &'static str)> {
    ICON_SETS.iter().find(|(n, ..)| n.eq_ignore_ascii_case(name))
}

pub const LINE_STYLES: &[&str] = &["solid", "dashed", "dotted"];
pub const ALIGNMENTS: &[&str] = &["left", "center", "right"];
pub const FILLS: &[&str] = &["solid", "gradient"];
pub const SPARK_KINDS: &[&str] = &["line", "bars"];

/// The closest of `candidates` to a misspelled `word`, when one is close.
pub fn suggest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let word = word.to_ascii_lowercase();
    let limit = (word.chars().count() / 3).clamp(1, 3);
    candidates
        .into_iter()
        .map(|c| (distance(&word, &c.to_ascii_lowercase()), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Edit distance over characters, a swap of neighbors counting as one edit.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misspellings_find_close_names() {
        assert_eq!(suggest("rde", ["red", "green"]), Some("red"));
        assert_eq!(suggest("bolf", ACTIONS.iter().map(|a| a.name)), Some("bold"));
        assert_eq!(suggest("zzzz", ["red"]), None);
    }

    #[test]
    fn icon_sets_name_real_icons() {
        for (_, icons, _) in ICON_SETS {
            for name in icons {
                assert!(icon(name).is_some(), "{name}");
            }
        }
    }
}
