//! The clipboard, file dialogs, and opening URLs and folders.

use std::path::PathBuf;

/// What the clipboard holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardContent {
    Paths(Vec<PathBuf>),
    Text(String),
}

pub struct PathPrompt<'a> {
    pub directories: bool,
    pub multiple: bool,
    pub title: &'a str,
}

/// Font files installed for this user only, which DirectWrite's system
/// collection does not list. They are registered under HKCU, by full path or
/// by a file name in the per-user fonts folder.
pub fn user_font_files() -> Vec<PathBuf> {
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::*;
    use windows::core::{PWSTR, w};
    let mut files = Vec::new();
    let folder = std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join(r"Microsoft\Windows\Fonts"));
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows NT\CurrentVersion\Fonts"),
            None,
            KEY_READ,
            &mut key,
        ) != ERROR_SUCCESS
        {
            return files;
        }
        let mut name = [0u16; 512];
        let mut data = [0u16; 1024];
        for index in 0.. {
            let mut name_len = name.len() as u32;
            let mut data_len = (data.len() * 2) as u32;
            let mut kind = 0u32;
            let result = RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr() as *mut u8),
                Some(&mut data_len),
            );
            if result == ERROR_NO_MORE_ITEMS {
                break;
            }
            if result != ERROR_SUCCESS || REG_VALUE_TYPE(kind) != REG_SZ {
                continue;
            }
            let chars = &data[..data_len as usize / 2];
            let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
            let path = PathBuf::from(String::from_utf16_lossy(&chars[..end]));
            let path = match &folder {
                Some(folder) if path.is_relative() => folder.join(path),
                _ => path,
            };
            if path.is_file() {
                files.push(path);
            }
        }
        let _ = RegCloseKey(key);
    }
    files
}

#[cfg(test)]
pub use fake::*;
#[cfg(not(test))]
pub use native::*;

/// Wraps an HTML fragment in the Windows "HTML Format" clipboard header,
/// whose offsets count UTF-8 bytes from the start of the data.
pub fn cf_html(fragment: &str) -> String {
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\n\
         StartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let prefix = "<html><head><meta charset=\"utf-8\"></head><body>\r\n<!--StartFragment-->";
    let suffix = "<!--EndFragment-->\r\n</body></html>";
    let start_html = HEADER.len();
    let start_fragment = start_html + prefix.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + suffix.len();
    format!(
        "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
         StartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n\
         {prefix}{fragment}{suffix}"
    )
}

pub fn read_clipboard_text() -> Option<String> {
    match read_clipboard()? {
        ClipboardContent::Text(text) => Some(text),
        ClipboardContent::Paths(paths) => Some(
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\r\n"),
        ),
    }
}

