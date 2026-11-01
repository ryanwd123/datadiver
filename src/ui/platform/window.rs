//! Windows: the Win32 window procedure, painting with a Direct2D render
//! target, and input translation. A window can also run headless, painting
//! into a bitmap, for tests and screenshots.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, DragQueryPoint, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use super::app::{Action, Contents, Cx, Event, MouseButton, OpenRequest, Placement, Runtime, View, WindowId};
use super::geometry::{Color, Point, Rect, point, rect};
use super::gfx::{Brushes, Canvas, Cursor, HitRegion};
use super::keys::{KeyEvent, Modifiers};
use super::text::{self, Font, TextOptions};

const WAKE_MESSAGE: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TOOLTIP_DELAY_MS: u128 = 600;

struct Tooltip {
    text: std::rc::Rc<str>,
    pos: Point,
}

pub struct WindowState<H> {
    pub(crate) hwnd: Option<HWND>,
    target: Option<ID2D1HwndRenderTarget>,
    brushes: Brushes,
    pub(crate) hits: Vec<HitRegion<H>>,
    /// Client size in DIPs, for headless windows.
    size: (f32, f32),
    /// Where a headless window would sit on screen.
    origin: (f32, f32),
    scale: f32,
    mouse: Point,
    left_down: bool,
    tracking_leave: bool,
    last_click: Option<(Instant, Point, MouseButton, u32)>,
    surrogate: Option<u16>,
    hover_since: Option<(Instant, Point)>,
    tooltip: Option<Tooltip>,
}

pub struct WindowSlot<V: View> {
    pub id: WindowId,
    /// Shared with the owner's slot when this window shows another's view.
    pub view: Rc<RefCell<V>>,
    /// The window whose view this one shows.
    pub owner: Option<WindowId>,
    pub(crate) state: RefCell<WindowState<V::Hit>>,
    dirty: Cell<bool>,
}

impl<V: View> WindowSlot<V> {
    pub(crate) fn hwnd(&self) -> Option<HWND> {
        self.state.borrow().hwnd
    }

