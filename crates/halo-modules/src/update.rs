//! Update check: compares the running version with the latest GitHub release of
//! the project and, on request, downloads its MSI and starts the installer (which
//! replaces this version and can relaunch Halo). Nothing is sent except the request
//! for the public release list.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Repository whose releases are checked.
pub const REPO: &str = "andritianaa/halo";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub page: String,
    /// Direct download of the x64 MSI.
    pub msi: Option<String>,
}

/// `v1.2.3` / `1.2.3` → (1, 2, 3); missing parts are 0, pre-release suffixes ignored.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches(['v', 'V']).split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let major = it.next()?.ok()?;
    let minor = it.next().unwrap_or(Ok(0)).ok()?;
    let patch = it.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(l), Some(c)) if l > c)
}

/// Parse GitHub's `releases/latest` JSON.
pub fn parse_release(json: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?;
    let page = v.get("html_url").and_then(|u| u.as_str()).unwrap_or_default().to_string();
    let msi = v.get("assets").and_then(|a| a.as_array()).and_then(|assets| {
        assets.iter().find_map(|a| {
            let name = a.get("name")?.as_str()?.to_ascii_lowercase();
            (name.ends_with(".msi") && name.contains("x64")).then(|| a.get("browser_download_url")?.as_str().map(String::from))?
        })
    });
    Some(Release { version: tag.trim_start_matches(['v', 'V']).to_string(), page, msi })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading,
    Failed(String),
}

#[derive(Clone, Default)]
pub struct UpdateHandle(Arc<Mutex<Status>>);

impl UpdateHandle {
    pub fn status(&self) -> Status {
        self.0.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set(&self, s: Status) {
        if let Ok(mut g) = self.0.lock() {
            *g = s;
        }
    }

    /// Ask GitHub for the latest release, in the background.
    pub fn check(&self) {
        if matches!(self.status(), Status::Checking | Status::Downloading) {
            return;
        }
        self.set(Status::Checking);
        let h = self.clone();
        std::thread::spawn(move || {
            let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
            let next = match fetch(&url).and_then(|j| parse_release(&j).ok_or_else(|| "unexpected answer".to_string())) {
                Ok(r) if is_newer(&r.version, CURRENT) => Status::Available(r),
                Ok(_) => Status::UpToDate,
                Err(e) => Status::Failed(e),
            };
            log::info!("update check: {next:?}");
            h.set(next);
        });
    }

    /// Download the available MSI and start it; `on_started` runs once the installer
    /// is launched (the app then quits so files can be replaced).
    pub fn install(&self, on_started: impl FnOnce() + Send + 'static) {
        let Status::Available(r) = self.status() else { return };
        let Some(url) = r.msi.clone() else {
            self.set(Status::Failed("no installer in this release".into()));
            return;
        };
        self.set(Status::Downloading);
        let h = self.clone();
        std::thread::spawn(move || {
            let dest: PathBuf = std::env::temp_dir().join(format!("halo-{}-x64.msi", r.version));
            match download(&url, &dest) {
                Ok(()) => match std::process::Command::new("msiexec").arg("/i").arg(&dest).spawn() {
                    Ok(_) => on_started(),
                    Err(e) => h.set(Status::Failed(e.to_string())),
                },
                Err(e) => h.set(Status::Failed(e)),
            }
        });
    }
}

#[cfg(windows)]
fn fetch(url: &str) -> Result<String, String> {
    halo_platform::http::get_string(url).map_err(|e| e.message().to_string())
}

#[cfg(windows)]
fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    halo_platform::http::download(url, dest).map(|_| ()).map_err(|e| e.message().to_string())
}

#[cfg(not(windows))]
fn fetch(_: &str) -> Result<String, String> {
    Err("unsupported".into())
}

#[cfg(not(windows))]
fn download(_: &str, _: &std::path::Path) -> Result<(), String> {
    Err("unsupported".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("2.0.0-beta.1"), Some((2, 0, 0)));
        assert!(is_newer("v0.10.0", "0.9.9"), "10 > 9, not text order");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn github_release_json_is_parsed() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/andritianaa/halo/releases/tag/v0.2.0",
            "assets":[{"name":"halo-0.2.0-x64.msi.sha256","browser_download_url":"https://x/sha"},
                      {"name":"halo-0.2.0-x64.msi","browser_download_url":"https://x/msi"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.msi.as_deref(), Some("https://x/msi"), "the checksum file is not the installer");
        assert!(parse_release("{}").is_none());
    }
}
