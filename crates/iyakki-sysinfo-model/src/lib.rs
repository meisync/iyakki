//! The figures of the System page as plain data and the pure rules over them (memory, byte and time words, the roots the figures are read from,
//! processor times, network and disk counters, processes and their network and disk use, processor use between two samples, the processes that
//! may not be signalled), the `kubectl` trait and the pod table and process rows of `sysprocs`. No I/O, no agent, no shell.
//! `iyakki_core_api::sysinfo`, `iyakki_shared::kubectl` and `iyakki_shared::sysprocs` re-export every item under the old path (alpha.9 P6, D-S2-01).

pub mod kubectl;
pub mod sysprocs;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ticks: u64) -> Proc {
        Proc { pid, ppid: 1, uid: 0, user: "root".into(), name: "p".into(), cmd: String::new(), state: 'S', threads: 1, ticks, rss: 0, start_ticks: 0, cgroup: String::new() }
    }

    #[test]
    fn bytes_and_spans_are_said_in_short_words() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_500), "1.5 KB");
        assert_eq!(human_bytes(3_200_000_000), "3.2 GB");
        assert_eq!(human_duration(5), "5s");
        assert_eq!(human_duration(310), "5m 10s");
        assert_eq!(human_duration(3 * 3600 + 120), "3h 2m");
        assert_eq!(human_duration(86400 * 3 + 4 * 3600), "3d 4h");
    }

    #[test]
    fn processor_use_is_a_share_of_the_ticks_between_two_samples() {
        let a = CpuTimes { busy: 100, total: 1000 };
        assert_eq!(cpu_percent(a, CpuTimes { busy: 150, total: 1100 }), 50.0);
        assert_eq!(cpu_percent(a, a), 0.0);
        assert_eq!(cpu_percent(CpuTimes { busy: 500, total: 1000 }, CpuTimes { busy: 100, total: 1100 }), 0.0);
    }

    #[test]
    fn each_process_is_measured_in_percent_of_one_processor() {
        let m = proc_cpu(&[proc(2, 100), proc(3, 5)], &[proc(2, 300), proc(4, 9)], 1.0);
        assert_eq!(m.len(), 1);
        assert!((m[&2] - 200.0).abs() < 0.01);
    }

    #[test]
    fn the_system_and_this_shell_are_never_signalled() {
        assert!(protected(0).is_some() && protected(1).is_some());
        assert!(protected(std::process::id()).is_some());
        assert!(protected(std::process::id().wrapping_add(7)).is_none());
    }

    #[test]
    fn network_and_disk_use_say_nothing_when_quiet() {
        assert!(NetUse::default().is_quiet() && NetUse::default().text().is_empty());
        let n = NetUse { connections: 3, listening: 1, udp: 0, bytes: Some((1_200_000, 40_000)) };
        assert_eq!(n.text(), "3 conn · 1 listening · ↓ 1.2 MB ↑ 40.0 KB");
        assert!(DiskUse::default().text().is_empty());
        assert_eq!(DiskUse { read_bytes: 2_000, write_bytes: 0 }.text(), "disk ↓ 2.0 KB ↑ 0 B");
    }

    #[test]
    fn memory_in_use_is_the_total_less_what_is_available() {
        let m = Memory { total: 100, available: 30, swap_total: 0, swap_free: 0 };
        assert_eq!(m.used(), 70);
        assert_eq!(Memory { total: 10, available: 20, ..Default::default() }.used(), 0);
        assert_eq!(SysRoots::real().proc, std::path::PathBuf::from("/proc"));
    }
}
