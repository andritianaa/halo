//! M7 — Persistent clipboard history (opt-in).
//!
//! Privacy: never records content flagged as sensitive by its source (password
//! managers), skips excluded apps, optional DPAPI encryption at rest, optional
//! auto-clear. Everything stays in the local database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::config::Config;
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::store::{Db, now};
use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "clipboard",
    cdc: "M7",
    name_key: "module.clipboard",
    category: Category::Productivity,
    icon: "clipboard",
    permissions: &[Permission::ClipboardRead],
    opt_in: true,
    default_enabled: false,
};

#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    /// PNG-encoded image.
    Image { png: Vec<u8>, width: u32, height: u32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub content: Content,
    pub source: String,
    pub created: i64,
    pub pinned: bool,
}

impl Entry {
    pub fn preview(&self) -> String {
        match &self.content {
            Content::Text(t) => t.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ⏎ ").chars().take(160).collect(),
            Content::Image { width, height, .. } => format!("Image {width}×{height}"),
        }
    }

    pub fn is_link(&self) -> bool {
        matches!(&self.content, Content::Text(t) if (t.starts_with("http://") || t.starts_with("https://")) && !t.trim().contains(char::is_whitespace))
    }
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub capacity: u32,
    pub encrypt: bool,
    pub autoclear_minutes: u32,
    pub images: bool,
    pub exclude_apps: Vec<String>,
}

