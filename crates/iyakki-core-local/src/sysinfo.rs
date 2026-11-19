//! What the machine is doing right now, read from `/proc` and `/sys`: processor use, memory, load, disks, network, heat and
//! every process. Both the SYSTEM tile and `iyakki top` use it. The folders are parameters so tests can supply a made-up
//! machine.

use std::collections::HashMap;
use std::path::Path;
pub use iyakki_core_api::sysinfo::{CpuTimes, DiskCounters, DiskUse, NetCounters, NetUse, Proc, SysRoots, TICKS_PER_SEC, cpu_percent, proc_cpu, protected};



fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
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


pub fn cpu_count(r: &SysRoots) -> usize {
    read(&r.proc.join("stat")).lines().filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(|c| c.is_ascii_digit())).count().max(1)
}

pub use iyakki_core_api::sysinfo::Memory;

/// Memory figures from `<proc>/meminfo`.
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


/// Bytes read and written per whole disk since boot, from the kernel's disk statistics (partitions, loop, RAM and compressed-memory
/// devices are left out so nothing is counted twice; the kernel counts in 512-byte sectors).
pub fn disk_counters(r: &SysRoots) -> Vec<DiskCounters> {
    read(&r.proc.join("diskstats"))
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let name = *f.get(2)?;
            if name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") || !r.sys.join("block").join(name).exists() {
                return None;
            }
            let rd: u64 = f.get(5)?.parse().ok()?;
            let wr: u64 = f.get(9)?.parse().ok()?;
            Some(DiskCounters { disk: name.into(), read: rd.saturating_mul(512), written: wr.saturating_mul(512) })
        })
        .collect()
}


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

/// The line of a process's control-group text that names a Kubernetes pod (`kubepods`), else empty. Kept short on purpose: it is
/// only read again by `sysprocs` in the agent to find the pod.
pub fn container_cgroup(text: &str) -> String {
    text.lines().find(|l| l.contains("kubepods")).map(|l| l.chars().take(300).collect()).unwrap_or_default()
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
        out.push(Proc { pid, ppid, uid, user: names.get(&uid).cloned().unwrap_or_else(|| uid.to_string()), name, cmd, state, threads, ticks, rss: rss_pages * PAGE, start_ticks, cgroup: container_cgroup(&read(&d.join("cgroup"))) });
    }
    out.sort_by_key(|p| p.pid);
    out
}





/// Reads the text of `/proc/<pid>/io`; a missing or unreadable file (another person's process, a process that has gone) is `None`.
pub fn parse_disk_use(text: &str) -> Option<DiskUse> {
    let (mut r, mut w) = (None, None);
    for l in text.lines() {
        match l.split_once(':') {
            Some(("read_bytes", v)) => r = v.trim().parse::<u64>().ok(),
            Some(("write_bytes", v)) => w = v.trim().parse::<u64>().ok(),
            _ => {}
        }
    }
    Some(DiskUse { read_bytes: r?, write_bytes: w? })
}

/// The disk use of the given processes (only those this program may look at). Call it for the few processes being shown.
pub fn disk_use(r: &SysRoots, pids: &[u32]) -> HashMap<u32, DiskUse> {
    pids.iter().filter_map(|p| Some((*p, parse_disk_use(&std::fs::read_to_string(r.proc.join(p.to_string()).join("io")).ok()?)?))).collect()
}

/// Socket number to what it is (`b'E'` established TCP, `b'L'` listening TCP, `b'U'` UDP), from the kernel's four socket tables.
fn socket_table(r: &SysRoots) -> HashMap<u64, u8> {
    let mut out = HashMap::new();
    for (file, udp) in [("net/tcp", false), ("net/tcp6", false), ("net/udp", true), ("net/udp6", true)] {
        for l in read(&r.proc.join(file)).lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (Some(st), Some(ino)) = (f.get(3), f.get(9).and_then(|x| x.parse::<u64>().ok())) else { continue };
            let kind = if udp { b'U' } else { match *st { "01" => b'E', "0A" => b'L', _ => continue } };
            out.insert(ino, kind);
        }
    }
    out
}

