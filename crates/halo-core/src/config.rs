//! Typed configuration stored in `%APPDATA%\Halo\config.toml` (spec §8). Local only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;
use crate::overlay::{Anchor, AutoHide, ExpandOn};

/// 2: adds the "controls" card (volume, brightness, Wi-Fi) to the open notch.
pub const CONFIG_VERSION: u32 = 2;

/// App theme preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    /// Follow Windows `AppsUseLightTheme`.
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlayConfig {
    pub anchor: Anchor,
    /// Monitor index (0 = primary). Missing monitor → primary.
    pub display: usize,
    pub auto_hide: AutoHide,
    pub expand_on: ExpandOn,
    /// Pointer distance (logical px) that counts as "approaching" the pill.
    pub proximity_px: f32,
    /// Indicators shown in the compact pill, in order.
    pub indicators: Vec<String>,
    /// Let clicks pass through the transparent shadow margin around the pill
    /// (WS_EX_LAYERED|WS_EX_TRANSPARENT, toggled from a cursor hit-test).
    /// Verified compatible with GPUI's DirectComposition rendering.
    pub click_through: bool,
    /// Notch theme id (see halo_ui::tokens::NOTCH_THEMES).
    pub notch_theme: String,
    /// Cards of the open notch, in order: media, today, focus, system, clock.
    pub cards: Vec<String>,
    /// Shortcuts in the open notch's bottom bar, in order.
    pub tabs: Vec<String>,
    /// Show a volume / brightness HUD in the notch when they change.
    pub hud: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            anchor: Anchor::default(),
            display: 0,
            auto_hide: AutoHide::default(),
            expand_on: ExpandOn::default(),
            proximity_px: 48.0,
            indicators: vec!["clock".into(), "cpu".into(), "battery".into()],
            click_through: true,
            notch_theme: "midnight".into(),
            cards: ["media", "today", "focus", "controls"].map(String::from).to_vec(),
            // Settings live behind the gear in the dashboard header.
            tabs: ["screenshot", "picker", "notes", "clipboard", "system", "dnd"].map(String::from).to_vec(),
            hud: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeyConfig {
    /// Show / expand / collapse the overlay.
    pub toggle_overlay: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self { toggle_overlay: "Ctrl+Alt+H".into() }
    }
}

/// Per-module settings (kept flat and primitive so `halo-core` stays independent
/// of module crates).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModuleSettings {
    /// M16: 1 (hard) … 5 (easy).
    pub spotlight_sensitivity: u8,
    /// M16: hole radius (logical px) and dim opacity (%).
    pub spotlight_radius: f32,
    pub spotlight_dim: u8,
    /// M8: activation key: "space" | "arrows" | "both".
    pub accent_activation: String,
    /// M8: minimum hold (ms) before the activation key opens the chooser.
    pub accent_delay_ms: u32,
    /// M8: all Latin variants instead of French-first.
    pub accent_all_languages: bool,
    /// M11 global shortcut.
    pub picker_hotkey: String,
    /// M19: icon set (only "lucide" is bundled with gpui-kit).
    pub icon_set: String,
    /// M7: max entries kept (pinned ones never evicted).
    pub clip_capacity: u32,
    /// M7: encrypt stored entries with DPAPI.
    pub clip_encrypt: bool,
    /// M7: delete unpinned entries older than N minutes (0 = never).
    pub clip_autoclear_minutes: u32,
    pub clip_images: bool,
    /// M7: never record copies made from these executables.
    pub clip_exclude_apps: Vec<String>,
    /// M4: also record window titles (off by default — titles can be private).
    pub activity_titles: bool,
    /// M4: apps never tracked.
    pub activity_exclude_apps: Vec<String>,
    /// M4: stop counting after N seconds without input.
    pub activity_idle_secs: u32,
    /// M4: user categories, `exe → category`.
    pub activity_categories: std::collections::BTreeMap<String, String>,
    /// Productivity mode: Pomodoro durations (minutes).
    pub focus_minutes: u32,
    pub break_minutes: u32,
    /// Productivity mode: apps that trigger a gentle notch reminder during focus.
    pub distraction_apps: Vec<String>,
    /// Pause media when a focus session ends.
    pub focus_pause_media: bool,
    /// M13 screen recording.
    pub capture_fps: u32,
    pub capture_mic: bool,
    pub capture_system: bool,
    /// M13: draw the mouse pointer in recordings.
    pub capture_cursor: bool,
    /// M13: click highlight in recordings: "off" | "ring" | "flash".
    pub capture_clicks: String,
    /// Microphone used by recordings (device name, "" = Windows default).
    pub mic_device: String,
    /// M14 audio recorder.
    pub audio_mic: bool,
    pub audio_system: bool,
    /// "wav" | "flac".
    pub audio_format: String,
    /// Do Not Disturb: manual switch, automatic during focus sessions, and schedule.
    pub dnd_manual: bool,
    pub dnd_during_focus: bool,
    pub dnd_scheduled: bool,
    pub dnd_from: String,
    pub dnd_to: String,
    /// M17: "off" | "times" | "sun".
    pub wintheme_schedule: String,
    pub wintheme_light_at: String,
    pub wintheme_dark_at: String,
    /// M17 sun schedule: coordinates entered by hand (no location service).
    pub wintheme_lat: f64,
    pub wintheme_lon: f64,
    /// M17: wallpaper/accent per mode ("" = Windows default / unchanged).
    pub wallpaper_light: String,
    pub wallpaper_dark: String,
    pub accent_light: String,
    pub accent_dark: String,
    /// M9 defaults.
    pub images_quality: u8,
    pub images_format: String,
    pub images_max_px: u32,
    /// File shelf: paths dropped on the notch (shortcuts only, files are not moved).
    pub shelf: Vec<String>,
    /// Look for a new version on GitHub at startup and once a day.
    pub update_check: bool,
}

