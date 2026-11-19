//! M15 — Local music player: MP3/FLAC/WAV/OGG/M4A, library, playlists, queue,
//! shuffle/repeat, tags + cover art, mini-controls in the notch, files opened from
//! Explorer (`halo.exe --play <files>`).
//!
//! Audio runs on its own thread (rodio); the UI talks to it through commands and a
//! shared state snapshot.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::module::{Category, Manifest, Module, ModuleContext};
use lofty::prelude::*;
use rand::seq::SliceRandom;
use rusqlite::params;

use crate::store::Db;

pub const MANIFEST: Manifest = Manifest {
    id: "music",
    cdc: "M15",
    name_key: "module.music",
    category: Category::Media,
    icon: "music",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

pub const AUDIO_EXT: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a"];

pub fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| AUDIO_EXT.contains(&e.to_lowercase().as_str()))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: f64,
}

impl Track {
    /// Read tags (falls back to the file name).
    pub fn read(path: &Path) -> Track {
        let fallback = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut t = Track { path: path.to_path_buf(), title: fallback, ..Default::default() };
        if let Ok(f) = lofty::read_from_path(path) {
            t.duration_s = f.properties().duration().as_secs_f64();
            if let Some(tag) = f.primary_tag().or_else(|| f.first_tag()) {
                if let Some(v) = tag.title() {
                    t.title = v.to_string();
                }
                t.artist = tag.artist().map(|v| v.to_string()).unwrap_or_default();
                t.album = tag.album().map(|v| v.to_string()).unwrap_or_default();
            }
        }
        t
    }

