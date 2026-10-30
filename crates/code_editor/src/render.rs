//! Painting with Direct2D and DirectWrite: the shared factories, the color
//! palette, fonts, and a cache of line layouts built only for visible lines.
//!
//! Painting goes to any `ID2D1RenderTarget`: the editor window's own
//! `ID2D1HwndRenderTarget`, or a host's target through `Editor::paint_into`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::text::{col_to_utf16, floor_boundary, utf16_to_col};
use crate::theme::{Ink, Paint, Palette};

/// Lines longer than this many bytes are laid out only up to here, like
/// Monaco's `stopRenderingLineAfter`.
pub const MAX_RENDER_BYTES: usize = 10_000;

/// Device-independent factories, created when the first editor needs them and
/// shared by every editor on the thread.
pub(crate) struct Shared {
    pub d2d: ID2D1Factory,
    pub dw: IDWriteFactory,
    /// The default code font: the first of Cascadia Mono, Consolas, Courier New.
    pub mono: String,
}

thread_local! {
    static SHARED: RefCell<Option<Rc<Shared>>> = const { RefCell::new(None) };
}

pub(crate) fn shared() -> Result<Rc<Shared>> {
    SHARED.with(|s| {
        if let Some(sh) = &*s.borrow() {
            return Ok(sh.clone());
        }
        let d2d: ID2D1Factory =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let mono = ["Cascadia Mono", "Consolas"]
            .into_iter()
            .find(|f| font_exists(&dw, f))
            .unwrap_or("Courier New")
            .to_string();
        let sh = Rc::new(Shared { d2d, dw, mono });
        *s.borrow_mut() = Some(sh.clone());
        Ok(sh)
    })
}

fn font_exists(dw: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let mut idx = 0;
        let mut exists = BOOL(0);
        coll.FindFamilyName(&HSTRING::from(name), &mut idx, &mut exists)
            .is_ok()
            && exists.as_bool()
    }
}

pub(crate) fn color(argb: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((argb >> 16) & 0xFF) as f32 / 255.0,
        g: ((argb >> 8) & 0xFF) as f32 / 255.0,
        b: (argb & 0xFF) as f32 / 255.0,
        a: (argb >> 24) as f32 / 255.0,
    }
}

pub(crate) fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

pub(crate) fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn text_range(a: u32, b: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: a,
        length: b.saturating_sub(a),
    }
}

/// Text formats and metrics for the current font, size and DPI.
pub(crate) struct Fonts {
    pub code: IDWriteTextFormat,
    /// Right-aligned, for line numbers.
    pub numbers: IDWriteTextFormat,
    /// Segoe UI, for the overlays.
    pub ui: IDWriteTextFormat,
    /// Segoe UI, centered, for buttons.
    pub ui_center: IDWriteTextFormat,
    typography: IDWriteTypography,
    pub line_h: f32,
    pub char_w: f32,
}

impl Fonts {
    fn new(dw: &IDWriteFactory, family: &str, size: f32, tab: usize, dpi: f32) -> Result<Fonts> {
        unsafe {
            let fam = HSTRING::from(family);
            let make = |size: f32, fam: &HSTRING| {
                dw.CreateTextFormat(
                    fam,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
            };
            let code = make(size, &fam)?;
            code.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            // Natural metrics of the font, to center text in the line height.
            let probe = dw.CreateTextLayout(&[b' ' as u16], &code, 1e4, 1e4)?;
            let mut tm = DWRITE_TEXT_METRICS::default();
            probe.GetMetrics(&mut tm)?;
            let lm = line_metrics(&probe);
            let (natural_h, natural_base) = lm.first().map_or((size * 1.2, size), |m| (m.height, m.baseline));
            // Lines sit on whole device pixels.
            let k = dpi / 96.0;
            let line_h = (size * 1.35 * k).round() / k;
            let baseline = ((natural_base + (line_h - natural_h) / 2.0) * k).round() / k;
            let char_w = tm.widthIncludingTrailingWhitespace;
            code.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_h, baseline)?;
            code.SetIncrementalTabStop(char_w * tab.max(1) as f32)?;

            let numbers = make(size, &fam)?;
            numbers.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            numbers.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            numbers.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, line_h, baseline)?;

            let ui = make(13.0, &HSTRING::from("Segoe UI"))?;
            ui.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            ui.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            let ui_center = make(13.0, &HSTRING::from("Segoe UI"))?;
            ui_center.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            ui_center.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            ui_center.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;

            let typography = dw.CreateTypography()?;
            for tag in [
                DWRITE_FONT_FEATURE_TAG_STANDARD_LIGATURES,
                DWRITE_FONT_FEATURE_TAG_CONTEXTUAL_ALTERNATES,
            ] {
                typography.AddFontFeature(DWRITE_FONT_FEATURE {
                    nameTag: tag,
                    parameter: 0,
                })?;
            }
            Ok(Fonts {
                code,
                numbers,
                ui,
                ui_center,
                typography,
                line_h,
                char_w,
            })
        }
    }
}

