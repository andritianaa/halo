//! `halo-platform` — thin, safe wrappers over the Win32 APIs Halo needs.
//!
//! Everything takes/returns plain types (`isize` HWND, [`Rect`] in physical px) so
//! the UI layer does not depend on `windows-rs` directly. On non-Windows targets the
//! functions are inert, which keeps `cargo check` working elsewhere.

pub use halo_core::overlay::Rect;

/// A monitor's work area (excludes the taskbar) and DPI scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub work_area: Rect,
    pub scale: f32,
    pub primary: bool,
}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod controls;
#[cfg(windows)]
pub mod drophelper;
#[cfg(windows)]
pub mod http;
#[cfg(windows)]
pub mod netstats;
#[cfg(windows)]
pub mod media;
#[cfg(windows)]
pub mod notifications;
#[cfg(windows)]
pub mod perf;
#[cfg(windows)]
pub mod proc;
#[cfg(windows)]
pub mod secure;
#[cfg(windows)]
pub mod hooks;
#[cfg(windows)]
pub mod input;
#[cfg(windows)]
pub mod registry;
#[cfg(windows)]
pub mod sys;
#[cfg(windows)]
pub mod task;

#[cfg(not(windows))]
mod stub {
    use super::*;
    pub fn monitors() -> Vec<Monitor> {
        vec![Monitor { work_area: Rect::new(0, 0, 1920, 1080), scale: 1.0, primary: true }]
    }
    pub fn style_overlay_window(_hwnd: isize) {}
    pub fn place_window(_hwnd: isize, _r: Rect) {}
    pub fn window_rect(_hwnd: isize) -> Option<Rect> {
        None
    }
    pub fn set_click_through(_hwnd: isize, _on: bool) {}
    pub fn cursor_pos() -> Option<(i32, i32)> {
        None
    }
    pub fn apps_use_light_theme() -> bool {
        false
    }
    pub fn set_backdrop(_hwnd: isize, _b: super::Backdrop) {}
    pub fn user_locale() -> String {
        "fr-FR".into()
    }
}
#[cfg(not(windows))]
pub use stub::*;

/// DWM system backdrop for glass surfaces (`blur-glass` token).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backdrop {
    None,
    /// Acrylic (transient window) — Windows 11 22H2+.
    Acrylic,
    Mica,
}

/// Pick the configured monitor, falling back to the primary one.
pub fn monitor_for(index: usize) -> Monitor {
    let all = monitors();
    all.get(index)
        .copied()
        .or_else(|| all.iter().find(|m| m.primary).copied())
        .or_else(|| all.first().copied())
        .unwrap_or(Monitor { work_area: Rect::new(0, 0, 1920, 1080), scale: 1.0, primary: true })
}
