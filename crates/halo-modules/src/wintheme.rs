//! M17 — Windows light/dark switch + per-mode wallpaper and accent, with optional
//! schedule (fixed times or sunrise/sunset from manually entered coordinates — no
//! location service). Registry paths are parameters so tests use a scratch key.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Datelike, Local, NaiveTime, Timelike};
use halo_core::config::Config;
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use halo_platform::registry;

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "wintheme",
    cdc: "M17",
    name_key: "module.wintheme",
    category: Category::System,
    icon: "sun-moon",
    permissions: &[Permission::Registry],
    opt_in: false,
    default_enabled: true,
};

pub const PERSONALIZE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
pub const DWM: &str = "Software\\Microsoft\\Windows\\DWM";

/// Default wallpapers shipped with Windows (11: "Bloom" light/dark; 10: hero image).
pub fn default_wallpaper(light: bool) -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("Web\\Wallpaper\\Windows");
    let candidates: &[&str] = if light { &["img0.jpg"] } else { &["img19.jpg", "img0.jpg"] };
    candidates.iter().map(|c| base.join(c)).find(|p| p.exists())
}

pub fn is_light(key: &str) -> Option<bool> {
    registry::get_dword(key, "AppsUseLightTheme").map(|v| v == 1)
}

/// Write the mode for apps + system shell. Returns false on registry failure.
pub fn write_mode(key: &str, light: bool) -> bool {
    let v = light as u32;
    registry::set_dword(key, "AppsUseLightTheme", v) && registry::set_dword(key, "SystemUsesLightTheme", v)
}

/// `#RRGGBB` → DWM ABGR DWORD.
pub fn accent_abgr(hex: &str) -> Option<u32> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let (r, g, b) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
    Some(0xFF00_0000 | (b << 16) | (g << 8) | r)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub light: bool,
    pub wallpaper: Option<PathBuf>,
    pub accent: Option<String>,
}

/// Apply a full switch on the real system (theme + wallpaper + accent) and notify apps.
///
/// Order matters: changing the wallpaper makes Explorer re-read its theme state, so it
/// goes first; the light/dark values are written last and announced right after.
/// Writing the mode before the wallpaper left the taskbar one switch behind (dark
/// taskbar in light mode and vice versa).
pub fn apply(plan: &Plan) -> bool {
    if let Some(w) = &plan.wallpaper {
        if w.exists() {
            halo_platform::set_wallpaper(w);
        }
    }
    if let Some(hex) = plan.accent.as_deref() {
        if let Some(abgr) = accent_abgr(hex) {
            registry::set_dword(DWM, "AccentColor", abgr);
            registry::set_dword(DWM, "ColorizationColor", abgr);
        }
    }
    let ok = write_mode(PERSONALIZE, plan.light);
    halo_platform::broadcast_setting_change("ImmersiveColorSet");
    // The taskbar sometimes handles the first notification before the wallpaper
    // change settles: announce again shortly after (idempotent).
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(600));
        halo_platform::broadcast_setting_change("ImmersiveColorSet");
    });
    ok
}

pub fn plan_for(config: &Config, light: bool) -> Plan {
    let s = &config.settings;
    let (wp, acc) = if light { (&s.wallpaper_light, &s.accent_light) } else { (&s.wallpaper_dark, &s.accent_dark) };
    Plan {
        light,
        wallpaper: if wp.is_empty() { default_wallpaper(light) } else { Some(PathBuf::from(wp)) },
        accent: (!acc.is_empty()).then(|| acc.clone()),
    }
}

// ── schedule ────────────────────────────────────────────────────────────────

/// Sunrise/sunset (local minutes after midnight) — NOAA approximation, ±2 min.
pub fn sun_times(day_of_year: u32, lat: f64, lon: f64, utc_offset_h: f64) -> Option<(u32, u32)> {
    let g = 2.0 * std::f64::consts::PI / 365.0 * (day_of_year as f64 - 1.0);
    let eqtime = 229.18 * (0.000075 + 0.001868 * g.cos() - 0.032077 * g.sin() - 0.014615 * (2.0 * g).cos() - 0.040849 * (2.0 * g).sin());
    let decl = 0.006918 - 0.399912 * g.cos() + 0.070257 * g.sin() - 0.006758 * (2.0 * g).cos() + 0.000907 * (2.0 * g).sin()
        - 0.002697 * (3.0 * g).cos()
        + 0.00148 * (3.0 * g).sin();
    let lat_r = lat.to_radians();
    let cos_ha = (90.833f64.to_radians().cos() / (lat_r.cos() * decl.cos())) - lat_r.tan() * decl.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None; // polar day/night
    }
    let ha = cos_ha.acos().to_degrees();
    let noon = 720.0 - 4.0 * lon - eqtime + utc_offset_h * 60.0;
    let rise = (noon - 4.0 * ha).rem_euclid(1440.0);
    let set = (noon + 4.0 * ha).rem_euclid(1440.0);
    Some((rise.round() as u32, set.round() as u32))
}

