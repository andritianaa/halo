//! Background worker thread with cooperative stop (same pattern as halo-modules).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct Worker {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    /// Run `step` every `period` until stopped. `step` returns `false` to end early.
    pub fn every(name: &str, period: Duration, mut step: impl FnMut() -> bool + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let join = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    if !step() {
                        break;
                    }
                    let mut left = period;
                    while left > Duration::ZERO && !flag.load(Ordering::Relaxed) {
                        let s = left.min(Duration::from_millis(50));
                        std::thread::sleep(s);
                        left = left.saturating_sub(s);
                    }
                }
            })
            .expect("spawn worker");
        Self { stop, join: Some(join) }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}
