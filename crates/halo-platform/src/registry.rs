//! HKCU registry helpers (M17 theme, M18 autostart). Key paths are parameters so
//! tests can run against a scratch key instead of touching real settings.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_DWORD,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

fn open(subkey: &str) -> Option<HKEY> {
    let mut key = HKEY::default();
    let rc = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    (rc == ERROR_SUCCESS).then_some(key)
}

pub fn set_string(subkey: &str, name: &str, value: &str) -> bool {
    let Some(key) = open(subkey) else { return false };
    let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2) };
    let rc = unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_SZ, Some(bytes)) };
    let _ = unsafe { RegCloseKey(key) };
    rc == ERROR_SUCCESS
}

pub fn set_dword(subkey: &str, name: &str, value: u32) -> bool {
    let Some(key) = open(subkey) else { return false };
    let rc = unsafe { RegSetValueExW(key, &HSTRING::from(name), None, REG_DWORD, Some(&value.to_le_bytes())) };
    let _ = unsafe { RegCloseKey(key) };
    rc == ERROR_SUCCESS
}

pub fn get_string(subkey: &str, name: &str) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn get_dword(subkey: &str, name: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(subkey),
            &HSTRING::from(name),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (rc == ERROR_SUCCESS).then_some(v)
}

pub fn delete_value(subkey: &str, name: &str) -> bool {
    let Some(key) = open(subkey) else { return false };
    let rc = unsafe { RegDeleteValueW(key, &HSTRING::from(name)) };
    let _ = unsafe { RegCloseKey(key) };
    rc == ERROR_SUCCESS
}

/// Delete a whole subkey tree (tests clean up their scratch key with this).
pub fn delete_tree(subkey: &str) -> bool {
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(subkey)) == ERROR_SUCCESS }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_on_scratch_key() {
        let key = format!("Software\\Halo\\Test-{}", std::process::id());
        assert!(set_string(&key, "s", "héllo"));
        assert!(set_dword(&key, "d", 42));
        assert_eq!(get_string(&key, "s").as_deref(), Some("héllo"));
        assert_eq!(get_dword(&key, "d"), Some(42));
        assert!(delete_value(&key, "s"));
        assert_eq!(get_string(&key, "s"), None);
        assert!(delete_tree(&key));
    }
}

/// Windows build number (22000+ = Windows 11), from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\CurrentBuildNumber`.
pub fn windows_build() -> u32 {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let mut buf = vec![0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            &HSTRING::from("CurrentBuildNumber"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if rc != ERROR_SUCCESS {
        return 0;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n]).trim().parse().unwrap_or(0)
}

/// Mica window backdrops exist from Windows 11 (build 22000).
pub fn supports_mica() -> bool {
    windows_build() >= 22_000
}
