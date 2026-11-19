//! M5 — Per-process network monitor and (optional) limiter.
//!
//! Monitoring works without admin rights: TCP/UDP connection tables with owning PIDs
//! (`GetExtendedTcpTable` / `GetExtendedUdpTable`, IPv4 + IPv6) sampled every 2 s on a
//! worker, aggregated per executable, plus "new app went online" events. Per-connection
//! byte counters come from TCP EStats, which Windows only allows elevated; otherwise
//! `bytes_available` is `false`.
//!
//! Limiting needs the WinDivert driver *and* admin rights. Halo never downloads,
//! installs or elevates anything by itself: rules are always saved to
//! `<Config::dir()>/netlimit.json`, and the shaping loop only runs after an explicit
//! [`NetHandle::start_limiter`] when `WinDivert.dll` is already present next to the
//! executable or in `<Config::dir()>/windivert/`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use serde::{Deserialize, Serialize};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "netlimit",
    cdc: "M5",
    name_key: "module.netlimit",
    category: Category::Network,
    icon: "globe",
    permissions: &[Permission::Elevation, Permission::Driver],
    opt_in: true,
    default_enabled: false,
};

pub const PERIOD_S: u64 = 2;
const MAX_EVENTS: usize = 200;
const REMOTE_SAMPLE: usize = 8;

// ---------------------------------------------------------------------------
// Connections

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TcpState {
    Closed,
    Listen,
    SynSent,
    SynRcvd,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
    /// UDP endpoints have no state.
    Stateless,
    Unknown,
}

impl TcpState {
    pub fn from_mib(v: u32) -> Self {
        match v {
            1 => Self::Closed,
            2 => Self::Listen,
            3 => Self::SynSent,
            4 => Self::SynRcvd,
            5 => Self::Established,
            6 => Self::FinWait1,
            7 => Self::FinWait2,
            8 => Self::CloseWait,
            9 => Self::Closing,
            10 => Self::LastAck,
            11 => Self::TimeWait,
            12 => Self::DeleteTcb,
            _ => Self::Unknown,
        }
    }
}

/// One socket from the system tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Conn {
    pub proto: Proto,
    pub local: SocketAddr,
    /// `None` for UDP and listening sockets.
    pub remote: Option<SocketAddr>,
    pub state: TcpState,
    pub pid: u32,
}

impl Conn {
    fn key(&self) -> (SocketAddr, Option<SocketAddr>) {
        (self.local, self.remote)
    }
}

/// Port fields of the MIB rows hold a network-order u16 in the low bytes.
fn mib_port(v: u32) -> u16 {
    u16::from_be((v & 0xFFFF) as u16)
}

/// IPv4 fields are network-order bytes stored in a u32.
fn mib_v4(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v.to_ne_bytes())
}

/// Current TCP + UDP sockets (IPv4 + IPv6) with their owning PID. Read-only, no admin.
pub fn connections() -> io::Result<Vec<Conn>> {
    sys::connections()
}

#[cfg(windows)]
mod sys {
    use super::*;
    use windows::Win32::NetworkManagement::IpHelper::*;
    use windows::Win32::Networking::WinSock::{IN6_ADDR, IN6_ADDR_0};

    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    const ERROR_ACCESS_DENIED: u32 = 5;

