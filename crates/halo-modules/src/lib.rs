//! Halo modules — one file per spec feature, each implementing
//! [`halo_core::module::Module`] and exposing a cloneable handle for the UI.

pub mod accent;
pub mod activity;
pub mod autostart;
pub mod catalog;
pub mod clipboard;
pub mod clock;
pub mod command;
pub mod focus;
pub mod colorpicker;
pub mod images;
pub mod media;
pub mod music;
pub mod notes;
pub mod notify;
pub mod resources;
pub mod shelf;
pub mod spotlight;
pub mod store;
pub mod system;
pub mod tasks;
pub mod update;
pub mod wifikeys;
pub mod wintheme;
pub mod worker;

use halo_core::config::Config;
use halo_core::module::Registry;

/// Typed handles the UI uses to read module state / trigger actions.
#[derive(Clone)]
pub struct Handles {
    pub system: system::SystemHandle,
    pub spotlight: spotlight::SpotlightHandle,
    pub picker: colorpicker::PickerHandle,
    pub accent: accent::AccentHandle,
    pub notes: Option<notes::NotesHandle>,
    pub clipboard: Option<clipboard::ClipboardHandle>,
    pub images: images::ImagesHandle,
    pub activity: Option<activity::ActivityHandle>,
    pub media: media::MediaHandle,
    pub tasks: Option<tasks::TasksHandle>,
    pub focus: focus::FocusHandle,
    pub resources: resources::ResourcesHandle,
    pub music: music::MusicHandle,
    pub wintheme: wintheme::WinThemeHandle,
    pub notify: notify::NotifyHandle,
}

/// Build every implemented module from the config and register it.
pub fn register_all(registry: &mut Registry, config: &Config) -> Handles {
    let s = &config.settings;
    let db = store::Db::open(&store::Db::default_path()).unwrap_or_else(|e| {
        log::error!("cannot open halo.db ({e:#}) — using an in-memory database");
        store::Db::memory()
    });

    let system = system::SystemMonitor::new();
    let spotlight = spotlight::Spotlight::new(s.spotlight_sensitivity);
    let picker = colorpicker::ColorPicker::new(worker::data_dir().join("colors.json"));
    let accent = accent::QuickAccent::new(
        accent::Activation::parse(&s.accent_activation),
        if s.accent_all_languages { accent::CharSet::All } else { accent::CharSet::Fr },
    );
    let notes = notes::Notes::new(&db);
    let clipboard = clipboard::ClipboardHistory::new(&db, config);
    let images = images::Images::new();
    let activity = activity::ActivityTracker::new(&db, config);
    let media = media::MediaSession::new();
    let tasks = tasks::Tasks::new(&db);
    let resources = resources::Resources::new();
    let music = music::Music::new(&db);
    let wintheme = wintheme::WinTheme::new(config);
    let notifications = notify::Notifications::new();

    let handles = Handles {
        system: system.handle(),
        spotlight: spotlight.handle(),
        picker: picker.handle(),
        accent: accent.handle(),
        notes: notes.handle(),
        clipboard: clipboard.handle(),
        images: images.handle(),
        activity: activity.handle(),
        media: media.handle(),
        tasks: tasks.handle(),
        focus: focus::FocusHandle::default(),
        resources: resources.handle(),
        music: music.handle(),
        wintheme: wintheme.handle(),
        notify: notifications.handle(),
    };
    registry.register(Box::new(system));
    registry.register(Box::new(spotlight));
    registry.register(Box::new(autostart::Autostart::new()));
    registry.register(Box::new(picker));
    registry.register(Box::new(accent));
    registry.register(Box::new(notes));
    registry.register(Box::new(clipboard));
    registry.register(Box::new(images));
    registry.register(Box::new(activity));
    registry.register(Box::new(media));
    registry.register(Box::new(tasks));
    registry.register(Box::new(resources));
    registry.register(Box::new(music));
    registry.register(Box::new(wintheme));
    registry.register(Box::new(notifications));
    registry.register(Box::new(wifikeys::WifiKeys));
    handles
}
