//! Two editors side by side in one window: Python on the left, SQL on the
//! right. Each is an independent child window; the title shows which one has
//! focus and where its caret is.
//!
//! Run with `cargo run -p code_editor --example two_editors`.

#![windows_subsystem = "windows"]

use std::cell::RefCell;

use code_editor::{Editor, Event, Options, Theme};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

thread_local! {
    static EDITORS: RefCell<Vec<Editor>> = const { RefCell::new(Vec::new()) };
}

const PYTHON: &str = "def greet(name: str) -> str:\n    \"\"\"Say hello.\"\"\"\n    return f\"Hello, {name}!\"\n\n\nfor n in [\"Ada\", \"Grace\"]:\n    print(greet(n))\n";
const SQL: &str = "-- Orders per customer\nSELECT c.name, COUNT(o.id) AS orders\nFROM customers c\nLEFT JOIN orders o ON o.customer_id = c.id\nGROUP BY c.name\nORDER BY orders DESC;\n";

fn layout(hwnd: HWND) {
    let mut rc = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let half = rc.right / 2;
    EDITORS.with(|e| {
        let e = e.borrow();
        e[0].set_bounds(0, 0, half - 1, rc.bottom);
        e[1].set_bounds(half + 1, 0, rc.right - half - 1, rc.bottom);
    });
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SIZE => {
                if EDITORS.with(|e| e.borrow().len() == 2) {
                    layout(hwnd);
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

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hInstance: instance,
            lpszClassName: w!("TwoEditors"),
            lpfnWndProc: Some(wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("TwoEditors"),
            w!("Two editors"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1200,
            700,
            None,
            None,
            Some(instance),
            None,
        )?;

        for (i, (text, lang)) in [(PYTHON, "python"), (SQL, "sql")].into_iter().enumerate() {
            let ed = Editor::create_with_theme(hwnd, Options::default(), Theme::Dark)?;
            ed.set_text(text);
            ed.set_language(lang);
            let target = hwnd.0 as isize;
            ed.on_event(move |ed, e| {
                if matches!(e, Event::SelectionChanged | Event::Focus(true)) && ed.has_focus() {
                    let p = ed.position();
                    let title = format!("Two editors - editor {} ({}) - Ln {}, Col {}", i + 1, lang, p.line + 1, p.col + 1);
                    let _ = SetWindowTextW(HWND(target as _), &HSTRING::from(title));
                }
            });
            EDITORS.with(|e| e.borrow_mut().push(ed));
        }
        layout(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWDEFAULT);
        EDITORS.with(|e| e.borrow()[0].focus());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        EDITORS.with(|e| e.borrow_mut().clear());
        Ok(())
    }
}