fn line_metrics(l: &IDWriteTextLayout) -> Vec<DWRITE_LINE_METRICS> {
    let mut buf = vec![DWRITE_LINE_METRICS::default(); 4];
    let mut n = 0u32;
    unsafe {
        if l.GetLineMetrics(Some(&mut buf), &mut n).is_err() {
            buf.resize(n as usize, Default::default());
            let _ = l.GetLineMetrics(Some(&mut buf), &mut n);
        }
    }
    buf.truncate(n as usize);
    buf
}

/// A colored run: UTF-16 range in the layout and its ink.
pub(crate) type Run = (u32, u32, Paint);

/// One laid-out line of text.
pub(crate) struct LineLayout {
    pub layout: IDWriteTextLayout,
    /// The text laid out (a prefix of the line when it is very long).
    pub text: String,
    pub width: f32,
    runs: Vec<Run>,
    /// Brush generation the runs were last colored with.
    colored: Cell<u32>,
    /// Frame in which the layout was last used.
    used: Cell<u32>,
}

impl LineLayout {
    /// x of byte column `col` (past the laid-out text, extrapolated by spaces).
    pub fn x_of(&self, col: usize, char_w: f32) -> f32 {
        if col > self.text.len() {
            return self.width + (col - self.text.len()) as f32 * char_w;
        }
        let i = col_to_utf16(&self.text, col) as u32;
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.layout.HitTestTextPosition(i, false, &mut x, &mut y, &mut m);
        }
        x
    }

    /// Byte column nearest to x.
    pub fn col_at(&self, x: f32) -> usize {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self.layout.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m);
        }
        let i = m.textPosition + if trailing.as_bool() { m.length } else { 0 };
        utf16_to_col(&self.text, i as usize)
    }
}

pub(crate) struct Renderer {
    pub shared: Rc<Shared>,
    pub palette: Palette,
    brushes: Vec<ID2D1SolidColorBrush>,
    /// The render target the brushes belong to.
    brush_rt: Option<ID2D1RenderTarget>,
    brush_gen: u32,
    /// Brushes for host colors (decorations), on the same target.
    custom: RefCell<HashMap<u32, ID2D1SolidColorBrush>>,
    pub fonts: Option<Rc<Fonts>>,
    cache: HashMap<u64, Rc<LineLayout>>,
    frame: u32,
}

impl Renderer {
    pub fn new(palette: Palette) -> Result<Self> {
        Ok(Renderer {
            shared: shared()?,
            palette,
            brushes: Vec::new(),
            brush_rt: None,
            brush_gen: 1,
            custom: RefCell::new(HashMap::new()),
            fonts: None,
            cache: HashMap::new(),
            frame: 0,
        })
    }

    /// Builds the fonts for a family, size (zoom applied) and tab size.
    pub fn set_font(&mut self, family: Option<&str>, size: f32, tab: usize, dpi: f32) -> Result<Rc<Fonts>> {
        let family = family.unwrap_or(&self.shared.mono).to_string();
        let f = Rc::new(Fonts::new(&self.shared.dw, &family, size, tab, dpi)?);
        self.fonts = Some(f.clone());
        self.cache.clear();
        Ok(f)
    }

    pub fn set_palette(&mut self, p: Palette) {
        self.palette = p;
        for (b, &c) in self.brushes.iter().zip(self.palette.colors.iter()) {
            unsafe { b.SetColor(&color(c)) };
        }
    }

    /// Drops everything tied to a render target (hidden editors keep nothing).
    pub fn release_target(&mut self) {
        self.brushes.clear();
        self.custom.borrow_mut().clear();
        self.brush_rt = None;
        self.brush_gen += 1;
    }

    /// Makes sure the brushes belong to `rt`.
    pub fn bind(&mut self, rt: &ID2D1RenderTarget) -> Result<()> {
        if self.brush_rt.as_ref().is_some_and(|b| b.as_raw() == rt.as_raw()) && !self.brushes.is_empty() {
            return Ok(());
        }
        self.brushes = self
            .palette
            .colors
            .iter()
            .map(|&c| unsafe { rt.CreateSolidColorBrush(&color(c), None) })
            .collect::<Result<_>>()?;
        self.brush_rt = Some(rt.clone());
        self.brush_gen += 1;
        self.custom.borrow_mut().clear();
        Ok(())
    }

    /// The brush for a paint (host colors get their own brush on first use).
    pub fn paint_brush(&self, p: Paint) -> Option<ID2D1SolidColorBrush> {
        match p {
            Paint::Ink(ink) => Some(self.brush(ink).clone()),
            Paint::Argb(c) => {
                let mut custom = self.custom.borrow_mut();
                if let Some(b) = custom.get(&c) {
                    return Some(b.clone());
                }
                let rt = self.brush_rt.as_ref()?;
                let b = unsafe { rt.CreateSolidColorBrush(&color(c), None) }.ok()?;
                custom.insert(c, b.clone());
                Some(b)
            }
        }
    }

    /// Fills a rectangle with a host color.
    pub fn fill_argb(&self, rt: &ID2D1RenderTarget, l: f32, t: f32, r: f32, b: f32, argb: u32) {
        if let Some(br) = self.paint_brush(Paint::Argb(argb)) {
            unsafe { rt.FillRectangle(&rect(l, t, r, b), &br) };
        }
    }

    pub fn brush(&self, ink: Ink) -> &ID2D1SolidColorBrush {
        &self.brushes[ink as usize]
    }