#[cfg(not(test))]
mod native {
    use super::{ClipboardContent, PathPrompt};
    use std::path::{Path, PathBuf};
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR, w};

    fn open_clipboard() -> bool {
        for _ in 0..10 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        false
    }

    pub fn write_clipboard(text: &str) {
        write_clipboard_formats(text, None);
    }

    /// Copies `html` as rich HTML, for pasting a table into a spreadsheet or
    /// an email, with `text` for programs that take only plain text.
    pub fn write_clipboard_html(html: &str, text: &str) {
        write_clipboard_formats(text, Some(html));
    }

    /// Copies `bytes` into a movable global block and hands it to the
    /// clipboard, which owns it from then on.
    unsafe fn set_clipboard_bytes(format: u32, bytes: &[u8]) {
        unsafe {
            if let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) {
                let target = GlobalLock(memory) as *mut u8;
                if !target.is_null() {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                    let _ = GlobalUnlock(memory);
                    let _ = SetClipboardData(format, Some(HANDLE(memory.0)));
                }
            }
        }
    }

    fn write_clipboard_formats(text: &str, html: Option<&str>) {
        if !open_clipboard() {
            return;
        }
        unsafe {
            let _ = EmptyClipboard();
            let wide = text
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<u8>>();
            set_clipboard_bytes(CF_UNICODETEXT.0 as u32, &wide);
            if let Some(html) = html {
                let format = RegisterClipboardFormatW(w!("HTML Format"));
                if format != 0 {
                    let mut bytes = super::cf_html(html).into_bytes();
                    bytes.push(0);
                    set_clipboard_bytes(format, &bytes);
                }
            }
            let _ = CloseClipboard();
        }
    }

    pub fn read_clipboard() -> Option<ClipboardContent> {
        if !open_clipboard() {
            return None;
        }
        unsafe {
            let mut content = None;
            if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok()
                && let Ok(handle) = GetClipboardData(CF_HDROP.0 as u32)
            {
                let drop = HDROP(handle.0);
                let count = DragQueryFileW(drop, u32::MAX, None);
                let mut paths = Vec::new();
                for index in 0..count {
                    let length = DragQueryFileW(drop, index, None);
                    let mut buffer = vec![0u16; length as usize + 1];
                    DragQueryFileW(drop, index, Some(&mut buffer));
                    paths.push(PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize])));
                }
                if !paths.is_empty() {
                    content = Some(ClipboardContent::Paths(paths));
                }
            }
            if content.is_none()
                && IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok()
                && let Ok(handle) = GetClipboardData(CF_UNICODETEXT.0 as u32)
            {
                let memory = HGLOBAL(handle.0);
                let source = GlobalLock(memory) as *const u16;
                if !source.is_null() {
                    let mut length = 0;
                    while *source.add(length) != 0 {
                        length += 1;
                    }
                    let text = String::from_utf16_lossy(std::slice::from_raw_parts(source, length));
                    let _ = GlobalUnlock(memory);
                    content = Some(ClipboardContent::Text(text));
                }
            }
            let _ = CloseClipboard();
            content
        }
    }

    fn shell_open(target: &HSTRING) {
        unsafe {
            ShellExecuteW(None, w!("open"), target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }

    /// Opens `url` in the default browser.
    pub fn open_url(url: &str) {
        shell_open(&HSTRING::from(url));
    }

    /// Opens a file or folder with its default program, such as File Explorer.
    pub fn open_path(path: &Path) {
        shell_open(&HSTRING::from(path.as_os_str()));
    }

    fn shell_item_path(item: &IShellItem) -> Option<PathBuf> {
        unsafe {
            let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as _));
            path.map(PathBuf::from)
        }
    }

    /// Asks for files or a folder to open. Blocks until the dialog closes;
    /// `None` when it was cancelled.
    pub fn prompt_for_paths(owner: Option<HWND>, prompt: PathPrompt) -> Result<Option<Vec<PathBuf>>, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| error.to_string())?;
            let mut options = dialog.GetOptions().map_err(|error| error.to_string())?;
            options |= FOS_FORCEFILESYSTEM;
            if prompt.directories {
                options |= FOS_PICKFOLDERS;
            }
            if prompt.multiple {
                options |= FOS_ALLOWMULTISELECT;
            }
            dialog.SetOptions(options).map_err(|error| error.to_string())?;
            let _ = dialog.SetTitle(&HSTRING::from(prompt.title));
            if dialog.Show(owner).is_err() {
                return Ok(None);
            }
            let results = dialog.GetResults().map_err(|error| error.to_string())?;
            let count = results.GetCount().map_err(|error| error.to_string())?;
            let mut paths = Vec::new();
            for index in 0..count {
                if let Ok(item) = results.GetItemAt(index)
                    && let Some(path) = shell_item_path(&item)
                {
                    paths.push(path);
                }
            }
            Ok(Some(paths))
        }
    }

    /// Asks where to save a file, starting in `directory` with `suggested`.
    pub fn prompt_for_new_path(owner: Option<HWND>, directory: &Path, suggested: &str) -> Result<Option<PathBuf>, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| error.to_string())?;
            let options = dialog.GetOptions().map_err(|error| error.to_string())?;
            dialog
                .SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)
                .map_err(|error| error.to_string())?;
            if let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(directory.as_os_str()), None)
            {
                let _ = dialog.SetFolder(&folder);
            }
            let _ = dialog.SetFileName(&HSTRING::from(suggested));
            if dialog.Show(owner).is_err() {
                return Ok(None);
            }
            let item = dialog.GetResult().map_err(|error| error.to_string())?;
            Ok(shell_item_path(&item))
        }
    }

    /// Asks for a color with the Windows color dialog, starting at `initial`
    /// (`0xRRGGBB`). Returns the chosen `0xRRGGBB`.
    pub fn choose_color(owner: Option<HWND>, initial: u32) -> Option<u32> {
        use windows::Win32::UI::Controls::Dialogs::*;
        thread_local! {
            static CUSTOM: std::cell::RefCell<[COLORREF; 16]> = std::cell::RefCell::new([COLORREF(0x00ff_ffff); 16]);
        }
        let to_colorref = |rgb: u32| COLORREF(((rgb & 0xff) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 0xff));
        CUSTOM.with(|custom| {
            let mut custom = custom.borrow_mut();
            let mut choose = CHOOSECOLORW {
                lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
                hwndOwner: owner.unwrap_or_default(),
                rgbResult: to_colorref(initial),
                lpCustColors: custom.as_mut_ptr(),
                Flags: CC_RGBINIT | CC_FULLOPEN,
                ..Default::default()
            };
            if !unsafe { ChooseColorW(&mut choose) }.as_bool() {
                return None;
            }
            let value = choose.rgbResult.0;
            Some(((value & 0xff) << 16) | (value & 0xff00) | ((value >> 16) & 0xff))
        })
    }
}

