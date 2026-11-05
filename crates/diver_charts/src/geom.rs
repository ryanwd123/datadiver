//! Points, rectangles and colors, in device-independent pixels.

pub type Point = (f32, f32);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// The rectangle between two corners, in either order.
    pub fn from_corners((x0, y0): Point, (x1, y1): Point) -> Rect {
        Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, (x, y): Point) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Shrinks each side by its amount; never below zero size.
    pub fn inset(&self, left: f32, top: f32, right: f32, bottom: f32) -> Rect {
        Rect::new(self.x + left, self.y + top, (self.w - left - right).max(0.0), (self.h - top - bottom).max(0.0))
    }

    /// Grows each side by `d`.
    pub fn grow(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// `0xrrggbb`.
    pub const fn hex(rgb: u32) -> Color {
        Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    pub const fn with_alpha(self, a: u8) -> Color {
        Color { a, ..self }
    }

    /// `[r, g, b, a]` from 0 to 1.
    pub fn floats(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|c| c as f32 / 255.0)
    }

    /// The color `t` of the way to `other`.
    pub fn mix(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color { r: m(self.r, other.r), g: m(self.g, other.g), b: m(self.b, other.b), a: m(self.a, other.a) }
    }

    /// Relative luminance (WCAG), for picking text that reads on a fill.
    pub fn luminance(self) -> f32 {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }
}
