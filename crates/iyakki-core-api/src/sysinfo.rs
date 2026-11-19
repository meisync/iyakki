//! Read-only system figures: the kernel command line, memory, uptime, load, the machine's model and processor count.
//! The parsers are pure so the local implementation and the tests share them.

use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// Memory figures in kibibytes (the unit of `/proc/meminfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemInfo {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Uptime {
    pub up_secs: f64,
    /// Summed idle time of all processors, as the kernel reports it.
    pub idle_secs: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// Read-only system information. Method names follow what the tiles read today (docs/development/UI-OS-SPLIT-PLAN.md, step 5).
pub trait Sysinfo: Send + Sync {
    /// The kernel command line, one line, without the trailing newline.
    fn cmdline(&self) -> ApiResult<String>;
    fn meminfo(&self) -> ApiResult<MemInfo>;
    fn uptime(&self) -> ApiResult<Uptime>;
    fn loadavg(&self) -> ApiResult<LoadAvg>;
    /// The product name from the firmware (`Mac mini`, `ThinkPad X1`); `Unsupported` where there is none.
    fn product_name(&self) -> ApiResult<String>;
    fn cpu_count(&self) -> ApiResult<u32>;
}

/// The machine the program runs on, as one handle (docs/development/UI-OS-SPLIT-PLAN.md step 5). Tiles ask `cx.os` instead of reading `/proc` and `/sys`;
/// the device gives them `iyakki_core_local::LocalOs`, tests give them `iyakki_core_fake::FakeOs`. Later slices add accessors (audio, disks, net).
pub trait Os: Send + Sync {
    fn sysinfo(&self) -> &dyn Sysinfo;
    fn audio(&self) -> &dyn crate::audio::Audio;
    fn disks(&self) -> &dyn crate::devices::Disks;
    fn net(&self) -> &dyn crate::devices::Net;
    /// Compressed memory, swap and `system.conf` (a shared handle: the Memory page runs its changes on a thread).
    #[cfg(feature = "native")]
    fn memory(&self) -> std::sync::Arc<dyn crate::memory::Memory>;
    /// Links, radios, brightness, sound cards, private addresses and Bluetooth hardware.
    #[cfg(feature = "native")]
    fn hardware(&self) -> std::sync::Arc<dyn crate::hardware::Hardware>;
    /// The kept kernel logs of earlier sessions.
    #[cfg(feature = "native")]
    fn crashlog(&self) -> std::sync::Arc<dyn crate::crashlog::CrashLog>;
    /// Finding the machines on the local network.
    #[cfg(feature = "native")]
    fn netscan(&self) -> std::sync::Arc<dyn crate::netscan::NetScan>;
    /// The model router's permissions and access log. A machine that does not say otherwise has none (it refuses).
    #[cfg(feature = "native")]
    fn router(&self) -> std::sync::Arc<dyn crate::router::Router> {
        std::sync::Arc::new(crate::router::NoRouter)
    }
    /// The pair group (fleet) permissions and access log. A machine that does not say otherwise has none (it refuses).
    #[cfg(feature = "native")]
    fn fleet(&self) -> std::sync::Arc<dyn crate::fleet::Fleet> {
        std::sync::Arc::new(crate::fleet::NoFleet)
    }
}

/// Parse the text of `/proc/meminfo`. `MemTotal` is required; a missing `MemAvailable` falls back to `MemFree`.
pub fn parse_meminfo(text: &str) -> ApiResult<MemInfo> {
    let field = |name: &str| -> Option<u64> {
        text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':')?.split_whitespace().next()?.parse().ok())
    };
    let total_kb = field("MemTotal").ok_or_else(|| ApiError::Io("meminfo has no MemTotal".into()))?;
    let free_kb = field("MemFree").unwrap_or(0);
    Ok(MemInfo { total_kb, free_kb, available_kb: field("MemAvailable").unwrap_or(free_kb), swap_total_kb: field("SwapTotal").unwrap_or(0), swap_free_kb: field("SwapFree").unwrap_or(0) })
}

/// Parse the text of `/proc/uptime` (`12345.67 23456.78`).
pub fn parse_uptime(text: &str) -> ApiResult<Uptime> {
    let mut it = text.split_whitespace();
    let up = it.next().and_then(|v| v.parse::<f64>().ok());
    let idle = it.next().and_then(|v| v.parse::<f64>().ok());
    match (up, idle) {
        (Some(up_secs), Some(idle_secs)) => Ok(Uptime { up_secs, idle_secs }),
        _ => Err(ApiError::Io("uptime is not two numbers".into())),
    }
}

