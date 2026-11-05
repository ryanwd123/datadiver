//! What a laid-out chart is made of: shapes and text a backend draws in
//! order, and the marks the pointer can find.

use crate::geom::{Color, Point, Rect};
use crate::theme::{Font, Theme};

/// Text sizes, which only the backend drawing the text knows exactly.
pub trait Measure {
    fn width(&self, text: &str, font: Font) -> f32;
}

/// A rough measure, for tests and for backends without text metrics.
pub struct Approx;

impl Measure for Approx {
    fn width(&self, text: &str, font: Font) -> f32 {
        let em = if font.bold { 0.58 } else { 0.54 };
        text.chars().count() as f32 * font.size * em
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baseline {
    Top,
    Middle,
    Bottom,
}

/// Corner radii, clockwise from the top left.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Corners {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_right: f32,
    pub bottom_left: f32,
}

impl Corners {
    pub const fn all(r: f32) -> Corners {
        Corners { top_left: r, top_right: r, bottom_right: r, bottom_left: r }
    }

    pub fn is_uniform(&self) -> bool {
        self.top_left == self.top_right && self.top_right == self.bottom_right && self.bottom_right == self.bottom_left
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect { rect: Rect, color: Color, corners: Corners, stroke: Option<(Color, f32)> },
    Line { from: Point, to: Point, color: Color, width: f32 },
    /// One line of text placed by its anchor and baseline at `at`, turned
    /// `angle` degrees clockwise around `at`.
    Text { text: String, at: Point, anchor: Anchor, baseline: Baseline, font: Font, color: Color, angle: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub shape: Shape,
    /// The mark this shape draws, so it can lift while hovered.
    pub mark: Option<usize>,
}

/// One row of a tooltip: a key of the series color, the name, the value.
#[derive(Clone, Debug, PartialEq)]
pub struct TipRow {
    pub color: Option<Color>,
    pub label: String,
    pub value: String,
    /// The row of the hovered mark.
    pub strong: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tooltip {
    pub title: String,
    pub rows: Vec<TipRow>,
}

/// Something the pointer can land on.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    /// Where the pointer finds it: larger than what is painted.
    pub area: Rect,
    /// What it points at, for placing the tooltip.
    pub rect: Rect,
    pub series: usize,
    pub index: usize,
    pub tip: Tooltip,
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub items: Vec<Item>,
    pub marks: Vec<Mark>,
}

impl Scene {
    pub fn new(width: f32, height: f32) -> Scene {
        Scene { width, height, ..Scene::default() }
    }

    pub fn push(&mut self, shape: Shape) {
        self.items.push(Item { shape, mark: None });
    }

    pub fn push_mark(&mut self, shape: Shape, mark: usize) {
        self.items.push(Item { shape, mark: Some(mark) });
    }

    pub fn rect(&mut self, rect: Rect, color: Color) {
        self.push(Shape::Rect { rect, color, corners: Corners::default(), stroke: None });
    }

    pub fn line(&mut self, from: Point, to: Point, color: Color, width: f32) {
        self.push(Shape::Line { from, to, color, width });
    }

    pub fn text(&mut self, text: impl Into<String>, at: Point, anchor: Anchor, baseline: Baseline, font: Font, color: Color) {
        self.push(Shape::Text { text: text.into(), at, anchor, baseline, font, color, angle: 0.0 });
    }

    /// The mark under `p`; the smallest wins where areas overlap.
    pub fn hit(&self, p: Point) -> Option<usize> {
        self.marks
            .iter()
            .enumerate()
            .filter(|(_, m)| m.area.contains(p))
            .min_by(|(_, a), (_, b)| (a.area.w * a.area.h).total_cmp(&(b.area.w * b.area.h)))
            .map(|(i, _)| i)
    }

    /// The tooltip of mark `i`, placed beside it within the scene.
    pub fn tooltip(&self, i: usize, theme: &Theme, measure: &dyn Measure) -> Vec<Item> {
        let Some(mark) = self.marks.get(i) else { return Vec::new() };
        tooltip(&mark.tip, mark.rect, Rect::new(0.0, 0.0, self.width, self.height), theme, measure)
    }
}

/// Lays out a tooltip next to `target`, kept inside `bounds`. Values lead,
/// in strong ink; series names follow in secondary ink.
pub fn tooltip(tip: &Tooltip, target: Rect, bounds: Rect, theme: &Theme, measure: &dyn Measure) -> Vec<Item> {
    let pad = 10.0;
    let key_w = 12.0;
    let row_h = theme.tip.line() + 3.0;
    let title_h = theme.tip_value.line() + 4.0;
    let value_w = tip.rows.iter().map(|r| measure.width(&r.value, theme.tip_value)).fold(0.0, f32::max);
    let label_w = tip.rows.iter().map(|r| measure.width(&r.label, if r.strong { theme.tip_value } else { theme.tip })).fold(0.0, f32::max);
    let keyed = tip.rows.iter().any(|r| r.color.is_some());
    let key_space = if keyed { key_w + 6.0 } else { 0.0 };
    let body_w = key_space + value_w + 12.0 + label_w;
    let w = (pad * 2.0 + body_w.max(measure.width(&tip.title, theme.tip))).ceil();
    let h = (pad * 2.0 + title_h + row_h * tip.rows.len() as f32 - 2.0).ceil();
    // Beside a tall target, right of it unless that runs out of room; below
    // a wide one, or above. Kept inside the bounds either way.
    let gap = 10.0;
    let (x, y) = if target.w > target.h {
        let below = target.bottom() + gap;
        let y = if below + h > bounds.bottom() - 4.0 { target.y - gap - h } else { below };
        (target.x + target.w / 2.0 - w / 2.0, y)
    } else {
        let right = target.right() + gap;
        let x = if right + w > bounds.right() - 4.0 { target.x - gap - w } else { right };
        (x, target.y + target.h / 2.0 - h / 2.0)
    };
    let x = x.clamp(bounds.x + 4.0, (bounds.right() - w - 4.0).max(bounds.x + 4.0));
    let y = y.clamp(bounds.y + 4.0, (bounds.bottom() - h - 4.0).max(bounds.y + 4.0));
    let r = Rect::new(x.round(), y.round(), w, h);

    let mut s = Scene::new(0.0, 0.0);
    let shadow = Color::hex(0x000000).with_alpha(if theme.dark { 70 } else { 22 });
    s.push(Shape::Rect { rect: Rect::new(r.x, r.y + 2.0, r.w, r.h).grow(1.0), color: shadow, corners: Corners::all(7.0), stroke: None });
    s.push(Shape::Rect { rect: r, color: theme.tooltip, corners: Corners::all(6.0), stroke: Some((theme.border, 1.0)) });
    s.text(&tip.title, (r.x + pad, r.y + pad), Anchor::Start, Baseline::Top, theme.tip, theme.text2);
    for (n, row) in tip.rows.iter().enumerate() {
        let cy = r.y + pad + title_h + n as f32 * row_h + row_h / 2.0 - 1.0;
        let mut x = r.x + pad;
        if let Some(color) = row.color {
            s.line((x, cy), (x + key_w, cy), color, 3.0);
            x += key_space;
        }
        let value_font = theme.tip_value;
        s.text(&row.value, (x + value_w, cy), Anchor::End, Baseline::Middle, value_font, theme.text);
        let label_font = if row.strong { theme.tip_value } else { theme.tip };
        let label_color = if row.strong { theme.text } else { theme.text2 };
        s.text(&row.label, (x + value_w + 12.0, cy), Anchor::Start, Baseline::Middle, label_font, label_color);
    }
    s.items
}

/// `text` cut with an ellipsis to fit `max` wide.
pub fn fit(text: &str, max: f32, font: Font, measure: &dyn Measure) -> String {
    if measure.width(text, font) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if measure.width(&candidate, font) <= max { lo = mid } else { hi = mid - 1 }
    }
    if lo == 0 { String::new() } else { chars[..lo].iter().collect::<String>().trim_end().to_string() + "…" }
}