    pub fn cover(path: &Path) -> Option<Vec<u8>> {
        let f = lofty::read_from_path(path).ok()?;
        let tag = f.primary_tag().or_else(|| f.first_tag())?;
        tag.pictures().first().map(|p| p.data().to_vec())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub fn next(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// Pure queue logic (tested): order, shuffle, repeat, next/previous.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub tracks: Vec<Track>,
    /// Play order (indices into `tracks`), shuffled or identity.
    pub order: Vec<usize>,
    /// Position in `order`.
    pub pos: usize,
    pub shuffle: bool,
    pub repeat: Repeat,
}

impl Queue {
    pub fn set(&mut self, tracks: Vec<Track>, start: usize) {
        self.tracks = tracks;
        self.order = (0..self.tracks.len()).collect();
        self.pos = start.min(self.tracks.len().saturating_sub(1));
        if self.shuffle {
            self.reshuffle();
        }
    }

    fn reshuffle(&mut self) {
        let cur = self.order.get(self.pos).copied();
        let mut rest: Vec<usize> = (0..self.tracks.len()).filter(|i| Some(*i) != cur).collect();
        rest.shuffle(&mut rand::rng());
        self.order = cur.into_iter().chain(rest).collect();
        self.pos = 0;
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.shuffle = on;
        if on {
            self.reshuffle();
        } else {
            let cur = self.order.get(self.pos).copied().unwrap_or(0);
            self.order = (0..self.tracks.len()).collect();
            self.pos = cur;
        }
    }

    pub fn current(&self) -> Option<&Track> {
        self.order.get(self.pos).and_then(|i| self.tracks.get(*i))
    }

    /// Advance after a track ended (`auto`) or on "next". None = stop.
    pub fn next(&mut self, auto: bool) -> Option<&Track> {
        if self.tracks.is_empty() {
            return None;
        }
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == Repeat::All || !auto {
            if self.repeat != Repeat::All && !auto {
                return None;
            }
            self.pos = 0;
        } else {
            return None;
        }
        self.current()
    }

    pub fn previous(&mut self) -> Option<&Track> {
        if self.pos > 0 {
            self.pos -= 1;
        }
        self.current()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlayerState {
    pub queue: Queue,
    pub playing: bool,
    pub position_s: f64,
    pub volume: f32,
    pub cover: Option<Vec<u8>>,
    pub error: Option<String>,
    pub seq: u64,
}

enum Cmd {
    PlayQueue(Vec<Track>, usize),
    Toggle,
    Next,
    Previous,
    Seek(f64),
    Volume(f32),
    Shuffle(bool),
    Repeat(Repeat),
    Stop,
    Quit,
}

#[derive(Clone)]
pub struct MusicHandle {
    state: Arc<Mutex<PlayerState>>,
    tx: Arc<Mutex<Option<Sender<Cmd>>>>,
    db: Option<Db>,
}

impl MusicHandle {
    pub fn get(&self) -> PlayerState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn send(&self, c: Cmd) {
        if let Some(tx) = self.tx.lock().ok().and_then(|t| t.clone()) {
            let _ = tx.send(c);
        }
    }

    pub fn play_tracks(&self, tracks: Vec<Track>, start: usize) {
        self.send(Cmd::PlayQueue(tracks, start));
    }

    pub fn play_files(&self, files: &[PathBuf]) {
        let tracks: Vec<Track> = files.iter().filter(|f| is_audio(f)).map(|f| Track::read(f)).collect();
        if !tracks.is_empty() {
            self.play_tracks(tracks, 0);
        }
    }

    pub fn toggle(&self) {
        self.send(Cmd::Toggle)
    }
    pub fn next(&self) {
        self.send(Cmd::Next)
    }
    pub fn previous(&self) {
        self.send(Cmd::Previous)
    }
    pub fn seek(&self, s: f64) {
        self.send(Cmd::Seek(s))
    }
    pub fn set_volume(&self, v: f32) {
        self.send(Cmd::Volume(v))
    }
    pub fn set_shuffle(&self, on: bool) {
        self.send(Cmd::Shuffle(on))
    }
    pub fn set_repeat(&self, r: Repeat) {
        self.send(Cmd::Repeat(r))
    }
    pub fn stop(&self) {
        self.send(Cmd::Stop)
    }

    // ── Library & playlists (SQLite) ────────────────────────────────────────

    /// Scan folders recursively (≤ 5000 files) and store tags. Runs synchronously.
    pub fn scan(&self, dirs: &[PathBuf]) -> usize {
        let Some(db) = &self.db else { return 0 };
        let mut files = Vec::new();
        for d in dirs {
            collect(d, &mut files, 0);
        }
        files.truncate(5000);
        let tracks: Vec<Track> = files.iter().map(|f| Track::read(f)).collect();
        let _ = db.with(|c| {
            for t in &tracks {
                c.execute(
                    "INSERT OR REPLACE INTO music_tracks (path, title, artist, album, duration) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![t.path.to_string_lossy(), t.title, t.artist, t.album, t.duration_s],
                )?;
            }
            Ok(())
        });
        tracks.len()
    }

    pub fn library(&self, query: &str) -> Vec<Track> {
        let Some(db) = &self.db else { return vec![] };
        let q = format!("%{}%", query.to_lowercase());
        db.with(|c| {
            let mut st = c.prepare(
                "SELECT path, title, artist, album, duration FROM music_tracks
                 WHERE lower(title || ' ' || artist || ' ' || album) LIKE ?1
                 ORDER BY artist, album, title LIMIT 500",
            )?;
            let r = st.query_map([q], |r| {
                Ok(Track {
                    path: PathBuf::from(r.get::<_, String>(0)?),
                    title: r.get(1)?,
                    artist: r.get(2)?,
                    album: r.get(3)?,
                    duration_s: r.get(4)?,
                })
            })?;
            r.collect()
        })
        .unwrap_or_default()
    }

    pub fn playlists(&self) -> Vec<(i64, String, usize)> {
        let Some(db) = &self.db else { return vec![] };
        db.with(|c| {
            let mut st = c.prepare(
                "SELECT p.id, p.name, COUNT(i.path) FROM music_playlists p
                 LEFT JOIN music_playlist_items i ON i.playlist = p.id GROUP BY p.id ORDER BY p.name",
            )?;
            let r = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as usize)))?;
            r.collect()
        })
        .unwrap_or_default()
    }

    pub fn save_playlist(&self, name: &str, tracks: &[Track]) -> i64 {
        let Some(db) = &self.db else { return -1 };
        db.with(|c| {
            c.execute("INSERT INTO music_playlists (name) VALUES (?1)", [name])?;
            let id = c.last_insert_rowid();
            for (i, t) in tracks.iter().enumerate() {
                c.execute(
                    "INSERT INTO music_playlist_items (playlist, pos, path) VALUES (?1, ?2, ?3)",
                    params![id, i as i64, t.path.to_string_lossy()],
                )?;
            }
            Ok(id)
        })
        .unwrap_or(-1)
    }

    pub fn playlist_tracks(&self, id: i64) -> Vec<Track> {
        let Some(db) = &self.db else { return vec![] };
        let paths: Vec<String> = db
            .with(|c| {
                let mut st = c.prepare("SELECT path FROM music_playlist_items WHERE playlist = ?1 ORDER BY pos")?;
                let r = st.query_map([id], |r| r.get(0))?;
                r.collect()
            })
            .unwrap_or_default();
        paths.into_iter().map(PathBuf::from).filter(|p| p.exists()).map(|p| Track::read(&p)).collect()
    }

    pub fn delete_playlist(&self, id: i64) {
        if let Some(db) = &self.db {
            let _ = db.with(|c| {
                c.execute("DELETE FROM music_playlist_items WHERE playlist = ?1", [id])?;
                c.execute("DELETE FROM music_playlists WHERE id = ?1", [id])
            });
        }
    }
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 8 || out.len() >= 5000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out, depth + 1);
        } else if is_audio(&p) {
            out.push(p);
        }
    }
}