/// Should it be light at `minute` (0..1440) given light/dark start minutes?
pub fn light_at(minute: u32, light_from: u32, dark_from: u32) -> bool {
    if light_from <= dark_from {
        minute >= light_from && minute < dark_from
    } else {
        minute >= light_from || minute < dark_from
    }
}

fn hhmm(s: &str) -> Option<u32> {
    let t = NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()?;
    Some(t.hour() * 60 + t.minute())
}

/// Desired mode now according to the config schedule, or None when manual.
pub fn scheduled_mode(config: &Config) -> Option<bool> {
    let s = &config.settings;
    let now = Local::now();
    let minute = now.hour() * 60 + now.minute();
    match s.wintheme_schedule.as_str() {
        "times" => Some(light_at(minute, hhmm(&s.wintheme_light_at)?, hhmm(&s.wintheme_dark_at)?)),
        "sun" => {
            let off = now.offset().local_minus_utc() as f64 / 3600.0;
            let (rise, set) = sun_times(now.ordinal(), s.wintheme_lat, s.wintheme_lon, off)?;
            Some(light_at(minute, rise, set))
        }
        _ => None,
    }
}

#[derive(Clone, Default)]
pub struct WinThemeHandle {
    config: Arc<Mutex<Option<Config>>>,
}

impl WinThemeHandle {
    /// Manual toggle from the UI (notch tab, hub button, launcher).
    pub fn toggle(&self) -> Option<bool> {
        let cfg = self.config.lock().ok()?.clone()?;
        let light = !is_light(PERSONALIZE).unwrap_or(false);
        apply(&plan_for(&cfg, light));
        Some(light)
    }

    pub fn set(&self, light: bool) {
        if let Some(cfg) = self.config.lock().ok().and_then(|c| c.clone()) {
            apply(&plan_for(&cfg, light));
        }
    }
}

pub struct WinTheme {
    handle: WinThemeHandle,
    worker: Option<Worker>,
}

impl WinTheme {
    pub fn new(config: &Config) -> Self {
        Self { handle: WinThemeHandle { config: Arc::new(Mutex::new(Some(config.clone()))) }, worker: None }
    }
    pub fn handle(&self) -> WinThemeHandle {
        self.handle.clone()
    }
}

impl Module for WinTheme {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn configure(&mut self, config: &Config) -> bool {
        if let Ok(mut c) = self.handle.config.lock() {
            *c = Some(config.clone());
        }
        false
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        // Schedule check every minute; only writes when the wanted mode differs.
        let cfg = self.handle.config.clone();
        self.worker = Some(Worker::every("halo-wintheme", Duration::from_secs(60), move || {
            let Some(c) = cfg.lock().ok().and_then(|c| c.clone()) else { return true };
            if let Some(want) = scheduled_mode(&c) {
                if is_light(PERSONALIZE) != Some(want) {
                    apply(&plan_for(&c, want));
                }
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
    }
}

pub fn wallpaper_exists(p: &str) -> bool {
    !p.is_empty() && Path::new(p).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrip_on_scratch_key() {
        let key = format!("Software\\Halo\\TestTheme-{}", std::process::id());
        assert!(write_mode(&key, true));
        assert_eq!(is_light(&key), Some(true));
        assert!(write_mode(&key, false));
        assert_eq!(is_light(&key), Some(false));
        registry::delete_tree(&key);
    }

    #[test]
    fn accent_conversion() {
        assert_eq!(accent_abgr("#818CF8"), Some(0xFFF8_8C81));
        assert_eq!(accent_abgr("nope"), None);
    }

    #[test]
    fn schedules() {
        assert!(light_at(8 * 60, 7 * 60, 19 * 60));
        assert!(!light_at(20 * 60, 7 * 60, 19 * 60));
        assert!(light_at(23 * 60, 22 * 60, 6 * 60), "wraps midnight");
        // Paris, 21 June (day 172), UTC+2: sunrise ≈ 05:47, sunset ≈ 21:58.
        let (rise, set) = sun_times(172, 48.8566, 2.3522, 2.0).unwrap();
        assert!((rise as i32 - (5 * 60 + 47)).abs() <= 6, "rise {rise}");
        assert!((set as i32 - (21 * 60 + 58)).abs() <= 6, "set {set}");
        assert!(sun_times(172, 80.0, 0.0, 0.0).is_none(), "polar day");
    }
}
