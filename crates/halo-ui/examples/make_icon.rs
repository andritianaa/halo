//! Generates `assets/icons/halo.ico` (16–256 px): charter app icon = dark tile
//! (#0B0D12, rounded) with the flat #818CF8 halo mark. Run:
//! `cargo run -p halo-ui --example make_icon --no-default-features`

use halo_ui::mark::mark_rgba;
use halo_ui::tokens::{ACCENT, Color};

fn tile(size: u32) -> Vec<u8> {
    let bg = Color::hex(0x0B0D12);
    let radius = size as f32 * 0.22;
    let inset = (size as f32 * 0.16).round() as u32;
    let mark_size = size - 2 * inset;
    let mark = mark_rgba(mark_size.max(1), ACCENT);
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            // Rounded-square coverage with 4x4 supersampling.
            let mut cover = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let dx = (radius - fx).max(fx - (s - radius)).max(0.0);
                    let dy = (radius - fy).max(fy - (s - radius)).max(0.0);
                    cover += (dx * dx + dy * dy <= radius * radius) as u32;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            let a_tile = cover as f32 / 16.0;
            let (mut r, mut g, mut b) = (bg.r() as f32, bg.g() as f32, bg.b() as f32);
            if x >= inset && y >= inset && x < inset + mark_size && y < inset + mark_size {
                let j = (((y - inset) * mark_size + (x - inset)) * 4) as usize;
                let a = mark[j + 3] as f32 / 255.0;
                r = r * (1.0 - a) + mark[j] as f32 * a;
                g = g * (1.0 - a) + mark[j + 1] as f32 * a;
                b = b * (1.0 - a) + mark[j + 2] as f32 * a;
            }
            px[i] = r as u8;
            px[i + 1] = g as u8;
            px[i + 2] = b as u8;
            px[i + 3] = (a_tile * 255.0) as u8;
        }
    }
    px
}

/// One ICO entry as a 32-bit BGRA DIB (bottom-up) + AND mask.
fn dib(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&40u32.to_le_bytes());
    v.extend_from_slice(&(size as i32).to_le_bytes());
    v.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(&[0u8; 24]);
    for y in (0..size).rev() {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            v.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    let row = size.div_ceil(32) * 4;
    v.extend(std::iter::repeat_n(0u8, (row * size) as usize));
    v
}

fn main() {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| dib(s, &tile(s))).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let d = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend_from_slice(&[d, d, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in &images {
        ico.extend_from_slice(img);
    }
    let out = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/icons/halo.ico");
    std::fs::write(out, &ico).expect("write ico");
    println!("wrote {out} ({} bytes)", ico.len());
}
