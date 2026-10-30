//! The real side of the boot-drive loss watcher (docs/STORAGE-LOSS.md): a thread that looks once a second, the sysfs / mount-table
//! environment, the `mount` / `umount2` effects of a reattach and the files the CLI talks through.
//!
//! The decisions are `iyakki_storage::loss`'s (tested there with fakes). The look itself spawns no process: it reads
//! `/proc/self/mountinfo` and `/sys`, and writes one byte to a hidden file on the data volume from a helper thread with a bounded wait
//! (a hung device must not hang the watcher; a probe still stuck from before is not started twice). Only the buttons "Check the
//! drive" and the CLI start a process (`e2fsck -n`, read-only, or `dmesg`).

use iyakki_storage::loss::{self, Actions, Candidate, DiskState, Env, Probe, State, Watcher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const PROBE_WAIT: Duration = Duration::from_millis(1500);
const SUPER_WAIT: Duration = Duration::from_millis(1500);

/// The machine, as the watcher looks at it. `root` is `/` (tests of the plain helpers use a made-up one).
pub struct SysEnv {
    pub sys: std::path::PathBuf,
    pub dev: std::path::PathBuf,
    pub mountinfo: std::path::PathBuf,
}

impl SysEnv {
    pub fn real() -> SysEnv {
        SysEnv { sys: "/sys".into(), dev: "/dev".into(), mountinfo: "/proc/self/mountinfo".into() }
    }
}

fn read_trim(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The first 1280 bytes of a node, read on a helper thread so that a hung device costs a timeout, not the watcher.
fn read_head(path: std::path::PathBuf, wait: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("iyakki-storage-head".into())
        .spawn(move || {
            let mut buf = vec![0u8; 1280];
            let r = std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut buf)).map(|_| buf);
            let _ = tx.send(r.ok());
        })
        .ok()?;
    rx.recv_timeout(wait).ok().flatten()
}

/// The FAT volume label of a boot sector (FAT32 at 0x47, FAT12/16 at 0x2B), trimmed.
pub fn fat_label(b: &[u8]) -> Option<String> {
    if b.len() < 0x52 || b[510] != 0x55 || b[511] != 0xAA {
        return None;
    }
    let off = if &b[0x52..0x5A] == b"FAT32   " { 0x47 } else { 0x2B };
    Some(String::from_utf8_lossy(&b[off..off + 11]).trim().to_string())
}

