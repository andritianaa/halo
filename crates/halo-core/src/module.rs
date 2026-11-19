//! Common module interface (spec §5, §9) and a registry that supervises modules.
//!
//! A disabled module is never started, so it costs nothing (spec §7). A module that
//! panics in `start`/`stop`/`tick` is stopped and reported on the bus instead of
//! taking the app down.

use std::panic::{self, AssertUnwindSafe};

use crate::bus::{Bus, Event};
use crate::config::Config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Core,
    Monitoring,
    Network,
    Productivity,
    Files,
    Security,
    Tools,
    Capture,
    Media,
    Comfort,
    System,
    Personalization,
}

/// Capabilities a module needs; shown to the user before opt-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Requests UAC elevation on demand (e.g. M5 network limiter).
    Elevation,
    /// Installs a driver (WinDivert).
    Driver,
    /// Low-level keyboard hook — only for Quick Accent, never logs.
    KeyboardHook,
    ClipboardRead,
    ScreenCapture,
    Microphone,
    ForegroundWindow,
    Registry,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    /// Stable id used in config (`[modules] clipboard = true`).
    pub id: &'static str,
    /// spec reference, e.g. "M7".
    pub cdc: &'static str,
    /// i18n key of the display name.
    pub name_key: &'static str,
    pub category: Category,
    /// Lucide icon name (gpui-kit `IconName`) or `halo/…` asset.
    pub icon: &'static str,
    pub permissions: &'static [Permission],
    /// Sensitive modules (M4, M5, M7, M10) are opt-in: off until explicitly enabled.
    pub opt_in: bool,
    pub default_enabled: bool,
}

/// Context handed to modules: they talk to the rest of Halo only through the bus.
#[derive(Clone)]
pub struct ModuleContext {
    pub bus: Bus,
}

pub trait Module: Send {
    fn manifest(&self) -> &Manifest;
    fn start(&mut self, cx: &ModuleContext) -> anyhow::Result<()>;
    fn stop(&mut self);
    /// Periodic work on the core thread; keep it cheap (adaptive sampling).
    fn tick(&mut self) {}
    /// Read this module's settings from the config. Return `true` if a running
    /// instance must be restarted to apply them.
    fn configure(&mut self, _config: &Config) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleState {
    Disabled,
    Running,
    Failed,
}

struct Slot {
    module: Box<dyn Module>,
    state: ModuleState,
}

pub struct Registry {
    slots: Vec<Slot>,
    cx: ModuleContext,
}

impl Registry {
    pub fn new(bus: Bus) -> Self {
        Self { slots: Vec::new(), cx: ModuleContext { bus } }
    }

    pub fn register(&mut self, module: Box<dyn Module>) {
        let id = module.manifest().id;
        assert!(self.slots.iter().all(|s| s.module.manifest().id != id), "duplicate module id {id}");
        self.slots.push(Slot { module, state: ModuleState::Disabled });
    }

    pub fn manifests(&self) -> impl Iterator<Item = (&Manifest, ModuleState)> {
        self.slots.iter().map(|s| (s.module.manifest(), s.state))
    }

    pub fn state(&self, id: &str) -> Option<ModuleState> {
        self.slots.iter().find(|s| s.module.manifest().id == id).map(|s| s.state)
    }

    /// Start/stop modules so that the running set matches `config`.
    pub fn apply(&mut self, config: &Config) {
        for i in 0..self.slots.len() {
            let restart = guard(|| self.slots[i].module.configure(config)).unwrap_or(false);
            if restart && self.slots[i].state == ModuleState::Running {
                self.set_enabled_at(i, false);
            }
            let m = self.slots[i].module.manifest();
            let default = m.default_enabled && !m.opt_in;
            let want = config.module_enabled(m.id, default);
            self.set_enabled_at(i, want);
        }
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(i) = self.slots.iter().position(|s| s.module.manifest().id == id) {
            self.set_enabled_at(i, enabled);
        }
    }