    /// Call a "fill buffer or tell me the size" API with an 8-byte aligned buffer.
    fn fetch(mut call: impl FnMut(*mut core::ffi::c_void, *mut u32) -> u32) -> io::Result<Vec<u64>> {
        let mut size: u32 = 16 * 1024;
        for _ in 0..6 {
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let mut sz = (buf.len() * 8) as u32;
            match call(buf.as_mut_ptr().cast(), &mut sz) {
                0 => return Ok(buf),
                ERROR_INSUFFICIENT_BUFFER => size = sz.saturating_add(4096),
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
        }
        Err(io::Error::other("connection table keeps growing"))
    }

    /// Rows of a `{ dwNumEntries: u32, table: [ROW; 1] }` table, bounds-checked.
    unsafe fn rows<Row: Copy>(buf: &[u64], table_offset: usize) -> Vec<Row> {
        let bytes = buf.len() * 8;
        let base = buf.as_ptr().cast::<u8>();
        let n = unsafe { base.cast::<u32>().read() } as usize;
        let max = bytes.saturating_sub(table_offset) / size_of::<Row>();
        let n = n.min(max);
        let first = unsafe { base.add(table_offset).cast::<Row>() };
        (0..n).map(|i| unsafe { first.add(i).read_unaligned() }).collect()
    }

    fn table_offset<Row>() -> usize {
        // dwNumEntries (u32) followed by the row array at the row's alignment.
        let align = align_of::<Row>();
        4usize.div_ceil(align) * align
    }

    pub fn connections() -> io::Result<Vec<Conn>> {
        let mut out = Vec::new();
        let tcp4 = fetch(|p, sz| unsafe { GetExtendedTcpTable(Some(p), sz, false, AF_INET, TCP_TABLE_OWNER_PID_ALL, 0) })?;
        for r in unsafe { rows::<MIB_TCPROW_OWNER_PID>(&tcp4, table_offset::<MIB_TCPROW_OWNER_PID>()) } {
            let state = TcpState::from_mib(r.dwState);
            let remote = (state != TcpState::Listen)
                .then(|| SocketAddr::V4(SocketAddrV4::new(mib_v4(r.dwRemoteAddr), mib_port(r.dwRemotePort))));
            out.push(Conn {
                proto: Proto::Tcp,
                local: SocketAddr::V4(SocketAddrV4::new(mib_v4(r.dwLocalAddr), mib_port(r.dwLocalPort))),
                remote,
                state,
                pid: r.dwOwningPid,
            });
        }
        // IPv6 may be disabled: ignore its errors.
        if let Ok(tcp6) =
            fetch(|p, sz| unsafe { GetExtendedTcpTable(Some(p), sz, false, AF_INET6, TCP_TABLE_OWNER_PID_ALL, 0) })
        {
            for r in unsafe { rows::<MIB_TCP6ROW_OWNER_PID>(&tcp6, table_offset::<MIB_TCP6ROW_OWNER_PID>()) } {
                let state = TcpState::from_mib(r.dwState);
                let remote = (state != TcpState::Listen).then(|| {
                    SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::from(r.ucRemoteAddr),
                        mib_port(r.dwRemotePort),
                        0,
                        r.dwRemoteScopeId,
                    ))
                });
                out.push(Conn {
                    proto: Proto::Tcp,
                    local: SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::from(r.ucLocalAddr),
                        mib_port(r.dwLocalPort),
                        0,
                        r.dwLocalScopeId,
                    )),
                    remote,
                    state,
                    pid: r.dwOwningPid,
                });
            }
        }
        if let Ok(udp4) = fetch(|p, sz| unsafe { GetExtendedUdpTable(Some(p), sz, false, AF_INET, UDP_TABLE_OWNER_PID, 0) }) {
            for r in unsafe { rows::<MIB_UDPROW_OWNER_PID>(&udp4, table_offset::<MIB_UDPROW_OWNER_PID>()) } {
                out.push(Conn {
                    proto: Proto::Udp,
                    local: SocketAddr::V4(SocketAddrV4::new(mib_v4(r.dwLocalAddr), mib_port(r.dwLocalPort))),
                    remote: None,
                    state: TcpState::Stateless,
                    pid: r.dwOwningPid,
                });
            }
        }
        if let Ok(udp6) = fetch(|p, sz| unsafe { GetExtendedUdpTable(Some(p), sz, false, AF_INET6, UDP_TABLE_OWNER_PID, 0) })
        {
            for r in unsafe { rows::<MIB_UDP6ROW_OWNER_PID>(&udp6, table_offset::<MIB_UDP6ROW_OWNER_PID>()) } {
                out.push(Conn {
                    proto: Proto::Udp,
                    local: SocketAddr::V6(SocketAddrV6::new(
                        Ipv6Addr::from(r.ucLocalAddr),
                        mib_port(r.dwLocalPort),
                        0,
                        r.dwLocalScopeId,
                    )),
                    remote: None,
                    state: TcpState::Stateless,
                    pid: r.dwOwningPid,
                });
            }
        }
        Ok(out)
    }

    pub enum EStatsError {
        /// Not elevated (or policy forbids): stop trying.
        Denied,
        /// Connection went away etc.
        Other,
    }

    fn as_bytes_mut<T>(v: &mut T) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut((v as *mut T).cast::<u8>(), size_of::<T>()) }
    }

    fn as_bytes<T>(v: &T) -> &[u8] {
        unsafe { std::slice::from_raw_parts((v as *const T).cast::<u8>(), size_of::<T>()) }
    }

    fn check(r: u32) -> Result<(), EStatsError> {
        match r {
            0 => Ok(()),
            ERROR_ACCESS_DENIED => Err(EStatsError::Denied),
            _ => Err(EStatsError::Other),
        }
    }

    /// (bytes in, bytes out) of an established TCP connection via EStats. When `enable`
    /// is set, collection is first switched on for this connection (runtime only,
    /// requires elevation, not persisted).
    pub fn estats_bytes(c: &Conn, enable: bool) -> Result<(u64, u64), EStatsError> {
        let Some(remote) = c.remote else { return Err(EStatsError::Other) };
        let rw = TCP_ESTATS_DATA_RW_v0 { EnableCollection: true };
        let mut rod = TCP_ESTATS_DATA_ROD_v0::default();
        match (c.local, remote) {
            (SocketAddr::V4(l), SocketAddr::V4(r)) => {
                let row = MIB_TCPROW_LH {
                    Anonymous: MIB_TCPROW_LH_0 { dwState: 5 },
                    dwLocalAddr: u32::from_ne_bytes(l.ip().octets()),
                    dwLocalPort: l.port().to_be() as u32,
                    dwRemoteAddr: u32::from_ne_bytes(r.ip().octets()),
                    dwRemotePort: r.port().to_be() as u32,
                };
                if enable {
                    check(unsafe { SetPerTcpConnectionEStats(&row, TcpConnectionEstatsData, as_bytes(&rw), 0, 0) })?;
                }
                check(unsafe {
                    GetPerTcpConnectionEStats(&row, TcpConnectionEstatsData, None, 0, None, 0, Some(as_bytes_mut(&mut rod)), 0)
                })?;
            }
            (SocketAddr::V6(l), SocketAddr::V6(r)) => {
                let row = MIB_TCP6ROW {
                    State: MIB_TCP_STATE_ESTAB,
                    LocalAddr: IN6_ADDR { u: IN6_ADDR_0 { Byte: l.ip().octets() } },
                    dwLocalScopeId: l.scope_id(),
                    dwLocalPort: l.port().to_be() as u32,
                    RemoteAddr: IN6_ADDR { u: IN6_ADDR_0 { Byte: r.ip().octets() } },
                    dwRemoteScopeId: r.scope_id(),
                    dwRemotePort: r.port().to_be() as u32,
                };
                if enable {
                    check(unsafe { SetPerTcp6ConnectionEStats(&row, TcpConnectionEstatsData, as_bytes(&rw), 0, 0) })?;
                }
                check(unsafe {
                    GetPerTcp6ConnectionEStats(&row, TcpConnectionEstatsData, None, 0, None, 0, Some(as_bytes_mut(&mut rod)), 0)
                })?;
            }
            _ => return Err(EStatsError::Other),
        }
        Ok((rod.DataBytesIn, rod.DataBytesOut))
    }

    /// Whether the current process token is elevated (no prompt, read-only).
    pub fn is_elevated() -> bool {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut token = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elev = TOKEN_ELEVATION::default();
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some((&mut elev as *mut TOKEN_ELEVATION).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elev.TokenIsElevated != 0
        }
    }

    pub fn exe_name(pid: u32) -> String {
        match pid {
            0 => "System Idle".into(),
            4 => "System".into(),
            _ => halo_platform::proc::process_exe(pid).unwrap_or_else(|| format!("[{pid}]")),
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use super::*;
    pub enum EStatsError {
        Denied,
        #[allow(dead_code)]
        Other,
    }
    pub fn connections() -> io::Result<Vec<Conn>> {
        Ok(Vec::new())
    }
    pub fn estats_bytes(_: &Conn, _: bool) -> Result<(u64, u64), EStatsError> {
        Err(EStatsError::Denied)
    }
    pub fn is_elevated() -> bool {
        false
    }
    pub fn exe_name(pid: u32) -> String {
        format!("[{pid}]")
    }
}

/// Whether Halo runs elevated (read-only check, never prompts).
pub fn is_elevated() -> bool {
    sys::is_elevated()
}

// ---------------------------------------------------------------------------
// Aggregation (pure)

/// Per-executable network summary.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppStat {
    pub exe: String,
    pub pids: Vec<u32>,
    pub tcp: usize,
    pub udp: usize,
    pub established: usize,
    pub listening: usize,
    /// Up to 8 distinct remote endpoints of established connections.
    pub remotes: Vec<SocketAddr>,
    /// Cumulative bytes over the app's live TCP connections (EStats, elevated only).
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
    /// Bytes/s since the previous sample (elevated only).
    pub rate_in: Option<u64>,
    pub rate_out: Option<u64>,
}

type ConnKey = (SocketAddr, Option<SocketAddr>);

fn is_public(a: &SocketAddr) -> bool {
    match a.ip() {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_unspecified(),
        IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unspecified(),
    }
}

