//! Rasteriser for the Halo mark (tray icon), so no binary asset is needed.
//!
//! Geometry from the charter's `#mark` symbol (viewBox 120): an incomplete outer arc
//! (r 36, stroke 9, 170/226 visible, rotated −54°), an inner orbit (r 16, stroke 7,
//! 70/100 visible, rotated 120°) and a satellite dot (r 7) where the outer arc starts.
//! Flat colour only.

use crate::tokens::Color;

/// RGBA8 pixels (`size × size × 4`) of the mark in `color` on transparent.
pub fn mark_rgba(size: u32, color: Color) -> Vec<u8> {
    let s = size as f32 / 120.0;
    let c = 60.0 * s;
    let deg = std::f32::consts::PI / 180.0;

    // Arc = start angle (SVG dash starts at 3 o'clock, clockwise) + visible sweep.
    let outer = Arc { r: 36.0 * s, w: 9.0 * s, start: -54.0 * deg, sweep: 170.0 / 36.0 };
    let inner = Arc { r: 16.0 * s, w: 7.0 * s, start: 120.0 * deg, sweep: 70.0 / 16.0 };
    let dot_a = -54.0 * deg;
    let (dx, dy) = (c + 36.0 * s * dot_a.cos(), c + 36.0 * s * dot_a.sin());
    let dot_r = 7.0 * s;

    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // 4×4 supersampling for clean edges at 16–32 px.
            let mut cover = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let hit = outer.contains(fx - c, fy - c)
                        || inner.contains(fx - c, fy - c)
                        || (fx - dx).hypot(fy - dy) <= dot_r;
                    cover += hit as u32;
                }
            }
            if cover > 0 {
                let i = ((y * size + x) * 4) as usize;
                px[i] = color.r();
                px[i + 1] = color.g();
                px[i + 2] = color.b();
                px[i + 3] = ((color.a() as u32 * cover) / 16) as u8;
            }
        }
    }
    px
}

struct Arc {
    r: f32,
    w: f32,
    start: f32,
    sweep: f32,
}

impl Arc {
    fn contains(&self, x: f32, y: f32) -> bool {
        let d = x.hypot(y);
        let half = self.w / 2.0;
        let tau = std::f32::consts::TAU;
        let rel = (y.atan2(x) - self.start).rem_euclid(tau);
        if (d - self.r).abs() <= half && rel <= self.sweep {
            return true;
        }
        // Round caps.
        let cap = |a: f32| (x - self.r * a.cos()).hypot(y - self.r * a.sin()) <= half;
        cap(self.start) || cap(self.start + self.sweep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens::ACCENT;

    #[test]
    fn draws_something_but_not_everything() {
        let px = mark_rgba(32, ACCENT);
        assert_eq!(px.len(), 32 * 32 * 4);
        let opaque = px.chunks(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 80 && opaque < 700, "{opaque} lit pixels");
        // Centre of the mark is empty (inside the inner orbit).
        let centre = ((16 * 32 + 16) * 4 + 3) as usize;
        assert_eq!(px[centre], 0);
    }
}
