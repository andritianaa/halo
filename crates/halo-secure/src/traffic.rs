//! Internet usage statistics, NetLimiter-style: live download/upload speed with a
//! 5-minute graph, usage history (today per hour, 7/30 days per day), and traffic per
//! app over the chosen period merged with the live connection table.
//!
//! Everything here is pure (tested); the Windows data comes from
//! `halo_platform::netstats`, sampled by the network worker.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::net::AppStat;

/// Seconds of live history kept for the speed graph.
pub const LIVE_SECONDS: usize = 300;

/// Turns cumulative interface counters into speeds and session totals.
#[derive(Clone, Debug, Default)]
pub struct Meter {
    last: Option<(Instant, u64, u64)>,
    /// Bytes per second (download, upload) over the last interval.
    pub rate: (u64, u64),
    /// Bytes since Halo started measuring.
    pub session: (u64, u64),
    /// One (down, up) speed per sample, oldest first, at most [`LIVE_SECONDS`].
    pub history: VecDeque<(u64, u64)>,
}

impl Meter {
    pub fn feed(&mut self, now: Instant, rx: u64, tx: u64) {
        if let Some((t, r0, t0)) = self.last {
            let dt = now.saturating_duration_since(t).as_secs_f64();
            if dt < 0.2 {
                return;
            }
            // Counters reset when an adapter goes down/up: skip that interval.
            if rx >= r0 && tx >= t0 {
                let (dr, dtx) = (rx - r0, tx - t0);
                self.rate = ((dr as f64 / dt) as u64, (dtx as f64 / dt) as u64);
                self.session.0 += dr;
                self.session.1 += dtx;
            } else {
                self.rate = (0, 0);
            }
            self.history.push_back(self.rate);
            while self.history.len() > LIVE_SECONDS {
                self.history.pop_front();
            }
        }
        self.last = Some((now, rx, tx));
    }

    /// Highest speed in the live history (download or upload).
    pub fn peak(&self) -> u64 {
        self.history.iter().map(|(d, u)| (*d).max(*u)).max().unwrap_or(0)
    }
}

/// Period shown in the statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    #[default]
    Today,
    Week,
    Month,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Today, Period::Week, Period::Month];

    /// (start, end) in Unix seconds and whether buckets are hourly (else daily),
    /// given the Unix time of today's local midnight and now.
    pub fn range(self, midnight: i64, now: i64) -> (i64, i64, bool) {
        match self {
            Period::Today => (midnight, now, true),
            Period::Week => (midnight - 6 * 86_400, now, false),
            Period::Month => (midnight - 29 * 86_400, now, false),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Period::Today => "net.period.today",
            Period::Week => "net.period.week",
            Period::Month => "net.period.month",
        }
    }
}

/// Readable name for a Windows usage attribution id: an exe path
/// (`\device\harddiskvolume3\…\chrome.exe` → `chrome.exe`) or a package family name
/// (`Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy` → `StartMenuExperienceHost`).
pub fn attribution_name(id: &str) -> String {
    if id.contains('\\') || id.to_ascii_lowercase().ends_with(".exe") {
        return id.rsplit('\\').next().unwrap_or(id).to_string();
    }
    let family = id.split('_').next().unwrap_or(id);
    family.rsplit('.').next().unwrap_or(family).to_string()
}

/// One row of the per-app table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppTraffic {
    /// Display name, without `.exe`.
    pub name: String,
    /// Executable name when known (rules match on it).
    pub exe: Option<String>,
    /// Bytes over the selected period (Windows usage data).
    pub rx: u64,
    pub tx: u64,
    /// Live speed (only when per-connection counters are available, i.e. elevated).
    pub rate_in: Option<u64>,
    pub rate_out: Option<u64>,
    pub connections: usize,
}

impl AppTraffic {
    pub fn total(&self) -> u64 {
        self.rx + self.tx
    }
}

