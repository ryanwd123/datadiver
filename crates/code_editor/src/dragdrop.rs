//! OLE drop target: files dragged onto the editor are reported to the host.

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::HDROP;
use windows::core::*;

use crate::platform;

/// Posted to the editor window with the dropped paths boxed in lparam.
pub const WM_APP_FILES: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 0x101;

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: HWND,
}

impl DropTarget {
    pub fn new(hwnd: HWND) -> Self {
        DropTarget { hwnd }
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn set_effect(effect: *mut DROPEFFECT, ok: bool) {
    if !effect.is_null() {
        unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
    }
}

fn has_files(data: &Ref<IDataObject>) -> bool {
    data.as_ref()
        .is_some_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK)
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        set_effect(effect, has_files(&data));
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        // The effect from DragEnter stands; OLE passes in the allowed effects.
        if !effect.is_null() {
            unsafe { *effect &= DROPEFFECT_COPY };
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data
            .as_ref()
            .and_then(|d| unsafe { d.GetData(&hdrop_format()) }.ok())
            .map(|mut m| unsafe {
                let files = platform::hdrop_paths(HDROP(m.u.hGlobal.0));
                ReleaseStgMedium(&mut m);
                files
            })
            .unwrap_or_default();
        set_effect(effect, !files.is_empty());
        if !files.is_empty() {
            // Reported from the message loop, outside OLE's drop call.
            let boxed = Box::into_raw(Box::new(files));
            unsafe {
                if windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(self.hwnd),
                    WM_APP_FILES,
                    WPARAM(0),
                    LPARAM(boxed as isize),
                )
                .is_err()
                {
                    drop(Box::from_raw(boxed));
                }
            }
        }
        Ok(())
    }
}
