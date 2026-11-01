//! Direct2D drawing for the demo: rectangles, text and the formatting marks.

use diver_format::Color;
use diver_format::draw::{Mark, Rect};
use diver_format::style::{Align, CellStyle, LineStyle};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;
use windows_numerics::Vector2;

pub fn rgb(rgb: u32) -> Color {
    Color::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn d2d(color: Color) -> D2D1_COLOR_F {
    let [r, g, b, a] = color.floats();
    D2D1_COLOR_F { r, g, b, a }
}

fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

fn v2((x, y): (f32, f32)) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// Fonts the demo uses, made once.
pub struct Fonts {
    pub dwrite: IDWriteFactory,
    pub ui: IDWriteTextFormat,
    pub ui_bold: IDWriteTextFormat,
    pub small: IDWriteTextFormat,
    pub cell: IDWriteTextFormat,
}

impl Fonts {
    pub fn new() -> Result<Fonts> {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let make = |size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                let format = dwrite.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                Ok(format)
            };
            Ok(Fonts {
                ui: make(12.5, DWRITE_FONT_WEIGHT_NORMAL)?,
                ui_bold: make(11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: make(11.5, DWRITE_FONT_WEIGHT_NORMAL)?,
                cell: make(12.5, DWRITE_FONT_WEIGHT_NORMAL)?,
                dwrite,
            })
        }
    }
}

pub struct Gfx<'a> {
    pub rt: &'a ID2D1RenderTarget,
    pub factory: &'a ID2D1Factory,
    pub fonts: &'a Fonts,
    brush: ID2D1SolidColorBrush,
    dashed: ID2D1StrokeStyle,
    dotted: ID2D1StrokeStyle,
    round: ID2D1StrokeStyle,
}

impl<'a> Gfx<'a> {
    pub fn new(rt: &'a ID2D1RenderTarget, factory: &'a ID2D1Factory, fonts: &'a Fonts) -> Result<Self> {
        unsafe {
            let brush = rt.CreateSolidColorBrush(&d2d(Color::rgb(0, 0, 0)), None)?;
            let style = |dash: D2D1_DASH_STYLE, cap: D2D1_CAP_STYLE| -> Result<ID2D1StrokeStyle> {
                let props = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: cap,
                    endCap: cap,
                    dashCap: cap,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: dash,
                    dashOffset: 0.0,
                };
                factory.CreateStrokeStyle(&props, None)
            };
            Ok(Gfx {
                dashed: style(D2D1_DASH_STYLE_DASH, D2D1_CAP_STYLE_FLAT)?,
                dotted: style(D2D1_DASH_STYLE_DOT, D2D1_CAP_STYLE_FLAT)?,
                round: style(D2D1_DASH_STYLE_SOLID, D2D1_CAP_STYLE_ROUND)?,
                rt,
                factory,
                fonts,
                brush,
            })
        }
    }

    fn brush(&self, color: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&d2d(color)) };
        &self.brush
    }

    fn stroke(&self, style: LineStyle) -> Option<&ID2D1StrokeStyle> {
        match style {
            LineStyle::Solid => None,
            LineStyle::Dashed => Some(&self.dashed),
            LineStyle::Dotted => Some(&self.dotted),
        }
    }

    pub fn fill(&self, r: Rect, color: Color) {
        unsafe { self.rt.FillRectangle(&rect_f(r), self.brush(color)) };
    }

    pub fn fill_rounded(&self, r: Rect, radius: f32, color: Color) {
        let rounded = D2D1_ROUNDED_RECT { rect: rect_f(r), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rounded, self.brush(color)) };
    }

    pub fn outline(&self, r: Rect, color: Color, width: f32) {
        unsafe { self.rt.DrawRectangle(&rect_f(r), self.brush(color), width, None) };
    }

    pub fn clip(&self, r: Rect) {
        unsafe { self.rt.PushAxisAlignedClip(&rect_f(r), D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    /// One line of text in `r`, vertically centered.
    pub fn text(&self, s: &str, format: &IDWriteTextFormat, r: Rect, color: Color) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.rt.DrawText(
                &wide,
                format,
                &rect_f(r),
                self.brush(color),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            )
        };
    }

    /// A cell's text with its formatting.
    pub fn cell_text(&self, s: &str, r: Rect, style: &CellStyle, align: Align, color: Color) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        let range = DWRITE_TEXT_RANGE { startPosition: 0, length: wide.len() as u32 };
        let pad = 5.0;
        let Ok(layout) = (unsafe {
            self.fonts.dwrite.CreateTextLayout(&wide, &self.fonts.cell, (r.w - 2.0 * pad).max(1.0), r.h)
        }) else {
            return;
        };
        unsafe {
            let _ = layout.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            if style.bold {
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, range);
            }
            if style.italic {
                let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, range);
            }
            if style.underline {
                let _ = layout.SetUnderline(true, range);
            }
            if style.strike {
                let _ = layout.SetStrikethrough(true, range);
            }
            self.rt.DrawTextLayout(v2((r.x + pad, r.y)), &layout, self.brush(color), D2D1_DRAW_TEXT_OPTIONS_CLIP);
        }
    }

    fn geometry(&self, points: &[(f32, f32)], closed: bool) -> Option<ID2D1PathGeometry> {
        let (first, rest) = points.split_first()?;
        unsafe {
            let geometry = self.factory.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            sink.BeginFigure(v2(*first), if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            let rest: Vec<Vector2> = rest.iter().map(|p| v2(*p)).collect();
            sink.AddLines(&rest);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            Some(geometry)
        }
    }

    pub fn mark(&self, mark: &Mark) {
        unsafe {
            match mark {
                Mark::Fill { rect, color } => self.fill(*rect, *color),
                Mark::Fade { rect, color } => {
                    let stops = [
                        D2D1_GRADIENT_STOP { position: 0.0, color: d2d(*color) },
                        D2D1_GRADIENT_STOP { position: 1.0, color: d2d(Color { a: 20, ..*color }) },
                    ];
                    let Ok(collection) =
                        self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                    else {
                        return;
                    };
                    let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                        startPoint: v2((rect.x, rect.y)),
                        endPoint: v2((rect.right(), rect.y)),
                    };
                    if let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &collection) {
                        self.rt.FillRectangle(&rect_f(*rect), &brush);
                    }
                }
                Mark::Outline { rect, color, width, style } => {
                    self.rt.DrawRectangle(&rect_f(*rect), self.brush(*color), *width, self.stroke(*style));
                }
                Mark::Line { from, to, color, width, style } => {
                    self.rt.DrawLine(v2(*from), v2(*to), self.brush(*color), *width, self.stroke(*style));
                }
                Mark::Polyline { points, color, width } => {
                    if let Some(g) = self.geometry(points, false) {
                        self.rt.DrawGeometry(&g, self.brush(*color), *width, &self.round);
                    }
                }
                Mark::Polygon { points, color } => {
                    if let Some(g) = self.geometry(points, true) {
                        self.rt.FillGeometry(&g, self.brush(*color), None);
                    }
                }
                Mark::Circle { center, radius, color } => {
                    let ellipse = D2D1_ELLIPSE { point: v2(*center), radiusX: *radius, radiusY: *radius };
                    self.rt.FillEllipse(&ellipse, self.brush(*color));
                }
            }
        }
    }
}