/// One side of a connection as the kernel's socket tables print it: `0100007F:1F90` (IPv4, address bytes in memory order) or the 32 hex
/// digits of an IPv6 address (four words in memory order), a colon and the port in hex.
fn parse_endpoint(s: &str) -> Option<(std::net::IpAddr, u16)> {
    let (a, p) = s.split_once(':')?;
    let port = u16::from_str_radix(p, 16).ok()?;
    let word = |h: &str| u32::from_str_radix(h, 16).ok().map(|w| w.to_le_bytes());
    let ip = match a.len() {
        8 => std::net::IpAddr::from(word(a)?),
        32 => {
            let mut b = [0u8; 16];
            for i in 0..4 {
                b[i * 4..i * 4 + 4].copy_from_slice(&word(a.get(i * 8..i * 8 + 8)?)?);
            }
            std::net::IpAddr::from(b)
        }
        _ => return None,
    };
    Some((canonical_ip(ip), port))
}

/// An IPv4 address carried in an IPv6 socket is the IPv4 address.
fn canonical_ip(ip: std::net::IpAddr) -> std::net::IpAddr {
    match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Socket number to its two ends, for sockets that have a peer (established TCP and connected UDP).
fn socket_ends(r: &SysRoots) -> HashMap<u64, ((std::net::IpAddr, u16), (std::net::IpAddr, u16))> {
    let mut out = HashMap::new();
    for file in ["net/tcp", "net/tcp6", "net/udp", "net/udp6"] {
        for l in read(&r.proc.join(file)).lines().skip(1) {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (Some(local), Some(remote), Some(ino)) = (f.get(1).and_then(|x| parse_endpoint(x)), f.get(2).and_then(|x| parse_endpoint(x)), f.get(9).and_then(|x| x.parse::<u64>().ok())) else { continue };
            if remote.1 != 0 {
                out.insert(ino, (local, remote));
            }
        }
    }
    out
}

/// One tracked connection of the kernel's conntrack table with its byte counters (`nf_conntrack_acct` on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flow {
    pub orig_src: (std::net::IpAddr, u16),
    pub orig_dst: (std::net::IpAddr, u16),
    /// Bytes the originator sent and bytes the other side answered.
    pub orig_bytes: u64,
    pub reply_bytes: u64,
}

/// Reads `/proc/net/nf_conntrack`: `ipv4 2 tcp 6 300 ESTABLISHED src=A dst=B sport=1 dport=2 packets=3 bytes=4 src=B dst=A sport=2 dport=1 packets=5 bytes=6 ...`.
/// Only TCP and UDP lines with both byte counters are kept (without accounting there are none, which tells the caller it is off).
pub fn parse_conntrack(text: &str) -> Vec<Flow> {
    let mut out = Vec::new();
    for l in text.lines() {
        let mut t = l.split_whitespace();
        let proto = t.nth(2).unwrap_or("");
        if proto != "tcp" && proto != "udp" {
            continue;
        }
        let (mut src, mut dst, mut sport, mut dport) = (None, None, None, None);
        let mut sides: Vec<((std::net::IpAddr, u16), (std::net::IpAddr, u16), u64)> = Vec::new();
        for w in l.split_whitespace() {
            let Some((k, v)) = w.split_once('=') else { continue };
            match k {
                "src" => src = v.parse::<std::net::IpAddr>().ok().map(canonical_ip),
                "dst" => dst = v.parse::<std::net::IpAddr>().ok().map(canonical_ip),
                "sport" => sport = v.parse::<u16>().ok(),
                "dport" => dport = v.parse::<u16>().ok(),
                "bytes" => {
                    if let (Some(s), Some(d), Some(sp), Some(dp), Ok(b)) = (src, dst, sport, dport, v.parse::<u64>()) {
                        sides.push(((s, sp), (d, dp), b));
                    }
                    (src, dst, sport, dport) = (None, None, None, None);
                }
                _ => {}
            }
        }
        if sides.len() == 2 {
            out.push(Flow { orig_src: sides[0].0, orig_dst: sides[0].1, orig_bytes: sides[0].2, reply_bytes: sides[1].2 });
        }
    }
    out
}

