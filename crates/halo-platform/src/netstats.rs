//! Internet usage figures Windows already keeps, readable without admin rights:
//! - interface byte counters (`GetIfTable2`) → live download/upload speed;
//! - the connection profile's usage history (WinRT `GetNetworkUsageAsync`) → per-hour /
//!   per-day totals, the data behind Settings › Network › Data usage;
//! - per-app attributed usage (`GetAttributedNetworkUsageAsync`).
//!
//! The WinRT calls block for a few milliseconds: call them from a worker thread.

use windows::Foundation::DateTime;
use windows::Networking::Connectivity::{DataUsageGranularity, NetworkInformation, NetworkUsageStates, TriStates};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

/// IANA ifType values of real uplinks: Ethernet, Wi-Fi, mobile broadband.
const UPLINK_TYPES: [u32; 4] = [6, 71, 243, 244];

/// Bytes received / sent by the physical uplinks since boot. `None` if unavailable.
pub fn interface_totals() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let t = &*table;
        let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
        let (mut rx, mut tx) = (0u64, 0u64);
        for r in rows {
            // Bit 0 of the flags = HardwareInterface (skips filter/virtual duplicates).
            let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 == 1;
            if hardware && UPLINK_TYPES.contains(&r.Type) && r.OperStatus == IfOperStatusUp {
                rx = rx.wrapping_add(r.InOctets);
                tx = tx.wrapping_add(r.OutOctets);
            }
        }
        FreeMibTable(table as _);
        Some((rx, tx))
    }
}

/// One history bucket.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageBucket {
    /// Unix seconds at the start of the bucket.
    pub start: i64,
    pub rx: u64,
    pub tx: u64,
}

/// Bytes used by one app over a period (id = exe path or package family name).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppUsage {
    pub id: String,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Hour,
    Day,
}

const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

fn to_winrt(unix: i64) -> DateTime {
    DateTime { UniversalTime: unix * 10_000_000 + UNIX_EPOCH_TICKS }
}

fn states() -> NetworkUsageStates {
    NetworkUsageStates { Roaming: TriStates::DoNotCare, Shared: TriStates::DoNotCare }
}

/// Usage of the current internet connection between two Unix times.
pub fn usage_history(start: i64, end: i64, g: Granularity) -> windows::core::Result<Vec<UsageBucket>> {
    let profile = NetworkInformation::GetInternetConnectionProfile()?;
    let gran = match g {
        Granularity::Hour => DataUsageGranularity::PerHour,
        Granularity::Day => DataUsageGranularity::PerDay,
    };
    let step = match g {
        Granularity::Hour => 3600,
        Granularity::Day => 86_400,
    };
    let list = profile.GetNetworkUsageAsync(to_winrt(start), to_winrt(end), gran, states())?.get()?;
    let mut out = Vec::new();
    for (i, u) in list.into_iter().enumerate() {
        out.push(UsageBucket { start: start + i as i64 * step, rx: u.BytesReceived()?, tx: u.BytesSent()? });
    }
    Ok(out)
}

/// Per-app usage of the current internet connection between two Unix times.
pub fn app_usage(start: i64, end: i64) -> windows::core::Result<Vec<AppUsage>> {
    let profile = NetworkInformation::GetInternetConnectionProfile()?;
    let list = profile.GetAttributedNetworkUsageAsync(to_winrt(start), to_winrt(end), states())?.get()?;
    let mut out = Vec::new();
    for u in list {
        out.push(AppUsage { id: u.AttributionId()?.to_string(), rx: u.BytesReceived()?, tx: u.BytesSent()? });
    }
    Ok(out)
}

/// Name of the current internet connection (Wi-Fi SSID, "Ethernet"…).
pub fn connection_name() -> Option<String> {
    NetworkInformation::GetInternetConnectionProfile().ok()?.ProfileName().ok().map(|s| s.to_string())
}
