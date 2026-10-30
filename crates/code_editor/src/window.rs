//! The editor's window plumbing and the public [`Editor`] handle.
//!
//! An editor runs in one of two modes:
//! - **Child window** ([`Editor::create`]): it has its own `HWND`, paints itself
//!   and receives input directly.
//! - **Hosted** ([`Editor::create_hosted`]): a host that draws everything into
//!   one window forwards input with [`Editor::handle_message`] and draws the
//!   editor with [`Editor::paint_into`]. A message-only window still carries the
//!   editor's timers and posted messages.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::decorations::{DecorationId, DecorationOptions};
use crate::editor::{KeyResult, Mods, State};
use crate::history::EditKind;
use crate::keymap::{Binding, Chord};
use crate::lang;
use crate::selection::{Selection, Selections};
use crate::text::{self, Buffer, Edit, Pos, Transaction};
use crate::theme::{Ink, Theme};
use crate::{Event, Options, Range, dragdrop, platform, render};

const CLASS: PCWSTR = w!("WinMonacoEditor");
const TIMER_BLINK: usize = 1;
const TIMER_SCROLL: usize = 2;
/// One-shot: reparse a big file after typing pauses.
const TIMER_PARSE: usize = 3;
const PARSE_DELAY_MS: u32 = 40;
/// One-shot: ask the language service for problems after typing pauses.
const TIMER_DIAGNOSE: usize = 4;
const DIAGNOSE_DELAY_MS: u32 = 500;
/// One-shot: the mouse rested (hosts that translate their own input).
const TIMER_HOVER: usize = 5;
const HOVER_DELAY_MS: u32 = 400;
/// From WinUser.h; windows-rs only has these with the Win32_UI_Controls feature.
const WM_MOUSEHOVER: u32 = 0x02A1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED_AFTERPARENT: u32 = 0x02E3;
const HOVER_DEFAULT: u32 = 0xFFFF_FFFF;

type Handler = Box<dyn FnMut(&Editor, &Event)>;

/// Where a hosted editor sits in its host's client area, in pixels.
#[derive(Clone, Copy, Default)]
struct Hosted {
    host: HWND,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    visible: bool,
}

pub(crate) struct Shell {
    state: RefCell<State>,
    handler: RefCell<Option<Handler>>,
    /// The editor's own window: a child window, or message-only when hosted.
    hwnd: Cell<HWND>,
    hosted: Cell<Option<Hosted>>,
    target: RefCell<Option<ID2D1HwndRenderTarget>>,
    /// The target is in software, waiting for the GPU to warm up.
    software: Cell<bool>,
    /// Click counting for double and triple clicks.
    clicks: Cell<(Instant, i32, i32, u32)>,
    /// A key the keymap handled; its WM_CHAR / WM_SYSCHAR is dropped.
    swallow_char: Cell<bool>,
    tracking: Cell<bool>,
    /// Where the mouse last moved, for the hover timer.
    hover_at: Cell<(f32, f32)>,
    /// When the input being handled arrived, for tracing input-to-frame time.
    input_at: Cell<Option<Instant>>,
    first_frame: Cell<bool>,
}

impl Shell {
    /// The window that gets input: the host when hosted.
    fn input_hwnd(&self) -> HWND {
        self.hosted.get().map_or(self.hwnd.get(), |h| h.host)
    }

    /// The editor's top-left in the input window's client pixels.
    fn origin(&self) -> (i32, i32) {
        self.hosted.get().map_or((0, 0), |h| (h.x, h.y))
    }

    fn invalidate(&self) {
        unsafe {
            match self.hosted.get() {
                Some(h) if h.host.is_invalid() => {}
                Some(h) => {
                    let rc = RECT {
                        left: h.x,
                        top: h.y,
                        right: h.x + h.w,
                        bottom: h.y + h.h,
                    };
                    let _ = InvalidateRect(Some(h.host), Some(&rc), false);
                }
                None => {
                    let _ = InvalidateRect(Some(self.hwnd.get()), None, false);
                }
            }
        }
    }

    fn set_timer(&self, id: usize, ms: Option<u32>) {
        let hwnd = self.hwnd.get();
        unsafe {
            match ms {
                Some(ms) => {
                    SetTimer(Some(hwnd), id, ms, None);
                }
                None => {
                    let _ = KillTimer(Some(hwnd), id);
                }
            }
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        let hwnd = self.hwnd.get();
        unsafe {
            if !hwnd.is_invalid() && IsWindow(Some(hwnd)).as_bool() {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}

/// An editor instance: a cheap handle to its window and state. Clones refer to
/// the same editor; the editor is destroyed with the last handle or by
/// [`Editor::close`]. It lives on the UI thread that created it.
#[derive(Clone)]
pub struct Editor {
    shell: Rc<Shell>,
}

/// A decoration to add: a range and how it looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoration {
    pub start: Pos,
    pub end: Pos,
    pub options: DecorationOptions,
}

/// A replacement of `start..end` with `text`, in positions of the current text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: Pos,
    pub end: Pos,
    pub text: String,
}

fn register_class() -> Result<()> {
    thread_local! {
        static REGISTERED: Cell<bool> = const { Cell::new(false) };
    }
    if REGISTERED.with(|r| r.get()) {
        return Ok(());
    }
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_IBEAM)?,
            hInstance: instance,
            lpszClassName: CLASS,
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        // A second thread registering the same class is fine.
        RegisterClassW(&wc);
    }
    REGISTERED.with(|r| r.set(true));
    Ok(())
}

fn xy(lparam: LPARAM) -> (i32, i32) {
    (
        (lparam.0 & 0xFFFF) as i16 as i32,
        ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    )
}

fn mods() -> Mods {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
    Mods {
        shift: down(VK_SHIFT),
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
    }
}

