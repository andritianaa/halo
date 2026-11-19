//! Halo capture modules: screenshot (M12), screen recording with audio (M13), audio recorder (M14).
//!
//! Every handle is cheap to clone (`Arc<Mutex<..>>` inside) and can be used from the UI thread:
//! screenshots run synchronously (a GDI blit + PNG encode, tens of ms), recordings run on their
//! own worker threads, which also own COM / Media Foundation initialisation.
//!
//! Windows only (GDI, WASAPI via cpal, Media Foundation).

pub mod annotate;
pub mod audio;
pub mod clock;
pub mod effects;
pub mod screenshot;
pub mod video;

use std::path::{Path, PathBuf};

use halo_core::config::Config;
use halo_core::module::Registry;

pub use audio::{AudioHandle, AudioRecorder, AudioState, Format, MicTest, Source, microphones, set_microphone};
pub use effects::{ClickStyle, Effects};
pub use halo_core::overlay::Rect;
pub use screenshot::{Screenshot, ScreenshotHandle, ScreenshotSettings};
pub use video::{AudioOpts, ScreenRecorder, VideoHandle, VideoState};

/// Handles the UI uses to drive the capture modules.
#[derive(Clone)]
pub struct Handles {
    pub screenshot: ScreenshotHandle,
    pub audio: AudioHandle,
    pub video: VideoHandle,
}

/// Register M12, M13 and M14 and return their handles.
pub fn register(registry: &mut Registry, _config: &Config) -> Handles {
    let shot = Screenshot::new();
    let audio = AudioRecorder::new();
    let video = ScreenRecorder::new();
    let handles = Handles { screenshot: shot.handle(), audio: audio.handle(), video: video.handle() };
    registry.register(Box::new(shot));
    registry.register(Box::new(audio));
    registry.register(Box::new(video));
    handles
}

/// `%USERPROFILE%\<sub>\Halo`.
pub(crate) fn user_folder(sub: &str) -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join(sub).join("Halo")
}

/// `"<prefix> YYYY-MM-DD HHhMMmSS.<ext>"`.
pub fn timestamp_name(prefix: &str, t: chrono::NaiveDateTime, ext: &str) -> String {
    format!("{prefix} {}.{ext}", t.format("%Y-%m-%d %Hh%Mm%S"))
}

/// First free path in `dir` for `name`, adding ` (2)`, ` (3)`… before the extension.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded")
}

/// Timestamped, unique output path in `dir` (created on demand).
pub(crate) fn output_path(dir: &Path, prefix: &str, ext: &str) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    Ok(unique_path(dir, &timestamp_name(prefix, chrono::Local::now().naive_local(), ext)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_format() {
        let t = chrono::NaiveDate::from_ymd_opt(2026, 3, 7).unwrap().and_hms_opt(9, 5, 2).unwrap();
        assert_eq!(timestamp_name("Capture", t, "png"), "Capture 2026-03-07 09h05m02.png");
    }

    #[test]
    fn unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = unique_path(dir.path(), "Capture x.png");
        assert_eq!(a.file_name().unwrap(), "Capture x.png");
        std::fs::write(&a, b"1").unwrap();
        let b = unique_path(dir.path(), "Capture x.png");
        assert_eq!(b.file_name().unwrap(), "Capture x (2).png");
        std::fs::write(&b, b"1").unwrap();
        assert_eq!(unique_path(dir.path(), "Capture x.png").file_name().unwrap(), "Capture x (3).png");
    }

    #[test]
    fn register_all() {
        let mut r = Registry::new(halo_core::bus::Bus::new());
        let _h = register(&mut r, &Config::default());
        let ids: Vec<_> = r.manifests().map(|(m, _)| m.id).collect();
        assert_eq!(ids, ["screenshot", "audiorec", "recorder"]);
    }
}
