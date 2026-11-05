//! A gallery of the charts, drawn with Direct2D: an overview of every
//! example as cards, and each example on its own at full size. Hovering a
//! bar shows its tooltip.
//!
//! Run with `cargo run -p diver_charts --example gallery`.
//!
//! Keys: Up/Down pick an example, Home shows the overview, T switches
//! between the light and dark themes.
//!
//! `--capture out.png` paints one frame to a PNG and exits; `--sample N`
//! opens example N (0 is the first), `--dark` uses the dark theme, `--size
//! 1400x900` sets the picture's size, and `--hover X,Y` (in DIPs) points
//! there first.

#![windows_subsystem = "windows"]

mod gfx;
mod samples;

use std::cell::RefCell;
use std::path::PathBuf;

use diver_charts::{Color, Font, Rect, Scene, Theme};
use gfx::{Fonts, Gfx};
use samples::Sample;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// Not in the features this crate builds `windows` with.
const WM_MOUSELEAVE: u32 = 0x02a3;

const SIDEBAR_W: f32 = 230.0;
const ENTRY_H: f32 = 28.0;
const MARGIN: f32 = 24.0;
const CARD_MIN_W: f32 = 440.0;
const CARD_H: f32 = 320.0;
const HEADER_H: f32 = 64.0;

const UI: Font = Font::new(12.5, false);
const UI_BOLD: Font = Font::new(12.5, true);
const SMALL: Font = Font::new(11.0, false);
const SECTION: Font = Font::new(10.5, true);
const HEADING: Font = Font::new(17.0, true);

/// Colors of the gallery around the charts, for each theme.
struct Chrome {
    page: Color,
    sidebar: Color,
    line: Color,
    text: Color,
    dim: Color,
    hover: Color,
    selected: Color,
}

impl Chrome {
    fn new(theme: &Theme) -> Chrome {
        if theme.dark {
            Chrome {
                page: Color::hex(0x0d0d0d),
                sidebar: Color::hex(0x161615),
                line: Color::hex(0x2c2c2a),
                text: theme.text,
                dim: theme.muted,
                hover: Color::hex(0x232321),
                selected: Color::hex(0x1f3550),
            }
        } else {
            Chrome {
                page: Color::hex(0xf3f3f1),
                sidebar: Color::hex(0xf9f9f7),
                line: Color::hex(0xe1e0d9),
                text: theme.text,
                dim: theme.text2,
                hover: Color::hex(0xecebe6),
                selected: Color::hex(0xdbe8f8),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Overview,
    Sample(usize),
    Theme,
}

struct App {
    factory: ID2D1Factory,
    fonts: Fonts,
    target: Option<ID2D1HwndRenderTarget>,
    samples: Vec<Sample>,
    /// The example shown on its own, or the overview.
    open: Option<usize>,
    dark: bool,
    scroll: f32,
    max_scroll: f32,
    /// What the last frame drew, for finding things under the pointer.
    hits: Vec<(Rect, Hit)>,
    charts: Vec<(Rect, Scene)>,
    hover: Option<Hit>,
    /// The chart and mark under the pointer.
    hover_mark: Option<(usize, usize)>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|a| a.try_borrow_mut().ok()?.as_mut().map(f))
}

impl App {
    fn theme(&self) -> Theme {
        if self.dark { Theme::dark() } else { Theme::light() }
    }

    fn hit(&self, p: (f32, f32)) -> Option<Hit> {
        self.hits.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, h)| *h)
    }

    fn mark_at(&self, (x, y): (f32, f32)) -> Option<(usize, usize)> {
        self.charts.iter().enumerate().find_map(|(i, (r, scene))| {
            if !r.contains((x, y)) {
                return None;
            }
            scene.hit((x - r.x, y - r.y)).map(|m| (i, m))
        })
    }

    /// Moves the pointer to `p`; true when what it is over changed.
    fn point(&mut self, p: Option<(f32, f32)>) -> bool {
        let hover = p.and_then(|p| self.hit(p));
        let mark = p.and_then(|p| self.mark_at(p));
        let changed = hover != self.hover || mark != self.hover_mark;
        self.hover = hover;
        self.hover_mark = mark;
        changed
    }

    fn click(&mut self, hit: Hit) {
        match hit {
            Hit::Overview => self.show(None),
            Hit::Sample(i) => self.show(Some(i)),
            Hit::Theme => self.dark = !self.dark,
        }
    }