    pub fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Evicts layouts not used recently, keeping the cache to a few screens.
    pub fn end_frame(&mut self, visible: usize) {
        if self.cache.len() > visible * 4 + 64 {
            let f = self.frame;
            self.cache.retain(|_, l| f.wrapping_sub(l.used.get()) < 2);
        }
    }

    /// The layout of `line` colored by `spans` (byte ranges), from the cache when
    /// the same text and colors were laid out before.
    pub fn layout(&mut self, line: &str, spans: &[(usize, usize, Paint)]) -> Rc<LineLayout> {
        let text = &line[..floor_boundary(line, MAX_RENDER_BYTES)];
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        spans.hash(&mut h);
        let key = h.finish();
        if let Some(l) = self.cache.get(&key) {
            l.used.set(self.frame);
            return l.clone();
        }
        let fonts = self.fonts.clone().expect("fonts are set before layout");
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.shared
                .dw
                .CreateTextLayout(&wide, &fonts.code, 1e7, fonts.line_h)
                .expect("CreateTextLayout")
        };
        unsafe {
            let _ = layout.SetTypography(&fonts.typography, text_range(0, wide.len() as u32));
        }
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = layout.GetMetrics(&mut m);
        }
        let runs = spans
            .iter()
            .filter(|s| s.0 < text.len())
            .map(|&(a, b, ink)| {
                (
                    col_to_utf16(text, a) as u32,
                    col_to_utf16(text, b.min(text.len())) as u32,
                    ink,
                )
            })
            .collect();
        let l = Rc::new(LineLayout {
            layout,
            text: text.to_string(),
            width: m.widthIncludingTrailingWhitespace,
            runs,
            colored: Cell::new(0),
            used: Cell::new(self.frame),
        });
        self.cache.insert(key, l.clone());
        l
    }

    /// Draws a line layout with its runs colored.
    pub fn draw_line(&self, rt: &ID2D1RenderTarget, l: &LineLayout, x: f32, y: f32) {
        unsafe {
            if l.colored.get() != self.brush_gen {
                for &(a, b, p) in &l.runs {
                    if let Some(br) = self.paint_brush(p) {
                        let _ = l.layout.SetDrawingEffect(&br, text_range(a, b));
                    }
                }
                l.colored.set(self.brush_gen);
            }
            rt.DrawTextLayout(
                v2(x, y),
                &l.layout,
                self.brush(Ink::Text),
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    pub fn fill(&self, rt: &ID2D1RenderTarget, l: f32, t: f32, r: f32, b: f32, ink: Ink) {
        unsafe { rt.FillRectangle(&rect(l, t, r, b), self.brush(ink)) };
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stroke(&self, rt: &ID2D1RenderTarget, l: f32, t: f32, r: f32, b: f32, w: f32, ink: Ink) {
        unsafe { rt.DrawRectangle(&rect(l, t, r, b), self.brush(ink), w, None) };
    }

    #[allow(clippy::too_many_arguments)]
    pub fn line(&self, rt: &ID2D1RenderTarget, x0: f32, y0: f32, x1: f32, y1: f32, w: f32, ink: Ink) {
        unsafe { rt.DrawLine(v2(x0, y0), v2(x1, y1), self.brush(ink), w, None) };
    }

    /// Draws short UI text in `format` inside a rect.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &self,
        rt: &ID2D1RenderTarget,
        s: &str,
        format: &IDWriteTextFormat,
        l: f32,
        t: f32,
        r: f32,
        b: f32,
        ink: Ink,
    ) {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            rt.DrawText(
                &wide,
                format,
                &rect(l, t, r, b),
                self.brush(ink),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

}

/// GPU driver warm-up: 0 not started, 1 running, 2 done.
static GPU: AtomicU8 = AtomicU8::new(0);

/// Loads the GPU driver on a worker thread. Creating the first hardware render
/// target otherwise blocks the UI thread for most of startup, so the first
/// frames are painted in software and this starts after the first frame (so it
/// does not compete with it for the DLL loader).
pub(crate) fn warm_gpu() {
    if GPU.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        std::thread::spawn(|| {
            let mut device = None;
            unsafe {
                let _ = D3D11CreateDevice(
                    None,
                    D3D_DRIVER_TYPE_HARDWARE,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                );
            }
            drop(device);
            GPU.store(2, Ordering::Release);
        });
    }
}

/// The GPU driver is loaded, so a hardware target is quick to create.
pub(crate) fn gpu_ready() -> bool {
    GPU.load(Ordering::Acquire) == 2
}

impl Renderer {
    /// A layout of overlay text, for measuring and hit testing.
    fn ui_text(&self, s: &str) -> Option<IDWriteTextLayout> {
        let f = self.fonts.as_ref()?;
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe { self.shared.dw.CreateTextLayout(&wide, &f.ui, 1e6, 100.0).ok() }
    }

    /// A layout of overlay text to color and draw.
    pub fn ui_layout(&self, s: &str, format: &IDWriteTextFormat, w: f32, h: f32) -> Option<IDWriteTextLayout> {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe { self.shared.dw.CreateTextLayout(&wide, format, w.max(1.0), h.max(1.0)).ok() }
    }

    /// Colors a UTF-16 range of a layout.
    pub fn color_range(&self, l: &IDWriteTextLayout, a: u32, b: u32, ink: Ink) {
        unsafe {
            let _ = l.SetDrawingEffect(self.brush(ink), text_range(a, b));
        }
    }

    pub fn draw_layout(&self, rt: &ID2D1RenderTarget, l: &IDWriteTextLayout, x: f32, y: f32, ink: Ink) {
        unsafe { rt.DrawTextLayout(v2(x, y), l, self.brush(ink), D2D1_DRAW_TEXT_OPTIONS_CLIP) };
    }

    /// x of byte `col` in overlay text `s`.
    pub fn ui_x(&self, s: &str, col: usize) -> f32 {
        let Some(l) = self.ui_text(s) else { return 0.0 };
        let i = col_to_utf16(s, col) as u32;
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = l.HitTestTextPosition(i, false, &mut x, &mut y, &mut m);
        }
        x
    }

    /// The byte column of overlay text `s` nearest to x.
    pub fn ui_col_at(&self, s: &str, x: f32) -> usize {
        let Some(l) = self.ui_text(s) else { return 0 };
        let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = l.HitTestPoint(x.max(0.0), 1.0, &mut trailing, &mut inside, &mut m);
        }
        let i = m.textPosition + if trailing.as_bool() { m.length } else { 0 };
        utf16_to_col(s, i as usize)
    }
}

/// Draws a single-line input: background, border, selection, text, caret,
/// and `placeholder` when empty. `right_pad` leaves room for buttons inside.
#[allow(clippy::too_many_arguments)]
fn paint_input(
    r: &Renderer,
    rt: &ID2D1RenderTarget,
    rc: crate::widgets::Rect,
    input: &mut crate::widgets::input::Input,
    focused: bool,
    caret_on: bool,
    placeholder: &str,
    right_pad: f32,
) {
    let Some(f) = r.fonts.clone() else { return };
    r.fill(rt, rc.l, rc.t, rc.r, rc.b, Ink::InputBg);
    let border = if focused { Ink::Focus } else { Ink::InputBorder };
    r.stroke(rt, rc.l + 0.5, rc.t + 0.5, rc.r - 0.5, rc.b - 0.5, 1.0, border);
    let (l, rr) = (rc.l + 4.0, rc.r - 4.0 - right_pad);
    let w = (rr - l).max(1.0);
    let cx = r.ui_x(&input.text, input.caret);
    if cx - input.scroll > w {
        input.scroll = cx - w;
    } else if cx < input.scroll {
        input.scroll = cx;
    }
    unsafe {
        rt.PushAxisAlignedClip(&rect(l, rc.t, rr, rc.b), D2D1_ANTIALIAS_MODE_ALIASED);
    }
    let x0 = l - input.scroll;
    let (a, b) = input.selection();
    if focused && a != b {
        let (xa, xb) = (r.ui_x(&input.text, a), r.ui_x(&input.text, b));
        r.fill(rt, x0 + xa, rc.t + 3.0, x0 + xb, rc.b - 3.0, Ink::Selection);
    }
    if input.text.is_empty() {
        r.text(rt, placeholder, &f.ui, l, rc.t, rr + right_pad, rc.b, Ink::WidgetDim);
    } else {
        r.text(rt, &input.text, &f.ui, x0, rc.t, x0 + 1e5, rc.b, Ink::WidgetText);
    }
    if focused && caret_on {
        let x = (x0 + cx).round();
        r.fill(rt, x, rc.t + 4.0, x + 1.0, rc.b - 4.0, Ink::WidgetText);
    }
    unsafe { rt.PopAxisAlignedClip() };
}

/// A button: a label centered in `rc`, highlighted when `on`.
fn paint_button(r: &Renderer, rt: &ID2D1RenderTarget, rc: crate::widgets::Rect, label: &str, on: bool, enabled: bool) {
    let Some(f) = r.fonts.clone() else { return };
    if on {
        r.fill(rt, rc.l, rc.t, rc.r, rc.b, Ink::ToggleOn);
        r.stroke(rt, rc.l + 0.5, rc.t + 0.5, rc.r - 0.5, rc.b - 0.5, 1.0, Ink::Focus);
    }
    let ink = if enabled { Ink::WidgetText } else { Ink::WidgetDim };
    r.text(rt, label, &f.ui_center, rc.l, rc.t, rc.r, rc.b, ink);
}

impl crate::editor::State {
    /// Draws the suggest popup.
    fn paint_suggest(&mut self, rt: &ID2D1RenderTarget) {
        use crate::widgets::suggest::{CompletionKind, MAX_ROWS, ROW_H};
        let Some(fr) = self.suggest_frame() else { return };
        let r = &self.render;
        let Some(f) = r.fonts.clone() else { return };
        r.fill(rt, fr.l + 2.0, fr.t + 2.0, fr.r + 2.0, fr.b + 2.0, Ink::Shadow);
        r.fill(rt, fr.l, fr.t, fr.r, fr.b, Ink::WidgetBg);
        r.stroke(rt, fr.l + 0.5, fr.t + 0.5, fr.r - 0.5, fr.b - 0.5, 1.0, Ink::WidgetBorder);
        let s = &self.suggest;
        unsafe {
            rt.PushAxisAlignedClip(&rect(fr.l + 1.0, fr.t + 1.0, fr.r - 1.0, fr.b - 1.0), D2D1_ANTIALIAS_MODE_ALIASED);
        }
        for (n, row) in s.rows.iter().enumerate().skip(s.top).take(MAX_ROWS) {
            let it = &s.items[row.item];
            let y = fr.t + 1.0 + (n - s.top) as f32 * ROW_H;
            if n == s.selected {
                r.fill(rt, fr.l + 1.0, y, fr.r - 1.0, y + ROW_H, Ink::ListSelected);
            }
            let icon = match it.kind {
                CompletionKind::Keyword => Ink::Keyword,
                CompletionKind::Function => Ink::Function,
                CompletionKind::Field | CompletionKind::Variable => Ink::Variable,
                CompletionKind::Class | CompletionKind::Module => Ink::Type,
                CompletionKind::Constant => Ink::Constant,
                CompletionKind::Text => Ink::WidgetDim,
            };
            r.text(rt, it.kind.icon(), &f.ui_center, fr.l + 4.0, y, fr.l + 22.0, y + ROW_H, icon);
            let detail_w = it.detail.as_deref().map_or(0.0, |d| r.ui_x(d, d.len()) + 12.0);
            let lx = fr.l + 26.0;
            if let Some(l) = r.ui_layout(&it.label, &f.ui, (fr.r - lx - detail_w).max(10.0), ROW_H) {
                let text16: Vec<u32> = {
                    let mut acc = 0u32;
                    it.label.chars().map(|c| { let a = acc; acc += c.len_utf16() as u32; a }).collect()
                };
                for &m in &row.matches {
                    if let Some(&u) = text16.get(m as usize) {
                        r.color_range(&l, u, u + it.label.chars().nth(m as usize).map_or(1, |c| c.len_utf16() as u32), Ink::ListMatch);
                    }
                }
                r.draw_layout(rt, &l, lx, y, Ink::WidgetText);
            }
            if let Some(d) = &it.detail {
                r.text(rt, d, &f.ui, fr.r - detail_w, y, fr.r - 4.0, y + ROW_H, Ink::WidgetDim);
            }
        }
        unsafe { rt.PopAxisAlignedClip() };
    }

    /// Draws the hover message box near the mouse.
    fn paint_tooltip(&self, rt: &ID2D1RenderTarget) {
        let (Some((x, y, msg)), Some(f)) = (&self.tooltip, &self.render.fonts) else { return };
        let r = &self.render;
        let lines: Vec<&str> = msg.lines().collect();
        let tw = lines.iter().map(|l| r.ui_x(l, l.len())).fold(0.0, f32::max) + 16.0;
        let th = lines.len() as f32 * 18.0 + 8.0;
        let (w, h) = (self.view.width, self.view.height);
        let left = x.min(w - tw - 4.0).max(0.0);
        let top = if y + 20.0 + th < h { y + 20.0 } else { (y - th - 8.0).max(0.0) };
        r.fill(rt, left + 2.0, top + 2.0, left + tw + 2.0, top + th + 2.0, Ink::Shadow);
        r.fill(rt, left, top, left + tw, top + th, Ink::WidgetBg);
        r.stroke(rt, left + 0.5, top + 0.5, left + tw - 0.5, top + th - 0.5, 1.0, Ink::WidgetBorder);
        for (i, l) in lines.iter().enumerate() {
            let ly = top + 4.0 + i as f32 * 18.0;
            r.text(rt, l, &f.ui, left + 8.0, ly, left + tw, ly + 18.0, Ink::WidgetText);
        }
    }

    /// Draws the find widget and go to line.
    fn paint_widgets(&mut self, rt: &ID2D1RenderTarget) {
        use crate::widgets::Focus;
        let w = self.view.width;
        let caret_on = self.caret_on && self.focused;
        if self.find.open {
            let p = self.doc.sels.primary();
            let status = self.find.status((p.start(), p.end()));
            let none = self.find.matches.is_empty();
            let l = self.find.layout(w);
            let r = &self.render;
            let fr = l.frame;
            r.fill(rt, fr.l + 2.0, fr.t, fr.r + 2.0, fr.b + 3.0, Ink::Shadow);
            r.fill(rt, fr.l, fr.t, fr.r, fr.b, Ink::WidgetBg);
            paint_button(r, rt, l.chevron, if self.find.replace { "\u{25BE}" } else { "\u{25B8}" }, false, true);
            let focus = self.focus;
            paint_input(r, rt, l.find_input, &mut self.find.find, focus == Focus::Find, caret_on, "Find", 66.0);
            paint_button(r, rt, l.toggles[0], "Aa", self.find.match_case, true);
            paint_button(r, rt, l.toggles[1], "ab", self.find.whole_word, true);
            let t = l.toggles[1];
            r.line(rt, t.l + 6.0, t.b - 5.0, t.r - 6.0, t.b - 5.0, 1.0, Ink::WidgetText);
            paint_button(r, rt, l.toggles[2], ".*", self.find.regex, true);
            if let Some(f) = r.fonts.clone() {
                let ink = if none && !self.find.find.text.is_empty() { Ink::Error } else { Ink::WidgetText };
                r.text(rt, &status, &f.ui, l.status.l + 4.0, l.status.t, l.status.r, l.status.b, ink);
            }
            paint_button(r, rt, l.prev, "\u{2191}", false, !none);
            paint_button(r, rt, l.next, "\u{2193}", false, !none);
            paint_button(r, rt, l.close, "\u{2715}", false, true);
            if self.find.replace {
                paint_input(r, rt, l.replace_input, &mut self.find.with, focus == Focus::Replace, caret_on, "Replace", 0.0);
                for (rc, label) in [(l.replace_one, "Replace"), (l.replace_all, "All")] {
                    r.stroke(rt, rc.l + 0.5, rc.t + 0.5, rc.r - 0.5, rc.b - 0.5, 1.0, Ink::InputBorder);
                    paint_button(r, rt, rc, label, false, !none);
                }
            }
        }
        if let Some(g) = &mut self.goto {
            let r = &self.render;
            let fr = crate::widgets::goto::GoTo::frame(w);
            r.fill(rt, fr.l + 2.0, fr.t, fr.r + 2.0, fr.b + 3.0, Ink::Shadow);
            r.fill(rt, fr.l, fr.t, fr.r, fr.b, Ink::WidgetBg);
            let n = self.doc.buffer.line_count();
            let hint = format!("Go to line (1\u{2013}{n}), or line:column");
            let rc = crate::widgets::goto::GoTo::input_rect(w);
            paint_input(r, rt, rc, &mut g.input, true, caret_on, &hint, 0.0);
        }
    }
}

/// A window render target for `hwnd` at `dpi`: on the GPU once it is warmed
/// up, in software before. Returns the target and whether it is hardware.
pub(crate) fn hwnd_target(hwnd: HWND, w: u32, h: u32, dpi: f32) -> Result<(ID2D1HwndRenderTarget, bool)> {
    let sh = shared()?;
    let hardware = gpu_ready();
    unsafe {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: if hardware { D2D1_RENDER_TARGET_TYPE_DEFAULT } else { D2D1_RENDER_TARGET_TYPE_SOFTWARE },
            ..Default::default()
        };
        let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U {
                width: w.max(1),
                height: h.max(1),
            },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = sh.d2d.CreateHwndRenderTarget(&props, &hprops)?;
        rt.SetDpi(dpi, dpi);
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
        Ok((rt, hardware))
    }
}

