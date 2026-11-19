//! M6 — Notes: multiple notes, folders, pins, search, auto-save, Markdown export.
//! Body is light Markdown (titles, lists, `- [ ]` checkboxes, **bold**, *italic*).

use std::path::{Path, PathBuf};

use halo_core::module::{Category, Manifest, Module, ModuleContext};
use rusqlite::params;

use crate::store::{Db, now};

pub const MANIFEST: Manifest = Manifest {
    id: "notes",
    cdc: "M6",
    name_key: "module.notes",
    category: Category::Productivity,
    icon: "notebook-pen",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub id: i64,
    pub folder: String,
    pub body: String,
    pub pinned: bool,
    pub updated: i64,
}

impl Note {
    /// First non-empty line without Markdown heading marks; empty if the note has
    /// no text yet (the UI shows a localized "Untitled").
    pub fn title(&self) -> String {
        self.body
            .lines()
            .map(|l| l.trim().trim_start_matches('#').trim())
            .find(|l| !l.is_empty())
            .unwrap_or("")
            .chars()
            .take(80)
            .collect()
    }

    /// Second meaningful line, for list previews, with Markdown marks removed.
    pub fn preview(&self) -> String {
        let line = self.body.lines().map(str::trim).filter(|l| !l.is_empty() && *l != "#").nth(1).unwrap_or("");
        let plain = strip_markdown(line);
        let mut t = plain.trim();
        for mark in ["[x] ", "[ ] ", "- ", "+ "] {
            t = t.strip_prefix(mark).unwrap_or(t);
        }
        t.trim().chars().take(90).collect()
    }

    /// Number of words in the note, Markdown marks (headings, lists, checkboxes) excluded.
    pub fn word_count(&self) -> usize {
        strip_markdown(&self.body)
            .split_whitespace()
            .filter(|w| *w != "[x]" && w.chars().any(char::is_alphanumeric))
            .count()
    }
}

#[derive(Clone)]
pub struct NotesHandle {
    db: Db,
}

impl NotesHandle {
    pub fn new(db: Db) -> anyhow::Result<Self> {
        db.with(|c| {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS notes (
                    id INTEGER PRIMARY KEY,
                    folder TEXT NOT NULL DEFAULT '',
                    body TEXT NOT NULL DEFAULT '',
                    pinned INTEGER NOT NULL DEFAULT 0,
                    updated INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS notes_updated ON notes(updated);",
            )
        })?;
        Ok(Self { db })
    }

    /// Notes matching `query` (case-insensitive) in `folder` (None = all), pinned first.
    pub fn list(&self, query: &str, folder: Option<&str>) -> Vec<Note> {
        let q = format!("%{}%", query.to_lowercase());
        self.db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT id, folder, body, pinned, updated FROM notes
                     WHERE lower(body) LIKE ?1 AND (?2 IS NULL OR folder = ?2)
                     ORDER BY pinned DESC, updated DESC",
                )?;
                let rows = st.query_map(params![q, folder], row)?;
                rows.collect()
            })
            .unwrap_or_default()
    }

    pub fn get(&self, id: i64) -> Option<Note> {
        self.db
            .with(|c| c.query_row("SELECT id, folder, body, pinned, updated FROM notes WHERE id = ?1", [id], row))
            .ok()
    }

    pub fn create(&self, body: &str, folder: &str) -> i64 {
        self.db
            .with(|c| {
                c.execute("INSERT INTO notes (folder, body, updated) VALUES (?1, ?2, ?3)", params![folder, body, now()])?;
                Ok(c.last_insert_rowid())
            })
            .unwrap_or(-1)
    }

    /// Auto-save: only writes when the body actually changed.
    pub fn save(&self, id: i64, body: &str) -> bool {
        self.db
            .with(|c| c.execute("UPDATE notes SET body = ?2, updated = ?3 WHERE id = ?1 AND body <> ?2", params![id, body, now()]))
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) {
        let _ = self.db.with(|c| c.execute("UPDATE notes SET pinned = ?2 WHERE id = ?1", params![id, pinned]));
    }

    pub fn set_folder(&self, id: i64, folder: &str) {
        let _ = self.db.with(|c| c.execute("UPDATE notes SET folder = ?2 WHERE id = ?1", params![id, folder]));
    }

    pub fn delete(&self, id: i64) {
        let _ = self.db.with(|c| c.execute("DELETE FROM notes WHERE id = ?1", [id]));
    }

    pub fn folders(&self) -> Vec<String> {
        self.db
            .with(|c| {
                let mut st = c.prepare("SELECT DISTINCT folder FROM notes WHERE folder <> '' ORDER BY folder")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap_or_default()
    }

    /// Export every note as `<title>.md` (or `.txt`) into `dir`. Returns written files.
    pub fn export(&self, dir: &Path, markdown: bool) -> anyhow::Result<Vec<PathBuf>> {
        std::fs::create_dir_all(dir)?;
        let mut out = Vec::new();
        for n in self.list("", None) {
            let mut name: String =
                n.title().chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
            if name.trim().is_empty() {
                name = format!("note-{}", n.id);
            }
            let mut path = dir.join(format!("{name}.{}", if markdown { "md" } else { "txt" }));
            let mut i = 2;
            while path.exists() {
                path = dir.join(format!("{name} ({i}).{}", if markdown { "md" } else { "txt" }));
                i += 1;
            }
            let body = if markdown { n.body.clone() } else { strip_markdown(&n.body) };
            std::fs::write(&path, body)?;
            out.push(path);
        }
        Ok(out)
    }
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note { id: r.get(0)?, folder: r.get(1)?, body: r.get(2)?, pinned: r.get(3)?, updated: r.get(4)? })
}