/// Merge period usage (`(attribution id, rx, tx)`) with the live connection table.
/// Apps that appear in either are listed, heaviest first, then by connections.
pub fn merge(live: &[AppStat], usage: &[(String, u64, u64)]) -> Vec<AppTraffic> {
    let mut rows: HashMap<String, AppTraffic> = HashMap::new();
    for (id, rx, tx) in usage {
        let name = attribution_name(id);
        let key = name.to_lowercase();
        let e = rows.entry(key).or_insert_with(|| AppTraffic {
            name: name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string(),
            exe: name.to_ascii_lowercase().ends_with(".exe").then(|| name.clone()),
            ..Default::default()
        });
        e.rx += rx;
        e.tx += tx;
    }
    for a in live {
        let key = a.exe.to_lowercase();
        let e = rows.entry(key).or_insert_with(|| AppTraffic {
            name: a.exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string(),
            ..Default::default()
        });
        e.exe = Some(a.exe.clone());
        e.connections += a.tcp + a.udp;
        e.rate_in = a.rate_in.or(e.rate_in);
        e.rate_out = a.rate_out.or(e.rate_out);
    }
    let mut out: Vec<AppTraffic> = rows.into_values().collect();
    out.sort_by(|a, b| b.total().cmp(&a.total()).then(b.connections.cmp(&a.connections)).then(a.name.cmp(&b.name)));
    out
}

/// Speed as "1.2 MB/s" (bytes per second, binary units like the rest of Halo).
pub fn format_rate(bytes_per_s: u64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = bytes_per_s as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{bytes_per_s} B/s") } else if v < 10.0 { format!("{v:.1} {}", UNITS[i]) } else { format!("{v:.0} {}", UNITS[i]) }
}

/// What the UI reads (cloned out of the network state).
#[derive(Clone, Debug, Default)]
pub struct TrafficSnapshot {
    /// Interface counters readable (live speed available).
    pub available: bool,
    pub rate: (u64, u64),
    pub session: (u64, u64),
    /// Live (down, up) speeds, oldest first.
    pub live: Vec<(u64, u64)>,
    pub peak: u64,
    pub period: Period,
    /// History buckets for the period: (start unix, rx, tx); hourly for Today.
    pub buckets: Vec<(i64, u64, u64)>,
    pub hourly: bool,
    /// Totals over the period.
    pub total: (u64, u64),
    /// Per-app usage over the period: (attribution id, rx, tx).
    pub usage: Vec<(String, u64, u64)>,
    /// Current internet connection name (Wi-Fi SSID, "Ethernet"…).
    pub connection: Option<String>,
    /// Windows usage history unavailable (no internet profile, API error).
    pub history_error: Option<String>,
}

/// Period figures fetched from Windows (worker thread only).
pub struct PeriodData {
    pub buckets: Vec<(i64, u64, u64)>,
    pub hourly: bool,
    pub usage: Vec<(String, u64, u64)>,
    pub connection: Option<String>,
    pub error: Option<String>,
}

/// Fetch history + per-app usage for a period (blocking WinRT calls).
#[cfg(windows)]
pub fn fetch_period(period: Period) -> PeriodData {
    use chrono::{Local, TimeZone};
    use halo_platform::netstats::{self, Granularity};
    let now = Local::now();
    let midnight = Local
        .from_local_datetime(&now.date_naive().and_hms_opt(0, 0, 0).unwrap_or_default())
        .earliest()
        .map(|d| d.timestamp())
        .unwrap_or(now.timestamp() - 86_400);
    let (start, end, hourly) = period.range(midnight, now.timestamp());
    let g = if hourly { Granularity::Hour } else { Granularity::Day };
    let mut error = None;
    let buckets = netstats::usage_history(start, end, g)
        .map(|v| v.into_iter().map(|b| (b.start, b.rx, b.tx)).collect())
        .unwrap_or_else(|e| {
            error = Some(e.message().to_string());
            Vec::new()
        });
    let usage = netstats::app_usage(start, end)
        .map(|v| v.into_iter().map(|a| (a.id, a.rx, a.tx)).collect())
        .unwrap_or_default();
    PeriodData { buckets, hourly, usage, connection: netstats::connection_name(), error }
}

