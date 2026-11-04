//! Checks parsed rules against the table's columns and the catalog, and
//! compiles the ones without errors into a [`Program`].

use crate::ast::*;
use crate::catalog::{self, ArgValue, Form, FunctionKind, PropertyValue};
use crate::colors::{self, Color};
use crate::parser::parse;
use crate::program::*;
use crate::stats::Aggregate;
use crate::style::{Align, Icon, LineStyle};
use crate::value::{Column, Value};
use crate::{Diagnostic, Severity};

/// Checked rules: the program of the rules without errors, and every problem.
#[derive(Clone, Debug, Default)]
pub struct Checked {
    pub program: Program,
    pub diagnostics: Vec<Diagnostic>,
}

impl Checked {
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error)
    }
}

pub fn check(text: &str, columns: &[Column]) -> Checked {
    let parsed = parse(text);
    let mut checker = Checker { columns, diagnostics: Vec::new() };
    let mut program = Program::default();
    for rule in &parsed.rules.rules {
        let errors_before = checker.error_count();
        let compiled = checker.rule(rule);
        let parse_error = parsed
            .diagnostics
            .iter()
            .any(|d| d.span.start >= rule.span.start && d.span.start <= rule.span.end);
        if let Some(compiled) = compiled
            && checker.error_count() == errors_before
            && !parse_error
        {
            program.rules.push(compiled);
        }
    }
    // Where the parser found a problem, it has the better message.
    let parse_spans: Vec<_> = parsed.diagnostics.iter().map(|d| d.span.clone()).collect();
    let overlaps = |d: &Diagnostic| {
        parse_spans.iter().any(|p| d.span.start <= p.end && p.start <= d.span.end.max(d.span.start + 1))
    };
    let mut diagnostics = parsed.diagnostics;
    diagnostics.extend(checker.diagnostics.into_iter().filter(|d| !overlaps(d)));
    diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    Checked { program, diagnostics }
}

const DEFAULT_SCALE: [Color; 3] = [Color::rgb(0xf8, 0x69, 0x6b), Color::rgb(0xff, 0xeb, 0x84), Color::rgb(0x63, 0xbe, 0x7b)];
const DEFAULT_BAR: Color = Color::rgb(0x63, 0x8e, 0xc6);
const DEFAULT_SPARK: Color = Color::rgb(0x44, 0x72, 0xc4);

struct Checker<'a> {
    columns: &'a [Column],
    diagnostics: Vec<Diagnostic>,
}

