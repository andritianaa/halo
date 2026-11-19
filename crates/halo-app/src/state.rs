//! App-wide state shared by every window (main thread only).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use halo_core::bus::Bus;
use halo_core::config::{Config, ThemePref};
use halo_core::i18n::{self, Lang};
use halo_core::module::{ModuleState, Registry};
use halo_modules::Handles;
use halo_ui::tokens::{Mode, Palette, Typography};

pub type Shared = Rc<RefCell<AppState>>;

/// Hub sections (sidebar entries).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Modules,
    System,
    Picker,
    Accent,
    Spotlight,
    Startup,
    Notes,
    Clipboard,
    Images,
    Activity,
    Productivity,
    Resources,
    Music,
    WinTheme,
    Notifications,
    Capture,
    Vault,
    Network,
    WifiKeys,
}

/// What a region selection is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionPurpose {
    Screenshot,
    Record,
}

pub struct AppState {
    pub config: Config,
    pub config_path: PathBuf,
    pub mode: Mode,
    pub typo: Typography,
    pub registry: Registry,
    pub handles: Handles,
    pub capture: halo_capture::Handles,
    pub secure: halo_secure::Handles,
    #[allow(dead_code)] // subscribed to by modules as they need cross-module events
    pub bus: Bus,
    /// Bumped on every user-visible change so views know to re-place/re-render.
    pub generation: u64,
    pub hub_window: Option<gpui_kit::AnyWindowHandle>,
    pub hub_view: Option<gpui_kit::WeakEntity<crate::hub::Hub>>,
    /// Section the hub should show next time it renders (set by notch tiles/tray).
    pub hub_section: Section,
    /// Request flags consumed by the overlay loop.
    pub want_hub: bool,
    pub want_picker: bool,
    pub want_palette: bool,
    /// Debug automation: scroll the hub panel to this offset (px).
    pub want_hub_scroll: Option<f32>,
    /// Open the region selector for this purpose.
    pub want_region: Option<RegionPurpose>,
    pub palette_window: Option<gpui_kit::AnyWindowHandle>,
    /// Files dropped on the notch's vault zone while the vault was locked; added
    /// as soon as the user unlocks it in the hub.
    pub pending_vault: Vec<PathBuf>,
    /// New-version check against the GitHub releases.
    pub update: halo_modules::update::UpdateHandle,
}

impl AppState {
    pub fn load(typo: Typography) -> Shared {
        let config_path = Config::default_path();
        let mut config = Config::load_or_default(&config_path);
        if !config_path.exists() {
            // First run: follow the Windows display language (9 supported, else English).
            config.lang = Lang::from_locale(&halo_platform::user_locale());
        }
        let bus = Bus::new();
        let mut registry = Registry::new(bus.clone());
        let handles = halo_modules::register_all(&mut registry, &config);
        let capture = halo_capture::register(&mut registry, &config);
        let secure = halo_secure::register(&mut registry, &config);
        registry.apply(&config);
        let mode = resolve_mode(config.theme);
        Rc::new(RefCell::new(Self {
            config,
            config_path,
            mode,
            typo,
            registry,
            handles,
            capture,
            secure,
            bus,
            generation: 0,
            hub_window: None,
            hub_view: None,
            hub_section: Section::General,
            want_hub: false,
            want_picker: false,
            want_palette: false,
            want_region: None,
            pending_vault: Vec::new(),
            update: Default::default(),
            want_hub_scroll: None,
            palette_window: None,
        }))
    }

    pub fn palette(&self) -> Palette {
        Palette::for_mode(self.mode)
    }

    pub fn t(&self, key: &str) -> String {
        i18n::t(self.config.lang, key).to_string()
    }

    /// Translate with {name} placeholders.
    pub fn tf(&self, key: &str, args: &[(&str, &str)]) -> String {
        i18n::tf(self.config.lang, key, args)
    }

    pub fn fr(&self) -> bool {
        self.config.lang == Lang::Fr
    }

    pub fn module_running(&self, id: &str) -> bool {
        self.registry.state(id) == Some(ModuleState::Running)
    }

    /// Apply a config change, persist it, and bump the generation.
    pub fn update_config(&mut self, f: impl FnOnce(&mut Config)) {
        f(&mut self.config);
        self.mode = resolve_mode(self.config.theme);
        self.registry.apply(&self.config);
        if let Err(err) = self.config.save(&self.config_path) {
            log::error!("saving config failed: {err:#}");
        }
        self.generation += 1;
    }

    pub fn open_hub(&mut self, section: Section) {
        self.hub_section = section;
        self.want_hub = true;
    }
}

pub fn resolve_mode(pref: ThemePref) -> Mode {
    match pref {
        ThemePref::Dark => Mode::Dark,
        ThemePref::Light => Mode::Light,
        ThemePref::System => {
            if halo_platform::apps_use_light_theme() {
                Mode::Light
            } else {
                Mode::Dark
            }
        }
    }
}