    pub(crate) fn invalidate(&self) {
        self.dirty.set(true);
        if let Some(hwnd) = self.hwnd() {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty.get()
    }

    pub(crate) fn viewport(&self) -> (f32, f32) {
        let state = self.state.borrow();
        match state.hwnd {
            Some(hwnd) => {
                let mut client = RECT::default();
                unsafe {
                    let _ = GetClientRect(hwnd, &mut client);
                }
                (
                    (client.right - client.left) as f32 / state.scale,
                    (client.bottom - client.top) as f32 / state.scale,
                )
            }
            None => state.size,
        }
    }

    /// Places a headless window, as a saved placement would.
    pub(crate) fn set_origin(&self, x: f32, y: f32) {
        self.state.borrow_mut().origin = (x, y);
    }

    pub(crate) fn set_size(&self, width: f32, height: f32) {
        self.state.borrow_mut().size = (width, height);
        self.invalidate();
    }

    pub(crate) fn placement(&self) -> Option<Placement> {
        let Some(hwnd) = self.hwnd() else {
            // A headless window sits where it was placed, at its size.
            let state = self.state.borrow();
            return Some(Placement {
                x: state.origin.0,
                y: state.origin.1,
                width: state.size.0,
                height: state.size.1,
                maximized: false,
            });
        };
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        unsafe { GetWindowPlacement(hwnd, &mut placement).ok()? };
        let normal = placement.rcNormalPosition;
        Some(Placement {
            x: normal.left as f32,
            y: normal.top as f32,
            width: (normal.right - normal.left) as f32,
            height: (normal.bottom - normal.top) as f32,
            maximized: placement.showCmd == SW_SHOWMAXIMIZED.0 as u32,
        })
    }

    /// The window's outer rectangle in screen pixels as it is now, maximized
    /// or not.
    pub(crate) fn outer_rect(&self) -> Option<Rect> {
        let Some(hwnd) = self.hwnd() else {
            let state = self.state.borrow();
            return Some(rect(state.origin.0, state.origin.1, state.size.0, state.size.1));
        };
        let mut outer = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut outer).ok()? };
        Some(rect(
            outer.left as f32,
            outer.top as f32,
            (outer.right - outer.left) as f32,
            (outer.bottom - outer.top) as f32,
        ))
    }

    pub(crate) fn activate(&self) {
        if let Some(hwnd) = self.hwnd() {
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    pub(crate) fn hit_at(&self, pos: Point) -> Option<V::Hit> {
        hit_region(&self.state.borrow().hits, pos).map(|region| region.hit.clone())
    }
}

fn hit_region<H>(hits: &[HitRegion<H>], pos: Point) -> Option<&HitRegion<H>> {
    hits.iter().rev().find(|region| region.rect.contains(pos))
}

pub(crate) fn new_slot<V: View>(
    id: WindowId,
    view: Rc<RefCell<V>>,
    owner: Option<WindowId>,
    hwnd: Option<HWND>,
    size: (f32, f32),
) -> Rc<WindowSlot<V>> {
    let scale = hwnd
        .map(|hwnd| unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0)
        .filter(|scale| *scale > 0.0)
        .unwrap_or(1.0);
    Rc::new(WindowSlot {
        id,
        view,
        owner,
        state: RefCell::new(WindowState {
            hwnd,
            target: None,
            brushes: Brushes::default(),
            hits: Vec::new(),
            size,
            origin: (0.0, 0.0),
            scale,
            mouse: Point::default(),
            left_down: false,
            tracking_leave: false,
            last_click: None,
            surrogate: None,
            hover_since: None,
            tooltip: None,
        }),
        dirty: Cell::new(true),
    })
}

/// Paints `slot` into `target`, recording its hit regions.
pub(crate) fn paint_into<V: View>(runtime: &Runtime<V>, slot: &WindowSlot<V>, target: &ID2D1RenderTarget, brushes: &mut Brushes) -> bool {
    let Ok(mut view) = slot.view.try_borrow_mut() else {
        return false;
    };
    let tooltip = slot
        .state
        .borrow()
        .tooltip
        .as_ref()
        .map(|tooltip| (tooltip.text.clone(), tooltip.pos));
    let mouse = slot.state.borrow().mouse;
    // A notify while painting asks for another frame.
    slot.dirty.set(false);
    let mut canvas = Canvas::new(target, brushes);
    canvas.set_mouse((mouse.x >= 0.0 && mouse.y >= 0.0).then_some(mouse));
    let mut cx = Cx {
        runtime,
        window: slot.id,
    };
    view.paint(&mut canvas, &mut cx);
    if let Some((text, pos)) = tooltip {
        paint_tooltip(&mut canvas, &text, pos);
    }
    slot.state.borrow_mut().hits = canvas.into_hits();
    true
}

fn paint_tooltip<H: Clone>(canvas: &mut Canvas<H>, text: &str, pos: Point) {
    let font = Font::new("Segoe UI", 12.0);
    let layout = text::layout(text, &font, TextOptions::wrapped(360.0));
    let width = layout.width + 16.0;
    let height = layout.height + 10.0;
    let x = (pos.x + 12.0).min(canvas.width - width - 4.0).max(4.0);
    let mut y = pos.y + 20.0;
    if y + height > canvas.height - 4.0 {
        y = (pos.y - height - 8.0).max(4.0);
    }
    let r = rect(x, y, width, height);
    canvas.shadow(r, 4.0);
    canvas.fill_rounded(r, 4.0, Color::rgb(0x2b2f36));
    canvas.draw_layout(&layout, point(x + 8.0, y + 5.0), Color::rgb(0xffffff));
}

fn render<V: View>(runtime: &Runtime<V>, slot: &WindowSlot<V>) {
    let (hwnd, target, mut brushes, scale) = {
        let mut state = slot.state.borrow_mut();
        let Some(hwnd) = state.hwnd else {
            return;
        };
        if state.target.is_none() {
            state.target = create_target(hwnd);
            state.brushes.clear();
        }
        let Some(target) = state.target.clone() else {
            return;
        };
        let brushes = std::mem::take(&mut state.brushes);
        (hwnd, target, brushes, state.scale)
    };
    let dpi = scale * 96.0;
    unsafe {
        target.SetDpi(dpi, dpi);
        target.BeginDraw();
    }
    let painted = paint_into(runtime, slot, &target, &mut brushes);
    let result = unsafe { target.EndDraw(None, None) };
    let mut state = slot.state.borrow_mut();
    state.brushes = brushes;
    if result.is_err() {
        // D2DERR_RECREATE_TARGET: the device was lost; draw again with a new target.
        state.target = None;
        slot.dirty.set(true);
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    } else if painted && !slot.dirty.get() {
        unsafe {
            let _ = ValidateRect(Some(hwnd), None);
        }
    }
}

thread_local! {
    // Never dropped, like the text system's factory.
    static D2D: std::mem::ManuallyDrop<RefCell<Option<ID2D1Factory>>> =
        const { std::mem::ManuallyDrop::new(RefCell::new(None)) };
    static RUNTIME: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

pub(crate) fn d2d_factory() -> ID2D1Factory {
    D2D.with(|factory| {
        factory
            .borrow_mut()
            .get_or_insert_with(|| unsafe {
                D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                    .expect("create a Direct2D factory")
            })
            .clone()
    })
}

fn create_target(hwnd: HWND) -> Option<ID2D1HwndRenderTarget> {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    let properties = D2D1_RENDER_TARGET_PROPERTIES::default();
    let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
        hwnd,
        pixelSize: D2D_SIZE_U {
            width: (client.right - client.left).max(1) as u32,
            height: (client.bottom - client.top).max(1) as u32,
        },
        presentOptions: D2D1_PRESENT_OPTIONS_NONE,
    };
    unsafe {
        d2d_factory()
            .CreateHwndRenderTarget(&properties, &hwnd_properties)
            .ok()
    }
}

fn runtime<'a, V: View>() -> Option<&'a Runtime<V>> {
    let pointer = RUNTIME.with(|runtime| runtime.get());
    if pointer.is_null() {
        None
    } else {
        Some(unsafe { &*(pointer as *const Runtime<V>) })
    }
}

fn slot_for<V: View>(runtime: &Runtime<V>, hwnd: HWND) -> Option<Rc<WindowSlot<V>>> {
    runtime
        .windows
        .borrow()
        .iter()
        .find(|slot| slot.state.borrow().hwnd == Some(hwnd))
        .cloned()
}

fn class_name() -> PCWSTR {
    w!("DataDiverWindow")
}

fn register_class<V: View>() {
    unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let icon = LoadIconW(Some(instance.into()), PCWSTR(1 as _)).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc::<V>),
            hInstance: instance.into(),
            hIcon: icon,
            hIconSm: icon,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: class_name(),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let wake = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wake_proc::<V>),
            hInstance: instance.into(),
            lpszClassName: w!("DataDiverWake"),
            ..Default::default()
        };
        RegisterClassExW(&wake);
    }
}

