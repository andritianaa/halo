//! Focus timer (Pomodoro) for the notch: work/break cycles, pause/resume.
//! Pure state machine driven by wall-clock time (tested), shared via a handle.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Focus,
    Break,
}

#[derive(Clone, Debug)]
pub struct Focus {
    pub focus_len: Duration,
    pub break_len: Duration,
    phase: Phase,
    /// Remaining time when paused, or at `since` when running.
    remaining: Duration,
    since: Option<Instant>,
    pub completed: u32,
}

impl Default for Focus {
    fn default() -> Self {
        Self::new(Duration::from_secs(25 * 60), Duration::from_secs(5 * 60))
    }
}

impl Focus {
    pub fn new(focus_len: Duration, break_len: Duration) -> Self {
        Self { focus_len, break_len, phase: Phase::Idle, remaining: focus_len, since: None, completed: 0 }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn running(&self) -> bool {
        self.since.is_some()
    }

    pub fn remaining(&self, now: Instant) -> Duration {
        match self.since {
            Some(s) => self.remaining.saturating_sub(now.duration_since(s)),
            None => self.remaining,
        }
    }

    /// 0 → 1 progress of the current phase.
    pub fn progress(&self, now: Instant) -> f32 {
        let total = match self.phase {
            Phase::Break => self.break_len,
            _ => self.focus_len,
        };
        1.0 - self.remaining(now).as_secs_f32() / total.as_secs_f32().max(1.0)
    }

    pub fn start(&mut self, now: Instant) {
        if self.phase == Phase::Idle {
            self.phase = Phase::Focus;
            self.remaining = self.focus_len;
        }
        if self.since.is_none() {
            self.since = Some(now);
        }
    }

    pub fn pause(&mut self, now: Instant) {
        if self.since.is_some() {
            self.remaining = self.remaining(now);
            self.since = None;
        }
    }

    pub fn toggle(&mut self, now: Instant) {
        if self.running() { self.pause(now) } else { self.start(now) }
    }

    pub fn reset(&mut self) {
        *self = Self { completed: self.completed, ..Self::new(self.focus_len, self.break_len) };
    }

    /// Advance phases; returns the phase that just ended, if any.
    pub fn tick(&mut self, now: Instant) -> Option<Phase> {
        if self.since.is_none() || self.remaining(now) > Duration::ZERO {
            return None;
        }
        let ended = self.phase;
        let (next, len) = match ended {
            Phase::Focus => {
                self.completed += 1;
                (Phase::Break, self.break_len)
            }
            _ => (Phase::Focus, self.focus_len),
        };
        self.phase = next;
        self.remaining = len;
        self.since = Some(now);
        Some(ended)
    }
}

#[derive(Clone, Default)]
pub struct FocusHandle(pub Arc<Mutex<Focus>>);

impl FocusHandle {
    pub fn with<R>(&self, f: impl FnOnce(&mut Focus) -> R) -> R {
        let mut g = self.0.lock().unwrap();
        f(&mut g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_and_pauses() {
        let t0 = Instant::now();
        let mut f = Focus::new(Duration::from_secs(10), Duration::from_secs(2));
        f.start(t0);
        assert_eq!(f.phase(), Phase::Focus);
        f.pause(t0 + Duration::from_secs(4));
        assert_eq!(f.remaining(t0 + Duration::from_secs(100)), Duration::from_secs(6), "frozen while paused");
        f.start(t0 + Duration::from_secs(50));
        assert_eq!(f.tick(t0 + Duration::from_secs(56)), Some(Phase::Focus));
        assert_eq!((f.phase(), f.completed), (Phase::Break, 1));
        assert_eq!(f.tick(t0 + Duration::from_secs(58)), Some(Phase::Break));
        assert_eq!(f.phase(), Phase::Focus);
        assert!((f.progress(t0 + Duration::from_secs(63)) - 0.5).abs() < 0.01);
        f.reset();
        assert_eq!((f.phase(), f.running(), f.completed), (Phase::Idle, false, 1));
    }
}
