//! Draws chart scenes with Direct2D, and measures their text with DirectWrite.

use std::cell::RefCell;
use std::collections::HashMap;

use diver_charts::{Anchor, Baseline, Color, Corners, Font, Item, Measure, Rect, Shape, Theme};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

fn d2d(color: Color) -> D2D1_COLOR_F {
    let [r, g, b, a] = color.floats();
    D2D1_COLOR_F { r, g, b, a }
}

fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// Text formats by size and weight, made as charts ask for them.
pub struct Fonts {
    dwrite: IDWriteFactory,
    formats: RefCell<HashMap<(u32, bool), IDWriteTextFormat>>,
}

impl Fonts {
    pub fn new() -> Result<Fonts> {
        let dwrite = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        Ok(Fonts { dwrite, formats: RefCell::new(HashMap::new()) })
    }

    fn format(&self, font: Font) -> Option<IDWriteTextFormat> {
        let key = (font.size.to_bits(), font.bold);
        if let Some(f) = self.formats.borrow().get(&key) {
            return Some(f.clone());
        }
        let weight = if font.bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        let format = unsafe {
            let f = self
                .dwrite
                .CreateTextFormat(w!("Segoe UI"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, font.size, w!("en-us"))
                .ok()?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
            f
        };
        self.formats.borrow_mut().insert(key, format.clone());
        Some(format)
    }

    /// The text laid out on one line, with its width and height.
    fn layout(&self, text: &str, font: Font) -> Option<(IDWriteTextLayout, f32, f32)> {
        let format = self.format(font)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&wide, &format, 10_000.0, 1_000.0).ok()?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m).ok()?;
            Some((layout, m.widthIncludingTrailingWhitespace, m.height))
        }
    }
}

impl Measure for Fonts {
    fn width(&self, text: &str, font: Font) -> f32 {
        self.layout(text, font).map_or(0.0, |(_, w, _)| w)
    }
}

pub struct Gfx<'a> {
    pub rt: &'a ID2D1RenderTarget,
    factory: &'a ID2D1Factory,
    pub fonts: &'a Fonts,
    brush: ID2D1SolidColorBrush,
}

impl<'a> Gfx<'a> {
    pub fn new(rt: &'a ID2D1RenderTarget, factory: &'a ID2D1Factory, fonts: &'a Fonts) -> Result<Self> {
        let brush = unsafe { rt.CreateSolidColorBrush(&d2d(Color::rgb(0, 0, 0)), None)? };
        Ok(Gfx { rt, factory, fonts, brush })
    }

