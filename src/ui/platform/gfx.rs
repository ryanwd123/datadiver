//! Drawing with Direct2D, and the hit regions a frame records for input.

use std::collections::HashMap;
use std::rc::Rc;

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE;
use windows::core::{IUnknown, Interface};
use windows_numerics::Vector2;

use super::geometry::{Color, Point, Rect, rect};
use super::text::{self, Align, Font, TextLayout, TextOptions};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    #[default]
    Arrow,
    IBeam,
    Hand,
    ResizeColumns,
    ResizeRows,
}

/// A rectangle that answers mouse input with `hit`. Later regions sit on top.
#[derive(Clone, Debug)]
pub struct HitRegion<H> {
    pub rect: Rect,
    pub hit: H,
    pub cursor: Cursor,
    pub tooltip: Option<Rc<str>>,
}

/// Brushes by color, kept for a render target's lifetime.
#[derive(Default)]
pub struct Brushes {
    solid: HashMap<[u32; 4], ID2D1SolidColorBrush>,
}

impl Brushes {
    fn get(&mut self, target: &ID2D1RenderTarget, color: Color) -> ID2D1SolidColorBrush {
        let key = [
            color.r.to_bits(),
            color.g.to_bits(),
            color.b.to_bits(),
            color.a.to_bits(),
        ];
        if let Some(brush) = self.solid.get(&key) {
            return brush.clone();
        }
        let brush = unsafe {
            target
                .CreateSolidColorBrush(&d2d_color(color), None)
                .expect("create a brush")
        };
        if self.solid.len() > 512 {
            self.solid.clear();
        }
        self.solid.insert(key, brush.clone());
        brush
    }

    pub fn clear(&mut self) {
        self.solid.clear();
    }
}

/// A stroke style from the shared factory; `None` draws solid.
fn stroke_style(dash: D2D1_DASH_STYLE, cap: D2D1_CAP_STYLE) -> Option<ID2D1StrokeStyle> {
    let props = D2D1_STROKE_STYLE_PROPERTIES {
        startCap: cap,
        endCap: cap,
        dashCap: cap,
        lineJoin: D2D1_LINE_JOIN_ROUND,
        miterLimit: 10.0,
        dashStyle: dash,
        dashOffset: 0.0,
    };
    unsafe { super::window::d2d_factory().CreateStrokeStyle(&props, None).ok() }
}

/// A path through `points`, filled and closed or open.
fn path(points: &[Point], closed: bool) -> Option<ID2D1PathGeometry> {
    let (first, rest) = points.split_first()?;
    unsafe {
        let geometry = super::window::d2d_factory().CreatePathGeometry().ok()?;
        let sink = geometry.Open().ok()?;
        let begin = if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
        sink.BeginFigure(Vector2 { X: first.x, Y: first.y }, begin);
        let rest: Vec<Vector2> = rest.iter().map(|p| Vector2 { X: p.x, Y: p.y }).collect();
        sink.AddLines(&rest);
        sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
        sink.Close().ok()?;
        Some(geometry)
    }
}

fn d2d_color(color: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    }
}

fn d2d_rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

/// One frame's drawing surface, `width` × `height` DIPs.
pub struct Canvas<'a, H> {
    target: &'a ID2D1RenderTarget,
    brushes: &'a mut Brushes,
    clips: Vec<Rect>,
    hits: Vec<HitRegion<H>>,
    pub width: f32,
    pub height: f32,
    /// The pointer, when it is over the window and not blocked by an overlay.
    mouse: Option<Point>,
}

impl<'a, H: Clone> Canvas<'a, H> {
    pub fn new(target: &'a ID2D1RenderTarget, brushes: &'a mut Brushes) -> Self {
        let size = unsafe { target.GetSize() };
        unsafe {
            target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        }
        text::with(|system| system.next_frame());
        Self {
            target,
            brushes,
            clips: Vec::new(),
            hits: Vec::new(),
            width: size.width,
            height: size.height,
            mouse: None,
        }
    }

    pub fn set_mouse(&mut self, mouse: Option<Point>) {
        self.mouse = mouse;
    }

    pub fn mouse(&self) -> Option<Point> {
        self.mouse
    }

    /// Whether the pointer is over `r`, within the current clip.
    pub fn hovered(&self, r: Rect) -> bool {
        let Some(mouse) = self.mouse else {
            return false;
        };
        r.contains(mouse) && self.clips.last().is_none_or(|clip| clip.contains(mouse))
    }

    pub fn bounds(&self) -> Rect {
        rect(0.0, 0.0, self.width, self.height)
    }

    /// Device pixels per DIP.
    pub fn scale(&self) -> f32 {
        let (mut x, mut y) = (0.0, 0.0);
        unsafe { self.target.GetDpi(&mut x, &mut y) };
        if x > 0.0 { x / 96.0 } else { 1.0 }
    }

