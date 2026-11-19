//! Launcher index (Ctrl+Space): installed apps from the Start menu shortcuts,
//! matched locally. Launching goes through the shell (`ShellExecute` via explorer).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::search::fold;

#[derive(Clone, Debug)]
pub struct App {
    pub name: String,
    pub path: PathBuf,
}

fn collect(dir: &Path, out: &mut Vec<App>, depth: u32) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out, depth + 1);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("url")) {
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let lower = name.to_lowercase();
            if lower.contains("uninstall") || lower.contains("désinstaller") || lower.contains("readme") {
                continue;
            }
            if !out.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) {
                out.push(App { name, path: p });
            }
        }
    }
}

fn scan() -> Vec<App> {
    let mut out = Vec::new();
    let roots = [
        std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs")),
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs")),
    ];
    for r in roots.into_iter().flatten() {
        collect(&r, &mut out, 0);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

static APPS: OnceLock<Vec<App>> = OnceLock::new();

pub fn apps() -> &'static [App] {
    APPS.get_or_init(scan)
}

/// Warm the index on a background thread so the first Ctrl+Space is instant.
pub fn prewarm() {
    std::thread::spawn(|| {
        apps();
    });
}

/// Best matches: prefix of name or of a word > substring.
pub fn find(query: &str, limit: usize) -> Vec<App> {
    let q = fold(query.trim());
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(i32, &App)> = apps()
        .iter()
        .filter_map(|a| {
            let n = fold(&a.name);
            let score = if n.starts_with(&q) {
                3
            } else if n.split_whitespace().any(|w| w.starts_with(&q)) {
                2
            } else if n.contains(&q) {
                1
            } else {
                return None;
            };
            Some((score, a))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.len().cmp(&b.1.name.len())));
    scored.into_iter().take(limit).map(|(_, a)| a.clone()).collect()
}

pub fn launch(path: &Path) {
    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
}

/// Web search in the default browser — only when the user explicitly picks it.
pub fn web_search(query: &str) {
    let q: String = query
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    let _ = std::process::Command::new("explorer.exe").arg(format!("https://duckduckgo.com/?q={q}")).spawn();
}

#[cfg(test)]
mod tests {
    #[test]
    fn index_scans_without_panicking() {
        let all = super::apps();
        if let Some(first) = all.first() {
            let hits = super::find(&first.name[..first.name.len().min(3)], 5);
            assert!(!hits.is_empty());
        }
    }
}
