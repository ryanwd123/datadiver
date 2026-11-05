//! A color picker drawn in the app's own style: a saturation and brightness
//! field over a hue bar, then groups of swatches. Hosts keep the text input
//! for the hex value and decide what a new color changes.

use crate::ui::hit::Hit;
use crate::ui::platform::{Align, Canvas, Color, Cursor, Font, Point, Rect, point, rect, text};
use crate::ui::widgets::{WidgetId, colors};

const HUE_HEIGHT: f32 = 14.0;
const GAP: f32 = 10.0;
const SWATCH: f32 = 22.0;
const SWATCH_GAP: f32 = 6.0;

/// What a click on the picker lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Saturation across, brightness down.
    Field,
    Hue,
    /// A swatch, counted across every group.
    Swatch(usize),
}

/// A titled row of swatches, each a tooltip and `0xRRGGBB`.
pub struct Swatches<'a> {
    pub title: &'a str,
    pub colors: &'a [(String, u32)],
}

pub struct ColorPicker {
    pub id: WidgetId,
    /// Hue in degrees, saturation and brightness from 0 to 1.
    hue: f32,
    saturation: f32,
    value: f32,
    /// The color as set, so it comes back exactly rather than rounded
    /// through hue, saturation and brightness.
    exact: Option<u32>,
    drag: Option<Part>,
    field: Rect,
    hue_bar: Rect,
    /// The swatches last painted, in [`Part::Swatch`] order.
    swatches: Vec<u32>,
}

impl ColorPicker {
    pub fn new(rgb: u32) -> Self {
        let mut picker = ColorPicker {
            id: WidgetId::new(),
            hue: 0.0,
            saturation: 0.0,
            value: 0.0,
            exact: None,
            drag: None,
            field: Rect::default(),
            hue_bar: Rect::default(),
            swatches: Vec::new(),
        };
        picker.set_rgb(rgb);
        picker
    }

    /// The color as `0xRRGGBB`.
    pub fn rgb(&self) -> u32 {
        self.exact.unwrap_or_else(|| hsv_to_rgb(self.hue, self.saturation, self.value))
    }

    /// The color as `#rrggbb`.
    pub fn hex(&self) -> String {
        format!("#{:06x}", self.rgb())
    }

