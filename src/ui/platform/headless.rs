//! Running views without a window: painting into a bitmap, saving PNG
//! screenshots, and driving input from tests.

use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::core::HSTRING;

use super::app::{Cx, Event, MouseButton, OpenRequest, Runtime, View, WindowId, WindowOptions};
use super::geometry::{Point, Rect};
use super::gfx::{Brushes, HitRegion};
use super::keys::{KeyEvent, Modifiers};
use super::window::{WindowSlot, d2d_factory, new_slot, paint_into};

fn wic_factory() -> windows::core::Result<IWICImagingFactory> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
    }
}

/// Paints `slot` into a new bitmap of its size at `scale`.
fn paint_bitmap<V: View>(runtime: &Runtime<V>, slot: &WindowSlot<V>, scale: f32) -> windows::core::Result<IWICBitmap> {
    let (width, height) = slot.viewport();
    let factory = wic_factory()?;
    let bitmap = unsafe {
        factory.CreateBitmap(
            (width * scale).ceil().max(1.0) as u32,
            (height * scale).ceil().max(1.0) as u32,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapCacheOnDemand,
        )?
    };
    let properties = D2D1_RENDER_TARGET_PROPERTIES {
        r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0 * scale,
        dpiY: 96.0 * scale,
        ..Default::default()
    };
    let target = unsafe { d2d_factory().CreateWicBitmapRenderTarget(&bitmap, &properties)? };
    let mut brushes = Brushes::default();
    unsafe { target.BeginDraw() };
    paint_into(runtime, slot, &target, &mut brushes);
    unsafe { target.EndDraw(None, None)? };
    Ok(bitmap)
}

fn save_png(bitmap: &IWICBitmap, path: &Path) -> windows::core::Result<()> {
    let factory = wic_factory()?;
    unsafe {
        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), 0x4000_0000)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(windows::core::Error::empty)?;
        frame.Initialize(None)?;
        let mut width = 0;
        let mut height = 0;
        bitmap.GetSize(&mut width, &mut height)?;
        frame.SetSize(width, height)?;
        let mut format = GUID_WICPixelFormat32bppPBGRA;
        frame.SetPixelFormat(&mut format)?;
        frame.WriteSource(bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}

/// Paints window `id` and saves it as a PNG at 1:1 scale.
pub fn capture<V: View>(runtime: &Runtime<V>, id: WindowId, path: &Path) -> Result<(), String> {
    let slot = runtime.window(id).ok_or("the window is gone")?;
    let bitmap = paint_bitmap(runtime, &slot, 1.0).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    save_png(&bitmap, path).map_err(|error| error.to_string())
}

/// An app without windows on screen, for tests. Windows open at a fixed size
/// and paint into bitmaps.
pub struct TestApp<V: View> {
    runtime: Box<Runtime<V>>,
}

fn open_headless<V: View>(runtime: &Runtime<V>, request: OpenRequest<V>) {
    let mut cx = Cx {
        runtime,
        window: request.id,
    };
    let size = match request.options.placement {
        Some(placement) => (placement.width, placement.height),
        None => (request.options.width, request.options.height),
    };
    let view = (request.build)(&mut cx);
    let slot = new_slot(request.id, view, None, size);
    if let Some(placement) = request.options.placement {
        slot.set_origin(placement.x, placement.y);
    }
    runtime.windows.borrow_mut().push(slot);
}

fn close_headless<V: View>(runtime: &Runtime<V>, id: WindowId) {
    let Some(slot) = runtime.window(id) else {
        return;
    };
    if let Ok(mut view) = slot.view.try_borrow_mut() {
        let mut cx = Cx { runtime, window: id };
        view.closing(&mut cx);
    }
    runtime.windows.borrow_mut().retain(|other| other.id != id);
}

impl<V: View> Default for TestApp<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: View> TestApp<V> {
    pub fn new() -> Self {
        Self {
            runtime: Box::new(Runtime::new()),
        }
    }

    pub fn runtime(&self) -> &Runtime<V> {
        &self.runtime
    }

    /// Opens a `width` × `height` window whose view `build` makes.
    pub fn open(&self, width: f32, height: f32, build: impl FnOnce(&mut Cx<V>) -> V + 'static) -> WindowId {
        let id = self.runtime.next_window_id();
        open_headless(
            &self.runtime,
            OpenRequest {
                options: WindowOptions {
                    width,
                    height,
                    visible: false,
                    ..Default::default()
                },
                build: Box::new(build),
                id,
            },
        );
        self.drain();
        id
    }

    pub fn windows(&self) -> Vec<WindowId> {
        self.runtime.windows.borrow().iter().map(|slot| slot.id).collect()
    }

    fn slot(&self, id: WindowId) -> Rc<WindowSlot<V>> {
        self.runtime.window(id).expect("the window is open")
    }

    fn drain(&self) -> bool {
        let mut ran = false;
        while self
            .runtime
            .drain(&|runtime, request| open_headless(runtime, request), &|runtime, id| close_headless(runtime, id))
        {
            ran = true;
        }
        ran
    }

    /// Runs `f` with window `id`'s view, then any work it queued.
    pub fn update<R>(&self, id: WindowId, f: impl FnOnce(&mut V, &mut Cx<V>) -> R) -> R {
        let slot = self.slot(id);
        let result = {
            let mut view = slot.view.borrow_mut();
            let mut cx = Cx {
                runtime: &self.runtime,
                window: id,
            };
            f(&mut view, &mut cx)
        };
        self.drain();
        result
    }

