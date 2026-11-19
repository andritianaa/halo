//! Per-process GPU usage via PDH "GPU Engine" counters (same source as Task Manager).
//! Instance names look like `pid_1234_luid_0x..._phys_0_eng_0_engtype_3D`.

use std::collections::HashMap;

use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

pub struct GpuCounters {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

// PDH handles are plain kernel handles; the struct is only used from one worker thread.
unsafe impl Send for GpuCounters {}

impl GpuCounters {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            let _ = PdhCollectQueryData(query); // prime: rates need two samples
            Some(Self { query, counter })
        }
    }

    /// Sample now. Returns (per-pid GPU %, total GPU %), Task-Manager style:
    /// per engine type sum per pid, then max across engine types.
    pub fn sample(&mut self) -> (HashMap<u32, f32>, f32) {
        let mut per_pid_type: HashMap<(u32, String), f32> = HashMap::new();
        let mut per_type: HashMap<String, f32> = HashMap::new();
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return (HashMap::new(), 0.0);
            }
            let (mut size, mut count) = (0u32, 0u32);
            let rc = PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if rc != PDH_MORE_DATA as u32 || size == 0 {
                return (HashMap::new(), 0.0);
            }
            let mut buf = vec![0u8; size as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                return (HashMap::new(), 0.0);
            }
            for it in std::slice::from_raw_parts(items, count as usize) {
                let name = it.szName.to_string().unwrap_or_default();
                let v = it.FmtValue.Anonymous.doubleValue as f32;
                let Some((pid, ty)) = parse(&name) else { continue };
                *per_pid_type.entry((pid, ty.clone())).or_default() += v;
                *per_type.entry(ty).or_default() += v;
            }
        }
        let mut per_pid: HashMap<u32, f32> = HashMap::new();
        for ((pid, _), v) in per_pid_type {
            let e = per_pid.entry(pid).or_default();
            *e = e.max(v.min(100.0));
        }
        let total = per_type.values().copied().fold(0.0f32, f32::max).min(100.0);
        (per_pid, total)
    }
}

impl Drop for GpuCounters {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

/// `pid_1234_luid_..._engtype_3D` → (1234, "3D")
pub fn parse(name: &str) -> Option<(u32, String)> {
    let pid = name.strip_prefix("pid_")?.split('_').next()?.parse().ok()?;
    let ty = name.rsplit("engtype_").next()?.to_string();
    Some((pid, ty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_instance_names() {
        assert_eq!(parse("pid_1234_luid_0x00000000_0x0000D1C5_phys_0_eng_0_engtype_3D"), Some((1234, "3D".into())));
        assert_eq!(parse("_Total"), None);
    }

    #[test]
    fn samples_without_error() {
        if let Some(mut g) = GpuCounters::new() {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let (_, total) = g.sample();
            assert!((0.0..=100.0).contains(&total));
        }
    }
}