/// Creates the Win32 window for `request` and builds its view.
pub(crate) fn open_window<V: View>(runtime: &Runtime<V>, request: OpenRequest<V>) {
    let options = request.options;
    let owner = match &request.contents {
        Contents::View(_) => None,
        Contents::Owned { owner, .. } => match runtime.window(*owner) {
            Some(slot) => Some(slot),
            None => return,
        },
    };
    let parent = owner.as_ref().and_then(|slot| slot.hwnd());
    // Owned windows get no taskbar button of their own.
    let ex_style = if owner.is_some() { WINDOW_EX_STYLE::default() } else { WS_EX_APPWINDOW };
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let dpi = 96.0_f32.max(dpi_for_point(options.placement.map(|p| (p.x as i32, p.y as i32))) as f32);
        let scale = dpi / 96.0;
        let mut outer = RECT {
            left: 0,
            top: 0,
            right: (options.width * scale) as i32,
            bottom: (options.height * scale) as i32,
        };
        let _ = AdjustWindowRectExForDpi(&mut outer, WS_OVERLAPPEDWINDOW, false, ex_style, dpi as u32);
        CreateWindowExW(
            ex_style,
            class_name(),
            &HSTRING::from(options.title.as_str()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer.right - outer.left,
            outer.bottom - outer.top,
            parent,
            None,
            Some(instance.into()),
            None,
        )
    };
    let Ok(hwnd) = hwnd else {
        eprintln!("cannot create a window");
        return;
    };
    let size = (options.width, options.height);
    let id = request.id;
    let (slot, opened) = match request.contents {
        Contents::View(build) => {
            unsafe { DragAcceptFiles(hwnd, true) };
            let mut cx = Cx { runtime, window: id };
            // The view sees its window while it builds, so it can spawn work for it.
            let view = build(&mut cx);
            (new_slot(id, Rc::new(RefCell::new(view)), None, Some(hwnd), size), None)
        }
        Contents::Owned { owner: owner_id, opened } => {
            let view = owner.as_ref().map(|slot| slot.view.clone()).expect("the owner is open");
            (new_slot(id, view, Some(owner_id), Some(hwnd), size), Some(opened))
        }
    };
    runtime.windows.borrow_mut().push(slot.clone());
    unsafe {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if let Some(saved) = options.placement {
            let _ = GetWindowPlacement(hwnd, &mut placement);
            placement.rcNormalPosition = RECT {
                left: saved.x as i32,
                top: saved.y as i32,
                right: (saved.x + saved.width) as i32,
                bottom: (saved.y + saved.height) as i32,
            };
            placement.showCmd = if !options.visible {
                SW_HIDE.0 as u32
            } else if saved.maximized {
                SW_SHOWMAXIMIZED.0 as u32
            } else {
                SW_SHOWNORMAL.0 as u32
            };
            let _ = SetWindowPlacement(hwnd, &placement);
        } else {
            if let Some(parent) = parent {
                center_over(hwnd, parent);
            }
            if options.visible {
                let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            }
        }
        if options.visible {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    slot.state.borrow_mut().scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
    slot.invalidate();
    if let Some(opened) = opened {
        runtime.queue(Action::Update(id, opened));
    }
}

/// Moves `hwnd` to the middle of `over`.
fn center_over(hwnd: HWND, over: HWND) {
    let mut window = RECT::default();
    let mut target = RECT::default();
    unsafe {
        if GetWindowRect(hwnd, &mut window).is_err() || GetWindowRect(over, &mut target).is_err() {
            return;
        }
        let width = window.right - window.left;
        let height = window.bottom - window.top;
        let x = target.left + (target.right - target.left - width) / 2;
        let y = target.top + (target.bottom - target.top - height) / 2;
        let _ = SetWindowPos(hwnd, None, x, y.max(target.top), 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn dpi_for_point(point: Option<(i32, i32)>) -> u32 {
    unsafe {
        let monitor = match point {
            Some((x, y)) => MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTOPRIMARY),
            None => MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY),
        };
        let mut dpi_x = 96;
        let mut dpi_y = 96;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        dpi_x
    }
}

pub(crate) fn close_window<V: View>(runtime: &Runtime<V>, id: WindowId) {
    let Some(slot) = runtime.window(id) else {
        return;
    };
    // Owned windows close first, while their owner is still there.
    for owned in owned_windows(runtime, id) {
        close_window(runtime, owned);
    }
    if let Ok(mut view) = slot.view.try_borrow_mut() {
        let mut cx = Cx { runtime, window: id };
        view.closing(&mut cx);
    }
    runtime.windows.borrow_mut().retain(|other| other.id != id);
    let hwnd = slot.state.borrow_mut().hwnd.take();
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
    if runtime.windows.borrow().is_empty() {
        unsafe { PostQuitMessage(0) };
    }
}

pub(crate) fn owned_windows<V: View>(runtime: &Runtime<V>, owner: WindowId) -> Vec<WindowId> {
    runtime
        .windows
        .borrow()
        .iter()
        .filter(|slot| slot.owner == Some(owner))
        .map(|slot| slot.id)
        .collect()
}

/// Monitor work areas in screen pixels.
pub fn displays() -> Vec<Rect> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> windows::core::BOOL {
        let areas = unsafe { &mut *(data.0 as *mut Vec<Rect>) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let work = info.rcWork;
            areas.push(rect(
                work.left as f32,
                work.top as f32,
                (work.right - work.left) as f32,
                (work.bottom - work.top) as f32,
            ));
        }
        true.into()
    }
    let mut areas: Vec<Rect> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut areas as *mut _ as isize));
    }
    areas
}

