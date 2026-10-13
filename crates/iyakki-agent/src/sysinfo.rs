//! What the machine is doing right now, read from `/proc` and `/sys`: processor use, memory, load, disks, network, heat and
//! every process. Both the SYSTEM tile and `iyakki top` use it. The folders are parameters so tests can supply a made-up
//! machine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Cumulative processor time in clock ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

pub fn cpu_times(r: &SysRoots) -> CpuTimes {
    let text = read(&r.proc.join("stat"));
    let Some(line) = text.lines().find(|l| l.starts_with("cpu ")) else { return CpuTimes::default() };
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal
    let get = |i: usize| v.get(i).copied().unwrap_or(0);
    let idle = get(3) + get(4);
    let total: u64 = v.iter().take(8).sum();
    CpuTimes { busy: total.saturating_sub(idle), total }
}

/// How much of the processor was used between two samples, 0 to 100.
pub fn cpu_percent(a: CpuTimes, b: CpuTimes) -> f32 {
    let (dt, db) = (b.total.saturating_sub(a.total), b.busy.saturating_sub(a.busy));
    if dt == 0 { 0.0 } else { (db as f32 / dt as f32 * 100.0).clamp(0.0, 100.0) }
}

pub fn cpu_count(r: &SysRoots) -> usize {
    read(&r.proc.join("stat")).lines().filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(|c| c.is_ascii_digit())).count().max(1)
}

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
pub fn memory(r: &SysRoots) -> Memory {
    let text = read(&r.proc.join("meminfo"));
    let kb = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.trim_start_matches(':').split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0) * 1024;
    let total = kb("MemTotal");
    Memory { total, available: if text.contains("MemAvailable") { kb("MemAvailable") } else { kb("MemFree") + kb("Cached") }, swap_total: kb("SwapTotal"), swap_free: kb("SwapFree") }
}

pub fn load_average(r: &SysRoots) -> (f32, f32, f32) {
    let t = read(&r.proc.join("loadavg"));
    let mut it = t.split_whitespace().filter_map(|x| x.parse::<f32>().ok());
    (it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0))
}

pub fn uptime_secs(r: &SysRoots) -> u64 {
    read(&r.proc.join("uptime")).split_whitespace().next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) as u64
}

/// Hottest temperature sensor in degrees Celsius, if the machine has any.
pub fn hottest_temp(r: &SysRoots) -> Option<f32> {
    std::fs::read_dir(r.sys.join("class/thermal")).ok()?.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone")).filter_map(|e| read(&e.path().join("temp")).trim().parse::<f32>().ok()).map(|m| m / 1000.0).filter(|t| *t > 0.0 && *t < 150.0).fold(None, |m, t| Some(m.map_or(t, |x: f32| x.max(t))))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetCounters {
    pub iface: String,
    pub rx: u64,
    pub tx: u64,
}

/// Bytes received and sent per interface since boot (the loopback is left out).
pub fn net_counters(r: &SysRoots) -> Vec<NetCounters> {
    read(&r.proc.join("net/dev"))
        .lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            let name = name.trim();
            (name != "lo" && v.len() >= 9).then(|| NetCounters { iface: name.into(), rx: v[0], tx: v[8] })
        })
        .collect()
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
}

/// Clock ticks per second, as every Linux reports in `/proc` (USER_HZ is 100 on all supported machines).
pub const TICKS_PER_SEC: u64 = 100;
const PAGE: u64 = 4096;

fn user_names(r: &SysRoots) -> HashMap<u32, String> {
    read(&r.passwd).lines().filter_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        let uid = f.nth(1)?.parse().ok()?;
        Some((uid, name.to_string()))
    }).collect()
}

fn parse_stat(text: &str) -> Option<(String, char, u32, u64, u32, u64, u64)> {
    // pid (comm) state ppid ... : the name can contain spaces and brackets, so split at the last `)`
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let name = text[open + 1..close].to_string();
    let f: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // f[0] state, f[1] ppid, f[11] utime, f[12] stime, f[17] num_threads, f[19] starttime, f[21] rss (pages)
    let state = f.first()?.chars().next()?;
    let ppid = f.get(1)?.parse().ok()?;
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let threads = f.get(17)?.parse().ok()?;
    let start = f.get(19)?.parse().ok()?;
    let rss = f.get(21)?.parse::<i64>().ok()?.max(0) as u64;
    Some((name, state, ppid, ticks, threads, start, rss))
}

