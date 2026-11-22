//! Opening `.ddash` files with DataDiver from Explorer.
//!
//! The association is per user, under `HKCU\Software\Classes`, so it needs
//! no administrator rights and touches no other user. Windows lets an app
//! claim an extension no other app has claimed; `.ddash` is DataDiver's own.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::{HSTRING, PCWSTR};

const PROG_ID: &str = "DataDiver.Dashboard";

fn classes(key: &str) -> String {
    format!(r"Software\Classes\{key}")
}

/// The command Explorer runs to open a dashboard with `exe`.
pub fn open_command(exe: &Path) -> String {
    format!("\"{}\" \"%1\"", exe.display())
}

fn set_default(key: &str, value: &str) -> Result<(), String> {
    unsafe {
        let mut handle = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(classes(key)),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut handle,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(format!("cannot write HKCU\\{}: error {}", classes(key), status.0));
        }
        let wide = value.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let status = RegSetValueExW(handle, PCWSTR::null(), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(handle);
        if status != ERROR_SUCCESS {
            return Err(format!("cannot write HKCU\\{}: error {}", classes(key), status.0));
        }
        Ok(())
    }
}

fn read_default(key: &str) -> Option<String> {
    unsafe {
        let mut size = 0u32;
        let path = HSTRING::from(classes(key));
        if RegGetValueW(HKEY_CURRENT_USER, &path, PCWSTR::null(), RRF_RT_REG_SZ, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            &path,
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

fn notify_shell() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/// Makes Explorer open `.ddash` files with `exe`.
pub fn register(exe: &Path) -> Result<(), String> {
    let extension = format!(".{}", super::dashboard::EXTENSION);
    set_default(&extension, PROG_ID)?;
    set_default(PROG_ID, "DataDiver dashboard")?;
    set_default(&format!(r"{PROG_ID}\DefaultIcon"), &format!("\"{}\",0", exe.display()))?;
    set_default(&format!(r"{PROG_ID}\shell\open\command"), &open_command(exe))?;
    notify_shell();
    Ok(())
}

/// Removes what [`register`] wrote. `.ddash` is left alone if another app
/// has taken it over since.
pub fn unregister() -> Result<(), String> {
    let extension = format!(".{}", super::dashboard::EXTENSION);
    unsafe {
        let status = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(classes(PROG_ID)));
        if status != ERROR_SUCCESS && status.0 != 2 {
            return Err(format!("cannot remove HKCU\\{}: error {}", classes(PROG_ID), status.0));
        }
        if read_default(&extension).as_deref() == Some(PROG_ID) {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(classes(&extension)));
        }
    }
    notify_shell();
    Ok(())
}

/// The command `.ddash` files open with, if DataDiver registered them.
pub fn registered_command() -> Option<String> {
    let extension = format!(".{}", super::dashboard::EXTENSION);
    (read_default(&extension).as_deref() == Some(PROG_ID)).then(|| read_default(&format!(r"{PROG_ID}\shell\open\command")))?
}

/// The exe a registered open command runs: the quoted path before `"%1"`.
fn command_exe(command: &str) -> Option<PathBuf> {
    let rest = command.trim().strip_prefix('"')?;
    Some(PathBuf::from(&rest[..rest.find('"')?]))
}

/// After the exe moved, points the association at where it is now, so a
/// double-click still works. Only when the registered exe is gone: another
/// DataDiver that still exists, such as a release build next to a debug
/// build, keeps the association. Does nothing if `.ddash` was never registered.
pub fn follow_moved_exe(exe: &Path) -> Result<bool, String> {
    match registered_command() {
        Some(command) if command != open_command(exe) && command_exe(&command).is_none_or(|old| !old.exists()) => {
            register(exe).map(|()| true)
        }
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_exe_of_an_open_command() {
        let exe = Path::new(r"C:\Program Files\DataDiver\datadiver.exe");
        assert_eq!(command_exe(&open_command(exe)), Some(exe.to_path_buf()));
        assert_eq!(command_exe("notepad.exe %1"), None);
    }
}

pub fn current_exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| format!("cannot find DataDiver's own path: {error}"))
}