/// Runs the app until its last window closes. `init` opens the first windows.
pub fn run<V: View>(init: impl FnOnce(&Runtime<V>)) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }
    register_class::<V>();
    let runtime: &'static Runtime<V> = Box::leak(Box::new(Runtime::new()));
    RUNTIME.with(|cell| cell.set(runtime as *const Runtime<V> as *const ()));
    let wake = unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("DataDiverWake"),
            w!(""),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .expect("create the wake window")
    };
    let wake_handle = wake.0 as isize;
    runtime.set_waker(std::sync::Arc::new(move || unsafe {
        let _ = PostMessageW(Some(HWND(wake_handle as _)), WAKE_MESSAGE, WPARAM(0), LPARAM(0));
    }));
    init(runtime);
    drain(runtime);
    let mut message = MSG::default();
    loop {
        let timeout = runtime
            .next_deadline()
            .into_iter()
            .chain(tooltip_deadline(runtime))
            .min()
            .map(|deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(u32::MAX as u128 - 1) as u32
            })
            .unwrap_or(windows::Win32::System::Threading::INFINITE);
        unsafe {
            MsgWaitForMultipleObjectsEx(None, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                let ids = runtime.windows.borrow().iter().map(|slot| slot.id).collect::<Vec<_>>();
                for id in ids {
                    close_window(runtime, id);
                }
                return;
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        drain(runtime);
        show_due_tooltips(runtime);
        if runtime.windows.borrow().is_empty() {
            return;
        }
    }
}

