//! Bridges tokens to GPUI types and wires gpui-kit (theme, assets, fonts).

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use gpui_kit::{App, AssetSource, BoxShadow, Hsla, SharedString, point, px, rgba};

use crate::tokens::{self, Color, Mode, Shadow, Typography};

/// Token colour → GPUI colour.
pub fn hsla(c: Color) -> Hsla {
    rgba(c.0).into()
}

/// Token shadow → GPUI box shadow.
pub fn box_shadow(s: Shadow) -> BoxShadow {
    BoxShadow {
        color: hsla(s.color),
        offset: point(px(0.0), px(s.offset_y)),
        blur_radius: px(s.blur),
        spread_radius: px(s.spread),
        inset: false,
    }
}

/// gpui-kit theme name for a mode (defined in `themes/halo.json`).
pub fn theme_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Dark => "Halo Dark",
        Mode::Light => "Halo Light",
    }
}

/// Asset source: Halo glyphs under `halo/…` (embedded), everything else from
/// gpui-kit's full Lucide catalogue (`AllAssets`).
pub struct HaloAssets;

const HALO_ICONS: &[(&str, &[u8])] = &[
    ("halo/mark.svg", include_bytes!("../../../assets/icons/mark.svg")),
];

impl AssetSource for HaloAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = HALO_ICONS.iter().find(|(p, _)| *p == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut out: Vec<SharedString> = HALO_ICONS
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect();
        out.extend(gpui_kit::assets::AllAssets.list(path)?);
        Ok(out)
    }
}

/// Directories searched for runtime resources (`fonts/`, `themes/`):
/// next to the executable first, then the workspace (dev builds).
pub fn resource_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
        }
    }
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    dirs
}

/// Loads every `.ttf`/`.otf` found in `assets/fonts/` (Inter, Space Grotesk,
/// JetBrains Mono — all OFL) then resolves the typography fallback chains.
pub fn load_fonts(cx: &mut App) -> Typography {
    let mut fonts: Vec<Cow<'static, [u8]>> = Vec::new();
    for dir in resource_dirs() {
        let Ok(entries) = std::fs::read_dir(dir.join("assets/fonts")) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
            if ext == "ttf" || ext == "otf" {
                if let Ok(bytes) = std::fs::read(&path) {
                    fonts.push(Cow::Owned(bytes));
                }
            }
        }
        if !fonts.is_empty() {
            break;
        }
    }
    if !fonts.is_empty() {
        if let Err(err) = cx.text_system().add_fonts(fonts) {
            log::warn!("failed to load bundled fonts: {err}");
        }
    }
    let typo = Typography::resolve(&cx.text_system().all_font_names());
    log::info!("typography: ui={} display={} mono={}", typo.ui, typo.display, typo.mono);
    typo
}

/// Load `themes/halo.json` into gpui-kit and apply the theme for `mode`.
pub fn init_theme(cx: &mut App, mode: Mode) {
    use gpui_kit::component::{Theme, ThemeRegistry};

    let Some(dir) = resource_dirs().into_iter().map(|d| d.join("themes")).find(|d| d.join("halo.json").exists())
    else {
        log::warn!("themes/halo.json not found — gpui-kit default theme in use");
        return;
    };
    let name = SharedString::from(theme_name(mode));
    if let Err(err) = ThemeRegistry::watch_dir(dir, cx, move |cx| {
        if let Some(theme) = ThemeRegistry::global(cx).themes().get(&name).cloned() {
            Theme::update(cx, |current| current.apply_config(&theme));
        }
    }) {
        log::error!("failed to watch themes directory: {err}");
    }
}

/// Switch the gpui-kit theme at runtime (theme already registered by [`init_theme`]).
pub fn apply_theme(cx: &mut App, mode: Mode) {
    use gpui_kit::component::{Theme, ThemeRegistry};

    let name = SharedString::from(theme_name(mode));
    if let Some(theme) = ThemeRegistry::global(cx).themes().get(&name).cloned() {
        Theme::update(cx, |current| current.apply_config(&theme));
    }
}

/// Re-export for call sites: `halo_ui::palette(mode)`.
pub fn palette(mode: Mode) -> tokens::Palette {
    tokens::Palette::for_mode(mode)
}
