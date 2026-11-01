//! A cell's formatting as simple marks (rectangles, lines, polygons), so
//! every host draws bars, icons and sparklines the same way with whatever
//! graphics it has.

use crate::catalog::Shape;
use crate::colors::Color;
use crate::style::{CellStyle, LineStyle};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w: w.max(0.0), h: h.max(0.0) }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w - 2.0 * dx, self.h - 2.0 * dy)
    }
}

pub type Point = (f32, f32);

#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Fill { rect: Rect, color: Color },
    /// `color` at the left fading to clear at the right.
    Fade { rect: Rect, color: Color },
    /// A rectangle's outline, its stroke centered on the edges.
    Outline { rect: Rect, color: Color, width: f32, style: LineStyle },
    Line { from: Point, to: Point, color: Color, width: f32, style: LineStyle },
    /// Connected segments with round joins.
    Polyline { points: Vec<Point>, color: Color, width: f32 },
    Polygon { points: Vec<Point>, color: Color },
    Circle { center: Point, radius: f32, color: Color },
}

/// What to draw for one cell: marks under the text, where the text goes,
/// and marks over it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellLayout {
    pub under: Vec<Mark>,
    /// `None` when the text is hidden.
    pub text: Option<Rect>,
    pub over: Vec<Mark>,
}

/// Lays out `style` in `cell` (DIPs). The grid draws its own background
/// first and the cell's text in `text`.
pub fn layout(style: &CellStyle, cell: Rect) -> CellLayout {
    let mut out = CellLayout { text: Some(cell), ..Default::default() };
    if let Some(color) = style.background {
        out.under.push(Mark::Fill { rect: cell, color });
    }
    if let Some(bar) = style.bar {
        let area = cell.inset(2.0, 3.0);
        let rect = Rect::new(area.x + area.w * bar.start, area.y, area.w * (bar.end - bar.start), area.h);
        out.under.push(if bar.gradient {
            Mark::Fade { rect, color: bar.color }
        } else {
            Mark::Fill { rect, color: Color { a: 190, ..bar.color } }
        });
        if let Some(axis) = bar.axis {
            let x = (area.x + area.w * axis).round() + 0.5;
            out.under.push(Mark::Line {
                from: (x, cell.y),
                to: (x, cell.bottom()),
                color: Color::rgb(0x40, 0x40, 0x40),
                width: 1.0,
                style: LineStyle::Dotted,
            });
        }
    }
    if let Some(icon) = style.icon {
        let side = (cell.h - 6.0).clamp(6.0, 14.0);
        let square = Rect::new(cell.x + 4.0, cell.y + (cell.h - side) / 2.0, side, side);
        out.over.extend(icon_marks(icon.shape, icon.color, square));
        out.text = Some(Rect::new(cell.x + side + 6.0, cell.y, cell.w - side - 6.0, cell.h));
    }
    if let Some(spark) = &style.sparkline {
        let text = out.text.unwrap_or(cell);
        let area = if style.hide_text {
            text
        } else {
            let keep = (text.w * 0.4).min(60.0);
            out.text = Some(Rect::new(text.x, text.y, keep, text.h));
            Rect::new(text.x + keep, text.y, text.w - keep, text.h)
        };
        out.over.extend(sparkline_marks(&spark.values, spark.color, spark.bars, area.inset(3.0, 3.0)));
    }
    if style.hide_text {
        out.text = None;
    }
    if let Some(border) = style.border {
        let half = border.width / 2.0;
        out.over.push(Mark::Outline {
            rect: cell.inset(half, half),
            color: border.color,
            width: border.width,
            style: border.style,
        });
    }
    out
}

