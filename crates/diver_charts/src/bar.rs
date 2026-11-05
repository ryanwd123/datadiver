//! Bar charts: columns or horizontal bars, one series or several, grouped
//! side by side or stacked.

use crate::format::{TickFormat, compact, trimmed};
use crate::geom::{Color, Rect};
use crate::scale::{Band, Linear, Ticks, nice_ticks};
use crate::scene::{Anchor, Baseline, Corners, Mark, Measure, Scene, Shape, TipRow, Tooltip, fit};
use crate::theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    /// One value per category; `None` draws no bar.
    pub values: Vec<Option<f64>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// Columns rising from a horizontal baseline.
    #[default]
    Vertical,
    /// Bars running right from a vertical baseline, categories top to bottom.
    Horizontal,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarChart {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub categories: Vec<String>,
    pub series: Vec<Series>,
    pub orientation: Orientation,
    pub stacked: bool,
    /// Values at the bar ends (stack totals when stacked), where they fit.
    pub value_labels: bool,
    pub category_title: Option<String>,
    pub value_title: Option<String>,
}

impl BarChart {
    pub fn new<S: Into<String>>(categories: impl IntoIterator<Item = S>) -> BarChart {
        BarChart { categories: categories.into_iter().map(Into::into).collect(), ..BarChart::default() }
    }

    pub fn series(self, name: impl Into<String>, values: impl IntoIterator<Item = f64>) -> BarChart {
        self.series_opt(name, values.into_iter().map(Some))
    }

    pub fn series_opt(mut self, name: impl Into<String>, values: impl IntoIterator<Item = Option<f64>>) -> BarChart {
        self.series.push(Series { name: name.into(), values: values.into_iter().collect() });
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> BarChart {
        self.title = Some(title.into());
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<String>) -> BarChart {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn horizontal(mut self) -> BarChart {
        self.orientation = Orientation::Horizontal;
        self
    }

    pub fn stacked(mut self) -> BarChart {
        self.stacked = true;
        self
    }

    pub fn value_labels(mut self) -> BarChart {
        self.value_labels = true;
        self
    }

    pub fn category_title(mut self, title: impl Into<String>) -> BarChart {
        self.category_title = Some(title.into());
        self
    }

    pub fn value_title(mut self, title: impl Into<String>) -> BarChart {
        self.value_title = Some(title.into());
        self
    }

    /// The series as drawn: past the palette's eight, the rest fold into
    /// one "Other" series of their sums.
    fn drawn_series(&self, theme: &Theme) -> Vec<(Series, Color)> {
        let n = theme.palette.len();
        let mut out: Vec<(Series, Color)> =
            self.series.iter().take(n).enumerate().map(|(i, s)| (s.clone(), theme.series(i))).collect();
        if self.series.len() > n {
            let values = (0..self.categories.len())
                .map(|c| {
                    let parts: Vec<f64> = self.series[n..].iter().filter_map(|s| value(s, c)).collect();
                    (!parts.is_empty()).then(|| parts.iter().sum())
                })
                .collect();
            out.push((Series { name: "Other".into(), values }, theme.other));
        }
        out
    }

    pub fn layout(&self, width: f32, height: f32, theme: &Theme, measure: &dyn Measure) -> Scene {
        let mut s = Scene::new(width, height);
        s.rect(Rect::new(0.0, 0.0, width, height), theme.surface);
        let pad = theme.pad;
        let mut top = pad;

        if let Some(title) = &self.title {
            let text = fit(title, width - 2.0 * pad, theme.title, measure);
            s.text(text, (pad, top), Anchor::Start, Baseline::Top, theme.title, theme.text);
            top += theme.title.line();
        }
        if let Some(subtitle) = &self.subtitle {
            let text = fit(subtitle, width - 2.0 * pad, theme.subtitle, measure);
            s.text(text, (pad, top + 2.0), Anchor::Start, Baseline::Top, theme.subtitle, theme.text2);
            top += theme.subtitle.line() + 2.0;
        }
        if self.title.is_some() || self.subtitle.is_some() {
            top += 10.0;
        }

        let series = self.drawn_series(theme);
        if series.len() >= 2 {
            top = legend(&mut s, &series, top, theme, measure) + 6.0;
        }

        let has_values = series.iter().any(|(se, _)| (0..self.categories.len()).any(|c| value(se, c).is_some()));
        if self.categories.is_empty() || !has_values {
            let at = (width / 2.0, (top + height - pad) / 2.0);
            s.text("No data", at, Anchor::Middle, Baseline::Middle, theme.axis_title, theme.muted);
            return s;
        }

        let bars = Bars { chart: self, series: &series, theme, measure };
        let frame = Rect::new(pad, top, width - 2.0 * pad, height - pad - top);
        match self.orientation {
            Orientation::Vertical => bars.vertical(&mut s, frame),
            Orientation::Horizontal => bars.horizontal(&mut s, frame),
        }
        s
    }
}

fn value(series: &Series, c: usize) -> Option<f64> {
    series.values.get(c).copied().flatten().filter(|v| v.is_finite())
}

/// Draws the legend in rows from `top`, and returns where it ends.
fn legend(s: &mut Scene, series: &[(Series, Color)], top: f32, theme: &Theme, measure: &dyn Measure) -> f32 {
    let font = theme.legend;
    let row_h = font.line() + 4.0;
    let (left, right) = (theme.pad, s.width - theme.pad);
    let (mut x, mut y) = (left, top);
    for (se, color) in series {
        let name = fit(&se.name, (right - left - 16.0).max(20.0), font, measure);
        let w = 10.0 + 6.0 + measure.width(&name, font);
        if x > left && x + w > right {
            x = left;
            y += row_h;
        }
        let cy = y + row_h / 2.0;
        let swatch = Rect::new(x, cy - 5.0, 10.0, 10.0);
        s.push(Shape::Rect { rect: swatch, color: *color, corners: Corners::all(2.0), stroke: None });
        s.text(name, (x + 16.0, cy), Anchor::Start, Baseline::Middle, font, theme.text2);
        x += w + 18.0;
    }
    y + row_h
}

/// One bar or stacked segment, along the category axis and the value axis.
struct Bar {
    series: usize,
    category: usize,
    /// Where it starts across the category axis, and its thickness.
    at: f32,
    thick: f32,
    /// Its ends on the value axis, in pixels: `base` on the baseline side.
    base: f32,
    end: f32,
    /// Draws the rounded data end.
    rounded: bool,
    negative: bool,
}

struct Bars<'a> {
    chart: &'a BarChart,
    series: &'a [(Series, Color)],
    theme: &'a Theme,
    measure: &'a dyn Measure,
}

impl Bars<'_> {
    fn n_cat(&self) -> usize {
        self.chart.categories.len()
    }