/// Group sockets per executable. `bytes` holds per-connection (in, out) counters when
/// EStats is available.
pub fn aggregate(conns: &[Conn], name: impl Fn(u32) -> String, bytes: Option<&HashMap<ConnKey, (u64, u64)>>) -> Vec<AppStat> {
    let mut by: HashMap<String, AppStat> = HashMap::new();
    for c in conns {
        let exe = name(c.pid);
        let a = by.entry(exe.to_lowercase()).or_insert_with(|| AppStat {
            exe: exe.clone(),
            pids: Vec::new(),
            tcp: 0,
            udp: 0,
            established: 0,
            listening: 0,
            remotes: Vec::new(),
            bytes_in: bytes.map(|_| 0),
            bytes_out: bytes.map(|_| 0),
            rate_in: None,
            rate_out: None,
        });
        if !a.pids.contains(&c.pid) {
            a.pids.push(c.pid);
        }
        match c.proto {
            Proto::Tcp => a.tcp += 1,
            Proto::Udp => a.udp += 1,
        }
        match c.state {
            TcpState::Established => {
                a.established += 1;
                if let Some(r) = c.remote
                    && a.remotes.len() < REMOTE_SAMPLE && !a.remotes.contains(&r) {
                        a.remotes.push(r);
                    }
            }
            TcpState::Listen => a.listening += 1,
            _ => {}
        }
        if let Some((i, o)) = bytes.and_then(|b| b.get(&c.key())) {
            a.bytes_in = a.bytes_in.map(|v| v + i);
            a.bytes_out = a.bytes_out.map(|v| v + o);
        }
    }
    let mut v: Vec<AppStat> = by.into_values().collect();
    for a in &mut v {
        a.pids.sort_unstable();
    }
    v.sort_by(|a, b| {
        b.established.cmp(&a.established).then((b.tcp + b.udp).cmp(&(a.tcp + a.udp))).then(a.exe.cmp(&b.exe))
    });
    v
}

/// "App went online" notification.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetEvent {
    /// Unix seconds.
    pub at: i64,
    pub exe: String,
    pub pids: Vec<u32>,
    pub remote: Option<SocketAddr>,
}

/// Remembers which apps were already seen online and emits events for new ones.
/// The first round is a baseline (no events for apps already online at start).
#[derive(Default, Debug)]
pub struct OnlineTracker {
    seen: HashSet<String>,
    baseline: bool,
    pub events: VecDeque<NetEvent>,
}

impl OnlineTracker {
    pub fn update(&mut self, apps: &[AppStat], now_unix: i64) -> Vec<NetEvent> {
        let mut new = Vec::new();
        for a in apps.iter().filter(|a| a.established > 0) {
            if self.seen.insert(a.exe.to_lowercase()) && self.baseline {
                let ev = NetEvent {
                    at: now_unix,
                    exe: a.exe.clone(),
                    pids: a.pids.clone(),
                    remote: a.remotes.iter().find(|r| is_public(r)).or(a.remotes.first()).copied(),
                };
                if self.events.len() == MAX_EVENTS {
                    self.events.pop_front();
                }
                self.events.push_back(ev.clone());
                new.push(ev);
            }
        }
        self.baseline = true;
        new
    }
}

// ---------------------------------------------------------------------------
// Token bucket (pure)

/// Token-bucket shaper. `allow` always admits the packet and returns how long the
/// caller should hold it (zero = send now); tokens can go into debt.
#[derive(Clone, Debug)]
pub struct Shaper {
    pub rate_bytes_per_s: u64,
    pub burst: u64,
    tokens: f64,
    last: Option<Instant>,
}

impl Shaper {
    pub fn new(rate_bytes_per_s: u64, burst: u64) -> Self {
        Self { rate_bytes_per_s, burst, tokens: burst as f64, last: None }
    }

    /// kbps = kilobits per second (1 kbps = 125 B/s); burst = max(¼ s of traffic, 16 KiB).
    pub fn from_kbps(kbps: u32) -> Self {
        let rate = kbps as u64 * 125;
        Self::new(rate, (rate / 4).max(16 * 1024))
    }

    fn refill(&mut self, now: Instant) {
        if let Some(last) = self.last {
            let dt = now.saturating_duration_since(last).as_secs_f64();
            self.tokens = (self.tokens + dt * self.rate_bytes_per_s as f64).min(self.burst as f64);
        }
        self.last = Some(match self.last {
            Some(l) if l > now => l,
            _ => now,
        });
    }

    /// Account `bytes` at `now`; returns the delay before they may be sent.
    /// A zero rate means "never" (`Duration::MAX`).
    pub fn allow(&mut self, bytes: usize, now: Instant) -> Duration {
        if self.rate_bytes_per_s == 0 {
            return Duration::MAX;
        }
        self.refill(now);
        self.tokens -= bytes as f64;
        if self.tokens >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-self.tokens / self.rate_bytes_per_s as f64)
        }
    }

    /// Give back tokens of a packet that was dropped instead of delayed.
    pub fn refund(&mut self, bytes: usize) {
        self.tokens = (self.tokens + bytes as f64).min(self.burst as f64);
    }
}

// ---------------------------------------------------------------------------
// Rules

/// Per-executable limit. `down_kbps`/`up_kbps` in kilobits per second.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// Executable file name, e.g. `steam.exe` (case-insensitive).
    pub exe: String,
    #[serde(default)]
    pub down_kbps: Option<u32>,
    #[serde(default)]
    pub up_kbps: Option<u32>,
    #[serde(default)]
    pub block: bool,
}

impl Rule {
    pub fn matches(&self, exe: &str) -> bool {
        self.exe.eq_ignore_ascii_case(exe)
    }
}

#[derive(Serialize, Deserialize)]
struct RulesFile {
    version: u32,
    rules: Vec<Rule>,
}

/// Rules persisted as JSON (atomic write).
#[derive(Clone, Debug)]
pub struct RuleSet {
    path: PathBuf,
    rules: Vec<Rule>,
}

impl RuleSet {
    pub fn default_path() -> PathBuf {
        halo_core::config::Config::dir().join("netlimit.json")
    }

    /// Load (missing or invalid file = no rules).
    pub fn load(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let rules = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<RulesFile>(&t).ok())
            .map(|f| f.rules)
            .unwrap_or_default();
        Self { path, rules }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&RulesFile { version: 1, rules: self.rules.clone() }).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// Insert or replace the rule for `rule.exe`, then save.
    pub fn set(&mut self, rule: Rule) -> io::Result<()> {
        match self.rules.iter_mut().find(|r| r.matches(&rule.exe)) {
            Some(r) => *r = rule,
            None => self.rules.push(rule),
        }
        self.save()
    }

    /// Remove the rule for `exe`; returns whether one existed.
    pub fn remove(&mut self, exe: &str) -> io::Result<bool> {
        let before = self.rules.len();
        self.rules.retain(|r| !r.matches(exe));
        let removed = self.rules.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    pub fn find(&self, exe: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.matches(exe))
    }
}

