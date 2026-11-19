//! Static catalogue of every planned module (spec §3.1, §11), so the UI can list
//! modules, their phase, opt-in status and required permissions before they exist.

use halo_core::module::{Category, Permission};

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub cdc: &'static str,
    pub id: &'static str,
    pub name_fr: &'static str,
    pub name_en: &'static str,
    pub category: Category,
    /// Lucide icon name (gpui-kit `IconName` kebab-case).
    pub icon: &'static str,
    pub phase: u8,
    pub opt_in: bool,
    pub permissions: &'static [Permission],
}

use Category::*;
use Permission::*;

pub const CATALOG: &[Entry] = &[
    Entry { cdc: "M1", id: "overlay", name_fr: "Overlay Halo", name_en: "Halo overlay", category: Core, icon: "circle-dot", phase: 0, opt_in: false, permissions: &[] },
    Entry { cdc: "M2", id: "system", name_fr: "Moniteurs système", name_en: "System monitors", category: Monitoring, icon: "activity", phase: 1, opt_in: false, permissions: &[] },
    Entry { cdc: "M3", id: "resources", name_fr: "Ressources par processus", name_en: "Per-process resources", category: Monitoring, icon: "cpu", phase: 3, opt_in: false, permissions: &[] },
    Entry { cdc: "M4", id: "activity", name_fr: "Suivi d'activité", name_en: "Activity tracking", category: Monitoring, icon: "chart-pie", phase: 2, opt_in: true, permissions: &[ForegroundWindow] },
    Entry { cdc: "M5", id: "netlimit", name_fr: "Limiteur réseau", name_en: "Network limiter", category: Network, icon: "globe", phase: 5, opt_in: true, permissions: &[Elevation, Driver] },
    Entry { cdc: "M6", id: "notes", name_fr: "Notes", name_en: "Notes", category: Productivity, icon: "notebook-pen", phase: 2, opt_in: false, permissions: &[] },
    Entry { cdc: "M7", id: "clipboard", name_fr: "Presse-papiers", name_en: "Clipboard", category: Productivity, icon: "clipboard", phase: 2, opt_in: true, permissions: &[ClipboardRead] },
    Entry { cdc: "M8", id: "accent", name_fr: "Quick Accent", name_en: "Quick Accent", category: Productivity, icon: "languages", phase: 1, opt_in: false, permissions: &[KeyboardHook] },
    Entry { cdc: "M9", id: "images", name_fr: "Images", name_en: "Images", category: Files, icon: "image", phase: 2, opt_in: false, permissions: &[] },
    Entry { cdc: "M10", id: "vault", name_fr: "Coffre-fort", name_en: "Vault", category: Security, icon: "lock", phase: 5, opt_in: true, permissions: &[] },
    Entry { cdc: "M11", id: "picker", name_fr: "Color picker", name_en: "Color picker", category: Tools, icon: "pipette", phase: 1, opt_in: false, permissions: &[ScreenCapture] },
    Entry { cdc: "M12", id: "screenshot", name_fr: "Capture d'écran", name_en: "Screenshot", category: Capture, icon: "camera", phase: 4, opt_in: false, permissions: &[ScreenCapture] },
    Entry { cdc: "M13", id: "recorder", name_fr: "Capture vidéo", name_en: "Screen recorder", category: Capture, icon: "video", phase: 4, opt_in: false, permissions: &[ScreenCapture, Microphone] },
    Entry { cdc: "M14", id: "audiorec", name_fr: "Enregistreur audio", name_en: "Audio recorder", category: Capture, icon: "mic", phase: 4, opt_in: false, permissions: &[Microphone] },
    Entry { cdc: "M15", id: "music", name_fr: "Musique", name_en: "Music", category: Media, icon: "music", phase: 3, opt_in: false, permissions: &[] },
    Entry { cdc: "M16", id: "spotlight", name_fr: "Spotlight souris", name_en: "Mouse spotlight", category: Comfort, icon: "mouse-pointer-2", phase: 1, opt_in: false, permissions: &[] },
    Entry { cdc: "M17", id: "wintheme", name_fr: "Thème Windows", name_en: "Windows theme", category: System, icon: "sun-moon", phase: 3, opt_in: false, permissions: &[Registry] },
    Entry { cdc: "M18", id: "autostart", name_fr: "Démarrage auto", name_en: "Launch at startup", category: System, icon: "power", phase: 1, opt_in: false, permissions: &[Registry] },
    Entry { cdc: "M19", id: "themes", name_fr: "Thèmes", name_en: "Themes", category: Personalization, icon: "palette", phase: 1, opt_in: false, permissions: &[] },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_m1_to_m19_once() {
        assert_eq!(CATALOG.len(), 19);
        for (i, e) in CATALOG.iter().enumerate() {
            assert_eq!(e.cdc, format!("M{}", i + 1));
        }
    }

    #[test]
    fn sensitive_modules_are_opt_in() {
        // spec §7: M4, M5, M7, M10 opt-in.
        for id in ["M4", "M5", "M7", "M10"] {
            assert!(CATALOG.iter().find(|e| e.cdc == id).unwrap().opt_in, "{id}");
        }
    }

    #[test]
    fn no_keylogging_permission_outside_quick_accent() {
        for e in CATALOG {
            if e.permissions.contains(&KeyboardHook) {
                assert_eq!(e.cdc, "M8");
            }
        }
    }
}
