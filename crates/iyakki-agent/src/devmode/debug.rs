//! `debug` scope: read-only looking at the device (`iyakki dev info|ps|dmesg|logs --system|status`). Everything is read from `/proc`,
//! `/sys`, `/dev/kmsg` and a few files, through [`Paths`] so tests can supply a made-up machine. Nothing here writes; lines that look
//! like a credential are hidden; every answer is bounded.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::relinfo::{ReleaseInfo, VERSION};
use crate::sysinfo::{self, SysRoots};

/// Most kernel log records one answer holds, and the most bytes of any text answer.
pub const MAX_RECORDS: usize = 4000;
pub const MAX_TEXT: usize = 256 * 1024;
pub const MAX_PROCS: usize = 3000;

/// Where the files are; [`Paths::real`] is this machine.
#[derive(Clone)]
pub struct Paths {
    pub sys: SysRoots,
    pub kmsg: PathBuf,
    pub boot_log: PathBuf,
    pub release: PathBuf,
    pub addrs: Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
}

impl Paths {
    pub fn real() -> Paths {
        Paths { sys: SysRoots::real(), kmsg: "/dev/kmsg".into(), boot_log: "/var/log/iyakki-boot.log".into(), release: crate::relinfo::RELEASE_FILE.into(), addrs: Arc::new(iyakki_devctl::ipv4_addresses) }
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn one(p: &Path) -> String {
    read(p).trim().to_string()
}

/// Hide lines that look like a credential.
pub fn scrub(line: &str) -> String {
    if crate::credguard::detect(line).is_some() { "[line hidden: it looks like a credential]".into() } else { line.to_string() }
}

// -------------------------------------------------------------------------------------------- kernel log

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kmsg {
    pub seq: u64,
    /// 0 emergency .. 7 debug.
    pub level: u8,
    /// Microseconds since boot.
    pub us: u64,
    pub msg: String,
}

/// Parse one `/dev/kmsg` record: `6,339,5140900,-;text` (continuation lines start with a space and are dropped).
pub fn parse_kmsg(rec: &str) -> Option<Kmsg> {
    let first = rec.lines().next()?;
    let (head, msg) = first.split_once(';')?;
    let mut f = head.split(',');
    let pri: u32 = f.next()?.parse().ok()?;
    let seq: u64 = f.next()?.parse().ok()?;
    let us: u64 = f.next()?.parse().ok()?;
    Some(Kmsg { seq, level: (pri & 7) as u8, us, msg: msg.to_string() })
}

/// The kernel log records after `since` (0: from the start), with `grep` (a case-insensitive substring) applied. When there are
/// more than `max`, the NEWEST are kept.
pub fn kmsg(p: &Paths, since: u64, grep: &str, max: usize) -> Vec<Kmsg> {
    use std::io::Read;
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    let max = max.clamp(1, MAX_RECORDS);
    let mut raw: Vec<String> = Vec::new();
    if let Ok(meta) = std::fs::metadata(&p.kmsg) {
        if meta.file_type().is_char_device() {
            if let Ok(mut f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(&p.kmsg) {
                let mut buf = vec![0u8; 16 * 1024];
                // one record per read; EAGAIN when caught up; EPIPE when records were overwritten under us (carry on)
                for _ in 0..200_000 {
                    match f.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => raw.push(String::from_utf8_lossy(&buf[..n]).into_owned()),
                        Err(e) if e.raw_os_error() == Some(libc::EPIPE) => continue,
                        Err(_) => break,
                    }
                }
            }
        } else {
            // a file in the same format (tests, saved logs): continuation lines start with a space
            for l in read(&p.kmsg).lines() {
                if l.starts_with(' ') {
                    continue;
                }
                raw.push(l.to_string());
            }
        }
    }
    let needle = grep.to_lowercase();
    let mut v: Vec<Kmsg> = raw.iter().filter_map(|r| parse_kmsg(r)).filter(|k| k.seq > since).filter(|k| needle.is_empty() || k.msg.to_lowercase().contains(&needle)).collect();
    if v.len() > max {
        v.drain(..v.len() - max);
    }
    for k in &mut v {
        k.msg = scrub(&k.msg);
    }
    v
}

pub fn kmsg_json(p: &Paths, since: u64, grep: &str, max: usize) -> Value {
    let v = kmsg(p, since, grep.chars().take(100).collect::<String>().as_str(), max);
    let next = v.last().map_or(since, |k| k.seq);
    json!({ "records": v.iter().map(|k| json!({ "seq": k.seq, "level": k.level, "us": k.us, "msg": k.msg })).collect::<Vec<_>>(), "next": next })
}

// -------------------------------------------------------------------------------------------- system log

/// The last `n` lines of the shell/agent lines (kernel log records tagged `[iyakki `), the boot log, or the audit tail.
pub fn syslog(p: &Paths, src: &str, n: usize, audit: &dyn Fn(usize) -> Vec<String>) -> Result<Value, String> {
    let n = n.clamp(1, 1000);
    let lines: Vec<String> = match src {
        "shell" | "agent" => {
            let recs = kmsg(p, 0, "[iyakki ", MAX_RECORDS);
            let start = recs.len().saturating_sub(n);
            recs[start..].iter().map(|k| format!("[{:>9.3}] {}", k.us as f64 / 1e6, k.msg)).collect()
        }
        "boot" => {
            let t = read(&p.boot_log);
            let l: Vec<&str> = t.lines().collect();
            l[l.len().saturating_sub(n)..].iter().map(|s| scrub(s)).collect()
        }
        "audit" => audit(n).into_iter().map(|s| scrub(&s)).collect(),
        // the web engine's own messages, kept by `enginelog.rs` (page addresses and content were never written)
        "engine" => engine_lines(&crate::enginelog::dir(), n),
        other => return Err(format!("unknown log {other:?}: use shell, boot, audit or engine")),
    };
    let mut text = lines.join("\n");
    if text.len() > MAX_TEXT {
        let cut = text.len() - MAX_TEXT;
        let from = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(0);
        text = text[from..].to_string();
    }
    Ok(json!({ "source": src, "lines": lines.len(), "text": text }))
}

/// The last `n` kept lines of the web engine, `[time] text`, credentials hidden once more.
pub fn engine_lines(dir: &Path, n: usize) -> Vec<String> {
    let all = crate::enginelog::read_in(dir);
    all[all.len().saturating_sub(n)..].iter().map(|(t, s)| scrub(&format!("[{t}] {s}"))).collect()
}

// -------------------------------------------------------------------------------------------- info

const TAINT: [(u32, &str); 19] = [
    (0, "proprietary module loaded"),
    (1, "module force-loaded"),
    (2, "SMP with a non-SMP part"),
    (3, "module force-unloaded"),
    (4, "machine check exception"),
    (5, "bad page"),
    (6, "user requested taint"),
    (7, "kernel died (oops)"),
    (8, "ACPI table overridden"),
    (9, "kernel warning"),
    (10, "staging driver loaded"),
    (11, "firmware workaround"),
    (12, "out-of-tree module loaded"),
    (13, "unsigned module loaded"),
    (14, "soft lockup"),
    (15, "kernel live-patched"),
    (16, "auxiliary taint"),
    (17, "struct randomization"),
    (18, "in-kernel test run"),
];

/// The names of the taint flags set in `/proc/sys/kernel/tainted`.
pub fn taint_flags(v: u64) -> Vec<&'static str> {
    TAINT.iter().filter(|(b, _)| v & (1u64 << b) != 0).map(|(_, n)| *n).collect()
}

fn dmi(p: &Paths, f: &str) -> String {
    one(&p.sys.sys.join("class/dmi/id").join(f))
}

fn disks(p: &Paths) -> Vec<Value> {
    let mut v = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(p.sys.sys.join("block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for n in names.into_iter().filter(|n| !n.starts_with("loop") && !n.starts_with("ram") && !n.starts_with("zram")) {
        let d = p.sys.sys.join("block").join(&n);
        let sectors: u64 = one(&d.join("size")).parse().unwrap_or(0);
        v.push(json!({ "name": n, "bytes": sectors * 512, "model": one(&d.join("device/model")), "removable": one(&d.join("removable")) == "1" }));
    }
    v
}

fn mounts(p: &Paths) -> Vec<Value> {
    const REAL: [&str; 10] = ["ext4", "ext2", "ext3", "vfat", "btrfs", "xfs", "f2fs", "exfat", "ntfs3", "iso9660"];
    read(&p.sys.proc.join("mounts"))
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 4 && REAL.contains(&f[2])).then(|| {
                let (total, free) = statvfs(Path::new(f[1]));
                json!({ "dev": f[0], "mount": f[1], "fs": f[2], "ro": f[3].split(',').any(|o| o == "ro"), "total": total, "free": free })
            })
        })
        .collect()
}

