//! Halo design tokens — the single source of truth for the visual language.
//!
//! Mirrors `design/DESIGN-SYSTEM.md` v0.3 (formalised from charte graphique v0.2).
//! `themes/halo.json` (gpui-kit) is derived from these values; a test keeps them in sync.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// sRGB colour with straight alpha, stored as `0xRRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub u32);

impl Color {
    /// Opaque colour from `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Self {
        Self((rgb << 8) | 0xFF)
    }

    /// Colour from `0xRRGGBB` and an alpha percentage (0–100).
    pub const fn hex_a(rgb: u32, alpha_pct: u32) -> Self {
        let a = (alpha_pct * 255 + 50) / 100;
        Self((rgb << 8) | a)
    }

    pub const fn r(self) -> u8 {
        (self.0 >> 24) as u8
    }
    pub const fn g(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub const fn b(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub const fn a(self) -> u8 {
        self.0 as u8
    }

    /// Same RGB, new alpha percentage.
    pub const fn with_alpha_pct(self, alpha_pct: u32) -> Self {
        Self::hex_a(self.0 >> 8, alpha_pct)
    }

    /// `#RRGGBB` or `#RRGGBBAA` (when not opaque).
    pub fn to_css(self) -> String {
        if self.a() == 0xFF {
            format!("#{:06X}", self.0 >> 8)
        } else {
            format!("#{:08X}", self.0)
        }
    }

    /// Alpha-composite `self` over an opaque `below`.
    pub fn over(self, below: Color) -> Color {
        let a = self.a() as f32 / 255.0;
        let mix = |top: u8, bot: u8| (top as f32 * a + bot as f32 * (1.0 - a)).round() as u32;
        Color::hex((mix(self.r(), below.r()) << 16) | (mix(self.g(), below.g()) << 8) | mix(self.b(), below.b()))
    }

    /// WCAG 2.1 relative luminance (alpha ignored).
    pub fn luminance(self) -> f32 {
        fn channel(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * channel(self.r()) + 0.7152 * channel(self.g()) + 0.0722 * channel(self.b())
    }

    /// WCAG 2.1 contrast ratio between two opaque colours.
    pub fn contrast(self, other: Color) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

/// The one accent. Flat, never in a gradient.
pub const ACCENT: Color = Color::hex(0x818CF8);

/// Full colour palette for one mode. Field names match `DESIGN-SYSTEM.md` (`surface-2` → `surface_2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub mode: Mode,
    pub bg: Color,
    pub surface: Color,
    pub surface_2: Color,
    pub surface_3: Color,
    pub ink: Color,
    pub ink_2: Color,
    pub ink_3: Color,
    pub line: Color,
    pub line_2: Color,
    pub glass: Color,

    pub accent: Color,
    pub accent_soft: Color,
    pub accent_text: Color,
    pub on_accent: Color,

    pub success: Color,
    pub success_text: Color,
    pub warning: Color,
    pub warning_text: Color,
    pub danger: Color,
    pub danger_text: Color,
    pub info: Color,
    pub info_text: Color,

    /// Overlay notch — identical in both modes (always pure black, like a hardware notch).
    pub overlay_bg: Color,
    pub overlay_ink: Color,
    pub overlay_ink_2: Color,
    pub overlay_line: Color,
    pub overlay_sep: Color,
    pub overlay_tile: Color,
    pub overlay_track: Color,
}

const SUCCESS: Color = Color::hex(0x34D399);
const WARNING: Color = Color::hex(0xFBBF24);
const DANGER: Color = Color::hex(0xF87171);
const INFO: Color = Color::hex(0x38BDF8);

impl Palette {
    pub const fn dark() -> Self {
        Self {
            mode: Mode::Dark,
            // v0.5 Fluent: Windows 11 dark neutrals (solid base #202020 under Mica,
            // card layer #2B2B2B, subtle fills, text primary / secondary / tertiary).
            bg: Color::hex(0x202020),
            surface: Color::hex(0x2B2B2B),
            surface_2: Color::hex(0x323232),
            surface_3: Color::hex(0x3D3D3D),
            ink: Color::hex(0xFFFFFF),
            ink_2: Color::hex(0xC5C5C5),
            ink_3: Color::hex(0x9E9E9E),
            line: Color::hex_a(0xFFFFFF, 7),
            line_2: Color::hex_a(0xFFFFFF, 12),
            glass: Color::hex_a(0x141414, 72),
            accent: ACCENT,
            accent_soft: ACCENT.with_alpha_pct(16),
            accent_text: ACCENT,
            on_accent: Color::hex(0x0A0A0A),
            success: SUCCESS,
            success_text: SUCCESS,
            warning: WARNING,
            warning_text: WARNING,
            danger: DANGER,
            danger_text: DANGER,
            info: INFO,
            info_text: INFO,
            ..Self::OVERLAY
        }
    }

    pub const fn light() -> Self {
        Self {
            mode: Mode::Light,
            // v0.5 Fluent: Windows 11 light neutrals (base #F3F3F3, card #FBFBFB).
            bg: Color::hex(0xF3F3F3),
            surface: Color::hex(0xFBFBFB),
            surface_2: Color::hex(0xEAEAEA),
            surface_3: Color::hex(0xE0E0E0),
            ink: Color::hex(0x1A1A1A),
            ink_2: Color::hex(0x5F5F5F),
            ink_3: Color::hex(0x707070),
            line: Color::hex_a(0x000000, 8),
            line_2: Color::hex_a(0x000000, 14),
            glass: Color::hex_a(0xFFFFFF, 72),
            accent: ACCENT,
            accent_soft: ACCENT.with_alpha_pct(16),
            // v0.3: #818CF8 as text on light is 2.64:1; same hue, denser, still flat.
            accent_text: Color::hex(0x4F46E5),
            on_accent: Color::hex(0x0A0A0A),
            success: SUCCESS,
            success_text: Color::hex(0x047857),
            warning: WARNING,
            warning_text: Color::hex(0xA84E08),
            danger: DANGER,
            danger_text: Color::hex(0xB91C1C),
            info: INFO,
            info_text: Color::hex(0x0369A1),
            ..Self::OVERLAY
        }
    }

    pub const fn for_mode(mode: Mode) -> Self {
        match mode {
            Mode::Dark => Self::dark(),
            Mode::Light => Self::light(),
        }
    }

    /// Base used only to share the overlay tokens between modes.
    const OVERLAY: Self = Self {
        mode: Mode::Dark,
        bg: Color(0),
        surface: Color(0),
        surface_2: Color(0),
        surface_3: Color(0),
        ink: Color(0),
        ink_2: Color(0),
        ink_3: Color(0),
        line: Color(0),
        line_2: Color(0),
        glass: Color(0),
        accent: ACCENT,
        accent_soft: Color(0),
        accent_text: Color(0),
        on_accent: Color(0),
        success: SUCCESS,
        success_text: SUCCESS,
        warning: WARNING,
        warning_text: WARNING,
        danger: DANGER,
        danger_text: DANGER,
        info: INFO,
        info_text: INFO,
        overlay_bg: Color::hex(0x000000),
        overlay_ink: Color::hex(0xE7EBF4),
        overlay_ink_2: Color::hex(0xCFD6E6),
        overlay_line: Color::hex_a(0xFFFFFF, 10),
        overlay_sep: Color::hex_a(0xFFFFFF, 16),
        overlay_tile: Color::hex_a(0xFFFFFF, 7),
        overlay_track: Color::hex_a(0xFFFFFF, 12),
    };
}

// ── Spacing (base 4) ─────────────────────────────────────────────────────────
pub const SPACE_1: f32 = 4.0;
pub const SPACE_2: f32 = 8.0;
pub const SPACE_3: f32 = 12.0;
pub const SPACE_4: f32 = 16.0;
pub const SPACE_6: f32 = 24.0;
pub const SPACE_8: f32 = 32.0;

// ── Radii ────────────────────────────────────────────────────────────────────
// v0.5 Fluent: 4 px for in-page elements (controls, cards, list items), 8 px for
// top-level surfaces (windows, flyouts, dialogs).
pub const RADIUS_SM: f32 = 4.0;
pub const RADIUS_MD: f32 = 4.0;
pub const RADIUS_LG: f32 = 8.0;
pub const RADIUS_XL: f32 = 12.0;
pub const RADIUS_PILL: f32 = 999.0;

pub const ICON_STROKE: f32 = 1.7;
pub const ICON_SM: f32 = 15.0;
pub const ICON_MD: f32 = 18.0;

// ── Depth ────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    pub offset_y: f32,
    pub blur: f32,
    pub spread: f32,
}

pub const SHADOW_FLOAT: Shadow = Shadow {
    color: Color::hex_a(0x000000, 60),
    offset_y: 12.0,
    blur: 30.0,
    spread: -8.0,
};

// ── Motion ───────────────────────────────────────────────────────────────────
pub const MOTION_FAST: Duration = Duration::from_millis(120);
pub const MOTION_BASE: Duration = Duration::from_millis(200);
pub const MOTION_SLOW: Duration = Duration::from_millis(240);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringSpec {
    /// Approximate period of the undamped oscillation, seconds.
    pub response: f32,
    /// Damping ratio (1.0 = critical).
    pub damping: f32,
}

/// Reaches 99 % of its target in ≈ 240 ms with < 1 % overshoot (charter: "ressort 180–240 ms").
pub const MOTION_SPRING: SpringSpec = SpringSpec { response: 0.28, damping: 0.86 };

/// Seconds of inactivity before the notch slims down (spec M1 default).
pub const OVERLAY_IDLE_SECS: u32 = 5;
/// "Presence" of the idle notch (0 = hidden, 1 = full). Kept for the auto-hide state
/// machine; the notch renders it as a slim bar rather than transparency.
pub const OVERLAY_FADED_OPACITY: f32 = 0.35;

// ── Overlay notch metrics (logical px) ───────────────────────────────────────
/// The overlay is a solid black notch attached to a screen edge (v0.4): square on the
/// edge side, rounded inner corners, concave "ears" blending into the edge.
/// `along` = size parallel to the edge, `depth` = size away from the edge.
pub mod overlay {
    use super::*;

    pub const COMPACT_DEPTH: f32 = 32.0;
    pub const COMPACT_V_DEPTH: f32 = 40.0;
    pub const EXPANDED_ALONG: f32 = 760.0;
    pub const EXPANDED_DEPTH: f32 = 252.0;
    pub const EXPANDED_V_ALONG: f32 = 340.0;
    pub const EXPANDED_V_DEPTH: f32 = 84.0;
    /// Idle notch collapses to a slim bar.
    pub const SLIM_ALONG: f32 = 104.0;
    pub const SLIM_DEPTH: f32 = 6.0;
    /// Concave fillet joining the notch to the screen edge.
    pub const EAR: f32 = 12.0;
    pub const RADIUS_COMPACT: f32 = 12.0;
    pub const RADIUS_EXPANDED: f32 = 28.0;
    /// Offset from the screen corner for corner anchors.
    pub const CORNER_INSET: f32 = SPACE_6;
    pub const PAD_X: f32 = SPACE_4;
    pub const GAP: f32 = 10.0;
    pub const EXPANDED_PAD: f32 = 18.0;
    pub const STATUS_DOT: f32 = 7.0;
    pub const TILE: f32 = 40.0;
    pub const TILE_GAP: f32 = 8.0;
    pub const TILE_RADIUS: f32 = 12.0;
    /// Estimated width of one compact indicator (icon + value) including its gap.
    pub const INDICATOR_W: f32 = GAP + 52.0;
    /// Estimated height of one stacked indicator in the vertical notch.
    pub const INDICATOR_V_H: f32 = 40.0;
}
// ── Notch themes (v0.4) ──────────────────────────────────────────────────────
/// Flat notch variants inspired by Islita's themes, adapted to the charter: background
/// + optional 1 px border only — no gradient, no glow; the accent stays #818CF8.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotchTheme {
    pub id: &'static str,
    pub name_fr: &'static str,
    pub name_en: &'static str,
    pub bg: Color,
    pub border: Option<Color>,
}