#[cfg(not(windows))]
pub fn fetch_period(_: Period) -> PeriodData {
    PeriodData { buckets: Vec::new(), hourly: true, usage: Vec::new(), connection: None, error: Some("unsupported".into()) }
}

#[cfg(windows)]
pub fn interface_totals() -> Option<(u64, u64)> {
    halo_platform::netstats::interface_totals()
}

#[cfg(not(windows))]
pub fn interface_totals() -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn meter_rates_session_and_counter_reset() {
        let t0 = Instant::now();
        let mut m = Meter::default();
        m.feed(t0, 1_000, 500);
        assert_eq!(m.rate, (0, 0), "first sample only primes");
        m.feed(t0 + Duration::from_secs(1), 3_000, 1_500);
        assert_eq!(m.rate, (2_000, 1_000));
        m.feed(t0 + Duration::from_secs(3), 7_000, 1_500);
        assert_eq!(m.rate, (2_000, 0));
        assert_eq!(m.session, (6_000, 1_000));
        // Adapter reset: counters go down → no negative speed, session unchanged.
        m.feed(t0 + Duration::from_secs(4), 10, 10);
        assert_eq!(m.rate, (0, 0));
        assert_eq!(m.session, (6_000, 1_000));
        assert_eq!(m.history.len(), 3);
        assert_eq!(m.peak(), 2_000);
        for i in 0..400 {
            m.feed(t0 + Duration::from_secs(5 + i), 10 + i * 10, 10);
        }
        assert_eq!(m.history.len(), LIVE_SECONDS);
    }

    #[test]
    fn periods_and_names() {
        let (s, e, hourly) = Period::Week.range(1_000_000, 1_050_000);
        assert_eq!((s, e, hourly), (1_000_000 - 6 * 86_400, 1_050_000, false));
        assert!(Period::Today.range(10, 20).2);
        assert_eq!(attribution_name("\\device\\harddiskvolume3\\program files\\google\\chrome\\application\\chrome.exe"), "chrome.exe");
        assert_eq!(attribution_name("Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy"), "StartMenuExperienceHost");
        assert_eq!(attribution_name("Claude_pzs8sxrjxfjjc"), "Claude");
    }

    #[test]
    fn merge_combines_usage_and_live_connections() {
        let live = vec![
            AppStat { exe: "Chrome.exe".into(), tcp: 5, udp: 1, rate_in: Some(900), ..Default::default() },
            AppStat { exe: "idle.exe".into(), tcp: 1, ..Default::default() },
        ];
        let usage = vec![
            ("\\device\\x\\chrome.exe".to_string(), 5_000, 1_000),
            ("\\device\\y\\chrome.exe".to_string(), 1_000, 0),
            ("Claude_abc".to_string(), 9_000, 9_000),
        ];
        let rows = merge(&live, &usage);
        assert_eq!(rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Claude", "chrome", "idle"]);
        let chrome = &rows[1];
        assert_eq!((chrome.rx, chrome.tx, chrome.connections, chrome.rate_in), (6_000, 1_000, 6, Some(900)));
        assert_eq!(chrome.exe.as_deref(), Some("Chrome.exe"), "live exe name wins (rules match on it)");
        assert_eq!(rows[0].exe, None, "packaged app: no exe to limit");
    }

    #[test]
    fn rates_format() {
        assert_eq!(format_rate(512), "512 B/s");
        assert_eq!(format_rate(1_536), "1.5 KB/s");
        assert_eq!(format_rate(25 * 1024 * 1024), "25 MB/s");
    }
}
