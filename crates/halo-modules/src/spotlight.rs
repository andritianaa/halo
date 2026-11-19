//! M16 — Mouse spotlight: shake the mouse to locate the cursor.
//! The detector is pure (tested); the module polls the cursor at 60 Hz on a worker.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use halo_core::module::{Category, Manifest, Module, ModuleContext};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "spotlight",
    cdc: "M16",
    name_key: "module.spotlight",
    category: Category::Comfort,
    icon: "mouse-pointer-2",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

/// Detects a "shake": several fast horizontal direction reversals in a short window.
#[derive(Clone, Debug)]
pub struct ShakeDetector {
    /// Reversals needed within `window` (lower = more sensitive).
    pub reversals: usize,
    /// Minimum travel (px) of each stroke between reversals.
    pub min_stroke: i32,
    pub window: Duration,
    last: Option<(i32, i32)>,
    dir: i32,
    stroke: i32,
    turns: VecDeque<Instant>,
    cooldown_until: Option<Instant>,
}

impl ShakeDetector {
    pub fn new(sensitivity: u8, scale: f32) -> Self {
        // sensitivity 1 (hard) … 5 (easy)
        let s = sensitivity.clamp(1, 5) as usize;
        Self {
            reversals: 7 - s,
            min_stroke: (60.0 * scale) as i32,
            window: Duration::from_millis(700),
            last: None,
            dir: 0,
            stroke: 0,
            turns: VecDeque::new(),
            cooldown_until: None,
        }
    }

    /// Feed a cursor sample; returns true when a shake is detected.
    pub fn feed(&mut self, now: Instant, x: i32, y: i32) -> bool {
        let Some((lx, _ly)) = self.last.replace((x, y)) else { return false };
        let dx = x - lx;
        if dx == 0 {
            return false;
        }
        let d = dx.signum();
        if d == self.dir || self.dir == 0 {
            self.dir = d;
            self.stroke += dx.abs();
            return false;
        }
        // Direction reversal.
        let long_enough = self.stroke >= self.min_stroke;
        self.dir = d;
        self.stroke = dx.abs();
        if !long_enough {
            return false;
        }
        self.turns.push_back(now);
        while self.turns.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            self.turns.pop_front();
        }
        if self.cooldown_until.is_some_and(|t| now < t) {
            return false;
        }
        if self.turns.len() >= self.reversals {
            self.turns.clear();
            self.cooldown_until = Some(now + Duration::from_millis(1500));
            return true;
        }
        false
    }
}

#[derive(Clone, Debug, Default)]
pub struct SpotlightState {
    /// Set when a shake is detected; the UI shows the spotlight until it expires.
    pub active_until: Option<Instant>,
    pub cursor: (i32, i32),
}

#[derive(Clone, Default)]
pub struct SpotlightHandle(Arc<Mutex<SpotlightState>>);

impl SpotlightHandle {
    pub fn get(&self) -> SpotlightState {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Manual trigger (e.g. from the notch or a test).
    pub fn trigger(&self, cursor: (i32, i32)) {
        if let Ok(mut s) = self.0.lock() {
            s.active_until = Some(Instant::now() + SPOT_DURATION);
            s.cursor = cursor;
        }
    }
}

pub const SPOT_DURATION: Duration = Duration::from_millis(1600);
/// Disappearing animation (hole grows, dim fades), also used after a click.
pub const SPOT_CLOSE: Duration = Duration::from_millis(280);

impl SpotlightHandle {
    /// Start closing now (the UI animates the last [`SPOT_CLOSE`]).
    pub fn close_soon(&self) {
        if let Ok(mut s) = self.0.lock() {
            let end = Instant::now() + SPOT_CLOSE;
            if s.active_until.is_some_and(|t| t > end) {
                s.active_until = Some(end);
            }
        }
    }
}

pub struct Spotlight {
    handle: SpotlightHandle,
    worker: Option<Worker>,
    pub sensitivity: u8,
}

impl Spotlight {
    pub fn new(sensitivity: u8) -> Self {
        Self { handle: SpotlightHandle::default(), worker: None, sensitivity }
    }

    pub fn handle(&self) -> SpotlightHandle {
        self.handle.clone()
    }
}

impl Module for Spotlight {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let shared = self.handle.0.clone();
        let scale = halo_platform::monitor_for(0).scale;
        let mut det = ShakeDetector::new(self.sensitivity, scale);
        self.worker = Some(Worker::every("halo-spotlight", Duration::from_millis(16), move || {
            if let Some((x, y)) = halo_platform::cursor_pos() {
                let now = Instant::now();
                let shake = det.feed(now, x, y);
                if let Ok(mut s) = shared.lock() {
                    s.cursor = (x, y);
                    if shake {
                        s.active_until = Some(now + SPOT_DURATION);
                    }
                }
            }
            true
        }));
        Ok(())
    }

    fn configure(&mut self, config: &halo_core::config::Config) -> bool {
        let s = config.settings.spotlight_sensitivity;
        let changed = s != self.sensitivity;
        self.sensitivity = s;
        changed
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(det: &mut ShakeDetector, pts: &[(u64, i32)]) -> bool {
        let t0 = Instant::now();
        pts.iter().any(|&(ms, x)| det.feed(t0 + Duration::from_millis(ms), x, 500))
    }

    #[test]
    fn fast_zigzag_is_a_shake() {
        let mut d = ShakeDetector::new(3, 1.0);
        let mut pts = vec![];
        for i in 0..12u64 {
            let x = if i % 2 == 0 { 400 } else { 600 };
            pts.push((i * 40, x));
            pts.push((i * 40 + 20, (x + 500) / 2));
        }
        assert!(run(&mut d, &pts));
    }

    #[test]
    fn normal_movement_is_not() {
        let mut d = ShakeDetector::new(3, 1.0);
        let pts: Vec<_> = (0..100u64).map(|i| (i * 16, 100 + i as i32 * 8)).collect();
        assert!(!run(&mut d, &pts));
        // Small jitter is not a shake either.
        let mut d = ShakeDetector::new(3, 1.0);
        let jitter: Vec<_> = (0..60u64).map(|i| (i * 16, 500 + if i % 2 == 0 { 3 } else { -3 })).collect();
        assert!(!run(&mut d, &jitter));
    }

    #[test]
    fn slow_zigzag_is_not() {
        let mut d = ShakeDetector::new(3, 1.0);
        let pts: Vec<_> = (0..10u64).map(|i| (i * 900, if i % 2 == 0 { 300 } else { 700 })).collect();
        assert!(!run(&mut d, &pts));
    }
}