/// Switches on the kernel's per-connection byte counters (`net/netfilter/nf_conntrack_acct`), which the per-process figures of
/// [`net_use`] need. `Ok(true)` when it was off and is now on, `Ok(false)` when it was already on, `Err` when the machine has no
/// connection tracking or the write is refused (not root).
pub fn enable_conntrack_accounting(proc: &Path) -> Result<bool, String> {
    let f = proc.join("sys/net/netfilter/nf_conntrack_acct");
    let now = std::fs::read_to_string(&f).map_err(|_| "this kernel has no connection tracking accounting".to_string())?;
    if now.trim() == "1" {
        return Ok(false);
    }
    std::fs::write(&f, "1").map_err(|e| format!("could not switch on byte counting: {e}"))?;
    Ok(true)
}

/// Bytes received and sent by one process over its connections, from the conntrack accounting: the process's sockets are matched to
/// flows by their two ends. A connection that the machine NATs, or a UDP socket without a peer, is not matched (its bytes are missing,
/// not invented).
pub fn flow_bytes(sockets: &[((std::net::IpAddr, u16), (std::net::IpAddr, u16))], flows: &[Flow]) -> (u64, u64) {
    let (mut rx, mut tx) = (0u64, 0u64);
    for (local, remote) in sockets {
        for f in flows {
            if f.orig_src == *local && f.orig_dst == *remote {
                tx = tx.saturating_add(f.orig_bytes);
                rx = rx.saturating_add(f.reply_bytes);
            } else if f.orig_dst == *local && f.orig_src == *remote {
                tx = tx.saturating_add(f.reply_bytes);
                rx = rx.saturating_add(f.orig_bytes);
            }
        }
    }
    (rx, tx)
}

/// The sockets of the given processes. Reads each process's `fd` folder (a process that has gone, or one this program may not look
/// into, is simply absent), so call it for the few processes being shown, not for the whole machine every second.
pub fn net_use(r: &SysRoots, pids: &[u32]) -> HashMap<u32, NetUse> {
    let table = socket_table(r);
    let ends = socket_ends(r);
    let flows = parse_conntrack(&read(&r.proc.join("net/nf_conntrack")));
    let init_ns = std::fs::read_link(r.proc.join("1/ns/net")).ok();
    let mut out = HashMap::new();
    for pid in pids {
        let d = r.proc.join(pid.to_string());
        let mut u = NetUse::default();
        let mut seen = std::collections::HashSet::new();
        let mut my_ends = Vec::new();
        for e in std::fs::read_dir(d.join("fd")).into_iter().flatten().flatten() {
            let Ok(t) = std::fs::read_link(e.path()) else { continue };
            let t = t.to_string_lossy().into_owned();
            let Some(ino) = t.strip_prefix("socket:[").and_then(|x| x.strip_suffix(']')).and_then(|x| x.parse::<u64>().ok()) else { continue };
            if !seen.insert(ino) {
                continue;
            }
            if let Some(e) = ends.get(&ino) {
                my_ends.push(*e);
            }
            match table.get(&ino) {
                Some(b'E') => u.connections += 1,
                Some(b'L') => u.listening += 1,
                Some(b'U') => u.udp += 1,
                _ => {}
            }
        }
        // a namespace of its own: its interface counters are this process's (and its pod mates') traffic
        let ns = std::fs::read_link(d.join("ns/net")).ok();
        if ns.is_some() && init_ns.is_some() && ns != init_ns {
            let sum = read(&d.join("net/dev")).lines().skip(2).filter_map(|l| {
                let (name, rest) = l.split_once(':')?;
                let v: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                (name.trim() != "lo" && v.len() >= 9).then(|| (v[0], v[8]))
            }).fold((0u64, 0u64), |(a, b), (x, y)| (a.saturating_add(x), b.saturating_add(y)));
            u.bytes = Some(sum);
        } else if !flows.is_empty() {
            // a host process: the kernel counts per connection when conntrack accounting is on (F-SYS-10)
            u.bytes = Some(flow_bytes(&my_ends, &flows));
        }
        out.insert(*pid, u);
    }
    out
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

pub use iyakki_core_api::sysinfo::{human_bytes, human_duration};

/// Where a short-lived shared file for another process belongs: the RAM-backed `/dev/shm` where it exists, else the temp folder.
/// (Kept here, in IYAKKI core, so that tiles do not name kernel paths themselves.)
pub fn shared_memory_dir() -> std::path::PathBuf {
    let shm = std::path::Path::new("/dev/shm");
    if shm.is_dir() {
        shm.to_path_buf()
    } else {
        std::env::temp_dir()
    }
}

/// The real `/proc` folder (the one place the tiles and services get it from; tests pass a made-up root instead).
pub fn proc_root() -> &'static Path {
    Path::new("/proc")
}