/// Most files kept on the shelf; the oldest drop off first.
pub const SHELF_MAX: usize = 24;

impl ModuleSettings {
    /// Add dropped paths to the front of the shelf (deduplicated, capped at
    /// [`SHELF_MAX`]). Returns how many were new.
    pub fn shelf_add<I: IntoIterator<Item = String>>(&mut self, paths: I) -> usize {
        let mut added = 0;
        for p in paths {
            let before = self.shelf.len();
            self.shelf.retain(|x| !x.eq_ignore_ascii_case(&p));
            if self.shelf.len() == before {
                added += 1;
            }
            self.shelf.insert(0, p);
        }
        self.shelf.truncate(SHELF_MAX);
        added
    }
}

impl Default for ModuleSettings {
    fn default() -> Self {
        Self {
            spotlight_sensitivity: 3,
            spotlight_radius: 110.0,
            spotlight_dim: 60,
            accent_activation: "space".into(),
            accent_delay_ms: 300,
            accent_all_languages: false,
            picker_hotkey: "Ctrl+Alt+C".into(),
            icon_set: "lucide".into(),
            clip_capacity: 500,
            clip_encrypt: false,
            clip_autoclear_minutes: 0,
            clip_images: true,
            clip_exclude_apps: ["KeePass.exe", "KeePassXC.exe", "1Password.exe", "Bitwarden.exe", "Dashlane.exe"]
                .map(String::from)
                .to_vec(),
            activity_titles: false,
            activity_exclude_apps: Vec::new(),
            activity_idle_secs: 300,
            activity_categories: Default::default(),
            focus_minutes: 25,
            break_minutes: 5,
            distraction_apps: ["Discord.exe", "steam.exe", "EpicGamesLauncher.exe", "WhatsApp.exe", "Netflix.exe", "Telegram.exe"]
                .map(String::from)
                .to_vec(),
            focus_pause_media: true,
            capture_fps: 30,
            capture_mic: false,
            capture_system: true,
            capture_cursor: true,
            capture_clicks: "ring".into(),
            mic_device: String::new(),
            audio_mic: true,
            audio_system: false,
            audio_format: "wav".into(),
            dnd_manual: false,
            dnd_during_focus: true,
            dnd_scheduled: false,
            dnd_from: "22:00".into(),
            dnd_to: "07:00".into(),
            wintheme_schedule: "off".into(),
            wintheme_light_at: "07:00".into(),
            wintheme_dark_at: "19:00".into(),
            wintheme_lat: 48.8566,
            wintheme_lon: 2.3522,
            wallpaper_light: String::new(),
            wallpaper_dark: String::new(),
            accent_light: String::new(),
            accent_dark: String::new(),
            images_quality: 82,
            images_format: "webp".into(),
            images_max_px: 1920,
            shelf: Vec::new(),
            update_check: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub lang: Lang,
    pub theme: ThemePref,
    pub overlay: OverlayConfig,
    pub hotkeys: HotkeyConfig,
    /// Module id → enabled. Absent = module default (sensitive modules default to off).
    pub modules: BTreeMap<String, bool>,
    pub settings: ModuleSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            lang: Lang::default(),
            theme: ThemePref::default(),
            overlay: OverlayConfig::default(),
            hotkeys: HotkeyConfig::default(),
            modules: BTreeMap::new(),
            settings: ModuleSettings::default(),
        }
    }
}