    /// The value range bars reach, always including zero.
    fn domain(&self) -> (f64, f64) {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for c in 0..self.n_cat() {
            let values = self.series.iter().filter_map(|(se, _)| value(se, c));
            if self.chart.stacked {
                let (pos, neg) = values.fold((0.0, 0.0), |(p, n), v| if v >= 0.0 { (p + v, n) } else { (p, n + v) });
                hi = hi.max(pos);
                lo = lo.min(neg);
            } else {
                for v in values {
                    hi = hi.max(v);
                    lo = lo.min(v);
                }
            }
        }
        (lo, hi)
    }

    fn ticks(&self, length: f32, per_tick: f32) -> Ticks {
        let count = (length / per_tick).round().clamp(2.0, 8.0) as usize;
        let (lo, hi) = self.domain();
        nice_ticks(lo, hi, count)
    }

    fn label_text(&self, v: f64) -> String {
        compact(v)
    }

    /// The bars across category band `band`, with values mapped by `scale`.
    fn bars(&self, band: &Band, scale: &Linear) -> Vec<Bar> {
        let gap = self.theme.gap;
        let n_s = self.series.len();
        let mut out = Vec::new();
        for c in 0..self.n_cat() {
            if self.chart.stacked {
                let thick = band.width.min(self.theme.bar_max).max(1.0);
                let at = band.center(c) - thick / 2.0;
                let values: Vec<Option<f64>> = self.series.iter().map(|(se, _)| value(se, c)).collect();
                let last_pos = values.iter().rposition(|v| v.is_some_and(|v| v > 0.0));
                let last_neg = values.iter().rposition(|v| v.is_some_and(|v| v < 0.0));
                let (mut pos, mut neg) = (0.0, 0.0);
                let (mut first_pos, mut first_neg) = (true, true);
                for (i, v) in values.iter().enumerate() {
                    let Some(v) = *v else { continue };
                    let negative = v < 0.0;
                    let (acc, first, last) = if negative {
                        (&mut neg, &mut first_neg, last_neg)
                    } else {
                        (&mut pos, &mut first_pos, last_pos)
                    };
                    let mut base = scale.map(*acc);
                    *acc += v;
                    let end = scale.map(*acc);
                    // The surface gap separates a segment from the one below.
                    if !*first && v != 0.0 {
                        base += gap.min((end - base).abs()) * (end - base).signum();
                    }
                    *first = false;
                    out.push(Bar { series: i, category: c, at, thick, base, end, rounded: last == Some(i), negative });
                }
            } else {
                let thick = ((band.width - gap * (n_s as f32 - 1.0)) / n_s as f32).min(self.theme.bar_max).max(1.0);
                let group = n_s as f32 * thick + (n_s as f32 - 1.0) * gap;
                let start = band.at(c) + (band.width - group) / 2.0;
                for (i, (se, _)) in self.series.iter().enumerate() {
                    let Some(v) = value(se, c) else { continue };
                    out.push(Bar {
                        series: i,
                        category: c,
                        at: start + i as f32 * (thick + gap),
                        thick,
                        base: scale.map(0.0),
                        end: scale.map(v),
                        rounded: true,
                        negative: v < 0.0,
                    });
                }
            }
        }
        out
    }

