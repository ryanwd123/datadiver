//! An editor drawn into its host's own render target with `paint_into`, the
//! way a single-HWND app (like Penmark) would embed it. The host paints a header
//! bar itself and forwards input to the editor with `handle_message`.
//!
//! Run with `cargo run -p code_editor --example hosted`.

#![windows_subsystem = "windows"]

use std::cell::RefCell;

use code_editor::{Editor, Options, Theme};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

const HEADER: i32 = 32;

struct Host {
    factory: ID2D1Factory,
    target: Option<ID2D1HwndRenderTarget>,
    editor: Option<Editor>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn editor() -> Option<Editor> {
    HOST.with(|h| h.borrow().as_ref().and_then(|h| h.editor.clone()))
}

fn paint(hwnd: HWND) {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let Some(host) = h.as_mut() else { return };
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let dpi = GetDpiForWindow(hwnd) as f32;
            if host.target.is_none() {
                let props = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: D2D_SIZE_U {
                        width: rc.right as u32,
                        height: rc.bottom as u32,
                    },
                    presentOptions: D2D1_PRESENT_OPTIONS_NONE,
                };
                host.target = host.factory.CreateHwndRenderTarget(&Default::default(), &props).ok();
            }
            let Some(t) = host.target.clone() else { return };
            t.SetDpi(dpi, dpi);
            let _ = t.Resize(&D2D_SIZE_U {
                width: rc.right as u32,
                height: rc.bottom as u32,
            });
            t.BeginDraw();
            let header = D2D1_COLOR_F {
                r: 0.18,
                g: 0.2,
                b: 0.25,
                a: 1.0,
            };
            t.Clear(Some(&header));
            if let Some(ed) = &host.editor {
                ed.paint_into(&t);
            }
            if t.EndDraw(None, None).is_err() {
                host.target = None;
            }
        }
    });
}

fn layout(hwnd: HWND) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let scale = unsafe { GetDpiForWindow(hwnd) } as i32;
    let head = HEADER * scale / 96;
    if let Some(ed) = editor() {
        ed.set_bounds(0, head, rc.right, rc.bottom - head);
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // Input goes to the editor first; it declines what is outside it.
        if let Some(ed) = editor()
            && let Some(r) = ed.handle_message(msg, wparam, lparam)
        {
            return r;
        }
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                paint(hwnd);
                let _ = EndPaint(hwnd, &ps);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE => {
                layout(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_GETDLGCODE => LRESULT(DLGC_WANTALLKEYS as isize),
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: instance,
            lpszClassName: w!("HostedEditor"),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("HostedEditor"),
            w!("Hosted editor (paint_into)"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            900,
            600,
            None,
            None,
            Some(instance),
            None,
        )?;
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let ed = Editor::create_hosted(hwnd, Options::default(), Theme::Dark)?;
        ed.set_text("# Drawn into the host's render target.\nprint(\"hello from paint_into\")\n");
        ed.set_language("python");
        HOST.with(|h| {
            *h.borrow_mut() = Some(Host {
                factory,
                target: None,
                editor: Some(ed.clone()),
            })
        });
        layout(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        ed.focus();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        HOST.with(|h| h.borrow_mut().take());
        Ok(())
    }
}
