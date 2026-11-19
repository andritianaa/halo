//! Now playing (any player) for the notch: polls the Windows media session on a
//! worker thread and exposes play/pause/next/previous. Part of M15 (mini-controls).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::module::{Category, Manifest, Module, ModuleContext};
use halo_platform::media::{Command, Media, NowPlaying};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "media",
    cdc: "M15",
    name_key: "module.media",
    category: Category::Media,
    icon: "disc",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Default)]
pub struct MediaHandle {
    state: Arc<Mutex<Option<NowPlaying>>>,
    pending: Arc<Mutex<Vec<Command>>>,
    seq: Arc<Mutex<u64>>,
}

impl MediaHandle {
    pub fn get(&self) -> Option<NowPlaying> {
        self.state.lock().ok().and_then(|s| s.clone())
    }

    pub fn seq(&self) -> u64 {
        self.seq.lock().map(|s| *s).unwrap_or(0)
    }

    /// Queue a command; executed on the media worker (WinRT calls block).
    pub fn send(&self, c: Command) {
        if let Ok(mut p) = self.pending.lock() {
            p.push(c);
        }
    }
}

pub struct MediaSession {
    handle: MediaHandle,
    worker: Option<Worker>,
}

impl MediaSession {
    pub fn new() -> Self {
        Self { handle: MediaHandle::default(), worker: None }
    }
    pub fn handle(&self) -> MediaHandle {
        self.handle.clone()
    }
}

impl Default for MediaSession {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for MediaSession {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let h = self.handle.clone();
        let mut media: Option<Media> = None;
        let mut last_key = String::new();
        let mut art: Option<Vec<u8>> = None;
        self.worker = Some(Worker::every("halo-media", Duration::from_millis(700), move || {
            if media.is_none() {
                media = Media::new();
            }
            let Some(m) = &media else { return true };
            let cmds: Vec<Command> = h.pending.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
            for c in cmds {
                m.send(c);
            }
            let np = m.now_playing(false).map(|mut n| {
                // Fetch album art only when the track changes.
                let key = format!("{}|{}|{}", n.source, n.title, n.artist);
                if key != last_key {
                    art = m.now_playing(true).and_then(|x| x.art);
                    last_key = key;
                }
                n.art = art.clone();
                n
            });
            if let Ok(mut s) = h.state.lock() {
                if *s != np {
                    *s = np;
                    if let Ok(mut q) = h.seq.lock() {
                        *q += 1;
                    }
                }
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
        if let Ok(mut s) = self.handle.state.lock() {
            *s = None;
        }
    }
}

/// "2:31"
pub fn mmss(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