    fn tooltip(&self, c: usize, hovered: usize) -> Tooltip {
        let row = |i: usize| {
            let (se, color) = &self.series[i];
            TipRow {
                color: Some(*color),
                label: se.name.clone(),
                value: value(se, c).map_or("—".into(), |v| trimmed(v, 4)),
                strong: i == hovered && self.series.len() > 1,
            }
        };
        let mut rows: Vec<TipRow> = if self.chart.stacked {
            // Top of the stack first, as the reader sees it.
            (0..self.series.len()).rev().map(row).collect()
        } else {
            (0..self.series.len()).map(row).collect()
        };
        if self.chart.stacked && self.series.len() > 1 {
            let total: f64 = self.series.iter().filter_map(|(se, _)| value(se, c)).sum();
            rows.push(TipRow { color: None, label: "Total".into(), value: trimmed(total, 4), strong: false });
        }
        Tooltip { title: self.chart.categories[c].clone(), rows }
    }

    /// Draws the bars and records their marks. `rect` turns (category start,
    /// thickness, value start, value end) into a rectangle.
    fn draw_bars(&self, s: &mut Scene, band: &Band, scale: &Linear, vertical: bool, plot: Rect) {
        let theme = self.theme;
        let rect = |at: f32, thick: f32, a: f32, b: f32| {
            if vertical { Rect::from_corners((at, a), (at + thick, b)) } else { Rect::from_corners((a, at), (b, at + thick)) }
        };
        let bars = self.bars(band, scale);
        let n_s = self.series.len() as f32;
        for bar in &bars {
            let r = rect(bar.at, bar.thick, bar.base, bar.end);
            let length = (bar.end - bar.base).abs();
            // The pointer finds a bar across its whole slot, and along the
            // full plot unless it is a stacked segment.
            let slot = if self.chart.stacked { band.step } else { (bar.thick + theme.gap).max(band.step / n_s) };
            let mid = bar.at + bar.thick / 2.0;
            let area = if self.chart.stacked {
                let grown = rect(mid - slot / 2.0, slot, bar.base, bar.end);
                if vertical { grown.inset(0.0, -theme.gap, 0.0, -theme.gap) } else { grown.inset(-theme.gap, 0.0, -theme.gap, 0.0) }
            } else if vertical {
                Rect::new(mid - slot / 2.0, plot.y, slot, plot.h)
            } else {
                Rect::new(plot.x, mid - slot / 2.0, plot.w, slot)
            };
            let mark = s.marks.len();
            s.marks.push(Mark { area, rect: r, series: bar.series, index: bar.category, tip: self.tooltip(bar.category, bar.series) });
            if length < 0.5 {
                continue;
            }
            let radius = if bar.rounded { theme.radius.min(bar.thick / 2.0).min(length) } else { 0.0 };
            let corners = match (vertical, bar.negative) {
                (true, false) => Corners { top_left: radius, top_right: radius, ..Corners::default() },
                (true, true) => Corners { bottom_left: radius, bottom_right: radius, ..Corners::default() },
                (false, false) => Corners { top_right: radius, bottom_right: radius, ..Corners::default() },
                (false, true) => Corners { top_left: radius, bottom_left: radius, ..Corners::default() },
            };
            let color = self.series[bar.series].1;
            s.push_mark(Shape::Rect { rect: r, color, corners, stroke: None }, mark);
        }
        if self.chart.value_labels {
            self.draw_value_labels(s, band, scale, vertical, &bars);
        }
    }

