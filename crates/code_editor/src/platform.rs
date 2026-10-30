//! Editor-side Win32 services: the clipboard, the context menu and cursors.

use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

pub fn get_clipboard(hwnd: HWND) -> Option<String> {
    unsafe {
        OpenClipboard(Some(hwnd)).ok()?;
        let text = (|| {
            let h = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let hg = HGLOBAL(h.0);
            let p = GlobalLock(hg) as *const u16;
            if p.is_null() {
                return None;
            }
            let max = GlobalSize(hg) / 2;
            let mut n = 0;
            while n < max && *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(hg);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

pub fn set_clipboard(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(hg) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(hg);
                if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hg.0))).is_err() {
                    let _ = GlobalFree(Some(hg));
                }
            }
        }
        let _ = CloseClipboard();
    }
}

pub fn clipboard_has_text() -> bool {
    unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() }
}

/// The paths in a list of dropped files.
pub fn hdrop_paths(hdrop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        (0..n)
            .filter_map(|i| {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
                (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..got])))
            })
            .collect()
    }
}

pub struct MenuItem<'a> {
    pub id: u32,
    pub label: &'a str,
    pub enabled: bool,
}

/// Shows a context menu at client pixel (x, y); returns the chosen item's id.
/// An id of 0 is a separator.
pub fn popup_menu(hwnd: HWND, items: &[MenuItem], x: i32, y: i32) -> Option<u32> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            let _ = if item.id == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())
            } else {
                let flags = if item.enabled { MF_STRING } else { MF_STRING | MF_GRAYED };
                AppendMenuW(menu, flags, item.id as usize, &HSTRING::from(item.label))
            };
        }
        let mut pt = POINT { x, y };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        (chosen.0 > 0).then_some(chosen.0 as u32)
    }
}

pub fn set_cursor(id: PCWSTR) {
    unsafe {
        if let Ok(c) = LoadCursorW(None, id) {
            SetCursor(Some(c));
        }
    }
}

/// Milliseconds between caret blinks.
pub fn caret_blink_ms() -> u32 {
    unsafe { GetCaretBlinkTime() }.clamp(200, 1200)
}

pub fn double_click_ms() -> u32 {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() }
}

/// Lines to scroll per wheel notch.
pub fn wheel_lines() -> u32 {
    let mut n = 3u32;
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWHEELSCROLLLINES,
            0,
            Some(&mut n as *mut u32 as _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    n
}

thread_local! {
    /// `WIN_MONACO_TRACE`: "1" for OutputDebugString, or a file to append to.
    static TRACE: Option<String> = std::env::var("WIN_MONACO_TRACE").ok().filter(|v| !v.is_empty());
}

pub fn tracing() -> bool {
    TRACE.with(|t| t.is_some())
}

/// Writes a timing line when `WIN_MONACO_TRACE` is set.
pub fn trace(msg: &str) {
    TRACE.with(|t| {
        let Some(dest) = t else { return };
        let line = format!("win_monaco: {msg}\n");
        if dest == "1" {
            unsafe {
                windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(&HSTRING::from(line));
            }
        } else if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dest) {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    });
}

/// Milliseconds since this process was created.
pub fn ms_since_process_start() -> f64 {
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64;
    unsafe {
        let (mut c, mut e, mut k, mut u) = Default::default();
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_err() {
            return 0.0;
        }
        (ft(GetSystemTimePreciseAsFileTime()) - ft(c)) / 10_000.0
    }
}