impl Policy {
    pub fn from_config(c: &Config) -> Self {
        let s = &c.settings;
        Self {
            capacity: s.clip_capacity.max(10),
            encrypt: s.clip_encrypt,
            autoclear_minutes: s.clip_autoclear_minutes,
            images: s.clip_images,
            exclude_apps: s.clip_exclude_apps.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ClipboardHandle {
    db: Db,
    policy: Arc<Mutex<Policy>>,
}

fn hash(kind: &str, data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(kind.as_bytes());
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl ClipboardHandle {
    pub fn new(db: Db, policy: Policy) -> anyhow::Result<Self> {
        db.with(|c| {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS clipboard (
                    id INTEGER PRIMARY KEY,
                    kind TEXT NOT NULL,
                    data BLOB NOT NULL,
                    width INTEGER NOT NULL DEFAULT 0,
                    height INTEGER NOT NULL DEFAULT 0,
                    hash TEXT NOT NULL UNIQUE,
                    source TEXT NOT NULL DEFAULT '',
                    created INTEGER NOT NULL,
                    pinned INTEGER NOT NULL DEFAULT 0,
                    encrypted INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS clipboard_created ON clipboard(created);",
            )
        })?;
        Ok(Self { db, policy: Arc::new(Mutex::new(policy)) })
    }

    pub fn set_policy(&self, p: Policy) {
        if let Ok(mut cur) = self.policy.lock() {
            *cur = p;
        }
    }

    fn policy(&self) -> Policy {
        self.policy.lock().map(|p| p.clone()).unwrap_or_else(|_| Policy {
            capacity: 500,
            encrypt: false,
            autoclear_minutes: 0,
            images: true,
            exclude_apps: vec![],
        })
    }

    /// Record a clip. Duplicates move to the top. Returns false if skipped.
    pub fn record(&self, content: Content, source: &str) -> bool {
        let p = self.policy();
        if p.exclude_apps.iter().any(|a| a.eq_ignore_ascii_case(source)) {
            return false;
        }
        let (kind, raw, w, h) = match &content {
            Content::Text(t) if !t.trim().is_empty() => ("text", t.as_bytes().to_vec(), 0, 0),
            Content::Image { png, width, height } if p.images => ("image", png.clone(), *width, *height),
            _ => return false,
        };
        let digest = hash(kind, &raw);
        let (data, enc) = match p.encrypt.then(|| halo_platform::secure::protect(&raw)).flatten() {
            Some(e) => (e, true),
            None => (raw, false),
        };
        let ok = self
            .db
            .with(|c| {
                let existing: Option<i64> =
                    c.query_row("SELECT id FROM clipboard WHERE hash = ?1", [&digest], |r| r.get(0)).optional()?;
                match existing {
                    Some(id) => c.execute("UPDATE clipboard SET created = ?2, source = ?3 WHERE id = ?1", params![id, now(), source])?,
                    None => c.execute(
                        "INSERT INTO clipboard (kind, data, width, height, hash, source, created, encrypted)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        params![kind, data, w, h, digest, source, now(), enc],
                    )?,
                };
                Ok(())
            })
            .is_ok();
        self.enforce(&p);
        ok
    }

    /// Capacity + auto-clear (pinned entries are kept).
    fn enforce(&self, p: &Policy) {
        let _ = self.db.with(|c| {
            c.execute(
                "DELETE FROM clipboard WHERE pinned = 0 AND id NOT IN (
                    SELECT id FROM clipboard ORDER BY pinned DESC, created DESC LIMIT ?1)",
                [p.capacity],
            )?;
            if p.autoclear_minutes > 0 {
                c.execute(
                    "DELETE FROM clipboard WHERE pinned = 0 AND created < ?1",
                    [now() - p.autoclear_minutes as i64 * 60],
                )?;
            }
            Ok(())
        });
    }

    pub fn list(&self, query: &str, limit: usize) -> Vec<Entry> {
        let q = query.to_lowercase();
        let rows: Vec<(i64, String, Vec<u8>, u32, u32, String, i64, bool, bool)> = self
            .db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT id, kind, data, width, height, source, created, pinned, encrypted
                     FROM clipboard ORDER BY pinned DESC, created DESC",
                )?;
                let r = st.query_map([], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
                })?;
                r.collect()
            })
            .unwrap_or_default();
        rows.into_iter()
            .filter_map(|(id, kind, data, w, h, source, created, pinned, enc)| {
                let data = if enc { halo_platform::secure::unprotect(&data)? } else { data };
                let content = if kind == "text" {
                    Content::Text(String::from_utf8_lossy(&data).into_owned())
                } else {
                    Content::Image { png: data, width: w, height: h }
                };
                Some(Entry { id, content, source, created, pinned })
            })
            .filter(|e| q.is_empty() || matches!(&e.content, Content::Text(t) if t.to_lowercase().contains(&q)))
            .take(limit)
            .collect()
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) {
        let _ = self.db.with(|c| c.execute("UPDATE clipboard SET pinned = ?2 WHERE id = ?1", params![id, pinned]));
    }

    pub fn delete(&self, id: i64) {
        let _ = self.db.with(|c| c.execute("DELETE FROM clipboard WHERE id = ?1", [id]));
    }

    /// "Vider": removes everything except pinned entries.
    pub fn clear(&self) {
        let _ = self.db.with(|c| c.execute("DELETE FROM clipboard WHERE pinned = 0", []));
    }

    pub fn count(&self) -> usize {
        self.db.with(|c| c.query_row("SELECT COUNT(*) FROM clipboard", [], |r| r.get::<_, i64>(0))).unwrap_or(0) as usize
    }

    /// Put an entry back on the system clipboard.
    pub fn copy(&self, e: &Entry) -> bool {
        let Ok(mut cb) = arboard::Clipboard::new() else { return false };
        match &e.content {
            Content::Text(t) => cb.set_text(t.clone()).is_ok(),
            Content::Image { png, .. } => match image::load_from_memory(png) {
                Ok(img) => {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    cb.set_image(arboard::ImageData { width: w as usize, height: h as usize, bytes: rgba.into_raw().into() }).is_ok()
                }
                Err(_) => false,
            },
        }
    }
}

/// Read the current system clipboard (text first, then image).
fn read_system(images: bool) -> Option<Content> {
    let mut cb = arboard::Clipboard::new().ok()?;
    if let Ok(t) = cb.get_text() {
        return Some(Content::Text(t));
    }
    if images {
        if let Ok(img) = cb.get_image() {
            let (w, h) = (img.width as u32, img.height as u32);
            if (w as u64) * (h as u64) > 40_000_000 {
                return None; // too large to keep
            }
            let buf = image::RgbaImage::from_raw(w, h, img.bytes.into_owned())?;
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(buf)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .ok()?;
            return Some(Content::Image { png, width: w, height: h });
        }
    }
    None
}