// ---------------------------------------------------------------------------
// Limiter status + WinDivert

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LimiterStatus {
    /// `WinDivert.dll` not found: rules are saved but inactive.
    DriverMissing,
    /// Driver DLL present but Halo is not elevated.
    NeedsElevation,
    /// Everything present; call `start_limiter()` to activate.
    Ready,
    Running,
    Error(String),
}

pub mod divert {
    //! Minimal dynamic binding to WinDivert 2.x (loaded only if already present).

    use std::collections::{BinaryHeap, HashMap};
    use std::cmp::Reverse;
    use std::ffi::{CString, c_char, c_void};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use super::{Rule, Shaper};

    pub const DLL: &str = "WinDivert.dll";
    /// Packets that would wait longer than this are dropped (TCP backs off).
    pub const MAX_DELAY: Duration = Duration::from_millis(400);
    const MAX_PORTS: usize = 200;
    const INVALID: usize = usize::MAX; // INVALID_HANDLE_VALUE

    /// `WinDivert.dll` next to the executable or in `<Config::dir()>/windivert/`.
    pub fn find_dll() -> Option<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
            candidates.push(dir.join(DLL));
        }
        candidates.push(halo_core::config::Config::dir().join("windivert").join(DLL));
        candidates.into_iter().find(|p| p.is_file())
    }

    /// WINDIVERT_ADDRESS (80 bytes); only the Outbound bit is interpreted.
    #[repr(C, align(8))]
    #[derive(Clone, Copy)]
    pub struct Address(pub [u8; 80]);

    impl Address {
        pub fn outbound(&self) -> bool {
            let flags = u32::from_le_bytes([self.0[8], self.0[9], self.0[10], self.0[11]]);
            flags >> 17 & 1 == 1
        }
    }

    type OpenFn = unsafe extern "C" fn(*const c_char, i32, i16, u64) -> *mut c_void;
    type RecvFn = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, *mut u32, *mut Address) -> i32;
    type SendFn = unsafe extern "C" fn(*mut c_void, *const c_void, u32, *mut u32, *const Address) -> i32;
    type ShutdownFn = unsafe extern "C" fn(*mut c_void, i32) -> i32;
    type CloseFn = unsafe extern "C" fn(*mut c_void) -> i32;

    pub struct WinDivert {
        _lib: libloading::Library,
        open: OpenFn,
        recv: RecvFn,
        send: SendFn,
        shutdown: ShutdownFn,
        close: CloseFn,
    }

    impl WinDivert {
        /// Load the DLL (does not open a handle, so the driver is not touched yet).
        pub fn load(path: &std::path::Path) -> Result<Self, String> {
            unsafe {
                let lib = libloading::Library::new(path).map_err(|e| e.to_string())?;
                let open = *lib.get::<OpenFn>(b"WinDivertOpen\0").map_err(|e| e.to_string())?;
                let recv = *lib.get::<RecvFn>(b"WinDivertRecv\0").map_err(|e| e.to_string())?;
                let send = *lib.get::<SendFn>(b"WinDivertSend\0").map_err(|e| e.to_string())?;
                let shutdown = *lib.get::<ShutdownFn>(b"WinDivertShutdown\0").map_err(|e| e.to_string())?;
                let close = *lib.get::<CloseFn>(b"WinDivertClose\0").map_err(|e| e.to_string())?;
                Ok(Self { _lib: lib, open, recv, send, shutdown, close })
            }
        }

        fn open(&self, filter: &str) -> Option<usize> {
            let f = CString::new(filter).ok()?;
            // Layer 0 = NETWORK, priority 0, flags 0 (divert + reinject).
            let h = unsafe { (self.open)(f.as_ptr(), 0, 0, 0) } as usize;
            (h != INVALID && h != 0).then_some(h)
        }

        fn recv(&self, h: usize, buf: &mut [u8], addr: &mut Address) -> Option<usize> {
            let mut n = 0u32;
            let ok = unsafe { (self.recv)(h as *mut c_void, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n, addr) };
            (ok != 0).then_some(n as usize)
        }

        fn send(&self, h: usize, pkt: &[u8], addr: &Address) -> bool {
            let mut n = 0u32;
            unsafe { (self.send)(h as *mut c_void, pkt.as_ptr().cast(), pkt.len() as u32, &mut n, addr) != 0 }
        }

        fn shutdown_recv(&self, h: usize) {
            unsafe { (self.shutdown)(h as *mut c_void, 1) };
        }

        fn close(&self, h: usize) {
            unsafe { (self.close)(h as *mut c_void) };
        }
    }

    /// WinDivert filter matching non-loopback TCP/UDP traffic on the given local ports.
    pub fn build_filter(ports: &[u16]) -> Option<String> {
        if ports.is_empty() {
            return None;
        }
        let list: Vec<String> = ports.iter().take(MAX_PORTS).map(|p| format!("localPort == {p}")).collect();
        Some(format!("(tcp or udp) and not loopback and ({})", list.join(" or ")))
    }

    /// Local port of an IPv4/IPv6 TCP/UDP packet (src port when outbound, dst otherwise).
    pub fn packet_local_port(pkt: &[u8], outbound: bool) -> Option<u16> {
        let (proto, off) = match pkt.first()? >> 4 {
            4 => (*pkt.get(9)?, ((pkt[0] & 0x0F) as usize) * 4),
            6 => (*pkt.get(6)?, 40),
            _ => return None,
        };
        if proto != 6 && proto != 17 {
            return None;
        }
        let at = if outbound { off } else { off + 2 };
        Some(u16::from_be_bytes([*pkt.get(at)?, *pkt.get(at + 1)?]))
    }

    /// What the sampler tells the shaping loop: local port → rule.
    #[derive(Default)]
    pub struct Plan {
        pub ports: HashMap<u16, Rule>,
    }

    struct Pending {
        due: Instant,
        seq: u64,
        pkt: Vec<u8>,
        addr: Address,
        handle: usize,
    }
    impl PartialEq for Pending {
        fn eq(&self, o: &Self) -> bool {
            (self.due, self.seq) == (o.due, o.seq)
        }
    }
    impl Eq for Pending {}
    impl PartialOrd for Pending {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Pending {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            (self.due, self.seq).cmp(&(o.due, o.seq))
        }
    }

    /// Running limiter: one receive/shape thread + one delayed-send thread.
    pub struct Limiter {
        dll: Arc<WinDivert>,
        plan: Arc<Mutex<Plan>>,
        filter: Arc<Mutex<Option<String>>>,
        generation: Arc<AtomicU64>,
        handle: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
        threads: Vec<JoinHandle<()>>,
    }

    impl Limiter {
        /// Start the shaping loop. Only call when elevated and the DLL is present: opening
        /// a WinDivert handle loads its (already present) kernel driver.
        pub fn start(dll: WinDivert) -> Result<Self, String> {
            let dll = Arc::new(dll);
            let plan = Arc::new(Mutex::new(Plan::default()));
            let filter = Arc::new(Mutex::new(None::<String>));
            let generation = Arc::new(AtomicU64::new(0));
            let handle = Arc::new(AtomicUsize::new(INVALID));
            let stop = Arc::new(AtomicBool::new(false));
            let error = Arc::new(Mutex::new(None));
            let (tx, rx) = mpsc::channel::<Pending>();

            let sender = {
                let dll = dll.clone();
                std::thread::Builder::new()
                    .name("halo-netlimit-send".into())
                    .spawn(move || {
                        let mut heap: BinaryHeap<Reverse<Pending>> = BinaryHeap::new();
                        loop {
                            let wait = heap
                                .peek()
                                .map(|Reverse(p)| p.due.saturating_duration_since(Instant::now()))
                                .unwrap_or(Duration::from_millis(200));
                            match rx.recv_timeout(wait) {
                                Ok(p) => heap.push(Reverse(p)),
                                Err(mpsc::RecvTimeoutError::Timeout) => {}
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                            let now = Instant::now();
                            while heap.peek().is_some_and(|Reverse(p)| p.due <= now) {
                                let Reverse(p) = heap.pop().unwrap();
                                dll.send(p.handle, &p.pkt, &p.addr);
                            }
                        }
                    })
                    .map_err(|e| e.to_string())?
            };

            let shaper = {
                let (dll, plan, filter, generation, handle, stop, error) =
                    (dll.clone(), plan.clone(), filter.clone(), generation.clone(), handle.clone(), stop.clone(), error.clone());
                std::thread::Builder::new()
                    .name("halo-netlimit".into())
                    .spawn(move || {
                        let mut shapers: HashMap<(String, bool), (u32, Shaper)> = HashMap::new();
                        let mut buf = vec![0u8; 65535];
                        let mut seq = 0u64;
                        while !stop.load(Ordering::Relaxed) {
                            let gen_now = generation.load(Ordering::SeqCst);
                            let Some(f) = filter.lock().ok().and_then(|f| f.clone()) else {
                                std::thread::sleep(Duration::from_millis(200));
                                continue;
                            };
                            let Some(h) = dll.open(&f) else {
                                if let Ok(mut e) = error.lock() {
                                    *e = Some(format!("WinDivertOpen failed: {}", std::io::Error::last_os_error()));
                                }
                                break;
                            };
                            handle.store(h, Ordering::SeqCst);
                            let mut addr = Address([0; 80]);
                            while !stop.load(Ordering::Relaxed) && generation.load(Ordering::SeqCst) == gen_now {
                                let Some(n) = dll.recv(h, &mut buf, &mut addr) else { break };
                                let pkt = &buf[..n];
                                let out = addr.outbound();
                                let rule = packet_local_port(pkt, out)
                                    .and_then(|port| plan.lock().ok().and_then(|p| p.ports.get(&port).cloned()));
                                let Some(rule) = rule else {
                                    dll.send(h, pkt, &addr);
                                    continue;
                                };
                                if rule.block {
                                    continue; // drop
                                }
                                let kbps = if out { rule.up_kbps } else { rule.down_kbps };
                                let Some(kbps) = kbps else {
                                    dll.send(h, pkt, &addr);
                                    continue;
                                };
                                let key = (rule.exe.to_lowercase(), out);
                                let entry = shapers.entry(key).or_insert_with(|| (kbps, Shaper::from_kbps(kbps)));
                                if entry.0 != kbps {
                                    *entry = (kbps, Shaper::from_kbps(kbps));
                                }
                                let now = Instant::now();
                                let delay = entry.1.allow(n, now);
                                if delay.is_zero() {
                                    dll.send(h, pkt, &addr);
                                } else if delay > MAX_DELAY {
                                    entry.1.refund(n);
                                } else {
                                    seq += 1;
                                    let _ = tx.send(Pending { due: now + delay, seq, pkt: pkt.to_vec(), addr, handle: h });
                                }
                            }
                            handle.store(INVALID, Ordering::SeqCst);
                            // Let delayed packets of this handle drain before closing it.
                            std::thread::sleep(MAX_DELAY);
                            dll.close(h);
                        }
                        drop(tx);
                    })
                    .map_err(|e| e.to_string())?
            };
            Ok(Self { dll, plan, filter, generation, handle, stop, error, threads: vec![shaper, sender] })
        }

        /// Update the port → rule plan (from the latest connection sample). Reopens the
        /// WinDivert handle when the set of ports changes.
        pub fn update(&self, ports: HashMap<u16, Rule>) {
            let mut keys: Vec<u16> = ports.keys().copied().collect();
            keys.sort_unstable();
            let new_filter = build_filter(&keys);
            if let Ok(mut p) = self.plan.lock() {
                p.ports = ports;
            }
            let changed = self.filter.lock().map(|mut f| {
                let changed = *f != new_filter;
                *f = new_filter;
                changed
            });
            if changed.unwrap_or(false) {
                self.generation.fetch_add(1, Ordering::SeqCst);
                let h = self.handle.load(Ordering::SeqCst);
                if h != INVALID {
                    self.dll.shutdown_recv(h);
                }
            }
        }

        pub fn error(&self) -> Option<String> {
            self.error.lock().ok().and_then(|e| e.clone())
        }

        pub fn is_running(&self) -> bool {
            self.threads.iter().all(|t| !t.is_finished())
        }

        pub fn stop(mut self) {
            self.stop_inner();
        }

        fn stop_inner(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            self.generation.fetch_add(1, Ordering::SeqCst);
            let h = self.handle.load(Ordering::SeqCst);
            if h != INVALID {
                self.dll.shutdown_recv(h);
            }
            for t in self.threads.drain(..) {
                let _ = t.join();
            }
        }
    }

    impl Drop for Limiter {
        fn drop(&mut self) {
            self.stop_inner();
        }
    }
}