static PROBE_BUSY: AtomicBool = AtomicBool::new(false);
static PROBE_BYTE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// A stat and a 1-byte fsync'd write of the probe file in `dir`; bounded by `wait`.
pub fn probe_dir(dir: &Path, wait: Duration) -> Probe {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // a probe that never came back is still blocked in the kernel: do not pile up threads behind it
    if PROBE_BUSY.swap(true, Ordering::SeqCst) {
        return Probe::TimedOut;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let d = dir.to_path_buf();
    let spawned = std::thread::Builder::new().name("iyakki-storage-probe".into()).spawn(move || {
        let r = (|| -> Result<(), std::io::Error> {
            std::fs::metadata(&d)?;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(d.join(loss::PROBE_FILE))?;
            f.write_all(&[PROBE_BYTE.fetch_add(1, Ordering::Relaxed)])?;
            f.sync_data()
        })();
        PROBE_BUSY.store(false, Ordering::SeqCst);
        let _ = tx.send(r.map_err(|e| e.raw_os_error().unwrap_or(5)));
    });
    if spawned.is_err() {
        PROBE_BUSY.store(false, Ordering::SeqCst);
        return Probe::Failed(11);
    }
    match rx.recv_timeout(wait) {
        Ok(Ok(())) => Probe::Ok,
        Ok(Err(errno)) => Probe::Failed(errno),
        Err(_) => Probe::TimedOut,
    }
}

impl Env for SysEnv {
    fn mountinfo(&self) -> String {
        std::fs::read_to_string(&self.mountinfo).unwrap_or_default()
    }

    fn disk_state(&self, disk: &str) -> DiskState {
        let d = self.sys.join("block").join(disk);
        if !d.exists() {
            return DiskState::Gone;
        }
        // SCSI and USB disks say `running`; an NVMe controller says `live` (a Mac's internal disk: this was read as offline and the
        // drive was declared lost one second after the boot). `resetting`, `connecting` and `deleting` are the NVMe states that are not fine.
        match read_trim(&d.join("device/state")).as_str() {
            "" | "running" | "live" => DiskState::Running,
            _ => DiskState::Offline,
        }
    }

    fn ext4_errors(&self, dev: &str) -> u64 {
        read_trim(&self.sys.join("fs/ext4").join(dev).join("errors_count")).parse().unwrap_or(0)
    }

    fn probe(&self) -> Probe {
        probe_dir(Path::new(loss::DATA_DIR), PROBE_WAIT)
    }

    fn candidates(&self) -> Vec<Candidate> {
        let table = loss::parse_mountinfo(&self.mountinfo());
        let mut out = Vec::new();
        let mut disks: Vec<String> = std::fs::read_dir(self.sys.join("block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        disks.sort();
        for d in disks.iter().filter(|d| !["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| d.starts_with(p))) {
            let dir = self.sys.join("block").join(d);
            let mut nodes: Vec<String> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.path().join("partition").exists()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            nodes.sort();
            if nodes.is_empty() {
                nodes.push(d.clone());
            }
            for n in nodes {
                let sys_node = if &n == d { dir.clone() } else { dir.join(&n) };
                if read_trim(&sys_node.join("size")).parse::<u64>().unwrap_or(0) == 0 {
                    continue;
                }
                let Some(head) = read_head(self.dev.join(&n), SUPER_WAIT) else { continue };
                let Some((uuid, label)) = loss::parse_super(&head) else { continue };
                let majmin = read_trim(&sys_node.join("dev"));
                let mounted = table.iter().any(|m| m.source == format!("/dev/{n}") || (!majmin.is_empty() && m.majmin == majmin));
                out.push(Candidate { name: n, majmin, uuid, label, mounted });
            }
        }
        out
    }

    fn identity(&self, dev: &str) -> Option<(String, String)> {
        loss::parse_super(&read_head(self.dev.join(dev), SUPER_WAIT)?)
    }

    fn majmin(&self, dev: &str) -> String {
        read_trim(&self.sys.join("class/block").join(dev).join("dev"))
    }

    fn uptime_secs(&self) -> u64 {
        std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok())).map_or(0, |v| v as u64)
    }
}

// ------------------------------------------------------------------------------------------------ effects

/// `mount(2)` / `umount2(2)` on the real system. Only the watcher thread calls these, and only along `Watcher::reattach`.
pub struct SysActions;

#[cfg(target_os = "linux")]
fn c(s: &str) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(s).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn sys_mount(src: &str, target: &str, fstype: Option<&str>, flags: libc::c_ulong, data: &str) -> Result<(), String> {
    let (s, t, d) = (c(src)?, c(target)?, c(data)?);
    let f = fstype.map(c).transpose()?;
    // SAFETY: every pointer is a valid NUL-terminated string that lives through the call; a null fstype is allowed for bind and move.
    let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), f.as_ref().map_or(std::ptr::null(), |x| x.as_ptr()), flags, d.as_ptr().cast()) };
    if rc == 0 { Ok(()) } else { Err(format!("mount {src} -> {target}: {}", std::io::Error::last_os_error())) }
}

#[cfg(target_os = "linux")]
impl Actions for SysActions {
    fn mount(&self, dev: &str, target: &str) -> Result<(), String> {
        let _ = std::fs::create_dir_all(target);
        // the options of the boot mount: rw,nosuid,nodev,relatime; ext4's journal is replayed by this mount, commit= is its default 5 s
        sys_mount(dev, target, Some("ext4"), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_RELATIME, "")
    }

    fn lazy_unmount(&self, target: &str) -> Result<(), String> {
        let t = c(target)?;
        // SAFETY: a valid NUL-terminated path; MNT_DETACH only detaches the mount from the tree.
        let rc = unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
        if rc == 0 { Ok(()) } else { Err(format!("umount {target}: {}", std::io::Error::last_os_error())) }
    }

    fn move_mount(&self, from: &str, to: &str) -> Result<(), String> {
        sys_mount(from, to, None, libc::MS_MOVE, "")
    }

    fn bind(&self, src: &str, target: &str) -> Result<(), String> {
        let _ = std::fs::create_dir_all(src);
        sys_mount(src, target, None, libc::MS_BIND, "")
    }

    fn make_dirs(&self) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(format!("{}/home", loss::DATA_DIR)).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(loss::DATA_DIR, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::create_dir_all("/run/iyakki");
        Ok(())
    }

