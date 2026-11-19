//! Clock indicator (part of M2, pulled into Phase 0 to give the pill real content).

use std::sync::{Arc, Mutex};

use halo_core::module::{Category, Manifest, Module, ModuleContext};

pub const MANIFEST: Manifest = Manifest {
    id: "clock",
    cdc: "M2",
    name_key: "module.clock",
    category: Category::Monitoring,
    icon: "clock",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

/// Shared, cheaply-cloned view of the current time label (`HH:MM`).
#[derive(Clone, Default)]
pub struct ClockReading(Arc<Mutex<String>>);

impl ClockReading {
    pub fn get(&self) -> String {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Clock {
    reading: ClockReading,
    running: bool,
}

impl Clock {
    pub fn reading(&self) -> ClockReading {
        self.reading.clone()
    }

    fn refresh(&self) {
        let now = chrono::Local::now().format("%H:%M").to_string();
        if let Ok(mut s) = self.reading.0.lock() {
            if *s != now {
                *s = now;
            }
        }
    }
}

impl Module for Clock {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        self.running = true;
        self.refresh();
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn tick(&mut self) {
        if self.running {
            self.refresh();
        }
    }
}

/// Current local time label, for callers that do not hold the module.
pub fn now_label() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo_core::bus::Bus;

    #[test]
    fn reading_is_hh_mm_after_start() {
        let mut c = Clock::default();
        c.start(&ModuleContext { bus: Bus::new() }).unwrap();
        let r = c.reading().get();
        assert_eq!(r.len(), 5);
        assert_eq!(&r[2..3], ":");
    }
}