pub struct ClipboardHistory {
    handle: Option<ClipboardHandle>,
    listener: Option<(halo_platform::hooks::HookThread, Worker)>,
}

impl ClipboardHistory {
    pub fn new(db: &Db, config: &Config) -> Self {
        let handle = ClipboardHandle::new(db.clone(), Policy::from_config(config))
            .map_err(|e| log::error!("clipboard db: {e:#}"))
            .ok();
        Self { handle, listener: None }
    }

    pub fn handle(&self) -> Option<ClipboardHandle> {
        self.handle.clone()
    }
}

impl Module for ClipboardHistory {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn configure(&mut self, config: &Config) -> bool {
        if let Some(h) = &self.handle {
            h.set_policy(Policy::from_config(config));
        }
        false
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let handle = self.handle.clone().ok_or_else(|| anyhow::anyhow!("clipboard database unavailable"))?;
        let (hook, rx) = halo_platform::hooks::clipboard();
        let rx = Mutex::new(rx);
        let worker = Worker::every("halo-clipboard-reader", Duration::from_millis(100), move || {
            let Ok(rx) = rx.lock() else { return false };
            let mut changed = false;
            while rx.try_recv().is_ok() {
                changed = true;
            }
            if changed {
                // Let the source finish writing all formats.
                std::thread::sleep(Duration::from_millis(60));
                if halo_platform::clipboard::is_sensitive() {
                    return true;
                }
                let source = halo_platform::clipboard::owner_exe().unwrap_or_default();
                if let Some(c) = read_system(handle.policy().images) {
                    handle.record(c, &source);
                }
            }
            true
        });
        self.listener = Some((hook, worker));
        Ok(())
    }

    fn stop(&mut self) {
        self.listener = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy { capacity: 3, encrypt: false, autoclear_minutes: 0, images: true, exclude_apps: vec!["KeePass.exe".into()] }
    }

    #[test]
    fn dedupe_capacity_pin_exclude() {
        let h = ClipboardHandle::new(Db::memory(), policy()).unwrap();
        assert!(h.record(Content::Text("a".into()), "x.exe"));
        assert!(h.record(Content::Text("b".into()), "x.exe"));
        assert!(h.record(Content::Text("a".into()), "x.exe")); // moves to top
        assert_eq!(h.count(), 2);
        let first = h.list("", 10)[0].clone();
        assert_eq!(first.content, Content::Text("a".into()));
        h.set_pinned(h.list("b", 1)[0].id, true);
        for t in ["c", "d", "e"] {
            h.record(Content::Text(t.into()), "x.exe");
        }
        let all = h.list("", 10);
        assert_eq!(all.len(), 3);
        assert!(all[0].pinned, "pinned kept and first");
        assert!(!h.record(Content::Text("secret".into()), "keepass.exe"), "excluded app");
        assert!(!h.record(Content::Text("   ".into()), "x.exe"), "blank");
        h.clear();
        assert_eq!(h.count(), 1, "clear keeps pinned");
    }

    #[test]
    fn encrypted_at_rest_and_searchable() {
        let mut p = policy();
        p.encrypt = true;
        let db = Db::memory();
        let h = ClipboardHandle::new(db.clone(), p).unwrap();
        h.record(Content::Text("mot de passe? non: une note".into()), "x.exe");
        let raw: Vec<u8> = db.with(|c| c.query_row("SELECT data FROM clipboard", [], |r| r.get(0))).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("note"), "stored encrypted");
        assert_eq!(h.list("NOTE", 5).len(), 1);
    }

    #[test]
    fn links_and_previews() {
        let e = Entry { id: 1, content: Content::Text("https://exemple.fr/a".into()), source: String::new(), created: 0, pinned: false };
        assert!(e.is_link());
        let e2 = Entry { content: Content::Text("ligne 1\n\nligne 2".into()), ..e };
        assert_eq!(e2.preview(), "ligne 1 ⏎ ligne 2");
    }
}