fn audio_thread(rx: Receiver<Cmd>, state: Arc<Mutex<PlayerState>>) {
    let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(mut s) => {
            s.log_on_drop(false);
            s
        }
        Err(e) => {
            if let Ok(mut st) = state.lock() {
                st.error = Some(format!("audio: {e}"));
            }
            return;
        }
    };
    let player = rodio::Player::connect_new(sink.mixer());
    let update = |f: &mut dyn FnMut(&mut PlayerState)| {
        if let Ok(mut s) = state.lock() {
            f(&mut s);
            s.seq += 1;
        }
    };
    let load = |t: Option<Track>, player: &rodio::Player| -> Option<String> {
        player.clear();
        let t = t?;
        match std::fs::File::open(&t.path).map_err(|e| e.to_string()).and_then(|f| rodio::Decoder::try_from(f).map_err(|e| e.to_string())) {
            Ok(src) => {
                player.append(src);
                player.play();
                None
            }
            Err(e) => Some(format!("{}: {e}", t.path.display())),
        }
    };
    let mut started = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Cmd::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(cmd) => {
                let mut q = state.lock().map(|s| s.queue.clone()).unwrap_or_default();
                let mut reload = false;
                match cmd {
                    Cmd::PlayQueue(tracks, start) => {
                        q.set(tracks, start);
                        reload = true;
                    }
                    Cmd::Toggle => {
                        if player.empty() && q.current().is_some() {
                            reload = true;
                        } else if player.is_paused() {
                            player.play();
                        } else {
                            player.pause();
                        }
                    }
                    Cmd::Next => reload = q.next(false).is_some() || { player.clear(); false },
                    Cmd::Previous => {
                        if player.get_pos() > Duration::from_secs(3) {
                            let _ = player.try_seek(Duration::ZERO);
                        } else {
                            q.previous();
                            reload = true;
                        }
                    }
                    Cmd::Seek(s) => {
                        let _ = player.try_seek(Duration::from_secs_f64(s.max(0.0)));
                    }
                    Cmd::Volume(v) => player.set_volume(v.clamp(0.0, 1.0) as _),
                    Cmd::Shuffle(on) => q.set_shuffle(on),
                    Cmd::Repeat(r) => q.repeat = r,
                    Cmd::Stop => {
                        player.clear();
                        started = false;
                    }
                    Cmd::Quit => unreachable!(),
                }
                let mut err = None;
                let mut cover = None;
                if reload {
                    err = load(q.current().cloned(), &player);
                    started = err.is_none();
                    cover = q.current().and_then(|t| Track::cover(&t.path));
                }
                let vol = player.volume() as f32;
                update(&mut |s| {
                    s.queue = q.clone();
                    s.volume = vol;
                    if reload {
                        s.cover = cover.clone();
                        s.error = err.clone();
                    }
                });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        // Track ended → advance.
        if started && player.empty() {
            let mut q = state.lock().map(|s| s.queue.clone()).unwrap_or_default();
            let next = q.next(true).cloned();
            let err = load(next.clone(), &player);
            started = next.is_some() && err.is_none();
            let cover = next.as_ref().and_then(|t| Track::cover(&t.path));
            update(&mut |s| {
                s.queue = q.clone();
                s.cover = cover.clone();
                s.error = err.clone();
            });
        }
        let (playing, pos) = (started && !player.is_paused() && !player.empty(), player.get_pos().as_secs_f64());
        if let Ok(mut s) = state.lock() {
            if s.playing != playing || (s.position_s - pos).abs() >= 1.0 {
                s.playing = playing;
                s.position_s = pos;
                s.seq += 1;
            }
        }
    }
}

