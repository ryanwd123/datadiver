//! Points, rectangles, and colors in device-independent pixels.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub const fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, other: Point) -> Point {
        point(self.x + other.x, self.y + other.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, other: Point) -> Point {
        point(self.x - other.x, self.y - other.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn origin(&self) -> Point {
        point(self.x, self.y)
    }

    pub fn center(&self) -> Point {
        point(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    /// Shrinks every side by `d`.
    pub fn inset(&self, d: f32) -> Rect {
        self.inset_xy(d, d)
    }

    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        rect(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }

    pub fn intersect(&self, other: &Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        rect(x, y, (right - x).max(0.0), (bottom - y).max(0.0))
    }

    /// The left `width` of this rectangle, and what remains.
    pub fn split_left(&self, width: f32) -> (Rect, Rect) {
        let width = width.clamp(0.0, self.w);
        (
            rect(self.x, self.y, width, self.h),
            rect(self.x + width, self.y, self.w - width, self.h),
        )
    }

    pub fn split_right(&self, width: f32) -> (Rect, Rect) {
        let width = width.clamp(0.0, self.w);
        (
            rect(self.right() - width, self.y, width, self.h),
            rect(self.x, self.y, self.w - width, self.h),
        )
    }

    pub fn split_top(&self, height: f32) -> (Rect, Rect) {
        let height = height.clamp(0.0, self.h);
        (
            rect(self.x, self.y, self.w, height),
            rect(self.x, self.y + height, self.w, self.h - height),
        )
    }

    pub fn split_bottom(&self, height: f32) -> (Rect, Rect) {
        let height = height.clamp(0.0, self.h);
        (
            rect(self.x, self.bottom() - height, self.w, height),
            rect(self.x, self.y, self.w, self.h - height),
        )
    }

    pub fn with_h(&self, h: f32) -> Rect {
        rect(self.x, self.y, self.w, h)
    }

    pub fn with_w(&self, w: f32) -> Rect {
        rect(self.x, self.y, w, self.h)
    }

    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        rect(self.x + dx, self.y + dy, self.w, self.h)
    }
}

/// A color with straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    /// `0xRRGGBB`.
    pub const fn rgb(value: u32) -> Color {
        Color {
            r: ((value >> 16) & 0xff) as f32 / 255.0,
            g: ((value >> 8) & 0xff) as f32 / 255.0,
            b: (value & 0xff) as f32 / 255.0,
            a: 1.0,
        }
    }

    /// `0xRRGGBBAA`.
    pub const fn rgba(value: u32) -> Color {
        Color {
            r: ((value >> 24) & 0xff) as f32 / 255.0,
            g: ((value >> 16) & 0xff) as f32 / 255.0,
            b: ((value >> 8) & 0xff) as f32 / 255.0,
            a: (value & 0xff) as f32 / 255.0,
        }
    }

    /// Hue, saturation, and lightness in 0..=1.
    pub fn hsla(h: f32, s: f32, l: f32, a: f32) -> Color {
        let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
        let p = 2.0 * l - q;
        let channel = |t: f32| {
            let t = t.rem_euclid(1.0);
            if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 0.5 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            }
        };
        if s == 0.0 {
            return Color { r: l, g: l, b: l, a };
        }
        Color {
            r: channel(h + 1.0 / 3.0),
            g: channel(h),
            b: channel(h - 1.0 / 3.0),
            a,
        }
    }

    pub fn alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    pub fn opacity(self, factor: f32) -> Color {
        Color {
            a: self.a * factor,
            ..self
        }
    }
}
