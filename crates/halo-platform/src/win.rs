//! Win32 implementation.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, WS_CAPTION, WS_EX_WINDOWEDGE, WS_POPUP, WS_SYSMENU, WS_THICKFRAME, GetCursorPos, GetWindowLongPtrW, GetWindowRect, HWND_TOPMOST, MONITORINFOF_PRIMARY,
    SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, w};

use super::{Backdrop, Monitor, Rect};

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut core::ffi::c_void)
}

/// All monitors with work area (physical px) and effective DPI scale.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(data.0 as *mut Vec<Monitor>);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hmon, &mut info).as_bool() {
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                let r = info.rcWork;
                out.push(Monitor {
                    work_area: Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
                    scale: dx as f32 / 96.0,
                    primary: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            }
        }
        BOOL(1)
    }
    let mut out: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut _ as isize));
    }
    // Stable order: primary first, then left-to-right.
    out.sort_by_key(|m| (!m.primary, m.work_area.x, m.work_area.y));
    out
}

/// Overlay window styles: frameless popup, topmost, no taskbar button, never steals
/// focus, and no DWM border/rounded corners/backdrop (the pill draws its own shape).
pub fn style_overlay_window(h: isize) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd(h), GWL_STYLE) as u32;
        let style = (style | WS_POPUP.0) & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0);
        SetWindowLongPtrW(hwnd(h), GWL_STYLE, style as isize);

        let ex = GetWindowLongPtrW(hwnd(h), GWL_EXSTYLE) as u32;
        let ex = (ex | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0)
            & !(WS_EX_APPWINDOW.0 | WS_EX_WINDOWEDGE.0);
        SetWindowLongPtrW(hwnd(h), GWL_EXSTYLE, ex as isize);

        // Windows 11: no accent border, square corners, no system backdrop.
        const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
        let border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(hwnd(h), DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd(h),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            size_of_val(&corner) as u32,
        );
        let backdrop = DWMSBT_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd(h),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            size_of_val(&backdrop) as u32,
        );
        let _ = SetWindowPos(
            hwnd(h),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
    }
}

/// Frameless focusable popup (command palette): no caption, no DWM border/corners,
/// topmost, no taskbar button — but it can take keyboard focus.
pub fn style_popup_window(h: isize) {
    unsafe {
        let style = GetWindowLongPtrW(hwnd(h), GWL_STYLE) as u32;
        let style = (style | WS_POPUP.0) & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0);
        SetWindowLongPtrW(hwnd(h), GWL_STYLE, style as isize);
        let ex = GetWindowLongPtrW(hwnd(h), GWL_EXSTYLE) as u32;
        let ex = (ex | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) & !(WS_EX_APPWINDOW.0 | WS_EX_WINDOWEDGE.0);
        SetWindowLongPtrW(hwnd(h), GWL_EXSTYLE, ex as isize);
        const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
        let border = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(hwnd(h), DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(hwnd(h), DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, size_of_val(&corner) as u32);
        let _ = SetWindowPos(hwnd(h), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED);
    }
}

/// Move/resize (physical px) and keep topmost without activating.
pub fn place_window(h: isize, r: Rect) {
    unsafe {
        let _ = SetWindowPos(hwnd(h), Some(HWND_TOPMOST), r.x, r.y, r.w, r.h, SWP_NOACTIVATE);
    }
}

pub fn window_rect(h: isize) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd(h), &mut r).ok()? };
    Some(Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top))
}

/// Experimental (spec M1 risk): mouse input passes through the whole window.
/// Toggle it from a cursor hit-test so only the pill itself is clickable.
pub fn set_click_through(h: isize, on: bool) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd(h), GWL_EXSTYLE) as u32;
        let bits = WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0;
        let new = if on { ex | bits } else { ex & !bits };
        if new != ex {
            SetWindowLongPtrW(hwnd(h), GWL_EXSTYLE, new as isize);
        }
    }
}

pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    unsafe { GetCursorPos(&mut p).ok()? };
    Some((p.x, p.y))
}

/// Tell running apps a setting changed (e.g. "ImmersiveColorSet" after a theme switch).
pub fn broadcast_setting_change(area: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
    let wide: Vec<u16> = area.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            windows::Win32::Foundation::WPARAM(0),
            windows::Win32::Foundation::LPARAM(wide.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
}

/// Set the desktop wallpaper (persisted for the user).
pub fn set_wallpaper(path: &std::path::Path) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
    };
    let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(wide.as_mut_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)
            .is_ok()
    }
}