    fn stop_components(&self) -> Vec<String> {
        loss::stop_registered()
    }

    fn start_components(&self, stopped: &[String]) {
        loss::start_registered(stopped);
    }
}

#[cfg(not(target_os = "linux"))]
impl Actions for SysActions {
    fn mount(&self, _: &str, _: &str) -> Result<(), String> {
        Err("only the Linux system mounts disks".into())
    }
    fn lazy_unmount(&self, _: &str) -> Result<(), String> {
        Err("only the Linux system mounts disks".into())
    }
    fn move_mount(&self, _: &str, _: &str) -> Result<(), String> {
        Err("only the Linux system mounts disks".into())
    }
    fn bind(&self, _: &str, _: &str) -> Result<(), String> {
        Err("only the Linux system mounts disks".into())
    }
    fn make_dirs(&self) -> Result<(), String> {
        Ok(())
    }
    fn stop_components(&self) -> Vec<String> {
        Vec::new()
    }
    fn start_components(&self, _: &[String]) {}
}

// ------------------------------------------------------------------------------------------------ compatible USB mode

/// Write (or remove) the GRUB file of the compatible USB mode under `boot_root` (the boot partition's top folder).
pub fn write_compat_file(boot_root: &Path, on: bool, usb_id: &str) -> Result<String, String> {
    let file = boot_root.join("boot/grub").join(loss::COMPAT_FILE);
    if on {
        let quirk = loss::compat_quirk(usb_id).ok_or("the drive's USB id is not known")?;
        if !boot_root.join("boot/grub").is_dir() {
            return Err("the boot partition has no boot/grub folder (not an IYAKKI boot partition)".into());
        }
        std::fs::write(&file, loss::compat_file_text(&quirk)).map_err(|e| format!("could not write {}: {e}", file.display()))?;
        Ok(format!("Compatible USB mode is on for the next start ({quirk}). It is slower but steadier. Switch it off here to go back."))
    } else {
        match std::fs::remove_file(&file) {
            Ok(()) => Ok("Compatible USB mode is off for the next start.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("Compatible USB mode was already off.".into()),
            Err(e) => Err(format!("could not remove {}: {e}", file.display())),
        }
    }
}

/// Find the IYAKKI-BOOT partition, mount it for a moment, write the file, sync and unmount. Settings -> Storage calls this.
pub fn write_compat_on_boot_partition(on: bool, usb_id: &str) -> Result<String, String> {
    let env = SysEnv::real();
    let table = loss::parse_mountinfo(&env.mountinfo());
    let mut node = None;
    let mut disks: Vec<String> = std::fs::read_dir(env.sys.join("block")).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    disks.sort();
    for d in disks.iter().filter(|d| !["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| d.starts_with(p))) {
        let dir = env.sys.join("block").join(d);
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.path().join("partition").exists()) {
            let n = e.file_name().to_string_lossy().into_owned();
            if read_head(env.dev.join(&n), SUPER_WAIT).is_some_and(|h| fat_label(&h).as_deref() == Some("IYAKKI-BOOT")) {
                node = Some(n);
            }
        }
    }
    let node = node.ok_or("the IYAKKI-BOOT partition was not found (is this the installed system?)")?;
    if let Some(m) = table.iter().find(|m| m.source == format!("/dev/{node}") && m.fstype == "vfat") {
        return write_compat_file(Path::new(&m.point), on, usb_id);
    }
    #[cfg(target_os = "linux")]
    {
        let target = "/run/iyakki-boot";
        let _ = std::fs::create_dir_all(target);
        sys_mount(&format!("/dev/{node}"), target, Some("vfat"), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, "")?;
        let r = write_compat_file(Path::new(target), on, usb_id);
        // SAFETY: sync takes no pointers.
        unsafe { libc::sync() };
        let t = c(target)?;
        // SAFETY: a valid NUL-terminated path.
        let rc = unsafe { libc::umount2(t.as_ptr(), 0) };
        if rc != 0 {
            let _ = unsafe { libc::umount2(t.as_ptr(), libc::MNT_DETACH) };
        }
        r
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err("only the Linux system mounts disks".into())
    }
}

// ------------------------------------------------------------------------------------------------ the status file and the CLI

/// The text of the RAM status file.
pub fn status_text(w: &Watcher, handled: Option<(&str, &str)>) -> String {
    let b = w.baseline.as_ref();
    let mut s = format!("state={}\nsince={}\ndev={}\nuuid={}\nlabel={}\nreattached={}\nnote={}\n", w.state.word(), w.since, b.map_or("", |b| b.dev.as_str()), b.map_or("", |b| b.uuid.as_str()), b.map_or("", |b| b.label.as_str()), w.reattached, w.note.replace('\n', " "));
    if let State::Failed(why) = &w.state {
        s.push_str(&format!("reason={}\n", why.replace('\n', " ")));
    }
    if let Some((id, result)) = handled {
        s.push_str(&format!("handled={id}\nresult={}\n", result.replace('\n', " ")));
    }
    s
}

/// One value of the status file text.
pub fn status_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn write_status(text: &str) {
    let _ = std::fs::create_dir_all("/run/iyakki");
    let tmp = format!("{}.tmp", loss::STATUS_FILE);
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, loss::STATUS_FILE);
    }
}

