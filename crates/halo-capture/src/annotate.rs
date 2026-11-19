//! Lightweight screenshot annotation model (M12), rasterised onto an RGBA image.
//!
//! Coordinates are image pixels. Arrows and rectangles are drawn with a flat colour and a
//! thickness; `Blur` pixelates its rectangle (irreversible, unlike a light gaussian blur).
//! `Text` is kept in the model only: Halo ships no font rasteriser in this crate, so the UI
//! draws text annotations itself (`rasterize` leaves them untouched).

use halo_core::overlay::Rect;
use halo_platform::capture::Image;

pub type Point = (i32, i32);

/// Pixelation block size for `Shape::Blur`, in px.
pub const BLUR_BLOCK: i32 = 10;

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { from: Point, to: Point },
    Rect(Rect),
    Text { pos: Point, text: String },
    Blur { rect: Rect },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub shape: Shape,
    /// RGBA; alpha blends over the image.
    pub color: [u8; 4],
    pub thickness: f32,
}

impl Annotation {
    pub fn new(shape: Shape) -> Self {
        Self { shape, color: [239, 68, 68, 255], thickness: 3.0 }
    }
}

/// Apply annotations in order (later ones on top).
pub fn rasterize(img: &mut Image, anns: &[Annotation]) {
    for a in anns {
        match &a.shape {
            Shape::Arrow { from, to } => arrow(img, *from, *to, a.thickness, a.color),
            Shape::Rect(r) => rect_outline(img, *r, a.thickness, a.color),
            Shape::Blur { rect } => pixelate(img, *rect, BLUR_BLOCK),
            Shape::Text { .. } => {}
        }
    }
}

fn blend(img: &mut Image, x: i32, y: i32, c: [u8; 4]) {
    if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
        return;
    }
    let i = ((y as u32 * img.width + x as u32) * 4) as usize;
    let a = c[3] as u32;
    for k in 0..3 {
        let d = img.rgba[i + k] as u32;
        img.rgba[i + k] = ((c[k] as u32 * a + d * (255 - a) + 127) / 255) as u8;
    }
    img.rgba[i + 3] = 255;
}

/// Thick segment (capsule): every pixel whose centre is within `t/2` of the segment.
pub fn line(img: &mut Image, a: Point, b: Point, t: f32, c: [u8; 4]) {
    let r = (t.max(1.0)) / 2.0;
    let pad = r.ceil() as i32 + 1;
    let (x0, x1) = (a.0.min(b.0) - pad, a.0.max(b.0) + pad);
    let (y0, y1) = (a.1.min(b.1) - pad, a.1.max(b.1) + pad);
    let x0 = x0.max(0);
    let y0 = y0.max(0);
    let x1 = x1.min(img.width as i32 - 1);
    let y1 = y1.min(img.height as i32 - 1);
    let (ax, ay, bx, by) = (a.0 as f32, a.1 as f32, b.0 as f32, b.1 as f32);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (px, py) = (x as f32, y as f32);
            let s = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
            let (qx, qy) = (ax + s * dx - px, ay + s * dy - py);
            if qx * qx + qy * qy <= r * r {
                blend(img, x, y, c);
            }
        }
    }
}

pub fn arrow(img: &mut Image, from: Point, to: Point, t: f32, c: [u8; 4]) {
    line(img, from, to, t, c);
    let (dx, dy) = ((from.0 - to.0) as f32, (from.1 - to.1) as f32);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let head = (4.0 * t).max(12.0).min(len);
    let (ux, uy) = (dx / len, dy / len);
    let ang = 25f32.to_radians();
    for s in [ang, -ang] {
        let (cs, sn) = (s.cos(), s.sin());
        let hx = ux * cs - uy * sn;
        let hy = ux * sn + uy * cs;
        let end = (to.0 + (hx * head).round() as i32, to.1 + (hy * head).round() as i32);
        line(img, to, end, t, c);
    }
}

/// Outline drawn inside the rectangle (`t` px bands).
pub fn rect_outline(img: &mut Image, r: Rect, t: f32, c: [u8; 4]) {
    let t = (t.round() as i32).max(1);
    let fill = |img: &mut Image, x0: i32, y0: i32, x1: i32, y1: i32| {
        for y in y0.max(0)..y1.min(img.height as i32) {
            for x in x0.max(0)..x1.min(img.width as i32) {
                blend(img, x, y, c);
            }
        }
    };
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    fill(img, x0, y0, x1, (y0 + t).min(y1));
    fill(img, x0, (y1 - t).max(y0), x1, y1);
    fill(img, x0, y0 + t, (x0 + t).min(x1), y1 - t);
    fill(img, (x1 - t).max(x0), y0 + t, x1, y1 - t);
}