    fn draw_value_labels(&self, s: &mut Scene, band: &Band, scale: &Linear, vertical: bool, bars: &[Bar]) {
        let theme = self.theme;
        let font = theme.label;
        // (category-axis center, room across, value, tip pixel, negative)
        let mut labels: Vec<(f32, f32, f64, f32, bool)> = Vec::new();
        if self.chart.stacked {
            for c in 0..self.n_cat() {
                let values: Vec<f64> = self.series.iter().filter_map(|(se, _)| value(se, c)).collect();
                if values.is_empty() {
                    continue;
                }
                let pos: f64 = values.iter().filter(|v| **v >= 0.0).sum();
                let neg: f64 = values.iter().filter(|v| **v < 0.0).sum();
                let (total, tip) = if pos > 0.0 || neg == 0.0 { (pos + neg, pos) } else { (neg, neg) };
                labels.push((band.center(c), band.step, total, scale.map(tip), tip < 0.0));
            }
        } else {
            for bar in bars {
                let v = value(&self.series[bar.series].0, bar.category).unwrap_or(0.0);
                labels.push((bar.at + bar.thick / 2.0, bar.thick + theme.gap * 2.0, v, bar.end, bar.negative));
            }
        }
        for (mid, room, v, tip, negative) in labels {
            let text = self.label_text(v);
            if vertical {
                if self.measure.width(&text, font) > room + 4.0 {
                    continue;
                }
                let (y, baseline) = if negative { (tip + 4.0, Baseline::Top) } else { (tip - 4.0, Baseline::Bottom) };
                s.text(text, (mid, y), Anchor::Middle, baseline, font, theme.text2);
            } else {
                if font.line() > room + 4.0 {
                    continue;
                }
                let (x, anchor) = if negative { (tip - 5.0, Anchor::End) } else { (tip + 5.0, Anchor::Start) };
                s.text(text, (x, mid), anchor, Baseline::Middle, font, theme.text2);
            }
        }
    }

    /// The widest value label, for leaving room at the bar ends.
    fn widest_value_label(&self) -> f32 {
        if !self.chart.value_labels {
            return 0.0;
        }
        let (lo, hi) = self.domain();
        [lo, hi].iter().map(|v| self.measure.width(&self.label_text(*v), self.theme.label)).fold(0.0, f32::max)
    }

