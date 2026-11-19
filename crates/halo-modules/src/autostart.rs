//! M18 — Launch Halo at Windows sign-in. Off by default.
//!
//! Release builds always run as administrator, and Windows skips elevated programs
//! listed in the `Run` key — so the sign-in start is a scheduled task with highest
//! privileges (created when Halo is elevated). Non-elevated debug builds fall back
//! to the HKCU `Run` value.

use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use halo_platform::registry;

pub const MANIFEST: Manifest = Manifest {
    id: "autostart",
    cdc: "M18",
    name_key: "module.autostart",
    category: Category::System,
    icon: "power",
    permissions: &[Permission::Registry],
    opt_in: false,
    default_enabled: false,
};

pub const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const VALUE: &str = "Halo";
/// Name of the elevated sign-in task in the Task Scheduler.
pub const TASK: &str = "Halo";

/// Command line registered for sign-in: quoted exe + `--minimized`.
pub fn command_line(exe: &std::path::Path) -> String {
    format!("\"{}\" --minimized", exe.display())
}

pub fn enable(run_key: &str, exe: &std::path::Path) -> bool {
    registry::set_string(run_key, VALUE, &command_line(exe))
}

pub fn disable(run_key: &str) -> bool {
    registry::delete_value(run_key, VALUE) || !is_enabled(run_key)
}

pub fn is_enabled(run_key: &str) -> bool {
    registry::get_string(run_key, VALUE).is_some()
}

/// Elevated sign-in task (preferred). On success the legacy `Run` value is removed.
pub fn enable_task(run_key: &str, exe: &std::path::Path) -> bool {
    let ok = halo_platform::task::create_logon_task(TASK, exe, "--minimized");
    if ok {
        registry::delete_value(run_key, VALUE);
    }
    ok
}

/// Started at sign-in, by the task or the `Run` value.
pub fn is_registered(run_key: &str) -> bool {
    halo_platform::task::task_exists(TASK) || is_enabled(run_key)
}

pub struct Autostart {
    run_key: String,
    /// Use the Task Scheduler (false in tests: never touch the real scheduler).
    use_task: bool,
}

impl Autostart {
    pub fn new() -> Self {
        Self { run_key: RUN_KEY.into(), use_task: true }
    }

    /// For tests: target a scratch key instead of the real Run key.
    pub fn with_key(run_key: &str) -> Self {
        Self { run_key: run_key.into(), use_task: false }
    }
}

impl Default for Autostart {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Autostart {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let exe = std::env::current_exe()?;
        if self.use_task && enable_task(&self.run_key, &exe) {
            return Ok(());
        }
        // Not elevated (debug build): the Run key still works for a non-admin exe.
        anyhow::ensure!(enable(&self.run_key, &exe), "cannot write {}", self.run_key);
        Ok(())
    }

    fn stop(&mut self) {
        if self.use_task {
            halo_platform::task::delete_task(TASK);
        }
        disable(&self.run_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use halo_core::bus::Bus;

    #[test]
    fn enable_disable_on_scratch_key() {
        let key = format!("Software\\Halo\\TestRun-{}", std::process::id());
        let mut m = Autostart::with_key(&key);
        m.start(&ModuleContext { bus: Bus::new() }).unwrap();
        assert!(is_enabled(&key));
        let cmd = registry::get_string(&key, VALUE).unwrap();
        assert!(cmd.starts_with('"') && cmd.ends_with("--minimized"));
        m.stop();
        assert!(!is_enabled(&key));
        registry::delete_tree(&key);
        // The real Run key was never touched.
    }
}
