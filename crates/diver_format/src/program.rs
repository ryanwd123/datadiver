//! Checked rules, ready to run against cells.

use std::cmp::Ordering;

use crate::ast::BinOp;
use crate::colors::{self, Color};
use crate::stats::{Aggregate, Stats};
use crate::style::*;
use crate::value::{self, Value};

/// One row of the table the rules run over.
pub trait Row {
    /// The value in column `column` (an index into the compiled columns).
    fn value(&self, column: usize) -> Value;

    /// The row's position, from 0, for `row()`.
    fn index(&self) -> usize {
        0
    }

    /// Today as days since 1970-01-01, for `today()` and `days_ago()`.
    fn today(&self) -> i64 {
        value::today_utc()
    }
}

/// A row held as values in column order.
pub struct ValuesRow<'a> {
    pub values: &'a [Value],
    pub index: usize,
}

impl Row for ValuesRow<'_> {
    fn value(&self, column: usize) -> Value {
        self.values.get(column).cloned().unwrap_or_default()
    }

    fn index(&self) -> usize {
        self.index
    }
}

/// A column a whole-column function reads: the formatted one or a named one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Cell,
    Index(usize),
}

impl Col {
    fn resolve(self, target: usize) -> usize {
        match self {
            Col::Cell => target,
            Col::Index(i) => i,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Abs,
    Round,
    Floor,
    Ceil,
    Len,
    Lower,
    Upper,
    Trim,
    Coalesce,
    Year,
    Month,
    Day,
    Weekday,
    Today,
    DaysAgo,
    Row,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relative {
    Duplicate,
    Unique,
    Frequency,
    Rank,
    PercentRank,
    Top,
    Bottom,
    TopPercent,
    BottomPercent,
    Zscore,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cx {
    Const(Value),
    Cell,
    Column(usize),
    Neg(Box<Cx>),
    Not(Box<Cx>),
    Binary(BinOp, Box<Cx>, Box<Cx>),
    IsNull(Box<Cx>, bool),
    Between(Box<Cx>, Box<Cx>, Box<Cx>, bool),
    In(Box<Cx>, Vec<Cx>, bool),
    Scalar(Scalar, Vec<Cx>),
    Aggregate(Aggregate, Col),
    /// The function, its column and its constant argument (n or p).
    Relative(Relative, Col, f64),
    /// A list's item or a struct's field, by the value of the second.
    Index(Box<Cx>, Box<Cx>),
    /// A struct's field.
    Field(Box<Cx>, String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    Scale(ScaleSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleSpec {
    pub colors: Vec<Color>,
    pub min: Option<Cx>,
    pub mid: Option<Cx>,
    pub max: Option<Cx>,
    pub by: Col,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarSpec {
    pub color: Color,
    pub negative: Option<Color>,
    pub min: Option<Cx>,
    pub max: Option<Cx>,
    pub gradient: bool,
    pub by: Col,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Threshold {
    /// A percent of the column's range.
    Percent(f64),
    Value(Cx),
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconsSpec {
    pub icons: [Icon; 3],
    pub low: Threshold,
    pub high: Threshold,
    pub by: Col,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SparkSpec {
    pub columns: Vec<Col>,
    pub color: Color,
    pub bars: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Background(Paint),
    Text(Paint),
    Bold,
    Italic,
    Underline,
    Strike,
    Border { width: Option<f32>, style: Option<LineStyle>, color: Option<Color> },
    Align(Align),
    Icon(Icon),
    Icons(IconsSpec),
    Bar(BarSpec),
    Sparkline(SparkSpec),
    Display(DisplaySpec),
    Tooltip(DisplaySpec),
    HideText,
    Stop,
}

/// `display(value, format, blank=)`, and `tooltip(…)`, which takes the same.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplaySpec {
    pub value: Cx,
    pub format: Option<crate::format::Format>,
    pub blank: Option<Cx>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompiledRule {
    pub branches: Vec<(Option<Cx>, Vec<Op>)>,
    pub otherwise: Option<Vec<Op>>,
}

/// What a host has to answer for the rules, so it can work it out ahead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Need {
    Aggregate(Aggregate),
    Frequency,
    Position,
}

/// Rules that passed checking. Rules with errors are left out; the rest
/// still run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub rules: Vec<CompiledRule>,
}

const DEFAULT_BORDER: Color = Color::rgb(0x7f, 0x7f, 0x7f);

impl Program {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The formatting for column `target` of `row`.
    pub fn style(&self, target: usize, row: &dyn Row, stats: &dyn Stats) -> CellStyle {
        let mut style = CellStyle::default();
        self.apply(target, row, stats, &mut style);
        style
    }

    /// Runs the rules over `style`, for several programs formatting one
    /// column: what a later one sets wins.
    pub fn apply(&self, target: usize, row: &dyn Row, stats: &dyn Stats, style: &mut CellStyle) {
        let env = Env { target, row, stats };
        for rule in &self.rules {
            let ops = rule
                .branches
                .iter()
                .find(|(condition, _)| condition.as_ref().is_none_or(|c| env.eval(c).truthy()))
                .map(|(_, ops)| ops)
                .or(rule.otherwise.as_ref());
            let Some(ops) = ops else { continue };
            if apply(&env, ops, style) {
                break;
            }
        }
    }

    /// Every statistic the rules ask for, per column, for column `target`.
    pub fn needs(&self, target: usize) -> Vec<(usize, Need)> {
        let mut out = Vec::new();
        let mut add = |col: Col, need: Need| {
            let item = (col.resolve(target), need);
            if !out.contains(&item) {
                out.push(item);
            }
        };
        fn walk(cx: &Cx, add: &mut dyn FnMut(Col, Need)) {
            match cx {
                Cx::Aggregate(aggregate, col) => add(*col, Need::Aggregate(*aggregate)),
                Cx::Relative(relative, col, _) => match relative {
                    Relative::Duplicate | Relative::Unique | Relative::Frequency => add(*col, Need::Frequency),
                    Relative::Zscore => {
                        add(*col, Need::Aggregate(Aggregate::Mean));
                        add(*col, Need::Aggregate(Aggregate::Stdev));
                    }
                    _ => add(*col, Need::Position),
                },
                Cx::Neg(a) | Cx::Not(a) | Cx::IsNull(a, _) | Cx::Field(a, _) => walk(a, add),
                Cx::Binary(_, a, b) | Cx::Index(a, b) => {
                    walk(a, add);
                    walk(b, add);
                }
                Cx::Between(a, b, c, _) => [a, b, c].into_iter().for_each(|x| walk(x, add)),
                Cx::In(a, list, _) => {
                    walk(a, add);
                    list.iter().for_each(|x| walk(x, add));
                }
                Cx::Scalar(_, args) => args.iter().for_each(|x| walk(x, add)),
                Cx::Const(_) | Cx::Cell | Cx::Column(_) => {}
            }
        }
        let range = |add: &mut dyn FnMut(Col, Need), col: Col, bounds: &[&Option<Cx>]| {
            add(col, Need::Aggregate(Aggregate::Min));
            add(col, Need::Aggregate(Aggregate::Max));
            for bound in bounds.iter().copied().flatten() {
                walk(bound, add);
            }
        };
        for rule in &self.rules {
            let ops = rule.branches.iter().flat_map(|(c, ops)| c.iter().map(Err).chain(ops.iter().map(Ok)));
            let ops = ops.chain(rule.otherwise.iter().flatten().map(Ok));
            for item in ops {
                match item {
                    Err(condition) => walk(condition, &mut add),
                    Ok(Op::Background(Paint::Scale(s)) | Op::Text(Paint::Scale(s))) => {
                        range(&mut add, s.by, &[&s.min, &s.mid, &s.max])
                    }
                    Ok(Op::Bar(b)) => range(&mut add, b.by, &[&b.min, &b.max]),
                    Ok(Op::Display(d) | Op::Tooltip(d)) => {
                        walk(&d.value, &mut add);
                        if let Some(blank) = &d.blank {
                            walk(blank, &mut add);
                        }
                    }
                    Ok(Op::Icons(i)) => {
                        let bounds: Vec<&Option<Cx>> = Vec::new();
                        range(&mut add, i.by, &bounds);
                        for t in [&i.low, &i.high] {
                            if let Threshold::Value(cx) = t {
                                walk(cx, &mut add);
                            }
                        }
                    }
                    Ok(_) => {}
                }
            }
        }
        out
    }
}

/// Applies `ops`; true when one of them is `stop`.
fn apply(env: &Env, ops: &[Op], style: &mut CellStyle) -> bool {
    let mut stop = false;
    for op in ops {
        match op {
            Op::Background(paint) => style.background = env.paint(paint).or(style.background),
            Op::Text(paint) => style.text = env.paint(paint).or(style.text),
            Op::Bold => style.bold = true,
            Op::Italic => style.italic = true,
            Op::Underline => style.underline = true,
            Op::Strike => style.strike = true,
            Op::Border { width, style: line, color } => {
                let border = style.border.get_or_insert(Border {
                    color: DEFAULT_BORDER,
                    width: 1.0,
                    style: LineStyle::Solid,
                });
                if let Some(width) = width {
                    border.width = *width;
                }
                if let Some(line) = line {
                    border.style = *line;
                }
                if let Some(color) = color {
                    border.color = *color;
                }
            }
            Op::Align(align) => style.align = Some(*align),
            Op::Icon(icon) => style.icon = Some(*icon),
            Op::Icons(spec) => style.icon = env.icons(spec).or(style.icon),
            Op::Bar(spec) => style.bar = env.bar(spec).or(style.bar),
            Op::Sparkline(spec) => style.sparkline = Some(env.sparkline(spec)),
            Op::Display(spec) => style.display = Some(env.display(spec)),
            Op::Tooltip(spec) => {
                let line = env.display(spec);
                if !line.is_empty() {
                    match &mut style.tooltip {
                        Some(text) => {
                            text.push('\n');
                            text.push_str(&line);
                        }
                        None => style.tooltip = Some(line),
                    }
                }
            }
            Op::HideText => style.hide_text = true,
            Op::Stop => stop = true,
        }
    }
    stop
}

struct Env<'a> {
    target: usize,
    row: &'a dyn Row,
    stats: &'a dyn Stats,
}

impl Env<'_> {
    fn number(&self, col: Col) -> Option<f64> {
        self.row.value(col.resolve(self.target)).number()
    }

    fn aggregate(&self, col: Col, aggregate: Aggregate) -> Option<f64> {
        self.stats.aggregate(col.resolve(self.target), aggregate)
    }

    fn bound(&self, cx: &Option<Cx>) -> Option<f64> {
        cx.as_ref().and_then(|cx| self.eval(cx).number())
    }

    /// The column's range, with explicit bounds in place of its own.
    fn range(&self, col: Col, min: &Option<Cx>, max: &Option<Cx>) -> Option<(f64, f64)> {
        let low = self.bound(min).or_else(|| self.aggregate(col, Aggregate::Min))?;
        let high = self.bound(max).or_else(|| self.aggregate(col, Aggregate::Max))?;
        Some((low, high))
    }

    fn paint(&self, paint: &Paint) -> Option<Color> {
        let s = match paint {
            Paint::Solid(color) => return Some(*color),
            Paint::Scale(s) => s,
        };
        let value = self.number(s.by)?;
        let (low, high) = self.range(s.by, &s.min, &s.max)?;
        let fraction = |v: f64, a: f64, b: f64| if b > a { ((v - a) / (b - a)).clamp(0.0, 1.0) as f32 } else { 0.5 };
        if s.colors.len() == 3 {
            let mid = self.bound(&s.mid).unwrap_or((low + high) / 2.0);
            return Some(if value <= mid {
                s.colors[0].mix(s.colors[1], if mid > low { fraction(value, low, mid) } else { 1.0 })
            } else {
                s.colors[1].mix(s.colors[2], fraction(value, mid, high))
            });
        }
        Some(colors::along(&s.colors, fraction(value, low, high)))
    }

    fn bar(&self, spec: &BarSpec) -> Option<Bar> {
        let value = self.number(spec.by)?;
        let (low, high) = self.range(spec.by, &spec.min, &spec.max)?;
        // A range across zero draws from a zero axis; otherwise bars start
        // at the left, empty at the range's low end (or at zero).
        let (start, end, axis) = if low < 0.0 && high > 0.0 {
            let span = high - low;
            let zero = (-low / span) as f32;
            let at = ((value.clamp(low, high) - low) / span) as f32;
            (zero.min(at), zero.max(at), Some(zero))
        } else if low < 0.0 {
            // All negative: bars grow left from the right edge.
            let base = if spec.max.is_some() { high } else { 0.0 };
            let length = ((base - value) / (base - low).max(f64::EPSILON)).clamp(0.0, 1.0) as f32;
            (1.0 - length, 1.0, None)
        } else {
            let base = if spec.min.is_some() { low } else { 0.0 };
            let length = ((value - base) / (high - base).max(f64::EPSILON)).clamp(0.0, 1.0) as f32;
            (0.0, length, None)
        };
        let color = match spec.negative {
            Some(negative) if value < 0.0 => negative,
            _ => spec.color,
        };
        Some(Bar { start, end, color, gradient: spec.gradient, axis })
    }

    fn icons(&self, spec: &IconsSpec) -> Option<Icon> {
        let value = self.number(spec.by)?;
        let (low, high) = self.range(spec.by, &None, &None)?;
        let cut = |t: &Threshold| match t {
            Threshold::Percent(p) => Some(low + (high - low) * p / 100.0),
            Threshold::Value(cx) => self.eval(cx).number(),
        };
        let (a, b) = (cut(&spec.low)?, cut(&spec.high)?);
        Some(spec.icons[if value < a { 0 } else if value < b { 1 } else { 2 }])
    }

    fn sparkline(&self, spec: &SparkSpec) -> Sparkline {
        let values = match spec.columns.as_slice() {
            [only] => match self.row.value(only.resolve(self.target)) {
                Value::List(items) => items.iter().map(Value::number).collect(),
                value => vec![value.number()],
            },
            columns => columns.iter().map(|c| self.number(*c)).collect(),
        };
        Sparkline { values, color: spec.color, bars: spec.bars }
    }

    fn display(&self, spec: &DisplaySpec) -> String {
        let value = self.eval(&spec.value);
        if value.is_null() {
            return spec.blank.as_ref().map(|blank| self.eval(blank).text().into_owned()).unwrap_or_default();
        }
        match &spec.format {
            Some(format) => format.apply(&value),
            None => value.text().into_owned(),
        }
    }

    fn eval(&self, cx: &Cx) -> Value {
        match cx {
            Cx::Const(v) => v.clone(),
            Cx::Cell => self.row.value(self.target),
            Cx::Column(i) => self.row.value(*i),
            Cx::Neg(a) => self.eval(a).number().map_or(Value::Null, |n| Value::Number(-n)),
            Cx::Not(a) => {
                let v = self.eval(a);
                if v.is_null() { Value::Null } else { Value::Bool(!v.truthy()) }
            }
            Cx::Binary(op, a, b) => self.binary(*op, a, b),
            Cx::IsNull(a, negated) => Value::Bool(self.eval(a).is_null() != *negated),
            Cx::Between(a, low, high, negated) => {
                let v = self.eval(a);
                let inside = matches!(v.compare(&self.eval(low)), Some(Ordering::Greater | Ordering::Equal))
                    && matches!(v.compare(&self.eval(high)), Some(Ordering::Less | Ordering::Equal));
                if v.is_null() { Value::Null } else { Value::Bool(inside != *negated) }
            }
            Cx::In(a, list, negated) => {
                let v = self.eval(a);
                if v.is_null() {
                    return Value::Null;
                }
                let found = list.iter().any(|item| v.compare(&self.eval(item)) == Some(Ordering::Equal));
                Value::Bool(found != *negated)
            }
            Cx::Scalar(f, args) => self.scalar(*f, args),
            Cx::Aggregate(aggregate, col) => self.aggregate(*col, *aggregate).map_or(Value::Null, Value::Number),
            Cx::Relative(f, col, arg) => self.relative(*f, *col, *arg),
            Cx::Index(a, index) => self.eval(a).index(&self.eval(index)),
            Cx::Field(a, name) => self.eval(a).field(name),
        }
    }

    fn binary(&self, op: BinOp, a: &Cx, b: &Cx) -> Value {
        let left = self.eval(a);
        match op {
            BinOp::And => {
                if !left.truthy() && !left.is_null() {
                    return Value::Bool(false);
                }
                let right = self.eval(b);
                return match (left.is_null(), right.truthy(), right.is_null()) {
                    (_, false, false) => Value::Bool(false),
                    (false, true, _) => Value::Bool(true),
                    _ => Value::Null,
                };
            }
            BinOp::Or => {
                if left.truthy() {
                    return Value::Bool(true);
                }
                let right = self.eval(b);
                return if right.truthy() {
                    Value::Bool(true)
                } else if left.is_null() || right.is_null() {
                    Value::Null
                } else {
                    Value::Bool(false)
                };
            }
            _ => {}
        }
        let right = self.eval(b);
        if left.is_null() || right.is_null() {
            return Value::Null;
        }
        let ordering = || left.compare(&right);
        let texts = || (left.text().to_lowercase(), right.text().to_lowercase());
        let numbers = || left.number().zip(right.number());
        let arithmetic = |f: fn(f64, f64) -> f64| numbers().map_or(Value::Null, |(x, y)| Value::Number(f(x, y)));
        match op {
            BinOp::Eq => Value::Bool(ordering() == Some(Ordering::Equal)),
            BinOp::Ne => Value::Bool(ordering() != Some(Ordering::Equal)),
            BinOp::Lt => Value::Bool(ordering() == Some(Ordering::Less)),
            BinOp::Le => Value::Bool(matches!(ordering(), Some(Ordering::Less | Ordering::Equal))),
            BinOp::Gt => Value::Bool(ordering() == Some(Ordering::Greater)),
            BinOp::Ge => Value::Bool(matches!(ordering(), Some(Ordering::Greater | Ordering::Equal))),
            BinOp::Like => {
                let (text, pattern) = texts();
                Value::Bool(like(&text.chars().collect::<Vec<_>>(), &pattern.chars().collect::<Vec<_>>()))
            }
            BinOp::Contains => {
                let (text, part) = texts();
                Value::Bool(text.contains(&part))
            }
            BinOp::StartsWith => {
                let (text, part) = texts();
                Value::Bool(text.starts_with(&part))
            }
            BinOp::EndsWith => {
                let (text, part) = texts();
                Value::Bool(text.ends_with(&part))
            }
            BinOp::Add => arithmetic(|x, y| x + y),
            BinOp::Sub => arithmetic(|x, y| x - y),
            BinOp::Mul => arithmetic(|x, y| x * y),
            BinOp::Div => match numbers() {
                Some((_, 0.0)) => Value::Null,
                _ => arithmetic(|x, y| x / y),
            },
            BinOp::Rem => match numbers() {
                Some((_, 0.0)) => Value::Null,
                _ => arithmetic(|x, y| x % y),
            },
            BinOp::And | BinOp::Or => unreachable!(),
        }
    }

    fn scalar(&self, f: Scalar, args: &[Cx]) -> Value {
        let arg = |i: usize| args.get(i).map_or(Value::Null, |a| self.eval(a));
        let number = |f: fn(f64) -> f64| arg(0).number().map_or(Value::Null, |n| Value::Number(f(n)));
        let text = |f: fn(&str) -> String| {
            let v = arg(0);
            if v.is_null() { Value::Null } else { Value::Text(f(&v.text())) }
        };
        let days = || value::date_days(&arg(0).text());
        let part = |f: fn((i64, u32, u32)) -> i64| {
            value::date_parts(&arg(0).text()).map_or(Value::Null, |p| Value::Number(f(p) as f64))
        };
        match f {
            Scalar::Abs => number(f64::abs),
            Scalar::Floor => number(f64::floor),
            Scalar::Ceil => number(f64::ceil),
            Scalar::Round => {
                let digits = arg(1).number().unwrap_or(0.0).clamp(-15.0, 15.0) as i32;
                let scale = 10f64.powi(digits);
                arg(0).number().map_or(Value::Null, |n| Value::Number((n * scale).round() / scale))
            }
            Scalar::Len => {
                let v = arg(0);
                if v.is_null() { Value::Null } else { Value::Number(v.text().chars().count() as f64) }
            }
            Scalar::Lower => text(str::to_lowercase),
            Scalar::Upper => text(str::to_uppercase),
            Scalar::Trim => text(|s| s.trim().to_string()),
            Scalar::Coalesce => args.iter().map(|a| self.eval(a)).find(|v| !v.is_null()).unwrap_or_default(),
            Scalar::Year => part(|(y, _, _)| y),
            Scalar::Month => part(|(_, m, _)| m as i64),
            Scalar::Day => part(|(_, _, d)| d as i64),
            Scalar::Weekday => days().map_or(Value::Null, |d| Value::Number(((d + 3).rem_euclid(7) + 1) as f64)),
            Scalar::Today => Value::Text(value::format_date(self.row.today())),
            Scalar::DaysAgo => days().map_or(Value::Null, |d| Value::Number((self.row.today() - d) as f64)),
            Scalar::Row => Value::Number(self.row.index() as f64 + 1.0),
        }
    }

    fn relative(&self, f: Relative, col: Col, arg: f64) -> Value {
        let column = col.resolve(self.target);
        let value = self.row.value(column);
        if value.is_null() {
            return Value::Null;
        }
        match f {
            Relative::Duplicate | Relative::Unique | Relative::Frequency => {
                let Some(count) = self.stats.frequency(column, &value) else {
                    return Value::Null;
                };
                return match f {
                    Relative::Duplicate => Value::Bool(count > 1),
                    Relative::Unique => Value::Bool(count == 1),
                    _ => Value::Number(count as f64),
                };
            }
            Relative::Zscore => {
                let (Some(n), Some(mean), Some(sd)) = (
                    value.number(),
                    self.stats.aggregate(column, Aggregate::Mean),
                    self.stats.aggregate(column, Aggregate::Stdev),
                ) else {
                    return Value::Null;
                };
                return if sd > 0.0 { Value::Number((n - mean) / sd) } else { Value::Number(0.0) };
            }
            _ => {}
        }
        let Some(position) = value.number().and_then(|n| self.stats.position(column, n)) else {
            return Value::Null;
        };
        let count = position.count as f64;
        let share = |p: f64| (count * p).ceil().max(1.0);
        match f {
            Relative::Rank => Value::Number(position.above as f64 + 1.0),
            Relative::PercentRank if position.count > 1 => Value::Number(position.below as f64 / (count - 1.0)),
            Relative::PercentRank => Value::Number(0.0),
            Relative::Top => Value::Bool((position.above as f64) < arg),
            Relative::Bottom => Value::Bool((position.below as f64) < arg),
            Relative::TopPercent => Value::Bool((position.above as f64) < share(arg)),
            Relative::BottomPercent => Value::Bool((position.below as f64) < share(arg)),
            Relative::Duplicate | Relative::Unique | Relative::Frequency | Relative::Zscore => unreachable!(),
        }
    }
}

/// SQL `LIKE`: `%` is any run of characters, `_` any one.
fn like(text: &[char], pattern: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('%', rest)) => (0..=text.len()).any(|i| like(&text[i..], rest)),
        Some(('_', rest)) => !text.is_empty() && like(&text[1..], rest),
        Some((c, rest)) => text.first() == Some(c) && like(&text[1..], rest),
    }
}