    fn brush(&self, color: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&d2d(color)) };
        &self.brush
    }

    pub fn fill(&self, r: Rect, color: Color) {
        unsafe { self.rt.FillRectangle(&rect_f(r), self.brush(color)) };
    }

    pub fn fill_rounded(&self, r: Rect, radius: f32, color: Color) {
        let rounded = D2D1_ROUNDED_RECT { rect: rect_f(r), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rounded, self.brush(color)) };
    }

    pub fn outline_rounded(&self, r: Rect, radius: f32, color: Color, width: f32) {
        let rounded = D2D1_ROUNDED_RECT { rect: rect_f(r), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawRoundedRectangle(&rounded, self.brush(color), width, None) };
    }

    pub fn clip(&self, r: Rect) {
        unsafe { self.rt.PushAxisAlignedClip(&rect_f(r), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    /// One line of app text, left aligned and vertically centered in `r`.
    pub fn text(&self, s: &str, font: Font, r: Rect, color: Color) {
        let text = diver_charts::scene::fit(s, r.w, font, self.fonts);
        self.draw_text(&text, (r.x, r.y + r.h / 2.0), Anchor::Start, Baseline::Middle, font, color, 0.0);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(&self, text: &str, (x, y): (f32, f32), anchor: Anchor, baseline: Baseline, font: Font, color: Color, angle: f32) {
        let Some((layout, w, h)) = self.fonts.layout(text, font) else { return };
        let left = match anchor {
            Anchor::Start => x,
            Anchor::Middle => x - w / 2.0,
            Anchor::End => x - w,
        };
        let top = match baseline {
            Baseline::Top => y,
            Baseline::Middle => y - h / 2.0,
            Baseline::Bottom => y - h,
        };
        unsafe {
            let mut saved = Matrix3x2::default();
            if angle != 0.0 {
                self.rt.GetTransform(&mut saved);
                self.rt.SetTransform(&(Matrix3x2::rotation_around(angle, v2(x, y)) * saved));
            }
            self.rt.DrawTextLayout(v2(left, top), &layout, self.brush(color), D2D1_DRAW_TEXT_OPTIONS_NONE);
            if angle != 0.0 {
                self.rt.SetTransform(&saved);
            }
        }
    }

    /// A rectangle with its own radius at each corner.
    fn corner_path(&self, r: Rect, c: Corners) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geometry = self.factory.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            let arc = |x: f32, y: f32, radius: f32| {
                if radius > 0.0 {
                    sink.AddArc(&D2D1_ARC_SEGMENT {
                        point: v2(x, y),
                        size: D2D_SIZE_F { width: radius, height: radius },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: D2D1_ARC_SIZE_SMALL,
                    });
                }
            };
            let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
            sink.BeginFigure(v2(x0 + c.top_left, y0), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(v2(x1 - c.top_right, y0));
            arc(x1, y0 + c.top_right, c.top_right);
            sink.AddLine(v2(x1, y1 - c.bottom_right));
            arc(x1 - c.bottom_right, y1, c.bottom_right);
            sink.AddLine(v2(x0 + c.bottom_left, y1));
            arc(x0, y1 - c.bottom_left, c.bottom_left);
            sink.AddLine(v2(x0, y0 + c.top_left));
            arc(x0 + c.top_left, y0, c.top_left);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geometry)
        }
    }

    /// Draws scene items at `origin`, lifting those of the hovered mark.
    pub fn items(&self, items: &[Item], origin: (f32, f32), hover: Option<usize>, theme: &Theme) {
        unsafe {
            let mut saved = Matrix3x2::default();
            self.rt.GetTransform(&mut saved);
            self.rt.SetTransform(&(Matrix3x2::translation(origin.0, origin.1) * saved));
            for item in items {
                let lift = |c: Color| if item.mark.is_some() && item.mark == hover { theme.lifted(c) } else { c };
                match &item.shape {
                    Shape::Rect { rect, color, corners, stroke } => {
                        let color = lift(*color);
                        if corners.is_uniform() {
                            let radius = corners.top_left;
                            let rounded = D2D1_ROUNDED_RECT { rect: rect_f(*rect), radiusX: radius, radiusY: radius };
                            if radius > 0.0 {
                                self.rt.FillRoundedRectangle(&rounded, self.brush(color));
                            } else {
                                self.rt.FillRectangle(&rect_f(*rect), self.brush(color));
                            }
                            if let Some((c, w)) = stroke {
                                let inner = D2D1_ROUNDED_RECT { rect: rect_f(rect.grow(-w / 2.0)), ..rounded };
                                self.rt.DrawRoundedRectangle(&inner, self.brush(*c), *w, None);
                            }
                        } else if let Some(path) = self.corner_path(*rect, *corners) {
                            self.rt.FillGeometry(&path, self.brush(color), None);
                            if let Some((c, w)) = stroke {
                                self.rt.DrawGeometry(&path, self.brush(*c), *w, None);
                            }
                        }
                    }
                    Shape::Line { from, to, color, width } => {
                        self.rt.DrawLine(v2(from.0, from.1), v2(to.0, to.1), self.brush(lift(*color)), *width, None);
                    }
                    Shape::Text { text, at, anchor, baseline, font, color, angle } => {
                        self.draw_text(text, *at, *anchor, *baseline, *font, lift(*color), *angle);
                    }
                }
            }
            self.rt.SetTransform(&saved);
        }
    }
}
