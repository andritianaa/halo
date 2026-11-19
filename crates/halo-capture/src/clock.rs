//! Pausable recording clock and frame pacing (pure, `Instant`s passed in for tests).

use std::time::{Duration, Instant};

/// Active (non-paused) recording time.
#[derive(Clone, Copy, Debug)]
pub struct PauseClock {
    running_since: Option<Instant>,
    acc: Duration,
}

impl PauseClock {
    pub fn start(now: Instant) -> Self {
        Self { running_since: Some(now), acc: Duration::ZERO }
    }

    pub fn is_paused(&self) -> bool {
        self.running_since.is_none()
    }

    pub fn pause(&mut self, now: Instant) {
        if let Some(t) = self.running_since.take() {
            self.acc += now.saturating_duration_since(t);
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if self.running_since.is_none() {
            self.running_since = Some(now);
        }
    }

    /// Recorded time so far; frozen while paused.
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.acc + self.running_since.map_or(Duration::ZERO, |t| now.saturating_duration_since(t))
    }
}

/// 100 ns units (Media Foundation time base).
pub const HNS_PER_SEC: i64 = 10_000_000;

/// Presentation time of frame `index` at `fps`, in 100 ns units.
pub fn frame_time_hns(index: u64, fps: u32) -> i64 {
    (index as i128 * HNS_PER_SEC as i128 / fps.max(1) as i128) as i64
}

/// Presentation time after `frames` audio frames at `rate` Hz, in 100 ns units.
pub fn audio_time_hns(frames: u64, rate: u32) -> i64 {
    (frames as i128 * HNS_PER_SEC as i128 / rate.max(1) as i128) as i64
}

/// Audio frames owed by the clock: `(elapsed - delay) * rate - written`, never negative.
/// The delay gives the capture buffers time to fill before their slot is consumed.
pub fn owed_frames(elapsed: Duration, delay: Duration, rate: u32, written: u64) -> u64 {
    let due = (elapsed.saturating_sub(delay).as_secs_f64() * rate as f64).floor() as u64;
    due.saturating_sub(written)
}

/// Decides when to capture the next video frame and which frame slot it fills.
///
/// Slots are anchored on active time, so when capture/encode falls behind, late slots are
/// skipped (dropped) instead of drifting: timestamps keep matching wall-clock time.
#[derive(Clone, Copy, Debug)]
pub struct Pacer {
    pub fps: u32,
    next: u64,
}

impl Pacer {
    pub fn new(fps: u32) -> Self {
        Self { fps: fps.max(1), next: 0 }
    }

    /// The slot to fill now, if one is due at `elapsed` active time.
    pub fn due(&mut self, elapsed: Duration) -> Option<u64> {
        let current = (elapsed.as_secs_f64() * self.fps as f64).floor() as u64;
        if current < self.next {
            return None;
        }
        self.next = current + 1;
        Some(current)
    }

    /// Active time at which the next slot becomes due.
    pub fn next_at(&self) -> Duration {
        Duration::from_secs_f64(self.next as f64 / self.fps as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_freezes_elapsed() {
        let t0 = Instant::now();
        let s = |ms| t0 + Duration::from_millis(ms);
        let mut c = PauseClock::start(t0);
        assert_eq!(c.elapsed(s(1000)), Duration::from_millis(1000));
        c.pause(s(1000));
        assert!(c.is_paused());
        assert_eq!(c.elapsed(s(5000)), Duration::from_millis(1000));
        c.pause(s(6000)); // idempotent
        c.resume(s(6000));
        c.resume(s(6500)); // idempotent
        assert_eq!(c.elapsed(s(6500)), Duration::from_millis(1500));
    }

    #[test]
    fn timestamps() {
        assert_eq!(frame_time_hns(0, 30), 0);
        assert_eq!(frame_time_hns(30, 30), HNS_PER_SEC);
        assert_eq!(frame_time_hns(1, 15), 666_666);
        assert_eq!(audio_time_hns(48_000, 48_000), HNS_PER_SEC);
        assert_eq!(audio_time_hns(480, 48_000), 100_000);
    }

    #[test]
    fn owed_audio() {
        let d = Duration::from_millis(100);
        assert_eq!(owed_frames(Duration::from_millis(50), d, 48_000, 0), 0);
        assert_eq!(owed_frames(Duration::from_millis(1100), d, 48_000, 0), 48_000);
        assert_eq!(owed_frames(Duration::from_millis(1100), d, 48_000, 40_000), 8_000);
        assert_eq!(owed_frames(Duration::from_millis(1100), d, 48_000, 50_000), 0);
    }

    #[test]
    fn pacer_paces_and_skips() {
        let ms = Duration::from_millis;
        let mut p = Pacer::new(10);
        assert_eq!(p.due(ms(0)), Some(0));
        assert_eq!(p.due(ms(50)), None);
        assert_eq!(p.next_at(), ms(100));
        assert_eq!(p.due(ms(100)), Some(1));
        // Encoder stalled for 350 ms: slots 2..4 are dropped, timestamps stay on time.
        assert_eq!(p.due(ms(450)), Some(4));
        assert_eq!(p.due(ms(460)), None);
        // 2 s at 15 fps → 30 frames.
        let mut p = Pacer::new(15);
        let n = (0..2000).filter(|t| p.due(ms(*t)).is_some()).count();
        assert_eq!(n, 30);
    }

    #[test]
    fn pause_resume_timeline_is_contiguous() {
        // Frames captured around a pause keep contiguous timestamps (no gap for the pause).
        let t0 = Instant::now();
        let s = |ms| t0 + Duration::from_millis(ms);
        let mut c = PauseClock::start(t0);
        let mut p = Pacer::new(10);
        let mut slots = vec![];
        for t in (0..=3000).step_by(10) {
            if t == 1000 {
                c.pause(s(t));
            }
            if t == 2500 {
                c.resume(s(t));
            }
            if !c.is_paused() {
                if let Some(i) = p.due(c.elapsed(s(t))) {
                    slots.push(i);
                }
            }
        }
        // 1.0 s before the pause + 0.5 s after → slots 0..=15, no gaps.
        assert_eq!(slots, (0..=15).collect::<Vec<_>>());
        assert_eq!(frame_time_hns(*slots.last().unwrap(), 10), 15_000_000);
    }
}