    pub fn read<R>(&self, id: WindowId, f: impl FnOnce(&V) -> R) -> R {
        let slot = self.slot(id);
        let view = slot.view.borrow();
        f(&view)
    }

    /// Delivers `event` to window `id`, returning whether the view used it.
    pub fn dispatch(&self, id: WindowId, event: Event<V::Hit>) -> bool {
        self.update(id, |view, cx| view.event(&event, cx))
    }

    /// Presses a key written like `ctrl-shift-p`, typing its text when the
    /// view does not use the press.
    pub fn press(&self, id: WindowId, stroke: &str) {
        let event = KeyEvent::parse(stroke).expect("valid keystroke");
        let text = event.typed().map(str::to_owned);
        if !self.dispatch(id, Event::Key(event))
            && let Some(text) = text
        {
            self.dispatch(id, Event::Text(text));
        }
    }

    /// Types `text` into the focused input.
    pub fn type_text(&self, id: WindowId, text: &str) {
        for ch in text.chars() {
            self.dispatch(id, Event::Text(ch.to_string()));
        }
    }

    /// Paints window `id`, updating its hit regions.
    pub fn render(&self, id: WindowId) {
        let slot = self.slot(id);
        if let Err(error) = paint_bitmap(&self.runtime, &slot, 1.0) {
            panic!("render failed: {error}");
        }
        self.drain();
    }

    pub fn resize(&self, id: WindowId, width: f32, height: f32) {
        self.slot(id).set_size(width, height);
        self.dispatch(id, Event::Moved);
    }

    pub fn hits(&self, id: WindowId) -> Vec<HitRegion<V::Hit>> {
        self.slot(id).state.borrow().hits.clone()
    }

    /// The rectangle of the last frame's top-most region matching `matches`.
    pub fn find(&self, id: WindowId, matches: impl Fn(&V::Hit) -> bool) -> Option<Rect> {
        self.hits(id)
            .into_iter()
            .rev()
            .find(|region| matches(&region.hit))
            .map(|region| region.rect)
    }

    fn hit_at(&self, id: WindowId, pos: Point) -> Option<V::Hit> {
        self.slot(id).hit_at(pos)
    }

    pub fn mouse_down(&self, id: WindowId, pos: Point, modifiers: Modifiers, clicks: u32) {
        let hit = self.hit_at(id, pos);
        self.dispatch(
            id,
            Event::MouseDown {
                pos,
                button: MouseButton::Left,
                modifiers,
                clicks,
                hit,
            },
        );
    }

    pub fn mouse_up(&self, id: WindowId, pos: Point) {
        let hit = self.hit_at(id, pos);
        self.dispatch(
            id,
            Event::MouseUp {
                pos,
                button: MouseButton::Left,
                modifiers: Modifiers::NONE,
                hit,
            },
        );
    }

    pub fn mouse_move(&self, id: WindowId, pos: Point, left_down: bool) {
        let hit = self.hit_at(id, pos);
        self.dispatch(
            id,
            Event::MouseMove {
                pos,
                modifiers: Modifiers::NONE,
                left_down,
                hit,
            },
        );
    }

    /// Clicks at `pos` after painting, so the click sees the current layout.
    pub fn click(&self, id: WindowId, pos: Point) {
        self.render(id);
        self.mouse_down(id, pos, Modifiers::NONE, 1);
        self.mouse_up(id, pos);
        self.render(id);
    }

    pub fn right_click(&self, id: WindowId, pos: Point) {
        self.render(id);
        let hit = self.hit_at(id, pos);
        self.dispatch(
            id,
            Event::MouseDown {
                pos,
                button: MouseButton::Right,
                modifiers: Modifiers::NONE,
                clicks: 1,
                hit: hit.clone(),
            },
        );
        self.dispatch(
            id,
            Event::MouseUp {
                pos,
                button: MouseButton::Right,
                modifiers: Modifiers::NONE,
                hit,
            },
        );
    }

    pub fn drag(&self, id: WindowId, from: Point, to: Point) {
        self.render(id);
        self.mouse_down(id, from, Modifiers::NONE, 1);
        self.mouse_move(id, to, true);
        self.mouse_up(id, to);
        self.render(id);
    }

    /// Scrolls the wheel by `lines` at `pos`; positive moves toward the top.
    pub fn scroll(&self, id: WindowId, pos: Point, delta: Point) {
        self.render(id);
        let hit = self.hit_at(id, pos);
        self.dispatch(
            id,
            Event::Wheel {
                pos,
                delta,
                modifiers: Modifiers::NONE,
                hit,
            },
        );
    }

    /// Runs finished background work and due timers until nothing is left
    /// running. Timers due later wait for [`TestApp::advance_clock`].
    pub fn run_until_parked(&self) {
        let started = Instant::now();
        loop {
            let ran = self.drain();
            if !ran && self.runtime.in_flight() == 0 {
                // A result may have arrived after the last drain looked.
                if !self.drain() {
                    return;
                }
            }
            if !ran {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "background work did not finish"
            );
        }
    }

    /// Moves the timer clock forward, then runs whatever became due.
    pub fn advance_clock(&self, by: Duration) {
        self.runtime.advance_clock(by);
        self.run_until_parked();
    }

    pub fn close(&self, id: WindowId) {
        close_headless(&self.runtime, id);
        self.drain();
    }
}

impl<V: View> Drop for TestApp<V> {
    fn drop(&mut self) {
        for id in self.windows() {
            close_headless(&self.runtime, id);
        }
    }
}