pub const NOTCH_THEMES: &[NotchTheme] = &[
    NotchTheme { id: "midnight", name_fr: "Minuit", name_en: "Midnight", bg: Color::hex(0x000000), border: None },
    NotchTheme { id: "graphite", name_fr: "Graphite", name_en: "Graphite", bg: Color::hex(0x1C1C1F), border: Some(Color::hex(0x2E2E33)) },
    NotchTheme { id: "ocean", name_fr: "Océan", name_en: "Ocean", bg: Color::hex(0x06121F), border: Some(Color::hex(0x1D3557)) },
    NotchTheme { id: "forest", name_fr: "Forêt", name_en: "Forest", bg: Color::hex(0x07140D), border: Some(Color::hex(0x1E3A2A)) },
    NotchTheme { id: "sunset", name_fr: "Crépuscule", name_en: "Sunset", bg: Color::hex(0x160B07), border: Some(Color::hex(0x4A2412)) },
    NotchTheme { id: "violet", name_fr: "Violet", name_en: "Violet", bg: Color::hex(0x0E0A18), border: Some(Color::hex(0x2E2352)) },
];

pub fn notch_theme(id: &str) -> NotchTheme {
    NOTCH_THEMES.iter().copied().find(|t| t.id == id).unwrap_or(NOTCH_THEMES[0])
}