pub(super) fn statvfs(p: &Path) -> (u64, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else { return (0, 0) };
    // SAFETY: `c` is a valid NUL-terminated path and `s` is a properly sized, zeroed out-parameter.
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) == 0 { (s.f_blocks as u64 * s.f_frsize as u64, s.f_bavail as u64 * s.f_frsize as u64) } else { (0, 0) }
    }
}

fn gpus(p: &Paths) -> Vec<Value> {
    let mut v = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(p.sys.sys.join("class/drm")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("card") && !n.contains('-')).collect();
    names.sort();
    for n in names {
        let d = p.sys.sys.join("class/drm").join(&n).join("device");
        let driver = std::fs::read_link(d.join("driver")).ok().and_then(|l| l.file_name().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
        let outputs: Vec<String> = std::fs::read_dir(p.sys.sys.join("class/drm")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|c| c.starts_with(&format!("{n}-"))).filter(|c| one(&p.sys.sys.join("class/drm").join(c).join("status")) == "connected").collect();
        v.push(json!({ "card": n, "driver": driver, "vendor": one(&d.join("vendor")), "device": one(&d.join("device")), "connected": outputs }));
    }
    v
}

fn sound(p: &Paths) -> Vec<String> {
    read(&p.sys.proc.join("asound/cards")).lines().filter(|l| l.contains(" [")).map(|l| l.trim().to_string()).collect()
}

/// Firmware files the kernel could not load (named in `Direct firmware load for X failed` lines), without duplicates.
pub fn missing_firmware(recs: &[Kmsg]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for k in recs {
        if let Some(rest) = k.msg.split("Direct firmware load for ").nth(1) {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            if !name.is_empty() && !v.contains(&name) {
                v.push(name);
            }
        }
    }
    v.truncate(40);
    v
}

pub fn info(p: &Paths, hostname: &str) -> Value {
    let rel = ReleaseInfo::load(&p.release);
    let mem = sysinfo::memory(&p.sys);
    let tainted: u64 = one(&p.sys.proc.join("sys/kernel/tainted")).parse().unwrap_or(0);
    let modules = read(&p.sys.proc.join("modules")).lines().filter(|l| !l.trim().is_empty()).count();
    let recs = kmsg(p, 0, "firmware", 1500);
    let (l1, l5, l15) = sysinfo::load_average(&p.sys);
    json!({
        "hostname": hostname,
        "device": { "vendor": dmi(p, "sys_vendor"), "product": dmi(p, "product_name"), "version": dmi(p, "product_version"), "board": dmi(p, "board_name"), "bios": dmi(p, "bios_version") },
        "image": { "version": rel.as_ref().map(|r| r.version.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| VERSION.to_string()), "variant": rel.as_ref().map(|r| r.variant.clone()).unwrap_or_default(), "built": rel.as_ref().map(|r| r.build_date.clone()).unwrap_or_default(), "source": rel.as_ref().map(|r| r.git_describe.clone()).unwrap_or_default(), "from_image": rel.is_some() },
        "kernel": { "release": one(&p.sys.proc.join("sys/kernel/osrelease")), "version": read(&p.sys.proc.join("version")).lines().next().unwrap_or("").to_string(), "cmdline": scrub(&one(&p.sys.proc.join("cmdline"))), "tainted": tainted, "taint": taint_flags(tainted), "modules": modules },
        "uptime_secs": sysinfo::uptime_secs(&p.sys),
        "load": [l1, l5, l15],
        "cpus": sysinfo::cpu_count(&p.sys),
        "memory": { "total": mem.total, "available": mem.available, "swap_total": mem.swap_total, "swap_free": mem.swap_free },
        "firmware": { "uefi": p.sys.sys.join("firmware/efi").is_dir(), "missing_files": missing_firmware(&recs) },
        "disks": disks(p),
        "mounts": mounts(p),
        "network": (p.addrs)().into_iter().map(|(i, a)| json!({ "iface": i, "address": a })).collect::<Vec<_>>(),
        "gpu": gpus(p),
        "sound": sound(p),
    })
}

// -------------------------------------------------------------------------------------------- processes

/// The processes: everyone's for an administrator, only the account's own Unix user's otherwise. Command lines go through
/// [`scrub`] (a program started with a key on its command line shows up hidden).
pub fn ps(p: &Paths, all: bool, own_user: &str) -> Value {
    let mut v: Vec<Value> = sysinfo::processes(&p.sys)
        .into_iter()
        .filter(|x| all || x.user == own_user)
        .take(MAX_PROCS)
        .map(|x| json!({ "pid": x.pid, "ppid": x.ppid, "user": x.user, "state": x.state.to_string(), "threads": x.threads, "cpu_secs": x.ticks / sysinfo::TICKS_PER_SEC, "rss": x.rss, "name": x.name, "cmd": scrub(&x.cmd.chars().take(200).collect::<String>()) }))
        .collect();
    v.truncate(MAX_PROCS);
    json!({ "processes": v, "all": all })
}

// -------------------------------------------------------------------------------------------- service status

/// A plain table: name, state (`running`, `stopped`, `ready`, `missing`), detail. Probes are by process name and by files, so it
/// works without talking to anything.
pub fn status(p: &Paths, data_dir: &Path, firewall_file: &Path) -> Value {
    let procs = sysinfo::processes(&p.sys);
    let find = |names: &[&str]| procs.iter().find(|x| names.iter().any(|n| x.name == *n || x.cmd.split_whitespace().next().is_some_and(|c| c.rsplit('/').next() == Some(n))));
    let mut rows: Vec<Value> = Vec::new();
    let mut row = |name: &str, state: &str, detail: String| rows.push(json!({ "name": name, "state": state, "detail": detail }));
    match find(&["k3s", "k3s-server"]) {
        Some(x) => row("k3s (dev cluster)", "running", format!("pid {}", x.pid)),
        None => row("k3s (dev cluster)", if data_dir.join("k3s-dev").is_dir() { "stopped" } else { "missing" }, if data_dir.join("k3s-dev").is_dir() { "installed, not running".into() } else { "not installed".into() }),
    }
    match find(&["lemond", "lemonade", "lemonade-server"]) {
        Some(x) => row("Lemonade (local models)", "running", format!("pid {}", x.pid)),
        None => row("Lemonade (local models)", "stopped", "not running".into()),
    }
    match find(&["ollama"]) {
        Some(x) => row("Ollama", "running", format!("pid {}", x.pid)),
        None => row("Ollama", "stopped", "not running".into()),
    }
    let fw = read(firewall_file);
    row("Firewall", if fw.trim().is_empty() { "default" } else { "configured" }, if fw.trim().is_empty() { "no saved rules".into() } else { format!("{} bytes of settings", fw.len()) });
    match find(&["smbd", "iyakki-share"]) {
        Some(x) => row("Shared folders", "running", format!("pid {}", x.pid)),
        None => row("Shared folders", "stopped", "not sharing".into()),
    }
    let cards = sound(p);
    row("Sound", if cards.is_empty() { "missing" } else { "ready" }, if cards.is_empty() { "the kernel sees no sound card".into() } else { format!("{} card(s): {}", cards.len(), cards.join("; ")) });
    match find(&["pipewire", "pulseaudio", "wireplumber"]) {
        Some(x) => row("Sound server", "running", format!("{} pid {}", x.name, x.pid)),
        None => row("Sound server", "none", "IYAKKI plays through the kernel's sound devices directly".into()),
    }
    let g = gpus(p);
    row("Graphics", if g.is_empty() { "missing" } else { "ready" }, g.iter().map(|x| format!("{} ({})", x["card"].as_str().unwrap_or(""), x["driver"].as_str().unwrap_or("no driver"))).collect::<Vec<_>>().join(", "));
    let me = procs.iter().filter(|x| x.name == "iyakki" || x.name.starts_with("iyakki")).count();
    row("IYAKKI shell/agent", if me > 0 { "running" } else { "unknown" }, format!("{me} process(es)"));
    json!({ "services": rows, "startup": startup_rows() })
}

/// The startup registry ([`crate::startup`]) as rows: id, name, state word, the step text or the reason (credential-filtered).
pub fn startup_rows() -> Vec<Value> {
    crate::startup::global()
        .snapshot()
        .into_iter()
        .map(|r| {
            let detail = match &r.state {
                crate::startup::StartupState::Failed(why) => why.clone(),
                _ => r.step.clone(),
            };
            json!({ "id": r.id, "name": r.name, "state": r.state.word(), "detail": scrub(&detail), "seconds": r.age })
        })
        .collect()
}
