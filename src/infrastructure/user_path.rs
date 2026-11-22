//! Putting DataDiver's folder on the user's PATH, so `datadiver` runs from
//! any terminal, for people and for agents such as Claude.
//!
//! The user's PATH is `Path` under `HKCU\Environment`: no administrator
//! rights, and only this user. Terminals read it when they start, so ones
//! already open keep the old PATH.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, WPARAM};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
use windows::core::{HSTRING, w};

const ENVIRONMENT: &str = "Environment";

/// The user's PATH as stored, unexpanded, and whether it uses `%VARIABLES%`.
fn read() -> Result<(String, bool), String> {
    unsafe {
        let mut kind = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        let status = RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(ENVIRONMENT), w!("Path"), flags, Some(&mut kind), None, Some(&mut size));
        if status.0 == 2 {
            // No user PATH yet.
            return Ok((String::new(), true));
        }
        if status != ERROR_SUCCESS {
            return Err(format!("cannot read your PATH: error {}", status.0));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(ENVIRONMENT),
            w!("Path"),
            flags,
            Some(&mut kind),
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if status != ERROR_SUCCESS {
            return Err(format!("cannot read your PATH: error {}", status.0));
        }
        let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Ok((String::from_utf16_lossy(&buffer[..length]), kind == REG_EXPAND_SZ))
    }
}

fn write(path: &str, expand: bool) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let status = RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(ENVIRONMENT), None, KEY_SET_VALUE, &mut key);
        if status != ERROR_SUCCESS {
            return Err(format!("cannot change your PATH: error {}", status.0));
        }
        let wide = path.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
        let status = RegSetValueExW(key, w!("Path"), None, if expand { REG_EXPAND_SZ } else { REG_SZ }, Some(bytes));
        let _ = RegCloseKey(key);
        if status != ERROR_SUCCESS {
            return Err(format!("cannot change your PATH: error {}", status.0));
        }
        // Tells Explorer, and the terminals it starts from now on, about the change.
        let mut result = 0usize;
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            Some(&mut result),
        );
        Ok(())
    }
}

/// Two PATH entries name the same folder: case and a trailing `\` don't matter.
fn same_folder(entry: &str, folder: &Path) -> bool {
    let normalize = |text: &str| text.trim().trim_end_matches(['\\', '/']).to_lowercase();
    normalize(entry) == normalize(&folder.to_string_lossy())
}

/// `path` with `folder` added at the end, or `None` when it is already there.
fn with_folder(path: &str, folder: &Path) -> Option<String> {
    if path.split(';').any(|entry| same_folder(entry, folder)) {
        return None;
    }
    let mut entries = path.split(';').filter(|entry| !entry.trim().is_empty()).map(str::to_owned).collect::<Vec<_>>();
    entries.push(folder.to_string_lossy().into_owned());
    Some(entries.join(";"))
}

/// `path` without `folder`, or `None` when it is not there.
fn without_folder(path: &str, folder: &Path) -> Option<String> {
    let kept = path.split(';').filter(|entry| !same_folder(entry, folder)).collect::<Vec<_>>();
    (kept.len() != path.split(';').count()).then(|| kept.join(";"))
}

/// Why `folder` should not go on PATH, if it shouldn't: every program in a
/// folder on PATH can then run by name, so it must hold DataDiver alone.
pub fn unsuitable_folder(folder: &Path, exe_name: &str) -> Option<String> {
    let others = std::fs::read_dir(folder)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            let lower = name.to_lowercase();
            [".exe", ".bat", ".cmd", ".com", ".ps1"].iter().any(|extension| lower.ends_with(extension))
                && !lower.eq_ignore_ascii_case(exe_name)
        })
        .collect::<Vec<_>>();
    if others.is_empty() {
        return None;
    }
    let shown = others.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    Some(format!(
        "{} holds other programs ({shown}{}), which would also run by name from any terminal. \
         Move datadiver.exe to a folder of its own, such as %LOCALAPPDATA%\\Programs\\DataDiver, and run this again from there.",
        folder.display(),
        if others.len() > 3 { ", …" } else { "" }
    ))
}

/// The folder of the running exe.
pub fn exe_folder() -> Result<(PathBuf, String), String> {
    let exe = super::file_association::current_exe()?;
    let name = exe.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = exe.parent().ok_or("DataDiver's folder is unknown")?.to_path_buf();
    Ok((folder, name))
}

/// Adds the running exe's folder to the user's PATH. Returns false when it
/// was already there.
pub fn add() -> Result<bool, String> {
    let (folder, name) = exe_folder()?;
    if let Some(reason) = unsuitable_folder(&folder, &name) {
        return Err(reason);
    }
    let (path, expand) = read()?;
    match with_folder(&path, &folder) {
        Some(path) => write(&path, expand).map(|()| true),
        None => Ok(false),
    }
}

/// Takes the running exe's folder off the user's PATH. Returns false when
/// it wasn't there.
pub fn remove() -> Result<bool, String> {
    let (folder, _) = exe_folder()?;
    let (path, expand) = read()?;
    match without_folder(&path, &folder) {
        Some(path) => write(&path, expand).map(|()| true),
        None => Ok(false),
    }
}

/// The message to show after [`add`] or [`remove`].
pub fn outcome_message(added: bool, changed: bool) -> String {
    let folder = exe_folder().map(|(folder, _)| folder.display().to_string()).unwrap_or_default();
    match (added, changed) {
        (true, true) => format!(
            "Added {folder} to your PATH. Terminals started from now on can run datadiver; restart ones already open, Claude Code included."
        ),
        (true, false) => format!("{folder} is already on your PATH"),
        (false, true) => format!("Removed {folder} from your PATH"),
        (false, false) => format!("{folder} was not on your PATH"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_a_folder_once_and_keeps_the_others() {
        let folder = Path::new(r"C:\Tools\DataDiver");
        assert_eq!(with_folder(r"C:\a;%USERPROFILE%\bin", folder).as_deref(), Some(r"C:\a;%USERPROFILE%\bin;C:\Tools\DataDiver"));
        assert_eq!(with_folder("", folder).as_deref(), Some(r"C:\Tools\DataDiver"));
        assert_eq!(with_folder(r"C:\a;;", folder).as_deref(), Some(r"C:\a;C:\Tools\DataDiver"));
        assert_eq!(with_folder(r"C:\a;c:\tools\datadiver\", folder), None);
    }

    #[test]
    fn removes_every_spelling_of_the_folder() {
        let folder = Path::new(r"C:\Tools\DataDiver");
        assert_eq!(without_folder(r"C:\a;C:\Tools\DataDiver;c:\tools\datadiver\;C:\b", folder).as_deref(), Some(r"C:\a;C:\b"));
        assert_eq!(without_folder(r"C:\a", folder), None);
    }

    #[test]
    fn a_folder_of_other_programs_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("datadiver.exe"), "").unwrap();
        std::fs::write(folder.path().join("data.csv"), "").unwrap();
        assert_eq!(unsuitable_folder(folder.path(), "datadiver.exe"), None);
        std::fs::write(folder.path().join("setup.exe"), "").unwrap();
        assert!(unsuitable_folder(folder.path(), "datadiver.exe").unwrap().contains("setup.exe"));
    }
}