    fn set_enabled_at(&mut self, i: usize, enabled: bool) {
        let cx = self.cx.clone();
        let slot = &mut self.slots[i];
        let id = slot.module.manifest().id.to_string();
        match (slot.state, enabled) {
            (ModuleState::Running, false) => {
                let _ = guard(|| slot.module.stop());
                slot.state = ModuleState::Disabled;
                cx.bus.publish(Event::ModuleToggled { id, enabled: false });
            }
            (ModuleState::Disabled | ModuleState::Failed, true) => match guard(|| slot.module.start(&cx)) {
                Ok(Ok(())) => {
                    slot.state = ModuleState::Running;
                    cx.bus.publish(Event::ModuleToggled { id, enabled: true });
                }
                Ok(Err(err)) => fail(slot, &cx, id, err.to_string()),
                Err(reason) => fail(slot, &cx, id, reason),
            },
            (ModuleState::Failed, false) => slot.state = ModuleState::Disabled,
            _ => {}
        }
    }

    /// Tick every running module; a panicking module is stopped and reported.
    pub fn tick(&mut self) {
        let cx = self.cx.clone();
        for slot in &mut self.slots {
            if slot.state == ModuleState::Running {
                if let Err(reason) = guard(|| slot.module.tick()) {
                    let id = slot.module.manifest().id.to_string();
                    let _ = guard(|| slot.module.stop());
                    fail(slot, &cx, id, reason);
                }
            }
        }
    }

    pub fn shutdown(&mut self) {
        for slot in &mut self.slots {
            if slot.state == ModuleState::Running {
                let _ = guard(|| slot.module.stop());
                slot.state = ModuleState::Disabled;
            }
        }
        self.cx.bus.publish(Event::Shutdown);
    }
}

fn fail(slot: &mut Slot, cx: &ModuleContext, id: String, reason: String) {
    log::error!("module {id} failed: {reason}");
    slot.state = ModuleState::Failed;
    cx.bus.publish(Event::ModuleFailed { id, reason });
}

fn guard<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        p.downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe {
        manifest: Manifest,
        panic_on_tick: bool,
    }

    impl Probe {
        fn boxed(id: &'static str, opt_in: bool, panic_on_tick: bool) -> Box<dyn Module> {
            Box::new(Probe {
                manifest: Manifest {
                    id,
                    cdc: "M0",
                    name_key: id,
                    category: Category::Tools,
                    icon: "circle",
                    permissions: &[],
                    opt_in,
                    default_enabled: true,
                },
                panic_on_tick,
            })
        }
    }

    impl Module for Probe {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }
        fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
            Ok(())
        }
        fn stop(&mut self) {}
        fn tick(&mut self) {
            if self.panic_on_tick {
                panic!("boom");
            }
        }
    }

    #[test]
    fn opt_in_modules_stay_off_by_default() {
        let mut r = Registry::new(Bus::new());
        r.register(Probe::boxed("plain", false, false));
        r.register(Probe::boxed("sensitive", true, false));
        r.apply(&Config::default());
        assert_eq!(r.state("plain"), Some(ModuleState::Running));
        assert_eq!(r.state("sensitive"), Some(ModuleState::Disabled));

        let mut cfg = Config::default();
        cfg.modules.insert("sensitive".into(), true);
        r.apply(&cfg);
        assert_eq!(r.state("sensitive"), Some(ModuleState::Running));
    }

    #[test]
    fn panicking_module_is_isolated() {
        let bus = Bus::new();
        let events = bus.subscribe();
        let mut r = Registry::new(bus);
        r.register(Probe::boxed("bad", false, true));
        r.register(Probe::boxed("good", false, false));
        r.apply(&Config::default());
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        r.tick();
        std::panic::set_hook(prev);
        assert_eq!(r.state("bad"), Some(ModuleState::Failed));
        assert_eq!(r.state("good"), Some(ModuleState::Running));
        assert!(events.try_iter().any(|e| matches!(e, Event::ModuleFailed { id, .. } if id == "bad")));
    }
}
