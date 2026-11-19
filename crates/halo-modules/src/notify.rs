//! Windows notifications mirrored in the notch (opt-in) + Do Not Disturb logic.
//!
//! A worker polls the action center every 2 s; toasts not seen before are queued for
//! the notch. Nothing is stored on disk. Do Not Disturb (manual, during focus, or on a
//! schedule) holds the notch quiet; Windows' own banners are not affected (no public
//! API) — the UI points users to Windows Focus Assist for those.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::config::Config;
use halo_core::module::{Category, Manifest, Module, ModuleContext};
use halo_platform::notifications::{self, Access, Toast};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "notifications",
    cdc: "M1+",
    name_key: "module.notifications",
    category: Category::Core,
    icon: "bell",
    permissions: &[],
    opt_in: true,
    default_enabled: false,
};

/// Is `minute` (0..1440) inside a quiet window `[from, to)` that may wrap midnight?
pub fn in_window(minute: u32, from: u32, to: u32) -> bool {
    if from == to {
        false
    } else if from < to {
        minute >= from && minute < to
    } else {
        minute >= from || minute < to
    }
}

fn hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?)
}

/// Do Not Disturb decision from config + focus state + local time (minutes).
pub fn dnd_active(config: &Config, focus_running: bool, minute: u32) -> bool {
    let s = &config.settings;
    if s.dnd_manual || (s.dnd_during_focus && focus_running) {
        return true;
    }
    s.dnd_scheduled && matches!((hhmm(&s.dnd_from), hhmm(&s.dnd_to)), (Some(f), Some(t)) if in_window(minute, f, t))
}

#[derive(Default)]
struct State {
    seen: HashSet<u32>,
    pending: VecDeque<Toast>,
    recent: VecDeque<Toast>,
    access: Option<Access>,
    primed: bool,
}

#[derive(Clone, Default)]
pub struct NotifyHandle(Arc<Mutex<State>>);

impl NotifyHandle {
    /// Next toast to show in the notch, if any.
    pub fn pop(&self) -> Option<Toast> {
        self.0.lock().ok()?.pending.pop_front()
    }

    /// Drop queued toasts (e.g. while Do Not Disturb is on).
    pub fn clear_pending(&self) {
        if let Ok(mut s) = self.0.lock() {
            s.pending.clear();
        }
    }

    pub fn recent(&self) -> Vec<Toast> {
        self.0.lock().map(|s| s.recent.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn access(&self) -> Option<Access> {
        self.0.lock().ok().and_then(|s| s.access)
    }

    pub fn dismiss(&self, id: u32) {
        notifications::dismiss(id);
        if let Ok(mut s) = self.0.lock() {
            s.recent.retain(|t| t.id != id);
        }
    }

    /// Feed a poll result (pure; tested). The first poll only primes `seen` so old
    /// toasts are not replayed at startup.
    pub fn ingest(&self, toasts: Vec<Toast>) {
        let Ok(mut s) = self.0.lock() else { return };
        let primed = s.primed;
        for t in toasts.iter().rev() {
            if s.seen.insert(t.id) && primed {
                s.pending.push_back(t.clone());
            }
        }
        s.primed = true;
        s.recent = toasts.into_iter().take(30).collect();
        while s.pending.len() > 10 {
            s.pending.pop_front();
        }
    }
}

pub struct Notifications {
    handle: NotifyHandle,
    worker: Option<Worker>,
}

impl Notifications {
    pub fn new() -> Self {
        Self { handle: NotifyHandle::default(), worker: None }
    }
    pub fn handle(&self) -> NotifyHandle {
        self.handle.clone()
    }
}

impl Default for Notifications {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Notifications {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let h = self.handle.clone();
        self.worker = Some(Worker::every("halo-notifications", Duration::from_secs(2), move || {
            let mut a = notifications::access();
            if a == Access::Unspecified {
                a = notifications::request_access();
            }
            if let Ok(mut s) = h.0.lock() {
                s.access = Some(a);
            }
            if a == Access::Allowed {
                h.ingest(notifications::toasts());
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
        if let Ok(mut s) = self.handle.0.lock() {
            *s = State::default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32) -> Toast {
        Toast { id, app: "App".into(), title: format!("t{id}"), body: String::new(), at: id as i64 }
    }

    #[test]
    fn only_new_toasts_after_priming() {
        let h = NotifyHandle::default();
        h.ingest(vec![t(2), t(1)]);
        assert!(h.pop().is_none(), "startup toasts are not replayed");
        h.ingest(vec![t(3), t(2), t(1)]);
        assert_eq!(h.pop().unwrap().id, 3);
        assert!(h.pop().is_none());
        assert_eq!(h.recent().len(), 3);
    }

    #[test]
    fn dnd_rules() {
        let mut c = Config::default();
        assert!(!dnd_active(&c, false, 600));
        c.settings.dnd_during_focus = true;
        assert!(dnd_active(&c, true, 600));
        c.settings.dnd_scheduled = true;
        c.settings.dnd_from = "22:00".into();
        c.settings.dnd_to = "07:00".into();
        assert!(dnd_active(&c, false, 23 * 60));
        assert!(dnd_active(&c, false, 6 * 60));
        assert!(!dnd_active(&c, false, 12 * 60));
        c.settings.dnd_manual = true;
        assert!(dnd_active(&c, false, 12 * 60));
    }
}