/// The real `/sys` folder.
pub fn sys_root() -> &'static Path {
    Path::new("/sys")
}

/// The kernel's network interface folder (`/sys/class/net`): one folder per interface.
pub fn net_class_root() -> &'static Path {
    Path::new("/sys/class/net")
}

/// The kernel command line (`<proc>/cmdline`); empty when unreadable.
pub fn cmdline(proc: &Path) -> String {
    iyakki_storage::cmdline_in(proc)
}

/// Seconds since boot with the fraction, from `<proc>/uptime`; `None` when unreadable.
pub fn uptime_f64(proc: &Path) -> Option<f64> {
    iyakki_storage::uptime_in(proc)
}

/// `MemAvailable` of this machine in bytes (`None` when the line is missing; no fallback to free memory).
pub fn mem_available_bytes() -> Option<u64> {
    mem_available_in(proc_root())
}

fn mem_available_in(proc: &Path) -> Option<u64> {
    let t = std::fs::read_to_string(proc.join("meminfo")).ok()?;
    t.lines().find_map(|l| l.strip_prefix("MemAvailable:")).and_then(|v| v.split_whitespace().next()).and_then(|n| n.parse::<u64>().ok()).map(|kb| kb * 1024)
}

/// Whole seconds since boot on this machine (0 when unreadable).
pub fn uptime_now() -> u64 {
    uptime_secs(&SysRoots::real())
}

/// Installed memory in whole kibibytes (0 when unreadable).
pub fn mem_total_kb(r: &SysRoots) -> u64 {
    memory(r).total / 1024
}

/// The machine's model (`<sys>/class/dmi/id/product_name`), trimmed; empty when unknown.
pub fn product_name(sys: &Path) -> String {
    read(&sys.join("class/dmi/id/product_name")).trim().to_string()
}

/// The user and group that own this process (`<proc>/self`), or root when unreadable.
pub fn self_owner(proc: &Path) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(proc.join("self")).map(|m| (m.uid(), m.gid())).unwrap_or((0, 0))
}

