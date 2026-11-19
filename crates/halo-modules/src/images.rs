//! M9 — Image resizer / compressor: resize (px, %, presets), high-quality resampling,
//! compression (JPEG/WebP quality), conversion (PNG/JPEG/WebP/AVIF), batch.
//! AVIF is supported as an output format only (decoding would require dav1d).
//! Re-encoding drops EXIF metadata (privacy-friendly default; "keep" is not supported
//! by the encoders used and is reported as such in the UI).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fast_image_resize as fr;
use halo_core::module::{Category, Manifest, Module, ModuleContext};
use image::{DynamicImage, ImageFormat};

pub const MANIFEST: Manifest = Manifest {
    id: "images",
    cdc: "M9",
    name_key: "module.images",
    category: Category::Files,
    icon: "image",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutFormat {
    Same,
    Png,
    Jpeg,
    Webp,
    Avif,
}

impl OutFormat {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "webp" => Self::Webp,
            "avif" => Self::Avif,
            _ => Self::Same,
        }
    }

    pub fn ext(self, src: &Path) -> String {
        match self {
            Self::Png => "png".into(),
            Self::Jpeg => "jpg".into(),
            Self::Webp => "webp".into(),
            Self::Avif => "avif".into(),
            Self::Same => src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resize {
    None,
    /// Fit inside a box, keep aspect ratio, never upscale.
    Fit { max_w: u32, max_h: u32 },
    Percent(f32),
    /// Exact width, height from aspect ratio.
    Width(u32),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub resize: Resize,
    pub format: OutFormat,
    /// 1–100 (JPEG, WebP lossy, AVIF).
    pub quality: u8,
    /// None = next to the source, with a suffix.
    pub out_dir: Option<PathBuf>,
    pub suffix: String,
}

impl Default for Job {
    fn default() -> Self {
        Self { resize: Resize::Fit { max_w: 1920, max_h: 1920 }, format: OutFormat::Webp, quality: 82, out_dir: None, suffix: "-halo".into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub before: u64,
    pub after: u64,
    pub size: (u32, u32),
}

pub fn target_size(w: u32, h: u32, r: Resize) -> (u32, u32) {
    let (w, h) = (w.max(1), h.max(1));
    let (nw, nh) = match r {
        Resize::None => (w, h),
        Resize::Fit { max_w, max_h } => {
            let s = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
            ((w as f64 * s).round() as u32, (h as f64 * s).round() as u32)
        }
        Resize::Percent(p) => ((w as f32 * p / 100.0).round() as u32, (h as f32 * p / 100.0).round() as u32),
        Resize::Width(nw) => (nw, (h as f64 * nw as f64 / w as f64).round() as u32),
    };
    (nw.max(1), nh.max(1))
}

/// High-quality Lanczos3 resize (SIMD) via fast_image_resize.
pub fn resize(img: &DynamicImage, w: u32, h: u32) -> anyhow::Result<DynamicImage> {
    if (w, h) == (img.width(), img.height()) {
        return Ok(img.clone());
    }
    let src = DynamicImage::ImageRgba8(img.to_rgba8());
    let mut dst = fr::images::Image::new(w, h, fr::PixelType::U8x4);
    let mut r = fr::Resizer::new();
    r.resize(&src, &mut dst, &fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3)))?;
    let buf = image::RgbaImage::from_raw(w, h, dst.into_vec()).ok_or_else(|| anyhow::anyhow!("resize buffer"))?;
    Ok(DynamicImage::ImageRgba8(buf))
}

pub fn encode(img: &DynamicImage, fmt: OutFormat, src_ext: &str, quality: u8) -> anyhow::Result<Vec<u8>> {
    let q = quality.clamp(1, 100);
    let fmt = if fmt == OutFormat::Same { OutFormat::parse(src_ext) } else { fmt };
    let mut out = Vec::new();
    match fmt {
        OutFormat::Jpeg => {
            let rgb = img.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q).encode_image(&rgb)?;
        }
        OutFormat::Webp => {
            let rgba = img.to_rgba8();
            let enc = webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height());
            out = enc.encode(q as f32).to_vec();
        }
        OutFormat::Avif => {
            let enc = image::codecs::avif::AvifEncoder::new_with_speed_quality(&mut out, 8, q);
            img.to_rgba8().write_with_encoder(enc)?;
        }
        OutFormat::Png | OutFormat::Same => {
            img.write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)?;
        }
    }
    Ok(out)
}

fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image").to_string();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..).map(|i| dir.join(format!("{stem} ({i}).{ext}"))).find(|p| !p.exists()).unwrap()
}