/// Every process that is still there when read.
pub fn processes(r: &SysRoots) -> Vec<Proc> {
    let names = user_names(r);
    let mut out = Vec::new();
    for e in std::fs::read_dir(&r.proc).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_string_lossy().parse::<u32>().ok() else { continue };
        let d = e.path();
        let Some((name, state, ppid, ticks, threads, start_ticks, rss_pages)) = parse_stat(&read(&d.join("stat"))) else { continue };
        let status = read(&d.join("status"));
        let uid = status.lines().find_map(|l| l.strip_prefix("Uid:")?.split_whitespace().next()?.parse().ok()).unwrap_or(0);
        let cmd = read(&d.join("cmdline")).split('\0').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
        out.push(Proc { pid, ppid, uid, user: names.get(&uid).cloned().unwrap_or_else(|| uid.to_string()), name, cmd, state, threads, ticks, rss: rss_pages * PAGE, start_ticks });
    }
    out.sort_by_key(|p| p.pid);
    out
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

/// Send a polite stop (SIGTERM) or a hard kill (SIGKILL) to a process. Process 1 and ourselves are refused.
pub fn signal(pid: u32, hard: bool) -> Result<(), String> {
    if let Some(why) = protected(pid) {
        return Err(format!("refused: {why}"));
    }
    // SAFETY: kill(2) takes two integers and touches no memory of ours.
    let r = unsafe { libc::kill(pid as libc::pid_t, if hard { libc::SIGKILL } else { libc::SIGTERM }) };
    if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < U.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn human_duration(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60);
    if d > 0 { format!("{d}d {h}h") } else if h > 0 { format!("{h}h {m}m") } else if m > 0 { format!("{m}m {}s", secs % 60) } else { format!("{secs}s") }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A made-up machine under a temp folder.
    pub fn machine(tag: &str) -> SysRoots {
        let d = std::env::temp_dir().join(format!("iyakki-sysinfo-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (proc, sys) = (d.join("proc"), d.join("sys"));
        std::fs::create_dir_all(proc.join("net")).unwrap();
        std::fs::create_dir_all(sys.join("class/thermal/thermal_zone0")).unwrap();
        std::fs::write(proc.join("stat"), "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\n").unwrap();
        std::fs::write(proc.join("meminfo"), "MemTotal:        8000000 kB\nMemFree:          500000 kB\nMemAvailable:    6000000 kB\nSwapTotal:       1000000 kB\nSwapFree:         900000 kB\n").unwrap();
        std::fs::write(proc.join("loadavg"), "0.52 0.40 0.30 1/200 999\n").unwrap();
        std::fs::write(proc.join("uptime"), "93784.55 180000.00\n").unwrap();
        std::fs::write(proc.join("net/dev"), "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0\n  eth0: 123456 100 0 0 0 0 0 0 7890 50 0 0 0 0 0 0\n").unwrap();
        std::fs::write(sys.join("class/thermal/thermal_zone0/temp"), "47500\n").unwrap();
        std::fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/sh\nnobody:x:65534:65534::/:/bin/false\n").unwrap();
        for (pid, name, state, ppid, utime, stime, threads, start, rss, uid, cmd) in [(1u32, "iyakki", 'S', 0u32, 500u64, 100u64, 12u32, 1u64, 5000u64, 0u32, "/sbin/iyakki\0serve\0"), (42, "my (odd) prog", 'R', 1, 3000, 1000, 1, 900, 25000, 65534, "")] {
            let p = proc.join(pid.to_string());
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("stat"), format!("{pid} ({name}) {state} {ppid} 0 0 0 0 0 0 0 0 0 {utime} {stime} 0 0 20 0 {threads} 0 {start} 123456 {rss} 0\n")).unwrap();
            std::fs::write(p.join("status"), format!("Name:\t{name}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n")).unwrap();
            std::fs::write(p.join("cmdline"), cmd).unwrap();
        }
        std::fs::write(proc.join("self"), "not a process folder").unwrap();
        SysRoots { proc, sys, passwd: d.join("passwd") }
    }

    #[test]
    fn processor_memory_load_uptime_heat_and_network_are_read() {
        let r = machine("basic");
        let t = cpu_times(&r);
        assert_eq!((t.busy, t.total), (150, 1000));
        assert_eq!(cpu_count(&r), 2);
        assert_eq!(cpu_percent(CpuTimes { busy: 150, total: 1000 }, CpuTimes { busy: 250, total: 1200 }), 50.0);
        assert_eq!(cpu_percent(t, t), 0.0, "no time passed");
        let m = memory(&r);
        assert_eq!((m.total, m.available, m.used()), (8_192_000_000, 6_144_000_000, 2_048_000_000));
        assert_eq!((m.swap_total, m.swap_free), (1_024_000_000, 921_600_000));
        assert_eq!(load_average(&r), (0.52, 0.40, 0.30));
        assert_eq!(uptime_secs(&r), 93784);
        assert_eq!(hottest_temp(&r), Some(47.5));
        assert_eq!(net_counters(&r), vec![NetCounters { iface: "eth0".into(), rx: 123456, tx: 7890 }], "the loopback is left out");
    }

    #[test]
    fn processes_are_read_with_odd_names_owners_and_command_lines() {
        let r = machine("procs");
        let p = processes(&r);
        assert_eq!(p.len(), 2, "the entry that is not a process is skipped");
        assert_eq!((p[0].pid, p[0].name.as_str(), p[0].user.as_str(), p[0].cmd.as_str(), p[0].state, p[0].threads), (1, "iyakki", "root", "/sbin/iyakki serve", 'S', 12));
        assert_eq!((p[0].ticks, p[0].rss), (600, 5000 * 4096));
        assert_eq!((p[1].name.as_str(), p[1].user.as_str(), p[1].ppid, p[1].ticks), ("my (odd) prog", "nobody", 1, 4000), "a name with brackets and spaces is read whole");
        assert_eq!(p[1].cmd, "", "a kernel-style process has no command line");
    }

    #[test]
    fn processor_use_per_process_comes_from_two_samples() {
        let r = machine("delta");
        let before = processes(&r);
        let mut after = before.clone();
        after[1].ticks += 150; // 1.5 s of processor time
        after.push(Proc { pid: 77, ..after[0].clone() }); // started since: no earlier sample
        let cpu = proc_cpu(&before, &after, 2.0);
        assert_eq!(cpu.get(&42).copied(), Some(75.0));
        assert_eq!(cpu.get(&1).copied(), Some(0.0));
        assert!(cpu.get(&77).is_none());
    }

    #[test]
    fn process_one_and_ourselves_cannot_be_signalled_and_a_real_child_can() {
        assert!(signal(1, true).unwrap_err().contains("system itself"));
        assert!(signal(0, false).is_err());
        assert!(signal(std::process::id(), false).unwrap_err().contains("your shell") || signal(std::process::id(), false).is_err());
        let mut child = std::process::Command::new("sleep").arg("30").spawn().expect("sleep");
        signal(child.id(), false).expect("terminate");
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(15));
        assert!(signal(4_000_000, false).is_err(), "no such process");
    }

    #[test]
    fn sizes_and_durations_read_well() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_500_000), "1.5 MB");
        assert_eq!(human_bytes(2_000_000_000_000), "2.0 TB");
        assert_eq!(human_duration(42), "42s");
        assert_eq!(human_duration(125), "2m 5s");
        assert_eq!(human_duration(3 * 3600 + 120), "3h 2m");
        assert_eq!(human_duration(2 * 86400 + 5 * 3600), "2d 5h");
        // the real machine can be read without panicking
        let real = SysRoots::real();
        assert!(memory(&real).total > 0 && !processes(&real).is_empty() && cpu_count(&real) >= 1);
    }
}