    fn vertical(&self, s: &mut Scene, frame: Rect) {
        let theme = self.theme;
        let tick = theme.tick;
        let mut left = frame.x;
        let mut bottom = frame.bottom();
        if self.chart.value_title.is_some() {
            left += theme.axis_title.line() + 6.0;
        }
        if self.chart.category_title.is_some() {
            bottom -= theme.axis_title.line() + 6.0;
        }
        let headroom = if self.chart.value_labels { theme.label.line() + 4.0 } else { tick.line() / 2.0 };

        // The tick count depends on the plot's height, which the category
        // labels below it shape; estimate with one line of them.
        let ticks = self.ticks(bottom - frame.y - tick.line() * 2.0, 56.0);
        let fmt = TickFormat::new(&ticks);
        let tick_labels: Vec<String> = ticks.values.iter().map(|v| fmt.format(*v)).collect();
        let tick_w = tick_labels.iter().map(|t| self.measure.width(t, tick)).fold(0.0, f32::max);
        let plot_x = left + tick_w + 8.0;
        let plot_r = frame.right();
        let band = Band::new(self.n_cat(), (plot_x, plot_r), 0.2, 0.1);

        // Category labels: level when they fit their slot, else turned 45°
        // and, when even that crowds, every few.
        let max_label = ((bottom - frame.y) * 0.3).clamp(40.0, 160.0);
        let names: Vec<String> = self.chart.categories.iter().map(|c| fit(c, max_label, tick, self.measure)).collect();
        let widest = names.iter().map(|n| self.measure.width(n, tick)).fold(0.0, f32::max);
        let level = widest <= band.step - 6.0;
        let diag = std::f32::consts::FRAC_1_SQRT_2;
        let (labels_h, every) = if level {
            (tick.line(), 1)
        } else {
            ((widest + tick.line()) * diag, (tick.line() / (band.step * diag)).ceil().max(1.0) as usize)
        };
        let plot_y = frame.y + headroom;
        let plot_b = (bottom - labels_h - 6.0).max(plot_y + 20.0);
        let plot = Rect::new(plot_x, plot_y, plot_r - plot_x, plot_b - plot_y);
        let scale = Linear::new((ticks.min, ticks.max), (plot.bottom(), plot.y));

        for (v, label) in ticks.values.iter().zip(&tick_labels) {
            let y = scale.map(*v).round() + 0.5;
            s.line((plot.x, y), (plot.right(), y), theme.grid, 1.0);
            s.text(label, (plot.x - 8.0, y), Anchor::End, Baseline::Middle, tick, theme.muted);
        }
        self.draw_bars(s, &band, &scale, true, plot);
        let zero = scale.map(0.0).round() + 0.5;
        s.line((plot.x, zero), (plot.right(), zero), theme.axis, 1.0);

        for (c, name) in names.iter().enumerate().step_by(every) {
            let x = band.center(c);
            if level {
                s.text(name, (x, plot.bottom() + 6.0), Anchor::Middle, Baseline::Top, tick, theme.muted);
            } else {
                let at = (x + 3.0, plot.bottom() + 8.0);
                s.push(Shape::Text {
                    text: name.clone(),
                    at,
                    anchor: Anchor::End,
                    baseline: Baseline::Middle,
                    font: tick,
                    color: theme.muted,
                    angle: -45.0,
                });
            }
        }
        if let Some(title) = &self.chart.category_title {
            let at = (plot.x + plot.w / 2.0, frame.bottom());
            s.text(title, at, Anchor::Middle, Baseline::Bottom, theme.axis_title, theme.muted);
        }
        if let Some(title) = &self.chart.value_title {
            let text = fit(title, plot.h, theme.axis_title, self.measure);
            let at = (frame.x + theme.axis_title.line() / 2.0, plot.y + plot.h / 2.0);
            s.push(Shape::Text {
                text,
                at,
                anchor: Anchor::Middle,
                baseline: Baseline::Middle,
                font: theme.axis_title,
                color: theme.muted,
                angle: -90.0,
            });
        }
    }