impl Checker<'_> {
    fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count()
    }

    fn error<T>(&mut self, span: &Span, message: impl Into<String>) -> Option<T> {
        self.diagnostics.push(Diagnostic { span: span.clone(), message: message.into(), severity: Severity::Error });
        None
    }

    fn warning(&mut self, span: &Span, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic { span: span.clone(), message: message.into(), severity: Severity::Warning });
    }

    fn rule(&mut self, rule: &Rule) -> Option<CompiledRule> {
        let mut branches = Vec::new();
        let mut ok = true;
        for branch in &rule.branches {
            let condition = match &branch.condition {
                Some(c) => {
                    let compiled = self.expr(c);
                    ok &= compiled.is_some();
                    compiled
                }
                None => None,
            };
            let ops = self.actions(&branch.actions);
            ok &= ops.is_some();
            branches.push((condition, ops.unwrap_or_default()));
        }
        let otherwise = rule.otherwise.as_ref().map(|actions| {
            let ops = self.actions(actions);
            ok &= ops.is_some();
            ops.unwrap_or_default()
        });
        ok.then_some(CompiledRule { branches, otherwise })
    }

    // ---- Expressions ----

    fn expr(&mut self, e: &Expr) -> Option<Cx> {
        Some(match &e.kind {
            ExprKind::Cell => Cx::Cell,
            ExprKind::Column(name) => Cx::Column(self.column(name, &e.span)?),
            ExprKind::Number(n) => Cx::Const(Value::Number(*n)),
            ExprKind::Percent(p) => Cx::Const(Value::Number(p / 100.0)),
            ExprKind::Str(s) => Cx::Const(Value::Text(s.clone())),
            ExprKind::Bool(b) => Cx::Const(Value::Bool(*b)),
            ExprKind::Null => Cx::Const(Value::Null),
            ExprKind::Length(_) => {
                return self.error(&e.span, "a width like `2px` goes in formatting, like `border_weight: 2px`");
            }
            ExprKind::Hex(_) => return self.error(&e.span, "a color goes in formatting, like `bg: #ff0000`"),
            ExprKind::Name(name) => return self.unknown_name(name, &e.span),
            ExprKind::Neg(a) => Cx::Neg(Box::new(self.expr(a)?)),
            ExprKind::Not(a) => Cx::Not(Box::new(self.expr(a)?)),
            ExprKind::Binary(op, a, b) => {
                let (a, b) = (self.expr(a), self.expr(b));
                Cx::Binary(*op, Box::new(a?), Box::new(b?))
            }
            ExprKind::IsNull { expr, negated } => Cx::IsNull(Box::new(self.expr(expr)?), *negated),
            ExprKind::Between { expr, low, high, negated } => {
                let (a, l, h) = (self.expr(expr), self.expr(low), self.expr(high));
                Cx::Between(Box::new(a?), Box::new(l?), Box::new(h?), *negated)
            }
            ExprKind::In { expr, list, negated } => {
                let a = self.expr(expr);
                let list: Vec<_> = list.iter().map(|x| self.expr(x)).collect();
                Cx::In(Box::new(a?), list.into_iter().collect::<Option<_>>()?, *negated)
            }
            ExprKind::Call { name, name_span, args } => return self.call(name, name_span, args, &e.span),
            ExprKind::Index { expr, index } => {
                let (a, i) = (self.expr(expr), self.expr(index));
                Cx::Index(Box::new(a?), Box::new(i?))
            }
            ExprKind::Field { expr, name, .. } => Cx::Field(Box::new(self.expr(expr)?), name.clone()),
            ExprKind::Error => return None,
        })
    }

    fn unknown_name<T>(&mut self, name: &str, span: &Span) -> Option<T> {
        if let Some(column) = self.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
            let quoted = crate::lexer::quote_column(&column.name);
            return self.error(span, format!("to use the column, write it in double quotes: {quoted}"));
        }
        if catalog::function(name).is_some() {
            return self.error(span, format!("`{name}` is a function: write `{name}(c)`"));
        }
        if Color::named(name).is_some() {
            return self.error(span, format!("`{name}` is a color: use it in formatting, like `bg: {name}`"));
        }
        let words = catalog::KEYWORDS.iter().map(|(k, _)| *k).chain(catalog::FUNCTIONS.iter().map(|f| f.name));
        match catalog::suggest(name, words) {
            Some(near) => self.error(span, format!("unknown name `{name}`; did you mean `{near}`?")),
            None => self.error(
                span,
                format!("unknown name `{name}`: text goes in single quotes ('{name}'), columns in double quotes"),
            ),
        }
    }

    /// The index of the column `name`.
    fn column(&mut self, name: &str, span: &Span) -> Option<usize> {
        if let Some(i) = self.columns.iter().position(|c| c.name == name) {
            return Some(i);
        }
        if let Some(i) = self.columns.iter().position(|c| c.name.eq_ignore_ascii_case(name)) {
            return Some(i);
        }
        let quoted = crate::lexer::quote_column(name);
        match catalog::suggest(name, self.columns.iter().map(|c| c.name.as_str())) {
            Some(near) => {
                let near = crate::lexer::quote_column(near);
                self.error(span, format!("no column {quoted}; did you mean {near}?"))
            }
            None => self.error(
                span,
                format!("no column {quoted}: double quotes name a column, single quotes are text ('{name}')"),
            ),
        }
    }

    /// `c` or `"name"`, for a function that reads a whole column.
    fn column_ref(&mut self, e: &Expr, what: &str) -> Option<Col> {
        match &e.kind {
            ExprKind::Cell => Some(Col::Cell),
            ExprKind::Column(name) => Some(Col::Index(self.column(name, &e.span)?)),
            ExprKind::Name(name) if self.columns.iter().any(|c| c.name.eq_ignore_ascii_case(name)) => {
                self.unknown_name(name, &e.span)
            }
            ExprKind::Error => None,
            _ => self.error(&e.span, format!("{what} takes a column: `c` or a \"column\"")),
        }
    }

    /// A number written out, like `10` or `10%`.
    fn constant(&mut self, e: &Expr, what: &str) -> Option<f64> {
        match &e.kind {
            ExprKind::Number(n) => Some(*n),
            ExprKind::Percent(p) => Some(p / 100.0),
            ExprKind::Neg(inner) => self.constant(inner, what).map(|n| -n),
            ExprKind::Error => None,
            _ => self.error(&e.span, format!("{what} must be a number written out, like 10 or 10%")),
        }
    }

    fn call(&mut self, name: &str, name_span: &Span, args: &[Arg], span: &Span) -> Option<Cx> {
        let Some(def) = catalog::function(name) else {
            if catalog::action(name).is_some() {
                return self.error(name_span, format!("`{name}(…)` is formatting: put it after `then`"));
            }
            let near = catalog::suggest(name, catalog::FUNCTIONS.iter().map(|f| f.name));
            return match near {
                Some(near) => self.error(name_span, format!("unknown function `{name}`; did you mean `{near}`?")),
                None => self.error(name_span, format!("unknown function `{name}`")),
            };
        };
        if args.len() < def.min_args || args.len() > def.max_args {
            let count = match (def.min_args, def.max_args) {
                (0, 0) => "no arguments".to_string(),
                (a, b) if a == b => format!("{a} argument{}", if a == 1 { "" } else { "s" }),
                (a, usize::MAX) => format!("at least {a}"),
                (a, b) => format!("{a} to {b} arguments"),
            };
            return self.error(span, format!("`{}` takes {count}: {}", def.name, def.signature));
        }
        let what = format!("`{}`", def.name);
        match def.kind {
            FunctionKind::Scalar => {
                let scalar = match def.name {
                    "abs" => Scalar::Abs,
                    "round" => Scalar::Round,
                    "floor" => Scalar::Floor,
                    "ceil" => Scalar::Ceil,
                    "len" => Scalar::Len,
                    "lower" => Scalar::Lower,
                    "upper" => Scalar::Upper,
                    "trim" => Scalar::Trim,
                    "coalesce" => Scalar::Coalesce,
                    "year" => Scalar::Year,
                    "month" => Scalar::Month,
                    "day" => Scalar::Day,
                    "weekday" => Scalar::Weekday,
                    "today" => Scalar::Today,
                    "days_ago" => Scalar::DaysAgo,
                    "row" => Scalar::Row,
                    other => unreachable!("scalar {other}"),
                };
                let compiled: Vec<_> = args.iter().map(|a| self.expr(&a.value)).collect();
                Some(Cx::Scalar(scalar, compiled.into_iter().collect::<Option<_>>()?))
            }
            FunctionKind::Aggregate => {
                let col = match args.first() {
                    Some(a) => self.column_ref(&a.value, &what)?,
                    None => Col::Cell,
                };
                let aggregate = match def.name {
                    "min" => Aggregate::Min,
                    "max" => Aggregate::Max,
                    "avg" | "mean" => Aggregate::Mean,
                    "median" => Aggregate::Median,
                    "sum" => Aggregate::Sum,
                    "count" => Aggregate::Count,
                    "distinct" => Aggregate::Distinct,
                    "stdev" => Aggregate::Stdev,
                    "percentile" => {
                        let Some(p) = args.get(1) else {
                            return self.error(span, "`percentile` needs the percent: percentile(c, 90%)");
                        };
                        Aggregate::Percentile(fraction(self.constant(&p.value, "the percentile")?))
                    }
                    other => unreachable!("aggregate {other}"),
                };
                Some(Cx::Aggregate(aggregate, col))
            }
            FunctionKind::Relative => {
                let col = match args.first() {
                    Some(a) => self.column_ref(&a.value, &what)?,
                    None => Col::Cell,
                };
                let (relative, default) = match def.name {
                    "duplicate" => (Relative::Duplicate, 0.0),
                    "unique" => (Relative::Unique, 0.0),
                    "frequency" => (Relative::Frequency, 0.0),
                    "rank" => (Relative::Rank, 0.0),
                    "percent_rank" => (Relative::PercentRank, 0.0),
                    "zscore" => (Relative::Zscore, 0.0),
                    "top" => (Relative::Top, 10.0),
                    "bottom" => (Relative::Bottom, 10.0),
                    "top_percent" => (Relative::TopPercent, 0.1),
                    "bottom_percent" => (Relative::BottomPercent, 0.1),
                    other => unreachable!("relative {other}"),
                };
                let arg = match args.get(1) {
                    Some(a) => {
                        let n = self.constant(&a.value, "the count")?;
                        if matches!(relative, Relative::TopPercent | Relative::BottomPercent) { fraction(n) } else { n }
                    }
                    None => default,
                };
                Some(Cx::Relative(relative, col, arg))
            }
        }
    }

    // ---- Formatting ----

    fn actions(&mut self, actions: &[Action]) -> Option<Vec<Op>> {
        let mut ops = Vec::new();
        let mut ok = true;
        for action in actions {
            match self.action(action) {
                Some(op) => ops.push(op),
                None => ok = false,
            }
        }
        ok.then_some(ops)
    }

    fn action(&mut self, action: &Action) -> Option<Op> {
        let name = action.name.as_str();
        let span = &action.name_span;
        let Some(def) = catalog::action(name) else {
            if Color::named(name).is_some() {
                return self.error(span, format!("`{name}` is a color: write `bg: {name}` or `text: {name}`"));
            }
            if catalog::function(name).is_some() {
                return self.error(span, format!("`{name}(…)` goes in a condition: `if {name}(c) then …`"));
            }
            return match catalog::suggest(name, catalog::ACTIONS.iter().map(|a| a.name)) {
                Some(near) => self.error(span, format!("unknown formatting `{name}`; did you mean `{near}`?")),
                None => self.error(span, format!("unknown formatting `{name}`")),
            };
        };
        match (def.form, &action.kind) {
            (Form::Flag, ActionKind::Flag) => Some(match name {
                "bold" => Op::Bold,
                "italic" => Op::Italic,
                "underline" => Op::Underline,
                "strike" => Op::Strike,
                "hide_text" => Op::HideText,
                "stop" => Op::Stop,
                other => unreachable!("flag {other}"),
            }),
            (Form::Flag, _) => self.error(&action.span, format!("`{name}` takes no value: just `{name}`")),
            (Form::Property(_), ActionKind::Flag) => {
                self.error(span, format!("`{name}` needs a value: {}", def.signature))
            }
            // The parser already said a value is missing.
            (Form::Property(_), ActionKind::Property(values)) if values.is_empty() => None,
            (Form::Property(kind), ActionKind::Property(values)) => self.property(name, kind, values),
            (Form::Property(_), ActionKind::Call(_)) => {
                self.error(span, format!("write `{name}:` and its value, like {}", def.signature))
            }
            (Form::Call, ActionKind::Call(args)) => self.visual(name, args, span),
            (Form::Call, ActionKind::Flag) => self.visual(name, &[], span),
            // `bar: green` is short for `bar(green)`.
            (Form::Call, ActionKind::Property(values)) => {
                let args: Vec<Arg> =
                    values.iter().map(|v| Arg { name: None, value: v.clone(), span: v.span.clone() }).collect();
                self.visual(name, &args, span)
            }
        }
    }

    /// Complains about values after the first that a property doesn't take.
    fn single<'v>(&mut self, name: &str, values: &'v [Expr]) -> Option<&'v Expr> {
        if let Some(extra) = values.get(1) {
            if let ExprKind::Name(word) = &extra.kind
                && catalog::action(word).is_some()
            {
                return self.error(&extra.span, format!("add a `,` before `{word}`"));
            }
            return self.error(&extra.span, format!("`{name}` takes one value"));
        }
        values.first()
    }

    fn property(&mut self, name: &str, kind: PropertyValue, values: &[Expr]) -> Option<Op> {
        match kind {
            PropertyValue::Paint => {
                let value = self.single(name, values)?;
                let paint = self.paint(value)?;
                Some(if name == "bg" { Op::Background(paint) } else { Op::Text(paint) })
            }
            PropertyValue::Color => {
                let value = self.single(name, values)?;
                let color = self.color(value)?;
                Some(Op::Border { width: None, style: None, color: Some(color) })
            }
            PropertyValue::Width => {
                let value = self.single(name, values)?;
                let width = self.width(value)?;
                Some(Op::Border { width: Some(width), style: None, color: None })
            }
            PropertyValue::LineStyle => {
                let value = self.single(name, values)?;
                let style = self.word(value, catalog::LINE_STYLES, "a line style")?;
                Some(Op::Border { width: None, style: Some(line_style(style)), color: None })
            }
            PropertyValue::Align => {
                let value = self.single(name, values)?;
                let align = match self.word(value, catalog::ALIGNMENTS, "an alignment")? {
                    "left" => Align::Left,
                    "center" => Align::Center,
                    _ => Align::Right,
                };
                Some(Op::Align(align))
            }
            PropertyValue::Border => {
                let (mut width, mut style, mut color) = (None, None, None);
                for value in values {
                    match &value.kind {
                        ExprKind::Length(_) | ExprKind::Number(_) if width.is_none() => width = Some(self.width(value)?),
                        ExprKind::Name(word) if style.is_none() && catalog::LINE_STYLES.contains(&word.as_str()) => {
                            style = Some(line_style(word))
                        }
                        _ if color.is_none() && !matches!(value.kind, ExprKind::Length(_) | ExprKind::Number(_)) => {
                            if let ExprKind::Name(word) = &value.kind
                                && catalog::action(word).is_some()
                            {
                                return self.error(&value.span, format!("add a `,` before `{word}`"));
                            }
                            color = Some(self.color(value)?)
                        }
                        _ => return self.error(&value.span, "`border` takes one width, one line style and one color"),
                    }
                }
                Some(Op::Border { width, style, color })
            }
            PropertyValue::Icon => {
                let first = values.first()?;
                let ExprKind::Name(word) = &first.kind else {
                    return self.error(&first.span, "expected an icon, like `up` or `check`");
                };
                let Some((shape, default)) = catalog::icon(word) else {
                    let near = catalog::suggest(word, catalog::ICONS.iter().map(|(n, ..)| *n));
                    return match near {
                        Some(near) => self.error(&first.span, format!("unknown icon `{word}`; did you mean `{near}`?")),
                        None => self.error(&first.span, format!("unknown icon `{word}`")),
                    };
                };
                let color = match values.get(1) {
                    Some(value) => self.color(value)?,
                    None => hex(default),
                };
                if let Some(extra) = values.get(2) {
                    return self.error(&extra.span, "`icon` takes an icon and a color");
                }
                Some(Op::Icon(Icon { shape, color }))
            }
        }
    }

    fn paint(&mut self, value: &Expr) -> Option<Paint> {
        if let ExprKind::Call { name, args, .. } = &value.kind
            && name == "scale"
        {
            let Op::Background(paint) = self.visual("scale", args, &value.span)? else { unreachable!() };
            return Some(paint);
        }
        Some(Paint::Solid(self.color(value)?))
    }

    fn color(&mut self, value: &Expr) -> Option<Color> {
        match &value.kind {
            ExprKind::Name(name) => {
                if let Some(color) = Color::named(name) {
                    return Some(color);
                }
                let near = catalog::suggest(name, colors::NAMED.iter().map(|(n, _)| *n));
                match near {
                    Some(near) => self.error(&value.span, format!("unknown color `{name}`; did you mean `{near}`?")),
                    None => self.error(&value.span, format!("unknown color `{name}`: use a color name or #rrggbb")),
                }
            }
            ExprKind::Hex(text) => match Color::parse_hex(text) {
                Some(color) => Some(color),
                None => self.error(&value.span, format!("`{text}` isn't a color: use #rgb or #rrggbb")),
            },
            ExprKind::Call { name, args, .. } if name == "rgb" => {
                if args.len() != 3 && args.len() != 4 {
                    return self.error(&value.span, "`rgb` takes red, green and blue (0-255) and an optional alpha (0-1)");
                }
                let mut parts = Vec::new();
                for arg in args {
                    parts.push(self.constant(&arg.value, "an `rgb` part")?);
                }
                let byte = |n: f64| n.clamp(0.0, 255.0).round() as u8;
                let alpha = parts.get(3).map_or(255, |a| (a.clamp(0.0, 1.0) * 255.0).round() as u8);
                Some(Color { a: alpha, ..Color::rgb(byte(parts[0]), byte(parts[1]), byte(parts[2])) })
            }
            ExprKind::Error => None,
            _ => self.error(&value.span, "expected a color, like `red` or `#ff0000`"),
        }
    }

    fn width(&mut self, value: &Expr) -> Option<f32> {
        match &value.kind {
            ExprKind::Length(n) | ExprKind::Number(n) if (0.0..=8.0).contains(n) => Some(*n as f32),
            ExprKind::Length(_) | ExprKind::Number(_) => self.error(&value.span, "widths go from 0px to 8px"),
            _ => self.error(&value.span, "expected a width, like `1px`"),
        }
    }

    fn word(&mut self, value: &Expr, words: &'static [&'static str], what: &str) -> Option<&'static str> {
        if let ExprKind::Name(name) = &value.kind {
            if let Some(word) = words.iter().find(|w| **w == name.as_str()) {
                return Some(word);
            }
            if let Some(near) = catalog::suggest(name, words.iter().copied()) {
                return self.error(&value.span, format!("unknown {what} `{name}`; did you mean `{near}`?"));
            }
        }
        self.error(&value.span, format!("expected {what}: {}", words.join(", ")))
    }

    /// `scale`, `bar`, `icons` and `sparkline`.
    fn visual(&mut self, name: &str, args: &[Arg], span: &Span) -> Option<Op> {
        let allowed = catalog::named_args(name);
        let mut positional = Vec::new();
        let mut named: Vec<(&str, &Expr)> = Vec::new();
        for arg in args {
            match &arg.name {
                None if !named.is_empty() => {
                    return self.error(&arg.span, "put values before named arguments like `min=`");
                }
                None => positional.push(&arg.value),
                Some((key, key_span)) => {
                    if !allowed.iter().any(|(n, ..)| n == key) {
                        let near = catalog::suggest(key, allowed.iter().map(|(n, ..)| *n));
                        let list = allowed.iter().map(|(n, ..)| format!("{n}=")).collect::<Vec<_>>().join(", ");
                        return match near {
                            Some(near) => self.error(key_span, format!("`{name}` has no `{key}=`; did you mean `{near}=`?")),
                            None => self.error(key_span, format!("`{name}` takes {list}")),
                        };
                    }
                    if named.iter().any(|(n, _)| n == key) {
                        return self.error(key_span, format!("`{key}=` is given twice"));
                    }
                    named.push((key.as_str(), &arg.value));
                }
            }
        }
        let mut options = Options { checker: self, named };
        options.visual(name, &positional, span)
    }
}