    /// Lets a component that draws itself (the hosted code editor) paint into
    /// the target, then restores the canvas's drawing modes.
    pub fn paint_native(&mut self, paint: impl FnOnce(&ID2D1RenderTarget)) {
        paint(self.target);
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            self.target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        }
    }

    pub fn into_hits(self) -> Vec<HitRegion<H>> {
        self.hits
    }

    fn brush(&mut self, color: Color) -> ID2D1SolidColorBrush {
        self.brushes.get(self.target, color)
    }

    pub fn clear(&mut self, color: Color) {
        unsafe { self.target.Clear(Some(&d2d_color(color))) };
    }

    pub fn fill(&mut self, r: Rect, color: Color) {
        if r.is_empty() || color.a <= 0.0 {
            return;
        }
        let brush = self.brush(color);
        unsafe { self.target.FillRectangle(&d2d_rect(r), &brush) };
    }

    pub fn fill_rounded(&mut self, r: Rect, radius: f32, color: Color) {
        if r.is_empty() || color.a <= 0.0 {
            return;
        }
        let brush = self.brush(color);
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: d2d_rect(r),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    /// A border `width` thick inside `r`.
    pub fn stroke(&mut self, r: Rect, color: Color, width: f32) {
        if r.is_empty() {
            return;
        }
        let width = width.min(r.w / 2.0).min(r.h / 2.0);
        self.fill(rect(r.x, r.y, r.w, width), color);
        self.fill(rect(r.x, r.bottom() - width, r.w, width), color);
        self.fill(rect(r.x, r.y + width, width, r.h - 2.0 * width), color);
        self.fill(
            rect(r.right() - width, r.y + width, width, r.h - 2.0 * width),
            color,
        );
    }

    pub fn stroke_rounded(&mut self, r: Rect, radius: f32, color: Color, width: f32) {
        if r.is_empty() {
            return;
        }
        let brush = self.brush(color);
        let half = width / 2.0;
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.DrawRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: d2d_rect(r.inset(half)),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
                width,
                None,
            );
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    pub fn line(&mut self, from: Point, to: Point, color: Color, width: f32) {
        let brush = self.brush(color);
        unsafe {
            self.target
                .SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.DrawLine(
                Vector2 { X: from.x, Y: from.y },
                Vector2 { X: to.x, Y: to.y },
                &brush,
                width,
                None,
            );
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    /// A line drawn dashed or dotted (`dots`), antialiased.
    pub fn dashed_line(&mut self, from: Point, to: Point, color: Color, width: f32, dots: bool) {
        let brush = self.brush(color);
        let style = stroke_style(if dots { D2D1_DASH_STYLE_DOT } else { D2D1_DASH_STYLE_DASH }, D2D1_CAP_STYLE_FLAT);
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.DrawLine(
                Vector2 { X: from.x, Y: from.y },
                Vector2 { X: to.x, Y: to.y },
                &brush,
                width,
                style.as_ref(),
            );
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    /// `r`'s outline, its stroke centered on the edges, dashed or dotted.
    pub fn dashed_rect(&mut self, r: Rect, color: Color, width: f32, dots: bool) {
        let brush = self.brush(color);
        let style = stroke_style(if dots { D2D1_DASH_STYLE_DOT } else { D2D1_DASH_STYLE_DASH }, D2D1_CAP_STYLE_FLAT);
        unsafe { self.target.DrawRectangle(&d2d_rect(r), &brush, width, style.as_ref()) };
    }

    /// `color` at `r`'s left fading to clear at its right.
    pub fn fill_fade(&mut self, r: Rect, color: Color) {
        if r.is_empty() {
            return;
        }
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d_color(color) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d_color(Color { a: color.a * 0.1, ..color }) },
        ];
        unsafe {
            let Ok(collection) = self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return;
            };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: r.x, Y: r.y },
                endPoint: Vector2 { X: r.right(), Y: r.y },
            };
            if let Ok(brush) = self.target.CreateLinearGradientBrush(&props, None, &collection) {
                self.target.FillRectangle(&d2d_rect(r), &brush);
            }
        }
    }

    /// Fills `r` with a gradient through `stops` (position 0–1, color), left
    /// to right, or top to bottom when `vertical`.
    pub fn fill_gradient(&mut self, r: Rect, stops: &[(f32, Color)], vertical: bool) {
        if r.is_empty() || stops.is_empty() {
            return;
        }
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|(position, color)| D2D1_GRADIENT_STOP { position: *position, color: d2d_color(*color) })
            .collect();
        unsafe {
            let Ok(collection) = self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
            else {
                return;
            };
            let end = if vertical { Vector2 { X: r.x, Y: r.bottom() } } else { Vector2 { X: r.right(), Y: r.y } };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: r.x, Y: r.y }, endPoint: end };
            if let Ok(brush) = self.target.CreateLinearGradientBrush(&props, None, &collection) {
                self.target.FillRectangle(&d2d_rect(r), &brush);
            }
        }
    }

    /// Fills the polygon through `points`, antialiased.
    pub fn fill_polygon(&mut self, points: &[Point], color: Color) {
        let Some(geometry) = path(points, true) else { return };
        let brush = self.brush(color);
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.FillGeometry(&geometry, &brush, None);
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    /// Connected segments through `points`, with round joins and ends.
    pub fn polyline(&mut self, points: &[Point], color: Color, width: f32) {
        let Some(geometry) = path(points, false) else { return };
        let brush = self.brush(color);
        let style = stroke_style(D2D1_DASH_STYLE_SOLID, D2D1_CAP_STYLE_ROUND);
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.DrawGeometry(&geometry, &brush, width, style.as_ref());
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    pub fn fill_circle(&mut self, center: Point, radius: f32, color: Color) {
        let brush = self.brush(color);
        let ellipse = D2D1_ELLIPSE { point: Vector2 { X: center.x, Y: center.y }, radiusX: radius, radiusY: radius };
        unsafe {
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            self.target.FillEllipse(&ellipse, &brush);
            self.target.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        }
    }

    /// A soft shadow around a popup.
    pub fn shadow(&mut self, r: Rect, radius: f32) {
        for step in 1..=6 {
            let spread = step as f32 * 1.5;
            let shadow = rect(r.x - spread, r.y - spread + 2.0, r.w + 2.0 * spread, r.h + 2.0 * spread);
            self.fill_rounded(shadow, radius + spread, Color::rgba(0x00000008));
        }
    }

    pub fn draw_layout(&mut self, layout: &TextLayout, origin: Point, color: Color) {
        let brush = self.brush(color);
        unsafe {
            self.target.DrawTextLayout(
                Vector2 {
                    X: origin.x,
                    Y: origin.y,
                },
                &layout.layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    /// Draws `layout` with byte ranges in their own colors.
    pub fn draw_layout_runs(
        &mut self,
        layout: &TextLayout,
        origin: Point,
        color: Color,
        runs: &[(usize, usize, Color)],
    ) {
        if runs.is_empty() {
            self.draw_layout(layout, origin, color);
            return;
        }
        for &(start, end, run_color) in runs {
            let brush: IUnknown = self.brush(run_color).cast().expect("brush is IUnknown");
            unsafe {
                let _ = layout.layout.SetDrawingEffect(&brush, layout.range(start, end));
            }
        }
        self.draw_layout(layout, origin, color);
        unsafe {
            let _ = layout.layout.SetDrawingEffect(
                None::<&IUnknown>,
                DWRITE_TEXT_RANGE {
                    startPosition: 0,
                    length: u32::MAX,
                },
            );
        }
    }

    /// Draws one line of text in `r`, vertically centered, cut off with `…`.
    pub fn text(&mut self, r: Rect, text: &str, font: &Font, color: Color, align: Align) -> Rc<TextLayout> {
        let layout = text::layout(text, font, TextOptions::truncated(r.w).align(align));
        let y = r.y + ((r.h - layout.height) / 2.0).max(0.0);
        self.push_clip(r);
        self.draw_layout(&layout, Point { x: r.x, y }, color);
        self.pop_clip();
        layout
    }

    /// Draws wrapped text from the top of `r`, returning its height.
    pub fn paragraph(&mut self, r: Rect, text: &str, font: &Font, color: Color) -> f32 {
        let layout = text::layout(text, font, TextOptions::wrapped(r.w));
        self.draw_layout(&layout, r.origin(), color);
        layout.height
    }

    pub fn push_clip(&mut self, r: Rect) {
        let clip = match self.clips.last() {
            Some(outer) => outer.intersect(&r),
            None => r,
        };
        self.clips.push(clip);
        unsafe {
            self.target
                .PushAxisAlignedClip(&d2d_rect(clip), D2D1_ANTIALIAS_MODE_ALIASED)
        };
    }

    pub fn pop_clip(&mut self) {
        if self.clips.pop().is_some() {
            unsafe { self.target.PopAxisAlignedClip() };
        }
    }

    pub fn clip(&self) -> Option<Rect> {
        self.clips.last().copied()
    }

    /// Answers mouse input over `r`, within the current clip, with `hit`.
    pub fn hit(&mut self, r: Rect, hit: H) {
        self.hit_with(r, hit, Cursor::Arrow, None);
    }

    pub fn hit_cursor(&mut self, r: Rect, hit: H, cursor: Cursor) {
        self.hit_with(r, hit, cursor, None);
    }

    pub fn hit_with(&mut self, r: Rect, hit: H, cursor: Cursor, tooltip: Option<&str>) {
        let r = match self.clips.last() {
            Some(clip) => clip.intersect(&r),
            None => r,
        };
        if r.is_empty() {
            return;
        }
        self.hits.push(HitRegion {
            rect: r,
            hit,
            cursor,
            tooltip: tooltip.map(Rc::from),
        });
    }

    /// Hit regions recorded so far, for widgets that resolve their own.
    pub fn hits(&self) -> &[HitRegion<H>] {
        &self.hits
    }
}