fn drain<V: View>(runtime: &Runtime<V>) {
    while runtime.drain(&|runtime, request| open_window(runtime, request), &|runtime, id| close_window(runtime, id)) {}
}

fn tooltip_deadline<V: View>(runtime: &Runtime<V>) -> Option<Instant> {
    runtime
        .windows
        .borrow()
        .iter()
        .filter_map(|slot| {
            let state = slot.state.borrow();
            if state.tooltip.is_some() {
                return None;
            }
            let (since, _) = state.hover_since?;
            hit_region(&state.hits, state.mouse)
                .and_then(|region| region.tooltip.as_ref())
                .map(|_| since + std::time::Duration::from_millis(TOOLTIP_DELAY_MS as u64))
        })
        .min()
}

fn show_due_tooltips<V: View>(runtime: &Runtime<V>) {
    let slots = runtime.windows.borrow().clone();
    for slot in slots {
        let mut state = slot.state.borrow_mut();
        if state.tooltip.is_some() || state.left_down {
            continue;
        }
        let Some((since, pos)) = state.hover_since else {
            continue;
        };
        if since.elapsed().as_millis() < TOOLTIP_DELAY_MS {
            continue;
        }
        let text = hit_region(&state.hits, state.mouse).and_then(|region| region.tooltip.clone());
        if let Some(text) = text {
            state.tooltip = Some(Tooltip { text, pos });
            drop(state);
            slot.invalidate();
        }
    }
}

unsafe extern "system" fn wake_proc<V: View>(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WAKE_MESSAGE {
        if let Some(runtime) = runtime::<V>() {
            drain(runtime);
        }
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn modifiers() -> Modifiers {
    let down = |key: VIRTUAL_KEY| unsafe { GetKeyState(key.0 as i32) } < 0;
    Modifiers {
        control: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        platform: down(VK_LWIN) || down(VK_RWIN),
    }
}

/// The key name of virtual key `vk`, as the keymap writes it.
fn key_name(vk: u16, extended: bool) -> Option<String> {
    let named = match VIRTUAL_KEY(vk) {
        VK_RETURN => "enter",
        VK_ESCAPE => "escape",
        VK_TAB => "tab",
        VK_BACK => "backspace",
        VK_DELETE => "delete",
        VK_INSERT => "insert",
        VK_HOME => "home",
        VK_END => "end",
        VK_PRIOR => "pageup",
        VK_NEXT => "pagedown",
        VK_LEFT => "left",
        VK_RIGHT => "right",
        VK_UP => "up",
        VK_DOWN => "down",
        VK_SPACE => "space",
        VK_ADD => "+",
        VK_SUBTRACT => "-",
        VK_MULTIPLY => "*",
        VK_DIVIDE => "/",
        VK_DECIMAL => ".",
        VK_APPS => "menu",
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN | VK_LSHIFT | VK_RSHIFT
        | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_CAPITAL | VK_NUMLOCK
        | VK_SCROLL => return None,
        _ => "",
    };
    let _ = extended;
    if !named.is_empty() {
        return Some(named.into());
    }
    if (VK_F1.0..=VK_F24.0).contains(&vk) {
        return Some(format!("f{}", vk - VK_F1.0 + 1));
    }
    if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) {
        return Some(((b'0' + (vk - VK_NUMPAD0.0) as u8) as char).to_string());
    }
    if (b'A' as u16..=b'Z' as u16).contains(&vk) {
        return Some(((vk as u8) as char).to_ascii_lowercase().to_string());
    }
    if (b'0' as u16..=b'9' as u16).contains(&vk) {
        return Some(((vk as u8) as char).to_string());
    }
    let mapped = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) } & 0x7fff_ffff;
    let ch = char::from_u32(mapped)?;
    if ch == '\0' {
        return None;
    }
    Some(ch.to_lowercase().to_string())
}