/// `iyakki storage status` (any process): the watcher's state.
pub fn cli_status() -> String {
    match std::fs::read_to_string(loss::STATUS_FILE) {
        Ok(t) => {
            let v = |k: &str| status_value(&t, k).unwrap_or("").to_string();
            let mut out = format!("drive: {} (since uptime {} s), device {}, UUID {}\n", v("state"), v("since"), v("dev"), v("uuid"));
            if !v("reason").is_empty() {
                out.push_str(&format!("reason: {}\n", v("reason")));
            }
            if !v("note").is_empty() {
                out.push_str(&format!("note: {}\n", v("note")));
            }
            out
        }
        Err(_) => "drive: not watched (the data is in RAM, or the watcher is not running)\n".into(),
    }
}

/// `iyakki storage reattach` (any process): ask the watcher and wait for its answer.
pub fn cli_reattach(wait: Duration) -> Result<String, String> {
    if !Path::new(loss::STATUS_FILE).exists() {
        return Err("the drive watcher is not running (data is kept in RAM, or this is not the IYAKKI system)".into());
    }
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_string();
    std::fs::write(loss::REQUEST_FILE, &id).map_err(|e| format!("could not ask the watcher: {e}"))?;
    let start = std::time::Instant::now();
    while start.elapsed() < wait {
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(t) = std::fs::read_to_string(loss::STATUS_FILE) {
            if status_value(&t, "handled") == Some(id.as_str()) {
                let result = status_value(&t, "result").unwrap_or("").to_string();
                return if status_value(&t, "state") == Some("Healthy") { Ok(result) } else { Err(result) };
            }
        }
    }
    let _ = std::fs::remove_file(loss::REQUEST_FILE);
    Err("the watcher did not answer in time".into())
}

// ------------------------------------------------------------------------------------------------ the watcher thread

/// Called after the data volume is mounted (and `/home` bound). Does nothing when the data lives in RAM.
pub fn start() {
    if let Err(e) = std::thread::Builder::new().name("iyakki-storage-watch".into()).spawn(run) {
        log::warn!("storage: could not start the drive watcher ({e})");
    }
}