fn sparkline_marks(values: &[Option<f64>], color: Color, bars: bool, area: Rect) -> Vec<Mark> {
    let known: Vec<f64> = values.iter().flatten().copied().filter(|v| v.is_finite()).collect();
    if known.is_empty() || area.w < 4.0 || area.h < 2.0 {
        return Vec::new();
    }
    let (mut low, mut high) = known.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    if bars {
        low = low.min(0.0);
        high = high.max(0.0);
    }
    let span = if high > low { high - low } else { 1.0 };
    let y = |v: f64| area.bottom() - ((v - low) / span) as f32 * area.h;
    let n = values.len();
    let mut out = Vec::new();
    if bars {
        let slot = area.w / n as f32;
        let zero = y(0.0);
        for (i, v) in values.iter().enumerate() {
            let Some(v) = v.filter(|v| v.is_finite()) else { continue };
            let top = y(v).min(zero);
            let height = (y(v) - zero).abs().max(1.0);
            let shade = if v < 0.0 { Color::rgb(0xe0, 0x38, 0x3e) } else { color };
            out.push(Mark::Fill {
                rect: Rect::new(area.x + slot * i as f32 + slot * 0.15, top, slot * 0.7, height),
                color: shade,
            });
        }
        return out;
    }
    let x = |i: usize| if n > 1 { area.x + area.w * i as f32 / (n - 1) as f32 } else { area.x + area.w / 2.0 };
    // Blanks break the line.
    let mut run: Vec<Point> = Vec::new();
    for (i, v) in values.iter().enumerate() {
        match v.filter(|v| v.is_finite()) {
            Some(v) => run.push((x(i), y(v))),
            None if run.len() > 1 => out.push(Mark::Polyline { points: std::mem::take(&mut run), color, width: 1.5 }),
            None => run.clear(),
        }
    }
    if run.len() > 1 {
        out.push(Mark::Polyline { points: run, color, width: 1.5 });
    }
    if let Some((i, v)) = values.iter().enumerate().rev().find_map(|(i, v)| v.map(|v| (i, v))) {
        out.push(Mark::Circle { center: (x(i), y(v)), radius: 2.0, color });
    }
    out
}

/// Points of a five-pointed star in the unit square.
fn star_points() -> Vec<Point> {
    (0..10)
        .map(|i| {
            let angle = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { 0.5 } else { 0.21 };
            (0.5 + r * angle.cos(), 0.53 + r * angle.sin())
        })
        .collect()
}