/// Parse the text of `/proc/loadavg` (`0.52 0.58 0.59 1/467 12345`).
pub fn parse_loadavg(text: &str) -> ApiResult<LoadAvg> {
    let v: Vec<f64> = text.split_whitespace().take(3).filter_map(|t| t.parse().ok()).collect();
    match v[..] {
        [one, five, fifteen] => Ok(LoadAvg { one, five, fifteen }),
        _ => Err(ApiError::Io("loadavg is not three numbers".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEM: &str = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nSwapTotal:       2048000 kB\nSwapFree:        2000000 kB\n";

    #[test]
    fn meminfo_is_read_and_falls_back_to_free_without_available() {
        let m = parse_meminfo(MEM).unwrap();
        assert_eq!((m.total_kb, m.free_kb, m.available_kb, m.swap_total_kb, m.swap_free_kb), (16384000, 1000000, 8000000, 2048000, 2000000));
        let m = parse_meminfo("MemTotal: 100 kB\nMemFree: 40 kB\n").unwrap();
        assert_eq!((m.available_kb, m.swap_total_kb), (40, 0));
    }

    #[test]
    fn meminfo_without_a_total_is_an_io_error_with_a_stable_code() {
        assert_eq!(parse_meminfo("MemFree: 1 kB").unwrap_err().code(), "io");
        assert_eq!(parse_meminfo("").unwrap_err().code(), "io");
    }

    #[test]
    fn uptime_and_loadavg_parse_and_refuse_junk() {
        assert_eq!(parse_uptime("123.5 400.25\n").unwrap(), Uptime { up_secs: 123.5, idle_secs: 400.25 });
        assert!(parse_uptime("x y").is_err());
        assert!(parse_uptime("5").is_err());
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/467 12345\n").unwrap(), LoadAvg { one: 0.52, five: 0.58, fifteen: 0.59 });
        assert!(parse_loadavg("0.5 0.5").is_err());
    }
}

/// Memory in bytes, as the Settings and Story pages show it (the UI never reads `/proc/meminfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl Memory {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

/// Bytes.
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// A time span as the pages show it ("3d 4h", "5m 10s").
pub fn human_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else if m > 0 { format!("{m}m {}s", secs % 60) } else { format!("{secs}s") }
}

// ---- moved from iyakki-core-local (F-SPLIT-03): the figures of the System page, as plain data ----
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SysRoots {
    pub proc: PathBuf,
    pub sys: PathBuf,
    pub passwd: PathBuf,
}

impl SysRoots {
    pub fn real() -> SysRoots {
        SysRoots { proc: "/proc".into(), sys: "/sys".into(), passwd: "/etc/passwd".into() }
    }
}

/// Cumulative processor time in clock ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

/// How much of the processor was used between two samples, 0 to 100.
pub fn cpu_percent(a: CpuTimes, b: CpuTimes) -> f32 {
    let (dt, db) = (b.total.saturating_sub(a.total), b.busy.saturating_sub(a.busy));
    if dt == 0 { 0.0 } else { (db as f32 / dt as f32 * 100.0).clamp(0.0, 100.0) }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetCounters {
    pub iface: String,
    pub rx: u64,
    pub tx: u64,
}

/// Bytes read from and written to one whole disk since boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskCounters {
    pub disk: String,
    pub read: u64,
    pub written: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub user: String,
    /// The program name (short).
    pub name: String,
    /// The command line, arguments joined by spaces (empty for kernel threads).
    pub cmd: String,
    pub state: char,
    pub threads: u32,
    /// Processor time used so far in clock ticks (user + system).
    pub ticks: u64,
    /// Resident memory in bytes.
    pub rss: u64,
    pub start_ticks: u64,
    /// The control-group line of a container process (the kubepods one); empty for a host process.
    pub cgroup: String,
}

/// Clock ticks per second, as every Linux reports in `/proc` (USER_HZ is 100 on all supported machines).
pub const TICKS_PER_SEC: u64 = 100;

/// What one process does on the network. The kernel keeps byte counters per network namespace, not per process, so a number of bytes
/// exists only for a process that lives in a namespace of its own (a container): then `bytes` is that namespace's traffic and
/// `shared_namespace` says other processes of the same pod add to it. A host process shows its connections only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetUse {
    /// TCP connections that are established.
    pub connections: u32,
    /// TCP ports it listens on.
    pub listening: u32,
    /// UDP sockets it holds.
    pub udp: u32,
    /// Bytes received and sent by its own network namespace (loopback left out), when it has one.
    pub bytes: Option<(u64, u64)>,
}

impl NetUse {
    pub fn is_quiet(&self) -> bool {
        self.connections == 0 && self.listening == 0 && self.udp == 0 && self.bytes.is_none()
    }

    /// "3 conn · 1 listening · ↓ 1.2 MB ↑ 40 KB", empty when there is nothing to say.
    pub fn text(&self) -> String {
        let mut v = Vec::new();
        if self.connections > 0 {
            v.push(format!("{} conn", self.connections));
        }
        if self.listening > 0 {
            v.push(format!("{} listening", self.listening));
        }
        if self.udp > 0 {
            v.push(format!("{} udp", self.udp));
        }
        if let Some((rx, tx)) = self.bytes {
            v.push(format!("↓ {} ↑ {}", human_bytes(rx), human_bytes(tx)));
        }
        v.join(" · ")
    }
}

/// What a process has read from and written to storage since it started (the kernel's own count in `/proc/<pid>/io`: bytes that
/// really went to or came from a disk, not those served from memory).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskUse {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

impl DiskUse {
    pub fn is_quiet(&self) -> bool {
        self.read_bytes == 0 && self.write_bytes == 0
    }

    /// "disk ↓ 1.2 MB ↑ 40 KB", empty when nothing went to or came from a disk.
    pub fn text(&self) -> String {
        if self.is_quiet() { String::new() } else { format!("disk ↓ {} ↑ {}", human_bytes(self.read_bytes), human_bytes(self.write_bytes)) }
    }
}

/// Processor use of each process between two samples, in percent of one processor (so 200 means two busy processors).
pub fn proc_cpu(before: &[Proc], after: &[Proc], elapsed_secs: f32) -> HashMap<u32, f32> {
    let prev: HashMap<u32, u64> = before.iter().map(|p| (p.pid, p.ticks)).collect();
    after
        .iter()
        .filter_map(|p| {
            let was = *prev.get(&p.pid)?;
            Some((p.pid, p.ticks.saturating_sub(was) as f32 / TICKS_PER_SEC as f32 / elapsed_secs.max(0.001) * 100.0))
        })
        .collect()
}

/// Why a process may not be signalled from here, if so.
pub fn protected(pid: u32) -> Option<&'static str> {
    if pid <= 1 {
        Some("this is the system itself (process 1)")
    } else if pid == std::process::id() {
        Some("this is the shell you are using")
    } else {
        None
    }
}