    fn show(&mut self, open: Option<usize>) {
        self.open = open;
        self.scroll = 0.0;
        self.hover_mark = None;
    }

    fn key(&mut self, vk: VIRTUAL_KEY) -> bool {
        let n = self.samples.len();
        match vk {
            VK_DOWN => self.show(Some(self.open.map_or(0, |i| (i + 1).min(n - 1)))),
            VK_UP => self.show(match self.open {
                None | Some(0) => None,
                Some(i) => Some(i - 1),
            }),
            VK_HOME => self.show(None),
            VK_T => self.dark = !self.dark,
            _ => return false,
        }
        true
    }

    fn paint(&mut self, g: &Gfx, width: f32, height: f32) {
        let theme = self.theme();
        let chrome = Chrome::new(&theme);
        self.hits.clear();
        self.charts.clear();
        g.fill(Rect::new(0.0, 0.0, width, height), chrome.page);
        let main = Rect::new(SIDEBAR_W + 1.0, 0.0, width - SIDEBAR_W - 1.0, height);
        match self.open {
            None => self.paint_overview(g, main, &theme, &chrome),
            Some(i) => self.paint_sample(g, main, i, &theme, &chrome),
        }
        self.paint_sidebar(g, height, &chrome);
        // The tooltip goes over everything, kept within its chart.
        if let Some((c, m)) = self.hover_mark
            && let Some((r, scene)) = self.charts.get(c)
        {
            let items = scene.tooltip(m, &theme, g.fonts);
            g.items(&items, (r.x, r.y), None, &theme);
        }
    }

    fn entry(&mut self, g: &Gfx, r: Rect, label: &str, hit: Hit, selected: bool, chrome: &Chrome) {
        if selected {
            g.fill_rounded(r, 4.0, chrome.selected);
        } else if self.hover == Some(hit) {
            g.fill_rounded(r, 4.0, chrome.hover);
        }
        let font = if selected { UI_BOLD } else { UI };
        g.text(label, font, r.inset(12.0, 0.0, 8.0, 0.0), chrome.text);
        self.hits.push((r, hit));
    }

    fn paint_sidebar(&mut self, g: &Gfx, height: f32, chrome: &Chrome) {
        g.fill(Rect::new(0.0, 0.0, SIDEBAR_W, height), chrome.sidebar);
        g.fill(Rect::new(SIDEBAR_W, 0.0, 1.0, height), chrome.line);
        g.text("Chart gallery", HEADING, Rect::new(18.0, 14.0, SIDEBAR_W - 36.0, 28.0), chrome.text);
        let foot = Rect::new(0.0, height - 74.0, SIDEBAR_W, 74.0);
        // A short window cuts the list off above the footer.
        g.clip(Rect::new(0.0, 0.0, SIDEBAR_W, foot.y));
        let first = self.hits.len();
        let mut y = 54.0;
        let row = |y: f32| Rect::new(8.0, y, SIDEBAR_W - 16.0, ENTRY_H - 2.0);
        self.entry(g, row(y), "Overview", Hit::Overview, self.open.is_none(), chrome);
        y += ENTRY_H + 8.0;
        let mut group = "";
        for i in 0..self.samples.len() {
            let (sample_group, name) = (self.samples[i].group, self.samples[i].name);
            if sample_group != group {
                group = sample_group;
                g.text(&group.to_uppercase(), SECTION, Rect::new(20.0, y, SIDEBAR_W - 40.0, 22.0), chrome.dim);
                y += 24.0;
            }
            self.entry(g, row(y), name, Hit::Sample(i), self.open == Some(i), chrome);
            y += ENTRY_H;
        }
        g.unclip();
        // Entries hidden under the footer take no clicks.
        let mut n = 0;
        self.hits.retain(|(r, _)| {
            n += 1;
            n <= first || r.y < foot.y
        });

        g.fill(Rect::new(0.0, foot.y, SIDEBAR_W, 1.0), chrome.line);
        let button = Rect::new(14.0, foot.y + 12.0, SIDEBAR_W - 28.0, 26.0);
        let bg = if self.hover == Some(Hit::Theme) { chrome.hover } else { chrome.page };
        g.fill_rounded(button, 4.0, bg);
        g.outline_rounded(button, 4.0, chrome.line, 1.0);
        let label = if self.dark { "Dark theme  ·  switch to light" } else { "Light theme  ·  switch to dark" };
        g.text(label, SMALL, button.inset(10.0, 0.0, 6.0, 0.0), chrome.text);
        self.hits.push((button, Hit::Theme));
        g.text("↑↓ examples   Home overview   T theme", SMALL, Rect::new(16.0, foot.y + 44.0, SIDEBAR_W - 32.0, 20.0), chrome.dim);
    }