    /// Shows `rgb`. Grays keep the hue they had, so dragging on from one
    /// doesn't jump to red. Returns whether the color changed.
    pub fn set_rgb(&mut self, rgb: u32) -> bool {
        let rgb = rgb & 0xff_ffff;
        if self.exact == Some(rgb) {
            return false;
        }
        let changed = self.rgb() != rgb;
        let (hue, saturation, value) = rgb_to_hsv(rgb);
        if saturation > 0.0 && value > 0.0 {
            self.hue = hue;
        }
        if value > 0.0 {
            self.saturation = saturation;
        }
        self.value = value;
        self.exact = Some(rgb);
        changed
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A press on `part`. Returns whether the color changed.
    pub fn mouse_down(&mut self, part: Part, pos: Point) -> bool {
        match part {
            Part::Field | Part::Hue => {
                self.drag = Some(part);
                self.drag_to(pos)
            }
            Part::Swatch(index) => self.swatches.get(index).copied().is_some_and(|rgb| self.set_rgb(rgb)),
        }
    }

    /// The pointer moved with the button down. Returns whether the color changed.
    pub fn mouse_drag(&mut self, pos: Point) -> bool {
        self.drag.is_some() && self.drag_to(pos)
    }

    pub fn mouse_up(&mut self) {
        self.drag = None;
    }

    /// Where the field and hue bar are, as a frame would place them.
    #[cfg(test)]
    pub fn lay_out(&mut self, field: Rect, hue_bar: Rect) {
        self.field = field;
        self.hue_bar = hue_bar;
    }

    fn drag_to(&mut self, pos: Point) -> bool {
        let before = self.rgb();
        let fraction = |at: f32, from: f32, length: f32| ((at - from) / length.max(1.0)).clamp(0.0, 1.0);
        match self.drag {
            Some(Part::Field) => {
                self.saturation = fraction(pos.x, self.field.x, self.field.w);
                self.value = 1.0 - fraction(pos.y, self.field.y, self.field.h);
            }
            Some(Part::Hue) => self.hue = fraction(pos.x, self.hue_bar.x, self.hue_bar.w) * 360.0,
            _ => return false,
        }
        self.exact = None;
        self.rgb() != before
    }

    /// How tall the picker paints at `width` with `groups`.
    pub fn height(width: f32, groups: &[Swatches], heading: &Font) -> f32 {
        let mut height = field_height(width) + GAP + HUE_HEIGHT;
        for group in groups.iter().filter(|group| !group.colors.is_empty()) {
            let rows = group.colors.len().div_ceil(per_row(width)) as f32;
            height += GAP + text::line_height(heading) + 4.0 + rows * (SWATCH + SWATCH_GAP) - SWATCH_GAP;
        }
        height
    }

    pub fn paint(&mut self, canvas: &mut Canvas<Hit>, r: Rect, groups: &[Swatches], heading: &Font) {
        let id = self.id;
        self.field = rect(r.x, r.y, r.w, field_height(r.w));
        let field = self.field;
        canvas.fill(field, rgb_color(hsv_to_rgb(self.hue, 1.0, 1.0)));
        canvas.fill_gradient(field, &[(0.0, Color::rgb(0xffffff)), (1.0, Color::rgba(0xffffff00))], false);
        canvas.fill_gradient(field, &[(0.0, Color::rgba(0x00000000)), (1.0, Color::rgb(0x000000))], true);
        canvas.stroke(field, colors::BORDER, 1.0);
        let at = point(field.x + self.saturation * field.w, field.y + (1.0 - self.value) * field.h);
        canvas.fill_circle(at, 7.0, Color::rgb(0xffffff));
        canvas.fill_circle(at, 5.0, rgb_color(self.rgb()));
        canvas.hit_cursor(field.inset(-4.0), Hit::ColorPicker(id, Part::Field), Cursor::Hand);

        self.hue_bar = rect(r.x, field.bottom() + GAP, r.w, HUE_HEIGHT);
        let bar = self.hue_bar;
        let stops: Vec<(f32, Color)> =
            (0..=6).map(|i| (i as f32 / 6.0, rgb_color(hsv_to_rgb(i as f32 * 60.0, 1.0, 1.0)))).collect();
        canvas.fill_gradient(bar, &stops, false);
        canvas.stroke(bar, colors::BORDER, 1.0);
        let x = bar.x + self.hue / 360.0 * bar.w;
        let knob = rect(x - 3.0, bar.y - 3.0, 6.0, bar.h + 6.0);
        canvas.fill_rounded(knob, 2.0, Color::rgb(0xffffff));
        canvas.stroke_rounded(knob, 2.0, colors::FOREGROUND, 1.0);
        canvas.hit_cursor(bar.inset_xy(-4.0, -4.0), Hit::ColorPicker(id, Part::Hue), Cursor::Hand);

        self.swatches.clear();
        let current = self.rgb();
        let line = text::line_height(heading);
        let per_row = per_row(r.w);
        let mut y = bar.bottom();
        for group in groups.iter().filter(|group| !group.colors.is_empty()) {
            y += GAP;
            canvas.text(rect(r.x, y, r.w, line), group.title, heading, colors::MUTED, Align::Left);
            y += line + 4.0;
            for (n, (tip, rgb)) in group.colors.iter().enumerate() {
                let swatch = rect(
                    r.x + (n % per_row) as f32 * (SWATCH + SWATCH_GAP),
                    y + (n / per_row) as f32 * (SWATCH + SWATCH_GAP),
                    SWATCH,
                    SWATCH,
                );
                canvas.fill_rounded(swatch, 4.0, rgb_color(*rgb));
                let (border, width) = if *rgb == current {
                    (colors::SELECTED_STRONG, 2.0)
                } else if canvas.hovered(swatch) {
                    (colors::FOREGROUND, 1.0)
                } else {
                    (colors::INPUT_BORDER, 1.0)
                };
                canvas.stroke_rounded(swatch, 4.0, border, width);
                let part = Part::Swatch(self.swatches.len());
                canvas.hit_with(swatch, Hit::ColorPicker(id, part), Cursor::Hand, Some(tip));
                self.swatches.push(*rgb);
            }
            let rows = group.colors.len().div_ceil(per_row) as f32;
            y += rows * (SWATCH + SWATCH_GAP) - SWATCH_GAP;
        }
    }
}

fn field_height(width: f32) -> f32 {
    (width * 0.62).clamp(80.0, 180.0).round()
}

fn per_row(width: f32) -> usize {
    (((width + SWATCH_GAP) / (SWATCH + SWATCH_GAP)).floor() as usize).max(1)
}

pub fn rgb_color(rgb: u32) -> Color {
    Color::rgb(rgb & 0xff_ffff)
}

/// `0xRRGGBB` from hue in degrees and saturation and brightness from 0 to 1.
pub fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> u32 {
    let h = (hue.rem_euclid(360.0)) / 60.0;
    let c = value * saturation;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = value - c;
    let byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// Hue in degrees and saturation and brightness from 0 to 1.
pub fn rgb_to_hsv(rgb: u32) -> (f32, f32, f32) {
    let [r, g, b] = [rgb >> 16, rgb >> 8, rgb].map(|v| (v & 0xff) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_survive_hue_saturation_and_brightness() {
        for rgb in [0x000000, 0xffffff, 0xff0000, 0x00ff00, 0x0000ff, 0x638ec6, 0xffc7ce, 0x9c0006, 0x808080, 0x123456] {
            let (h, s, v) = rgb_to_hsv(rgb);
            assert_eq!(hsv_to_rgb(h, s, v), rgb, "{rgb:06x}");
        }
    }

    #[test]
    fn dragging_moves_through_the_field_and_hue_and_grays_keep_their_hue() {
        let mut picker = ColorPicker::new(0xff0000);
        picker.field = rect(0.0, 0.0, 100.0, 100.0);
        picker.hue_bar = rect(0.0, 110.0, 100.0, 14.0);
        // The top-right corner is the hue itself; the bottom is black.
        assert!(!picker.mouse_down(Part::Field, point(100.0, 0.0)));
        assert!(picker.mouse_drag(point(100.0, 100.0)));
        assert_eq!(picker.rgb(), 0x000000);
        picker.mouse_up();
        assert!(!picker.mouse_drag(point(50.0, 50.0)));
        // A third of the way along the hue bar is green.
        picker.mouse_down(Part::Field, point(100.0, 0.0));
        picker.mouse_up();
        picker.mouse_down(Part::Hue, point(100.0 / 3.0, 117.0));
        assert_eq!(picker.rgb(), 0x00ff00);
        picker.mouse_up();
        // Picking gray, then saturating again, keeps green.
        picker.set_rgb(0x808080);
        assert_eq!(picker.hex(), "#808080");
        picker.mouse_down(Part::Field, point(100.0, 0.0));
        assert_eq!(picker.rgb(), 0x00ff00);
    }
}