/// The text the key press types, without disturbing dead-key state.
fn typed_text(vk: u16, scan: u32) -> Option<String> {
    let mut state = [0u8; 256];
    unsafe {
        GetKeyboardState(&mut state).ok()?;
    }
    let mut buffer = [0u16; 8];
    let layout = unsafe { GetKeyboardLayout(0) };
    // Flag 4 keeps the keyboard state unchanged (Windows 10 1607 and later).
    let count = unsafe { ToUnicodeEx(vk as u32, scan, &state, &mut buffer, 4, Some(layout)) };
    if count <= 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..count as usize]);
    (!text.chars().any(|ch| ch.is_control())).then_some(text)
}

fn mouse_point(lparam: LPARAM, scale: f32) -> Point {
    let x = (lparam.0 & 0xffff) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xffff) as i16 as f32;
    point(x / scale, y / scale)
}

fn dispatch<V: View>(runtime: &Runtime<V>, slot: &WindowSlot<V>, event: Event<V::Hit>) -> Option<bool> {
    let Ok(mut view) = slot.view.try_borrow_mut() else {
        return None;
    };
    let mut cx = Cx {
        runtime,
        window: slot.id,
    };
    let handled = view.event(&event, &mut cx);
    Some(handled)
}

fn set_cursor(cursor: Cursor) {
    let name = match cursor {
        Cursor::Arrow => IDC_ARROW,
        Cursor::IBeam => IDC_IBEAM,
        Cursor::Hand => IDC_HAND,
        Cursor::ResizeColumns => IDC_SIZEWE,
        Cursor::ResizeRows => IDC_SIZENS,
    };
    unsafe {
        if let Ok(cursor) = LoadCursorW(None, name) {
            SetCursor(Some(cursor));
        }
    }
}

fn place_ime<V: View>(slot: &WindowSlot<V>, hwnd: HWND) {
    let caret = slot.view.try_borrow().ok().and_then(|view| view.caret(slot.id));
    let Some(caret) = caret else {
        return;
    };
    let scale = slot.state.borrow().scale;
    unsafe {
        let context = ImmGetContext(hwnd);
        if context.is_invalid() {
            return;
        }
        let form = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT {
                x: (caret.x * scale) as i32,
                y: (caret.y * scale) as i32,
            },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(context, &form);
        let _ = ImmReleaseContext(hwnd, context);
    }
}

