//! Read-only probe: which network usage sources work without admin rights?
use windows::Foundation::DateTime;
use windows::Networking::Connectivity::{DataUsageGranularity, NetworkInformation, NetworkUsageStates, TriStates};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

fn filetime_now() -> i64 {
    // WinRT DateTime = 100 ns ticks since 1601-01-01.
    let unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    unix.as_secs() as i64 * 10_000_000 + 116_444_736_000_000_000
}

fn main() {
    // 1. Interface counters.
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_ok() {
            let tab = &*t;
            let rows = std::slice::from_raw_parts(tab.Table.as_ptr(), tab.NumEntries as usize);
            let (mut rin, mut rout) = (0u64, 0u64);
            for r in rows {
                if r.Type == 71 || r.Type == 6 {
                    rin += r.InOctets;
                    rout += r.OutOctets;
                }
            }
            println!("ifTable2: {} rows, wifi+ethernet in={rin} out={rout}", tab.NumEntries);
            FreeMibTable(t as _);
        }
    }
    // 2/3. WinRT usage history.
    let profile = match NetworkInformation::GetInternetConnectionProfile() {
        Ok(p) => p,
        Err(e) => return println!("no internet profile: {e}"),
    };
    println!("profile: {:?}", profile.ProfileName());
    let end = filetime_now();
    let start = end - 24 * 3600 * 10_000_000;
    let states = NetworkUsageStates { Roaming: TriStates::DoNotCare, Shared: TriStates::DoNotCare };
    match profile.GetNetworkUsageAsync(DateTime { UniversalTime: start }, DateTime { UniversalTime: end }, DataUsageGranularity::PerHour, states).and_then(|op| op.get()) {
        Ok(list) => {
            let n = list.Size().unwrap_or(0);
            let (mut s, mut r) = (0u64, 0u64);
            for u in &list {
                s += u.BytesSent().unwrap_or(0);
                r += u.BytesReceived().unwrap_or(0);
            }
            println!("usage 24h: {n} buckets, sent={s} received={r}");
        }
        Err(e) => println!("GetNetworkUsageAsync: {e}"),
    }
    match profile.GetAttributedNetworkUsageAsync(DateTime { UniversalTime: start }, DateTime { UniversalTime: end }, states).and_then(|op| op.get()) {
        Ok(list) => {
            println!("attributed apps (24h): {}", list.Size().unwrap_or(0));
            for u in list.into_iter().take(8) {
                println!("  {:?} / {:?}: rx={} tx={}", u.AttributionName().unwrap_or_default(), u.AttributionId().unwrap_or_default(), u.BytesReceived().unwrap_or(0), u.BytesSent().unwrap_or(0));
            }
        }
        Err(e) => println!("GetAttributedNetworkUsageAsync: {e}"),
    }
}