// ── Typography ───────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub weight: u16,
    pub line_height: f32,
}

pub const TEXT_DISPLAY: TextStyle = TextStyle { size: 56.0, weight: 600, line_height: 1.15 };
// v0.5 Fluent type ramp: Title 28, Body Strong 16 for card headers, Body 14, Caption 12.
pub const TEXT_H1: TextStyle = TextStyle { size: 28.0, weight: 600, line_height: 1.15 };
pub const TEXT_H2: TextStyle = TextStyle { size: 16.0, weight: 600, line_height: 1.25 };
pub const TEXT_BODY: TextStyle = TextStyle { size: 16.0, weight: 400, line_height: 1.6 };
pub const TEXT_LABEL: TextStyle = TextStyle { size: 14.0, weight: 400, line_height: 1.5 };
pub const TEXT_MICRO: TextStyle = TextStyle { size: 12.0, weight: 400, line_height: 1.4 };
pub const TEXT_PILL: TextStyle = TextStyle { size: 13.0, weight: 500, line_height: 1.2 };

/// Preferred families with Windows fallbacks, most preferred first.
pub const FONT_DISPLAY: &[&str] = &["Space Grotesk", "Segoe UI Variable Display", "Segoe UI"];
pub const FONT_UI: &[&str] = &["Inter", "Segoe UI Variable Text", "Segoe UI"];
pub const FONT_MONO: &[&str] = &["JetBrains Mono", "Cascadia Mono", "Consolas"];