fn shell_of(hwnd: HWND) -> Option<Rc<Shell>> {
    let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Weak<Shell>;
    unsafe { p.as_ref() }.and_then(Weak::upgrade)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        if msg == WM_NCDESTROY {
            let p = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut Weak<Shell>;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let Some(shell) = shell_of(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        // Calls that send messages back to this window happen before the state
        // is borrowed.
        if shell.hosted.get().is_none()
            && matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
            && GetFocus() != hwnd
        {
            SetFocus(Some(hwnd)).ok();
        }
        // Default processing runs after the state is released: it can send
        // messages (WM_SIZE from WM_WINDOWPOSCHANGED) back to this window.
        let r = message(&shell, msg, wparam, lparam).unwrap_or_else(|| DefWindowProcW(hwnd, msg, wparam, lparam));
        dispatch(&shell);
        r
    }
}

/// Handles a message for the editor; `None` asks for default processing.
fn message(shell: &Rc<Shell>, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if msg == WM_CONTEXTMENU {
        context_menu(shell, lparam);
        return Some(LRESULT(0));
    }
    if matches!(msg, WM_KEYDOWN | WM_CHAR | WM_LBUTTONDOWN) && platform::tracing() {
        shell.input_at.set(Some(Instant::now()));
    }
    handle(shell, msg, wparam, lparam)
}

fn handle(shell: &Rc<Shell>, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let Ok(mut st) = shell.state.try_borrow_mut() else {
        return None;
    };
    let hwnd = shell.hwnd.get();
    let input = shell.input_hwnd();
    let (ox, oy) = shell.origin();
    let k = st.view.dpi / 96.0;
    let dip = |l: LPARAM| {
        let (x, y) = xy(l);
        ((x - ox) as f32 / k, (y - oy) as f32 / k)
    };
    let inside = |(x, y): (f32, f32), st: &State| x >= 0.0 && y >= 0.0 && x < st.view.width && y < st.view.height;
    let r = match msg {
        WM_PAINT if shell.hosted.get().is_none() => {
            let mut ps = PAINTSTRUCT::default();
            unsafe { BeginPaint(hwnd, &mut ps) };
            paint(shell, &mut st, hwnd);
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SIZE if shell.hosted.get().is_none() => {
            let (w, h) = xy(lparam);
            let (w, h) = (w as u16 as u32, h as u16 as u32);
            st.resize(w, h);
            if let Some(t) = &*shell.target.borrow() {
                unsafe {
                    let _ = t.Resize(&windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U {
                        width: w.max(1),
                        height: h.max(1),
                    });
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED_AFTERPARENT | WM_DPICHANGED => {
            let dpi = unsafe { GetDpiForWindow(input) } as f32;
            st.set_dpi(dpi);
            if let Some(t) = &*shell.target.borrow() {
                unsafe { t.SetDpi(dpi, dpi) };
            }
            if let Some(h) = shell.hosted.get() {
                st.resize(h.w.max(0) as u32, h.h.max(0) as u32);
            }
            return if msg == WM_DPICHANGED { None } else { Some(LRESULT(0)) };
        }
        WM_GETDLGCODE => LRESULT((DLGC_WANTALLKEYS | DLGC_WANTCHARS | DLGC_WANTARROWS | DLGC_WANTTAB) as isize),
        WM_SETFOCUS => {
            st.set_focus(true);
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            st.set_focus(false);
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            st.hide_tooltip();
            let chord = Chord::current(wparam.0 as u16);
            match st.key(chord) {
                KeyResult::Handled => {
                    shell.swallow_char.set(true);
                    LRESULT(0)
                }
                KeyResult::ContextMenu => {
                    shell.swallow_char.set(true);
                    unsafe {
                        let _ = PostMessageW(Some(hwnd), WM_CONTEXTMENU, WPARAM(hwnd.0 as usize), LPARAM(-1));
                    }
                    LRESULT(0)
                }
                KeyResult::Unhandled => {
                    shell.swallow_char.set(false);
                    return None;
                }
            }
        }
        WM_CHAR => {
            if !shell.swallow_char.replace(false) || (wparam.0 as u32) >= 0x20 {
                st.on_char(wparam.0 as u16);
            }
            LRESULT(0)
        }
        WM_SYSCHAR if shell.swallow_char.replace(false) => LRESULT(0),
        WM_IME_STARTCOMPOSITION => {
            place_ime(shell, &mut st);
            return None;
        }
        WM_LBUTTONDOWN => {
            let p = dip(lparam);
            if !inside(p, &st) {
                // A hosted editor loses focus to clicks elsewhere in the host.
                st.set_focus(false);
                apply_effects(shell, &mut st);
                return None;
            }
            st.set_focus(true);
            let (px, py) = xy(lparam);
            let clicks = count_click(shell, px, py);
            unsafe { SetCapture(input) };
            st.hide_tooltip();
            st.mouse_down(p.0, p.1, mods(), clicks);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = dip(lparam);
            if st.drag.is_some() {
                st.drag_to(x, y);
                let start = st.autoscroll_needed();
                shell.set_timer(TIMER_SCROLL, start.then_some(30));
            } else {
                if !inside((x, y), &st) {
                    st.mouse_leave();
                    st.hide_tooltip();
                    apply_effects(shell, &mut st);
                    return None;
                }
                st.hover_at(x, y);
                if st.tooltip.as_ref().is_some_and(|t| (t.0 - x).abs() > 4.0 || (t.1 - y).abs() > 4.0) {
                    st.hide_tooltip();
                }
                if !shell.tracking.replace(true) {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_HOVER,
                        hwndTrack: input,
                        dwHoverTime: HOVER_DEFAULT,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEHOVER => {
            // Hover fires once per tracking; the next move tracks again.
            shell.tracking.set(false);
            let (x, y) = dip(lparam);
            if !inside((x, y), &st) {
                return None;
            }
            st.hover(x, y);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            shell.tracking.set(false);
            st.mouse_leave();
            st.hide_tooltip();
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            if st.drag.is_none() && shell.hosted.get().is_some() {
                return None;
            }
            st.mouse_up();
            shell.set_timer(TIMER_SCROLL, None);
            if msg == WM_LBUTTONUP {
                apply_effects(shell, &mut st);
                drop(st);
                unsafe {
                    let _ = ReleaseCapture();
                }
                return Some(LRESULT(0));
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            let p = dip(lparam);
            if !inside(p, &st) {
                return None;
            }
            st.set_focus(true);
            st.right_down(p.0, p.1);
            LRESULT(0)
        }
        WM_RBUTTONUP if shell.hosted.get().is_some() => {
            // Hosts get no WM_CONTEXTMENU for their own client area here, so
            // the menu opens from the button release.
            let p = dip(lparam);
            if !inside(p, &st) {
                return None;
            }
            let (x, y) = xy(lparam);
            let mut pt = POINT { x, y };
            unsafe {
                let _ = ClientToScreen(input, &mut pt);
            }
            drop(st);
            context_menu(shell, LPARAM(((pt.y as u16 as isize) << 16) | pt.x as u16 as isize));
            return Some(LRESULT(0));
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            st.hide_tooltip();
            let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / WHEEL_DELTA as f32;
            st.wheel(delta, msg == WM_MOUSEHWHEEL, mods());
            LRESULT(0)
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(input, &mut pt);
            }
            let (x, y) = ((pt.x - ox) as f32 / k, (pt.y - oy) as f32 / k);
            if !inside((x, y), &st) {
                return None;
            }
            let cursor = match st.view.region(x, y) {
                _ if st.over_widget(x, y) => IDC_ARROW,
                crate::view::Region::Text => IDC_IBEAM,
                _ => IDC_ARROW,
            };
            platform::set_cursor(cursor);
            LRESULT(1)
        }
        WM_TIMER => {
            match wparam.0 {
                TIMER_BLINK => {
                    if st.focused {
                        st.blink();
                    } else {
                        shell.set_timer(TIMER_BLINK, None);
                    }
                }
                TIMER_SCROLL if !st.autoscroll() => shell.set_timer(TIMER_SCROLL, None),
                TIMER_PARSE => {
                    shell.set_timer(TIMER_PARSE, None);
                    st.parse_deferred();
                }
                TIMER_DIAGNOSE => {
                    shell.set_timer(TIMER_DIAGNOSE, None);
                    st.refresh_diagnostics();
                }
                TIMER_HOVER => {
                    shell.set_timer(TIMER_HOVER, None);
                    if st.drag.is_none() {
                        let (x, y) = shell.hover_at.get();
                        st.hover(x, y);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DROPFILES => {
            let hdrop = HDROP(wparam.0 as _);
            let files = platform::hdrop_paths(hdrop);
            unsafe { DragFinish(hdrop) };
            st.events.push(Event::FilesDropped(files));
            LRESULT(0)
        }
        crate::editor::WM_APP_PARSED => {
            let tree = unsafe { Box::from_raw(lparam.0 as *mut tree_sitter::Tree) };
            st.parsed(wparam.0, *tree);
            LRESULT(0)
        }
        dragdrop::WM_APP_FILES => {
            let files = unsafe { Box::from_raw(lparam.0 as *mut Vec<std::path::PathBuf>) };
            st.events.push(Event::FilesDropped(*files));
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = RevokeDragDrop(hwnd);
            }
            for timer in [TIMER_BLINK, TIMER_SCROLL, TIMER_DIAGNOSE, TIMER_HOVER] {
                shell.set_timer(timer, None);
            }
            LRESULT(0)
        }
        _ => return None,
    };
    apply_effects(shell, &mut st);
    Some(r)
}

fn count_click(shell: &Shell, x: i32, y: i32) -> u32 {
    let (t, px, py, n) = shell.clicks.get();
    let dx = unsafe { GetSystemMetrics(SM_CXDOUBLECLK) } / 2;
    let dy = unsafe { GetSystemMetrics(SM_CYDOUBLECLK) } / 2;
    let quick = t.elapsed().as_millis() <= platform::double_click_ms() as u128;
    let n = if quick && (x - px).abs() <= dx && (y - py).abs() <= dy {
        n % 3 + 1
    } else {
        1
    };
    shell.clicks.set((Instant::now(), x, y, n));
    n
}

/// Applies what the state asked for: repaint, caret blink, IME position.
fn apply_effects(shell: &Shell, st: &mut State) {
    let fx = std::mem::take(&mut st.fx);
    st.notify();
    let visible = shell.hosted.get().is_none_or(|h| h.visible);
    if fx.invalidate && visible {
        shell.invalidate();
    }
    if fx.reset_blink && st.focused && visible {
        st.caret_on = true;
        let ms = st.opts.cursor_blinking.then(platform::caret_blink_ms);
        shell.set_timer(TIMER_BLINK, ms);
    }
    if !st.focused || !visible {
        shell.set_timer(TIMER_BLINK, None);
    }
    if fx.ime && st.focused {
        place_ime(shell, st);
    }
    // Rearmed by every edit, so it fires once typing pauses.
    if fx.reparse {
        shell.set_timer(TIMER_PARSE, Some(PARSE_DELAY_MS));
    }
    if fx.diagnose {
        shell.set_timer(TIMER_DIAGNOSE, Some(DIAGNOSE_DELAY_MS));
    }
}

fn place_ime(shell: &Shell, st: &mut State) {
    let (x, y, h) = st.caret_point();
    let k = st.view.dpi / 96.0;
    let (ox, oy) = shell.origin();
    let hwnd = shell.input_hwnd();
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        let cf = COMPOSITIONFORM {
            dwStyle: CFS_POINT,
            ptCurrentPos: POINT {
                x: ox + (x * k) as i32,
                y: oy + (y * k) as i32,
            },
            rcArea: RECT::default(),
        };
        let _ = ImmSetCompositionWindow(himc, &cf);
        let mut lf = LOGFONTW {
            lfHeight: -((h * 0.75 * k) as i32),
            ..Default::default()
        };
        let face: Vec<u16> = "Segoe UI".encode_utf16().collect();
        lf.lfFaceName[..face.len()].copy_from_slice(&face);
        let _ = ImmSetCompositionFontW(himc, &lf);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

/// Paints a child-window editor into its own render target.
fn paint(shell: &Shell, st: &mut State, hwnd: HWND) {
    if st.view.width <= 0.0 || st.view.height <= 0.0 {
        return;
    }
    let mut slot = shell.target.borrow_mut();
    // Moves to the GPU once its driver is loaded.
    if shell.software.get() && render::gpu_ready() {
        *slot = None;
        st.render.release_target();
    }
    if slot.is_none() {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        match render::hwnd_target(hwnd, rc.right as u32, rc.bottom as u32, st.view.dpi) {
            Ok((t, hardware)) => {
                *slot = Some(t);
                shell.software.set(!hardware);
            }
            Err(_) => return,
        }
    }
    let t = slot.clone().unwrap();
    let started = platform::tracing().then(Instant::now);
    unsafe {
        t.BeginDraw();
        st.paint(&t);
        if let Err(e) = t.EndDraw(None, None)
            && e.code() == D2DERR_RECREATE_TARGET
        {
            *slot = None;
            st.render.release_target();
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
    if shell.software.get() {
        render::warm_gpu();
    }
    if let Some(t0) = started {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if shell.first_frame.replace(false) {
            platform::trace(&format!(
                "first frame {:.1} ms after process start (paint {ms:.2} ms)",
                platform::ms_since_process_start()
            ));
        } else if let Some(i) = shell.input_at.take() {
            platform::trace(&format!(
                "input to frame {:.2} ms (paint {ms:.2} ms)",
                i.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }
}

/// Shows the context menu; `lparam` holds screen coordinates, or -1 for the
/// keyboard (at the caret).
fn context_menu(shell: &Rc<Shell>, lparam: LPARAM) {
    let input = shell.input_hwnd();
    let (ox, oy) = shell.origin();
    let (items, at) = {
        let Ok(mut st) = shell.state.try_borrow_mut() else { return };
        if !st.opts.context_menu {
            return;
        }
        let k = st.view.dpi / 96.0;
        let at = if lparam.0 == -1 {
            let (x, y, h) = st.caret_point();
            (ox + (x * k) as i32, oy + ((y + h) * k) as i32)
        } else {
            let mut pt = POINT {
                x: xy(lparam).0,
                y: xy(lparam).1,
            };
            unsafe {
                let _ = ScreenToClient(input, &mut pt);
            }
            (pt.x, pt.y)
        };
        (st.menu_items(), at)
    };
    // The menu runs its own message loop, so the state is not borrowed here.
    let chosen = platform::popup_menu(input, &items, at.0, at.1);
    if let Some(id) = chosen
        && let Ok(mut st) = shell.state.try_borrow_mut()
    {
        st.menu_command(id);
        apply_effects(shell, &mut st);
    }
    dispatch(shell);
}

/// Delivers queued events to the host's handler, outside any borrow of the
/// state, so the handler may call back into the editor.
fn dispatch(shell: &Rc<Shell>) {
    loop {
        let events = match shell.state.try_borrow_mut() {
            Ok(mut st) => {
                st.notify();
                std::mem::take(&mut st.events)
            }
            Err(_) => return,
        };
        if events.is_empty() {
            return;
        }
        let Some(mut h) = shell.handler.borrow_mut().take() else {
            return;
        };
        let ed = Editor { shell: shell.clone() };
        for e in &events {
            h(&ed, e);
        }
        let mut slot = shell.handler.borrow_mut();
        if slot.is_none() {
            *slot = Some(h);
        }
    }
}

impl Editor {
    /// Creates an editor as a child window of `parent`, in the dark theme. It
    /// starts visible with a zero size; position it with [`Editor::set_bounds`].
    pub fn create(parent: HWND, options: Options) -> Result<Editor> {
        Editor::create_with_theme(parent, options, Theme::Dark)
    }

    /// Like [`Editor::create`] with a starting theme.
    pub fn create_with_theme(parent: HWND, options: Options, theme: Theme) -> Result<Editor> {
        let shell = Editor::new_shell(options, theme)?;
        let hwnd = Editor::create_window(&shell, Some(parent), WS_CHILD | WS_CLIPSIBLINGS | WS_VISIBLE)?;
        unsafe {
            let target: IDropTarget = dragdrop::DropTarget::new(hwnd).into();
            if RegisterDragDrop(hwnd, &target).is_err() {
                DragAcceptFiles(hwnd, true);
            }
        }
        Ok(Editor { shell })
    }

    /// Creates an editor without a window of its own, for hosts that draw into
    /// one window (as Penmark-style apps do). The host positions it with
    /// [`Editor::set_bounds`] (in `host` client pixels), forwards input with
    /// [`Editor::handle_message`], and draws it with [`Editor::paint_into`]
    /// whenever `host` repaints the editor's rectangle.
    pub fn create_hosted(host: HWND, options: Options, theme: Theme) -> Result<Editor> {
        let shell = Editor::new_shell(options, theme)?;
        Editor::create_window(&shell, Some(HWND_MESSAGE), WINDOW_STYLE::default())?;
        shell.hosted.set(Some(Hosted {
            host,
            visible: true,
            ..Default::default()
        }));
        let mut st = shell.state.borrow_mut();
        st.hwnd = host;
        st.view.dpi = unsafe { GetDpiForWindow(host) }.max(96) as f32;
        drop(st);
        Ok(Editor { shell })
    }

    fn new_shell(options: Options, theme: Theme) -> Result<Rc<Shell>> {
        register_class()?;
        let state = State::new(HWND::default(), options, theme)?;
        Ok(Rc::new(Shell {
            state: RefCell::new(state),
            handler: RefCell::new(None),
            hwnd: Cell::new(HWND::default()),
            hosted: Cell::new(None),
            target: RefCell::new(None),
            software: Cell::new(false),
            clicks: Cell::new((Instant::now(), 0, 0, 0)),
            swallow_char: Cell::new(false),
            tracking: Cell::new(false),
            hover_at: Cell::new((0.0, 0.0)),
            input_at: Cell::new(None),
            first_frame: Cell::new(true),
        }))
    }

    fn create_window(shell: &Rc<Shell>, parent: Option<HWND>, style: WINDOW_STYLE) -> Result<HWND> {
        let weak = Box::into_raw(Box::new(Rc::downgrade(shell)));
        let hwnd = unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                CLASS,
                w!(""),
                style,
                0,
                0,
                0,
                0,
                parent,
                None,
                Some(instance),
                Some(weak as _),
            )
        };
        let hwnd = match hwnd {
            Ok(h) => h,
            Err(e) => {
                drop(unsafe { Box::from_raw(weak) });
                return Err(e);
            }
        };
        shell.hwnd.set(hwnd);
        let mut st = shell.state.borrow_mut();
        st.hwnd = hwnd;
        st.own_hwnd = hwnd;
        st.view.dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32;
        Ok(hwnd)
    }

    /// The editor's own window (message-only for hosted editors).
    pub fn hwnd(&self) -> HWND {
        self.shell.hwnd.get()
    }

    /// Runs `f` on the state, then repaints and delivers events as needed.
    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        let r = {
            let mut st = self.shell.state.borrow_mut();
            let r = f(&mut st);
            apply_effects(&self.shell, &mut st);
            r
        };
        dispatch(&self.shell);
        r
    }

    fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.shell.state.borrow())
    }

    /// Sets the handler for editor events (replacing any previous one). The
    /// handler gets the editor, so it does not need to hold a handle itself.
    pub fn on_event(&self, f: impl FnMut(&Editor, &Event) + 'static) {
        *self.shell.handler.borrow_mut() = Some(Box::new(f));
    }

    /// Positions the editor in its parent's (or host's) client pixels. A zero
    /// size is fine.
    pub fn set_bounds(&self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(mut hd) = self.shell.hosted.get() {
            self.shell.invalidate();
            (hd.x, hd.y, hd.w, hd.h) = (x, y, w.max(0), h.max(0));
            self.shell.hosted.set(Some(hd));
            self.with(|s| s.resize(w.max(0) as u32, h.max(0) as u32));
            return;
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd(), None, x, y, w.max(0), h.max(0), SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }

    /// Shows or hides the editor panel. A hidden editor keeps its text,
    /// history, selections and scroll position, but releases its render target
    /// and stops its timers. Focus inside it moves to the parent.
    pub fn set_visible(&self, visible: bool) {
        let hwnd = self.hwnd();
        if let Some(mut h) = self.shell.hosted.get() {
            h.visible = visible;
            self.shell.hosted.set(Some(h));
            if !visible {
                self.with(|s| {
                    s.set_focus(false);
                    s.render.release_target();
                });
            }
            self.shell.invalidate();
            return;
        }
        unsafe {
            if !visible {
                if GetFocus() == hwnd {
                    let _ = SetFocus(GetParent(hwnd).ok());
                }
                let _ = ShowWindow(hwnd, SW_HIDE);
                self.shell.set_timer(TIMER_BLINK, None);
                self.shell.set_timer(TIMER_SCROLL, None);
                *self.shell.target.borrow_mut() = None;
                self.shell.state.borrow_mut().render.release_target();
            } else {
                let _ = ShowWindow(hwnd, SW_SHOWNA);
            }
        }
    }

    /// Whether the panel is shown (its parent may still be hidden).
    pub fn is_visible(&self) -> bool {
        if let Some(h) = self.shell.hosted.get() {
            return h.visible;
        }
        let style = unsafe { GetWindowLongPtrW(self.hwnd(), GWL_STYLE) } as u32;
        style & WS_VISIBLE.0 != 0
    }

    /// Destroys the editor window; focus inside it moves to the parent. Other
    /// handles to this editor become inert.
    pub fn close(&self) {
        let hwnd = self.hwnd();
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                return;
            }
            if GetFocus() == hwnd {
                let _ = SetFocus(GetParent(hwnd).ok());
            }
            let _ = DestroyWindow(hwnd);
        }
        *self.shell.target.borrow_mut() = None;
        self.shell.state.borrow_mut().render.release_target();
    }

    /// Gives the editor keyboard focus (for hosted editors: marks it focused;
    /// the host keeps routing keys to it).
    pub fn focus(&self) {
        if self.shell.hosted.get().is_some() {
            self.with(|s| s.set_focus(true));
            return;
        }
        unsafe {
            let _ = SetFocus(Some(self.hwnd()));
        }
    }

    pub fn has_focus(&self) -> bool {
        self.read(|s| s.focused)
    }

    /// For hosted editors: handles a message the host window received (input,
    /// focus, cursor and DPI messages; mouse positions in host client pixels).
    /// Returns `None` when the editor did not handle it, so the host should.
    pub fn handle_message(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        if self.shell.hosted.get().is_none_or(|h| !h.visible) || msg == WM_PAINT {
            return None;
        }
        let r = message(&self.shell, msg, wparam, lparam);
        dispatch(&self.shell);
        r
    }

    /// For hosted editors: draws the editor at its bounds into the host's
    /// render target, between the host's `BeginDraw` and `EndDraw`. The target
    /// must use the host window's DPI.
    pub fn paint_into(&self, rt: &ID2D1RenderTarget) {
        let Some(h) = self.shell.hosted.get() else { return };
        if !h.visible || h.w <= 0 || h.h <= 0 {
            return;
        }
        let mut st = self.shell.state.borrow_mut();
        // The host's target decides the DPI, which also covers bitmap targets
        // for screenshots.
        let (mut dpi, mut dpi_y) = (0.0, 0.0);
        unsafe { rt.GetDpi(&mut dpi, &mut dpi_y) };
        if dpi > 0.0 && dpi != st.view.dpi {
            st.set_dpi(dpi);
            st.resize(h.w.max(0) as u32, h.h.max(0) as u32);
        }
        let k = st.view.dpi / 96.0;
        unsafe {
            let mut old = windows_numerics::Matrix3x2::default();
            rt.GetTransform(&mut old);
            let m = windows_numerics::Matrix3x2 {
                M31: old.M31 + h.x as f32 / k * old.M11 + h.y as f32 / k * old.M21,
                M32: old.M32 + h.x as f32 / k * old.M12 + h.y as f32 / k * old.M22,
                ..old
            };
            rt.SetTransform(&m);
            st.paint(rt);
            rt.SetTransform(&old);
        }
    }

    // ---- Input from hosts that translate their own events ----
    //
    // A host with its own keymap, focus and hit testing calls these instead of
    // `handle_message`. Positions are DIPs from the editor's top-left.

    /// Marks a hosted editor focused or not; the host decides where keys go.
    pub fn set_focused(&self, focused: bool) {
        if self.read(|s| s.focused) != focused {
            self.with(|s| s.set_focus(focused));
        }
    }

    /// A key press. Returns whether the editor used it; the host gets the rest.
    pub fn key_down(&self, chord: Chord) -> bool {
        self.shell.set_timer(TIMER_HOVER, None);
        let r = self.with(|s| {
            s.hide_tooltip();
            s.key(chord)
        });
        match r {
            KeyResult::Handled => true,
            KeyResult::ContextMenu => {
                self.show_context_menu(None);
                true
            }
            KeyResult::Unhandled => false,
        }
    }

    /// Typed text, as WM_CHAR delivers it: auto-closing and suggestions apply.
    pub fn type_chars(&self, text: &str) {
        self.with(|s| text.encode_utf16().for_each(|u| s.on_char(u)));
    }

    /// A left button press; `clicks` counts double and triple clicks.
    pub fn mouse_down(&self, x: f32, y: f32, mods: Mods, clicks: u32) {
        self.shell.set_timer(TIMER_HOVER, None);
        self.with(|s| {
            s.set_focus(true);
            s.hide_tooltip();
            s.mouse_down(x, y, mods, clicks);
        });
    }

    /// The mouse moved: drags a selection or scrollbar, or tracks hover.
    pub fn mouse_move(&self, x: f32, y: f32) {
        let shell = &self.shell;
        self.with(|s| {
            if s.drag.is_some() {
                s.drag_to(x, y);
                shell.set_timer(TIMER_SCROLL, s.autoscroll_needed().then_some(30));
            } else {
                s.hover_at(x, y);
                if s.tooltip.as_ref().is_some_and(|t| (t.0 - x).abs() > 4.0 || (t.1 - y).abs() > 4.0) {
                    s.hide_tooltip();
                }
                // The tooltip shows once the mouse rests.
                if s.tooltip.is_none() {
                    shell.hover_at.set((x, y));
                    shell.set_timer(TIMER_HOVER, Some(HOVER_DELAY_MS));
                }
            }
        });
    }

    pub fn mouse_up(&self) {
        self.shell.set_timer(TIMER_SCROLL, None);
        self.with(|s| s.mouse_up());
    }

    pub fn mouse_leave(&self) {
        self.shell.set_timer(TIMER_HOVER, None);
        self.with(|s| {
            s.mouse_leave();
            s.hide_tooltip();
        });
    }

    /// Wheel notches (positive: away from the user, or right when horizontal).
    pub fn wheel(&self, notches: f32, horizontal: bool, mods: Mods) {
        self.shell.set_timer(TIMER_HOVER, None);
        self.with(|s| {
            s.hide_tooltip();
            s.wheel(notches, horizontal, mods);
        });
    }

    /// A right click: moves the caret there unless it is in a selection, then
    /// shows the context menu.
    pub fn context_menu_at(&self, x: f32, y: f32) {
        self.with(|s| {
            s.set_focus(true);
            s.right_down(x, y);
        });
        self.show_context_menu(Some((x, y)));
    }

    /// Shows the context menu at (x, y) DIPs, or at the caret.
    fn show_context_menu(&self, at: Option<(f32, f32)>) {
        let input = self.shell.input_hwnd();
        if input.is_invalid() {
            return;
        }
        let lparam = match at {
            None => LPARAM(-1),
            Some((x, y)) => {
                let k = self.read(|s| s.view.dpi / 96.0);
                let (ox, oy) = self.shell.origin();
                let mut pt = POINT {
                    x: ox + (x * k) as i32,
                    y: oy + (y * k) as i32,
                };
                unsafe {
                    let _ = ClientToScreen(input, &mut pt);
                }
                LPARAM(((pt.y as u16 as isize) << 16) | pt.x as u16 as isize)
            }
        };
        context_menu(&self.shell, lparam);
    }

    /// What the mouse resting at (x, y) DIPs shows, if anything, as the hover
    /// timer would; for hosts and tests that drive it themselves.
    pub fn hover_now(&self, x: f32, y: f32) -> Option<String> {
        self.with(|s| {
            s.hover(x, y);
            s.tooltip.as_ref().map(|t| t.2.clone())
        })
    }

    /// The signature popup's contents, while it is open.
    pub fn signature(&self) -> Option<crate::SignatureHelp> {
        self.read(|s| s.signature.clone())
    }

    /// Sets the host's language features (`None` goes back to the grammar).
    pub fn set_language_service(&self, service: Option<Rc<dyn crate::LanguageService>>) {
        self.with(|s| s.set_service(service));
    }

    /// Asks the language service for problems now rather than after the pause.
    pub fn refresh_diagnostics(&self) {
        self.shell.set_timer(TIMER_DIAGNOSE, None);
        self.with(|s| s.refresh_diagnostics());
    }

    /// The underlined problems as (start, end, message), by offset.
    pub fn diagnostics(&self) -> Vec<(usize, usize, String)> {
        self.read(|s| {
            let mut out: Vec<_> = s
                .decorations
                .in_range(0, s.doc.buffer.len())
                .filter(|t| t.options.squiggle.is_some())
                .map(|t| (t.start, t.end, t.options.hover.clone().unwrap_or_default()))
                .collect();
            out.sort();
            out
        })
    }

    /// The labels the suggest popup shows, in order, while it is open.
    pub fn suggestions(&self) -> Option<Vec<String>> {
        self.read(|s| {
            s.suggest
                .open
                .then(|| s.suggest.rows.iter().map(|r| s.suggest.items[r.item].label.clone()).collect())
        })
    }

    /// The caret in DIPs from the editor's top-left: (x, y, height).
    pub fn caret(&self) -> (f32, f32, f32) {
        self.with(|s| s.caret_point())
    }

    /// Where the mouse pointer should be an arrow rather than an I-beam (the
    /// gutter, scrollbars and overlays), as (x, y, width, height) in DIPs.
    pub fn arrow_areas(&self) -> Vec<(f32, f32, f32, f32)> {
        self.with(|s| s.arrow_areas())
            .into_iter()
            .map(|r| (r.l, r.t, r.w(), r.h()))
            .collect()
    }

    pub fn text(&self) -> String {
        self.read(|s| s.doc.buffer.text())
    }

    /// Replaces the text as a new document (history and decorations cleared).
    pub fn set_text(&self, text: &str) {
        self.with(|s| s.load(Buffer::new(text)));
    }

    /// Loads a file's bytes (any supported encoding) and picks the language from
    /// its name.
    pub fn load_file(&self, path: &Path) -> std::io::Result<()> {
        let bytes = std::fs::read(path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.load_bytes(&bytes, &name);
        Ok(())
    }

    /// Loads file contents; `name` picks the language. Invalid bytes are
    /// replaced and reported with [`Event::Warning`].
    pub fn load_bytes(&self, bytes: &[u8], name: &str) {
        self.with(|s| {
            let d = text::decode(bytes);
            if d.lossy {
                s.events.push(Event::Warning(format!(
                    "{name} contains bytes that are not valid text; they were replaced."
                )));
            }
            s.load(Buffer::from_decoded(d));
            s.set_lang_for_file(name);
        });
    }

    /// The text encoded with its file's encoding and line breaks.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.read(|s| s.doc.buffer.to_bytes())
    }

    /// Writes the text to `path` and marks it saved.
    pub fn save_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_bytes())?;
        self.mark_saved();
        Ok(())
    }

    /// Whether the text differs from the last save (or load).
    pub fn is_dirty(&self) -> bool {
        self.read(|s| s.doc.is_dirty())
    }

    pub fn mark_saved(&self) {
        self.with(|s| s.doc.history.mark_saved());
    }

    /// Changes with every edit; unique across editors.
    pub fn version(&self) -> u64 {
        self.read(|s| s.doc.buffer.version)
    }

    /// Line `i` (0-based) without its line break.
    pub fn line(&self, i: usize) -> String {
        self.read(|s| {
            if i < s.doc.buffer.line_count() {
                s.doc.buffer.line(i).into_owned()
            } else {
                String::new()
            }
        })
    }

    pub fn line_count(&self) -> usize {
        self.read(|s| s.doc.buffer.line_count())
    }

    pub fn eol(&self) -> text::Eol {
        self.read(|s| s.doc.buffer.eol)
    }

    pub fn set_eol(&self, eol: text::Eol) {
        self.with(|s| s.doc.buffer.eol = eol);
    }

    pub fn encoding(&self) -> text::Encoding {
        self.read(|s| s.doc.buffer.encoding)
    }

    /// Applies edits as one undoable step, like Monaco's `executeEdits`. The
    /// ranges are in the current text and must not overlap; carets move with
    /// the text.
    pub fn apply_edits(&self, edits: &[TextEdit]) {
        self.with(|s| {
            let b = &s.doc.buffer;
            let tx = Transaction::new(
                edits
                    .iter()
                    .map(|e| Edit::new(b.offset(e.start), b.offset(e.end), text::normalize_eol(&e.text)))
                    .collect(),
            );
            let sels = s.doc.sels.map_tx(&tx);
            s.edit(crate::commands::Outcome { tx, sels }, EditKind::Other);
        });
    }

    /// The primary selection as a byte range of `text()` (start before end).
    pub fn selection_range(&self) -> std::ops::Range<usize> {
        self.read(|s| {
            let p = s.doc.sels.primary();
            p.start()..p.end()
        })
    }

    /// The primary caret as a byte offset of `text()`.
    pub fn caret_offset(&self) -> usize {
        self.read(|s| s.doc.sels.primary().head)
    }

    /// Selects `anchor..head` (byte offsets, clamped to char boundaries) as the
    /// only selection, and scrolls the caret into view.
    pub fn select_range(&self, anchor: usize, head: usize) {
        self.with(|s| {
            let b = &s.doc.buffer;
            let (anchor, head) = (b.floor_char(anchor.min(b.len())), b.floor_char(head.min(b.len())));
            s.set_sels(Selections::single(Selection::new(anchor, head)));
            s.reveal(false);
        });
    }

    /// Replaces a byte range of `text()` as one undo step.
    pub fn replace_range(&self, range: std::ops::Range<usize>, text: &str) {
        let (start, end) = self.read(|s| {
            let b = &s.doc.buffer;
            (b.pos(b.floor_char(range.start.min(b.len()))), b.pos(b.floor_char(range.end.min(b.len()))))
        });
        self.apply_edits(&[TextEdit {
            start,
            end,
            text: text.to_string(),
        }]);
    }

    /// The primary caret.
    pub fn position(&self) -> Pos {
        self.read(|s| s.doc.buffer.pos(s.doc.sels.primary().head))
    }

    /// Every selection; the primary one first.
    pub fn selections(&self) -> Vec<Range> {
        self.read(|s| {
            let b = &s.doc.buffer;
            let p = s.doc.sels.primary_index();
            let mut v: Vec<Range> = s
                .doc
                .sels
                .iter()
                .map(|r| Range {
                    anchor: b.pos(r.anchor),
                    head: b.pos(r.head),
                })
                .collect();
            v.swap(0, p);
            v
        })
    }

    /// Sets the selections; the first one is primary.
    pub fn set_selections(&self, ranges: &[Range]) {
        if ranges.is_empty() {
            return;
        }
        self.with(|s| {
            let b = &s.doc.buffer;
            let v = ranges
                .iter()
                .map(|r| Selection::new(b.offset(r.anchor), b.offset(r.head)))
                .collect();
            s.set_sels(Selections::new(v, 0));
        });
    }

    /// Removes the decorations `old` and adds `new`, returning their ids (like
    /// Monaco's `deltaDecorations`). Decorations move with edits.
    pub fn delta_decorations(&self, old: &[DecorationId], new: &[Decoration]) -> Vec<DecorationId> {
        self.with(|s| {
            let b = &s.doc.buffer;
            let new = new
                .iter()
                .map(|d| (b.offset(d.start), b.offset(d.end), d.options.clone()))
                .collect();
            let ids = s.decorations.delta(old, new);
            s.invalidate();
            ids
        })
    }

    /// Where a decoration is now, if it still exists.
    pub fn decoration_range(&self, id: DecorationId) -> Option<(Pos, Pos)> {
        self.read(|s| {
            let t = s.decorations.get(id)?;
            Some((s.doc.buffer.pos(t.start), s.doc.buffer.pos(t.end)))
        })
    }

    /// The language id, or `None` for plain text.
    pub fn language(&self) -> Option<&'static str> {
        self.read(|s| s.lang.map(|l| l.id))
    }

    /// Sets the language by id; unknown or not compiled-in ids give plain text.
    pub fn set_language(&self, id: &str) {
        self.with(|s| s.set_lang(lang::by_id(id)));
    }

    /// Sets the language from a file name's extension.
    pub fn set_language_for_file(&self, name: &str) {
        self.with(|s| s.set_lang_for_file(name));
    }

    /// Switches to a base theme (dropping color overrides).
    pub fn set_theme(&self, theme: Theme) {
        self.with(|s| s.set_theme(theme));
    }

    /// Overrides one theme color by name (`keyword`, `string`, `background`,
    /// `selection`, `lineNumber`, ...; see [`Ink::from_name`]). Returns false for
    /// an unknown name.
    pub fn set_theme_color(&self, name: &str, argb: u32) -> bool {
        let Some(ink) = Ink::from_name(name) else { return false };
        self.with(|s| {
            let mut p = s.render.palette.clone();
            p.set(ink, argb);
            s.render.set_palette(p);
            s.invalidate();
        });
        true
    }

    pub fn options(&self) -> Options {
        self.read(|s| s.opts.clone())
    }

    pub fn set_options(&self, o: Options) {
        self.with(|s| s.set_options(o));
    }

    pub fn set_read_only(&self, ro: bool) {
        self.with(|s| s.opts.read_only = ro);
    }

    /// Binds a chord such as `"Ctrl+Shift+P"` to a host command, delivered as
    /// [`Event::Command`]. Host bindings take precedence over the defaults.
    /// Returns false when the chord does not parse.
    pub fn add_command(&self, chord: &str, id: u32) -> bool {
        let Some(c) = Chord::parse(chord) else { return false };
        self.with(|s| s.keymap.bind(c, Binding::Command(id)));
        true
    }

    /// Binds a chord to an editor action, or unbinds it with `None`.
    pub fn bind_key(&self, chord: &str, action: Option<crate::Action>) -> bool {
        let Some(c) = Chord::parse(chord) else { return false };
        let b = action.map_or(Binding::Nothing, Binding::Action);
        self.with(|s| s.keymap.bind(c, b));
        true
    }

    /// Runs an editor action as if its key was pressed.
    pub fn run(&self, action: crate::Action) {
        self.with(|s| s.run(action));
    }

    /// Sets the completion provider. Its items come first, then the language's
    /// keywords and the words of the text. The provider runs while the editor is
    /// busy, so it must not call back into the editor.
    pub fn set_completion_provider(&self, p: impl crate::CompletionProvider + 'static) {
        self.with(|s| s.provider = Some(Rc::new(p)));
    }

    /// Inserts text at every caret, as typing would.
    pub fn type_text(&self, text: &str) {
        self.with(|s| s.type_text(text));
    }

    pub fn zoom(&self) -> f32 {
        self.read(|s| s.zoom)
    }

    pub fn set_zoom(&self, z: f32) {
        self.with(|s| s.set_zoom(z));
    }

    /// Scrolls so `line` (0-based) is visible, centered when it is far away.
    pub fn reveal_line(&self, line: usize) {
        self.with(|s| {
            s.ensure_fonts();
            s.view.lines = s.rows.rows(s.doc.buffer.line_count());
            let row = s.rows.row_of(line, 0);
            s.view.reveal_line(row, 0, true);
            s.invalidate();
        });
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Action;

    /// A hidden top-level window to parent editors in tests.
    fn parent() -> HWND {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                400,
                300,
                None,
                None,
                None,
                None,
            )
            .unwrap()
        }
    }

    fn send(hwnd: HWND, msg: u32, w: usize, l: isize) {
        unsafe {
            SendMessageW(hwnd, msg, Some(WPARAM(w)), Some(LPARAM(l)));
        }
    }

    #[test]
    fn typing_through_window_messages() {
        let p = parent();
        let ed = Editor::create(p, Options::default()).unwrap();
        ed.set_bounds(0, 0, 400, 300);
        let changes = Rc::new(Cell::new(0));
        let c = changes.clone();
        ed.on_event(move |_, e| {
            if let Event::ContentChanged { .. } = e {
                c.set(c.get() + 1);
            }
        });
        let h = ed.hwnd();
        for ch in "hi there".encode_utf16() {
            send(h, WM_CHAR, ch as usize, 0);
        }
        // A surrogate pair arrives as two WM_CHARs.
        for u in "😀".encode_utf16() {
            send(h, WM_CHAR, u as usize, 0);
        }
        assert_eq!(ed.text(), "hi there😀");
        send(h, WM_KEYDOWN, VK_HOME.0 as usize, 0);
        send(h, WM_KEYDOWN, VK_DELETE.0 as usize, 0);
        send(h, WM_KEYDOWN, VK_RETURN.0 as usize, 0);
        assert_eq!(ed.text(), "\ni there😀");
        assert_eq!(ed.position(), Pos::new(1, 0));
        assert!(changes.get() >= 3);
        ed.run(Action::Undo);
        assert_eq!(ed.text(), "i there😀");
        // Control characters from handled keys are not typed.
        send(h, WM_CHAR, 0x08, 0);
        assert_eq!(ed.text(), "i there😀");
        assert!(ed.is_dirty());
        ed.close();
        unsafe {
            let _ = DestroyWindow(p);
        }
    }

    #[test]
    fn handlers_can_call_back_into_the_editor() {
        let p = parent();
        let ed = Editor::create(p, Options::default()).unwrap();
        let seen = Rc::new(RefCell::new(String::new()));
        let s = seen.clone();
        ed.on_event(move |ed, e| {
            if let Event::ContentChanged { .. } = e {
                *s.borrow_mut() = ed.text();
            }
        });
        ed.type_text("abc");
        assert_eq!(*seen.borrow(), "abc");
        ed.close();
        unsafe {
            let _ = DestroyWindow(p);
        }
    }

    #[test]
    fn host_commands_and_panel_lifecycle() {
        let p = parent();
        let ed = Editor::create(p, Options::default()).unwrap();
        ed.set_bounds(0, 0, 300, 200);
        assert!(ed.add_command("Ctrl+Shift+E", 42));
        let got = Rc::new(Cell::new(0));
        let g = got.clone();
        ed.on_event(move |_, e| {
            if let Event::Command(id) = e {
                g.set(*id);
            }
        });
        // Simulate the key with the modifiers reported down.
        ed.with(|s| {
            s.key(Chord::new(b'E' as u16, true, true, false));
        });
        assert_eq!(got.get(), 42);

        ed.type_text("keep me");
        ed.set_visible(false);
        assert!(!ed.is_visible());
        assert!(ed.shell.target.borrow().is_none());
        ed.set_visible(true);
        assert!(ed.is_visible());
        assert_eq!(ed.text(), "keep me");
        let h = ed.hwnd();
        ed.close();
        assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
        unsafe {
            let _ = DestroyWindow(p);
        }
    }

    #[test]
    fn dropping_the_last_handle_destroys_the_window() {
        let p = parent();
        let ed = Editor::create(p, Options::default()).unwrap();
        let h = ed.hwnd();
        drop(ed);
        assert!(!unsafe { IsWindow(Some(h)) }.as_bool());
        unsafe {
            let _ = DestroyWindow(p);
        }
    }

    #[test]
    fn decorations_follow_edits() {
        let p = parent();
        let ed = Editor::create(p, Options::default()).unwrap();
        ed.set_text("let x = bad;");
        let opts = DecorationOptions {
            squiggle: Some(crate::Severity::Error),
            hover: Some("undefined name".into()),
            stickiness: crate::Stickiness::NeverGrows,
            ..Default::default()
        };
        let ids = ed.delta_decorations(
            &[],
            &[Decoration {
                start: Pos::new(0, 8),
                end: Pos::new(0, 11),
                options: opts,
            }],
        );
        ed.apply_edits(&[TextEdit {
            start: Pos::new(0, 0),
            end: Pos::new(0, 0),
            text: "// x\n".into(),
        }]);
        assert_eq!(ed.decoration_range(ids[0]), Some((Pos::new(1, 8), Pos::new(1, 11))));
        ed.run(crate::Action::Undo);
        assert_eq!(ed.text(), "let x = bad;");
        assert_eq!(ed.decoration_range(ids[0]), Some((Pos::new(0, 8), Pos::new(0, 11))));
        assert!(ed.delta_decorations(&ids, &[]).is_empty());
        assert_eq!(ed.decoration_range(ids[0]), None);
        assert!(ed.set_theme_color("keyword", 0xFFFF0000));
        assert!(!ed.set_theme_color("nope", 0));
        ed.close();
        unsafe {
            let _ = DestroyWindow(p);
        }
    }

    #[test]
    fn hosted_editor_takes_forwarded_input_and_paints_into_a_target() {
        let host = parent();
        let ed = Editor::create_hosted(host, Options::default(), Theme::Light).unwrap();
        ed.set_bounds(10, 20, 300, 200);
        // Typing reaches a hosted editor only through the host.
        for ch in "abc".encode_utf16() {
            assert!(ed.handle_message(WM_CHAR, WPARAM(ch as usize), LPARAM(0)).is_some());
        }
        assert_eq!(ed.text(), "abc");
        // A click outside its bounds is the host's.
        let outside = LPARAM(5 | (5 << 16));
        assert!(ed.handle_message(WM_LBUTTONDOWN, WPARAM(0), outside).is_none());
        assert!(!ed.has_focus());
        let inside = LPARAM(40 | (30 << 16));
        assert!(ed.handle_message(WM_LBUTTONDOWN, WPARAM(0), inside).is_some());
        ed.handle_message(WM_LBUTTONUP, WPARAM(0), inside);
        assert!(ed.has_focus());
        let t = render::hwnd_target(host, 400, 300, 96.0).unwrap().0;
        unsafe {
            t.BeginDraw();
            ed.paint_into(&t);
            t.EndDraw(None, None).unwrap();
        }
        ed.set_visible(false);
        assert!(ed.handle_message(WM_CHAR, WPARAM('x' as usize), LPARAM(0)).is_none());
        drop(ed);
        unsafe {
            let _ = DestroyWindow(host);
        }
    }

    #[test]
    fn hosted_editor_takes_translated_input_without_a_host_window() {
        // A host's headless tests have no window to give it.
        let ed = Editor::create_hosted(HWND::default(), Options::default(), Theme::Light).unwrap();
        ed.set_language("sql");
        ed.set_bounds(0, 0, 400, 200);
        ed.set_focused(true);
        ed.type_chars("x (");
        assert_eq!(ed.text(), "x ()");
        assert_eq!(ed.caret_offset(), 3);
        // Keys the editor has no use for are the host's.
        assert!(!ed.key_down(Chord::new(VK_F9.0, false, false, false)));
        assert!(ed.key_down(Chord::new(VK_HOME.0, false, false, false)));
        assert_eq!(ed.caret_offset(), 0);
        ed.select_range(2, 4);
        assert_eq!(ed.selection_range(), 2..4);
        ed.replace_range(2..4, "y");
        assert_eq!(ed.text(), "x y");
        // A double click selects the word under it: the caret is after `y`.
        let (x, _, h) = ed.caret();
        ed.mouse_down(x - 2.0, h / 2.0, Mods::default(), 2);
        ed.mouse_up();
        assert_eq!(ed.selection_range(), 2..3);
        assert!(ed.arrow_areas().iter().any(|&(x, _, w, _)| x == 0.0 && w > 0.0));
        ed.set_focused(false);
        assert!(!ed.has_focus());
    }
}