fn run() {
    let env = SysEnv::real();
    // Settings -> Storage -> Compatible USB mode: writes a GRUB file on the boot partition (takes effect at the next start)
    loss::register_compat_writer(write_compat_on_boot_partition);
    loss::set_compat_active(loss::compat_active(&std::fs::read_to_string("/proc/cmdline").unwrap_or_default()));
    let mut w = Watcher::new();
    if !w.learn(&env) {
        log::info!("storage: no ext4 data volume is mounted, nothing to watch");
        return;
    }
    let base = w.baseline.clone().unwrap_or_else(|| unreachable_baseline());
    let link = keep_awake_and_link(&env, &base.dev);
    log::info!("{}", loss::link_line(link.as_ref()));
    if link.as_ref().is_some_and(|l| l.slow() || l.behind_hub) {
        log::warn!("storage: {}", loss::link_advice(link.as_ref()));
    }
    let mut link = link;
    loss::publish(&w, None, link.clone());
    write_status(&status_text(&w, None));
    let clock = std::time::Instant::now();
    let mut announced = false;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let now = clock.elapsed().as_millis() as u64;
        let before = w.state.clone();
        w.tick(&env, now);
        if before.healthy() && !w.state.healthy() && !announced {
            announced = true;
            // the port path of the drive (read before it vanished) helps tell a cable from a hub
            let line = loss::link_line(link.as_ref());
            w.ring.push(w.since, line.clone());
            log::error!("storage: drive lost at uptime {} s ({})", w.since, w.state.word());
            log::warn!("{line}");
        }
        let mut handled: Option<(String, String)> = None;
        let mut check: Option<String> = None;
        let manual_file = std::fs::read_to_string(loss::REQUEST_FILE).ok();
        let manual = loss::take_reattach_request() || manual_file.is_some();
        if manual_file.is_some() {
            let _ = std::fs::remove_file(loss::REQUEST_FILE);
        }
        if (manual && !w.state.healthy()) || w.auto_due(now) {
            let r = w.reattach(&env, &SysActions, now, manual);
            match &r {
                Ok(()) => log::info!("storage: drive reconnected"),
                Err(e) => log::warn!("storage: reconnect: {e}"),
            }
            if r.is_ok() {
                let dev = w.baseline.as_ref().map(|b| b.dev.clone()).unwrap_or_default();
                link = keep_awake_and_link(&env, &dev).or(link);
                announced = false;
            }
            if let Some(id) = manual_file {
                handled = Some((id.trim().to_string(), r.map_or_else(|e| e, |_| w.note.clone())));
            }
        } else if let Some(id) = manual_file {
            handled = Some((id.trim().to_string(), if w.state.healthy() { "The drive is healthy; nothing to reconnect.".into() } else { w.note.clone() }));
        }
        if loss::take_check_request() {
            check = Some(check_drive(&w));
        }
        loss::publish(&w, check, link.clone());
        write_status(&status_text(&w, handled.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))));
        if w.state.healthy() {
            announced = false;
        }
    }
}

fn unreachable_baseline() -> iyakki_storage::loss::Baseline {
    iyakki_storage::loss::Baseline { dev: String::new(), majmin: String::new(), uuid: String::new(), label: String::new(), errors: 0, home_bound: false }
}

/// Runtime PM "on" for every USB disk that backs IYAKKI-DATA or IYAKKI-BOOT (autosuspend of the boot drive is never wanted), and
/// the link facts of the data disk.
fn keep_awake_and_link(env: &SysEnv, data_dev: &str) -> Option<loss::UsbLink> {
    let data_disk = loss::disk_of(data_dev);
    let mut disks = vec![data_disk.clone()];
    if let Ok(rd) = std::fs::read_dir(env.sys.join("block")) {
        for e in rd.flatten() {
            let d = e.file_name().to_string_lossy().into_owned();
            if d == data_disk || ["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| d.starts_with(p)) {
                continue;
            }
            // the FAT boot partition of IYAKKI (label IYAKKI-BOOT), on any disk
            let parts: Vec<String> = std::fs::read_dir(e.path()).into_iter().flatten().flatten().filter(|p| p.path().join("partition").exists()).map(|p| p.file_name().to_string_lossy().into_owned()).collect();
            if parts.iter().any(|p| read_head(env.dev.join(p), SUPER_WAIT).is_some_and(|h| fat_label(&h).as_deref() == Some("IYAKKI-BOOT"))) {
                disks.push(d);
            }
        }
    }
    for d in &disks {
        for w in loss::keep_usb_awake(&env.sys, d) {
            log::info!("storage: runtime power management of {w} set to on");
        }
    }
    loss::usb_link(&env.sys, &data_disk)
}

// ------------------------------------------------------------------------------------------------ on request (processes may start here)

/// "Check the drive": `e2fsck -n` (read-only, never repairs) on the returned node when the tool exists, else the kernel log's tail.
fn check_drive(w: &Watcher) -> String {
    let env = SysEnv::real();
    let node = w.baseline.as_ref().and_then(|b| env.candidates().into_iter().find(|c| c.uuid == b.uuid && !c.mounted).map(|c| c.name));
    let tool = ["/sbin/e2fsck", "/usr/sbin/e2fsck"].iter().find(|p| Path::new(p).exists());
    let text = match (node, tool) {
        (Some(n), Some(t)) => run_bounded(t, &["-n", &format!("/dev/{n}")], Duration::from_secs(60)).map_or_else(|e| format!("The check could not run: {e}"), |o| format!("Check of {n} (read only, nothing was changed):\n{}", last_lines(&o, 6))),
        _ => run_bounded("dmesg", &[], Duration::from_secs(5)).map_or_else(|e| format!("The kernel log could not be read: {e}"), |o| format!("The check tool is not installed. The end of the kernel log:\n{}", last_lines(&o, 8))),
    };
    text
}

fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Wait for the child to end, at most `limit` (then it is killed). A status taken by the PID-1 reaper (`Gone`) is an end like
/// any other: the output is still in the pipes.
fn wait_ended(child: &mut std::process::Child, limit: Duration) -> Result<(), String> {
    let start = std::time::Instant::now();
    loop {
        match iyakki_core_api::child_status(child) {
            s if s.is_over() => return Ok(()),
            _ if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("it took too long and was stopped".into());
            }
        }
    }
}

/// Run a program, wait at most `limit`, return its output.
fn run_bounded(prog: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    use std::io::Read;
    let mut child = std::process::Command::new(prog).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    wait_ended(&mut child, limit)?;
    let mut out = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut out);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut out);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_whose_status_was_taken_by_the_reaper_is_seen_as_ended_and_its_output_is_kept() {
        let mut c = std::process::Command::new("echo").arg("kept").stdout(std::process::Stdio::piped()).spawn().unwrap();
        iyakki_core_api::steal_status(c.id());
        assert!(c.try_wait().is_err(), "std alone reports ECHILD here");
        assert!(wait_ended(&mut c, Duration::from_secs(5)).is_ok());
        let mut o = String::new();
        std::io::Read::read_to_string(&mut c.stdout.take().unwrap(), &mut o).unwrap();
        assert_eq!(o.trim(), "kept");
        let mut slow = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        assert!(wait_ended(&mut slow, Duration::from_millis(150)).is_err());
    }

    #[test]
    fn the_watcher_part_of_this_file_spawns_no_process() {
        let src = include_str!("storageloss.rs");
        let watcher = src.split("// ---- on request").next().unwrap_or("");
        let watcher = watcher.split("on request (processes may start here)").next().unwrap_or(watcher);
        let needle = ["Command", "::new"].concat();
        assert!(!watcher.contains(&needle), "the look and the thread spawn no process");
    }

    #[test]
    fn a_probe_on_a_folder_writes_one_byte_and_reports_errors() {
        let d = std::env::temp_dir().join(format!("iyakki-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(probe_dir(&d, Duration::from_secs(2)), Probe::Ok);
        assert_eq!(std::fs::metadata(d.join(loss::PROBE_FILE)).unwrap().len(), 1);
        assert_eq!(probe_dir(&d, Duration::from_secs(2)), Probe::Ok);
        assert_eq!(std::fs::metadata(d.join(loss::PROBE_FILE)).unwrap().len(), 1);
        std::fs::remove_dir_all(&d).unwrap();
        assert!(matches!(probe_dir(&d, Duration::from_secs(2)), Probe::Failed(2)));
    }

    #[test]
    fn a_probe_still_stuck_is_not_started_twice_and_does_not_hang_the_caller() {
        PROBE_BUSY.store(true, Ordering::SeqCst);
        let t = std::time::Instant::now();
        assert_eq!(probe_dir(Path::new("/nonexistent"), Duration::from_secs(5)), Probe::TimedOut);
        assert!(t.elapsed() < Duration::from_secs(1));
        PROBE_BUSY.store(false, Ordering::SeqCst);
    }

    #[test]
    fn a_fifo_that_never_answers_times_out_in_the_head_reader() {
        // a named pipe with no writer blocks open() forever, like a hung device node
        let p = std::env::temp_dir().join(format!("iyakki-fifo-{}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let c = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
        // SAFETY: a valid NUL-terminated path in a temp folder.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let t = std::time::Instant::now();
        assert!(read_head(p.clone(), Duration::from_millis(300)).is_none());
        assert!(t.elapsed() < Duration::from_secs(2));
        // release the stuck helper thread so the test process ends cleanly
        let _ = std::fs::OpenOptions::new().write(true).open(&p);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_compat_file_is_written_and_removed_on_a_made_up_boot_partition_and_only_with_a_valid_id() {
        let root = std::env::temp_dir().join(format!("iyakki-compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("boot/grub")).unwrap();
        let msg = write_compat_file(&root, true, "04e8:4001").unwrap();
        assert!(msg.contains("usb-storage.quirks=04e8:4001:u") && msg.contains("next start"));
        let t = std::fs::read_to_string(root.join("boot/grub/iyakki-usb-compat.cfg")).unwrap();
        assert!(t.contains("set iyakki_usbq=\"usb-storage.quirks=04e8:4001:u\""));
        assert!(write_compat_file(&root, true, "04e8:4001\"; reboot; \"").is_err(), "nothing but 4+4 hex digits reaches the boot menu");
        assert!(write_compat_file(&root, false, "").unwrap().contains("off"));
        assert!(!root.join("boot/grub/iyakki-usb-compat.cfg").exists());
        assert!(write_compat_file(&root, false, "").unwrap().contains("already off"));
        let empty = root.join("not-a-boot-partition");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(write_compat_file(&empty, true, "04e8:4001").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_status_file_round_trips_for_the_cli() {
        let mut w = Watcher::new();
        w.state = State::Failed("boom\nbad".into());
        let t = status_text(&w, Some(("42", "no luck")));
        assert_eq!(status_value(&t, "state"), Some("Failed"));
        assert_eq!(status_value(&t, "reason"), Some("boom bad"));
        assert_eq!((status_value(&t, "handled"), status_value(&t, "result")), (Some("42"), Some("no luck")));
    }

    #[test]
    fn an_nvme_controller_that_is_live_is_a_running_disk_and_one_that_is_resetting_is_not() {
        let root = std::env::temp_dir().join(format!("iyakki-nvmestate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/block/nvme0n1/device")).unwrap();
        std::fs::create_dir_all(root.join("sys/block/sdb/device")).unwrap();
        let env = SysEnv { sys: root.join("sys"), dev: root.join("dev"), mountinfo: root.join("mountinfo") };
        std::fs::write(root.join("sys/block/nvme0n1/device/state"), "live\n").unwrap();
        assert_eq!(env.disk_state("nvme0n1"), DiskState::Running);
        std::fs::write(root.join("sys/block/nvme0n1/device/state"), "resetting\n").unwrap();
        assert_eq!(env.disk_state("nvme0n1"), DiskState::Offline);
        std::fs::write(root.join("sys/block/sdb/device/state"), "running\n").unwrap();
        assert_eq!(env.disk_state("sdb"), DiskState::Running);
        std::fs::write(root.join("sys/block/sdb/device/state"), "offline\n").unwrap();
        assert_eq!(env.disk_state("sdb"), DiskState::Offline);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_fat_label_of_a_boot_sector_is_read() {
        let mut b = vec![0u8; 512];
        b[510] = 0x55;
        b[511] = 0xAA;
        b[0x52..0x5A].copy_from_slice(b"FAT32   ");
        b[0x47..0x47 + 11].copy_from_slice(b"IYAKKI-BOOT");
        assert_eq!(fat_label(&b).as_deref(), Some("IYAKKI-BOOT"));
        assert_eq!(fat_label(&[0u8; 512]), None);
    }

    #[test]
    fn the_real_env_reads_a_made_up_sys_and_mount_table_without_touching_devices() {
        let root = std::env::temp_dir().join(format!("iyakki-sysenv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/block/sdb/sdb3")).unwrap();
        std::fs::create_dir_all(root.join("dev")).unwrap();
        std::fs::write(root.join("sys/block/sdb/sdb3/partition"), "3\n").unwrap();
        std::fs::write(root.join("sys/block/sdb/sdb3/size"), "100\n").unwrap();
        std::fs::write(root.join("sys/block/sdb/sdb3/dev"), "8:19\n").unwrap();
        // a regular file stands in for the partition node
        let mut b = vec![0u8; 2048];
        b[1024 + 0x38] = 0x53;
        b[1024 + 0x39] = 0xEF;
        b[1024 + 0x68] = 7;
        b[1024 + 0x78..1024 + 0x78 + 11].copy_from_slice(b"IYAKKI-DATA");
        std::fs::write(root.join("dev/sdb3"), &b).unwrap();
        std::fs::write(root.join("mountinfo"), "36 25 8:3 / /var/lib/iyakki rw - ext4 /dev/sda3 rw\n").unwrap();
        let env = SysEnv { sys: root.join("sys"), dev: root.join("dev"), mountinfo: root.join("mountinfo") };
        let c = env.candidates();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].name.as_str(), c[0].label.as_str(), c[0].majmin.as_str(), c[0].mounted), ("sdb3", "IYAKKI-DATA", "8:19", false));
        assert_eq!(env.disk_state("sdb"), DiskState::Running);
        assert_eq!(env.disk_state("sda"), DiskState::Gone);
        let _ = std::fs::remove_dir_all(&root);
    }
}
