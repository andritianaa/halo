//! M2 — System monitors: CPU, RAM, battery, volume, Wi-Fi, Bluetooth, clock.
//! Sampling runs on a worker thread (1 s fast probes, 5 s radio probes).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Datelike;
use halo_core::module::{Category, Manifest, Module, ModuleContext};
use halo_platform::sys::{self, Battery, Bluetooth, Volume, Wifi};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "system",
    cdc: "M2",
    name_key: "module.system",
    category: Category::Monitoring,
    icon: "activity",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub cpu: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub battery: Battery,
    pub volume: Option<Volume>,
    pub wifi: Option<Wifi>,
    pub bluetooth: Bluetooth,
    pub clock: String,
    /// Local date parts (UI formats them in the user's language).
    pub weekday: u8,
    pub day: u8,
    pub month: u8,
    /// Incremented on every sample — lets the UI skip re-renders when unchanged.
    pub seq: u64,
}

impl Snapshot {
    /// Localized short date, e.g. `mer. 18 nov.` / `Wed 18 Nov` (chrono locale code like `fr_FR`).
    pub fn date_label(&self, locale: &str) -> String {
        if self.day == 0 {
            return String::new();
        }
        let Some(d) = chrono::NaiveDate::from_ymd_opt(chrono::Local::now().year(), self.month as u32, self.day as u32) else {
            return String::new();
        };
        let loc = chrono::Locale::try_from(locale).unwrap_or(chrono::Locale::en_US);
        d.format_localized("%a %-d %b", loc).to_string()
    }

    pub fn ram_percent(&self) -> f32 {
        if self.ram_total == 0 { 0.0 } else { self.ram_used as f32 * 100.0 / self.ram_total as f32 }
    }
}

/// Cheap, cloneable read access for the UI.
#[derive(Clone, Default)]
pub struct SystemHandle(Arc<Mutex<Snapshot>>);

impl SystemHandle {
    pub fn get(&self) -> Snapshot {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Set the master volume and reflect it immediately.
    pub fn set_volume(&self, level: f32) {
        if sys::set_volume(level) {
            if let Ok(mut s) = self.0.lock() {
                if let Some(v) = s.volume.as_mut() {
                    v.level = level.clamp(0.0, 1.0);
                }
                s.seq += 1;
            }
        }
    }

    pub fn toggle_mute(&self) {
        let muted = self.get().volume.map(|v| v.muted).unwrap_or(false);
        if sys::set_mute(!muted) {
            if let Ok(mut s) = self.0.lock() {
                if let Some(v) = s.volume.as_mut() {
                    v.muted = !muted;
                }
                s.seq += 1;
            }
        }
    }
}

pub struct SystemMonitor {
    handle: SystemHandle,
    worker: Option<Worker>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        Self { handle: SystemHandle::default(), worker: None }
    }

    pub fn handle(&self) -> SystemHandle {
        self.handle.clone()
    }
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for SystemMonitor {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let shared = self.handle.0.clone();
        let mut sys = sysinfo::System::new().map_err(|e| anyhow::anyhow!("sysinfo: {e}"))?;
        let mut last_radio: Option<Instant> = None;
        let (mut wifi, mut bt) = (None, Bluetooth::default());
        self.worker = Some(Worker::every("halo-system", Duration::from_secs(1), move || {
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            if last_radio.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                wifi = sys::wifi();
                bt = sys::bluetooth();
                last_radio = Some(Instant::now());
            }
            let now = chrono::Local::now();
            let snap = Snapshot {
                cpu: sys.global_cpu_usage(),
                ram_used: sys.used_memory(),
                ram_total: sys.total_memory(),
                battery: sys::battery(),
                volume: sys::volume(),
                wifi: wifi.clone(),
                bluetooth: bt.clone(),
                clock: now.format("%H:%M").to_string(),
                weekday: now.weekday().num_days_from_monday() as u8,
                day: now.day() as u8,
                month: now.month() as u8,
                seq: 0,
            };
            if let Ok(mut s) = shared.lock() {
                let seq = s.seq + 1;
                *s = Snapshot { seq, ..snap };
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }
}

/// Human-readable bytes (binary units, 1 decimal): "9.2 GB".
pub fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo_core::bus::Bus;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(9_878_000_000), "9.2 GB");
    }

    #[test]
    fn samples_real_machine() {
        let mut m = SystemMonitor::new();
        let h = m.handle();
        m.start(&ModuleContext { bus: Bus::new() }).unwrap();
        let t = Instant::now();
        while h.get().seq == 0 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        m.stop();
        let s = h.get();
        assert!(s.seq > 0, "no sample");
        assert!(s.ram_total > 0 && s.ram_used <= s.ram_total);
        assert_eq!(s.clock.len(), 5);
    }
}
