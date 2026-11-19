//! M4 — Activity tracking (opt-in, replaces the out-of-scope keylogger).
//!
//! Records which application is in the foreground and for how long. Guarantees:
//! no keystrokes, no keys, no clipboard, no screenshots; window titles only when
//! explicitly enabled; exclusion list; pause; idle time not counted; local only.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::config::Config;
use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use rusqlite::params;

use crate::store::{Db, now};
use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "activity",
    cdc: "M4",
    name_key: "module.activity",
    category: Category::Monitoring,
    icon: "chart-pie",
    permissions: &[Permission::ForegroundWindow],
    opt_in: true,
    default_enabled: false,
};

#[derive(Clone, Debug)]
pub struct Rules {
    pub titles: bool,
    pub exclude: Vec<String>,
    pub idle_secs: u32,
    pub categories: BTreeMap<String, String>,
}

impl Rules {
    pub fn from_config(c: &Config) -> Self {
        let s = &c.settings;
        Self {
            titles: s.activity_titles,
            exclude: s.activity_exclude_apps.clone(),
            idle_secs: s.activity_idle_secs.max(30),
            categories: s.activity_categories.clone(),
        }
    }

    pub fn category(&self, app: &str) -> String {
        if let Some(c) = self.categories.iter().find(|(k, _)| k.eq_ignore_ascii_case(app)).map(|(_, v)| v) {
            return c.clone();
        }
        default_category(app).to_string()
    }
}

/// Built-in categories (user overrides win).
pub fn default_category(app: &str) -> &'static str {
    let a = app.to_lowercase();
    let has = |xs: &[&str]| xs.iter().any(|x| a.contains(x));
    if has(&["code", "devenv", "idea", "rider", "pycharm", "clion", "zed", "nvim", "terminal", "powershell", "cmd.exe", "wt.exe"]) {
        "Développement"
    } else if has(&["chrome", "firefox", "msedge", "brave", "opera", "arc"]) {
        "Web"
    } else if has(&["slack", "teams", "discord", "outlook", "thunderbird", "whatsapp", "telegram", "signal"]) {
        "Communication"
    } else if has(&["winword", "excel", "powerpnt", "onenote", "notion", "obsidian", "acrobat"]) {
        "Bureautique"
    } else if has(&["figma", "photoshop", "illustrator", "blender", "premiere", "afterfx", "davinci", "gimp", "krita"]) {
        "Création"
    } else if has(&["spotify", "vlc", "netflix", "steam", "epicgames", "music"]) {
        "Divertissement"
    } else {
        "Autre"
    }
}

/// Current foreground session being accumulated in memory.
#[derive(Clone, Debug, PartialEq)]
struct Current {
    app: String,
    title: String,
    start: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppTotal {
    pub app: String,
    pub category: String,
    pub seconds: i64,
}

#[derive(Clone)]
pub struct ActivityHandle {
    db: Db,
    rules: Arc<Mutex<Rules>>,
    paused: Arc<AtomicBool>,
    current: Arc<Mutex<Option<Current>>>,
}

impl ActivityHandle {
    pub fn new(db: Db, rules: Rules) -> anyhow::Result<Self> {
        db.with(|c| {
            c.execute_batch(
                "CREATE TABLE IF NOT EXISTS activity (
                    id INTEGER PRIMARY KEY,
                    app TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT '',
                    start INTEGER NOT NULL,
                    end INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS activity_start ON activity(start);",
            )
        })?;
        Ok(Self {
            db,
            rules: Arc::new(Mutex::new(rules)),
            paused: Arc::new(AtomicBool::new(false)),
            current: Arc::new(Mutex::new(None)),
        })
    }

    pub fn rules(&self) -> Rules {
        self.rules.lock().map(|r| r.clone()).unwrap()
    }

    pub fn set_rules(&self, r: Rules) {
        if let Ok(mut cur) = self.rules.lock() {
            *cur = r;
        }
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, p: bool) {
        self.paused.store(p, Ordering::Relaxed);
        if p {
            self.flush(now());
        }
    }

