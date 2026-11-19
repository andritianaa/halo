//! M3 — Per-process resource monitor with history (CPU/GPU/RAM), Pulse-style.
//!
//! Samples every 2 s on a worker: global CPU/RAM/GPU + per-core usage + per-process
//! CPU/GPU/memory, kept in ring buffers (1 h). Answers "who used the CPU/GPU over the
//! last N minutes?" and records peaks. The aggregation core is pure and tested.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use halo_core::module::{Category, Manifest, Module, ModuleContext};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

use crate::worker::Worker;

pub const MANIFEST: Manifest = Manifest {
    id: "resources",
    cdc: "M3",
    name_key: "module.resources",
    category: Category::Monitoring,
    icon: "cpu",
    permissions: &[],
    opt_in: false,
    default_enabled: true,
};

pub const PERIOD_S: u64 = 2;
/// 1 h of history at 2 s.
pub const CAP: usize = 1800;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub cpu: f32,
    pub ram: f32,
    pub gpu: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProcSample {
    pub pid: u32,
    pub name: String,
    /// % of the whole machine (all cores = 100).
    pub cpu: f32,
    pub gpu: f32,
    pub mem: u64,
}

#[derive(Clone, Debug, Default)]
struct Hist {
    name: String,
    cpu: VecDeque<f32>,
    gpu: VecDeque<f32>,
    mem: u64,
    /// Index of the global sample when last seen (to align windows).
    last_seen: u64,
    hot: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub pid: u32,
    pub name: String,
    pub avg_cpu: f32,
    pub max_cpu: f32,
    pub avg_gpu: f32,
    pub max_gpu: f32,
    pub mem: u64,
    /// CPU history inside the window (oldest first), for sparklines.
    pub history: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Cpu,
    Gpu,
    Mem,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Peak {
    /// Unix seconds.
    pub at: i64,
    pub name: String,
    pub cpu: f32,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    pub global: VecDeque<Point>,
    pub cores: Vec<f32>,
    pub cpu_name: String,
    pub total_mem: u64,
    procs: HashMap<u32, Hist>,
    pub peaks: VecDeque<Peak>,
    pub seq: u64,
}

fn push<T>(q: &mut VecDeque<T>, v: T) {
    if q.len() == CAP {
        q.pop_front();
    }
    q.push_back(v);
}

impl History {
    /// Add one sampling round.
    pub fn record(&mut self, point: Point, cores: Vec<f32>, procs: Vec<ProcSample>, now_unix: i64) {
        self.seq += 1;
        push(&mut self.global, point);
        self.cores = cores;
        for p in procs {
            let h = self.procs.entry(p.pid).or_insert_with(|| Hist { name: p.name.clone(), ..Default::default() });
            // Fill gaps for processes that were not seen (keeps windows aligned).
            let missing = (self.seq - 1).saturating_sub(h.last_seen).min(CAP as u64) as usize;
            if h.last_seen > 0 {
                for _ in 0..missing {
                    push(&mut h.cpu, 0.0);
                    push(&mut h.gpu, 0.0);
                }
            }
            push(&mut h.cpu, p.cpu);
            push(&mut h.gpu, p.gpu);
            h.mem = p.mem;
            h.last_seen = self.seq;
            // Peak: ≥ 80 % CPU for 3 consecutive samples.
            h.hot = if p.cpu >= 80.0 { h.hot.saturating_add(1) } else { 0 };
            if h.hot == 3 {
                if self.peaks.len() == 50 {
                    self.peaks.pop_front();
                }
                self.peaks.push_back(Peak { at: now_unix, name: h.name.clone(), cpu: p.cpu });
            }
        }
        // Forget processes gone for longer than the history.
        let seq = self.seq;
        self.procs.retain(|_, h| seq - h.last_seen < CAP as u64);
    }

    /// Top processes over the last `samples` rounds (0 = whole session).
    pub fn top(&self, samples: usize, by: SortBy, limit: usize) -> Vec<Stat> {
        let n = if samples == 0 { CAP } else { samples };
        let mut v: Vec<Stat> = self
            .procs
            .iter()
            .filter(|(_, h)| self.seq - h.last_seen < n as u64)
            .map(|(pid, h)| {
                let take = |q: &VecDeque<f32>| -> Vec<f32> {
                    // Pad for samples where the process did not exist yet.
                    let since_seen = (self.seq - h.last_seen) as usize;
                    let mut w: Vec<f32> = q.iter().rev().take(n.saturating_sub(since_seen)).copied().collect();
                    w.reverse();
                    w
                };
                let (c, g) = (take(&h.cpu), take(&h.gpu));
                let avg = |w: &[f32]| if w.is_empty() { 0.0 } else { w.iter().sum::<f32>() / n.min(self.global.len().max(1)) as f32 };
                let max = |w: &[f32]| w.iter().copied().fold(0.0f32, f32::max);
                Stat {
                    pid: *pid,
                    name: h.name.clone(),
                    avg_cpu: avg(&c),
                    max_cpu: max(&c),
                    avg_gpu: avg(&g),
                    max_gpu: max(&g),
                    mem: h.mem,
                    history: c,
                }
            })
            .collect();
        match by {
            SortBy::Cpu => v.sort_by(|a, b| b.avg_cpu.total_cmp(&a.avg_cpu)),
            SortBy::Gpu => v.sort_by(|a, b| b.avg_gpu.total_cmp(&a.avg_gpu)),
            SortBy::Mem => v.sort_by(|a, b| b.mem.cmp(&a.mem)),
        }
        v.truncate(limit);
        v
    }