/// The named arguments of one call, read as the call needs them.
struct Options<'c, 'a, 'e> {
    checker: &'c mut Checker<'a>,
    named: Vec<(&'e str, &'e Expr)>,
}

impl Options<'_, '_, '_> {
    fn get(&self, key: &str) -> Option<&Expr> {
        self.named.iter().find(|(k, _)| *k == key).map(|(_, e)| *e)
    }

    fn expr(&mut self, key: &str) -> Result<Option<Cx>, ()> {
        match self.get(key).cloned() {
            Some(e) => self.checker.expr(&e).map(Some).ok_or(()),
            None => Ok(None),
        }
    }

    fn by(&mut self) -> Option<Col> {
        match self.get("by").cloned() {
            Some(e) => self.checker.column_ref(&e, "`by=`"),
            None => Some(Col::Cell),
        }
    }

    fn word(&mut self, key: &str, words: &'static [&'static str]) -> Option<Option<&'static str>> {
        match self.get(key).cloned() {
            Some(e) => self.checker.word(&e, words, &format!("`{key}=` value")).map(Some),
            None => Some(None),
        }
    }

    fn visual(&mut self, name: &str, positional: &[&Expr], span: &Span) -> Option<Op> {
        match name {
            "scale" => {
                let colors = match positional.len() {
                    0 => DEFAULT_SCALE.to_vec(),
                    2 | 3 => {
                        let mut colors = Vec::new();
                        for e in positional {
                            colors.push(self.checker.color(e)?);
                        }
                        colors
                    }
                    _ => return self.checker.error(span, "`scale` takes two or three colors: scale(red, white, green)"),
                };
                let (min, mid, max) = (self.expr("min").ok()?, self.expr("mid").ok()?, self.expr("max").ok()?);
                if mid.is_some() && colors.len() == 2 {
                    let e = self.get("mid").cloned().unwrap();
                    self.checker.warning(&e.span, "`mid=` needs three colors to have a middle");
                }
                let by = self.by()?;
                Some(Op::Background(Paint::Scale(ScaleSpec { colors, min, mid, max, by })))
            }
            "bar" => {
                if positional.len() > 2 {
                    return self.checker.error(&positional[2].span, "`bar` takes a color and a color for negative values");
                }
                let color = match positional.first() {
                    Some(e) => self.checker.color(e)?,
                    None => DEFAULT_BAR,
                };
                let negative = match positional.get(1) {
                    Some(e) => Some(self.checker.color(e)?),
                    None => None,
                };
                let (min, max) = (self.expr("min").ok()?, self.expr("max").ok()?);
                let gradient = self.word("fill", catalog::FILLS)? == Some("gradient");
                let by = self.by()?;
                Some(Op::Bar(BarSpec { color, negative, min, max, gradient, by }))
            }
            "icons" => {
                let set_name = match positional.first() {
                    Some(e) => match &e.kind {
                        ExprKind::Name(n) if catalog::icon_set(n).is_some() => n.clone(),
                        ExprKind::Name(n) => {
                            let near = catalog::suggest(n, catalog::ICON_SETS.iter().map(|(n, ..)| *n));
                            let all = catalog::ICON_SETS.iter().map(|(n, ..)| *n).collect::<Vec<_>>().join(", ");
                            return match near {
                                Some(near) => self.checker.error(&e.span, format!("unknown icon set `{n}`; did you mean `{near}`?")),
                                None => self.checker.error(&e.span, format!("unknown icon set `{n}`: {all}")),
                            };
                        }
                        _ => return self.checker.error(&e.span, "expected an icon set, like `arrows`"),
                    },
                    None => "arrows".to_string(),
                };
                let reverse = match positional.get(1) {
                    Some(e) if matches!(&e.kind, ExprKind::Name(n) if n == "reverse") => true,
                    Some(e) => return self.checker.error(&e.span, "expected `reverse` or a named argument like `low=`"),
                    None => false,
                };
                if let Some(e) = positional.get(2) {
                    return self.checker.error(&e.span, "`icons` takes a set and `reverse`");
                }
                let (_, names, _) = catalog::icon_set(&set_name).unwrap();
                let set_colors = catalog::icon_set_colors(&set_name);
                let mut icons = [0, 1, 2].map(|i| {
                    let (shape, color) = catalog::icon(names[i]).unwrap();
                    Icon { shape, color: hex(set_colors.map_or(color, |c| c[i])) }
                });
                if reverse {
                    icons.reverse();
                }
                let low = self.threshold("low", 33.0)?;
                let high = self.threshold("high", 67.0)?;
                let by = self.by()?;
                Some(Op::Icons(IconsSpec { icons, low, high, by }))
            }
            "sparkline" => {
                if positional.is_empty() {
                    return self.checker.error(span, "`sparkline` needs columns: sparkline(\"jan\", \"feb\", \"mar\")");
                }
                let mut columns = Vec::new();
                for e in positional {
                    columns.push(self.checker.column_ref(e, "`sparkline`")?);
                }
                let color = match self.get("color").cloned() {
                    Some(e) => self.checker.color(&e)?,
                    None => DEFAULT_SPARK,
                };
                let bars = self.word("kind", catalog::SPARK_KINDS)? == Some("bars");
                Some(Op::Sparkline(SparkSpec { columns, color, bars }))
            }
            "display" => {
                let Some(value) = positional.first() else {
                    return self.checker.error(span, "`display` needs a value: display(c[0]) or display(c, '#,##0.00')");
                };
                if let Some(extra) = positional.get(2) {
                    return self.checker.error(&extra.span, "`display` takes a value and a format");
                }
                let value = self.checker.expr(value)?;
                let format = match positional.get(1) {
                    Some(Expr { kind: ExprKind::Str(text), span }) => match crate::format::Format::parse(text) {
                        Ok(_) if text.is_empty() => return self.checker.error(span, "the format is empty"),
                        Ok(format) => Some(format),
                        Err(message) => return self.checker.error(span, message),
                    },
                    Some(e) => {
                        return self
                            .checker
                            .error(&e.span, "the format is text in quotes, like '#,##0.00' or 'YYYY-MM-DD'");
                    }
                    None => None,
                };
                let blank = self.expr("blank").ok()?;
                Some(Op::Display(DisplaySpec { value, format, blank }))
            }
            other => unreachable!("visual {other}"),
        }
    }

    fn threshold(&mut self, key: &str, default: f64) -> Option<Threshold> {
        match self.get(key).cloned() {
            Some(Expr { kind: ExprKind::Percent(p), .. }) => Some(Threshold::Percent(p)),
            Some(e) => self.checker.expr(&e).map(Threshold::Value),
            None => Some(Threshold::Percent(default)),
        }
    }
}

/// `90%`, `0.9` and `90` all mean 0.9.
fn fraction(n: f64) -> f64 {
    if n > 1.0 { n / 100.0 } else { n }
}

fn hex(rgb: u32) -> Color {
    Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn line_style(word: &str) -> LineStyle {
    match word {
        "dashed" => LineStyle::Dashed,
        "dotted" => LineStyle::Dotted,
        _ => LineStyle::Solid,
    }
}

/// The named-argument value kinds, for completion.
pub fn named_arg_value(call: &str, name: &str) -> Option<ArgValue> {
    catalog::named_args(call).iter().find(|(n, ..)| *n == name).map(|(_, v, _)| *v)
}
