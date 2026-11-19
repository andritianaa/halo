//! Local SQLite database (`%APPDATA%\Halo\halo.db`) shared by modules — spec §8.
//! Each module owns its tables and creates them on first use.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=2000;")?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// In-memory database for tests.
    pub fn memory() -> Self {
        Self(Arc::new(Mutex::new(Connection::open_in_memory().expect("sqlite in memory"))))
    }

    pub fn default_path() -> std::path::PathBuf {
        crate::worker::data_dir().join("halo.db")
    }

    /// Run `f` with the connection locked.
    pub fn with<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> anyhow::Result<R> {
        let conn = self.0.lock().map_err(|_| anyhow::anyhow!("db poisoned"))?;
        Ok(f(&conn)?)
    }
}

/// Unix seconds now.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