    /// Last `samples` global points (oldest first).
    pub fn window(&self, samples: usize) -> Vec<Point> {
        let n = if samples == 0 { CAP } else { samples };
        let skip = self.global.len().saturating_sub(n);
        self.global.iter().skip(skip).copied().collect()
    }
}

#[derive(Clone, Default)]
pub struct ResourcesHandle(Arc<Mutex<History>>);

impl ResourcesHandle {
    pub fn with<R>(&self, f: impl FnOnce(&History) -> R) -> R {
        let g = self.0.lock().unwrap();
        f(&g)
    }
}

pub struct Resources {
    handle: ResourcesHandle,
    worker: Option<Worker>,
}

impl Resources {
    pub fn new() -> Self {
        Self { handle: ResourcesHandle::default(), worker: None }
    }
    pub fn handle(&self) -> ResourcesHandle {
        self.handle.clone()
    }
}

impl Default for Resources {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Resources {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }

    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        let shared = self.handle.0.clone();
        let mut sys = System::new_with_specifics(RefreshKind::nothing()).map_err(|e| anyhow::anyhow!("sysinfo: {e}"))?;
        sys.refresh_cpu_all();
        let cores = sys.cpus().len().max(1) as f32;
        let cpu_name = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
        let mut gpu = halo_platform::perf::GpuCounters::new();
        self.worker = Some(Worker::every("halo-resources", Duration::from_secs(PERIOD_S), move || {
            sys.refresh_cpu_all();
            sys.refresh_memory();
            sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
            let (gpu_pid, gpu_total) = gpu.as_mut().map(|g| g.sample()).unwrap_or_default();
            let procs: Vec<ProcSample> = sys
                .processes()
                .iter()
                .map(|(pid, p)| {
                    let pid = pid.as_u32();
                    ProcSample {
                        pid,
                        name: p.name().to_string_lossy().into_owned(),
                        cpu: (p.cpu_usage() / cores).min(100.0),
                        gpu: gpu_pid.get(&pid).copied().unwrap_or(0.0),
                        mem: p.memory(),
                    }
                })
                .filter(|p| p.pid != 0)
                .collect();
            let total = sys.total_memory().max(1);
            let point = Point { cpu: sys.global_cpu_usage(), ram: sys.used_memory() as f32 * 100.0 / total as f32, gpu: gpu_total };
            let per_core = sys.cpus().iter().map(|c| c.usage()).collect();
            if let Ok(mut h) = shared.lock() {
                h.cpu_name = cpu_name.clone();
                h.total_mem = total;
                h.record(point, per_core, procs, crate::store::now());
            }
            true
        }));
        Ok(())
    }

    fn stop(&mut self) {
        self.worker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ps(pid: u32, name: &str, cpu: f32) -> ProcSample {
        ProcSample { pid, name: name.into(), cpu, gpu: cpu / 2.0, mem: pid as u64 * 1000 }
    }

    #[test]
    fn top_over_window_and_peaks() {
        let mut h = History::default();
        for i in 0..10 {
            let heavy = if i >= 5 { 90.0 } else { 5.0 };
            h.record(Point { cpu: heavy, ram: 50.0, gpu: 0.0 }, vec![1.0; 4], vec![ps(1, "build.exe", heavy), ps(2, "chat.exe", 10.0)], i);
        }
        // Over the last 5 samples, build.exe dominates.
        let top = h.top(5, SortBy::Cpu, 5);
        assert_eq!(top[0].name, "build.exe");
        assert!((top[0].avg_cpu - 90.0).abs() < 0.01);
        assert_eq!(top[0].history.len(), 5);
        // Over the whole session its average is lower but max is 90.
        let all = h.top(0, SortBy::Cpu, 5);
        assert!((all[0].avg_cpu - 47.5).abs() < 0.01);
        assert_eq!(all[0].max_cpu, 90.0);
        assert_eq!(h.peaks.len(), 1, "one sustained peak");
        assert_eq!(h.top(0, SortBy::Mem, 1)[0].name, "chat.exe");
        assert_eq!(h.window(3).len(), 3);
    }

    #[test]
    fn processes_that_exit_drop_out_of_short_windows() {
        let mut h = History::default();
        h.record(Point::default(), vec![], vec![ps(7, "short.exe", 50.0)], 0);
        for i in 1..5 {
            h.record(Point::default(), vec![], vec![ps(8, "long.exe", 1.0)], i);
        }
        assert!(h.top(2, SortBy::Cpu, 10).iter().all(|s| s.name != "short.exe"));
        assert!(h.top(0, SortBy::Cpu, 10).iter().any(|s| s.name == "short.exe"));
    }

    #[test]
    fn ring_buffer_is_capped() {
        let mut h = History::default();
        for i in 0..(CAP + 50) {
            h.record(Point::default(), vec![], vec![ps(1, "a", 1.0)], i as i64);
        }
        assert_eq!(h.global.len(), CAP);
        assert_eq!(h.top(0, SortBy::Cpu, 1)[0].history.len(), CAP);
    }
}