    /// Feed the foreground app at time `t` (None = idle/locked/paused).
    pub fn observe(&self, t: i64, app: Option<(&str, &str)>) {
        let rules = self.rules();
        let app = app.filter(|(a, _)| {
            !a.is_empty() && !a.eq_ignore_ascii_case("halo.exe") && !rules.exclude.iter().any(|x| x.eq_ignore_ascii_case(a))
        });
        let next = app.map(|(a, title)| Current {
            app: a.to_string(),
            title: if rules.titles { title.chars().take(200).collect() } else { String::new() },
            start: t,
        });
        let mut cur = self.current.lock().unwrap();
        let same = match (&*cur, &next) {
            (Some(c), Some(n)) => c.app == n.app && c.title == n.title,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some(c) = cur.take() {
            self.write(&c, t);
        }
        *cur = next;
    }

    /// Close the current session at `t` (pause/stop/shutdown).
    pub fn flush(&self, t: i64) {
        if let Some(c) = self.current.lock().unwrap().take() {
            self.write(&c, t);
        }
    }

    fn write(&self, c: &Current, end: i64) {
        if end - c.start < 1 {
            return;
        }
        let _ = self.db.with(|db| {
            db.execute(
                "INSERT INTO activity (app, title, start, end) VALUES (?1, ?2, ?3, ?4)",
                params![c.app, c.title, c.start, end],
            )
        });
    }

    /// Per-app totals for `[from, to)`, including the open session; longest first.
    pub fn totals(&self, from: i64, to: i64) -> Vec<AppTotal> {
        let rules = self.rules();
        let mut map: BTreeMap<String, i64> = BTreeMap::new();
        let rows: Vec<(String, i64)> = self
            .db
            .with(|c| {
                let mut st = c.prepare(
                    "SELECT app, SUM(MIN(end, ?2) - MAX(start, ?1)) FROM activity
                     WHERE end > ?1 AND start < ?2 GROUP BY app",
                )?;
                let r = st.query_map(params![from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
                r.collect()
            })
            .unwrap_or_default();
        for (app, s) in rows {
            *map.entry(app).or_default() += s;
        }
        if let Some(c) = &*self.current.lock().unwrap() {
            let s = to.min(now()) - c.start.max(from);
            if s > 0 {
                *map.entry(c.app.clone()).or_default() += s;
            }
        }
        let mut v: Vec<AppTotal> =
            map.into_iter().map(|(app, seconds)| AppTotal { category: rules.category(&app), app, seconds }).collect();
        v.sort_by(|a, b| b.seconds.cmp(&a.seconds));
        v
    }

    /// Totals grouped by category.
    pub fn by_category(&self, from: i64, to: i64) -> Vec<(String, i64)> {
        let mut map: BTreeMap<String, i64> = BTreeMap::new();
        for t in self.totals(from, to) {
            *map.entry(t.category).or_default() += t.seconds;
        }
        let mut v: Vec<_> = map.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }

    /// Delete all recorded activity.
    pub fn erase(&self) {
        *self.current.lock().unwrap() = None;
        let _ = self.db.with(|c| c.execute("DELETE FROM activity", []));
    }
}

/// Local midnight (unix seconds) `days_ago` days back.
pub fn day_start(days_ago: i64) -> i64 {
    use chrono::{Local, TimeZone};
    let d = Local::now().date_naive() - chrono::Duration::days(days_ago);
    Local
        .from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(now)
}

/// "2 h 05", "12 min", "45 s".
pub fn format_duration(s: i64) -> String {
    if s >= 3600 {
        format!("{} h {:02}", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

pub struct ActivityTracker {
    handle: Option<ActivityHandle>,
    running: Option<(halo_platform::hooks::HookThread, Worker)>,
}

impl ActivityTracker {
    pub fn new(db: &Db, config: &Config) -> Self {
        let handle = ActivityHandle::new(db.clone(), Rules::from_config(config))
            .map_err(|e| log::error!("activity db: {e:#}"))
            .ok();
        Self { handle, running: None }
    }

    pub fn handle(&self) -> Option<ActivityHandle> {
        self.handle.clone()
    }
}

fn sample(h: &ActivityHandle, hwnd: isize) {
    let t = now();
    let idle = halo_platform::proc::idle_seconds() >= h.rules().idle_secs;
    if h.paused() || idle || hwnd == 0 {
        h.observe(t, None);
        return;
    }
    let pid = halo_platform::proc::window_pid(hwnd);
    let app = halo_platform::proc::process_exe(pid).unwrap_or_default();
    let title = if h.rules().titles { halo_platform::proc::window_title(hwnd) } else { String::new() };
    h.observe(t, Some((&app, &title)));
}

impl Module for ActivityTracker {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn configure(&mut self, config: &Config) -> bool {
        if let Some(h) = &self.handle {
            h.set_rules(Rules::from_config(config));
        }
        false
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let h = self.handle.clone().ok_or_else(|| anyhow::anyhow!("activity database unavailable"))?;
        let (hook, rx) = halo_platform::hooks::foreground();
        let rx = Mutex::new(rx);
        sample(&h, halo_platform::proc::foreground_window());
        // Event-driven on focus changes, plus a 5 s heartbeat for idle detection
        // and title changes. Sessions are flushed every minute so a crash loses little.
        let mut beats = 0u32;
        let worker = Worker::every("halo-activity", Duration::from_secs(1), move || {
            let mut fg = None;
            if let Ok(rx) = rx.lock() {
                while let Ok(h) = rx.try_recv() {
                    fg = Some(h);
                }
            }
            beats += 1;
            if let Some(w) = fg {
                sample(&h, w);
            } else if beats % 5 == 0 {
                sample(&h, halo_platform::proc::foreground_window());
            }
            if beats % 60 == 0 {
                let t = now();
                h.flush(t);
                sample(&h, halo_platform::proc::foreground_window());
            }
            true
        });
        self.running = Some((hook, worker));
        Ok(())
    }

    fn stop(&mut self) {
        self.running = None;
        if let Some(h) = &self.handle {
            h.flush(now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules { titles: false, exclude: vec!["Secret.exe".into()], idle_secs: 300, categories: BTreeMap::new() }
    }

    #[test]
    fn sessions_totals_exclusions() {
        let h = ActivityHandle::new(Db::memory(), rules()).unwrap();
        h.observe(1000, Some(("Code.exe", "main.rs — Halo")));
        h.observe(1600, Some(("chrome.exe", "Docs")));
        h.observe(1900, Some(("secret.exe", "x"))); // excluded → counts as nothing
        h.observe(2500, Some(("Code.exe", "")));
        h.flush(2800);
        let t = h.totals(0, 10_000);
        assert_eq!(t[0], AppTotal { app: "Code.exe".into(), category: "Développement".into(), seconds: 900 });
        assert_eq!(t[1].seconds, 300);
        assert!(t.iter().all(|x| !x.app.eq_ignore_ascii_case("secret.exe")));
        // Window clipping.
        assert_eq!(h.totals(1500, 1700).iter().map(|x| x.seconds).sum::<i64>(), 200);
        let cats = h.by_category(0, 10_000);
        assert_eq!(cats[0], ("Développement".into(), 900));
    }

    #[test]
    fn titles_only_when_enabled_and_idle_gaps() {
        let db = Db::memory();
        let h = ActivityHandle::new(db.clone(), rules()).unwrap();
        h.observe(0, Some(("Code.exe", "private title")));
        h.observe(100, None); // idle
        h.observe(400, Some(("Code.exe", "private title")));
        h.flush(500);
        let titles: Vec<String> = db
            .with(|c| {
                let mut st = c.prepare("SELECT title FROM activity")?;
                let r = st.query_map([], |r| r.get(0))?;
                r.collect()
            })
            .unwrap();
        assert!(titles.iter().all(String::is_empty), "titles not stored by default");
        assert_eq!(h.totals(0, 1000)[0].seconds, 200, "idle gap excluded");
        h.erase();
        assert!(h.totals(0, 1000).is_empty());
    }

    #[test]
    fn categories_and_format() {
        let mut r = rules();
        r.categories.insert("chrome.exe".into(), "Recherche".into());
        assert_eq!(r.category("Chrome.exe"), "Recherche");
        assert_eq!(default_category("Slack.exe"), "Communication");
        assert_eq!(format_duration(7500), "2 h 05");
        assert_eq!(format_duration(720), "12 min");
        assert!(day_start(0) <= now() && now() - day_start(0) < 86_400 + 3600);
    }
}