/// Stand-ins for tests, so nothing reaches the desktop. Tests queue dialog
/// answers and read back what was copied or opened.
#[cfg(test)]
pub mod fake {
    use super::{ClipboardContent, PathPrompt};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use windows::Win32::Foundation::HWND;

    #[derive(Default)]
    pub struct FakeSystem {
        pub clipboard: Option<ClipboardContent>,
        /// The rich HTML copied alongside the text, if any.
        pub clipboard_html: Option<String>,
        pub opened_urls: Vec<String>,
        pub opened_paths: Vec<PathBuf>,
        /// How many times each dialog was shown.
        pub path_prompts: usize,
        pub new_path_prompts: usize,
        /// Answers for the next dialogs; an empty queue cancels.
        pub path_answers: VecDeque<Option<Vec<PathBuf>>>,
        pub new_path_answers: VecDeque<Option<PathBuf>>,
        pub color_answers: VecDeque<Option<u32>>,
    }

    thread_local! {
        static SYSTEM: RefCell<FakeSystem> = RefCell::new(FakeSystem::default());
    }

    pub fn with<R>(f: impl FnOnce(&mut FakeSystem) -> R) -> R {
        SYSTEM.with(|system| f(&mut system.borrow_mut()))
    }

    pub fn write_clipboard(text: &str) {
        with(|system| {
            system.clipboard = Some(ClipboardContent::Text(text.to_owned()));
            system.clipboard_html = None;
        });
    }

    pub fn write_clipboard_html(html: &str, text: &str) {
        with(|system| {
            system.clipboard = Some(ClipboardContent::Text(text.to_owned()));
            system.clipboard_html = Some(html.to_owned());
        });
    }

    pub fn read_clipboard() -> Option<ClipboardContent> {
        with(|system| system.clipboard.clone())
    }

    pub fn open_url(url: &str) {
        with(|system| system.opened_urls.push(url.to_owned()));
    }

    pub fn open_path(path: &Path) {
        with(|system| system.opened_paths.push(path.to_path_buf()));
    }

    pub fn prompt_for_paths(_owner: Option<HWND>, _prompt: PathPrompt) -> Result<Option<Vec<PathBuf>>, String> {
        Ok(with(|system| {
            system.path_prompts += 1;
            system.path_answers.pop_front().flatten()
        }))
    }

    pub fn prompt_for_new_path(
        _owner: Option<HWND>,
        _directory: &Path,
        _suggested: &str,
    ) -> Result<Option<PathBuf>, String> {
        Ok(with(|system| {
            system.new_path_prompts += 1;
            system.new_path_answers.pop_front().flatten()
        }))
    }

    pub fn choose_color(_owner: Option<HWND>, _initial: u32) -> Option<u32> {
        with(|system| system.color_answers.pop_front().flatten())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn cf_html_offsets_point_at_the_document_and_fragment() {
        let data = super::cf_html("<table>é</table>");
        let offset = |name: &str| -> usize {
            let start = data.find(&format!("{name}:")).unwrap() + name.len() + 1;
            data[start..start + 10].parse().unwrap()
        };
        assert_eq!(&data[offset("StartFragment")..offset("EndFragment")], "<table>é</table>");
        assert!(data[offset("StartHTML")..].starts_with("<html>"));
        assert_eq!(offset("EndHTML"), data.len());
    }
}