    fn horizontal(&self, s: &mut Scene, frame: Rect) {
        let theme = self.theme;
        let tick = theme.tick;
        let mut left = frame.x;
        let mut bottom = frame.bottom();
        if self.chart.category_title.is_some() {
            left += theme.axis_title.line() + 6.0;
        }
        if self.chart.value_title.is_some() {
            bottom -= theme.axis_title.line() + 6.0;
        }
        let max_label = (frame.w * 0.3).clamp(40.0, 200.0);
        let names: Vec<String> = self.chart.categories.iter().map(|c| fit(c, max_label, tick, self.measure)).collect();
        let widest = names.iter().map(|n| self.measure.width(n, tick)).fold(0.0, f32::max);
        let plot_x = left + widest + 10.0;
        let plot_r = (frame.right() - self.widest_value_label() - 6.0).max(plot_x + 20.0);
        let plot_b = (bottom - tick.line() - 6.0).max(frame.y + 20.0);
        let plot = Rect::new(plot_x, frame.y, plot_r - plot_x, plot_b - frame.y);

        // Fewer ticks until their labels sit apart.
        let mut per_tick = 90.0;
        let (ticks, fmt, tick_labels) = loop {
            let ticks = self.ticks(plot.w, per_tick);
            let fmt = TickFormat::new(&ticks);
            let labels: Vec<String> = ticks.values.iter().map(|v| fmt.format(*v)).collect();
            let widest = labels.iter().map(|t| self.measure.width(t, tick)).fold(0.0, f32::max);
            let spacing = plot.w / (labels.len().max(2) - 1) as f32;
            if widest + 12.0 <= spacing || ticks.values.len() <= 3 {
                break (ticks, fmt, labels);
            }
            per_tick *= 1.4;
        };
        let _ = fmt;
        let scale = Linear::new((ticks.min, ticks.max), (plot.x, plot.right()));
        let band = Band::new(self.n_cat(), (plot.y, plot.bottom()), 0.2, 0.1);

        for (v, label) in ticks.values.iter().zip(&tick_labels) {
            let x = scale.map(*v).round() + 0.5;
            s.line((x, plot.y), (x, plot.bottom()), theme.grid, 1.0);
            s.text(label, (x, plot.bottom() + 6.0), Anchor::Middle, Baseline::Top, tick, theme.muted);
        }
        self.draw_bars(s, &band, &scale, false, plot);
        let zero = scale.map(0.0).round() + 0.5;
        s.line((zero, plot.y), (zero, plot.bottom()), theme.axis, 1.0);

        let every = (tick.line() / band.step).ceil().max(1.0) as usize;
        for (c, name) in names.iter().enumerate().step_by(every) {
            s.text(name, (plot.x - 10.0, band.center(c)), Anchor::End, Baseline::Middle, tick, theme.muted);
        }
        if let Some(title) = &self.chart.value_title {
            let at = (plot.x + plot.w / 2.0, frame.bottom());
            s.text(title, at, Anchor::Middle, Baseline::Bottom, theme.axis_title, theme.muted);
        }
        if let Some(title) = &self.chart.category_title {
            let text = fit(title, plot.h, theme.axis_title, self.measure);
            let at = (frame.x + theme.axis_title.line() / 2.0, plot.y + plot.h / 2.0);
            s.push(Shape::Text {
                text,
                at,
                anchor: Anchor::Middle,
                baseline: Baseline::Middle,
                font: theme.axis_title,
                color: theme.muted,
                angle: -90.0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Approx;

    fn rects(scene: &Scene) -> Vec<Rect> {
        scene
            .items
            .iter()
            .filter(|i| i.mark.is_some())
            .filter_map(|i| match i.shape {
                Shape::Rect { rect, .. } => Some(rect),
                _ => None,
            })
            .collect()
    }

    fn texts(scene: &Scene) -> Vec<&str> {
        scene
            .items
            .iter()
            .filter_map(|i| match &i.shape {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn one_bar_per_value() {
        let chart = BarChart::new(["a", "b", "c"]).series("x", [1.0, 2.0, 3.0]);
        let scene = chart.layout(600.0, 400.0, &Theme::light(), &Approx);
        let bars = rects(&scene);
        assert_eq!(bars.len(), 3);
        assert_eq!(scene.marks.len(), 3);
        // Taller for bigger values, on one baseline, capped in thickness.
        assert!(bars[0].h < bars[1].h && bars[1].h < bars[2].h);
        assert!((bars[0].bottom() - bars[2].bottom()).abs() < 0.01);
        assert!(bars.iter().all(|b| b.w <= Theme::light().bar_max + 0.01), "{bars:?}");
    }

    #[test]
    fn missing_values_draw_nothing() {
        let chart = BarChart::new(["a", "b"]).series_opt("x", [Some(1.0), None]);
        let scene = chart.layout(600.0, 400.0, &Theme::light(), &Approx);
        assert_eq!(rects(&scene).len(), 1);
    }

    #[test]
    fn no_data_says_so() {
        let scene = BarChart::new(Vec::<String>::new()).layout(400.0, 300.0, &Theme::light(), &Approx);
        assert!(texts(&scene).contains(&"No data"));
        let scene = BarChart::new(["a"]).series_opt("x", [None]).layout(400.0, 300.0, &Theme::light(), &Approx);
        assert!(texts(&scene).contains(&"No data"));
    }

    #[test]
    fn stacks_sit_on_each_other_with_a_gap() {
        let chart = BarChart::new(["a"]).series("x", [10.0]).series("y", [10.0]).stacked();
        let theme = Theme::light();
        let scene = chart.layout(400.0, 400.0, &theme, &Approx);
        let bars = rects(&scene);
        assert_eq!(bars.len(), 2);
        let (low, high) = (bars[0], bars[1]);
        assert!(high.bottom() <= low.y);
        assert!((low.y - high.bottom() - theme.gap).abs() < 0.01);
        assert_eq!(low.x, high.x);
    }

    #[test]
    fn only_the_stack_top_is_rounded() {
        let chart = BarChart::new(["a"]).series("x", [10.0]).series("y", [10.0]).stacked();
        let scene = chart.layout(400.0, 400.0, &Theme::light(), &Approx);
        let corners: Vec<Corners> = scene
            .items
            .iter()
            .filter_map(|i| match (i.mark, &i.shape) {
                (Some(_), Shape::Rect { corners, .. }) => Some(*corners),
                _ => None,
            })
            .collect();
        assert_eq!(corners[0], Corners::default());
        assert!(corners[1].top_left > 0.0 && corners[1].bottom_left == 0.0);
    }

    #[test]
    fn negative_bars_hang_below_zero() {
        let chart = BarChart::new(["a", "b"]).series("x", [5.0, -5.0]);
        let scene = chart.layout(400.0, 400.0, &Theme::light(), &Approx);
        let bars = rects(&scene);
        assert!((bars[0].bottom() - bars[1].y).abs() < 0.01);
    }

    #[test]
    fn grouped_bars_sit_side_by_side() {
        let chart = BarChart::new(["a"]).series("x", [1.0]).series("y", [2.0]);
        let theme = Theme::light();
        let scene = chart.layout(400.0, 400.0, &theme, &Approx);
        let bars = rects(&scene);
        assert!((bars[1].x - bars[0].right() - theme.gap).abs() < 0.01);
        // A legend names both.
        assert!(texts(&scene).contains(&"x") && texts(&scene).contains(&"y"));
    }

    #[test]
    fn a_single_series_has_no_legend() {
        let chart = BarChart::new(["a"]).series("only", [1.0]);
        let scene = chart.layout(400.0, 400.0, &Theme::light(), &Approx);
        assert!(!texts(&scene).contains(&"only"));
    }

    #[test]
    fn extra_series_fold_into_other() {
        let mut chart = BarChart::new(["a"]);
        for i in 0..10 {
            chart = chart.series(format!("s{i}"), [1.0]);
        }
        let scene = chart.stacked().layout(800.0, 400.0, &Theme::light(), &Approx);
        assert_eq!(rects(&scene).len(), 9);
        assert!(texts(&scene).contains(&"Other"));
        let tip = &scene.marks[8].tip;
        assert_eq!(tip.rows[0].label, "Other");
        assert_eq!(tip.rows[0].value, "2");
        assert_eq!(tip.rows.last().unwrap().value, "10");
    }

    #[test]
    fn horizontal_bars_run_right() {
        let chart = BarChart::new(["a", "b"]).series("x", [1.0, 2.0]).horizontal();
        let scene = chart.layout(600.0, 300.0, &Theme::light(), &Approx);
        let bars = rects(&scene);
        assert!(bars[0].w < bars[1].w);
        assert!((bars[0].x - bars[1].x).abs() < 0.01);
        assert!(bars[0].y < bars[1].y);
    }

    #[test]
    fn the_pointer_finds_a_bar_above_it() {
        let chart = BarChart::new(["a", "b"]).series("x", [1.0, 10.0]);
        let scene = chart.layout(400.0, 400.0, &Theme::light(), &Approx);
        let small = rects(&scene)[0];
        // Well above the short bar still hits it.
        let hit = scene.hit((small.x + small.w / 2.0, small.y - 100.0));
        assert_eq!(hit, Some(0));
        assert_eq!(scene.marks[0].tip.title, "a");
    }

    #[test]
    fn value_labels_print_compact() {
        let chart = BarChart::new(["a", "b"]).series("x", [12_900.0, 4_200_000.0]).value_labels();
        let scene = chart.layout(600.0, 400.0, &Theme::light(), &Approx);
        let texts = texts(&scene);
        assert!(texts.contains(&"12.9K") && texts.contains(&"4.2M"));
    }
}
