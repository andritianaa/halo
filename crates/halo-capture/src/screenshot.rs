//! M12 — Screenshots: full virtual screen, one monitor, a region or a window; PNG to a folder
//! (`%USERPROFILE%\Pictures\Halo`) and/or the clipboard, with optional annotations.
//!
//! All calls are synchronous (GDI blit + PNG encode, typically < 100 ms for a 4K screen) and
//! can run on any thread. Coordinates are physical pixels (the app must be per-monitor DPI aware).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, anyhow, bail};
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use halo_core::overlay::Rect;
use halo_platform::capture::{self, Image};

use crate::annotate::{self, Annotation};

pub const MANIFEST: Manifest = Manifest {
    id: "screenshot",
    cdc: "M12",
    name_key: "module.screenshot",
    category: Category::Capture,
    icon: "camera",
    permissions: &[Permission::ScreenCapture],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenshotSettings {
    pub folder: PathBuf,
    pub copy_to_clipboard: bool,
    pub save_to_file: bool,
}

impl Default for ScreenshotSettings {
    fn default() -> Self {
        Self { folder: crate::user_folder("Pictures"), copy_to_clipboard: true, save_to_file: true }
    }
}

#[derive(Default)]
struct State {
    enabled: bool,
    settings: ScreenshotSettings,
    last: Option<PathBuf>,
}

#[derive(Clone, Default)]
pub struct ScreenshotHandle(Arc<Mutex<State>>);

impl ScreenshotHandle {
    pub fn settings(&self) -> ScreenshotSettings {
        self.0.lock().unwrap().settings.clone()
    }

    pub fn set_settings(&self, s: ScreenshotSettings) {
        self.0.lock().unwrap().settings = s;
    }

    /// Path of the last saved capture.
    pub fn last(&self) -> Option<PathBuf> {
        self.0.lock().unwrap().last.clone()
    }

    /// Grab pixels only (for an annotation editor); nothing is saved.
    pub fn grab(&self, r: Rect) -> anyhow::Result<Image> {
        if !self.0.lock().unwrap().enabled {
            bail!("screenshot module is disabled");
        }
        capture::grab(r).ok_or_else(|| anyhow!("screen capture failed for {r:?}"))
    }

    /// Capture a region (physical px). Returns the saved PNG path (`None` if `save_to_file` is off).
    pub fn capture_region(&self, r: Rect) -> anyhow::Result<Option<PathBuf>> {
        let img = self.grab(r)?;
        self.output(&img)
    }

    /// `None` = whole virtual screen (all monitors); `Some(i)` = monitor `i`
    /// (primary first, then left to right — same order as `halo_platform::monitors()`).
    pub fn capture_screen(&self, monitor: Option<usize>) -> anyhow::Result<Option<PathBuf>> {
        let r = match monitor {
            None => capture::virtual_screen(),
            Some(i) => *monitor_bounds().get(i).ok_or_else(|| anyhow!("no monitor {i}"))?,
        };
        self.capture_region(r)
    }

    /// Capture a top-level window by HWND (visible frame bounds, as on screen).
    pub fn capture_window(&self, hwnd: isize) -> anyhow::Result<Option<PathBuf>> {
        let r = window_bounds(hwnd).ok_or_else(|| anyhow!("invalid window"))?;
        self.capture_region(r)
    }

    /// Rasterise annotations onto an image (from [`grab`](Self::grab)) and save/copy it.
    pub fn save_annotated(&self, mut img: Image, anns: &[Annotation]) -> anyhow::Result<Option<PathBuf>> {
        annotate::rasterize(&mut img, anns);
        self.output(&img)
    }

    fn output(&self, img: &Image) -> anyhow::Result<Option<PathBuf>> {
        let s = self.settings();
        if s.copy_to_clipboard {
            if let Err(e) = copy_to_clipboard(img) {
                log::warn!("screenshot: clipboard copy failed: {e}");
                if !s.save_to_file {
                    return Err(e);
                }
            }
        }
        if !s.save_to_file {
            return Ok(None);
        }
        let path = save_png(img, &s.folder)?;
        self.0.lock().unwrap().last = Some(path.clone());
        Ok(Some(path))
    }
}

/// Save as `Capture YYYY-MM-DD HHhMMmSS.png` (unique) in `folder`.
pub fn save_png(img: &Image, folder: &std::path::Path) -> anyhow::Result<PathBuf> {
    let path = crate::output_path(folder, "Capture", "png")?;
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.rgba.clone()).context("image buffer size")?;
    buf.save_with_format(&path, image::ImageFormat::Png)?;
    Ok(path)
}