/// Font families resolved against what is actually installed/loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typography {
    pub display: String,
    pub ui: String,
    pub mono: String,
}

impl Typography {
    /// Pick the first available family of each chain.
    pub fn resolve(available: &[String]) -> Self {
        fn pick(chain: &[&str], available: &[String]) -> String {
            chain
                .iter()
                .find(|f| available.iter().any(|a| a.eq_ignore_ascii_case(f)))
                .unwrap_or(chain.last().expect("non-empty chain"))
                .to_string()
        }
        Self {
            display: pick(FONT_DISPLAY, available),
            ui: pick(FONT_UI, available),
            mono: pick(FONT_MONO, available),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AA_TEXT: f32 = 4.5;
    const AA_NON_TEXT: f32 = 3.0;

    fn both() -> [Palette; 2] {
        [Palette::dark(), Palette::light()]
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Color::hex(0x818CF8).to_css(), "#818CF8");
        assert_eq!(Color::hex_a(0xFFFFFF, 8).a(), 20);
        assert_eq!(Color::hex_a(0x0C0E14, 90).to_css(), "#0C0E14E6");
    }

    #[test]
    fn body_text_meets_aa() {
        for p in both() {
            for surface in [p.bg, p.surface, p.surface_2] {
                assert!(p.ink.contrast(surface) >= AA_TEXT, "{:?} ink", p.mode);
                assert!(p.ink_2.contrast(surface) >= AA_TEXT, "{:?} ink_2 on {}", p.mode, surface.to_css());
            }
        }
    }

    #[test]
    fn decorative_text_meets_non_text_minimum() {
        for p in both() {
            for surface in [p.bg, p.surface] {
                let r = p.ink_3.contrast(surface);
                assert!(r >= AA_NON_TEXT, "{:?} ink_3 on {} = {r}", p.mode, surface.to_css());
            }
        }
    }

    #[test]
    fn accent_and_semantic_text_meet_aa() {
        for p in both() {
            for c in [p.accent_text, p.success_text, p.warning_text, p.danger_text, p.info_text] {
                for surface in [p.bg, p.surface] {
                    let r = c.contrast(surface);
                    assert!(r >= AA_TEXT, "{:?} {} on {} = {r}", p.mode, c.to_css(), surface.to_css());
                }
            }
            assert!(p.on_accent.contrast(p.accent) >= AA_TEXT);
        }
    }

    #[test]
    fn overlay_is_identical_and_readable_in_both_modes() {
        let (d, l) = (Palette::dark(), Palette::light());
        assert_eq!(d.overlay_bg, l.overlay_bg);
        assert_eq!(d.overlay_ink, l.overlay_ink);
        // Worst case: translucent pill composited over a white wallpaper.
        let pill_on_white = d.overlay_bg.over(Color::hex(0xFFFFFF));
        assert!(d.overlay_ink.contrast(pill_on_white) >= AA_TEXT);
        assert!(d.overlay_ink_2.contrast(pill_on_white) >= AA_TEXT);
        assert!(d.accent.contrast(pill_on_white) >= AA_NON_TEXT);
    }

    #[test]
    fn notch_themes_are_dark_flat_and_readable() {
        let p = Palette::dark();
        for th in NOTCH_THEMES {
            assert!(p.overlay_ink.contrast(th.bg) >= 13.0, "{}", th.id);
            assert!(p.overlay_ink_2.contrast(th.bg) >= AA_TEXT, "{}", th.id);
            assert!(ACCENT.contrast(th.bg) >= AA_NON_TEXT, "{}", th.id);
            assert_eq!(th.bg.a(), 0xFF, "opaque");
        }
        assert_eq!(notch_theme("nope").id, "midnight");
    }

    #[test]
    fn single_accent() {
        for p in both() {
            assert_eq!(p.accent, ACCENT);
        }
    }

    #[test]
    fn typography_falls_back() {
        let t = Typography::resolve(&["Segoe UI".into(), "Consolas".into(), "inter".into()]);
        assert_eq!(t.ui, "Inter");
        assert_eq!(t.display, "Segoe UI");
        assert_eq!(t.mono, "Consolas");
    }
}