#[cfg(any(test, feature = "test-support"))]
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
    fn disk_counters_count_whole_disks_in_bytes_and_leave_out_partitions_loop_and_ram() {
        let r = tests::machine("diskstats");
        std::fs::create_dir_all(r.sys.join("block/sda")).unwrap();
        std::fs::create_dir_all(r.sys.join("block/nvme0n1")).unwrap();
        std::fs::create_dir_all(r.sys.join("block/loop0")).unwrap();
        std::fs::create_dir_all(r.sys.join("block/zram0")).unwrap();
        std::fs::write(
            r.proc.join("diskstats"),
            "   8       0 sda 100 0 2000 5 50 0 1000 7 0 12 12
   8       1 sda1 90 0 1900 4 40 0 900 6 0 10 10
 259       0 nvme0n1 10 0 8 1 20 0 16 2 0 3 3
   7       0 loop0 5 0 5000 0 0 0 0 0 0 0 0
 253       0 zram0 5 0 4000 0 5 0 4000 0 0 0 0
   1       0 ram0 1 0 8 0 0 0 0 0 0 0 0
 garbage line
",
        )
        .unwrap();
        let d = disk_counters(&r);
        assert_eq!(d, vec![DiskCounters { disk: "sda".into(), read: 2000 * 512, written: 1000 * 512 }, DiskCounters { disk: "nvme0n1".into(), read: 8 * 512, written: 16 * 512 }]);
        // no statistics file (macOS, a container): nothing, not an error
        std::fs::remove_file(r.proc.join("diskstats")).unwrap();
        assert!(disk_counters(&r).is_empty());
    }

    #[test]
    fn sockets_of_a_process_are_counted_by_kind_and_bytes_exist_only_in_a_namespace_of_its_own() {
        let r = machine("netuse");
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5001 1 0 100 0 0 10 0\n   1: 0100007F:1F91 0100007F:C000 01 00000000:00000000 00:00000000 00000000     0        0 5002 1 0 100 0 0 10 0\n   2: 0100007F:1F92 0100007F:C001 06 00000000:00000000 00:00000000 00000000     0        0 5003 1 0 100 0 0 10 0\n";
        std::fs::write(r.proc.join("net/tcp"), table).unwrap();
        std::fs::write(r.proc.join("net/udp"), "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n   0: 00000000:0044 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 6001 2 0\n").unwrap();
        // pid 1: a listener, an established connection (twice open: counted once), a closing one (ignored), a pipe; same namespace as init
        let (p1, p42) = (r.proc.join("1"), r.proc.join("42"));
        for (d, links) in [(&p1, vec![("3", "socket:[5001]"), ("4", "socket:[5002]"), ("5", "socket:[5002]"), ("6", "socket:[5003]"), ("7", "pipe:[77]")]), (&p42, vec![("3", "socket:[6001]"), ("4", "socket:[9999]")])] {
            std::fs::create_dir_all(d.join("fd")).unwrap();
            std::fs::create_dir_all(d.join("ns")).unwrap();
            for (n, t) in links {
                std::os::unix::fs::symlink(t, d.join("fd").join(n)).unwrap();
            }
        }
        std::os::unix::fs::symlink("net:[4026531840]", p1.join("ns/net")).unwrap();
        // pid 42 lives in a namespace of its own, with its own interface counters
        std::os::unix::fs::symlink("net:[4026532999]", p42.join("ns/net")).unwrap();
        std::fs::create_dir_all(p42.join("net")).unwrap();
        std::fs::write(p42.join("net/dev"), "Inter-|   Receive |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n    lo: 1 1 0 0 0 0 0 0 1 1 0 0 0 0 0 0\n  eth0: 2000 5 0 0 0 0 0 0 900 4 0 0 0 0 0 0\n").unwrap();
        let u = net_use(&r, &[1, 42, 4242]);
        assert_eq!(u[&1], NetUse { connections: 1, listening: 1, udp: 0, bytes: None }, "a host process: connections only, no bytes");
        assert_eq!(u[&1].text(), "1 conn · 1 listening");
        assert_eq!(u[&42], NetUse { connections: 0, listening: 0, udp: 1, bytes: Some((2000, 900)) }, "its own namespace: the namespace's bytes, loopback left out");
        assert!(u[&42].text().contains("1 udp") && u[&42].text().contains("↓ 2.0 KB ↑ 900 B"), "{}", u[&42].text());
        assert!(u[&4242].is_quiet() && u[&4242].text().is_empty(), "a process that is not there has nothing");
        // no socket tables at all (another system): everything is quiet, nothing fails
        let bare = machine("netuse-bare");
        assert!(net_use(&bare, &[1, 42]).values().all(NetUse::is_quiet));
    }

    #[test]
    fn conntrack_bytes_are_matched_to_the_sockets_of_a_host_process() {
        let r = machine("conntrack");
        // established TCP, local 10.0.0.5:4444 (0A000005, little endian bytes) to 1.2.3.4:443
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0500000A:115C 04030201:01BB 01 00000000:00000000 00:00000000 00000000     0        0 7001 1 0 100 0 0 10 0\n   1: 0500000A:0016 090A000A:C350 01 00000000:00000000 00:00000000 00000000     0        0 7002 1 0 100 0 0 10 0\n";
        std::fs::write(r.proc.join("net/tcp"), tcp).unwrap();
        let ct = "ipv4     2 tcp      6 431999 ESTABLISHED src=10.0.0.5 dst=1.2.3.4 sport=4444 dport=443 packets=10 bytes=1000 src=1.2.3.4 dst=10.0.0.5 sport=443 dport=4444 packets=12 bytes=5000 [ASSURED] mark=0 zone=0 use=2\nipv4     2 tcp      6 431999 ESTABLISHED src=10.0.10.9 dst=10.0.0.5 sport=50000 dport=22 packets=4 bytes=300 src=10.0.0.5 dst=10.0.10.9 sport=22 dport=50000 packets=3 bytes=700 [ASSURED] mark=0 zone=0 use=2\nipv4     2 icmp     1 29 src=10.0.0.5 dst=8.8.8.8 type=8 code=0 id=1 packets=1 bytes=84 src=8.8.8.8 dst=10.0.0.5 type=0 code=0 id=1 packets=1 bytes=84 mark=0 use=1\n";
        std::fs::write(r.proc.join("net/nf_conntrack"), ct).unwrap();
        let p = r.proc.join("1");
        std::fs::create_dir_all(p.join("fd")).unwrap();
        std::fs::create_dir_all(p.join("ns")).unwrap();
        for (n, t) in [("3", "socket:[7001]"), ("4", "socket:[7002]")] {
            std::os::unix::fs::symlink(t, p.join("fd").join(n)).unwrap();
        }
        std::os::unix::fs::symlink("net:[4026531840]", p.join("ns/net")).unwrap();
        let u = net_use(&r, &[1]);
        // outbound: sent 1000, received 5000; the ssh login answered on port 22: sent (reply) 700, received (orig) 300
        assert_eq!(u[&1].bytes, Some((5300, 1700)));
        assert_eq!(u[&1].connections, 2);
        // the parser keeps only tcp and udp lines with both counters
        assert_eq!(parse_conntrack(ct).len(), 2);
        assert!(parse_conntrack("ipv4 2 tcp 6 10 ESTABLISHED src=1.1.1.1 dst=2.2.2.2 sport=1 dport=2 src=2.2.2.2 dst=1.1.1.1 sport=2 dport=1\n").is_empty(), "no byte counters: accounting is off");
        assert_eq!(parse_endpoint("0500000A:115C"), Some(("10.0.0.5".parse().unwrap(), 4444)));
        assert!(parse_endpoint("zz:1").is_none() && parse_endpoint("0500000A").is_none());
        let v6 = parse_endpoint("00000000000000000000000001000000:0050").unwrap();
        assert_eq!(v6.0, "::1".parse::<std::net::IpAddr>().unwrap());
    }

    #[test]
    fn conntrack_accounting_is_switched_on_once_and_a_machine_without_it_says_so() {
        let r = machine("ctacct");
        assert!(enable_conntrack_accounting(&r.proc).is_err());
        let d = r.proc.join("sys/net/netfilter");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("nf_conntrack_acct"), "0\n").unwrap();
        assert_eq!(enable_conntrack_accounting(&r.proc), Ok(true));
        assert_eq!(std::fs::read_to_string(d.join("nf_conntrack_acct")).unwrap(), "1");
        assert_eq!(enable_conntrack_accounting(&r.proc), Ok(false));
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

    #[test]
    fn kernel_helpers_read_from_the_given_roots() {
        let r = machine("helpers");
        std::fs::write(r.proc.join("cmdline"), "quiet iyakki.x=1\n").unwrap();
        assert_eq!(cmdline(&r.proc), "quiet iyakki.x=1\n");
        assert_eq!(cmdline(&r.proc.join("nowhere")), "");
        assert_eq!(mem_total_kb(&r), 8_000_000);
        std::fs::create_dir_all(r.sys.join("class/dmi/id")).unwrap();
        std::fs::write(r.sys.join("class/dmi/id/product_name"), "MacBookPro14,1\n").unwrap();
        assert_eq!(product_name(&r.sys), "MacBookPro14,1");
        assert_eq!(product_name(&r.proc.join("nowhere")), "");
        std::fs::remove_file(r.proc.join("self")).unwrap();
        std::fs::create_dir_all(r.proc.join("self")).unwrap();
        let m = std::fs::metadata(r.proc.join("self")).unwrap();
        use std::os::unix::fs::MetadataExt;
        assert_eq!(self_owner(&r.proc), (m.uid(), m.gid()));
        assert_eq!(self_owner(&r.proc.join("nowhere")), (0, 0));
        assert_eq!(uptime_f64(&r.proc), Some(93784.55));
        assert_eq!(uptime_f64(&r.proc.join("nowhere")), None);
        assert_eq!(mem_available_in(&r.proc), Some(6_000_000 * 1024));
        std::fs::write(r.proc.join("meminfo"), "MemTotal: 1 kB\nMemFree: 1 kB\n").unwrap();
        assert_eq!(mem_available_in(&r.proc), None, "no MemAvailable line, no guess");
        assert_eq!(net_class_root(), Path::new("/sys/class/net"));
        assert_eq!(proc_root(), Path::new("/proc"));
        assert_eq!(sys_root(), Path::new("/sys"));
    }
    /// F-SYS-01 against the real kernel and the real tools: a real `sleep` and a real busy shell are started; the process list read
    /// from the real `/proc` finds them with the right parent, owner and command line, the processor use between two samples puts the
    /// busy one far above the sleeper, memory and uptime agree with `free` and `ps`, stopping the busy one with the tile's own signal
    /// call ends it, and it is gone from the next listing. Skipped where `free`, `ps` or `id` are missing.
    #[test]
    fn the_readers_agree_with_the_real_kernel_and_the_real_ps_and_free() {
        let out = |c: &str, a: &[&str]| std::process::Command::new(c).args(a).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let (Some(uid), Some(free), Some(_)) = (out("id", &["-u"]), out("free", &["-b"]), out("ps", &["-o", "pid=", "-p", "1"])) else {
            eprintln!("skipped: id, free or ps is missing");
            return;
        };
        let r = SysRoots::real();
        let mut sleeper = std::process::Command::new("sleep").arg("77").spawn().unwrap();
        let mut busy = std::process::Command::new("sh").args(["-c", "while :; do :; done"]).spawn().unwrap();
        let before = processes(&r);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let after = processes(&r);
        let me = std::process::id();
        let find = |v: &[Proc], pid: u32| v.iter().find(|p| p.pid == pid).cloned();
        let (s, b) = (find(&after, sleeper.id()).expect("the sleeper is listed"), find(&after, busy.id()).expect("the busy shell is listed"));
        assert_eq!((s.ppid, b.ppid), (me, me));
        assert_eq!(s.uid.to_string(), uid);
        assert!(s.cmd.ends_with("sleep 77"), "{:?}", s.cmd);
        assert!(!s.user.is_empty());
        // rss and name agree with ps
        let ps_rss: u64 = out("ps", &["-o", "rss=", "-p", &s.pid.to_string()]).unwrap().trim().parse().unwrap();
        assert!(s.rss / 1024 >= ps_rss / 2 && s.rss / 1024 <= ps_rss * 2 + 64, "rss {} vs ps {} KiB", s.rss / 1024, ps_rss);
        assert_eq!(out("ps", &["-o", "comm=", "-p", &b.pid.to_string()]).unwrap(), b.name);
        // processor use between two samples
        let cpu = proc_cpu(&before, &after, 1.5);
        assert!(cpu.get(&b.pid).copied().unwrap_or(0.0) > 30.0, "the busy shell used {:?}%", cpu.get(&b.pid));
        assert!(cpu.get(&s.pid).copied().unwrap_or(0.0) < 10.0, "the sleeper used {:?}%", cpu.get(&s.pid));
        // memory total, as `free` says it
        let total: u64 = free.lines().find(|l| l.starts_with("Mem:")).unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
        assert_eq!(memory(&r).total, total);
        // uptime within a few seconds of /proc/uptime read at once
        let up = uptime_secs(&r);
        assert!(uptime_now().abs_diff(up) <= 3);
        // stop it as the tile does
        signal(busy.id(), true).expect("kill");
        let status = busy.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(9));
        assert!(find(&processes(&r), busy.id()).is_none(), "gone from the next listing");
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }
}