/// Process one file. Never overwrites the source.
pub fn process(src: &Path, job: &Job) -> anyhow::Result<Outcome> {
    let before = std::fs::metadata(src)?.len();
    let img = image::open(src)?;
    let (w, h) = target_size(img.width(), img.height(), job.resize);
    let img = resize(&img, w, h)?;
    let src_ext = src.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
    let bytes = encode(&img, job.format, &src_ext, job.quality)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let dir = job.out_dir.clone().unwrap_or_else(|| src.parent().map(Path::to_path_buf).unwrap_or_default());
    std::fs::create_dir_all(&dir)?;
    let dst = unique(dir.join(format!("{stem}{}.{}", job.suffix, job.format.ext(src))));
    std::fs::write(&dst, &bytes)?;
    Ok(Outcome { src: src.to_path_buf(), dst, before, after: bytes.len() as u64, size: (w, h) })
}

pub fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
    )
}

/// Batch progress shared with the UI.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub total: usize,
    pub done: usize,
    pub results: Vec<Result<Outcome, String>>,
    pub running: bool,
}

#[derive(Clone, Default)]
pub struct ImagesHandle(Arc<Mutex<Batch>>);

impl ImagesHandle {
    pub fn get(&self) -> Batch {
        self.0.lock().map(|b| b.clone()).unwrap_or_default()
    }

    /// Run a batch on a background thread (one file at a time, results streamed).
    pub fn run(&self, files: Vec<PathBuf>, job: Job) {
        let files: Vec<PathBuf> = files.into_iter().filter(|f| is_image(f)).collect();
        if let Ok(mut b) = self.0.lock() {
            if b.running {
                return;
            }
            *b = Batch { total: files.len(), done: 0, results: Vec::new(), running: true };
        }
        let shared = self.0.clone();
        std::thread::spawn(move || {
            for f in files {
                let r = process(&f, &job).map_err(|e| format!("{}: {e}", f.display()));
                if let Ok(mut b) = shared.lock() {
                    b.done += 1;
                    b.results.push(r);
                }
            }
            if let Ok(mut b) = shared.lock() {
                b.running = false;
            }
        });
    }
}

pub struct Images {
    handle: ImagesHandle,
}

impl Images {
    pub fn new() -> Self {
        Self { handle: ImagesHandle::default() }
    }
    pub fn handle(&self) -> ImagesHandle {
        self.handle.clone()
    }
}

impl Default for Images {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Images {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &Path) -> PathBuf {
        let mut img = image::RgbaImage::new(1200, 800);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255]);
        }
        let p = dir.join("photo.png");
        img.save(&p).unwrap();
        p
    }

    #[test]
    fn sizes() {
        assert_eq!(target_size(4000, 3000, Resize::Fit { max_w: 1920, max_h: 1920 }), (1920, 1440));
        assert_eq!(target_size(800, 600, Resize::Fit { max_w: 1920, max_h: 1920 }), (800, 600), "no upscale");
        assert_eq!(target_size(1000, 500, Resize::Percent(50.0)), (500, 250));
        assert_eq!(target_size(1000, 500, Resize::Width(300)), (300, 150));
    }

    #[test]
    fn convert_every_format_without_touching_source() {
        let dir = tempfile::tempdir().unwrap();
        let src = sample(dir.path());
        let before = std::fs::read(&src).unwrap();
        for fmt in [OutFormat::Webp, OutFormat::Jpeg, OutFormat::Png, OutFormat::Avif] {
            let job = Job { resize: Resize::Width(600), format: fmt, quality: 70, ..Job::default() };
            let o = process(&src, &job).unwrap();
            assert_eq!(o.size, (600, 400));
            if fmt == OutFormat::Avif {
                // AVIF is output-only (decoding would need dav1d): check the ISO-BMFF brand.
                let head = std::fs::read(&o.dst).unwrap();
                assert_eq!(&head[4..12], b"ftypavif");
            } else {
                let back = image::open(&o.dst).unwrap();
                assert_eq!((back.width(), back.height()), (600, 400), "{fmt:?}");
            }
            assert!(o.after > 0);
        }
        assert_eq!(std::fs::read(&src).unwrap(), before, "source untouched");
        // A second run does not overwrite the first output.
        let a = process(&src, &Job::default()).unwrap().dst;
        let b = process(&src, &Job::default()).unwrap().dst;
        assert_ne!(a, b);
    }

    #[test]
    fn batch_runs_in_background() {
        let dir = tempfile::tempdir().unwrap();
        let src = sample(dir.path());
        let h = ImagesHandle::default();
        h.run(vec![src, dir.path().join("notes.txt")], Job::default());
        let t = std::time::Instant::now();
        while h.get().running && t.elapsed().as_secs() < 30 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let b = h.get();
        assert_eq!((b.total, b.done), (1, 1), "non-images filtered");
        assert!(b.results[0].is_ok());
    }
}
