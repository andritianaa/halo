//! Debug-only command channel used by automated UI tests: the app polls
//! `%TEMP%\halo-devcmd.txt`, runs each line once and truncates the file.
//! Compiled out of release builds.

use std::path::PathBuf;

pub fn path() -> PathBuf {
    std::env::temp_dir().join("halo-devcmd.txt")
}

/// Pending commands (file consumed atomically by renaming).
pub fn take() -> Vec<String> {
    if !cfg!(debug_assertions) {
        return Vec::new();
    }
    let p = path();
    let tmp = p.with_extension("taken");
    let _ = std::fs::remove_file(&tmp); // stale file from a killed run
    if std::fs::rename(&p, &tmp).is_err() {
        return Vec::new();
    }
    let text = std::fs::read_to_string(&tmp).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp);
    text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}