// ---------------------------------------------------------------------------
// Sampler (system side)

/// Owns the caches between samples (pid → exe, EStats state, previous byte totals).
pub struct Sampler {
    names: HashMap<u32, String>,
    estats: bool,
    enabled: HashSet<ConnKey>,
    prev: HashMap<String, (u64, u64)>,
    prev_at: Option<Instant>,
}

impl Sampler {
    /// `try_estats`: attempt per-connection byte counters (only useful when elevated).
    pub fn new(try_estats: bool) -> Self {
        Self { names: HashMap::new(), estats: try_estats, enabled: HashSet::new(), prev: HashMap::new(), prev_at: None }
    }

    pub fn bytes_available(&self) -> bool {
        self.estats
    }

    /// One round: connection list + per-app aggregation.
    pub fn sample(&mut self) -> io::Result<(Vec<Conn>, Vec<AppStat>)> {
        let conns = connections()?;
        let live: HashSet<u32> = conns.iter().map(|c| c.pid).collect();
        self.names.retain(|pid, _| live.contains(pid));
        for pid in &live {
            self.names.entry(*pid).or_insert_with(|| sys::exe_name(*pid));
        }

        let mut bytes: Option<HashMap<ConnKey, (u64, u64)>> = None;
        if self.estats {
            let mut map = HashMap::new();
            let mut enabled = HashSet::new();
            for c in conns.iter().filter(|c| c.proto == Proto::Tcp && c.state == TcpState::Established) {
                let first = !self.enabled.contains(&c.key());
                match sys::estats_bytes(c, first) {
                    Ok(v) => {
                        map.insert(c.key(), v);
                        enabled.insert(c.key());
                    }
                    Err(sys::EStatsError::Denied) => {
                        log::info!("netlimit: TCP EStats denied — byte counters unavailable (not elevated)");
                        self.estats = false;
                        break;
                    }
                    Err(sys::EStatsError::Other) => {}
                }
            }
            self.enabled = enabled;
            if self.estats {
                bytes = Some(map);
            }
        }

        let names = &self.names;
        let mut apps = aggregate(&conns, |pid| names.get(&pid).cloned().unwrap_or_else(|| format!("[{pid}]")), bytes.as_ref());
        let now = Instant::now();
        if bytes.is_some() {
            let dt = self.prev_at.map(|t| now.saturating_duration_since(t).as_secs_f64()).unwrap_or(0.0);
            let mut prev = HashMap::new();
            for a in &mut apps {
                let cur = (a.bytes_in.unwrap_or(0), a.bytes_out.unwrap_or(0));
                if let (Some(p), true) = (self.prev.get(&a.exe.to_lowercase()), dt > 0.1) {
                    a.rate_in = Some((cur.0.saturating_sub(p.0) as f64 / dt) as u64);
                    a.rate_out = Some((cur.1.saturating_sub(p.1) as f64 / dt) as u64);
                }
                prev.insert(a.exe.to_lowercase(), cur);
            }
            self.prev = prev;
        }
        self.prev_at = Some(now);
        Ok((conns, apps))
    }

