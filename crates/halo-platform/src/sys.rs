//! System indicators for M2: battery, master volume, Wi-Fi, Bluetooth.

use windows::Win32::Devices::Bluetooth::{
    BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS, BluetoothFindDeviceClose,
    BluetoothFindFirstDevice, BluetoothFindFirstRadio, BluetoothFindNextDevice, BluetoothFindRadioClose,
    BluetoothIsConnectable,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WlanQueryInterface, wlan_interface_state_connected, wlan_intf_opcode_current_connection,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub present: bool,
    pub percent: Option<u8>,
    pub charging: bool,
    pub on_ac: bool,
    pub seconds_left: Option<u32>,
}

pub fn battery() -> Battery {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return Battery::default();
    }
    let present = s.BatteryFlag != 128 && s.BatteryFlag != 255;
    Battery {
        present,
        percent: (present && s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent),
        charging: present && s.BatteryFlag & 8 != 0,
        on_ac: s.ACLineStatus == 1,
        seconds_left: (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime),
    }
}

fn com_init() {
    // Ignore RPC_E_CHANGED_MODE: the thread is already in an apartment and COM works.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

fn endpoint_volume() -> windows::core::Result<IAudioEndpointVolume> {
    com_init();
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Volume {
    /// 0.0 – 1.0
    pub level: f32,
    pub muted: bool,
}

pub fn volume() -> Option<Volume> {
    let v = endpoint_volume().ok()?;
    unsafe { Some(Volume { level: v.GetMasterVolumeLevelScalar().ok()?, muted: v.GetMute().ok()?.as_bool() }) }
}

thread_local! {
    /// Endpoint kept per thread and refreshed every few seconds (default device may change).
    static ENDPOINT: std::cell::RefCell<Option<(IAudioEndpointVolume, std::time::Instant)>> = const { std::cell::RefCell::new(None) };
}

/// Like [`volume`], but reuses the endpoint: cheap enough to poll every frame tick
/// (the notch's volume HUD).
pub fn volume_fast() -> Option<Volume> {
    ENDPOINT.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_none_or(|(_, at)| at.elapsed() > std::time::Duration::from_secs(3)) {
            *slot = endpoint_volume().ok().map(|e| (e, std::time::Instant::now()));
        }
        let (v, _) = slot.as_ref()?;
        let read = unsafe { (v.GetMasterVolumeLevelScalar(), v.GetMute()) };
        match read {
            (Ok(level), Ok(muted)) => Some(Volume { level, muted: muted.as_bool() }),
            _ => {
                *slot = None;
                None
            }
        }
    })
}

pub fn set_volume(level: f32) -> bool {
    endpoint_volume()
        .and_then(|v| unsafe { v.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) })
        .is_ok()
}

