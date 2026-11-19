//! Visual aids burned into screen recordings: the mouse pointer (drawn by the grab)
//! and click highlights — an expanding ring or a short flash at each click — so
//! viewers can follow what is clicked.

use std::time::{Duration, Instant};

/// How clicks are shown in recordings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClickStyle {
    #[default]
    Off,
    /// A ring that grows and fades.
    Ring,
    /// A filled disc that flashes and fades.
    Flash,
}

impl ClickStyle {
    pub fn parse(s: &str) -> Self {
        match s {
            "ring" => ClickStyle::Ring,
            "flash" => ClickStyle::Flash,
            _ => ClickStyle::Off,
        }
    }
}

/// Options for a recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    pub cursor: bool,
    pub clicks: ClickStyle,
}

impl Default for Effects {
    fn default() -> Self {
        Self { cursor: true, clicks: ClickStyle::Off }
    }
}

/// How long a click stays visible.
pub const CLICK_LIFE: Duration = Duration::from_millis(450);
/// Halo accent (#818CF8), the colour of click highlights.
const ACCENT: [u8; 3] = [0x81, 0x8C, 0xF8];

/// Remembers recent clicks (screen px) from successive button samples.
#[derive(Debug, Default)]
pub struct ClickTracker {
    clicks: Vec<(Instant, i32, i32)>,
    was_down: bool,
}

impl ClickTracker {
    /// Feed the current button state; a press (up → down) starts a highlight.
    pub fn sample(&mut self, now: Instant, down: bool, at: (i32, i32)) {
        if down && !self.was_down {
            self.clicks.push((now, at.0, at.1));
        }
        self.was_down = down;
        self.clicks.retain(|(t, ..)| now.saturating_duration_since(*t) < CLICK_LIFE);
    }

    /// Draw live highlights onto a frame whose top-left is screen point `origin`.
    /// `scale` = DPI scale (sizes are in logical px).
    pub fn draw(&self, now: Instant, style: ClickStyle, rgba: &mut [u8], w: usize, h: usize, origin: (i32, i32), scale: f32) {
        if style == ClickStyle::Off {
            return;
        }
        for (t, x, y) in &self.clicks {
            let p = (now.saturating_duration_since(*t).as_secs_f32() / CLICK_LIFE.as_secs_f32()).clamp(0.0, 1.0);
            let (cx, cy) = ((x - origin.0) as f32, (y - origin.1) as f32);
            match style {
                ClickStyle::Ring => {
                    let r = (10.0 + 22.0 * p) * scale;
                    ring(rgba, w, h, cx, cy, r, 3.0 * scale, ACCENT, 1.0 - p);
                }
                ClickStyle::Flash => {
                    // Quick bright pulse, then fade.
                    let a = if p < 0.25 { 0.55 } else { 0.55 * (1.0 - (p - 0.25) / 0.75) };
                    disc(rgba, w, h, cx, cy, 22.0 * scale, ACCENT, a);
                }
                ClickStyle::Off => {}
            }
        }
    }
}

fn blend(px: &mut [u8], color: [u8; 3], a: f32) {
    for i in 0..3 {
        px[i] = (px[i] as f32 * (1.0 - a) + color[i] as f32 * a).round() as u8;
    }
}

/// Anti-aliased filled disc, alpha `a` (0..1).
pub fn disc(rgba: &mut [u8], w: usize, h: usize, cx: f32, cy: f32, r: f32, color: [u8; 3], a: f32) {
    paint(rgba, w, h, cx, cy, r + 1.0, |d| (r - d + 0.5).clamp(0.0, 1.0) * a, color);
}

/// Anti-aliased ring of thickness `t` centred on radius `r`.
pub fn ring(rgba: &mut [u8], w: usize, h: usize, cx: f32, cy: f32, r: f32, t: f32, color: [u8; 3], a: f32) {
    paint(rgba, w, h, cx, cy, r + t, |d| (t / 2.0 - (d - r).abs() + 0.5).clamp(0.0, 1.0) * a, color);
}

fn paint(rgba: &mut [u8], w: usize, h: usize, cx: f32, cy: f32, reach: f32, cov: impl Fn(f32) -> f32, color: [u8; 3]) {
    if rgba.len() < w * h * 4 {
        return;
    }
    let x0 = (cx - reach).floor().max(0.0) as usize;
    let y0 = (cy - reach).floor().max(0.0) as usize;
    let x1 = ((cx + reach).ceil() as i64).clamp(0, w as i64) as usize;
    let y1 = ((cy + reach).ceil() as i64).clamp(0, h as i64) as usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let a = cov(d);
            if a > 0.0 {
                let i = (y * w + x) * 4;
                blend(&mut rgba[i..i + 4], color, a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
    }

    fn px(f: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let i = (y * w + x) * 4;
        [f[i], f[i + 1], f[i + 2]]
    }

    #[test]
    fn press_starts_one_highlight_and_it_expires() {
        let t0 = Instant::now();
        let mut c = ClickTracker::default();
        c.sample(t0, true, (10, 10));
        c.sample(t0 + Duration::from_millis(16), true, (12, 10)); // held: no new click
        assert_eq!(c.clicks.len(), 1);
        c.sample(t0 + Duration::from_millis(32), false, (12, 10));
        c.sample(t0 + Duration::from_millis(48), true, (30, 30));
        assert_eq!(c.clicks.len(), 2);
        c.sample(t0 + CLICK_LIFE + Duration::from_millis(100), false, (0, 0));
        assert!(c.clicks.is_empty());
    }

    #[test]
    fn ring_paints_its_radius_not_its_centre_and_clips_at_edges() {
        let (w, h) = (64, 64);
        let mut f = frame(w, h);
        ring(&mut f, w, h, 32.0, 32.0, 20.0, 3.0, ACCENT, 1.0);
        assert_eq!(px(&f, w, 32, 32), [0, 0, 0], "centre untouched");
        assert_eq!(px(&f, w, 52, 32), ACCENT, "on the ring");
        // Near an edge: no panic, partial drawing.
        ring(&mut f, w, h, 2.0, 2.0, 20.0, 3.0, ACCENT, 1.0);
        disc(&mut f, w, h, -50.0, -50.0, 10.0, ACCENT, 1.0);
    }

    #[test]
    fn styles_draw_relative_to_frame_origin() {
        let (w, h) = (100, 100);
        let t0 = Instant::now();
        let mut c = ClickTracker::default();
        c.sample(t0, true, (1050, 550));
        let mut f = frame(w, h);
        c.draw(t0, ClickStyle::Flash, &mut f, w, h, (1000, 500), 1.0);
        assert_ne!(px(&f, w, 50, 50), [0, 0, 0], "flash centred on the click");
        let mut g = frame(w, h);
        c.draw(t0, ClickStyle::Off, &mut g, w, h, (1000, 500), 1.0);
        assert!(g.iter().all(|v| *v == 0));
        assert_eq!(ClickStyle::parse("ring"), ClickStyle::Ring);
    }
}
