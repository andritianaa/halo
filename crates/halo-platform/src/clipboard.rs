//! Clipboard metadata for M7: who copied, and whether the source asked to keep the
//! content out of history (password managers set these formats).

use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::Foundation::HGLOBAL;
use windows::core::w;

/// True when the current clipboard content must not be recorded.
pub fn is_sensitive() -> bool {
    unsafe {
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        let ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
        if IsClipboardFormatAvailable(exclude).is_ok() || IsClipboardFormatAvailable(ignore).is_ok() {
            return true;
        }
        let can_history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if IsClipboardFormatAvailable(can_history).is_err() {
            return false;
        }
        // Present: sensitive only when its DWORD value is 0.
        if OpenClipboard(None).is_err() {
            return true; // be conservative
        }
        let mut sensitive = true;
        if let Ok(h) = GetClipboardData(can_history) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
            if !p.is_null() {
                sensitive = *p == 0;
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        sensitive
    }
}

/// Executable name of the clipboard owner window's process, if any.
pub fn owner_exe() -> Option<String> {
    let owner = unsafe { GetClipboardOwner() }.ok()?;
    if owner.is_invalid() {
        return None;
    }
    crate::proc::process_exe(crate::proc::window_pid(owner.0 as isize))
}
