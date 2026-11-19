//! Processes, windows and user idle time (M3, M4, M5, M7).

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};
use windows::core::PWSTR;

/// Full image path of a process (None for protected/system processes).
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Executable file name of a process, e.g. `Code.exe`.
pub fn process_exe(pid: u32) -> Option<String> {
    process_path(pid).and_then(|p| p.rsplit(['\\', '/']).next().map(str::to_string))
}

pub fn window_pid(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    pid
}

pub fn window_title(hwnd: isize) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(HWND(hwnd as *mut _), &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow().0 as isize }
}

/// Seconds since the last keyboard/mouse input (any app). Used to pause tracking.
pub fn idle_seconds() -> u32 {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if !GetLastInputInfo(&mut info).as_bool() {
            return 0;
        }
        GetTickCount().wrapping_sub(info.dwTime) / 1000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_is_resolved() {
        let exe = process_exe(std::process::id()).unwrap();
        assert!(exe.to_lowercase().ends_with(".exe"), "{exe}");
        let _ = idle_seconds();
        let fg = foreground_window();
        if fg != 0 {
            let _ = window_title(fg);
            assert!(window_pid(fg) > 0);
        }
    }
}