/// Is the foreground window covering a whole monitor (video, game, presentation)?
/// The desktop and the shell (taskbar) never count.
pub fn foreground_is_fullscreen() -> bool {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetShellWindow, GetDesktopWindow};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == GetShellWindow() || fg == GetDesktopWindow() {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "WorkerW" | "Progman" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Is a virtual key currently held down (e.g. Esc = 0x1B)?
pub fn key_down(vk: i32) -> bool {
    (unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(vk) } as u16) & 0x8000 != 0
}

/// `HKCU\…\Themes\Personalize\AppsUseLightTheme` (missing → dark).
pub fn apps_use_light_theme() -> bool {
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    res.is_ok() && value == 1
}

/// DWM backdrop (Windows 11 22H2+; silently ignored on Windows 10).
pub fn set_backdrop(h: isize, backdrop: Backdrop) {
    let kind = match backdrop {
        Backdrop::None => DWMSBT_NONE,
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        Backdrop::Mica => DWMSBT_MAINWINDOW,
    };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd(h),
            DWMWA_SYSTEMBACKDROP_TYPE,
            &kind as *const _ as *const _,
            size_of_val(&kind) as u32,
        );
    }
}

/// Dark or light window chrome (title bar, Mica tint) independent of the system
/// setting, so the backdrop matches the app theme.
pub fn set_dark_chrome(h: isize, dark: bool) {
    use windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE;
    let v: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd(h), DWMWA_USE_IMMERSIVE_DARK_MODE, &v as *const _ as *const _, size_of_val(&v) as u32);
    }
}

/// User UI locale, e.g. "fr-FR".
pub fn user_locale() -> String {
    // Read from the registry to avoid pulling the Globalization feature.
    let mut buf = [0u16; 85];
    let mut size = (buf.len() * 2) as u32;
    let res = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Control Panel\\International"),
            w!("LocaleName"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if res.is_err() {
        return "fr-FR".into();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// The process runs with administrator rights (elevated token).
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some((&mut elev as *mut TOKEN_ELEVATION).cast()), size_of::<TOKEN_ELEVATION>() as u32, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok && elev.TokenIsElevated != 0
    }
}

/// Start this executable again with administrator rights ("runas": UAC prompt),
/// forwarding the command-line arguments. Returns false if it could not start
/// (prompt declined, policy).
pub fn relaunch_elevated() -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::HSTRING;
    let Ok(exe) = std::env::current_exe() else { return false };
    let args: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a })
        .collect();
    let (exe, params) = (HSTRING::from(exe.as_os_str()), HSTRING::from(args.join(" ")));
    // ShellExecute returns a value > 32 on success.
    let r = unsafe { ShellExecuteW(None, w!("runas"), &exe, &params, None, SW_SHOWNORMAL) };
    r.0 as isize > 32
}

/// Start `exe args` with the rights of the signed-in user (not elevated), from an
/// elevated process: the token of the shell (Explorer) is duplicated and used for the
/// new process. Returns the new process id.
pub fn spawn_unelevated(exe: &std::path::Path, args: &str) -> Option<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary,
    };
    use windows::Win32::System::Threading::{
        CREATE_PROCESS_LOGON_FLAGS, CreateProcessWithTokenW, OpenProcess, OpenProcessToken, PROCESS_INFORMATION,
        PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    use windows::core::{HSTRING, PWSTR};
    unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = Default::default();
        let ok = OpenProcessToken(proc, TOKEN_DUPLICATE, &mut token).is_ok();
        let _ = CloseHandle(proc);
        if !ok {
            return None;
        }
        let mut primary = Default::default();
        let access = TOKEN_ACCESS_MASK(TOKEN_QUERY.0 | TOKEN_ASSIGN_PRIMARY.0 | TOKEN_DUPLICATE.0 | TOKEN_ADJUST_DEFAULT.0 | TOKEN_ADJUST_SESSIONID.0);
        let dup = DuplicateTokenEx(token, access, None, SecurityImpersonation, TokenPrimary, &mut primary);
        let _ = CloseHandle(token);
        dup.ok()?;
        let mut cmd: Vec<u16> = format!("\"{}\" {args}", exe.display()).encode_utf16().chain(std::iter::once(0)).collect();
        let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let r = CreateProcessWithTokenW(
            primary,
            CREATE_PROCESS_LOGON_FLAGS(0),
            &HSTRING::from(exe.as_os_str()),
            Some(PWSTR(cmd.as_mut_ptr())),
            Default::default(),
            None,
            None,
            &si,
            &mut pi,
        );
        let _ = CloseHandle(primary);
        if let Err(e) = r {
            log::warn!("spawn_unelevated: {e}");
            return None;
        }
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        Some(pi.dwProcessId)
    }
}