    /// Draws a chart in a card at `r`, and keeps its scene for the pointer.
    fn card(&mut self, g: &Gfx, r: Rect, chart: Scene, theme: &Theme, chrome: &Chrome) {
        let hover = self.hover_mark.filter(|(c, _)| *c == self.charts.len()).map(|(_, m)| m);
        g.fill(r.grow(1.0), chrome.line);
        g.clip(r);
        g.items(&chart.items, (r.x, r.y), hover, theme);
        g.unclip();
        self.charts.push((r, chart));
    }

    fn paint_overview(&mut self, g: &Gfx, area: Rect, theme: &Theme, chrome: &Chrome) {
        let inner = area.inset(MARGIN, MARGIN, MARGIN, 0.0);
        let cols = ((inner.w + MARGIN) / (CARD_MIN_W + MARGIN)).floor().max(1.0) as usize;
        let card_w = (inner.w - MARGIN * (cols as f32 - 1.0)) / cols as f32;
        let rows = self.samples.len().div_ceil(cols);
        let content = rows as f32 * (CARD_H + MARGIN) + MARGIN;
        self.max_scroll = (content - area.h).max(0.0);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll);
        g.clip(area);
        for i in 0..self.samples.len() {
            let x = inner.x + (i % cols) as f32 * (card_w + MARGIN);
            let y = inner.y + (i / cols) as f32 * (CARD_H + MARGIN) - self.scroll;
            let r = Rect::new(x.round(), y.round(), card_w.floor(), CARD_H);
            if r.bottom() < area.y || r.y > area.bottom() {
                continue;
            }
            let scene = self.samples[i].chart.layout(r.w, r.h, theme, g.fonts);
            self.card(g, r, scene, theme, chrome);
            // A click on the card's empty parts opens it.
            self.hits.push((r, Hit::Sample(i)));
        }
        g.unclip();
    }

    fn paint_sample(&mut self, g: &Gfx, area: Rect, i: usize, theme: &Theme, chrome: &Chrome) {
        self.max_scroll = 0.0;
        let sample = &self.samples[i];
        let head = Rect::new(area.x + MARGIN, area.y + 14.0, area.w - 2.0 * MARGIN, 26.0);
        g.text(sample.name, HEADING, head, chrome.text);
        g.text(sample.about, UI, Rect::new(head.x, head.bottom(), head.w, 20.0), chrome.dim);
        let r = area.inset(MARGIN, HEADER_H, MARGIN, MARGIN);
        let r = Rect::new(r.x.round(), r.y.round(), r.w.floor(), r.h.floor());
        let scene = sample.chart.layout(r.w, r.h, theme, g.fonts);
        self.card(g, r, scene, theme, chrome);
    }
}

fn paint_window(hwnd: HWND) {
    with_app(|app| unsafe {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let dpi = GetDpiForWindow(hwnd) as f32;
        let size = D2D_SIZE_U { width: rc.right as u32, height: rc.bottom as u32 };
        if app.target.is_none() {
            let props = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: size, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
            app.target = app.factory.CreateHwndRenderTarget(&Default::default(), &props).ok();
        }
        let Some(target) = app.target.clone() else { return };
        target.SetDpi(dpi, dpi);
        let _ = target.Resize(&size);
        target.BeginDraw();
        let factory = app.factory.clone();
        let fonts = std::ptr::addr_of!(app.fonts);
        if let Ok(g) = Gfx::new(&target, &factory, &*fonts) {
            let scale = dpi / 96.0;
            app.paint(&g, rc.right as f32 / scale, rc.bottom as f32 / scale);
        }
        if target.EndDraw(None, None).is_err() {
            app.target = None;
        }
    });
}

fn dips(hwnd: HWND, lparam: LPARAM) -> (f32, f32) {
    let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    (x / scale, y / scale)
}

