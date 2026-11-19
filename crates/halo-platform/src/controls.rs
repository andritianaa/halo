//! Quick controls for the notch: built-in display brightness (WMI) and the Wi-Fi /
//! Bluetooth radios (WinRT `Windows.Devices.Radios`).
//!
//! WMI calls take a few milliseconds, so brightness lives on its own thread: it polls
//! the current level every 250 ms (to notice changes made with the keyboard keys) and
//! applies the levels the UI asks for. External monitors (DDC/CI) are not covered —
//! on a desktop PC `level()` stays `None`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Arc;
use std::time::Duration;

use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{VARIANT, VariantToString, VariantToUInt32};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
    WbemLocator,
};
use windows::core::{BSTR, HSTRING, w};

const UNKNOWN: u32 = u32::MAX;

/// Built-in panel brightness, 0–100. Cheap to clone; the worker thread does the WMI work.
#[derive(Clone)]
pub struct Brightness {
    level: Arc<AtomicU32>,
    tx: Sender<u8>,
}

impl Brightness {
    /// Start the watcher thread. On machines without a WMI-controllable panel the
    /// thread exits and [`Brightness::level`] keeps returning `None`.
    pub fn spawn() -> Self {
        let level = Arc::new(AtomicU32::new(UNKNOWN));
        let (tx, rx) = channel();
        let l2 = level.clone();
        let _ = std::thread::Builder::new().name("halo-brightness".into()).spawn(move || {
            if let Err(e) = run(&l2, rx) {
                log::info!("brightness control unavailable: {e}");
            }
        });
        Self { level, tx }
    }

    pub fn level(&self) -> Option<u8> {
        let v = self.level.load(Ordering::Relaxed);
        (v != UNKNOWN).then_some(v as u8)
    }

    /// Ask for a new level (0–100); applied asynchronously.
    pub fn set(&self, percent: u8) {
        let p = percent.min(100);
        if self.level().is_some() {
            self.level.store(p as u32, Ordering::Relaxed);
        }
        let _ = self.tx.send(p);
    }
}

fn run(level: &AtomicU32, rx: Receiver<u8>) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let svc = locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)?;
        CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE)?;
        // First read decides whether the machine supports it at all.
        let first = query_brightness(&svc)?;
        level.store(first as u32, Ordering::Relaxed);
        loop {
            // Coalesce bursts of requests (slider drags) into the latest value.
            let mut want = None;
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(v) => {
                    want = Some(v);
                    while let Ok(v) = rx.try_recv() {
                        want = Some(v);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
            if let Some(v) = want {
                if let Err(e) = set_brightness(&svc, v) {
                    log::warn!("set brightness: {e}");
                }
            }
            if let Ok(v) = query_brightness(&svc) {
                level.store(v as u32, Ordering::Relaxed);
            }
        }
    }
}

unsafe fn first_object(svc: &IWbemServices, wql: &str) -> windows::core::Result<IWbemClassObject> {
    unsafe {
        let en = svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(wql), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)?;
        let mut objs = [None];
        let mut n = 0u32;
        en.Next(WBEM_INFINITE, &mut objs, &mut n).ok()?;
        objs[0].take().ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))
    }
}

unsafe fn query_brightness(svc: &IWbemServices) -> windows::core::Result<u8> {
    unsafe {
        let obj = first_object(svc, "SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE")?;
        let mut v = VARIANT::default();
        obj.Get(w!("CurrentBrightness"), 0, &mut v, None, None)?;
        Ok(VariantToUInt32(&v)?.min(100) as u8)
    }
}

unsafe fn set_brightness(svc: &IWbemServices, percent: u8) -> windows::core::Result<()> {
    unsafe {
        let inst = first_object(svc, "SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE")?;
        let mut path = VARIANT::default();
        inst.Get(w!("__PATH"), 0, &mut path, None, None)?;
        let mut buf = [0u16; 512];
        VariantToString(&path, &mut buf)?;
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let path = BSTR::from_wide(&buf[..len]);

        let mut class = None;
        svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), Default::default(), None, Some(&mut class), None)?;
        let class = class.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        let mut in_def = None;
        class.GetMethod(w!("WmiSetBrightness"), 0, &mut in_def, std::ptr::null_mut())?;
        let in_def = in_def.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        let params = in_def.SpawnInstance(0)?;
        // WMI marshals CIM uint32 as VT_I4 (VT_UI4 → WBEM_E_TYPE_MISMATCH).
        let timeout = VARIANT::from(0i32);
        let bright = VARIANT::from(percent);
        params.Put(w!("Timeout"), 0, &timeout, 0)?;
        params.Put(w!("Brightness"), 0, &bright, 0)?;
        svc.ExecMethod(&path, &BSTR::from("WmiSetBrightness"), Default::default(), None, &params, None, None)?;
        Ok(())
    }
}

/// Wi-Fi or Bluetooth radio switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioType {
    Wifi,
    Bluetooth,
}

fn find_radio(kind: RadioType) -> windows::core::Result<Option<Radio>> {
    let want = match kind {
        RadioType::Wifi => RadioKind::WiFi,
        RadioType::Bluetooth => RadioKind::Bluetooth,
    };
    let radios = Radio::GetRadiosAsync()?.get()?;
    for r in radios {
        if r.Kind()? == want {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// Is the radio on? `None` when there is no such radio. Blocking (WinRT): call it
/// from a worker thread.
pub fn radio_on(kind: RadioType) -> Option<bool> {
    find_radio(kind).ok().flatten().and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
}

/// Switch a radio on or off (what the quick settings toggles do). Blocking.
pub fn set_radio(kind: RadioType, on: bool) -> bool {
    let run = || -> windows::core::Result<bool> {
        let _ = Radio::RequestAccessAsync()?.get()?;
        let Some(r) = find_radio(kind)? else { return Ok(false) };
        let state = if on { RadioState::On } else { RadioState::Off };
        Ok(r.SetStateAsync(state)?.get()? == windows::Devices::Radios::RadioAccessStatus::Allowed)
    };
    run().unwrap_or_else(|e| {
        log::warn!("radio {kind:?}: {e}");
        false
    })
}

/// Open a Settings page (`ms-settings:network-wifi`, …).
pub fn open_settings(uri: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let uri = HSTRING::from(uri);
    unsafe {
        ShellExecuteW(None, w!("open"), &uri, None, None, SW_SHOWNORMAL);
    }
}