pub fn set_mute(muted: bool) -> bool {
    endpoint_volume().and_then(|v| unsafe { v.SetMute(muted, std::ptr::null()) }).is_ok()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wifi {
    pub connected: bool,
    /// May be empty on Windows 11 24H2+ without location permission.
    pub ssid: String,
    /// 0 – 100
    pub signal: u32,
    pub rx_mbps: u32,
}

pub fn wifi() -> Option<Wifi> {
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
            return None; // no WLAN service (desktop without Wi-Fi)
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut out = None;
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            out = Some(Wifi::default());
            for it in items {
                if it.isState != wlan_interface_state_connected {
                    continue;
                }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                let rc = WlanQueryInterface(
                    h,
                    &it.InterfaceGuid,
                    wlan_intf_opcode_current_connection,
                    None,
                    &mut size,
                    &mut data,
                    None,
                );
                if rc == 0 && !data.is_null() {
                    let attr = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let a = &attr.wlanAssociationAttributes;
                    let len = (a.dot11Ssid.uSSIDLength as usize).min(32);
                    out = Some(Wifi {
                        connected: true,
                        ssid: String::from_utf8_lossy(&a.dot11Ssid.ucSSID[..len]).into_owned(),
                        signal: a.wlanSignalQuality,
                        rx_mbps: a.ulRxRate / 1000,
                    });
                    WlanFreeMemory(data);
                    break;
                }
            }
            WlanFreeMemory(list as *const _);
        }
        WlanCloseHandle(h, None);
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bluetooth {
    pub radio: bool,
    pub enabled: bool,
    pub connected: Vec<String>,
}

pub fn bluetooth() -> Bluetooth {
    unsafe {
        let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
        let mut radio = HANDLE::default();
        let Ok(find) = BluetoothFindFirstRadio(&params, &mut radio) else { return Bluetooth::default() };
        let _ = BluetoothFindRadioClose(find);
        let enabled = BluetoothIsConnectable(Some(radio)).as_bool();
        let mut connected = Vec::new();
        let search = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: true.into(),
            fReturnRemembered: true.into(),
            fReturnConnected: true.into(),
            fReturnUnknown: false.into(),
            fIssueInquiry: false.into(),
            cTimeoutMultiplier: 0,
            hRadio: radio,
        };
        let mut info = BLUETOOTH_DEVICE_INFO { dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
        if let Ok(df) = BluetoothFindFirstDevice(&search, &mut info) {
            loop {
                if info.fConnected.as_bool() {
                    let n = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
                    connected.push(String::from_utf16_lossy(&info.szName[..n]));
                }
                info = BLUETOOTH_DEVICE_INFO { dwSize: size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
                if BluetoothFindNextDevice(df, &mut info).is_err() {
                    break;
                }
            }
            let _ = BluetoothFindDeviceClose(df);
        }
        let _ = CloseHandle(radio);
        Bluetooth { radio: true, enabled, connected }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only probes of the real machine; prints values, asserts only sanity.
    #[test]
    fn probes_do_not_fail() {
        let b = battery();
        if let Some(p) = b.percent {
            assert!(p <= 100);
        }
        if let Some(v) = volume() {
            assert!((0.0..=1.0).contains(&v.level));
        }
        let w = wifi();
        let bt = bluetooth();
        println!("battery={b:?}\nvolume={:?}\nwifi={w:?}\nbluetooth={bt:?}", volume());
    }
}

/// Saved Wi-Fi profiles as Windows stores them: (profile name, profile XML).
///
/// Asks for the plain-text key (`WLAN_PROFILE_GET_PLAINTEXT_KEY`); Windows only honours
/// it for an elevated process — otherwise the XML carries the key still encrypted
/// (`<protected>true</protected>`). Profiles are read from every Wi-Fi adapter and
/// de-duplicated by name.
pub fn wifi_profiles() -> Vec<(String, String)> {
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_PROFILE_GET_PLAINTEXT_KEY, WLAN_PROFILE_INFO_LIST, WlanGetProfile, WlanGetProfileList,
    };
    use windows::core::{PCWSTR, PWSTR};
    let mut out: Vec<(String, String)> = Vec::new();
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
            return out;
        }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let n = (*list).dwNumberOfItems as usize;
            let items = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), n);
            for it in items {
                let mut profiles: *mut WLAN_PROFILE_INFO_LIST = std::ptr::null_mut();
                if WlanGetProfileList(h, &it.InterfaceGuid, None, &mut profiles) != 0 || profiles.is_null() {
                    continue;
                }
                let pn = (*profiles).dwNumberOfItems as usize;
                for p in std::slice::from_raw_parts((*profiles).ProfileInfo.as_ptr(), pn) {
                    let len = p.strProfileName.iter().position(|c| *c == 0).unwrap_or(p.strProfileName.len());
                    let name = String::from_utf16_lossy(&p.strProfileName[..len]);
                    if out.iter().any(|(n, _)| *n == name) {
                        continue;
                    }
                    let mut xml = PWSTR::null();
                    let mut flags = WLAN_PROFILE_GET_PLAINTEXT_KEY;
                    if WlanGetProfile(h, &it.InterfaceGuid, PCWSTR(p.strProfileName.as_ptr()), None, &mut xml, Some(&mut flags), None) == 0
                        && !xml.is_null()
                    {
                        out.push((name, xml.to_string().unwrap_or_default()));
                        WlanFreeMemory(xml.0 as _);
                    }
                }
                WlanFreeMemory(profiles as _);
            }
            WlanFreeMemory(list as _);
        }
        WlanCloseHandle(h, None);
    }
    out
}