fn invalidate(hwnd: HWND) {
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Repaints now, then finds what is under the pointer in the new frame.
fn repaint_and_point(hwnd: HWND, p: Option<(f32, f32)>) {
    paint_window(hwnd);
    if with_app(|app| app.point(p)) == Some(true) {
        invalidate(hwnd);
    }
}

fn cursor(hwnd: HWND) -> Option<(f32, f32)> {
    unsafe {
        let mut p = POINT::default();
        GetCursorPos(&mut p).ok()?;
        let _ = ScreenToClient(hwnd, &mut p);
        let scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        Some((p.x as f32 / scale, p.y as f32 / scale))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                paint_window(hwnd);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let p = dips(hwnd, lparam);
                with_app(|app| {
                    if let Some(hit) = app.hit(p) {
                        app.click(hit);
                    }
                });
                repaint_and_point(hwnd, Some(p));
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut track);
                let p = dips(hwnd, lparam);
                if with_app(|app| app.point(Some(p))) == Some(true) {
                    invalidate(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if with_app(|app| app.point(None)) == Some(true) {
                    invalidate(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xffff) as i16 as f32;
                with_app(|app| app.scroll = (app.scroll - delta * 0.8).clamp(0.0, app.max_scroll));
                repaint_and_point(hwnd, cursor(hwnd));
                invalidate(hwnd);
                LRESULT(0)
            }
            WM_KEYDOWN => {
                if with_app(|app| app.key(VIRTUAL_KEY(wparam.0 as u16))) == Some(true) {
                    repaint_and_point(hwnd, cursor(hwnd));
                    invalidate(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

struct Args {
    capture: Option<PathBuf>,
    sample: Option<usize>,
    dark: bool,
    size: Option<(u32, u32)>,
    hover: Option<(f32, f32)>,
}

fn pair<T: std::str::FromStr>(s: Option<String>, sep: char) -> Option<(T, T)> {
    let s = s?;
    let (a, b) = s.split_once(sep)?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn args() -> Args {
    let mut out = Args { capture: None, sample: None, dark: false, size: None, hover: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--capture" => out.capture = it.next().map(PathBuf::from),
            "--sample" => out.sample = it.next().and_then(|n| n.parse().ok()),
            "--dark" => out.dark = true,
            "--size" => out.size = pair(it.next(), 'x'),
            "--hover" => out.hover = pair(it.next(), ','),
            _ => {}
        }
    }
    out
}

/// Paints a frame at 96 DPI into a bitmap, and saves it when given a path.
fn capture(path: Option<&std::path::Path>, width: u32, height: u32) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = wic.CreateBitmap(width, height, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        };
        with_app(|app| -> Result<()> {
            let target = app.factory.CreateWicBitmapRenderTarget(&bitmap, &props)?;
            target.BeginDraw();
            let factory = app.factory.clone();
            let fonts = std::ptr::addr_of!(app.fonts);
            let g = Gfx::new(&target, &factory, &*fonts)?;
            app.paint(&g, width as f32, height as f32);
            target.EndDraw(None, None)
        })
        .unwrap_or(Ok(()))?;
        let Some(path) = path else { return Ok(()) };
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(Error::empty)?;
        frame.Initialize(None)?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(&bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}

/// A windowed app has no console, so a panic would just close it: say what
/// happened instead, and keep it in a file too.
fn report_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("{info}\n\n{}", std::backtrace::Backtrace::force_capture());
        let path = std::env::temp_dir().join("diver_charts_gallery_panic.txt");
        let _ = std::fs::write(&path, &text);
        default(info);
        let message = HSTRING::from(format!("{info}\n\nDetails: {}", path.display()));
        unsafe {
            MessageBoxW(None, &message, w!("Gallery panicked"), MB_ICONERROR);
        }
    }));
}

fn main() -> Result<()> {
    report_panics();
    let args = args();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: instance,
            lpszClassName: w!("DiverChartsGallery"),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let samples = samples::all();
        let open = args.sample.map(|i| i.min(samples.len() - 1));
        let app = App {
            factory,
            fonts: Fonts::new()?,
            target: None,
            samples,
            open,
            dark: args.dark,
            scroll: 0.0,
            max_scroll: 0.0,
            hits: Vec::new(),
            charts: Vec::new(),
            hover: None,
            hover_mark: None,
        };
        APP.with(|a| *a.borrow_mut() = Some(app));

        if let Some(path) = &args.capture {
            let (w, h) = args.size.unwrap_or((1400, 900));
            if let Some(p) = args.hover {
                capture(None, w, h)?;
                with_app(|app| app.point(Some(p)));
            }
            let result = capture(Some(path), w, h);
            APP.with(|a| a.borrow_mut().take());
            return result;
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("DiverChartsGallery"),
            w!("Chart gallery"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1500,
            960,
            None,
            None,
            Some(instance),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|a| a.borrow_mut().take());
        Ok(())
    }
}
