//! Tasks & reminders (notch "Aujourd'hui" card, command bar `tâche …` / `rappel …`).
//! Local SQLite; due reminders surface as notch live activities.

use halo_core::module::{Category, Manifest, Module, ModuleContext};
use rusqlite::params;

use crate::store::{Db, now};

pub const MANIFEST: Manifest = Manifest {
    id: "tasks",
    cdc: "M6+",
    name_key: "module.tasks",
    category: Category::Productivity,
    icon: "list-checks",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: i64,
    pub text: String,
    pub done: bool,
    /// Unix seconds; tasks without a due date belong to "today" until done.
    pub due: Option<i64>,
    /// Reminder fires at `due` when true.
    pub remind: bool,
    pub notified: bool,
}

#[derive(Clone)]
pub struct TasksHandle {
    db: Db,
}

impl TasksHandle {
    pub fn new(db: Db) -> anyhow::Result<Self> {
        db.with(|c| {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS tasks (
                    id INTEGER PRIMARY KEY,
                    text TEXT NOT NULL,
                    done INTEGER NOT NULL DEFAULT 0,
                    due INTEGER,
                    remind INTEGER NOT NULL DEFAULT 0,
                    notified INTEGER NOT NULL DEFAULT 0,
                    created INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS focus_log (
                    id INTEGER PRIMARY KEY,
                    ended INTEGER NOT NULL,
                    minutes INTEGER NOT NULL
                );",
            )
        })?;
        Ok(Self { db })
    }

    pub fn add(&self, text: &str, due: Option<i64>, remind: bool) -> i64 {
        self.db
            .with(|c| {
                c.execute(
                    "INSERT INTO tasks (text, due, remind, created) VALUES (?1, ?2, ?3, ?4)",
                    params![text.trim(), due, remind, now()],
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap_or(-1)
    }

    /// Open tasks due before `until` (or without date) + tasks completed today.
    pub fn today(&self, day_start: i64, until: i64) -> Vec<Task> {
        self.db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT id, text, done, due, remind, notified FROM tasks
                     WHERE (done = 0 AND (due IS NULL OR due < ?2))
                        OR (done = 1 AND created >= ?1 AND (due IS NULL OR due < ?2))
                     ORDER BY done, COALESCE(due, 9e18), id",
                )?;
                let r = st.query_map(params![day_start, until], row)?;
                r.collect()
            })
            .unwrap_or_default()
    }

    pub fn all_open(&self) -> Vec<Task> {
        self.db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT id, text, done, due, remind, notified FROM tasks WHERE done = 0 ORDER BY COALESCE(due, 9e18), id",
                )?;
                let r = st.query_map([], row)?;
                r.collect()
            })
            .unwrap_or_default()
    }

    /// Productivity stats: record a completed focus session.
    pub fn log_focus(&self, minutes: u32) {
        let _ = self.db.with(|c| c.execute("INSERT INTO focus_log (ended, minutes) VALUES (?1, ?2)", params![now(), minutes]));
    }

    /// (sessions, minutes) completed since `from`.
    pub fn focus_stats(&self, from: i64) -> (u32, u32) {
        self.db
            .with(|c| {
                c.query_row("SELECT COUNT(*), COALESCE(SUM(minutes), 0) FROM focus_log WHERE ended >= ?1", [from], |r| {
                    Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32))
                })
            })
            .unwrap_or((0, 0))
    }

    pub fn toggle(&self, id: i64) {
        let _ = self.db.with(|c| c.execute("UPDATE tasks SET done = 1 - done WHERE id = ?1", [id]));
    }

    pub fn delete(&self, id: i64) {
        let _ = self.db.with(|c| c.execute("DELETE FROM tasks WHERE id = ?1", [id]));
    }

    /// Reminders that are due and not yet shown; marks them notified.
    pub fn take_due_reminders(&self, t: i64) -> Vec<Task> {
        let due: Vec<Task> = self
            .db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT id, text, done, due, remind, notified FROM tasks
                     WHERE remind = 1 AND notified = 0 AND done = 0 AND due <= ?1",
                )?;
                let r = st.query_map([t], row)?;
                r.collect()
            })
            .unwrap_or_default();
        for d in &due {
            let _ = self.db.with(|c| c.execute("UPDATE tasks SET notified = 1 WHERE id = ?1", [d.id]));
        }
        due
    }
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task { id: r.get(0)?, text: r.get(1)?, done: r.get(2)?, due: r.get(3)?, remind: r.get(4)?, notified: r.get(5)? })
}

pub struct Tasks {
    handle: Option<TasksHandle>,
}

impl Tasks {
    pub fn new(db: &Db) -> Self {
        Self { handle: TasksHandle::new(db.clone()).map_err(|e| log::error!("tasks db: {e:#}")).ok() }
    }
    pub fn handle(&self) -> Option<TasksHandle> {
        self.handle.clone()
    }
}

impl Module for Tasks {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        anyhow::ensure!(self.handle.is_some(), "tasks database unavailable");
        Ok(())
    }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_and_reminders() {
        let h = TasksHandle::new(Db::memory()).unwrap();
        let a = h.add("Envoyer la proposition", None, false);
        let b = h.add("Réunion", Some(1_000), true);
        h.add("Plus tard", Some(10_000_000_000), false);
        let today = h.today(0, 5_000);
        assert_eq!(today.iter().map(|t| t.id).collect::<Vec<_>>(), vec![b, a]);
        h.toggle(a);
        assert!(h.today(0, 5_000).iter().find(|t| t.id == a).unwrap().done);
        assert_eq!(h.take_due_reminders(999).len(), 0);
        assert_eq!(h.take_due_reminders(1_000)[0].id, b);
        assert!(h.take_due_reminders(2_000).is_empty(), "notified once");
        assert_eq!(h.all_open().len(), 2);
        h.log_focus(25);
        h.log_focus(25);
        assert_eq!(h.focus_stats(0), (2, 50));
        assert_eq!(h.focus_stats(now() + 10), (0, 0));
    }
}