impl Config {
    /// `%APPDATA%\Halo`, or `%HALO_HOME%` when set (tests/portable use).
    pub fn dir() -> PathBuf {
        if let Some(home) = std::env::var_os("HALO_HOME") {
            return PathBuf::from(home);
        }
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Halo")
    }

    pub fn default_path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Load, or return defaults if the file is missing. A corrupt file is backed up
    /// to `config.toml.bak` and replaced by defaults rather than blocking startup.
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut cfg) => {
                    cfg.migrate();
                    cfg
                }
                Err(err) => {
                    log::error!("invalid config {}: {err} — using defaults", path.display());
                    let _ = std::fs::copy(path, path.with_extension("toml.bak"));
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Bring an older config up to [`CONFIG_VERSION`] (additive changes only).
    pub fn migrate(&mut self) {
        if self.version < 2 && !self.overlay.cards.iter().any(|c| c == "controls") {
            self.overlay.cards.push("controls".into());
        }
        self.version = self.version.max(CONFIG_VERSION);
    }

    /// Atomic save (write temp + rename).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let text = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
        Ok(())
    }

    pub fn module_enabled(&self, id: &str, default: bool) -> bool {
        self.modules.get(id).copied().unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("halo-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn v1_configs_gain_the_controls_card_once() {
        let mut c: Config = toml::from_str("version = 1\n[overlay]\ncards = [\"media\", \"focus\"]\n").unwrap();
        c.migrate();
        assert_eq!(c.overlay.cards, ["media", "focus", "controls"]);
        assert_eq!(c.version, CONFIG_VERSION);
        // A user who later removes it keeps it removed.
        c.overlay.cards.retain(|x| x != "controls");
        c.migrate();
        assert_eq!(c.overlay.cards, ["media", "focus"]);
    }

    #[test]
    fn shelf_dedupes_moves_to_front_and_caps() {
        let mut s = ModuleSettings::default();
        assert_eq!(s.shelf_add(["C:\\a.pdf".to_string(), "C:\\b.png".to_string()]), 2);
        assert_eq!(s.shelf, ["C:\\b.png", "C:\\a.pdf"]);
        assert_eq!(s.shelf_add(["c:\\A.PDF".to_string()]), 0, "same file, different case");
        assert_eq!(s.shelf[0], "c:\\A.PDF");
        assert_eq!(s.shelf.len(), 2);
        s.shelf_add((0..40).map(|i| format!("C:\\f{i}")));
        assert_eq!(s.shelf.len(), SHELF_MAX);
        assert_eq!(s.shelf[0], "C:\\f39");
    }

    #[test]
    fn roundtrip() {
        let path = tmp("roundtrip");
        let mut cfg = Config::default();
        cfg.overlay.anchor = Anchor::RightCenter;
        cfg.overlay.auto_hide = AutoHide::AlwaysVisible;
        cfg.modules.insert("activity".into(), true);
        cfg.save(&path).unwrap();
        assert_eq!(Config::load_or_default(&path), cfg);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg: Config = toml::from_str("[overlay]\nanchor = \"bottom-left\"\n").unwrap();
        assert_eq!(cfg.overlay.anchor, Anchor::BottomLeft);
        assert_eq!(cfg.overlay.auto_hide, AutoHide::default());
        assert_eq!(cfg.hotkeys.toggle_overlay, "Ctrl+Alt+H");
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let path = tmp("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "this is = = not toml").unwrap();
        assert_eq!(Config::load_or_default(&path), Config::default());
        assert!(path.with_extension("toml.bak").exists());
    }
}