/// An icon's marks in `square`.
pub fn icon_marks(shape: Shape, color: Color, square: Rect) -> Vec<Mark> {
    let s = square.w;
    let at = |(x, y): Point| (square.x + x * s, square.y + y * s);
    let poly = |points: &[Point]| Mark::Polygon { points: points.iter().map(|p| at(*p)).collect(), color };
    let faint = Color { a: 70, ..color };
    let white = Color::rgb(255, 255, 255);
    match shape {
        Shape::ArrowUp => vec![poly(&[(0.5, 0.05), (0.95, 0.5), (0.68, 0.5), (0.68, 0.95), (0.32, 0.95), (0.32, 0.5), (0.05, 0.5)])],
        Shape::ArrowDown => vec![poly(&[(0.5, 0.95), (0.95, 0.5), (0.68, 0.5), (0.68, 0.05), (0.32, 0.05), (0.32, 0.5), (0.05, 0.5)])],
        Shape::ArrowRight => vec![poly(&[(0.95, 0.5), (0.5, 0.05), (0.5, 0.32), (0.05, 0.32), (0.05, 0.68), (0.5, 0.68), (0.5, 0.95)])],
        Shape::TriangleUp => vec![poly(&[(0.5, 0.15), (0.95, 0.85), (0.05, 0.85)])],
        Shape::TriangleDown => vec![poly(&[(0.5, 0.85), (0.95, 0.15), (0.05, 0.15)])],
        Shape::Dash => vec![Mark::Fill { rect: Rect::new(square.x + 0.15 * s, square.y + 0.4 * s, 0.7 * s, 0.2 * s), color }],
        Shape::Circle => vec![Mark::Circle { center: at((0.5, 0.5)), radius: 0.45 * s, color }],
        Shape::Check => vec![Mark::Polyline { points: vec![at((0.12, 0.52)), at((0.4, 0.8)), at((0.9, 0.2))], color, width: 0.17 * s }],
        Shape::Cross => vec![
            Mark::Line { from: at((0.18, 0.18)), to: at((0.82, 0.82)), color, width: 0.17 * s, style: LineStyle::Solid },
            Mark::Line { from: at((0.82, 0.18)), to: at((0.18, 0.82)), color, width: 0.17 * s, style: LineStyle::Solid },
        ],
        Shape::Warn => vec![
            poly(&[(0.5, 0.05), (0.97, 0.92), (0.03, 0.92)]),
            Mark::Line { from: at((0.5, 0.36)), to: at((0.5, 0.64)), color: white, width: 0.12 * s, style: LineStyle::Solid },
            Mark::Circle { center: at((0.5, 0.78)), radius: 0.065 * s, color: white },
        ],
        Shape::Star => vec![poly(&star_points())],
        Shape::StarEmpty => {
            let mut points: Vec<Point> = star_points().into_iter().map(at).collect();
            points.push(points[0]);
            vec![Mark::Polygon { points: star_points().into_iter().map(at).collect(), color: faint }, Mark::Polyline { points, color, width: 1.0 }]
        }
        Shape::StarHalf => {
            let half: Vec<Point> = star_points().into_iter().map(|(x, y)| at((x.min(0.5), y))).collect();
            let mut outline: Vec<Point> = star_points().into_iter().map(at).collect();
            outline.push(outline[0]);
            vec![
                Mark::Polygon { points: star_points().into_iter().map(at).collect(), color: faint },
                Mark::Polygon { points: half, color },
                Mark::Polyline { points: outline, color, width: 1.0 },
            ]
        }
        Shape::Flag => vec![
            Mark::Line { from: at((0.2, 0.05)), to: at((0.2, 0.95)), color, width: 0.12 * s, style: LineStyle::Solid },
            poly(&[(0.2, 0.08), (0.9, 0.3), (0.2, 0.55)]),
        ],
        Shape::Square => vec![Mark::Fill { rect: square.inset(0.15 * s, 0.15 * s), color }],
        Shape::Diamond => vec![poly(&[(0.5, 0.05), (0.95, 0.5), (0.5, 0.95), (0.05, 0.5)])],
        Shape::Bars1 | Shape::Bars2 | Shape::Bars3 => {
            let lit = match shape {
                Shape::Bars1 => 1,
                Shape::Bars2 => 2,
                _ => 3,
            };
            (0..3)
                .map(|i| {
                    let height = 0.35 + 0.3 * i as f32;
                    Mark::Fill {
                        rect: Rect::new(square.x + (0.08 + 0.32 * i as f32) * s, square.y + (0.95 - height) * s, 0.24 * s, height * s),
                        color: if i < lit { color } else { faint },
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::{Bar, Icon};

    #[test]
    fn icons_take_room_from_the_text() {
        let style = CellStyle {
            icon: Some(Icon { shape: Shape::ArrowUp, color: Color::rgb(0, 128, 0) }),
            ..Default::default()
        };
        let layout = layout(&style, Rect::new(0.0, 0.0, 100.0, 20.0));
        assert_eq!(layout.text, Some(Rect::new(20.0, 0.0, 80.0, 20.0)));
        assert_eq!(layout.over.len(), 1);
    }

    #[test]
    fn bars_fill_their_share_and_hidden_text_has_no_rect() {
        let style = CellStyle {
            bar: Some(Bar { start: 0.0, end: 0.5, color: Color::rgb(0, 0, 255), gradient: false, axis: None }),
            hide_text: true,
            ..Default::default()
        };
        let layout = layout(&style, Rect::new(10.0, 0.0, 104.0, 20.0));
        let Mark::Fill { rect, .. } = &layout.under[0] else { panic!() };
        assert_eq!((rect.x, rect.w), (12.0, 50.0));
        assert_eq!(layout.text, None);
    }

    #[test]
    fn blanks_break_sparklines() {
        let marks = sparkline_marks(
            &[Some(1.0), Some(2.0), None, Some(3.0), Some(1.0)],
            Color::rgb(0, 0, 0),
            false,
            Rect::new(0.0, 0.0, 40.0, 10.0),
        );
        assert_eq!(marks.iter().filter(|m| matches!(m, Mark::Polyline { .. })).count(), 2);
    }
}