/// One row being painted: columns `a..b` of model line `line`.
struct Seg {
    row: usize,
    line: usize,
    a: usize,
    b: usize,
    /// The row is its line's first or last.
    first: bool,
    last: bool,
    y: f32,
    lay: Rc<LineLayout>,
}

impl Seg {
    /// x (in text coordinates) of column `col` of the line, which must be in
    /// this row.
    fn x(&self, col: usize, char_w: f32) -> f32 {
        self.lay.x_of(col.clamp(self.a, self.b) - self.a, char_w)
    }
}

impl crate::editor::State {
    /// Paints the whole editor at the target's current transform, in a box of
    /// `view.width` x `view.height` DIPs at the origin.
    pub(crate) fn paint(&mut self, rt: &ID2D1RenderTarget) {
        self.ensure_fonts();
        if self.render.bind(rt).is_err() {
            return;
        }
        self.view.lines = self.rows.rows(self.doc.buffer.line_count());
        self.view.clamp_scroll();
        self.render.begin_frame();
        let (w, h) = (self.view.width, self.view.height);
        let lh = self.view.line_h;
        let vis = self.view.visible();
        // The model lines the visible rows show.
        let spans: Vec<(usize, usize, usize, usize)> = vis
            .clone()
            .map(|row| {
                let (line, a, b) = self.row_span(row);
                (row, line, a, b)
            })
            .collect();
        let lines = match (spans.first(), spans.last()) {
            (Some(f), Some(l)) => f.1..l.1 + 1,
            _ => 0..0,
        };
        self.ensure_highlight(lines.clone());
        let brackets = if self.opts.match_brackets { self.bracket_pair() } else { None };
        let guides = if self.opts.indent_guides { self.indent_guides(lines.clone()) } else { Vec::new() };
        let marks = self.occurrence_marks(lines.clone());
        let folds = if self.opts.folding {
            self.fold_regions(lines.start, lines.end.saturating_sub(1))
        } else {
            Vec::new()
        };
        self.update_find();
        let found: Vec<(usize, usize)> = if self.find.open {
            let (a, e) = (self.doc.buffer.line_start(lines.start), self.doc.buffer.line_start(lines.end));
            let m = &self.find.matches;
            let i = m.partition_point(|x| x.1 < a);
            m[i..].iter().take_while(|x| x.0 <= e).copied().collect()
        } else {
            Vec::new()
        };
        let segs: Vec<Seg> = spans
            .iter()
            .map(|&(row, line, a, b)| {
                let len = self.doc.buffer.line_len(line);
                Seg {
                    row,
                    line,
                    a,
                    b,
                    first: a == 0,
                    last: b >= len,
                    y: self.view.line_y(row),
                    lay: self.seg_layout(line, a, b),
                }
            })
            .collect();
        let r = &self.render;
        let v = &self.view;
        let buf = &self.doc.buffer;
        let sels = &self.doc.sels;
        let text_x = v.text_x() - v.scroll_x;
        let gutter_w = v.gutter_w();
        let char_w = v.char_w;
        // Rectangles (x0, x1, y) covering offsets a..e on the visible rows.
        let range_rects = |a: usize, e: usize| -> Vec<(f32, f32, f32)> {
            let mut out = Vec::new();
            for s in &segs {
                let ls = buf.line_start(s.line);
                let (sa, sb) = (ls + s.a, ls + s.b);
                if e < sa || a > sb || (e == sa && a < sa) || (a == sb && !s.last && e > sb) {
                    continue;
                }
                let (ca, ce) = (a.max(sa) - ls, e.min(sb) - ls);
                out.push((text_x + s.x(ca, char_w), text_x + s.x(ce, char_w), s.y));
            }
            out
        };
        let caret_rows: Vec<usize> = sels.iter().map(|s| self.row_of(s.head)).collect();
        let caret_lines: Vec<usize> = sels.iter().map(|s| buf.line_of(s.head)).collect();
        unsafe {
            rt.PushAxisAlignedClip(&rect(0.0, 0.0, w, h), D2D1_ANTIALIAS_MODE_ALIASED);
            rt.Clear(Some(&color(r.palette.get(Ink::Bg))));

            if self.opts.highlight_current_line && sels.iter().all(|s| s.is_empty()) {
                for s in segs.iter().filter(|s| caret_rows.contains(&s.row)) {
                    r.fill(rt, gutter_w, s.y, w, s.y + lh, Ink::CurrentLine);
                }
            }

            // Decoration line backgrounds.
            let (vis_from, vis_to) = (buf.line_start(lines.start), buf.line_start(lines.end));
            let decos: Vec<&crate::decorations::Tracked> = self.decorations.in_range(vis_from, vis_to).collect();
            for d in &decos {
                if let Some(c) = d.options.line_background {
                    let (a, e) = (buf.line_of(d.start), buf.line_of(d.end));
                    for s in segs.iter().filter(|s| s.line >= a && s.line <= e) {
                        r.fill_argb(rt, gutter_w, s.y, w, s.y + lh, c);
                    }
                }
            }

            rt.PushAxisAlignedClip(&rect(gutter_w, 0.0, w, h), D2D1_ANTIALIAS_MODE_ALIASED);
            for d in &decos {
                if let Some(c) = d.options.background {
                    for (x0, x1, y) in range_rects(d.start, d.end) {
                        r.fill_argb(rt, x0, y, x1, y + lh, c);
                    }
                }
            }
            // Indent guides, on each row of a line.
            let tab_w = char_w * self.opts.tab_size.max(1) as f32;
            for g in &guides {
                for s in segs.iter().filter(|s| s.line == g.line) {
                    for i in 0..g.levels {
                        let x = (text_x + i as f32 * tab_w).floor() + 0.5;
                        let ink = if g.active == Some(i) { Ink::IndentGuideActive } else { Ink::IndentGuide };
                        r.line(rt, x, s.y, x, s.y + lh, 1.0, ink);
                    }
                }
            }
            // Find matches, then other occurrences of the word or selection.
            let prim = sels.primary();
            for &(a, e) in &found {
                let ink = if (a, e) == (prim.start(), prim.end()) { Ink::FindMatchCurrent } else { Ink::FindMatch };
                for (x0, x1, y) in range_rects(a, e) {
                    r.fill(rt, x0, y, x1, y + lh, ink);
                }
            }
            for &(a, e) in &marks {
                for (x0, x1, y) in range_rects(a, e) {
                    r.fill(rt, x0, y, x1, y + lh, Ink::WordHighlight);
                }
            }
            // Selections; a selection running past a line end shows a sliver.
            let sel_ink = if self.focused { Ink::Selection } else { Ink::SelectionInactive };
            for s in sels.iter().filter(|s| !s.is_empty()) {
                let end_line = buf.line_of(s.end());
                for (x0, x1, y) in range_rects(s.start(), s.end()) {
                    let seg = segs.iter().find(|g| g.y == y);
                    let more = seg.is_some_and(|g| g.last && g.line < end_line);
                    let x1 = if more { x1 + char_w * 0.5 } else { x1 };
                    r.fill(rt, x0, y, x1, y + lh, sel_ink);
                }
            }
            // Matching brackets.
            if let Some((a, m)) = brackets {
                for at in [a, m] {
                    let n = buf.char_at(at).map_or(1, char::len_utf8);
                    for (x0, x1, y) in range_rects(at, at + n) {
                        r.fill(rt, x0, y, x1, y + lh, Ink::BracketMatch);
                        r.stroke(rt, x0 + 0.5, y + 0.5, x1 - 0.5, y + lh - 0.5, 1.0, Ink::BracketMatchBorder);
                    }
                }
            }
            // Text.
            for s in &segs {
                r.draw_line(rt, &s.lay, text_x, s.y);
            }
            // A folded line ends with a "⋯" marker.
            for s in segs.iter().filter(|s| s.last && self.is_folded(s.line)) {
                let x = text_x + s.lay.width + char_w;
                let (x0, x1) = (x, x + char_w * 3.0);
                r.fill(rt, x0, s.y + 3.0, x1, s.y + lh - 3.0, Ink::FoldedBg);
                if let Some(f) = &r.fonts {
                    r.text(rt, "\u{22EF}", &f.ui_center, x0, s.y, x1, s.y + lh, Ink::FoldIcon);
                }
            }
            // Squiggles under decorated ranges.
            for d in &decos {
                let Some(sev) = d.options.squiggle else { continue };
                let ink = match sev {
                    crate::decorations::Severity::Error => Ink::Error,
                    crate::decorations::Severity::Warning => Ink::Warning,
                    crate::decorations::Severity::Info => Ink::Info,
                };
                for (x0, x1, y) in range_rects(d.start, d.end) {
                    let x1 = x1.max(x0 + char_w);
                    let base = y + lh - 2.5;
                    let mut x = x0;
                    let mut up = true;
                    while x < x1 {
                        let nx = (x + 2.0).min(x1);
                        let (y0, y1) = if up { (base, base - 2.0) } else { (base - 2.0, base) };
                        r.line(rt, x, y0, nx, y1, 1.0, ink);
                        x = nx;
                        up = !up;
                    }
                }
            }
            // Whitespace: dots for spaces, arrows for tabs.
            if self.opts.render_whitespace {
                for s in &segs {
                    let y = s.y + lh / 2.0;
                    let lay = &s.lay;
                    for (i, c) in lay.text.char_indices() {
                        if c != ' ' && c != '\t' {
                            continue;
                        }
                        let (x0, x1) = (text_x + lay.x_of(i, char_w), text_x + lay.x_of(i + 1, char_w));
                        if c == ' ' {
                            let cx = (x0 + x1) / 2.0;
                            r.fill(rt, cx - 1.0, y - 1.0, cx + 1.0, y + 1.0, Ink::Whitespace);
                        } else {
                            let (a, b) = (x0 + 2.0, x1 - 2.0);
                            r.line(rt, a, y, b, y, 1.0, Ink::Whitespace);
                            r.line(rt, b - 3.0, y - 3.0, b, y, 1.0, Ink::Whitespace);
                            r.line(rt, b - 3.0, y + 3.0, b, y, 1.0, Ink::Whitespace);
                        }
                    }
                }
            }
            // Carets (not while an overlay input has focus).
            if self.focused && self.caret_on && self.focus == crate::widgets::Focus::Text {
                for (s, &row) in sels.iter().zip(&caret_rows) {
                    let p = buf.pos(s.head);
                    if let Some(g) = segs.iter().find(|g| g.row == row) {
                        let x = (text_x + g.x(p.col, char_w)).round();
                        r.fill(rt, x, g.y, x + 2.0, g.y + lh, Ink::Caret);
                    }
                }
            }
            rt.PopAxisAlignedClip();

            // Gutter: line numbers on each line's first row, fold chevrons.
            if let Some(f) = &r.fonts {
                let right = v.numbers_w();
                for s in segs.iter().filter(|s| s.first) {
                    if v.line_numbers {
                        let ink = if caret_lines.contains(&s.line) { Ink::LineNumberActive } else { Ink::LineNumber };
                        r.text(rt, &(s.line + 1).to_string(), &f.numbers, 0.0, s.y, right, s.y + lh, ink);
                    }
                    if v.folding {
                        let folded = self.is_folded(s.line);
                        let header = folded || folds.iter().any(|r| r.0 == s.line);
                        if header && (folded || self.gutter_hover) {
                            let icon = if folded { "\u{203A}" } else { "\u{2304}" };
                            let x0 = right + 2.0;
                            r.text(rt, icon, &f.ui_center, x0, s.y, x0 + char_w * 2.0, s.y + lh, Ink::FoldIcon);
                        }
                    }
                }
            }

            // Gutter markers.
            for d in &decos {
                if let Some(c) = d.options.gutter_marker {
                    let row = self.row_of(d.start);
                    if let Some(s) = segs.iter().find(|s| s.row == row) {
                        r.fill_argb(rt, 2.0, s.y + 2.0, 5.0, s.y + lh - 2.0, c);
                    }
                }
            }

            // Overlay scrollbars.
            let sb = crate::view::SCROLLBAR;
            if let Some((top, th)) = v.vthumb() {
                let ink = if self.hover == Some(crate::view::Region::VScroll)
                    || matches!(self.drag, Some(crate::editor::Drag::VThumb { .. }))
                {
                    Ink::ScrollbarThumbHover
                } else {
                    Ink::ScrollbarThumb
                };
                r.fill(rt, w - sb, top, w, top + th, ink);
            }
            if let Some((left, tw)) = v.hthumb() {
                let ink = if self.hover == Some(crate::view::Region::HScroll)
                    || matches!(self.drag, Some(crate::editor::Drag::HThumb { .. }))
                {
                    Ink::ScrollbarThumbHover
                } else {
                    Ink::ScrollbarThumb
                };
                let x = v.text_x() + left;
                r.fill(rt, x, h - sb + 4.0, x + tw, h, ink);
            }
            self.paint_widgets(rt);
            self.paint_suggest(rt);
            self.paint_tooltip(rt);
            rt.PopAxisAlignedClip();
        }
        self.render.end_frame(vis.len());
    }
}
