//! Spring animation used by the overlay (`MOTION_SPRING`).
//!
//! Framework-agnostic: the caller advances it with the frame delta and asks GPUI for
//! another frame only while `!is_settled()`, so a settled overlay costs no CPU.

use crate::tokens::SpringSpec;

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
    stiffness: f32,
    damping: f32,
}

/// Below these, the spring snaps to its target and stops requesting frames.
const REST_DELTA: f32 = 0.001;
const REST_VELOCITY: f32 = 0.01;
/// Fixed integration step (s); large frame gaps are subdivided for stability.
const STEP: f32 = 1.0 / 240.0;

impl Spring {
    pub fn new(spec: SpringSpec, value: f32) -> Self {
        let omega = std::f32::consts::TAU / spec.response.max(0.01);
        Self {
            value,
            velocity: 0.0,
            target: value,
            stiffness: omega * omega,
            damping: 2.0 * spec.damping * omega,
        }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Jump without animating (e.g. reduced motion).
    pub fn snap(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    pub fn is_settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.0
    }

    /// Advance by `dt` seconds. Returns `true` while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        if self.is_settled() {
            return false;
        }
        // Clamp pathological gaps (window hidden, debugger) to keep it stable.
        let mut remaining = dt.clamp(0.0, 0.1);
        while remaining > 0.0 {
            let h = remaining.min(STEP);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h; // semi-implicit Euler
            self.value += self.velocity * h;
            remaining -= h;
        }
        if (self.value - self.target).abs() < REST_DELTA && self.velocity.abs() < REST_VELOCITY {
            self.snap(self.target);
            return false;
        }
        true
    }
}

/// Linear interpolation helper for animating sizes/radii from a 0→1 progress.
pub fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::MOTION_SPRING;

    fn settle_time(mut s: Spring) -> (f32, f32) {
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        let mut peak = s.value();
        while s.step(dt) {
            t += dt;
            peak = peak.max(s.value());
            assert!(t < 2.0, "spring never settled");
        }
        (t, peak)
    }

    #[test]
    fn settles_on_target_quickly_with_little_overshoot() {
        let mut s = Spring::new(MOTION_SPRING, 0.0);
        s.set_target(1.0);
        let (t, peak) = settle_time(s);
        assert!(t > 0.15 && t < 0.6, "settle time {t}");
        assert!(peak < 1.02, "overshoot {peak}");
    }

    #[test]
    fn settled_spring_requests_no_frames() {
        let mut s = Spring::new(MOTION_SPRING, 0.5);
        assert!(s.is_settled());
        assert!(!s.step(1.0 / 60.0));
    }

    #[test]
    fn survives_huge_frame_gap() {
        let mut s = Spring::new(MOTION_SPRING, 0.0);
        s.set_target(1.0);
        s.step(5.0);
        assert!(s.value().is_finite());
    }
}