pub fn copy_to_clipboard(img: &Image) -> anyhow::Result<()> {
    let mut cb = arboard::Clipboard::new()?;
    cb.set_image(arboard::ImageData {
        width: img.width as usize,
        height: img.height as usize,
        bytes: std::borrow::Cow::Borrowed(&img.rgba),
    })?;
    Ok(())
}

/// Full bounds of every monitor (physical px), primary first then left to right.
pub fn monitor_bounds() -> Vec<Rect> {
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
    use windows::core::BOOL;

    unsafe extern "system" fn collect(hmon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(data.0 as *mut Vec<(bool, Rect)>);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hmon, &mut info).as_bool() {
                let r = info.rcMonitor;
                out.push((info.dwFlags & MONITORINFOF_PRIMARY != 0, Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)));
            }
        }
        BOOL(1)
    }
    let mut out: Vec<(bool, Rect)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut _ as isize));
    }
    out.sort_by_key(|(p, r)| (!*p, r.x, r.y));
    out.into_iter().map(|(_, r)| r).collect()
}

/// Visible bounds of a window: DWM extended frame (no invisible resize borders), falling back
/// to `GetWindowRect`.
pub fn window_bounds(hwnd: isize) -> Option<Rect> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    let mut r = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            HWND(hwnd as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut _ as *mut _,
            size_of::<RECT>() as u32,
        )
        .is_ok()
    };
    let r = if ok { Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top) } else { halo_platform::window_rect(hwnd)? };
    (r.w > 0 && r.h > 0).then_some(r)
}

pub struct Screenshot {
    handle: ScreenshotHandle,
}

impl Screenshot {
    pub fn new() -> Self {
        Self { handle: ScreenshotHandle::default() }
    }
    pub fn handle(&self) -> ScreenshotHandle {
        self.handle.clone()
    }
}

impl Default for Screenshot {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Screenshot {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        self.handle.0.lock().unwrap().enabled = true;
        Ok(())
    }
    fn stop(&mut self) {
        self.handle.0.lock().unwrap().enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(folder: PathBuf) -> ScreenshotHandle {
        let h = ScreenshotHandle::default();
        h.0.lock().unwrap().enabled = true;
        h.set_settings(ScreenshotSettings { folder, copy_to_clipboard: false, save_to_file: true });
        h
    }

    #[test]
    fn disabled_module_refuses() {
        let h = ScreenshotHandle::default();
        assert!(h.capture_region(Rect::new(0, 0, 8, 8)).is_err());
    }

    #[test]
    fn png_roundtrip_synthetic() {
        let dir = tempfile::tempdir().unwrap();
        let img = Image { width: 5, height: 3, rgba: vec![10; 60] };
        let p = save_png(&img, dir.path()).unwrap();
        assert!(p.file_name().unwrap().to_string_lossy().starts_with("Capture "));
        assert_eq!(image::image_dimensions(&p).unwrap(), (5, 3));
    }

    /// Real capture, privacy-safe: 64×64 into a temp dir, only the PNG header is checked, then deleted.
    #[test]
    fn capture_small_region_to_temp() {
        let dir = tempfile::tempdir().unwrap();
        let h = enabled(dir.path().to_path_buf());
        let vs = capture::virtual_screen();
        let p = h.capture_region(Rect::new(vs.x, vs.y, 64, 64)).unwrap().unwrap();
        assert_eq!(image::image_dimensions(&p).unwrap(), (64, 64));
        assert_eq!(h.last().as_deref(), Some(p.as_path()));
        std::fs::remove_file(&p).unwrap();
        assert!(!monitor_bounds().is_empty());
    }
}