unsafe extern "system" fn wndproc<V: View>(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let default = || unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    let Some(runtime) = runtime::<V>() else {
        return default();
    };
    let Some(slot) = slot_for(runtime, hwnd) else {
        return default();
    };
    let scale = slot.state.borrow().scale;
    let result = match message {
        WM_PAINT => {
            render(runtime, &slot);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE => {
            let width = (lparam.0 & 0xffff) as u32;
            let height = ((lparam.0 >> 16) & 0xffff) as u32;
            if let Some(target) = &slot.state.borrow().target {
                unsafe {
                    let _ = target.Resize(&D2D_SIZE_U {
                        width: width.max(1),
                        height: height.max(1),
                    });
                }
            }
            slot.invalidate();
            dispatch(runtime, &slot, Event::Moved);
            LRESULT(0)
        }
        WM_MOVE => {
            dispatch(runtime, &slot, Event::Moved);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xffff) as f32;
            slot.state.borrow_mut().scale = dpi / 96.0;
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            slot.invalidate();
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT {
                x: (480.0 * scale) as i32,
                y: (320.0 * scale) as i32,
            };
            LRESULT(0)
        }
        WM_ACTIVATE => {
            let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
            dispatch(runtime, &slot, Event::Activated(active));
            default()
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = (wparam.0 & 0xffff) as u16;
            let scan = ((lparam.0 >> 16) & 0xff) as u32;
            let extended = (lparam.0 >> 24) & 1 == 1;
            let Some(key) = key_name(vk, extended) else {
                return default();
            };
            let modifiers = modifiers();
            let key_char = if modifiers.control || modifiers.alt {
                None
            } else {
                typed_text(vk, scan)
            };
            let event = KeyEvent::new(key, modifiers, key_char);
            let alt_f4 = event.key == "f4" && modifiers.alt && !modifiers.control;
            slot.state.borrow_mut().tooltip = None;
            let handled = dispatch(runtime, &slot, Event::Key(event)).unwrap_or(false);
            if handled {
                // The key did something, so drop the text it would also type.
                let mut pending = MSG::default();
                unsafe {
                    while PeekMessageW(&mut pending, Some(hwnd), WM_CHAR, WM_CHAR, PM_REMOVE).as_bool() {}
                    while PeekMessageW(&mut pending, Some(hwnd), WM_SYSCHAR, WM_SYSCHAR, PM_REMOVE).as_bool() {}
                }
                LRESULT(0)
            } else if message == WM_SYSKEYDOWN && (alt_f4 || vk == VK_SPACE.0) {
                default()
            } else if message == WM_SYSKEYDOWN {
                // Unhandled Alt chords would otherwise open the window menu.
                LRESULT(0)
            } else {
                default()
            }
        }
        WM_SYSCHAR => {
            if wparam.0 as u16 == b' ' as u16 {
                default()
            } else {
                LRESULT(0)
            }
        }
        WM_CHAR => {
            let unit = wparam.0 as u16;
            let text = if (0xD800..0xDC00).contains(&unit) {
                slot.state.borrow_mut().surrogate = Some(unit);
                None
            } else if (0xDC00..0xE000).contains(&unit) {
                let high = slot.state.borrow_mut().surrogate.take();
                high.map(|high| String::from_utf16_lossy(&[high, unit]))
            } else {
                char::from_u32(unit as u32)
                    .filter(|ch| !ch.is_control())
                    .map(|ch| ch.to_string())
            };
            if let Some(text) = text {
                dispatch(runtime, &slot, Event::Text(text));
            }
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => {
            place_ime(&slot, hwnd);
            default()
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            let pos = mouse_point(lparam, scale);
            let button = match message {
                WM_LBUTTONDOWN => MouseButton::Left,
                WM_RBUTTONDOWN => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let clicks = {
                let mut state = slot.state.borrow_mut();
                state.tooltip = None;
                state.hover_since = None;
                let now = Instant::now();
                let limit = std::time::Duration::from_millis(unsafe { GetDoubleClickTime() } as u64);
                let clicks = match state.last_click {
                    Some((at, from, previous, count))
                        if previous == button
                            && now.duration_since(at) <= limit
                            && (from.x - pos.x).abs() <= 4.0
                            && (from.y - pos.y).abs() <= 4.0 =>
                    {
                        count + 1
                    }
                    _ => 1,
                };
                state.last_click = Some((now, pos, button, clicks));
                if button == MouseButton::Left {
                    state.left_down = true;
                }
                clicks
            };
            unsafe {
                SetCapture(hwnd);
            }
            let hit = slot.hit_at(pos);
            dispatch(
                runtime,
                &slot,
                Event::MouseDown {
                    pos,
                    button,
                    modifiers: modifiers(),
                    clicks,
                    hit,
                },
            );
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
            let pos = mouse_point(lparam, scale);
            let button = match message {
                WM_LBUTTONUP => MouseButton::Left,
                WM_RBUTTONUP => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            if button == MouseButton::Left {
                slot.state.borrow_mut().left_down = false;
            }
            unsafe {
                let _ = ReleaseCapture();
            }
            let hit = slot.hit_at(pos);
            dispatch(
                runtime,
                &slot,
                Event::MouseUp {
                    pos,
                    button,
                    modifiers: modifiers(),
                    hit,
                },
            );
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let pos = mouse_point(lparam, scale);
            let left_down = wparam.0 & 0x0001 != 0;
            let (track, hover_changed) = {
                let mut state = slot.state.borrow_mut();
                let before = hit_region(&state.hits, state.mouse).map(|region| region.rect);
                state.mouse = pos;
                let after = hit_region(&state.hits, pos).map(|region| region.rect);
                let changed = before != after;
                if changed || state.hover_since.is_none() {
                    state.hover_since = Some((Instant::now(), pos));
                }
                if changed && state.tooltip.take().is_some() {
                    slot.dirty.set(true);
                }
                let track = !state.tracking_leave;
                state.tracking_leave = true;
                (track, changed)
            };
            if track {
                let mut event = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                unsafe {
                    let _ = TrackMouseEvent(&mut event);
                }
            }
            let hit = slot.hit_at(pos);
            dispatch(
                runtime,
                &slot,
                Event::MouseMove {
                    pos,
                    modifiers: modifiers(),
                    left_down,
                    hit,
                },
            );
            if hover_changed {
                // Hovered buttons draw highlighted.
                slot.invalidate();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            {
                let mut state = slot.state.borrow_mut();
                state.tracking_leave = false;
                state.hover_since = None;
                state.tooltip = None;
                state.mouse = point(-1.0, -1.0);
            }
            dispatch(runtime, &slot, Event::MouseLeave);
            slot.invalidate();
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let mut screen = POINT {
                x: (lparam.0 & 0xffff) as i16 as i32,
                y: ((lparam.0 >> 16) & 0xffff) as i16 as i32,
            };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut screen);
            }
            let pos = point(screen.x as f32 / scale, screen.y as f32 / scale);
            let amount = ((wparam.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
            let mut lines = 3u32;
            unsafe {
                let _ = SystemParametersInfoW(
                    SPI_GETWHEELSCROLLLINES,
                    0,
                    Some(&mut lines as *mut u32 as *mut _),
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                );
            }
            let lines = lines.clamp(1, 20) as f32;
            let delta = if message == WM_MOUSEWHEEL {
                point(0.0, amount * lines)
            } else {
                point(-amount * lines, 0.0)
            };
            slot.state.borrow_mut().tooltip = None;
            let hit = slot.hit_at(pos);
            dispatch(
                runtime,
                &slot,
                Event::Wheel {
                    pos,
                    delta,
                    modifiers: modifiers(),
                    hit,
                },
            );
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if (lparam.0 & 0xffff) as u32 == HTCLIENT {
                let cursor = {
                    let state = slot.state.borrow();
                    hit_region(&state.hits, state.mouse)
                        .map(|region| region.cursor)
                        .unwrap_or_default()
                };
                set_cursor(cursor);
                LRESULT(1)
            } else {
                default()
            }
        }
        WM_DROPFILES => {
            let drop = HDROP(wparam.0 as _);
            let mut paths = Vec::new();
            let mut at = POINT::default();
            unsafe {
                let count = DragQueryFileW(drop, u32::MAX, None);
                for index in 0..count {
                    let length = DragQueryFileW(drop, index, None);
                    let mut buffer = vec![0u16; length as usize + 1];
                    DragQueryFileW(drop, index, Some(&mut buffer));
                    paths.push(PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])));
                }
                let _ = DragQueryPoint(drop, &mut at);
                DragFinish(drop);
            }
            let pos = point(at.x as f32 / scale, at.y as f32 / scale);
            let hit = slot.hit_at(pos);
            dispatch(runtime, &slot, Event::Drop { paths, pos, hit });
            unsafe {
                let _ = SetForegroundWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let close = match slot.view.try_borrow_mut() {
                Ok(mut view) => {
                    let mut cx = Cx {
                        runtime,
                        window: slot.id,
                    };
                    view.close_requested(&mut cx)
                }
                Err(_) => false,
            };
            if close {
                close_window(runtime, slot.id);
            }
            LRESULT(0)
        }
        _ => default(),
    };
    drain(runtime);
    result
}