/// Plain-text export: drop heading marks, emphasis and checkbox syntax.
pub fn strip_markdown(s: &str) -> String {
    s.lines()
        .map(|l| {
            let t = l.trim_start();
            let t = t.trim_start_matches('#').trim_start();
            let t = t.replace("- [x] ", "[x] ").replace("- [ ] ", "[ ] ");
            t.replace("**", "").replace('*', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Toggle the n-th Markdown checkbox (`- [ ]` ↔ `- [x]`) in `body`.
pub fn toggle_checkbox(body: &str, index: usize) -> String {
    let mut seen = 0;
    body.lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with("- [ ] ") || t.starts_with("- [x] ") {
                seen += 1;
                if seen - 1 == index {
                    return if t.starts_with("- [ ] ") { l.replacen("- [ ] ", "- [x] ", 1) } else { l.replacen("- [x] ", "- [ ] ", 1) };
                }
            }
            l.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct Notes {
    handle: Option<NotesHandle>,
}

impl Notes {
    pub fn new(db: &Db) -> Self {
        Self { handle: NotesHandle::new(db.clone()).map_err(|e| log::error!("notes db: {e:#}")).ok() }
    }

    pub fn handle(&self) -> Option<NotesHandle> {
        self.handle.clone()
    }
}

impl Module for Notes {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        anyhow::ensure!(self.handle.is_some(), "notes database unavailable");
        Ok(())
    }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h() -> NotesHandle {
        NotesHandle::new(Db::memory()).unwrap()
    }

    #[test]
    fn crud_search_pin() {
        let n = h();
        let a = n.create("# Courses\n- [ ] pain\n- [x] lait", "Perso");
        let b = n.create("Idées projet Halo", "");
        assert_eq!(n.list("", None).len(), 2);
        assert_eq!(n.list("HALO", None)[0].id, b);
        assert_eq!(n.list("", Some("Perso"))[0].id, a);
        n.set_pinned(b, false);
        n.set_pinned(a, true);
        assert_eq!(n.list("", None)[0].id, a, "pinned first");
        assert_eq!(n.get(a).unwrap().title(), "Courses");
        assert!(n.save(a, "# Courses\n- [x] pain"));
        assert!(!n.save(a, "# Courses\n- [x] pain"), "no-op save skipped");
        assert_eq!(n.folders(), vec!["Perso".to_string()]);
        n.delete(b);
        assert!(n.get(b).is_none());
    }

    #[test]
    fn title_preview_and_words() {
        let note = |body: &str| Note { id: 1, folder: String::new(), body: body.into(), pinned: false, updated: 0 };
        assert_eq!(note("# ").title(), "", "new note has no title (UI shows Untitled)");
        let n = note("# Groceries\n\n**Bread** and *milk*\n- [ ] eggs");
        assert_eq!(n.title(), "Groceries");
        assert_eq!(n.preview(), "Bread and milk");
        assert_eq!(n.word_count(), 5);
        assert_eq!(note("# \n").word_count(), 0);
        let list = note("# Groceries\n- [x] bread\n- [ ] coffee");
        assert_eq!(list.preview(), "bread", "list and checkbox marks hidden");
        assert_eq!(list.word_count(), 3);
    }

    #[test]
    fn checkboxes_and_plain_export() {
        let body = "# T\n- [ ] a\n- [ ] b";
        assert_eq!(toggle_checkbox(body, 1), "# T\n- [ ] a\n- [x] b");
        assert_eq!(strip_markdown("## **Gras** *it*\n- [x] fait"), "Gras it\n[x] fait");
    }

    #[test]
    fn export_files() {
        let n = h();
        n.create("# Même titre\nx", "");
        n.create("# Même titre\ny", "");
        let dir = tempfile::tempdir().unwrap();
        let files = n.export(dir.path(), true).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.exists()));
    }
}