pub struct Music {
    handle: MusicHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Music {
    pub fn new(db: &Db) -> Self {
        let ok = db
            .with(|c| {
                c.execute_batch(
                    "CREATE TABLE IF NOT EXISTS music_tracks (path TEXT PRIMARY KEY, title TEXT, artist TEXT, album TEXT, duration REAL);
                     CREATE TABLE IF NOT EXISTS music_playlists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                     CREATE TABLE IF NOT EXISTS music_playlist_items (playlist INTEGER NOT NULL, pos INTEGER NOT NULL, path TEXT NOT NULL);",
                )
            })
            .map_err(|e| log::error!("music db: {e:#}"))
            .is_ok();
        let state = PlayerState { volume: 1.0, ..Default::default() };
        Self {
            handle: MusicHandle { state: Arc::new(Mutex::new(state)), tx: Arc::new(Mutex::new(None)), db: ok.then(|| db.clone()) },
            join: None,
        }
    }

    pub fn handle(&self) -> MusicHandle {
        self.handle.clone()
    }
}

impl Module for Music {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let (tx, rx) = channel();
        *self.handle.tx.lock().unwrap() = Some(tx);
        let state = self.handle.state.clone();
        self.join = Some(std::thread::Builder::new().name("halo-music".into()).spawn(move || audio_thread(rx, state))?);
        Ok(())
    }

    fn stop(&mut self) {
        self.handle.send(Cmd::Quit);
        *self.handle.tx.lock().unwrap() = None;
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Explorer "Lire avec Halo" for audio files (HKCU, current user only).
pub fn register_explorer_verb(exe: &Path) -> bool {
    AUDIO_EXT.iter().all(|ext| {
        let key = format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\HaloPlay");
        halo_platform::registry::set_string(&key, "", "Lire avec Halo")
            && halo_platform::registry::set_string(&key, "Icon", &format!("\"{}\"", exe.display()))
            && halo_platform::registry::set_string(&format!("{key}\\command"), "", &format!("\"{}\" --play \"%1\"", exe.display()))
    })
}

pub fn unregister_explorer_verb() {
    for ext in AUDIO_EXT {
        halo_platform::registry::delete_tree(&format!("Software\\Classes\\SystemFileAssociations\\.{ext}\\shell\\HaloPlay"));
    }
}

pub fn explorer_verb_registered() -> bool {
    halo_platform::registry::get_string("Software\\Classes\\SystemFileAssociations\\.mp3\\shell\\HaloPlay\\command", "").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(n: &str) -> Track {
        Track { path: PathBuf::from(format!("{n}.mp3")), title: n.into(), ..Default::default() }
    }

    #[test]
    fn queue_order_repeat_shuffle() {
        let mut q = Queue::default();
        q.set(vec![tr("a"), tr("b"), tr("c")], 1);
        assert_eq!(q.current().unwrap().title, "b");
        assert_eq!(q.next(true).unwrap().title, "c");
        assert!(q.next(true).is_none(), "repeat off ends");
        q.repeat = Repeat::All;
        assert_eq!(q.next(true).unwrap().title, "a");
        q.repeat = Repeat::One;
        assert_eq!(q.next(true).unwrap().title, "a");
        assert_eq!(q.next(false).unwrap().title, "b", "manual next ignores repeat-one");
        assert_eq!(q.previous().unwrap().title, "a");
        q.set_shuffle(true);
        assert_eq!(q.current().unwrap().title, "a", "shuffle keeps current first");
        let mut seen: Vec<usize> = q.order.clone();
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2]);
        q.set_shuffle(false);
        assert_eq!(q.current().unwrap().title, "a");
    }

    #[test]
    fn decodes_and_tags_a_generated_wav() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tone.wav");
        // 0.5 s of silence, 8 kHz mono 16-bit.
        let data = vec![0u8; 8000];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(&p, wav).unwrap();
        let t = Track::read(&p);
        assert_eq!(t.title, "tone");
        assert!((t.duration_s - 0.5).abs() < 0.05, "{}", t.duration_s);
        assert!(rodio::Decoder::try_from(std::fs::File::open(&p).unwrap()).is_ok());
        // Library + playlists on an in-memory db.
        let m = Music::new(&Db::memory());
        let h = m.handle();
        assert_eq!(h.scan(&[dir.path().to_path_buf()]), 1);
        assert_eq!(h.library("TON").len(), 1);
        let id = h.save_playlist("Test", &h.library(""));
        assert_eq!(h.playlists()[0], (id, "Test".into(), 1));
        assert_eq!(h.playlist_tracks(id).len(), 1);
        h.delete_playlist(id);
        assert!(h.playlists().is_empty());
    }
}