    fn exe(&self, pid: u32) -> Option<&str> {
        self.names.get(&pid).map(String::as_str)
    }
}

/// Local ports of the sockets owned by apps that have a rule (for the limiter).
pub fn plan_ports(conns: &[Conn], exe_of: impl Fn(u32) -> Option<String>, rules: &[Rule]) -> HashMap<u16, Rule> {
    let mut out = HashMap::new();
    for c in conns {
        let Some(exe) = exe_of(c.pid) else { continue };
        if let Some(r) = rules.iter().find(|r| r.matches(&exe))
            && (r.block || r.down_kbps.is_some() || r.up_kbps.is_some()) {
                out.insert(c.local.port(), r.clone());
            }
    }
    out
}

// ---------------------------------------------------------------------------
// Handle

/// Snapshot returned to the UI.
#[derive(Clone, Debug, Default)]
pub struct NetSnapshot {
    pub apps: Vec<AppStat>,
    pub bytes_available: bool,
    /// Unix seconds of the sample (0 = none yet).
    pub at: i64,
}

struct State {
    snapshot: NetSnapshot,
    /// Usage statistics (live speed, history, per app); `period` is chosen by the UI.
    traffic: crate::traffic::TrafficSnapshot,
    tracker: OnlineTracker,
    rules: RuleSet,
    elevated: bool,
    limiter: Option<divert::Limiter>,
    limiter_error: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone)]
pub struct NetHandle(Arc<Mutex<State>>);

fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl NetHandle {
    pub fn new(rules_path: impl Into<PathBuf>) -> Self {
        Self(Arc::new(Mutex::new(State {
            snapshot: NetSnapshot::default(),
            traffic: Default::default(),
            tracker: OnlineTracker::default(),
            rules: RuleSet::load(rules_path),
            elevated: is_elevated(),
            limiter: None,
            limiter_error: None,
            last_error: None,
        })))
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> NetSnapshot {
        self.st().snapshot.clone()
    }

    /// Live speed, history and per-app usage for the selected period.
    pub fn traffic(&self) -> crate::traffic::TrafficSnapshot {
        self.st().traffic.clone()
    }

    /// Per-app rows: period usage merged with live connections.
    pub fn app_traffic(&self) -> Vec<crate::traffic::AppTraffic> {
        let s = self.st();
        crate::traffic::merge(&s.snapshot.apps, &s.traffic.usage)
    }

    /// Change the statistics period (refetched by the worker within a second).
    pub fn set_period(&self, period: crate::traffic::Period) {
        self.st().traffic.period = period;
    }

    /// "New app went online" events, oldest first.
    pub fn events(&self) -> Vec<NetEvent> {
        self.st().tracker.events.iter().cloned().collect()
    }

    pub fn clear_events(&self) {
        self.st().tracker.events.clear();
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.st().rules.rules().to_vec()
    }

    /// Insert/replace a rule and persist it (active only while the limiter runs).
    pub fn set_rule(&self, rule: Rule) -> io::Result<()> {
        let r = self.st().rules.set(rule);
        self.record_io(r)
    }

    pub fn remove_rule(&self, exe: &str) -> io::Result<bool> {
        let r = self.st().rules.remove(exe);
        self.record_io(r)
    }

    fn record_io<T>(&self, r: io::Result<T>) -> io::Result<T> {
        if let Err(e) = &r {
            self.st().last_error = Some(e.to_string());
        }
        r
    }

    pub fn is_elevated(&self) -> bool {
        self.st().elevated
    }

    pub fn last_error(&self) -> Option<String> {
        self.st().last_error.clone()
    }

    pub fn limiter_status(&self) -> LimiterStatus {
        let s = self.st();
        if let Some(l) = &s.limiter {
            if let Some(e) = l.error() {
                return LimiterStatus::Error(e);
            }
            if l.is_running() {
                return LimiterStatus::Running;
            }
            return LimiterStatus::Error("limiter stopped".into());
        }
        if let Some(e) = &s.limiter_error {
            return LimiterStatus::Error(e.clone());
        }
        if divert::find_dll().is_none() {
            LimiterStatus::DriverMissing
        } else if !s.elevated {
            LimiterStatus::NeedsElevation
        } else {
            LimiterStatus::Ready
        }
    }

    /// Activate packet shaping. Requires an existing `WinDivert.dll` (+ its .sys) and an
    /// elevated process; never installs, downloads or elevates anything.
    pub fn start_limiter(&self) -> Result<(), LimiterStatus> {
        let mut s = self.st();
        if s.limiter.is_some() {
            return Ok(());
        }
        let Some(path) = divert::find_dll() else { return Err(LimiterStatus::DriverMissing) };
        if !s.elevated {
            return Err(LimiterStatus::NeedsElevation);
        }
        match divert::WinDivert::load(&path).and_then(divert::Limiter::start) {
            Ok(l) => {
                s.limiter = Some(l);
                s.limiter_error = None;
                Ok(())
            }
            Err(e) => {
                s.limiter_error = Some(e.clone());
                Err(LimiterStatus::Error(e))
            }
        }
    }

    pub fn stop_limiter(&self) {
        let l = self.st().limiter.take();
        if let Some(l) = l {
            l.stop();
        }
        self.st().limiter_error = None;
    }

    /// Store one sampling round (called by the worker).
    fn publish(&self, sampler: &Sampler, conns: &[Conn], apps: Vec<AppStat>) {
        let now = now_unix();
        let mut s = self.st();
        let new = s.tracker.update(&apps, now);
        for e in &new {
            log::info!("netlimit: {} went online", e.exe);
        }
        if let Some(l) = &s.limiter {
            l.update(plan_ports(conns, |pid| sampler.exe(pid).map(str::to_string), s.rules.rules()));
        }
        s.snapshot = NetSnapshot { apps, bytes_available: sampler.bytes_available(), at: now };
        s.last_error = None;
    }
}

/// Every second: interface counters → live speed. Every 30 s, or as soon as the
/// period changes: Windows usage history and per-app usage (WinRT, off the lock).
fn traffic_worker(h: NetHandle) -> Worker {
    use crate::traffic::{Meter, Period, fetch_period, interface_totals};
    let mut meter = Meter::default();
    let mut fetched: Option<(Period, Instant)> = None;
    Worker::every("halo-net-traffic", Duration::from_secs(1), move || {
        let now = Instant::now();
        let totals = interface_totals();
        if let Some((rx, tx)) = totals {
            meter.feed(now, rx, tx);
        }
        let period = h.st().traffic.period;
        let due = fetched.is_none_or(|(p, at)| p != period || now.duration_since(at) >= Duration::from_secs(30));
        let data = due.then(|| {
            fetched = Some((period, now));
            fetch_period(period)
        });
        let mut s = h.st();
        let t = &mut s.traffic;
        t.available = totals.is_some();
        t.rate = meter.rate;
        t.session = meter.session;
        t.live = meter.history.iter().copied().collect();
        t.peak = meter.peak();
        if let Some(d) = data {
            // The user may have switched period while we were fetching.
            if t.period == period {
                t.total = d.buckets.iter().fold((0, 0), |(r, x), (_, br, bx)| (r + br, x + bx));
                t.buckets = d.buckets;
                t.hourly = d.hourly;
                t.usage = d.usage;
                t.connection = d.connection;
                t.history_error = d.error;
            }
        }
        true
    })
}

// ---------------------------------------------------------------------------
// Module

pub struct Net {
    handle: NetHandle,
    worker: Option<Worker>,
    traffic_worker: Option<Worker>,
}

impl Net {
    pub fn new() -> Self {
        Self::with_rules_path(RuleSet::default_path())
    }

    pub fn with_rules_path(path: impl Into<PathBuf>) -> Self {
        Self { handle: NetHandle::new(path), worker: None, traffic_worker: None }
    }

    pub fn handle(&self) -> NetHandle {
        self.handle.clone()
    }
}

impl Default for Net {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Net {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let h = self.handle.clone();
        let mut sampler = Sampler::new(h.is_elevated());
        self.worker = Some(Worker::every("halo-netlimit-sample", Duration::from_secs(PERIOD_S), move || {
            match sampler.sample() {
                Ok((conns, apps)) => h.publish(&sampler, &conns, apps),
                Err(e) => h.st().last_error = Some(format!("connection table: {e}")),
            }
            true
        }));
        self.traffic_worker = Some(traffic_worker(self.handle.clone()));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
        self.traffic_worker = None;
        self.handle.stop_limiter();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(proto: Proto, local: &str, remote: Option<&str>, state: TcpState, pid: u32) -> Conn {
        Conn { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid }
    }

    #[test]
    fn token_bucket_math() {
        let t0 = Instant::now();
        let mut s = Shaper::new(1000, 1000);
        assert_eq!(s.allow(1000, t0), Duration::ZERO, "burst admits immediately");
        let d = s.allow(500, t0);
        assert!((d.as_secs_f64() - 0.5).abs() < 1e-6, "{d:?}");
        // After 1 s: +1000 tokens, debt 500 → 500 left; 400 passes, 200 more waits 0.1 s.
        assert_eq!(s.allow(400, t0 + Duration::from_secs(1)), Duration::ZERO);
        let d = s.allow(200, t0 + Duration::from_secs(1));
        assert!((d.as_secs_f64() - 0.1).abs() < 1e-6, "{d:?}");
        // Refund puts tokens back.
        s.refund(200);
        assert_eq!(s.allow(100, t0 + Duration::from_secs(1)), Duration::ZERO);
        // Long idle never exceeds the burst.
        let t = t0 + Duration::from_secs(100);
        assert_eq!(s.allow(1000, t), Duration::ZERO);
        assert!(s.allow(1, t) > Duration::ZERO);
        // Time going backwards does not mint tokens.
        assert!(s.allow(1, t0) > Duration::ZERO);
        // Zero rate = blocked.
        assert_eq!(Shaper::new(0, 0).allow(1, t0), Duration::MAX);
        // kbps conversion: 800 kbps = 100 000 B/s.
        let k = Shaper::from_kbps(800);
        assert_eq!(k.rate_bytes_per_s, 100_000);
        assert_eq!(k.burst, 25_000);
        // Sustained rate converges to rate: 10 × 10 kB at 100 kB/s ≈ 1 s total delay budget.
        let mut k = Shaper::new(100_000, 10_000);
        let mut last = Duration::ZERO;
        for _ in 0..11 {
            last = k.allow(10_000, t0);
        }
        assert!((last.as_secs_f64() - 1.0).abs() < 1e-6, "{last:?}");
    }

    #[test]
    fn rules_json_roundtrip() {
        let dir = std::env::temp_dir().join(format!("halo-netlimit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("netlimit.json");
        let mut rs = RuleSet::load(&path);
        assert!(rs.rules().is_empty());
        rs.set(Rule { exe: "steam.exe".into(), down_kbps: Some(8000), up_kbps: None, block: false }).unwrap();
        rs.set(Rule { exe: "telemetry.exe".into(), down_kbps: None, up_kbps: None, block: true }).unwrap();
        rs.set(Rule { exe: "STEAM.EXE".into(), down_kbps: Some(4000), up_kbps: Some(500), block: false }).unwrap();
        assert_eq!(rs.rules().len(), 2, "same exe replaced case-insensitively");

        let back = RuleSet::load(&path);
        assert_eq!(back.rules(), rs.rules());
        assert_eq!(back.find("steam.exe").unwrap().up_kbps, Some(500));
        // Missing optional fields default.
        std::fs::write(&path, r#"{"version":1,"rules":[{"exe":"a.exe"}]}"#).unwrap();
        assert_eq!(RuleSet::load(&path).rules()[0], Rule { exe: "a.exe".into(), down_kbps: None, up_kbps: None, block: false });
        // Invalid file = no rules (no panic).
        std::fs::write(&path, "not json").unwrap();
        assert!(RuleSet::load(&path).rules().is_empty());

        let mut rs = back;
        assert!(rs.remove("Telemetry.exe").unwrap());
        assert!(!rs.remove("nope.exe").unwrap());
        assert_eq!(RuleSet::load(&path).rules().len(), 1);

        // Handle view, limiter inactive without the driver.
        let h = NetHandle::new(&path);
        assert_eq!(h.rules().len(), 1);
        h.set_rule(Rule { exe: "x.exe".into(), down_kbps: None, up_kbps: Some(64), block: false }).unwrap();
        assert_eq!(RuleSet::load(&path).rules().len(), 2);
        assert!(matches!(h.limiter_status(), LimiterStatus::DriverMissing | LimiterStatus::NeedsElevation | LimiterStatus::Ready));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aggregation_and_online_events() {
        let names = |pid: u32| match pid {
            1 | 2 => "chrome.exe".to_string(),
            3 => "svc.exe".to_string(),
            _ => "new.exe".to_string(),
        };
        let round1 = vec![
            conn(Proto::Tcp, "10.0.0.2:50000", Some("1.1.1.1:443"), TcpState::Established, 1),
            conn(Proto::Tcp, "10.0.0.2:50001", Some("1.1.1.1:443"), TcpState::Established, 2),
            conn(Proto::Tcp, "10.0.0.2:50002", Some("8.8.8.8:443"), TcpState::TimeWait, 2),
            conn(Proto::Tcp, "0.0.0.0:445", None, TcpState::Listen, 3),
            conn(Proto::Udp, "0.0.0.0:5353", None, TcpState::Stateless, 3),
        ];
        let apps = aggregate(&round1, names, None);
        assert_eq!(apps[0].exe, "chrome.exe");
        assert_eq!(apps[0].pids, vec![1, 2]);
        assert_eq!((apps[0].tcp, apps[0].established), (3, 2));
        assert_eq!(apps[0].remotes, vec!["1.1.1.1:443".parse::<SocketAddr>().unwrap()]);
        assert_eq!(apps[0].bytes_in, None);
        assert_eq!((apps[1].exe.as_str(), apps[1].listening, apps[1].udp), ("svc.exe", 1, 1));

        let mut t = OnlineTracker::default();
        assert!(t.update(&apps, 0).is_empty(), "first round is the baseline");
        let mut round2 = round1.clone();
        round2.push(conn(Proto::Tcp, "10.0.0.2:50100", Some("127.0.0.1:9"), TcpState::Established, 9));
        round2.push(conn(Proto::Tcp, "10.0.0.2:50101", Some("93.184.216.34:80"), TcpState::Established, 9));
        let ev = t.update(&aggregate(&round2, names, None), 5);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].exe, "new.exe");
        assert_eq!(ev[0].remote, Some("93.184.216.34:80".parse().unwrap()), "public endpoint preferred");
        assert!(t.update(&aggregate(&round2, names, None), 7).is_empty(), "only once");
        assert_eq!(t.events.len(), 1);

        // Byte counters summed per app when available.
        let mut b = HashMap::new();
        b.insert(round1[0].key(), (100, 10));
        b.insert(round1[1].key(), (50, 5));
        let apps = aggregate(&round1, names, Some(&b));
        assert_eq!((apps[0].bytes_in, apps[0].bytes_out), (Some(150), Some(15)));
        assert_eq!(apps[1].bytes_in, Some(0));

        // Limiter plan: ports of ruled apps only.
        let rules = vec![Rule { exe: "Chrome.exe".into(), down_kbps: Some(100), up_kbps: None, block: false }];
        let plan = plan_ports(&round1, |pid| Some(names(pid)), &rules);
        let mut ports: Vec<u16> = plan.keys().copied().collect();
        ports.sort();
        assert_eq!(ports, vec![50000, 50001, 50002]);
    }

    #[test]
    fn divert_helpers() {
        assert_eq!(divert::build_filter(&[]), None);
        assert_eq!(
            divert::build_filter(&[80, 443]).unwrap(),
            "(tcp or udp) and not loopback and (localPort == 80 or localPort == 443)"
        );
        // IPv4/TCP: src 50000 → dst 443.
        let mut v4 = vec![0u8; 40];
        v4[0] = 0x45;
        v4[9] = 6;
        v4[20..22].copy_from_slice(&50000u16.to_be_bytes());
        v4[22..24].copy_from_slice(&443u16.to_be_bytes());
        assert_eq!(divert::packet_local_port(&v4, true), Some(50000));
        assert_eq!(divert::packet_local_port(&v4, false), Some(443));
        // IPv6/UDP.
        let mut v6 = vec![0u8; 48];
        v6[0] = 0x60;
        v6[6] = 17;
        v6[40..42].copy_from_slice(&5353u16.to_be_bytes());
        assert_eq!(divert::packet_local_port(&v6, true), Some(5353));
        // ICMP / truncated.
        v4[9] = 1;
        assert_eq!(divert::packet_local_port(&v4, true), None);
        assert_eq!(divert::packet_local_port(&[0x45, 0, 0], true), None);
        let mut a = divert::Address([0; 80]);
        assert!(!a.outbound());
        a.0[10] = 0b10; // bit 17
        assert!(a.outbound());
    }

    #[test]
    fn live_connection_table_read_only() {
        // Open a local listener + connection so there is at least one row of ours.
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let _c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let conns = connections().unwrap();
        let me = std::process::id();
        assert!(conns.iter().any(|c| c.pid == me && c.local.port() == port && c.state == TcpState::Listen), "own listener listed");
        assert!(conns.iter().any(|c| c.pid == me && c.state == TcpState::Established));
        // Sampler without EStats (never enables anything in tests).
        let mut s = Sampler::new(false);
        let (_, apps) = s.sample().unwrap();
        assert!(!s.bytes_available());
        assert!(apps.iter().any(|a| a.pids.contains(&me)));
        let _ = is_elevated();
    }
}
