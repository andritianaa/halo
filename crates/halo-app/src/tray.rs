//! System tray icon + menu and the global hotkey (spec Phase 0: tray, raccourcis globaux).

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use halo_core::config::ThemePref;
use halo_core::overlay::Anchor;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::state::AppState;

/// Actions coming from the tray or the hotkey, handled by the overlay tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    ToggleOverlay,
    HotkeyToggle,
    Picker,
    Palette,
    Screenshot,
    Record,
    SetAnchor(Anchor),
    SetTheme(ThemePref),
    OpenSettings,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    toggle: MenuItem,
    anchors: Vec<(Anchor, CheckMenuItem)>,
    themes: Vec<(ThemePref, CheckMenuItem)>,
    settings: MenuItem,
    quit: MenuItem,
}

impl Tray {
    pub fn new(state: &AppState) -> anyhow::Result<Self> {
        let t = |k: &str| state.t(k);
        let toggle = MenuItem::new(t("tray.show"), true, None);
        let anchors: Vec<_> = Anchor::ALL
            .iter()
            .map(|&a| (a, CheckMenuItem::new(t(a.label_key()), true, a == state.config.overlay.anchor, None)))
            .collect();
        let themes: Vec<_> = [
            (ThemePref::System, "tray.theme.system"),
            (ThemePref::Dark, "tray.theme.dark"),
            (ThemePref::Light, "tray.theme.light"),
        ]
        .into_iter()
        .map(|(p, k)| (p, CheckMenuItem::new(t(k), true, p == state.config.theme, None)))
        .collect();
        let settings = MenuItem::new(t("tray.settings"), true, None);
        let quit = MenuItem::new(t("tray.quit"), true, None);

        let anchor_refs: Vec<&dyn tray_icon::menu::IsMenuItem> =
            anchors.iter().map(|(_, i)| i as &dyn tray_icon::menu::IsMenuItem).collect();
        let theme_refs: Vec<&dyn tray_icon::menu::IsMenuItem> =
            themes.iter().map(|(_, i)| i as &dyn tray_icon::menu::IsMenuItem).collect();
        let position = Submenu::with_items(t("tray.position"), true, &anchor_refs)?;
        let theme = Submenu::with_items(t("tray.theme"), true, &theme_refs)?;
        let menu = Menu::with_items(&[
            &toggle,
            &PredefinedMenuItem::separator(),
            &position,
            &theme,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ])?;

        let rgba = halo_ui::mark::mark_rgba(32, halo_ui::tokens::ACCENT);
        let icon = TrayIconBuilder::new()
            .with_tooltip(t("tray.tooltip"))
            .with_icon(tray_icon::Icon::from_rgba(rgba, 32, 32)?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        Ok(Self { _icon: icon, toggle, anchors, themes, settings, quit })
    }

    /// Drain pending menu/tray events.
    pub fn poll(&self, out: &mut Vec<Action>) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = ev.id();
            if id == self.toggle.id() {
                out.push(Action::ToggleOverlay);
            } else if id == self.settings.id() {
                out.push(Action::OpenSettings);
            } else if id == self.quit.id() {
                out.push(Action::Quit);
            } else if let Some((a, _)) = self.anchors.iter().find(|(_, i)| i.id() == id) {
                out.push(Action::SetAnchor(*a));
            } else if let Some((p, _)) = self.themes.iter().find(|(_, i)| i.id() == id) {
                out.push(Action::SetTheme(*p));
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                out.push(Action::ToggleOverlay);
            }
        }
    }

    /// Reflect the current config in the check marks.
    pub fn sync(&self, state: &AppState) {
        for (a, item) in &self.anchors {
            item.set_checked(*a == state.config.overlay.anchor);
        }
        for (p, item) in &self.themes {
            item.set_checked(*p == state.config.theme);
        }
    }
}

pub struct Hotkeys {
    _manager: GlobalHotKeyManager,
    toggle: Option<HotKey>,
    picker: Option<HotKey>,
    palette: Option<HotKey>,
    launcher: Option<HotKey>,
    screenshot: Option<HotKey>,
    record: Option<HotKey>,
}

fn register(manager: &GlobalHotKeyManager, spec: &str) -> Option<HotKey> {
    match spec.parse::<HotKey>() {
        Ok(hk) => match manager.register(hk) {
            Ok(()) => Some(hk),
            Err(err) => {
                log::warn!("hotkey {spec} unavailable: {err}");
                None
            }
        },
        Err(err) => {
            log::warn!("invalid hotkey {spec}: {err}");
            None
        }
    }
}

impl Hotkeys {
    pub fn new(state: &AppState) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let toggle = register(&manager, &state.config.hotkeys.toggle_overlay);
        let picker = register(&manager, &state.config.settings.picker_hotkey);
        let palette = register(&manager, "Ctrl+Alt+Space");
        // PowerToys-style launcher shortcut; Ctrl+Alt+Space stays as a fallback.
        let launcher = register(&manager, "Ctrl+Space");
        let screenshot = register(&manager, "Ctrl+Alt+S");
        let record = register(&manager, "Ctrl+Alt+R");
        Ok(Self { _manager: manager, toggle, picker, palette, launcher, screenshot, record })
    }

    pub fn poll(&self, out: &mut Vec<Action>) {
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state() != HotKeyState::Pressed {
                continue;
            }
            if Some(ev.id()) == self.toggle.map(|h| h.id()) {
                out.push(Action::HotkeyToggle);
            } else if Some(ev.id()) == self.picker.map(|h| h.id()) {
                out.push(Action::Picker);
            } else if Some(ev.id()) == self.palette.map(|h| h.id()) || Some(ev.id()) == self.launcher.map(|h| h.id()) {
                out.push(Action::Palette);
            } else if Some(ev.id()) == self.screenshot.map(|h| h.id()) {
                out.push(Action::Screenshot);
            } else if Some(ev.id()) == self.record.map(|h| h.id()) {
                out.push(Action::Record);
            }
        }
    }
}