/// Replace each `block`×`block` cell inside `r` by its average colour.
pub fn pixelate(img: &mut Image, r: Rect, block: i32) {
    let block = block.max(1);
    let (x0, y0) = (r.x.max(0), r.y.max(0));
    let (x1, y1) = ((r.x + r.w).min(img.width as i32), (r.y + r.h).min(img.height as i32));
    let w = img.width as usize;
    let mut by = y0;
    while by < y1 {
        let ey = (by + block).min(y1);
        let mut bx = x0;
        while bx < x1 {
            let ex = (bx + block).min(x1);
            let mut sum = [0u64; 3];
            let n = ((ex - bx) * (ey - by)) as u64;
            for y in by..ey {
                for x in bx..ex {
                    let i = (y as usize * w + x as usize) * 4;
                    for k in 0..3 {
                        sum[k] += img.rgba[i + k] as u64;
                    }
                }
            }
            for y in by..ey {
                for x in bx..ex {
                    let i = (y as usize * w + x as usize) * 4;
                    for k in 0..3 {
                        img.rgba[i + k] = ((sum[k] + n / 2) / n) as u8;
                    }
                }
            }
            bx = ex;
        }
        by = ey;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank(w: u32, h: u32) -> Image {
        Image { width: w, height: h, rgba: [255, 255, 255, 255].repeat((w * h) as usize) }
    }
    fn px(img: &Image, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.rgba[i..i + 4].try_into().unwrap()
    }
    const RED: [u8; 4] = [255, 0, 0, 255];

    #[test]
    fn rect_outline_has_thickness_and_hollow_inside() {
        let mut img = blank(40, 40);
        rasterize(&mut img, &[Annotation { shape: Shape::Rect(Rect::new(5, 5, 30, 30)), color: RED, thickness: 3.0 }]);
        assert_eq!(px(&img, 5, 5), RED);
        assert_eq!(px(&img, 7, 20), RED); // left band, 3 px wide
        assert_eq!(px(&img, 8, 20), [255; 4]); // just inside
        assert_eq!(px(&img, 34, 34), RED);
        assert_eq!(px(&img, 20, 20), [255; 4]);
        assert_eq!(px(&img, 4, 4), [255; 4]); // outside
    }

    #[test]
    fn arrow_draws_shaft_and_head() {
        let mut img = blank(100, 50);
        rasterize(&mut img, &[Annotation { shape: Shape::Arrow { from: (10, 25), to: (90, 25) }, color: RED, thickness: 3.0 }]);
        assert_eq!(px(&img, 50, 25), RED);
        assert_eq!(px(&img, 50, 26), RED);
        assert_eq!(px(&img, 50, 30), [255; 4]);
        // Head wings go back-left from the tip, above and below the shaft.
        assert_eq!(px(&img, 82, 21), RED);
        assert_eq!(px(&img, 82, 29), RED);
        assert_eq!(px(&img, 95, 25), [255; 4]);
    }

    #[test]
    fn translucent_colour_blends() {
        let mut img = blank(10, 10);
        line(&mut img, (0, 5), (9, 5), 1.0, [0, 0, 0, 128]);
        assert_eq!(px(&img, 5, 5), [127, 127, 127, 255]);
    }

    #[test]
    fn blur_averages_blocks_inside_rect_only() {
        let mut img = blank(20, 20);
        // Checkerboard inside the first block.
        for y in 0..10 {
            for x in 0..10 {
                if (x + y) % 2 == 0 {
                    let i = ((y * 20 + x) * 4) as usize;
                    img.rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                }
            }
        }
        let before_outside = px(&img, 15, 15);
        rasterize(&mut img, &[Annotation::new(Shape::Blur { rect: Rect::new(0, 0, 10, 10) })]);
        let c = px(&img, 0, 0);
        assert!((126..=129).contains(&c[0]), "{c:?}");
        assert_eq!(px(&img, 9, 9), c);
        assert_eq!(px(&img, 15, 15), before_outside);
    }

    #[test]
    fn text_is_model_only_and_out_of_bounds_is_safe() {
        let mut img = blank(8, 8);
        let orig = img.clone();
        rasterize(
            &mut img,
            &[
                Annotation::new(Shape::Text { pos: (1, 1), text: "hi".into() }),
                Annotation::new(Shape::Rect(Rect::new(100, 100, 5, 5))),
                Annotation::new(Shape::Blur { rect: Rect::new(-50, -50, 10, 10) }),
            ],
        );
        assert_eq!(img, orig);
        arrow(&mut img, (-20, -20), (50, 50), 4.0, RED); // clipped, no panic
    }
}
