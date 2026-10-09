//! Host resource statistics sampler.
//!
//! Reads CPU and memory metrics from the OS:
//!   - Linux: /proc/stat (CPU deltas) and /proc/meminfo
//!   - macOS: returns zeros (libc binding complexity avoided)
//!   - Other: returns zeros

use std::time::Duration;
use tokio::time;

#[derive(Debug, Clone, Copy, Default)]
pub struct HostSnapshot {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
}

pub async fn sample_loop(tx: tokio::sync::watch::Sender<HostSnapshot>) {
    let mut prev = read_cpu_raw();
    loop {
        time::sleep(Duration::from_secs(2)).await;
        let curr = read_cpu_raw();
        let cpu_pct = compute_cpu_pct(prev, curr);
        prev = curr;
        let (mem_used, mem_total) = read_mem();
        let _ = tx.send(HostSnapshot { cpu_pct, mem_used, mem_total });
    }
}

// ── Linux ──────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Default)]
struct CpuRaw {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
}

#[cfg(target_os = "linux")]
fn read_cpu_raw() -> CpuRaw {
    use std::io::Read;
    let mut buf = String::new();
    if std::fs::File::open("/proc/stat")
        .and_then(|mut f| f.read_to_string(&mut buf))
        .is_err()
    {
        return CpuRaw::default();
    }
    let line = buf.lines().next().unwrap_or_default();
    let mut parts = line.split_whitespace().skip(1);
    macro_rules! next {
        ($p:expr) => {
            $p.next().unwrap_or("0").parse::<u64>().unwrap_or(0)
        };
    }
    CpuRaw {
        user: next!(parts),
        nice: next!(parts),
        system: next!(parts),
        idle: next!(parts),
        iowait: next!(parts),
        irq: next!(parts),
        softirq: next!(parts),
    }
}

#[cfg(target_os = "linux")]
fn compute_cpu_pct(prev: CpuRaw, curr: CpuRaw) -> f32 {
    let prev_idle = prev.idle + prev.iowait;
    let curr_idle = curr.idle + curr.iowait;
    let prev_total = prev.user
        + prev.nice
        + prev.system
        + prev.idle
        + prev.iowait
        + prev.irq
        + prev.softirq;
    let curr_total = curr.user
        + curr.nice
        + curr.system
        + curr.idle
        + curr.iowait
        + curr.irq
        + curr.softirq;
    let total_delta = curr_total.saturating_sub(prev_total);
    let idle_delta = curr_idle.saturating_sub(prev_idle);
    if total_delta == 0 {
        return 0.0;
    }
    ((total_delta - idle_delta) as f32 / total_delta as f32 * 100.0).clamp(0.0, 100.0)
}

#[cfg(target_os = "linux")]
fn read_mem() -> (u64, u64) {
    use std::io::Read;
    let mut buf = String::new();
    if std::fs::File::open("/proc/meminfo")
        .and_then(|mut f| f.read_to_string(&mut buf))
        .is_err()
    {
        return (0, 0);
    }
    let mut total_kb = 0u64;
    let mut avail_kb = 0u64;
    for line in buf.lines() {
        if line.starts_with("MemTotal:") {
            total_kb = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
        } else if line.starts_with("MemAvailable:") {
            avail_kb = line
                .split_whitespace()
                .nth(1)
                .unwrap_or("0")
                .parse()
                .unwrap_or(0);
        }
    }
    let total = total_kb * 1024;
    let avail = avail_kb * 1024;
    (total.saturating_sub(avail), total)
}

// ── macOS and other platforms ──────────────────────────────────────────────────
//
// Return zeros to keep the binary simple; the Linux path is the important one.

#[cfg(not(target_os = "linux"))]
#[derive(Clone, Copy, Default)]
struct CpuRaw;

#[cfg(not(target_os = "linux"))]
fn read_cpu_raw() -> CpuRaw {
    CpuRaw
}

#[cfg(not(target_os = "linux"))]
fn compute_cpu_pct(_prev: CpuRaw, _curr: CpuRaw) -> f32 {
    0.0
}

#[cfg(not(target_os = "linux"))]
fn read_mem() -> (u64, u64) {
    (0, 0)
}
